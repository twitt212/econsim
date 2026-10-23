//! Firms (docs/DESIGN.md §7): planning, pricing, investment, production, books, bankruptcy and entry.
//!
//! Tick order (sim::step): `plan` → labour market → `produce` → … → goods clearing → `close_books`.
//! `sim::begin_month` resets the live monthly fields (`demand`, `sales`, `output`, `interest_paid`,
//! `wage_bill`, …), so `close_books` copies last month's realised values into `FirmsState` for `plan`.

use rayon::prelude::*;

use crate::banking;
use crate::common::{self, human_capital};
use crate::ledger::{self, Account};
use crate::rng::{self, stream, Rng};
use crate::testkit::calibrated_labor_productivity;
use crate::types::*;

/// Speed at which expected demand adapts to last month's demand (§7.1).
/// Maximum output relative to nominal capacity (overtime, extra shifts).
const MAX_UTILIZATION: f64 = 1.1;
/// Months of cash costs a firm keeps before returning surplus cash, and the monthly payout share of the surplus.
const EXCESS_CASH_MONTHS: f64 = 3.0;
const EXCESS_CASH_PAYOUT: f64 = 0.05;
/// Months over which the gap to desired capital is closed (adjustment costs / time-to-build).
const CAPITAL_ADJ_MONTHS: f64 = 48.0;
/// Sales below this share of output signal excess supply (markup cut).
const EXCESS_SUPPLY_RATIO: f64 = 0.92;
/// Per-reprice mean reversion of the markup toward the sector norm.
const MARKUP_ANCHOR: f64 = 0.45;
/// Wage-growth response (annual) per unit of unemployment gap (empirical wage Phillips slope ≈0.5–1).
const WAGE_PHILLIPS_SLOPE: f64 = 0.8;
/// Monthly adjustment of the long-run cost of funds used in target-return pricing (≈4-year half-life).
const LONG_RUN_RATE_ADAPT: f64 = 0.015;
/// Growth entry starts once households per firm exceed the calibrated ratio by this share; at most this share of
/// the firm count enters per month.
const GROWTH_ENTRY_SLACK: f64 = 0.01;
const GROWTH_ENTRY_MAX_SHARE: f64 = 0.002;
/// Monthly adjustment of the reference fill rate against which firms judge labour-market tightness.
const FILL_REF_ADAPT: f64 = 0.02;
/// Okun's-law output loss per point of unemployment (Ball, Leigh & Loungani 2017: ≈2).
const OKUN_COEFFICIENT: f64 = 2.0;
/// Unmet demand above this share of demand raises markups (§7.2).
const UNMET_DEMAND_TRIGGER: f64 = 0.15;
/// Maximum random markup step per repricing event (§7.2).
const MARKUP_STEP: f64 = 0.03;
/// Markup bounds (§7.2).
const MARKUP_MIN: f64 = 0.02;
const MARKUP_MAX: f64 = 0.6;
/// Maximum monthly price change for ordinary firms and for Energy (model choice, §7.2).
const PRICE_MOVE_MAX: f64 = 0.10;
const ENERGY_PRICE_MOVE_MAX: f64 = 0.25;
/// Weight of the oil ratio in the Energy price; the rest is the unit-cost ratio (§7.2). Integrate's price; used when
/// `energy_fuel_split` is off.
const ENERGY_OIL_WEIGHT: f64 = 0.5;
/// Months in the moving average of the oil ratio that utilities' price follows (p1-energy, `energy_fuel_split`).
const ENERGY_OIL_AVG_MONTHS: f64 = 12.0;
/// Monthly wage-offer growth bounds (model choice, §6/§7).
/// Downward nominal wage rigidity: posted wage offers fall at most 0.1% a month (Daly & Hobijn 2014).
/// Monthly adaptation of the usual wage-signal frequencies (≈3-year half-life, like the natural-rate estimate).
const WAGE_SIGNAL_REF_ADAPT: f64 = 0.02;
const WAGE_GROWTH_MIN: f64 = -0.001;
const WAGE_GROWTH_MAX: f64 = 0.015;
/// Wage-growth premium when the firm had unfilled vacancies last month / cut when it planned layoffs.
/// Repricing is forced once the price has been below unit cost for this many months (model choice).
const FORCED_REPRICE_MONTHS: u32 = 2;
/// Bankruptcy after net worth < 0 for this many consecutive months (§7.5).
const DISTRESS_MONTHS: u32 = 6;
/// Recovery on capital in a liquidation, paid by the Fund (§7.5 / model choice).
const LIQUIDATION_RECOVERY: f64 = 0.4;
/// Months a dead slot waits before it is reborn (§7.5).
const ENTRY_DELAY_MONTHS: u32 = 3;
/// Capital-productivity fallback for a sector with no living firms (matches testkit).
const FALLBACK_CAPITAL_PRODUCTIVITY: f64 = 1.0 / 30.0;
/// Profit rate given to an entrant before its first close (neutral prior).
const ENTRANT_PROFIT_RATE: f64 = 0.08;
/// p1-firmsentry: RNG key offset (on the ENTRY stream) for the entry gate, kept apart from the entrant's own draws.
const ENTRY_GATE_KEY: u64 = 1 << 40;
/// p1-firmsentry: RNG key offset (on the FIRMS stream) for the monthly productivity shock of each firm.
const PROD_SHOCK_KEY: u64 = 1 << 42;
/// p1-firmsentry: RNG key offset (on the ENTRY stream) for choosing an owner-founder, and the tries allowed.
const FOUNDER_KEY: u64 = 1 << 43;
const FOUNDER_TRIES: usize = 16;

/// Per-firm bookkeeping that is not part of the shared `Firm` type. Vectors are indexed by firm id and
/// resized lazily to the number of firm slots.
#[derive(Clone, Debug, Default)]
pub struct FirmsState {
    /// Unit cost at first observation; anchors the Energy price (§7.2). 0 = not yet set.
    pub uc0: Vec<f64>,
    /// p1-energy (`energy_fuel_split`): non-crude unit cost (unit cost less the crude term) at the Energy anchor; the
    /// utilities leg prices its change. 0 = not yet set.
    pub nc0: Vec<f64>,
    /// p1-energy: oil ratio at the first month planned (the Energy anchor) and the 12-month average of the oil ratio
    /// (`oil_slow`, starts at the anchor), both for `energy_fuel_split`. `pump_hist` is the monthly pump-price history
    /// (oldest first) for `pump_expectations`, read by `common::pump_yoy`.
    pub oil_ratio0: f64,
    /// b6-firmsize S3: per sector, [fast, slow] EMAs of the capital-weighted profit rate (3-month and slow anchor).
    /// `entry_ema_init[j]` is false until the sector's first observation sets both.
    pub entry_ema: [[f64; 2]; N_SECTORS],
    pub entry_ema_init: [bool; N_SECTORS],
    pub oil_slow: f64,
    pub pump_hist: Vec<f64>,
    /// Wage bill / (labour productivity × price) at first observation; anchors the wage pull (§7.4). 0 = unset.
    pub labor_share0: Vec<f64>,
    /// Number of workers when the firm last planned; used to infer hires for the unfilled-vacancy signal.
    pub workers_at_plan: Vec<u32>,
    /// Last month's realised demand (real units, incl. unmet), captured in `close_books`.
    pub last_demand: Vec<f64>,
    /// Last month's realised sales (real units).
    pub last_sales: Vec<f64>,
    /// Last month's real output.
    pub last_output: Vec<f64>,
    /// Last month's interest paid (nominal).
    pub last_interest: Vec<f64>,
    /// Last month's cash costs: wages incl. employer tax, input costs and interest (nominal).
    pub last_costs: Vec<f64>,
    /// Number of months closed so far (0 before the first `close_books`).
    pub months_run: u32,
    /// Slow moving average of banks' business lending base rate (the long-run cost of funds firms price on), and its
    /// value when the world was built.
    pub rate_lr: f64,
    pub rate_lr0: f64,
    /// Firms' perceived business lending base rate (smoothed with `rate_perception_months`; 0 until first set).
    pub rate_plan: f64,
    /// rz12: the stages of the firms' rate-perception cascade when it has more than one (the last stage is `rate_plan`).
    pub rate_stages: [Option<f64>; 4],
    /// Mean business lending base rate this month.
    pub rate_now: f64,
    /// Per sector: last month's total expected demand (real units) and fast/slow averages of its annualised growth,
    /// for extrapolative investment (`investment_growth_horizon_years`).
    pub sector_demand_prev: [f64; N_SECTORS],
    pub sector_growth_fast: [f64; N_SECTORS],
    pub sector_growth_slow: [f64; N_SECTORS],
    /// Smoothed real oil ratio (`common::real_oil_ratio`, deflated by the price-level index) for extraction labour
    /// demand (0 until first set). Real, so the index does not raise labour per unit of energy.
    pub oil_ratio_lagged: f64,
    /// b9-oil: slow EWMA of the household mean expected inflation (user-cost mode 3); `pi_e_capital_init` is false until
    /// first set.
    pub pi_e_capital_slow: f64,
    pub pi_e_capital_init: bool,
    /// Slow average of the economy-wide fill rate (hires ÷ hires needed).
    pub fill_ref: f64,
    /// Slow averages of the employment-weighted shares of firms that missed hiring needs and that planned layoffs.
    pub unfilled_ref: f64,
    pub layoff_ref: f64,
    // b9-inflation M-D: demand-induced markup deviation d (decimal, as the markup) and the slow norm n of the Okun gap;
    // both stay at their defaults until `markup_dev_pull` is non-zero. `demand_gap_init` is false until the norm has been
    // set from `markup_dev_gap_ref`.
    pub demand_dev: f64,
    pub demand_gap_norm: f64,
    pub demand_gap_init: bool,
    /// p1-firmsentry: labour force (employed + unemployed households) when entry first ran; firm entry targets the
    /// firm count in proportion to the labour force relative to this reference (0 = unset).
    pub lf_ref: f64,
    /// p1-firmsentry: number of living firms when `lf_ref` was set (the growth-entry target scales from it).
    pub firms_ref: f64,
    /// p1-firmsentry: productivity anchor per sector, the geometric mean of firm productivity relative to the sector
    /// TFP index (`exog.tfp`) when the idiosyncratic process first ran (0 = unset). Fixed thereafter, so exits of
    /// low-productivity firms cannot pull the anchor up (no ratchet) and the exogenous trend passes through.
    pub lp_anchor: [f64; N_SECTORS],
}

/// Grow every per-firm vector to the current number of firm slots.
fn ensure_state(w: &mut World) {
    let n = w.firms.len();
    let old = w.firms_state.last_demand.len();
    let s = &mut w.firms_state;
    s.uc0.resize(n, 0.0);
    s.nc0.resize(n, 0.0);
    s.labor_share0.resize(n, 0.0);
    s.workers_at_plan.resize(n, 0);
    s.last_demand.resize(n, 0.0);
    s.last_sales.resize(n, 0.0);
    s.last_output.resize(n, 0.0);
    s.last_interest.resize(n, 0.0);
    s.last_costs.resize(n, 0.0);
    // Firms seen for the first time start with "last month" = their expected demand (steady state), so the first
    // plan doesn't read zero demand and shed workers.
    for f in old..n {
        let fm = &w.firms[f];
        s.last_demand[f] = fm.expected_demand;
        s.last_sales[f] = fm.expected_demand;
        s.last_output[f] = fm.expected_demand;
        s.workers_at_plan[f] = fm.workers.len() as u32;
    }
}

#[inline]
fn finite_or(x: f64, d: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        d
    }
}

/// Sector prices P_s (common::sector_price, 1.0 at t=0), indexed by sector.
fn sector_prices(w: &World) -> [f64; N_SECTORS] {
    Sector::ALL.map(|s| common::sector_price(w, s))
}

/// Mean household inflation expectation (annual). Falls back to the inflation target with no households.
pub fn mean_expected_inflation(w: &World) -> f64 {
    // Fixed-size chunks reduced in chunk order, so the sum does not depend on the thread count.
    let (sum, n) = w
        .households
        .par_chunks(8192)
        .map(|c| {
            c.iter()
                .filter(|h| h.alive && h.expected_inflation.is_finite())
                .fold((0.0, 0usize), |(s, n), h| (s + h.expected_inflation, n + 1))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .fold((0.0, 0usize), |(s, n), (a, b)| (s + a, n + b));
    if n > 0 {
        sum / n as f64
    } else {
        w.cfg.inflation_target
    }
}

/// Σ human capital (efficiency units), Σ wages, Σ positive wages paid and head count of a firm's current workers.
/// Read-only, so it runs inside the parallel passes.
fn worker_totals(cfg: &crate::config::Config, households: &[Household], workers: &[HhId]) -> (f64, f64, f64) {
    let mut eff = 0.0;
    let mut wages = 0.0;
    let mut paid = 0.0;
    for &h in workers {
        let hh = &households[h as usize];
        eff += human_capital(cfg, hh);
        wages += hh.wage;
        if hh.wage.is_finite() && hh.wage > 0.0 {
            paid += hh.wage;
        }
    }
    (eff, wages, paid)
}

/// Firm slots per fixed parallel chunk in the planning, production and closing passes (fixed, so results are
/// thread-count independent).
const FIRM_CHUNK: usize = 1024;

/// Read-only inputs shared by every firm's plan this month.
struct PlanCtx {
    /// Mean household inflation expectation (annual).
    pi_e: f64,
    /// Expectation used only in the user cost of capital (Step 8a): `pi_e` unless `user_cost_household_expectation`.
    pi_e_capital: f64,
    /// Current unemployment rate.
    u: f64,
    /// Realised CPI inflation over the past year.
    pi_yoy: f64,
    /// Sector prices P_s.
    p: [f64; N_SECTORS],
    /// Oil price / base.
    oil_ratio: f64,
    /// p1-energy: motor-fuel price index (`common::fuel_index`, 1 at the anchor).
    fuel_index: f64,
    /// p1-energy: 12-month average oil ratio / oil ratio at the anchor (utilities' oil leg).
    oil_slow_rel: f64,
    /// Real oil price / base (deflated by the price-level index), smoothed over ≈6 months (drilling responds with a lag).
    oil_ratio_lagged: f64,
    /// Aggregate output gap from Okun's law, 2 × (u* − u), bounded at ±10%.
    output_gap: f64,
    /// Goods-input price per unit of the Goods price: (1 − s) + s × (import price / its t=0 value), with s the
    /// imported share of firms' intermediate inputs (exchange-rate pass-through; 1.0 at t=0).
    goods_input_mult: f64,
    /// Change in the long-run cost of funds since the world was built (annual, bounded).
    capital_return_adj: f64,
    /// Openings per needed hire: reference fill rate ÷ expected fill rate.
    fill_rate: f64,
    /// Per sector: recent demand growth above its long-run average (annual), for extrapolative investment.
    growth_excess: [f64; N_SECTORS],
    /// Usual frequencies of the unfilled and layoff wage signals (0 unless `wage_signal_relative`).
    unfilled_ref: f64,
    layoff_ref: f64,
    /// S2: per-firm markup-norm adjustment κ·max(0, ln q_f) − c_j (empty unless `markup_size_kappa` > 0).
    size_adj: Vec<f64>,
    /// S4: per-firm gap z = ln(K/k*) − sector capital-weighted mean (NaN when undefined; empty unless `inv_lumpy`).
    inv_z: Vec<f64>,
}

/// S4 pre-pass: z_f = ln(max(K_f, ε)/ks_f) − Z_j, where Z_j is the capital-weighted mean of the same log gap over the
/// firms of sector j. `group[f]` is None for dead firms; `ks[f]` is last month's smoothed desired capital. Firms with
/// no desired capital (ks ≤ 0) get NaN and keep the legacy rule. Sequential in firm-index order.
fn lumpy_gap_z(group: &[Option<usize>], k: &[f64], ks: &[f64]) -> Vec<f64> {
    let n = group.len();
    let mut ell = vec![f64::NAN; n];
    let mut wsum = [0.0f64; N_SECTORS];
    let mut wl = [0.0f64; N_SECTORS];
    for f in 0..n {
        let Some(j) = group[f] else { continue };
        if !(ks[f].is_finite() && ks[f] > 0.0) {
            continue;
        }
        let w = k[f].max(0.0);
        let l = (k[f].max(1e-9) / ks[f]).ln();
        ell[f] = l;
        wsum[j] += w;
        wl[j] += w * l;
    }
    (0..n)
        .map(|f| match group[f] {
            Some(j) if ell[f].is_finite() => {
                let zbar = if wsum[j] > 0.0 { wl[j] / wsum[j] } else { 0.0 };
                ell[f] - zbar
            }
            _ => f64::NAN,
        })
        .collect()
}

/// S4 investment rule for a firm with gap `z` (see `lumpy_gap_z`). Returns Some(inv_units) when the lumpy rule
/// applies (inaction above `s_hi`, or catch-up below −`s_lo` when `s_lo` > 0), and None for the legacy rule.
fn lumpy_inv_units(z: f64, cfg: &crate::config::Config, k_now: f64, ks: f64, k_lab: f64, dep_m: f64) -> Option<f64> {
    if !z.is_finite() {
        return None;
    }
    if z > cfg.inv_inaction_band {
        return Some(0.0);
    }
    if cfg.inv_catchup_band > 0.0 && z < -cfg.inv_catchup_band {
        let months = cfg.inv_catchup_months.max(1.0);
        let gap = ((ks - k_now) / months).clamp(0.0, cfg.inv_catchup_cap * k_now.max(k_lab).max(0.0));
        return Some(finite_or(dep_m * k_now + gap, 0.0).max(0.0));
    }
    None
}

/// S2 pre-pass (Atkeson & Burstein 2008; capped compensation). `group[f]` is the sector of a firm that gets a size
/// premium (None = dead, or exempt such as Energy, which gets 0). `ed[f]` is last month's expected demand. Returns, per
/// firm, κ·max(0, ln q_f) − c_j where q_f = ed_f / (mean ed over the sector's firms), ω_f = ed_f / Σ ed in the sector,
/// and c_j = min(c_max, κ Σ_f ω_f max(0, ln q_f)). Sequential in firm-index order (no map iteration).
fn size_markup_adj(group: &[Option<usize>], ed: &[f64], kappa: f64, comp_max: f64, q_min: f64) -> Vec<f64> {
    let ln_qmin = q_min.max(1.0).ln();
    let n = group.len();
    let mut sum_ed = [0.0f64; N_SECTORS];
    let mut cnt = [0usize; N_SECTORS];
    for f in 0..n {
        if let Some(j) = group[f] {
            sum_ed[j] += ed[f].max(0.0);
            cnt[j] += 1;
        }
    }
    let mut lnq = vec![0.0f64; n];
    let mut comp_sum = [0.0f64; N_SECTORS];
    for f in 0..n {
        let Some(j) = group[f] else { continue };
        let e = ed[f].max(0.0);
        let mean = if cnt[j] > 0 { sum_ed[j] / cnt[j] as f64 } else { 0.0 };
        if e > 0.0 && mean > 0.0 {
            lnq[f] = ((e / mean).ln() - ln_qmin).max(0.0);
            comp_sum[j] += e / sum_ed[j] * lnq[f];
        }
    }
    let comp: Vec<f64> = comp_sum.iter().map(|&s| (kappa * s).clamp(0.0, comp_max.max(0.0))).collect();
    (0..n)
        .map(|f| match group[f] {
            Some(j) => kappa * lnq[f] - comp[j],
            None => 0.0,
        })
        .collect()
}

/// One firm's planning decisions, computed in parallel and applied sequentially.
struct FirmPlan {
    f: usize,
    n_workers: usize,
    expected_demand: f64,
    target_output: f64,
    vacancies: u32,
    openings: u32,
    planned_layoffs: u32,
    unit_cost: f64,
    markup: f64,
    price: f64,
    months_since_reprice: u32,
    wage_offer: f64,
    uc0_init: Option<f64>,
    nc0_init: Option<f64>,
    labor_share0_init: Option<f64>,
    /// Real capital goods wanted before financing.
    inv_units: f64,
    /// Nominal cash available for investment after one month of costs.
    internal_funds: f64,
    /// Nominal cost of one real unit of capital goods.
    unit_inv_cost: f64,
    /// Smoothed desired capital this month (Step 8b), written back to the firm.
    k_star_smooth: f64,
}

/// Drilling-labour multiplier on Energy's labour requirement, `(1-x) + x·oil_ratio^ε` with x the extraction share.
/// Exactly 1 for other sectors or when the share is off. Used identically in planning and production, so labour
/// per unit of energy moves with oil while energy output is not rationed by drilling employment.
fn extraction_factor(cfg: &crate::config::Config, sector: Sector, oil_ratio_lagged: f64) -> f64 {
    if sector == Sector::Energy && cfg.energy_extraction_labor_share > 0.0 {
        let x = cfg.energy_extraction_labor_share;
        (1.0 - x) + x * oil_ratio_lagged.max(0.1).powf(cfg.energy_extraction_elasticity)
    } else {
        1.0
    }
}

/// Decisions of one alive firm (§7.1–§7.3). Pure function of the world, so it can run in parallel.
/// Vacancies a firm posts this month, and the hiring gap it leaves unposted (`deferred`, heads).
///
/// `gap_units` is the hiring gap in heads, `n` the headcount, `age_months` the firm age and `draw` a uniform draw
/// from the HIRING stream (used only on the throttled path). Legacy (`vacancy_net_share` >= 1, or a firm younger than
/// `vacancy_young_exempt_months`) posts `ceil(gap)`. Otherwise the free band `B = max(vacancy_free_units, replacement)`
/// is posted in full and only the excess is throttled by the share `s` (b4-asym M1). `vacancy_rounding` picks how the
/// throttled posting is rounded: 0 stochastic (Step 9), 1 round up, 2 hybrid (band rounded up, excess stochastic).
/// With `vacancy_free_units` = 0 mode 0 is the Step 9 expression exactly.
pub(crate) fn posted_vacancies(cfg: &crate::config::Config, gap_units: f64, n: usize, age_months: u32, draw: f64) -> (u32, f64) {
    let deferred = |v: u32| if gap_units.is_finite() { (gap_units - v as f64).max(0.0) } else { 0.0 };
    if cfg.vacancy_net_share >= 1.0 || age_months < cfg.vacancy_young_exempt_months {
        let v = if gap_units > 1e-6 { gap_units.ceil() as u32 } else { 0 };
        return (v, deferred(v));
    }
    let repl = if cfg.vacancy_replace { cfg.separation_rate * n as f64 } else { 0.0 };
    let band = cfg.vacancy_free_units.max(0.0).max(repl);
    let share = cfg.vacancy_net_share.max(0.0);
    let gap = if gap_units > 0.0 { gap_units } else { 0.0 };
    let excess = (gap - band).max(0.0);
    let v = match cfg.vacancy_rounding {
        1 => {
            let x = if gap > 0.0 { finite_or(gap.min(band) + excess * share, 0.0).max(0.0) } else { 0.0 };
            if gap > 1e-6 { x.ceil() as u32 } else { 0 }
        }
        2 => {
            if gap > 1e-6 {
                let e = finite_or(excess * share, 0.0).max(0.0);
                let fl = e.floor();
                gap.min(band).ceil() as u32 + fl as u32 + u32::from(draw < e - fl)
            } else {
                0
            }
        }
        _ => {
            // Step 9: stochastic rounding of the throttled posting `x`.
            let x = if gap > 0.0 { finite_or(gap.min(band) + excess * share, 0.0).max(0.0) } else { 0.0 };
            let fl = x.floor();
            fl as u32 + u32::from(draw < x - fl)
        }
    };
    (v, deferred(v))
}

fn plan_firm(w: &World, f: usize, ctx: &PlanCtx) -> Option<FirmPlan> {
    let cfg = &w.cfg;
    let fm = &w.firms[f];
    if !fm.alive {
        return None;
    }
    let st = &w.firms_state;
    let j = fm.sector.idx();
    let pt_mult = 1.0 + cfg.payroll_tax + cfg.employer_benefits;
    let mut rng = rng::agent_rng(w.seed, w.tick, stream::FIRMS, f as u64);

    // §7.1 expected demand; construction adds housing work in progress (backlog).
    let prev_ed = finite_or(fm.expected_demand, 0.0).max(0.0);
    let ed = if st.months_run == 0 {
        prev_ed
    } else {
        // Demand signal: sales plus half of unmet demand — unmet orders are partly re-routed elsewhere, so
        // counting them fully over-states demand and destabilises the accelerator (Assenza et al. 2015).
        let sold = finite_or(st.last_sales[f], 0.0);
        let d = sold + 0.5 * (finite_or(st.last_demand[f], 0.0) - sold).max(0.0);
        (prev_ed + cfg.demand_adapt * (d - prev_ed)).max(0.0)
    };
    // Intentional: construction firms carry a backlog of houses in progress (housing, not a sector role).
    let backlog = if fm.sector == Sector::Construction {
        finite_or(fm.backlog, 0.0).max(0.0)
    } else {
        0.0
    };
    let ed_plan = ed + backlog;

    // §7.1 desired output, capped by capacity.
    // Capacity is soft: overtime and extra shifts allow up to MAX_UTILIZATION of nominal capacity.
    let capacity = finite_or(fm.capital.max(0.0) * fm.capital_productivity.max(0.0), 0.0) * MAX_UTILIZATION;
    let target_inv = cfg.inventory_target[j] * ed_plan;
    let y_star = finite_or(ed_plan + cfg.inventory_adjust * (target_inv - fm.inventory), 0.0)
        .max(0.0)
        .min(capacity);

    // §7.1 labour demand in efficiency units.
    let (l_now, wage_sum, n) = {
        let mut eff = 0.0;
        let mut wages = 0.0;
        for &h in &fm.workers {
            let hh = &w.households[h as usize];
            eff += human_capital(cfg, hh);
            wages += hh.wage;
        }
        (eff, wages, fm.workers.len())
    };
    // Oil and gas extraction (part of Energy): drilling labour scales with the lagged oil price, so the sector hires
    // when oil is dear and sheds workers when it is cheap (US rig counts follow prices with ≈6 months' lag). Drilling
    // labour is not refining capacity, so output per worker is lp / extraction (see `extraction_factor`).
    let lp = fm.labor_productivity.max(1e-9) / extraction_factor(cfg, fm.sector, ctx.oil_ratio_lagged);
    let l_star = y_star / lp;
    let avg_h = if n > 0 && l_now > 1e-9 { l_now / n as f64 } else { 1.0 };
    let gap_units = (l_star - l_now) / avg_h;
    // Only draw on the HIRING stream when the throttled posting path is taken (s < 1 and the firm is not exempt).
    let needs_draw = cfg.vacancy_net_share < 1.0 && fm.age_months >= cfg.vacancy_young_exempt_months;
    let draw = if needs_draw {
        rng::agent_rng(w.seed, w.tick, stream::HIRING, f as u64).gen::<f64>()
    } else {
        0.0
    };
    let (mut vacancies, _hiring_deferred) = posted_vacancies(cfg, gap_units, n, fm.age_months, draw);
    if n == 0 && y_star > 0.0 {
        vacancies = vacancies.max(1);
    }
    // Openings posted: enough that the expected number of fills equals the hires needed (§7.1, JOLTS concept).
    // Openings exceed needed hires when filling is harder than usual (fill rate below its slow average), and fall
    // short of them when it is easier; on average openings equal needed hires (the JOLTS level).
    let openings = if vacancies > 0 { (vacancies as f64 * ctx.fill_rate).ceil() as u32 } else { 0 };
    let planned_layoffs = if n > 0 && l_now > cfg.layoff_trigger * l_star {
        (((l_now - l_star) / avg_h) * cfg.layoff_speed).floor() as u32
    } else {
        0
    };

    // §7.2 unit cost: wages (incl. employer tax) per unit, intermediates, crude, interest.
    let last_output = finite_or(st.last_output[f], 0.0);
    // Normal-cost pricing (Lavoie 2014): labour cost per unit of the workforce's potential output, not of actual
    // output — otherwise unit costs (and prices) rise when firms cut production, a spurious recessionary cost-push.
    let wage_per_unit = if n > 0 && l_now > 1e-9 {
        wage_sum * pt_mult / (lp * l_now)
    } else {
        fm.wage_offer * pt_mult / lp
    };
    // Goods inputs are a blend of domestic goods and imports, the latter priced at p_world / fx.
    let io_cost: f64 = (0..N_SECTORS)
        .map(|s| {
            let p = if cfg.tradable_weight[s] != 0.0 { ctx.p[s] * ctx.goods_input_mult } else { ctx.p[s] };
            cfg.io[s][j] * p
        })
        .sum();
    // Intentional: Energy alone buys crude oil and is priced off the oil price.
    let crude = if fm.sector == Sector::Energy {
        crate::common::energy_crude_unit_cost(cfg, ctx.oil_ratio)
    } else {
        0.0
    };
    let interest_pu = if last_output > 1e-9 {
        finite_or(st.last_interest[f] / last_output, 0.0)
    } else {
        0.0
    };
    // Capital cost at normal utilisation (normal-cost pricing): depreciation per unit of normal output.
    let normal_output = fm.capital * fm.capital_productivity * cfg.target_utilization;
    let capital_pu = if normal_output > 1e-9 {
        (cfg.depreciation_annual + cfg.business_property_tax + ctx.capital_return_adj) / 12.0 * fm.capital
            * common::capital_price(cfg, &ctx.p)
            / normal_output
    } else {
        0.0
    };
    // Financial costs are not marked up (normal-cost pricing; the empirical cost channel is small — Barth & Ramey
    // 2001), which avoids a spurious 'price puzzle' when floating business-loan rates rise.
    let _ = interest_pu;
    let mut uc = wage_per_unit + io_cost + crude + capital_pu;
    if !uc.is_finite() || uc <= 0.0 {
        uc = finite_or(fm.unit_cost, 1.0).max(1e-6);
    }
    // Intentional: Energy prices follow the oil price and its own unit cost (see the Energy branch in repricing).
    let is_energy = fm.sector == Sector::Energy;
    let uc0 = if st.uc0[f] > 0.0 { st.uc0[f] } else { uc };
    let uc0_init = if is_energy && st.uc0[f] <= 0.0 { Some(uc) } else { None };
    // p1-energy (`energy_fuel_split`): non-crude unit cost. Crude (the spot oil term in `uc`) enters the fuel line only;
    // the utilities leg prices the change in non-crude cost against its anchor. The anchor is taken at the anchor oil
    // ratio (`oil_ratio0`), not the current one, so it is the same for every firm whatever the oil price was when it
    // was set: uc0 carries crude at oil_ratio0, and an entrant without its own anchor takes the sector median uc0
    // less that crude.
    let nc0 = if st.nc0[f] > 0.0 {
        st.nc0[f]
    } else {
        (uc0 - crate::common::energy_crude_unit_cost(cfg, st.oil_ratio0)).max(1e-9)
    };
    let nc0_init = if cfg.energy_fuel_split && is_energy && st.nc0[f] <= 0.0 { Some(nc0) } else { None };

    // §7.2 pricing (Calvo repricing; Energy reprices every month).
    let old_price = if fm.price > 0.0 && fm.price.is_finite() { fm.price } else { uc };
    let msr = fm.months_since_reprice + 1;
    let draw: f64 = rng.gen();
    let forced = old_price < uc && fm.months_since_reprice >= FORCED_REPRICE_MONTHS;
    let reprices = draw < cfg.reprice_prob[j] || forced;
    let (price, markup, months_since_reprice) = if !reprices {
        (old_price, fm.markup, msr)
    } else if is_energy {
        let raw = if cfg.energy_fuel_split {
            // Energy (p1-energy): price = φ·F + (1−φ)·U, P0 = 1. F is the motor-fuel index (taxes, a margin indexed to
            // the non-energy price level, and the spot crude pass-through). U is utilities: the non-crude cost ratio
            // times the 12-month average oil ratio raised to `util_oil_elasticity`.
            let util = ((uc - crude).max(1e-9) / nc0) * ctx.oil_slow_rel.powf(cfg.util_oil_elasticity);
            cfg.energy_fuel_share * ctx.fuel_index + (1.0 - cfg.energy_fuel_share) * util
        } else {
            // Energy: price = P0 × (½·oil/oil₀ + ½·uc/uc₀), P0 = 1.
            ENERGY_OIL_WEIGHT * ctx.oil_ratio + (1.0 - ENERGY_OIL_WEIGHT) * (uc / uc0)
        };
        let mv = ENERGY_PRICE_MOVE_MAX;
        (clamp_move(raw, old_price, mv), fm.markup, 0)
    } else {
        // Markup adaptation from inventory and unmet-demand signals (§7.2).
        let mut m = fm.markup;
        let unmet = st.last_demand[f] > 1e-9
            && st.last_demand[f] - st.last_sales[f] > UNMET_DEMAND_TRIGGER * st.last_demand[f];
        // Excess supply: sales well below output (excess capacity) or an inventory glut. The rule is symmetric so
        // markups fall in slumps as well as rising in booms.
        let excess = (st.last_output[f] > 1e-9 && st.last_sales[f] < EXCESS_SUPPLY_RATIO * st.last_output[f])
            || (target_inv > 1e-9 && fm.inventory > 1.5 * target_inv && old_price > ctx.p[j]);
        if (fm.inventory < 0.5 * target_inv && !excess) || unmet {
            m += MARKUP_STEP * rng.gen::<f64>();
        } else if excess {
            m -= MARKUP_STEP * rng.gen::<f64>();
        }
        // Competitive anchor: entry and market-share competition pull markups back toward the sector norm
        // (Dosi, Fagiolo & Roventini 2010).
        // Innovation rents: during a tech boom the Tech sector's norm markup rises as new products gain adoption
        // (ramping over three years), and falls back when the boom ends (Schumpeterian rents; Pástor & Veronesi 2009).
        let rent = if fm.sector == Sector::Tech && w.exog.tech_boom {
            cfg.tech_boom_markup_bonus * (f64::from(w.exog.tech_boom_months) / 36.0).min(1.0)
        } else {
            0.0
        };
        // S2: a size premium above the sector norm (off when κ = 0, which is the legacy norm).
        let size_term = if cfg.markup_size_kappa > 0.0 && !is_energy { ctx.size_adj.get(f).copied().unwrap_or(0.0) } else { 0.0 };
        // b9-inflation M-D: the anchor is re-centred on the demand-induced deviation d (zero, the legacy line, when off).
        let dev = st.demand_dev;
        if dev != 0.0 {
            m += MARKUP_ANCHOR * (cfg.initial_markup[j] + rent + size_term + dev - m);
        } else {
            m += MARKUP_ANCHOR * (cfg.initial_markup[j] + rent + size_term - m);
        }
        // Demand pull (the output-gap term of a New-Keynesian Phillips curve): margins widen when the firm runs
        // above normal utilisation and narrow below it.
        // The gap is the aggregate one (Okun: 2 × the unemployment gap), not the firm's own utilisation, which
        // would bias share-weighted markups upward as sales concentrate in capacity-constrained firms.
        m += cfg.markup_demand_pull * ctx.output_gap;
        m = finite_or(m, MARKUP_MIN).clamp(MARKUP_MIN, MARKUP_MAX);
        // Firms build expected inflation into prices for the months since their last reprice.
        let raw = (1.0 + m) * uc * (1.0 + ctx.pi_e * msr as f64 / 12.0);
        (clamp_move(raw, old_price, PRICE_MOVE_MAX), m, 0)
    };

    // §7.4 / §6 wage offer: Phillips-curve drift, vacancy and layoff signals, labour-share pull.
    let ls0 = if st.labor_share0[f] > 0.0 {
        st.labor_share0[f]
    } else {
        fm.wage_offer * pt_mult / (lp * old_price)
    };
    let labor_share0_init = if st.labor_share0[f] <= 0.0 && ls0.is_finite() && ls0 > 0.0 {
        Some(ls0)
    } else {
        None
    };
    let target_offer = lp * price * ls0 / pt_mult;
    let hires_net = n as i64 - st.workers_at_plan[f] as i64;
    let unfilled = fm.vacancies > 0 && hires_net < fm.vacancies as i64;
    // Wages index 70% to expected and 30% to realised inflation (catch-up raises; Christiano, Eichenbaum & Evans
    // 2005), so real wages track productivity even when inflation departs from expectations.
    let tw = cfg.wage_norm_target_weight.clamp(0.0, 1.0);
    let pi_w = tw * cfg.inflation_target + (1.0 - tw) * (0.7 * ctx.pi_e + 0.3 * ctx.pi_yoy);
    let mut g = (pi_w + cfg.tfp_growth_annual) / 12.0 + WAGE_PHILLIPS_SLOPE * (w.natural_unemployment() - ctx.u) / 12.0;
    g += cfg.wage_unfilled_bonus * (f64::from(u8::from(unfilled)) - ctx.unfilled_ref);
    g -= cfg.wage_layoff_cut * (f64::from(u8::from(planned_layoffs > 0)) - ctx.layoff_ref);
    if target_offer.is_finite() && target_offer > 0.0 && fm.wage_offer > 0.0 {
        g += cfg.labor_share_pull * (target_offer / fm.wage_offer).ln();
    }
    let g = finite_or(g, 0.0).clamp(WAGE_GROWTH_MIN, WAGE_GROWTH_MAX);
    let wage_offer = fm.wage_offer * (1.0 + g);

    // §7.3 investment: desired capital, user cost and the profit-rate gap.
    let util_cp = cfg.target_utilization * fm.capital_productivity;
    let k_now = fm.capital.max(0.0);
    let dep_m = cfg.depreciation_annual / 12.0;
    // Neoclassical accelerator: desired capital follows expected demand and falls with the user cost of capital
    // with elasticity σ (Chirinko, Fazzari & Meyer 2011 estimate σ ≈ 0.4). This is the interest-rate → investment
    // channel; r_K above the user cost (profitability) tilts it further.
    let perceived = banking::business_rate(w, f as FirmId) + (w.firms_state.rate_plan - w.firms_state.rate_now);
    let user_cost = (perceived + cfg.risk_premium_firm_weight * w.exog.risk_premium - ctx.pi_e_capital + cfg.depreciation_annual + cfg.investment_hurdle)
        .max(0.02);
    let user_cost0 = cfg.initial_curve[2] + cfg.business_spread - cfg.inflation_target
        + cfg.depreciation_annual
        + cfg.investment_hurdle;
    let profit_tilt = (1.0 + 0.5 * (finite_or(fm.profit_rate, 0.0) - user_cost)).clamp(0.8, 1.2);
    // Capital is sized to full demand (including orders the firm could not fill), so persistent shortages are
    // eventually met with capacity rather than permanent rationing.
    let full_demand = ed_plan.max(finite_or(st.last_demand[f], 0.0));
    // Tobin's-q link (stage 4): a sector whose equity is high relative to its normal replacement value wants more
    // capital. Bounded, and off unless `investment_q_sens[j]` is set (sector equity from markets, last month).
    // rz20: `investment_q_sens_all` > 0 makes the sectors without their own q-sensitivity (`investment_q_sens[j]` = 0) respond to the Fund's
    // aggregate q (the equity-value-weighted mean of ln q_rel over the sectors, `banking::fund_q_log`) with that elasticity; a sector with
    // its own sensitivity (Tech) keeps its own q. The factor is bounded as the sector factor is.
    let q_sens = cfg.investment_q_sens[j];
    let q_factor = if q_sens != 0.0 && w.fund.sector_q0[j] > 0.0 {
        w.fund.sector_q_rel[j].max(1e-6).powf(q_sens).clamp(0.75, 1.33)
    } else if q_sens == 0.0 && cfg.investment_q_sens_all > 0.0 {
        (cfg.investment_q_sens_all * banking::fund_q_log(w)).exp().clamp(0.75, 1.33)
    } else if q_sens == 0.0 && cfg.rerate_q_sens > 0.0 && w.fund.rerate_q != 0.0 {
        // rz20 (re-rating form): the same bounded factor, read from the smoothed log of a `SharePrices` re-rating only.
        (cfg.rerate_q_sens * w.fund.rerate_q).exp().clamp(0.75, 1.33)
    } else {
        1.0
    };
    let k_star = if util_cp > 1e-9 {
        full_demand / util_cp * (user_cost0 / user_cost).powf(cfg.investment_sensitivity) * profit_tilt * q_factor
            * (1.0 + cfg.investment_growth_horizon_years[j] * ctx.growth_excess[j]).clamp(0.7, 1.6)
    } else {
        0.0
    };
    // Partial adjustment of desired capital (Step 8b): with `kstar_adapt` > 0, desired capital moves that share of the
    // way from last month's smoothed value to today's raw k*. 0 keeps the raw k* (legacy).
    let ks = if cfg.kstar_adapt > 0.0 {
        let prev = if fm.k_star_smooth.is_finite() && fm.k_star_smooth > 0.0 { fm.k_star_smooth } else { k_star };
        prev + cfg.kstar_adapt * (k_star - prev)
    } else {
        k_star
    };
    // Capacity adjusts gradually (time-to-build): the gap closes over CAPITAL_ADJ_MONTHS (48 months) of the
    // distance to desired capital, expanding by at most 2% of K a month; excess capital is run down mainly through
    // depreciation (gross investment falls ~20–30% in recessions, not to zero).
    // b5-firmhet S1: with `inv_expand_labour_ref` the 2% expansion cap is on max(K, K_lab), so a firm with K = 0 and
    // workers can still order capital (legacy: 2% of K, which gives a zero cap at K = 0).
    let k_lab = if cfg.inv_expand_labour_ref {
        labour_capital_ref(lp, fm.workers.len() as f64, fm.capital_productivity, cfg.target_utilization)
    } else {
        0.0
    };
    let gap = ((ks - k_now) / CAPITAL_ADJ_MONTHS)
        .clamp(-cfg.capital_rundown_max_monthly * k_now, 0.02 * expansion_cap_base(k_now, k_lab, cfg.inv_expand_labour_ref));
    let legacy_inv = finite_or(dep_m * k_now + gap, 0.0).max(0.0);
    // S4: lumpy rule on the firm-specific gap (off unless `inv_lumpy`; legacy when the firm's z is inside the bands).
    let inv_units = if cfg.inv_lumpy {
        let z = ctx.inv_z.get(f).copied().unwrap_or(f64::NAN);
        let k_lab_c = labour_capital_ref(lp, fm.workers.len() as f64, fm.capital_productivity, cfg.target_utilization);
        lumpy_inv_units(z, cfg, k_now, ks, k_lab_c, dep_m).unwrap_or(legacy_inv)
    } else {
        legacy_inv
    };
    let unit_inv_cost = role_sum(&cfg.capital_goods_mix, |s| ctx.p[s]);
    let internal_funds = (fm.deposits - st.last_costs[f]).max(0.0);

    Some(FirmPlan {
        f,
        n_workers: n,
        expected_demand: ed,
        target_output: y_star,
        vacancies,
        openings,
        planned_layoffs,
        unit_cost: uc,
        markup,
        price,
        months_since_reprice,
        wage_offer,
        uc0_init,
        nc0_init,
        labor_share0_init,
        inv_units,
        internal_funds,
        unit_inv_cost,
        k_star_smooth: ks,
    })
}

/// b5-firmhet S1: capital that equips `workers` at normal utilisation, lp × workers / (cp × u_n). Zero when the
/// capital productivity or normal utilisation is not positive.
fn labour_capital_ref(lp: f64, workers: f64, cp: f64, u_n: f64) -> f64 {
    if cp > 1e-9 && u_n > 1e-9 {
        lp * workers / (cp * u_n)
    } else {
        0.0
    }
}

/// b5-firmhet S1: entrant capital. `legacy` is 0.5 × the sector's median capital. With a worker floor n_w > 0 the
/// entrant gets at least the capital that equips n_w workers at its own productivity: max(legacy, K_lab(n_w)).
/// A floor of 0 returns `legacy` unchanged.
fn entrant_capital(legacy: f64, floor_workers: f64, lp: f64, cp: f64, u_n: f64) -> f64 {
    if floor_workers > 0.0 {
        legacy.max(labour_capital_ref(lp, floor_workers, cp, u_n))
    } else {
        legacy
    }
}

/// b5-firmhet S1: base of the 2% monthly expansion cap. Legacy: K. With `labour_ref`: max(K, K_lab).
fn expansion_cap_base(k_now: f64, k_lab: f64, labour_ref: bool) -> f64 {
    if labour_ref {
        k_now.max(k_lab)
    } else {
        k_now
    }
}

/// p1-energy (`energy_fuel_split`): the month's inputs to the Energy price, `(fuel index, 12-month average oil ratio /
/// anchor)`. Sets the oil anchor on the first call and updates the moving average of the oil ratio.
fn energy_pricing_inputs(w: &mut World) -> (f64, f64) {
    let r = crate::common::oil_ratio(w);
    let n_index = crate::common::non_energy_price_index(w);
    {
        let st = &mut w.firms_state;
        if st.oil_ratio0 <= 0.0 {
            st.oil_ratio0 = r;
            st.oil_slow = r;
        }
        st.oil_slow += (r - st.oil_slow) / ENERGY_OIL_AVG_MONTHS;
    }
    let st = &w.firms_state;
    let fuel = crate::common::fuel_index(&w.cfg, r, st.oil_ratio0, n_index);
    (fuel, st.oil_slow / st.oil_ratio0)
}

/// p1-energy (`pump_expectations`): appends this month's pump price to the history that `common::pump_yoy` reads.
fn record_pump_price(w: &mut World) {
    let pump = crate::common::gasoline_price(w);
    let st = &mut w.firms_state;
    st.pump_hist.push(pump);
    if st.pump_hist.len() > crate::common::PUMP_HIST_LEN {
        let excess = st.pump_hist.len() - crate::common::PUMP_HIST_LEN;
        st.pump_hist.drain(0..excess);
    }
}

/// Move `old` towards `raw` by at most `max_move` (relative); non-finite `raw` keeps `old`.
fn clamp_move(raw: f64, old: f64, max_move: f64) -> f64 {
    if !raw.is_finite() {
        return old;
    }
    raw.clamp(old * (1.0 - max_move), old * (1.0 + max_move)).max(1e-6)
}

/// Economy-wide expected fill rate: last month's hires ÷ last month's hires needed, bounded to
/// [cfg.min_fill_rate, 1] (measuring against openings would feed back on itself, since openings scale with 1/q).
/// Before any hires have been needed it is cfg.initial_fill_rate.
fn expected_fill_rate(w: &World) -> f64 {
    let cfg = &w.cfg;
    let (hires, needed) = (w.labor.hires_last, w.labor.needed_last);
    let q = if needed > 0 { hires as f64 / needed as f64 } else { cfg.initial_fill_rate };
    finite_or(q, cfg.initial_fill_rate).clamp(cfg.min_fill_rate, 1.0)
}

/// Update and return the slow averages of the employment-weighted shares of firms whose last plan missed its hiring
/// need and that planned layoffs (the same signals `plan_firm` uses).
fn wage_signal_refs(w: &mut World) -> (f64, f64) {
    let st = &w.firms_state;
    let (mut emp, mut unf, mut lay) = (0.0, 0.0, 0.0);
    for (f, fm) in w.firms.iter().enumerate() {
        if !fm.alive {
            continue;
        }
        let n = fm.workers.len();
        let wap = st.workers_at_plan.get(f).copied().unwrap_or(0);
        let e = n as f64;
        emp += e;
        if fm.vacancies > 0 && (n as i64 - wap as i64) < fm.vacancies as i64 {
            unf += e;
        }
        if fm.planned_layoffs > 0 {
            lay += e;
        }
    }
    let (su, sl) = if emp > 0.0 { (unf / emp, lay / emp) } else { (0.0, 0.0) };
    let st = &mut w.firms_state;
    if st.unfilled_ref <= 0.0 && st.layoff_ref <= 0.0 {
        st.unfilled_ref = su;
        st.layoff_ref = sl;
    } else {
        st.unfilled_ref += WAGE_SIGNAL_REF_ADAPT * (su - st.unfilled_ref);
        st.layoff_ref += WAGE_SIGNAL_REF_ADAPT * (sl - st.layoff_ref);
    }
    (st.unfilled_ref, st.layoff_ref)
}

/// 3-month annualised core inflation (None before 4 months of history).
fn core_nowcast(w: &World) -> Option<f64> {
    let h = &w.prices.core_history;
    let n = h.len();
    if n < 4 || !(h[n - 4] > 0.0) {
        return None;
    }
    let g = (h[n - 1] / h[n - 4]).powf(4.0) - 1.0;
    g.is_finite().then_some(g)
}

/// Inflation expectations firms use for wage norms, price resets and the user cost of capital: households' mean
/// expectation, or with `firm_nowcast_weight` > 0 a blend of the target and a 3-month annualised core nowcast (firms
/// watch timely price data while staying anchored on the target, as in survey evidence on firms' expectations).
fn firm_expected_inflation_base(w: &World) -> f64 {
    let hh = mean_expected_inflation(w);
    let wt = w.cfg.firm_nowcast_weight.clamp(0.0, 1.0);
    if wt <= 0.0 {
        return hh;
    }
    match core_nowcast(w) {
        Some(nc) => (1.0 - wt) * w.cfg.inflation_target + wt * nc.clamp(-0.05, 0.15),
        None => hh,
    }
}

/// Firms' price-setting inflation expectations: the base expectation less the policy terms of b7-transmission.
/// π^e_f = base − χ_mp·L_e·tanh(gap_t / L_e) − χ_s·L_s·tanh(S_t / L_s). Each term is skipped when its weight is zero, so
/// with both weights at zero the value is the base exactly (the legacy path).
fn firm_expected_inflation(w: &World) -> f64 {
    let base = firm_expected_inflation_base(w);
    let mut pi = base;
    let chi = w.cfg.firm_mp_expect;
    if chi != 0.0 {
        pi -= policy_expectation_term(chi, w.cfg.firm_mp_expect_cap, w.shocks.policy_gap(w.tick));
    }
    let chi_s = w.cfg.firm_stance_weight;
    if chi_s != 0.0 {
        pi -= policy_expectation_term(chi_s, w.cfg.firm_stance_cap, w.monetary.firm_stance);
    }
    pi
}

/// b7-transmission: χ·L·tanh(x / L), the bounded odd response of firms' expectations to a policy signal x (a decimal
/// policy gap, or the stance). |result| ≤ |χ|·L; zero when L is not positive or x is not finite.
pub fn policy_expectation_term(chi: f64, cap: f64, x: f64) -> f64 {
    if !(cap > 0.0) || !x.is_finite() {
        return 0.0;
    }
    chi * cap * (x / cap).tanh()
}

/// b9-oil: the expectation that enters the investment user cost (`PlanCtx::pi_e_capital`). `pi_e_firm` is the firm
/// expectation already computed in `plan`. Mode 0 is the legacy statement.
fn capital_expectation(w: &mut World, pi_e_firm: f64) -> f64 {
    match w.cfg.user_cost_pi_e_mode {
        1 => pi_e_firm,
        2 => w.monetary.pi_lr.unwrap_or(w.cfg.inflation_target) + w.cfg.user_cost_pi_e_offset,
        3 => {
            let hh = mean_expected_inflation(w);
            let gain = 1.0 / (1.0 + w.cfg.user_cost_pi_e_months.max(0.0));
            let st = &mut w.firms_state;
            if !st.pi_e_capital_init || !st.pi_e_capital_slow.is_finite() {
                st.pi_e_capital_slow = hh;
                st.pi_e_capital_init = true;
            } else {
                st.pi_e_capital_slow += gain * (hh - st.pi_e_capital_slow);
            }
            st.pi_e_capital_slow
        }
        _ => {
            if w.cfg.user_cost_household_expectation {
                let hh = mean_expected_inflation(w);
                // rz2: leave (1 - weight) of the energy-salience part of the household expectation out of the user cost. The state is the
                // one `consumption::decide` keeps (last month's value here, which is what the firms see in every arm alike).
                let wgt = w.cfg.firm_pump_user_cost_weight;
                if wgt < 1.0 {
                    hh - (1.0 - wgt) * w.consumption.pump_comp
                } else {
                    hh
                }
            } else {
                pi_e_firm
            }
        }
    }
}

/// Update each sector's demand-growth averages and return recent growth above the long-run average (annual,
/// bounded). Firms extrapolate it when sizing capacity (Gennaioli, Ma & Shleifer 2016: investment follows
/// extrapolative growth expectations), which produces overbuilding in booms and investment busts after them.
fn sector_growth_excess(w: &mut World) -> [f64; N_SECTORS] {
    let mut demand = [0.0f64; N_SECTORS];
    for fm in w.firms.iter().filter(|f| f.alive) {
        demand[fm.sector.idx()] += finite_or(fm.expected_demand, 0.0).max(0.0);
    }
    let st = &mut w.firms_state;
    let mut out = [0.0; N_SECTORS];
    for s in 0..N_SECTORS {
        let prev = st.sector_demand_prev[s];
        if prev > 0.0 && demand[s] > 0.0 {
            let g = ((demand[s] / prev).powi(12) - 1.0).clamp(-0.5, 0.5);
            st.sector_growth_fast[s] += (g - st.sector_growth_fast[s]) / 6.0;
            st.sector_growth_slow[s] += (g - st.sector_growth_slow[s]) / 60.0;
        }
        st.sector_demand_prev[s] = demand[s];
        out[s] = (st.sector_growth_fast[s] - st.sector_growth_slow[s]).clamp(-0.2, 0.2);
    }
    out
}

/// b9-inflation M-D: one month of the demand-induced markup deviation, d' = (1 − ν)·d + κ·(g − n), bounded by ±cap. A NaN
/// or infinite result is replaced by 0.
fn demand_dev_step(prev: f64, gap: f64, norm: f64, kappa: f64, nu: f64, cap: f64) -> f64 {
    let d = (1.0 - nu.clamp(0.0, 1.0)) * prev + kappa * (gap - norm);
    if d.is_finite() { d.clamp(-cap.max(0.0), cap.max(0.0)) } else { 0.0 }
}

/// b9-inflation M-D: update `FirmsState::demand_dev` from this month's Okun gap g = 2(u* − u), clamped to ±0.1, and the
/// slow norm n of that gap (EWMA with gain 1/(1 + M), starting from `markup_dev_gap_ref`). Does nothing when
/// `markup_dev_pull` is 0, so the state stays at its zero defaults.
fn update_demand_dev(w: &mut World) {
    let k = w.cfg.markup_dev_pull;
    if k == 0.0 {
        return;
    }
    let gap = (OKUN_COEFFICIENT * (w.natural_unemployment() - w.unemployment_rate())).clamp(-0.1, 0.1);
    let (nu, cap, gain) = (w.cfg.markup_dev_decay, w.cfg.markup_dev_max, 1.0 / (1.0 + w.cfg.markup_dev_norm_months.max(0.0)));
    let gap_ref = w.cfg.markup_dev_gap_ref;
    let st = &mut w.firms_state;
    if !st.demand_gap_init {
        st.demand_gap_init = true;
        st.demand_gap_norm = gap_ref;
    }
    let norm = st.demand_gap_norm;
    st.demand_dev = demand_dev_step(st.demand_dev, gap, norm, k, nu, cap);
    st.demand_gap_norm = if gap.is_finite() { norm + gain * (gap - norm) } else { norm };
}

/// b7-tariff (M7): the imported-input cost multiplier for a firm with imported share `s_in`, at the border price
/// `imp_rel` and the statutory duty `tau` (already clamped to be non-negative). With the mechanism on (flag set, and a
/// duty this month or a non-zero lagged factor `ln_f_firm`), the share is `s_in · exp(ln_f_firm)` and the landed price
/// carries the duty `(1 + tau)`; otherwise the legacy expression. Pure, so the unit test can call it directly.
fn goods_input_mult(cfg: &crate::config::Config, tau: f64, ln_f_firm: f64, imp_rel: f64, s_in: f64) -> f64 {
    if cfg.tariff_pass_through && (tau > 0.0 || ln_f_firm != 0.0) {
        let s_t = s_in * ln_f_firm.exp();
        finite_or((1.0 - s_t) + s_t * imp_rel * (1.0 + tau), 1.0)
    } else {
        finite_or((1.0 - s_in) + s_in * imp_rel, 1.0)
    }
}

/// Expected demand, target output, labour demand (vacancies/planned_layoffs), prices, wage offers,
/// investment decisions and their financing (docs/DESIGN.md §7.1–§7.3).
pub fn plan(w: &mut World) {
    ensure_state(w);
    // Target-return pricing: the normal return firms build into prices follows a slow average of the cost of funds,
    // so persistent changes in interest rates are competed into prices (and real wages) rather than into profits,
    // while short-lived policy moves barely reach prices (the cost channel is small: Barth & Ramey 2001).
    {
        let alive: Vec<f64> = w.banks.iter().filter(|b| b.alive).map(|b| b.business_rate_base).collect();
        if !alive.is_empty() {
            let mean = alive.iter().sum::<f64>() / alive.len() as f64;
            let st = &mut w.firms_state;
            if st.rate_lr0 <= 0.0 && mean.is_finite() && mean > 0.0 {
                st.rate_lr0 = mean;
                st.rate_lr = mean;
            } else if mean.is_finite() {
                st.rate_lr += LONG_RUN_RATE_ADAPT * (mean - st.rate_lr);
            }
            if mean.is_finite() {
                // rz1 F1: the firms may carry their own lag (`rate_perception_firm_months` >= 0); negative = the common lag.
                let lag = if w.cfg.rate_perception_firm_months >= 0.0 { w.cfg.rate_perception_firm_months } else { w.cfg.rate_perception_months.max(0.0) };
                // rz12: with `rate_perception_firm_stages` (or, when negative, `rate_perception_stages`) above 1 the perception is a cascade of
                // exponential smoothers with the same mean delay `lag` (gamma-shaped). One stage: the legacy line.
                let stages = if w.cfg.rate_perception_firm_stages >= 1 { w.cfg.rate_perception_firm_stages as usize } else { w.cfg.rate_perception_stages as usize };
                // rz12: `rate_perception_credit_bypass` takes that share of the CreditSpread shock (and its tail) out of the smoothed series and
                // adds it back as it is, so firms see their credit spread at once and only the rest of the base rate is perceived with a lag.
                let bypass = w.cfg.rate_perception_credit_bypass;
                st.rate_plan = if bypass > 0.0 {
                    let credit_now = bypass * w.shocks.credit_spread_add_tail(w.tick, w.cfg.credit_tail_half_life_months);
                    crate::common::lag_cascade(&mut st.rate_stages, stages, lag, mean - credit_now) + credit_now
                } else if stages > 1 {
                    crate::common::lag_cascade(&mut st.rate_stages, stages, lag, mean)
                } else if st.rate_plan == 0.0 || lag == 0.0 {
                    mean
                } else {
                    st.rate_plan + (mean - st.rate_plan) / (1.0 + lag)
                };
                st.rate_now = mean;
            }
        }
    }
    // Integration (fix-trade): firms price imported inputs off the lagged, partially passed-through import price from the
    // last goods clear, not the instant spot price.
    let imp_rel = w.goods.import_rel.unwrap_or_else(|| crate::goods::import_pass_through(w, &w.goods).0);
    let s_in = finite_or(w.cfg.firm_input_import_share, 0.0).clamp(0.0, 1.0);
    let (unfilled_ref, layoff_ref) = if w.cfg.wage_signal_relative { wage_signal_refs(w) } else { (0.0, 0.0) };
    let growth_excess = sector_growth_excess(w);
    update_demand_dev(w);
    let energy_pricing = if w.cfg.energy_fuel_split { energy_pricing_inputs(w) } else { (1.0, 1.0) };
    if w.cfg.pump_expectations {
        record_pump_price(w);
    }
    let pi_e = firm_expected_inflation(w);
    let pi_e_capital = capital_expectation(w, pi_e);
    let ctx = PlanCtx {
        growth_excess,
        unfilled_ref,
        layoff_ref,
        inv_z: if w.cfg.inv_lumpy {
            let group: Vec<Option<usize>> = w.firms.iter().map(|fm| if fm.alive { Some(fm.sector.idx()) } else { None }).collect();
            let k: Vec<f64> = w.firms.iter().map(|fm| finite_or(fm.capital, 0.0)).collect();
            let ks: Vec<f64> = w.firms.iter().map(|fm| fm.k_star_smooth).collect();
            lumpy_gap_z(&group, &k, &ks)
        } else {
            Vec::new()
        },
        size_adj: if w.cfg.markup_size_kappa > 0.0 {
            let group: Vec<Option<usize>> = w
                .firms
                .iter()
                .map(|fm| if fm.alive && fm.sector != Sector::Energy { Some(fm.sector.idx()) } else { None })
                .collect();
            let ed: Vec<f64> = w.firms.iter().map(|fm| finite_or(fm.expected_demand, 0.0)).collect();
            size_markup_adj(&group, &ed, w.cfg.markup_size_kappa, w.cfg.markup_size_comp_max, w.cfg.markup_size_q_min)
        } else {
            Vec::new()
        },
        pi_yoy: if w.cfg.firm_nowcast_weight > 0.0 {
            core_nowcast(w).unwrap_or_else(|| w.inflation_yoy())
        } else {
            w.inflation_yoy()
        }
        .clamp(-0.05, 0.2),
        pi_e,
        pi_e_capital,
        u: w.unemployment_rate(),
        p: sector_prices(w),
        oil_ratio: crate::common::oil_ratio(w),
        fuel_index: energy_pricing.0,
        oil_slow_rel: energy_pricing.1,
        oil_ratio_lagged: {
            let r = crate::common::real_oil_ratio(w);
            let st = &mut w.firms_state;
            st.oil_ratio_lagged = if st.oil_ratio_lagged > 0.0 { st.oil_ratio_lagged + (r - st.oil_ratio_lagged) / 6.0 } else { r };
            st.oil_ratio_lagged
        },
        output_gap: (OKUN_COEFFICIENT * (w.natural_unemployment() - w.unemployment_rate())).clamp(-0.1, 0.1),
        // b7-tariff (M7): imported inputs pay the duty at the border, on last month's import-share factor.
        goods_input_mult: {
            // H8 (b8-world): the firm channel's duty.
            let tau_t = crate::world::tau_for(w, crate::world::Channel::Firm);
            goods_input_mult(&w.cfg, tau_t, w.goods.tariff.ln_f_firm, imp_rel, s_in)
        },
        capital_return_adj: if w.firms_state.rate_lr0 > 0.0 {
            w.cfg.target_return_pass * (w.firms_state.rate_lr - w.firms_state.rate_lr0).clamp(-0.04, 0.04)
        } else {
            0.0
        },
        fill_rate: {
            let q = expected_fill_rate(w);
            let st = &mut w.firms_state;
            st.fill_ref = if st.fill_ref > 0.0 { st.fill_ref + FILL_REF_ADAPT * (q - st.fill_ref) } else { q };
            (st.fill_ref / q).clamp(0.5, 3.0)
        },
    };
    let plans: Vec<Option<FirmPlan>> = {
        let wr: &World = w;
        (0..wr.firms.len()).into_par_iter().map(|f| plan_firm(wr, f, &ctx)).collect()
    };

    // Apply each firm's decisions to its own record, in parallel over fixed chunks (no cross-firm effects).
    {
        let st = &mut w.firms_state;
        let firms = &mut w.firms;
        firms
            .par_chunks_mut(FIRM_CHUNK)
            .zip(st.uc0.par_chunks_mut(FIRM_CHUNK))
            .zip(st.nc0.par_chunks_mut(FIRM_CHUNK))
            .zip(st.labor_share0.par_chunks_mut(FIRM_CHUNK))
            .zip(st.workers_at_plan.par_chunks_mut(FIRM_CHUNK))
            .zip(plans.par_chunks(FIRM_CHUNK))
            .for_each(|(((((fms, uc0), nc0), lsh), wap), pls)| {
                for (k, (fm, pl)) in fms.iter_mut().zip(pls).enumerate() {
                    let Some(pl) = pl else { continue };
                    if let Some(v) = pl.uc0_init {
                        uc0[k] = v;
                    }
                    if let Some(v) = pl.nc0_init {
                        nc0[k] = v;
                    }
                    if let Some(v) = pl.labor_share0_init {
                        lsh[k] = v;
                    }
                    wap[k] = pl.n_workers as u32;
                    fm.expected_demand = pl.expected_demand;
                    fm.target_output = pl.target_output;
                    fm.vacancies = pl.vacancies;
                    fm.openings = pl.openings;
                    fm.planned_layoffs = pl.planned_layoffs;
                    fm.unit_cost = pl.unit_cost;
                    fm.markup = pl.markup;
                    fm.price = pl.price;
                    fm.months_since_reprice = pl.months_since_reprice;
                    fm.wage_offer = pl.wage_offer;
                    fm.investment_order = pl.inv_units;
                    fm.k_star_smooth = pl.k_star_smooth;
                }
            });
    }

    // §7.3 financing: internal funds first, then a business loan; if refused, scale investment down.
    // Sequential in firm order, since each approval depends on the bank's capital after earlier loans.
    for pl in plans.iter().flatten() {
        if pl.inv_units <= 0.0 {
            continue;
        }
        let cost = pl.inv_units * pl.unit_inv_cost;
        if cost > pl.internal_funds {
            let need = cost - pl.internal_funds;
            if banking::request_business_loan(w, pl.f as FirmId, need).is_none() {
                w.firms[pl.f].investment_order = if pl.unit_inv_cost > 1e-9 {
                    pl.internal_funds / pl.unit_inv_cost
                } else {
                    0.0
                };
            }
        }
    }
}

/// Per-firm quantities carried from the first production pass to the loan and payment passes.
#[derive(Clone, Copy, Default)]
struct ProdPre {
    /// Cash needs less deposits; a business loan is requested for a positive gap.
    gap: f64,
    /// Positive wages paid to the firm's workers this month (before benefits and payroll tax).
    paid: f64,
    /// Imported crude paid to the rest of the world this month.
    crude: f64,
    /// Re-fit Step 4c: production taxes on domestic crude, paid to the Treasury this month (Energy; 0 when off).
    upstream_tax: f64,
    /// Re-fit Step 4c: private mineral royalties on domestic crude, paid to the Fund this month (Energy; 0 when off).
    upstream_royalty: f64,
}

/// Bank-side, government, rest-of-world and flow totals reduced per chunk in `produce`.
#[derive(Default)]
struct ProdParts {
    bank: Vec<f64>,
    tga: f64,
    row: f64,
    wages: f64,
    imports: f64,
    /// Re-fit Step 4c: royalties paid to the Fund, and the upstream taxes and royalties in total.
    fund: f64,
    upstream_tax: f64,
    upstream_royalty: f64,
}

/// Produce output, pay wages and employer payroll tax, place intermediate & investment orders, depreciate capital
/// (docs/DESIGN.md §7.4).
///
/// Order within the month: output and cash needs for every firm; then business-loan requests in firm order (a
/// request depends on the firm's own pre-payment state and on its bank's capital, which deposit payments leave
/// unchanged); then wage credits to workers and debits to firms, depreciation and imports in parallel fixed chunks.
pub fn produce(w: &mut World) {
    ensure_state(w);
    let nf = w.firms.len();
    let nb = w.banks.len();
    if w.orders.firm_inputs.len() < nf {
        w.orders.firm_inputs.resize(nf, [0.0; N_SECTORS]);
    }
    if w.orders.firm_investment.len() < nf {
        w.orders.firm_investment.resize(nf, [0.0; N_SECTORS]);
    }
    let p = sector_prices(w);
    let oil_r = crate::common::oil_ratio(w);
    let io = w.cfg.io;
    let pt = w.cfg.payroll_tax;
    let benefits = w.cfg.employer_benefits;
    let crude_intensity = w.cfg.crude_intensity;
    let oil_import_share = w.cfg.oil_import_share;
    // Re-fit Step 4c: the upstream split of domestic crude (None on the legacy path, where it is unpaid).
    let upstream = w.cfg.upstream_split();
    let (up_tax_rate, up_roy_rate) = upstream.map_or((0.0, 0.0), |(t, r, _, _)| (t, r));
    let dep_m = w.cfg.depreciation_annual / 12.0;
    // Same lagged oil ratio that `plan` used this month (plan runs first and updates it).
    let oil_lagged = w.firms_state.oil_ratio_lagged;
    let mut pre = vec![ProdPre::default(); nf];

    // Pass 1 (parallel): output, inventory, cash needs and the loan gap, wage totals, and the order books.
    {
        let cfg = &w.cfg;
        let households = &w.households;
        let firms = &mut w.firms;
        let inputs = &mut w.orders.firm_inputs;
        let invest = &mut w.orders.firm_investment;
        firms
            .par_chunks_mut(FIRM_CHUNK)
            .zip(inputs.par_chunks_mut(FIRM_CHUNK))
            .zip(invest.par_chunks_mut(FIRM_CHUNK))
            .zip(pre.par_chunks_mut(FIRM_CHUNK))
            .for_each(|(((fms, inp), inv), out)| {
                for (k, fm) in fms.iter_mut().enumerate() {
                    if !fm.alive {
                        continue;
                    }
                    let s = fm.sector;
                    let j = s.idx();
                    let (eff, wage_sum, paid) = worker_totals(cfg, households, &fm.workers);
                    let lp = fm.labor_productivity / extraction_factor(cfg, s, oil_lagged);
                    let capacity =
                        finite_or(fm.capital.max(0.0) * fm.capital_productivity.max(0.0), 0.0) * MAX_UTILIZATION;
                    let output = finite_or(fm.target_output.min(lp * eff).min(capacity), 0.0).max(0.0);

                    fm.output = output;
                    if cfg.nonstorable_sectors[j] {
                        // Non-storable output (Services): unsold capacity from last month perishes.
                        fm.inventory = output;
                    } else {
                        fm.inventory += output;
                    }

                    // Working capital: the loan request for the gap between deposits and this month's cash needs.
                    let inputs_nom: f64 = (0..N_SECTORS).map(|i| io[i][j] * output * p[i]).sum();
                    // Intentional: crude imports are Energy's input (oil-linked, see `crude_intensity`).
                    let crude_import = if s == Sector::Energy {
                        crude_intensity * output * oil_r * oil_import_share
                    } else {
                        0.0
                    };
                    // Re-fit Step 4c: domestic crude (1 − s of the Energy input at the oil price) pays the upstream taxes and
                    // royalties; the margin and the upstream capital are not paid to anyone.
                    let (upstream_tax, upstream_royalty) = if upstream.is_some() && s == Sector::Energy {
                        let dom = crude_intensity * output * oil_r * (1.0 - oil_import_share);
                        (up_tax_rate * dom, up_roy_rate * dom)
                    } else {
                        (0.0, 0.0)
                    };
                    let need = wage_sum * (1.0 + pt + benefits) + inputs_nom + crude_import + upstream_tax + upstream_royalty;
                    let gap = need - fm.deposits;
                    out[k] = ProdPre { gap, paid, crude: crude_import, upstream_tax, upstream_royalty };

                    // Orders: intermediate inputs by supplying sector; capital goods from Goods (80%) and
                    // Construction (20%).
                    for i in 0..N_SECTORS {
                        inp[k][i] = io[i][j] * output * p[i];
                    }
                    let inv_order = finite_or(fm.investment_order, 0.0).max(0.0);
                    for s in 0..N_SECTORS {
                        let mix = cfg.capital_goods_mix[s];
                        if mix != 0.0 {
                            inv[k][s] = mix * inv_order * p[s];
                        }
                    }
                }
            });
    }

    // Pass 2 (sequential, firm order): business-loan requests for positive gaps.
    for (f, pr) in pre.iter().enumerate() {
        if pr.gap > 0.0 {
            let _ = banking::request_business_loan(w, f as FirmId, pr.gap);
        }
    }

    // Pass 3 (parallel over households): wages with employer benefits credited to each worker in place.
    // A worker's employer is the firm listing it (`employer` and `workers` agree), and only living firms pay.
    {
        let firms = &w.firms;
        let parts: Vec<Vec<f64>> = w
            .households
            .par_chunks_mut(FIRM_CHUNK)
            .zip(w.hh_month.par_chunks_mut(FIRM_CHUNK))
            .map(|(hhs, mons)| {
                let mut bank = vec![0.0; nb];
                for (hh, mon) in hhs.iter_mut().zip(mons.iter_mut()) {
                    let Some(f) = hh.employer else { continue };
                    if !firms[f as usize].alive {
                        continue;
                    }
                    let wage = hh.wage;
                    if !(wage.is_finite() && wage > 0.0) {
                        continue;
                    }
                    // Cash wage plus employer-paid benefits (earmarked for Services by consumption::decide).
                    let x = wage * (1.0 + benefits);
                    if x > 0.0 && x.is_finite() {
                        hh.deposits += x;
                        bank[hh.bank as usize] += x;
                    }
                    mon.income.wage += wage;
                }
                bank
            })
            .collect();
        let mut net = vec![0.0; nb];
        for b in &parts {
            for (a, x) in net.iter_mut().zip(b) {
                *a += x;
            }
        }
        crate::fiscal::apply_bank_net(w, &net);
    }

    // Pass 4 (parallel over firms): wage and payroll-tax debits, imported crude, depreciation.
    let dep_goods = common::capital_price(&w.cfg, &p);
    let ptax_m = w.cfg.business_property_tax / 12.0;
    let parts: Vec<ProdParts> = w
        .firms
        .par_chunks_mut(FIRM_CHUNK)
        .zip(pre.par_chunks(FIRM_CHUNK))
        .map(|(fms, pres)| {
            let mut acc = ProdParts { bank: vec![0.0; nb], ..Default::default() };
            for (fm, pr) in fms.iter_mut().zip(pres) {
                if !fm.alive {
                    continue;
                }
                let paid = pr.paid;
                // Wages with benefits, then employer payroll tax (→ Government), each as `ledger::pay`.
                let wages_gross = paid * (1.0 + benefits);
                if wages_gross > 0.0 && wages_gross.is_finite() {
                    fm.deposits -= wages_gross;
                    acc.bank[fm.bank as usize] -= wages_gross;
                }
                let tax = pt * paid;
                if tax > 0.0 && tax.is_finite() {
                    fm.deposits -= tax;
                    acc.bank[fm.bank as usize] -= tax;
                    acc.tga += tax;
                }
                fm.wage_bill = paid * (1.0 + benefits) + tax;
                acc.wages += paid;

                // Energy: imported crude paid to the rest of the world; domestic crude is upstream income.
                if pr.crude > 0.0 {
                    let c = pr.crude;
                    if c.is_finite() {
                        fm.deposits -= c;
                        acc.bank[fm.bank as usize] -= c;
                        acc.row += c;
                    }
                    fm.input_cost += c;
                    acc.imports += c;
                }
                // Re-fit Step 4c: domestic crude's production taxes (to the TGA, hence government revenue) and private
                // royalties (to the Fund), each treated like imported crude: a debit to the firm and its bank, and a cost.
                if pr.upstream_tax > 0.0 && pr.upstream_tax.is_finite() {
                    let t = pr.upstream_tax;
                    fm.deposits -= t;
                    acc.bank[fm.bank as usize] -= t;
                    acc.tga += t;
                    fm.input_cost += t;
                    acc.upstream_tax += t;
                }
                if pr.upstream_royalty > 0.0 && pr.upstream_royalty.is_finite() {
                    let r = pr.upstream_royalty;
                    fm.deposits -= r;
                    acc.bank[fm.bank as usize] -= r;
                    acc.fund += r;
                    fm.input_cost += r;
                    acc.upstream_royalty += r;
                }

                // Depreciation (§7.4): capital ×= (1 − δ/12); cost valued at the Goods price.
                let cap = fm.capital.max(0.0);
                let dep_units = cap * dep_m;
                fm.capital = cap - dep_units;
                fm.depreciation_cost = dep_units * dep_goods;

                // Business property tax on the capital stock (a tax on production, paid monthly).
                let ptax = cap * dep_goods * ptax_m;
                if ptax > 0.0 && ptax.is_finite() {
                    fm.deposits -= ptax;
                    acc.bank[fm.bank as usize] -= ptax;
                    acc.tga += ptax;
                    fm.input_cost += ptax;
                }
            }
            acc
        })
        .collect();

    let mut net = vec![0.0; nb];
    let mut tga = 0.0;
    let mut row = 0.0;
    let mut wages = 0.0;
    let mut imports = 0.0;
    let (mut fund, mut up_tax, mut up_roy) = (0.0, 0.0, 0.0);
    for acc in &parts {
        for (a, x) in net.iter_mut().zip(&acc.bank) {
            *a += x;
        }
        tga += acc.tga;
        row += acc.row;
        wages += acc.wages;
        imports += acc.imports;
        fund += acc.fund;
        up_tax += acc.upstream_tax;
        up_roy += acc.upstream_royalty;
    }
    // Rest-of-world credit for imported crude: its bank receives the deposit and reserve inflow.
    if row > 0.0 {
        let br = w.row.bank as usize;
        net[br] += row;
        w.row.deposits += row;
    }
    // Re-fit Step 4c: the royalty credit mirrors the rest-of-world crude credit. The Fund's bank receives the deposit;
    // the Fund's dividends received are untouched, so sector equity valuation is unaffected.
    if fund > 0.0 {
        let fb = w.fund.bank as usize;
        net[fb] += fund;
        w.fund.deposits += fund;
    }
    crate::fiscal::apply_bank_net(w, &net);
    w.cb.tga += tga;
    // Employer payroll tax and business property tax are government revenue.
    w.gov.revenue += tga;
    w.flows.wages += wages;
    w.flows.imports += imports;
    // b8-world H9: imported crude is Energy's import (customs value); it is in `row.imports_this_month` too.
    w.flows.imports_by_sector[0] += imports;
    w.flows.upstream_tax += up_tax;
    w.flows.upstream_royalty += up_roy;
    w.row.imports_this_month += imports;
}

/// Per-chunk totals from the parallel part of `close_books`.
#[derive(Default)]
struct CloseParts {
    bank: Vec<f64>,
    fund_in: f64,
    div_regular: f64,
    div_special: f64,
    /// Payouts of owner-operated firms, (owner, amount), in firm order.
    owner_pay: Vec<(HhId, f64)>,
    profits: f64,
    /// Firm slots that go bankrupt this month, in ascending order.
    bankrupt: Vec<usize>,
    /// Regular dividends paid to the Fund by sector (stage 4 sector equity).
    div_sector: [f64; N_SECTORS],
}

/// Profits, dividends to the Fund, capital received from investment, bankruptcy and entry (docs/DESIGN.md §7.5).
///
/// Profit, dividend and last-month snapshots, and the bankruptcy test, are computed per firm in parallel over
/// fixed chunks: each firm pays its dividend from its own deposits, and the Fund is never constrained. Firms that
/// fail the test are settled afterwards in firm order. They touch only their own loans, workers and balances.
pub fn close_books(w: &mut World) {
    ensure_state(w);
    let nf = w.firms.len();
    let nb = w.banks.len();
    let p = sector_prices(w);
    let p_cap = common::capital_price(&w.cfg, &p);
    let payout = w.cfg.payout_ratio;
    let default_months = w.cfg.business_default_months;
    let fund_bank = w.fund.bank as usize;
    // p1-firmsentry: fixed operating cost, a share of the mean revenue of living firms, paid to the Fund (fees for
    // financial services). It falls on every firm, so low-margin firms bleed out at any age (exit hazard).
    let merton_r = finite_or(w.cfg.merton_cost_of_capital, 0.0);
    let merton_q_min = finite_or(w.cfg.merton_q_min, 0.0).max(0.0);
    let merton_q_max = finite_or(w.cfg.merton_q_max, 1.0).max(merton_q_min);
    // Re-fit Step 5b: the default hazard (0 = the deterministic DISTRESS_MONTHS counter) and the draw's keys.
    let distress_hazard = finite_or(w.cfg.distress_default_hazard, 0.0).clamp(0.0, 1.0);
    let (seed, tick) = (w.seed, w.tick);
    let fixed_cost = {
        let (sum, n) = w.firms.iter().filter(|f| f.alive).fold((0.0, 0usize), |(s, n), f| (s + f.revenue.max(0.0), n + 1));
        if n > 0 { finite_or(w.cfg.firm_fixed_cost_share, 0.0).max(0.0) * sum / n as f64 } else { 0.0 }
    };
    // p1-firmsentry: each firm's labour productivity follows a mean-reverting log AR(1) around its sector's mean, so
    // established firms that fall behind keep losing customers and eventually exit (the exit hazard does not vanish
    // with age). Sector means are taken over living firms at the start of the month.
    let prod_sigma = finite_or(w.cfg.firm_prod_sigma, 0.0).max(0.0);
    let prod_revert = finite_or(w.cfg.firm_prod_revert, 0.0).clamp(0.0, 1.0);
    // The process runs on productivity relative to the sector TFP index, so the exogenous trend and shocks (applied to
    // both in exog.rs) are not mean-reverted away. The anchor is the geometric mean at first use and then fixed.
    let tfp = w.exog.tfp;
    let lp_base: [f64; N_SECTORS] = if prod_sigma > 0.0 {
        let mut sum = [0.0; N_SECTORS];
        let mut n = [0usize; N_SECTORS];
        for f in w.firms.iter().filter(|f| f.alive && f.labor_productivity > 0.0) {
            let s = f.sector.idx();
            sum[s] += (f.labor_productivity / tfp[s].max(1e-12)).ln();
            n[s] += 1;
        }
        for s in 0..N_SECTORS {
            if w.firms_state.lp_anchor[s] <= 0.0 && n[s] > 0 {
                w.firms_state.lp_anchor[s] = (sum[s] / n[s] as f64).exp();
            }
        }
        w.firms_state.lp_anchor
    } else {
        [0.0; N_SECTORS]
    };
    // p1-firmsentry: each firm's monthly productivity shock is re-centred within its sector (the output-weighted mean of
    // the shocks is removed), so the shocks move firms relative to one another (dispersion and selection) without a
    // sector-wide monthly productivity move. A sector-wide shock would act like a TFP shock on every firm in the sector
    // and, since an agent stands for many real firms, would not average out. Output weights: the sector price index
    // and unit costs are output-weighted, so the aggregate sees no net productivity shock.
    let shock: Vec<f64> = if prod_sigma > 0.0 {
        let mut eps = vec![0.0; nf];
        let mut sum = [0.0; N_SECTORS];
        let mut wsum = [0.0; N_SECTORS];
        for (f, fm) in w.firms.iter().enumerate() {
            let s_idx = fm.sector.idx();
            if fm.alive && lp_base[s_idx] > 0.0 && fm.labor_productivity > 0.0 {
                let mut r = rng::agent_rng(w.seed, w.tick, stream::FIRMS, PROD_SHOCK_KEY + f as u64);
                eps[f] = prod_sigma * rng::normal(&mut r);
                let wt = fm.output.max(0.0);
                sum[s_idx] += wt * eps[f];
                wsum[s_idx] += wt;
            }
        }
        for (f, fm) in w.firms.iter().enumerate() {
            let s_idx = fm.sector.idx();
            if fm.alive && lp_base[s_idx] > 0.0 && fm.labor_productivity > 0.0 && wsum[s_idx] > 0.0 {
                eps[f] -= sum[s_idx] / wsum[s_idx];
            }
        }
        eps
    } else {
        Vec::new()
    };
    let parts: Vec<CloseParts> = {
        let loans = &w.loans;
        let households = &w.households;
        let caps = &w.fills.firm_capital_received;
        let st = &mut w.firms_state;
        let firms = &mut w.firms;
        firms
            .par_chunks_mut(FIRM_CHUNK)
            .zip(st.last_demand.par_chunks_mut(FIRM_CHUNK))
            .zip(st.last_sales.par_chunks_mut(FIRM_CHUNK))
            .zip(st.last_output.par_chunks_mut(FIRM_CHUNK))
            .zip(st.last_interest.par_chunks_mut(FIRM_CHUNK))
            .zip(st.last_costs.par_chunks_mut(FIRM_CHUNK))
            .enumerate()
            .map(|(c, (((((fms, ld), ls), lo), li), lc))| {
                let mut acc = CloseParts { bank: vec![0.0; nb], ..Default::default() };
                for (k, fm) in fms.iter_mut().enumerate() {
                    if !fm.alive {
                        fm.months_dead += 1;
                        continue;
                    }
                    let f = c * FIRM_CHUNK + k;
                    let s_idx = fm.sector.idx();
                    let base = lp_base[s_idx];
                    if prod_sigma > 0.0 && base > 0.0 && fm.labor_productivity > 0.0 {
                        let eps = shock[f];
                        let rel = (fm.labor_productivity / tfp[s_idx].max(1e-12)).ln();
                        let lnlp = (1.0 - prod_revert) * rel + prod_revert * base.ln() + eps;
                        fm.labor_productivity = tfp[s_idx] * lnlp.exp();
                    }
                    if let Some(&cap_in) = caps.get(f) {
                        fm.capital += finite_or(cap_in, 0.0).max(0.0);
                    }
                    // Profit and the snapshot inputs (sim::begin_month has reset the monthly flows).
                    let cash_costs = fm.wage_bill + fm.input_cost + fm.interest_paid;
                    let pretax = fm.revenue - fm.wage_bill - fm.input_cost - fm.interest_paid - fm.depreciation_cost - fixed_cost;
                    let profit = finite_or(pretax - fm.taxes, 0.0);
                    // The fixed cost is paid as far as deposits allow (the rest stays unpaid, and the profit above
                    // still bears the full cost).
                    let fixed_paid = fixed_cost.min(fm.deposits.max(0.0));
                    if fixed_paid > 0.0 {
                        fm.deposits -= fixed_paid;
                        acc.bank[fm.bank as usize] -= fixed_paid;
                        acc.fund_in += fixed_paid;
                    }
                    let cap_value = fm.capital.max(0.0) * p_cap;
                    let ret = if cap_value > 1e-6 { (12.0 * profit / cap_value).clamp(-1.0, 1.0) } else { 0.0 };
                    let profit_rate = finite_or(0.9 * fm.profit_rate + 0.1 * ret, fm.profit_rate);
                    let deposits = fm.deposits;
                    let inventory_value = fm.inventory.max(0.0) * fm.price;
                    let months_distressed_prev = fm.months_distressed;
                    fm.profit = profit;
                    fm.profit_rate = profit_rate;
                    acc.profits += profit;

                    // Dividends to the Fund when profitable and liquid (more than two months of cash costs).
                    // Plus buybacks/special dividends: cash above EXCESS_CASH_MONTHS of costs is returned gradually
                    // (US corporates hold ≈1–2 months of revenue in cash), so retained cash does not leak out of the
                    // income circuit. Paid as `ledger::pay_up_to(Firm → Fund)`.
                    let regular = if profit > 0.0 && deposits > 2.0 * cash_costs { payout * profit } else { 0.0 };
                    let excess = (deposits - regular - EXCESS_CASH_MONTHS * cash_costs.max(0.0)).max(0.0);
                    let total = regular + EXCESS_CASH_PAYOUT * excess;
                    // An owner who has died is no longer the owner (the household slot may be recycled).
                    if let Some(o) = fm.owner {
                        if !households.get(o as usize).is_some_and(|h| h.alive) {
                            fm.owner = None;
                        }
                    }
                    if total > 0.0 {
                        let avail = fm.deposits.max(0.0);
                        let x = total.min(avail);
                        if x > 0.0 && x.is_finite() {
                            fm.deposits -= x;
                            acc.bank[fm.bank as usize] -= x;
                        }
                        let paid = x.max(0.0);
                        fm.dividends = paid;
                        // Owner-operated: the same payout goes to the owner household instead of the Fund.
                        match fm.owner {
                            Some(o) if paid > 0.0 => acc.owner_pay.push((o, paid)),
                            _ => {
                                if x > 0.0 && x.is_finite() {
                                    acc.fund_in += x;
                                }
                                let reg_paid = paid.min(regular);
                                acc.div_regular += reg_paid;
                                acc.div_special += paid - reg_paid;
                                acc.div_sector[fm.sector.idx()] += reg_paid;
                            }
                        }
                    }

                    // Snapshot last month's realised values for the next plan.
                    ld[k] = fm.demand;
                    ls[k] = fm.sales;
                    lo[k] = fm.output;
                    li[k] = fm.interest_paid;
                    lc[k] = cash_costs;

                    // §7.5 bankruptcy: deposits below −1 month of costs, or net worth < 0 for DISTRESS_MONTHS months.
                    let principal: f64 = fm.loans.iter().filter_map(|&l| loans.get(l)).map(|l| l.principal).sum();
                    let deposits_now = fm.deposits;
                    // p1-firmsentry: Merton-style test. Capital is valued at its earning power, q × replacement cost,
                    // with q the smoothed return on capital over the cost of capital (clamped), so asset values fall
                    // in slumps and indebted firms default in downturns.
                    let q = if merton_r > 0.0 { (profit_rate / merton_r).clamp(merton_q_min, merton_q_max) } else { 1.0 };
                    let net_worth = deposits_now + inventory_value + cap_value * q - principal;
                    let months_distressed = if net_worth < 0.0 { months_distressed_prev + 1 } else { 0 };
                    fm.months_distressed = months_distressed;
                    // Arrears: a business loan unpaid for `business_default_months` (a refused rollover that the firm
                    // cannot repay, or interest it cannot pay) is an event of default (§11.3).
                    let in_default = fm
                        .loans
                        .iter()
                        .filter_map(|&l| loans.get(l))
                        .any(|l| l.kind == LoanKind::Business && l.months_delinquent >= default_months);
                    let insolvent = deposits_now < -cash_costs || in_default;
                    // Re-fit Step 5b: with a hazard, a firm with negative net worth defaults at the hazard each month
                    // instead of after DISTRESS_MONTHS consecutive months.
                    let distress_default = if distress_hazard > 0.0 {
                        net_worth < 0.0 && firm_default_draw(seed, tick, f, distress_hazard)
                    } else {
                        months_distressed >= DISTRESS_MONTHS
                    };
                    if insolvent || distress_default {
                        acc.bankrupt.push(f);
                    } else {
                        fm.age_months += 1;
                    }
                }
                acc
            })
            .collect()
    };

    let mut net = vec![0.0; nb];
    let mut fund_in = 0.0;
    let mut bankrupt_list: Vec<usize> = Vec::new();
    for acc in &parts {
        for s in 0..N_SECTORS {
            w.fund.sector_dividends_received[s] += acc.div_sector[s];
        }
        for (a, x) in net.iter_mut().zip(&acc.bank) {
            *a += x;
        }
        fund_in += acc.fund_in;
        w.fund.dividends_received += acc.div_regular;
        w.fund.special_dividends += acc.div_special;
        w.flows.profits += acc.profits;
        for &(o, x) in &acc.owner_pay {
            ledger::credit_owner_payout(w, o, x);
            w.hh_month[o as usize].income.business += x;
        }
        bankrupt_list.extend_from_slice(&acc.bankrupt);
    }
    // Fund credit for dividends received (`ledger::pay` into the Fund).
    if fund_in > 0.0 {
        w.fund.deposits += fund_in;
        net[fund_bank] += fund_in;
    }
    crate::fiscal::apply_bank_net(w, &net);
    for f in bankrupt_list {
        bankrupt(w, f, &p);
    }

    // b6-firmsize S3: update the sector profit-rate EMAs once a month, before any entry draw this month.
    if w.cfg.entry_gate_cyclical {
        update_entry_ema(w);
    }
    // Entry: slots dead for ENTRY_DELAY_MONTHS are reborn in their sector.
    // p1-firmsentry: a dead slot is reborn only if the entry gate admits an entrant (see `entry_accepts`).
    if w.firms.iter().any(|fm| !fm.alive && fm.months_dead >= ENTRY_DELAY_MONTHS) {
        let stats = sector_stats(w);
        let cap_share = w.cfg.entry_reborn_max_share;
        let mut admitted = vec![0usize; N_SECTORS];
        for f in 0..nf {
            if w.firms[f].alive || w.firms[f].months_dead < ENTRY_DELAY_MONTHS {
                continue;
            }
            let j = w.firms[f].sector.idx();
            if cap_share > 0.0 {
                let cap = ((stats[j].n as f64 * cap_share).ceil() as usize).max(1);
                if admitted[j] >= cap {
                    continue;
                }
            }
            if entry_accepts(w, f as u64, &stats[j]) {
                admitted[j] += 1;
                reborn(w, f, &stats, &p);
            }
        }
    }
    growth_entry(w, &p);
    w.firms_state.months_run += 1;
}

/// Business formation grows with the economy: when alive households per alive firm exceed the calibrated ratio,
/// new firms enter (start-up equity from the Fund, as for re-entry), in sectors drawn by the calibrated employment
/// shares, at most GROWTH_ENTRY_MAX_SHARE of the firm count a month.
///
/// p1-firmsentry (`entry_labour_force`): the firm count instead targets `FIRMS_ref × LF / LF_ref`, where LF is the
/// number of employed and unemployed households, and LF_ref and FIRMS_ref (the living firm count) are both recorded
/// the first time entry runs. Growth in the labour force therefore adds firms in proportion, and the count does not
/// compound when the labour force is flat (households count retirees, so the old ratio gave a steady entry flow
/// regardless of demand). Each new slot must pass the entry
/// gate (`entry_gate_slope`), so entry is procyclical.
fn growth_entry(w: &mut World, p: &[f64; N_SECTORS]) {
    let alive_f = w.firms.iter().filter(|f| f.alive).count();
    // Alive households as counted by this month's demography pass (cheap; no extra scan).
    let alive_h = if w.demography.n_alive > 0 { w.demography.n_alive } else { w.households.iter().filter(|h| h.alive).count() };
    if alive_f == 0 {
        return;
    }
    let wanted = if w.cfg.entry_labour_force {
        let lf = w
            .households
            .iter()
            .filter(|h| h.alive && matches!(h.status, LaborStatus::Employed | LaborStatus::Unemployed))
            .count() as f64;
        if w.firms_state.lf_ref <= 0.0 {
            w.firms_state.lf_ref = lf.max(1.0);
            w.firms_state.firms_ref = alive_f as f64;
        }
        w.firms_state.firms_ref * lf / w.firms_state.lf_ref
    } else {
        alive_h as f64 / w.cfg.households_per_firm
    };
    if wanted <= alive_f as f64 * (1.0 + GROWTH_ENTRY_SLACK) {
        return;
    }
    let max_new = ((alive_f as f64 * GROWTH_ENTRY_MAX_SHARE).ceil() as usize).max(1);
    let k = ((wanted - alive_f as f64).floor() as usize).min(max_new);
    if k == 0 {
        return;
    }
    let shares = w.cfg.employment_share;
    let total: f64 = shares.iter().sum();
    let mut r = rng::module_rng(w.seed, w.tick, stream::ENTRY);
    for i in 0..k {
        let u: f64 = r.gen::<f64>() * total;
        let mut acc = 0.0;
        // Rounding fallback: the last sector (Construction) if the draw lands beyond the cumulative shares.
        let mut s = N_SECTORS - 1;
        for (j, &x) in shares.iter().enumerate() {
            acc += x;
            if u < acc {
                s = j;
                break;
            }
        }
        if w.cfg.entry_gate_slope > 0.0 {
            let stats = sector_stats(w);
            if !entry_accepts(w, (1u64 << 39) + i as u64, &stats[s]) {
                continue;
            }
        }
        let Some(tmpl) = w.firms.iter().find(|f| f.sector.idx() == s) else { continue };
        let mut fm = tmpl.clone();
        let id = w.firms.len();
        fm.id = id as FirmId;
        fm.alive = false;
        fm.months_dead = ENTRY_DELAY_MONTHS;
        fm.workers.clear();
        fm.loans.clear();
        fm.owner = None;
        fm.deposits = 0.0;
        fm.inventory = 0.0;
        fm.capital = 0.0;
        fm.vacancies = 0;
        fm.openings = 0;
        w.firms.push(fm);
        ensure_state(w);
        let stats = sector_stats(w);
        reborn(w, id, &stats, p);
    }
}

/// b6-firmsize S3: monthly update of each sector's EMAs of the sector profit rate. The signal is the capital-weighted
/// rate when `entry_gate_capital_weighted` is on and the legacy unweighted mean otherwise, the same switch that the
/// flat gate uses, so the cyclical gate can be run on either signal. The fast EMA has a 3-month smoothing constant;
/// the slow one moves at `entry_gate_anchor_rate` per month and is set at first use. Sectors with no living firm keep
/// their values. Draws no random numbers.
fn update_entry_ema(w: &mut World) {
    let stats = sector_stats(w);
    let slow_rate = finite_or(w.cfg.entry_gate_anchor_rate, 0.0).clamp(0.0, 1.0);
    let capital_weighted = w.cfg.entry_gate_capital_weighted;
    for (j, st) in stats.iter().enumerate() {
        if st.n == 0 {
            continue;
        }
        let raw = if capital_weighted { st.profit_rate_kw } else { st.profit_rate };
        let x = finite_or(raw, 0.0);
        let fs = &mut w.firms_state;
        fs.entry_ema[j] = ema_step(fs.entry_ema[j], fs.entry_ema_init[j], x, slow_rate);
        fs.entry_ema_init[j] = true;
    }
}

/// b6-firmsize S3: one EMA update of [fast, slow] given the sector observation x. The first observation sets both
/// to x; afterwards fast moves 1/3 of the way to x (3-month smoothing) and slow moves `slow_rate` of the way.
#[inline]
fn ema_step(prev: [f64; 2], init: bool, x: f64, slow_rate: f64) -> [f64; 2] {
    if !init {
        return [x, x];
    }
    let [fast, slow] = prev;
    [fast + (x - fast) / 3.0, slow + slow_rate * (x - slow)]
}

/// b6-firmsize S3: cyclical entry logit z = logit(p0) + β (fast − slow) + γ (fast − hurdle) + β_u · 100 · (u* − u).
/// The β term is the gap signal, the γ term the level signal (off at 0), and the last term the aggregate fallback
/// (off at 0). Returns the admission probability logistic(z).
#[inline]
fn cyclical_entry_prob(p0: f64, beta: f64, fast: f64, slow: f64, gamma: f64, hurdle: f64, u_beta: f64, u_gap: f64) -> f64 {
    let p0 = p0.clamp(1e-6, 1.0 - 1e-6);
    let z = (p0 / (1.0 - p0)).ln() + beta * (fast - slow) + gamma * (fast - hurdle) + u_beta * 100.0 * u_gap;
    1.0 / (1.0 + (-z).exp())
}

/// p1-firmsentry: entry gate. A potential entrant in the sector of `stats` enters with probability
/// logistic(entry_gate_slope × (sector profit rate − entry_gate_hurdle)), so entry runs when incumbents earn above the
/// hurdle and stops in slumps (Hopenhayn 1992: entry when expected profit covers the sunk cost). `key` makes the draw
/// reproducible per candidate. Slope 0 admits every candidate. An empty sector always admits one.
fn entry_accepts(w: &World, key: u64, st: &SectorStats) -> bool {
    let slope = w.cfg.entry_gate_slope;
    if slope <= 0.0 || st.n == 0 {
        return true;
    }
    // b5-firmhet S1 fallback: a sector under its contestability share of calibrated firms admits entrants (off at 0).
    if w.cfg.entry_contestability_share > 0.0 && (st.n as f64) < w.cfg.entry_contestability_share * st.calib_n {
        return true;
    }
    // b6-firmsize S3: cyclical gate, p = logistic(logit(p0) + β (π̃_j − π̄_j)) on the sector's EMAs.
    if w.cfg.entry_gate_cyclical {
        let [fast, slow] = w.firms_state.entry_ema.get(st.idx).copied().unwrap_or([0.0, 0.0]);
        let u_gap = w.natural_unemployment() - w.unemployment_rate();
        let p = cyclical_entry_prob(
            finite_or(w.cfg.entry_gate_base_prob, 0.5),
            finite_or(w.cfg.entry_gate_cyc_slope, 0.0),
            finite_or(fast, 0.0),
            finite_or(slow, 0.0),
            finite_or(w.cfg.entry_gate_level_slope, 0.0),
            w.cfg.entry_gate_hurdle,
            finite_or(w.cfg.entry_gate_u_beta, 0.0),
            finite_or(u_gap, 0.0),
        );
        let mut r = rng::agent_rng(w.seed, w.tick, stream::ENTRY, ENTRY_GATE_KEY + key);
        return r.gen::<f64>() < p;
    }
    // b5-firmhet S1: capital-weighted profit signal when `entry_gate_capital_weighted` (legacy: unweighted mean).
    let signal = if w.cfg.entry_gate_capital_weighted { st.profit_rate_kw } else { st.profit_rate };
    let p = 1.0 / (1.0 + (-slope * (signal - w.cfg.entry_gate_hurdle)).exp());
    let mut r = rng::agent_rng(w.seed, w.tick, stream::ENTRY, ENTRY_GATE_KEY + key);
    r.gen::<f64>() < p
}

/// Lay off workers, liquidate assets to the Fund, repay what deposits allow, write off the rest and
/// leave the slot dead with zero deposits.
/// Re-fit Step 5b: whether a distressed firm defaults this month, a Bernoulli(`hazard`) draw on the FIRM_DEFAULT stream
/// keyed by firm, so it is common across the paired runs and independent of every other stream.
pub fn firm_default_draw(seed: u64, tick: u32, f: usize, hazard: f64) -> bool {
    let mut r = rng::agent_rng(seed, tick, stream::FIRM_DEFAULT, f as u64);
    r.gen::<f64>() < hazard
}

fn bankrupt(w: &mut World, f: usize, p: &[f64; N_SECTORS]) {
    let fid = f as FirmId;
    let workers: Vec<HhId> = w.firms[f].workers.clone();
    // b7-spikes diagnostic: headcount of the largest firm failing this month.
    w.flows.bk_max_heads = w.flows.bk_max_heads.max(workers.len() as u32);
    for h in workers {
        common::separate_worker(w, h, LaborStatus::Unemployed);
    }

    // The Fund buys the capital at a liquidation value (Fund → Firm).
    // p1-firmsentry: with the Merton test on, the fire-sale recovery is LIQUIDATION_RECOVERY × clamp(q, floor, 1), where
    // q is the firm's earning-power value and `merton_recovery_floor` sets how far a slump can cut the recovery. With
    // the floor at 1 the loss is a fixed share (the default for the profile), so the test changes who defaults, not
    // the loss given default.
    let merton_r = finite_or(w.cfg.merton_cost_of_capital, 0.0);
    let recovery = if merton_r > 0.0 {
        let q = (w.firms[f].profit_rate / merton_r).clamp(0.0, 1.0);
        let floor = finite_or(w.cfg.merton_recovery_floor, 1.0).clamp(0.0, 1.0);
        LIQUIDATION_RECOVERY * q.max(floor)
    } else {
        LIQUIDATION_RECOVERY
    };
    let liquidation = recovery * w.firms[f].capital.max(0.0) * common::capital_price(&w.cfg, p);
    ledger::pay(w, Account::Fund, Account::Firm(fid), liquidation);

    // Creditors: repay as much principal as positive deposits allow, write off the remainder.
    let loans: Vec<LoanId> = w.firms[f].loans.clone();
    for lid in loans {
        let principal = match w.loans.get(lid) {
            Some(l) => l.principal,
            None => continue,
        };
        let avail = w.firms[f].deposits.max(0.0);
        let amt = avail.min(principal);
        if amt > 0.0 {
            ledger::repay_principal(w, lid, amt);
        }
        if let Some(l) = w.loans.get(lid) {
            let (bank, biz) = (l.bank as usize, l.kind == LoanKind::Business);
            let wrote = ledger::write_off(w, lid);
            if biz {
                w.banks[bank].biz_writeoffs += wrote;
            }
        }
    }

    // Residual balance: the Fund covers an overdraft, or takes any positive remainder.
    let residual = w.firms[f].deposits;
    if residual < 0.0 {
        ledger::pay(w, Account::Fund, Account::Firm(fid), -residual);
    } else if residual > 0.0 {
        ledger::pay(w, Account::Firm(fid), Account::Fund, residual);
    }

    // Re-fit Step 5a: the written-off inventory is recorded, so the CIPI fold can treat it as a volume change.
    let (bk_sector, bk_units) = (w.firms[f].sector.idx(), w.firms[f].inventory.max(0.0));
    w.flows.bk_inventory_units[bk_sector] += bk_units;
    let fm = &mut w.firms[f];
    fm.alive = false;
    fm.months_dead = 0;
    fm.months_distressed = 0;
    fm.k_star_smooth = 0.0;
    fm.inventory = 0.0;
    fm.capital = 0.0;
    fm.target_output = 0.0;
    fm.vacancies = 0;
    fm.openings = 0;
    fm.planned_layoffs = 0;
    fm.backlog = 0.0;
    fm.expected_demand = 0.0;
    fm.investment_order = 0.0;
    w.flows.firm_bankruptcies += 1;
}

/// Per-sector averages and medians over living firms, used to set up entrants.
#[derive(Clone, Debug, Default)]
struct SectorStats {
    n: usize,
    lp: f64,
    cp: f64,
    wage: f64,
    capitals: Vec<f64>,
    /// p1-firmsentry: mean smoothed profit rate of living firms (the entry gate's profit signal).
    profit_rate: f64,
    /// b5-firmhet S1: profit rate weighted by capital value (falls back to `profit_rate` when total capital is 0).
    profit_rate_kw: f64,
    /// Median reference unit cost (`uc0`) of living firms with one set; 0 when none has one.
    uc0_median: f64,
    /// b6-firmsize S3: sector index of these statistics (the cyclical gate reads its EMAs).
    idx: usize,
    /// Median non-crude anchor (`nc0`, p1-energy) of living firms with one set; 0 when none has one.
    nc0_median: f64,
    /// b5-firmhet S1: calibrated firm count of the sector (init.rs build_firms rule).
    calib_n: f64,
}

/// b5-firmhet S1: the firm count init.rs gives a sector, `max(2, round(round(n / households_per_firm) × share))`.
fn calibrated_firm_count(n_households: usize, households_per_firm: f64, share: f64) -> usize {
    let n = n_households.max(50);
    let nf_target = (n as f64 / households_per_firm).round();
    ((nf_target * share).round() as usize).max(2)
}

fn sector_stats(w: &World) -> Vec<SectorStats> {
    let mut stats: Vec<SectorStats> = vec![SectorStats::default(); N_SECTORS];
    let mut uc0s: Vec<Vec<f64>> = vec![Vec::new(); N_SECTORS];
    let mut nc0s: Vec<Vec<f64>> = vec![Vec::new(); N_SECTORS];
    let mut kw_num = [0.0f64; N_SECTORS];
    let mut kw_den = [0.0f64; N_SECTORS];
    for (f, fm) in w.firms.iter().enumerate().filter(|(_, f)| f.alive) {
        let s = &mut stats[fm.sector.idx()];
        s.n += 1;
        s.lp += fm.labor_productivity;
        s.cp += fm.capital_productivity;
        s.wage += fm.wage_offer;
        s.profit_rate += fm.profit_rate;
        let k_w = fm.capital.max(0.0);
        kw_num[fm.sector.idx()] += k_w * fm.profit_rate;
        kw_den[fm.sector.idx()] += k_w;
        s.capitals.push(fm.capital);
        let u0 = w.firms_state.uc0.get(f).copied().unwrap_or(0.0);
        if u0 > 0.0 && u0.is_finite() {
            uc0s[fm.sector.idx()].push(u0);
        }
        let n0 = w.firms_state.nc0.get(f).copied().unwrap_or(0.0);
        if n0 > 0.0 && n0.is_finite() {
            nc0s[fm.sector.idx()].push(n0);
        }
    }
    for (j, s) in stats.iter_mut().enumerate() {
        s.idx = j;
        s.calib_n = calibrated_firm_count(w.cfg.n_households, w.cfg.households_per_firm, w.cfg.employment_share[j]) as f64;
        if s.n > 0 {
            let n = s.n as f64;
            s.lp /= n;
            s.cp /= n;
            s.wage /= n;
            s.profit_rate /= n;
            s.profit_rate_kw = if kw_den[j] > 0.0 { kw_num[j] / kw_den[j] } else { s.profit_rate };
            s.capitals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        }
        let u = &mut uc0s[j];
        if !u.is_empty() {
            u.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            s.uc0_median = u[u.len() / 2];
        }
        let n = &mut nc0s[j];
        if !n.is_empty() {
            n.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            s.nc0_median = n[n.len() / 2];
        }
    }
    stats
}

/// p1-firmsentry: an employed household that founds the entrant in slot `f` (a draw on the ENTRY stream with its own
/// key). Up to `FOUNDER_TRIES` random households are tried; None if none is employed.
fn pick_founder(w: &World, f: usize) -> Option<HhId> {
    let n = w.households.len();
    if n == 0 {
        return None;
    }
    let mut r = rng::agent_rng(w.seed, w.tick, stream::ENTRY, FOUNDER_KEY + f as u64);
    for _ in 0..FOUNDER_TRIES {
        let h = ((r.gen::<f64>() * n as f64) as usize).min(n - 1);
        let hh = &w.households[h];
        if hh.alive && hh.status == LaborStatus::Employed {
            return Some(h as HhId);
        }
    }
    None
}

/// Reborn a dead slot as an entrant in the same sector (§7.5).
fn reborn(w: &mut World, f: usize, stats: &[SectorStats], p: &[f64; N_SECTORS]) {
    let s = w.firms[f].sector;
    let j = s.idx();
    let st = &stats[j];
    let mut rng = rng::agent_rng(w.seed, w.tick, stream::ENTRY, f as u64);

    // p1-firmsentry: with the productivity process on, entrants are drawn at the sector's anchored (geometric-mean)
    // level with zero log-mean noise, so they do not enter above the typical incumbent.
    let anchor = if w.cfg.firm_prod_sigma > 0.0 { w.firms_state.lp_anchor[j] } else { 0.0 };
    let lp = if anchor > 0.0 {
        let sig = w.cfg.entrant_productivity_sigma.max(0.0);
        (anchor * w.exog.tfp[j] * (sig * rng::normal(&mut rng)).exp()).max(1e-9)
    } else {
        let base_lp = if st.n > 0 { st.lp } else { calibrated_labor_productivity(&w.cfg, s) };
        (base_lp * rng::lognormal_mean1(&mut rng, w.cfg.entrant_productivity_sigma)).max(1e-9)
    };
    let cp = if st.n > 0 && st.cp > 0.0 { st.cp } else { FALLBACK_CAPITAL_PRODUCTIVITY };
    let legacy_capital = if st.n > 0 && !st.capitals.is_empty() {
        0.5 * st.capitals[st.capitals.len() / 2]
    } else {
        0.5 * lp / cp
    };
    // b5-firmhet S1: entrant capital floor (off at 0: legacy 0.5 × median capital).
    let capital = entrant_capital(legacy_capital, w.cfg.entrant_capital_floor_workers, lp, cp, w.cfg.target_utilization);
    let wage_offer = if st.n > 0 && st.wage > 0.0 { st.wage } else { w.firms[f].wage_offer };
    let price = p[j];
    let markup = w.cfg.initial_markup[j];
    let capacity = capital * cp;

    // Startup equity: `entrant_startup_months` of a two-worker payroll. In proprietor sectors an employed household
    // founds the firm and pays what its deposits allow; the Fund covers the rest (the founder becomes the owner, so
    // profits are paid to the household: proprietors' income). Elsewhere the Fund pays it all.
    let startup = w.cfg.entrant_startup_months * 2.0 * wage_offer * (1.0 + w.cfg.payroll_tax);
    let founder = if w.cfg.owner_founders && w.cfg.proprietor_sectors[j] { pick_founder(w, f) } else { None };
    match founder {
        Some(h) => {
            let from_founder = ledger::pay_up_to(w, Account::Household(h), Account::Firm(f as FirmId), startup);
            ledger::pay(w, Account::Fund, Account::Firm(f as FirmId), (startup - from_founder).max(0.0));
            // The founder leaves the current job (job-to-job, with the continuous-employment count kept) and works
            // in the new firm as its first worker at the entrant's wage, so owners work in their own firms.
            // Re-fit Step 4b: with `founder_wage_hc` the founder is paid like any other hire, at the wage offer times
            // human capital (legacy: the bare offer).
            let founder_wage = if w.cfg.founder_wage_hc {
                wage_offer * common::human_capital(&w.cfg, &w.households[h as usize])
            } else {
                wage_offer
            };
            common::hire_worker(w, h, f as FirmId, founder_wage);
        }
        None => ledger::pay(w, Account::Fund, Account::Firm(f as FirmId), startup),
    }

    let fm = &mut w.firms[f];
    fm.owner = founder;
    fm.alive = true;
    fm.age_months = 0;
    fm.months_dead = 0;
    fm.months_distressed = 0;
    fm.k_star_smooth = 0.0;
    fm.labor_productivity = lp;
    fm.capital = capital;
    fm.capital_productivity = cp;
    fm.wage_offer = wage_offer;
    fm.price = price;
    fm.markup = markup;
    fm.months_since_reprice = 0;
    fm.unit_cost = price / (1.0 + markup);
    fm.inventory = 0.0;
    fm.output = 0.0;
    fm.sales = 0.0;
    fm.demand = 0.0;
    fm.target_output = 0.0;
    fm.vacancies = 0;
    fm.openings = 0;
    fm.planned_layoffs = 0;
    fm.backlog = 0.0;
    fm.expected_demand = 0.5 * capacity;
    fm.profit_rate = ENTRANT_PROFIT_RATE;
    fm.investment_order = 0.0;

    // Entrants inherit the sector's prevailing reference unit cost, so their own first cost does not set a permanent
    // Energy price anchor (uc/uc0 would otherwise undercut incumbents for good).
    let st = &mut w.firms_state;
    st.uc0[f] = stats[j].uc0_median;
    st.nc0[f] = stats[j].nc0_median;
    st.labor_share0[f] = 0.0;
    st.workers_at_plan[f] = 0;
    st.last_demand[f] = fm.expected_demand;
    st.last_sales[f] = 0.0;
    st.last_output[f] = 0.0;
    st.last_interest[f] = 0.0;
    st.last_costs[f] = 0.0;
    // `entrant_loss_cf_reset`: the failed predecessor's tax losses died with it; the entrant starts with none.
    if w.cfg.entrant_loss_cf_reset {
        if let Some(cf) = w.fiscal.firm_loss_cf.get_mut(f) {
            *cf = 0.0;
        }
    }
    w.flows.firm_entries += 1;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::sim;
    use crate::testkit::tiny_world;

    #[test]
    fn extraction_labour_sees_no_real_oil_drift_with_index_on() {
        // The extraction divisor must use the real oil ratio: with the index on and no oil noise, the nominal ratio
        // grows with the price level, but the real ratio that firms use (common::real_oil_ratio) stays at 1, so the
        // extraction factor does not drift. The nominal factor is shown to drift, so the test has power.
        let cfg = Config { n_households: 60, oil_index_to_world: true, stochastic_oil: false, ..Default::default() };
        let share = cfg.energy_extraction_labor_share;
        let eps = cfg.energy_extraction_elasticity;
        let mut w = tiny_world(cfg.clone(), 41);
        let factor = |r: f64| (1.0 - share) + share * r.max(0.1).powf(eps);
        let mut real_factors = Vec::new();
        let mut nominal_factor_late = 0.0;
        for t in [0u32, 240] {
            w.tick = t;
            crate::exog::step(&mut w);
            real_factors.push(factor(crate::common::real_oil_ratio(&w)));
            if t == 240 {
                nominal_factor_late = factor(crate::common::oil_ratio(&w));
            }
        }
        assert!((real_factors[0] - 1.0).abs() < 1e-12, "real factor at 0 {}", real_factors[0]);
        assert!((real_factors[1] - 1.0).abs() < 1e-12, "real factor at 240 {}", real_factors[1]);
        assert!(nominal_factor_late > 1.1, "nominal factor at 240 {nominal_factor_late} (test lacks power)");
        // The labour factor itself, as firms apply it, equals extraction_factor on the real ratio.
        assert_eq!(extraction_factor(&cfg, Sector::Energy, 1.0).to_bits(), 1.0f64.to_bits());
        assert_eq!(extraction_factor(&cfg, Sector::Food, 2.0).to_bits(), 1.0f64.to_bits());
    }

    fn test_world(seed: u64) -> World {
        tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            seed,
        )
    }

    // rz1 F1: the firms' perceived business base rate (`rate_plan`) moves 1 / (1 + lag) of the way to this month's mean each
    // month; with lag 0 it is the current rate. This is the existing smoother that the investment user cost reads (`perceived`).
    #[test]
    fn firms_perceived_rate_follows_rate_perception_months() {
        let step = |lag: f64| -> (f64, f64, f64) {
            let mut w = tiny_world(Config { n_households: 60, rate_perception_months: lag, ..Default::default() }, 7);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.05;
            }
            plan(&mut w);
            let first = w.firms_state.rate_plan;
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.06;
            }
            plan(&mut w);
            (first, w.firms_state.rate_plan, w.firms_state.rate_now)
        };
        let (first, second, now) = step(9.0);
        assert!((first - 0.05).abs() < 1e-15, "the first call sets the perceived rate to the current one");
        assert!((now - 0.06).abs() < 1e-15);
        assert!((second - (0.05 + 0.01 / 10.0)).abs() < 1e-12, "moves 1 / (1 + 9) of the way: {second}");
        let (_, second0, now0) = step(0.0);
        assert!((second0 - 0.06).abs() < 1e-15 && (now0 - 0.06).abs() < 1e-15, "lag 0 reads the current rate");
    }

    // rz1 F1: `rate_perception_firm_months` >= 0 sets the firms' lag on its own; negative follows `rate_perception_months`.
    #[test]
    fn firm_lag_override_replaces_the_common_lag_for_firms_only() {
        let second_rate_plan = |common: f64, firm: f64| -> f64 {
            let cfg = Config { n_households: 60, rate_perception_months: common, rate_perception_firm_months: firm, ..Default::default() };
            let mut w = tiny_world(cfg, 7);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.05;
            }
            plan(&mut w);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.06;
            }
            plan(&mut w);
            w.firms_state.rate_plan
        };
        assert!((second_rate_plan(9.0, -1.0) - (0.05 + 0.01 / 10.0)).abs() < 1e-12, "negative: the common lag");
        assert!((second_rate_plan(9.0, 3.0) - (0.05 + 0.01 / 4.0)).abs() < 1e-12, "override 3 replaces 9");
        assert!((second_rate_plan(0.0, 3.0) - (0.05 + 0.01 / 4.0)).abs() < 1e-12, "override works with a zero common lag");
        assert!((second_rate_plan(9.0, 0.0) - 0.06).abs() < 1e-15, "override 0 means no firm lag although the common lag is 9");
    }

    // rz12: `rate_perception_firm_stages` (negative: `rate_perception_stages`) above 1 makes the firms' perceived rate a cascade with the same
    // mean delay; one stage is the single smoother.
    #[test]
    fn firm_stages_cascade_follows_the_stage_keys() {
        let rate_plan_after = |lag: f64, stages: u32, firm_stages: i32, months: usize| -> f64 {
            let cfg = Config { n_households: 60, rate_perception_months: lag, rate_perception_stages: stages, rate_perception_firm_stages: firm_stages, ..Default::default() };
            let mut w = tiny_world(cfg, 7);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.05;
            }
            plan(&mut w);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.06;
            }
            for _ in 0..months {
                plan(&mut w);
            }
            w.firms_state.rate_plan
        };
        // One stage is the legacy smoother: moves 1 / (1 + lag) of the way in the first month.
        assert!((rate_plan_after(10.0, 1, -1, 1) - (0.05 + 0.01 / 11.0)).abs() < 1e-12);
        // Two stages of lag 5: after one month the first stage is at 0.05 + 0.01/6, the second at 0.05 + (0.01/6)/6.
        let s1 = 0.01 / 6.0;
        assert!((rate_plan_after(10.0, 2, -1, 1) - (0.05 + s1 / 6.0)).abs() < 1e-12, "{}", rate_plan_after(10.0, 2, -1, 1));
        // The firm key overrides the household key, in both directions; the households' key alone reaches the firms when the firm key is negative.
        assert_eq!(rate_plan_after(10.0, 1, 2, 3).to_bits(), rate_plan_after(10.0, 2, -1, 3).to_bits());
        assert_eq!(rate_plan_after(10.0, 2, 1, 3).to_bits(), rate_plan_after(10.0, 1, -1, 3).to_bits());
        // Gamma-shaped: three months after a step the second-stage perception has moved less than the single smoother's.
        assert!(rate_plan_after(10.0, 2, -1, 3) < rate_plan_after(10.0, 1, -1, 3));
    }

    // rz12: with `rate_perception_credit_bypass` the CreditSpread shock's part of the business base rate is perceived at once; the rest
    // is smoothed. Off (0) is the legacy smoother; the bypass share is graded.
    #[test]
    fn credit_bypass_perceives_the_spread_shock_at_once() {
        let gap_after_shock = |bypass: f64| -> f64 {
            let cfg = Config { n_households: 60, rate_perception_months: 10.0, rate_perception_credit_bypass: bypass, ..Default::default() };
            let mut w = tiny_world(cfg, 7);
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.05;
            }
            plan(&mut w);
            // a spread shock of 200 bp is in force this tick and raises every bank's base rate by it
            let t = w.tick;
            w.shocks.credit_spread = Some((0.02, t + 12));
            for b in w.banks.iter_mut() {
                b.business_rate_base = 0.05 + 0.02;
            }
            plan(&mut w);
            w.firms_state.rate_plan - w.firms_state.rate_now // perceived minus current
        };
        assert!((gap_after_shock(0.0) - (-0.02 * 10.0 / 11.0)).abs() < 1e-12, "no bypass: the perceived rate lags the whole jump");
        assert!(gap_after_shock(1.0).abs() < 1e-12, "full bypass: the spread shock is perceived at once, nothing else moved");
        assert!((gap_after_shock(0.5) - (-0.01 * 10.0 / 11.0)).abs() < 1e-12, "half bypass");
    }

    // rz12: the bypassed amount is the spread shock's value including its decaying tail, the amount `banking::set_rates` adds to the base rate.
    #[test]
    fn credit_bypass_follows_the_decaying_tail() {
        let cfg = Config { n_households: 60, rate_perception_months: 10.0, rate_perception_credit_bypass: 1.0, credit_tail_half_life_months: 5.0, ..Default::default() };
        let mut w = tiny_world(cfg, 7);
        for b in w.banks.iter_mut() {
            b.business_rate_base = 0.05;
        }
        plan(&mut w);
        // the shock's window ended at this tick: its contribution is one step down the tail, 0.02 x 0.5^(1/5)
        let t = w.tick;
        w.shocks.credit_spread = Some((0.02, t));
        let tail = 0.02 * 0.5f64.powf(1.0 / 5.0);
        assert!((w.shocks.credit_spread_add_tail(t, 5.0) - tail).abs() < 1e-15);
        for b in w.banks.iter_mut() {
            b.business_rate_base = 0.05 + tail;
        }
        plan(&mut w);
        assert!((w.firms_state.rate_plan - w.firms_state.rate_now).abs() < 1e-12, "the tail is perceived at once: {}", w.firms_state.rate_plan - w.firms_state.rate_now);
    }

    /// `test_world` on the pre-re-fit defaults (see `testkit::legacy_p1off_config`).
    fn legacy_test_world(seed: u64) -> World {
        tiny_world(Config { n_households: 500, ..crate::testkit::legacy_p1off_config() }, seed)
    }

    /// One emulated month: plan → produce → goods market (demand = expected demand × factor) → close_books.
    fn run_month(w: &mut World, demand_factor: f64) {
        sim::begin_month(w);
        plan(w);
        produce(w);
        for f in 0..w.firms.len() {
            if !w.firms[f].alive {
                continue;
            }
            let demand = w.firms[f].expected_demand * demand_factor;
            let sales = demand.min(w.firms[f].inventory.max(0.0));
            let revenue = sales * w.firms[f].price;
            w.firms[f].demand = demand;
            w.firms[f].sales = sales;
            w.firms[f].inventory -= sales;
            w.firms[f].revenue = revenue;
            ledger::pay(w, Account::CentralBank, Account::Firm(f as FirmId), revenue);
            // Inputs and capital goods are paid for at their ordered value; capital arrives in full.
            let inputs: f64 = w.orders.firm_inputs[f].iter().sum();
            let capex: f64 = w.orders.firm_investment[f].iter().sum();
            w.firms[f].input_cost = inputs;
            ledger::pay(w, Account::Firm(f as FirmId), Account::CentralBank, inputs + capex);
            w.fills.firm_capital_received[f] = w.firms[f].investment_order;
        }
        close_books(w);
        w.tick += 1;
    }

    #[test]
    fn markups_respond_to_unmet_demand_and_inventory_glut() {
        let mut cfg = Config {
            n_households: 500,
            ..Default::default()
        };
        cfg.reprice_prob = [1.0; N_SECTORS];
        let mut w = tiny_world(cfg, 3);
        ensure_state(&mut w);
        let goods: Vec<usize> =
            (0..w.firms.len()).filter(|&f| w.firms[f].sector == Sector::DurableGoods).collect();
        let (short, glut) = (goods[0], goods[1]);
        let m0 = w.cfg.initial_markup[Sector::DurableGoods.idx()];

        // Unmet demand (half of last month's demand unsold) → markup rises.
        w.firms_state.months_run = 1;
        w.firms_state.last_demand[short] = 1.0;
        w.firms_state.last_sales[short] = 0.5;
        w.firms[short].inventory = 0.0;
        // Large inventory above the sector average price → markup falls.
        w.firms[glut].price = 1.2;
        w.firms[glut].inventory = 10.0 * w.firms[glut].expected_demand + 10.0;
        w.firms_state.last_demand[glut] = 1.0;
        w.firms_state.last_sales[glut] = 1.0;
        plan(&mut w);
        assert!(w.firms[short].markup > m0, "unmet demand should raise the markup");
        assert!(w.firms[glut].markup < m0, "inventory glut at a high price should lower the markup");
        assert!(w.firms[glut].price <= 1.2 * 1.1 + 1e-9 && w.firms[glut].price >= 1.2 * 0.9 - 1e-9);
    }

    #[test]
    fn vacancies_rise_with_demand_and_layoffs_follow_a_fall() {
        let w0 = test_world(5);
        let mut up = w0.clone();
        let mut down = w0.clone();
        for f in 0..w0.firms.len() {
            up.firms[f].expected_demand *= 1.1;
            down.firms[f].expected_demand *= 0.5;
        }
        plan(&mut up);
        plan(&mut down);
        let vac = |w: &World| w.firms.iter().map(|f| f.vacancies as u64).sum::<u64>();
        let lay = |w: &World| w.firms.iter().map(|f| f.planned_layoffs as u64).sum::<u64>();
        assert!(vac(&up) > vac(&down), "vacancies should rise with demand");
        assert!(lay(&down) > 0, "a demand fall should plan layoffs");
        assert_eq!(lay(&up), 0, "a demand rise should plan no layoffs");
    }

    #[test]
    fn vacancies_rise_with_demand_in_throttled_modes() {
        // b4-asym M1: the demand rise / fall test also holds for the round-up and hybrid postings.
        for mode in [1u32, 2] {
            let w0 = {
                let mut w = test_world(5);
                w.cfg.vacancy_net_share = 0.3;
                w.cfg.vacancy_rounding = mode;
                w.cfg.vacancy_free_units = 1.0;
                w
            };
            let mut up = w0.clone();
            let mut down = w0.clone();
            for f in 0..w0.firms.len() {
                up.firms[f].expected_demand *= 1.1;
                down.firms[f].expected_demand *= 0.5;
            }
            plan(&mut up);
            plan(&mut down);
            let vac = |w: &World| w.firms.iter().map(|f| f.vacancies as u64).sum::<u64>();
            let lay = |w: &World| w.firms.iter().map(|f| f.planned_layoffs as u64).sum::<u64>();
            assert!(vac(&up) > vac(&down), "mode {mode}: vacancies should rise with demand");
            assert!(lay(&down) > 0, "mode {mode}: a demand fall should plan layoffs");
            assert_eq!(lay(&up), 0, "mode {mode}: a demand rise should plan no layoffs");
        }
    }

    #[test]
    fn wages_paid_equal_workers_wages_and_payroll_tax() {
        let mut w = test_world(11);
        plan(&mut w);
        produce(&mut w);
        let pt = w.cfg.payroll_tax;
        let mut total_wage_bill = 0.0;
        for f in w.firms.iter().filter(|f| f.alive) {
            let worker_wages: f64 = f.workers.iter().map(|&h| w.households[h as usize].wage).sum();
            let paid: f64 = f.workers.iter().map(|&h| w.hh_month[h as usize].income.wage).sum();
            assert!((paid - worker_wages).abs() < 1e-6 * (1.0 + worker_wages));
            let ben = w.cfg.employer_benefits;
            assert!((f.wage_bill - worker_wages * (1.0 + pt + ben)).abs() < 1e-6 * (1.0 + worker_wages));
            total_wage_bill += f.wage_bill;
        }
        let total_income: f64 = w.hh_month.iter().map(|m| m.income.wage).sum();
        assert!((total_income - w.flows.wages).abs() < 1e-6 * (1.0 + total_income));
        assert!(total_wage_bill > 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    // p1-energy (`energy_fuel_split`): the Energy price is the fuel line (half the index, crude ≈ 0.52 of its price) with
    // utilities (non-crude cost, and the 12-month average oil ratio at elasticity 0.1). A 30% oil jump moves the fuel
    // line ≈ 16% and utilities only ≈ 0.25% after one month.
    #[test]
    fn energy_fuel_split_price_follows_fuel_line() {
        let mut w = test_world(13);
        w.cfg.energy_fuel_split = true;
        plan(&mut w);
        let energy: Vec<usize> = (0..w.firms.len()).filter(|&f| w.firms[f].sector == Sector::Energy).collect();
        for &f in &energy {
            assert!((w.firms[f].price - 1.0).abs() < 1e-9);
        }
        w.exog.oil_price = 1.3 * w.exog.oil_base;
        plan(&mut w);
        for &f in &energy {
            let p = w.firms[f].price;
            assert!(p > 1.05 && p <= 1.12, "energy price {p} should follow a 30% oil jump through the fuel line");
        }
    }

    // p1-energy: an Energy entrant born while oil is at 3x its anchor takes the sector's median non-crude anchor. The
    // old anchor (uc0 less the current crude term) went to the 1e-9 floor here and compounded the entrant's price.
    #[test]
    fn energy_entrant_anchor_ignores_current_oil() {
        let mut w = test_world(13);
        w.cfg.energy_fuel_split = true;
        plan(&mut w);
        w.exog.oil_price = 3.0 * w.exog.oil_base;
        for _ in 0..3 {
            plan(&mut w);
        }
        let f = (0..w.firms.len())
            .find(|&f| w.firms[f].sector == Sector::Energy && w.firms[f].alive)
            .expect("an Energy firm");
        w.firms[f].alive = false;
        w.firms[f].months_dead = ENTRY_DELAY_MONTHS;
        let stats = sector_stats(&w);
        let med = stats[Sector::Energy.idx()].nc0_median;
        assert!(med > 0.0, "incumbent Energy firms should carry a non-crude anchor");
        let p = sector_prices(&w);
        reborn(&mut w, f, &stats, &p);
        let nc0 = w.firms_state.nc0[f];
        assert!((nc0 / med - 1.0).abs() < 1e-9, "entrant anchor {nc0} should equal the incumbents' median {med}");
        plan(&mut w);
        let price = w.firms[f].price;
        assert!(price.is_finite() && price > 0.5 && price < 2.0, "entrant Energy price {price} should not compound");
    }

    #[test]
    fn energy_price_tracks_oil() {
        let mut w = legacy_test_world(13);
        plan(&mut w); // sets uc0 at the base oil price; Energy prices start at 1.0
        let energy: Vec<usize> = (0..w.firms.len()).filter(|&f| w.firms[f].sector == Sector::Energy).collect();
        for &f in &energy {
            assert!((w.firms[f].price - 1.0).abs() < 1e-9);
        }
        w.exog.oil_price = 1.3 * w.exog.oil_base;
        plan(&mut w);
        for &f in &energy {
            let p = w.firms[f].price;
            assert!(p > 1.1 && p <= 1.25 + 1e-9, "energy price {p} should follow a 30% oil jump");
        }
    }

    #[test]
    fn owner_operated_dividend_reaches_owner_and_stays_consistent() {
        let mut w = test_world(11);
        let f = w.firms.iter().position(|fm| fm.alive).expect("an alive firm");
        let owner: HhId = 0;
        w.households[owner as usize].alive = true;
        w.firms[f].owner = Some(owner);
        ledger::pay(&mut w, Account::CentralBank, Account::Firm(f as FirmId), 1e6);
        // Identical world in which the same firm is shareholder-owned: its payout goes to the Fund instead.
        let mut wf = w.clone();
        wf.firms[f].owner = None;
        for x in [&mut w, &mut wf] {
            sim::begin_month(x);
            x.firms[f].revenue = 10_000.0;
            close_books(x);
        }
        let paid = w.firms[f].dividends;
        assert!(paid > 0.0, "a profitable, liquid firm pays a dividend");
        assert_eq!(wf.firms[f].dividends, paid);
        assert_eq!(w.firms[f].owner, Some(owner));
        // Owner receives exactly the payout (deposits and income.business); the Fund receives one payout less.
        assert!((w.households[owner as usize].deposits - wf.households[owner as usize].deposits - paid).abs() < 1e-6);
        assert!((w.hh_month[owner as usize].income.business - paid).abs() < 1e-9);
        let fund_in = |x: &World| x.fund.dividends_received + x.fund.special_dividends;
        assert!((fund_in(&wf) - fund_in(&w) - paid).abs() < 1e-6);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn dead_owner_is_cleared_and_dividend_goes_to_fund() {
        let mut w = test_world(12);
        let f = w.firms.iter().position(|fm| fm.alive).expect("an alive firm");
        let owner: HhId = 0;
        w.households[owner as usize].alive = false;
        w.firms[f].owner = Some(owner);
        ledger::pay(&mut w, Account::CentralBank, Account::Firm(f as FirmId), 1e6);
        sim::begin_month(&mut w);
        w.firms[f].revenue = 10_000.0;
        close_books(&mut w);
        assert_eq!(w.firms[f].owner, None);
        assert!(w.firms[f].dividends > 0.0);
        assert_eq!(w.hh_month[owner as usize].income.business, 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn bankruptcy_and_reentry() {
        let mut w = test_world(17);
        let f = (0..w.firms.len()).find(|&f| w.firms[f].sector == Sector::OtherServices).unwrap();
        let n_workers = w.firms[f].workers.len();
        assert!(n_workers > 0);
        // Drain the firm's deposits far below zero.
        let d = w.firms[f].deposits;
        ledger::pay(&mut w, Account::Firm(f as FirmId), Account::Government, d + 1.0e9);

        run_month(&mut w, 1.0);
        assert!(!w.firms[f].alive, "insolvent firm should go bankrupt");
        assert!(w.firms[f].workers.is_empty());
        assert!(w.firms[f].deposits.abs() < 1e-6, "dead firm ends with zero deposits");
        assert!(w.flows.firm_bankruptcies >= 1);
        ledger::check_consistency(&w).unwrap();
        assert!(w.households.iter().filter(|h| h.status == LaborStatus::Unemployed).count() > 0);

        for _ in 0..3 {
            run_month(&mut w, 1.0);
        }
        assert!(w.firms[f].alive, "slot should be reborn after the entry delay");
        assert!(w.firms[f].deposits > 0.0);
        assert!(w.flows.firm_entries >= 1);
        ledger::check_consistency(&w).unwrap();
    }

    /// `entrant_loss_cf_reset`: a firm reborn in a bankrupt slot starts with no corporate-tax loss carry-forward and
    /// pays the tax on its first profit. Legacy (`false`) hands it the failed predecessor's losses.
    #[test]
    fn reborn_firm_starts_with_no_loss_carry_forward() {
        let run = |reset: bool| -> (f64, f64) {
            let mut w = test_world(17);
            assert!(w.cfg.entrant_loss_cf_reset, "the fix is the default");
            w.cfg.entrant_loss_cf_reset = reset;
            let f = (0..w.firms.len()).find(|&f| w.firms[f].sector == Sector::OtherServices).unwrap();
            // The failing firm has piled up tax losses, and goes bankrupt.
            w.fiscal.firm_loss_cf.resize(w.firms.len(), 0.0);
            w.fiscal.firm_loss_cf[f] = 1.0e9;
            let d = w.firms[f].deposits;
            ledger::pay(&mut w, Account::Firm(f as FirmId), Account::Government, d + 1.0e9);
            run_month(&mut w, 1.0);
            assert!(!w.firms[f].alive, "insolvent firm should go bankrupt");
            for _ in 0..3 {
                run_month(&mut w, 1.0);
            }
            assert!(w.firms[f].alive, "slot should be reborn after the entry delay");
            let cf = w.fiscal.firm_loss_cf[f];
            // The entrant's first month with a pre-tax profit of 100,000.
            sim::begin_month(&mut w);
            w.firms[f].revenue = 1.0e5;
            crate::fiscal::collect_and_finance(&mut w);
            (cf, w.firms[f].taxes)
        };
        let (cf, tax) = run(true);
        assert_eq!(cf, 0.0);
        let want = Config::default().corporate_tax * 1.0e5;
        assert!((tax - want).abs() <= 1e-9 * want, "entrant pays {tax}, want {want}");
        let (cf, tax) = run(false);
        assert_eq!(cf, 1.0e9, "legacy: the entrant inherits the predecessor's losses");
        assert_eq!(tax, 0.0);
    }

    #[test]
    fn two_years_of_emulated_demand_keeps_prices_stable() {
        let mut w = legacy_test_world(23);
        let mut max_move: f64 = 0.0;
        let mut profitable_months = 0usize;
        let mut firm_months = 0usize;
        for m in 0..24 {
            let before: Vec<f64> = w.firms.iter().map(|f| f.price).collect();
            run_month(&mut w, 1.0);
            for (f, fm) in w.firms.iter().enumerate() {
                if fm.alive && before[f] > 0.0 {
                    max_move = max_move.max((fm.price / before[f] - 1.0).abs());
                    firm_months += 1;
                    if fm.profit > 0.0 {
                        profitable_months += 1;
                    }
                }
            }
            ledger::check_consistency(&w).unwrap();
            if m % 6 == 5 {
                let alive = w.firms.iter().filter(|f| f.alive).count();
                let avg_price: f64 = w.firms.iter().filter(|f| f.alive).map(|f| f.price).sum::<f64>() / alive as f64;
                eprintln!(
                    "month {:2}: alive {:3}/{}, avg price {:.4}, bankruptcies {}, entries {}",
                    m + 1,
                    alive,
                    w.firms.len(),
                    avg_price,
                    w.flows.firm_bankruptcies,
                    w.flows.firm_entries
                );
            }
        }
        for s in Sector::ALL {
            let fs: Vec<&Firm> = w.firms.iter().filter(|f| f.alive && f.sector == s).collect();
            let n = fs.len() as f64;
            eprintln!(
                "{:13} price {:.3} uc {:.3} markup {:.3} profit_rate {:+.3} deposits/costs {:.2} capital/output {:.1}",
                s.name(),
                fs.iter().map(|f| f.price).sum::<f64>() / n,
                fs.iter().map(|f| f.unit_cost).sum::<f64>() / n,
                fs.iter().map(|f| f.markup).sum::<f64>() / n,
                fs.iter().map(|f| f.profit_rate).sum::<f64>() / n,
                fs.iter().map(|f| f.deposits / (f.wage_bill + f.input_cost + f.interest_paid + 1e-9)).sum::<f64>() / n,
                fs.iter().map(|f| f.capital * f.capital_productivity / (f.output + 1e-9)).sum::<f64>() / n,
            );
        }
        let share = profitable_months as f64 / firm_months.max(1) as f64;
        eprintln!("max monthly price move {:.3}, profitable firm-months {:.1}%", max_move, 100.0 * share);
        for fm in w.firms.iter().filter(|f| f.alive) {
            assert!(fm.price.is_finite() && (0.9..=1.1).contains(&fm.price), "price {} outside ±10%", fm.price);
        }
        assert!(max_move <= ENERGY_PRICE_MOVE_MAX + 1e-9);
        let prices: Vec<f64> = w.firms.iter().filter(|f| f.alive && f.sector != Sector::Energy).map(|f| f.price).collect();
        let mean = prices.iter().sum::<f64>() / prices.len() as f64;
        assert!((mean - 1.0).abs() < 0.10, "non-energy prices drifted to {mean:.3}");
    }

    /// b7-tariff (spec 5.8): the imported-input multiplier at τ = 0.1 with no lagged factor is the duty-inclusive
    /// landed price; with the flag off, or with no duty and no lagged factor, it is the legacy expression, bit for bit.
    #[test]
    fn goods_input_mult_carries_the_duty_only_when_the_mechanism_is_on() {
        let on = Config::default();
        let off = Config { tariff_pass_through: false, ..Config::default() };
        let (s, imp_rel) = (0.2, 1.05);
        let with_duty = goods_input_mult(&on, 0.1, 0.0, imp_rel, s);
        assert!((with_duty - ((1.0 - s) + s * imp_rel * 1.1)).abs() <= 1e-12, "{with_duty}");
        let legacy = (1.0 - s) + s * imp_rel;
        assert_eq!(goods_input_mult(&off, 0.1, 0.0, imp_rel, s), legacy);
        assert_eq!(goods_input_mult(&on, 0.0, 0.0, imp_rel, s), legacy);
        assert_eq!(goods_input_mult(&off, 0.0, 0.0, imp_rel, s), legacy);
    }
}

#[cfg(test)]
mod upstream_split_tests {
    use super::*;
    use crate::common::energy_crude_unit_cost;
    use crate::config::Config;
    use crate::ledger;
    use crate::testkit::{capital_output_months, depreciation_share_for, tiny_world, CAPITAL_OUTPUT_MONTHS};

    fn small(cfg: Config) -> Config {
        Config { n_households: 500, ..cfg }
    }

    /// Re-fit Step 4c (i): at the defaults the crude term in Energy's unit cost plus the upstream capital share is the
    /// full crude intensity, so the split moves the cost between Energy's price and its capital and changes no total.
    #[test]
    fn upstream_split_conserves_the_crude_intensity() {
        let cfg = Config::default();
        let (_, _, _, kappa) = cfg.upstream_split().expect("split is on at the defaults");
        let s = cfg.oil_import_share;
        let total = energy_crude_unit_cost(&cfg, 1.0) + kappa * cfg.crude_intensity * (1.0 - s);
        assert!((total - cfg.crude_intensity).abs() < 1e-12, "crude {total} vs {}", cfg.crude_intensity);
        assert!(kappa > 0.5 && kappa < 0.8, "upstream capital share {kappa}");
        assert!(cfg.upstream_split().is_some());
        assert!(capital_output_months(&cfg, Sector::Energy) > CAPITAL_OUTPUT_MONTHS + 10.0);
        assert_eq!(capital_output_months(&cfg, Sector::Food), CAPITAL_OUTPUT_MONTHS);
    }

    /// Re-fit Step 4c (ii): Energy's calibrated labour share is above 0.05 with the defaults and with the legacy config.
    #[test]
    fn energy_labour_share_stays_positive_under_both_paths() {
        for cfg in [Config::default(), crate::testkit::legacy_p1off_config()] {
            let j = Sector::Energy.idx();
            let inputs: f64 = (0..N_SECTORS).map(|i| cfg.io[i][j]).sum();
            let labor_share = 1.0 / (1.0 + cfg.initial_markup[j]) - inputs
                - energy_crude_unit_cost(&cfg, 1.0)
                - depreciation_share_for(&cfg, Sector::Energy)
                - 0.01;
            assert!(labor_share > 0.05, "labour share {labor_share}");
        }
    }

    /// Re-fit Step 4c (iii): after one month of production with the split on, the Fund receives the royalties
    /// recorded in the flows, the Treasury receives at least the upstream taxes, and the ledger is consistent.
    #[test]
    fn upstream_payments_reach_the_fund_and_treasury_and_keep_the_ledger() {
        let mut w = tiny_world(small(Config::default()), 3);
        let fund0 = w.fund.deposits;
        let tga0 = w.cb.tga;
        plan(&mut w);
        produce(&mut w);
        assert!(w.flows.upstream_tax > 0.0, "no upstream tax recorded");
        let ratio = w.flows.upstream_royalty / w.flows.upstream_tax;
        assert!((ratio - 0.10 / 0.08).abs() < 1e-9, "royalty to tax ratio {ratio}");
        assert!((w.fund.deposits - fund0 - w.flows.upstream_royalty).abs() < 1e-6 * (1.0 + fund0.abs()));
        assert!(w.cb.tga - tga0 >= w.flows.upstream_tax - 1e-6, "TGA did not receive the upstream tax");
        ledger::check_consistency(&w).expect("ledger consistent after upstream payments");
    }

    /// Re-fit Step 4c: the legacy path (`upstream_margin` 1.0) pays no upstream tax or royalty.
    #[test]
    fn legacy_path_pays_no_upstream_payments() {
        let mut w = tiny_world(small(crate::testkit::legacy_p1off_config()), 3);
        plan(&mut w);
        produce(&mut w);
        assert_eq!(w.flows.upstream_tax, 0.0);
        assert_eq!(w.flows.upstream_royalty, 0.0);
    }
}

#[cfg(test)]
mod bankruptcy_tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    /// Re-fit Step 5b: the default draw is Bernoulli(hazard) across firms, and the 1/6 hazard gives a mean distress spell
    /// of six months, the same as the deterministic counter it replaces.
    #[test]
    fn default_draw_has_the_hazard_frequency() {
        let h = 1.0 / 6.0;
        let n = 20_000usize;
        let hits = (0..n).filter(|&f| firm_default_draw(7, 40, f, h)).count();
        let freq = hits as f64 / n as f64;
        assert!((freq - h).abs() < 0.01, "default frequency {freq} against hazard {h}");
        // The draw is deterministic in its key and differs across months.
        assert_eq!(firm_default_draw(7, 40, 3, h), firm_default_draw(7, 40, 3, h));
    }

    /// Re-fit Step 5a: a bankrupt firm's inventory is recorded in the flows by sector, and the firm's stock goes to zero.
    #[test]
    fn bankruptcy_records_the_written_off_inventory() {
        let cfg = Config { n_households: 500, ..Default::default() };
        let mut w = tiny_world(cfg, 5);
        let f = w.firms.iter().position(|fm| fm.alive && fm.inventory > 0.0).unwrap_or(0);
        w.firms[f].inventory = 7.5;
        let sector = w.firms[f].sector.idx();
        let before = w.flows.bk_inventory_units;
        let p = [1.0; N_SECTORS];
        bankrupt(&mut w, f, &p);
        assert!(!w.firms[f].alive);
        assert!((w.flows.bk_inventory_units[sector] - before[sector] - 7.5).abs() < 1e-12);
        assert_eq!(w.firms[f].inventory, 0.0);
        ledger::check_consistency(&w).expect("ledger consistent after bankruptcy");
    }
}

/// b4-asym M1 unit tests for the vacancy posting rule (`posted_vacancies`).
#[cfg(test)]
mod b4_asym_tests {
    use super::posted_vacancies;
    use crate::config::Config;

    fn cfg_with(share: f64, mode: u32, band: f64, replace: bool) -> Config {
        let mut c = Config::default();
        c.vacancy_net_share = share;
        c.vacancy_rounding = mode;
        c.vacancy_free_units = band;
        c.vacancy_replace = replace;
        c
    }

    fn draws() -> Vec<f64> {
        (0..100).map(|k| 0.005 + 0.01 * k as f64).collect()
    }

    fn gaps() -> Vec<f64> {
        (0..=60).map(|k| k as f64 * 0.17).collect()
    }

    // 1. s >= 1 gives ceil(G) for any mode, band or age.
    #[test]
    fn full_share_is_legacy_ceil_for_any_mode_band_and_age() {
        for mode in 0..3 {
            for band in [0.0, 1.0, 3.0] {
                for age in [0u32, 5, 100] {
                    let mut c = cfg_with(1.0, mode, band, true);
                    c.vacancy_young_exempt_months = 12;
                    for &g in &gaps() {
                        for &d in &draws()[..5] {
                            let (v, _) = posted_vacancies(&c, g, 40, age, d);
                            let want = if g > 1e-6 { g.ceil() as u32 } else { 0 };
                            assert_eq!(v, want, "mode {mode} band {band} age {age} gap {g}");
                        }
                    }
                }
            }
        }
    }

    // 2. Mode 0 with band 0 reproduces the Step 9 expression bit for bit.
    #[test]
    fn mode0_zero_band_equals_step9_formula() {
        for &replace in &[false, true] {
            for &share in &[0.3, 0.5, 0.999999] {
                let c = cfg_with(share, 0, 0.0, replace);
                for &n in &[0usize, 7, 90] {
                    for &g in &gaps() {
                        for &d in &draws() {
                            let repl = if replace { c.separation_rate * n as f64 } else { 0.0 };
                            let x = if g > 0.0 { repl.min(g) + (g - repl).max(0.0) * share } else { 0.0 };
                            let x = if x.is_finite() { x.max(0.0) } else { 0.0 };
                            let fl = x.floor();
                            let want = fl as u32 + u32::from(d < x - fl);
                            let (v, _) = posted_vacancies(&c, g, n, 50, d);
                            assert_eq!(v, want, "share {share} replace {replace} n {n} gap {g} draw {d}");
                        }
                    }
                }
            }
        }
    }

    // 3. Mode 1 (round up): inside the band it is ceil(G); positive throttled gaps post at least one; V <= ceil(G);
    //    V is monotone in G.
    #[test]
    fn round_up_mode_bounds_and_monotone() {
        let band = 2.0;
        let c = cfg_with(0.3, 1, band, false);
        let mut prev = 0u32;
        for &g in &gaps() {
            let (v, _) = posted_vacancies(&c, g, 10, 50, 0.5);
            if g <= band && g > 1e-6 {
                assert_eq!(v, g.ceil() as u32, "inside band gap {g}");
            }
            if g > 1e-6 {
                assert!(v >= 1, "positive gap {g} should post at least one");
            }
            assert!(v as f64 <= g.ceil() + 1e-9, "V {v} above ceil(G) for gap {g}");
            assert!(v >= prev, "V not monotone at gap {g}");
            prev = v;
        }
    }

    // 4. Mode 2 (hybrid): averaged over draws it equals ceil(min(G,B)) + s(G-B)+ within 0.01.
    #[test]
    fn hybrid_mode_mean_matches_expectation() {
        for &(band, share) in &[(1.0, 0.3), (2.0, 0.5), (0.0, 0.3)] {
            let c = cfg_with(share, 2, band, false);
            for &g in &gaps() {
                let mean: f64 = draws().iter().map(|&d| posted_vacancies(&c, g, 10, 50, d).0 as f64).sum::<f64>()
                    / draws().len() as f64;
                let want = if g > 1e-6 { g.min(band).ceil() + share * (g - band).max(0.0) } else { 0.0 };
                assert!((mean - want).abs() <= 0.01 + 1e-9, "band {band} share {share} gap {g}: mean {mean} want {want}");
            }
        }
    }

    // 5. Young-exempt firms take the legacy path regardless of share, mode or draw.
    #[test]
    fn young_firms_take_legacy_path() {
        let mut c = cfg_with(0.3, 0, 0.0, true);
        c.vacancy_young_exempt_months = 12;
        for &g in &gaps() {
            for &d in &draws()[..5] {
                let (v, _) = posted_vacancies(&c, g, 40, 3, d);
                assert_eq!(v, if g > 1e-6 { g.ceil() as u32 } else { 0 }, "young firm gap {g}");
            }
        }
        // Once the firm is past the exemption window the throttled path applies again.
        let (old, _) = posted_vacancies(&c, 5.0, 40, 12, 0.5);
        assert!(old <= 5, "throttled posting should not exceed the gap");
    }

    // 6. 0 <= deferred <= G for every mode.
    #[test]
    fn deferred_is_between_zero_and_gap() {
        for mode in 0..3 {
            for &band in &[0.0, 1.5] {
                let c = cfg_with(0.4, mode, band, true);
                for &g in &gaps() {
                    for &d in &draws()[..10] {
                        let (_, def) = posted_vacancies(&c, g, 30, 50, d);
                        assert!(def >= 0.0 && def <= g + 1e-12, "mode {mode} gap {g}: deferred {def}");
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod b5_firmhet_tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    #[test]
    fn calibrated_counts_match_the_init_rule() {
        // n = 20,000, 25 households per firm: 800 firms. Food share 0.07 gives 56, Nondurables 0.10 gives 80.
        assert_eq!(calibrated_firm_count(20_000, 25.0, 0.07), 56);
        assert_eq!(calibrated_firm_count(20_000, 25.0, 0.10), 80);
        // Floor of two firms.
        assert_eq!(calibrated_firm_count(20_000, 25.0, 0.001), 2);
    }

    #[test]
    fn labour_capital_reference_matches_hand_value() {
        // lp 2, 3 workers, cp 4, u_n 0.5: K_lab = 2 × 3 / (4 × 0.5) = 3.
        assert!((labour_capital_ref(2.0, 3.0, 4.0, 0.5) - 3.0).abs() < 1e-12);
        assert_eq!(labour_capital_ref(2.0, 3.0, 0.0, 0.5), 0.0);
    }

    #[test]
    fn entrant_capital_respects_the_worker_floor() {
        // Floor off: the legacy value comes back unchanged, bit for bit.
        assert_eq!(entrant_capital(0.1, 0.0, 1.0, 2.0, 0.85).to_bits(), 0.1f64.to_bits());
        // Floor 4 workers: K_lab = 1 × 4 / (2 × 0.85); the legacy 0.1 is lifted to it.
        let k = entrant_capital(0.1, 4.0, 1.0, 2.0, 0.85);
        assert!((k - 4.0 / 1.7).abs() < 1e-12, "entrant capital {k}");
        assert!(k >= 4.0 * 1.0 / (2.0 * 0.85) - 1e-12);
        // A legacy capital above the floor is kept.
        assert_eq!(entrant_capital(5.0, 4.0, 1.0, 2.0, 0.85), 5.0);
    }

    #[test]
    fn expansion_base_uses_the_labour_reference_only_when_on() {
        // K = 0 with workers: the labour reference gives a positive cap base; legacy gives zero.
        assert_eq!(expansion_cap_base(0.0, 3.0, false), 0.0);
        assert_eq!(expansion_cap_base(0.0, 3.0, true), 3.0);
        assert_eq!(expansion_cap_base(5.0, 3.0, true), 5.0);
        assert_eq!(expansion_cap_base(2.0, 3.0, false), 2.0);
    }

    #[test]
    fn capital_weighted_signal_matches_hand_computation() {
        let mut w = tiny_world(Config { n_households: 500, ..Default::default() }, 1);
        let food: Vec<usize> = (0..w.firms.len()).filter(|&f| w.firms[f].sector == Sector::Food).collect();
        for &f in &food {
            w.firms[f].alive = false;
        }
        // Two living Food firms (the tiny world has two Food slots): (capital, profit rate) = (1, 0.02), (3, 0.10).
        let rows = [(1.0, 0.02), (3.0, 0.10)];
        for (i, &(k, pr)) in rows.iter().enumerate() {
            let f = food[i];
            w.firms[f].alive = true;
            w.firms[f].capital = k;
            w.firms[f].profit_rate = pr;
        }
        let stats = sector_stats(&w);
        let st = &stats[Sector::Food.idx()];
        // Capital-weighted: (1 × 0.02 + 3 × 0.10) / (1 + 3) = 0.08.
        assert!((st.profit_rate_kw - 0.08).abs() < 1e-12, "weighted {}", st.profit_rate_kw);
        // Unweighted mean: (0.02 + 0.10) / 2 = 0.06, so the two signals differ.
        assert!((st.profit_rate - 0.06).abs() < 1e-12, "mean {}", st.profit_rate);
    }

    #[test]
    fn cyclical_ema_follows_the_entry_gate_signal_switch() {
        // The cyclical gate's EMAs take the same signal as the flat gate: capital-weighted when
        // `entry_gate_capital_weighted` is on, the legacy unweighted mean when it is off.
        let build = |capital_weighted: bool| {
            let cfg = Config { n_households: 500, entry_gate_capital_weighted: capital_weighted, ..Default::default() };
            let mut w = tiny_world(cfg, 1);
            let food: Vec<usize> = (0..w.firms.len()).filter(|&f| w.firms[f].sector == Sector::Food).collect();
            for &f in &food {
                w.firms[f].alive = false;
            }
            for (i, &(k, pr)) in [(1.0, 0.02), (3.0, 0.10)].iter().enumerate() {
                let f = food[i];
                w.firms[f].alive = true;
                w.firms[f].capital = k;
                w.firms[f].profit_rate = pr;
            }
            update_entry_ema(&mut w);
            w.firms_state.entry_ema[Sector::Food.idx()]
        };
        // First observation sets both EMAs to the signal: weighted 0.08, unweighted 0.06.
        let weighted = build(true);
        let unweighted = build(false);
        assert!((weighted[0] - 0.08).abs() < 1e-12 && (weighted[1] - 0.08).abs() < 1e-12, "weighted {weighted:?}");
        assert!((unweighted[0] - 0.06).abs() < 1e-12 && (unweighted[1] - 0.06).abs() < 1e-12, "unweighted {unweighted:?}");
    }
}

#[cfg(test)]
mod b5_size_markup_tests {
    use super::size_markup_adj;

    #[test]
    fn size_premium_is_monotone_in_relative_demand_and_compensation_is_capped() {
        // One sector: demands 1, 2, 4 (mean 7/3). Energy-like exempt firm (None) and a dead one must get 0.
        let group = [Some(0), Some(0), Some(0), None, None];
        let ed = [1.0, 2.0, 4.0, 100.0, 0.0];
        let (kappa, cmax) = (0.05, 0.02);
        let adj = size_markup_adj(&group, &ed, kappa, cmax, 1.0);
        assert!(adj[0] <= adj[1] && adj[1] <= adj[2], "premium must rise with demand: {adj:?}");
        assert_eq!(adj[3], 0.0);
        assert_eq!(adj[4], 0.0);
        // Compensation c_j = min(cmax, κ Σ ω max(0, ln q)) lies in [0, cmax]; the smallest firm (q < 1) has
        // adj = −c_j, which must be in [−cmax, 0].
        let c = -adj[0];
        assert!((0.0..=cmax + 1e-12).contains(&c), "compensation out of range: {c}");
        // Premium of the largest firm is κ·ln q (uncapped part) minus the same c.
        let lnq = (4.0f64 / (7.0 / 3.0)).ln();
        assert!((adj[2] - (kappa * lnq - c)).abs() < 1e-12);
    }

    #[test]
    fn zero_compensation_cap_leaves_pure_premium() {
        let group = [Some(3), Some(3)];
        let ed = [1.0, 3.0];
        let adj = size_markup_adj(&group, &ed, 0.08, 0.0, 1.0);
        assert_eq!(adj[0], 0.0);
        assert!(adj[1] > 0.0);
    }

    #[test]
    fn large_firm_threshold_gives_premium_only_above_q_min() {
        // Demands 1, 2, 4 in one sector (mean 7/3): q = 0.43, 0.86, 1.71, so none reaches q = 3 and the
        // premium and the compensation are zero.
        let group = [Some(0), Some(0), Some(0)];
        let ed = [1.0, 2.0, 4.0];
        let adj = size_markup_adj(&group, &ed, 0.08, 0.04, 3.0);
        assert!(adj.iter().all(|a| a.abs() < 1e-12), "{adj:?}");
        // With a 10-firm sector where one firm has q > 3, only that firm gets a premium.
        let mut g = vec![Some(2usize); 10];
        let mut e = vec![1.0f64; 10];
        e[0] = 40.0;
        let adj = size_markup_adj(&g, &e, 0.08, 0.04, 3.0);
        assert!(adj[0] > 0.0 && adj[1..].iter().all(|a| *a <= 0.0 + 1e-12 && *a >= -0.04 - 1e-12), "{adj:?}");
        g[0] = None;
        assert!(size_markup_adj(&g, &e, 0.08, 0.04, 3.0)[0] == 0.0);
    }

    #[test]
    fn equal_demand_gives_no_premium() {
        let group = [Some(1), Some(1), Some(1)];
        let adj = size_markup_adj(&group, &[2.0, 2.0, 2.0], 0.05, 0.04, 1.0);
        assert!(adj.iter().all(|a| a.abs() < 1e-12), "{adj:?}");
    }
}

#[cfg(test)]
mod b5_lumpy_tests {
    use super::{lumpy_gap_z, lumpy_inv_units};
    use crate::config::Config;

    #[test]
    fn firm_gap_is_centred_on_the_capital_weighted_sector_mean() {
        let group = [Some(0), Some(0), Some(0), Some(1), None];
        let k = [1.0, 3.0, 0.0, 2.0, 5.0];
        let ks = [2.0, 2.0, 2.0, 4.0, 4.0];
        let z = lumpy_gap_z(&group, &k, &ks);
        // Sector 0: the capital-weighted sum of z is zero (firm 2 has K = 0 and sits far below the mean).
        let wsum: f64 = (0..2).map(|f| k[f] * z[f]).sum::<f64>() + k[2] * z[2];
        assert!(wsum.abs() < 1e-9, "weighted z should sum to 0 within a sector: {wsum}");
        assert!(z[2] < z[0] && z[0] < z[1], "zero-capital firm is furthest below: {z:?}");
        // Sector 1 has one firm, so z = 0 there. Dead firms get NaN.
        assert!(z[3].abs() < 1e-12);
        assert!(z[4].is_nan());
    }

    #[test]
    fn lumpy_rule_inaction_catchup_and_legacy_bands() {
        let cfg = Config { inv_lumpy: true, inv_inaction_band: 0.40, inv_catchup_band: 0.25, ..Config::default() };
        let (k, ks, k_lab, dep) = (10.0, 12.0, 0.0, 0.01 / 12.0);
        // Above the inaction band: no investment at all.
        assert_eq!(lumpy_inv_units(0.5, &cfg, k, ks, k_lab, dep), Some(0.0));
        // Below the catch-up band: replacement plus the gap, capped at 5% of max(K, K_lab) per month.
        let inv = lumpy_inv_units(-0.5, &cfg, k, ks, k_lab, dep).unwrap();
        let gap = (ks - k) / 12.0;
        assert!((inv - (dep * k + gap.min(0.05 * k))).abs() < 1e-12, "inv {inv}");
        // Inside the bands: legacy.
        assert_eq!(lumpy_inv_units(0.0, &cfg, k, ks, k_lab, dep), None);
        assert_eq!(lumpy_inv_units(f64::NAN, &cfg, k, ks, k_lab, dep), None);
    }

    #[test]
    fn zero_capital_firm_with_desired_capital_orders_catchup_capital() {
        let cfg = Config { inv_lumpy: true, inv_inaction_band: 0.40, inv_catchup_band: 0.25, ..Config::default() };
        // K = 0 with workers: K_lab > 0 lets the cap bite, so a positive order is placed.
        let inv = lumpy_inv_units(-3.0, &cfg, 0.0, 5.0, 2.0, 0.001).unwrap();
        assert!(inv > 0.0 && inv <= 0.05 * 2.0 + 1e-12, "inv {inv}");
    }
}

#[cfg(test)]
mod cyclical_gate_tests {
    use super::{cyclical_entry_prob, ema_step};

    /// At zero gap, zero level term and no u-gap, the gate admits with the base probability p0.
    #[test]
    fn gate_at_zero_signal_is_base_probability() {
        for &p0 in &[0.15, 0.5, 0.8] {
            let p = cyclical_entry_prob(p0, 300.0, 0.05, 0.05, 0.0, 0.03, 0.0, 0.0);
            assert!((p - p0).abs() < 1e-12, "p0 {p0} -> {p}");
        }
    }

    /// The gap term is monotone: a fast EMA above the slow one raises the admission probability, below it lowers it.
    #[test]
    fn gap_term_is_monotone_and_signed() {
        let base = cyclical_entry_prob(0.5, 300.0, 0.05, 0.05, 0.0, 0.03, 0.0, 0.0);
        let up = cyclical_entry_prob(0.5, 300.0, 0.06, 0.05, 0.0, 0.03, 0.0, 0.0);
        let down = cyclical_entry_prob(0.5, 300.0, 0.04, 0.05, 0.0, 0.03, 0.0, 0.0);
        assert!(up > base && base > down, "{down} {base} {up}");
        // With β = 300 a 1-point gap moves the logit by 3, so the probability is logistic(3) at p0 = 0.5.
        assert!((up - 1.0 / (1.0 + (-3.0f64).exp())).abs() < 1e-12, "up {up}");
    }

    /// The level term shifts the logit by γ (fast − hurdle): a sector above the hurdle is admitted more often.
    #[test]
    fn level_term_shifts_logit_by_gamma_times_excess_over_hurdle() {
        let p = cyclical_entry_prob(0.5, 0.0, 0.08, 0.08, 40.0, 0.03, 0.0, 0.0);
        let want = 1.0 / (1.0 + (-(40.0 * 0.05f64)).exp());
        assert!((p - want).abs() < 1e-12, "{p} vs {want}");
        let below = cyclical_entry_prob(0.5, 0.0, 0.0, 0.0, 40.0, 0.03, 0.0, 0.0);
        assert!(below < 0.5 && p > 0.5);
    }

    /// The aggregate fallback adds β_u · 100 · (u* − u) to the logit: unemployment above u* closes the gate.
    #[test]
    fn u_gap_term_closes_gate_in_slumps() {
        // u* − u = −0.01 (unemployment 1 point above natural) with β_u = 0.5 gives a logit shift of −0.5.
        let slump = cyclical_entry_prob(0.5, 0.0, 0.0, 0.0, 0.0, 0.03, 0.5, -0.01);
        let want = 1.0 / (1.0 + (0.5f64).exp());
        assert!((slump - want).abs() < 1e-12, "{slump} vs {want}");
        let boom = cyclical_entry_prob(0.5, 0.0, 0.0, 0.0, 0.0, 0.03, 0.5, 0.01);
        assert!(boom > 0.5 && slump < 0.5);
    }

    /// EMA update: the first observation sets both EMAs to x; afterwards fast moves 1/3 of the way and slow moves
    /// `slow_rate` of the way, and the two are not interchanged.
    #[test]
    fn ema_first_observation_then_update() {
        let first = ema_step([0.0, 0.0], false, 0.10, 0.0115);
        assert_eq!(first, [0.10, 0.10]);
        let next = ema_step(first, true, 0.22, 0.0115);
        assert!((next[0] - (0.10 + (0.22 - 0.10) / 3.0)).abs() < 1e-15, "fast {}", next[0]);
        assert!((next[1] - (0.10 + 0.0115 * 0.12)).abs() < 1e-15, "slow {}", next[1]);
        // Slow EMA moves far less than fast EMA on the same shock.
        assert!((next[1] - 0.10) < (next[0] - 0.10));
    }
}

// b7-transmission M-Es / M-E tests.
#[cfg(test)]
mod policy_expectation_tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    /// Both weights at zero: the firms' expectation is the base expectation exactly.
    #[test]
    fn zero_weights_give_the_base_expectation_exactly() {
        let mut w = tiny_world(Config { n_households: 500, ..Default::default() }, 7);
        w.shocks.policy_shift = Some((0.01, 0, 0.9));
        w.monetary.firm_stance = 0.02;
        assert_eq!(firm_expected_inflation(&w), firm_expected_inflation_base(&w));
    }

    // rz2: `firm_pump_user_cost_weight` leaves the energy-salience part of household expected inflation out of the user cost.
    /// At weight 1 (the off value) the user-cost expectation is the household mean bit for bit, whatever the state holds.
    #[test]
    fn firm_pump_weight_one_is_the_household_mean() {
        let mut w = tiny_world(Config { n_households: 300, ..Default::default() }, 3);
        w.consumption.pump_comp = 0.01;
        assert_eq!(w.cfg.firm_pump_user_cost_weight, 1.0);
        let hh = mean_expected_inflation(&w);
        assert_eq!(capital_expectation(&mut w, 0.031).to_bits(), hh.to_bits());
    }

    /// At weight 0 the pump component is taken out; weight 1/2 takes half; modes 1-3 ignore the weight.
    #[test]
    fn firm_pump_weight_removes_the_pump_component_from_the_user_cost() {
        let mut w = tiny_world(Config { n_households: 300, firm_pump_user_cost_weight: 0.0, ..Default::default() }, 3);
        w.consumption.pump_comp = 0.01;
        let hh = mean_expected_inflation(&w);
        assert!((capital_expectation(&mut w, 0.031) - (hh - 0.01)).abs() < 1e-15);
        w.cfg.firm_pump_user_cost_weight = 0.5;
        assert!((capital_expectation(&mut w, 0.031) - (hh - 0.005)).abs() < 1e-15);
        w.cfg.user_cost_pi_e_mode = 1;
        assert_eq!(capital_expectation(&mut w, 0.031).to_bits(), 0.031f64.to_bits());
        w.cfg.user_cost_pi_e_mode = 2;
        assert!((capital_expectation(&mut w, 0.031) - (w.monetary.pi_lr.unwrap_or(w.cfg.inflation_target) + w.cfg.user_cost_pi_e_offset)).abs() < 1e-15);
    }

    /// A positive policy gap or a positive stance lowers the expectation, each by at most its χ·L bound.
    #[test]
    fn positive_gap_and_stance_lower_expectations_within_bounds() {
        let cfg = Config {
            n_households: 500,
            firm_mp_expect: 0.5,
            firm_mp_expect_cap: 0.03,
            firm_stance_weight: 0.5,
            firm_stance_cap: 0.01,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 7);
        let base = firm_expected_inflation_base(&w);
        w.shocks.policy_shift = Some((0.01, 0, 0.9));
        w.monetary.firm_stance = 0.0;
        let by_gap = firm_expected_inflation(&w);
        assert!(by_gap < base, "gap should lower expectations");
        assert!(base - by_gap <= 0.5 * 0.03 + 1e-12);
        w.shocks.policy_shift = None;
        w.monetary.firm_stance = 0.02;
        let by_stance = firm_expected_inflation(&w);
        assert!(by_stance < base, "positive stance should lower expectations");
        assert!(base - by_stance <= 0.5 * 0.01 + 1e-12);
    }

    /// The term is odd in its signal and bounded by |χ|·L.
    #[test]
    fn policy_expectation_term_is_odd_and_bounded() {
        let (chi, cap) = (0.5, 0.03);
        for x in [1e-4, 0.01, 0.2, 5.0] {
            let p = policy_expectation_term(chi, cap, x);
            let m = policy_expectation_term(chi, cap, -x);
            assert!((p + m).abs() < 1e-15);
            assert!(p > 0.0 && p <= chi * cap + 1e-15);
        }
        assert_eq!(policy_expectation_term(chi, 0.0, 0.01), 0.0);
        assert_eq!(policy_expectation_term(chi, cap, f64::NAN), 0.0);
    }

    /// b9-oil test helper: set every alive household's expected inflation.
    fn set_alive_expectation(w: &mut World, v: f64) {
        for h in w.households.iter_mut().filter(|h| h.alive) {
            h.expected_inflation = v;
        }
    }

    /// b9-oil test 1: mode 0 is the legacy statement, bit for bit, and leaves the mode-3 state untouched.
    #[test]
    fn user_cost_mode_zero_is_legacy() {
        let cfg = Config { n_households: 300, user_cost_household_expectation: true, ..Default::default() };
        let mut w = tiny_world(cfg, 5);
        assert_eq!(w.cfg.user_cost_pi_e_mode, 0);
        let hh = mean_expected_inflation(&w);
        assert_eq!(capital_expectation(&mut w, 0.031).to_bits(), hh.to_bits());
        assert!(!w.firms_state.pi_e_capital_init);
        w.cfg.user_cost_household_expectation = false;
        assert_eq!(capital_expectation(&mut w, 0.031).to_bits(), 0.031f64.to_bits());
        assert!(!w.firms_state.pi_e_capital_init);
    }

    /// b9-oil test 2: mode 1 returns the firm expectation exactly, whatever the household bool.
    #[test]
    fn user_cost_mode_one_is_firm_expectation() {
        for on in [true, false] {
            let cfg = Config { n_households: 300, user_cost_household_expectation: on, user_cost_pi_e_mode: 1, ..Default::default() };
            let mut w = tiny_world(cfg, 6);
            assert_eq!(capital_expectation(&mut w, 0.0275).to_bits(), 0.0275f64.to_bits());
        }
    }

    /// b9-oil test 3: mode 2 is pi^LR plus the offset, or the target plus the offset when pi^LR is unset.
    #[test]
    fn user_cost_mode_two_uses_pi_lr_plus_offset() {
        let cfg = Config { n_households: 300, user_cost_pi_e_mode: 2, user_cost_pi_e_offset: 0.003, ..Default::default() };
        let target = cfg.inflation_target;
        let mut w = tiny_world(cfg, 7);
        w.monetary.pi_lr = Some(0.021);
        assert!((capital_expectation(&mut w, 0.9) - 0.024).abs() < 1e-15);
        w.monetary.pi_lr = None;
        assert!((capital_expectation(&mut w, 0.9) - (target + 0.003)).abs() < 1e-15);
    }

    /// b9-oil test 4: mode 3 is a slow EWMA of the household mean, with gain 1/(1+months).
    #[test]
    fn user_cost_mode_three_is_a_slow_ewma() {
        let cfg = Config { n_households: 300, user_cost_pi_e_mode: 3, user_cost_pi_e_months: 24.0, ..Default::default() };
        let mut w = tiny_world(cfg, 8);
        set_alive_expectation(&mut w, 0.02);
        let first = capital_expectation(&mut w, 0.0);
        assert!((first - 0.02).abs() < 1e-12, "first {first}");
        assert!(w.firms_state.pi_e_capital_init);
        set_alive_expectation(&mut w, 0.03);
        let second = capital_expectation(&mut w, 0.0);
        assert!((second - (0.02 + 0.01 / 25.0)).abs() < 1e-12, "second {second}");
        for _ in 0..400 {
            capital_expectation(&mut w, 0.0);
        }
        assert!((w.firms_state.pi_e_capital_slow - 0.03).abs() < 1e-3, "converged {}", w.firms_state.pi_e_capital_slow);
        // Memory 0: the gain is 1, so the value follows the household mean.
        w.cfg.user_cost_pi_e_months = 0.0;
        set_alive_expectation(&mut w, 0.05);
        let hh = mean_expected_inflation(&w);
        let v = capital_expectation(&mut w, 0.0);
        assert!((v - hh).abs() < 1e-15, "memory 0 gave {v}, mean {hh}");
    }

    /// b9-oil test 5: a 12-month rise of the household mean by 0.005, then a return, moves mode 3 by less than 0.002.
    #[test]
    fn user_cost_mode_three_damps_a_transient() {
        let cfg = Config { n_households: 300, user_cost_pi_e_mode: 3, user_cost_pi_e_months: 24.0, ..Default::default() };
        let mut w = tiny_world(cfg, 9);
        set_alive_expectation(&mut w, 0.02);
        capital_expectation(&mut w, 0.0);
        let base = w.firms_state.pi_e_capital_slow;
        let mut peak_dev: f64 = 0.0;
        for _ in 0..12 {
            set_alive_expectation(&mut w, 0.025);
            let v = capital_expectation(&mut w, 0.0);
            peak_dev = peak_dev.max((v - base).abs());
        }
        for _ in 0..24 {
            set_alive_expectation(&mut w, 0.02);
            let v = capital_expectation(&mut w, 0.0);
            peak_dev = peak_dev.max((v - base).abs());
        }
        assert!(peak_dev < 0.002, "peak deviation {peak_dev}");
        assert!(peak_dev > 0.001, "test lacks power: peak deviation {peak_dev}");
    }

    /// b9-oil test 6: the labour profile (share 0.10, elasticity 1.0) gives factor 1 at oil ratio 1, 1.05 at 1.5, and
    /// exactly 1 outside Energy.
    #[test]
    fn extraction_profile_values() {
        let cfg = Config { energy_extraction_labor_share: 0.10, energy_extraction_elasticity: 1.0, ..Default::default() };
        assert_eq!(extraction_factor(&cfg, Sector::Energy, 1.0).to_bits(), 1.0f64.to_bits());
        assert!((extraction_factor(&cfg, Sector::Energy, 1.5) - 1.05).abs() < 1e-12);
        for s in Sector::ALL {
            if s != Sector::Energy {
                assert_eq!(extraction_factor(&cfg, s, 1.5).to_bits(), 1.0f64.to_bits());
            }
        }
    }

    /// b9-oil test 7: two identical worlds in mode 3 agree after 24 steps.
    #[test]
    fn user_cost_modes_are_deterministic() {
        let run = || {
            let cfg = Config { n_households: 300, user_cost_pi_e_mode: 3, user_cost_pi_e_months: 24.0, ..Default::default() };
            let mut w = tiny_world(cfg, 10);
            for _ in 0..24 {
                crate::sim::step(&mut w);
            }
            w.firms_state.pi_e_capital_slow
        };
        assert_eq!(run().to_bits(), run().to_bits());
    }

    /// b9-inflation M-D: the one-month step by hand (decay 0.02, κ 0.04, bound 0.05).
    #[test]
    fn demand_dev_step_hand_values() {
        let first = demand_dev_step(0.0, -0.01, 0.0, 0.04, 0.02, 0.05);
        assert!((first + 0.0004).abs() < 1e-12, "first {first}");
        let second = demand_dev_step(first, -0.01, 0.0, 0.04, 0.02, 0.05);
        assert!((second + 0.000792).abs() < 1e-12, "second {second}");
        // A gap equal to the norm adds nothing: the state only decays.
        let decay = demand_dev_step(-0.02, -0.01, -0.01, 0.04, 0.02, 0.05);
        assert!((decay - 0.98 * -0.02).abs() < 1e-15, "decay {decay}");
        // A persistent gap saturates at the bound and stays there.
        let mut d = 0.0;
        for _ in 0..200 {
            d = demand_dev_step(d, -0.1, 0.0, 0.04, 0.02, 0.05);
        }
        assert_eq!(d, -0.05);
        assert_eq!(demand_dev_step(0.0, f64::NAN, 0.0, 0.04, 0.02, 0.05), 0.0);
    }

    /// b9-inflation M-D: with the key off the state never moves and the norm is never set.
    #[test]
    fn demand_dev_off_stays_zero() {
        let mut w = tiny_world(Config { n_households: 60, ..Default::default() }, 43);
        for _ in 0..24 {
            crate::sim::step(&mut w);
        }
        assert_eq!(w.firms_state.demand_dev.to_bits(), 0.0f64.to_bits());
        assert_eq!(w.firms_state.demand_gap_norm.to_bits(), 0.0f64.to_bits());
        assert!(!w.firms_state.demand_gap_init);
    }

    /// b9-inflation M-D: the norm is a slow EWMA of the Okun gap (gain 1/(1 + M)) that starts from the config value.
    #[test]
    fn demand_gap_norm_is_a_slow_ewma() {
        let cfg = Config {
            n_households: 60,
            markup_dev_pull: 0.04,
            markup_dev_gap_ref: -0.0098,
            markup_dev_norm_months: 120.0,
            ..Default::default()
        };
        let nat = cfg.natural_unemployment;
        let mut w = tiny_world(cfg, 44);
        // With no history, unemployment_rate() is the natural rate; u* = nat − 0.01 fixes the Okun gap at −0.02.
        w.monetary.u_star = nat - 0.01;
        let gap = (OKUN_COEFFICIENT * (w.natural_unemployment() - w.unemployment_rate())).clamp(-0.1, 0.1);
        assert!((gap + 0.02).abs() < 1e-12, "gap {gap}");
        update_demand_dev(&mut w);
        let expected = -0.0098 + (gap + 0.0098) / 121.0;
        assert!((w.firms_state.demand_gap_norm - expected).abs() < 1e-15, "norm {}", w.firms_state.demand_gap_norm);
        for _ in 0..600 {
            update_demand_dev(&mut w);
        }
        assert!((w.firms_state.demand_gap_norm + 0.02).abs() < 5e-4, "norm {}", w.firms_state.demand_gap_norm);
    }

    /// b9-inflation M-D: a lower demand-induced deviation lowers the markups of live non-Energy firms.
    #[test]
    fn demand_dev_lowers_markups() {
        let cfg = Config { n_households: 60, ..Default::default() };
        let mut a = tiny_world(cfg.clone(), 45);
        let mut b = tiny_world(cfg, 45);
        a.firms_state.demand_dev = -0.03;
        for _ in 0..6 {
            crate::sim::step(&mut a);
            crate::sim::step(&mut b);
        }
        let mean = |w: &World| {
            let v: Vec<f64> =
                w.firms.iter().filter(|f| f.alive && f.sector != Sector::Energy).map(|f| f.markup).collect();
            v.iter().sum::<f64>() / v.len() as f64
        };
        assert!(mean(&b) - mean(&a) > 0.005, "A {} B {}", mean(&a), mean(&b));
    }

    /// b9-inflation M-D: with the mechanism on, the state is a deterministic function of the run, and it moves.
    #[test]
    fn demand_dev_is_deterministic() {
        let cfg = Config { n_households: 60, markup_dev_pull: 0.16, ..Default::default() };
        let run = || {
            let mut w = tiny_world(cfg.clone(), 46);
            for _ in 0..24 {
                crate::sim::step(&mut w);
            }
            (w.firms_state.demand_dev, w.firms_state.demand_gap_norm)
        };
        let (d1, n1) = run();
        let (d2, n2) = run();
        assert_eq!(d1.to_bits(), d2.to_bits());
        assert_eq!(n1.to_bits(), n2.to_bits());
        assert!(d1 != 0.0 && n1 != 0.0, "state did not move: {d1} {n1}");
    }
}
