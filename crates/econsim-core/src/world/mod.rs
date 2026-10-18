//! World block (b8-world, stage 1): twelve semi-structural bloc models solved offline (`data/bloc_solution.json`) and stepped
//! in Rust, shadow-coupled to the unchanged ABM. The blocs see the US (policy deviation, expected path, output gap, Dollar
//! level); the ABM does not see the blocs yet. The block is off by default (`Config::world_block_enabled`); with it off the
//! hooks take the legacy branches and the output is byte-identical to M3 (spec 0.1, 1.4, 6.3).
//!
//! Stage-1 scope (spec 3, Stage 1): the blocs, the network, the US inputs, `BlocPolicy`/`BlocDemand`/`GlobalDemand`, the
//! `world` metrics (first group), `expected_rate_path`, and the replay function `step_inputs` for the reference parity tests.
//! `after_exog` and `settle` are empty in stage 1 (the oil, dollar and finance links come in later stages).

pub mod bloc;
pub mod dollar;
pub mod finance;
pub mod net;
pub mod oil;
pub mod oil_check;
pub mod params;
pub mod positions;
pub mod trade;
pub mod usnow;
pub mod wmetrics;

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::rng::{self, normal, stream};
use crate::shocks::{Shock, BLOC_NAMES};
use crate::types::{role_mean, World};
use bloc::{BlocState, UsHist, UserKind, UserShock};
use net::{Net, UsInputs};
use oil::OilMarket;
use params::{Par, WorldParams};
use trade::{TariffComp, TradeOut, TradeState};
use wmetrics::WorldMetrics;

/// Number of blocs.
pub const NB: usize = 12;
/// Forecast horizon of the forcing paths (D29); the kernel tail covers k >= K.
pub const K: usize = 240;
/// Length of the state vector (y x3, i x1, z x13, pi x13, mrr x3).
pub const NS: usize = 33;

/// The twelve blocs in solution order (spec 1.9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Bloc {
    EA,
    CN,
    JP,
    UK,
    CA,
    MX,
    IN,
    KR,
    BR,
    AU,
    #[serde(rename = "OPEC+")]
    OpecPlus,
    RoW,
}

impl Bloc {
    pub const ALL: [Bloc; NB] = [
        Bloc::EA,
        Bloc::CN,
        Bloc::JP,
        Bloc::UK,
        Bloc::CA,
        Bloc::MX,
        Bloc::IN,
        Bloc::KR,
        Bloc::BR,
        Bloc::AU,
        Bloc::OpecPlus,
        Bloc::RoW,
    ];

    pub fn idx(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Bloc::EA => "EA",
            Bloc::CN => "CN",
            Bloc::JP => "JP",
            Bloc::UK => "UK",
            Bloc::CA => "CA",
            Bloc::MX => "MX",
            Bloc::IN => "IN",
            Bloc::KR => "KR",
            Bloc::BR => "BR",
            Bloc::AU => "AU",
            Bloc::OpecPlus => "OPEC+",
            Bloc::RoW => "RoW",
        }
    }
}

/// Index in solution order of a validated bloc name (`shocks::BLOC_NAMES`).
pub fn bloc_index(name: &str) -> usize {
    BLOC_NAMES.iter().position(|&n| n == name).unwrap_or_else(|| panic!("unknown bloc {name}; validate the shock first"))
}

/// This month's values read by other modules (spec 1.2 `WorldOut`): the US input and the bloc gaps (stage 1); the import
/// prices, the effective duties and the exports read by hooks H6-H8 when `trade_active` (stage 2).
#[derive(Clone, Debug, Default)]
pub struct WorldOut {
    pub ius: f64,
    /// Stage 3: the US output gap used this month (y_US, %), read by the oil market's activity of the next month.
    pub y_us: f64,
    pub y_gap: [f64; NB],
    pub pi_gap: [f64; NB],
    pub i_gap: [f64; NB],
    pub z: [f64; NB],
    /// Import price relative to the base by sector, and its CPI-weighted index (H6).
    pub imp_rel: [f64; trade::S],
    pub imp_rel_cpi: f64,
    /// Effective duty per channel, decimal (H8).
    pub tau_eff_hh: f64,
    pub tau_eff_firm: f64,
    /// Exports by sector, nominal model units per month (H7).
    pub exports: [f64; trade::S],
}

/// The reference values captured at the first reported month (spec D36, 1.2 `Bases`): ln P_j, ln S_j (the bloc's own level,
/// z/100 + ln P - ln PT) and ln PT (the US tradables price, log).
#[derive(Clone, Debug)]
pub struct Bases {
    pub ln_p: [f64; NB],
    pub ln_s: [f64; NB],
    pub ln_pt: f64,
}

/// One month of inputs and outputs, as the reference replay file records it (spec 2.1, `world-dump`).
#[derive(Clone, Debug)]
pub struct MonthRecord {
    pub us: UsInputs,
    /// The innovations used this month, [e_y, e_pi, e_i, eta] per bloc (scaled; zero when the scale is zero).
    pub eps: [[f64; 4]; NB],
    /// Outputs [y, pi, i, z] per bloc.
    pub out: [[f64; 4]; NB],
}

/// The block (spec 1.2 `WorldBlock`). `World.world` is `None` when the block is off.
#[derive(Clone, Debug)]
pub struct WorldBlock {
    pub params: Arc<WorldParams>,
    pub par: Par,
    pub blocs: [BlocState; NB],
    pub us: UsHist,
    /// Months stepped since creation (the bloc clock; equal to the ABM tick when burn-in is zero).
    pub n: u32,
    /// Active user shocks, with their activation month on the bloc clock.
    pub users: Vec<UserShock>,
    /// Index into `ShockState::world_shocks` of the first shock not yet taken.
    pub consumed: usize,
    pub bases: Option<Bases>,
    /// Stage 2: the trade state (spec 1.2). Created with the block; its bases are captured at the end of the last burn-in month
    /// (D36; a world without burn-in at its first step).
    pub trade: Option<TradeState>,
    /// Stage 3: the oil market (spec 2.4), created when `world_link_oil` is set; it steps in shadow through the burn-in.
    pub oil: Option<Box<OilMarket>>,
    /// Stage 4-1 (spec 2.6, 2.7): the bilateral dollar and the financial factor, created when `world_link_dollar` is set.
    pub dollar: Option<dollar::DollarState>,
    /// Stage 4-2 (spec 2.7 official part): the official Treasury block and its term-premium state, created when `world_link_finance`.
    pub fin: Option<finance::FinState>,
    /// Stage 4-3 (spec 2.8): the memo cross-border positions and their identities, created when `world_positions`.
    pub pos: Option<positions::PositionsState>,
    pub out: WorldOut,
    /// This month's `world` metrics, copied into `Metrics::world` by `metrics::record`.
    pub diag: WorldMetrics,
    /// This month's inputs and outputs (for `econsim-cli world-dump`).
    pub last: Option<Box<MonthRecord>>,
}

impl WorldBlock {
    /// A block at creation: zero states, no bases, the embedded tables and the config's coefficients.
    pub fn new(cfg: &Config) -> WorldBlock {
        let params = WorldParams::load();
        let par = Par::from_config(cfg, &params);
        let trade = TradeState::new(cfg, &params);
        let oil = if cfg.world_link_oil { Some(Box::new(OilMarket::new(cfg, &params.oil))) } else { None };
        let dollar = if cfg.world_link_dollar { Some(dollar::DollarState::new(cfg, &params, trade.tab.dollar_w)) } else { None };
        let fin = if cfg.world_link_finance { Some(finance::FinState::new(&params)) } else { None };
        let pos = if cfg.world_positions { Some(positions::PositionsState::new(cfg, &params)) } else { None };
        WorldBlock {
            pos,
            params,
            par,
            oil,
            dollar,
            fin,
            blocs: std::array::from_fn(|_| BlocState::default()),
            us: UsHist::default(),
            n: 0,
            users: Vec::new(),
            consumed: 0,
            bases: None,
            trade: Some(trade),
            out: WorldOut::default(),
            diag: WorldMetrics::default(),
            last: None,
        }
    }

    /// Registers a bloc shock that fired at bloc month `t0` (`BlocPolicy`, `BlocDemand`, `GlobalDemand`). Other shocks are
    /// ignored here.
    pub fn add_user(&mut self, shock: &Shock, t0: u32) {
        let wp = Arc::clone(&self.params);
        match shock {
            Shock::BlocPolicy { bloc, bps, months } => {
                let b = bloc_index(bloc);
                let pin = bloc::pin_vector(&wp.sol[b], *months);
                self.users.push(UserShock { bloc: b, t0, kind: UserKind::Policy { coef: bps / 100.0, pin } });
            }
            Shock::BlocDemand { bloc, pct, half_life_months, .. } => {
                let b = bloc_index(bloc);
                let scale = bloc::demand_scale(&wp.sol[b], *half_life_months, *pct);
                self.users.push(UserShock { bloc: b, t0, kind: UserKind::Demand { scale, hl: *half_life_months } });
            }
            Shock::GlobalDemand { pct, half_life_months } => {
                for b in 0..NB {
                    let scale = bloc::demand_scale(&wp.sol[b], *half_life_months, *pct);
                    self.users.push(UserShock { bloc: b, t0, kind: UserKind::Demand { scale, hl: *half_life_months } });
                }
            }
            Shock::Tariff { partner, sector, pct, retaliate } => {
                let partner = partner.as_deref().map(bloc_index);
                if let Some(tr) = self.trade.as_mut() {
                    tr.tariffs.push(TariffComp { partner, sector: *sector, tau: pct / 100.0, retaliate: *retaliate });
                }
            }
            // Stage 4-1 (spec 2.6, D31): the dollar term of a credibility shock, from its firing month on the bloc clock.
            Shock::UsCredibility { dollar_pct, months, .. } => {
                if let Some(d) = self.dollar.as_mut() {
                    d.add_cred(*dollar_pct, *months, t0);
                }
            }
            _ => {}
        }
    }

    /// One month from injected US inputs and innovations (the replay path of spec 2.1 and the parity tests). `isolated` sets
    /// F = Peff = 0 (spec 1.9). Returns the outputs (y, pi, i, z, mrr) per bloc.
    pub fn step_inputs(&mut self, us: &UsInputs, eps: Option<&[[f64; 4]; NB]>, isolated: bool) -> [[f64; 5]; NB] {
        let x = self.core(us, eps, isolated, None);
        self.finish(us, eps, &x, true, 0.0, &TradeOut::default());
        x
    }

    /// The month's computation from the inputs: the M-F bound on the rule and UIP inputs, the network terms (unless isolated),
    /// every bloc, then the US histories for the next month.
    fn core(&mut self, us: &UsInputs, eps: Option<&[[f64; 4]; NB]>, isolated: bool, gap: Option<&[[f64; 3]; NB]>) -> [[f64; 5]; NB] {
        let t = self.n;
        // The US output gap of month t-1 is unknown in month 0; the reference has F = 0 there (spec 2.3, F_{i,j} for j < 0).
        let y_eff = if t == 0 { 0.0 } else { us.y_us };
        let mut exb = [0.0; K];
        for k in 0..K {
            exb[k] = bloc::bound_pp(us.ex[k], &self.par);
        }
        let net: Option<Net> = if isolated { None } else { Some(Net::build(&self.params, &self.blocs, &self.us, y_eff, gap)) };
        let mut x = [[0.0; 5]; NB];
        for i in 0..NB {
            let e = eps.map(|e| e[i]);
            x[i] = bloc::step_bloc(i, &mut self.blocs[i], &self.params, &self.par, net.as_ref(), us, &exb, &self.us, &self.users, t, e);
        }
        // Histories for the next month: ius and the Dollar and oil levels of month t; y_US of month t-1 (index 0 is y_US(t-2)
        // at the next step, see `UsHist`).
        self.us.ius = [us.ius, self.us.ius[0], self.us.ius[1], self.us.ius[2]];
        self.us.y_us = [y_eff, self.us.y_us[0], self.us.y_us[1], self.us.y_us[2]];
        self.us.oil = [us.oil, self.us.oil[0], self.us.oil[1], self.us.oil[2]];
        let mut a = [0.0; 14];
        a[0] = us.dollar;
        a[1..].copy_from_slice(&self.us.a[..13]);
        self.us.a = a;
        x
    }

    /// Stores the outputs, captures the bases at the first reported month, fills the `world` metrics and the replay record.
    fn finish(&mut self, us: &UsInputs, eps: Option<&[[f64; 4]; NB]>, x: &[[f64; 5]; NB], reported: bool, ln_pt: f64, tout: &TradeOut) {
        if reported && self.bases.is_none() {
            let ln_p: [f64; NB] = std::array::from_fn(|i| self.blocs[i].ln_p);
            let ln_s: [f64; NB] = std::array::from_fn(|i| ln_s_level(x[i][bloc::OUT_Z], ln_p[i], ln_pt));
            self.bases = Some(Bases { ln_p, ln_s, ln_pt });
        }
        let mut d = WorldMetrics::default();
        for i in 0..NB {
            let p = &self.params.blocs[i];
            let xi = x[i];
            d.y_gap[i] = xi[bloc::OUT_Y];
            d.pi_gap[i] = xi[bloc::OUT_PI];
            d.i_gap[i] = xi[bloc::OUT_I];
            d.z[i] = xi[bloc::OUT_Z];
            d.infl[i] = p.inflation_target + xi[bloc::OUT_PI];
            d.policy[i] = (p.neutral_real_rate + p.inflation_target + xi[bloc::OUT_I]).max(p.policy_rate_floor);
            d.headline[i] = d.infl[i];
            d.u_gap[i] = self.blocs[i].u_okun;
            match &self.bases {
                Some(b) => {
                    let ln_s_now = ln_s_level(xi[bloc::OUT_Z], self.blocs[i].ln_p, ln_pt);
                    d.s_index[i] = (ln_s_now - b.ln_s[i]).exp();
                    d.p_index[i] = (self.blocs[i].ln_p - b.ln_p[i]).exp();
                }
                None => {
                    d.s_index[i] = 1.0;
                    d.p_index[i] = 1.0;
                }
            }
        }
        d.tau_eff_hh = tout.tau_eff_hh;
        d.tau_eff_firm = tout.tau_eff_firm;
        d.tau_by_origin = tout.tau_by_origin;
        d.ret_tau = tout.ret_tau;
        d.import_price_idx = 100.0 * tout.imp_rel_cpi;
        self.out = WorldOut {
            ius: us.ius,
            y_us: us.y_us,
            y_gap: d.y_gap,
            pi_gap: d.pi_gap,
            i_gap: d.i_gap,
            z: d.z,
            imp_rel: tout.imp_rel,
            imp_rel_cpi: tout.imp_rel_cpi,
            tau_eff_hh: tout.tau_eff_hh,
            tau_eff_firm: tout.tau_eff_firm,
            exports: tout.exports,
        };
        if let Some(m) = self.oil.as_deref() {
            let od = m.diag;
            d.p_bar_pct = od.p_bar;
            d.brent_usd = od.brent_usd;
            d.oil_supply_pct = od.supply;
            d.oil_demand_pct = od.demand;
            d.opec_out_mbd = od.opec_mbd;
            d.us_prod_mbd = od.us_mbd;
            d.stocks = od.stocks;
            d.spare_pct = od.spare;
            d.oil_dev_pct = od.dev;
            d.oil_us_demand_pct = od.us_demand;
        }
        self.diag = d;
        let mut rec = MonthRecord { us: us.clone(), eps: [[0.0; 4]; NB], out: [[0.0; 4]; NB] };
        if let Some(e) = eps {
            rec.eps = *e;
        }
        for i in 0..NB {
            rec.out[i] = [x[i][bloc::OUT_Y], x[i][bloc::OUT_PI], x[i][bloc::OUT_I], x[i][bloc::OUT_Z]];
        }
        self.last = Some(Box::new(rec));
        // Pushes of the US histories are done in `core`; the bloc clock advances here.
        self.n += 1;
    }

    /// The month in the ABM: user shocks activated since the last month, the US inputs, the four innovations of each bloc
    /// (drawn whether or not their scale is zero), the month, the metrics.
    fn step_month(&mut self, w: &mut World) {
        let fired: Vec<Shock> = w.shocks.world_shocks[self.consumed..].iter().map(|(s, _)| s.clone()).collect();
        for s in &fired {
            self.add_user(s, self.n);
        }
        self.consumed = w.shocks.world_shocks.len();
        let first = self.n == 0;
        let mut us = net::us_inputs(w, &mut self.us, first, &self.par);
        if let Some(m) = self.oil.as_deref() {
            us.oil = m.bloc_oil.0;
            us.qty = m.bloc_oil.1;
        }
        let eps = draws(w, &self.params, &self.par);
        let ln_pt = {
            let pt = role_mean(&w.cfg.tradable_weight, |s| w.prices.sector[s]);
            if pt > 0.0 && pt.is_finite() {
                pt.ln()
            } else {
                0.0
            }
        };
        let rep = reported(w);
        let link = w.cfg.world_link_trade;
        // D36: the trade bases are captured at the end of the last burn-in month (`WorldBlock::settle`). A world without burn-in
        // has no such month: its bases are captured at its first step (the fallback here, which also covers a missed capture).
        // No ABM state is overwritten: the ABM's FX base and its lagged logs continue from the burn-in.
        if rep {
            self.capture_bases(w, Some(&us));
        }
        // The US import-demand gap enters the network only in coupled mode (spec 2.3, D27).
        let coupled = link && rep && self.trade.as_ref().map_or(false, |t| t.par.us_import_channel && t.base.is_some());
        if let Some(tr) = self.trade.as_mut() {
            tr.pre_bloc(w);
        }
        // Stage 4-1 (spec 2.6): the dollar link acts in reported months once its base exists. Phase A sets the dollar fields of
        // the US inputs before the bloc step; phase B writes the broad rate after it. `ln_pt_prev` is the price of month t-1.
        let dollar_on = w.cfg.world_link_dollar && rep && self.dollar.as_ref().map_or(false, |d| d.base.is_some());
        let ln_pt_prev = if dollar_on { dollar::ln_pt_of(w) } else { 0.0 };
        let mut a_bil = [0.0; NB];
        if dollar_on {
            if let Some(d) = self.dollar.as_mut() {
                dollar::phase_a(d, w, &mut us, self.n);
                a_bil = d.a_bil;
            }
        }
        let gap: Option<[[f64; 3]; NB]> = if coupled { self.trade.as_ref().map(|t| t.gap_ring) } else { None };
        let x = self.core(&us, Some(&eps), false, gap.as_ref());
        if dollar_on {
            if let Some(d) = self.dollar.as_mut() {
                dollar::phase_b(d, w, &self.blocs, ln_pt_prev, self.n);
            }
        }
        let tout = match self.trade.as_mut() {
            Some(tr) => {
                // List B (rev 9): in dollar mode with `world_dollar_neutral` the pass-through reads z against its conditional forecast.
                let zref: Option<[f64; NB]> = if dollar_on && w.cfg.world_dollar_neutral {
                    self.dollar.as_ref().filter(|d| d.base.as_ref().map_or(false, |b| b.fc.is_some())).map(|d| d.z_ref(self.n))
                } else {
                    None
                };
                tr.post_bloc_ref(w, &self.blocs, if dollar_on { Some(&a_bil) } else { None }, zref.as_ref())
            }
            None => TradeOut::default(),
        };
        self.finish(&us, Some(&eps), &x, rep, ln_pt, &tout);
    }

    /// The conditional forecast of the blocs' z from the current state (spec 2.6, rev 9, D47). A light copy of the block (parameters,
    /// the twelve bloc states and the US history) is stepped `FC_MONTHS` months through `core`, coupled, with no innovation, no user
    /// shock and no dollar addend, under the US inputs the blocs themselves expect at the base: `us_b` = the inputs of the base month
    /// (`skip` = 1: the base month ran and the forecast starts at the next one) or of the first month (`skip` = 0, a world without
    /// burn-in). The policy path is the base month's expected path moved forward month by month (`ius_m = ex_b[skip + m]`,
    /// `ex_m[k] = ex_b[skip + m + k]`, zero beyond K); the activity, oil and quantity terms and the non-policy-risk term decay at the
    /// persistences the blocs project them with (`world_rho_f`, `world_rho_oil`, `world_rho_qty`, `fin_factor_rho`). Rational by
    /// construction: fed these same inputs the block reproduces the forecast exactly (test `forecast_reproduces_the_block`).
    #[inline(never)]
    pub fn forecast(&self, w: &World, us_b: &UsInputs, skip: usize) -> dollar::Forecast {
        let mut tw = WorldBlock {
            params: Arc::clone(&self.params),
            par: self.par,
            blocs: self.blocs.clone(),
            us: self.us.clone(),
            n: self.n,
            users: Vec::new(),
            consumed: 0,
            bases: None,
            trade: None,
            oil: None,
            dollar: None,
            fin: None,
            pos: None,
            out: WorldOut::default(),
            diag: WorldMetrics::default(),
            last: None,
        };
        let fex_b = self.dollar.as_ref().map_or(0.0, |d| d.f_now - w.cfg.world_vix_per_unit * crate::exog::fin_factor_mean(&w.cfg) - d.fpol);
        let mut z = Vec::with_capacity(dollar::FC_MONTHS);
        let mut us = UsInputs::zero();
        for m in 0..dollar::FC_MONTHS {
            let s = skip + m;
            for k in 0..K {
                us.ex[k] = if s + k < K { us_b.ex[s + k] } else { 0.0 };
            }
            us.ius = if s == 0 { us_b.ius } else { us.ex[0] };
            let e = s as i32;
            us.y_us = us_b.y_us * self.par.rho_f.powi(e);
            us.oil = us_b.oil * self.par.rho_oil.powi(e);
            us.qty = us_b.qty * self.par.rho_qty.powi(e);
            us.fex = fex_b * self.par.fin_rho.powi(e);
            let x = tw.core(&us, None, false, None);
            tw.n += 1;
            z.push(std::array::from_fn(|j| x[j][bloc::OUT_Z]));
        }
        dollar::Forecast { n0: self.n, z }
    }

    /// The bases of the trade and the dollar modules (D36, spec 1.6b, 2.6): captured at the end of the last burn-in month
    /// (`settle`) or at the first step of a world without burn-in. Each module captures once. No shock captures a base.
    #[inline(never)]
    pub fn capture_bases(&mut self, w: &World, us_now: Option<&UsInputs>) {
        if let Some(tr) = self.trade.as_mut() {
            if tr.base.is_none() {
                tr.capture(w, &self.blocs);
            }
        }
        // List B (rev 9, D47): a world without burn-in has no base month that ran, so its forecast starts at its first month, with the
        // inputs of that month as the expectations (index 0); the settle branch below does the same for a world with a burn-in.
        let fc = match (self.dollar.as_ref(), us_now) {
            (Some(d), Some(us)) if d.base.is_none() && w.cfg.world_dollar_neutral => Some(Arc::new(self.forecast(w, us, 0))),
            _ => None,
        };
        if let Some(d) = self.dollar.as_mut() {
            d.capture(w, &self.blocs);
            if let Some(f) = fc {
                d.set_forecast(f);
            }
        }
        if let Some(fs) = self.fin.as_mut() {
            fs.capture(w);
        }
        if let Some(ps) = self.pos.as_mut() {
            ps.capture(w, [0.0; NB], true);
        }
    }
}

/// The four standard normals of bloc `i` this month, from `agent_rng(seed, tick, WORLD_BLOC, i)`. Always drawn in this order.
fn draws_raw(w: &World, i: usize) -> [f64; 4] {
    let mut r = rng::agent_rng(w.seed, w.tick, stream::WORLD_BLOC, i as u64);
    [normal(&mut r), normal(&mut r), normal(&mut r), normal(&mut r)]
}

/// The innovations of the month for each bloc: [e_y, e_pi, e_i, eta] = sd x shock scale x standard normal (`draws_raw`).
fn draws(w: &World, wp: &WorldParams, par: &Par) -> [[f64; 4]; NB] {
    std::array::from_fn(|i| {
        let z = draws_raw(w, i);
        let sd = &wp.blocs[i].sd;
        let sc = par.shock_scale;
        // `rp_sd_mult` (world_fx_rp_sd_mult, 1.0 by default) scales the risk-premium innovation only; 1.0 is exact.
        [sd.demand * sc * z[0], sd.cost_push * sc * z[1], sd.policy * sc * z[2], sd.risk_premium * sc * z[3] * par.rp_sd_mult]
    })
}

/// The nominal level ln S_j = z/100 + ln P_j - ln PT of a bloc (spec 2.2.5, D11): foreign currency per USD, log.
pub fn ln_s_level(z_pct: f64, ln_p: f64, ln_pt: f64) -> f64 {
    z_pct / 100.0 + ln_p - ln_pt
}

/// Whether the month is part of the reported run (spec D36): not the burn-in, which is `tick_offset == 0` and
/// `tick < burn_in_months`.
pub fn reported(w: &World) -> bool {
    !(w.tick_offset == 0 && w.tick < w.cfg.burn_in_months)
}

/// Creates the block when `world_block_enabled` is set (hook H12, after `calibrate_purchases`).
pub fn init_if_enabled(w: &mut World) {
    if w.cfg.world_block_enabled {
        w.world = Some(Box::new(WorldBlock::new(&w.cfg)));
    }
}

/// After the exogenous step (spec 1.3, stage 3): the oil market clears from the previous month's bloc outputs and writes the ABM
/// oil price when the oil link acts. A no-op when the block is off or the oil link is off. Stage 4-1 (spec 2.7): the financial
/// factor's filter and the injections into f_t run after it when the dollar link is on (every month, burn-in included).
#[inline(never)]
pub fn after_exog(w: &mut World) {
    let Some(mut wb) = w.world.take() else { return };
    let rep = reported(w);
    {
        let WorldBlock { oil, out, .. } = &mut *wb;
        if let Some(m) = oil.as_deref_mut() {
            oil::apply(w, m, out, rep);
        }
    }
    if w.cfg.world_link_dollar {
        if let Some(d) = wb.dollar.as_mut() {
            finance::after_exog(w, d, rep);
        }
    }
    // Stage 4-2: the official programmes and the credibility term register before the curve of the month is priced (reported months).
    if w.cfg.world_link_finance && rep {
        if let Some(fs) = wb.fin.as_mut() {
            finance::register_shocks(w, fs);
        }
    }
    w.world = Some(wb);
}

/// Whether the oil link acts this month (spec 1.3, D36): the link is on, the block exists and the month is reported.
#[inline(never)]
pub fn oil_active(w: &World) -> bool {
    w.cfg.world_link_oil && w.world.is_some() && reported(w)
}

/// Whether the bilateral dollar link acts this month (spec 2.6, hook H2): the link is on, the block exists and the month is
/// reported. Then `monetary::exchange_rate` is skipped (the blocs' index writes `w.row.fx` in `step_month`).
#[inline(never)]
pub fn dollar_active(w: &World) -> bool {
    w.cfg.world_link_dollar && w.world.is_some() && reported(w)
}

/// Whether the trade link acts this month (spec 1.3, D36): the link is on, the block exists and the month is reported.
#[inline(never)]
pub fn trade_active(w: &World) -> bool {
    w.cfg.world_link_trade && w.world.is_some() && reported(w)
}

/// Whether the finance link acts this month (spec 1.3, D36): the link is on, the block exists and the month is reported.
#[inline(never)]
pub fn finance_active(w: &World) -> bool {
    w.cfg.world_link_finance && w.world.is_some() && reported(w)
}

/// Hook H5 (`update_curve`, stage 4-2): the finance term premium of tenor `j` (decimal), None when the finance link does not act.
#[inline(never)]
pub fn world_tp_fo(w: &World, j: usize) -> Option<f64> {
    if finance_active(w) {
        Some(finance::tp_finance(w, j))
    } else {
        None
    }
}

/// Hook H5 (`rebalance_row`, stage 4-2): the cumulative net official sales at market value (model units), None when the finance link
/// does not act.
#[inline(never)]
pub fn world_row_extra(w: &World) -> Option<f64> {
    if finance_active(w) {
        Some(finance::row_extra_deposits(w))
    } else {
        None
    }
}

/// Debug-build identities of the world block (stage 4-3, hook H22 in the `cfg!(debug_assertions)` block of `sim::step`, after
/// `ledger::check_consistency`): the NIIP identity residual and the bilateral current accounts of the month. `Ok` when the block, the
/// positions or the month do not apply.
pub fn check_identities(w: &World) -> Result<(), String> {
    let Some(wb) = w.world.as_deref() else { return Ok(()) };
    let Some(ps) = wb.pos.as_ref() else { return Ok(()) };
    if ps.base.is_none() || ps.updates == 0 || !reported(w) {
        return Ok(());
    }
    let gdp = crate::common::gdp_month_model(w).max(1e-300);
    if !(ps.resid.abs() <= 1e-9 * gdp) {
        return Err(format!("NIIP identity residual {:e} against monthly GDP {:e}", ps.resid, gdp));
    }
    let cas: f64 = ps.ca_j.iter().sum();
    if !((cas - ps.ca_total).abs() <= 1e-9 * gdp) {
        return Err(format!("bilateral current accounts sum to {:e}, total {:e}", cas, ps.ca_total));
    }
    Ok(())
}

/// Hook H19 (`ledger::service_government_debt`, stage 4-3): the coupons paid to RoW this month (a no-op unless `world_positions`).
#[inline(never)]
pub fn note_row_coupon(w: &mut World, x: f64) {
    if !w.cfg.world_positions {
        return;
    }
    if let Some(ps) = w.world.as_deref_mut().and_then(|wb| wb.pos.as_mut()) {
        ps.cpn_month += x;
    }
}

/// Hook H20 (`banking::service_deposits`, stage 4-3): the deposit interest paid to RoW this month (a no-op unless `world_positions`).
#[inline(never)]
pub fn note_row_dep_interest(w: &mut World, x: f64) {
    if !w.cfg.world_positions {
        return;
    }
    if let Some(ps) = w.world.as_deref_mut().and_then(|wb| wb.pos.as_mut()) {
        ps.dep_int_month += x;
    }
}

/// Hook H21 (`fiscal::auction_bonds`, stage 4-3): RoW's purchase of `face` of the new bond `id` for `paid` cash (a no-op unless `world_positions`).
#[inline(never)]
pub fn note_row_primary(w: &mut World, id: crate::types::BondId, face: f64, paid: f64) {
    if !w.cfg.world_positions {
        return;
    }
    if let Some(ps) = w.world.as_deref_mut().and_then(|wb| wb.pos.as_mut()) {
        ps.prim_month.push((id, face, paid));
    }
}

/// The duty channels of the b7 read sites (spec H8): household (retail wedge, CPI wedge) and firm (inputs, equipment).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Hh,
    Firm,
}

/// The statutory duty of a read site (spec H8): the block's effective duty when `trade_active`, otherwise the literal b7
/// expression of the site (the set-level `ImportTariff` level, clamped at zero).
#[inline(never)]
pub fn tau_for(w: &World, ch: Channel) -> f64 {
    if trade_active(w) {
        if let Some(wb) = w.world.as_deref() {
            return match ch {
                Channel::Hh => wb.out.tau_eff_hh,
                Channel::Firm => wb.out.tau_eff_firm,
            };
        }
    }
    finite_or(w.shocks.tariff, 0.0).max(0.0)
}

#[inline]
fn finite_or(x: f64, default: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

/// One month of the block (hook H2, after `monetary::exchange_rate`). A no-op when the block is off.
#[inline(never)]
pub fn step(w: &mut World) {
    let Some(mut wb) = w.world.take() else { return };
    wb.step_month(w);
    // Stage 4-2 (spec 2.7 official part, section 3.4): the official block's flows of the month, with the dollar of the month.
    if w.cfg.world_link_finance {
        let rep = reported(w);
        let fx0 = wb.trade.as_ref().and_then(|t| t.base.as_ref()).map(|b| b.fx0);
        if let Some(fs) = wb.fin.as_mut() {
            finance::step_flows(w, fs, fx0, rep);
        }
    }
    w.world = Some(wb);
}

/// The settlement of the month's realised flows (hook in `firms.close`, after `goods::clear`): the bilateral trade memo and
/// the state of the trade block for the next month. Writes no balance. A no-op when the block is off.
#[inline(never)]
pub fn settle(w: &mut World) {
    let Some(mut wb) = w.world.take() else { return };
    wb.settle(w);
    w.world = Some(wb);
}

/// Today's start (docs/CALIBRATION.md, section world-usnow): at the end of the steered window, install the dated reference of the trade block: today's duty
/// by origin and goods group, import origin shares, export destination shares and the partners' retaliatory duties. A no-op when the
/// block or the trade link is off or the vintage is unknown. Returns whether the reference was installed.
pub fn rebase_usnow(w: &mut World, id: &str) -> bool {
    if !w.cfg.world_link_trade {
        return false;
    }
    let Some(v) = usnow::vintage(id) else { return false };
    let lvl = finite_or(w.shocks.tariff, 0.0).max(0.0);
    let Some(wb) = w.world.as_deref_mut() else { return false };
    match wb.trade.as_mut() {
        Some(tr) => {
            tr.rebase_today(v, lvl);
            true
        }
        None => false,
    }
}

impl WorldBlock {
    fn settle(&mut self, w: &World) {
        if let Some(m) = self.oil.as_deref_mut() {
            oil::settle(w, m);
        }
        // Stage 4-1 (spec 1.6b, 2.6): the bases of this module and of the dollar are captured at the same month.
        let base_month = w.tick_offset == 0 && w.cfg.burn_in_months > 0 && w.tick + 1 == w.cfg.burn_in_months;
        // List B (rev 9, D47): the conditional forecast from the base month, made from the inputs of the month that just ran.
        let fc = match (base_month, self.dollar.as_ref(), self.last.as_deref()) {
            (true, Some(d), Some(rec)) if d.base.is_none() && w.cfg.world_dollar_neutral => Some(Arc::new(self.forecast(w, &rec.us, 1))),
            _ => None,
        };
        let Some(tr) = self.trade.as_mut() else { return };
        tr.settle(w);
        // D36: the trade bases are the ABM's own values at the end of the last burn-in month (after its settlement), so the
        // first reported month starts from the burn-in state and a shock at reported month 0 is measured against it.
        if tr.base.is_none() && base_month {
            tr.capture(w, &self.blocs);
        }
        // Stage 4-3: positions capture at the same month and update in reported months; the ledger interest accumulators are reset.
        let dollar_on = w.cfg.world_link_dollar && reported(w) && self.dollar.as_ref().map_or(false, |d| d.base.is_some());
        let ln_s_now = if dollar_on { self.dollar.as_ref().map(|d| d.ln_s_now) } else { None };
        if let Some(ps) = self.pos.as_mut() {
            if base_month {
                ps.capture(w, [0.0; NB], false);
            } else if reported(w) {
                ps.update(w, &tr.exp_bloc, &tr.imp_bloc, ln_s_now);
            } else {
                ps.cpn_month = 0.0;
                ps.dep_int_month = 0.0;
                ps.prim_month.clear();
            }
        }
        if base_month {
            if let Some(dl) = self.dollar.as_mut() {
                dl.capture(w, &self.blocs);
                if let Some(f) = fc {
                    dl.set_forecast(f);
                }
            }
            // Stage 4-2 (spec 1.6b, 2.7): the official tranche is captured here too (a world with a burn-in captures at this month, not at
            // its first reported step: PREFLIGHT 2.3).
            if let Some(fs) = self.fin.as_mut() {
                fs.capture(w);
            }
        }
        let scale = w.cfg.effective_scale();
        let ann = 12.0 * scale / 1e9;
        let d = &mut self.diag;
        for j in 0..NB {
            d.exports_bn[j] = tr.exp_bloc[j] * ann;
            d.imports_bn[j] = tr.imp_bloc[j] * ann;
            d.r_import[j] = 100.0 * tr.r[j];
            d.us_import_gap[j] = tr.gap_ring[j][0];
            d.z_tilde[j] = tr.z_tilde[j];
        }
        d.c_t = tr.c_t;
        d.import_volume_idx = tr.imp_vol_idx;
        d.export_volume_idx = tr.exp_vol_idx;
        d.exports_goods_bn = std::array::from_fn(|j| tr.exp_goods[j] * ann);
        if tr.usnow.is_some() {
            d.imports_goods_bn = std::array::from_fn(|j| tr.imp_goods[j] * ann);
        }
        if let Some(ps) = self.pos.as_ref() {
            let gdp = crate::common::gdp_month_model(w);
            if ps.base.is_some() && gdp > 0.0 {
                d.niip_pct_gdp = 100.0 * ps.niip / (12.0 * gdp);
                d.ca_pct_gdp = 100.0 * ps.ca_total / gdp;
                d.sfc_world_error = ps.resid.abs() / gdp;
                d.ca_bilateral_bn = std::array::from_fn(|j| ps.ca_j[j] * ann);
            }
        }
        // Stage 4-1 diagnostics (`fx_broad` with the link on or off; the dollar fields once the dollar state has a base).
        d.fx_broad = match tr.base.as_ref() {
            Some(b) => 100.0 * w.row.fx / b.fx0,
            None => 100.0,
        };
        if let Some(dl) = self.dollar.as_ref() {
            if let Some(b) = dl.base.as_ref() {
                d.f_vix = dl.f_now;
                d.f_risk = dl.frisk - b.frisk0;
                d.a_dollar = 100.0 * dl.a_common;
                d.s_bil = std::array::from_fn(|j| dl.ln_s_now[j].exp());
                d.z_fc = dl.z_ref_now;
            }
        }
        // Stage 4-2 diagnostics (spec 3.6): zero when the finance link is off (no state).
        if let Some(fs) = self.fin.as_ref() {
            d.fo_bn = fs.fo_rw;
            d.fo_flow_bn = fs.flow_rw;
            d.tp_fo_bp = fs.tp10_fo_bp;
            d.tp_cred_bp = fs.tp10_cred_bp;
            d.fo_extra = fs.extra;
            d.f_ring_mean = fs.ring.iter().sum::<f64>() / 6.0;
            let gdp_ann = 12.0 * crate::common::gdp_month_model(w);
            if gdp_ann > 0.0 {
                d.row_mv_gdp_pct = 100.0 * w.row.bonds_value / gdp_ann;
                d.row_dep_gdp_pct = 100.0 * w.row.deposits / gdp_ann;
            }
            if let Some(b) = fs.base.as_ref() {
                if w.row.bonds_value > 0.0 {
                    d.fo_share_row_pct = 100.0 * fs.fo_rw * b.k_fo / w.row.bonds_value;
                }
            }
            let face_row: f64 = w.bonds.iter().map(|bd| bd.held_by(crate::types::BondHolder::RestOfWorld)).sum();
            if w.gov.debt_face > 0.0 {
                d.row_face_share_pct = 100.0 * face_row / w.gov.debt_face;
            }
        }
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_trade;
#[cfg(test)]
mod tests_oil;
#[cfg(test)]
mod tests_dollar;
#[cfg(test)]
mod tests_dollar_rv;
#[cfg(test)]
mod tests_listb;
#[cfg(test)]
mod tests_listb_rv;
#[cfg(test)]
mod tests_finance;
#[cfg(test)]
mod tests_finance_rv;
#[cfg(test)]
mod tests_ret_pos;
#[cfg(test)]
mod tests_s5;
#[cfg(test)]
mod tests_t2;
#[cfg(test)]
mod tests_usnow;
#[cfg(test)]
mod tests_ret_pos_rv;
