//! World oil market (b8-world stage 3, spec 2.4): one world price for the real Brent price, cleared every month in closed form
//! against thirteen demand regions (the US and the twelve blocs), three supply groups (OPEC+ with a price-band and spare-capacity
//! rule, US tight oil with a two-stage lag, other non-OPEC conventional with a slow lag) and a fast spare-capacity response, an
//! inventory sector that carries the precautionary shock, and the monthly-average adjustment w = 0.789. Port of
//! the research prototype `oil_market_model.py` (published as `scripts/world/oil_reference.py`): with one aggregate region, zero regional
//! real exchange rates and no activity process of its own, the recursion reproduces the prototype line by line (test
//! `oil_matches_python_prototype`).
//!
//! Integrated run (the only use inside the ABM): the activity of each region is the bloc output gap of the previous month (the
//! US: y_US), its real exchange rate is the bloc's real depreciation of the previous month, and the market price feeds the ABM
//! through `exog.oil_price` and the blocs through their oil and OPEC+ quantity channels. The market steps in shadow through the
//! burn-in (D36); no link acts before the reported run.
//!
//! Continuity (S2-R1 applied to the oil price; ruling S3-R1, spec 2.4 as revised in rev 5): the ABM log deviation is
//! `A_t = p_t - offset + G_t` with `G_t = kappa rho^(t+1)` for reported months `t >= 0`, `kappa = L_b - (p_b - offset)`, where
//! `L_b` is the legacy log deviation of the base month (the last burn-in month) and `p_b` the market's price there. `rho` is the
//! legacy persistence `oil_ar` unless `world_oil_splice_rho` overrides it. In a matched shocked and unshocked pair `kappa` and
//! `G` are identical, so `A_shock - A_base = p_shock - p_base` exactly.

use rand::Rng;
use serde::Deserialize;
use std::collections::BTreeMap;

use super::{Bloc, WorldOut, NB};
use crate::config::Config;
use crate::exog::poisson;
use crate::rng::{self, normal, stream};
use crate::shocks::Shock;
use crate::types::World;

/// Winsorisation of the unit-variance Student-t draws, in sd units (as `exog::oil_month`).
pub const WINSOR: f64 = 8.0;
/// The threshold below which the OilPrice log shock is treated as gone (exact-path copy U is dropped; spec 2.4).
pub const EXACT_DROP: f64 = 1e-4;

/// The research tables of the oil market (embedded in `world_params.json`, `oil` block; `build_world_params.py::build_oil`).
#[derive(Debug, Clone, Deserialize)]
pub struct OilRegion {
    pub name: String,
    pub omega: f64,
    pub beta: f64,
    pub phi: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OilSupplyTables {
    pub eps_s: f64,
    pub spare_cap: f64,
    pub theta_o: f64,
    pub pi_l: f64,
    pub pi_h: f64,
    pub kap_l: f64,
    pub kap_h: f64,
    pub tau_min: f64,
    pub tau_decay: f64,
    pub stock_kappa: f64,
    pub market_share_unwind: f64,
    pub sh_us: f64,
    pub eps_us_lr: f64,
    pub rho_us: f64,
    pub sh_rest: f64,
    pub eps_rest_lr: f64,
    pub rho_rest: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OilInventoryTables {
    pub psi: f64,
    pub rho_e: f64,
    pub xi: f64,
    pub xi_d: f64,
    pub lam_n: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OilShockTables {
    pub rho_i: f64,
    pub rho_q: f64,
    pub theta_q: f64,
    pub sig_i: f64,
    pub nu_i: f64,
    pub garch_a: f64,
    pub garch_b: f64,
    pub jump_lam_i: f64,
    pub jump_mu_i: f64,
    pub jump_sd_i: f64,
    pub rho_d: f64,
    pub jump_lam_d: f64,
    pub jump_mean_d: f64,
    pub sig_o: f64,
    pub nu_o: f64,
    pub sig_y: f64,
    pub rho_y: f64,
    pub news_cut_share: f64,
    pub news_phase: f64,
    pub rho_n: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OilOpecTables {
    pub liquids_mbd: f64,
    pub uae_liquids_mbd: f64,
    pub uae_spare_share: f64,
    pub uae_in_liquids_mbd: f64,
    pub uae_out_liquids_mbd: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OilTables {
    pub q_mbd: f64,
    pub brent_usd: f64,
    pub stochastic_mean_offset: f64,
    pub w: f64,
    pub eta0: f64,
    pub eta1: f64,
    pub rho_z: f64,
    pub rho_ylag: f64,
    pub beta_y: f64,
    pub phibar: f64,
    pub regions: Vec<OilRegion>,
    pub supply: OilSupplyTables,
    pub inventories: OilInventoryTables,
    pub shocks: OilShockTables,
    pub opec: OilOpecTables,
    /// Alternatives: profile name -> research keys and values (`oil.alternatives.<name>.changes`).
    pub profiles: BTreeMap<String, BTreeMap<String, f64>>,
}

/// The run-time parameters of the market: the tables, the profile's changes and the `world_*` levers, with the regional demand
/// coefficients derived (D12). `n` is 13 in the integrated run (US first, then the blocs in solution order) and 1 for the
/// isolated aggregate market of `oil-check` and the prototype tests.
#[derive(Debug, Clone)]
pub struct OilParams {
    pub n: usize,
    pub w: f64,
    pub omega: Vec<f64>,
    pub beta: Vec<f64>,
    pub eta0_i: Vec<f64>,
    pub eta1_i: Vec<f64>,
    pub eta0: f64,
    pub eta1: f64,
    pub rho_z: f64,
    pub rho_ylag: f64,
    pub eps_s: f64,
    pub spare_cap: f64,
    pub theta_o: f64,
    pub pi_l: f64,
    pub pi_h: f64,
    pub kap_l: f64,
    pub kap_h: f64,
    pub tau_min: f64,
    pub tau_decay: f64,
    pub stock_kappa: f64,
    /// Market-share regime: the floor rule is off and OPEC+ output unwinds at this rate, in % of Q per month.
    pub unwind: f64,
    pub sh_us: f64,
    pub eps_us_lr: f64,
    pub rho_u1: f64,
    pub rho_u2: f64,
    pub sh_rest: f64,
    pub eps_rest_lr: f64,
    pub rho_rest: f64,
    pub psi: f64,
    pub rho_e: f64,
    pub xi: f64,
    pub xi_d: f64,
    pub lam_n: f64,
    pub rho_i: f64,
    pub rho_q: f64,
    pub theta_q: f64,
    pub sig_i: f64,
    pub nu_i: f64,
    pub garch_a: f64,
    pub garch_b: f64,
    pub jump_lam_i: f64,
    pub jump_mu_i: f64,
    pub jump_sd_i: f64,
    pub rho_d: f64,
    pub jump_lam_d: f64,
    pub jump_mean_d: f64,
    pub sig_o: f64,
    pub nu_o: f64,
    pub sig_y: f64,
    pub rho_y: f64,
    pub news_cut_share: f64,
    pub news_phase: f64,
    pub rho_n: f64,
    /// OPEC+ liquids over Q (the divisor of the OPEC+ quantity channel: spec 2.2.2, 0.4626 with the UAE in).
    pub opec_q: f64,
    pub q_mbd: f64,
    /// Real Brent anchor in 2024 USD (`anchors.Brent_2024_usd`): the market's reported Brent level is `brent_usd exp(p/100)`.
    pub brent_usd: f64,
    /// 1 when `stochastic_oil` is on, else 0: the scale of the structural draws (the draws are always taken).
    pub shock_on: f64,
    /// Mean offset subtracted from the market log price (D6); zero when not stochastic.
    pub offset: f64,
    /// Stochastic mean of the OPEC+ quantity input, subtracted from the blocs' quantity channel when `stochastic_oil` is on
    /// (the same rule as `offset`; zero in deterministic runs). Stage-3 fix round, S3-R8.
    pub qty_offset: f64,
    pub market_share: bool,
}

/// The profile changes map research keys onto fields (names as in `oil/params.json`).
fn apply_change(p: &mut OilParams, key: &str, v: f64) {
    match key {
        "eta0" => p.eta0 = v,
        "psi" => p.psi = v,
        "jump_lam_D" => p.jump_lam_d = v,
        "jump_mean_D" => p.jump_mean_d = v,
        "sig_O" => p.sig_o = v,
        "sig_I" => p.sig_i = v,
        "sig_y" => p.sig_y = v,
        "jump_mu_I" => p.jump_mu_i = v,
        "jump_lam_I" => p.jump_lam_i = v,
        "eps_US_LR" => p.eps_us_lr = v,
        "rho_U1" => p.rho_u1 = v,
        "rho_U2" => p.rho_u2 = v,
        other => panic!("oil profile key {other} is not a market parameter"),
    }
}

impl OilParams {
    /// The integrated market from the tables, the profile and the config (`n` = 13), or the isolated aggregate market (`n` = 1)
    /// when `aggregate` is set. Regional eta0_i = eta0 phi_i / phibar and eta1_i likewise (D12); for the aggregate the single
    /// region carries eta0 and eta1 unchanged, which is the prototype.
    pub fn new(t: &OilTables, cfg: &Config, aggregate: bool) -> OilParams {
        let s = &t.supply;
        let sh = &t.shocks;
        let mut p = OilParams {
            n: if aggregate { 1 } else { t.regions.len() },
            w: t.w,
            omega: Vec::new(),
            beta: Vec::new(),
            eta0_i: Vec::new(),
            eta1_i: Vec::new(),
            eta0: t.eta0,
            eta1: t.eta1,
            rho_z: t.rho_z,
            rho_ylag: t.rho_ylag,
            eps_s: s.eps_s,
            spare_cap: s.spare_cap,
            theta_o: s.theta_o,
            pi_l: s.pi_l,
            pi_h: s.pi_h,
            kap_l: s.kap_l,
            kap_h: s.kap_h,
            tau_min: s.tau_min,
            tau_decay: s.tau_decay,
            stock_kappa: s.stock_kappa,
            unwind: 0.0,
            sh_us: s.sh_us,
            eps_us_lr: s.eps_us_lr,
            rho_u1: s.rho_us,
            rho_u2: s.rho_us,
            sh_rest: s.sh_rest,
            eps_rest_lr: s.eps_rest_lr,
            rho_rest: s.rho_rest,
            psi: t.inventories.psi,
            rho_e: t.inventories.rho_e,
            xi: t.inventories.xi,
            xi_d: t.inventories.xi_d,
            lam_n: t.inventories.lam_n,
            rho_i: sh.rho_i,
            rho_q: sh.rho_q,
            theta_q: sh.theta_q,
            sig_i: sh.sig_i,
            nu_i: sh.nu_i,
            garch_a: sh.garch_a,
            garch_b: sh.garch_b,
            jump_lam_i: sh.jump_lam_i,
            jump_mu_i: sh.jump_mu_i,
            jump_sd_i: sh.jump_sd_i,
            rho_d: sh.rho_d,
            jump_lam_d: sh.jump_lam_d,
            jump_mean_d: sh.jump_mean_d,
            sig_o: sh.sig_o,
            nu_o: sh.nu_o,
            sig_y: sh.sig_y,
            rho_y: sh.rho_y,
            news_cut_share: sh.news_cut_share,
            news_phase: sh.news_phase,
            rho_n: sh.rho_n,
            opec_q: 0.0,
            q_mbd: t.q_mbd,
            brent_usd: t.brent_usd,
            shock_on: if cfg.stochastic_oil { 1.0 } else { 0.0 },
            offset: if cfg.stochastic_oil { cfg.world_oil_mean_offset } else { 0.0 },
            qty_offset: if cfg.stochastic_oil { cfg.world_oil_qty_offset } else { 0.0 },
            market_share: cfg.world_opec_regime == "market_share",
        };
        // Profile (D4, D13, the shale regimes): changes replace the research values.
        let profile = t
            .profiles
            .get(cfg.world_oil_profile.as_str())
            .unwrap_or_else(|| panic!("unknown world_oil_profile {}", cfg.world_oil_profile));
        for (k, v) in profile {
            apply_change(&mut p, k, *v);
        }
        // Levers (D5) and scales.
        p.rho_q = cfg.world_oil_rho_q;
        p.sig_i *= cfg.world_oil_sig_i_mult * cfg.world_oil_sigma_mult;
        p.sig_o *= cfg.world_oil_sigma_mult;
        p.sig_y *= cfg.world_oil_sigma_mult;
        // Regime: market share switches the floor rule off and unwinds OPEC+ output at the research rate (assumption, REPORT 3).
        if p.market_share {
            p.kap_l = 0.0;
            p.unwind = s.market_share_unwind * 100.0 / t.q_mbd;
        }
        // UAE switch (D7): with the UAE out, OPEC+ liquids and the UAE's share of spare capacity leave the group.
        if cfg.world_uae_in_opec {
            p.opec_q = t.opec.uae_in_liquids_mbd / t.q_mbd;
        } else {
            p.opec_q = t.opec.uae_out_liquids_mbd / t.q_mbd;
            p.spare_cap *= 1.0 - cfg.world_opec_uae_spare_share;
        }
        // Regional coefficients (D12). The phibar used is the sum over the regions of this table (equal to the table's value).
        if aggregate {
            p.omega = vec![1.0];
            p.beta = vec![t.beta_y];
            p.eta0_i = vec![p.eta0];
            p.eta1_i = vec![p.eta1];
        } else {
            let phibar: f64 = t.regions.iter().map(|r| r.omega * r.phi).sum();
            p.omega = t.regions.iter().map(|r| r.omega).collect();
            p.beta = t.regions.iter().map(|r| r.beta).collect();
            p.eta0_i = t.regions.iter().map(|r| p.eta0 * r.phi / phibar).collect();
            p.eta1_i = t.regions.iter().map(|r| p.eta1 * r.phi / phibar).collect();
        }
        p
    }

}

/// Region index of a bloc in the integrated market (0 is the US).
pub fn region_of(b: Bloc) -> usize {
    1 + b.idx()
}

/// The month's innovations and scheduled inputs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OilShocks {
    /// Oil-specific innovation, already scaled (sig_I x unit-variance draw).
    pub e_i: f64,
    /// Extra consumption-demand innovation (zero by default).
    pub e_q: f64,
    /// New supply-disruption innovation (compensated jumps), % of Q; positive is a loss.
    pub e_d: f64,
    /// OPEC+ policy surprise, % of Q; positive is more supply.
    pub e_o: f64,
    /// Expectation (news) innovation, % of Q (zero in the ABM; used by the golden and `oil-check`).
    pub nu: f64,
}

/// Inputs known at the start of the month: the previous month's activity and real exchange rates by region, an optional
/// monthly-average target (exact path), a replay supply total (historical test), and a scheduled supply loss.
#[derive(Debug, Clone, Default)]
pub struct OilInputs {
    pub y: Vec<f64>,
    pub rfx: Vec<f64>,
    pub target: Option<f64>,
    pub supply_override: Option<f64>,
    pub ext_supply: f64,
}

/// The month's published values of one copy of the market (`OilState::step`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OilOut {
    pub p: f64,
    pub pbar: f64,
    pub supply: f64,
    pub demand: f64,
    pub di: f64,
    pub i: f64,
    pub o: f64,
    pub us: f64,
    pub sd: f64,
    pub q: f64,
    pub tau: f64,
    /// Region 0 (the US) demand deviation d_US = beta_0 ylag_0 + q - z_0 - eta0_0 (pbar + rfx_0) of this month, % of Q (W3-18).
    pub us_d: f64,
}

/// The state of one copy of the market (spec 2.4 state variables).
#[derive(Debug, Clone, PartialEq)]
pub struct OilState {
    pub p: f64,
    pub pbar: f64,
    pub z: Vec<f64>,
    pub ylag: Vec<f64>,
    pub tau: f64,
    pub o: f64,
    pub u1: f64,
    pub u2: f64,
    pub r: f64,
    pub sd: f64,
    pub ui: f64,
    pub q: f64,
    pub i: f64,
    pub n: f64,
    pub cut_pend: f64,
    pub hist: [f64; 3],
}

impl OilState {
    pub fn new(p: &OilParams) -> OilState {
        OilState {
            p: 0.0,
            pbar: 0.0,
            z: vec![0.0; p.n],
            ylag: vec![0.0; p.n],
            tau: 0.0,
            o: 0.0,
            u1: 0.0,
            u2: 0.0,
            r: 0.0,
            sd: 0.0,
            ui: 0.0,
            q: 0.0,
            i: 0.0,
            n: 0.0,
            cut_pend: 0.0,
            hist: [0.0; 3],
        }
    }

    /// One month (spec 2.4). The order of the updates is the prototype's. `OilInputs::target` solves the precautionary innovation
    /// so that the monthly-average price equals the target (exact path, copy U); `supply_override` replays a historical total.
    pub fn step(&mut self, p: &OilParams, inp: &OilInputs, sh: &OilShocks) -> OilOut {
        let w = p.w;
        // Shocks: the precautionary flow, the persistent consumption shift, the disruption state.
        self.ui = p.rho_i * self.ui + sh.e_i;
        self.q = p.rho_q * self.q + p.theta_q * sh.e_i + sh.e_q;
        self.sd = p.rho_d * self.sd + sh.e_d;
        // Activity of each region (the previous month's gap), smoothed.
        for i in 0..p.n {
            self.ylag[i] = p.rho_ylag * self.ylag[i] + (1.0 - p.rho_ylag) * inp.y[i];
        }
        // News: the expectation state and the phased-in announced cut.
        let nu = sh.nu;
        self.n = p.rho_n * self.n + nu;
        self.cut_pend += p.news_cut_share * nu;
        let step_cut = (1.0 - p.news_phase) * self.cut_pend;
        self.cut_pend -= step_cut;
        // OPEC+: the three-month average price drives the target; the spare-capacity cap bounds the output.
        let ptil = (self.hist[0] + self.hist[1] + self.hist[2]) / 3.0;
        let mut dtau = -p.kap_l * (p.pi_l - ptil).max(0.0) + p.kap_h * (ptil - p.pi_h).max(0.0) - p.stock_kappa * self.i + sh.e_o
            - step_cut;
        if p.market_share {
            dtau += p.unwind;
        }
        let decay = if ptil >= p.pi_l { p.tau_decay } else { 0.0 };
        self.tau = (self.tau * (1.0 - decay) + dtau).max(p.tau_min).min(p.spare_cap);
        self.o = (self.o + p.theta_o * (self.tau - self.o)).min(p.spare_cap);
        // US tight oil (two stages) and the rest of non-OPEC conventional supply respond to last month's price.
        let pl = self.hist[0];
        self.u1 = p.rho_u1 * self.u1 + (1.0 - p.rho_u1) * p.eps_us_lr * pl;
        self.u2 = p.rho_u2 * self.u2 + (1.0 - p.rho_u2) * self.u1;
        self.r = p.rho_rest * self.r + (1.0 - p.rho_rest) * p.eps_rest_lr * pl;
        let mut s_pre = self.o + p.sh_us * self.u2 + p.sh_rest * self.r - self.sd - inp.ext_supply;
        let mut eps_s = p.eps_s;
        if let Some(so) = inp.supply_override {
            s_pre = so;
            eps_s = 0.0;
        }
        // Market clearing (closed form, D12 aggregates): k, a and the regional sums.
        let k = eps_s + p.eta0 + (1.0 - p.rho_z) * p.eta1;
        let mut sum_bl = 0.0;
        let mut sum_z = 0.0;
        let mut rx = 0.0;
        for i in 0..p.n {
            sum_bl += p.omega[i] * p.beta[i] * self.ylag[i];
            sum_z += p.omega[i] * self.z[i];
            rx += p.omega[i] * (p.eta0_i[i] + (1.0 - p.rho_z) * p.eta1_i[i]) * inp.rfx[i];
        }
        let a = s_pre - sum_bl - self.q + p.rho_z * sum_z + rx;
        let psi_e = p.psi * (1.0 - p.rho_e);
        let pm1 = self.p;
        let stock_fb = p.xi * self.i.max(0.0) + p.xi_d * self.i.min(0.0);
        if let Some(tg) = inp.target {
            let p_t = (tg - (1.0 - w) * pm1) / w;
            self.ui = p_t * (k * w + psi_e) + stock_fb + a + k * (1.0 - w) * pm1 - p.psi * p.lam_n * self.n;
        }
        let rhs = self.ui + p.psi * p.lam_n * self.n - stock_fb - a - k * (1.0 - w) * pm1;
        self.p = rhs / (k * w + psi_e);
        self.pbar = w * self.p + (1.0 - w) * pm1;
        let di = -psi_e * self.p - stock_fb + p.psi * p.lam_n * self.n + self.ui;
        self.i += di;
        // Slow demand response, updated with this month's price plus each region's real exchange rate.
        for i in 0..p.n {
            self.z[i] = p.rho_z * self.z[i] + (1.0 - p.rho_z) * p.eta1_i[i] * (self.pbar + inp.rfx[i]);
        }
        self.hist = [self.pbar, self.hist[0], self.hist[1]];
        let supply = s_pre + eps_s * self.pbar;
        let mut demand = 0.0;
        for i in 0..p.n {
            demand += p.omega[i]
                * (p.beta[i] * self.ylag[i] + self.q - self.z[i] - p.eta0_i[i] * (self.pbar + inp.rfx[i]));
        }
        let us_d = p.beta[0] * self.ylag[0] + self.q - self.z[0] - p.eta0_i[0] * (self.pbar + inp.rfx[0]);
        OilOut {
            p: self.p,
            pbar: self.pbar,
            supply,
            demand,
            di,
            i: self.i,
            o: self.o,
            us: self.u2,
            sd: self.sd,
            q: self.q,
            tau: self.tau,
            us_d,
        }
    }
}

/// The per-stream generator state of the structural draws: the GARCH variance and the last base draw (prototype `simulate`).
#[derive(Debug, Clone, PartialEq)]
pub struct DrawState {
    pub h: f64,
    pub last_base: f64,
}

impl Default for DrawState {
    fn default() -> Self {
        DrawState { h: 1.0, last_base: 0.0 }
    }
}

/// Unit-variance Student-t draw with `nu` degrees of freedom (integer `nu` gives the exact chi-square), winsorised at +/-8.
fn student_unit<R: Rng + ?Sized>(r: &mut R, nu: f64) -> f64 {
    if !(nu > 0.0) {
        return normal(r);
    }
    let k = nu.round().max(1.0) as usize;
    let mut chi = 0.0;
    for _ in 0..k {
        let x = normal(r);
        chi += x * x;
    }
    let z = normal(r);
    (z * ((nu - 2.0) / chi).sqrt()).clamp(-WINSOR, WINSOR)
}

fn exponential<R: Rng + ?Sized>(r: &mut R, mean: f64) -> f64 {
    let u: f64 = r.gen();
    -(1.0 - u).ln() * mean
}

/// The oil-specific innovation of the month (stream `WORLD_OIL_I`): GARCH(1,1) Student-t base draw plus compensated jumps,
/// scaled by sig_I and the shock switch. Always draws in the same order.
pub fn draw_innovation<R: Rng + ?Sized>(p: &OilParams, g: &mut DrawState, r: &mut R) -> f64 {
    g.h = ((1.0 - p.garch_a - p.garch_b) + p.garch_a * g.last_base * g.last_base + p.garch_b * g.h).clamp(0.04, 25.0);
    let zt = student_unit(r, p.nu_i);
    let base = zt * g.h.sqrt();
    g.last_base = base;
    let nj = poisson(r, p.jump_lam_i);
    let jump = if nj > 0 {
        let mean = p.jump_mu_i * nj as f64;
        let sd = p.jump_sd_i * (nj as f64).sqrt();
        mean + sd * normal(r)
    } else {
        0.0
    };
    let var_j = p.jump_lam_i * (p.jump_mu_i * p.jump_mu_i + p.jump_sd_i * p.jump_sd_i);
    let ei = (1.0 - var_j).max(0.05).sqrt() * base + jump - p.jump_lam_i * p.jump_mu_i;
    p.shock_on * p.sig_i * ei
}

/// The disruption and OPEC+ innovations of the month (stream `WORLD_OIL_S`): compensated Poisson jumps, and a Student-t OPEC+
/// surprise. Activity is not drawn in the integrated run (D5). Always draws in the same order.
pub fn draw_supply<R: Rng + ?Sized>(p: &OilParams, r: &mut R) -> (f64, f64) {
    let nd = poisson(r, p.jump_lam_d);
    let mut sz = 0.0;
    for _ in 0..nd {
        sz += exponential(r, p.jump_mean_d);
    }
    let e_d = p.shock_on * (sz - p.jump_lam_d * p.jump_mean_d);
    let e_o = p.shock_on * p.sig_o * student_unit(r, p.nu_o);
    (e_d, e_o)
}

/// The prototype's activity innovation (isolated `oil-check` only, drawn on `WORLD_OIL_S` after the supply draws).
pub fn draw_activity<R: Rng + ?Sized>(p: &OilParams, r: &mut R) -> f64 {
    p.sig_y * normal(r)
}

/// The OilSupply loss path (spec 2.4 `OilSupply`, the ramp, hold and recovery of the Hormuz preset): `k` months after the
/// shock. With ramp 1, hold 3, recover 6 it gives 12, 12, 12, 10, 8, 6, 4, 2, 0 (the research's K7 path).
pub fn supply_path(pct: f64, ramp: u32, hold: u32, recover: u32, k: u32) -> f64 {
    let ramp = ramp.max(1);
    let full_from = ramp - 1;
    if k < full_from {
        return pct * f64::from(k + 1) / f64::from(ramp);
    }
    let hold_end = full_from + hold;
    if k < hold_end {
        return pct;
    }
    let j = k - hold_end;
    if j < recover {
        return pct * (1.0 - f64::from(j + 1) / f64::from(recover));
    }
    0.0
}

/// A scheduled oil event fired by a user shock, with its firing month on the ABM clock.
#[derive(Debug, Clone, PartialEq)]
pub enum OilEvent {
    Supply { pct: f64, ramp: u32, hold: u32, recover: u32, t0: u32 },
    Opec { per_month: f64, months: u32, t0: u32 },
}

impl OilEvent {
    /// Scheduled supply loss and policy surprise of month `t` (% of Q; a loss is positive, a surprise positive is more supply).
    pub fn at(&self, t: u32) -> (f64, f64) {
        match *self {
            OilEvent::Supply { pct, ramp, hold, recover, t0 } => (supply_path(pct, ramp, hold, recover, t.saturating_sub(t0)), 0.0),
            OilEvent::Opec { per_month, months, t0 } => {
                let k = t.saturating_sub(t0);
                (0.0, if k < months { per_month } else { 0.0 })
            }
        }
    }

    fn finished(&self, t: u32) -> bool {
        match *self {
            OilEvent::Supply { ramp, hold, recover, t0, .. } => t.saturating_sub(t0) >= ramp + hold + recover,
            OilEvent::Opec { months, t0, .. } => t.saturating_sub(t0) >= months,
        }
    }
}

/// The splice of the ABM oil price onto the market (S3-R1): `G_t = kappa rho^(t+1)`. The OPEC+ quantity input has the same form
/// (stage-3 fix round, S3-R8 option b): `G^q_t = kappa_q rho^(t+1)` with `kappa_q = LEGACY_QTY_BASE - (qty_b - qty_offset)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Splice {
    pub kappa: f64,
    pub rho: f64,
    pub kappa_q: f64,
}

/// The legacy OPEC+ quantity input of the base month: zero in burn-in (the legacy blocs see no oil quantity channel).
pub const LEGACY_QTY_BASE: f64 = 0.0;

/// This month's values of the market for the metrics (`WorldMetrics` stage-3 group).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OilDiag {
    pub p_bar: f64,
    pub brent_usd: f64,
    pub supply: f64,
    pub demand: f64,
    pub opec_mbd: f64,
    pub us_mbd: f64,
    pub stocks: f64,
    pub spare: f64,
    pub dev: f64,
    pub live_u: bool,
    /// OPEC+ quantity input of this month: the market's deviation (% of OPEC+ liquids, before the offset and the splice) and
    /// what the blocs receive (offset removed, splice added; zero in burn-in). Stage-3 fix round, S3-R8.
    pub qty_mkt: f64,
    pub qty_iface: f64,
    /// Region 0 (US) demand deviation of this month, % of Q (the world metric `oil_us_demand_pct`, W3-18).
    pub us_demand: f64,
}

/// The block-level market: copy A (the structural market), copy U (the exact path of a live OilPrice shock), the draw state,
/// the scheduled events, the splice, and this month's published values (`bloc_oil` feeds the blocs' oil and OPEC+ channels).
#[derive(Debug, Clone)]
pub struct OilMarket {
    pub par: OilParams,
    pub a: OilState,
    pub u: Option<OilState>,
    pub gar: DrawState,
    pub events: Vec<OilEvent>,
    pub consumed: usize,
    pub splice: Option<Splice>,
    pub rho_splice: f64,
    pub t_rep: u32,
    /// (oil gap in %, OPEC+ quantity deviation in %) of this month for the blocs (zeros when the link is off).
    pub bloc_oil: (f64, f64),
    pub diag: OilDiag,
    /// `world_oil_feedback`: the market reads the blocs' output gaps and real exchange rates (true, the default).
    pub feedback: bool,
    /// `world_oil_shock_mask`: the structural sources drawn into the market (I, D, O; the default IDO).
    pub mask: String,
}

impl OilMarket {
    /// The block-level market of a run: the integrated parameters, the splice persistence (S3-R1: `oil_ar` unless overridden).
    pub fn new(cfg: &Config, t: &OilTables) -> OilMarket {
        let par = OilParams::new(t, cfg, false);
        let rho_splice = if cfg.world_oil_splice_rho < 0.0 { cfg.oil_ar } else { cfg.world_oil_splice_rho };
        OilMarket {
            a: OilState::new(&par),
            par,
            u: None,
            gar: DrawState::default(),
            events: Vec::new(),
            consumed: 0,
            splice: None,
            rho_splice,
            t_rep: 0,
            bloc_oil: (0.0, 0.0),
            diag: OilDiag::default(),
            feedback: cfg.world_oil_feedback,
            mask: cfg.world_oil_shock_mask.clone(),
        }
    }
}

/// The market's log-deviation splice term of the reported month (spec S3-R1); zero before the base is captured.
pub fn splice_term(m: &OilMarket) -> f64 {
    match m.splice {
        Some(s) => s.kappa * s.rho.powi(m.t_rep as i32 + 1),
        None => 0.0,
    }
}

/// The splice term of the OPEC+ quantity input of the reported month (S3-R8 option b); zero before the base is captured.
pub fn splice_term_q(m: &OilMarket) -> f64 {
    match m.splice {
        Some(s) => s.kappa_q * s.rho.powi(m.t_rep as i32 + 1),
        None => 0.0,
    }
}

/// The inputs of the month from the previous month's bloc and US outputs: activity by region (US first) and the bloc real
/// depreciation (the US is zero).
fn inputs_from(out: &WorldOut, n: usize) -> (Vec<f64>, Vec<f64>) {
    let mut y = vec![0.0; n];
    let mut rfx = vec![0.0; n];
    if n == NB + 1 {
        y[0] = out.y_us;
        for b in 0..NB {
            y[1 + b] = out.y_gap[b];
            rfx[1 + b] = out.z[b];
        }
    }
    (y, rfx)
}

/// Captures the splice at the end of the last burn-in month (the settlement hook, spec D36): `kappa` from the legacy level and
/// the market's shadow price of that month. Idempotent.
pub fn settle(w: &World, m: &mut OilMarket) {
    if m.splice.is_some() {
        return;
    }
    if w.tick_offset == 0 && w.cfg.burn_in_months > 0 && w.tick + 1 == w.cfg.burn_in_months {
        // The legacy level of the base month without the user shock: the shock is the perturbation, not part of the base level
        // (stage-3 review R4; with burn-in 0 and a month-0 shock the shock would otherwise enter kappa and be counted twice).
        let l_b = 100.0 * w.exog.oil_ar;
        let qty_b = m.a.o / m.par.opec_q;
        m.splice = Some(Splice {
            kappa: l_b - (m.a.pbar - m.par.offset),
            rho: m.rho_splice,
            kappa_q: LEGACY_QTY_BASE - (qty_b - m.par.qty_offset),
        });
    }
}

/// One month of the market (hook in `world::after_exog`, before the bloc step; spec 1.3). Steps copy A (and U when a live
/// OilPrice shock is on the exact path), writes `exog.oil_ar` and `exog.oil_price` when the link acts (reported months), and
/// sets the bloc channels. In burn-in the market only steps (shadow) and the blocs see the legacy oil gap.
pub fn apply(w: &mut World, m: &mut OilMarket, out: &WorldOut, reported: bool) {
    let n = m.par.n;
    let (mut y, mut rfx) = inputs_from(out, n);
    // world_oil_feedback false: the market ignores the blocs' activity and real exchange rates (the exact-path interface test,
    // stage-3 review; the default true reads them).
    if !m.feedback {
        y.iter_mut().for_each(|v| *v = 0.0);
        rfx.iter_mut().for_each(|v| *v = 0.0);
    }
    // Scheduled supply loss and OPEC+ surprises (user shocks fired by `exog` this month, plus those in progress).
    let t = w.tick;
    let fired: Vec<(Shock, u32)> = w.shocks.world_shocks[m.consumed..].to_vec();
    m.consumed = w.shocks.world_shocks.len();
    for (s, t0) in fired {
        match s {
            Shock::OilSupply { pct_of_world_supply, ramp_months, hold_months, recover_months } => m.events.push(OilEvent::Supply {
                pct: pct_of_world_supply,
                ramp: ramp_months,
                hold: hold_months,
                recover: recover_months,
                t0,
            }),
            Shock::OpecCut { mbd, months } => {
                let per = -(mbd / m.par.q_mbd) * 100.0 / f64::from(months);
                m.events.push(OilEvent::Opec { per_month: per, months, t0 });
            }
            _ => {}
        }
    }
    let mut ext = 0.0;
    let mut e_o_sched = 0.0;
    for ev in &m.events {
        let (l, o) = ev.at(t);
        ext += l;
        e_o_sched += o;
    }
    m.events.retain(|ev| !ev.finished(t));
    // Structural draws (always taken; scaled to zero when `stochastic_oil` is off).
    let mut ri = rng::agent_rng(w.seed, t, stream::WORLD_OIL_I, 0);
    let mut e_i = draw_innovation(&m.par, &mut m.gar, &mut ri);
    let mut rs = rng::agent_rng(w.seed, t, stream::WORLD_OIL_S, 0);
    let (mut e_d, mut e_o_draw) = draw_supply(&m.par, &mut rs);
    // world_oil_shock_mask (W3-19 cells): a source not listed is zeroed after its draw, so every stream is consumed as before.
    if !m.mask.contains('I') {
        e_i = 0.0;
    }
    if !m.mask.contains('D') {
        e_d = 0.0;
    }
    if !m.mask.contains('O') {
        e_o_draw = 0.0;
    }
    let shocks = OilShocks { e_i, e_q: 0.0, e_d, e_o: e_o_draw + e_o_sched, nu: 0.0 };
    // Splice capture when there is no burn-in (the first reported month is the base): the base is the month's legacy level.
    if reported && m.splice.is_none() {
        // Without the user shock of this month (stage-3 review R4): kappa must be the same in a matched pair.
        let l_b = 100.0 * w.exog.oil_ar;
        let qty_b = m.a.o / m.par.opec_q;
        m.splice = Some(Splice {
            kappa: l_b - (m.a.pbar - m.par.offset),
            rho: m.rho_splice,
            kappa_q: LEGACY_QTY_BASE - (qty_b - m.par.qty_offset),
        });
    }
    let inputs = OilInputs { y, rfx, target: None, supply_override: None, ext_supply: ext };
    let a_before = m.a.clone();
    let out_a = m.a.step(&m.par, &inputs, &shocks);
    // The legacy OilPrice shock: exact path (copy U pins the monthly-average price to the structural price plus the shock) or an
    // additive log shock in the ABM price (world_oil_exact_path off).
    let shock = w.shocks.oil_log_shock;
    let exact = w.cfg.world_oil_exact_path && shock.abs() > EXACT_DROP;
    let (p_used, o_used, live_u, out_used, st_used) = if exact {
        let target = out_a.pbar + 100.0 * shock;
        let mut u = match m.u.take() {
            Some(u) => u,
            None => a_before,
        };
        let inp_u = OilInputs { target: Some(target), ..inputs.clone() };
        let out_u = u.step(&m.par, &inp_u, &shocks);
        let (po, oo) = (out_u.pbar, out_u.o);
        m.u = Some(u);
        let st = m.u.as_ref().map(|s| s.i).unwrap_or(0.0);
        (po, oo, true, out_u, st)
    } else {
        m.u = None;
        (out_a.pbar, out_a.o, false, out_a, m.a.i)
    };
    let off = m.par.offset;
    let g = if reported { splice_term(m) } else { 0.0 };
    let a_copy_a = out_a.pbar - off + g;
    let a_used = p_used - off + g + if exact { 0.0 } else { 100.0 * shock };
    // The OPEC+ quantity input (S3-R8 option b): the market's deviation less the stochastic mean, plus the decaying splice.
    let qty_mkt = o_used / m.par.opec_q;
    let qty_iface = if reported { qty_mkt - m.par.qty_offset + splice_term_q(m) } else { 0.0 };
    if reported {
        let index = crate::common::oil_index(w);
        w.exog.oil_ar = a_copy_a / 100.0;
        w.exog.oil_price = w.exog.oil_base * index * (a_used / 100.0).exp();
        m.bloc_oil = (a_used, qty_iface);
        m.t_rep += 1;
    } else {
        m.bloc_oil = (100.0 * (w.exog.oil_ar + w.exog.oil_shock), 0.0);
    }
    let mb_per_pct = m.par.q_mbd / 100.0;
    m.diag = OilDiag {
        p_bar: p_used,
        brent_usd: m.par.brent_usd * (p_used / 100.0).exp(),
        supply: out_used.supply,
        demand: out_used.demand,
        opec_mbd: o_used * mb_per_pct,
        us_mbd: m.par.sh_us * out_used.us * mb_per_pct,
        stocks: st_used * mb_per_pct,
        spare: m.par.spare_cap - o_used,
        dev: a_used,
        live_u,
        qty_mkt,
        qty_iface,
        us_demand: out_used.us_d,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supply_path_matches_research_hormuz_path() {
        // Research K7: 12 % for 3 months, then linear restoration over 6 months (ramp 1, hold 3, recover 6).
        let v: Vec<f64> = (0..10).map(|k| supply_path(12.0, 1, 3, 6, k)).collect();
        let want = [12.0, 12.0, 12.0, 10.0, 8.0, 6.0, 4.0, 2.0, 0.0, 0.0];
        for (a, b) in v.iter().zip(want.iter()) {
            assert!((a - b).abs() < 1e-12, "{v:?}");
        }
    }
}
