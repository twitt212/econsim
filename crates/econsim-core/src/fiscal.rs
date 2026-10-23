//! Government (docs/DESIGN.md §9): transfers, purchases, taxes, debt financing.
//!
//! Tick order (§3): `pay_transfers` runs after wages are paid, `plan_spending` before goods clearing and
//! `collect_and_finance` after banking. Every money movement goes through `crate::ledger`.

use rand::Rng;
use rayon::prelude::*;

use crate::common;
use crate::ledger::{self, Account};
use crate::rng::{self, stream};
use crate::types::*;

/// Share of pension income that is subject to income tax (§9.1).
const PENSION_TAXABLE: f64 = 0.85;
/// Earliest age at which a head may claim Social Security (fix-fiscal).
const SS_CLAIM_MIN_AGE: f64 = 62.0;
/// Key prefix for the claiming draws on `stream::FISCAL` (fix-fiscal), so they do not share keys with other draws.
const SS_CLAIM_KEY: u64 = 1;
/// Key prefix for the claim-age walk that seeds the claimed stock at the first reported month (fix-fiscal).
const SS_SEED_KEY: u64 = 2;

/// Monthly probability that an unclaimed head of the given age claims (fix-fiscal). Annual conditional claiming rates
/// follow SSA evidence: about 25% claim at 62, a steady flow at 63-66, a spike at the full retirement age (67), a
/// smaller flow at 68-69, and everyone unclaimed claims at 70. A monthly hazard h gives annual rate 1-(1-h)^12.
fn claim_hazard_monthly(age: f64) -> f64 {
    let annual: f64 = match age.floor() as i64 {
        62 => 0.25,
        63..=66 => 0.07,
        67 => 0.35,
        68 | 69 => 0.20,
        _ => 1.0,
    };
    if annual >= 1.0 {
        1.0
    } else {
        1.0 - (1.0 - annual).powf(1.0_f64 / 12.0)
    }
}

/// Benefit at claim relative to the PIA, by age at claim (fix-fiscal): SSA actuarial reduction of 5/9 of 1% per month
/// before the full retirement age (67) down to 70%, and delayed retirement credits of 8% a year after it, to 124% at 70.
pub(crate) fn claim_age_factor(age: f64) -> f64 {
    match age.floor() as i64 {
        i64::MIN..=62 => 0.70,
        63 => 0.75,
        64 => 0.80,
        65 => 0.8667,
        66 => 0.9333,
        67 => 1.0,
        68 => 1.08,
        69 => 1.16,
        _ => 1.24,
    }
}
/// Claim age (years) of a head that has claimed by `age_months`, from a walk of the monthly hazard from 62 to the
/// current age, one draw per month on the keyed `stream::FISCAL` stream (fix-fiscal). Burn-in freezes ages, so the
/// monthly draws are not repeated there; at the first reported month the walk gives the claimed stock as if the head
/// had been drawn every month since 62. Returns `None` if the head has not claimed.
pub(crate) fn ss_seed_claim_age(seed: u64, tick: u32, hh_id: u64, age_months: u32) -> Option<f64> {
    let start = (SS_CLAIM_MIN_AGE * 12.0) as u32;
    let mut r = rng::agent_rng(seed, tick, stream::FISCAL, (SS_SEED_KEY << 40) | hh_id);
    for m in start..=age_months {
        let age = m as f64 / 12.0;
        let h = claim_hazard_monthly(age);
        if h > 0.0 && r.gen::<f64>() < h {
            return Some(age);
        }
    }
    None
}

/// Trend nominal GDP: share of the previous trend kept each month, and the trend growth rate (annual).
/// Monthly re-anchoring of trend GDP toward actual (half-life ≈ 2 years).
const TREND_CORRECTION: f64 = 0.03;
/// Health (Medicare + Medicaid) share of government purchases at t=0 (≈6% of GDP). Health is mandatory spending: it
/// scales with the number of households headed by someone aged 65+ and is outside the debt rule and shock multiplier.
/// Metrics report it as personal consumption in kind, as the NIPAs do.
pub const HEALTH_SHARE: f64 = 0.27;
/// Health cost growth per year above the growth in older households (CBO: excess cost growth in Medicare ≈0.8%/yr).
/// Simplification (fix-fiscal): Medicare (`medicare_share`) is tied to households headed by someone aged 65+, and Medicaid
/// to all alive households.
pub const EXCESS_HEALTH_COST_GROWTH: f64 = 0.008;
/// Purchases response per unit of debt/GDP above target.
const FISCAL_DEBT_RESPONSE: f64 = 0.25;
/// Income-tax rate response per unit of debt/GDP above target.
const TAX_DEBT_RESPONSE: f64 = 0.075;
/// Smoothing weight on this month's outlays in the average used for the TGA target.
const OUTLAY_SMOOTHING: f64 = 0.1;
/// TGA target = this many months of average outlays, floored at a share of monthly GDP (§9.4).
const TGA_OUTLAY_MONTHS: f64 = 0.5;
const TGA_GDP_FLOOR: f64 = 0.01;

/// Households and firms per fixed parallel chunk in transfer and tax passes (fixed, so results are thread-count
/// independent).
const FISCAL_CHUNK: usize = 1024;

/// Government module state. Money lives in the ledger; these are bookkeeping extras.
#[derive(Clone, Debug, Default)]
pub struct FiscalState {
    /// Multiplier on government purchases set by `init` so the t=0 economy is a demand–supply steady state
    /// (0 = not calibrated, treated as 1).
    pub purchases_calibration: f64,
    /// Smoothed trend nominal GDP per month (0 until the first `plan_spending`).
    pub trend_gdp: f64,
    /// Smoothed monthly government outlays used for the TGA target.
    pub outlays_avg: f64,
    pub outlays_init: bool,
    /// TGA target set by the last `collect_and_finance`.
    pub tga_target: f64,
    /// Year-to-date gross wages per household (payroll-tax cap); reset each January.
    pub ytd_wages: Vec<f64>,
    /// Unpaid household tax liabilities carried forward, per household.
    pub hh_arrears: Vec<f64>,
    /// Unpaid bank corporate tax carried forward, per bank.
    pub bank_arrears: Vec<f64>,
    /// Corporate-tax loss carry-forward per firm slot (a dead slot keeps it; `firms::reborn` clears it for the entrant
    /// when `entrant_loss_cf_reset` is on).
    pub firm_loss_cf: Vec<f64>,
    /// Bank corporate tax paid this month (model units); observation only, for after-tax bank net income.
    pub bank_tax_month: f64,
    /// b5-udur M5: per-household UI extension state (0 not unemployed, 1 benefit spell, 2 extended). Updated only
    /// when `udur_ui_ext_t1` < 99.
    pub ui_ext: Vec<u8>,
    /// Average wage index base: mean monthly wage of employed households at the first month of the world (0 until set).
    pub wage_base: f64,
    /// Running sum and count of the monthly mean employed wage since the last January update (12-month average).
    pub wage_sum: f64,
    pub wage_n: u32,
    /// Health spending per household aged 65+ at the first reported month (set once; 0 until then).
    pub health_per_older_hh: f64,
    /// Trend nominal GDP per household at the first reported month (0 until set).
    pub gdp_per_hh_base: f64,
    /// True once the reported-run base for health spending has been captured.
    pub health_base_set: bool,
    /// Planned health and total government purchases this month (model units, before realisation).
    pub health_month: f64,
    pub purchases_month: f64,
    /// Pension payments this month (model units), for the social-security share of GDP.
    pub pension_month: f64,
    // fix-fiscal
    /// Debt/GDP moving average used by the fiscal rules, and whether it has been initialised.
    pub debt_gdp_avg: f64,
    pub debt_avg_init: bool,
    /// Per household: Fund shares at the last realisation pass, cost basis of those shares (model $) and capital
    /// losses carried forward.
    pub fund_shares_prev: Vec<f64>,
    pub fund_basis: Vec<f64>,
    pub cg_loss_cf: Vec<f64>,
    /// Capital-gains tax assessed this month (model units).
    pub capgains_month: f64,
    /// Medicaid spending per household (all alive households) at the first reported month (fix-fiscal).
    pub medicaid_per_hh: f64,
    /// True once the claimed stock has been seeded at the first reported month (fix-fiscal).
    pub ss_seeded: bool,
    /// Per slot: birth-month key (calendar month minus age) of the household that last occupied it, and whether the
    /// keys have been set. A slot reused by an entrant in the same tick (demography adds entrants into dead slots)
    /// changes its key, so the entrant does not inherit the previous occupant's shares, basis, loss carry-forward or
    /// arrears (fix-fiscal).
    pub occupant_key: Vec<i64>,
    pub occupant_init: bool,
}

#[inline]
fn nonneg(x: f64) -> f64 {
    if x.is_finite() && x > 0.0 {
        x
    } else {
        0.0
    }
}

fn ensure_len<T: Clone>(v: &mut Vec<T>, n: usize, fill: T) {
    if v.len() < n {
        v.resize(n, fill);
    }
}

/// Dollar parameters in the current nominal units (round 7 step 1). `min_wage`, `ui_cap`, `asset_scale` and
/// `entry_scale` are multiplied into the base-year values; the legacy path returns the unscaled values.
#[derive(Clone, Copy, Debug)]
pub struct Nominal {
    pub min_wage: f64,
    pub ui_cap: f64,
    pub asset_scale: f64,
    pub entry_scale: f64,
}

/// Nominal dollar parameters for this month. The burn-in scale is `gov.nominal_scale` (the wage index, set each
/// January of the frozen burn-in and at its anchor); the UI cap follows the wage index throughout (SAWW rule).
pub fn nominal(w: &World) -> Nominal {
    let c = &w.cfg;
    let s = if c.burn_in_nominal_neutral && w.gov.nominal_scale > 0.0 { w.gov.nominal_scale } else { 1.0 };
    let wi = (w.prices.wage_index / 100.0).max(1e-9);
    Nominal {
        min_wage: c.min_wage_monthly * s,
        ui_cap: c.benefit_cap * if c.ui_cap_wage_indexed { wi } else { s },
        asset_scale: s,
        entry_scale: if c.entry_wage_indexed { wi } else { 1.0 },
    }
}

impl Nominal {
    /// Unscaled values (every factor 1.0), as at t=0.
    pub fn base(c: &crate::config::Config) -> Self {
        Nominal { min_wage: c.min_wage_monthly, ui_cap: c.benefit_cap, asset_scale: 1.0, entry_scale: 1.0 }
    }
}

/// Trend nominal GDP per month (§9.2), used for government purchases and the TGA floor.
/// Falls back to `common::gdp_month_model` before `plan_spending` has run.
pub fn trend_gdp_month(w: &World) -> f64 {
    if w.fiscal.trend_gdp > 0.0 {
        w.fiscal.trend_gdp
    } else {
        nonneg(common::gdp_month_model(w))
    }
}

/// Unemployment benefits, pensions (with COLA), means-tested support, helicopter money.
///
/// b5-udur M5: extended UI months for this month from u3, the mean of the last ≤3 unemployment rates (0 if none).
fn ui_extension_months(w: &World) -> u32 {
    let h = &w.history;
    let n = h.len().min(3);
    let u3 = if n == 0 { 0.0 } else { h[h.len() - n..].iter().map(|m| m.unemployment_rate).sum::<f64>() / n as f64 };
    let c = &w.cfg;
    let m = if u3 >= c.udur_ui_ext_t2 {
        c.udur_ui_ext_m2
    } else if u3 >= c.udur_ui_ext_t1 {
        c.udur_ui_ext_m1
    } else {
        0.0
    };
    m.max(0.0).round() as u32
}

/// Government payments use `ledger::pay(Government → Household)`; the TGA may go negative until
/// `collect_and_finance` restores it. Helicopter money is paid by the central bank.
pub fn pay_transfers(w: &mut World) {
    update_wage_index(w);
    let tick = w.tick;
    // COLA (§9.3): each January the index and all pensions rise with trailing CPI inflation (floored at 0).
    // Round 7 step 1 (`burn_in_nominal_neutral`): the frozen burn-in and its anchor month track the wage index, so
    // pensions and brackets keep their 2026 relation to wages; the reported run uses CPI as in US law. The calendar
    // is absolute (tick + tick_offset), so the COLA due at the anchor January is applied.
    let abs = w.tick + w.tick_offset;
    let frozen_now = w.tick_offset == 0 && w.tick < w.cfg.burn_in_months;
    let anchor_month = w.tick_offset > 0 && w.tick == 0;
    let nn = w.cfg.burn_in_nominal_neutral;
    let january = if nn { abs % 12 == 0 && abs > 0 } else { tick % 12 == 0 && tick > 0 };
    let wage_track = nn && (frozen_now || anchor_month);
    let cola_factor = if !january {
        1.0
    } else if wage_track {
        let f = (w.prices.wage_index / 100.0) / w.gov.cola_index.max(1e-12);
        if f.is_finite() && f > 0.0 {
            f
        } else {
            1.0
        }
    } else {
        let f = 1.0 + w.inflation_yoy().max(0.0);
        if f.is_finite() {
            f
        } else {
            1.0
        }
    };
    w.gov.cola_index *= cola_factor;
    if wage_track && january {
        w.gov.nominal_scale = w.gov.cola_index;
    }
    let cola = w.gov.cola_index;
    let nom = nominal(w);
    // Round 7 step 2: dollar means-tested parameters on the wage index (AWI) unless `wage_indexed_thresholds` is off.
    let mt_index = if w.cfg.wage_indexed_thresholds { (w.prices.wage_index / 100.0).max(1e-9) } else { cola };
    let wage_index = w.prices.wage_index;
    let repl = w.cfg.benefit_replacement;
    let cap = nom.ui_cap;
    let price = w.fund.share_price;
    let seed = w.seed;
    let cfg = &w.cfg;
    // Claims are not drawn during burn-in, when ages are frozen (a monthly draw would repeat the same age). The first
    // reported month seeds the claimed stock by walking the hazard from 62 to each head's age (fix-fiscal).
    let frozen = w.tick_offset == 0 && w.tick < cfg.burn_in_months;
    let seed_claims = !frozen && !w.fiscal.ss_seeded;
    if !frozen {
        w.fiscal.ss_seeded = true;
    }
    let heli = nonneg(w.cb.helicopter_per_household);
    // RZ23: the Treasury-financed one-off payment of a `Rebate` shock (0 unless one is in force): credited as the helicopter payment is,
    // paid out of the TGA with the government's other payments, and refilled by the bond auction of `collect_and_finance`.
    let reb = nonneg(w.shocks.rebate_per_household(tick));
    let nb = w.banks.len();
    // b5-udur M5: extended UI months for this month (0 when off), from the last ≤3 monthly unemployment rates.
    let ext_on = w.cfg.udur_ui_ext_t1 < 99.0;
    let ext = if ext_on { ui_extension_months(w) } else { 0 };
    let bm = w.cfg.benefit_months;
    if ext_on && w.fiscal.ui_ext.len() < w.households.len() {
        w.fiscal.ui_ext.resize(w.households.len(), 0);
    }
    let mut off_state: Vec<u8> = if ext_on { Vec::new() } else { vec![0u8; w.households.len()] };
    let uis: &mut [u8] = if ext_on { &mut w.fiscal.ui_ext[..] } else { &mut off_state[..] };

    // One parallel pass over fixed household chunks. Each household's government transfer and helicopter money
    // are credited to its own deposits and income in place. Neither payer can run short (the TGA is restored
    // later and the central bank creates reserves), so the TGA, bank and totals are reduced per chunk in chunk order.
    let parts: Vec<(Vec<f64>, f64, f64, u32, f64)> = w
        .households
        .par_chunks_mut(FISCAL_CHUNK)
        .zip(w.hh_month.par_chunks_mut(FISCAL_CHUNK))
        .zip(uis.par_chunks_mut(FISCAL_CHUNK))
        .map(|((hhs, mons), uis_c)| {
            let mut bank = vec![0.0; nb];
            let mut gov_sum = 0.0;
            let mut heli_sum = 0.0;
            let mut reb_sum = 0.0;
            let mut ext_n = 0u32;
            for ((hh, mon), uist) in hhs.iter_mut().zip(mons.iter_mut()).zip(uis_c.iter_mut()) {
                if !hh.alive {
                    // b5-udur M5: a dead slot's extension state must not reach a replacement entrant.
                    *uist = 0;
                    continue;
                }
                let mut total = 0.0;
                mon.income.unemployment_benefit = 0.0;
                mon.income.pension = 0.0;
                if ext_on {
                    // b5-udur M5 steps 1–3: state update, and the reachback of extended months to exhausted spells.
                    // A household that has never worked (an entrant placed in a reused slot) has no UI claim. Test on
                    // last_wage, not months_employed: months_employed resets to 0 at every separation (common.rs), so it
                    // is 0 for every unemployed household. last_wage is kept from the last job and is 0 only for entrants.
                    if hh.status != LaborStatus::Unemployed || hh.last_wage <= 0.0 {
                        *uist = 0;
                    }
                    if hh.benefit_months_left > 0 && *uist == 0 {
                        *uist = 1;
                    }
                    let mu = u32::from(hh.months_unemployed);
                    if hh.benefit_months_left == 0 && *uist == 1 && ext > 0 && mu < bm + ext {
                        hh.benefit_months_left = (bm + ext - mu) as u16;
                        *uist = 2;
                    }
                }
                // Unemployment insurance (§6): min(replacement × last wage, cap) for benefit_months.
                let mut paid_ui = false;
                if hh.status == LaborStatus::Unemployed && hh.benefit_months_left > 0 {
                    let b = nonneg(repl * hh.last_wage).min(cap);
                    hh.benefit_months_left -= 1;
                    mon.income.unemployment_benefit = b;
                    total += b;
                    paid_ui = true;
                }
                if ext_on {
                    // Step 6 (counted at payment: the state the month was paid in, so the last standard month is not an
                    // extension month), then step 5: a spell that is paid down to zero this month, with an extension
                    // available, starts it. Gated on paid_ui so a spell already past benefit_months + ext (reachback
                    // skipped it) never receives a full extension here (b6-reentry fix of the b5-udur review finding).
                    if paid_ui && *uist == 2 {
                        ext_n += 1;
                    }
                    if paid_ui && hh.benefit_months_left == 0 && *uist == 1 && ext > 0 {
                        hh.benefit_months_left = ext as u16;
                        *uist = 2;
                    }
                }
                // Social security (§9.3, fix-fiscal): not tied to labour-force exit. A claimed head (pension > 0) is paid
                // the COLA-indexed benefit whatever its status. An unclaimed head aged 62+ claims with the age-based
                // monthly hazard; the benefit is the PIA times the claim-age factor. A zero PIA stays unclaimed.
                if hh.pension > 0.0 {
                    hh.pension *= cola_factor;
                } else if !frozen && hh.age_years() >= SS_CLAIM_MIN_AGE {
                    if seed_claims {
                        if let Some(age) = ss_seed_claim_age(seed, tick, hh.id as u64, u32::from(hh.age_months)) {
                            let pia =
                                nonneg(common::social_security_pia(hh.career_earnings, hh.career_months.into(), wage_index));
                            hh.pension = pia * claim_age_factor(age);
                        }
                    } else if hh.status == LaborStatus::Retired {
                        // Retirees aged 62+ claim at once (fix-fiscal integration, reviewer fix), at the age factor.
                        let pia = nonneg(common::social_security_pia(hh.career_earnings, hh.career_months.into(), wage_index));
                        hh.pension = pia * claim_age_factor(hh.age_years());
                    } else {
                        let h = claim_hazard_monthly(hh.age_years());
                        if h > 0.0 {
                            let mut r = rng::agent_rng(seed, tick, stream::FISCAL, (SS_CLAIM_KEY << 40) | hh.id as u64);
                            if r.gen::<f64>() < h {
                                let pia =
                                    nonneg(common::social_security_pia(hh.career_earnings, hh.career_months.into(), wage_index));
                                hh.pension = pia * claim_age_factor(hh.age_years());
                            }
                        }
                    }
                }
                mon.income.pension = hh.pension;
                total += hh.pension;
                // Means-tested support (fix-fiscal): asset-tested, net of UI and pensions, so UI is paid first.
                let liquid = hh.deposits + hh.shares * price;
                let s = means_tested_support(cfg, mt_index, nom.asset_scale, hh, &mon.income, liquid);
                if s > 0.0 {
                    mon.income.transfers += s;
                    total += s;
                }
                // Government → household (`ledger::pay`): deposits and the bank's deposits and reserves rise.
                let amt = nonneg(total);
                if amt > 0.0 {
                    hh.deposits += amt;
                    bank[hh.bank as usize] += amt;
                    gov_sum += amt;
                }
                // Helicopter money (§9.3, §16): created by the central bank, not drawn from the TGA.
                if heli > 0.0 {
                    hh.deposits += heli;
                    bank[hh.bank as usize] += heli;
                    mon.income.windfall += heli; // fix-households: transitory windfall, not permanent income
                    heli_sum += heli;
                }
                // RZ23: a Treasury-financed one-off payment is credited exactly as the helicopter payment is (so the first month is the
                // same to the last bit), is a transitory windfall, and is counted with the government's payments after the loop: it
                // comes out of the TGA, not from the central bank.
                if reb > 0.0 {
                    hh.deposits += reb;
                    bank[hh.bank as usize] += reb;
                    mon.income.windfall += reb;
                    reb_sum += reb;
                }
            }
            (bank, gov_sum, heli_sum, ext_n, reb_sum)
        })
        .collect();

    let mut bank_net = vec![0.0; nb];
    let mut gov_paid = 0.0;
    let mut heli_total = 0.0;
    let mut reb_total = 0.0;
    let mut ui_ext_n = 0u32;
    for (b, g, h, e, r) in &parts {
        for (a, x) in bank_net.iter_mut().zip(b) {
            *a += x;
        }
        gov_paid += g;
        heli_total += h;
        reb_total += r;
        ui_ext_n += e;
    }
    w.flows.udur_ui_ext_n += ui_ext_n;
    apply_bank_net(w, &bank_net);
    gov_paid += reb_total; // RZ23: the rebate is a government payment (zero unless a `Rebate` shock is in force)
    w.cb.tga -= gov_paid;
    w.gov.transfers += gov_paid;
    w.gov.outlays += gov_paid;
    w.fiscal.pension_month = w
        .households
        .iter()
        .zip(w.hh_month.iter())
        .filter(|(h, _)| h.alive)
        .map(|(_, m)| m.income.pension)
        .sum();

    if heli > 0.0 {
        w.cb.helicopter_total += heli_total;
        w.flows.helicopter += heli_total;
    }
    // RZ23: the Treasury-financed payment is reported in the one-off-payment series (`helicopter_bn_ar`) so that a response per dollar can
    // be read as for the helicopter, but it is not central-bank money: `cb.helicopter_total` is not touched.
    if reb > 0.0 {
        w.flows.helicopter += reb_total;
    }
}

/// Means-tested support for one household this month, in model $ (fix-fiscal). Aged households (retired or 65+) get an
/// SSI-like floor: the monthly maximum less UI, pension and wage income, paid only while liquid wealth (deposits plus
/// Fund value) is below the asset limit. Working-age households get a SNAP-like benefit (the maximum less
/// `snap_phaseout` of net income, scaled down for students and those living with parents, zero above the asset limit)
/// and an EITC-like credit on wages.
///
/// Round 7 steps 1–2: dollar maxima and EITC thresholds are base-year $ indexed by `index`, which is the wage index
/// (AWI) as a proxy that keeps caseload shares stable, not the CPI as in US law. Benefits per recipient stay at US
/// maxima × index. The SSI asset limit is not indexed (US law) and is scaled only by `asset_scale`, the nominal scale
/// of the burn-in. SNAP has no asset test under broad-based categorical eligibility (about 44 states), so
/// `snap_asset_limit` and `snap_asset_end` default to a value no household reaches.
fn means_tested_support(
    cfg: &crate::config::Config,
    index: f64,
    asset_scale: f64,
    hh: &Household,
    inc: &IncomeMonth,
    liquid: f64,
) -> f64 {
    let couple = hh.partnered;
    let wage = nonneg(inc.wage);
    let unearned = nonneg(inc.unemployment_benefit) + nonneg(inc.pension);
    if hh.status == LaborStatus::Retired || hh.age_years() >= 65.0 {
        if liquid >= cfg.ssi_asset_limit * asset_scale {
            return 0.0;
        }
        let max = if couple { cfg.ssi_max_couple } else { cfg.ssi_max_single } * index;
        return nonneg(max - unearned - wage);
    }
    let mut s = 0.0;
    // Asset phase-out: full benefit up to `snap_asset_limit`, linear to zero at `snap_asset_end`.
    let (a_lim, a_end) = (cfg.snap_asset_limit * asset_scale, cfg.snap_asset_end * asset_scale);
    let span = (a_end - a_lim).max(1e-9);
    let asset_factor = ((a_end - liquid) / span).clamp(0.0, 1.0);
    if asset_factor > 0.0 {
        let max = if couple { cfg.snap_max_couple } else { cfg.snap_max_single } * index;
        let discount = if hh.status == LaborStatus::Student || hh.tenure == Tenure::WithParents {
            cfg.snap_student_discount
        } else {
            1.0
        };
        s += asset_factor * discount * nonneg(max - cfg.snap_phaseout * (wage + unearned));
    }
    let plateau = cfg.eitc_plateau * index;
    let start = cfg.eitc_phaseout_start * index;
    let credit = cfg.eitc_rate * wage.min(plateau) - cfg.eitc_phaseout_rate * (wage - start).max(0.0);
    s + nonneg(credit)
}

/// Average wage index (§9.3, the AWI of the SSA benefit formula): 100 at t=0. Each January it is set to 100 × (mean
/// monthly wage of employed households over the previous 12 months) ÷ (the same mean in the first month of the world).
/// The monthly means are summed in household order, so the result does not depend on the thread count.
fn update_wage_index(w: &mut World) {
    let (mut sum, mut n) = (0.0f64, 0u32);
    for h in w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed) {
        sum += h.wage;
        n += 1;
    }
    if n == 0 {
        return;
    }
    let mean = sum / f64::from(n);
    if w.fiscal.wage_base <= 0.0 {
        w.fiscal.wage_base = mean;
    }
    if (w.tick + w.tick_offset) % 12 == 0 && w.fiscal.wage_n > 0 {
        let idx = 100.0 * (w.fiscal.wage_sum / f64::from(w.fiscal.wage_n)) / w.fiscal.wage_base;
        if idx.is_finite() && idx > 0.0 {
            w.prices.wage_index = idx;
        }
        w.fiscal.wage_sum = 0.0;
        w.fiscal.wage_n = 0;
    }
    w.fiscal.wage_sum += mean;
    w.fiscal.wage_n += 1;
}

/// Months since the start of the reported run (negative during burn-in), on the calendar used by demography.
fn reported_months(w: &World) -> i64 {
    i64::from(w.tick) + i64::from(w.tick_offset) - i64::from(w.cfg.burn_in_months)
}

/// Add `net[b]` to the deposits and reserves of bank `b` (the bank side of payments whose amounts were already
/// applied to the payers and payees in place).
pub(crate) fn apply_bank_net(w: &mut World, net: &[f64]) {
    for (b, &x) in net.iter().enumerate() {
        if x != 0.0 {
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
    }
}

/// Debt-stabilising tax rule: income-tax rates rise 0.75pp per 10pp of debt/GDP above target at strength 1 (0.375pp
/// at the default strength 0.5), and fall below it, bounded to [−4pp, +5pp]. With the purchases rule this keeps
/// debt/GDP from drifting (Bohn 1998 fiscal reaction).
pub fn tax_rule_add(w: &World) -> f64 {
    let trend = w.fiscal.trend_gdp;
    if trend <= 0.0 {
        return 0.0;
    }
    (w.cfg.fiscal_rule_strength * TAX_DEBT_RESPONSE * debt_rule_gap(w)).clamp(-0.04, 0.05)
}

/// Gap the fiscal rules respond to (round 7 step 3): smoothed debt/GDP less the target path. The target is
/// `initial_debt_gdp` during burn-in and rises by `debt_target_slope` a year of the reported run (CBO current-law path).
pub fn debt_rule_gap(w: &World) -> f64 {
    let years = reported_months(w).max(0) as f64 / 12.0;
    smoothed_debt_gdp(w) - (w.cfg.initial_debt_gdp + w.cfg.debt_target_slope * years)
}

/// Debt/GDP used by the fiscal rules (fix-fiscal): a moving average over `fiscal_rule_avg_months`, so the rules do not
/// tighten within a downturn. Before the first `plan_spending` it is the current ratio.
pub fn smoothed_debt_gdp(w: &World) -> f64 {
    if w.fiscal.debt_avg_init {
        w.fiscal.debt_gdp_avg
    } else {
        w.gov.debt_face / (12.0 * w.fiscal.trend_gdp).max(1e-9)
    }
}

/// Government purchase orders into `w.orders.government` (§9.2): health (ageing-driven, `health_spending`) plus other
/// purchases = trend nominal GDP × share × shock multiplier × debt rule.
pub fn plan_spending(w: &mut World) {
    // Trend GDP (§9.2): trend ← 0.97 × trend × (1 + g/12) + 0.03 × GDP, initialised to GDP on first call, where g is
    // the nominal trend growth from configuration (`Config::nominal_trend_growth`).
    let gdp = nonneg(common::gdp_month_model(w));
    let g_trend = w.cfg.nominal_trend_growth();
    let f = &mut w.fiscal;
    f.trend_gdp = if f.trend_gdp > 0.0 {
        // Acyclical: budgets grow with trend nominal GDP and only slowly re-anchor to actual GDP.
        f.trend_gdp * (1.0 + g_trend / 12.0) + TREND_CORRECTION * (gdp - f.trend_gdp)
    } else {
        gdp
    };

    let mult = w.shocks.gov_spending_mult(w.tick);
    // Debt-stabilising fiscal rule (cf. the stability-pact rules in Dosi et al. 2010): purchases shade down by
    // 2.5% per 10pp of debt/GDP above target at strength 1 (1.25% at the default), clamped to [0.75, 1.3]. It applies
    // to non-health purchases only.
    let current = w.gov.debt_face / (12.0 * w.fiscal.trend_gdp).max(1e-9);
    {
        let avg_months = w.cfg.fiscal_rule_avg_months.max(1.0);
        let f = &mut w.fiscal;
        if f.debt_avg_init {
            f.debt_gdp_avg += (current - f.debt_gdp_avg) / avg_months;
        } else {
            f.debt_gdp_avg = current;
            f.debt_avg_init = true;
        }
    }
    let rule = (1.0 - w.cfg.fiscal_rule_strength * FISCAL_DEBT_RESPONSE * debt_rule_gap(w)).clamp(0.75, 1.3);
    let calib = if w.fiscal.purchases_calibration > 0.0 { w.fiscal.purchases_calibration } else { 1.0 };
    let base = w.cfg.gov_purchases_share * calib * w.fiscal.trend_gdp;
    let health = health_spending(w, base);
    let other = nonneg((1.0 - HEALTH_SHARE) * base * mult * rule);
    let purchases = health + other;
    w.fiscal.health_month = health;
    w.fiscal.purchases_month = purchases;
    // `gov_purchase_split` covers total purchases, health included. Health goes to `health_sector_mix`; other
    // purchases take the non-health remainder, (split − HEALTH_SHARE × health mix) / (1 − HEALTH_SHARE), so the t=0
    // sector orders equal the split of total purchases (fix-fiscal).
    let split = w.cfg.gov_purchase_split;
    let hmix = w.cfg.health_sector_mix;
    for s in 0..N_SECTORS {
        let other_split = (split[s] - HEALTH_SHARE * hmix[s]) / (1.0 - HEALTH_SHARE);
        // A negative value would mean the health sector mix exceeds the split (a config error); clamp it so the
        // non-health orders never go negative. Debug builds flag it.
        debug_assert!(other_split >= 0.0, "gov_purchase_split below the health sector mix in sector {s}");
        w.orders.government[s] += other * other_split.max(0.0) + health * hmix[s];
    }
}

/// Planned health purchases this month (model units). At the first reported month the base is captured and split
/// into Medicare, spent per household aged 65+, and Medicaid, spent per alive household (fix-fiscal). Month 0 equals
/// `HEALTH_SHARE × base`, the t=0 calibration. After that each part grows with trend GDP per household (relative to its
/// t=0 value) and with (1 + EXCESS_HEALTH_COST_GROWTH)^(years since the start of the reported run). Burn-in
/// (demography frozen) uses the t=0 share without ageing.
fn health_spending(w: &mut World, base: f64) -> f64 {
    let (mut n65, mut n_alive) = (0usize, 0usize);
    for h in w.households.iter().filter(|h| h.alive) {
        n_alive += 1;
        if h.age_years() >= 65.0 {
            n65 += 1;
        }
    }
    let trend = w.fiscal.trend_gdp.max(0.0);
    let rep = reported_months(w);
    let medicare_share = w.cfg.medicare_share;
    let f = &mut w.fiscal;
    if !f.health_base_set && rep >= 0 {
        f.health_base_set = true;
        f.health_per_older_hh = if n65 > 0 { medicare_share * HEALTH_SHARE * base / n65 as f64 } else { 0.0 };
        f.medicaid_per_hh = if n_alive > 0 { (1.0 - medicare_share) * HEALTH_SHARE * base / n_alive as f64 } else { 0.0 };
        f.gdp_per_hh_base = if n_alive > 0 { trend / n_alive as f64 } else { 0.0 };
    }
    if !f.health_base_set || f.gdp_per_hh_base <= 0.0 || n_alive == 0 {
        return nonneg(HEALTH_SHARE * base);
    }
    let gdp_per_hh_ratio = (trend / n_alive as f64) / f.gdp_per_hh_base;
    let years = rep.max(0) as f64 / 12.0;
    let growth = (1.0 + EXCESS_HEALTH_COST_GROWTH).powf(years);
    let medicare = f.health_per_older_hh * n65 as f64;
    let medicaid = f.medicaid_per_hh * n_alive as f64;
    nonneg((medicare + medicaid) * gdp_per_hh_ratio * growth)
}

/// Income, payroll and corporate taxes; bond issuance to keep the TGA at target (§9.1, §9.4).
///
/// Household taxes that cannot be paid from deposits accumulate as arrears. Bond issuance is at par
/// (price 1.0); the TGA is restored to its target when it falls below it. Surpluses are not used for buybacks.
pub fn collect_and_finance(w: &mut World) {
    let tick = w.tick;
    let (n_hh, n_firms, n_banks) = (w.households.len(), w.firms.len(), w.banks.len());
    {
        let f = &mut w.fiscal;
        ensure_len(&mut f.ytd_wages, n_hh, 0.0);
        ensure_len(&mut f.hh_arrears, n_hh, 0.0);
        ensure_len(&mut f.bank_arrears, n_banks, 0.0);
        ensure_len(&mut f.firm_loss_cf, n_firms, 0.0);
        if tick % 12 == 0 {
            f.ytd_wages.par_iter_mut().for_each(|x| *x = 0.0);
        }
    }
    let nb = n_banks;
    // Slots whose occupant changed since the last pass (fix-fiscal): their carried state belongs to the previous owner.
    let new_occ = occupant_changes(w);
    for (i, &new) in new_occ.iter().enumerate() {
        if new {
            w.fiscal.hh_arrears[i] = 0.0;
            w.fiscal.ytd_wages[i] = 0.0;
        }
    }
    // Central-bank net income is read before any tax is collected below (fix-fiscal): taxes drain reserves, but
    // interest on reserves was paid earlier, in banking::service, on the larger pre-tax balances.
    let cb_net = central_bank_net_income(w);
    // Capital-gains tax on Fund shares realised this month (fix-fiscal), per household.
    let cgt = realised_gains_tax(w, &new_occ);

    // --- households: tax owed this month and payment from deposits, one parallel pass over fixed chunks ---
    // Each household's year-to-date wage, arrears and deposits change in place. A household pays at most its
    // deposits (as `ledger::pay_up_to`), and the government is unconstrained, so the bank and TGA totals are
    // reduced per chunk in chunk order.
    let add = w.shocks.income_tax_add(tick) + tax_rule_add(w);
    let cola = if w.gov.cola_index > 0.0 { w.gov.cola_index } else { 1.0 };
    let cap = w.cfg.payroll_cap_annual * cola;
    let payroll = w.cfg.payroll_tax;
    let cfg = &w.cfg;
    let dist_prev = &w.consumption.distributions_prev;
    let hh_parts: Vec<(Vec<f64>, f64, f64)> = w
        .households
        .par_chunks_mut(FISCAL_CHUNK)
        .zip(w.fiscal.ytd_wages.par_chunks_mut(FISCAL_CHUNK))
        .zip(w.fiscal.hh_arrears.par_chunks_mut(FISCAL_CHUNK))
        .zip(w.hh_month.par_chunks_mut(FISCAL_CHUNK))
        .enumerate()
        .map(|(c, (((hhs, ytds), arrs), mons))| {
            let mut bank = vec![0.0; nb];
            let mut tga = 0.0;
            let mut paid_sum = 0.0;
            for (k, (((hh, ytd), arr), mon)) in
                hhs.iter_mut().zip(ytds.iter_mut()).zip(arrs.iter_mut()).zip(mons.iter_mut()).enumerate()
            {
                if !hh.alive {
                    *arr = 0.0;
                    continue;
                }
                let wage = nonneg(mon.income.wage);
                // Progressive income tax on annualised taxable income (§9.1); transfers are untaxed.
                let taxable_annual = 12.0
                    * (wage
                        + mon.income.interest.max(0.0)
                        + dist_prev.get(c * FISCAL_CHUNK + k).copied().unwrap_or(0.0)
                        + mon.income.rent_received.max(0.0)
                        + mon.income.unemployment_benefit.max(0.0)
                        + PENSION_TAXABLE * mon.income.pension.max(0.0));
                // No taxable income means no tax (exact: the bracket sum is zero at zero gross income).
                let income_tax = if taxable_annual > 0.0 {
                    nonneg(common::income_tax_annual_indexed(cfg, taxable_annual, add, cola) / 12.0)
                } else {
                    0.0
                };
                // Employee payroll tax on wages up to the annual cap (§9.1).
                let room = (cap - *ytd).max(0.0);
                let payroll_tax = payroll * wage.min(room);
                let cg = cgt.get(c * FISCAL_CHUNK + k).copied().unwrap_or(0.0);
                let owed = income_tax + nonneg(payroll_tax) + cg;
                *ytd += wage;

                let due = owed + *arr;
                // Same as `ledger::pay_up_to(Household → Government)`: pay at most the deposits (never below zero).
                let x = due.min(hh.deposits.max(0.0));
                if x > 0.0 && x.is_finite() {
                    hh.deposits -= x;
                    bank[hh.bank as usize] -= x;
                    tga += x;
                }
                let paid = x.max(0.0);
                *arr = (due - paid).max(0.0);
                mon.taxes += paid;
                paid_sum += paid;
            }
            (bank, tga, paid_sum)
        })
        .collect();
    let mut hh_bank = vec![0.0; nb];
    let mut hh_paid = 0.0;
    let mut tga_in = 0.0;
    for (b, t, p) in &hh_parts {
        for (a, x) in hh_bank.iter_mut().zip(b) {
            *a += x;
        }
        tga_in += t;
        hh_paid += p;
    }
    apply_bank_net(w, &hh_bank);
    w.cb.tga += tga_in;

    // --- firms: corporate tax with loss carry-forward (§16), one parallel pass over fixed chunks ---
    let cit = w.cfg.corporate_tax;
    let firm_parts: Vec<(Vec<f64>, f64, f64)> = w
        .firms
        .par_chunks_mut(FISCAL_CHUNK)
        .zip(w.fiscal.firm_loss_cf.par_chunks_mut(FISCAL_CHUNK))
        .map(|(fms, cfs)| {
            let mut bank = vec![0.0; nb];
            let mut tga = 0.0;
            let mut paid_sum = 0.0;
            for (f, cf_slot) in fms.iter_mut().zip(cfs.iter_mut()) {
                let cf = nonneg(*cf_slot);
                let pretax = f.revenue - f.wage_bill - f.input_cost - f.interest_paid - f.depreciation_cost;
                let (tax, cf_new) = if !f.alive || !pretax.is_finite() {
                    (0.0, cf)
                } else {
                    let (taxable, cf_new) = if pretax >= 0.0 {
                        let used = pretax.min(cf);
                        (pretax - used, cf - used)
                    } else {
                        (0.0, cf - pretax)
                    };
                    (nonneg(cit * taxable), cf_new)
                };
                *cf_slot = cf_new;
                if tax > 0.0 {
                    f.deposits -= tax;
                    bank[f.bank as usize] -= tax;
                    tga += tax;
                    f.taxes = tax;
                    paid_sum += tax;
                }
            }
            (bank, tga, paid_sum)
        })
        .collect();
    let mut firm_bank = vec![0.0; nb];
    let mut firm_paid = 0.0;
    for (b, t, p) in &firm_parts {
        for (a, x) in firm_bank.iter_mut().zip(b) {
            *a += x;
        }
        w.cb.tga += t;
        firm_paid += p;
    }
    apply_bank_net(w, &firm_bank);

    // --- banks: corporate tax on positive estimated profit ---
    let mut bank_paid = 0.0;
    for b in 0..n_banks {
        if !w.banks[b].alive {
            w.fiscal.bank_arrears[b] = 0.0;
            continue;
        }
        // Re-fit Step 4a: the base is the bank P&L after operating expenses (`service_bank_pnl` has run this month).
        // Legacy (`bank_tax_after_opex` false) taxes interest income less interest expense and loan losses.
        let profit = if w.cfg.bank_tax_after_opex {
            w.banks[b].profit
        } else {
            w.banks[b].interest_income - w.banks[b].interest_expense - w.banks[b].loan_losses
        };
        let due = nonneg(cit * profit) + w.fiscal.bank_arrears[b];
        let paid = ledger::pay_up_to(w, Account::Bank(b as BankId), Account::Government, due);
        w.fiscal.bank_arrears[b] = (due - paid).max(0.0);
        bank_paid += paid;
    }
    w.fiscal.bank_tax_month = bank_paid;

    // --- central bank remittance (fix-fiscal): its net income goes to the TGA, as the Fed remits to Treasury ---
    let remit = central_bank_remittance(w, cb_net);
    ledger::pay(w, Account::CentralBank, Account::Government, remit);
    w.gov.revenue += remit;
    w.gov.revenue += hh_paid + firm_paid + bank_paid;
    w.gov.deficit = w.gov.outlays - w.gov.revenue;

    // --- financing (§9.4): keep the TGA at target by auctioning bonds at par ---
    let gdp = nonneg(common::gdp_month_model(w));
    let outlays = nonneg(w.gov.outlays);
    {
        let f = &mut w.fiscal;
        if f.outlays_init {
            f.outlays_avg += OUTLAY_SMOOTHING * (outlays - f.outlays_avg);
        } else {
            f.outlays_avg = outlays;
            f.outlays_init = true;
        }
        f.tga_target = (TGA_OUTLAY_MONTHS * f.outlays_avg).max(TGA_GDP_FLOOR * gdp);
    }
    let need = w.fiscal.tga_target - w.cb.tga;
    if need > 0.0 && need.is_finite() {
        auction_bonds(w, need);
    }
    w.gov.debt_face = w.bonds.iter().map(|b| b.face_outstanding()).sum();
}

/// Whether each slot's occupant changed since the last pass (fix-fiscal). The key is the calendar month of birth
/// (tick + tick_offset − age_months), which is constant for a household and differs for an entrant in a reused slot.
/// Ageing is frozen during burn-in (no entries or deaths then), so the keys are only compared in the reported run and
/// are first set on the first pass after burn-in. Dead slots get a sentinel key, so a later entrant is also detected.
fn occupant_changes(w: &mut World) -> Vec<bool> {
    let n = w.households.len();
    let frozen = w.tick_offset == 0 && w.tick < w.cfg.burn_in_months;
    let f = &mut w.fiscal;
    ensure_len(&mut f.occupant_key, n, i64::MIN);
    let mut out = vec![false; n];
    if frozen {
        f.occupant_init = false;
        return out;
    }
    let compare = f.occupant_init;
    let now = i64::from(w.tick) + i64::from(w.tick_offset);
    for (i, h) in w.households.iter().enumerate() {
        if !h.alive {
            f.occupant_key[i] = i64::MIN;
            continue;
        }
        let key = now - i64::from(h.age_months);
        if compare && f.occupant_key[i] != key {
            out[i] = true;
        }
        f.occupant_key[i] = key;
    }
    f.occupant_init = true;
    out
}

/// Capital-gains tax on Fund shares realised this month (fix-fiscal), per household and indexed like `w.households`.
/// Realised gains are (a) sales, which realise a pro-rata share of the cost basis (purchases add to basis at the current
/// share price), and (b) deemed realisation of `cap_gains_realisation`/12 of the unrealised gain each month, which steps
/// the basis up by the amount realised. Net realised gains are taxed at `cap_gains_tax` after losses carried forward.
/// Households in a reused slot (`new_occ`) start from a clean state.
fn realised_gains_tax(w: &mut World, new_occ: &[bool]) -> Vec<f64> {
    let n = w.households.len();
    let price = nonneg(w.fund.share_price);
    let rate = w.cfg.cap_gains_tax;
    let turnover_m = nonneg(w.cfg.cap_gains_realisation) / 12.0;
    let f = &mut w.fiscal;
    ensure_len(&mut f.fund_shares_prev, n, 0.0);
    ensure_len(&mut f.fund_basis, n, 0.0);
    ensure_len(&mut f.cg_loss_cf, n, 0.0);
    let mut tax = vec![0.0; n];
    for (i, h) in w.households.iter().enumerate() {
        if !h.alive || new_occ[i] {
            f.fund_shares_prev[i] = 0.0;
            f.fund_basis[i] = 0.0;
            f.cg_loss_cf[i] = 0.0;
            if !h.alive {
                continue;
            }
        }
        let shares = nonneg(h.shares);
        let prev = f.fund_shares_prev[i];
        let delta = shares - prev;
        let mut gain = 0.0;
        if delta > 0.0 {
            f.fund_basis[i] += delta * price;
        } else if delta < 0.0 && prev > 0.0 {
            let sold = -delta;
            let cost = f.fund_basis[i] * (sold / prev).min(1.0);
            gain += sold * price - cost;
            f.fund_basis[i] -= cost;
        }
        // Deemed realisation of part of the unrealised gain on the remaining position.
        let unrealised = shares * price - f.fund_basis[i];
        if unrealised > 0.0 && turnover_m > 0.0 {
            let realised = turnover_m * unrealised;
            f.fund_basis[i] += realised;
            gain += realised;
        }
        if gain != 0.0 {
            let cf = f.cg_loss_cf[i];
            let taxable = if gain >= 0.0 {
                let used = gain.min(cf);
                f.cg_loss_cf[i] = cf - used;
                gain - used
            } else {
                f.cg_loss_cf[i] = cf - gain;
                0.0
            };
            tax[i] = rate * nonneg(taxable);
        }
        f.fund_shares_prev[i] = shares;
    }
    f.capgains_month = tax.iter().sum();
    tax
}

/// Central-bank net income this month (fix-fiscal): coupons on the bonds the central bank holds, plus discount-window
/// interest, less interest on reserves (the Fed's income statement). Read at the top of `collect_and_finance`, after
/// `banking::service` has paid the reserve and discount interest on the month's balances and before taxes drain
/// reserves. The coupons are paid later by `monetary::markets`.
fn central_bank_net_income(w: &World) -> f64 {
    let policy = if w.cb.policy_rate.is_finite() { w.cb.policy_rate.max(0.0) } else { 0.0 };
    let coupons: f64 = w
        .bonds
        .iter()
        .map(|b| b.held_by(BondHolder::CentralBank) * b.coupon.max(0.0) / 12.0)
        .sum();
    let (mut reserves, mut borrowed) = (0.0, 0.0);
    for b in w.banks.iter().filter(|b| b.alive) {
        reserves += b.reserves.max(0.0);
        borrowed += b.cb_borrowing.max(0.0);
    }
    let discount = (policy + w.cfg.discount_spread).max(0.0) / 12.0;
    coupons + borrowed * discount - reserves * policy / 12.0
}

/// Remittance of central-bank net income `net` to the Treasury (fix-fiscal). Positive income first repays the deferred
/// asset; the rest is remitted. Negative income builds a deferred asset and nothing is remitted.
fn central_bank_remittance(w: &mut World, net: f64) -> f64 {
    let net = if net.is_finite() { net } else { 0.0 };
    let cb = &mut w.cb;
    let mut remit = 0.0;
    if net < 0.0 {
        cb.deferred_asset += -net;
    } else {
        let repaid = net.min(cb.deferred_asset);
        cb.deferred_asset -= repaid;
        remit = net - repaid;
    }
    cb.remittance_month = remit;
    cb.remittance_total += remit;
    remit
}

/// Face value to issue per tenor this month (fix-fiscal). The total is the financing need, which includes redemptions
/// of maturing bonds. Each tenor first refinances its expected redemptions, stock_j / tenor_j for uniform vintages,
/// then the net financing (total less expected redemptions) is spread by the target stock mix, so net issuance moves
/// the stock toward `maturity_stock_mix`. A feedback term of `issuance_feedback` × (target share × debt − stock) corrects
/// the drift. Shares are floored at zero and renormalised to `total`.
fn tenor_faces(w: &World, total: f64) -> [f64; N_TENORS] {
    let mut stock = [0.0; N_TENORS];
    for b in &w.bonds {
        if let Some(j) = TENORS.iter().position(|&t| t == b.tenor_months) {
            stock[j] += b.face_outstanding();
        }
    }
    let debt: f64 = stock.iter().sum();
    let target_sum: f64 = w.cfg.maturity_stock_mix.iter().sum();
    let kappa = w.cfg.issuance_feedback;
    let mut redeem = [0.0; N_TENORS];
    for j in 0..N_TENORS {
        redeem[j] = stock[j] / f64::from(TENORS[j]);
    }
    let net = total - redeem.iter().sum::<f64>();
    let mut raw = [0.0; N_TENORS];
    for j in 0..N_TENORS {
        let target = if target_sum > 0.0 { w.cfg.maturity_stock_mix[j] / target_sum } else { 1.0 / N_TENORS as f64 };
        raw[j] = nonneg(redeem[j] + net * target + kappa * (target * debt - stock[j]));
    }
    let sum: f64 = raw.iter().sum();
    if sum > 0.0 && sum.is_finite() {
        for x in raw.iter_mut() {
            *x *= total / sum;
        }
    }
    raw
}

/// Sell new bonds with total face `total` at par. One issue per tenor; the tenor split follows
/// the tenor split of `tenor_faces` and the buyer split `auction_shares` (banks, Fund, RoW). The bank share goes to
/// banks in proportion to positive reserves, or to the Fund if no bank has positive reserves.
fn auction_bonds(w: &mut World, total: f64) {
    let faces = tenor_faces(w, total);
    let shares = w.cfg.auction_shares;
    let pool: Vec<(BankId, f64)> = w
        .banks
        .iter()
        .enumerate()
        .filter(|(_, b)| b.alive && b.reserves > 0.0 && b.reserves.is_finite())
        .map(|(i, b)| (i as BankId, b.reserves))
        .collect();
    let pool_total: f64 = pool.iter().map(|p| p.1).sum();

    for j in 0..N_TENORS {
        let face = faces[j];
        if !(face > 0.0) {
            continue;
        }
        let bank_face = face * shares[0];
        let mut fund_face = face * shares[1];
        let mut buyers: Vec<(BondHolder, f64)> = Vec::with_capacity(pool.len() + 2);
        if pool_total > 0.0 {
            for &(b, r) in &pool {
                let f = bank_face * r / pool_total;
                if f > 0.0 {
                    buyers.push((BondHolder::Bank(b), f));
                }
            }
        } else {
            fund_face += bank_face;
        }
        if fund_face > 0.0 {
            buyers.push((BondHolder::Fund, fund_face));
        }
        let row_face = face * shares[2];
        if row_face > 0.0 {
            buyers.push((BondHolder::RestOfWorld, row_face));
        }
        let Some(&(first, first_face)) = buyers.first() else {
            continue;
        };
        let coupon = nonneg(w.curve.yields[j]);
        let idx = ledger::issue_bond(w, TENORS[j], coupon, first, first_face, 1.0);
        for &(h, f) in &buyers[1..] {
            ledger::reopen_bond(w, idx, h, f, 1.0);
        }
        if row_face > 0.0 {
            let id = w.bonds[idx].id;
            crate::world::note_row_primary(w, id, row_face, row_face * 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    fn world(seed: u64) -> World {
        tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            seed,
        )
    }

    fn total_deposits(w: &World) -> f64 {
        w.households.iter().map(|h| h.deposits).sum()
    }

    fn total_reserves(w: &World) -> f64 {
        w.banks.iter().map(|b| b.reserves).sum()
    }

    #[test]
    fn unemployed_worker_gets_benefits_for_exactly_benefit_months() {
        let mut w = world(3);
        let months = w.cfg.benefit_months;
        let h = w
            .households
            .iter()
            .position(|x| x.status == LaborStatus::Employed)
            .expect("an employed household");
        // Separation starts benefit eligibility (common::separate_worker sets benefit_months_left).
        common::separate_worker(&mut w, h as HhId, LaborStatus::Unemployed);
        assert_eq!(u32::from(w.households[h].benefit_months_left), months);

        let mut received = 0;
        for _ in 0..(months + 3) {
            w.hh_month[h].income = IncomeMonth::default();
            pay_transfers(&mut w);
            if w.hh_month[h].income.unemployment_benefit > 0.0 {
                received += 1;
            }
        }
        assert_eq!(received, months);
        assert_eq!(w.households[h].benefit_months_left, 0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn income_tax_is_progressive() {
        let mut w = world(5);
        let wages = [2_000.0, 5_000.0, 10_000.0, 14_000.0];
        for (i, &wage) in wages.iter().enumerate() {
            ledger::pay(&mut w, Account::CentralBank, Account::Household(i as HhId), 1e7);
            w.hh_month[i].income = IncomeMonth {
                wage,
                ..Default::default()
            };
        }
        collect_and_finance(&mut w);
        let rates: Vec<f64> = wages
            .iter()
            .enumerate()
            .map(|(i, &wage)| w.hh_month[i].taxes / wage)
            .collect();
        for k in 1..rates.len() {
            assert!(
                rates[k] > rates[k - 1],
                "effective rate should rise with income: {rates:?}"
            );
        }
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn deficit_triggers_bond_issuance_back_to_tga_target() {
        let mut w = world(9);
        // Empty the TGA through the ledger.
        let tga = w.cb.tga;
        ledger::pay(&mut w, Account::Government, Account::CentralBank, tga);
        assert!(w.cb.tga.abs() < 1e-9);
        let n_bonds = w.bonds.len();
        let debt0 = w.gov.debt_face;

        for _ in 0..3 {
            pay_transfers(&mut w);
            plan_spending(&mut w);
            collect_and_finance(&mut w);
            let target = w.fiscal.tga_target;
            assert!(target > 0.0);
            assert!(
                (w.cb.tga - target).abs() < 1e-6 * (1.0 + target),
                "tga {} should be restored to target {}",
                w.cb.tga,
                target
            );
            w.tick += 1;
        }
        assert!(w.bonds.len() > n_bonds);
        assert!(w.gov.debt_face > debt0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn helicopter_money_adds_deposits_and_reserves_without_reducing_tga() {
        let mut w = world(11);
        w.cb.helicopter_per_household = 100.0;
        let dep0 = total_deposits(&w);
        let res0 = total_reserves(&w);
        let tga0 = w.cb.tga;

        pay_transfers(&mut w);

        let n_alive = w.households.iter().filter(|h| h.alive).count() as f64;
        let heli = 100.0 * n_alive;
        assert!((w.flows.helicopter - heli).abs() < 1e-6);
        assert!((w.cb.helicopter_total - heli).abs() < 1e-6);
        assert!(w.hh_month.iter().all(|m| m.income.windfall >= 100.0));

        // Only the government's own transfers reduce the TGA; helicopter money does not.
        let gov_paid = w.gov.transfers;
        assert!((tga0 - w.cb.tga - gov_paid).abs() < 1e-6 * (1.0 + tga0.abs()));
        // Deposits and reserves rise by both government transfers and helicopter money.
        let dep_gain = total_deposits(&w) - dep0;
        let res_gain = total_reserves(&w) - res0;
        assert!((dep_gain - (gov_paid + heli)).abs() < 1e-6 * (1.0 + dep_gain.abs()));
        assert!((res_gain - (gov_paid + heli)).abs() < 1e-6 * (1.0 + res_gain.abs()));
        ledger::check_consistency(&w).unwrap();
    }

    // ---- RZ23: Shock::Rebate, the Treasury-financed one-off payment ----

    fn schedule_rebate(w: &mut World, per_household: f64, months: u32) {
        let tick = w.tick;
        w.shocks.schedule(crate::shocks::ScheduledShock { at_month: tick, shock: crate::shocks::Shock::Rebate { per_household, months } });
        w.shocks.activate_due(tick);
    }

    #[test]
    fn rebate_is_paid_from_the_tga_creates_no_central_bank_money_and_is_a_windfall() {
        let mut w = world(11);
        schedule_rebate(&mut w, 100.0, 1);
        assert_eq!(w.cb.helicopter_per_household, 0.0, "the helicopter input is untouched");
        let (dep0, res0, tga0) = (total_deposits(&w), total_reserves(&w), w.cb.tga);

        pay_transfers(&mut w);

        let n_alive = w.households.iter().filter(|h| h.alive).count() as f64;
        let reb = 100.0 * n_alive;
        // Reported in the one-off-payment series, but not central-bank money.
        assert!((w.flows.helicopter - reb).abs() < 1e-6);
        assert_eq!(w.cb.helicopter_total, 0.0);
        assert!(w.hh_month.iter().zip(w.households.iter()).filter(|(_, h)| h.alive).all(|(m, _)| m.income.windfall >= 100.0));
        // It comes out of the TGA with the government's other payments (the TGA falls by everything the government paid).
        let gov_paid = w.gov.transfers;
        assert!(gov_paid >= reb - 1e-6, "transfers {gov_paid} include the rebate {reb}");
        assert!((tga0 - w.cb.tga - gov_paid).abs() < 1e-6 * (1.0 + tga0.abs()));
        assert!((w.gov.outlays - gov_paid).abs() < 1e-6 * (1.0 + gov_paid));
        // Deposits and reserves of the banking system rise by the same amount (the reserves come from the TGA).
        let (dep_gain, res_gain) = (total_deposits(&w) - dep0, total_reserves(&w) - res0);
        assert!((dep_gain - gov_paid).abs() < 1e-6 * (1.0 + dep_gain.abs()));
        assert!((res_gain - gov_paid).abs() < 1e-6 * (1.0 + res_gain.abs()));
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn rebate_is_financed_by_the_bond_auction_that_refills_the_tga() {
        let mut quiet = world(12);
        let mut paid = world(12);
        schedule_rebate(&mut paid, 2_000.0, 1);
        for w in [&mut quiet, &mut paid] {
            // Empty the TGA through the ledger, so that the auction has to refill it from nothing in both worlds.
            let tga = w.cb.tga;
            ledger::pay(w, Account::Government, Account::CentralBank, tga);
            pay_transfers(w);
            plan_spending(w);
            collect_and_finance(w);
        }
        let n_alive = paid.households.iter().filter(|h| h.alive).count() as f64;
        let reb = 2_000.0 * n_alive;
        // The TGA is back at its target in both worlds. The target follows the smoothed outlays, so the month with the rebate
        // leaves a larger one: the extra debt is the rebate plus that larger balance, and the deficit is the rebate.
        for w in [&quiet, &paid] {
            assert!((w.cb.tga - w.fiscal.tga_target).abs() < 1e-6 * (1.0 + w.fiscal.tga_target));
        }
        let tga_gain = paid.cb.tga - quiet.cb.tga;
        assert!(tga_gain >= -1e-6 && tga_gain < reb, "the larger target {tga_gain} is part of the rebate {reb}");
        assert!((paid.gov.debt_face - quiet.gov.debt_face - reb - tga_gain).abs() < 1e-6 * reb, "debt {} vs {}", paid.gov.debt_face, quiet.gov.debt_face);
        assert!((paid.gov.deficit - quiet.gov.deficit - reb).abs() < 1e-6 * reb);
        ledger::check_consistency(&paid).unwrap();
    }

    #[test]
    fn rebate_runs_for_its_months_only_and_off_it_changes_nothing() {
        let mut w = world(13);
        schedule_rebate(&mut w, 50.0, 2);
        let tick0 = w.tick;
        assert_eq!(w.shocks.rebate_per_household(tick0), 50.0);
        assert_eq!(w.shocks.rebate_per_household(tick0 + 1), 50.0);
        assert_eq!(w.shocks.rebate_per_household(tick0 + 2), 0.0);
        // No rebate in force: the month's payments are exactly those of a world that never heard of the shock.
        let (mut a, mut b) = (world(14), world(14));
        pay_transfers(&mut a);
        pay_transfers(&mut b);
        assert_eq!(a.gov.transfers.to_bits(), b.gov.transfers.to_bits());
        assert_eq!(a.flows.helicopter, 0.0);
        assert!(a.hh_month.iter().all(|m| m.income.windfall == 0.0));
    }

    #[test]
    fn purchases_follow_trend_gdp_and_sector_split() {
        let mut w = world(2);
        plan_spending(&mut w);
        let total: f64 = w.orders.government.iter().sum();
        let rule = (1.0 - w.cfg.fiscal_rule_strength * FISCAL_DEBT_RESPONSE * debt_rule_gap(&w)).clamp(0.75, 1.3);
        let base = w.cfg.gov_purchases_share * trend_gdp_month(&w);
        // Health is the t=0 share of base purchases (no debt rule or shock); other purchases carry both.
        let other = (1.0 - HEALTH_SHARE) * base * w.shocks.gov_spending_mult(w.tick) * rule;
        let expect = HEALTH_SHARE * base + other;
        assert!(expect > 0.0);
        assert!((total - expect).abs() < 1e-6 * expect);
        assert!((w.fiscal.health_month - HEALTH_SHARE * base).abs() < 1e-6 * expect);
        // Health goes by `health_sector_mix` (HealthEducation); other purchases take the non-health remainder of
        // `gov_purchase_split`.
        let he = Sector::HealthEducation.idx();
        let hmix = w.cfg.health_sector_mix;
        for s in Sector::ALL {
            let i = s.idx();
            let other_split = (w.cfg.gov_purchase_split[i] - HEALTH_SHARE * hmix[i]) / (1.0 - HEALTH_SHARE);
            let health = if i == he { HEALTH_SHARE * base } else { 0.0 };
            assert!((w.orders.government[i] - (other_split * other + health)).abs() < 1e-6 * total);
        }
        assert_eq!(w.orders.government[Sector::Energy.idx()], 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    // fix-fiscal
    #[test]
    fn t0_sector_orders_match_the_purchase_split() {
        // With no debt-rule or shock multiplier (rule = 1, mult = 1), health is HEALTH_SHARE of purchases and the
        // sector orders equal gov_purchase_split × total, as they did before health was split out.
        let mut w = world(12);
        let debt_gdp = w.gov.debt_face / (12.0 * trend_gdp_month(&w));
        w.cfg.initial_debt_gdp = debt_gdp;
        assert!((w.shocks.gov_spending_mult(w.tick) - 1.0).abs() < 1e-12);
        plan_spending(&mut w);
        let total: f64 = w.orders.government.iter().sum();
        for s in 0..N_SECTORS {
            let want = w.cfg.gov_purchase_split[s] * total;
            assert!((w.orders.government[s] - want).abs() < 1e-9 * total, "sector {s}: {} vs {want}", w.orders.government[s]);
        }
    }

    // fix-fiscal
    #[test]
    fn reused_slot_does_not_inherit_capital_gains() {
        let mut w = world(14);
        // Warm the basis up: shares held with a price rise, then the household dies and an entrant takes its slot.
        let i = w.households.iter().position(|h| h.alive).expect("alive household");
        w.households[i].shares = 1_000.0;
        w.fund.share_price = 1.0;
        w.tick = w.cfg.burn_in_months + 100;
        w.tick_offset = 0;
        w.fiscal.occupant_init = false;
        let _ = occupant_changes(&mut w);
        let none = vec![false; w.households.len()];
        let _ = realised_gains_tax(&mut w, &none);
        w.fund.share_price = 2.0;
        // Death and reuse within one tick: new occupant (different age, so different birth key).
        w.households[i].age_months = w.households[i].age_months.wrapping_add(37);
        let new_occ = occupant_changes(&mut w);
        assert!(new_occ[i], "reused slot should be detected as a new occupant");
        w.households[i].shares = 0.0;
        let tax = realised_gains_tax(&mut w, &new_occ);
        assert_eq!(tax[i], 0.0, "entrant must not be taxed on the previous occupant's gain");
        assert_eq!(w.fiscal.fund_basis[i], 0.0);
    }

    // fix-fiscal
    #[test]
    fn ssi_floor_is_asset_tested() {
        let w = world(4);
        let mut hh = w.households[0].clone();
        hh.status = LaborStatus::Retired;
        hh.partnered = false;
        let inc = IncomeMonth::default();
        let cfg = &w.cfg;
        assert!((means_tested_support(cfg, 1.0, 1.0, &hh, &inc, 0.0) - cfg.ssi_max_single).abs() < 1e-9);
        assert_eq!(means_tested_support(cfg, 1.0, 1.0, &hh, &inc, cfg.ssi_asset_limit + 1.0), 0.0);
    }

    // Round 7 step 1: the frozen burn-in tracks the wage index in its January COLA and nominal scale, and the UI cap
    // follows the wage index.
    #[test]
    fn anchor_month_tracks_wage_index() {
        let cfg = Config { n_households: 1_000, burn_in_months: 24, ..Default::default() };
        let mut w = crate::init::build_world(cfg, 3);
        // build_world runs the 24-month burn-in; one step is the anchor month (a January on the absolute calendar).
        crate::sim::step(&mut w);
        let wi = w.prices.wage_index / 100.0;
        assert!((w.gov.cola_index - wi).abs() < 1e-9, "cola {} wage {}", w.gov.cola_index, wi);
        assert!((w.gov.nominal_scale - w.gov.cola_index).abs() < 1e-12);
        let nom = nominal(&w);
        assert!((nom.ui_cap - w.cfg.benefit_cap * wi).abs() < 1e-9);
        assert!((nom.min_wage - w.cfg.min_wage_monthly * wi).abs() < 1e-9);
    }

    // fix-fiscal
    #[test]
    fn eitc_maximum_is_about_600_a_month_and_phases_out_near_60k() {
        let cfg = crate::config::Config::default();
        let max_at = |wage: f64| {
            let w = world(4);
            let mut hh = w.households[0].clone();
            hh.status = LaborStatus::Employed;
            hh.age_months = 40 * 12;
            hh.partnered = false;
            hh.tenure = Tenure::Renter;
            let inc = IncomeMonth { wage, ..IncomeMonth::default() };
            means_tested_support(&cfg, 1.0, 1.0, &hh, &inc, 1e9)
        };
        assert!((max_at(cfg.eitc_plateau) - 596.0).abs() < 5.0, "max {}", max_at(cfg.eitc_plateau));
        assert!(max_at(4_776.0) < 1.0, "phase-out should end near 4,776 a month (about $57k a year)");
    }

    // fix-fiscal
    #[test]
    fn ss_seed_walk_matches_monthly_hazard() {
        // A head aged 62y0m has one monthly draw (about 2.4%); aged 62y11m, twelve draws give the 25% annual rate.
        let n = 20_000u64;
        let share = |age_months: u32| {
            (0..n).filter(|&id| ss_seed_claim_age(7, 0, id, age_months).is_some()).count() as f64 / n as f64
        };
        let s0 = share(744);
        let s11 = share(755);
        assert!((s0 - 0.024).abs() < 0.008, "claimed at 62y0m {s0}");
        assert!((s11 - 0.25).abs() < 0.02, "claimed at 62y11m {s11}");
        // Everyone unclaimed at 70 has claimed by the walk, at a claim age of 70 or earlier.
        for id in 0..2000u64 {
            let age = ss_seed_claim_age(7, 0, id, 840).expect("claimed by 70");
            assert!(age <= 70.0);
        }
    }

    #[test]
    fn ss_claimed_share_of_62_year_olds_at_first_reported_month() {
        // Burn-in freezes ages and draws no claims; the first reported month seeds the claimed stock of non-retirees.
        // Retirees aged 62+ claim at once (init and the monthly rule), so the share is about 0.4 of 62-year-olds: the
        // retired ones (roughly 40%) plus the walked claims of the rest (reviewer fix to the integrated claiming rule).
        let mut cfg = Config::default();
        cfg.n_households = 10_000;
        cfg.burn_in_months = 72;
        let mut w = crate::init::build_world(cfg, 1);
        crate::sim::step(&mut w);
        let (mut n, mut c) = (0usize, 0usize);
        for h in w.households.iter().filter(|h| h.alive && (744..756).contains(&(h.age_months as usize))) {
            n += 1;
            if h.pension > 0.0 {
                c += 1;
            }
        }
        let share = c as f64 / n.max(1) as f64;
        assert!(n > 50 && (0.25..0.55).contains(&share), "claimed share at 62 {share} of {n}");
    }

    #[test]
    fn social_security_claims_by_age_and_sums_to_one_by_70() {
        let mut unclaimed = 1.0;
        for a in 62..70 {
            let h = claim_hazard_monthly(a as f64 + 0.5);
            let annual = 1.0 - (1.0 - h).powi(12);
            if a == 62 {
                assert!((annual - 0.25).abs() < 1e-9);
            }
            unclaimed *= 1.0 - annual;
        }
        assert!(unclaimed < 0.3 && unclaimed > 0.2, "unclaimed at 70 {unclaimed}");
        assert_eq!(claim_hazard_monthly(70.5), 1.0);
        assert!(claim_age_factor(62.0) < 0.75 && claim_age_factor(67.0) == 1.0 && claim_age_factor(70.0) > 1.2);
    }

    #[test]
    fn central_bank_loss_builds_deferred_asset_and_profit_repays_it() {
        let mut w = world(6);
        plan_spending(&mut w);
        w.cb.policy_rate = 0.5;
        collect_and_finance(&mut w);
        assert_eq!(w.cb.remittance_month, 0.0);
        assert!(w.cb.deferred_asset > 0.0, "interest on reserves above coupons should build a deferred asset");
        ledger::check_consistency(&w).unwrap();
        let deferred = w.cb.deferred_asset;
        w.cb.policy_rate = 0.0;
        collect_and_finance(&mut w);
        assert!(w.cb.deferred_asset < deferred, "positive income should repay the deferred asset first");
        assert!(w.cb.remittance_month >= 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn tenor_issuance_sums_to_need() {
        let w = world(8);
        let faces = tenor_faces(&w, 1.0);
        assert!((faces.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        assert!(faces.iter().all(|&x| x >= 0.0));
    }

    /// b5-udur M5: an exhausted eligible spell is extended exactly once; an entrant (never on benefits) is never paid.
    #[test]
    fn ui_extension_pays_eligible_once_and_never_entrants() {
        let cfg = Config {
            n_households: 500,
            benefit_months: 2,
            udur_ui_ext_t1: 0.0,
            udur_ui_ext_m1: 3.0,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 31);
        w.fiscal.ui_ext = vec![0u8; w.households.len()];
        let (eligible, entrant) = (0usize, 1usize);
        for (h, months, left) in [(eligible, 2u16, 1u16), (entrant, 0u16, 0u16)] {
            let hh = &mut w.households[h];
            hh.alive = true;
            hh.status = LaborStatus::Unemployed;
            hh.months_unemployed = months;
            hh.benefit_months_left = left;
            hh.last_wage = if h == eligible { 2_000.0 } else { 0.0 };
            hh.months_employed = 0;
        }
        w.fiscal.ui_ext[eligible] = 1;
        // Month 1: the last paid month of the standard entitlement; the extension (3 months) starts.
        pay_transfers(&mut w);
        assert_eq!(w.households[eligible].benefit_months_left, 3, "extension not started");
        assert_eq!(w.fiscal.ui_ext[eligible], 2);
        assert_eq!(w.households[entrant].benefit_months_left, 0, "entrant was paid");
        assert_eq!(w.fiscal.ui_ext[entrant], 0);
        assert_eq!(w.flows.udur_ui_ext_n, 0, "the standard last month is not an extension month");
        // Month 2: the extension is paid once more and is not restarted.
        w.flows = Flows::default();
        pay_transfers(&mut w);
        assert_eq!(w.households[eligible].benefit_months_left, 2);
        assert_eq!(w.flows.udur_ui_ext_n, 1);
        assert_eq!(w.households[entrant].benefit_months_left, 0);
    }

    /// b5-udur M5: a dead slot's extension state (2) does not survive into a replacement entrant placed in the
    /// same slot, which must not be paid even with an extension available.
    #[test]
    fn ui_extension_state_does_not_leak_into_reused_slot() {
        let cfg = Config {
            n_households: 500,
            benefit_months: 2,
            udur_ui_ext_t1: 0.0,
            udur_ui_ext_m1: 3.0,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 37);
        w.fiscal.ui_ext = vec![0u8; w.households.len()];
        let slot = 2usize;
        w.households[slot].alive = false;
        w.fiscal.ui_ext[slot] = 2;
        pay_transfers(&mut w);
        assert_eq!(w.fiscal.ui_ext[slot], 0, "dead slot kept its extension state");
        // Entrant placed in the slot: Unemployed, never employed, no benefit months.
        let hh = &mut w.households[slot];
        hh.alive = true;
        hh.status = LaborStatus::Unemployed;
        hh.months_unemployed = 0;
        hh.bls_unemployed = 0;
        hh.months_employed = 0;
        hh.benefit_months_left = 0;
        hh.last_wage = 0.0;
        w.flows = Flows::default();
        pay_transfers(&mut w);
        assert_eq!(w.households[slot].benefit_months_left, 0, "entrant in a reused slot was paid");
        assert_eq!(w.flows.udur_ui_ext_n, 0);
    }

    /// b6-reentry fix: a spell already past benefit_months + ext (reachback cap) gets no extension at step 5.
    /// Setup from the b5-udur review: benefit_months 6, extension 3, a 30-month exhausted spell in state 1.
    #[test]
    fn long_exhausted_spell_gets_no_extension() {
        let cfg = Config {
            n_households: 500,
            benefit_months: 6,
            udur_ui_ext_t1: 0.0,
            udur_ui_ext_m1: 3.0,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 61);
        w.fiscal.ui_ext = vec![0u8; w.households.len()];
        let h = 0usize;
        {
            let hh = &mut w.households[h];
            hh.alive = true;
            hh.status = LaborStatus::Unemployed;
            hh.months_employed = 0;
            hh.last_wage = 2_000.0;
            hh.benefit_months_left = 0;
            hh.months_unemployed = 30;
        }
        w.fiscal.ui_ext[h] = 1;
        let mut paid_months = 0;
        for _ in 0..6 {
            w.households[h].months_unemployed += 1;
            w.households[h].bls_unemployed += 1;
            pay_transfers(&mut w);
            if w.hh_month[h].income.unemployment_benefit > 0.0 {
                paid_months += 1;
            }
        }
        assert_eq!(paid_months, 0, "a spell past benefit_months + ext was paid");
        assert_eq!(w.households[h].benefit_months_left, 0);
        assert_eq!(w.fiscal.ui_ext[h], 1, "state changed for a spell the reachback skipped");
        assert_eq!(w.flows.udur_ui_ext_n, 0);
    }

    /// b5-udur M5 (multi-month): one spell with 2 standard months and a 3-month extension is paid exactly 5 months,
    /// the extension is counted once, and it is not restarted after it runs out.
    #[test]
    fn ui_extension_multi_month_spell_is_extended_once() {
        let cfg = Config {
            n_households: 500,
            benefit_months: 2,
            udur_ui_ext_t1: 0.0,
            udur_ui_ext_m1: 3.0,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 53);
        w.fiscal.ui_ext = vec![0u8; w.households.len()];
        let h = 0usize;
        {
            let hh = &mut w.households[h];
            hh.alive = true;
            hh.status = LaborStatus::Unemployed;
            hh.months_employed = 0;
            hh.last_wage = 2_000.0;
            hh.benefit_months_left = 2;
            hh.months_unemployed = 0;
            hh.bls_unemployed = 0;
        }
        let mut paid_months = 0;
        for _ in 0..8 {
            w.households[h].months_unemployed += 1;
            w.households[h].bls_unemployed += 1;
            pay_transfers(&mut w);
            if w.hh_month[h].income.unemployment_benefit > 0.0 {
                paid_months += 1;
            }
        }
        assert_eq!(paid_months, 5, "2 standard + 3 extension months expected");
        assert_eq!(w.flows.udur_ui_ext_n, 3, "extension should be counted once, month by month");
        assert_eq!(w.households[h].benefit_months_left, 0);
    }

    /// b5-udur M5 (multi-month): an unemployed household with a previous job, whose months_employed is 0 (the
    /// simulation invariant after a separation), is eligible for the extension and is not treated as an entrant.
    #[test]
    fn ui_extension_eligibility_uses_last_wage_not_months_employed() {
        let cfg = Config { n_households: 500, benefit_months: 1, udur_ui_ext_t1: 0.0, udur_ui_ext_m1: 2.0, ..Default::default() };
        let mut w = tiny_world(cfg, 59);
        w.fiscal.ui_ext = vec![0u8; w.households.len()];
        let h = 0usize;
        let hh = &mut w.households[h];
        hh.alive = true;
        hh.status = LaborStatus::Unemployed;
        hh.months_employed = 0;
        hh.last_wage = 1_500.0;
        hh.benefit_months_left = 1;
        hh.months_unemployed = 1;
        pay_transfers(&mut w);
        assert_eq!(w.fiscal.ui_ext[h], 2, "displaced worker was not extended");
        assert_eq!(w.households[h].benefit_months_left, 2);
    }
}

#[cfg(test)]
mod rz23_tests {
    //! RZ23: the Treasury-financed `Rebate` shock and the windfall run-down, through whole months (paired worlds at 1,500 households).
    use crate::config::Config;
    use crate::shocks::{ScheduledShock, Shock};
    use crate::sim::{self, Comparison, Scenario};
    use std::sync::OnceLock;

    const N: usize = 1500;
    const SEEDS: [u64; 2] = [1, 2];

    fn cfg() -> Config {
        Config { n_households: N, ..Config::default() }
    }

    static WORLDS: OnceLock<Vec<crate::types::World>> = OnceLock::new();

    /// Built once on the test thread (a world build uses rayon; building one from inside a worker of `compare_with` can deadlock).
    fn worlds() {
        WORLDS.get_or_init(|| SEEDS.iter().map(|&s| sim::common_world(&cfg(), s).0).collect());
    }

    /// Two arms from one common world per seed; `tweak` changes the common world's configuration before both arms run from it (the
    /// scenario's own `config` is not applied to a world that is already built).
    fn paired(tweak: &(dyn Fn(&mut Config) + Sync), shocks: Vec<ScheduledShock>, months: u32, seeds: &[u64]) -> Comparison {
        worlds();
        let sc = Scenario { config: cfg(), shocks, months, seeds: seeds.to_vec() };
        let common = |seed: u64| -> sim::CommonWorld {
            let ws = WORLDS.get().expect("worlds() ran first");
            let mut w = ws[SEEDS.iter().position(|&s| s == seed).expect("a test seed")].clone();
            tweak(&mut w.cfg);
            (w, None)
        };
        assert!(sc.config.sfc_enforce, "the stock-flow checks are on, so a bad month would stop the run");
        sim::compare_with(&sc, &common, &mut |_| {})
    }

    fn keep(_: &mut Config) {}

    fn at0(shock: Shock) -> Vec<ScheduledShock> {
        vec![ScheduledShock { at_month: 0, shock }]
    }

    fn json<T: serde::Serialize>(x: &T) -> String {
        serde_json::to_string(x).unwrap()
    }

    fn cum_cons_gap(c: &Comparison, a: usize, b: usize) -> f64 {
        (a..b).map(|t| c.shocked[t].consumption_real_bn_ar - c.baseline[t].consumption_real_bn_ar).sum()
    }

    fn c_debt(c: &Comparison) -> f64 {
        c.shocked[11].gov_debt_bn - c.baseline[11].gov_debt_bn
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-9 * (1.0 + a.abs().max(b.abs()))
    }

    #[test]
    fn a_zero_rebate_and_a_world_without_one_are_byte_identical() {
        for seed in SEEDS {
            let none = paired(&keep, vec![], 6, &[seed]);
            let zero = paired(&keep, at0(Shock::Rebate { per_household: 0.0, months: 3 }), 6, &[seed]);
            assert_eq!(json(&none.baseline), json(&zero.baseline), "seed {seed}");
            assert_eq!(json(&none.baseline), json(&zero.shocked), "seed {seed}: the zero rebate changed the history");
        }
    }

    #[test]
    fn the_windfall_keys_do_nothing_without_a_one_off_payment() {
        // Candidate A and the repair B change only the run-down and the two MPCs of the windfall rule; the windfall stock is zero
        // in a run with no payment, so the history is the same byte for byte. With a payment it is not.
        let changed = |c: &mut Config| {
            c.windfall_decay = 0.0;
            c.mpc_windfall_low = 0.42;
            c.mpc_windfall_high = 0.10;
        };
        for seed in SEEDS {
            let a = paired(&keep, vec![], 8, &[seed]);
            let b = paired(&changed, vec![], 8, &[seed]);
            assert_eq!(json(&a.baseline), json(&b.baseline), "seed {seed}: windfall keys moved a run with no payment");
            let paid = Shock::Rebate { per_household: 2_000.0, months: 1 };
            let c = paired(&keep, at0(paid.clone()), 8, &[seed]);
            let d = paired(&changed, at0(paid), 8, &[seed]);
            assert_eq!(json(&c.baseline), json(&d.baseline), "seed {seed}: the baseline arm is the same");
            assert_ne!(json(&c.shocked), json(&d.shocked), "seed {seed}: with a payment the keys matter");
        }
    }

    #[test]
    fn rebate_and_helicopter_pay_the_same_first_month_and_differ_in_what_they_leave() {
        for seed in SEEDS {
            let heli = paired(&keep, at0(Shock::Helicopter { per_household: 2_000.0, months: 1 }), 12, &[seed]);
            let reb = paired(&keep, at0(Shock::Rebate { per_household: 2_000.0, months: 1 }), 12, &[seed]);
            // The payment, its spending in the month it arrives and the paired baseline are the same: one windfall rule (the sums
            // are added in another order, so equal to rounding).
            assert!(close(heli.shocked[0].helicopter_bn_ar, reb.shocked[0].helicopter_bn_ar), "seed {seed}");
            assert!(reb.shocked[0].helicopter_bn_ar > 0.0 && reb.shocked[1].helicopter_bn_ar == 0.0);
            // Household spending is decided before the Treasury sells its bonds; only the imputed financial services in measured
            // consumption see the buyers' deposits (8e-7 of consumption here), so the match is to 1e-4, not to the bit.
            let (ch, cr) = (heli.shocked[0].consumption_real_bn_ar, reb.shocked[0].consumption_real_bn_ar);
            assert!((ch - cr).abs() < 1e-4 * ch.abs(), "seed {seed}: first-month spending {ch} vs {cr}");
            assert_eq!(json(&heli.baseline), json(&reb.baseline));
            // The Treasury borrowed it: the debt ratio is above the helicopter's from the first month (the rebate adds to the debt, the
            // helicopter does not), and so is the gross debt at the end of the year.
            let debt_gap = |c: &Comparison, t: usize| c.shocked[t].gov_debt_gdp - c.baseline[t].gov_debt_gdp;
            assert!(debt_gap(&reb, 2) > debt_gap(&heli, 2) + 0.3, "seed {seed}: helicopter {} rebate {}", debt_gap(&heli, 2), debt_gap(&reb, 2));
            assert!(c_debt(&reb) > c_debt(&heli), "seed {seed}: gross debt");
            // Neither stops the run on a stock-flow identity.
            for m in reb.shocked.iter().chain(heli.shocked.iter()) {
                assert_eq!(m.sfc_error, 0.0, "seed {seed}");
            }
        }
    }

    #[test]
    fn the_run_down_adds_spending_after_the_first_quarter_and_zero_removes_it() {
        let paid = Shock::Rebate { per_household: 2_000.0, months: 1 };
        // The mechanism at the pre-RZ23 calibration (MPCs 0.65 and 0.15, run-down 0.03), against the same MPCs with no run-down. RZ23 made 0.39, 0.09 and
        // no run-down the defaults (v2026.10.23.1); at those MPCs the unspent payment is spent through the buffer rule instead, so this test pins the old ones.
        let with_run_down = |c: &mut Config| {
            c.mpc_windfall_low = 0.65;
            c.mpc_windfall_high = 0.15;
            c.windfall_decay = 0.03;
        };
        let off = |c: &mut Config| {
            c.mpc_windfall_low = 0.65;
            c.mpc_windfall_high = 0.15;
            c.windfall_decay = 0.0;
        };
        for seed in SEEDS {
            let on = paired(&with_run_down, at0(paid.clone()), 24, &[seed]);
            let zero = paired(&off, at0(paid.clone()), 24, &[seed]);
            // The first month does not use the run-down (the stock is empty); months 4-24 spend clearly more with it.
            assert_eq!(cum_cons_gap(&on, 0, 1).to_bits(), cum_cons_gap(&zero, 0, 1).to_bits(), "seed {seed}: the first month");
            let (late_on, late_zero) = (cum_cons_gap(&on, 3, 24), cum_cons_gap(&zero, 3, 24));
            assert!(late_on > late_zero * 1.15, "seed {seed}: months 4-24 {late_on} with the run-down, {late_zero} without");
        }
    }

    #[test]
    fn a_three_month_rebate_keeps_the_ledger_and_the_housing_invariants_for_two_years() {
        worlds();
        for seed in SEEDS {
            let mut w = WORLDS.get().unwrap()[SEEDS.iter().position(|&s| s == seed).unwrap()].clone();
            let t0 = w.tick;
            w.shocks.schedule(ScheduledShock { at_month: t0, shock: Shock::Rebate { per_household: 2_000.0, months: 3 } });
            for k in 0..24 {
                sim::step(&mut w);
                crate::ledger::check_consistency(&w).unwrap_or_else(|e| panic!("seed {seed} month {k}: {e}"));
                crate::housing::check_invariants(&w).unwrap_or_else(|e| panic!("seed {seed} month {k}: {e}"));
                assert_eq!(w.history.last().unwrap().sfc_error, 0.0, "seed {seed} month {k}");
            }
            assert!(w.gov.debt_face > 0.0 && w.history.iter().all(|m| m.consumption_real_bn_ar.is_finite() && m.gdp_real_bn_ar.is_finite()));
        }
    }
}
