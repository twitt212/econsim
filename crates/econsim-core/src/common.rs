//! Small shared helpers used by several modules (shared helpers).

use crate::config::Config;
use crate::types::*;

/// World price level (1.0 at the first month of the world, 2% a year): the foreign-currency price of the world
/// basket. Uses calendar time (`tick + tick_offset`), so it continues smoothly across the burn-in reset.
pub fn world_price(w: &World) -> f64 {
    (1.0 + w.cfg.world_goods_inflation).powf((w.tick + w.tick_offset) as f64 / 12.0)
}

// fix-trade: oil is a world price. Energy-sector trade is priced off the world energy price, which moves with the
// oil price relative to its base (elasticity `Config::world_energy_oil_elasticity`). Other sectors keep `world_price`.
/// Oil price relative to its base (1.0 when the base is unusable).
/// Re-fit Step 4c: Energy's crude term in unit cost at oil ratio `oil_ratio`. Legacy (`upstream_split` None): the full
/// crude term `crude_intensity × oil_ratio`. With the split on, the imported share `s` and the taxes, royalties and
/// margin of the domestic share (`(1 − s) × (τ + ρ + m)`) are in unit cost; the upstream capital share κ is excluded,
/// because `testkit::capital_output_months` carries it in Energy's capital cost.
pub fn energy_crude_unit_cost(cfg: &Config, oil_ratio: f64) -> f64 {
    match cfg.upstream_split() {
        None => cfg.crude_intensity * oil_ratio,
        Some((tau, rho, m, _)) => {
            let s = cfg.oil_import_share;
            cfg.crude_intensity * oil_ratio * (s + (1.0 - s) * (tau + rho + m))
        }
    }
}

pub fn oil_ratio(w: &World) -> f64 {
    if w.exog.oil_base > 0.0 && w.exog.oil_price.is_finite() && w.exog.oil_price > 0.0 {
        w.exog.oil_price / w.exog.oil_base
    } else {
        1.0
    }
}

/// Domestic price-level index on the oil price (p0-exog): (1+π*)^((tick + tick_offset − burn_in)/12) when
/// `oil_index_to_world` is on, else 1. It is 1 at reported month 0 and continuous across the burn-in reset.
pub fn oil_index(w: &World) -> f64 {
    if !w.cfg.oil_index_to_world {
        return 1.0;
    }
    let months = i64::from(w.tick) + i64::from(w.tick_offset) - i64::from(w.cfg.burn_in_months);
    (1.0 + w.cfg.inflation_target).powf(months as f64 / 12.0)
}

/// Real oil ratio: `oil_ratio` divided by the domestic price-level index (equal to `oil_ratio` when the index is off).
/// Real-cost channels (firms' extraction labour, which is a productivity divisor) use this, so the index creates no
/// drift in them. Nominal channels (crude cost, the Energy oil leg, gasoline, the world energy price) use `oil_ratio`.
pub fn real_oil_ratio(w: &World) -> f64 {
    oil_ratio(w) / oil_index(w)
}

/// Share of the motor-fuel price that is crude at the base oil price (p1-energy): `gas_crude_yield` × base oil ÷ 42 over
/// the full per-gallon price at base. Its oil elasticity is the fuel line's own oil elasticity, ≈ 0.5.
pub fn fuel_crude_share(cfg: &Config) -> f64 {
    let crude = cfg.gas_crude_yield * cfg.oil_base / 42.0;
    let total = cfg.gas_tax + cfg.gas_margin + crude;
    if total > 0.0 { crude / total } else { 0.0 }
}

/// Factor on the world price of sector `s`'s traded goods: for Energy, the oil ratio raised to the energy elasticity
/// (fix-trade), 1 for every other sector. With `energy_fuel_split` (p1-energy) the Energy factor is the real oil ratio
/// raised to the fuel line's oil elasticity (`world_energy_oil_elasticity` × `fuel_crude_share`, the fuel line only).
/// Real oil is used so the domestic price-level index does not enter the world price twice (p0-exog review): the world
/// price already grows at world goods inflation, so the world energy price has no drift beyond that trend.
pub fn world_sector_factor(w: &World, s: usize) -> f64 {
    if s == Sector::Energy.idx() {
        let f = if w.cfg.energy_fuel_split {
            let e = w.cfg.world_energy_oil_elasticity * fuel_crude_share(&w.cfg);
            real_oil_ratio(w).powf(e)
        } else {
            oil_ratio(w).powf(w.cfg.world_energy_oil_elasticity)
        };
        if f.is_finite() && f > 0.0 { f } else { 1.0 }
    } else {
        1.0
    }
}

/// Mean of the non-Energy sector prices (1.0 at t=0): the non-energy cost level that the fuel margin is indexed to
/// (p1-energy). The Energy price itself is excluded, so the fuel margin does not depend on Energy's own price.
pub fn non_energy_price_index(w: &World) -> f64 {
    let e = Sector::Energy.idx();
    let (sum, n) = (0..N_SECTORS)
        .filter(|&s| s != e)
        .fold((0.0, 0usize), |(sum, n), s| (sum + sector_price(w, Sector::from_idx(s)), n + 1));
    if n > 0 { sum / n as f64 } else { 1.0 }
}

/// Reference level of the non-energy price index for the fuel margin (p1-energy): (1 + π*)^(burn-in / 12). The
/// non-energy index is 1 at tick 0 and drifts at about π* over the burn-in, so it is about this level at the first
/// reported month. Dividing by it puts the margin in $/gal at the reported start, where the non-energy index is near
/// its trend, and gives the $3.30 pump price at $65 WTI documented in `config.rs`.
pub fn fuel_margin_ref(cfg: &Config) -> f64 {
    (1.0 + cfg.inflation_target).powf(f64::from(cfg.burn_in_months) / 12.0)
}

/// Motor-fuel price index for the Energy price (p1-energy), 1.0 at the anchor (tick 0, where the non-energy index is 1).
/// `oil_rel` is the oil ratio and `oil_rel0` the oil ratio at the anchor; `n_index` the non-energy price index
/// (`non_energy_price_index`, 1 at tick 0). The margin is in $/gal at the reported start (`fuel_margin_ref`).
pub fn fuel_index(cfg: &Config, oil_rel: f64, oil_rel0: f64, n_index: f64) -> f64 {
    let k = cfg.oil_base / 42.0 * cfg.gas_crude_yield;
    let n_ref = fuel_margin_ref(cfg);
    let num = cfg.gas_tax + cfg.gas_margin * n_index / n_ref + k * oil_rel;
    let den = cfg.gas_tax + cfg.gas_margin / n_ref + k * oil_rel0;
    if den > 0.0 && num.is_finite() { (num / den).max(0.0) } else { 1.0 }
}

/// Retail gasoline price, $/gal. Without `energy_fuel_split`: the Energy price plus the oil price (integrate). With it
/// (p1-energy): the motor-fuel line, fixed taxes + a margin indexed to the non-energy price level (divided by
/// `fuel_margin_ref`, so the margin is in $/gal at the reported start) + the crude pass-through `gas_crude_yield` × oil
/// price ÷ 42. About $3.30 at $65 WTI when the non-energy index is at its reference (`fuel_margin_ref`, about 1.27 at the
/// 144-month burn-in), which is the reported month 0 on average.
pub fn gasoline_price(w: &World) -> f64 {
    let cfg = &w.cfg;
    if cfg.energy_fuel_split {
        let margin = cfg.gas_margin * non_energy_price_index(w) / fuel_margin_ref(cfg);
        cfg.gas_tax + margin + cfg.gas_crude_yield * w.exog.oil_price / 42.0
    } else {
        // ~$1.25 refining/distribution/taxes + crude pass-through (42 gal/bbl, ~0.66 crude yield factor)
        1.25 * w.prices.sector[Sector::Energy.idx()].max(0.1) + w.exog.oil_price / 42.0 * 1.0
    }
}

/// rz12: one month of a cascade of `stages` exponential smoothers in series (a gamma-shaped delay). Each stage moves
/// `1 / (1 + lag / stages)` of the way to its input, and a stage's input is the already-updated value of the stage before it, so the
/// mean delay is `lag` months whatever the number of stages and the weight on a change that is `k` months old rises to a hump
/// instead of falling from the first month. `state[k]` is stage k's value (`None` until first set: it then takes its input).
/// Returns the last stage. `stages` is clamped to 1..=4. With one stage this is the smoother `p + (x - p) / (1 + lag)`.
pub fn lag_cascade(state: &mut [Option<f64>; 4], stages: usize, lag: f64, x: f64) -> f64 {
    let n = stages.clamp(1, 4);
    let per = lag / n as f64;
    let mut input = x;
    for slot in state.iter_mut().take(n) {
        let out = match *slot {
            Some(p) if per > 0.0 && p.is_finite() => p + (input - p) / (1.0 + per),
            _ => input,
        };
        *slot = Some(out);
        input = out;
    }
    input
}

/// Twelve-month change in the pump price, as a fraction, from the monthly history kept in `firms_state.pump_hist`
/// (p1-energy). 0 until thirteen months exist. This is the expected-inflation salience term (`Config::pump_salience`).
pub fn pump_yoy(w: &World) -> f64 {
    let h = &w.firms_state.pump_hist;
    match h.as_slice() {
        [first, .., last] if h.len() == PUMP_HIST_LEN && *first > 0.0 => finite_or_zero(last / first - 1.0),
        _ => 0.0,
    }
}

/// Months of pump-price history kept (13 gives one year-on-year change).
pub const PUMP_HIST_LEN: usize = 13;

fn finite_or_zero(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

/// Mincer human capital (docs/DESIGN.md §6): edu premium × skill × exp(0.035x − 0.0006x²),
/// x = years since end of schooling (≥ 0).
pub fn human_capital(cfg: &Config, hh: &Household) -> f64 {
    let (prem, school_end) = match hh.education {
        Education::HighSchool => (cfg.edu_premium[0], 18.0),
        Education::College => (cfg.edu_premium[1], 22.0),
        Education::Graduate => (cfg.edu_premium[2], 24.0),
    };
    let x = (hh.age_years() - school_end).max(0.0);
    prem * hh.skill as f64 * (0.035 * x - 0.0006 * x * x).exp()
}

/// Remove a worker from their employer (if any) and set a new status. Keeps `firm.workers` consistent.
/// When the new status is Unemployed, starts benefit eligibility.
pub fn separate_worker(w: &mut World, h: HhId, new_status: LaborStatus) {
    let benefit_months = w.cfg.benefit_months;
    let hh = &mut w.households[h as usize];
    if let Some(f) = hh.employer.take() {
        let fm = &mut w.firms[f as usize];
        if let Some(pos) = fm.workers.iter().position(|&x| x == h) {
            fm.workers.swap_remove(pos);
        }
    }
    if hh.status == LaborStatus::Employed {
        hh.last_wage = hh.wage;
    }
    hh.wage = 0.0;
    hh.months_in_job = 0;
    hh.months_employed = 0;
    if new_status == LaborStatus::Unemployed && hh.status == LaborStatus::Employed {
        hh.benefit_months_left = benefit_months.min(u16::MAX as u32) as u16;
        hh.months_unemployed = 0;
        hh.bls_unemployed = 0;
    }
    hh.status = new_status;
}

/// Hire a worker at a wage. Caller must ensure the worker is not employed elsewhere.
pub fn hire_worker(w: &mut World, h: HhId, f: FirmId, wage: f64) {
    // Round 7 step 6: a job-to-job move keeps the continuous-employment count; a hire from another status starts at 0.
    let (was_emp, me) = (w.households[h as usize].employer.is_some(), w.households[h as usize].months_employed);
    if was_emp {
        separate_worker(w, h, LaborStatus::Unemployed);
    }
    let hh = &mut w.households[h as usize];
    hh.months_employed = if was_emp { me } else { 0 };
    hh.employer = Some(f);
    hh.status = LaborStatus::Employed;
    hh.wage = wage;
    hh.months_in_job = 0;
    hh.months_unemployed = 0;
    hh.bls_unemployed = 0;
    hh.benefit_months_left = 0;
    w.firms[f as usize].workers.push(h);
}

/// Annual income tax (federal progressive + flat state) on annual taxable income before deduction.
/// `add` adds a flat rate to every bracket (IncomeTax shock).
pub fn income_tax_annual(cfg: &Config, gross_annual: f64, add: f64) -> f64 {
    income_tax_annual_indexed(cfg, gross_annual, add, 1.0)
}

/// Income tax with brackets and the standard deduction indexed by `index` (CPI-based COLA index, 1.0 at t=0), as
/// US federal brackets are: tax = index × T(gross / index).
pub fn income_tax_annual_indexed(cfg: &Config, gross_annual: f64, add: f64, index: f64) -> f64 {
    let ix = if index > 0.0 { index } else { 1.0 };
    ix * income_tax_base(cfg, gross_annual / ix, add)
}

fn income_tax_base(cfg: &Config, gross_annual: f64, add: f64) -> f64 {
    let taxable = (gross_annual - cfg.standard_deduction).max(0.0);
    let mut tax = 0.0;
    let mut lower = 0.0;
    for &(upper, rate) in &cfg.tax_brackets {
        if taxable <= lower {
            break;
        }
        let slice = taxable.min(upper) - lower;
        tax += slice * (rate + add).max(0.0);
        lower = upper;
    }
    tax + taxable * cfg.state_income_tax
}

/// Social-security primary insurance amount (monthly) from career earnings in base-year dollars (2025 bend points).
/// AIME = career earnings / max(months, 420) is put in current dollars by the average wage index (100 at t=0), and the
/// bend points move with the same index, as in the SSA formula (wage indexing up to eligibility). After the first
/// payment, pensions rise with the CPI COLA (fiscal::pay_transfers).
pub fn social_security_pia(career_earnings: f64, career_months: u32, wage_index: f64) -> f64 {
    let scale = wage_index / 100.0;
    let aime = career_earnings / (career_months.max(420) as f64) * scale;
    let b1 = 1_226.0 * scale;
    let b2 = 7_391.0 * scale;
    let pia = 0.9 * aime.min(b1) + 0.32 * (aime.min(b2) - b1).max(0.0) + 0.15 * (aime - b2).max(0.0);
    pia.max(0.0)
}

/// Monthly mortality hazard at a given age (Gompertz–Makeham, docs §5).
pub fn mortality_monthly(age_years: f64) -> f64 {
    ((0.0005 + 0.00003 * (0.094 * age_years).exp()) / 12.0).min(1.0)
}

/// Approximate remaining life expectancy in years at a given age.
pub fn life_expectancy(age_years: f64) -> f64 {
    let mut surv = 1.0;
    let mut e = 0.0;
    let mut a = age_years;
    while a < 110.0 && surv > 1e-4 {
        let q = 1.0 - (1.0 - mortality_monthly(a)).powi(12);
        surv *= 1.0 - q;
        e += surv;
        a += 1.0;
    }
    e.max(1.0)
}

/// Hedonic market value of a house at the current HPI.
#[inline]
pub fn house_value(w: &World, house: HouseId) -> f64 {
    w.prices.hpi * w.houses[house as usize].quality
}

/// Outstanding mortgage principal on a house (0 if none).
pub fn mortgage_balance(w: &World, house: HouseId) -> f64 {
    w.houses[house as usize]
        .mortgage
        .and_then(|l| w.loans.get(l))
        .map(|l| l.principal)
        .unwrap_or(0.0)
}

/// Price at which firms' capital is valued: `Config::capital_valuation_mix` over the sector prices `p`.
#[inline]
pub fn capital_price(cfg: &Config, p: &[f64; N_SECTORS]) -> f64 {
    role_mean(&cfg.capital_valuation_mix, |s| p[s])
}

/// `capital_price` over the last cleared sector prices.
#[inline]
pub fn capital_price_world(w: &World) -> f64 {
    role_mean(&w.cfg.capital_valuation_mix, |s| sector_price(w, Sector::from_idx(s)))
}

/// Book equity of the alive firms each household owns, indexed by household id: capital × capital price + deposits −
/// outstanding loan principal. One pass over firms and loans; callers that need many net worths compute it once.
pub fn owner_equity(w: &World) -> Vec<f64> {
    let p_goods = capital_price_world(w);
    let mut eq = vec![0.0; w.households.len()];
    for fm in w.firms.iter().filter(|f| f.alive) {
        let Some(o) = fm.owner else { continue };
        let principal: f64 = fm.loans.iter().filter_map(|&l| w.loans.get(l)).map(|l| l.principal).sum();
        if let Some(e) = eq.get_mut(o as usize) {
            *e += fm.capital.max(0.0) * p_goods + fm.deposits - principal;
        }
    }
    eq
}

/// Household net worth: deposits + fund shares + houses − mortgages on them + book equity of owned firms
/// (`owner_equity`, indexed by household id).
pub fn net_worth(w: &World, h: HhId, owner_equity: &[f64]) -> f64 {
    let hh = &w.households[h as usize];
    let mut nw = hh.deposits + hh.shares * w.fund.share_price;
    for &p in &hh.properties {
        nw += house_value(w, p) - mortgage_balance(w, p);
    }
    nw + owner_equity.get(h as usize).copied().unwrap_or(0.0)
}

/// Sector price (last cleared), defaulting to 1.0.
#[inline]
pub fn sector_price(w: &World, s: Sector) -> f64 {
    let p = w.prices.sector[s.idx()];
    if p > 0.0 {
        p
    } else {
        1.0
    }
}

/// Nominal GDP per month in model units (per-agent dollars, unscaled), from the last recorded metrics;
/// before any history exists, estimated as 2.0 × the wage bill (the model's GDP / wage-bill ratio, with housing
/// services and indirect taxes included as in BEA GDP).
pub fn gdp_month_model(w: &World) -> f64 {
    if let Some(m) = w.last() {
        if m.gdp_nominal_bn_ar > 0.0 {
            return m.gdp_nominal_bn_ar * 1e9 / 12.0 / w.cfg.effective_scale();
        }
    }
    let wages: f64 = w.households.iter().filter(|h| h.alive).map(|h| h.wage).sum();
    2.0 * wages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    // p1-energy: the motor-fuel line at base is about $3.30 a gallon at $65 oil (US $3.26), and its crude share is
    // about half of the fuel price.
    #[test]
    fn gasoline_at_base_oil_is_near_us_pump_price() {
        let cfg = Config { n_households: 60, energy_fuel_split: true, ..Default::default() };
        let mut w = tiny_world(cfg, 3);
        w.exog.oil_price = w.exog.oil_base;
        // Non-energy index at its burn-in reference (the reported start), where the margin is $1.00/gal.
        let n_ref = fuel_margin_ref(&w.cfg);
        for p in w.prices.sector.iter_mut() {
            *p = n_ref;
        }
        let g = gasoline_price(&w);
        assert!((g - 3.30).abs() < 0.05, "gasoline {g}");
        assert!((fuel_crude_share(&w.cfg) - 0.525).abs() < 0.01);
    }

    // p1-energy: at tick 0 (non-energy index 1) the fuel margin is the reference-scaled $/gal, so the fuel index is 1 at
    // the anchor and the pump price is lower than at the reported start by the drift of the non-energy level.
    #[test]
    fn fuel_margin_is_scaled_to_reported_start() {
        let cfg = Config { energy_fuel_split: true, ..Default::default() };
        let mut w = tiny_world(cfg, 3);
        // tiny_world has no burn-in; restore the 144-month burn-in so the reference is (1 + π*)^12.
        w.cfg.burn_in_months = 144;
        let n_ref = fuel_margin_ref(&w.cfg);
        assert!((n_ref - 1.02f64.powf(12.0)).abs() < 1e-9);
        w.exog.oil_price = w.exog.oil_base;
        for p in w.prices.sector.iter_mut() {
            *p = 1.0;
        }
        let g0 = gasoline_price(&w);
        assert!((g0 - (w.cfg.gas_tax + w.cfg.gas_margin / n_ref + w.cfg.gas_crude_yield * w.cfg.oil_base / 42.0)).abs() < 1e-12);
        assert!(g0 < 3.30);
    }

    // p1-energy: the fuel index is 1 at the anchor, and moves by the crude pass-through (1.12/42 $/gal per $/bbl).
    #[test]
    fn fuel_index_is_one_at_anchor_and_has_crude_pass_through() {
        let cfg = Config::default();
        assert!((fuel_index(&cfg, 1.0, 1.0, 1.0) - 1.0).abs() < 1e-12);
        let up = fuel_index(&cfg, 1.1, 1.0, 1.0);
        let k = cfg.gas_crude_yield * cfg.oil_base / 42.0;
        let n_ref = fuel_margin_ref(&cfg);
        assert!((up - (1.0 + 0.1 * k / (cfg.gas_tax + cfg.gas_margin / n_ref + k))).abs() < 1e-12);
    }

    // p0-exog review: with the index on and no oil noise, the world energy factor has no trend against the target
    // price level (the real oil ratio is used, so the domestic index does not enter the world price twice).
    #[test]
    fn world_energy_factor_has_no_trend_with_index_on() {
        let cfg = Config {
            n_households: 60,
            energy_fuel_split: true,
            oil_index_to_world: true,
            stochastic_oil: false,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 4);
        let mut factors = Vec::new();
        let e = Sector::Energy.idx();
        for t in 0..120u32 {
            w.tick = t;
            w.tick_offset = 0;
            w.exog.oil_price = w.exog.oil_base * oil_index(&w);
            factors.push(world_sector_factor(&w, e).ln());
        }
        let first = factors[0];
        let last = factors[119];
        assert!((last - first).abs() < 1e-9, "world energy factor drifts: {first} -> {last}");
    }

    // Round 7 step 6: continuous employment survives a job-to-job move, and only separation or a hire from a
    // non-employed status resets it.
    #[test]
    fn job_to_job_hire_keeps_continuous_employment() {
        let mut w = tiny_world(Config { n_households: 200, ..Default::default() }, 3);
        assert!(w.firms.len() >= 2);
        let h: HhId = 0;
        hire_worker(&mut w, h, 0, 3_000.0);
        w.households[h as usize].months_employed = 20;
        hire_worker(&mut w, h, 1, 3_200.0);
        let hh = &w.households[h as usize];
        assert_eq!(hh.employer, Some(1));
        assert_eq!(hh.months_employed, 20);
        assert_eq!(hh.months_in_job, 0);
    }

    #[test]
    fn separation_resets_continuous_employment() {
        let mut w = tiny_world(Config { n_households: 200, ..Default::default() }, 3);
        let h: HhId = 0;
        hire_worker(&mut w, h, 0, 3_000.0);
        w.households[h as usize].months_employed = 20;
        separate_worker(&mut w, h, LaborStatus::Unemployed);
        assert_eq!(w.households[h as usize].months_employed, 0);
    }

    #[test]
    fn hire_from_unemployed_starts_continuous_employment_at_zero() {
        let mut w = tiny_world(Config { n_households: 200, ..Default::default() }, 3);
        let h: HhId = 0;
        separate_worker(&mut w, h, LaborStatus::Unemployed);
        w.households[h as usize].months_employed = 15;
        hire_worker(&mut w, h, 0, 3_000.0);
        assert_eq!(w.households[h as usize].months_employed, 0);
    }

    // rz12: the perception cascade. One stage is the single exponential smoother bit for bit.
    #[test]
    fn lag_cascade_one_stage_is_the_single_smoother() {
        let mut st = [None; 4];
        let mut p = 0.05f64;
        assert_eq!(lag_cascade(&mut st, 1, 9.0, 0.05).to_bits(), 0.05f64.to_bits(), "first call takes the input");
        for x in [0.06, 0.07, 0.06, 0.05] {
            p = p + (x - p) / (1.0 + 9.0);
            assert_eq!(lag_cascade(&mut st, 1, 9.0, x).to_bits(), p.to_bits());
        }
        assert_eq!(lag_cascade(&mut [None; 4], 2, 0.0, 0.07).to_bits(), 0.07f64.to_bits(), "lag 0 passes the input through at any stage count");
    }

    // rz12: two stages of lag/2 each keep the mean delay `lag` but weight a change by a hump (zero-ish at once, largest a few months later).
    #[test]
    fn lag_cascade_two_stages_keep_the_mean_delay_and_make_a_hump() {
        let lag = 10.0;
        let impulse = |stages: usize| -> Vec<f64> {
            // response of the output to a unit step at month 0 from rest, differenced into the weight on each month's change
            let mut st = [None; 4];
            lag_cascade(&mut st, stages, lag, 0.0);
            let mut out = Vec::new();
            let mut prev = 0.0;
            for _ in 0..400 {
                let y = lag_cascade(&mut st, stages, lag, 1.0);
                out.push(y - prev);
                prev = y;
            }
            out
        };
        for stages in [1usize, 2, 3, 4] {
            let w = impulse(stages);
            let total: f64 = w.iter().sum();
            let mean: f64 = w.iter().enumerate().map(|(k, x)| k as f64 * x).sum::<f64>() / total;
            assert!((total - 1.0).abs() < 1e-9, "weights sum to 1 ({stages} stages)");
            assert!((mean - lag).abs() < 1e-6, "mean delay {mean} for {stages} stages");
        }
        let one = impulse(1);
        let two = impulse(2);
        assert!(one[0] > one[1] && one[1] > one[2], "one stage: the weight falls from the first month");
        let (peak, _) = two.iter().enumerate().fold((0usize, 0.0f64), |(bi, bv), (i, &v)| if v > bv { (i, v) } else { (bi, bv) });
        assert!(peak >= 3 && peak <= 6, "two stages: hump at {peak}");
        assert!(two[0] < one[0] / 2.0, "less weight on the first month");
        let var = |w: &[f64]| { let m: f64 = w.iter().enumerate().map(|(k, x)| k as f64 * x).sum(); w.iter().enumerate().map(|(k, x)| (k as f64 - m).powi(2) * x).sum::<f64>() };
        assert!(var(&two) < var(&one), "two stages are less dispersed than one");
    }

    // rz12: a stage that has not been set takes its input (every stage on the first call), and `stages` is clamped to 1..=4.
    #[test]
    fn lag_cascade_first_call_sets_every_stage_and_clamps() {
        let mut st = [None; 4];
        assert_eq!(lag_cascade(&mut st, 4, 9.0, 0.04).to_bits(), 0.04f64.to_bits());
        assert!(st.iter().all(|s| *s == Some(0.04)));
        let mut st9 = [None; 4];
        let mut st4 = [None; 4];
        for x in [0.04, 0.05, 0.03] {
            assert_eq!(lag_cascade(&mut st9, 9, 9.0, x).to_bits(), lag_cascade(&mut st4, 4, 9.0, x).to_bits(), "more than 4 stages are 4");
        }
        let mut st0 = [None; 4];
        let mut st1 = [None; 4];
        for x in [0.04, 0.05, 0.03] {
            assert_eq!(lag_cascade(&mut st0, 0, 9.0, x).to_bits(), lag_cascade(&mut st1, 1, 9.0, x).to_bits(), "0 stages are 1");
        }
    }
}
