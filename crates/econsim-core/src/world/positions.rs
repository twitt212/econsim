//! Stage 4-3 (spec 2.8, D22, D40): memo cross-border positions and the stock-flow identities.
//!
//! Nine stocks by bloc. Two are ledger-backed (RoW's Treasuries at market value and RoW's deposits, split by fixed bloc shares);
//! seven are memo (assets: equity, debt, FDI, bank claims; liabilities: agency and corporate debt, equity, FDI), set at the base month
//! from the research tables times `k_GDP,b` and then updated by reinvested income and valuation. Nothing here changes an ABM quantity.
//!
//! Identity (spec 2.8): `NIIP_t - NIIP_{t-1} = CA_ledger + PI_memo + Val`, with
//!   CA_ledger = (exports - imports) - (coupons + deposit interest paid to RoW),
//!   PI_memo   = sum of the income of the memo assets less the income of the memo liabilities,
//!   Val       = valuation of the memo stocks - valuation of RoW's Treasuries at constant face (price change of the bonds held at t-1,
//!               a bond that matured is priced at par).
//! The coupon and the deposit interest are the amounts the ledger actually paid (hooks H19 and H20 accumulate them); every other term
//! is computed from states the ledger does not use in the same step.

use super::params::WorldParams;
use super::NB;
use crate::config::Config;
use crate::rng::{self, normal, stream};
use crate::types::{BondHolder, BondId, World};

/// Index offset of the noise draws on `WORLD_FX` (the dollar module's zeta uses 0..NB): `(seed, tick, WORLD_FX, POS_NOISE_BASE + j)`.
pub const POS_NOISE_BASE: u64 = 16;
/// Standard deviation of the equity valuation noise eta (spec 2.8, [assumed]).
pub const ETA_SD: f64 = 0.02;

/// Constants of the base capture and the memo stocks (bloc order).
#[derive(Clone, Debug)]
pub struct PosBase {
    /// k_GDP,b = 12 tau_b / world_gdp_rw_bn (model units per real-world $bn).
    pub k: f64,
    pub tau_b: f64,
}

/// Output of one memo step: primary income by bloc and in total, and the valuation (assets less liabilities), model units.
#[derive(Clone, Copy, Debug)]
pub struct MemoOut {
    pub pi: [f64; NB],
    pub pi_total: f64,
    pub val: f64,
}

#[derive(Clone, Debug)]
pub struct PositionsState {
    pub base: Option<PosBase>,
    /// True when the base was captured at the first step of a world without burn-in (`capture_bases`): the first update has no
    /// previous-month marks of the bonds, so its identity residual is not meaningful and is set to zero.
    pub fallback: bool,
    pub fc_eq: f64,
    pub fc_debt: f64,
    pub fc_fdi: f64,
    pub fc_bank: f64,
    pub fc_liab: f64,
    pub y_a: f64,
    pub y_l: f64,
    pub load: [f64; NB],
    pub tic_share: [f64; NB],
    pub dep_share: [f64; NB],
    /// Memo stocks (model units): assets equity, debt, FDI, bank claims; liabilities agency+corporate, equity, FDI.
    pub a_eq: [f64; NB],
    pub a_debt: [f64; NB],
    pub a_fdi: [f64; NB],
    pub a_bank: [f64; NB],
    pub l_ag: [f64; NB],
    pub l_eq: [f64; NB],
    pub l_fdi: [f64; NB],
    /// Previous month's values for the increments.
    pub ln_fx_prev: f64,
    pub ln_s_prev: [f64; NB],
    pub sp_prev: f64,
    pub niip_prev: f64,
    /// RoW's bonds at the end of the previous month: (id, price, face), in bond order.
    pub bonds_prev: Vec<(BondId, f64, f64)>,
    /// Accumulators of the ledger interest paid to RoW this month (hooks H19, H20); reset in `settle`.
    pub cpn_month: f64,
    pub dep_int_month: f64,
    /// RoW's purchases at the auction this month: (bond id, face, cash paid) (hook H21).
    pub prim_month: Vec<(BondId, f64, f64)>,
    // --- this month's outputs ---
    pub niip: f64,
    pub ca_total: f64,
    pub pi_memo: f64,
    pub val_memo: f64,
    pub val_hold: f64,
    pub resid: f64,
    pub ca_j: [f64; NB],
    /// This month's valuation inputs (diagnostics for the tests): the change of ln S_j used and the US equity return.
    pub ds: [f64; NB],
    pub r_us: f64,
    pub updates: u32,
}

impl PositionsState {
    pub fn new(cfg: &Config, wp: &WorldParams) -> PositionsState {
        let _ = cfg;
        let t = &wp.finance.positions;
        let arr = |v: &Vec<f64>| -> [f64; NB] { std::array::from_fn(|j| if v.len() == NB { v[j] } else { 0.0 }) };
        PositionsState {
            base: None,
            fallback: false,
            fc_eq: t.fc_eq,
            fc_debt: t.fc_debt,
            fc_fdi: t.fc_fdi,
            fc_bank: t.fc_bank,
            fc_liab: t.fc_liab,
            y_a: t.yield_assets_pct / 100.0,
            y_l: t.yield_liab_pct / 100.0,
            load: arr(&t.equity_loading),
            tic_share: arr(&t.tic_share),
            dep_share: arr(&t.dep_share),
            a_eq: arr(&t.a_eq),
            a_debt: arr(&t.a_debt),
            a_fdi: arr(&t.a_fdi),
            a_bank: arr(&t.a_bank),
            l_ag: arr(&t.l_agcorp),
            l_eq: arr(&t.l_eq),
            l_fdi: arr(&t.l_fdi),
            ln_fx_prev: 0.0,
            ln_s_prev: [0.0; NB],
            sp_prev: 0.0,
            niip_prev: 0.0,
            bonds_prev: Vec::new(),
            cpn_month: 0.0,
            dep_int_month: 0.0,
            prim_month: Vec::new(),
            niip: 0.0,
            ca_total: 0.0,
            pi_memo: 0.0,
            val_memo: 0.0,
            val_hold: 0.0,
            resid: 0.0,
            ca_j: [0.0; NB],
            ds: [0.0; NB],
            r_us: 0.0,
            updates: 0,
        }
    }

    /// Coupons and deposit interest paid to RoW, allocated to the blocs: `cpn tic_share[j] + dep dep_share[j]` (pure).
    pub fn ledger_interest_by_bloc(&self, cpn: f64, dep: f64) -> [f64; NB] {
        std::array::from_fn(|j| cpn * self.tic_share[j] + dep * self.dep_share[j])
    }

    pub fn memo_assets(&self) -> f64 {
        (0..NB).map(|j| self.a_eq[j] + self.a_debt[j] + self.a_fdi[j] + self.a_bank[j]).sum()
    }
    pub fn memo_liabs(&self) -> f64 {
        (0..NB).map(|j| self.l_ag[j] + self.l_eq[j] + self.l_fdi[j]).sum()
    }

    /// The pure memo step (spec 2.8): `X_t = X_{t-1} (1 + v + y/12)` for the seven memo stocks, with `v` the valuation rate of each stock
    /// from `ds` (the change of ln S_j, foreign currency per dollar), `r_us` (the US equity return) and `eta` (the equity noise). Returns the
    /// primary income by bloc, its total and the valuation (assets less liabilities) of the month, in model units.
    pub fn memo_step(&mut self, ds: &[f64; NB], r_us: f64, eta: &[f64; NB]) -> MemoOut {
        let (ya, yl) = (self.y_a / 12.0, self.y_l / 12.0);
        let mut out = MemoOut { pi: [0.0; NB], pi_total: 0.0, val: 0.0 };
        for j in 0..NB {
            let v_eq = self.load[j] * r_us + eta[j] - self.fc_eq * ds[j];
            let v_debt = -self.fc_debt * ds[j];
            let v_fdi = -self.fc_fdi * ds[j];
            let v_bank = -self.fc_bank * ds[j];
            let v_ag = -self.fc_liab * ds[j];
            let (v_leq, v_lfdi) = (r_us, r_us);
            let inc_a = ya * (self.a_eq[j] + self.a_debt[j] + self.a_fdi[j] + self.a_bank[j]);
            let inc_l = yl * (self.l_ag[j] + self.l_eq[j] + self.l_fdi[j]);
            let val_a = v_eq * self.a_eq[j] + v_debt * self.a_debt[j] + v_fdi * self.a_fdi[j] + v_bank * self.a_bank[j];
            let val_l = v_ag * self.l_ag[j] + v_leq * self.l_eq[j] + v_lfdi * self.l_fdi[j];
            self.a_eq[j] *= 1.0 + v_eq + ya;
            self.a_debt[j] *= 1.0 + v_debt + ya;
            self.a_fdi[j] *= 1.0 + v_fdi + ya;
            self.a_bank[j] *= 1.0 + v_bank + ya;
            self.l_ag[j] *= 1.0 + v_ag + yl;
            self.l_eq[j] *= 1.0 + v_leq + yl;
            self.l_fdi[j] *= 1.0 + v_lfdi + yl;
            out.pi[j] = inc_a - inc_l;
            out.pi_total += inc_a - inc_l;
            out.val += val_a - val_l;
        }
        out
    }

    /// Captures the base (once): the memo stocks are the research values (real-world $bn) times k, the previous-month references are
    /// the ledger's values now. `ln_s` = the bilateral log rates relative to the dollar base when the dollar link acts (else zeros).
    pub fn capture(&mut self, w: &World, ln_s: [f64; NB], fallback: bool) {
        if self.base.is_some() {
            return;
        }
        let tau = crate::fiscal::trend_gdp_month(w);
        let gdp_rw = w.cfg.world_gdp_rw_bn;
        if !(tau.is_finite() && tau > 0.0 && gdp_rw > 0.0) {
            return;
        }
        let k = 12.0 * tau / gdp_rw;
        for j in 0..NB {
            self.a_eq[j] *= k;
            self.a_debt[j] *= k;
            self.a_fdi[j] *= k;
            self.a_bank[j] *= k;
            self.l_ag[j] *= k;
            self.l_eq[j] *= k;
            self.l_fdi[j] *= k;
        }
        self.base = Some(PosBase { k, tau_b: tau });
        self.fallback = fallback;
        self.ln_fx_prev = if w.row.fx.is_finite() && w.row.fx > 0.0 { w.row.fx.ln() } else { 0.0 };
        self.ln_s_prev = ln_s;
        self.sp_prev = w.fund.share_price;
        self.bonds_prev = row_bonds(w);
        self.niip_prev = self.memo_assets() - self.memo_liabs() - w.row.bonds_value - w.row.deposits;
        self.niip = self.niip_prev;
        self.cpn_month = 0.0;
        self.dep_int_month = 0.0;
        self.prim_month.clear();
    }

    /// One reported month, after `goods::clear` and the markets (called from `WorldBlock::settle`). `x_j`, `m_j`: this month's
    /// bilateral exports and customs imports (model units). `ln_s`: the bilateral log rates relative to the dollar base when the
    /// dollar link acts, `None` otherwise (then every bloc revalues with the change of the ABM's broad dollar).
    pub fn update(&mut self, w: &World, x_j: &[f64; NB], m_j: &[f64; NB], ln_s: Option<[f64; NB]>) {
        if self.base.is_none() {
            return;
        }
        // 1. dollar increments by bloc
        let ln_fx = if w.row.fx.is_finite() && w.row.fx > 0.0 { w.row.fx.ln() } else { self.ln_fx_prev };
        let ds: [f64; NB] = match ln_s {
            Some(l) => std::array::from_fn(|j| l[j] - self.ln_s_prev[j]),
            None => [ln_fx - self.ln_fx_prev; NB],
        };
        // 2. the US equity return and the noise
        let r_us = if self.sp_prev > 0.0 && w.fund.share_price > 0.0 { (w.fund.share_price / self.sp_prev).ln() } else { 0.0 };
        let eta: [f64; NB] = std::array::from_fn(|j| {
            let mut r = rng::agent_rng(w.seed, w.tick, stream::WORLD_FX, POS_NOISE_BASE + j as u64);
            ETA_SD * normal(&mut r)
        });
        // 3. memo stocks: X_t = X_{t-1} (1 + v) + yield/12 X_{t-1}
        let MemoOut { pi, pi_total: pi_memo, val } = self.memo_step(&ds, r_us, &eta);
        // 4. RoW's Treasuries: the valuation of the face held at t-1 (a matured bond is priced at par), and the new references
        let mut val_hold = 0.0;
        let mut now: Vec<(BondId, f64, f64)> = Vec::with_capacity(self.bonds_prev.len() + 8);
        let mut pi_prev = 0usize;
        for idx in 0..w.bonds.len() {
            let b = &w.bonds[idx];
            // previous entries whose bond is gone matured: priced at par
            while pi_prev < self.bonds_prev.len() && self.bonds_prev[pi_prev].0 < b.id {
                let (_, p0, f0) = self.bonds_prev[pi_prev];
                val_hold += f0 * (1.0 - p0);
                pi_prev += 1;
            }
            let face = b.held_by(BondHolder::RestOfWorld);
            let had = pi_prev < self.bonds_prev.len() && self.bonds_prev[pi_prev].0 == b.id;
            if face > 0.0 || had {
                let p = crate::monetary::issue_price(w, idx);
                if had {
                    let (_, p0, f0) = self.bonds_prev[pi_prev];
                    val_hold += f0 * (p - p0);
                    pi_prev += 1;
                }
                if face > 0.0 {
                    now.push((b.id, p, face));
                }
            }
        }
        while pi_prev < self.bonds_prev.len() {
            let (_, p0, f0) = self.bonds_prev[pi_prev];
            val_hold += f0 * (1.0 - p0);
            pi_prev += 1;
        }
        self.bonds_prev = now;
        // The auction: RoW pays par for the new issue, whose market price is not 1 when its tenor's yield is below zero (coupon floored at 0).
        let mut val_prim = 0.0;
        for &(id, face, paid) in &self.prim_month {
            if let Some(idx) = w.bonds.iter().rposition(|b| b.id == id) {
                val_prim += face * crate::monetary::issue_price(w, idx) - paid;
            }
        }
        // 5. the identity
        let x_tot = w.row.exports_this_month;
        let m_tot = w.row.imports_this_month;
        let i_led = self.cpn_month + self.dep_int_month;
        let ca_ledger = (x_tot - m_tot) - i_led;
        let niip = self.memo_assets() - self.memo_liabs() - w.row.bonds_value - w.row.deposits;
        let rhs = ca_ledger + pi_memo + val - val_hold - val_prim;
        self.resid = if self.fallback && self.updates == 0 { 0.0 } else { (niip - self.niip_prev) - rhs };
        self.niip_prev = niip;
        self.niip = niip;
        self.ca_total = ca_ledger + pi_memo;
        self.pi_memo = pi_memo;
        self.val_memo = val;
        self.val_hold = val_hold + val_prim;
        let led = self.ledger_interest_by_bloc(self.cpn_month, self.dep_int_month);
        for j in 0..NB {
            self.ca_j[j] = x_j[j] - m_j[j] - led[j] + pi[j];
        }
        self.ds = ds;
        self.r_us = r_us;
        self.ln_fx_prev = ln_fx;
        if let Some(l) = ln_s {
            self.ln_s_prev = l;
        }
        self.sp_prev = w.fund.share_price;
        self.cpn_month = 0.0;
        self.dep_int_month = 0.0;
        self.prim_month.clear();
        self.updates += 1;
    }
}

/// RoW's face held per bond at the end of the month with the month's final price (bond order).
fn row_bonds(w: &World) -> Vec<(BondId, f64, f64)> {
    let mut v = Vec::new();
    for idx in 0..w.bonds.len() {
        let face = w.bonds[idx].held_by(BondHolder::RestOfWorld);
        if face > 0.0 {
            v.push((w.bonds[idx].id, crate::monetary::issue_price(w, idx), face));
        }
    }
    v
}
