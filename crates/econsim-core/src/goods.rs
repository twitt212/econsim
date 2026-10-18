//! Goods market (docs/DESIGN.md §8.3, §16): match orders to firm inventories, settle payments, record imports,
//! exports and sales, and update the sector and CPI price indices.
//!
//! Household allocation (preferred-firm switching and the retry firm) is computed in parallel from an immutable
//! snapshot; firm-level fills are aggregated sequentially; every payment goes through `ledger::pay`.
//!
//! Clearing order: households, government, firm intermediate inputs, firm capital goods, exports.

use rayon::prelude::*;

use crate::common::{self, sector_price};
use crate::ledger::{self, Account};
use crate::rng::{self, stream, Rng, SimRng};
use crate::types::*;

/// Random suppliers drawn per buyer and sector for firm-to-firm orders.
/// Keeps the matching O(buyers) rather than O(buyers × suppliers).
const FIRM_DRAWS: usize = 4;
/// Import shares move with relative import prices: a share is scaled by (terms / terms at t=0)^elasticity
/// (config `import_price_elasticity`), clamped to [0.5, 2.0] × base and at most `MAX_IMPORT_SHARE`.
const MAX_IMPORT_SHARE: f64 = 0.9;
/// Longest CPI / core-CPI history kept.
const CPI_HISTORY_MAX: usize = 600;
/// Sentinel for "no firm" (no alive seller in the sector).
const NO_FIRM: FirmId = FirmId::MAX;
/// RNG id offsets so firm-level draws never collide with household ids.
const STREAM_INPUTS: u64 = 1 << 40;
const STREAM_INVESTMENT: u64 = 2 << 40;

/// p1-fx: a price-elasticity factor with a distributed lag. `ratio` is the price ratio now (positive), `z` its lagged
/// log (see `lagged_logs`). The log factor is sr × ln ratio + (lr − sr) × z, so the impact elasticity is `sr` and the
/// long-run elasticity is `lr`. With lr = sr the lag drops out and the factor is exactly ratio^sr (the p0 path).
/// p1-fx: the long-run elasticity from its optional config value. `None` (or a non-finite value) gives the short-run
/// one, so the distributed lag is inert and an override of the short-run value takes effect.
fn long_run_or(lr: Option<f64>, sr: f64) -> f64 {
    lr.filter(|x| x.is_finite()).unwrap_or(sr)
}

pub(crate) fn elasticity_factor(ratio: f64, z: f64, sr: f64, lr: f64) -> f64 {
    let base = ratio.powf(sr);
    if lr == sr {
        base
    } else {
        base * z.exp().powf(lr - sr)
    }
}

/// p1-fx: one month of the lagged log state. It moves `speed` of the way from last month's value toward `now_ln`, and
/// starts at the current value. Non-finite inputs keep the previous value (or zero).
fn lagged_log(prev: Option<f64>, now_ln: f64, speed: f64) -> f64 {
    let now = if now_ln.is_finite() { now_ln } else { prev.filter(|x| x.is_finite()).unwrap_or(0.0) };
    let z_prev = prev.filter(|x| x.is_finite()).unwrap_or(now);
    let z = z_prev + speed * (now - z_prev);
    if z.is_finite() {
        z
    } else {
        now
    }
}

/// p1-fx: the lagged log state for each channel, given last month's states (None before the first clear).
fn lagged_logs<const N: usize>(prev: Option<[f64; N]>, now_ln: [f64; N], speed: f64) -> [f64; N] {
    std::array::from_fn(|i| lagged_log(prev.map(|p| p[i]), now_ln[i], speed))
}

/// The legacy export block (DESIGN 8.3, p1-fx): exports by sector from the price ratio p_world × world_f / (sector_p × fx) and
/// its lagged log state `prev` (None before the first clear). Returns the exports and the next lagged state; the caller stores
/// the state. `clear` calls this on the link-off path, with the same arithmetic as before. With the link on the block's exports
/// replace it (H7) and the state is left alone. Pure in `w`; the inputs are passed in so `clear` keeps its own month values.
pub(crate) fn legacy_exports(
    w: &World,
    prev: Option<[f64; N_SECTORS]>,
    sector_p: &[f64; N_SECTORS],
    world_f: &[f64; N_SECTORS],
    gdp: f64,
) -> ([f64; N_SECTORS], [f64; N_SECTORS]) {
    let p_world = common::world_price(w);
    let fx = finite_or(w.row.fx, 1.0);
    let export_sr = finite_or(w.cfg.export_elasticity_sr, 1.5);
    let export_lr = long_run_or(w.cfg.export_elasticity_lr, export_sr);
    let export_lag_speed = finite_or(w.cfg.export_lag_speed, 1.0).clamp(0.0, 1.0);
    // fix-trade: Energy exports are priced against the oil-linked world energy price.
    let export_ratio: [f64; N_SECTORS] = std::array::from_fn(|s| p_world * world_f[s] / (sector_p[s] * fx));
    let export_z = lagged_logs(prev, export_ratio.map(f64::ln), export_lag_speed);
    let mut exports = [0.0; N_SECTORS];
    for s in 0..N_SECTORS {
        let factor = elasticity_factor(export_ratio[s], export_z[s], export_sr, export_lr);
        let x = w.cfg.export_base_share_gdp[s] * gdp * factor;
        exports[s] = if x.is_finite() && x > 0.0 { x } else { 0.0 };
    }
    (exports, export_z)
}

/// Import share at the current relative import price: `factor` is the price factor (`elasticity_factor` of the
/// domestic-to-import terms ratio now over its t=0 value; see `MAX_IMPORT_SHARE`).
fn scaled_import_share(base: f64, factor: f64) -> f64 {
    let factor = if factor.is_finite() && factor > 0.0 { factor } else { 1.0 };
    (base * factor).clamp(0.5 * base, 2.0 * base).min(MAX_IMPORT_SHARE).max(0.0)
}

// fix-trade: import prices pass through exchange-rate moves partially and with a lag, but world inflation passes
// through fully. spot is the dollar-implied import price, p_world / fx over its t=0 value (world inflation and the FX
// level both in it). e is the log FX move since the first clear (positive when the dollar weakens), and z is a lagged
// state that moves a fraction `import_pass_through_speed` of the way each month toward `import_pass_through_long` × e
// (Campa & Goldberg 2005; Gopinath et al. 2010). The import price relative to t=0 is spot × exp(z − e): with full,
// instant pass-through (long = speed = 1) it is exactly spot. Pure given `st`, which holds the previous z and the FX
// base; returns (import price relative to t=0, next z).
pub fn import_pass_through(w: &World, st: &GoodsState) -> (f64, f64) {
    let fxb = crate::monetary::fx_base(w);
    let fx = finite_or(w.row.fx, 1.0).max(f64::MIN_POSITIVE);
    let fx_clear = st.fx_clear.filter(|x| x.is_finite() && *x > 0.0).unwrap_or(fx);
    let spot = finite_or((common::world_price(w) / fx) / fxb.import_price, 1.0);
    let pt = finite_or(w.cfg.import_pass_through_long, 1.0);
    let speed = finite_or(w.cfg.import_pass_through_speed, 1.0).clamp(0.0, 1.0);
    let e = (fx_clear / fx).ln();
    let e = if e.is_finite() { e } else { 0.0 };
    let z_prev = st.pt_z.filter(|x| x.is_finite()).unwrap_or(0.0);
    let z = z_prev + speed * (pt * e - z_prev);
    let z = if z.is_finite() { z } else { 0.0 };
    let imp_rel = spot * (z - e).exp();
    (if imp_rel.is_finite() && imp_rel > 0.0 { imp_rel } else { spot }, z)
}

/// b7-tariff: lagged tariff state, carried across months in `GoodsState`. `retail` is the retail wedge r (M1);
/// `z_hh` and `z_firm` are the lagged logs of the household and firm tariff prices (M2, zero before the first tariff);
/// `ln_f_firm` is this month's firm import-share log factor, read by `firms.rs` for imported-input costs (M7).
#[derive(Clone, Copy, Debug, Default)]
pub struct TariffState {
    pub retail: f64,
    pub z_hh: f64,
    pub z_firm: f64,
    pub ln_f_firm: f64,
}

/// b7-tariff: one month of the tariff mechanism, as used by `clear` and by the metrics.
#[derive(Clone, Copy, Debug)]
pub struct TariffMonth {
    /// Whether the mechanism runs this month: a tariff is set, or the lagged state has not decayed to zero.
    pub active: bool,
    /// Statutory tariff τ_t.
    pub tau: f64,
    /// Retail wedge r_t (zero when inactive).
    pub retail: f64,
    /// Household import-share factor f^H_t (one when inactive).
    pub f_hh: f64,
    /// Firm import-share factor f^F_t (one when inactive).
    pub f_firm: f64,
}

impl Default for TariffMonth {
    fn default() -> Self {
        TariffMonth { active: false, tau: 0.0, retail: 0.0, f_hh: 1.0, f_firm: 1.0 }
    }
}

/// b7-tariff: a tariff state with no tariff this month is snapped to zero below this size.
const TARIFF_SNAP: f64 = 1e-9;

/// b7-tariff (M6): the tradables price index. Domestic goods (relative to t=0) and imports (relative to t=0, with the
/// retail wedge `retail` on top) are blended at the import share `m` (fixed weights).
fn tradables_price(m: f64, domestic: f64, imp_rel: f64, retail: f64) -> f64 {
    finite_or((1.0 - m) * domestic + m * (imp_rel * (1.0 + retail)), 1.0)
}

/// b7-tariff: one month of M1 (retail wedge) and M2 (tariff-elastic import shares). Inactive, with the flag off or with
/// no tariff and no lagged state left, it returns the identity and leaves `s` untouched; the legacy path then runs.
///
/// M1: r_t = r_{t-1} + λ_R (φ_R τ_t − r_{t-1}), from r_{-1} = 0. M2: x^H = ln(1 + r_t), x^F = ln(1 + τ_t),
/// z^k_t = z^k_{t-1} + λ_S (x^k_t − z^k_{t-1}) from z_{-1} = 0, and ln f^k_t = −ε_S x^k_t − (ε_L − ε_S) z^k_t.
/// Rates are clamped to [0, 1] and elasticities to be non-negative (config docs).
pub fn tariff_step(cfg: &crate::config::Config, s: &mut TariffState, tau: f64) -> TariffMonth {
    let tau = finite_or(tau, 0.0).max(0.0);
    let active = cfg.tariff_pass_through && (tau > 0.0 || s.retail != 0.0 || s.z_hh != 0.0 || s.z_firm != 0.0);
    if !active {
        return TariffMonth { active: false, tau, ..TariffMonth::default() };
    }
    let lam_r = finite_or(cfg.tariff_retail_speed, 0.0).clamp(0.0, 1.0);
    let phi = finite_or(cfg.tariff_retail_long, 1.0).max(0.0);
    let eps_s = finite_or(cfg.tariff_share_elasticity_sr, 0.0).max(0.0);
    let eps_l = finite_or(cfg.tariff_share_elasticity_lr, 0.0).max(0.0);
    let lam_s = finite_or(cfg.tariff_share_lag_speed, 0.0).clamp(0.0, 1.0);
    let snap = |x: f64| if tau == 0.0 && x.abs() < TARIFF_SNAP { 0.0 } else { x };
    let retail = snap(s.retail + lam_r * (phi * tau - s.retail));
    let x_h = (1.0 + retail).ln();
    let x_f = (1.0 + tau).ln();
    let z_hh = snap(s.z_hh + lam_s * (x_h - s.z_hh));
    let z_firm = snap(s.z_firm + lam_s * (x_f - s.z_firm));
    let ln_f_hh = -eps_s * x_h - (eps_l - eps_s) * z_hh;
    let ln_f_firm = -eps_s * x_f - (eps_l - eps_s) * z_firm;
    let ln_f_hh = if ln_f_hh.is_finite() { ln_f_hh } else { 0.0 };
    let ln_f_firm = if ln_f_firm.is_finite() { ln_f_firm } else { 0.0 };
    s.retail = retail;
    s.z_hh = z_hh;
    s.z_firm = z_firm;
    s.ln_f_firm = ln_f_firm;
    TariffMonth { active: true, tau, retail, f_hh: ln_f_hh.exp(), f_firm: ln_f_firm.exp() }
}

/// b7-tariff (M4): importers absorb the part of the statutory duty above the retail wedge. Each covered household
/// sector s owes G_s = (τ − r) × V_s, where V_s is its customs value this month, shared across the alive firms of
/// sector s in proportion to their revenue (firm order), or equally when those firms have no revenue. A positive share
/// is paid by the firm to Government; a negative one (a tariff cut while r > τ) is paid by Government to the firm.
/// With no alive firm in the sector the amount cannot be charged: it goes to `flows.tariff_uncollected` and is in no
/// revenue metric (`flows.tariff_absorbed` records only the amounts that move).
fn settle_tariff_gap(w: &mut World, cust_cov: &[f64; N_CONSUMER], tau: f64, retail: f64) {
    let gap = tau - retail;
    for s in 0..N_CONSUMER {
        let g = gap * cust_cov[s];
        if g == 0.0 || !g.is_finite() {
            continue;
        }
        let n_alive = w.firms.iter().filter(|f| f.alive && f.sector.idx() == s).count();
        if n_alive == 0 {
            w.flows.tariff_uncollected += g;
            continue;
        }
        let total: f64 = w
            .firms
            .iter()
            .filter(|f| f.alive && f.sector.idx() == s)
            .map(|f| f.revenue.max(0.0))
            .sum();
        let equal = !(total > 0.0);
        for f in 0..w.firms.len() {
            let (alive, sector, rev) = (w.firms[f].alive, w.firms[f].sector.idx(), w.firms[f].revenue.max(0.0));
            if !alive || sector != s {
                continue;
            }
            let a = if equal { g / n_alive as f64 } else { g * rev / total };
            if !(a != 0.0) || !a.is_finite() {
                continue;
            }
            if a > 0.0 {
                ledger::pay(w, Account::Firm(f as FirmId), Account::Government, a);
            } else {
                ledger::pay(w, Account::Government, Account::Firm(f as FirmId), -a);
            }
            w.firms[f].input_cost += a;
            w.gov.revenue += a;
            w.flows.tariff_absorbed += a;
        }
    }
}

/// t=0 reference values for the CPI.
#[derive(Clone, Copy, Debug, Default)]
pub struct CpiBase {
    /// Lease-stock rent (CPI rent, see `lease_rent`) at the first call to `clear`.
    pub lease: f64,
    /// Sector prices at the first call to `clear`.
    pub sector: [f64; N_SECTORS],
}

/// How one household split its consumer budget in one sector.
#[derive(Clone, Copy, Debug, Default)]
struct Split {
    /// Paid to the preferred firm (net of sales tax).
    pref: f64,
    /// Paid to the retry firm (net of sales tax).
    alt: f64,
    /// Imported: paid to Rest of World and, for covered sectors, the duty wedge (budget net of sales tax; the wedge is
    /// W = B − B/(1+r)).
    imported: f64,
    /// Gross budget left unspent (non-Goods only).
    unspent: f64,
}

/// Households per parallel payment chunk (fixed, so results do not depend on the thread count).
const PAY_CHUNK: usize = 1024;

/// Per-chunk household payment totals, merged in chunk order.
#[derive(Clone, Debug)]
struct PayTotals {
    consumption: [f64; N_CONSUMER],
    unspent_by_sector: [f64; N_CONSUMER],
    imp: f64,
    tax: f64,
    tariff: f64,
    /// b7-tariff: customs value of covered household imports by sector (M4 base).
    cust_cov: [f64; N_CONSUMER],
    /// b7-tariff: household duty wedge by sector (the retail part, paid to Government).
    tariff_by_sector: [f64; N_CONSUMER],
    /// b7-tariff: household duty on the legacy path (lump-sum, `tariff_pass_through` off); not in `tariff_by_sector`.
    legacy: f64,
    // b7-spikes (diagnostic): unmet alternative-supplier budget imported (tradables).
    unmet_imp: f64,
    /// b8-world H9: customs value of household imports by sector (duty excluded), accumulated beside `imp`.
    imp_s: [f64; N_SECTORS],
}

impl PayTotals {
    fn new() -> Self {
        PayTotals {
            consumption: [0.0; N_CONSUMER],
            unspent_by_sector: [0.0; N_CONSUMER],
            imp: 0.0,
            tax: 0.0,
            tariff: 0.0,
            cust_cov: [0.0; N_CONSUMER],
            tariff_by_sector: [0.0; N_CONSUMER],
            legacy: 0.0,
            unmet_imp: 0.0,
            imp_s: [0.0; N_SECTORS],
        }
    }
    fn merge(&mut self, o: &PayTotals) {
        for s in 0..N_CONSUMER {
            self.consumption[s] += o.consumption[s];
            self.unspent_by_sector[s] += o.unspent_by_sector[s];
        }
        for s in 0..N_CONSUMER {
            self.cust_cov[s] += o.cust_cov[s];
            self.tariff_by_sector[s] += o.tariff_by_sector[s];
        }
        self.imp += o.imp;
        for s in 0..N_SECTORS {
            self.imp_s[s] += o.imp_s[s];
        }
        self.tax += o.tax;
        self.tariff += o.tariff;
        self.legacy += o.legacy;
        self.unmet_imp += o.unmet_imp;
    }
}

/// One leftover-budget sale in step 5b: household `h` spends `gross` on sector `s` at firm `f` (net `net` to the firm,
/// the rest sales tax); `real` is the quantity sold.
#[derive(Clone, Copy, Debug)]
struct Walk {
    h: u32,
    s: u8,
    f: u32,
    gross: f64,
    net: f64,
    real: f64,
}

/// One supplier draw for a firm-to-firm order.
#[derive(Clone, Copy, Debug)]
struct Draw {
    buyer: FirmId,
    supplier: FirmId,
    nominal: f64,
    /// b7-tariff: the order is in a covered (tradable) sector, so an unmet part pays the duty (M5).
    cov: bool,
    /// b8-world H9: the sector of the order (for the imports by sector).
    sector: u8,
}

/// Which firm-to-firm order book is being cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirmUse {
    Inputs,
    Investment,
}

/// Weighted-draw index over a cumulative weight array. `pick(u)` returns exactly
/// `cum.partition_point(|c| c <= u).min(len - 1)`; `tab` gives, per uniform bucket of `[0, total)`, a lower bound
/// on that index, so the search starts next to the answer instead of over the whole array.
#[derive(Clone, Debug, Default)]
struct Cdf {
    cum: Vec<f64>,
    tab: Vec<u32>,
    /// Buckets per unit of weight (`0` when the total is not positive).
    inv: f64,
    /// Bucket lower edge per unit bucket index, shaded down by a relative 1e-9 so rounding in `u * inv` can never
    /// place a draw below its bucket's bound.
    step: f64,
}

impl Cdf {
    /// Rebuild from the weights in order (cumulative sums accumulate in the same order as before).
    fn build(&mut self, weights: impl Iterator<Item = f64>) {
        self.cum.clear();
        let mut acc = 0.0;
        for x in weights {
            acc += x;
            self.cum.push(acc);
        }
        let n = self.cum.len();
        self.tab.clear();
        if n == 0 {
            self.inv = 0.0;
            self.step = 0.0;
            return;
        }
        let total = self.cum[n - 1];
        let scale = total / n as f64;
        self.inv = if total > 0.0 { n as f64 / total } else { 0.0 };
        self.step = scale * (1.0 - 1e-9);
        let mut p = 0usize;
        for b in 0..n {
            let lo = b as f64 * self.step;
            while p < n && self.cum[p] <= lo {
                p += 1;
            }
            self.tab.push(p as u32);
        }
    }

    #[inline]
    fn total(&self) -> f64 {
        self.cum.last().copied().unwrap_or(0.0)
    }

    #[inline]
    fn pick(&self, u: f64) -> usize {
        let n = self.cum.len();
        if n == 0 {
            return 0;
        }
        let b = ((u * self.inv) as usize).min(self.tab.len() - 1);
        // Entries before `i` are <= the bucket bound <= u, so the answer is at or after `i`.
        let mut i = self.tab[b] as usize;
        let mut steps = 0;
        while i < n && self.cum[i] <= u {
            i += 1;
            steps += 1;
            if steps == 8 {
                i += self.cum[i..].partition_point(|&c| c <= u);
                break;
            }
        }
        i.min(n - 1)
    }
}

/// Compact per-household copy used by the parallel household passes, so they do not stream whole `Household`s.
#[derive(Clone, Copy, Debug)]
struct HhSlim {
    id: u64,
    alive: bool,
    /// Preferred firm per consumer sector after this month's switching.
    pref: [FirmId; N_CONSUMER],
}

/// Goods-market state: CPI base values and per-month scratch buffers, reused to avoid allocation.
#[derive(Clone, Debug, Default)]
pub struct GoodsState {
    /// b8-world H9: customs value of this firm stage's unmet imports by sector (flushed to `flows` in `finish_firm_stage`).
    imp_s: [f64; N_SECTORS],
    /// Household budget left unspent this month, by consumer sector (diagnostic).
    pub unspent_by_sector: [f64; N_CONSUMER],
    // fix-trade: import price relative to t=0 after partial pass-through (None before the first clear).
    pub import_rel: Option<f64>,
    // fix-trade: lagged pass-through state of the log FX move (see `import_pass_through`), and the FX level at the first
    // clear that the FX move is measured from.
    pub pt_z: Option<f64>,
    pub fx_clear: Option<f64>,
    /// t=0 reference values for the CPI, captured on the first call to `clear`.
    pub cpi_base: Option<CpiBase>,
    /// b7-tariff: lagged tariff state (M1, M2), updated once per `clear`.
    pub tariff: TariffState,
    /// b7-tariff: this month's tariff step, for the metrics.
    pub tariff_month: TariffMonth,
    /// b7-tariff: household import share by sector this month, after the tariff factor (metrics).
    pub hh_import_share_now: [f64; N_SECTORS],
    /// b7-tariff: firms' imported-input share this month, after the tariff factor (metrics).
    pub firm_input_share_now: f64,
    /// b7-tariff: unmet tradable-sector orders by buyer this month (the duty base, M5); scratch, reset in `firm_stage`.
    unmet_cov: Vec<f64>,
    /// b6-firmsize S5: log appeal per firm slot (x = ln a), used only with `demand_logit`. Set at t=0 in init.rs.
    pub appeal_ln: Vec<f64>,
    alive: [Vec<FirmId>; N_SECTORS],
    /// Per firm: sector index if alive, `NOT_SELLING` otherwise.
    seller: Vec<u8>,
    cum: [Cdf; N_SECTORS],
    price: Vec<f64>,
    avail: Vec<f64>,
    dem: Vec<f64>,
    dem_alt: Vec<f64>,
    fill: Vec<f64>,
    /// Last month's fill fraction per firm (rationing signal for customer switching).
    last_fill: Vec<f64>,
    /// Cumulative market-share weights per consumer sector (aligned with `alive`).
    share_cum: [Cdf; N_SECTORS],
    fill_alt: Vec<f64>,
    unmet: Vec<f64>,
    choice: Vec<[FirmId; N_CONSUMER]>,
    /// Per household and consumer sector: gross budget left unspent (filled in the payment loop, read in step 5b).
    unspent: Vec<[f64; N_CONSUMER]>,
    /// Net firm-to-firm payment per firm in a firm stage (scratch).
    firm_net: Vec<f64>,
    /// Per household and consumer sector: nominal budget share of the order (before rationing), written in step 1.
    /// Read in steps 2 and 3, where it is only used if the household's preferred firm is a seller.
    hh_dem: Vec<[f64; N_CONSUMER]>,
    /// Retry demand per household chunk, as (retry firm, unmet first-pass nominal), in household then sector order.
    retry_parts: Vec<Vec<(FirmId, f64)>>,
    /// Compact per-household state (id, alive, preferred firms), refreshed in step 1.
    slim: Vec<HhSlim>,
    /// Households holding some unspent budget, in household order (step 5b).
    left_hh: Vec<u32>,
    /// Per firm: position in its sector's `alive` list (valid for alive sellers).
    sec_pos: Vec<u32>,
    /// Leftover-budget allocations (step 5b), in household order.
    allocs: Vec<Walk>,
}

#[inline]
fn finite_or(x: f64, default: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

#[inline]
/// Lease-stock rent index for this month (CPI and PCE shelter, metrics `cpi_rent_hist`): the average market rent over
/// the last 12 months, so a market-rent change reaches the CPI over a year as leases renew. Metrics records the rent
/// history after this step, so this month's rent is the last entry added here.
pub fn lease_rent(w: &World) -> f64 {
    let hist = &w.metrics_state.rent_hist;
    let n = hist.len().min(11);
    let sum: f64 = hist[hist.len() - n..].iter().sum::<f64>() + w.prices.rent_index;
    sum / (n + 1) as f64
}

fn pos_or_one(x: f64) -> f64 {
    if x.is_finite() && x > 0.0 {
        x
    } else {
        1.0
    }
}

/// Replace non-finite or negative entries with zero.
#[inline]
fn clean<const K: usize>(a: [f64; K]) -> [f64; K] {
    a.map(|x| if x.is_finite() && x > 0.0 { x } else { 0.0 })
}

/// Sentinel in `GoodsState::seller` for a firm that is not alive.
const NOT_SELLING: u8 = u8::MAX;

/// Whether firm `f` is an alive seller of sector index `s`. `seller[f]` is the firm's sector index when alive,
/// `NOT_SELLING` otherwise (a compact copy of `alive && sector`, so the parallel loops avoid touching `Firm`s).
#[inline]
fn is_seller(seller: &[u8], f: FirmId, s: usize) -> bool {
    match seller.get(f as usize) {
        Some(&k) => k as usize == s,
        None => false,
    }
}

/// Fraction of `real_demand` that `avail` can cover (1 when demand is zero).
#[inline]
fn fill_fraction(real_demand: f64, avail: f64) -> f64 {
    if real_demand <= avail {
        1.0
    } else {
        (avail / real_demand).clamp(0.0, 1.0)
    }
}

/// Book `nominal` $ of demand at firm `f` (posted `price`) against its available stock.
/// Sells pro rata if stock is short; returns the fill fraction.
fn settle(f: &mut Firm, avail: &mut f64, nominal: f64, price: f64) -> f64 {
    settle_inner(f, avail, nominal, price, true)
}

/// Like `settle`, but for re-routed (retry) demand whose unmet part was already counted at the first firm:
/// only the quantity actually sold is added to this firm's demand.
fn settle_retry(f: &mut Firm, avail: &mut f64, nominal: f64, price: f64) -> f64 {
    settle_inner(f, avail, nominal, price, false)
}

fn settle_inner(f: &mut Firm, avail: &mut f64, nominal: f64, price: f64, count_unmet: bool) -> f64 {
    let real = nominal / price;
    let fill = fill_fraction(real, *avail);
    let sold = fill * real;
    f.demand += if count_unmet { real } else { sold };
    f.sales += sold;
    f.inventory = (f.inventory - sold).max(0.0);
    f.revenue += fill * nominal;
    *avail = (*avail - sold).max(0.0);
    fill
}

/// Weight on expected demand (real units) when routing firm/government orders across suppliers.
const ROUTING_DEMAND_WEIGHT: f64 = 0.2;
/// Probability that a customer rationed by its preferred firm last month switches to another.
const RATIONED_SWITCH_PROB: f64 = 0.5;
/// p1-firmsentry: probability that a customer switches to a firm charging the same price (`entrant_tie_switch`).
const TIE_SWITCH_PROB: f64 = 0.5;
/// RNG key for the household retry draw.
const RETRY_KEY: u64 = 1 << 40;
/// b6-firmsize S5: RNG key for the appeal shock, `APPEAL_KEY + firm id` on the GOODS stream (free: keys 1<<40 and 2<<40
/// are the retry and the firm-order streams).
const APPEAL_KEY: u64 = 5 << 40;

/// b6-firmsize S5: one month of the appeal process for every firm slot, in firm order. An entrant (age 0) is reset to
/// the barrier `entrant_appeal_ln`; an alive incumbent moves to x ← |x − μ_a + σ_a η| (reflected Gibrat: the log
/// stays non-negative, and its stationary law is exponential with rate ζ, so the appeal has a Pareto tail A^(−ζ)).
/// Dead slots keep their value until they are reborn.
fn step_appeal(w: &World, appeal: &mut Vec<f64>) {
    let nf = w.firms.len();
    let barrier = finite_or(w.cfg.entrant_appeal_ln, 0.0);
    if appeal.len() < nf {
        appeal.resize(nf, barrier);
    }
    let sig = finite_or(w.cfg.appeal_sigma, 0.0).max(0.0);
    let zeta = finite_or(w.cfg.appeal_zeta, 1.1).max(0.0);
    for (f, fm) in w.firms.iter().enumerate() {
        if !fm.alive {
            continue;
        }
        if fm.age_months == 0 {
            appeal[f] = barrier;
        } else {
            let mut r = rng::agent_rng(w.seed, w.tick, stream::GOODS, APPEAL_KEY + f as u64);
            let eta = rng::normal(&mut r);
            appeal[f] = appeal_step_ln(appeal[f], eta, sig, zeta);
        }
    }
}

/// b6-firmsize S5: one reflected Gibrat step of the log appeal, x' = |x − μ_a + σ_a η| with μ_a = ζ σ_a² / 2.
/// The stationary law of x is exponential with rate ζ, so the appeal a = e^x has a Pareto tail with exponent ζ.
#[inline]
fn appeal_step_ln(x: f64, eta: f64, sig: f64, zeta: f64) -> f64 {
    let mu = zeta * sig * sig / 2.0;
    (x - mu + sig * eta).abs()
}

/// b6-firmsize S5: unnormalised household choice weight v_f = a_f · p_f^(−ε) from the log appeal and the price.
#[inline]
fn logit_choice_weight(appeal_ln: f64, price: f64, eps: f64) -> f64 {
    appeal_ln.exp() * price.max(1e-9).powf(-eps)
}

fn push_capped(v: &mut Vec<f64>, x: f64) {
    v.push(x);
    if v.len() > CPI_HISTORY_MAX {
        let excess = v.len() - CPI_HISTORY_MAX;
        v.drain(0..excess);
    }
}

/// Clear all orders in `w.orders`, settle payments, record sales/consumption/flows and update `w.prices`.
///
/// Households: each buys its import share from RoW (customs value to RoW, retail duty wedge and absorbed duty to
/// Government; legacy path: lump-sum tariff) and the domestic remainder from its preferred firm; unmet domestic budget
/// gets one retry at a random firm; unmet Goods budget is imported; other
/// unmet budget is unspent. Sales tax (`cfg.sales_tax`) is remitted to Government on every household purchase;
/// orders are gross of it.
pub fn clear(w: &mut World) {
    let nf = w.firms.len();
    let nh = w.households.len();
    let (tick, seed) = (w.tick, w.seed);
    let tax = finite_or(w.cfg.sales_tax, 0.0).max(0.0);
    // H8 (b8-world): the household channel's duty; the firm channel's duty is read where firm_stage is called below.
    let tariff = crate::world::tau_for(w, crate::world::Channel::Hh);
    let tariff_firm = crate::world::tau_for(w, crate::world::Channel::Firm);
    let gross_up = 1.0 + tax;
    let net = 1.0 / gross_up;
    let k_switch = finite_or(w.cfg.switch_intensity, 0.0);
    let switch_prob = finite_or(w.cfg.switch_prob, 0.0);

    let gdp = common::gdp_month_model(w);
    // H6/H7 (b8-world): with the trade link on (and a reported month), import prices and exports come from the block.
    let trade_on = crate::world::trade_active(w);
    let mut st = std::mem::take(&mut w.goods);
    // b7-tariff: the tariff step (M1, M2) runs before the import shares and the firm stages below.
    let tp = tariff_step(&w.cfg, &mut st.tariff, tariff);
    let mut hhs = std::mem::take(&mut w.households);
    let orders = std::mem::take(&mut w.orders.household);
    w.fills.firm_capital_received.resize(nf, 0.0);
    w.fills.household_unspent.resize(nh, 0.0);

    // ---- 0. per-firm buffers, alive lists, CPI base (§13.4)
    let sector_p: [f64; N_SECTORS] = std::array::from_fn(|s| pos_or_one(sector_price(w, Sector::from_idx(s))));
    st.cpi_base.get_or_insert_with(|| CpiBase {
        lease: pos_or_one(lease_rent(w)),
        sector: sector_p,
    });
    // Import shares respond to the relative price of domestic goods and imports: the terms ratio is the sector price
    // over the (passed-through) import price, each relative to t=0. fix-trade: that import price is the same one the CPI
    // uses.
    let fxb = crate::monetary::fx_base(w);
    let fx = finite_or(w.row.fx, 1.0);
    // p1-fx: short-run (p0 `import_price_elasticity`) and long-run import-share elasticities, reached through a lagged
    // terms ratio. An unset long-run value (`None`) takes the short-run one, which makes the lag inert (the p0 path).
    let elasticity = finite_or(w.cfg.import_price_elasticity, 0.0);
    let elasticity_lr = long_run_or(w.cfg.import_elasticity_lr, elasticity);
    let import_lag_speed = finite_or(w.cfg.import_lag_speed, 1.0).clamp(0.0, 1.0);
    let tw = w.cfg.tradable_weight;
    let tradable: [bool; N_SECTORS] = std::array::from_fn(|s| tw[s] != 0.0);
    let cpi_tw = w.cfg.cpi_tradable_weight;
    // fix-trade: import prices use the partially passed-through import price. The state is read from `st` (taken above),
    // and the new state is stored back in it.
    // H6 (b8-world): the import prices by sector and their CPI-weighted index. Legacy: one pass-through price for all.
    let (imp_rel_sec, imp_rel_cpi_v): ([f64; N_SECTORS], f64) = if trade_on {
        let wo = &w.world.as_deref().expect("trade_active implies the block").out;
        st.import_rel = Some(wo.imp_rel_cpi);
        (wo.imp_rel, wo.imp_rel_cpi)
    } else {
        let (imp_rel, z_next) = import_pass_through(w, &st);
        st.import_rel = Some(imp_rel);
        st.pt_z = Some(z_next);
        st.fx_clear.get_or_insert(fx);
        ([imp_rel; N_SECTORS], imp_rel)
    };
    // Energy's import price also carries the oil-linked world price (`common::world_sector_factor`).
    let world_f: [f64; N_SECTORS] = std::array::from_fn(|s| common::world_sector_factor(w, s));
    let terms_ratio = |s: usize| (sector_p[s] / (fxb.terms[s] * fxb.import_price)) / (imp_rel_sec[s] * world_f[s]);
    // p1-fx: the terms ratios by sector, and their tradables mean (the firm channels' ratio), with the lagged logs.
    let tr_now: [f64; N_SECTORS + 1] =
        std::array::from_fn(|i| if i < N_SECTORS { terms_ratio(i) } else { role_mean(&tw, |s| terms_ratio(s)) });
    let tr_z = lagged_logs(w.trade_import_z, tr_now.map(f64::ln), import_lag_speed);
    w.trade_import_z = Some(tr_z);
    let imp_factor = |i: usize| elasticity_factor(tr_now[i], tr_z[i], elasticity, elasticity_lr);
    // b7-tariff (M2): covered household sectors (those with a CPI import weight) take the tariff-elastic factor.
    let duty_hh: [bool; N_CONSUMER] = std::array::from_fn(|s| tp.active && cpi_tw[s] != 0.0);
    let m_share: [f64; N_SECTORS] = std::array::from_fn(|s| {
        let base = finite_or(w.cfg.import_share[s], 0.0).clamp(0.0, 1.0);
        if s < N_CONSUMER && duty_hh[s] {
            scaled_import_share(base, imp_factor(s) * tp.f_hh)
        } else {
            scaled_import_share(base, imp_factor(s))
        }
    });
    // Firms' imported inputs and equipment are priced off the tradables terms of trade (times the tariff factor).
    let firm_f = if tp.active { tp.f_firm } else { 1.0 };
    let m_firm_inputs = scaled_import_share(
        finite_or(w.cfg.firm_input_import_share, 0.0).clamp(0.0, 1.0),
        imp_factor(N_SECTORS) * firm_f,
    );
    let m_firm_equipment = scaled_import_share(
        finite_or(w.cfg.firm_equipment_import_share, 0.0).clamp(0.0, 1.0),
        imp_factor(N_SECTORS) * firm_f,
    );
    st.hh_import_share_now = m_share;
    st.firm_input_share_now = m_firm_inputs;
    // Tradables CPI: domestic goods and imports, each relative to its t=0 price, at the household's base tradables
    // import share (fixed weights). fix-trade: the composite excludes Food (`Config::cpi_tradable_weight`).
    let cpi_tradable: [bool; N_SECTORS] = std::array::from_fn(|s| cpi_tw[s] != 0.0);
    let m_cpi_goods = finite_or(role_mean(&cpi_tw, |s| finite_or(w.cfg.import_share[s], 0.0)), 0.0).clamp(0.0, 1.0);
    // p1-firmsentry: introductory discount for young firms (see the customer weights in step 1).
    let intro_months = w.cfg.entrant_intro_months;
    let intro_discount = finite_or(w.cfg.entrant_intro_discount, 0.0).clamp(0.0, 1.0);
    let tie_switch = w.cfg.entrant_tie_switch;
    st.price.clear();
    st.avail.clear();
    st.seller.clear();
    for list in st.alive.iter_mut() {
        list.clear();
    }
    for (i, f) in w.firms.iter().enumerate() {
        let s = f.sector.idx();
        let mut p = if f.price.is_finite() && f.price > 0.0 { f.price } else { sector_p[s] };
        // p1-firmsentry: introductory discount for young firms (they must win customers on price as well).
        if f.alive && f.age_months < intro_months {
            p *= 1.0 - intro_discount;
        }
        let inv = finite_or(f.inventory, 0.0).max(0.0);
        // Intentional: construction stock reserved for houses in progress is not sold to non-housing buyers (§10).
        let reserve = if f.sector == Sector::Construction {
            finite_or(f.backlog, 0.0).max(0.0).min(inv)
        } else {
            0.0
        };
        st.price.push(p);
        st.avail.push(if f.alive { inv - reserve } else { 0.0 });
        st.seller.push(if f.alive { s as u8 } else { NOT_SELLING });
        if f.alive {
            st.alive[s].push(i as FirmId);
        }
    }
    st.sec_pos.clear();
    st.sec_pos.resize(nf, 0);
    for s in 0..N_SECTORS {
        for (k, &f) in st.alive[s].iter().enumerate() {
            st.sec_pos[f as usize] = k as u32;
        }
    }
    st.dem.clear();
    st.dem.resize(nf, 0.0);
    st.dem_alt.clear();
    st.dem_alt.resize(nf, 0.0);
    st.last_fill = std::mem::take(&mut st.fill);
    st.last_fill.resize(nf, 1.0);
    st.fill.clear();
    st.fill.resize(nf, 1.0);
    st.fill_alt.clear();
    st.fill_alt.resize(nf, 1.0);
    st.unmet.clear();
    st.unmet.resize(nf, 0.0);
    st.imp_s = [0.0; N_SECTORS];

    // ---- 0b. contractual and export orders are served before households (rationing falls on household
    //          purchases, whose unmet Goods demand spills into imports): government purchases first
    let gov_budget = w.orders.government;
    let paid = pool_sales(w, &mut st, Account::Government, gov_budget);
    let gov_total: f64 = paid.iter().sum();
    w.gov.purchases += gov_total;
    w.gov.outlays += gov_total;
    w.flows.government += gov_total;

    // ---- 7. firm intermediate inputs, then capital goods
    let inputs = std::mem::take(&mut w.orders.firm_inputs);
    firm_stage(w, &mut st, &inputs, FirmUse::Inputs, tariff_firm, m_firm_inputs, tp.active);
    w.orders.firm_inputs = inputs;

    let investment = std::mem::take(&mut w.orders.firm_investment);
    firm_stage(w, &mut st, &investment, FirmUse::Investment, tariff_firm, m_firm_equipment, tp.active);
    w.orders.firm_investment = investment;

    // ---- 8. exports: RoW demand per sector (DESIGN §8.3, §16). A stronger dollar makes exports dearer abroad.
    // p1-fx: the export price elasticity is `export_elasticity_sr` on impact and `export_elasticity_lr` once the lagged
    // price state (`export_lag_speed`) has caught up (distributed lag; the J-curve). Equal values give the p0 path.
    // H7 (b8-world): with the trade link on, the exports are the block's bilateral sum (spec 2.5); the legacy lagged
    // price state (`trade_export_z`) is left alone.
    let exports: [f64; N_SECTORS] = if trade_on {
        w.world.as_deref().expect("trade_active implies the block").out.exports
    } else {
        let (exports, export_z) = legacy_exports(w, w.trade_export_z, &sector_p, &world_f, gdp);
        w.trade_export_z = Some(export_z);
        exports
    };
    w.orders.exports = exports;
    let paid = pool_sales(w, &mut st, Account::RestOfWorld, exports);
    let exp_total: f64 = paid.iter().sum();
    w.flows.exports += exp_total;
    for s in 0..N_SECTORS {
        w.flows.exports_by_sector[s] += paid[s];
    }
    w.row.exports_this_month += exp_total;


    // b6-firmsize S5: appeal process (no draws when `demand_logit` is off).
    let logit = w.cfg.demand_logit;
    let demand_eps = finite_or(w.cfg.demand_elasticity, 4.5);
    let rechoice = if logit { finite_or(w.cfg.demand_rechoice_prob, 0.0).clamp(0.0, 1.0) } else { 0.0 };
    if logit {
        step_appeal(w, &mut st.appeal_ln);
    }
    // ---- 1. preferred-firm switching (§8.3) per household, in parallel. Candidate firms are sampled in proportion
    //         to market share (expected demand), so customers are spread across firms roughly in line with capacity.
    st.choice.clear();
    st.choice.resize(nh, [NO_FIRM; N_CONSUMER]);
    // p1-firmsentry: a young firm's customer weight is floored at a share of its sector's mean expected demand.
    let customer_floor = finite_or(w.cfg.entrant_customer_floor, 0.0).max(0.0);
    for s in 0..N_SECTORS {
        let (alive, expected) = (&st.alive[s], &w.firms);
        let mean_ed = if alive.is_empty() {
            0.0
        } else {
            alive.iter().map(|&f| finite_or(expected[f as usize].expected_demand, 0.0).max(0.0)).sum::<f64>()
                / alive.len() as f64
        };
        st.share_cum[s].build(alive.iter().map(|&f| {
            if logit {
                // b6-firmsize S5: choice weight a_f · p_f^(−ε); no entrant floor (legacy only).
                let x = st.appeal_ln.get(f as usize).copied().unwrap_or(0.0);
                return logit_choice_weight(x, st.price[f as usize], demand_eps) + 1e-9;
            }
            let fm = &expected[f as usize];
            let ed = finite_or(fm.expected_demand, 0.0).max(0.0);
            let ed = if fm.age_months < intro_months { ed.max(customer_floor * mean_ed) } else { ed };
            ed + 1e-9
        }));
    }
    // Step 2's first-pass demand per household (preferred firm after switching) is computed here, while the
    // household is already in hand; the per-firm sums are accumulated in household order in step 2.
    st.hh_dem.clear();
    st.hh_dem.resize(nh, [0.0; N_CONSUMER]);
    st.slim.clear();
    st.slim.resize(nh, HhSlim { id: 0, alive: false, pref: [NO_FIRM; N_CONSUMER] });
    {
        let seller: &[u8] = &st.seller;
        let alive = &st.alive;
        let price = &st.price;
        let share_cum = &st.share_cum;
        let last_fill = &st.last_fill;
        let orders = &orders;
        let sample = |r: &mut SimRng, s: usize| -> FirmId {
            let cdf = &share_cum[s];
            let u = r.gen::<f64>() * cdf.total();
            alive[s][cdf.pick(u)]
        };
        hhs.par_iter_mut()
            .zip(st.slim.par_iter_mut())
            .zip(st.hh_dem.par_iter_mut())
            .enumerate()
            .for_each(|(i, ((hh, sl), dm))| {
            sl.id = hh.id as u64;
            sl.alive = hh.alive;
            if !hh.alive {
                sl.pref = [NO_FIRM; N_CONSUMER];
                return;
            }
            let o = clean(orders.get(i).copied().unwrap_or_default());
            let mut r = rng::agent_rng(seed, tick, stream::GOODS, hh.id as u64);
            for s in 0..N_CONSUMER {
                let list = &alive[s];
                if list.is_empty() {
                    continue;
                }
                let pref = hh.preferred_firm[s];
                if !is_seller(seller, pref, s) {
                    hh.preferred_firm[s] = sample(&mut r, s);
                } else if last_fill[pref as usize] < 0.98 && r.gen::<f64>() < RATIONED_SWITCH_PROB {
                    // Customers rationed last month look elsewhere.
                    hh.preferred_firm[s] = sample(&mut r, s);
                } else if rechoice > 0.0 && r.gen::<f64>() < rechoice {
                    // b6-firmsize S5: a random re-draw in proportion to the choice weight, with no price test.
                    hh.preferred_firm[s] = sample(&mut r, s);
                } else if r.gen::<f64>() < switch_prob {
                    let cand = sample(&mut r, s);
                    let (p_old, p_new) = (price[pref as usize], price[cand as usize]);
                    // Delli Gatti et al. (2011) switching rule. p1-firmsentry: with `entrant_tie_switch` a tie no longer
                    // blocks the switch (a coin toss), so equally priced firms, including entrants, can win customers.
                    if cand != pref && (p_new < p_old || (tie_switch && p_new == p_old)) {
                        let prob = if p_new < p_old {
                            1.0 - (k_switch * (p_new - p_old) / p_new).exp()
                        } else {
                            TIE_SWITCH_PROB
                        };
                        if r.gen::<f64>() < prob {
                            hh.preferred_firm[s] = cand;
                        }
                    }
                }
            }
            for s in 0..N_CONSUMER {
                sl.pref[s] = hh.preferred_firm[s];
                dm[s] = o[s] * (1.0 - m_share[s]) * net;
            }
        });
    }

    // ---- 2. first pass: domestic demand at preferred firms, pro-rata fills (household order)
    // Per consumer sector (in parallel): the first-pass demand of each seller, summed over households in household
    // order. A sector's preferred firms are its own sellers, so the sectors touch disjoint firms.
    {
        let (slim, hh_dem, seller, pos, alive) = (&st.slim, &st.hh_dem, &st.seller, &st.sec_pos, &st.alive);
        let sums: Vec<Vec<f64>> = (0..N_CONSUMER)
            .into_par_iter()
            .map(|s| {
                let mut acc = vec![0.0; alive[s].len()];
                for (sl, dm) in slim.iter().zip(hh_dem) {
                    let pref = sl.pref[s];
                    if is_seller(seller, pref, s) {
                        acc[pos[pref as usize] as usize] += dm[s];
                    }
                }
                acc
            })
            .collect();
        for s in 0..N_CONSUMER {
            for (k, &f) in alive[s].iter().enumerate() {
                st.dem[f as usize] += sums[s][k];
            }
        }
    }
    for f in 0..nf {
        if st.dem[f] > 0.0 {
            st.fill[f] = settle(&mut w.firms[f], &mut st.avail[f], st.dem[f], st.price[f]);
        }
    }

    // ---- 3. retry pass: a rationed customer looks for a seller that still has stock — the retry firm is drawn in
    //         proportion to remaining stock after the first pass (search directed at available supply).
    for s in 0..N_CONSUMER {
        let (alive, avail) = (&st.alive[s], &st.avail);
        st.cum[s].build(alive.iter().map(|&f| avail[f as usize].max(0.0)));
    }
    let nchunks = nh.div_ceil(PAY_CHUNK);
    st.retry_parts.resize_with(nchunks, Vec::new);
    for part in st.retry_parts.iter_mut() {
        part.clear();
    }
    {
        let seller: &[u8] = &st.seller;
        let fill = &st.fill;
        let cum = &st.cum;
        let alive = &st.alive;
        let dem: &[[f64; N_CONSUMER]] = &st.hh_dem;
        let slim: &[HhSlim] = &st.slim;
        st.choice
            .par_chunks_mut(PAY_CHUNK)
            .zip(st.retry_parts.par_iter_mut())
            .zip(slim.par_chunks(PAY_CHUNK).zip(dem.par_chunks(PAY_CHUNK)))
            .for_each(|((chs, part), (slc, dmc))| {
                for (choice, (sl, dm)) in chs.iter_mut().zip(slc.iter().zip(dmc)) {
                    *choice = [NO_FIRM; N_CONSUMER];
                    if !sl.alive {
                        continue;
                    }
                    // Seeded on first use: the stream is keyed by household, so lazy seeding draws the same numbers.
                    let mut r: Option<SimRng> = None;
                    for s in 0..N_CONSUMER {
                        let pref = sl.pref[s];
                        let d = dm[s];
                        let paid1 = if is_seller(seller, pref, s) { d * fill[pref as usize] } else { 0.0 };
                        let unmet1 = d - paid1;
                        let total = cum[s].total();
                        if unmet1 > 0.0 && total > 0.0 {
                            let rng = r.get_or_insert_with(|| {
                                rng::agent_rng(seed, tick, stream::GOODS, RETRY_KEY | sl.id)
                            });
                            let u = rng.gen::<f64>() * total;
                            let alt = alive[s][cum[s].pick(u)];
                            if alt != pref {
                                choice[s] = alt;
                                part.push((alt, unmet1));
                            }
                        }
                    }
                }
            });
    }
    for part in &st.retry_parts {
        for &(alt, amt) in part {
            st.dem_alt[alt as usize] += amt;
        }
    }
    for f in 0..nf {
        if st.dem_alt[f] > 0.0 {
            st.fill_alt[f] = settle_retry(&mut w.firms[f], &mut st.avail[f], st.dem_alt[f], st.price[f]);
        }
    }

    w.households = hhs;

    // ---- 4+5. per-household split and payments (sequential, through the ledger). The split is the same
    //           arithmetic as a separate parallel pass: paid to the preferred firm, paid to the retry firm, imported
    //           (goods: plus unmet goods budget), and unspent (non-goods unmet budget, gross of sales tax).
    st.unspent.clear();
    st.unspent.resize(nh, [0.0; N_CONSUMER]);
    // Fixed-size chunks (independent of the thread count) so the merged sums are deterministic. Each chunk
    // debits its own households and accumulates receipts into a `PayBatch`; batches are merged in chunk order.
    let (imp_hh, tariff_hh, tax_hh);
    let (cust_cov, tariff_by_sector, legacy_hh);
    {
        let seller: &[u8] = &st.seller;
        let fill = &st.fill;
        let fill_alt = &st.fill_alt;
        let choice = &st.choice;
        let slim: &[HhSlim] = &st.slim;
        let orders = &orders;
        let row_bank = w.row.bank as usize;
        let (nf, nb) = (w.firms.len(), w.banks.len());
        let mut hhs = std::mem::take(&mut w.households);
        let parts: Vec<(ledger::PayBatch, PayTotals, Vec<u32>)> = hhs
            .par_chunks_mut(PAY_CHUNK)
            .zip(st.unspent.par_chunks_mut(PAY_CHUNK))
            .zip(w.fills.household_unspent.par_chunks_mut(PAY_CHUNK))
            .zip(slim.par_chunks(PAY_CHUNK))
            .zip(w.hh_month.par_chunks_mut(PAY_CHUNK))
            .enumerate()
            .map(|(c, ((((hs, unsp), hu), slc), mons))| {
                let base = c * PAY_CHUNK;
                let mut batch = ledger::PayBatch::new(nf, nb, row_bank);
                let mut tot = PayTotals::new();
                let mut left: Vec<u32> = Vec::new();
                for (j, (hh, mon)) in hs.iter_mut().zip(mons.iter_mut()).enumerate() {
                    let h = base + j;
                    let mut spent = [0.0; N_CONSUMER];
                    let (mut imp_h, mut tax_h, mut tariff_h, mut unspent_h) = (0.0, 0.0, 0.0, 0.0);
                    if hh.alive {
                        let o = clean(orders.get(h).copied().unwrap_or_default());
                        let pref_row = slc[j].pref;
                        for s in 0..N_CONSUMER {
                            let d = o[s] * (1.0 - m_share[s]) * net;
                            let imp = o[s] * m_share[s] * net;
                            let pref = pref_row[s];
                            let paid1 = if is_seller(seller, pref, s) { d * fill[pref as usize] } else { 0.0 };
                            let unmet1 = (d - paid1).max(0.0);
                            let alt = choice[h][s];
                            let paid2 = if alt != NO_FIRM { unmet1 * fill_alt[alt as usize] } else { 0.0 };
                            let unmet2 = (unmet1 - paid2).max(0.0);
                            // Unmet tradables budget is imported; other sectors' unmet budget is unspent.
                            let sp = if tradable[s] {
                                tot.unmet_imp += unmet2; // b7-spikes diagnostic: unmet part of the imported budget
                                Split { pref: paid1, alt: paid2, imported: imp + unmet2, unspent: 0.0 }
                            } else {
                                Split { pref: paid1, alt: paid2, imported: imp, unspent: unmet2 * gross_up }
                            };
                            unsp[j][s] = sp.unspent;
                            // ---- payments for this sector (debit the household now; receipts batched)
                            batch.hh_pay(hh, ledger::Payee::Firm(pref as usize), sp.pref);
                            if sp.alt > 0.0 {
                                batch.hh_pay(hh, ledger::Payee::Firm(alt as usize), sp.alt);
                            }
                            let dom = sp.pref + sp.alt;
                            let tax_s = tax * (dom + sp.imported);
                            if duty_hh[s] {
                                // b7-tariff (M3): the import budget splits into customs value (to RoW) and the duty
                                // wedge (to Government); the household pays the budget once, with no overshoot.
                                let cust = sp.imported / (1.0 + tp.retail);
                                let wedge = sp.imported - cust;
                                spent[s] = dom + sp.imported + tax_s;
                                imp_h += cust;
                                tot.imp_s[s] += cust;
                                tariff_h += wedge;
                                tot.cust_cov[s] += cust;
                                tot.tariff_by_sector[s] += wedge;
                            } else {
                                let tariff_s = if tp.active { 0.0 } else { tariff * sp.imported };
                                spent[s] = dom + sp.imported + tax_s + tariff_s;
                                imp_h += sp.imported;
                                tot.imp_s[s] += sp.imported;
                                tariff_h += tariff_s;
                                tot.legacy += tariff_s;
                            }
                            tot.consumption[s] += spent[s];
                            tax_h += tax_s;
                            unspent_h += sp.unspent;
                            tot.unspent_by_sector[s] += sp.unspent;
                        }
                    } else {
                        unsp[j] = [0.0; N_CONSUMER];
                    }
                    mon.consumption = spent;
                    batch.hh_pay(hh, ledger::Payee::RestOfWorld, imp_h);
                    batch.hh_pay(hh, ledger::Payee::Government, tax_h + tariff_h);
                    tot.imp += imp_h;
                    tot.tariff += tariff_h;
                    tot.tax += tax_h;
                    hu[j] += unspent_h;
                    if unsp[j].iter().any(|&x| x > 0.0) {
                        left.push((base + j) as u32);
                    }
                }
                (batch, tot, left)
            })
            .collect();
        w.households = hhs;

        // Receipts summed over chunks in chunk order (parallel over firms; banks and scalars are small).
        let mut batch = ledger::PayBatch::new(nf, nb, row_bank);
        batch
            .firm
            .par_iter_mut()
            .enumerate()
            .for_each(|(f, x)| *x = parts.iter().map(|(b, _, _)| b.firm[f]).sum());
        let mut tot = PayTotals::new();
        st.left_hh.clear();
        for (b, t, left) in &parts {
            for (a, x) in batch.bank.iter_mut().zip(&b.bank) {
                *a += x;
            }
            batch.row += b.row;
            batch.gov += b.gov;
            tot.merge(t);
            st.left_hh.extend_from_slice(left);
        }
        // b8-world H9: the household imports by sector (customs value, duty excluded) into the month's flows.
        for s in 0..N_SECTORS {
            w.flows.imports_by_sector[s] += tot.imp_s[s];
        }
        batch.apply(w);
        for s in 0..N_CONSUMER {
            w.flows.consumption[s] += tot.consumption[s];
            st.unspent_by_sector[s] = tot.unspent_by_sector[s];
        }
        imp_hh = tot.imp;
        tariff_hh = tot.tariff;
        tax_hh = tot.tax;
        cust_cov = tot.cust_cov;
        tariff_by_sector = tot.tariff_by_sector;
        legacy_hh = tot.legacy;
        w.flows.hh_unmet_imports += tot.unmet_imp; // b7-spikes diagnostic
    }
    // b7-tariff: with pass-through, imports are customs value only (BEA); the duty wedge is in consumption.
    w.flows.imports += if tp.active { imp_hh } else { imp_hh + tariff_hh };
    w.row.imports_this_month += imp_hh;
    w.gov.revenue += tax_hh + tariff_hh;
    w.flows.tariff_hh = tariff_by_sector;
    w.flows.tariff_legacy += legacy_hh;

    // ---- 5b. sector-level clearing of leftovers: budgets still unspent after the retry are matched to any stock
    //          remaining in the sector (a deterministic walk over households and firms), so output does not go
    //          unsold while demand goes unmet (search in thin retail markets eventually finds the seller).
    //          The walk is planned in household order over compact arrays only (sectors are independent, so the
    //          per-sector firm pointers and stock are the same as a sector-by-sector walk); the balances are then
    //          applied: firms in allocation order, households in parallel over fixed chunks.
    st.allocs.clear();
    let mut fi = [0usize; N_CONSUMER];
    let active: [bool; N_CONSUMER] = std::array::from_fn(|s| st.unspent_by_sector[s] > 0.0);
    for &h in &st.left_hh {
        let hu = h as usize;
        for s in 0..N_CONSUMER {
            if !active[s] {
                continue;
            }
            let mut left = st.unspent[hu][s];
            if left <= 0.0 {
                continue;
            }
            let firms_s = &st.alive[s];
            while left > 1e-9 && fi[s] < firms_s.len() {
                let f = firms_s[fi[s]] as usize;
                let price = st.price[f];
                let stock_value = st.avail[f].max(0.0) * price * gross_up;
                if stock_value <= 1e-9 {
                    fi[s] += 1;
                    continue;
                }
                let gross = left.min(stock_value);
                let net_paid = gross / gross_up;
                let real = net_paid / price;
                st.avail[f] = (st.avail[f] - real).max(0.0);
                st.allocs.push(Walk { h, s: s as u8, f: f as u32, gross, net: net_paid, real });
                left -= gross;
            }
        }
    }
    // Firms, in allocation order (each allocation: the sale, and the buyer's payment as `ledger::pay_hh_to_firm`).
    for a in &st.allocs {
        let fm = &mut w.firms[a.f as usize];
        fm.sales += a.real;
        fm.inventory = (fm.inventory - a.real).max(0.0);
        fm.revenue += a.net;
        if a.net > 0.0 && a.net.is_finite() {
            fm.deposits += a.net;
            let b = fm.bank as usize;
            w.banks[b].deposits += a.net;
            w.banks[b].reserves += a.net;
        }
    }
    // Households (and the sales tax, to the TGA), in parallel over fixed household chunks; allocations are sorted by
    // household, so each chunk's allocations are a contiguous range.
    let nb = w.banks.len();
    let allocs: &[Walk] = &st.allocs;
    let parts: Vec<(Vec<f64>, f64, [f64; N_CONSUMER], [f64; N_CONSUMER])> = w
        .households
        .par_chunks_mut(PAY_CHUNK)
        .zip(w.fills.household_unspent.par_chunks_mut(PAY_CHUNK))
        .zip(w.hh_month.par_chunks_mut(PAY_CHUNK))
        .enumerate()
        .map(|(c, ((hs, hu), mons))| {
            let base = c * PAY_CHUNK;
            let lo = allocs.partition_point(|a| (a.h as usize) < base);
            let hi = allocs.partition_point(|a| (a.h as usize) < base + hs.len());
            let mut bank = vec![0.0; nb];
            let mut tax_sum = 0.0;
            let mut cons = [0.0; N_CONSUMER];
            let mut unsp = [0.0; N_CONSUMER];
            for a in &allocs[lo..hi] {
                let j = a.h as usize - base;
                let s = a.s as usize;
                let hh = &mut hs[j];
                let mon = &mut mons[j];
                if a.net > 0.0 && a.net.is_finite() {
                    hh.deposits -= a.net;
                    bank[hh.bank as usize] -= a.net;
                }
                let tax_s = a.gross - a.net;
                if tax_s > 0.0 {
                    hh.deposits -= tax_s;
                    bank[hh.bank as usize] -= tax_s;
                    tax_sum += tax_s;
                }
                mon.consumption[s] += a.gross;
                cons[s] += a.gross;
                hu[j] -= a.gross;
                unsp[s] += a.gross;
            }
            (bank, tax_sum, cons, unsp)
        })
        .collect();
    for (bank, tax, cons, unsp) in &parts {
        for (b, &x) in bank.iter().enumerate() {
            if x != 0.0 {
                w.banks[b].deposits += x;
                w.banks[b].reserves += x;
            }
        }
        if *tax != 0.0 {
            w.cb.tga += tax;
            w.gov.revenue += tax;
        }
        for s in 0..N_CONSUMER {
            w.flows.consumption[s] += cons[s];
            st.unspent_by_sector[s] -= unsp[s];
        }
    }

    // b7-tariff (M4): importers absorb the duty above the retail wedge (revenue at the end of clear).
    if tp.active {
        settle_tariff_gap(w, &cust_cov, tp.tau, tp.retail);
    }

    // ---- 9. sector prices (sales-weighted) and CPI / core CPI (§13.4)
    let mut num = [0.0; N_SECTORS];
    let mut den = [0.0; N_SECTORS];
    for (i, f) in w.firms.iter().enumerate() {
        if !f.alive || !(f.sales > 0.0) {
            continue;
        }
        let s = f.sector.idx();
        num[s] += f.sales * st.price[i];
        den[s] += f.sales;
    }
    for s in 0..N_SECTORS {
        if den[s] > 0.0 {
            let p = num[s] / den[s];
            if p.is_finite() && p > 0.0 {
                w.prices.sector[s] = p;
            }
        }
    }
    if let Some(base) = st.cpi_base {
        let rent_rel = finite_or(lease_rent(w) / base.lease, 1.0);
        let rel = |s: usize| finite_or(sector_price(w, Sector::from_idx(s)) / base.sector[s], 1.0);
        // Tradables: domestic goods and imports (priced in dollars over the dollar index), each relative to t=0.
        // b7-tariff (M6): the import component carries the retail wedge (fixed Laspeyres weights).
        let wedge = if tp.active { tp.retail } else { 0.0 };
        let g = tradables_price(m_cpi_goods, role_mean(&cpi_tw, rel), imp_rel_cpi_v, wedge);
        // Price index of each CPI component: tradables use the blend above, other sectors their sector price.
        let x = |s: usize| if cpi_tradable[s] { g } else { rel(s) };
        let cfg = &w.cfg;
        let shelter = cfg.cpi_weight_shelter * rent_rel;
        let headline = 100.0 * role_sum_from(shelter, &cfg.cpi_weights, x);
        // Core CPI: the core sectors' components, with the weights normalised by their own sum.
        let core_weights: [f64; N_SECTORS] =
            std::array::from_fn(|s| if cfg.cpi_core_sectors[s] { cfg.cpi_weights[s] } else { 0.0 });
        let core_w = role_sum_from(cfg.cpi_weight_shelter, &core_weights, |_| 1.0);
        let core = 100.0 * role_sum_from(shelter, &core_weights, x) / core_w;
        let pce = 100.0 * role_sum_from(cfg.pce_core_weight_shelter * rent_rel, &cfg.pce_core_weights, x);
        let cpi = if headline.is_finite() { headline } else { w.prices.cpi };
        let core = if core.is_finite() { core } else { w.prices.core_cpi };
        w.prices.cpi = cpi;
        w.prices.core_cpi = core;
        push_capped(&mut w.prices.cpi_history, cpi);
        push_capped(&mut w.prices.core_history, core);
        let pce = if pce.is_finite() { pce } else { w.prices.core_pce };
        w.prices.core_pce = pce;
        push_capped(&mut w.prices.core_pce_history, pce);
    }

    w.orders.household = orders;
    st.tariff_month = tp;
    w.goods = st;
}

/// Sell to a single buyer type (government or RoW) across all alive firms of each sector, pro rata to available
/// stock. Returns the amount paid per sector.
/// Supplier weight for routing non-household orders: available stock plus a share of expected demand, so a
/// sold-out firm still registers demand (and can expand) instead of becoming invisible to buyers.
#[inline]
fn supplier_weight(avail: f64, fm: &Firm) -> f64 {
    avail.max(0.0) + ROUTING_DEMAND_WEIGHT * finite_or(fm.expected_demand, 0.0).max(0.0)
}

fn pool_sales(w: &mut World, st: &mut GoodsState, buyer: Account, budget: [f64; N_SECTORS]) -> [f64; N_SECTORS] {
    let mut paid_by_sector = [0.0; N_SECTORS];
    for s in 0..N_SECTORS {
        let x = finite_or(budget[s], 0.0).max(0.0);
        if x <= 0.0 {
            continue;
        }
        let total: f64 = st.alive[s].iter().map(|&f| supplier_weight(st.avail[f as usize], &w.firms[f as usize])).sum();
        if total <= 0.0 {
            continue;
        }
        for i in 0..st.alive[s].len() {
            let f = st.alive[s][i] as usize;
            // Each firm's weight is read before it is settled, so this equals the weight computed up front.
            let nominal = x * supplier_weight(st.avail[f], &w.firms[f]) / total;
            if nominal <= 0.0 {
                continue;
            }
            let fill = settle(&mut w.firms[f], &mut st.avail[f], nominal, st.price[f]);
            let paid = fill * nominal;
            if paid > 0.0 {
                ledger::pay(w, buyer, Account::Firm(f as FirmId), paid);
            }
            paid_by_sector[s] += paid;
        }
    }
    paid_by_sector
}

/// b7-spikes F2: pooled firm-order routing (`Config::firm_order_pool`). Each buyer's domestic order in a sector is
/// spread over all alive suppliers in proportion to their weights, with no random draws: the sector's pooled order
/// X_s = sum_b x_bs is shared out as D_f = X_s w_f / W_s and each supplier is settled once. Buyers pay their share
/// P_s / X_s of the suppliers' receipts, and the unpaid rest is imported as in the legacy path. Returns the net
/// deposit changes for `finish_firm_stage`. Sequential loops in buyer, sector and supplier order (RAYON-independent).
///
/// Open item (joint3): pooled routing is not wired to b7-tariff's covered-order duty bookkeeping (`Draw.cov`,
/// `st.unmet_cov`). With an active tariff, `firm_order_pool` = true under-collects the duty on covered unmet orders.
/// Logic unchanged from b7-spikes.
fn route_pooled(
    w: &mut World,
    st: &mut GoodsState,
    orders: &[[f64; N_SECTORS]],
    usage: FirmUse,
    m_goods: f64,
) -> Vec<f64> {
    let nf = w.firms.len();
    let nb = nf.min(orders.len());
    let capital_ok: [bool; N_SECTORS] = std::array::from_fn(|s| w.cfg.capital_goods_mix[s] != 0.0);
    let tradable: [bool; N_SECTORS] = std::array::from_fn(|s| w.cfg.tradable_weight[s] != 0.0);
    // 1. Domestic orders after the imported share, which goes to `unmet` exactly as in the legacy path.
    let mut x = vec![[0.0f64; N_SECTORS]; nb];
    for b in 0..nb {
        if !w.firms[b].alive {
            continue;
        }
        for s in 0..N_SECTORS {
            let o = finite_or(orders[b][s], 0.0);
            if o <= 0.0 || (usage == FirmUse::Investment && !capital_ok[s]) {
                continue;
            }
            let m = if tradable[s] { m_goods } else { 0.0 };
            if m > 0.0 {
                st.unmet[b] += m * o;
                st.imp_s[s] += m * o;
            }
            x[b][s] = (1.0 - m) * o;
        }
    }
    // 2. Supplier weights, read before any settlement.
    let mut wgt = vec![0.0f64; nf];
    let mut wsum = [0.0f64; N_SECTORS];
    for s in 0..N_SECTORS {
        for i in 0..st.alive[s].len() {
            let f = st.alive[s][i] as usize;
            wgt[f] = supplier_weight(st.avail[f], &w.firms[f]);
            wsum[s] += wgt[f];
        }
    }
    // 3. Pooled order per sector. A sector with no supplier weight is imported in full, as in the legacy path.
    let mut xs = [0.0f64; N_SECTORS];
    for s in 0..N_SECTORS {
        if wsum[s] <= 0.0 {
            for b in 0..nb {
                if x[b][s] > 0.0 {
                    st.unmet[b] += x[b][s];
                    st.imp_s[s] += x[b][s];
                    x[b][s] = 0.0;
                }
            }
        } else {
            for b in 0..nb {
                xs[s] += x[b][s];
            }
        }
    }
    // 4. Each supplier is settled once on its share D_f. P_s is the receipts and Q_s the units sold by sector.
    let mut recv = vec![0.0f64; nf];
    let mut p_s = [0.0f64; N_SECTORS];
    let mut q_s = [0.0f64; N_SECTORS];
    for s in 0..N_SECTORS {
        if xs[s] <= 0.0 {
            continue;
        }
        for i in 0..st.alive[s].len() {
            let f = st.alive[s][i] as usize;
            let d = xs[s] * wgt[f] / wsum[s];
            if d <= 0.0 {
                continue;
            }
            let phi = settle(&mut w.firms[f], &mut st.avail[f], d, st.price[f]);
            st.fill[f] = phi;
            let got = phi * d;
            recv[f] = got;
            p_s[s] += got;
            q_s[s] += got / st.price[f];
        }
    }
    // 5. Deposits: suppliers receive phi_f D_f; buyers pay P_s x_bs / X_s. Unpaid input and capital orders are imported.
    let mut net = std::mem::take(&mut st.firm_net);
    net.clear();
    net.resize(nf, 0.0);
    for f in 0..nf {
        net[f] += recv[f];
    }
    for b in 0..nb {
        for s in 0..N_SECTORS {
            let xb = x[b][s];
            if xb <= 0.0 || xs[s] <= 0.0 {
                continue;
            }
            let paid = xb * p_s[s] / xs[s];
            let unmet = xb - paid;
            w.flows.firm_shortfall_imports += unmet;
            if paid > 0.0 && paid.is_finite() {
                net[b] -= paid;
            }
            match usage {
                FirmUse::Inputs => {
                    w.firms[b].input_cost += paid;
                    w.flows.intermediate += paid;
                    st.unmet[b] += unmet;
                    st.imp_s[s] += unmet;
                }
                FirmUse::Investment => {
                    let units_per_paid = if p_s[s] > 0.0 { q_s[s] / p_s[s] } else { 0.0 };
                    w.fills.firm_capital_received[b] += paid * units_per_paid;
                    w.flows.investment += paid;
                    st.unmet[b] += unmet;
                    st.imp_s[s] += unmet;
                }
            }
        }
    }
    net
}

/// Clear firm-to-firm orders (intermediate inputs or capital goods). Each buyer's budget per sector is split
/// over `FIRM_DRAWS` suppliers drawn with probability proportional to available stock. Unmet input orders are
/// imported from RoW at the sector price (production is never input-constrained); unmet capital orders go
/// unfilled.
fn firm_stage(
    w: &mut World,
    st: &mut GoodsState,
    orders: &[[f64; N_SECTORS]],
    usage: FirmUse,
    tariff: f64,
    m_goods: f64,
    active: bool,
) {
    let nf = w.firms.len();
    let (tick, seed) = (w.tick, w.seed);
    st.unmet_cov.clear();
    st.unmet_cov.resize(nf, 0.0);
    // b7-spikes F2: pooled routing replaces the random supplier draws; the deposit and import tail is shared.
    // The covered-unmet reset above runs first, so `st.unmet_cov` is sized on both routes (F2 adds nothing to it).
    if w.cfg.firm_order_pool {
        let net = route_pooled(w, st, orders, usage, m_goods);
        finish_firm_stage(w, st, net, usage, tariff, active);
        return;
    }
    let key = match usage {
        FirmUse::Inputs => STREAM_INPUTS,
        FirmUse::Investment => STREAM_INVESTMENT,
    };

    // cumulative available stock per sector, for weighted sampling
    for s in 0..N_SECTORS {
        let (alive, firms, avail) = (&st.alive[s], &w.firms, &st.avail);
        st.cum[s].build(alive.iter().map(|&f| supplier_weight(avail[f as usize], &firms[f as usize])));
    }

    // Draws per buyer are independent (each buyer has its own RNG stream), so buyers are drawn in parallel over
    // fixed-size chunks and the chunks are concatenated in buyer order.
    const DRAW_CHUNK: usize = 64;
    let nb = nf.min(orders.len());
    // Sectors that can be ordered as capital goods (`Config::capital_goods_mix`).
    let capital_ok: [bool; N_SECTORS] = std::array::from_fn(|s| w.cfg.capital_goods_mix[s] != 0.0);
    let tradable: [bool; N_SECTORS] = std::array::from_fn(|s| w.cfg.tradable_weight[s] != 0.0);
    let parts: Vec<Vec<Draw>> = {
        let (firms, cum, alive) = (&w.firms, &st.cum, &st.alive);
        orders[..nb]
            .par_chunks(DRAW_CHUNK)
            .enumerate()
            .map(|(c, ords)| {
                let mut out = Vec::new();
                for (j, ord) in ords.iter().enumerate() {
                    let b = c * DRAW_CHUNK + j;
                    if !firms[b].alive {
                        continue;
                    }
                    let mut r: Option<SimRng> = None;
                    for s in 0..N_SECTORS {
                        let x = finite_or(ord[s], 0.0);
                        if x <= 0.0 || (usage == FirmUse::Investment && !capital_ok[s]) {
                            continue;
                        }
                        // A share of firms' manufactured inputs and equipment is bought abroad.
                        // Imported share of tradables inputs and equipment (`m_goods`); other sectors are domestic.
                        let m = if tradable[s] { m_goods } else { 0.0 };
                        if m > 0.0 {
                            out.push(Draw { buyer: b as FirmId, supplier: NO_FIRM, nominal: m * x, cov: tradable[s], sector: s as u8 });
                        }
                        let x = (1.0 - m) * x;
                        let total = cum[s].total();
                        if total <= 0.0 {
                            out.push(Draw { buyer: b as FirmId, supplier: NO_FIRM, nominal: x, cov: tradable[s], sector: s as u8 });
                            continue;
                        }
                        let rng = r.get_or_insert_with(|| rng::agent_rng(seed, tick, stream::GOODS, key | b as u64));
                        let per = x / FIRM_DRAWS as f64;
                        for _ in 0..FIRM_DRAWS {
                            let u = rng.gen::<f64>() * total;
                            let supplier = alive[s][cum[s].pick(u)];
                            out.push(Draw { buyer: b as FirmId, supplier, nominal: per, cov: tradable[s], sector: s as u8 });
                        }
                    }
                }
                out
            })
            .collect()
    };
    // supplier-level fills
    for d in parts.iter().flatten() {
        if d.supplier != NO_FIRM {
            st.dem[d.supplier as usize] += d.nominal;
        }
    }
    for f in 0..nf {
        if st.dem[f] > 0.0 {
            st.fill[f] = settle(&mut w.firms[f], &mut st.avail[f], st.dem[f], st.price[f]);
        }
        st.dem[f] = 0.0;
    }

    // buyer-level payments. Firm-to-firm deposit transfers never check balances, so they are netted per firm and
    // applied once below (the same updates `ledger::pay` makes, summed).
    let mut net = std::mem::take(&mut st.firm_net);
    net.clear();
    net.resize(nf, 0.0);
    for &d in parts.iter().flatten() {
        let b = d.buyer as usize;
        if d.supplier == NO_FIRM {
            st.unmet[b] += d.nominal;
            st.imp_s[d.sector as usize] += d.nominal;
            if active && d.cov {
                st.unmet_cov[b] += d.nominal;
            }
            continue;
        }
        let sup = d.supplier as usize;
        let paid = d.nominal * st.fill[sup];
        let unmet = d.nominal - paid;
        if active && d.cov {
            st.unmet_cov[b] += unmet;
        }
        w.flows.firm_shortfall_imports += unmet; // b7-spikes diagnostic: order not paid by a domestic supplier
        if paid > 0.0 && paid.is_finite() && b != sup {
            net[b] -= paid;
            net[sup] += paid;
        }
        match usage {
            FirmUse::Inputs => {
                w.firms[b].input_cost += paid;
                w.flows.intermediate += paid;
                st.unmet[b] += unmet;
                st.imp_s[d.sector as usize] += unmet;
            }
            FirmUse::Investment => {
                w.fills.firm_capital_received[b] += paid / st.price[sup];
                w.flows.investment += paid;
                st.unmet[b] += unmet;
                st.imp_s[d.sector as usize] += unmet;
            }
        }
    }

    finish_firm_stage(w, st, net, usage, tariff, active);
}

/// Applies the firm-to-firm deposit changes `net` and imports the unmet input and capital-goods orders. Shared by
/// the legacy draw routing and the pooled routing (`Config::firm_order_pool`). `active` is the b7-tariff pass-through
/// flag (`TariffMonth::active`), as in `firm_stage`.
fn finish_firm_stage(
    w: &mut World,
    st: &mut GoodsState,
    net: Vec<f64>,
    usage: FirmUse,
    tariff: f64,
    active: bool,
) {
    let nf = w.firms.len();
    // b8-world H9: this stage's unmet imports by sector (customs value, duty excluded) into the month's flows.
    for s in 0..N_SECTORS {
        w.flows.imports_by_sector[s] += st.imp_s[s];
        st.imp_s[s] = 0.0;
    }
    for (f, &x) in net.iter().enumerate() {
        if x != 0.0 {
            let fm = &mut w.firms[f];
            fm.deposits += x;
            let b = &mut w.banks[fm.bank as usize];
            b.deposits += x;
            b.reserves += x;
        }
    }
    st.firm_net = net;

    // Unmet input and capital-goods orders are imported at customs value; covered unmet orders also pay the duty to
    // Government (legacy path: every unmet order pays the lump-sum duty). Production is never input-constrained, and
    // roughly a third of US equipment investment is imported anyway.
    let goods_p = pos_or_one(role_mean(&w.cfg.capital_valuation_mix, |s| w.prices.sector[s]));
    {
        for b in 0..nf {
            let u = st.unmet[b];
            st.unmet[b] = 0.0;
            // b7-tariff (M5): the duty falls on the covered unmet orders only; the customs value u goes to RoW.
            let u_cov = std::mem::take(&mut st.unmet_cov[b]);
            if u <= 0.0 {
                continue;
            }
            if active {
                ledger::pay(w, Account::Firm(b as FirmId), Account::RestOfWorld, u);
                let t = tariff * u_cov;
                if t > 0.0 {
                    ledger::pay(w, Account::Firm(b as FirmId), Account::Government, t);
                }
                match usage {
                    FirmUse::Inputs => {
                        w.firms[b].input_cost += u + t;
                        w.flows.tariff_inputs += t;
                    }
                    FirmUse::Investment => {
                        w.fills.firm_capital_received[b] += u / goods_p;
                        w.flows.investment += u + t;
                        w.flows.tariff_equipment += t;
                    }
                }
                w.flows.imports += u;
                w.row.imports_this_month += u;
                w.gov.revenue += t;
                continue;
            }
            ledger::pay(w, Account::Firm(b as FirmId), Account::RestOfWorld, u);
            let t = tariff * u;
            if t > 0.0 {
                ledger::pay(w, Account::Firm(b as FirmId), Account::Government, t);
            }
            match usage {
                FirmUse::Inputs => w.firms[b].input_cost += u + t,
                FirmUse::Investment => {
                    w.fills.firm_capital_received[b] += u / goods_p;
                    w.flows.investment += u + t;
                }
            }
            w.flows.imports += u + t;
            w.row.imports_this_month += u;
            w.gov.revenue += t;
            w.flows.tariff_legacy += t;
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::consumption;
    use crate::ledger::check_consistency;
    use crate::sim::begin_month;
    use crate::testkit::tiny_world;

    /// Tiny world with `inv` units of stock in every firm, after `begin_month`.
    fn world(n: usize, seed: u64, inv: f64) -> World {
        // Tests that check exact budget identities run without sales tax (it is tested separately).
        let mut w = tiny_world(
            Config {
                n_households: n,
                sales_tax: 0.0,
                ..Default::default()
            },
            seed,
        );
        begin_month(&mut w);
        for f in w.firms.iter_mut() {
            f.inventory = inv;
        }
        w
    }

    fn sum_orders(w: &World) -> [f64; N_CONSUMER] {
        let mut t = [0.0; N_CONSUMER];
        for o in &w.orders.household {
            for s in 0..N_CONSUMER {
                t[s] += o[s];
            }
        }
        t
    }

    #[test]
    fn ample_stock_clears_every_budget() {
        let mut w = world(500, 3, 1e7);
        consumption::decide(&mut w);
        let budget = sum_orders(&w);
        clear(&mut w);
        check_consistency(&w).unwrap();
        for s in 0..N_CONSUMER {
            let spent: f64 = w.hh_month.iter().map(|m| m.consumption[s]).sum();
            assert!(
                (spent - budget[s]).abs() <= 1e-6 * budget[s].max(1.0),
                "sector {s}: spent {spent} vs budget {}",
                budget[s]
            );
            assert!((w.flows.consumption[s] - spent).abs() <= 1e-6 * spent.max(1.0));
        }
        assert!(w.fills.household_unspent.iter().all(|&x| x.abs() < 1e-6));
        assert!(w.firms.iter().all(|f| f.inventory >= 0.0));
        assert!(w.flows.imports > 0.0, "import leakage should be recorded");
    }

    #[test]
    fn scarce_stock_imports_goods_and_leaves_rest_unspent() {
        let mut w = world(500, 4, 0.0);
        consumption::decide(&mut w);
        let budget = sum_orders(&w);
        let row_imports_before = w.row.imports_this_month;
        clear(&mut w);
        check_consistency(&w).unwrap();

        // Tradable sectors (`tradable_weight` != 0) with no domestic stock import their whole budget; the others
        // import only the import share and leave the rest unspent.
        let g = Sector::DurableGoods.idx();
        assert!(w.cfg.tradable_weight[g] != 0.0);
        let goods_spent: f64 = w.hh_month.iter().map(|m| m.consumption[g]).sum();
        assert!((goods_spent - budget[g]).abs() <= 1e-6 * budget[g]);

        let m = w.cfg.import_share;
        let mut expected_unspent = 0.0;
        let mut expected_imports = 0.0;
        for s in 0..N_CONSUMER {
            if w.cfg.tradable_weight[s] != 0.0 {
                expected_imports += budget[s];
            } else {
                expected_imports += budget[s] * m[s];
                expected_unspent += budget[s] * (1.0 - m[s]);
            }
        }
        let unspent: f64 = w.fills.household_unspent.iter().sum();
        assert!((unspent - expected_unspent).abs() <= 1e-6 * expected_unspent);
        let imports = w.row.imports_this_month - row_imports_before;
        assert!((imports - expected_imports).abs() <= 1e-6 * expected_imports);
    }

    #[test]
    fn households_leave_dead_firms() {
        let mut w = world(500, 5, 1e7);
        let s = Sector::OtherServices.idx();
        let dead = w.firms.iter().position(|f| f.sector == Sector::OtherServices).unwrap() as FirmId;
        w.firms[dead as usize].alive = false;
        consumption::decide(&mut w);
        clear(&mut w);
        check_consistency(&w).unwrap();
        assert!(w.households.iter().all(|h| h.preferred_firm[s] != dead));
        assert!(w.households.iter().all(|h| w.firms[h.preferred_firm[s] as usize].alive));
        assert_eq!(w.firms[dead as usize].sales, 0.0);
    }

    /// b7-tariff (unit tests 1 and the flag): the retail wedge follows r_t = φτ(1 − (1−λ)^t) at constant τ, the
    /// household share factor falls, and with the flag off the step is inactive and leaves the state untouched.
    #[test]
    fn tariff_step_path_and_flag() {
        let cfg = Config::default();
        let (lam, phi, tau) = (cfg.tariff_retail_speed, cfg.tariff_retail_long, 0.25);
        let mut s = TariffState::default();
        let mut prev_fh = f64::INFINITY;
        for t in 1..=40 {
            let m = tariff_step(&cfg, &mut s, tau);
            assert!(m.active);
            let want = phi * tau * (1.0 - (1.0 - lam).powi(t));
            assert!((m.retail - want).abs() <= 1e-12, "t={t}: {} vs {want}", m.retail);
            assert!(m.f_hh < 1.0 && m.f_hh <= prev_fh, "t={t}: f_hh {}", m.f_hh);
            prev_fh = m.f_hh;
        }
        let off = Config { tariff_pass_through: false, ..Config::default() };
        let mut s0 = TariffState::default();
        let m = tariff_step(&off, &mut s0, tau);
        assert!(!m.active && m.retail == 0.0 && m.f_hh == 1.0 && m.f_firm == 1.0);
        assert_eq!((s0.retail, s0.z_hh, s0.z_firm, s0.ln_f_firm), (0.0, 0.0, 0.0, 0.0));
    }

    /// b7-tariff (unit tests 3, 4 and 5, one clear with ample stock): the duty identity W + G = τV, coverage (only the
    /// covered household sectors pay), RoW receipts equal customs value, households pay their budget without overshoot,
    /// and importers absorb (τ − r)V when τ > r.
    #[test]
    fn tariff_duty_identity_coverage_and_no_overshoot() {
        let mut w = world(500, 3, 1e7);
        w.shocks.tariff = 0.25;
        consumption::decide(&mut w);
        let budget = sum_orders(&w);
        let (rev_before, row_before) = (w.gov.revenue, w.row.imports_this_month);
        clear(&mut w);
        check_consistency(&w).unwrap();
        let r = w.goods.tariff_month.retail;
        assert!(w.goods.tariff_month.active && r > 0.0);
        let wedge: f64 = w.flows.tariff_hh.iter().sum();
        let absorbed = w.flows.tariff_absorbed;
        assert!(wedge > 0.0);
        // The wedge is r × V, so V = W / r is the covered customs value.
        let cust = wedge / r;
        for s in 0..N_CONSUMER {
            if w.cfg.cpi_tradable_weight[s] == 0.0 {
                assert_eq!(w.flows.tariff_hh[s], 0.0, "uncovered sector {s} pays no duty");
            }
        }
        assert!((wedge + absorbed - 0.25 * cust).abs() <= 1e-9 * 0.25 * cust, "W + G vs tau V");
        assert!((absorbed - (0.25 - r) * cust).abs() <= 1e-9 * absorbed.abs().max(1.0), "absorption (tau - r) V");
        assert!((w.gov.revenue - rev_before - (wedge + absorbed)).abs() <= 1e-9 * wedge);
        // RoW receives the customs value of every household import: the covered V = W / r plus the uncovered sectors'
        // import budgets (Energy, Food, services pay no duty but are still imports).
        let row_hh = w.row.imports_this_month - row_before;
        let uncovered: f64 = (0..N_CONSUMER)
            .filter(|&s| w.cfg.cpi_tradable_weight[s] == 0.0)
            .map(|s| budget[s] * w.goods.hh_import_share_now[s])
            .sum();
        assert!((row_hh - cust - uncovered).abs() <= 1e-9 * row_hh, "RoW gets customs value");
        assert!((w.flows.imports - row_hh).abs() <= 1e-9 * row_hh, "imports are customs value");
        let spent: f64 = w.flows.consumption.iter().sum();
        let budget_total: f64 = budget.iter().sum();
        assert!((spent - budget_total).abs() <= 1e-6 * budget_total, "households pay the budget, no overshoot");
    }

    /// b7-tariff (unit test 5, the other sign): a tariff cut below the retail wedge makes the Treasury pay firms. With
    /// τ = 0 the absorbed duty is −r V = −W, so the two channels net to zero, and the ledger stays consistent.
    #[test]
    fn tariff_cut_below_retail_wedge_pays_firms_from_government() {
        let mut w = world(500, 4, 1e7);
        w.goods.tariff.retail = 0.1;
        w.shocks.tariff = 0.0;
        consumption::decide(&mut w);
        clear(&mut w);
        check_consistency(&w).unwrap();
        let wedge: f64 = w.flows.tariff_hh.iter().sum();
        assert!(w.goods.tariff_month.active && w.goods.tariff_month.retail > 0.0);
        assert!(wedge > 0.0 && w.flows.tariff_absorbed < 0.0);
        assert!((w.flows.tariff_absorbed + wedge).abs() <= 1e-9 * wedge, "G = -W at tau = 0");
    }

    /// b7-tariff (unit test 6): the tradables price carries the retail wedge on the import share alone.
    #[test]
    fn tradables_composite_carries_the_retail_wedge() {
        let (m, dom, p) = (0.467, 1.02, 0.97);
        let base = tradables_price(m, dom, p, 0.0);
        let with = tradables_price(m, dom, p, 0.1);
        assert!((with - base - m * p * 0.1).abs() <= 1e-12, "{with} vs {base}");
        assert_eq!(tradables_price(m, dom, p, 0.0), finite_or((1.0 - m) * dom + m * p, 1.0));
    }

    /// b7-tariff (unit test 7): a 36-month run with a 25% tariff for 18 months. Every month is consistent and finite,
    /// and the retail wedge and lagged state decay after the tariff is removed.
    #[test]
    fn tariff_multi_month_run_is_consistent_and_decays() {
        let mut w = tiny_world(Config { n_households: 400, ..Default::default() }, 9);
        let mut retail_at = Vec::new();
        for t in 0..36 {
            w.shocks.tariff = if t < 18 { 0.25 } else { 0.0 };
            crate::sim::step(&mut w);
            check_consistency(&w).unwrap();
            let st = w.goods.tariff;
            assert!(st.retail.is_finite() && st.z_hh.is_finite() && st.z_firm.is_finite());
            assert!(w.prices.cpi.is_finite() && w.gov.revenue.is_finite());
            retail_at.push((st.retail, st.z_hh));
        }
        let (r18, _) = retail_at[17];
        let (r35, z35) = retail_at[35];
        assert!(r18 > 0.0 && r35 < r18, "retail decays after the tariff ends");
        let z_peak = retail_at.iter().map(|x| x.1).fold(f64::MIN, f64::max);
        assert!(z35 > 0.0 && z35 < z_peak, "lagged share state decays from its peak");
    }

    /// b7-tariff (spec 5.6): with domestic prices held fixed (`clear` does not reprice firms) and a forced retail wedge
    /// (λ_R = 0), a wedge of 0.1 raises the tradables composite by m̄ · imp_rel · 0.1, so the headline CPI rises by
    /// 100 · (CPI weight of the tradable sectors) · m̄ · imp_rel · 0.1. Here the duty leaves the domestic sector prices
    /// unchanged (every firm starts at price 1 and `clear` does not reprice), so the match is exact to rounding: the
    /// measured relative error is about 2e-14, and the tolerance is 1e-9. The sales weights could move domestic prices
    /// in other worlds; this one is the check of the formula, with imp_rel ≠ 1 after a 20% dollar move.
    #[test]
    fn forced_retail_wedge_raises_the_cpi_by_the_import_share() {
        let mut base = world(500, 11, 1e7);
        consumption::decide(&mut base);
        clear(&mut base); // the first clear sets the t=0 CPI base, with no duty
        base.row.fx *= 1.2; // a dollar move after t=0, so the import price differs from 1 and the check is not trivial
        base.cfg.tariff_retail_speed = 0.0; // the wedge stays at its forced value
        base.shocks.tariff = 0.1;
        let run = |retail: f64| {
            let mut w = base.clone();
            w.goods.tariff.retail = retail;
            consumption::decide(&mut w);
            clear(&mut w);
            check_consistency(&w).unwrap();
            w
        };
        let (a, b) = (run(0.1), run(0.0));
        assert_eq!(a.goods.tariff_month.retail, 0.1);
        assert_eq!(b.goods.tariff_month.retail, 0.0);
        let imp = a.goods.import_rel.unwrap();
        let cpi_trad_weight: f64 =
            (0..N_SECTORS).filter(|&s| a.cfg.cpi_tradable_weight[s] != 0.0).map(|s| a.cfg.cpi_weights[s]).sum();
        let m_bar = role_mean(&a.cfg.cpi_tradable_weight, |s| a.cfg.import_share[s]);
        let want = 100.0 * cpi_trad_weight * m_bar * imp * 0.1;
        let got = a.prices.cpi - b.prices.cpi;
        assert!(want > 0.0);
        assert!((got - want).abs() <= 1e-9 * want, "CPI rise {got} vs {want}");
    }

    /// b7-tariff (spec 5.9): real consumption excludes the household duty wedge and real investment the equipment duty.
    /// Recording the same month again with both flows zeroed raises the real series by exactly the duty, and nominal
    /// consumption does not move.
    #[test]
    fn metrics_real_volumes_exclude_the_duties() {
        let mut w = tiny_world(Config { n_households: 2000, ..Default::default() }, 33);
        w.shocks.tariff = 0.25;
        crate::sim::step(&mut w);
        let ann = 12.0 * w.cfg.effective_scale() / 1e9;
        let before = w.history.last().unwrap().clone();
        let wedge: f64 = w.flows.tariff_hh.iter().sum();
        let equipment = w.flows.tariff_equipment;
        assert!(wedge > 0.0 && equipment > 0.0);
        w.flows.tariff_hh = [0.0; N_CONSUMER];
        w.flows.tariff_equipment = 0.0;
        crate::metrics::record(&mut w);
        let after = w.history.last().unwrap().clone();
        let dc = after.consumption_real_bn_ar - before.consumption_real_bn_ar;
        let di = after.investment_real_bn_ar - before.investment_real_bn_ar;
        assert!((dc - wedge * ann).abs() <= 1e-9 * wedge * ann, "real C rise {dc} vs {}", wedge * ann);
        assert!((di - equipment * ann).abs() <= 1e-9 * equipment * ann, "real I rise {di} vs {}", equipment * ann);
        assert_eq!(after.consumption_nominal_bn_ar, before.consumption_nominal_bn_ar);
    }

    /// b7-tariff (spec 5.9): with no duty, the mechanism on and off give the same real series, bit for bit.
    #[test]
    fn zero_duty_reproduces_the_legacy_real_series_bit_for_bit() {
        let run = |pass_through: bool| {
            let cfg = Config { n_households: 2000, tariff_pass_through: pass_through, ..Default::default() };
            let mut w = tiny_world(cfg, 33);
            for _ in 0..6 {
                crate::sim::step(&mut w);
            }
            w.history
        };
        let (on, off) = (run(true), run(false));
        assert_eq!(on.len(), 6);
        assert_eq!(off.len(), 6);
        for (a, b) in on.iter().zip(&off) {
            assert_eq!(a.consumption_real_bn_ar.to_bits(), b.consumption_real_bn_ar.to_bits());
            assert_eq!(a.household_purchases_real_bn_ar.to_bits(), b.household_purchases_real_bn_ar.to_bits());
            for j in 0..N_CONSUMER {
                assert_eq!(a.consumption_by_sector_real[j].to_bits(), b.consumption_by_sector_real[j].to_bits());
            }
        }
    }

    /// b7-tariff (M4 fallback): a covered sector whose alive firms have no revenue shares its gap equally; with no
    /// alive firm the gap goes to `tariff_uncollected` and absorbed duty does not move.
    #[test]
    fn tariff_gap_falls_back_to_equal_shares_then_to_uncollected() {
        let mut w = world(400, 12, 1e7);
        let s = (0..N_CONSUMER).find(|&s| w.cfg.cpi_tradable_weight[s] != 0.0).unwrap();
        let mut cust = [0.0; N_CONSUMER];
        cust[s] = 100.0;
        let members: Vec<usize> =
            (0..w.firms.len()).filter(|&f| w.firms[f].alive && w.firms[f].sector.idx() == s).collect();
        assert!(members.len() >= 2);
        for &f in &members {
            w.firms[f].revenue = 0.0;
        }
        let before: Vec<f64> = members.iter().map(|&f| w.firms[f].input_cost).collect();
        settle_tariff_gap(&mut w, &cust, 0.25, 0.0);
        let g = 0.25 * 100.0;
        let share = g / members.len() as f64;
        for (i, &f) in members.iter().enumerate() {
            assert!((w.firms[f].input_cost - before[i] - share).abs() <= 1e-9 * share, "firm {f} share");
        }
        assert!((w.flows.tariff_absorbed - g).abs() <= 1e-9 * g);
        assert_eq!(w.flows.tariff_uncollected, 0.0);
        for &f in &members {
            w.firms[f].alive = false;
        }
        let absorbed = w.flows.tariff_absorbed;
        settle_tariff_gap(&mut w, &cust, 0.25, 0.0);
        assert_eq!(w.flows.tariff_absorbed, absorbed);
        assert!((w.flows.tariff_uncollected - g).abs() <= 1e-9 * g);
    }

    #[test]
    fn tariff_on_imports_goes_to_government() {
        let mut w = world(400, 6, 1e7);
        // b7-tariff: the legacy lump-sum semantics, which this test pins.
        w.cfg.tariff_pass_through = false;
        w.shocks.tariff = 0.25;
        consumption::decide(&mut w);
        let budget = sum_orders(&w);
        let tga_before = w.cb.tga;
        clear(&mut w);
        check_consistency(&w).unwrap();
        let import_base: f64 = (0..N_CONSUMER).map(|s| budget[s] * w.cfg.import_share[s]).sum();
        let tariff = 0.25 * import_base;
        assert!((w.gov.revenue - tariff).abs() <= 1e-6 * tariff);
        assert!((w.cb.tga - tga_before - tariff).abs() <= 1e-6 * tariff);
    }

    #[test]
    fn rest_of_world_balance_matches_trade() {
        let mut w = world(400, 8, 1e7);
        consumption::decide(&mut w);
        let row_before = w.row.deposits;
        clear(&mut w);
        check_consistency(&w).unwrap();
        // RoW receives imports and pays for exports; nothing else touches its deposits in this module.
        let delta = w.row.deposits - row_before;
        let expected = w.row.imports_this_month - w.row.exports_this_month;
        assert!((delta - expected).abs() <= 1e-6 * expected.abs().max(1.0));
    }

    #[test]
    fn price_indices_start_at_base_and_history_grows() {
        let mut w = world(400, 7, 1e7);
        consumption::decide(&mut w);
        let n0 = w.prices.cpi_history.len();
        clear(&mut w);
        assert_eq!(w.prices.cpi_history.len(), n0 + 1);
        assert_eq!(w.prices.core_history.len(), w.prices.cpi_history.len());
        // All prices are 1.0 at t=0 and the rent index is unchanged, so the index is exactly 100.
        assert!((w.prices.cpi - 100.0).abs() < 1e-6, "cpi {}", w.prices.cpi);
        assert!((w.prices.core_cpi - 100.0).abs() < 1e-6);
        assert!(w.prices.sector.iter().all(|p| p.is_finite() && *p > 0.0));
    }

    // fix-trade: the lagged pass-through state is read back from `GoodsState`, so the impact move is speed × long × e
    // (not the full move), and it converges to long × e. With speed = long = 1 it is exactly the spot import price.
    #[test]
    fn import_pass_through_is_partial_lagged_and_full_at_unit_settings() {
        let cfg = Config { n_households: 10, world_goods_inflation: 0.0, ..Default::default() };
        let mut w = tiny_world(cfg, 3);
        w.row.fx = 1.0;
        let mut st = GoodsState::default();
        let (r0, z0) = import_pass_through(&w, &st);
        assert!((r0 - 1.0).abs() < 1e-12);
        st.import_rel = Some(r0);
        st.pt_z = Some(z0);
        st.fx_clear = Some(1.0);

        // A 10% dollar appreciation: the FX-implied log import price move is e = ln(1 / 1.1) (negative).
        w.row.fx = 1.1;
        let e = (1.0f64 / 1.1).ln();
        let (r1, z1) = import_pass_through(&w, &st);
        let (long, speed) = (w.cfg.import_pass_through_long, w.cfg.import_pass_through_speed);
        assert!((r1.ln() - speed * long * e).abs() < 1e-12, "impact {} vs {}", r1.ln(), speed * long * e);
        assert!(r1.ln().abs() < 0.1 * e.abs(), "impact pass-through should be far from full");
        assert!((z1 - speed * long * e).abs() < 1e-12);

        // Held fx: the state converges to the long-run pass-through, ln import price = long × e.
        let mut r = r1;
        for _ in 0..400 {
            let (rr, zz) = import_pass_through(&w, &st);
            st.import_rel = Some(rr);
            st.pt_z = Some(zz);
            r = rr;
        }
        assert!((r.ln() - long * e).abs() < 1e-6, "long run {} vs {}", r.ln(), long * e);

        // Unit settings: full, instant pass-through, exactly the spot import price.
        let mut cfg1 = w.cfg.clone();
        cfg1.import_pass_through_long = 1.0;
        cfg1.import_pass_through_speed = 1.0;
        w.cfg = cfg1;
        let mut st1 = GoodsState::default();
        st1.fx_clear = Some(1.0);
        let (full, _) = import_pass_through(&w, &st1);
        assert!((full - 1.0 / 1.1).abs() < 1e-12, "full {}", full);
    }

    #[test]
    fn stock_limits_are_respected_under_shortage() {
        let mut w = world(400, 9, 50.0);
        consumption::decide(&mut w);
        clear(&mut w);
        check_consistency(&w).unwrap();
        for f in &w.firms {
            assert!(f.inventory >= 0.0);
            assert!(f.sales <= 50.0 + 1e-9);
        }
    }

    #[test]
    fn sales_tax_is_remitted_from_gross_budgets() {
        let mut w = world(400, 10, 1e7);
        w.cfg.sales_tax = 0.1;
        consumption::decide(&mut w);
        let budget = sum_orders(&w);
        let total_budget: f64 = budget.iter().sum();
        let tga_before = w.cb.tga;
        clear(&mut w);
        check_consistency(&w).unwrap();
        // with ample stock and no tariff, households pay exactly their gross budgets
        let spent: f64 = w.hh_month.iter().map(|m| m.consumption.iter().sum::<f64>()).sum();
        assert!((spent - total_budget).abs() <= 1e-6 * total_budget);
        // the tax share of gross spending is tau / (1 + tau)
        let tax = total_budget * 0.1 / 1.1;
        assert!((w.gov.revenue - tax).abs() <= 1e-6 * tax);
        assert!((w.cb.tga - tga_before - tax).abs() <= 1e-6 * tax);
    }
}

#[cfg(test)]
mod trade_lag_tests {
    use super::{elasticity_factor, lagged_log, long_run_or};
    use crate::config::Config;

    /// With the default lag settings an override of the short-run import elasticity moves the long-run one too, so the
    /// import shares respond to it (the p0 `import_price_elasticity` is not silently ignored).
    #[test]
    fn import_override_reaches_the_import_share_factor() {
        let base = crate::testkit::legacy_p1off_config();
        let over = Config { import_price_elasticity: 2.0, ..crate::testkit::legacy_p1off_config() };
        assert_eq!(base.import_elasticity_lr, None);
        let sr_base = long_run_or(base.import_elasticity_lr, base.import_price_elasticity);
        let sr_over = long_run_or(over.import_elasticity_lr, over.import_price_elasticity);
        assert_eq!(sr_base, 0.5);
        assert_eq!(sr_over, 2.0);
        // at speed 1 the lagged state is the current ratio, so the factor is ratio^lr
        let ratio: f64 = 0.8;
        let z = ratio.ln();
        let f_base = elasticity_factor(ratio, z, base.import_price_elasticity, sr_base);
        let f_over = elasticity_factor(ratio, z, over.import_price_elasticity, sr_over);
        assert_eq!(f_base.to_bits(), ratio.powf(0.5).to_bits());
        assert_eq!(f_over.to_bits(), ratio.powf(2.0).to_bits());
        assert_ne!(f_base, f_over);
    }

    /// An explicit long-run value is used as given; a non-finite one falls back to the short-run value.
    #[test]
    fn explicit_long_run_value_is_used() {
        assert_eq!(long_run_or(Some(0.6), 0.2), 0.6);
        assert_eq!(long_run_or(Some(f64::NAN), 0.2), 0.2);
        assert_eq!(long_run_or(None, 0.2), 0.2);
    }

    /// With equal short- and long-run elasticities the factor is exactly ratio^sr (the p0 path), whatever the lag.
    #[test]
    fn equal_elasticities_give_the_plain_power() {
        for &(r, z) in &[(0.7_f64, -0.3_f64), (1.9, 0.4), (1.0, 0.0)] {
            assert_eq!(elasticity_factor(r, z, 1.5, 1.5).to_bits(), r.powf(1.5).to_bits());
        }
    }

    /// A permanent rise in the ratio moves the log factor by sr at impact and by lr once the lag has caught up.
    #[test]
    fn lagged_factor_moves_from_short_run_to_long_run() {
        let (sr, lr, speed) = (0.3, 1.0, 0.08);
        let mut z = None;
        let mut first = 0.0;
        let mut last = 0.0;
        for t in 0..600 {
            let ln_r = if t == 0 { 0.0 } else { 0.1 };
            let zn = lagged_log(z, ln_r, speed);
            if t == 0 {
                // the state starts at the current value, so there is no impact in the first month
                assert!(zn.abs() < 1e-15);
            }
            z = Some(zn);
            let log_factor = elasticity_factor(ln_r.exp(), zn, sr, lr).ln();
            if t == 1 {
                first = log_factor;
            }
            last = log_factor;
        }
        // one month after the shift the lag has moved `speed` of the way: the log factor is sr + (lr - sr) * speed
        assert!((first - 0.1 * (sr + (lr - sr) * speed)).abs() < 1e-9, "impact {first}");
        assert!((last - lr * 0.1).abs() < 1e-3, "long run {last}");
    }

    /// Non-finite inputs keep the previous lagged value.
    #[test]
    fn lagged_log_ignores_non_finite_input() {
        assert_eq!(lagged_log(Some(0.2), f64::NAN, 0.5), 0.2);
        assert_eq!(lagged_log(None, f64::NEG_INFINITY, 0.5), 0.0);
    }
}

#[cfg(test)]
mod appeal_tests {
    use super::{appeal_step_ln, logit_choice_weight, step_appeal, Cdf, APPEAL_KEY};
    use crate::config::Config;
    use crate::rng::{self, stream};
    use crate::testkit::tiny_world;

    /// The reflected step never goes negative, for any state and shock (the abs keeps the log appeal on [0, ∞)).
    #[test]
    fn reflection_keeps_log_appeal_nonnegative() {
        for &sig in &[0.03, 0.10, 0.15] {
            for &zeta in &[0.5, 1.1, 2.0] {
                for i in 0..200 {
                    let x = i as f64 * 0.05;
                    for j in -40..=40 {
                        let eta = j as f64 * 0.1;
                        let y = appeal_step_ln(x, eta, sig, zeta);
                        assert!(y >= 0.0 && y.is_finite(), "x {x} eta {eta} sig {sig} zeta {zeta} -> {y}");
                    }
                }
            }
        }
    }

    /// An entrant (age 0) is reset to the barrier, an incumbent moves, and a dead slot keeps its value.
    #[test]
    fn entrant_reset_and_dead_slot_hold() {
        let cfg = Config { n_households: 60, demand_logit: true, appeal_sigma: 0.10, entrant_appeal_ln: 0.25, ..Default::default() };
        let mut w = tiny_world(cfg, 11);
        let nf = w.firms.len();
        assert!(nf >= 3, "tiny world needs firms, got {nf}");
        for fm in w.firms.iter_mut() {
            fm.alive = true;
            fm.age_months = 12;
        }
        w.firms[0].age_months = 0;
        w.firms[1].alive = false;
        let mut appeal = vec![3.0; nf];
        w.tick = 5;
        step_appeal(&w, &mut appeal);
        assert_eq!(appeal[0].to_bits(), 0.25f64.to_bits(), "entrant not reset to barrier");
        assert_eq!(appeal[1].to_bits(), 3.0f64.to_bits(), "dead slot moved");
        assert!(appeal[2] != 3.0 && appeal[2] >= 0.0, "incumbent did not move: {}", appeal[2]);
    }

    /// Scalar process check. The stationary law of x = ln a is exponential with rate ζ, so the Hill estimate on
    /// a = e^x over the top 500 of 20,000 paths (3,000 months each, started at 0) recovers ζ = 1.1 within ±0.1. The
    /// test uses σ_a = 0.10: the relaxation time is about 2/(σ²ζ²) ≈ 165 months, so 3,000 months is well past it. At
    /// the shipped σ_a = 0.03 the relaxation time is about 1,800 months, so the cross-section is not stationary and the
    /// test would fail (see the b6-firmsize record in docs/CALIBRATION.md).
    #[test]
    fn hill_exponent_matches_zeta_at_stationarity() {
        let sig = 0.10;
        let zeta = 1.1;
        let (paths, months, k) = (20_000u64, 3_000u32, 500usize);
        let mut finals: Vec<f64> = Vec::with_capacity(paths as usize);
        for p in 0..paths {
            let mut r = rng::agent_rng(7, 0, stream::GOODS, APPEAL_KEY + p);
            let mut x = 0.0f64;
            for _ in 0..months {
                let eta = rng::normal(&mut r);
                x = appeal_step_ln(x, eta, sig, zeta);
            }
            finals.push(x);
        }
        finals.sort_by(|a, b| b.partial_cmp(a).unwrap());
        // Hill on the log scale: gamma = mean(x_i) - x_(k+1) over the top k; tail index = 1 / gamma.
        let gamma: f64 = finals[..k].iter().sum::<f64>() / k as f64 - finals[k];
        let hill = 1.0 / gamma;
        assert!((hill - zeta).abs() <= 0.1, "Hill exponent {hill} not within 0.1 of zeta {zeta}");
    }

    /// Choice frequencies are proportional to v = a · p^(−ε) in a small sector (sampled through the same CDF the
    /// household pass uses).
    #[test]
    fn choice_frequencies_are_proportional_to_v() {
        let eps = 4.5;
        let appeal = [0.1f64, 0.7, 1.4, 0.3];
        let price = [1.0f64, 1.1, 0.95, 1.3];
        let v: Vec<f64> = appeal.iter().zip(&price).map(|(&x, &p)| logit_choice_weight(x, p, eps)).collect();
        let total: f64 = v.iter().sum();
        let mut cdf = Cdf::default();
        cdf.build(v.iter().copied());
        let n = 400_000usize;
        let mut counts = [0usize; 4];
        let mut r = rng::agent_rng(3, 0, stream::GOODS, 99);
        for _ in 0..n {
            let u: f64 = rng::Rng::gen::<f64>(&mut r) * cdf.total();
            counts[cdf.pick(u)] += 1;
        }
        for i in 0..4 {
            let freq = counts[i] as f64 / n as f64;
            let want = v[i] / total;
            assert!((freq - want).abs() < 0.005, "firm {i}: freq {freq} vs v-share {want}");
        }
    }
}

#[cfg(test)]
mod pool_tests {
    //! b7-spikes F2: pooled firm-order routing (`Config::firm_order_pool`).
    use super::*;
    use crate::config::Config;
    use crate::consumption;
    use crate::ledger::check_consistency;
    use crate::sim::begin_month;
    use crate::testkit::tiny_world;

    fn small_world(seed: u64) -> World {
        let mut w = tiny_world(
            Config { n_households: 400, sales_tax: 0.0, firm_order_pool: true, ..Default::default() },
            seed,
        );
        begin_month(&mut w);
        w
    }

    /// Hand-built goods state for `route_pooled`: every firm sells at price 1 with stock `stock`.
    fn state(w: &World, stock: f64) -> GoodsState {
        let nf = w.firms.len();
        let mut st = GoodsState::default();
        for (f, fm) in w.firms.iter().enumerate() {
            if fm.alive {
                st.alive[fm.sector.idx()].push(f as FirmId);
            }
        }
        st.avail = vec![stock; nf];
        st.price = vec![1.0; nf];
        st.fill = vec![1.0; nf];
        st.unmet = vec![0.0; nf];
        st.dem = vec![0.0; nf];
        st
    }

    /// Every alive firm orders `amount(b)` units of sector 0 (no imported share).
    fn orders_in_sector0(w: &World, amount: impl Fn(usize) -> f64) -> Vec<[f64; N_SECTORS]> {
        (0..w.firms.len())
            .map(|b| {
                let mut o = [0.0; N_SECTORS];
                if w.firms[b].alive {
                    o[0] = amount(b);
                }
                o
            })
            .collect()
    }

    #[test]
    fn ample_stock_fills_every_buyer_and_sales_follow_weights() {
        let mut w = small_world(7);
        let orders = orders_in_sector0(&w, |_| 10.0);
        let mut st = state(&w, 1e6);
        let before = st.avail.clone();
        let members = st.alive[0].clone();
        let weights: Vec<f64> = members.iter().map(|&f| supplier_weight(before[f as usize], &w.firms[f as usize])).collect();
        let total_w: f64 = weights.iter().sum();
        let x_total: f64 = orders.iter().map(|o| o[0]).sum();
        let net = route_pooled(&mut w, &mut st, &orders, FirmUse::Inputs, 0.0);
        for (b, o) in orders.iter().enumerate() {
            if o[0] > 0.0 {
                assert!(st.unmet[b].abs() < 1e-9, "buyer {b} rationed under ample stock: {}", st.unmet[b]);
            }
        }
        // Sales by supplier are proportional to its routing weight: D_f = X_s w_f / W_s, fill 1.
        for (k, &f) in members.iter().enumerate() {
            let f = f as usize;
            let sold = before[f] - st.avail[f];
            let expect = x_total * weights[k] / total_w;
            assert!((sold - expect).abs() <= 1e-9 * expect.max(1.0), "firm {f}: sold {sold} vs {expect}");
        }
        // Deposits net to zero: buyers' payments equal suppliers' receipts.
        let total: f64 = net.iter().sum();
        assert!(total.abs() <= 1e-9 * x_total.max(1.0), "net deposits do not net to zero: {total}");
    }

    #[test]
    fn scarce_stock_gives_every_buyer_in_a_sector_the_same_fill_fraction() {
        let mut w = small_world(8);
        let orders = orders_in_sector0(&w, |b| 5.0 + (b % 7) as f64);
        let mut st = state(&w, 0.5);
        let _net = route_pooled(&mut w, &mut st, &orders, FirmUse::Inputs, 0.0);
        let mut fracs = Vec::new();
        for (b, o) in orders.iter().enumerate() {
            if o[0] > 0.0 {
                fracs.push((o[0] - st.unmet[b]) / o[0]); // paid share of the order
            }
        }
        let f0 = fracs[0];
        assert!(f0 > 0.0 && f0 < 1.0, "scarce stock should leave a partial fill, got {f0}");
        for (i, f) in fracs.iter().enumerate() {
            assert!((f - f0).abs() <= 1e-9, "buyer {i}: fill {f} vs {f0}");
        }
    }

    #[test]
    fn pooled_routing_keeps_the_month_consistent_with_ample_and_no_stock() {
        for (inv, seed) in [(1e7, 3u64), (0.0, 4u64)] {
            let mut w = small_world(seed);
            for f in w.firms.iter_mut() {
                f.inventory = inv;
            }
            consumption::decide(&mut w);
            clear(&mut w);
            check_consistency(&w).unwrap();
            assert!(w.flows.imports > 0.0 || inv > 0.0);
        }
    }
}
