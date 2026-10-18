//! Stage 4-1: the financial factor of the world block (spec 2.7, 2.6): the injections of the policy surprise, `RiskOff` and the
//! China risk-off demand into the ABM's factor `f_t`, the policy part Fpol, the non-policy part Fex (read in phase A), and the
//! risk factor Frisk (asymmetric AR(1) filter of the innovations of F). Runs every month in `world::after_exog` while the dollar
//! link is on (burn-in included; the injections only in reported months).
//!
//! Stage 4-2 (below): the official Treasury block (spec 2.7 official part, D19, D39, D43-D46). Its functions run only with
//! `world_link_finance`.

use super::dollar::DollarState;
use super::{bloc_index, params::WorldParams, NB};
use crate::monetary;
use crate::shocks::Shock;
use crate::types::{BankId, BondHolder, World};

/// One month of the factor filter (spec 2.7 and 2.6), after the ABM's `exog::step`.
///
/// 1. Reported months only: the `PolicyShift` activated this month adds (VIX per 100 bp / VIX per unit)(bps/100) to `f` and
///    VIX per 100 bp (bps/100) to this month's policy injection; the `RiskOff` and `BlocDemand{risk_off}` entries fired since
///    the last injection add `vix_points / VIX per unit` and `china_risk_pts_per_pct |pct| / VIX per unit`.
/// 2. `F_t = VIX per unit f_t`, `Fpol_t = rho Fpol_{t-1} + injection`, `e = F_t - rho F_{t-1}`,
///    `eps = e` if `e > 0` else `asym e`, `Frisk_t = rho_risk Frisk_{t-1} + eps`.
#[inline(never)]
pub fn after_exog(w: &mut World, d: &mut DollarState, rep: bool) {
    let unit = w.cfg.world_vix_per_unit;
    let per_100bp = w.cfg.world_vix_per_100bp;
    let rho = w.cfg.fin_factor_rho;
    let mut inj_pts = 0.0;
    if rep {
        if let Some((b, t0, _)) = w.shocks.policy_shift {
            if t0 == w.tick {
                let bps = b * 1e4;
                w.exog.fin_factor += (per_100bp / unit) * (bps / 100.0);
                inj_pts += per_100bp * (bps / 100.0);
            }
        }
        let fired: Vec<Shock> = w.shocks.world_shocks[d.fin_consumed..].iter().map(|(s, _)| s.clone()).collect();
        for s in &fired {
            match s {
                Shock::RiskOff { vix_points } => w.exog.fin_factor += vix_points / unit,
                Shock::BlocDemand { pct, risk_off: true, .. } => {
                    w.exog.fin_factor += w.cfg.world_china_risk_pts_per_pct * pct.abs() / unit;
                }
                _ => {}
            }
        }
        d.fin_consumed = w.shocks.world_shocks.len();
    }
    let f = unit * w.exog.fin_factor;
    d.fpol = rho * d.fpol + inj_pts;
    d.frisk = frisk_step(d.frisk, d.f_prev, f, rho, w.cfg.world_fx_risk_asym, w.cfg.world_fx_risk_rho);
    d.f_prev = f;
    d.f_now = f;
}

/// The risk-factor filter (spec 2.6): e = F_t - rho F_{t-1}; eps = e if e > 0, else asym x e; Frisk_t = risk_rho Frisk_{t-1} + eps.
/// All in VIX points. Pure, so the tests drive the same code that `after_exog` runs.
pub fn frisk_step(frisk_prev: f64, f_prev: f64, f: f64, rho: f64, asym: f64, risk_rho: f64) -> f64 {
    let e = f - rho * f_prev;
    let eps = if e > 0.0 { e } else { asym * e };
    risk_rho * frisk_prev + eps
}

// ---------------------------------------------------------------------------------------------------------------------------
// Stage 4-2 (spec 2.7 official part, D19, D39, D43-D46). All official quantities are real-world-equivalent ($bn rw) unless
// marked "model"; `k_FO` converts rw to model units. NB = 12 blocs in solution order (CN is index 1).
// ---------------------------------------------------------------------------------------------------------------------------

/// An `OfficialTreasury` programme: `per_month` real-world $bn a month from absolute tick `t0` for `months` months.
#[derive(Clone, Debug)]
pub struct OfficialProg {
    pub bloc: Option<usize>,
    pub per_month: f64,
    pub t0: u32,
    pub months: u32,
}

/// The term-premium part of a `UsCredibility` shock: `tp_bp` basis points at the 10y with the profile of `dollar::cred_profile`.
#[derive(Clone, Debug)]
pub struct TpCred {
    pub tp_bp: f64,
    pub months: u32,
    pub t0: u32,
}

/// The base of the official block, captured at the end of the last burn-in month (spec 1.6b).
#[derive(Clone, Debug)]
pub struct FinBase {
    /// Trend GDP (model, monthly) of the base month: the reference of the memo growth and of `GDP^rw`.
    pub tau_b: f64,
    /// H_j,b in model units (market value): `nu x official_share_j x tic_share_j x RoW_MV_b`.
    pub h_b: [f64; NB],
    /// k_FO = FO_b / 3773.1: model units per real-world $bn.
    pub k_fo: f64,
    /// RoW's bond market value and deposits at the base (model units; reported by the diagnostics).
    pub row_mv_b: f64,
    pub row_dep_b: f64,
}

/// The state of the official Treasury block (spec 1.2, 2.7 official part). Created with the block when `world_link_finance` is set.
#[derive(Clone, Debug)]
pub struct FinState {
    pub base: Option<FinBase>,
    /// `nu x official_share_j x tic_j / sum tic`: the fraction of RoW's market value that is bloc j's official tranche.
    pub tranche_frac: [f64; NB],
    /// The sourced official total, real-world $bn (finance `foreign_official_ust_bn`).
    pub fo_total_rw: f64,
    /// Deviation of H_j from its memo path `h_b tau_t / tau_b` (model units): exactly 0.0 until a flow executes.
    pub dh: [f64; NB],
    /// T_t (% of annual GDP^rw): the transitory term.
    pub t_trans: f64,
    /// 100 (sum dH / k_FO) / GDP^rw_t after the last step: the stock deviation in % of GDP.
    pub stock_pp: f64,
    /// ln D_{t-1} (zero at the base).
    pub ln_d_prev: f64,
    /// Trend GDP read at the previous `step_flows` (the base value before the first reported step).
    pub tau_prev: f64,
    /// The last six values of F - Fbar (VIX points); filled every month from the ABM's f_t, burn-in included.
    pub ring: [f64; 6],
    /// Cumulative net official SALES at market value (model units): minus the sum of executed purchases.
    pub extra: f64,
    pub progs: Vec<OfficialProg>,
    pub creds: Vec<TpCred>,
    /// Index into `ShockState::world_shocks` of the first entry not yet registered.
    pub consumed: usize,
    // outputs of the month (metrics)
    pub fo_rw: f64,
    pub flow_rw: f64,
    pub tp10_fo_bp: f64,
    pub tp10_cred_bp: f64,
}

impl FinState {
    /// The tranche of each bloc from the sourced tables (spec 1.7, 2.7): `nu = foreign_official_ust_bn / sum_j official_share_j
    /// tic_j` and `tranche_frac_j = nu official_share_j tic_j / sum_k tic_k`.
    pub fn new(wp: &WorldParams) -> FinState {
        let f = &wp.finance;
        let sum_tic: f64 = f.tic_ust_bn.iter().sum();
        let s: f64 = f.tic_ust_bn.iter().zip(&f.official_share).map(|(t, o)| t * o).sum();
        let nu = if s > 0.0 { f.foreign_official_ust_bn / s } else { 0.0 };
        let tranche_frac: [f64; NB] = std::array::from_fn(|j| {
            if sum_tic > 0.0 {
                nu * f.official_share.get(j).copied().unwrap_or(0.0) * f.tic_ust_bn.get(j).copied().unwrap_or(0.0) / sum_tic
            } else {
                0.0
            }
        });
        FinState {
            base: None,
            tranche_frac,
            fo_total_rw: f.foreign_official_ust_bn,
            dh: [0.0; NB],
            t_trans: 0.0,
            stock_pp: 0.0,
            ln_d_prev: 0.0,
            tau_prev: 0.0,
            ring: [0.0; 6],
            extra: 0.0,
            progs: Vec::new(),
            creds: Vec::new(),
            consumed: 0,
            fo_rw: 0.0,
            flow_rw: 0.0,
            tp10_fo_bp: 0.0,
            tp10_cred_bp: 0.0,
        }
    }

    /// Captures the tranche at the end of the last burn-in month (spec 1.6b, 2.7; `settle` and `capture_bases`). Once: a no-op when
    /// `base` exists. No shock captures. The base is left None unless RoW's market value and the trend are finite and positive.
    #[inline(never)]
    pub fn capture(&mut self, w: &World) {
        if self.base.is_some() || !(self.fo_total_rw > 0.0) {
            return;
        }
        let row_mv = w.row.bonds_value;
        let tau = crate::fiscal::trend_gdp_month(w);
        if !(row_mv > 0.0 && row_mv.is_finite() && tau > 0.0 && tau.is_finite()) {
            return;
        }
        let h_b: [f64; NB] = std::array::from_fn(|j| self.tranche_frac[j] * row_mv);
        let fo_b: f64 = h_b.iter().sum();
        let k_fo = fo_b / self.fo_total_rw;
        self.base = Some(FinBase { tau_b: tau, h_b, k_fo, row_mv_b: row_mv, row_dep_b: w.row.deposits });
        self.tau_prev = tau;
        self.fo_rw = self.fo_total_rw;
    }

    /// The official part of the term premium at tenor `j` (decimal), the state of the previous month: 0.0 bit for bit until a flow
    /// has executed (spec 2.7, D44): `-TP_SLOPE[j] (theta_stock/1e4 stock + theta_trans/1e4 T)`.
    pub fn tp_fo(&self, j: usize, theta_stock: f64, theta_trans: f64) -> f64 {
        if self.stock_pp == 0.0 && self.t_trans == 0.0 {
            return 0.0;
        }
        -monetary::TP_SLOPE[j] * (theta_stock / 1e4 * self.stock_pp + theta_trans / 1e4 * self.t_trans)
    }

    /// The credibility part at tenor `j` for tick `tick` (decimal): `sum_c tp_bp/1e4 p(age) TP_SLOPE[j] / TP_SLOPE[3]` (D46: the 10y
    /// exact, the 3m zero); 0.0 when no component is active.
    pub fn tp_cred(&self, j: usize, tick: u32) -> f64 {
        let mut s = 0.0;
        for c in &self.creds {
            if tick >= c.t0 {
                s += c.tp_bp / 1e4 * super::dollar::cred_profile(tick - c.t0, c.months);
            }
        }
        if s == 0.0 {
            0.0
        } else {
            s * monetary::TP_SLOPE[j] / monetary::TP_SLOPE[3]
        }
    }
}

/// `H_j` of a month in model units along the memo path plus the deviation: `h_b tau / tau_b + dh` (spec 2.7, D44).
#[inline]
fn memo_plus_dev(h_b: f64, tau: f64, tau_b: f64, dh: f64) -> f64 {
    h_b * tau / tau_b + dh
}

/// Registers the finance shocks fired since the last call (`OfficialTreasury`, the term-premium part of `UsCredibility`), in
/// `world::after_exog`, before the month's curve is priced (a credibility shock moves the curve of its own month). Reported
/// months only. The dollar part of `UsCredibility` is 4-1's and is not touched here.
#[inline(never)]
pub fn register_shocks(w: &World, fs: &mut FinState) {
    for (s, tick) in w.shocks.world_shocks[fs.consumed..].iter() {
        match s {
            Shock::OfficialTreasury { bloc, bn, months } => {
                fs.progs.push(OfficialProg {
                    bloc: bloc.as_deref().map(bloc_index),
                    per_month: bn / f64::from(*months),
                    t0: *tick,
                    months: *months,
                });
            }
            Shock::UsCredibility { tp_bp, months, .. } => fs.creds.push(TpCred { tp_bp: *tp_bp, months: *months, t0: *tick }),
            _ => {}
        }
    }
    fs.consumed = w.shocks.world_shocks.len();
}

/// The finance term premium at tenor `j` for this month's curve, decimal (hook H5 through `world::world_tp_fo`): the official part
/// plus the credibility part. 0.0 when the block has no finance state.
#[inline(never)]
pub fn tp_finance(w: &World, j: usize) -> f64 {
    let Some(wb) = w.world.as_deref() else { return 0.0 };
    let Some(fs) = wb.fin.as_ref() else { return 0.0 };
    fs.tp_fo(j, w.cfg.world_fo_theta_stock, w.cfg.world_fo_theta_trans) + fs.tp_cred(j, w.tick)
}

/// The cumulative net official sales at market value, model units (hook H5 of `rebalance_row`, through `world::world_row_extra`).
#[inline(never)]
pub fn row_extra_deposits(w: &World) -> f64 {
    w.world.as_deref().and_then(|wb| wb.fin.as_ref()).map_or(0.0, |fs| fs.extra)
}

/// One month of the official block, in `world::step` after `step_month` (the block is out of `w` there, so its state is an argument).
/// `fx0` is the broad dollar of the base month and `rep` says whether the month is reported. The F ring is filled every month, burn-in
/// included; the flows, the stock and the transitory term only in reported months (spec 3.4, D44, D45).
#[inline(never)]
pub fn step_flows(w: &mut World, fs: &mut FinState, fx0: Option<f64>, rep: bool) {
    // 1. The F ring: this month's F - Fbar in VIX points, the six most recent months (this month included).
    let unit = w.cfg.world_vix_per_unit;
    let fbar = unit * crate::exog::fin_factor_mean(&w.cfg);
    fs.ring.rotate_left(1);
    fs.ring[5] = unit * w.exog.fin_factor - fbar;
    if !rep {
        return;
    }
    // 2. The base and the trade base must exist, and the trend must be a positive number.
    let Some(fx0) = fx0 else { return };
    let (h_b, k_fo, tau_b) = match fs.base.as_ref() {
        Some(b) => (b.h_b, b.k_fo, b.tau_b),
        None => return,
    };
    let tau = crate::fiscal::trend_gdp_month(w);
    if !(tau > 0.0 && tau.is_finite()) {
        return;
    }
    let (theta_s, theta_t) = (w.cfg.world_fo_theta_stock, w.cfg.world_fo_theta_trans);
    let (rho_t, eta, vix_resp, trend_cn) =
        (w.cfg.world_fo_trans_rho, w.cfg.world_fo_dollar_resp, w.cfg.world_fo_vix_resp, w.cfg.world_fo_trend_cn);
    let gdp_rw_total = w.cfg.world_gdp_rw_bn;
    // 3. The terms in force in this month's curve (the state of the previous month), for the metrics.
    fs.tp10_fo_bp = 1e4 * fs.tp_fo(3, theta_s, theta_t);
    fs.tp10_cred_bp = 1e4 * fs.tp_cred(3, w.tick);
    // 4. H_j of the previous month, in real-world $bn.
    let h_rw: [f64; NB] = std::array::from_fn(|j| memo_plus_dev(h_b[j], fs.tau_prev, tau_b, fs.dh[j]) / k_fo);
    let fo_prev: f64 = h_rw.iter().sum();
    // 5. ln D_t against the base (the link's index when the dollar link is on; the ABM's own rate otherwise).
    let ln_d = if w.row.fx.is_finite() && w.row.fx > 0.0 && fx0 > 0.0 { (w.row.fx / fx0).ln() } else { fs.ln_d_prev };
    let d_ln = 100.0 * (ln_d - fs.ln_d_prev);
    let mean6: f64 = fs.ring.iter().sum::<f64>() / 6.0;
    // 6. The bloc flows in real-world $bn: the dollar, the VIX and the trend terms, the programmes, then the per-bloc cap.
    let mut n_rw = [0.0; NB];
    for j in 0..NB {
        let hj = h_rw[j];
        let mut n = eta * d_ln * hj;
        if fo_prev > 0.0 {
            n += vix_resp * mean6 * hj / fo_prev;
        }
        if j == 1 {
            n += trend_cn / 12.0;
        }
        for p in &fs.progs {
            if w.tick >= p.t0 && w.tick - p.t0 < p.months {
                match p.bloc {
                    Some(b) if b == j => n += p.per_month,
                    Some(_) => {}
                    None => {
                        if fo_prev > 0.0 {
                            n += p.per_month * hj / fo_prev;
                        }
                    }
                }
            }
        }
        n_rw[j] = n.max(-hj); // a bloc cannot sell more than it holds (D44)
    }
    // 7. Execution (model units): purchases from the banks, then the Fund last (D45); sales to the banks.
    let req_model = n_rw.iter().sum::<f64>() * k_fo;
    let mut exec_signed = 0.0; // model units, + = net purchase executed
    if req_model > monetary::EPS {
        let mut cps: Vec<BondHolder> = (0..w.banks.len()).map(|b| BondHolder::Bank(b as BankId)).collect();
        cps.push(BondHolder::Fund);
        exec_signed = monetary::trade_with(w, BondHolder::RestOfWorld, &cps, req_model, true);
    } else if req_model < -monetary::EPS {
        exec_signed = -monetary::trade_with_banks(w, BondHolder::RestOfWorld, -req_model, false);
    }
    let share = if req_model.abs() > monetary::EPS { exec_signed / req_model } else { 0.0 };
    // 8. The state after the month.
    let growth = tau / fs.tau_prev;
    for j in 0..NB {
        fs.dh[j] = fs.dh[j] * growth + n_rw[j] * k_fo * share;
    }
    fs.extra -= exec_signed;
    let gdp_rw = gdp_rw_total * tau / tau_b;
    let exec_rw = exec_signed / k_fo;
    fs.t_trans = rho_t * fs.t_trans + 100.0 * exec_rw / gdp_rw;
    fs.stock_pp = 100.0 * (fs.dh.iter().sum::<f64>() / k_fo) / gdp_rw;
    fs.tau_prev = tau;
    fs.ln_d_prev = ln_d;
    fs.fo_rw = (0..NB).map(|j| memo_plus_dev(h_b[j], tau, tau_b, fs.dh[j])).sum::<f64>() / k_fo;
    fs.flow_rw = exec_rw;
}
