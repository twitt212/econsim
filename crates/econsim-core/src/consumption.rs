//! Household consumption & saving (docs/DESIGN.md §8.1–8.2) and Fund portfolio rebalancing.
//!
//! `decide` computes every household's budget in parallel from an immutable snapshot of the world, writes the
//! results back per household, and then applies Fund purchases and sales sequentially through `ledger`.
//! Capital income (interest, dividends, rent) is paid after this module runs, so the budget uses last month's
//! amounts recorded by `record_capital_income`, which the orchestrator must call at the end of each tick.

use rayon::prelude::*;
use std::array;

use crate::common;
use crate::config::Config;
use crate::types::*;

/// Months of energy-price history kept (13 gives one year-on-year change).
const ENERGY_HIST_LEN: usize = 13;
/// Adaptation speed of expected inflation towards its anchor (DESIGN §8.1, Hommes-style).
const EXP_INFL_ADAPT: f64 = 0.1;
/// Sensitivity of the income MPC to the real rate, and the neutral real rate around which it is centred.
const NEUTRAL_REAL_RATE: f64 = 0.01;

/// Households per fixed chunk in Fund rebalancing (fixed, so results are thread-count independent).
const FUND_CHUNK: usize = 2048;
/// Landlords' operating costs as a share of rent received.
const LANDLORD_OPEX_SHARE: f64 = 0.35;
/// Stream for the persistent impatience type (fix-households). Drawn at tick 0 so each household keeps its type.
const STREAM_HH_TYPE: u64 = crate::rng::stream::HH_TYPE;

/// Target share of household financial wealth held as deposits — set to the calibrated initial composition (the
/// model's house-price/mortgage scale relative to survey-scale income makes deposits ≈ 40% of financial assets).
const LIQUID_SHARE: f64 = 0.40;

/// Consumption is concave in wealth: the very rich consume a small share of marginal wealth (bequest motives,
/// De Nardi 2004; Carroll 2000). Wealth above WEALTH_KINK counts at WEALTH_TAIL_WEIGHT.
///
const WEALTH_KINK: f64 = 1.0e6;
const WEALTH_TAIL_WEIGHT: f64 = 0.35;
/// Monthly capital income above this counts at WEALTH_TAIL_WEIGHT for consumption.
const CAPITAL_INCOME_KINK: f64 = 20_000.0;
/// Wage index (per 100 at build time) at reported month 0 under the default 144-month burn-in, about 1.60: the
/// wage-indexed kinks (`Config::distquick_kink_wage_indexed`, plan item 14) are scaled by wage_index / 100 / this, so
/// they equal the base $1M and $20k at reported month 0 and rise with wages thereafter.
const KINK_WAGE_INDEX_AT_START: f64 = 1.60;

/// Kink scale for the consumption kinks: 1 (the base nominal kinks) unless `distquick_kink_wage_indexed`, in which
/// case the kinks follow the wage index (plan item 14). `wage_index` is the model's nominal wage index per 100.
#[inline]
fn kink_scale(cfg: &Config, wage_index: f64) -> f64 {
    if cfg.distquick_kink_wage_indexed {
        (wage_index / 100.0).max(1e-9) / KINK_WAGE_INDEX_AT_START
    } else {
        1.0
    }
}

/// `scale` multiplies the kink ($1M for wealth, $20k a month for capital income); 1 = base.
#[inline]
fn concave_wealth(x: f64, scale: f64) -> f64 {
    let kink = WEALTH_KINK * scale;
    if x <= kink {
        x
    } else {
        kink + WEALTH_TAIL_WEIGHT * (x - kink)
    }
}

#[inline]
fn concave_capital_income(x: f64, scale: f64) -> f64 {
    let kink = CAPITAL_INCOME_KINK * scale;
    if x <= kink {
        x
    } else {
        kink + WEALTH_TAIL_WEIGHT * (x - kink)
    }
}

/// Marginal propensity to consume out of (after-tax) capital income: Di Maggio, Kermani & Majlesi (2020) ≈0.43 out of
/// dividends; Baker, Nagel & Wurgler (2007) ≈0.5 or less. 0.75 made higher rates on a large public debt an income
/// transfer that offset tightening (10-year yield ≈4.9%); at 0.4 the 10-year averages ≈4.3%.
const MPC_CAPITAL_INCOME: f64 = 0.4;

/// Weight on the energy-price salience term in expected inflation. Calibrated so a sustained 50% yoy rise in
/// energy prices lifts household expectations by ≈ 1pp in steady state (Coibion & Gorodnichenko 2015).
const EXP_INFL_SALIENCE: f64 = 0.03;
const EXP_INFL_MIN: f64 = -0.05;
const EXP_INFL_MAX: f64 = 0.20;
/// Share of a household's excess deposits moved into Fund shares each month.
const FUND_BUY_RATE: f64 = 0.02;
/// Life table computed for integer ages 0..=LIFE_TABLE_MAX_AGE.
const LIFE_TABLE_MAX_AGE: usize = 120;

/// Consumption-module state. Per-household memory is indexed by household id and resized lazily.
#[derive(Clone, Debug, Default)]
pub struct ConsumptionState {
    /// Interest + dividends + business income + rent received last month, per household (see `record_capital_income`).
    pub capital_income_prev: Vec<f64>,
    /// Taxable dividends + business income received last month, per household. These are paid after the month's tax
    /// step (markets and firm books close later), so fiscal taxes them a month in arrears (like estimated payments).
    /// Dividends count at half weight (qualified-dividend rates of 0/15/20% vs ordinary rates) and business income at
    /// 80% (the 20% qualified-business-income deduction).
    pub distributions_prev: Vec<f64>,
    /// Energy-sector price level, oldest first, at most 13 months.
    pub energy_price_hist: Vec<f64>,
    /// Remaining life expectancy (years) by integer age, computed once.
    life_table: Vec<f64>,
    /// Fund purchases (+) and sales (−) decided this month, applied sequentially.
    rebalance: Vec<f64>,
    /// Households' perceived 2-year rate (smoothed with `rate_perception_months`; None until first set).
    yield24_perceived: Option<f64>,
    /// rz1 F1: households' perceived deposit rate (decimal; the same exponential smoother and lag as `yield24_perceived`;
    /// None until first set, and unused while `rate_perception_deposit` is off).
    deposit_perceived: Option<f64>,
    /// rz12: stages of the households' rate-perception cascades (2-year leg, deposit leg) when `rate_perception_stages` > 1.
    y24_stages: [Option<f64>; 4],
    dep_stages: [Option<f64>; 4],
    /// rz2: the energy-salience part of household expected inflation, as an economy-level state: z' = (1 - EXP_INFL_ADAPT) z + term,
    /// so that expected inflation = (anchored part) + z. Updated only when `hh_pump_real_rate_weight` or `firm_pump_user_cost_weight`
    /// is below 1; 0 otherwise.
    pub(crate) pump_comp: f64,
    /// Mean discretionary multiplier D over alive households in the latest month (1 = no effect).
    pub discretionary_mult_mean: f64,
    // fix-households: unspent one-off payments per household, run down at `windfall_decay` a month.
    pub windfall_stock: Vec<f64>,
    // fix-households: the household is in an unemployment spell that has received UI (cleared when employed or on a
    // new occupant). Only such spells get the exhaustion rules; other unemployed spells keep the patient rules.
    pub ui_spell: Vec<bool>,
    // fix-households: persistent impatience type per household slot. `type_u` is a uniform draw keyed on the seed in
    // force at the first month (before the burn-in re-seed in init::build_world), the slot and `type_n` (the redraw
    // count). A new occupant of a slot (age falls, or the slot was seen dead) redraws. `impatient` is `type_u < share`.
    pub type_u: Vec<f64>,
    pub type_n: Vec<u32>,
    pub type_age: Vec<u32>,
    pub type_seed: Option<u64>,
    pub impatient: Vec<bool>,
    // fix-households: smoothed log price and reference log price of Energy and Food (`necessity_multipliers`).
    pub necessity_ln_avg: [f64; 2],
    pub necessity_ln_ref: [f64; 2],
    pub necessity_init: bool,
    // fix-households: posted (planned) mean price of each consumer sector last month. Energy and Food move to this
    // month's posted price, since firms reprice in `firms::plan` before `decide` and `goods::clear` updates the sector
    // price only after sales.
    pub posted_prev: [f64; N_CONSUMER],
}

#[inline]
fn finite_or(x: f64, default: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        default
    }
}

/// Read-only inputs shared by every household in one month.
struct Env<'a> {
    w: &'a World,
    target: f64,
    pi_yoy: f64,
    eyoy: f64,
    /// Consumer-sector prices (P_s), positive.
    price: [f64; N_CONSUMER],
    /// Income-tax shock in force this month.
    tax_add: f64,
    /// Real-quantity multiplier on subsistence by sector (1 except Energy and Food; `necessity_multipliers`).
    necessity: [f64; N_CONSUMER],
    /// Nominal multiplier on supernumerary spending by sector: (price ÷ reference price) × real multiplier for Energy
    /// and Food, so that their real supernumerary spending follows the same low elasticity (1 elsewhere).
    necessity_extra: [f64; N_CONSUMER],
    /// 2-year market yield (`curve.yield_at(24)`), identical for every household this month.
    yield24: f64,
    /// rz1 F1: perceived minus current mean deposit rate (decimal), added to each household's own bank deposit rate in the
    /// real rate. Exactly 0.0 (and then skipped) unless `rate_perception_deposit` is on and `rate_perception_months` > 0.
    dep_shift: f64,
    /// b9-inflation M-H: this month's household policy term (decimal), χ_h·L_h·tanh(gap_t / L_h). Zero unless
    /// `hh_mp_expect` is non-zero and a PolicyShift gap is in force; subtracted from each household's expected inflation.
    hh_term: f64,
    /// rz2: (1 - hh_pump_real_rate_weight) x the energy-salience part of expected inflation this month; added back to the real rate
    /// (the real rate uses expected inflation less this part). Exactly 0 when the key is at its off value 1.
    pump_shift: f64,
    life: &'a [f64],
}

/// Record each household's capital income for this month (interest + dividends + business income + rent received).
/// `decide` uses these values next month, because capital income is paid after consumption in the tick.
/// The orchestrator must call this at the end of each tick (after `monetary::markets`).
pub fn record_capital_income(w: &mut World) {
    w.hh_month
        .par_iter()
        .map(|m| finite_or(0.5 * m.income.dividends.max(0.0) + 0.8 * m.income.business.max(0.0), 0.0))
        .collect_into_vec(&mut w.consumption.distributions_prev);
    let v = &mut w.consumption.capital_income_prev;
    // Element-wise and order-preserving (collect_into_vec), so the parallel map yields the same vector.
    w.hh_month
        .par_iter()
        .map(|m| {
            let x = m.income.interest + m.income.dividends + m.income.business + m.income.rent_received;
            finite_or(x, 0.0)
        })
        .collect_into_vec(v);
}

/// Set each household's consumption budget and sector split into `w.orders.household`,
/// update expectations and permanent income, and rebalance deposits vs Fund shares.
///
/// Budgets are gross of sales tax; `goods::clear` remits the tax.
pub fn decide(w: &mut World) {
    let n = w.households.len();
    let target = finite_or(w.cfg.inflation_target, 0.02);
    let mut cs = std::mem::take(&mut w.consumption);

    // energy price history and its year-on-year change (salience term, §8.1)
    cs.energy_price_hist
        .push(finite_or(common::sector_price(w, Sector::Energy), 1.0));
    if cs.energy_price_hist.len() > ENERGY_HIST_LEN {
        let excess = cs.energy_price_hist.len() - ENERGY_HIST_LEN;
        cs.energy_price_hist.drain(0..excess);
    }
    let eyoy = match cs.energy_price_hist.as_slice() {
        [first, .., last] if cs.energy_price_hist.len() == ENERGY_HIST_LEN && *first > 0.0 => {
            finite_or(last / first - 1.0, 0.0)
        }
        _ => 0.0,
    };
    let pi_yoy = finite_or(w.inflation_yoy(), 0.0);

    if cs.life_table.is_empty() {
        cs.life_table = (0..=LIFE_TABLE_MAX_AGE)
            .map(|a| common::life_expectancy(a as f64))
            .collect();
    }
    cs.capital_income_prev.resize(n, 0.0);
    cs.rebalance.clear();
    cs.rebalance.resize(n, 0.0);
    cs.windfall_stock.resize(n, 0.0);
    cs.ui_spell.resize(n, false);
    let seed = *cs.type_seed.get_or_insert(w.seed);
    assign_types(&w.cfg, seed, &w.households, &mut cs);

    // fix-households: Energy and Food are priced at this month's posted price: last month's sector price, moved by the
    // change in the posted mean price since last month (firms have already repriced in `firms::plan`).
    let posted = posted_sector_prices(w);
    let price_now: [f64; N_CONSUMER] = array::from_fn(|s| {
        let p = common::sector_price(w, Sector::from_idx(s));
        if is_necessity(s) && posted[s].is_finite() && cs.posted_prev[s] > 0.0 {
            p * posted[s] / cs.posted_prev[s]
        } else {
            p
        }
    });
    for s in 0..N_CONSUMER {
        if posted[s].is_finite() {
            cs.posted_prev[s] = posted[s];
        }
    }
    let cpi = finite_or(w.prices.cpi, 100.0);
    let (necessity, necessity_extra) = necessity_multipliers(&w.cfg, &price_now, cpi, &mut cs);

    let mut hhs = std::mem::take(&mut w.households);
    let mut orders = std::mem::take(&mut w.orders.household);
    orders.resize(n, [0.0; N_CONSUMER]);
    // Budgets and Fund trades (+ purchase, - sale), per household in fixed chunks. Each household's trade uses its
    // own deposits and shares only; the bank side (deposits and reserves move together) and the Fund side are
    // reduced per chunk in chunk order and applied below.
    let nb = w.banks.len();
    let fund_bank = w.fund.bank as usize;
    let parts: Vec<(Vec<f64>, f64, f64, f64, usize)> = {
        let wr: &World = w;
        let y_now = wr.curve.yield_at(24.0);
        let lag = wr.cfg.rate_perception_months.max(0.0);
        // rz12: with `rate_perception_stages` > 1 the perception is a cascade of smoothers with the same mean delay (one stage: legacy).
        let stages = wr.cfg.rate_perception_stages as usize;
        let y24p = if stages > 1 {
            common::lag_cascade(&mut cs.y24_stages, stages, lag, y_now)
        } else {
            match cs.yield24_perceived {
                Some(p) if lag > 0.0 && p.is_finite() => p + (y_now - p) / (1.0 + lag),
                _ => y_now,
            }
        };
        cs.yield24_perceived = Some(y24p);
        // rz1 F1: the deposit-rate leg through the same smoother (off: shift 0, the legacy path).
        let dep_shift = if wr.cfg.rate_perception_deposit && lag > 0.0 {
            let dep_now = mean_deposit_rate(wr);
            let dep_p = if stages > 1 {
                common::lag_cascade(&mut cs.dep_stages, stages, lag, dep_now)
            } else {
                match cs.deposit_perceived {
                    Some(p) if p.is_finite() => p + (dep_now - p) / (1.0 + lag),
                    _ => dep_now,
                }
            };
            cs.deposit_perceived = Some(dep_p);
            dep_p - dep_now
        } else {
            0.0
        };
        // rz2: the energy-salience term of expected inflation (the same expression `decide_one` adds), kept as a state z so the
        // real rate can leave it out. Not touched at the off value (weight 1), so that path is the legacy one bit for bit.
        let pump_shift = {
            let wgt = wr.cfg.hh_pump_real_rate_weight;
            // the state is also what the firms' user cost reads (`firm_pump_user_cost_weight`), so it is kept when either weight is below 1
            if wgt < 1.0 || wr.cfg.firm_pump_user_cost_weight < 1.0 {
                let term = if wr.cfg.pump_expectations {
                    wr.cfg.pump_salience * common::pump_yoy(wr)
                } else {
                    EXP_INFL_SALIENCE * eyoy * wr.cfg.cpi_weights[Sector::Energy.idx()]
                };
                cs.pump_comp = (1.0 - EXP_INFL_ADAPT) * cs.pump_comp + finite_or(term, 0.0);
                (1.0 - wgt) * cs.pump_comp
            } else {
                0.0
            }
        };
        let env = Env {
            w: wr,
            target,
            pi_yoy,
            eyoy,
            price: price_now,
            tax_add: wr.shocks.income_tax_add(wr.tick) + crate::fiscal::tax_rule_add(wr),
            necessity,
            necessity_extra,
            yield24: y24p,
            dep_shift,
            hh_term: if wr.cfg.hh_mp_expect != 0.0 {
                crate::firms::policy_expectation_term(wr.cfg.hh_mp_expect, wr.cfg.hh_mp_expect_cap, wr.shocks.policy_gap(wr.tick))
            } else {
                0.0
            },
            pump_shift,
            life: &cs.life_table,
        };
        let price = wr.fund.share_price;
        hhs.par_chunks_mut(FUND_CHUNK)
            .zip(orders.par_chunks_mut(FUND_CHUNK))
            .zip(cs.rebalance.par_chunks_mut(FUND_CHUNK))
            .zip(cs.windfall_stock.par_chunks_mut(FUND_CHUNK))
            .zip(cs.capital_income_prev.par_chunks(FUND_CHUNK))
            .zip(wr.hh_month.par_chunks(FUND_CHUNK))
            .zip(cs.impatient.par_chunks(FUND_CHUNK))
            .zip(cs.ui_spell.par_chunks_mut(FUND_CHUNK))
            .map(|(((((((hs, os), rs), wfs), caps), hms), imps), sps)| {
                let mut bank = vec![0.0; nb];
                let (mut fund_dep, mut fund_units) = (0.0, 0.0);
                let (mut dsum, mut dn) = (0.0, 0usize);
                for (((((((hh, ord), reb), wf), cap), hm), imp), sp) in
                    hs.iter_mut().zip(os).zip(rs).zip(wfs).zip(caps).zip(hms).zip(imps).zip(sps)
                {
                    if let Some(d) = decide_one(&env, hh, hm, ord, reb, wf, *cap, *imp, sp) {
                        dsum += d;
                        dn += 1;
                    }
                    fund_trade(hh, *reb, price, &mut bank, &mut fund_dep, &mut fund_units);
                }
                (bank, fund_dep, fund_units, dsum, dn)
            })
            .collect()
    };
    w.households = hhs;
    w.orders.household = orders;

    let mut delta = vec![0.0; nb];
    let (mut fund_dep, mut fund_units) = (0.0, 0.0);
    let (mut dsum, mut dn) = (0.0, 0usize);
    for (bank, fd, fu, ds, dk) in &parts {
        for (d, &x) in delta.iter_mut().zip(bank) {
            *d += x;
        }
        fund_dep += fd;
        fund_units += fu;
        dsum += ds;
        dn += dk;
    }
    cs.discretionary_mult_mean = if dn > 0 { dsum / dn as f64 } else { 1.0 };
    delta[fund_bank] += fund_dep;
    for (b, &x) in delta.iter().enumerate() {
        if x != 0.0 {
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
    }
    w.fund.deposits += fund_dep;
    w.fund.total_shares += fund_units;
    w.consumption = cs;
}

/// One household's Fund trade for rebalancing amount `r` (+ buy `r` of deposits, - sell `-r`), as
/// `ledger::fund_buy` / `ledger::fund_sell` at share price `price`. The household's deposits and shares change here;
/// its bank's deposits and reserves change by `bank` (per bank), and the Fund's deposits and shares by the other two.
fn fund_trade(hh: &mut Household, r: f64, price: f64, bank: &mut [f64], fund_dep: &mut f64, fund_units: &mut f64) {
    if r > 0.0 {
        // ledger::fund_buy(amount = r)
        if price <= 0.0 {
            return;
        }
        if r.is_finite() {
            hh.deposits -= r;
            bank[hh.bank as usize] -= r;
            *fund_dep += r;
        }
        let units = r / price;
        hh.shares += units;
        *fund_units += units;
    } else if r < 0.0 {
        // ledger::fund_sell(amount = -r)
        if price <= 0.0 {
            return;
        }
        let held = hh.shares;
        let units = ((-r) / price).min(held);
        let value = units * price;
        hh.shares -= units;
        *fund_units -= units;
        if value > 0.0 && value.is_finite() {
            hh.deposits += value;
            bank[hh.bank as usize] += value;
            *fund_dep -= value;
        }
    }
}

/// Sectors with the necessity price response (Energy and Food).
#[inline]
fn is_necessity(s: usize) -> bool {
    s == Sector::Energy.idx() || s == Sector::Food.idx()
}

/// Posted mean price of each consumer sector: planned output-weighted mean of the alive firms' posted prices. NaN where
/// no firm in the sector plans output this month.
fn posted_sector_prices(w: &World) -> [f64; N_CONSUMER] {
    let mut num = [0.0; N_CONSUMER];
    let mut den = [0.0; N_CONSUMER];
    for f in w.firms.iter().filter(|f| f.alive) {
        let s = f.sector.idx();
        let q = f.target_output;
        if s < N_CONSUMER && q.is_finite() && q > 0.0 && f.price.is_finite() && f.price > 0.0 {
            num[s] += q * f.price;
            den[s] += q;
        }
    }
    array::from_fn(|s| if den[s] > 0.0 { num[s] / den[s] } else { f64::NAN })
}

/// Real-quantity multipliers on subsistence for Energy and Food (fix-households): exp(-e_long * (A - A_ref)), with A an
/// exponentially smoothed log RELATIVE price (sector price over the CPI), so that a general inflation does not change
/// real necessity demand (homogeneity of degree zero). The reference is the first month, so the multiplier starts at 1.
fn necessity_multipliers(
    cfg: &Config,
    price: &[f64; N_CONSUMER],
    cpi: f64,
    cs: &mut ConsumptionState,
) -> ([f64; N_CONSUMER], [f64; N_CONSUMER]) {
    let mut mult = [1.0; N_CONSUMER];
    let mut extra = [1.0; N_CONSUMER];
    let ln_cpi = cpi.max(1e-9).ln();
    let sectors = [(Sector::Energy, cfg.energy_elasticity_long), (Sector::Food, cfg.food_elasticity_long)];
    for (k, &(sec, e_long)) in sectors.iter().enumerate() {
        let ln_p = finite_or(price[sec.idx()].max(1e-9).ln() - ln_cpi, 0.0);
        if !cs.necessity_init {
            cs.necessity_ln_avg[k] = ln_p;
            cs.necessity_ln_ref[k] = ln_p;
        }
        cs.necessity_ln_avg[k] += cfg.necessity_adapt * (ln_p - cs.necessity_ln_avg[k]);
        mult[sec.idx()] = (-e_long * (cs.necessity_ln_avg[k] - cs.necessity_ln_ref[k])).exp();
        extra[sec.idx()] = mult[sec.idx()] * (ln_p - cs.necessity_ln_ref[k]).exp();
    }
    cs.necessity_init = true;
    (mult, extra)
}

/// Refresh the persistent impatience type of every household slot (see `ConsumptionState::type_u`). A draw is keyed on
/// (seed, redraw count, stream, slot), so it does not depend on call order or thread count. Entrants (age falls, or the
/// slot was seen dead) get a new draw and lose the previous occupant's windfall stock.
fn assign_types(cfg: &Config, seed: u64, hhs: &[Household], cs: &mut ConsumptionState) {
    let n = hhs.len();
    cs.type_u.resize(n, f64::NAN);
    cs.type_n.resize(n, 0);
    cs.type_age.resize(n, 0);
    cs.impatient.resize(n, false);
    for (i, hh) in hhs.iter().enumerate() {
        if !hh.alive {
            cs.type_age[i] = u32::MAX;
            continue;
        }
        let age = hh.age_months as u32;
        if cs.type_u[i].is_nan() || age < cs.type_age[i] {
            if !cs.type_u[i].is_nan() {
                cs.type_n[i] += 1;
            }
            let mut rng = crate::rng::agent_rng(seed, cs.type_n[i], STREAM_HH_TYPE, i as u64);
            cs.type_u[i] = crate::rng::Rng::gen::<f64>(&mut rng);
            cs.windfall_stock[i] = 0.0;
            cs.ui_spell[i] = false;
        }
        cs.type_age[i] = age;
        cs.impatient[i] = cs.type_u[i] < cfg.impatient_share;
    }
}

/// Discretionary multiplier D for one household (DESIGN §8.3): exp of the weighted sum of the real-rate gap, excess
/// money, the unemployment gap and the bank-capital credit term, clamped to `[disc_mult_min, disc_mult_max]`.
/// Pure and monotone: D rises with excess money and with the bank capital surplus, and falls with the real rate and
/// with unemployment above the natural rate.
pub fn discretionary_multiplier(cfg: &Config, real_rate: f64, excess_money: f64, u_gap: f64, credit_gap: f64) -> f64 {
    // b7-transmission M-R: the rate gap saturates in a tightening (h = identity unless `disc_rate_sat_scale` > 0).
    let slack = (-u_gap).max(0.0);
    let rate_gap = saturating_rate_gap(
        NEUTRAL_REAL_RATE - real_rate,
        cfg.disc_rate_knee,
        cfg.disc_rate_sat_scale,
        cfg.disc_rate_sat_slack,
        slack,
    );
    let x = cfg.disc_theta_rate * rate_gap
        + cfg.disc_theta_money * excess_money
        + cfg.disc_theta_unemp * u_gap
        + cfg.disc_theta_credit * credit_gap;
    let d = finite_or(x.exp(), 1.0);
    d.clamp(cfg.disc_mult_min, cfg.disc_mult_max)
}

/// b7-transmission M-R: the rate gap g = NEUTRAL − real rate, bounded below the knee k. h(g) = g when `scale` is zero or
/// g ≥ k; otherwise h(g) = k − L_eff·tanh((k − g)/L_eff), with L_eff = L/(1 + γ·slack/0.01) and slack = max(u − u*, 0)
/// (decimal). Continuous and monotone; h ≥ k − L_eff, so a deep tightening saturates instead of compounding. With
/// `scale` zero it is the identity, so the legacy path is unchanged. Literature: Tenreyro & Thwaites 2016 AEJ:Macro;
/// Berger & Vavra 2015 Econometrica; Ascari & Haber 2022 EJ.
pub fn saturating_rate_gap(g: f64, knee: f64, scale: f64, gamma: f64, slack: f64) -> f64 {
    if !(scale > 0.0) || !g.is_finite() || g >= knee {
        return g;
    }
    let l_eff = scale / (1.0 + gamma * slack / 0.01);
    knee - l_eff * ((knee - g) / l_eff).tanh()
}

/// rz1 F1: mean deposit rate over the alive banks, in bank order (0 when none is alive).
fn mean_deposit_rate(w: &World) -> f64 {
    let (mut sum, mut n) = (0.0f64, 0usize);
    for b in w.banks.iter().filter(|b| b.alive) {
        sum += finite_or(b.deposit_rate, 0.0);
        n += 1;
    }
    if n > 0 { sum / n as f64 } else { 0.0 }
}

/// excess_money = ln(deposits ÷ desired money) − norm, with desired money = the liquid buffer target (clamped to ±2,
/// so that households with near-zero income do not dominate). `norm` is the baseline average of the log ratio
/// (`Config::disc_money_norm`), so that the multiplier averages ≈1 in the calibrated state.
fn excess_money(deposits: f64, buffer: f64, norm: f64) -> f64 {
    const FLOOR: f64 = 1e-6;
    (finite_or((deposits.max(FLOOR) / buffer.max(FLOOR)).ln(), 0.0) - norm).clamp(-2.0, 2.0)
}

/// Decide one household's budget, sector split, expectations and Fund trade (pure function of `env`).
/// Returns the household's discretionary multiplier, or None for a dead household.
fn decide_one(
    env: &Env,
    hh: &mut Household,
    hm: &HhMonth,
    ord: &mut [f64; N_CONSUMER],
    reb: &mut f64,
    wf: &mut f64,
    cap_prev: f64,
    impatient: bool,
    ui_spell: &mut bool,
) -> Option<f64> {
    *ord = [0.0; N_CONSUMER];
    *reb = 0.0;
    // Kink scale: 1 (base) or the wage index when `distquick_kink_wage_indexed` (see `kink_scale`).
    let ks = kink_scale(&env.w.cfg, env.w.prices.wage_index);
    if !hh.alive {
        hh.consumption_budget = 0.0;
        *wf = 0.0;
        return None;
    }
    let w = env.w;
    let cfg: &Config = &w.cfg;
    let share_price = finite_or(w.fund.share_price, 0.0).max(0.0);
    let dep_rate = w
        .banks
        .get(hh.bank as usize)
        .map_or(0.0, |b| finite_or(b.deposit_rate, 0.0));

    // expected inflation (§8.1): anchored adaptive plus energy-salience term, clamped
    let pi_prev = finite_or(hh.expected_inflation, env.target);
    let pi_e = (pi_prev
        // Well-anchored expectations (credible inflation target): 70% weight on the target, 30% on recent inflation.
        + EXP_INFL_ADAPT * ((0.7 * env.target + 0.3 * env.pi_yoy) - pi_prev)
        + if cfg.pump_expectations {
            // p1-energy: the 12-month change in the pump price (fuel line), `pump_salience` per unit
            cfg.pump_salience * common::pump_yoy(w)
        } else {
            EXP_INFL_SALIENCE * env.eyoy * cfg.cpi_weights[Sector::Energy.idx()]
        })
        .clamp(EXP_INFL_MIN, EXP_INFL_MAX);
    // b9-inflation M-H: a tightening (positive policy gap) lowers households' expected inflation, bounded by chi*L.
    let pi_e = if env.hh_term != 0.0 { (pi_e - env.hh_term).clamp(EXP_INFL_MIN, EXP_INFL_MAX) } else { pi_e };

    // after-tax income estimate and permanent income
    let inc = hm.income;
    let cap = finite_or(cap_prev, 0.0);
    let taxable = inc.wage + cap + 0.85 * inc.pension;
    let income_tax = common::income_tax_annual_indexed(cfg, 12.0 * taxable.max(0.0), env.tax_add, w.gov.cola_index) / 12.0;
    let payroll = cfg.payroll_tax * inc.wage;
    // Split after-tax income into labour/transfer income (high MPC, smoothed into permanent income) and capital
    // income (low MPC: concentrated among wealthy households; Kaplan & Violante 2014).
    let avg_tax = if taxable > 1e-9 { (income_tax / taxable).clamp(0.0, 0.6) } else { 0.0 };
    let cap_net = (cap * (1.0 - avg_tax)).max(0.0);
    let y = (inc.wage + inc.unemployment_benefit + inc.pension + inc.transfers + cap - income_tax - payroll).max(0.0);
    let y_lab = (y - cap_net).max(0.0);
    // fix-households: while on unemployment insurance, permanent income steps down at the start of the spell (by
    // `ui_yp_onset`, the pre-loss level times the onset factor) and then drifts down by `ui_yp_drift` a month, which gives
    // Ganong & Noel's (2019) fall at onset and slow decline during receipt. It falls again at exhaustion (see below).
    let unemployed = hh.status == LaborStatus::Unemployed;
    // fix-households: impatient types (KVW 2014) smooth permanent income less, so their spending follows current income.
    let yp_w = if impatient { cfg.impatient_yp_weight } else { 0.9 };
    // Benefits paid this month count as UI: pay_transfers decrements `benefit_months_left` when it pays, so the last
    // paid month already has `benefit_months_left == 0`.
    let on_ui = unemployed && (inc.unemployment_benefit > 0.0 || hh.benefit_months_left > 0);
    let spell_start = on_ui && !*ui_spell;
    if on_ui {
        *ui_spell = true;
    } else if !unemployed {
        *ui_spell = false;
    }
    // Spells without UI (short job searches, entrants, re-entrants) get the patient rules, not the exhaustion rules.
    let post_ui = unemployed && !on_ui && *ui_spell;
    let yp = if on_ui {
        let f = if spell_start { cfg.ui_yp_onset } else { 1.0 - cfg.ui_yp_drift };
        finite_or(hh.permanent_income * f, 0.0).max(0.0)
    } else if post_ui {
        // fix-households: after exhaustion permanent income falls slowly towards current (transfer) income, so spending
        // falls at exhaustion and then levels off (Ganong & Noel 2019), instead of converging to transfers within a year.
        let pull = cfg.exhaust_yp_pull;
        finite_or((1.0 - pull) * hh.permanent_income + pull * y_lab, 0.0).max(0.0)
    } else {
        finite_or(yp_w * hh.permanent_income + (1.0 - yp_w) * y_lab, 0.0).max(0.0)
    };

    // liquid wealth, buffer target by age, real rate
    let deposits = finite_or(hh.deposits, 0.0);
    let fund_value = finite_or(hh.shares * share_price, 0.0);
    // Liquid buffer = deposits; Fund wealth has its own (smaller) wealth effect below.
    // fix-households: a one-off payment credited this month is transitory income (spent by the windfall rule below), so
    // the liquidity that sets the MPC and the wealth terms is the balance before receipt.
    let wf_in = finite_or(inc.windfall, 0.0).max(0.0).min(deposits.max(0.0));
    let w_liq = deposits - wf_in;
    let wf_stock = finite_or(*wf, 0.0).max(0.0);
    let age = hh.age_years();
    // fix-households: impatient households hold a small buffer (KVW 2014 hand-to-mouth)
    let k =if impatient {
        cfg.impatient_buffer_months
    } else if age < 30.0 {
        2.0
    } else if age < 55.0 {
        4.0
    } else {
        8.0
    };
    let buffer = k * yp;
    // Real interest rate households face: mix of deposit and 2-year market rates (savings, auto/credit, ARMs).
    // rz1 F1: with `rate_perception_deposit` the deposit leg is the smoothed rate (the household's bank rate plus the
    // perceived-minus-current shift of the mean); otherwise `dep_rate` verbatim.
    let dep_rate = if env.dep_shift != 0.0 { dep_rate + env.dep_shift } else { dep_rate };
    let real_rate = 0.5 * dep_rate + 0.5 * env.yield24 - pi_e + w.exog.risk_premium;
    // rz2: the real rate leaves out (1 - weight) of the energy-salience part of expected inflation (off value: shift is 0, legacy line).
    let real_rate = if env.pump_shift != 0.0 { real_rate + env.pump_shift } else { real_rate };

    // housing: committed outflow (none when living with parents), owner maintenance (added to Services)
    let withparents = hh.tenure == Tenure::WithParents;
    let housing_cost = if withparents { 0.0 } else { finite_or(hh.housing_cost, 0.0).max(0.0) };
    let owner_home = if hh.tenure == Tenure::Owner { hh.home } else { None };

    // sector subsistence (nominal), reduced for those living with parents
    let factor = if withparents { 0.6 } else { 1.0 };
    let subsist: [f64; N_CONSUMER] = array::from_fn(|i| env.price[i] * cfg.subsistence[i] * factor * env.necessity[i]);
    let sub_cost: f64 = subsist.iter().sum();

    // b7-transmission M-R: the unemployment gap is a pure function of the economy, so it is read here for mpc_y (the
    // discretionary term below uses the same value).
    let u_gap = w.natural_unemployment() - w.unemployment_rate();
    // raw budget before housing cost and the subsistence floor
    let core = if hh.status == LaborStatus::Retired {
        // retirees: pension plus an annuity of liquid wealth over remaining life expectancy plus a bequest/longevity
        // horizon (fix-households: De Nardi 2004; De Nardi, French & Jones 2010), discounted at a slow-moving real rate
        // so that policy rate moves do not change the drawdown
        let life = env.life.get((age.max(0.0) as usize).min(LIFE_TABLE_MAX_AGE)).copied().unwrap_or(1.0);
        let months = ((life + cfg.retiree_horizon_years) * 12.0).round().max(1.0) as u32;
        finite_or(hh.pension, 0.0)
            + annuity_payment(concave_wealth((w_liq + fund_value).max(0.0), ks), cfg.retiree_real_rate, months)
    } else {
        // Intertemporal substitution: ≈1% lower consumption per 1pp higher real rate (EIS ≈ 0.5–1).
        // fix-households: patient households' income MPC is lowered by `patient_mpc_offset`, offsetting the extra spending
        // of impatient types so that aggregate spending and its sector mix stay near the calibrated state.
        let patient_offset = if impatient { 0.0 } else { cfg.patient_mpc_offset };
        // b7-transmission M-R: with a saturation scale, the rate term uses the bounded gap −h(g); with none it is the
        // legacy expression, verbatim.
        let mpc_y = if cfg.disc_rate_sat_scale > 0.0 {
            let h = saturating_rate_gap(
                NEUTRAL_REAL_RATE - real_rate,
                cfg.disc_rate_knee,
                cfg.disc_rate_sat_scale,
                cfg.disc_rate_sat_slack,
                (-u_gap).max(0.0),
            );
            cfg.mpc_income - patient_offset - cfg.consumption_rate_sens * (-h).max(-0.03)
        } else {
            cfg.mpc_income - patient_offset - cfg.consumption_rate_sens * (real_rate - NEUTRAL_REAL_RATE).max(-0.03)
        };
        let mpc_w = if w_liq > buffer {
            if impatient {
                cfg.impatient_mpc_above
            } else {
                cfg.mpc_wealth_above
            }
        } else {
            cfg.mpc_wealth_below
        };
        let house_equity: f64 = hh
            .properties
            .iter()
            .map(|&p| common::house_value(w, p) - common::mortgage_balance(w, p))
            .sum();
        let excess_liquid = w_liq - buffer;
        let excess_liquid = if excess_liquid > 0.0 { concave_wealth(excess_liquid, ks) } else { excess_liquid };
        mpc_y * yp
            + MPC_CAPITAL_INCOME * concave_capital_income(cap_net, ks)
            + mpc_w * excess_liquid
            + cfg.mpc_housing_wealth_annual / 12.0 * concave_wealth(house_equity.max(0.0), ks)
            // p1-equity: the Fund-wealth term reads the 12-month EWMA of the share price, not this month's price.
            + cfg.mpc_fund_wealth_annual / 12.0
                * concave_wealth(finite_or(hh.shares * crate::equity::smoothed_share_price(w), 0.0), ks)
    };
    let core = finite_or(core, 0.0);
    // Pass-through spending (landlords' operating costs out of rent, employer health benefits) is funded by the matching
    // income, so it sits outside the cap (round 7 step 4: computed here, added to the budget after the cap).
    let landlord_opex = (LANDLORD_OPEX_SHARE * inc.rent_received).max(0.0);
    let benefits = if hh.status == LaborStatus::Employed { (cfg.employer_benefits * inc.wage).max(0.0) } else { 0.0 };
    let pass = landlord_opex + benefits;
    // subsistence floor; then the budget is capped by the cash available (round 7 step 4, `housing_first_budget`): this
    // month's scheduled mortgage payments and the pass-through spending come first, and there is no credit line unless
    // `consumer_credit_months` > 0. Legacy: deposits plus a one-month credit line of yp (§8.1).
    let mut c = (core - housing_cost).max(sub_cost);
    // fix-households: after benefit exhaustion the one-month credit line moves from permanent towards current income
    // (`exhaust_credit_share` on permanent income), which gives Ganong & Noel's drop at exhaustion without a full cut;
    // on UI it stays at permanent income.
    let credit_line = if post_ui {
        let s = cfg.exhaust_credit_share;
        (1.0 - s) * y_lab + s * yp
    } else {
        yp
    };
    let room = if cfg.housing_first_budget {
        let due = crate::banking::mortgage_payments_of(w, hh);
        (deposits - due - pass).max(0.0) + cfg.consumer_credit_months * credit_line
    } else {
        deposits + credit_line
    };
    c = c.min(room).max(0.0);
    // Headroom for the discretionary and windfall spending above the budget `c` (which includes `pass`).
    let headroom = |c: f64| if cfg.housing_first_budget { room - (c - pass) } else { deposits + yp - c };
    // discretionary multiplier (DESIGN §8.3), computed below with the split
    // credit term: only a capital shortfall at the household's bank (below the requirement) restricts credit, so the term
    // is zero in normal times and bites in a credit crunch
    let credit_gap = (crate::banking::capital_ratio(w, hh.bank) - cfg.capital_requirement).clamp(-0.05, 0.0);
    let disc_d = discretionary_multiplier(cfg, real_rate, excess_money(deposits, buffer, cfg.disc_money_norm), u_gap, credit_gap);

    // owner maintenance is part of the budget and goes to household services
    let maint = owner_home
        .map_or(0.0, |h| cfg.maintenance / 12.0 * common::house_value(w, h))
        .max(0.0)
        .min(c);
    // Landlords' operating costs (maintenance, management, insurance ≈ 35% of rent) are bought as household services;
    // employer-paid health benefits received with wages are spent on health care (`health_sector_mix`). Both are in `pass`.
    c += pass;
    // (summed in the HEAD order, so the legacy path stays bit-identical)
    let maint = maint + landlord_opex + benefits;
    let split_total = c - maint;

    // Linear expenditure system (§8.2) with the discretionary multiplier (§8.3). The multiplier scales the
    // discretionary part of the supernumerary spending, dw_i·β_i·extra. The change in total spending comes out of
    // (or goes into) deposits through the goods market; it is limited so that the budget stays within deposits plus
    // the one-month credit line, and it never takes spending below subsistence.
    let beta = cfg.supernumerary_share;
    let dw = cfg.discretionary_weight;
    if split_total >= sub_cost {
        let extra = split_total - sub_cost;
        let disc_share: f64 = (0..N_CONSUMER).map(|i| dw[i] * beta[i]).sum();
        let mut d_eff = disc_d;
        if disc_share > 0.0 && extra > 0.0 {
            let mut delta = (disc_d - 1.0) * disc_share * extra;
            if delta > 0.0 {
                delta = delta.min(headroom(c).max(0.0));
            }
            d_eff = 1.0 + delta / (disc_share * extra);
            c += delta;
        }
        // fix-households: the supernumerary part of Energy and Food is scaled by `necessity_extra`, and the other sectors'
        // supernumerary parts shrink in proportion, so the total is unchanged.
        let mut xs: [f64; N_CONSUMER] = array::from_fn(|i| beta[i] * extra * (1.0 + (d_eff - 1.0) * dw[i]));
        let (mut moved, mut others) = (0.0, 0.0);
        for s in 0..N_CONSUMER {
            if is_necessity(s) {
                let new = xs[s] * env.necessity_extra[s];
                moved += new - xs[s];
                xs[s] = new;
            } else {
                others += xs[s];
            }
        }
        if moved != 0.0 && others > 0.0 {
            let g = ((others - moved) / others).max(0.0);
            for s in 0..N_CONSUMER {
                if !is_necessity(s) {
                    xs[s] *= g;
                }
            }
        }
        for i in 0..N_CONSUMER {
            ord[i] = subsist[i] + xs[i];
        }
    } else {
        let scale = if sub_cost > 0.0 { split_total / sub_cost } else { 0.0 };
        for i in 0..N_CONSUMER {
            ord[i] = subsist[i] * scale;
        }
    }
    // fix-households: one-off payments are spent at once at a liquidity-dependent MPC (Parker et al. 2013), split across
    // sectors by discretionary weight (durables about 30%). Whatever is not spent stays in deposits and is run down by
    // the wealth rules from next month.
    // The liquidity test uses an absolute floor as well as the buffer: impatient types have a 0.1-month buffer, and the
    // hand-to-mouth households that the windfall rule is meant for hold less than a month of income.
    // Deposits already hold this month's labour and transfer income (the capital part is paid after `decide`), so the
    // pre-receipt liquidity is the balance less `y_lab`, not the balance less the one-off payment alone.
    let wf_floor = buffer.max(cfg.windfall_liquid_months * yp);
    let liq_pre = w_liq - y_lab;
    let mpc_wf = if liq_pre < wf_floor { cfg.mpc_windfall_low } else { cfg.mpc_windfall_high };
    let want_wf = mpc_wf * wf_in + cfg.windfall_decay * wf_stock;
    let ws = want_wf.min(headroom(c).max(0.0));
    if ws > 0.0 {
        c += ws;
        for i in 0..N_CONSUMER {
            ord[i] += ws * cfg.windfall_sector_mix[i];
        }
    }
    *wf = (wf_stock + wf_in - ws).max(0.0).min(deposits.max(0.0));
    // Maintenance and landlords' operating costs are household service purchases (`Config::household_services_mix`);
    // employer-paid health benefits are health-care purchases (`Config::health_sector_mix`). A sector with zero
    // weight gets nothing.
    let benefits_spend = benefits;
    let services_spend = maint - benefits;
    for s in 0..N_CONSUMER {
        let mix = cfg.household_services_mix[s];
        if mix != 0.0 {
            ord[s] += services_spend * mix;
        }
        let hmix = cfg.health_sector_mix[s];
        if hmix != 0.0 {
            ord[s] += benefits_spend * hmix;
        }
    }

    // Fund rebalancing (§11.4): households keep deposits of at least the larger of the precautionary buffer and a
    // liquid-asset share of financial wealth (US: deposits ≈ 15–20% of household financial assets, ≈ 10 months of
    // income); 10% of any excess a month goes into the Fund; shares are sold when deposits fall below half the buffer.
    if share_price > 0.0 {
        // fix-households: impatient households invest only from deposits above a few months of income
        let desired = if impatient {
            (1.5 * buffer + c).max(cfg.impatient_fund_floor_months * yp)
        } else {
            (1.5 * buffer + c).max(LIQUID_SHARE * (deposits + fund_value))
        };
        let excess = deposits - desired;
        if excess > 0.0 {
            *reb = FUND_BUY_RATE * excess;
        } else if deposits < 0.5 * buffer && hh.shares > 0.0 {
            *reb = -(buffer - deposits).min(fund_value);
        }
    }
    *reb = finite_or(*reb, 0.0);

    hh.consumption_budget = c;
    hh.expected_inflation = pi_e;
    hh.permanent_income = yp;
    Some(disc_d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ledger::{check_consistency, Account};
    use crate::testkit::tiny_world;

    /// Tiny world where incomes are set from the testkit wages, so that quintiles differ.
    fn base_world(seed: u64) -> World {
        base_world_cfg(Config { n_households: 500, ..Default::default() }, seed)
    }

    fn base_world_cfg(cfg: Config, seed: u64) -> World {
        let mut w = tiny_world(cfg, seed);
        for (h, m) in w.households.iter().zip(w.hh_month.iter_mut()) {
            m.income.wage = h.wage;
            m.income.pension = h.pension;
        }
        w
    }

    /// Sum of budgets per consumer sector over the households in `members`.
    fn spend_by_sector(w: &World, members: &[usize]) -> [f64; N_CONSUMER] {
        let mut t = [0.0; N_CONSUMER];
        for &i in members {
            for s in 0..N_CONSUMER {
                t[s] += w.orders.household[i][s];
            }
        }
        t
    }

    #[test]
    fn budgets_are_consistent_and_bounded() {
        let mut w = base_world(2);
        let deposits_before: Vec<f64> = w.households.iter().map(|h| h.deposits).collect();
        decide(&mut w);
        check_consistency(&w).unwrap();
        for (i, h) in w.households.iter().enumerate() {
            let o = w.orders.household[i];
            assert!(o.iter().all(|x| x.is_finite() && *x >= 0.0), "household {i}");
            let total: f64 = o.iter().sum();
            assert!((total - h.consumption_budget).abs() <= 1e-6 * total.max(1.0));
            // Pass-through spending (landlord operating costs out of rent, employer health benefits) is funded by the
            // matching income and sits on top of the deposits-plus-one-month cap.
            let inc = &w.hh_month[i].income;
            let pass = LANDLORD_OPEX_SHARE * inc.rent_received.max(0.0)
                + if h.status == LaborStatus::Employed { w.cfg.employer_benefits * inc.wage.max(0.0) } else { 0.0 };
            assert!(h.consumption_budget <= deposits_before[i].max(0.0) + h.permanent_income + pass + 1e-6);
            assert!(h.expected_inflation >= EXP_INFL_MIN && h.expected_inflation <= EXP_INFL_MAX);
        }
    }

    // Round 7 step 4: with no default credit line, spending is capped at deposits less this month's scheduled mortgage
    // payments and pass-through spending.
    #[test]
    fn housing_first_budget_leaves_mortgage_money_unspent() {
        let mut w = base_world(5);
        assert!(w.cfg.housing_first_budget && w.cfg.consumer_credit_months == 0.0);
        let deposits_before: Vec<f64> = w.households.iter().map(|h| h.deposits).collect();
        decide(&mut w);
        check_consistency(&w).unwrap();
        for (i, h) in w.households.iter().enumerate() {
            let inc = &w.hh_month[i].income;
            let pass = LANDLORD_OPEX_SHARE * inc.rent_received.max(0.0)
                + if h.status == LaborStatus::Employed { w.cfg.employer_benefits * inc.wage.max(0.0) } else { 0.0 };
            let due = crate::banking::mortgage_payments_of(&w, h);
            let bound = (deposits_before[i] - due - pass).max(0.0);
            assert!(h.consumption_budget - pass <= bound + 1e-6, "household {i}: {} > {bound}", h.consumption_budget - pass);
        }
    }

    #[test]
    fn oil_shock_shifts_spending_to_energy_most_for_low_income() {
        let base = base_world(11);
        let mut a = base.clone();
        let mut b = base.clone();
        b.prices.sector[Sector::Energy.idx()] *= 2.0;
        decide(&mut a);
        decide(&mut b);

        // income quintiles by permanent income (identical in both runs)
        let mut idx: Vec<usize> = (0..a.households.len()).filter(|&i| a.households[i].alive).collect();
        idx.sort_by(|&i, &j| {
            a.households[i]
                .permanent_income
                .partial_cmp(&a.households[j].permanent_income)
                .unwrap()
        });
        let q = idx.len() / 5;
        let e = Sector::Energy.idx();
        let g = Sector::DurableGoods.idx();
        let sv = Sector::OtherServices.idx();
        let mut d_energy = Vec::new();
        println!("quintile  yp($/m)   share E: before -> after   share G: before -> after   share S: before -> after   G+S $ change");
        for k in 0..5 {
            let lo = k * q;
            let hi = if k == 4 { idx.len() } else { (k + 1) * q };
            let members = &idx[lo..hi];
            let yp_mean: f64 = members.iter().map(|&i| a.households[i].permanent_income).sum::<f64>()
                / members.len() as f64;
            let sa = spend_by_sector(&a, members);
            let sb = spend_by_sector(&b, members);
            let ta: f64 = sa.iter().sum();
            let tb: f64 = sb.iter().sum();
            let (ea, eb) = (sa[e] / ta, sb[e] / tb);
            let (ga, gb) = (sa[g] / ta, sb[g] / tb);
            let (va, vb) = (sa[sv] / ta, sb[sv] / tb);
            println!(
                "Q{}       {:>8.0}   {:.3} -> {:.3}          {:.3} -> {:.3}          {:.3} -> {:.3}          {:+.0}",
                k + 1,
                yp_mean,
                ea,
                eb,
                ga,
                gb,
                va,
                vb,
                (sb[g] + sb[sv]) - (sa[g] + sa[sv])
            );
            assert!(eb > ea, "energy share should rise in Q{}", k + 1);
            assert!(gb + vb < ga + va, "Goods+Services share should fall in Q{}", k + 1);
            d_energy.push(eb - ea);
        }
        // the energy-share increase is largest for the lowest-income quintile
        assert!(d_energy[0] > d_energy[4]);
    }

    #[test]
    fn energy_salience_raises_expected_inflation() {
        let legacy = crate::testkit::legacy_p1off_config();
        let mut flat = base_world_cfg(Config { n_households: 500, ..legacy.clone() }, 5);
        let mut shock = base_world_cfg(Config { n_households: 500, ..legacy }, 5);
        let mut pi_flat = 0.0;
        let mut pi_shock = 0.0;
        for _ in 0..20 {
            decide(&mut flat);
            shock.prices.sector[Sector::Energy.idx()] *= 1.5;
            decide(&mut shock);
            pi_flat = flat.households[0].expected_inflation;
            pi_shock = shock.households[0].expected_inflation;
        }
        assert!(pi_shock > pi_flat + 0.005, "flat {pi_flat} shock {pi_shock}");
        assert!(shock.households.iter().all(|h| h.expected_inflation <= EXP_INFL_MAX));
    }

    // p1-energy (`pump_expectations`): the salience term follows the 12-month change in the pump price, not the Energy
    // sector price.
    #[test]
    fn pump_price_salience_raises_expected_inflation() {
        let mut flat = base_world(5);
        let mut shock = base_world(5);
        flat.cfg.pump_expectations = true;
        shock.cfg.pump_expectations = true;
        let p = common::gasoline_price(&flat);
        flat.firms_state.pump_hist = vec![p; 13];
        let mut rising = vec![p; 12];
        rising.push(1.5 * p);
        shock.firms_state.pump_hist = rising;
        let mut pi_flat = 0.0;
        let mut pi_shock = 0.0;
        for _ in 0..20 {
            decide(&mut flat);
            decide(&mut shock);
            pi_flat = flat.households[0].expected_inflation;
            pi_shock = shock.households[0].expected_inflation;
        }
        // A +50% pump change adds pump_salience × 0.5 = 0.002 a month. The gap relaxes at EXP_INFL_ADAPT (0.1) a month,
        // so after 20 months it is 0.002 × (1 − 0.9^20) / 0.1 ≈ 0.0176 (1.76pp), not the 0.2pp of a one-off shock.
        let gap = pi_shock - pi_flat;
        assert!((0.012..=0.024).contains(&gap), "gap {gap} (flat {pi_flat} shock {pi_shock})");
        assert!(shock.households.iter().all(|h| h.expected_inflation <= EXP_INFL_MAX));
    }

    #[test]
    fn capital_income_is_recorded_for_next_month() {
        let mut w = base_world(9);
        w.hh_month[0].income.interest = 100.0;
        w.hh_month[0].income.dividends = 25.0;
        w.hh_month[0].income.rent_received = 50.0;
        record_capital_income(&mut w);
        assert_eq!(w.consumption.capital_income_prev[0], 175.0);
        assert_eq!(w.consumption.capital_income_prev.len(), w.households.len());
        decide(&mut w);
        assert!(w.households[0].consumption_budget.is_finite());
    }

    #[test]
    fn discretionary_multiplier_rises_with_excess_money_and_falls_with_real_rates() {
        // the legacy (linear) response: since J4 the default has the R3 keys on, and R3 adds h(0) > 0 at the neutral rate
        let cfg = Config { disc_rate_knee: 0.0, disc_rate_sat_scale: 0.0, disc_rate_sat_slack: 0.0, ..Config::default() };
        let base = discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE, 0.0, 0.0, 0.0);
        assert!((base - 1.0).abs() < 1e-12, "neutral state gives D = 1, got {base}");
        // hot potato: more excess money, more discretionary spending
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE, 0.5, 0.0, 0.0) > base);
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE, -0.5, 0.0, 0.0) < base);
        // higher real rates, less discretionary spending (and the reverse)
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE + 0.01, 0.0, 0.0, 0.0) < base);
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE - 0.01, 0.0, 0.0, 0.0) > base);
        // unemployment above the natural rate and a credit crunch both lower D
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE, 0.0, -0.03, 0.0) < base);
        assert!(discretionary_multiplier(&cfg, NEUTRAL_REAL_RATE, 0.0, 0.0, -0.03) < base);
        // clamped to the configured range
        assert_eq!(discretionary_multiplier(&cfg, -1.0, 0.0, 0.0, 0.0), cfg.disc_mult_max);
        assert_eq!(discretionary_multiplier(&cfg, 1.0, 0.0, 0.0, 0.0), cfg.disc_mult_min);
    }

    // joint3 F3 (b7-transmission review): with the R3 keys a deep tightening gives a discretionary response closer to 1
    // than the legacy (identity) response.
    #[test]
    fn discretionary_response_to_a_deep_tightening_is_bounded_with_the_r3_keys() {
        let legacy = Config { disc_rate_knee: 0.0, disc_rate_sat_scale: 0.0, disc_rate_sat_slack: 0.0, ..Config::default() };
        let r3 = Config { disc_rate_knee: 0.005, disc_rate_sat_scale: 0.005, disc_rate_sat_slack: 1.0, ..Config::default() };
        let real = NEUTRAL_REAL_RATE + 0.05;
        let d_legacy = discretionary_multiplier(&legacy, real, 0.0, -0.02, 0.0);
        let d_r3 = discretionary_multiplier(&r3, real, 0.0, -0.02, 0.0);
        assert!((d_r3 - 1.0).abs() < (d_legacy - 1.0).abs(), "R3 {d_r3} should be closer to 1 than legacy {d_legacy}");
    }

    // rz1 F1: the deposit-rate leg of the real rate goes through the same smoother as the 2-year leg.
    fn d_mean_after_deposit_rise(lag: f64, deposit_leg: bool, rise: f64) -> (f64, Option<f64>) {
        let cfg = Config { n_households: 500, rate_perception_months: lag, rate_perception_deposit: deposit_leg, ..Default::default() };
        let mut w = base_world_cfg(cfg, 21);
        for b in w.banks.iter_mut() {
            b.deposit_rate = 0.01;
        }
        decide(&mut w);
        for b in w.banks.iter_mut() {
            b.deposit_rate = 0.01 + rise;
        }
        decide(&mut w);
        (w.consumption.discretionary_mult_mean, w.consumption.deposit_perceived)
    }

    // rz12: with two stages the perceived deposit rate has moved less after one month than with one stage (a hump, not a jump), and the
    // 2-year leg is a cascade too; one stage keeps the legacy state.
    #[test]
    fn two_stage_perception_moves_the_deposit_leg_more_slowly() {
        let run = |stages: u32| -> (f64, Option<f64>, Option<f64>) {
            let cfg = Config { n_households: 500, rate_perception_months: 10.0, rate_perception_deposit: true, rate_perception_stages: stages, ..Default::default() };
            let mut w = base_world_cfg(cfg, 21);
            for b in w.banks.iter_mut() {
                b.deposit_rate = 0.01;
            }
            decide(&mut w);
            let y_before = w.curve.yield_at(24.0);
            for b in w.banks.iter_mut() {
                b.deposit_rate = 0.03;
            }
            for y in w.curve.yields.iter_mut() {
                *y += 0.02; // the whole curve shifts, so the 2-year yield moves by exactly 0.02
            }
            decide(&mut w);
            (w.consumption.discretionary_mult_mean, w.consumption.deposit_perceived, w.consumption.yield24_perceived.map(|y| y - y_before))
        };
        let (_, dep1, y1) = run(1);
        let (_, dep2, y2) = run(2);
        assert!((dep1.unwrap() - (0.01 + 0.02 / 11.0)).abs() < 1e-12, "one stage: 1 / (1 + 10) of the way: {dep1:?}");
        assert!((dep2.unwrap() - (0.01 + 0.02 / 6.0 / 6.0)).abs() < 1e-12, "two stages of lag 5: {dep2:?}");
        // the 2-year leg is a cascade too, with the same stage counts
        assert!((y1.unwrap() - 0.02 / 11.0).abs() < 1e-12, "one stage, 2-year leg: {y1:?}");
        assert!((y2.unwrap() - 0.02 / 6.0 / 6.0).abs() < 1e-12, "two stages, 2-year leg: {y2:?}");
    }

    #[test]
    fn deposit_leg_off_changes_nothing_and_stores_no_state() {
        // Flag off, any lag: the same discretionary multiplier whether or not the flag key exists at its default.
        let (d_off, st_off) = d_mean_after_deposit_rise(6.0, false, 0.02);
        assert!(st_off.is_none(), "no perceived deposit rate is kept while the flag is off");
        // Flag on with lag 0: the smoother is skipped, identical to flag off.
        let (d_lag0_on, st_lag0) = d_mean_after_deposit_rise(0.0, true, 0.02);
        let (d_lag0_off, _) = d_mean_after_deposit_rise(0.0, false, 0.02);
        assert_eq!(d_lag0_on.to_bits(), d_lag0_off.to_bits());
        assert!(st_lag0.is_none());
        assert!(d_off.is_finite());
    }

    #[test]
    fn deposit_leg_smooths_a_deposit_rate_rise_with_the_same_lag() {
        let rise = 0.02;
        let (d_off, _) = d_mean_after_deposit_rise(6.0, false, rise);
        let (d_on, perceived) = d_mean_after_deposit_rise(6.0, true, rise);
        // The perceived rate moves 1 / (1 + lag) of the way: 0.01 + 0.02 / 7.
        assert!((perceived.unwrap() - (0.01 + rise / 7.0)).abs() < 1e-12, "{perceived:?}");
        // A rise in the deposit rate depresses the discretionary multiplier less when the household sees only part of it.
        assert!(d_on > d_off, "smoothed {d_on} vs current {d_off}");
        // A fall mirrors it: the smoothed leg keeps the multiplier lower than the current rate does.
        let (d_off_dn, _) = d_mean_after_deposit_rise(6.0, false, -0.005);
        let (d_on_dn, _) = d_mean_after_deposit_rise(6.0, true, -0.005);
        assert!(d_on_dn < d_off_dn, "smoothed {d_on_dn} vs current {d_off_dn}");
    }

    #[test]
    fn mean_deposit_rate_counts_alive_banks_only() {
        let mut w = base_world(5);
        let n = w.banks.len();
        assert!(n >= 2);
        for (i, b) in w.banks.iter_mut().enumerate() {
            b.deposit_rate = 0.01 * (i + 1) as f64;
        }
        let all: f64 = (1..=n).map(|i| 0.01 * i as f64).sum::<f64>() / n as f64;
        assert!((mean_deposit_rate(&w) - all).abs() < 1e-15);
        w.banks[n - 1].alive = false;
        let alive: f64 = (1..n).map(|i| 0.01 * i as f64).sum::<f64>() / (n - 1) as f64;
        assert!((mean_deposit_rate(&w) - alive).abs() < 1e-15);
        for b in w.banks.iter_mut() {
            b.alive = false;
        }
        assert_eq!(mean_deposit_rate(&w), 0.0);
    }

    #[test]
    fn fund_rebalancing_moves_deposits_and_shares_consistently() {
        let mut w = base_world(13);
        // a very liquid household (money endowed through the ledger) should buy Fund shares
        crate::ledger::pay(&mut w, Account::CentralBank, Account::Household(0), 5.0e6);
        let shares_before = w.households[0].shares;
        let deposits_before = w.households[0].deposits;
        decide(&mut w);
        check_consistency(&w).unwrap();
        assert!(w.households[0].shares > shares_before);
        assert!(w.households[0].deposits < deposits_before);
    }

    // b9-inflation M-H: a household policy term keyed to the PolicyShift gap.
    /// Without a gap the term is zero, so the expectation path is bit-identical to the key at 0.
    #[test]
    fn hh_policy_term_is_zero_without_a_gap() {
        let mut off = tiny_world(Config { n_households: 500, ..Default::default() }, 9);
        let mut on = tiny_world(Config { n_households: 500, hh_mp_expect: 0.08, ..Default::default() }, 9);
        for _ in 0..12 {
            crate::sim::step(&mut off);
            crate::sim::step(&mut on);
        }
        assert!(off
            .households
            .iter()
            .zip(on.households.iter())
            .all(|(a, b)| a.expected_inflation.to_bits() == b.expected_inflation.to_bits()));
    }

    /// After a +100bp PolicyShift the household expectation is lower than in the key-0 world, by a bounded amount.
    #[test]
    fn hh_policy_term_lowers_expectations_after_tightening() {
        let cfg = Config { n_households: 500, hh_mp_expect: 0.08, ..Default::default() };
        let mut on = tiny_world(cfg.clone(), 10);
        let mut off = tiny_world(Config { hh_mp_expect: 0.0, ..cfg }, 10);
        on.shocks.policy_shift = Some((0.01, 0, 0.9));
        off.shocks.policy_shift = Some((0.01, 0, 0.9));
        for _ in 0..6 {
            crate::sim::step(&mut on);
            crate::sim::step(&mut off);
        }
        let mean = |w: &World| w.households.iter().map(|h| h.expected_inflation).sum::<f64>() / w.households.len() as f64;
        let lower_by = mean(&off) - mean(&on);
        assert!(lower_by > 1e-5 && lower_by < 0.01, "lower by {lower_by}");
    }

    // rz2: `hh_pump_real_rate_weight` leaves the energy-salience part of expected inflation out of the real rate.
    /// Weight 1 (the off value) is the legacy path: no state is written and a pump-price surge gives the same bits as the default.
    #[test]
    fn pump_weight_one_is_the_legacy_path_and_stores_no_state() {
        let surge = |w: &mut World| {
            w.cfg.pump_expectations = true;
            let p = common::gasoline_price(w);
            let mut rising = vec![p; 12];
            rising.push(1.5 * p);
            w.firms_state.pump_hist = rising;
        };
        let mut dflt = base_world(7);
        let mut one = base_world(7);
        one.cfg.hh_pump_real_rate_weight = 1.0;
        surge(&mut dflt);
        surge(&mut one);
        for _ in 0..12 {
            decide(&mut dflt);
            decide(&mut one);
        }
        assert_eq!(dflt.consumption.pump_comp, 0.0);
        assert_eq!(one.consumption.pump_comp, 0.0);
        assert_eq!(dflt.consumption.discretionary_mult_mean.to_bits(), one.consumption.discretionary_mult_mean.to_bits());
        assert!(dflt.households.iter().zip(one.households.iter()).all(|(a, b)| a.consumption_budget.to_bits() == b.consumption_budget.to_bits()));
    }

    /// The stored component equals the gap the pump surge opens in household expected inflation (z' = 0.9 z + s p, the same recursion).
    #[test]
    fn pump_component_equals_the_energy_salience_gap_in_expected_inflation() {
        let mut flat = base_world(5);
        let mut shock = base_world(5);
        for w in [&mut flat, &mut shock] {
            w.cfg.pump_expectations = true;
            w.cfg.hh_pump_real_rate_weight = 0.0;
        }
        let p = common::gasoline_price(&flat);
        flat.firms_state.pump_hist = vec![p; 13];
        let mut rising = vec![p; 12];
        rising.push(1.5 * p);
        shock.firms_state.pump_hist = rising;
        for _ in 0..20 {
            decide(&mut flat);
            decide(&mut shock);
        }
        let gap = shock.households[0].expected_inflation - flat.households[0].expected_inflation;
        assert!(gap > 0.012, "gap {gap}");
        assert!((shock.consumption.pump_comp - gap).abs() < 1e-9, "z {} gap {gap}", shock.consumption.pump_comp);
        assert_eq!(flat.consumption.pump_comp, 0.0);
    }

    /// After a pump-price surge the weight-0 world has a higher real rate and so a lower discretionary multiplier than the weight-1 world;
    /// with a flat pump price the two are bit-identical (the component is zero).
    #[test]
    fn pump_weight_zero_lowers_the_discretionary_multiplier_only_after_a_pump_surge() {
        let setup = |wgt: f64, surge: bool| {
            let mut w = base_world(9);
            w.cfg.pump_expectations = true;
            w.cfg.hh_pump_real_rate_weight = wgt;
            let p = common::gasoline_price(&w);
            let mut h = vec![p; 12];
            h.push(if surge { 1.5 * p } else { p });
            w.firms_state.pump_hist = h;
            w
        };
        let run = |mut w: World| {
            for _ in 0..15 {
                decide(&mut w);
            }
            w.consumption.discretionary_mult_mean
        };
        let (d1, d0) = (run(setup(1.0, true)), run(setup(0.0, true)));
        assert!(d0 < d1 - 1e-4, "weight 0 {d0} weight 1 {d1}");
        let (f1, f0) = (run(setup(1.0, false)), run(setup(0.0, false)));
        assert_eq!(f1.to_bits(), f0.to_bits());
        // half weight sits between
        let dh = run(setup(0.5, true));
        assert!(dh < d1 && dh > d0, "half {dh} between {d0} and {d1}");
    }

    /// The pump component is kept when only the firm weight is below 1 (the firms' user cost reads it), and the households' real rate is then
    /// unchanged (its own weight is 1): the discretionary multiplier equals the one of the all-ones world bit for bit.
    #[test]
    fn pump_state_is_kept_when_only_the_firm_weight_is_below_one() {
        let setup = |firm_w: f64| {
            let mut w = base_world(11);
            w.cfg.pump_expectations = true;
            w.cfg.firm_pump_user_cost_weight = firm_w;
            let p = common::gasoline_price(&w);
            let mut h = vec![p; 12];
            h.push(1.5 * p);
            w.firms_state.pump_hist = h;
            w
        };
        let mut off = setup(1.0);
        let mut on = setup(0.0);
        for _ in 0..10 {
            decide(&mut off);
            decide(&mut on);
        }
        assert_eq!(off.consumption.pump_comp, 0.0);
        assert!(on.consumption.pump_comp > 0.005, "z {}", on.consumption.pump_comp);
        assert_eq!(off.consumption.discretionary_mult_mean.to_bits(), on.consumption.discretionary_mult_mean.to_bits());
    }

    /// The term is χ·L·tanh(x/L): bounded by χ·L at large gaps. The stored expectation stays inside its clamp.
    #[test]
    fn hh_policy_term_is_bounded() {
        let t = crate::firms::policy_expectation_term(0.16, 0.03, 10.0);
        assert!((t - 0.16 * 0.03).abs() < 1e-12, "term {t}");
        let mut w = tiny_world(Config { n_households: 500, hh_mp_expect: 0.16, ..Default::default() }, 11);
        w.shocks.policy_shift = Some((0.10, 0, 0.9));
        for _ in 0..12 {
            crate::sim::step(&mut w);
        }
        assert!(w.households.iter().all(|h| h.expected_inflation >= EXP_INFL_MIN && h.expected_inflation <= EXP_INFL_MAX));
    }
}

// b7-transmission M-R tests.
#[cfg(test)]
mod saturating_rate_tests {
    use super::saturating_rate_gap;

    #[test]
    fn identity_when_off_or_above_the_knee() {
        assert_eq!(saturating_rate_gap(-0.7, 0.0, 0.0, 0.5, 1.0), -0.7);
        assert_eq!(saturating_rate_gap(0.3, 0.0, 0.01, 0.5, 1.0), 0.3);
        assert_eq!(saturating_rate_gap(0.005, 0.005, 0.01, 0.0, 0.0), 0.005);
    }

    #[test]
    fn continuous_monotone_and_bounded_below() {
        let (k, l, gamma, slack) = (0.005, 0.01, 0.5, 0.0);
        let l_eff = l;
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=400 {
            let g = -0.05 + 0.0002 * i as f64; // −5pp to +3pp
            let h = saturating_rate_gap(g, k, l, gamma, slack);
            assert!(h >= prev - 1e-15, "not monotone at g {g}");
            assert!(h >= k - l_eff - 1e-12, "below the floor at g {g}: {h}");
            prev = h;
        }
        // Continuous at the knee (value and slope).
        let at = saturating_rate_gap(k, k, l, gamma, slack);
        assert!((at - k).abs() < 1e-15);
        let eps = 1e-7;
        let slope = (saturating_rate_gap(k + eps, k, l, gamma, slack) - saturating_rate_gap(k - eps, k, l, gamma, slack))
            / (2.0 * eps);
        assert!((slope - 1.0).abs() < 1e-6, "slope {slope}");
    }

    #[test]
    fn less_scale_with_more_slack() {
        let (k, l, gamma) = (0.005, 0.01, 0.5);
        let deep = -0.5;
        let calm = saturating_rate_gap(deep, k, l, gamma, 0.0);
        let slacked = saturating_rate_gap(deep, k, l, gamma, 0.02); // 2pp of slack
        assert!(slacked > calm, "slack should narrow the saturation: {slacked} vs {calm}");
    }
}
