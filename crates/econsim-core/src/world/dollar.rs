//! Stage 4-1: the bilateral dollar (spec 2.6, D30, D31, D37). Two phases a month around the bloc step, in reported months only,
//! with the dollar link on (`Config::world_link_dollar`).
//!
//! * Phase A (before `core`): the premium step of `fx::exchange_rate` (the exact calls and state updates of the merged code,
//!   because `exchange_rate` is skipped in these months), the credibility dollar term, the risk addend of the financial factor,
//!   the idiosyncratic bilateral noise `zeta`, the common addend the blocs see and its projection, and the non-policy-risk
//!   demand `fex` (2.7).
//! * Phase B (after `core`): the bilateral rates lnS_j, the broad index ln D = sum_j wH10_j lnS_j, and `w.row.fx = fx_0 exp(ln D)`.
//!
//! The base values are captured once (`DollarState::capture`, called from `WorldBlock::capture_bases`), never by a shock. The
//! Frisk filter runs every month in `finance::after_exog`; this module reads its state.

use std::sync::Arc;

use super::bloc::BlocState;
use super::net::UsInputs;
use super::params::WorldParams;
use super::{ln_s_level, K, NB};
use crate::config::Config;
use crate::exog;
use crate::fx;
use crate::rng::{self, normal, stream};
use crate::types::World;

/// The base values of the dollar (spec 2.6): the bloc values of the base month, ln PT, the broad rate and Frisk.
#[derive(Clone, Debug)]
pub struct DollarBase {
    /// Raw bloc outputs z_{j,b} (`hist.z[0]`).
    pub z: [f64; NB],
    /// ln P_{j,b} (`ln_p`).
    pub ln_p: [f64; NB],
    /// ln S_{j,b} = z/100 + ln P - ln PT (2.2.5).
    pub ln_s: [f64; NB],
    /// ln PT_b.
    pub ln_pt: f64,
    /// fx_0 (the broad rate of the base month).
    pub fx0: f64,
    /// Frisk_b (VIX points).
    pub frisk0: f64,
    /// List B (rev 9, D47): the conditional forecast of the blocs' z from the base month, shared by every clone of the world (the two
    /// arms of a pair). `None` with `world_dollar_neutral` off: the 4-1 form, z against its base value.
    pub fc: Option<Arc<Forecast>>,
}

/// Months of the conditional forecast (spec 2.6, rev 9): the forecast of z for the m-th reported month, m < `FC_MONTHS`; beyond it the
/// last value is held (the forcing has decayed: the expected US path is zero beyond K = 240 months).
pub const FC_MONTHS: usize = 480;

/// The blocs' conditional forecast of their real exchange rate made at the base month (spec 2.6, rev 9, D47): the path of z that the
/// twelve coupled blocs follow from their base state when the US inputs follow the expectations held at the base (the expected
/// policy path, the projection laws of the activity, oil, quantity and financial-factor terms), with no innovations and no shock.
/// It is a function of the base state only (shock-independent), computed once, so both arms of a pair share it.
#[derive(Clone, Debug)]
pub struct Forecast {
    /// Bloc clock of the first reported month (index 0 of `z`).
    pub n0: u32,
    pub z: Vec<[f64; NB]>,
}

/// One `UsCredibility` component (spec 2.6, D31): the dollar term ln(1 - dollar_pct/100), its profile length and firing month on
/// the bloc clock.
#[derive(Clone, Debug)]
pub struct CredComp {
    pub ln_dollar: f64,
    pub months: u32,
    pub t0: u32,
}

/// The dollar state of the block (spec 1.2, 2.6). `None` in the block when the link is off.
#[derive(Clone, Debug)]
pub struct DollarState {
    pub base: Option<DollarBase>,
    /// Frisk_t (VIX points): the filtered risk factor. Updated in `finance::after_exog` every month.
    pub frisk: f64,
    /// F_{t-1} (VIX points), for the innovation of the risk factor.
    pub f_prev: f64,
    /// F_t of the current month (VIX points), set by `finance::after_exog`.
    pub f_now: f64,
    /// Fpol_t (VIX points): the policy-induced part of F.
    pub fpol: f64,
    /// Index into `ShockState::world_shocks` of the first RiskOff or BlocDemand not yet injected into f.
    pub fin_consumed: usize,
    /// Active credibility components.
    pub cred: Vec<CredComp>,
    /// Idiosyncratic bilateral AR(1) noise zeta_{j,t} (log points); zero at the base.
    pub zeta: [f64; NB],
    /// Risk-off loadings gamma_j (% dollar appreciation per VIX point), bloc order.
    pub gamma: [f64; NB],
    /// Normalised H10 broad-dollar weights wH10_j.
    pub wh10: [f64; NB],
    /// Innovation sd of zeta_j (% per month) by bloc.
    pub sd: [f64; NB],
    /// This month: the bilateral addends a_{j,t} (log points).
    pub a_bil: [f64; NB],
    /// This month: the common addend a_t (the blocs see it, log points).
    pub a_common: f64,
    /// This month: the bilateral log rates lnS_{j,t} of phase B.
    pub ln_s_now: [f64; NB],
    /// This month: the premium-driven part q_psi (log points).
    pub qpsi: f64,
    /// This month: the credibility dollar term (log points).
    pub cred_now: f64,
    /// This month: the broad risk addend gbar_t (log points).
    pub gbar: f64,
    /// This month: the z each bloc is read against (phase B), for the `z_fc` diagnostic.
    pub z_ref_now: [f64; NB],
}

/// The profile p(age) of a credibility shock (spec 2.6): 1 for `age < months`, then `0.5^((age - months + 1)/3)`.
pub fn cred_profile(age: u32, months: u32) -> f64 {
    if age < months {
        1.0
    } else {
        0.5f64.powf(f64::from(age - months + 1) / 3.0)
    }
}

/// ln PT of the month (spec 2.6): the log of the tradables price level of `trade::pt_level`, zero when it is not positive.
#[inline(never)]
pub fn ln_pt_of(w: &World) -> f64 {
    let pt = super::trade::pt_level(w);
    if pt > 0.0 && pt.is_finite() {
        pt.ln()
    } else {
        0.0
    }
}

impl DollarState {
    /// A dollar state for a block created with the link on (`WorldBlock::new`). The H10 weights are the trade tables' (normalised).
    pub fn new(cfg: &Config, wp: &WorldParams, wh10: [f64; NB]) -> DollarState {
        let gamma: [f64; NB] = std::array::from_fn(|j| wp.finance.gamma_pct_per_vix.get(j).copied().unwrap_or(0.0));
        // Bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW. AE = EA, JP, UK, CA, AU; EME = MX, IN, KR, BR, RoW.
        let sd: [f64; NB] = [
            cfg.world_fx_idio_sd_ae,
            cfg.world_fx_idio_sd_cn,
            cfg.world_fx_idio_sd_ae,
            cfg.world_fx_idio_sd_ae,
            cfg.world_fx_idio_sd_ae,
            cfg.world_fx_idio_sd_eme,
            cfg.world_fx_idio_sd_eme,
            cfg.world_fx_idio_sd_eme,
            cfg.world_fx_idio_sd_eme,
            cfg.world_fx_idio_sd_ae,
            cfg.world_fx_idio_sd_opec,
            cfg.world_fx_idio_sd_eme,
        ];
        DollarState {
            base: None,
            frisk: 0.0,
            f_prev: 0.0,
            f_now: 0.0,
            fpol: 0.0,
            fin_consumed: 0,
            cred: Vec::new(),
            zeta: [0.0; NB],
            gamma,
            wh10,
            sd,
            a_bil: [0.0; NB],
            a_common: 0.0,
            ln_s_now: [0.0; NB],
            qpsi: 0.0,
            cred_now: 0.0,
            gbar: 0.0,
            z_ref_now: [0.0; NB],
        }
    }

    /// Captures the base (spec 2.6): once, at the end of the last burn-in month or the first step of a world without burn-in.
    /// `blocs` are the bloc states of that month; `w.row.fx` is the broad rate (1.0 when it is not finite).
    pub fn capture(&mut self, w: &World, blocs: &[BlocState; NB]) {
        if self.base.is_some() {
            return;
        }
        let z: [f64; NB] = std::array::from_fn(|j| blocs[j].hist.z[0]);
        let ln_p: [f64; NB] = std::array::from_fn(|j| blocs[j].ln_p);
        let ln_pt = ln_pt_of(w);
        let ln_s: [f64; NB] = std::array::from_fn(|j| ln_s_level(z[j], ln_p[j], ln_pt));
        let fx0 = if w.row.fx.is_finite() { w.row.fx } else { 1.0 };
        self.base = Some(DollarBase { z, ln_p, ln_s, ln_pt, fx0, frisk0: self.frisk, fc: None });
    }

    /// Stores the conditional forecast (once, right after the base is captured).
    #[inline(never)]
    pub fn set_forecast(&mut self, fc: Arc<Forecast>) {
        if let Some(b) = self.base.as_mut() {
            if b.fc.is_none() {
                b.fc = Some(fc);
            }
        }
    }

    /// The z each bloc is read against in bloc month `n`: its conditional forecast with `world_dollar_neutral` (a forecast-error
    /// state, zero at the base), otherwise its base value (the 4-1 form).
    #[inline(never)]
    pub fn z_ref(&self, n: u32) -> [f64; NB] {
        let Some(b) = self.base.as_ref() else { return [0.0; NB] };
        match b.fc.as_ref() {
            Some(f) if !f.z.is_empty() => {
                let m = (n.saturating_sub(f.n0) as usize).min(f.z.len() - 1);
                f.z[m]
            }
            _ => b.z,
        }
    }

    /// Registers a `UsCredibility` shock that fired at bloc month `t0` (from `WorldBlock::add_user`).
    pub fn add_cred(&mut self, dollar_pct: f64, months: u32, t0: u32) {
        self.cred.push(CredComp { ln_dollar: (1.0 - dollar_pct / 100.0).ln(), months, t0 });
    }

    /// The credibility dollar term at bloc month `n`, with `k` months ahead: sum over components of ln(1 - dollar_pct/100) p(age + k).
    pub fn cred_at(&self, n: u32, k: u32) -> f64 {
        self.cred.iter().map(|c| c.ln_dollar * cred_profile(n.saturating_sub(c.t0) + k, c.months)).sum()
    }
}

/// Phase A (spec 2.6, reported months with the link on and the base captured), before the bloc step. `n` is the bloc clock.
/// Sets the dollar fields of `us` (`dollar_delta`, `dollar`, `proj_on`, `dollar_proj`, `fex`) and this month's outputs in `d`.
#[inline(never)]
pub fn phase_a(d: &mut DollarState, w: &mut World, us: &mut UsInputs, n: u32) {
    let delta = us.dollar;
    us.dollar_delta = delta;
    let rho_psi = w.cfg.fx_psi_rho;
    let adj = w.cfg.fx_adjust;
    let wgt = w.cfg.fx_psi_weight;
    // The premium step of `fx::exchange_rate` (fx.rs:74-120), with its state updates (guarded by finiteness, as there).
    let psi = fx::anchored_premium(w.fx_psi_state, w.fx_psi_exog_prev, w.exog.fx_psi(), rho_psi, false);
    let mp_on = w.cfg.fx_mp_premium != 0.0;
    let z = if mp_on {
        fx::monetary_surprise_premium(w.cfg.fx_mp_premium, w.cfg.fx_mp_cap, w.shocks.policy_gap(w.tick))
    } else {
        0.0
    };
    let q_new = if mp_on {
        fx::psi_step(0.0, 0.0, w.fx_psi_state + w.monetary.fx_mp_prev, 0.0, psi + z, adj, wgt).1
    } else {
        fx::psi_step(0.0, 0.0, w.fx_psi_state, 0.0, psi, adj, wgt).1
    };
    if q_new.is_finite() {
        w.fx_psi_state = psi;
        w.fx_psi_component = q_new;
        if mp_on {
            w.monetary.fx_mp_prev = z;
        }
    }
    w.fx_psi_exog_prev = w.exog.fx_psi();
    let qpsi = if q_new.is_finite() { q_new } else { w.fx_psi_component };

    let Some(base) = d.base.as_ref() else { return };
    let frisk0 = base.frisk0;
    // Credibility dollar term at this month (age 0 in the shock month).
    let cred = d.cred_at(n, 0);
    // Risk addend: g_j = world_fx_risk_mult gamma_j/100 (Frisk - Frisk_b); gbar = sum_j wH10_j g_j.
    let mult = w.cfg.world_fx_risk_mult;
    let g: [f64; NB] = std::array::from_fn(|j| mult * d.gamma[j] / 100.0 * (d.frisk - frisk0));
    let gbar: f64 = (0..NB).map(|j| d.wh10[j] * g[j]).sum();
    // Idiosyncratic noise: one normal per bloc and month from WORLD_FX, drawn whether or not the sd is zero.
    let rho_z = w.cfg.world_fx_idio_rho;
    for j in 0..NB {
        let mut r = rng::agent_rng(w.seed, w.tick, stream::WORLD_FX, j as u64);
        let e = normal(&mut r);
        d.zeta[j] = rho_z * d.zeta[j] + d.sd[j] / 100.0 * e;
    }
    // Common addend and the bilateral addends (log points). q = qpsi + cred.
    let q = qpsi + cred;
    let a_common = delta + q + gbar;
    let a_bil: [f64; NB] = std::array::from_fn(|j| delta + q + g[j]);
    us.dollar = a_common;
    us.proj_on = true;
    for k in 1..=K {
        let kf = k as i32;
        let proj = qpsi * rho_psi.powi(kf) + gbar * w.cfg.world_fx_risk_rho.powi(kf) + d.cred_at(n, k as u32);
        us.dollar_proj[k - 1] = proj;
    }
    // Non-policy risk: fex = F_t - Fbar - Fpol_t (2.7), in VIX points.
    let fbar = w.cfg.world_vix_per_unit * exog::fin_factor_mean(&w.cfg);
    us.fex = d.f_now - fbar - d.fpol;

    d.qpsi = qpsi;
    d.cred_now = cred;
    d.gbar = gbar;
    d.a_common = a_common;
    d.a_bil = a_bil;
}

/// Phase B (spec 2.6, after the bloc step): lnS_{j,t} from the bloc outputs of this month, ln D = sum_j wH10_j lnS_{j,t}, and
/// `w.row.fx = fx_0 exp(ln D)` when finite. `ln_pt_prev` is the ln PT of the month before this one (the price `step_month`
/// read before the month cleared).
#[inline(never)]
pub fn phase_b(d: &mut DollarState, w: &mut World, blocs: &[BlocState; NB], ln_pt_prev: f64, n: u32) {
    let zr = d.z_ref(n);
    d.z_ref_now = zr;
    let Some(base) = d.base.as_ref() else { return };
    let mut ln_d = 0.0;
    for j in 0..NB {
        let z = blocs[j].hist.z[0];
        let ln_s = (z - zr[j]) / 100.0 + (blocs[j].ln_p - base.ln_p[j]) - (ln_pt_prev - base.ln_pt) + d.a_bil[j] + d.zeta[j];
        d.ln_s_now[j] = ln_s;
        ln_d += d.wh10[j] * ln_s;
    }
    if ln_d.is_finite() {
        w.row.fx = base.fx0 * ln_d.exp();
    }
}
