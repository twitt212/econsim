//! Aggregate metrics recorded each month and distribution snapshots (docs/DESIGN.md §14).
//!
//! Conventions: money aggregates are **scaled to the US economy** (× cfg.effective_scale()) and expressed in
//! **billions of dollars at annual rates** (`_bn_ar`) or **billions** for stocks (`_bn`). Rates and inflation
//! are percentages (4.2 = 4.2%). Indices are levels (100 at t=0 where noted).

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::types::*;
use crate::common;

pub const AGE_BUCKETS: [(u32, u32); 6] = [(18, 24), (25, 34), (35, 44), (45, 54), (55, 64), (65, 200)];

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub tick: u32,
    /// Calendar label, e.g. 2026.0 = Jan 2026.
    pub date: f64,

    // --- output ---
    pub gdp_nominal_bn_ar: f64,
    pub gdp_real_bn_ar: f64,
    /// Real change in private inventories, $bn annualised (part of GDP).
    #[serde(default)]
    pub inventory_change_real_bn_ar: f64,
    /// Real household purchases of goods and services (excluding housing services, in-kind health and FISIM), $bn ar.
    #[serde(default)]
    pub household_purchases_real_bn_ar: f64,
    /// Real household purchases by consumer sector (Energy … OtherServices; Construction is never bought by
    /// households), $bn annualised, deflated by each sector's price relative to the base month (as consumption).
    #[serde(default)]
    pub consumption_by_sector_real: [f64; N_CONSUMER],
    /// Mean discretionary-spending multiplier D across alive households this month (1 = no effect; consumption.rs).
    #[serde(default)]
    pub discretionary_multiplier_mean: f64,
    pub gdp_real_growth_yoy: f64,
    pub consumption_real_bn_ar: f64,
    /// Personal consumption expenditures, nominal ($bn, annual rate).
    #[serde(default)]
    pub consumption_nominal_bn_ar: f64,
    pub investment_real_bn_ar: f64,
    pub residential_investment_bn_ar: f64,
    pub government_bn_ar: f64,
    pub exports_bn_ar: f64,
    pub imports_bn_ar: f64,
    // --- b7-tariff: tariff outputs (zero with no duties; see `tariff_pass_through`) ---
    /// b7-tariff: statutory tariff τ in percent.
    #[serde(default)]
    pub tariff_rate_pct: f64,
    /// b7-tariff: retail wedge r in percent (share of the duty passed to the retail price so far).
    #[serde(default)]
    pub tariff_retail_pct: f64,
    /// b7-tariff: all duties collected this month (household wedge, absorbed, inputs, equipment; legacy path: the
    /// lump-sum duty), annualised.
    #[serde(default)]
    pub tariff_revenue_bn_ar: f64,
    /// b7-tariff: duty absorbed by domestic firms (signed), annualised.
    #[serde(default)]
    pub tariff_absorbed_bn_ar: f64,
    /// b7-tariff: landed import price index (t=0 = 100), with the retail wedge.
    #[serde(default)]
    pub import_price_index: f64,
    /// b7-tariff: household import share of tradables consumption, percent (CPI weights).
    #[serde(default)]
    pub hh_import_share_pct: f64,
    /// b7-tariff: imported share of firms' tradables inputs, percent.
    #[serde(default)]
    pub firm_input_import_share_pct: f64,
    /// Proprietors' income: profits of owner-operated firms paid to their owner households (BEA NIPA 2.1 concept).
    pub proprietors_income_bn_ar: f64,
    pub capacity_utilization: f64,

    // --- prices ---
    pub cpi: f64,
    pub core_cpi: f64,
    pub inflation_yoy: f64,
    pub core_inflation_yoy: f64,
    /// Core PCE-weighted inflation, yoy %.
    #[serde(default)]
    pub core_pce_inflation_yoy: f64,
    pub energy_inflation_yoy: f64,
    pub shelter_inflation_yoy: f64,
    pub inflation_mom_annualized: f64,
    pub expected_inflation: f64,
    pub sector_prices: [f64; N_SECTORS],
    pub oil_price: f64,
    pub gasoline_price: f64,
    /// Trade-weighted dollar index (100 at t=0; higher = stronger dollar).
    #[serde(default)]
    pub dollar_index: f64,

    // --- labour ---
    pub unemployment_rate: f64,
    pub participation_rate: f64,
    /// Employed household heads (millions, scaled).
    pub employment_millions: f64,
    /// Employed persons (millions): heads × CPS employed-persons-per-household ratio (≈1.21).
    #[serde(default)]
    pub employment_persons_millions: f64,
    pub vacancies_millions: f64,
    pub vacancy_rate: f64,
    pub hires_rate: f64,
    pub layoffs_rate: f64,
    pub avg_wage_annual: f64,
    pub median_wage_annual: f64,
    pub wage_growth_yoy: f64,
    pub real_wage_index: f64,
    pub wage_by_age: [f64; 6],
    pub unemployment_by_age: [f64; 6],
    pub employment_by_sector: [f64; N_SECTORS],

    // --- rates ---
    pub policy_rate: f64,
    pub yields: [f64; N_TENORS],
    pub term_premium: [f64; N_TENORS],
    pub mortgage_rate: f64,
    pub avg_outstanding_mortgage_rate: f64,
    pub deposit_rate: f64,
    pub business_loan_rate: f64,
    /// Average business loan rate less the policy rate (pp): the credit spread.
    pub credit_spread: f64,
    /// Trailing-12-month business write-offs as % of business loans (annualised loss rate).
    pub loan_loss_rate: f64,

    // --- housing ---
    pub hpi: f64,
    pub median_house_price: f64,
    pub hpi_growth_yoy: f64,
    pub rent_index: f64,
    pub median_rent: f64,
    pub house_sales_millions_ar: f64,
    pub housing_starts_millions_ar: f64,
    pub homeownership_rate: f64,
    pub homeownership_by_age: [f64; 6],
    pub foreclosures_thousands_ar: f64,
    pub listings: f64,
    pub months_supply: f64,
    pub vacancy_rate_housing: f64,
    pub price_to_income: f64,

    // --- credit & banks ---
    pub mortgage_debt_bn: f64,
    pub business_debt_bn: f64,
    pub new_mortgages_bn_ar: f64,
    pub bank_capital_ratio: f64,
    pub bank_profits_bn_ar: f64,
    pub loan_losses_bn_ar: f64,
    pub deposits_bn: f64,
    pub reserves_bn: f64,

    // --- central bank & government ---
    pub cb_assets_bn: f64,
    pub cb_bonds_bn: f64,
    pub qe_purchases_bn_ar: f64,
    pub helicopter_bn_ar: f64,
    pub gov_debt_bn: f64,
    pub gov_debt_gdp: f64,
    pub deficit_gdp: f64,
    /// Pension (Social Security) payments, percent of nominal GDP.
    #[serde(default)]
    pub social_security_gdp: f64,
    /// Health purchases (Medicare + Medicaid, in kind), percent of nominal GDP.
    #[serde(default)]
    pub health_gdp: f64,
    /// Unemployment insurance paid, percent of nominal GDP (integration: UI vs means-tested split).
    #[serde(default)]
    pub ui_gdp: f64,
    /// Government transfers, percent of nominal GDP: `w.gov.transfers`, the Treasury's total transfer outlays, which
    /// includes UI and Social Security. `means_tested_gdp` is the means-tested part alone.
    #[serde(default)]
    pub transfers_gdp: f64,
    /// Means-tested cash and near-cash support (EITC, SNAP, SSI, TANF, housing aid), percent of nominal GDP. Only
    /// `income.transfers` is credited here; the helicopter is credited to `windfall` (Round 7 step 0).
    #[serde(default)]
    pub means_tested_gdp: f64,
    /// Homeownership on the HVS basis: owner-occupied / occupied units (owners + renters with a home), percent.
    /// `homeownership_rate` keeps the household-basis denominator (`in_tenure_base`).
    #[serde(default)]
    pub homeownership_rate_hvs: f64,
    /// Rental vacancy on the HVS basis: vacant units for rent / (renter-occupied + vacant for rent), percent.
    #[serde(default)]
    pub rental_vacancy_rate_hvs: f64,
    /// Adults without a home, percent of alive households: WithParents aged 25+ plus renters with no home.
    #[serde(default)]
    pub unhoused_share: f64,
    /// Household mortgages 90+ days delinquent (`months_delinquent >= 3`), percent of household mortgages.
    #[serde(default)]
    pub mortgage_delinquent_90: f64,
    /// Renter evictions this month, annualised, thousands of US households.
    #[serde(default)]
    pub evictions_thousands_ar: f64,
    /// Owner-occupied foreclosures, annualised, thousands of US households (subset of `foreclosures_thousands_ar`).
    #[serde(default)]
    pub foreclosures_owner_occ_thousands_ar: f64,
    /// Policy rate estimate of r* (`monetary.r_star_est`, or the configured r*), percent.
    #[serde(default)]
    pub r_star_est: f64,
    /// Central-bank remittance to the Treasury this month, percent of nominal GDP (integration: fix-fiscal).
    #[serde(default)]
    pub remittance_gdp: f64,
    /// Central-bank deferred asset (negative net income not yet remitted), percent of annual nominal GDP.
    #[serde(default)]
    pub deferred_asset_gdp: f64,
    pub gov_revenue_bn_ar: f64,
    pub gov_outlays_bn_ar: f64,
    pub gov_interest_bn_ar: f64,

    // --- markets & distribution ---
    pub fund_share_price: f64,
    pub equity_value_bn: f64,
    /// Equity value ÷ annual after-tax corporate profits (firms' profit after tax, annualised). 0 when profits ≤ 0.
    #[serde(default)]
    pub pe_ratio: f64,
    /// Time-varying equity risk premium over the 10y yield, percent (equity::equity_risk_premium).
    #[serde(default)]
    pub equity_risk_premium: f64,
    /// Trailing 12-month nominal total return on the Fund's equity (capital gain + distributed equity dividends),
    /// percent; 0 until 12 months exist.
    #[serde(default)]
    pub equity_return_12m: f64,
    /// Equity value by sector, $bn (stage 4 sector valuation; Sector::ALL order; sums to equity_value_bn).
    #[serde(default)]
    pub equity_value_by_sector: [f64; N_SECTORS],
    /// Sector P/E: sector equity ÷ (12 × the sector's monthly profit); 0 where profit is not positive.
    #[serde(default)]
    pub pe_by_sector: [f64; N_SECTORS],
    /// 1 while a tech boom is in force (Markov regime or forced TechBoom shock), else 0.
    #[serde(default)]
    pub tech_boom: f64,
    /// Tech productivity growth this month, annualised, percent.
    #[serde(default)]
    pub tfp_growth_tech: f64,
    /// Gross investment by Tech firms, $bn at annual rates (capital bought by Tech firms, at capital prices).
    #[serde(default)]
    pub tech_investment_bn_ar: f64,
    pub wealth_gini: f64,
    pub income_gini: f64,
    pub top10_wealth_share: f64,
    pub median_net_worth: f64,
    /// Real consumption growth yoy by income quintile (bottom → top), percent.
    pub consumption_by_quintile: [f64; 5],
    pub real_consumption_per_hh: f64,

    // --- firms ---
    pub firms_alive: f64,
    pub bankruptcies_ar: f64,
    pub avg_markup: f64,
    pub profit_share: f64,

    // --- demography ---
    pub population_households_m: f64,
    pub mean_age: f64,
    pub retirees_share: f64,
    /// Alive household heads aged 65 and over, percent of alive households.
    #[serde(default)]
    pub age65_share: f64,
    /// Alive partnered (married or cohabiting) households, percent of alive households.
    #[serde(default)]
    pub partnered_share: f64,

    // --- b5-udur: unemployment duration (alive Unemployed households; 0 when there are none) ---
    /// Mean duration in weeks: (mean months unemployed + 0.5) × 4.345.
    #[serde(default)]
    pub dur_mean_weeks: f64,
    /// Grouped median duration in weeks (each whole month m is the bin [m, m+1)).
    #[serde(default)]
    pub dur_median_weeks: f64,
    /// Share of unemployed with 6+ months unemployed (27+ weeks), percent.
    #[serde(default)]
    pub ltu_share: f64,
    /// b6-reentry: the same three statistics on the BLS search-spell clock (`Household::bls_unemployed`, restarted at
    /// every U→N re-entry, resumed or not). The resumed clock above counts a re-entrant's prior spell.
    #[serde(default)]
    pub dur_mean_weeks_bls: f64,
    #[serde(default)]
    pub dur_median_weeks_bls: f64,
    #[serde(default)]
    pub ltu_share_bls: f64,
    /// Monthly job finding by duration bucket (1–3, 4–6, 7–12, 13+ months), percent: hires ÷ start-of-month stock.
    #[serde(default)]
    pub jf_by_dur_pct: [f64; 4],
    /// Raw counts by duration bucket: unemployed stock and hires from unemployment (for pooling).
    #[serde(default)]
    pub u_dur_n: [f64; 4],
    #[serde(default)]
    pub hires_u_dur_n: [f64; 4],

    /// 0 when ledger consistency holds.
    pub sfc_error: f64,

    /// Observation fields (metrics-out, step 0b). Flattened: each is a top-level key in the JSON record.
    #[serde(flatten, default)]
    pub obs: Observations,

    // b7-spikes (diagnostic keys, no effect on the economy): per-sector real CIPI before any exclusion, the two import
    // sources, and the largest bankrupt firm as a percentage of employed heads.
    #[serde(default)]
    pub cipi_by_sector_real_bn_ar: [f64; N_SECTORS],
    #[serde(default)]
    pub firm_shortfall_imports_bn_ar: f64,
    #[serde(default)]
    pub hh_unmet_imports_bn_ar: f64,
    #[serde(default)]
    pub bk_max_heads_pct: f64,
    // b8-world H11: the world block's monthly metrics; absent (not serialised) when the block is off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<crate::world::wmetrics::WorldMetrics>,
}

impl Metrics {
    /// Field-wise mean over an iterator of metrics (used to average seeds).
    pub fn mean<'a>(it: impl Iterator<Item = &'a Metrics>, n: f64) -> Metrics {
        let mut acc: Option<serde_json::Value> = None;
        let mut tick = 0u32;
        for m in it {
            if acc.is_none() {
                tick = m.tick;
            }
            let v = serde_json::to_value(m).expect("metrics serialize");
            acc = Some(match acc {
                None => v,
                Some(a) => add_json(a, &v),
            });
        }
        let Some(a) = acc else { return Metrics::default() };
        let mut a = scale_json(a, 1.0 / n);
        // `tick` is an integer and identical across seeds: keep it exact instead of averaging it as a float
        // (a fractional u32 would fail to deserialize).
        if let serde_json::Value::Object(map) = &mut a {
            map.insert("tick".to_string(), serde_json::json!(tick));
        }
        serde_json::from_value(a).expect("averaged metrics deserialize")
    }
}

fn add_json(a: serde_json::Value, b: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value::*;
    match (a, b) {
        (Number(x), Number(y)) => {
            serde_json::json!(x.as_f64().unwrap_or(0.0) + y.as_f64().unwrap_or(0.0))
        }
        // A null (an undefined observation) in any seed makes the mean undefined too.
        (Null, _) | (_, Null) => Null,
        (Array(xs), Array(ys)) => Array(xs.into_iter().zip(ys).map(|(x, y)| add_json(x, y)).collect()),
        (Object(mut xs), Object(ys)) => {
            for (k, y) in ys {
                if let Some(x) = xs.remove(k) {
                    xs.insert(k.clone(), add_json(x, y));
                }
            }
            Object(xs)
        }
        (a, _) => a,
    }
}

/// b5-udur: duration summary of the alive unemployed. Integer accumulation over a 241-bin histogram (months 240 and
/// over share the last bin); draws no random numbers and changes no state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DurationStats {
    pub mean_weeks: f64,
    pub median_weeks: f64,
    pub ltu_share: f64,
}

pub fn duration_stats(hh: &[Household]) -> DurationStats {
    duration_stats_on(hh, false)
}

/// b6-reentry: duration statistics on the BLS search-spell clock (`bls = true`) or the resumed clock (`bls = false`).
pub fn duration_stats_on(hh: &[Household], bls: bool) -> DurationStats {
    let mut hist = vec![0u32; 241];
    let (mut n, mut sum, mut ltu) = (0u64, 0u64, 0u64);
    for h in hh.iter().filter(|h| h.alive && h.status == LaborStatus::Unemployed) {
        let m = if bls { h.bls_unemployed } else { h.months_unemployed };
        hist[(m as usize).min(240)] += 1;
        n += 1;
        sum += u64::from(m);
        if m >= 6 {
            ltu += 1;
        }
    }
    if n == 0 {
        return DurationStats::default();
    }
    let half = n as f64 / 2.0;
    let mut cum = 0.0;
    let mut median = 0.0;
    for (m, &c) in hist.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let f = f64::from(c);
        if cum + f >= half {
            median = m as f64 + (half - cum) / f;
            break;
        }
        cum += f;
    }
    let nf = n as f64;
    DurationStats {
        mean_weeks: (sum as f64 / nf + 0.5) * 4.345,
        median_weeks: median * 4.345,
        ltu_share: 100.0 * ltu as f64 / nf,
    }
}

/// Deserialises a nullable observation: null (an undefined value, written from NaN) reads back as NaN, so a
/// serialised record, and a seed mean of such records, deserialises into `Metrics` again.
fn de_null_nan<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    Ok(Option::<f64>::deserialize(d)?.unwrap_or(f64::NAN))
}

fn scale_json(a: serde_json::Value, s: f64) -> serde_json::Value {
    use serde_json::Value::*;
    match a {
        Number(x) => serde_json::json!(x.as_f64().unwrap_or(0.0) * s),
        Array(xs) => Array(xs.into_iter().map(|x| scale_json(x, s)).collect()),
        Object(xs) => Object(xs.into_iter().map(|(k, v)| (k, scale_json(v, s))).collect()),
        o => o,
    }
}

/// Cross-sectional snapshot for the UI (distributions at the current month).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub tick: u32,
    pub age_buckets: Vec<String>,
    pub by_age_mean_income: Vec<f64>,
    pub by_age_mean_net_worth: Vec<f64>,
    pub by_age_median_net_worth: Vec<f64>,
    pub by_age_ownership: Vec<f64>,
    pub by_age_employment_rate: Vec<f64>,
    pub by_age_mortgage_rate: Vec<f64>,
    /// Net-worth deciles (upper bounds, $).
    pub net_worth_deciles: Vec<f64>,
    /// Income deciles (annual, $).
    pub income_deciles: Vec<f64>,
    /// Firm size histogram: (upper bound in persons = employees x 1.75, count) — scaled counts not applied.
    pub firm_size_hist: Vec<(u32, u32)>,
    pub sector_employment: Vec<f64>,
    pub sector_prices: Vec<f64>,
    pub sector_markups: Vec<f64>,
    pub yield_curve: Vec<(u32, f64)>,
    pub house_price_deciles: Vec<f64>,
    /// Sample of individual households for the agent explorer (at most 200).
    pub sample_households: Vec<HouseholdView>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct HouseholdView {
    pub id: u32,
    pub age: f64,
    pub education: String,
    pub status: String,
    pub tenure: String,
    pub wage_annual: f64,
    pub income_annual: f64,
    pub deposits: f64,
    pub fund_value: f64,
    pub home_value: f64,
    pub mortgage_balance: f64,
    pub mortgage_rate: f64,
    pub rent: f64,
    pub net_worth: f64,
    pub consumption_monthly: f64,
    pub employer_sector: String,
}

/// Distributional metrics (Ginis, top-10 share, median net worth, quintile consumption) are recomputed every
/// this many months and carried forward in between.
pub const DIST_EVERY: u32 = 3;
/// Share of gross rent that is housing value added (the rest — maintenance, management — is bought as Services).
const HOUSING_VALUE_ADDED_SHARE: f64 = 0.65;
/// Employed persons per employed household head (CPS: ≈160M employed / ≈132M households, adjusted for
/// households without an employed head).
const PERSONS_PER_EMPLOYED_HEAD: f64 = 1.75;

/// rz1 F1b: smallest annual margin (10 bp) a leg must reach before its margin becomes the base margin.
const FISIM_LATCH_MIN: f64 = 0.001;

/// rz1 F1b: the base of the constant-margin real FISIM, captured at the first recorded month of the reported window (reported
/// month 0; `sim::compare` shares the baseline arm's capture with the shocked arm). Per leg, the margin is latched on the first
/// month the leg has a margin of at least `FISIM_LATCH_MIN` on a positive stock; until then the leg follows its current margin
/// (its nominal amount), so a leg whose margin is zero or nearly zero at month 0 (a deposit leg at the policy floor: 3 of 64
/// exploratory seeds have a deposit margin under 1 bp at month 0) joins the constant-margin flow when its margin first
/// becomes meaningful and is never dropped for the run. `anchor` is the month-0 ratio of the general price index to
/// the legacy FISIM deflator: it indexes the real flow to the legacy level at month 0 (a volume index times the base value).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FisimBase {
    /// True once month 0 has been captured.
    pub captured: bool,
    /// General price index over the legacy FISIM deflator at month 0 (both relative to the model's base prices).
    pub anchor: f64,
    /// Latched deposit margin (annual, decimal); None until the deposit leg first has a positive margin.
    pub dep: Option<f64>,
    /// Latched household-loan margin (annual, decimal); None until the loan leg first has a positive margin.
    pub loan: Option<f64>,
}

/// rz1 F1b: the monthly real FISIM flow at constant margins. `dep` is the household deposits per bank (as `HhPartial::dep`).
/// `legacy_defl` is the legacy FISIM deflator (the OtherServices price relative to base) and `general_defl` the general price
/// index (the PCE deflator excluding FISIM: nominal over real consumption before the FISIM line, relative to base).
fn fisim_constant_margin_real(
    w: &World,
    dep: &[f64],
    st: &mut MetricsState,
    nominal: f64,
    legacy_defl: f64,
    general_defl: f64,
) -> f64 {
    let r_ref = w.curve.yields[0].max(0.0);
    let dep_stock: f64 = dep.iter().map(|d| d.max(0.0)).sum();
    let dep_part: f64 = dep.iter().zip(w.banks.iter()).map(|(&d, b)| d.max(0.0) * (r_ref - b.deposit_rate).max(0.0)).sum();
    let (mut loan_stock, mut loan_part) = (0.0f64, 0.0f64);
    for (_, l) in w.loans.iter().filter(|(_, l)| matches!(l.borrower, Borrower::Household(_))) {
        loan_stock += l.principal.max(0.0);
        loan_part += l.principal.max(0.0) * (l.annual_rate - r_ref).max(0.0);
    }
    fisim_real_at_base(&mut st.fisim_base, dep_stock, dep_part, loan_stock, loan_part, nominal, legacy_defl, general_defl)
}

/// Pure core of `fisim_constant_margin_real` (testable without a world). `dep_part` and `loan_part` are the current nominal
/// annual margin amounts of the two legs (stock x current margin), so a leg's current margin is `part / stock`.
///
/// * First call (reported month 0): latches the legs whose margin is at least `FISIM_LATCH_MIN`, sets the anchor and returns the
///   legacy value `nominal / legacy_defl` unchanged (bit for bit).
/// * Later calls: `anchor x (dep_stock x m_dep + loan_stock x m_loan) / 12 / general_defl`, with each leg at its latched margin.
///   A leg not yet latched contributes its current amount `part` (its nominal) until its margin first reaches `FISIM_LATCH_MIN`;
///   it latches that month and contributes `part` that month too.
/// * If a deflator is not a positive finite number the legacy deflator is used (and then the anchor is 1).
fn fisim_real_at_base(
    b: &mut FisimBase,
    dep_stock: f64,
    dep_part: f64,
    loan_stock: f64,
    loan_part: f64,
    nominal: f64,
    legacy_defl: f64,
    general_defl: f64,
) -> f64 {
    let ok = |x: f64| x.is_finite() && x > 0.0;
    let (legacy, general) = if ok(legacy_defl) && ok(general_defl) { (legacy_defl, general_defl) } else { (1.0, 1.0) };
    let latch = |slot: &mut Option<f64>, stock: f64, part: f64| -> f64 {
        match *slot {
            Some(m) => stock * m,
            None if stock > 0.0 && part >= FISIM_LATCH_MIN * stock => {
                *slot = Some(part / stock);
                part
            }
            None => part,
        }
    };
    if !b.captured {
        b.captured = true;
        b.anchor = general / legacy;
        latch(&mut b.dep, dep_stock, dep_part);
        latch(&mut b.loan, loan_stock, loan_part);
        return div(nominal, legacy_defl);
    }
    let flow = (latch(&mut b.dep, dep_stock, dep_part) + latch(&mut b.loan, loan_stock, loan_part)) / 12.0;
    b.anchor * flow / general
}

/// Persistent state for the metrics module.
#[derive(Clone, Debug, Default)]
pub struct MetricsState {
    /// True once the base prices and base real wage have been captured (first recorded month).
    pub initialized: bool,
    /// Sector prices at the first recorded month: the deflator base for real GDP.
    pub base_sector: [f64; N_SECTORS],
    /// Firms' inventory units by sector at the previous record (for the change in private inventories); None
    /// before the first record.
    pub inventory_prev: Option<[f64; N_SECTORS]>,
    /// Real average wage (nominal / CPI) at the first recorded month with employment; 0 until set.
    pub base_real_wage: f64,
    /// Market rent index at the first recorded month (housing-services deflator base).
    pub base_rent: f64,
    /// Energy-sector price, one entry per recorded month (energy inflation).
    pub energy_hist: Vec<f64>,
    /// Market rent index, one entry per recorded month.
    pub rent_hist: Vec<f64>,
    /// Lease-stock rent index (CPI rent, fix-housing review 9, fix 3): the average market rent over the last 12 months,
    /// which is the average of the rents of in-force leases when each lease renews once a year at the market rent
    /// (BLS rent sample). One entry per recorded month. Shelter inflation is its year-on-year change.
    pub cpi_rent_hist: Vec<f64>,
    /// Cached distributional metrics.
    pub dist: DistCache,
    /// True once the observation distribution statistics have been computed (annual refresh).
    pub obs_ready: bool,
    /// Annual observation state (year-end snapshots and the statistics computed from them).
    pub annual: AnnualCache,
    /// rz1 F1b: the base of the constant-margin real FISIM (margins and price anchor of the first recorded month of the reported
    /// window). Used only with `fisim_constant_margin`; `sim::compare` resets it after the warm-up.
    pub fisim_base: FisimBase,
    /// Base levels for the observation indices, captured at the first recorded month (or first sale).
    pub base_cpi: f64,
    pub base_world: f64,
    pub base_txn: f64,
    /// Transaction-price index state (carried forward in months without sales).
    pub txn_index: f64,
    /// (tick, real consumption per household by income quintile), sampled at each distributional refresh.
    pub quintile_hist: Vec<(u32, [f64; 5])>,
    /// Employer slot of each household at the previous record (u32::MAX when not employed). Metrics-only copy of
    /// model state, used to measure bankruptcy displacement without a new counter in the model.
    pub prev_employer: Vec<u32>,
    /// Firm regular plus special dividends declared at the previous record (the Fund's receipts paid next month).
    pub prev_firm_div: f64,
    /// Per firm slot: investment orders summed over the months since the last annual refresh (reset on death).
    pub inv_acc: Vec<f64>,
    /// Months accumulated into `inv_acc` since the last annual refresh.
    pub inv_months: u32,
    /// Per firm slot: capital at the previous annual refresh, i.e. at the start of the investment window in `inv_acc`
    /// (0 for dead slots and before the first refresh). Zero-filled on resize.
    pub inv_k0: Vec<f64>,
    /// Per firm slot: months of investment orders in `inv_acc` since the last refresh (a slot born mid-window has
    /// fewer than 12 and is left out of the investment shares).
    pub inv_slot_months: Vec<u32>,
    /// Per-month scratch buffers, taken out of the state while `compute` runs and handed back afterwards so the
    /// allocations are reused. Contents are cleared on use and never carried between months.
    scratch_wages: Vec<f64>,
    scratch_incomes: Vec<f64>,
    scratch_rents: Vec<f64>,
    scratch_owner_values: Vec<f64>,
    scratch_rows: Vec<(f64, f64, f64)>,
}

/// Last computed distributional metrics (carried forward between refreshes).
#[derive(Clone, Debug, Default)]
pub struct DistCache {
    pub valid: bool,
    pub wealth_gini: f64,
    pub income_gini: f64,
    pub top10_wealth_share: f64,
    pub median_net_worth: f64,
    pub consumption_by_quintile: [f64; 5],
    /// Annual distribution and firm observations (refreshed every 12th month, carried forward otherwise).
    pub obs: DistObs,
}

// ---------------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------------

/// Division that returns 0 for a zero or non-finite denominator/result.
#[inline]
fn div(a: f64, b: f64) -> f64 {
    if b != 0.0 {
        let r = a / b;
        if r.is_finite() {
            return r;
        }
    }
    0.0
}

/// Percentage growth from `prev` to `now`; 0 when `prev` is not positive.
#[inline]
fn growth_pct(now: f64, prev: f64) -> f64 {
    if prev > 0.0 {
        100.0 * (now / prev - 1.0)
    } else {
        0.0
    }
}

/// Year-over-year change in percent of a monthly series (oldest first). Uses the 12-month lag once it exists,
/// otherwise annualises the history so far (same convention as `World::inflation_yoy`).
fn yoy_pct(h: &[f64]) -> f64 {
    let n = h.len();
    if n >= 13 {
        growth_pct(h[n - 1], h[n - 13])
    } else if n >= 2 && h[0] > 0.0 && h[n - 1] > 0.0 {
        100.0 * ((h[n - 1] / h[0]).powf(12.0 / (n - 1) as f64) - 1.0)
    } else {
        0.0
    }
}

/// Median of an unsorted slice (reorders it). O(n) via selection.
fn median_mut(v: &mut [f64]) -> f64 {
    let n = v.len();
    if n == 0 {
        return 0.0;
    }
    let mid = n / 2;
    let (lower, m, _) = v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
    let upper = *m;
    if n % 2 == 1 {
        upper
    } else {
        let lower_max = lower.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        0.5 * (lower_max + upper)
    }
}

/// Elements per chunk in the chunked parallel passes (sort partition, quintile sums). Fixed, so the output never
/// depends on the thread count.
const SEL_CHUNK: usize = 4096;
/// Below this length the sort runs sequentially.
const PAR_MIN: usize = 1 << 14;
/// Evenly spaced sample size for the sort splitters.
const SPLIT_SAMPLE: usize = 512;

fn median_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Order-preserving 64-bit key of `x` under `f64::total_cmp`.
#[inline]
fn total_key(x: f64) -> u64 {
    let b = x.to_bits();
    if b >> 63 == 1 {
        !b
    } else {
        b | (1u64 << 63)
    }
}

#[inline]
fn from_total_key(k: u64) -> f64 {
    let b = if k >> 63 == 1 { k & !(1u64 << 63) } else { !k };
    f64::from_bits(b)
}

/// Sort keys ascending: LSD radix sort, six passes of 11 bits, on one reusable buffer. Linear time.
fn radix_sort_keys(keys: &mut Vec<u64>) {
    const BITS: u32 = 11;
    const MASK: u64 = (1 << BITS) - 1;
    let mut tmp = vec![0u64; keys.len()];
    let mut count = [0usize; 1 << BITS];
    for pass in 0..6u32 {
        let shift = pass * BITS;
        count.iter_mut().for_each(|c| *c = 0);
        for &k in keys.iter() {
            count[((k >> shift) & MASK) as usize] += 1;
        }
        // All keys share this digit (common within a bucket of close values): the pass would be the identity.
        if !keys.is_empty() && count[((keys[0] >> shift) & MASK) as usize] == keys.len() {
            continue;
        }
        let mut sum = 0usize;
        for c in count.iter_mut() {
            let t = *c;
            *c = sum;
            sum += t;
        }
        for &k in keys.iter() {
            let d = ((k >> shift) & MASK) as usize;
            tmp[count[d]] = k;
            count[d] += 1;
        }
        std::mem::swap(keys, &mut tmp);
    }
}

/// Sort `v` ascending in `f64::total_cmp` order (the order `sort_unstable_by(total_cmp)` produces: the sorted
/// sequence is unique, since equal keys are bit-identical).
fn radix_sort_total(v: &mut [f64]) {
    let mut keys: Vec<u64> = v.iter().map(|&x| total_key(x)).collect();
    radix_sort_keys(&mut keys);
    for (x, &k) in v.iter_mut().zip(keys.iter()) {
        *x = from_total_key(k);
    }
}

/// Buckets in the parallel sort: values are split by sampled splitters (fixed chunks, in order), each bucket is
/// radix sorted on its own, and the buckets are written back in order. Same sorted sequence as `radix_sort_total`.
const SORT_BUCKETS: usize = 4;

fn par_sort_total(v: &mut [f64]) {
    let n = v.len();
    if n < PAR_MIN {
        radix_sort_total(v);
        return;
    }
    let mut samp: Vec<u64> = (0..SPLIT_SAMPLE).map(|i| total_key(v[i * n / SPLIT_SAMPLE])).collect();
    samp.sort_unstable();
    let split: Vec<u64> = (1..SORT_BUCKETS).map(|b| samp[b * SPLIT_SAMPLE / SORT_BUCKETS]).collect();
    let parts: Vec<Vec<Vec<u64>>> = v
        .par_chunks(SEL_CHUNK)
        .map(|ch| {
            let mut out: Vec<Vec<u64>> = vec![Vec::new(); SORT_BUCKETS];
            for &x in ch {
                let k = total_key(x);
                out[split.partition_point(|&s| s <= k)].push(k);
            }
            out
        })
        .collect();
    let buckets: Vec<Vec<u64>> = (0..SORT_BUCKETS)
        .into_par_iter()
        .map(|b| {
            let mut keys: Vec<u64> = Vec::with_capacity(parts.iter().map(|p| p[b].len()).sum());
            for p in &parts {
                keys.extend_from_slice(&p[b]);
            }
            radix_sort_keys(&mut keys);
            keys
        })
        .collect();
    let mut slices: Vec<&mut [f64]> = Vec::with_capacity(SORT_BUCKETS);
    let mut rest: &mut [f64] = v;
    for b in &buckets {
        let (head, tail) = rest.split_at_mut(b.len());
        slices.push(head);
        rest = tail;
    }
    slices.into_par_iter().zip(buckets.par_iter()).for_each(|(dst, keys)| {
        for (x, &k) in dst.iter_mut().zip(keys.iter()) {
            *x = from_total_key(k);
        }
    });
}

/// Sum of `xs` in a fixed order over four interleaved lanes (lane i mod 4, lanes combined in a fixed order). The
/// four independent accumulators break the add-latency chain, and the result never depends on the thread count.
fn lane_sum(xs: &[f64]) -> f64 {
    let mut acc = [0.0f64; 4];
    let mut it = xs.chunks_exact(4);
    for c in &mut it {
        acc[0] += c[0];
        acc[1] += c[1];
        acc[2] += c[2];
        acc[3] += c[3];
    }
    for (k, &x) in it.remainder().iter().enumerate() {
        acc[k] += x;
    }
    (acc[0] + acc[1]) + (acc[2] + acc[3])
}

/// Gini coefficient (0..1) of values sorted ascending: sum_i (2i - n - 1) x_i / (n * sum x).
/// Returns 0 when the total is not positive (so equal wealth gives exactly 0).
fn gini_sorted(xs: &[f64]) -> f64 {
    let n = xs.len();
    let total: f64 = lane_sum(xs);
    if n == 0 || total <= 0.0 {
        return 0.0;
    }
    let nf = n as f64;
    // Σ_i (2i − n − 1) x_i over i = 0..n (1-based rank i+1), in the same four-lane order as `lane_sum`.
    let w = |i: usize, x: f64| (2.0 * (i as f64 + 1.0) - nf - 1.0) * x;
    let mut acc = [0.0f64; 4];
    let mut it = xs.chunks_exact(4);
    let mut i = 0usize;
    for c in &mut it {
        for k in 0..4 {
            acc[k] += w(i + k, c[k]);
        }
        i += 4;
    }
    for (k, &x) in it.remainder().iter().enumerate() {
        acc[k] += w(i + k, x);
    }
    div((acc[0] + acc[1]) + (acc[2] + acc[3]), nf * total)
}

/// Share (percent) of the total held by the top `frac` of values sorted ascending.
fn top_share(sorted: &[f64], frac: f64) -> f64 {
    let n = sorted.len();
    let total: f64 = lane_sum(sorted);
    if n == 0 || total <= 0.0 {
        return 0.0;
    }
    let k = ((n as f64 * frac).ceil() as usize).clamp(1, n);
    let top: f64 = lane_sum(&sorted[n - k..]);
    100.0 * top / total
}

/// Upper bounds of the ten deciles of values sorted ascending (last one is the maximum).
fn deciles(sorted: &[f64]) -> Vec<f64> {
    let n = sorted.len();
    if n == 0 {
        return Vec::new();
    }
    (1..=10usize).map(|d| sorted[(d * n).div_ceil(10) - 1]).collect()
}

/// Consumption sums and household counts by income quintile. Quintile q holds the income ranks
/// [ceil(q n/5), ceil((q+1) n/5)) (rank k lies in quintile floor(5k/n)), with ranks taken in income order and ties
/// broken by household order. `s` is the incomes of `rows` sorted ascending. Computed from the cumulative
/// consumption below each quintile edge: one parallel pass, then ties at the four edges.
fn quintile_sums(rows: &[(f64, f64, f64)], s: &[f64]) -> ([f64; 5], [usize; 5]) {
    let n = rows.len();
    let ends: [usize; 5] = std::array::from_fn(|q| ((q + 1) * n + 4) / 5);
    // Edge value for each of the first four quintile ends: the income of the last rank below the edge.
    let ts: [f64; 4] = std::array::from_fn(|q| if ends[q] > 0 { s[ends[q] - 1] } else { f64::NEG_INFINITY });
    let lt: [usize; 4] = std::array::from_fn(|q| s.partition_point(|x| x.total_cmp(&ts[q]).is_lt()));
    type Acc = ([f64; 4], f64, Vec<(u8, f64)>);
    let parts: Vec<Acc> = rows
        .par_chunks(SEL_CHUNK)
        .map(|ch| {
            let mut below = [0.0f64; 4];
            let mut total = 0.0f64;
            let mut ties: Vec<(u8, f64)> = Vec::new();
            for r in ch {
                total += r.1;
                for q in 0..4 {
                    match r.0.total_cmp(&ts[q]) {
                        std::cmp::Ordering::Less => below[q] += r.1,
                        std::cmp::Ordering::Equal => ties.push((q as u8, r.1)),
                        std::cmp::Ordering::Greater => {}
                    }
                }
            }
            (below, total, ties)
        })
        .collect();
    let mut below = [0.0f64; 4];
    let mut total = 0.0f64;
    let mut ties: Vec<(u8, f64)> = Vec::new();
    for (b, t, tie) in parts {
        for q in 0..4 {
            below[q] += b[q];
        }
        total += t;
        ties.extend(tie);
    }
    // Ties at edge q: the first (ends[q] - lt[q]) of them in household order fall below the edge.
    let mut take: [usize; 4] = std::array::from_fn(|q| ends[q].saturating_sub(lt[q]));
    let mut tie_sum = [0.0f64; 4];
    for &(q, c) in &ties {
        let q = q as usize;
        if take[q] > 0 {
            tie_sum[q] += c;
            take[q] -= 1;
        }
    }
    let cum: [f64; 4] = std::array::from_fn(|q| below[q] + tie_sum[q]);
    let mut q_sum = [0.0f64; 5];
    q_sum[0] = cum[0];
    for q in 1..4 {
        q_sum[q] = cum[q] - cum[q - 1];
    }
    q_sum[4] = total - cum[3];
    let q_cnt: [usize; 5] = std::array::from_fn(|q| ends[q] - if q == 0 { 0 } else { ends[q - 1] });
    (q_sum, q_cnt)
}

fn bucket_of(age_years: u32) -> Option<usize> {
    AGE_BUCKETS
        .iter()
        .position(|&(lo, hi)| age_years >= lo && age_years <= hi)
}

/// Households counted in the homeownership denominator: owners, renters, and adult (>=25) residents with parents.
fn in_tenure_base(h: &Household) -> bool {
    match h.tenure {
        Tenure::Owner | Tenure::Renter => true,
        Tenure::WithParents => h.age_months / 12 >= 25,
    }
}

/// Tenure, unhoused and mortgage-delinquency counts for the Round 7 metrics (serial, cheap: one pass per record).
#[derive(Default)]
struct TenureExtras {
    alive: usize,
    owners: usize,
    /// Renter households with a home (HVS homeownership denominator).
    renters_occ: usize,
    /// Renter-occupied houses: occupant with owner other than the occupant (HVS rental vacancy denominator).
    renter_houses: usize,
    unhoused: usize,
    vacant_for_rent: usize,
    mort_total: usize,
    mort_dlq90: usize,
}

fn tenure_extras(w: &World) -> TenureExtras {
    let mut e = TenureExtras::default();
    for h in w.households.iter().filter(|h| h.alive) {
        e.alive += 1;
        match h.tenure {
            Tenure::Owner => e.owners += 1,
            Tenure::Renter if h.home.is_some() => e.renters_occ += 1,
            Tenure::Renter => e.unhoused += 1,
            Tenure::WithParents if h.age_months / 12 >= 25 => e.unhoused += 1,
            Tenure::WithParents => {}
        }
    }
    for hs in w.houses.iter().filter(|hs| hs.completed) {
        match hs.occupant {
            None if hs.for_rent => e.vacant_for_rent += 1,
            None => {}
            Some(o) if hs.owner != Owner::Household(o) => e.renter_houses += 1,
            Some(_) => {}
        }
    }
    for (_, l) in w.loans.iter() {
        if let (LoanKind::Mortgage { .. }, Borrower::Household(_)) = (l.kind, l.borrower) {
            e.mort_total += 1;
            if l.months_delinquent >= 3 {
                e.mort_dlq90 += 1;
            }
        }
    }
    e
}

/// The record 12 months before the month being recorded, if any.
fn back12(w: &World) -> Option<&Metrics> {
    let n = w.history.len();
    if n >= 12 {
        w.history.get(n - 12)
    } else {
        // Before 12 reported months, reach back into the end of burn-in.
        let p = w.history_pre.len();
        (p + n >= 12).then(|| &w.history_pre[p + n - 12])
    }
}

// ---------------------------------------------------------------------------------------------
// record
// ---------------------------------------------------------------------------------------------

/// Compute this month's metrics and push onto `w.history`.
///
/// Cost: a constant number of O(n) passes over households, firms, banks, houses and loans. The distributional
/// block (two sorts of n elements) runs only every `DIST_EVERY` months.
pub fn record(w: &mut World) {
    let mut st = std::mem::take(&mut w.metrics_state);
    let mut m = compute(w, &mut st);
    w.metrics_state = st;
    // b8-world H11: copy this month's world metrics (None, and so no key, when the block is off).
    m.world = w.world.as_deref().map(|wb| wb.diag.clone());
    w.history.push(m);
}

/// Households (and houses) per parallel chunk. Fixed, so the merged sums never depend on the thread count.
const HH_CHUNK: usize = 4096;

/// Household aggregates over one chunk (or, after `household_pass`, over all households).
struct HhPartial {
    housing_nom: f64,
    /// Deposits per bank over all households (dead included), as in `ledger::check_consistency`.
    dep: Vec<f64>,
    shares_sum: f64,
    sfc_ok: bool,
    alive: usize,
    employed: usize,
    unemployed: usize,
    retired: usize,
    age65: usize,
    partnered: usize,
    age_sum: f64,
    exp_infl_sum: f64,
    wage_sum: f64,
    emp_sector: [f64; N_SECTORS],
    owners: usize,
    tenure_base: usize,
    b_emp: [usize; 6],
    b_unemp: [usize; 6],
    b_own: [usize; 6],
    b_base: [usize; 6],
    b_wage: [f64; 6],
    incomes: Vec<f64>,
    wages: Vec<f64>,
    rents: Vec<f64>,
    /// (annual income, raw consumption sum, net worth) per alive household; only on distributional months.
    rows: Vec<(f64, f64, f64)>,
    /// Observation rows (distributional months only), and alive households with negative deposits and their total.
    ext: Vec<ExtRow>,
    overdrawn: usize,
    overdraft_sum: f64,
}

impl HhPartial {
    fn empty(nb: usize) -> Self {
        Self::with_capacity(nb, 0)
    }

    fn with_capacity(nb: usize, n: usize) -> Self {
        HhPartial {
            housing_nom: 0.0,
            dep: vec![0.0; nb],
            shares_sum: 0.0,
            sfc_ok: true,
            alive: 0,
            employed: 0,
            unemployed: 0,
            retired: 0,
            age65: 0,
            partnered: 0,
            age_sum: 0.0,
            exp_infl_sum: 0.0,
            wage_sum: 0.0,
            emp_sector: [0.0; N_SECTORS],
            owners: 0,
            tenure_base: 0,
            b_emp: [0; 6],
            b_unemp: [0; 6],
            b_own: [0; 6],
            b_base: [0; 6],
            b_wage: [0.0; 6],
            incomes: Vec::with_capacity(n),
            wages: Vec::with_capacity(n),
            rents: Vec::with_capacity(n),
            rows: Vec::new(),
            ext: Vec::new(),
            overdrawn: 0,
            overdraft_sum: 0.0,
        }
    }

    /// Add the scalar totals of `o` into `self` (vectors are handled by `household_pass`).
    fn merge(&mut self, o: &HhPartial) {
        self.housing_nom += o.housing_nom;
        for (a, b) in self.dep.iter_mut().zip(&o.dep) {
            *a += b;
        }
        self.shares_sum += o.shares_sum;
        self.sfc_ok &= o.sfc_ok;
        self.alive += o.alive;
        self.employed += o.employed;
        self.unemployed += o.unemployed;
        self.retired += o.retired;
        self.age65 += o.age65;
        self.partnered += o.partnered;
        self.age_sum += o.age_sum;
        self.exp_infl_sum += o.exp_infl_sum;
        self.wage_sum += o.wage_sum;
        self.overdrawn += o.overdrawn;
        self.overdraft_sum += o.overdraft_sum;
        for s in 0..N_SECTORS {
            self.emp_sector[s] += o.emp_sector[s];
        }
        self.owners += o.owners;
        self.tenure_base += o.tenure_base;
        for b in 0..6 {
            self.b_emp[b] += o.b_emp[b];
            self.b_unemp[b] += o.b_unemp[b];
            self.b_own[b] += o.b_own[b];
            self.b_base[b] += o.b_base[b];
            self.b_wage[b] += o.b_wage[b];
        }
    }
}

fn household_partial(
    w: &World,
    chunk: &[Household],
    mons: &[HhMonth],
    nb: usize,
    want_rows: bool,
    biz_eq: &[f64],
) -> HhPartial {
    let mut p = HhPartial::with_capacity(nb, chunk.len());
    for (h, m) in chunk.iter().zip(mons) {
        p.dep[h.bank as usize] += h.deposits;
        p.shares_sum += h.shares;
        if !h.deposits.is_finite() || !h.shares.is_finite() {
            p.sfc_ok = false;
        }
        if !h.alive {
            continue;
        }
        match (h.tenure, h.home) {
            (Tenure::Renter, Some(_)) => p.housing_nom += h.rent.max(0.0),
            (Tenure::Owner, Some(home)) => p.housing_nom += w.prices.rent_index * w.houses[home as usize].quality,
            _ => {}
        }
        p.alive += 1;
        if h.deposits < 0.0 {
            p.overdrawn += 1;
            p.overdraft_sum -= h.deposits;
        }
        if want_rows {
            let nw = common::net_worth(w, h.id, biz_eq);
            p.rows.push((m.income.total() * 12.0, m.consumption.iter().sum::<f64>(), nw));
            p.ext.push(ExtRow {
                disp: (m.income.total() - m.taxes) * 12.0,
                earn: m.income.wage * 12.0,
                age: u32::from(h.age_months / 12),
                nw,
                htm: h.deposits < 0.5 * m.income.total(),
                owner: h.tenure == Tenure::Owner,
            });
        }
        if h.age_months >= 65 * 12 {
            p.age65 += 1;
        }
        if h.partnered {
            p.partnered += 1;
        }
        p.age_sum += h.age_years();
        p.exp_infl_sum += h.expected_inflation;
        p.incomes.push(m.income.total() * 12.0);
        let bkt = bucket_of(u32::from(h.age_months / 12));
        match h.status {
            LaborStatus::Employed => {
                p.employed += 1;
                p.wage_sum += h.wage;
                p.wages.push(h.wage);
                if let Some(b) = bkt {
                    p.b_emp[b] += 1;
                    p.b_wage[b] += h.wage;
                }
                if let Some(f) = h.employer {
                    p.emp_sector[w.firms[f as usize].sector.idx()] += 1.0;
                }
            }
            LaborStatus::Unemployed => {
                p.unemployed += 1;
                if let Some(b) = bkt {
                    p.b_unemp[b] += 1;
                }
            }
            LaborStatus::Retired => p.retired += 1,
            _ => {}
        }
        if in_tenure_base(h) {
            p.tenure_base += 1;
            if let Some(b) = bkt {
                p.b_base[b] += 1;
            }
        }
        match h.tenure {
            Tenure::Owner => {
                p.owners += 1;
                if let Some(b) = bkt {
                    p.b_own[b] += 1;
                }
            }
            Tenure::Renter => {
                if h.rent > 0.0 {
                    p.rents.push(h.rent);
                }
            }
            Tenure::WithParents => {}
        }
    }
    p
}

/// Fused household pass: fills `incomes`, `wages`, `rents` (household order) and returns the totals.
fn household_pass(
    w: &World,
    want_rows: bool,
    incomes: &mut Vec<f64>,
    wages: &mut Vec<f64>,
    rents: &mut Vec<f64>,
    rows: &mut Vec<(f64, f64, f64)>,
    ext: &mut Vec<ExtRow>,
) -> HhPartial {
    let nb = w.banks.len();
    let biz_eq = if want_rows { common::owner_equity(w) } else { Vec::new() };
    let parts: Vec<HhPartial> = w
        .households
        .par_chunks(HH_CHUNK)
        .zip(w.hh_month.par_chunks(HH_CHUNK))
        .map(|(c, mons)| household_partial(w, c, mons, nb, want_rows, &biz_eq))
        .collect();
    let mut tot = HhPartial::empty(nb);
    // One copy per vector, in chunk order (household order).
    let (n_inc, n_wage, n_rent): (usize, usize, usize) = parts
        .iter()
        .fold((0, 0, 0), |a, p| (a.0 + p.incomes.len(), a.1 + p.wages.len(), a.2 + p.rents.len()));
    incomes.reserve(n_inc);
    wages.reserve(n_wage);
    rents.reserve(n_rent);
    rows.reserve(parts.iter().map(|p| p.rows.len()).sum());
    for mut p in parts {
        tot.merge(&p);
        incomes.append(&mut p.incomes);
        wages.append(&mut p.wages);
        rents.append(&mut p.rents);
        rows.append(&mut p.rows);
        ext.append(&mut p.ext);
    }
    tot
}

/// Per-chunk house counts and owner-occupied values.
struct HousePartial {
    completed: usize,
    vacant_completed: usize,
    for_sale: usize,
    owner_occ_values: Vec<f64>,
}

fn house_partial(w: &World, chunk: &[House]) -> HousePartial {
    let mut p = HousePartial { completed: 0, vacant_completed: 0, for_sale: 0, owner_occ_values: Vec::new() };
    for hs in chunk {
        if hs.completed {
            p.completed += 1;
            if hs.occupant.is_none() {
                p.vacant_completed += 1;
            }
        }
        if hs.for_sale {
            p.for_sale += 1;
        }
        if let (Some(occ), Owner::Household(o)) = (hs.occupant, hs.owner) {
            if occ == o {
                p.owner_occ_values.push(w.prices.hpi * hs.quality);
            }
        }
    }
    p
}

fn compute(w: &World, st: &mut MetricsState) -> Metrics {
    let scale = w.cfg.effective_scale();
    // model $ per month (per-agent, unscaled) -> US $bn at annual rates; and -> US $bn stock.
    let ann = 12.0 * scale / 1e9;
    let stock = scale / 1e9;
    let mscale = scale / 1e6;
    let tick = w.tick;

    // ---- base prices (first recorded month) and sector prices relative to base ----
    if !st.initialized {
        for s in Sector::ALL {
            st.base_sector[s.idx()] = common::sector_price(w, s);
        }
        st.base_rent = w.prices.rent_index;
        st.initialized = true;
    }
    let mut rel = [1.0f64; N_SECTORS];
    for s in Sector::ALL {
        let b = st.base_sector[s.idx()];
        if b > 0.0 {
            rel[s.idx()] = common::sector_price(w, s) / b;
        }
    }

    // ---- national accounts: nominal flows and real flows (deflated) ----
    let fl = &w.flows;
    // Housing services (BEA treatment): value added of tenant- and owner-occupied housing = actual rent plus imputed
    // rent of owner-occupiers (market rent for the home), net of the operating costs already counted as Services.
    // One parallel pass over households (housing value added, counts, sums, medians inputs, deposits per bank).
    let mut incomes = std::mem::take(&mut st.scratch_incomes);
    let mut wages = std::mem::take(&mut st.scratch_wages);
    let mut rents = std::mem::take(&mut st.scratch_rents);
    incomes.clear();
    wages.clear();
    rents.clear();
    let want_rows = !st.dist.valid || tick % DIST_EVERY == 0;
    let mut rows = std::mem::take(&mut st.scratch_rows);
    rows.clear();
    let mut ext: Vec<ExtRow> = Vec::new();
    let hp = household_pass(w, want_rows, &mut incomes, &mut wages, &mut rents, &mut rows, &mut ext);
    let housing_nom = HOUSING_VALUE_ADDED_SHARE * hp.housing_nom;
    let rent_rel = if st.base_rent > 0.0 { w.prices.rent_index / st.base_rent } else { 1.0 };
    let goods_c_nom: f64 = fl.consumption.iter().sum();
    let c_nom = goods_c_nom + housing_nom;
    // b7-tariff (BEA treatment of duties, M8): real consumption excludes the household duty wedge, and real investment
    // excludes the equipment duty, so duties move nominal aggregates but not real volumes. Without duties this is the
    // unchanged consumption series.
    let duties_real = fl.tariff_hh.iter().sum::<f64>() != 0.0 || fl.tariff_equipment != 0.0;
    let cons_vol: [f64; N_CONSUMER] =
        if duties_real { std::array::from_fn(|j| fl.consumption[j] - fl.tariff_hh[j]) } else { fl.consumption };
    let goods_c_vol: f64 = cons_vol.iter().sum();
    let c_vol_nom = goods_c_vol + housing_nom;
    // consumption deflator: spending-weighted sector prices and the rent index (household-level real consumption
    // still uses the nominal series, duties included)
    let c_price: f64 = (0..N_CONSUMER).map(|j| fl.consumption[j] * rel[j]).sum::<f64>() + housing_nom * rent_rel;
    let d_cons = if c_nom > 0.0 { c_price / c_nom } else { 1.0 };
    let c_price_vol: f64 = (0..N_CONSUMER).map(|j| cons_vol[j] * rel[j]).sum::<f64>() + housing_nom * rent_rel;
    let d_cons_vol = if c_vol_nom > 0.0 { c_price_vol / c_vol_nom } else { 1.0 };
    let c_real = div(c_vol_nom, d_cons_vol);
    let purchases_real: f64 = (0..N_CONSUMER).map(|j| div(cons_vol[j], rel[j])).sum();

    let inv_nom = fl.investment;
    // Investment is deflated by the capital-goods price (`Config::capital_valuation_mix`).
    let inv_real = div(inv_nom - fl.tariff_equipment, role_mean(&w.cfg.capital_valuation_mix, |s| rel[s]));
    let res_nom = fl.residential_investment;
    // Intentional: residential investment is construction output (housing), deflated by the Construction price.
    let res_real = div(res_nom, rel[Sector::Construction.idx()]);

    let gov_nom = fl.government;
    // Medicare/Medicaid health care is bought by government from Services but recorded by BEA as personal
    // consumption (social benefits in kind), not government consumption: move that share from G to C.
    // Health purchases are planned in fiscal::plan_spending; this month's realised purchases carry the same health
    // fraction (equal to the planned amount when orders are filled in full).
    let health_frac = if w.fiscal.purchases_month > 0.0 { w.fiscal.health_month / w.fiscal.purchases_month } else { 0.0 };
    let in_kind_nom = health_frac * gov_nom;
    let in_kind_real = div(in_kind_nom, role_mean(&w.cfg.health_sector_mix, |s| rel[s]));
    // Government purchase deflator (integration fix, correctness finding): the non-health purchases the government makes
    // from each sector, weighted by this month's orders net of the health purchases (health_frac of total orders, split
    // by `Config::health_sector_mix`). Health is deflated by its own sector mix above, so it does not enter this price.
    let gov_orders: [f64; N_SECTORS] = std::array::from_fn(|s| w.orders.government[s].max(0.0));
    let gov_orders_tot: f64 = gov_orders.iter().sum();
    let health_mix_tot: f64 = w.cfg.health_sector_mix.iter().sum();
    let (mut gnum, mut gden) = (0.0f64, 0.0f64);
    for s in 0..N_SECTORS {
        let health_s = if health_mix_tot > 0.0 {
            health_frac * gov_orders_tot * w.cfg.health_sector_mix[s] / health_mix_tot
        } else {
            0.0
        };
        let wt = (gov_orders[s] - health_s).max(0.0);
        gnum += wt * rel[s];
        gden += wt;
    }
    let d_gov = if gden > 0.0 {
        gnum / gden
    } else {
        // No non-health orders this month: fall back to the price of all government orders, then to the mean sector price.
        let all: f64 = gov_orders.iter().sum();
        if all > 0.0 {
            (0..N_SECTORS).map(|s| gov_orders[s] * rel[s]).sum::<f64>() / all
        } else {
            rel.iter().sum::<f64>() / N_SECTORS as f64
        }
    };
    let gov_real = div(gov_nom - in_kind_nom, d_gov);
    let gov_nom = gov_nom - in_kind_nom;
    let c_nom = c_nom + in_kind_nom;
    let c_real = c_real + in_kind_real;

    // Financial services furnished without payment (FISIM; BEA PCE): households pay for deposit and loan services
    // through the spread of deposit rates below, and mortgage rates above, a reference rate (the 3-month yield).
    let fisim_nom = {
        let r_ref = w.curve.yields[0].max(0.0);
        let dep_part: f64 = hp
            .dep
            .iter()
            .zip(w.banks.iter())
            .map(|(&d, b)| d.max(0.0) * (r_ref - b.deposit_rate).max(0.0))
            .sum();
        let loan_part: f64 = w
            .loans
            .iter()
            .filter(|(_, l)| matches!(l.borrower, Borrower::Household(_)))
            .map(|(_, l)| l.principal.max(0.0) * (l.annual_rate - r_ref).max(0.0))
            .sum();
        (dep_part + loan_part) / 12.0
    };
    let (c_nom_pre_fisim, c_real_pre_fisim) = (c_nom, c_real);
    let c_nom = c_nom + fisim_nom;
    // rz1 F1b: with `fisim_constant_margin` the real FISIM follows the household deposit and loan stocks at the margins of the first
    // reported month, deflated by the general price index (PCE deflator excluding FISIM) and indexed to the legacy value at month 0
    // (SNA 2008 15.114). Nominal FISIM is unchanged. Off: nominal over the OtherServices price (legacy).
    let legacy_defl = role_mean(&w.cfg.financial_services_mix, |s| rel[s]);
    let fisim_real = if w.cfg.fisim_constant_margin {
        let general_defl = div(c_nom_pre_fisim, c_real_pre_fisim);
        fisim_constant_margin_real(w, &hp.dep, st, fisim_nom, legacy_defl, general_defl)
    } else {
        div(fisim_nom, legacy_defl)
    };
    let c_real = c_real + fisim_real;

    let exp_nom = fl.exports;
    let imp_nom = fl.imports;
    // Trade is deflated by the tradables price (`Config::tradable_weight`). Integration (fix-trade finding): realised
    // exports are deflated sector by sector, so Energy exports move with the Energy price.
    let tradables_rel = role_mean(&w.cfg.tradable_weight, |s| rel[s]);
    let exp_real: f64 = (0..N_SECTORS).map(|s| div(fl.exports_by_sector[s], rel[s])).sum();
    let imp_real = div(imp_nom, tradables_rel);

    // Change in private inventories (part of BEA GDP): this month's change in firms' inventory units by sector,
    // valued at sector prices (real: at base prices). Without it, measured GDP is final sales and decouples from
    // production and employment over the cycle.
    let mut inv_units = [0.0f64; N_SECTORS];
    for f in w.firms.iter().filter(|f| f.alive) {
        inv_units[f.sector.idx()] += f.inventory.max(0.0);
    }
    // Re-fit Step 5a: with `cipi_excludes_writeoffs`, the inventory written off in bankruptcies this month is added back
    // (an "other change in volume", not a fall in inventory investment). Legacy: the plain change.
    let cipi_writeoffs = w.cfg.cipi_excludes_writeoffs;
    // b7-spikes diagnostic: per-sector real CIPI before any exclusion (Δunits + write-off), computed beside the fold.
    let cipi_by_sector_real: [f64; N_SECTORS] = match st.inventory_prev {
        Some(prev) => std::array::from_fn(|s| {
            (inv_units[s] - prev[s] + w.flows.bk_inventory_units[s]) * st.base_sector[s] * ann
        }),
        None => [0.0; N_SECTORS],
    };
    let (cipi_nom, cipi_real) = match st.inventory_prev {
        Some(prev) => (0..N_SECTORS).fold((0.0, 0.0), |(n, r), s| {
            // b7-spikes F1: unsold perishable output is not inventory investment (services are recorded when delivered).
            if w.cfg.cipi_excludes_nonstorable && w.cfg.nonstorable_sectors[s] {
                return (n, r);
            }
            let d = if cipi_writeoffs {
                inv_units[s] - prev[s] + w.flows.bk_inventory_units[s]
            } else {
                inv_units[s] - prev[s]
            };
            (n + d * common::sector_price(w, Sector::from_idx(s)), r + d * st.base_sector[s])
        }),
        None => (0.0, 0.0),
    };
    st.inventory_prev = Some(inv_units);
    let gdp_nom_m = c_nom + inv_nom + res_nom + gov_nom + exp_nom - imp_nom + cipi_nom;
    let gdp_real_m = c_real + inv_real + res_real + gov_real + exp_real - imp_real + cipi_real;
    let gdp_real_bn_ar = gdp_real_m * ann;

    let (alive, employed, unemployed, retired) = (hp.alive, hp.employed, hp.unemployed, hp.retired);
    let (age_sum, exp_infl_sum, wage_sum) = (hp.age_sum, hp.exp_infl_sum, hp.wage_sum);
    let age65 = hp.age65;
    let partnered = hp.partnered;
    let (emp_sector, owners, tenure_base) = (hp.emp_sector, hp.owners, hp.tenure_base);
    let (b_emp, b_unemp, b_own, b_base, b_wage) = (hp.b_emp, hp.b_unemp, hp.b_own, hp.b_base, hp.b_wage);
    let shares_sum = hp.shares_sum;
    let overdrawn = hp.overdrawn;
    let overdraft_sum = hp.overdraft_sum;
    let mut sfc_ok = hp.sfc_ok;
    let mut dep = hp.dep;
    let n_alive = alive as f64;
    let employed_f = employed as f64;
    let unemployed_f = unemployed as f64;
    let labour_force = employed_f + unemployed_f;

    // ---- houses (one parallel pass, merged in chunk order) ----
    let hs_parts: Vec<HousePartial> = w.houses.par_chunks(HH_CHUNK).map(|c| house_partial(w, c)).collect();
    let mut completed = 0usize;
    let mut vacant_completed = 0usize;
    let mut for_sale = 0usize;
    let mut owner_occ_values = std::mem::take(&mut st.scratch_owner_values);
    owner_occ_values.clear();
    for mut p in hs_parts {
        completed += p.completed;
        vacant_completed += p.vacant_completed;
        for_sale += p.for_sale;
        owner_occ_values.append(&mut p.owner_occ_values);
    }

    // ---- medians (independent selections, run in parallel) ----
    let ((med_wage, median_house_price), (median_income, median_rent)) = rayon::join(
        || rayon::join(|| median_mut(&mut wages), || median_mut(&mut owner_occ_values)),
        || rayon::join(|| median_mut(&mut incomes), || median_mut(&mut rents)),
    );


    // ---- distributional block (refreshed every DIST_EVERY months, carried forward otherwise) ----
    if !st.dist.valid || tick % DIST_EVERY == 0 {
        // rows: (income annual, real consumption per household this month, net worth), from the household pass
        rows.par_iter_mut().for_each(|r| r.1 = div(r.1, d_cons));
        // Income sorted ascending (values only: equal values are interchangeable), then quintiles by income rank.
        let (mut incs, mut nws): (Vec<f64>, Vec<f64>) = rayon::join(
            || rows.par_iter().map(|r| r.0).collect(),
            || rows.par_iter().map(|r| r.2).collect(),
        );
        rayon::join(|| par_sort_total(&mut incs), || par_sort_total(&mut nws));
        let (q_sum, q_cnt) = quintile_sums(&rows, &incs);
        let mut q_mean = [0.0f64; 5];
        for q in 0..5 {
            q_mean[q] = div(q_sum[q], q_cnt[q] as f64);
        }
        let income_gini = gini_sorted(&incs);

        // Wealth Gini, top-10% share and median: sorted values.

        let mut growth = [0.0f64; 5];
        if let Some(prior) = st.quintile_hist.iter().rev().find(|e| e.0 + 12 == tick).map(|e| e.1) {
            for q in 0..5 {
                growth[q] = growth_pct(q_mean[q], prior[q]);
            }
        }
        st.quintile_hist.push((tick, q_mean));
        let cons: Vec<f64> = rows.iter().map(|r| r.1).collect();
        let prev_obs = std::mem::take(&mut st.dist.obs);
        st.dist = DistCache {
            valid: true,
            wealth_gini: gini_sorted(&nws),
            income_gini,
            top10_wealth_share: top_share(&nws, 0.1),
            median_net_worth: median_sorted(&nws),
            consumption_by_quintile: growth,
            obs: DistObs::default(),
        };
        // The observation-only distribution and firm statistics (sorts over households and firms) are refreshed
        // annually (every 12th month, and at the first distributional month); otherwise carried forward.
        if !st.obs_ready || tick % 12 == 0 {
            refresh_dist_obs(w, &ext, &cons, &nws, &mut st.dist.obs);
            st.obs_ready = true;
        } else {
            st.dist.obs = prev_obs;
        }
    }
    let dist = st.dist.clone();

    // ---- labour market ----
    let unemployment_rate = 100.0 * div(unemployed_f, labour_force);
    let participation_rate = 100.0 * div(labour_force, n_alive);
    let avg_wage_annual = 12.0 * div(wage_sum, employed_f);
    let median_wage_annual = 12.0 * med_wage;
    let wage_by_age: [f64; 6] = std::array::from_fn(|b| 12.0 * div(b_wage[b], b_emp[b] as f64));
    let unemployment_by_age: [f64; 6] =
        std::array::from_fn(|b| 100.0 * div(b_unemp[b] as f64, (b_emp[b] + b_unemp[b]) as f64));
    let employment_by_sector: [f64; N_SECTORS] = std::array::from_fn(|s| emp_sector[s] * mscale);

    // ---- firms ----
    let mut firms_n = 0usize;
    let mut out_sum = 0.0f64;
    let mut markup_out = 0.0f64;
    let mut markup_simple = 0.0f64;
    let mut profit_sum = 0.0f64;
    let mut vac_sum = 0.0f64;
    let mut capacity_sum = 0.0f64;
    for f in &w.firms {
        dep[f.bank as usize] += f.deposits;
    }
    dep[w.fund.bank as usize] += w.fund.deposits;
    dep[w.row.bank as usize] += w.row.deposits;
    for f in w.firms.iter().filter(|f| f.alive) {
        firms_n += 1;
        out_sum += f.output;
        markup_out += f.markup * f.output;
        markup_simple += f.markup;
        profit_sum += f.profit;
        vac_sum += f.openings as f64;
        capacity_sum += f.capital * f.capital_productivity;
    }
    let capacity_utilization = 100.0 * div(out_sum, capacity_sum);
    let avg_markup = 100.0
        * if out_sum > 0.0 {
            div(markup_out, out_sum)
        } else {
            div(markup_simple, firms_n as f64)
        };
    let profit_share = 100.0 * div(profit_sum, gdp_nom_m);
    let vacancy_rate = 100.0 * div(vac_sum, employed_f + vac_sum);
    let hires_rate = 100.0 * div(w.flows.hires as f64, employed_f);
    let layoffs_rate = 100.0 * div(w.flows.layoffs as f64, employed_f);

    // ---- loans, banks ----
    let nb = w.banks.len();
    let mut mort_by_bank = vec![0.0f64; nb];
    let mut biz_by_bank = vec![0.0f64; nb];
    let mut loan_book = vec![0.0f64; nb];
    let mut mort_total = 0.0f64;
    let mut mort_rate_wsum = 0.0f64;
    let mut biz_total = 0.0f64;
    for (_, l) in w.loans.iter() {
        loan_book[l.bank as usize] += l.principal;
        if let LoanKind::Mortgage { house } = l.kind {
            if w.houses[house as usize].mortgage != Some(l.id) {
                sfc_ok = false;
            }
        }
        match l.kind {
            LoanKind::Mortgage { .. } => {
                mort_total += l.principal;
                mort_rate_wsum += l.principal * l.annual_rate;
                mort_by_bank[l.bank as usize] += l.principal;
            }
            LoanKind::Business => {
                biz_total += l.principal;
                biz_by_bank[l.bank as usize] += l.principal;
            }
            LoanKind::Consumer => {}
        }
    }
    let mut nb_alive = 0usize;
    let mut mortgage_rate_sum = 0.0f64;
    let mut deposit_rate_sum = 0.0f64;
    let mut business_rate_sum = 0.0f64;
    let mut bank_deposits = 0.0f64;
    let mut bank_reserves = 0.0f64;
    let mut bank_profit = 0.0f64;
    let mut bank_equity = 0.0f64;
    let mut rwa = 0.0f64;
    let mut biz_writeoffs_ttm = 0.0f64;
    let mut biz_loans_banks = 0.0f64;
    for (i, b) in w.banks.iter().enumerate() {
        if !b.alive {
            continue;
        }
        nb_alive += 1;
        biz_writeoffs_ttm += b.biz_writeoff_hist.iter().sum::<f64>();
        biz_loans_banks += b.business_loans.max(0.0);
        mortgage_rate_sum += b.mortgage_rate;
        deposit_rate_sum += b.deposit_rate;
        business_rate_sum += b.business_rate_base;
        bank_deposits += b.deposits;
        bank_reserves += b.reserves;
        bank_profit += b.profit;
        bank_equity += b.equity();
        rwa += 0.5 * mort_by_bank[i] + biz_by_bank[i];
    }
    let nb_alive_f = nb_alive as f64;
    let bank_capital_ratio = 100.0 * div(bank_equity, rwa);

    // ---- sfc consistency (same checks as ledger::check_consistency, fused into the passes above) ----
    for (i, b) in w.banks.iter().enumerate() {
        let tol = 1e-6 * (1.0 + b.deposits.abs());
        if (dep[i] - b.deposits).abs() > tol {
            sfc_ok = false;
        }
        let tol = 1e-6 * (1.0 + b.loans.abs());
        if (loan_book[i] - b.loans).abs() > tol {
            sfc_ok = false;
        }
        if !b.reserves.is_finite() || !b.deposits.is_finite() {
            sfc_ok = false;
        }
    }
    let shares: f64 = shares_sum;
    if (shares - w.fund.total_shares).abs() > 1e-6 * (1.0 + shares.abs()) {
        sfc_ok = false;
    }
    let sfc_error = if sfc_ok { 0.0 } else { 1.0 };

    // ---- prices ----
    let cpi = w.prices.cpi;
    let core_cpi = w.prices.core_cpi;
    let inflation_yoy = 100.0 * w.inflation_yoy();
    let core_inflation_yoy = 100.0 * w.core_inflation_yoy();
    let core_pce_inflation_yoy = 100.0 * w.core_pce_inflation_yoy();
    st.energy_hist.push(common::sector_price(w, Sector::Energy));
    st.rent_hist.push(w.prices.rent_index);
    // Lease-stock (CPI) rent: a 12-month average of the market rent, so a change in market rents reaches the CPI over a
    // year as leases renew (1/12 of the stock a month).
    let lo = st.rent_hist.len().saturating_sub(12);
    let lease_stock = st.rent_hist[lo..].iter().sum::<f64>() / (st.rent_hist.len() - lo) as f64;
    st.cpi_rent_hist.push(lease_stock);
    let energy_inflation_yoy = yoy_pct(&st.energy_hist);
    let shelter_inflation_yoy = yoy_pct(&st.cpi_rent_hist);
    let ch = &w.prices.cpi_history;
    let inflation_mom_annualized = if ch.len() >= 2 && ch[ch.len() - 2] > 0.0 {
        100.0 * ((ch[ch.len() - 1] / ch[ch.len() - 2]).powi(12) - 1.0)
    } else {
        0.0
    };
    let expected_inflation = 100.0 * div(exp_infl_sum, n_alive);

    // ---- real wage ----
    let real_wage = div(avg_wage_annual, cpi);
    if st.base_real_wage <= 0.0 && real_wage > 0.0 {
        st.base_real_wage = real_wage;
    }
    let real_wage_index = if st.base_real_wage > 0.0 {
        100.0 * real_wage / st.base_real_wage
    } else {
        100.0
    };

    // ---- GDP growth and wage growth vs 12 months ago ----
    let prev = back12(w);
    let gdp_real_growth_yoy = prev.map_or(0.0, |p| growth_pct(gdp_real_bn_ar, p.gdp_real_bn_ar));
    let wage_growth_yoy = prev.map_or(0.0, |p| growth_pct(avg_wage_annual, p.avg_wage_annual));
    let hpi_growth_yoy = prev.map_or(0.0, |p| growth_pct(w.prices.hpi, p.hpi));

    // ---- housing flows ----
    let house_sales_m = w.flows.house_sales as f64;
    let months_supply_listings = for_sale as f64 * scale;
    let house_sales_us_m = house_sales_m * scale;
    let months_supply = div(months_supply_listings, house_sales_us_m.max(1.0));
    st.scratch_wages = wages;
    st.scratch_incomes = incomes;
    st.scratch_rents = rents;
    st.scratch_rows = rows;
    st.scratch_owner_values = owner_occ_values;

    // ---- government ----
    let debt_face = w.gov.debt_face;
    let gov_debt_gdp = 100.0 * div(debt_face, 12.0 * gdp_nom_m);
    let deficit_gdp = 100.0 * div(w.gov.outlays - w.gov.revenue, gdp_nom_m);
    let social_security_gdp = 100.0 * div(w.fiscal.pension_month, gdp_nom_m);
    let health_gdp = 100.0 * div(in_kind_nom, gdp_nom_m);
    let ui_nom: f64 = w.hh_month.iter().map(|m| m.income.unemployment_benefit.max(0.0)).sum();
    let ui_gdp = 100.0 * div(ui_nom, gdp_nom_m);
    let transfers_gdp = 100.0 * div(w.gov.transfers, gdp_nom_m);
    let means_tested_nom: f64 = w.hh_month.iter().map(|m| m.income.transfers.max(0.0)).sum();
    let means_tested_gdp = 100.0 * div(means_tested_nom, gdp_nom_m);
    let tenure = tenure_extras(w);
    let r_star_est = 100.0 * w.monetary.r_star_est.unwrap_or(w.cfg.r_star);
    let remittance_gdp = 100.0 * div(w.cb.remittance_month, gdp_nom_m);
    let deferred_asset_gdp = 100.0 * div(w.cb.deferred_asset, 12.0 * gdp_nom_m);

    // ---- real consumption ----
    let real_consumption_per_hh = div(c_real, n_alive);

    // ---- demography ----
    let retirees_share = 100.0 * div(retired as f64, n_alive);

    let proprietors_nom: f64 = w
        .households
        .iter()
        .zip(&w.hh_month)
        .filter(|(h, _)| h.alive)
        .map(|(_, m)| m.income.business)
        .sum();

    // ---- stage 4: sector equity, sector P/E, tech boom and tech productivity ----
    let mut profit_by_sector = [0.0; N_SECTORS];
    let mut tech_inv_units = 0.0;
    for (f, fm) in w.firms.iter().enumerate().filter(|(_, fm)| fm.alive) {
        profit_by_sector[fm.sector.idx()] += fm.profit;
        if fm.sector == Sector::Tech {
            tech_inv_units += w.fills.firm_capital_received.get(f).copied().unwrap_or(0.0).max(0.0);
        }
    }
    let equity_value_by_sector = w.fund.sector_equity.map(|e| e * stock);
    let pe_by_sector = std::array::from_fn(|s| {
        if profit_by_sector[s] > 0.0 {
            w.fund.sector_equity[s] / (12.0 * profit_by_sector[s])
        } else {
            0.0
        }
    });
    let tech_investment_bn_ar = tech_inv_units * common::capital_price_world(w) * ann;
    let tfp_growth_tech = 100.0 * (w.exog.tfp_growth_m[Sector::Tech.idx()].powi(12) - 1.0);

    let dur = duration_stats(&w.households);
    let dur_bls = duration_stats_on(&w.households, true);
    let mut m = Metrics {
        tick,
        date: 2026.0 + tick as f64 / 12.0,
        dur_mean_weeks: dur.mean_weeks,
        dur_median_weeks: dur.median_weeks,
        ltu_share: dur.ltu_share,
        dur_mean_weeks_bls: dur_bls.mean_weeks,
        dur_median_weeks_bls: dur_bls.median_weeks,
        ltu_share_bls: dur_bls.ltu_share,
        jf_by_dur_pct: std::array::from_fn(|b| 100.0 * div(fl.hires_u_dur[b] as f64, fl.u_stock_dur[b] as f64)),
        u_dur_n: std::array::from_fn(|b| f64::from(fl.u_stock_dur[b])),
        hires_u_dur_n: std::array::from_fn(|b| f64::from(fl.hires_u_dur[b])),

        gdp_nominal_bn_ar: gdp_nom_m * ann,
        gdp_real_bn_ar,
        inventory_change_real_bn_ar: cipi_real * ann,
        household_purchases_real_bn_ar: purchases_real * ann,
        consumption_by_sector_real: std::array::from_fn(|j| div(cons_vol[j], rel[j]) * ann),
        discretionary_multiplier_mean: w.consumption.discretionary_mult_mean,
        gdp_real_growth_yoy,
        consumption_real_bn_ar: c_real * ann,
        consumption_nominal_bn_ar: c_nom * ann,
        investment_real_bn_ar: inv_real * ann,
        residential_investment_bn_ar: res_nom * ann,
        government_bn_ar: gov_nom * ann,
        exports_bn_ar: exp_nom * ann,
        imports_bn_ar: imp_nom * ann,
        tariff_rate_pct: 100.0 * crate::world::tau_for(w, crate::world::Channel::Hh),
        tariff_retail_pct: 100.0 * w.goods.tariff_month.retail,
        tariff_revenue_bn_ar: ann
            * (fl.tariff_hh.iter().sum::<f64>()
                + fl.tariff_absorbed
                + fl.tariff_inputs
                + fl.tariff_equipment
                + fl.tariff_legacy),
        tariff_absorbed_bn_ar: ann * fl.tariff_absorbed,
        import_price_index: 100.0 * w.goods.import_rel.unwrap_or(1.0) * (1.0 + w.goods.tariff_month.retail),
        hh_import_share_pct: 100.0 * role_mean(&w.cfg.cpi_tradable_weight, |s| w.goods.hh_import_share_now[s]),
        firm_input_import_share_pct: 100.0 * w.goods.firm_input_share_now,
        proprietors_income_bn_ar: proprietors_nom * ann,
        capacity_utilization,

        cpi,
        core_cpi,
        inflation_yoy,
        core_inflation_yoy,
        core_pce_inflation_yoy,
        energy_inflation_yoy,
        shelter_inflation_yoy,
        inflation_mom_annualized,
        expected_inflation,
        sector_prices: w.prices.sector,
        oil_price: w.exog.oil_price,
        gasoline_price: common::gasoline_price(w),
        dollar_index: 100.0 * w.row.fx,

        unemployment_rate,
        participation_rate,
        employment_millions: employed_f * mscale,
        employment_persons_millions: employed_f * mscale * PERSONS_PER_EMPLOYED_HEAD,
        vacancies_millions: vac_sum * mscale,
        vacancy_rate,
        hires_rate,
        layoffs_rate,
        avg_wage_annual,
        median_wage_annual,
        wage_growth_yoy,
        real_wage_index,
        wage_by_age,
        unemployment_by_age,
        employment_by_sector,

        policy_rate: 100.0 * w.cb.policy_rate,
        yields: w.curve.yields.map(|y| 100.0 * y),
        term_premium: w.curve.term_premium.map(|y| 100.0 * y),
        mortgage_rate: 100.0 * div(mortgage_rate_sum, nb_alive_f),
        avg_outstanding_mortgage_rate: 100.0 * div(mort_rate_wsum, mort_total),
        deposit_rate: 100.0 * div(deposit_rate_sum, nb_alive_f),
        business_loan_rate: 100.0 * div(business_rate_sum, nb_alive_f),
        // Over the Treasury yields business loans are priced off (half 2-year, half 5-year), like corporate spreads.
        credit_spread: 100.0 * div(business_rate_sum, nb_alive_f) - 50.0 * (w.curve.yields[1] + w.curve.yields[2]),
        loan_loss_rate: 100.0 * div(biz_writeoffs_ttm, biz_loans_banks),

        hpi: w.prices.hpi,
        median_house_price,
        hpi_growth_yoy,
        rent_index: w.prices.rent_index,
        median_rent,
        house_sales_millions_ar: house_sales_m * 12.0 * scale / 1e6,
        housing_starts_millions_ar: w.flows.housing_starts as f64 * 12.0 * scale / 1e6,
        homeownership_rate: 100.0 * div(owners as f64, tenure_base as f64),
        homeownership_by_age: std::array::from_fn(|b| 100.0 * div(b_own[b] as f64, b_base[b] as f64)),
        foreclosures_thousands_ar: w.flows.foreclosures as f64 * 12.0 * scale / 1e3,
        listings: months_supply_listings,
        months_supply,
        vacancy_rate_housing: 100.0 * div(vacant_completed as f64, completed as f64),
        price_to_income: div(median_house_price, median_income),

        mortgage_debt_bn: mort_total * stock,
        business_debt_bn: biz_total * stock,
        new_mortgages_bn_ar: fl.new_mortgage_value * ann,
        bank_capital_ratio,
        bank_profits_bn_ar: bank_profit * ann,
        loan_losses_bn_ar: fl.loan_losses * ann,
        deposits_bn: bank_deposits * stock,
        reserves_bn: bank_reserves * stock,

        cb_assets_bn: (w.cb.bonds_value + w.cb.discount_loans) * stock,
        cb_bonds_bn: w.cb.bonds_value * stock,
        qe_purchases_bn_ar: fl.qe_purchases * ann,
        helicopter_bn_ar: fl.helicopter * ann,
        gov_debt_bn: debt_face * stock,
        gov_debt_gdp,
        deficit_gdp,
        social_security_gdp,
        health_gdp,
        ui_gdp,
        transfers_gdp,
        means_tested_gdp,
        homeownership_rate_hvs: 100.0 * div(tenure.owners as f64, (tenure.owners + tenure.renters_occ) as f64),
        rental_vacancy_rate_hvs: 100.0 * div(tenure.vacant_for_rent as f64, (tenure.renter_houses + tenure.vacant_for_rent) as f64),
        unhoused_share: 100.0 * div(tenure.unhoused as f64, tenure.alive as f64),
        mortgage_delinquent_90: 100.0 * div(tenure.mort_dlq90 as f64, tenure.mort_total as f64),
        evictions_thousands_ar: w.flows.evictions as f64 * 12.0 * scale / 1e3,
        foreclosures_owner_occ_thousands_ar: w.flows.foreclosures_owner_occ as f64 * 12.0 * scale / 1e3,
        r_star_est,
        remittance_gdp,
        deferred_asset_gdp,
        gov_revenue_bn_ar: w.gov.revenue * ann,
        gov_outlays_bn_ar: w.gov.outlays * ann,
        gov_interest_bn_ar: w.gov.interest_paid * ann,

        fund_share_price: w.fund.share_price,
        equity_value_bn: w.fund.equity_value * stock,
        // E / (12 × monthly profit): the scale factors cancel, so this is the same ratio in bn and model units.
        pe_ratio: if profit_sum > 0.0 { w.fund.equity_value / (12.0 * profit_sum) } else { 0.0 },
        equity_risk_premium: 100.0 * w.monetary.erp_current,
        equity_return_12m: crate::monetary::trailing_equity_return(w).map_or(0.0, |r| 100.0 * r),
        equity_value_by_sector,
        pe_by_sector,
        tech_boom: if w.exog.tech_boom { 1.0 } else { 0.0 },
        tfp_growth_tech,
        tech_investment_bn_ar,
        wealth_gini: dist.wealth_gini,
        income_gini: dist.income_gini,
        top10_wealth_share: dist.top10_wealth_share,
        median_net_worth: dist.median_net_worth,
        consumption_by_quintile: dist.consumption_by_quintile,
        real_consumption_per_hh,

        firms_alive: firms_n as f64 * scale,
        bankruptcies_ar: fl.firm_bankruptcies as f64 * 12.0 * scale,
        avg_markup,
        profit_share,

        population_households_m: n_alive * scale / 1e6,
        mean_age: div(age_sum, n_alive),
        retirees_share,
        age65_share: 100.0 * div(age65 as f64, n_alive),
        partnered_share: 100.0 * div(partnered as f64, n_alive),

        sfc_error,
        obs: Observations::default(),
        cipi_by_sector_real_bn_ar: cipi_by_sector_real,
        firm_shortfall_imports_bn_ar: fl.firm_shortfall_imports * ann,
        hh_unmet_imports_bn_ar: fl.hh_unmet_imports * ann,
        bk_max_heads_pct: 100.0 * div(f64::from(fl.bk_max_heads), employed_f),
        world: None,
    };
    let locals = ObsLocals {
        res_real,
        owners,
        tenure_base,
        employed: employed_f,
        overdrawn,
        overdraft_sum,
        n_alive,
        gov_real,
    };
    fill_observations(w, st, &mut m, &locals, &dist);
    m
}


/// Observation fields added in metrics-out (step 0b). Flattened into the monthly JSON record, so every key is top
/// level. Nothing here feeds back into the model. Units follow the rest of `Metrics`: rates in percent, money in
/// $bn at annual rates (`_bn_ar`) or $bn stocks (`_bn`), counts scaled to the US unless named `_n` (raw model count).
/// Quarterly fields (`dist`) and annual observation fields carry forward between refreshes, annual fields between year-ends.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Observations {
    // --- equity and the Fund ---
    /// Monthly total return on the Fund's equity (capital gain plus equity dividends paid), percent.
    pub equity_return_1m: f64,
    /// Regular dividends declared by firms this month (bank dividends excluded), $bn ar. Paid to the Fund next month.
    pub fund_dividends_regular_bn_ar: f64,
    /// Special payouts declared by firms this month (surplus-cash buybacks; bank excess capital excluded), $bn ar.
    /// Paid to the Fund next month.
    pub fund_dividends_special_bn_ar: f64,
    /// Dividends in the Fund's equity return this month, $bn ar: firm declarations of last month (regular plus
    /// special) plus bank dividends declared this month. This is the amount behind `equity_return_1m`.
    pub fund_dividends_paid_bn_ar: f64,
    /// Bank dividends declared this month (regular and excess-capital payouts together), $bn ar.
    pub bank_dividends_bn_ar: f64,
    /// Long rate equity investors discount at (y_eq), percent.
    pub equity_discount_rate: f64,
    /// Firm profit by sector as booked (before dividends), annualised, $bn ar. Sector::ALL order.
    pub sector_earnings_bn_ar: [f64; N_SECTORS],
    /// Regular dividends paid by each sector this month, annualised, $bn ar.
    pub sector_dividends_bn_ar: [f64; N_SECTORS],
    /// Sector Tobin's q relative to its normal level (1.0 at the first valuation).
    pub sector_q: [f64; N_SECTORS],

    // --- policy and the curve ---
    /// Expected average short rate over each tenor (the expectations component of the curve), percent. TENORS order.
    pub expected_short_rate: [f64; N_TENORS],
    /// Unrounded smoothed shadow policy rate, percent.
    pub shadow_rate: f64,
    /// Real-time natural real rate estimate r*, percent (the configured value until the rule adapts it). Same value as the
    /// top-level `Metrics::r_star_est`. Joint calibration 4 (R-19): named `obs_r_star_est` because this struct is flattened
    /// into `Metrics`, and the old name `r_star_est` made the JSON carry two `r_star_est` keys (the compare mean returned the
    /// second one as 0.0).
    pub obs_r_star_est: f64,
    /// Real-time natural unemployment estimate u*, percent.
    pub u_star: f64,
    // --- p1-curve state (present only when `Config::curve_report` is on) ---
    /// Long-run expected inflation π^LR (the curve's long-run anchor is r* + π^LR), percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pi_lr: Option<f64>,
    /// Unsmoothed rule target i* this month, percent (the rule's raw target before the EWMA).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i_star: Option<f64>,
    /// EWMA of the rule target that the curve prices, percent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub i_star_smooth: Option<f64>,
    /// Term-premium level factor state (the exogenous AR(1), plus its financial loading), percent of yield.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tp_level: Option<f64>,
    /// Term-premium curvature factor state (the Nelson–Siegel hump factor), percent of yield.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tp_curv: Option<f64>,

    // --- banks and credit ---
    /// Mean lender balance-sheet premium on business loans across alive banks, percent.
    pub lender_premium: f64,
    /// Expected business loss rate (PD × LGD on firm debt, business-loan weighted), annual percent. Monthly.
    pub expected_business_loss_rate: f64,
    /// Business write-offs of the current and two previous months, annualised (x4), percent of business loans. A
    /// three-month window, not a trailing 12-month sum.
    pub biz_chargeoff_3m_ar: f64,
    /// Loan-weighted average rate on loans to firms (includes the default-risk premium), percent.
    pub firm_loan_rate: f64,
    /// 90th percentile of the firm-loan rate, weighted by principal, percent. Monthly.
    pub firm_loan_rate_p90: f64,
    /// Interest expense of banks (deposit interest and discount-window interest), $bn ar.
    pub bank_interest_expense_bn_ar: f64,
    /// Net interest income of banks (interest income less interest expense), $bn ar.
    pub net_interest_income_bn_ar: f64,
    /// Total bank assets (reserves, loans and government bonds), $bn.
    pub bank_assets_bn: f64,
    /// Consumer (non-mortgage household) loan balances, $bn stock.
    pub consumer_credit_bn: f64,
    /// Mortgage balance that is at least 1 month delinquent, percent of mortgage balance.
    pub mortgage_delinquent_30_share: f64,
    /// Mortgage balance that is at least 3 months delinquent, percent of mortgage balance.
    pub mortgage_delinquent_90_share: f64,
    /// Alive households with negative deposits (overdrawn), percent of alive households.
    pub overdraft_share: f64,
    /// Household overdraft balances (negative deposits, alive households), $bn stock.
    pub overdraft_balance_bn: f64,

    // --- firms ---
    /// New firms (entries), annualised count.
    pub firm_entries_ar: f64,
    /// Alive firms by age class (<1y, 1-4y, 5-9y, 10y+), count.
    pub firms_by_age: [f64; 4],
    /// Firm exits this month by age at exit, annualised count (x12, scaled). Classes: <1y, 1-1.9y, 2-4.9y, 5-9.9y,
    /// 10y+. A firm exits when it goes bankrupt; the exit month is the one in which its slot dies.
    pub firm_exits_by_age_ar: [f64; 5],
    /// Employed heads displaced this month by firms that went bankrupt, annualised count (x12, scaled). Heads of a
    /// firm at exit are its last-month workers, so the ratio to exits is the mean employment of exiting firms.
    pub bankruptcy_displaced_heads_ar: f64,
    /// Employed heads displaced by bankruptcy this month, percent of employed heads at the previous record.
    pub flow_bankruptcy_displaced_pct: f64,
    /// Alive firms by size class (persons: employees x 1.75; classes 0-4, 5-9, 10-19, 20-49, 50-99, 100-249,
    /// 250-499, 500-999, 1000+), count.
    pub firm_size_counts: [f64; 9],
    /// Employment share by size class (persons basis), percent of employment at alive firms.
    pub firm_size_emp_share: [f64; 9],
    /// Employment share of firms with 500 or more persons (heads x 1.75), percent. Equals the two top size classes.
    pub firm_share_500plus: f64,
    /// Zipf exponent of the firm-size tail (−slope of log rank on log size in persons, firms with 10+ persons).
    /// Annual.
    pub firm_size_tail_exponent: f64,
    /// Annual job creation by firm age, percent of average employment, in three classes: 0 = firms aged under 12
    /// months at the start of the year (BDS ages 0 and 1, so this includes continuing firms), 1 = aged 1-4 years,
    /// 2 = aged 5 years and over (age at the start of the year). Updated each January.
    pub job_creation_by_age: [f64; 3],
    /// Annual job creation by firms born since the last January (BDS age 0 only), percent of average employment.
    /// Each January. Its share of total job creation is the startup share.
    pub job_creation_entrants_pct: f64,
    /// Annual job destruction by firm age (same classes; exits included), percent of average employment. Each January.
    pub job_destruction_by_age: [f64; 3],
    /// Markup (price over unit cost) of alive firms: 10th percentile, percent (100 × markup). Annual.
    pub markup_p10: f64,
    /// Median markup, percent (100 × markup). Annual.
    pub markup_p50: f64,
    /// 90th percentile of the markup, percent (100 × markup). Annual.
    pub markup_p90: f64,
    /// Cross-sectional variance of log wage OFFERS across alive firms (offer dispersion, not realised pay). Offers
    /// are near-identical across firms early on, so this is close to zero; use `wage_log_var_between_firm` for
    /// realised between-employer pay dispersion. Monthly.
    pub wage_offer_log_var: f64,
    /// Coefficient of variation of wage offers across alive firms, percent.
    pub wage_offer_cv: f64,
    /// Realised log-wage variance of employed heads (total, head-weighted). Annual (January).
    pub wage_log_var_total: f64,
    /// Between-employer part of `wage_log_var_total`: head-weighted variance of employer mean log wages. Annual.
    pub wage_log_var_between_firm: f64,
    /// Within-employer part of `wage_log_var_total` (between + within = total). Annual.
    pub wage_log_var_within_firm: f64,
    /// Between-employer share of the realised log-wage variance, percent. Raw (no worker fixed effects). Annual.
    pub wage_between_firm_share: f64,
    /// Share of alive firms with a full 12-month window whose investment (sum of monthly orders over the year to
    /// January) exceeds 20% of capital, percent. Annual; null until 12 months are recorded.
    #[serde(deserialize_with = "de_null_nan")]
    pub investment_spike_share: f64,
    /// Share of alive firms with a full 12-month window whose investment is below 1% of capital in absolute value,
    /// percent. Annual; null until 12 months are recorded.
    #[serde(deserialize_with = "de_null_nan")]
    pub investment_inaction_share: f64,

    // --- b5-firmhet: firm-distribution observations (S0; read-only, monthly; see fill_firm_dist) ---
    /// Alive firms per sector (raw model count).
    #[serde(default)]
    pub firms_by_sector: [f64; N_SECTORS],
    /// Herfindahl of firm revenue within each sector (0..1; 1 = monopoly).
    #[serde(default)]
    pub firm_hhi_by_sector: [f64; N_SECTORS],
    /// Share of employed heads held by the largest 1% of alive firms, percent.
    #[serde(default)]
    pub firm_emp_top1_share: f64,
    /// Share of employed heads held by the largest 0.34% of alive firms (the US share of firms with 500+ staff), percent.
    #[serde(default)]
    pub firm_emp_top034_share: f64,
    /// Within-sector sd of log labour productivity (pooled, sector-demeaned, productive firms only).
    #[serde(default, deserialize_with = "de_null_nan")]
    pub firm_lp_sd_within: f64,
    /// Pooled 90-10 range of log labour productivity after sector demeaning.
    #[serde(default, deserialize_with = "de_null_nan")]
    pub firm_lp_p90_p10: f64,
    /// Olley-Pakes gap: within sector, employment-weighted minus unweighted mean log productivity, then
    /// employment-weighted across sectors (log points).
    #[serde(default, deserialize_with = "de_null_nan")]
    pub firm_op_gap_lp: f64,
    /// OLS slope of ln heads on sector-relative ln productivity.
    #[serde(default, deserialize_with = "de_null_nan")]
    pub firm_size_lp_slope: f64,
    /// Percentiles of the markup over variable cost, 100 × (price / (unit cost − capital cost per unit at normal
    /// utilisation) − 1). Capital cost per unit = δ/12 × P_K / (capital productivity × u_n); the target-return term is
    /// excluded (De Loecker, Eeckhout & Unger 2020 concept).
    #[serde(default, deserialize_with = "de_null_nan")]
    pub markup_avc_p10: f64,
    #[serde(default, deserialize_with = "de_null_nan")]
    pub markup_avc_p50: f64,
    #[serde(default, deserialize_with = "de_null_nan")]
    pub markup_avc_p90: f64,
    /// Percentiles of ln(K / k_star_smooth) across alive firms with positive capital and target.
    #[serde(default, deserialize_with = "de_null_nan")]
    pub capital_gap_p10: f64,
    #[serde(default, deserialize_with = "de_null_nan")]
    pub capital_gap_p50: f64,
    #[serde(default, deserialize_with = "de_null_nan")]
    pub capital_gap_p90: f64,
    /// Sd of ln(K / k_star_smooth) around its sector mean (pooled).
    #[serde(default, deserialize_with = "de_null_nan")]
    pub capital_gap_sd_within: f64,
    /// Median ln(K / k_star_smooth) by sector (0 where the sector has no firm with a defined gap).
    #[serde(default)]
    pub capital_gap_median_by_sector: [f64; N_SECTORS],
    /// Capacity utilisation by sector (output / (capital × capital productivity)), percent.
    #[serde(default)]
    pub cu_by_sector: [f64; N_SECTORS],
    /// Owner-operated firms by the owner's status (raw counts): working in own firm, employed elsewhere, unemployed,
    /// not in the labour force.
    #[serde(default)]
    pub owners_in_own_firm: f64,
    #[serde(default)]
    pub owners_employed_elsewhere: f64,
    #[serde(default)]
    pub owners_unemployed: f64,
    #[serde(default)]
    pub owners_not_in_lf: f64,

    // --- income-side firm and bank accounts (monthly, $bn at annual rates unless stated; observation only) ---
    /// After-tax profits of shareholder-owned firms (no owner household): the NIPA corporate-profits analogue.
    #[serde(default)]
    pub corp_profits_bn_ar: f64,
    /// After-tax profits of owner-operated firms (NIPA: part of proprietors' income, with the owner's own pay).
    #[serde(default)]
    pub owner_firm_profits_bn_ar: f64,
    /// Wages paid by owner-operated firms to their own owners (NIPA counts this in proprietors' income).
    #[serde(default)]
    pub owner_own_wage_bn_ar: f64,
    /// Bank net income after the corporate tax paid this month (bank_profits_bn_ar is before tax).
    #[serde(default)]
    pub bank_net_income_bn_ar: f64,
    /// Firm-sector accounts: gross value added at basic prices (revenue less intermediate inputs, property tax
    /// included in value added), wage bill (incl. employer taxes and benefits), depreciation, interest paid,
    /// corporate tax, and business debt (stock, $bn).
    #[serde(default)]
    pub firm_va_bn_ar: f64,
    #[serde(default)]
    pub firm_wage_bill_bn_ar: f64,
    #[serde(default)]
    pub firm_depreciation_bn_ar: f64,
    #[serde(default)]
    pub firm_interest_bn_ar: f64,
    #[serde(default)]
    pub firm_corp_tax_bn_ar: f64,
    #[serde(default)]
    pub firm_debt_bn: f64,
    /// Re-fit Step 4c: production taxes on domestic crude (to the Treasury) and private royalties (to the Fund), $bn
    /// annualised. Not intermediate consumption, so they are added back to `firm_va_bn_ar`. Zero with the split off.
    #[serde(default)]
    pub upstream_tax_bn_ar: f64,
    #[serde(default)]
    pub upstream_royalty_bn_ar: f64,
    /// Effective markup quantiles: price over current unit cost of alive firms, percent above 1 (annual, January).
    #[serde(default)]
    pub markup_eff_p10: f64,
    #[serde(default)]
    pub markup_eff_p50: f64,
    #[serde(default)]
    pub markup_eff_p90: f64,
    /// After-tax profits of alive firms by sector (all firms, shareholder-owned and owner-operated), $bn at annual rates.
    #[serde(default)]
    pub firm_profits_by_sector_bn_ar: [f64; N_SECTORS],
    /// Real government purchases (the `gov_real` term of real GDP), $bn at annual rates.
    #[serde(default)]
    pub government_real_bn_ar: f64,
    /// Wages per unit of human capital over employed household heads (model $ per month per efficiency unit).
    /// NaN (written as null) when no head is employed.
    #[serde(default, deserialize_with = "de_null_nan")]
    pub wage_per_eff: f64,
    /// The same ratio over employed heads in their first month in the job (`months_in_job` == 0).
    /// NaN (written as null) in a month with no new hires.
    #[serde(default, deserialize_with = "de_null_nan")]
    pub newhire_wage_per_eff: f64,

    // --- worker flows (monthly, percent) ---
    /// Employer-to-employer moves, percent of employed household heads.
    pub flow_ee_pct: f64,
    /// Exogenous employed-to-unemployed separations, percent of employed household heads.
    pub flow_eu_exo_pct: f64,
    /// Hires from unemployment into employment, percent of unemployed household heads at the start of the labour
    /// step (the same stock the hires are drawn from).
    pub flow_ue_pct: f64,
    /// Employed household heads (raw model count, unscaled): sampling-noise indicator.
    pub employed_heads_n: f64,

    // --- stayers' wage dynamics (annual) ---
    /// 10th percentile of the 12-month log wage change of job-stayers, percent. Each January; null until a year
    /// with at least 10 stayers and a full year-end snapshot exists.
    #[serde(deserialize_with = "de_null_nan")]
    pub stayer_wage_change_p10: f64,
    /// Median 12-month log wage change of job-stayers, percent. Each January; null until valid.
    #[serde(deserialize_with = "de_null_nan")]
    pub stayer_wage_change_p50: f64,
    /// 90th percentile of the 12-month log wage change of job-stayers, percent. Each January; null until valid.
    #[serde(deserialize_with = "de_null_nan")]
    pub stayer_wage_change_p90: f64,
    /// Kelley skewness of the 12-month log wage change of stayers: (P90 + P10 − 2 P50) ÷ (P90 − P10). Each January;
    /// null until valid.
    #[serde(deserialize_with = "de_null_nan")]
    pub stayer_wage_change_kelley: f64,

    // --- distribution (existing fields quarterly; observation fields annual) ---
    /// Share of total net worth held by the top 1% of households, percent.
    pub wealth_top1_share: f64,
    /// Share of total net worth held by the bottom 50% of households, percent.
    pub wealth_bottom50_share: f64,
    /// Households with net worth at or below zero, percent of alive households.
    pub share_nonpositive_nw: f64,
    /// Median net worth by age bucket (AGE_BUCKETS), model dollars per household.
    pub median_nw_by_age: [f64; 6],
    /// Hand-to-mouth households (deposits below half a month's gross income), percent of alive households.
    pub htm_share: f64,
    /// Wealthy hand-to-mouth: hand-to-mouth households that own their home (illiquid housing), percent.
    pub htm_wealthy_share: f64,
    /// Gini of disposable income (gross income less taxes), annual.
    pub disposable_income_gini: f64,
    /// Gini of earnings (wage income, annual; zeros for non-workers).
    pub earnings_gini: f64,
    /// Gini of real consumption per household.
    pub consumption_gini: f64,
    /// Share of total earnings held by the top 1% of households, percent.
    pub earnings_top1_share: f64,

    // --- real activity, prices and housing ---
    /// Real residential investment (construction output in housing), $bn ar.
    pub real_residential_investment_bn_ar: f64,
    /// Nominal trend GDP (the fiscal trend used for government purchases), $bn ar.
    pub trend_gdp_nominal_bn_ar: f64,
    /// Energy-sector price level (the Energy price the household energy bill is based on; 1.0 at t=0).
    pub energy_price_level: f64,
    /// Core PCE-weighted price level (the model's core PCE index).
    pub core_pce_level: f64,
    /// Oil price AR(1) state (log deviation), percent.
    pub oil_ar_pct: f64,
    /// Oil user-shock component (log deviation), percent.
    pub oil_shock_pct: f64,
    /// Average nominal price of this month's house sales relative to the first month with sales, 100 = base.
    pub transaction_price_index: f64,
    /// Expected annual house-price appreciation used by buyers, percent.
    pub expected_hpa: f64,
    /// Real trade-weighted dollar index (dollar index deflated by the CPI and the world price), 100 at t=0.
    pub real_dollar_index: f64,
    /// World price level (foreign-currency price of the world basket), 100 at t=0.
    pub world_price_index: f64,
    /// Productivity (TFP) level by sector, 1.0 at t=0. Sector::ALL order.
    pub tfp_level: [f64; N_SECTORS],
    /// Owner-occupier households, millions (scaled).
    pub owners_m: f64,
    /// Households in the homeownership denominator, millions (scaled).
    pub tenure_base_m: f64,
}


// ---------------------------------------------------------------------------------------------
// metrics-out (step 0b): observation fields from existing state
// ---------------------------------------------------------------------------------------------

/// Number of firm-size classes (see `SIZE_UPPER`).
const N_SIZE_CLASSES: usize = 9;
/// Firm-size class upper bounds (persons): classes are (B[k], B[k+1]] for k = 0..9, with B[0] = 0.
const SIZE_UPPER: [u32; 10] = FIRM_SIZE_BOUNDS;
/// Firm age class at a given age in months: <12, <60, <120, 120 and over.
fn firm_age_class(age_months: u32) -> usize {
    match age_months {
        0..=11 => 0,
        12..=59 => 1,
        60..=119 => 2,
        _ => 3,
    }
}
/// Firm age class at the start of a year (months): 0 = first year, 1 = 1-4 years, 2 = 5 years and over.
fn age_class3(age_months: u32) -> usize {
    match age_months {
        0..=11 => 0,
        12..=59 => 1,
        _ => 2,
    }
}
/// Firm-size class (0..9) of a firm's size in persons (heads x PERSONS_PER_EMPLOYED_HEAD), the same unit as
/// `employment_persons_millions` and the mean firm size in facts_firms.py.
fn size_class_persons(persons: f64) -> usize {
    (1..SIZE_UPPER.len()).find(|&i| persons <= SIZE_UPPER[i] as f64).map_or(0, |i| i - 1).min(8)
}
/// Firm size in persons for a firm with `heads` employed household heads.
fn firm_persons(heads: usize) -> f64 {
    heads as f64 * PERSONS_PER_EMPLOYED_HEAD
}
/// Exit age class (months at exit): <12, 12-23, 24-59, 60-119, 120 and over. The first two give exits under 2 years.
fn exit_age_class(age_months: u32) -> usize {
    match age_months {
        0..=11 => 0,
        12..=23 => 1,
        24..=59 => 2,
        60..=119 => 3,
        _ => 4,
    }
}
/// Split of the log-wage variance of employed heads into between-employer and within-employer parts. `groups` holds
/// per employer [count, sum of log wages, sum of squared log wages]. Returns (total, between, within) with
/// total = between + within exactly (head-weighted), so the between share is in [0, 1].
fn wage_variance_split(groups: &[[f64; 3]]) -> (f64, f64, f64) {
    let n: f64 = groups.iter().map(|g| g[0]).sum();
    if n <= 1.0 {
        return (0.0, 0.0, 0.0);
    }
    let s: f64 = groups.iter().map(|g| g[1]).sum();
    let ss: f64 = groups.iter().map(|g| g[2]).sum();
    let mu = s / n;
    let total = (ss / n - mu * mu).max(0.0);
    let (mut between, mut within) = (0.0f64, 0.0f64);
    for g in groups.iter().filter(|g| g[0] > 0.0) {
        let mf = g[1] / g[0];
        between += g[0] * (mf - mu) * (mf - mu);
        within += (g[2] - g[1] * g[1] / g[0]).max(0.0);
    }
    (total, between / n, within / n)
}

/// Quantile of values sorted ascending (nearest rank).
fn quantile_sorted(v: &[f64], p: f64) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    let k = ((p * v.len() as f64).ceil() as usize).clamp(1, v.len());
    v[k - 1]
}

/// Household row for the distribution block (alive households only).
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExtRow {
    /// Disposable income (gross income less taxes), annual.
    disp: f64,
    /// Wage income, annual.
    earn: f64,
    /// Age in whole years.
    age: u32,
    /// Net worth.
    nw: f64,
    /// Deposits below half a month's gross income.
    htm: bool,
    /// Owner-occupier.
    owner: bool,
}

/// Snapshot of a firm slot at the previous year-end.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FirmSnap {
    age: u32,
    emp: u32,
    alive: bool,
}

/// Snapshot of a household slot at the previous year-end.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HhSnap {
    age: u32,
    employer: u32,
    wage: f64,
    employed: bool,
}

/// Annual observations: year-end snapshots and the statistics computed from the last two of them.
#[derive(Clone, Debug, Default)]
pub struct AnnualCache {
    have_snap: bool,
    firm_snap: Vec<FirmSnap>,
    hh_snap: Vec<HhSnap>,
    pub job_creation_by_age: [f64; 3],
    pub job_destruction_by_age: [f64; 3],
    /// Job creation by firms born since the last refresh (BDS age 0), percent of average employment.
    pub job_creation_entrants_pct: f64,
    /// Stayers' 12-month log wage change percentiles; valid only when `stayer_valid` (else reported as null).
    pub stayer_valid: bool,
    pub stayer_p10: f64,
    pub stayer_p50: f64,
    pub stayer_p90: f64,
    pub stayer_kelley: f64,
    /// Realised log-wage variance of employed heads (total, between employers, within employers), and the
    /// between-employer share of it, percent.
    pub pay_var_total: f64,
    pub pay_var_between: f64,
    pub pay_var_within: f64,
    pub pay_between_share: f64,
    /// Annual investment-rate shares from the 12-month order sums; valid only when `inv_valid`.
    pub inv_valid: bool,
    pub inv_spike_share: f64,
    pub inv_inaction_share: f64,
}

/// Annual distribution and firm-level observations (carried forward between refreshes).
#[derive(Clone, Debug, Default)]
pub struct DistObs {
    pub wealth_top1_share: f64,
    pub wealth_bottom50_share: f64,
    pub share_nonpositive_nw: f64,
    pub median_nw_by_age: [f64; 6],
    pub htm_share: f64,
    pub htm_wealthy_share: f64,
    pub disposable_income_gini: f64,
    pub earnings_gini: f64,
    pub consumption_gini: f64,
    pub earnings_top1_share: f64,
    pub firm_size_tail_exponent: f64,
    pub markup_p10: f64,
    pub markup_p50: f64,
    pub markup_p90: f64,
    pub markup_eff_p10: f64,
    pub markup_eff_p50: f64,
    pub markup_eff_p90: f64,
}

/// Annual refresh of the distribution observations. `cons` is real consumption per household (alive), `nws` the
/// net worths sorted ascending.
fn refresh_dist_obs(w: &World, ext: &[ExtRow], cons: &[f64], nws: &[f64], d: &mut DistObs) {
    let n = ext.len();
    let mut disp: Vec<f64> = ext.iter().map(|r| r.disp).collect();
    let mut earn: Vec<f64> = ext.iter().map(|r| r.earn).collect();
    let mut cs: Vec<f64> = cons.to_vec();
    let mut mk: Vec<f64> = w.firms.iter().filter(|f| f.alive).map(|f| 100.0 * f.markup).collect();
    // Sorted in total_cmp order (the same sequence as sort_unstable_by(total_cmp)), in parallel.
    rayon::join(
        || rayon::join(|| par_sort_total(&mut disp), || par_sort_total(&mut earn)),
        || rayon::join(|| par_sort_total(&mut cs), || par_sort_total(&mut mk)),
    );
    if n > 0 && !nws.is_empty() {
        let total: f64 = nws.iter().sum();
        d.wealth_top1_share = top_share(nws, 0.01);
        d.wealth_bottom50_share = if total > 0.0 {
            100.0 * nws[..nws.len() / 2].iter().sum::<f64>() / total
        } else {
            0.0
        };
        d.share_nonpositive_nw = 100.0 * nws.iter().filter(|&&x| x <= 0.0).count() as f64 / n as f64;
        let mut htm = 0usize;
        let mut wealthy = 0usize;
        for r in ext {
            if r.htm {
                htm += 1;
                if r.owner {
                    wealthy += 1;
                }
            }
        }
        d.htm_share = 100.0 * htm as f64 / n as f64;
        d.htm_wealthy_share = 100.0 * wealthy as f64 / n as f64;
        // Net worth by age bucket: one bucketing pass, then a selection per bucket.
        let mut by_age: [Vec<f64>; 6] = std::array::from_fn(|_| Vec::new());
        for r in ext {
            if let Some(b) = bucket_of(r.age) {
                by_age[b].push(r.nw);
            }
        }
        for (b, slot) in d.median_nw_by_age.iter_mut().enumerate() {
            let v = &mut by_age[b];
            *slot = if v.is_empty() { 0.0 } else { median_mut(v) };
        }
        d.disposable_income_gini = gini_sorted(&disp);
        d.earnings_gini = gini_sorted(&earn);
        d.earnings_top1_share = top_share(&earn, 0.01);
    }
    d.consumption_gini = gini_sorted(&cs);

    // Firms: markup quantiles, and the Zipf tail of firm size.
    d.markup_p10 = quantile_sorted(&mk, 0.10);
    d.markup_p50 = quantile_sorted(&mk, 0.50);
    d.markup_p90 = quantile_sorted(&mk, 0.90);
    let mut mke: Vec<f64> =
        w.firms.iter().filter(|f| f.alive && f.unit_cost > 0.0).map(|f| 100.0 * (f.price / f.unit_cost - 1.0)).collect();
    mke.sort_unstable_by(|a, b| a.total_cmp(b));
    d.markup_eff_p10 = quantile_sorted(&mke, 0.10);
    d.markup_eff_p50 = quantile_sorted(&mke, 0.50);
    d.markup_eff_p90 = quantile_sorted(&mke, 0.90);
    // Sizes in persons (the unit of the size classes), firms with 10 or more persons.
    let mut sizes: Vec<f64> = w
        .firms
        .iter()
        .filter(|f| f.alive)
        .map(|f| firm_persons(f.employees()))
        .filter(|&p| p >= 10.0)
        .collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    d.firm_size_tail_exponent = if sizes.len() >= 10 {
        // ln(rank) = a + b ln(size) over ranks 1..N (largest first): Zipf exponent alpha = -b.
        let (mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0);
        for (k, &s) in sizes.iter().enumerate() {
            let x = s.ln();
            let y = ((k + 1) as f64).ln();
            sx += x;
            sy += y;
            sxx += x * x;
            sxy += x * y;
        }
        let m = sizes.len() as f64;
        let den = m * sxx - sx * sx;
        if den.abs() > 1e-12 { -((m * sxy - sx * sy) / den) } else { 0.0 }
    } else {
        0.0
    };

}

/// Monthly credit indicators: the principal-weighted 90th percentile of the firm-loan rate (percent) and the business-loan
/// weighted expected business loss rate (annual percent). Cheap (about 1k firm loans and one banking pass), so monthly.
fn credit_obs(w: &World) -> (f64, f64) {
    // Firm loans: loan-weighted quantile of the rate, and the expected business loss rate (by bank business loans).
    let mut fl: Vec<(f64, f64)> = w
        .loans
        .iter()
        .filter(|(_, l)| matches!(l.borrower, Borrower::Firm(_)) && l.principal > 0.0)
        .map(|(_, l)| (l.annual_rate, l.principal))
        .collect();
    fl.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tot: f64 = fl.iter().map(|x| x.1).sum();
    let firm_loan_rate_p90 = if tot > 0.0 {
        let mut acc = 0.0;
        let mut q = fl.last().map_or(0.0, |x| x.0);
        for &(r, p) in &fl {
            acc += p;
            if acc >= 0.9 * tot {
                q = r;
                break;
            }
        }
        100.0 * q
    } else {
        0.0
    };
    let el = crate::banking::expected_loss_rates(w);
    let (mut num, mut den) = (0.0, 0.0);
    for (b, bank) in w.banks.iter().enumerate() {
        if bank.alive && bank.business_loans > 0.0 {
            num += el[b] * bank.business_loans;
            den += bank.business_loans;
        }
    }
    let expected_business_loss_rate = if den > 0.0 { 100.0 * num / den } else { 0.0 };
    (firm_loan_rate_p90, expected_business_loss_rate)
}

/// Annual update at a year-end (tick % 12 == 0): job creation and destruction by firm age, and stayers' 12-month
/// log wage changes, both from the previous year-end snapshot; the realised pay variance split, which needs no
/// snapshot. Stores the new snapshot.
fn update_annual(w: &World, a: &mut AnnualCache) {
    // Realised pay: log wages of employed heads, split into between-employer and within-employer variance.
    let mut groups = vec![[0.0f64; 3]; w.firms.len()];
    for h in w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed && h.wage > 0.0) {
        let Some(f) = h.employer else { continue };
        let Some(g) = groups.get_mut(f as usize) else { continue };
        let l = h.wage.ln();
        g[0] += 1.0;
        g[1] += l;
        g[2] += l * l;
    }
    let (total, between, within) = wage_variance_split(&groups);
    a.pay_var_total = total;
    a.pay_var_between = between;
    a.pay_var_within = within;
    a.pay_between_share = if total > 0.0 { 100.0 * between / total } else { 0.0 };

    let cur_f: Vec<FirmSnap> = w
        .firms
        .iter()
        .map(|f| FirmSnap { age: f.age_months, emp: f.employees() as u32, alive: f.alive })
        .collect();
    a.stayer_valid = false;
    if a.have_snap {
        let mut cr = [0.0f64; 3];
        let mut de = [0.0f64; 3];
        let mut entrant_cr = 0.0f64;
        let (mut emp_prev, mut emp_cur) = (0.0f64, 0.0f64);
        for (i, c) in cur_f.iter().enumerate() {
            let p = a.firm_snap.get(i).copied().unwrap_or_default();
            if p.alive {
                emp_prev += p.emp as f64;
            }
            if c.alive {
                emp_cur += c.emp as f64;
            }
            let same = p.alive && c.alive && c.age == p.age + 12;
            if same {
                let d = c.emp as f64 - p.emp as f64;
                let k = age_class3(p.age);
                if d > 0.0 {
                    cr[k] += d;
                } else {
                    de[k] -= d;
                }
            } else {
                if p.alive {
                    de[age_class3(p.age)] += p.emp as f64;
                }
                if c.alive {
                    // A slot that is not a continuing firm is a firm born since the last refresh (BDS age 0).
                    cr[0] += c.emp as f64;
                    entrant_cr += c.emp as f64;
                }
            }
        }
        let avg = 0.5 * (emp_prev + emp_cur);
        for k in 0..3 {
            a.job_creation_by_age[k] = 100.0 * div(cr[k], avg);
            a.job_destruction_by_age[k] = 100.0 * div(de[k], avg);
        }
        a.job_creation_entrants_pct = 100.0 * div(entrant_cr, avg);
        // Stayers: employed now with 12+ months in the job, at the same employer 12 months ago (same household).
        let mut ch: Vec<f64> = Vec::new();
        for (i, h) in w.households.iter().enumerate() {
            let Some(p) = a.hh_snap.get(i) else { continue };
            if !(h.alive && h.status == LaborStatus::Employed && h.months_in_job >= 12) {
                continue;
            }
            let Some(f) = h.employer else { continue };
            if p.employed && p.employer == f && p.age + 12 == h.age_months as u32 && p.wage > 0.0 && h.wage > 0.0 {
                ch.push(100.0 * (h.wage / p.wage).ln());
            }
        }
        if ch.len() >= 10 {
            a.stayer_valid = true;
            ch.sort_by(|x, y| x.total_cmp(y));
            a.stayer_p10 = quantile_sorted(&ch, 0.10);
            a.stayer_p50 = quantile_sorted(&ch, 0.50);
            a.stayer_p90 = quantile_sorted(&ch, 0.90);
            let den = a.stayer_p90 - a.stayer_p10;
            a.stayer_kelley = if den.abs() > 1e-12 { (a.stayer_p90 + a.stayer_p10 - 2.0 * a.stayer_p50) / den } else { 0.0 };
        }
    }
    a.firm_snap = cur_f;
    a.hh_snap = w
        .households
        .iter()
        .map(|h| HhSnap {
            age: h.age_months as u32,
            employer: h.employer.unwrap_or(u32::MAX),
            wage: h.wage,
            employed: h.alive && h.status == LaborStatus::Employed && h.employer.is_some(),
        })
        .collect();
    a.have_snap = true;
}

/// Locals of `compute` that the observation fields reuse.
pub(crate) struct ObsLocals {
    pub res_real: f64,
    pub owners: usize,
    pub tenure_base: usize,
    pub employed: f64,
    pub overdrawn: usize,
    pub overdraft_sum: f64,
    pub n_alive: f64,
    pub gov_real: f64,
}

/// Fill the monthly observation fields (and the carried-forward quarterly and annual ones) into `m.obs`.
fn fill_observations(w: &World, st: &mut MetricsState, m: &mut Metrics, loc: &ObsLocals, dist: &DistCache) {
    let scale = w.cfg.effective_scale();
    let ann = 12.0 * scale / 1e9;
    let stock = scale / 1e9;
    let o = &mut m.obs;
    fill_firm_dist(w, o);

    // ---- equity and the Fund ----
    o.equity_return_1m = 100.0 * w.monetary.equity_returns.last().copied().unwrap_or(0.0);
    o.fund_dividends_regular_bn_ar = w.fund.dividends_received * ann;
    o.fund_dividends_special_bn_ar = w.fund.special_dividends * ann;
    o.equity_discount_rate = 100.0 * w.monetary.equity_long_rate.unwrap_or(0.0);
    o.sector_dividends_bn_ar = std::array::from_fn(|s| w.fund.sector_dividends_received[s] * ann);
    o.sector_q = w.fund.sector_q_rel;

    // ---- policy and the curve ----
    o.expected_short_rate = w.curve.expected.map(|x| 100.0 * x);
    o.shadow_rate = 100.0 * w.monetary.shadow_rate.unwrap_or(0.0);
    o.obs_r_star_est = 100.0 * w.monetary.r_star_est.unwrap_or(w.cfg.r_star);
    let u_star = if w.monetary.u_star > 0.0 { w.monetary.u_star } else { w.cfg.natural_unemployment };
    o.u_star = 100.0 * u_star;
    if w.cfg.curve_report {
        o.pi_lr = Some(100.0 * w.monetary.pi_lr.unwrap_or(w.cfg.inflation_target));
        o.i_star = w.monetary.i_star.map(|x| 100.0 * x);
        o.i_star_smooth = w.monetary.i_star_smooth.map(|x| 100.0 * x);
        o.tp_level = Some(100.0 * w.exog.tp_level());
        o.tp_curv = Some(100.0 * w.exog.tp_curv());
    }

    // ---- banks ----
    let (mut ebp_sum, mut nb_alive) = (0.0f64, 0usize);
    let (mut int_exp, mut int_inc, mut assets, mut biz_loans, mut biz_wo3) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for b in w.banks.iter().filter(|b| b.alive) {
        nb_alive += 1;
        ebp_sum += b.ebp;
        int_exp += b.interest_expense;
        int_inc += b.interest_income;
        assets += b.reserves + b.loans + b.bonds_value;
        // Last three months: this month's write-offs (not yet rolled into the history) and the two before it.
        let pos = b.biz_hist_pos as usize % 12;
        biz_wo3 += b.biz_writeoffs;
        for k in 1..=2 {
            biz_wo3 += b.biz_writeoff_hist[(pos + 12 - k) % 12];
        }
        biz_loans += b.business_loans.max(0.0);
    }
    o.lender_premium = 100.0 * div(ebp_sum, nb_alive as f64);
    o.biz_chargeoff_3m_ar = 400.0 * div(biz_wo3, biz_loans);
    o.bank_interest_expense_bn_ar = int_exp * ann;
    o.net_interest_income_bn_ar = (int_inc - int_exp) * ann;
    o.bank_assets_bn = assets * stock;

    // ---- loans ----
    let (mut firm_w, mut firm_p) = (0.0f64, 0.0f64);
    let (mut consumer, mut mort, mut d30, mut d90) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (_, l) in w.loans.iter() {
        match (l.borrower, l.kind) {
            (Borrower::Firm(_), _) => {
                firm_w += l.principal.max(0.0) * l.annual_rate;
                firm_p += l.principal.max(0.0);
            }
            (_, LoanKind::Consumer) => consumer += l.principal,
            _ => {}
        }
        if let LoanKind::Mortgage { .. } = l.kind {
            mort += l.principal;
            if l.months_delinquent >= 1 {
                d30 += l.principal;
            }
            if l.months_delinquent >= 3 {
                d90 += l.principal;
            }
        }
    }
    o.firm_loan_rate = 100.0 * div(firm_w, firm_p);
    o.consumer_credit_bn = consumer * stock;
    o.mortgage_delinquent_30_share = 100.0 * div(d30, mort);
    o.mortgage_delinquent_90_share = 100.0 * div(d90, mort);
    o.overdraft_share = 100.0 * div(loc.overdrawn as f64, loc.n_alive);
    o.overdraft_balance_bn = loc.overdraft_sum * stock;

    // ---- firms ----
    let mut age_n = [0.0f64; 4];
    let mut size_n = [0.0f64; N_SIZE_CLASSES];
    let mut size_emp = [0.0f64; N_SIZE_CLASSES];
    let mut profit_by_sector = [0.0f64; N_SECTORS];
    let (mut lw_sum, mut lw_sq, mut lw_n) = (0.0f64, 0.0f64, 0usize);
    let (mut wo_sum, mut wo_sq) = (0.0f64, 0.0f64);
    let mut emp_total = 0.0f64;
    for f in w.firms.iter().filter(|f| f.alive) {
        age_n[firm_age_class(f.age_months)] += 1.0;
        let e = f.employees();
        size_n[size_class_persons(firm_persons(e))] += 1.0;
        size_emp[size_class_persons(firm_persons(e))] += e as f64;
        emp_total += e as f64;
        profit_by_sector[f.sector.idx()] += f.profit;
        if f.wage_offer > 0.0 {
            let l = f.wage_offer.ln();
            lw_sum += l;
            lw_sq += l * l;
            lw_n += 1;
            wo_sum += f.wage_offer;
            wo_sq += f.wage_offer * f.wage_offer;
        }
    }
    // Firm exits this month: a slot that went bankrupt this month is dead with months_dead == 0 (bankrupt() sets it,
    // and it keeps age_months at death).
    let mut exits = [0.0f64; 5];
    for f in w.firms.iter().filter(|f| !f.alive && f.months_dead == 0) {
        exits[exit_age_class(f.age_months)] += 1.0;
    }
    o.firm_exits_by_age_ar = exits.map(|x| x * 12.0 * scale);
    o.firm_entries_ar = w.flows.firm_entries as f64 * 12.0 * scale;
    o.firms_by_age = age_n.map(|x| x * scale);
    o.firm_size_counts = size_n.map(|x| x * scale);
    o.firm_size_emp_share = size_emp.map(|x| 100.0 * div(x, emp_total));
    o.firm_share_500plus = o.firm_size_emp_share[7] + o.firm_size_emp_share[8];
    o.sector_earnings_bn_ar = profit_by_sector.map(|p| p * ann);
    o.wage_offer_log_var = if lw_n > 1 {
        let mu = lw_sum / lw_n as f64;
        (lw_sq / lw_n as f64 - mu * mu).max(0.0)
    } else {
        0.0
    };
    o.wage_offer_cv = if lw_n > 1 {
        let mu = wo_sum / lw_n as f64;
        100.0 * (wo_sq / lw_n as f64 - mu * mu).max(0.0).sqrt() / mu.max(1e-12)
    } else {
        0.0
    };

    // ---- worker flows (monthly) ----
    let fl = &w.flows;
    // Start-of-step unemployed heads (labour step tally), the stock the hires are drawn from.
    // Every denominator is a start-of-step stock from the labour tally, the same basis as the separations and hires.
    let u_start: f64 = fl.u_stock_edu.iter().map(|&x| x as f64).sum();
    let e_start: f64 = fl.emp_edu.iter().map(|&x| x as f64).sum();
    o.flow_ee_pct = 100.0 * div(fl.ee as f64, e_start);
    o.flow_eu_exo_pct = 100.0 * div(fl.sep_exo as f64, e_start);
    o.flow_ue_pct = 100.0 * div(fl.hires_from_u as f64, u_start);
    o.employed_heads_n = loc.employed;

    // Bankruptcy displacement: households employed at the previous record whose employer slot exited this month.
    // Metrics-only snapshot of the employer of each household; nothing here feeds back into the model.
    // Households added this month have no employer at the previous record (u32::MAX); existing entries are kept.
    st.prev_employer.resize(w.households.len(), u32::MAX);
    let (mut prev_emp_n, mut displaced) = (0usize, 0usize);
    for (i, h) in w.households.iter().enumerate() {
        let prev = st.prev_employer[i];
        if prev != u32::MAX {
            prev_emp_n += 1;
            if let Some(f) = w.firms.get(prev as usize) {
                if h.alive && !f.alive && f.months_dead == 0 {
                    displaced += 1;
                }
            }
        }
        st.prev_employer[i] = match (h.alive && h.status == LaborStatus::Employed, h.employer) {
            (true, Some(f)) => f,
            _ => u32::MAX,
        };
    }
    o.bankruptcy_displaced_heads_ar = displaced as f64 * 12.0 * scale;
    o.flow_bankruptcy_displaced_pct = 100.0 * div(displaced as f64, prev_emp_n as f64);

    // Fund dividends in the equity return this month: firm declarations of the previous record plus bank dividends
    // declared this month (banks' dividends are reset at the start of each month, so they are this month's).
    let bank_div: f64 = w.banks.iter().map(|b| if b.dividends.is_finite() { b.dividends.max(0.0) } else { 0.0 }).sum();
    o.bank_dividends_bn_ar = bank_div * ann;
    o.fund_dividends_paid_bn_ar = (st.prev_firm_div + bank_div) * ann;
    st.prev_firm_div = w.fund.dividends_received.max(0.0) + w.fund.special_dividends.max(0.0);

    // ---- real activity, prices, housing ----
    o.real_residential_investment_bn_ar = loc.res_real * ann;
    o.trend_gdp_nominal_bn_ar = crate::fiscal::trend_gdp_month(w) * ann;
    o.energy_price_level = w.prices.sector[Sector::Energy.idx()];
    o.core_pce_level = w.prices.core_pce;
    o.oil_ar_pct = 100.0 * w.exog.oil_ar;
    o.oil_shock_pct = 100.0 * w.exog.oil_shock;
    let txn = if fl.house_sales > 0 { fl.house_sale_value / fl.house_sales as f64 } else { 0.0 };
    if txn > 0.0 {
        if st.base_txn <= 0.0 {
            st.base_txn = txn;
        }
        st.txn_index = 100.0 * txn / st.base_txn;
    }
    o.transaction_price_index = if st.txn_index > 0.0 { st.txn_index } else { 100.0 };
    o.expected_hpa = 100.0 * crate::housing::expected_hpa(w);
    let cpi = w.prices.cpi;
    let pw = crate::common::world_price(w);
    if st.base_cpi <= 0.0 && cpi > 0.0 {
        st.base_cpi = cpi;
    }
    if st.base_world <= 0.0 && pw > 0.0 {
        st.base_world = pw;
    }
    let cpi_rel = if st.base_cpi > 0.0 { cpi / st.base_cpi } else { 1.0 };
    let pw_rel = if st.base_world > 0.0 { pw / st.base_world } else { 1.0 };
    o.world_price_index = 100.0 * pw_rel;
    o.real_dollar_index = 100.0 * w.row.fx * cpi_rel / pw_rel;
    o.tfp_level = w.exog.tfp;
    o.owners_m = loc.owners as f64 * scale / 1e6;
    o.tenure_base_m = loc.tenure_base as f64 * scale / 1e6;

    // ---- annual distribution and firm observations (carried forward) ----
    o.earnings_top1_share = dist.obs.earnings_top1_share;
    o.wealth_top1_share = dist.obs.wealth_top1_share;
    o.wealth_bottom50_share = dist.obs.wealth_bottom50_share;
    o.share_nonpositive_nw = dist.obs.share_nonpositive_nw;
    o.median_nw_by_age = dist.obs.median_nw_by_age;
    o.htm_share = dist.obs.htm_share;
    o.htm_wealthy_share = dist.obs.htm_wealthy_share;
    o.disposable_income_gini = dist.obs.disposable_income_gini;
    o.earnings_gini = dist.obs.earnings_gini;
    o.consumption_gini = dist.obs.consumption_gini;
    let (firm_loan_rate_p90, expected_business_loss_rate) = credit_obs(w);
    o.expected_business_loss_rate = expected_business_loss_rate;
    o.firm_loan_rate_p90 = firm_loan_rate_p90;
    o.firm_size_tail_exponent = dist.obs.firm_size_tail_exponent;
    o.markup_p10 = dist.obs.markup_p10;
    o.markup_p50 = dist.obs.markup_p50;
    o.markup_p90 = dist.obs.markup_p90;
    o.markup_eff_p10 = dist.obs.markup_eff_p10;
    o.markup_eff_p50 = dist.obs.markup_eff_p50;
    o.markup_eff_p90 = dist.obs.markup_eff_p90;
    o.government_real_bn_ar = loc.gov_real * ann;
    {
        let (mut wage_num, mut wage_den, mut nh_num, mut nh_den) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for h in w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed) {
            let hc = common::human_capital(&w.cfg, h);
            wage_num += h.wage;
            wage_den += hc;
            if h.months_in_job == 0 {
                nh_num += h.wage;
                nh_den += hc;
            }
        }
        o.wage_per_eff = if wage_den > 0.0 { wage_num / wage_den } else { f64::NAN };
        o.newhire_wage_per_eff = if nh_den > 0.0 { nh_num / nh_den } else { f64::NAN };
    }
    let ptax_m = w.cfg.business_property_tax / 12.0;
    let pk = common::capital_price_world(w);
    let mut sector_profit = [0.0f64; N_SECTORS];
    let (mut corp, mut own, mut own_wage, mut va, mut wb, mut dep, mut int, mut tax) =
        (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for (i, f) in w.firms.iter().enumerate().filter(|(_, f)| f.alive) {
        sector_profit[f.sector.idx()] += f.profit;
        match f.owner {
            Some(h) => {
                own += f.profit;
                if w.households.get(h as usize).is_some_and(|hh| hh.employer == Some(i as FirmId)) {
                    own_wage += w.hh_month[h as usize].income.wage;
                }
            }
            None => corp += f.profit,
        }
        // Capital before this month's depreciation carried the property tax (charged in produce).
        let k0 = f.capital.max(0.0) / (1.0 - w.cfg.depreciation_annual / 12.0);
        va += f.revenue - f.input_cost + k0 * pk * ptax_m;
        wb += f.wage_bill;
        dep += f.depreciation_cost;
        int += f.interest_paid;
        tax += f.taxes;
    }
    let bank_profit: f64 = w.banks.iter().filter(|b| b.alive).map(|b| b.profit).sum();
    let debt: f64 = w.loans.iter().filter(|(_, l)| l.kind == LoanKind::Business).map(|(_, l)| l.principal.max(0.0)).sum();
    o.corp_profits_bn_ar = corp * ann;
    o.owner_firm_profits_bn_ar = own * ann;
    o.owner_own_wage_bn_ar = own_wage * ann;
    o.bank_net_income_bn_ar = (bank_profit - w.fiscal.bank_tax_month) * ann;
    // Re-fit Step 4c: the upstream taxes and royalties are value added (income of government and households), not
    // intermediate inputs, so they are added back to the firm value added they reduced in `input_cost`.
    o.firm_va_bn_ar = (va + w.flows.upstream_tax + w.flows.upstream_royalty) * ann;
    o.upstream_tax_bn_ar = w.flows.upstream_tax * ann;
    o.upstream_royalty_bn_ar = w.flows.upstream_royalty * ann;
    o.firm_wage_bill_bn_ar = wb * ann;
    o.firm_depreciation_bn_ar = dep * ann;
    o.firm_interest_bn_ar = int * ann;
    o.firm_corp_tax_bn_ar = tax * ann;
    o.firm_debt_bn = debt * stock;
    o.firm_profits_by_sector_bn_ar = sector_profit.map(|x| x * ann);

    // ---- annual observations (January; carried forward) ----
    // The investment window closes at the January refresh: it holds the 12 months before this record.
    if w.tick % 12 == 0 {
        update_annual(w, &mut st.annual);
        update_investment(w, st);
    }
    // Monthly accumulation of investment orders per firm slot (after the refresh, so each window has 12 months).
    accumulate_investment(st, &w.firms);
    st.inv_months += 1;

    let a = &st.annual;
    o.job_creation_by_age = a.job_creation_by_age;
    o.job_destruction_by_age = a.job_destruction_by_age;
    o.job_creation_entrants_pct = a.job_creation_entrants_pct;
    let nan_if = |ok: bool, x: f64| if ok { x } else { f64::NAN };
    o.stayer_wage_change_p10 = nan_if(a.stayer_valid, a.stayer_p10);
    o.stayer_wage_change_p50 = nan_if(a.stayer_valid, a.stayer_p50);
    o.stayer_wage_change_p90 = nan_if(a.stayer_valid, a.stayer_p90);
    o.stayer_wage_change_kelley = nan_if(a.stayer_valid, a.stayer_kelley);
    o.wage_log_var_total = a.pay_var_total;
    o.wage_log_var_between_firm = a.pay_var_between;
    o.wage_log_var_within_firm = a.pay_var_within;
    o.wage_between_firm_share = a.pay_between_share;
    o.investment_spike_share = nan_if(a.inv_valid, a.inv_spike_share);
    o.investment_inaction_share = nan_if(a.inv_valid, a.inv_inaction_share);
}

/// Adds this month's investment orders to each firm slot's window sum. New slots are zero-filled and existing sums
/// are kept, so a firm born mid-window does not reset the others. Dead slots are cleared.
fn accumulate_investment(st: &mut MetricsState, firms: &[Firm]) {
    st.inv_acc.resize(firms.len(), 0.0);
    st.inv_slot_months.resize(firms.len(), 0);
    for ((acc, n), f) in st.inv_acc.iter_mut().zip(st.inv_slot_months.iter_mut()).zip(firms.iter()) {
        if f.alive {
            *acc += f.investment_order;
            *n += 1;
        } else {
            *acc = 0.0;
            *n = 0;
        }
    }
}

/// Annual investment rates: each firm's investment over the 12 months before this January (sum of monthly orders)
/// over its capital at the start of that window (the capital recorded at the previous January, `inv_k0`). Shares of
/// alive firms above 20% (spikes) and below 1% in absolute value (inaction), over the firms that have a full 12-month
/// window (firms born inside the window are left out). Valid only after 12 accumulated months; resets the window
/// either way, and records each alive firm's capital as the start-of-window capital for the next one.
fn update_investment(w: &World, st: &mut MetricsState) {
    let valid = st.inv_months >= 12;
    let (mut n, mut spike, mut inact) = (0usize, 0usize, 0usize);
    st.inv_k0.resize(w.firms.len(), 0.0);
    if valid {
        for (((f, &acc), &months), &k0) in
            w.firms.iter().zip(st.inv_acc.iter()).zip(st.inv_slot_months.iter()).zip(st.inv_k0.iter())
        {
            if f.alive && k0 > 0.0 && months >= 12 {
                n += 1;
                let r = acc / k0;
                if r > 0.20 {
                    spike += 1;
                }
                if r.abs() < 0.01 {
                    inact += 1;
                }
            }
        }
    }
    st.annual.inv_valid = valid;
    st.annual.inv_spike_share = 100.0 * div(spike as f64, n as f64);
    st.annual.inv_inaction_share = 100.0 * div(inact as f64, n as f64);
    for (k0, f) in st.inv_k0.iter_mut().zip(w.firms.iter()) {
        *k0 = if f.alive { f.capital } else { 0.0 };
    }
    st.inv_acc.iter_mut().for_each(|x| *x = 0.0);
    st.inv_slot_months.iter_mut().for_each(|x| *x = 0);
    st.inv_months = 0;
}

// ---------------------------------------------------------------------------------------------
// snapshot
// ---------------------------------------------------------------------------------------------

fn household_view(w: &World, h: &Household, m: &HhMonth, biz_eq: &[f64]) -> HouseholdView {
    let (home_value, mortgage_balance, mortgage_rate) = match (h.tenure, h.home) {
        (Tenure::Owner, Some(hm)) => {
            let rate = w.houses[hm as usize]
                .mortgage
                .and_then(|l| w.loans.get(l))
                .map(|l| 100.0 * l.annual_rate)
                .unwrap_or(0.0);
            (
                common::house_value(w, hm),
                common::mortgage_balance(w, hm),
                rate,
            )
        }
        _ => (0.0, 0.0, 0.0),
    };
    HouseholdView {
        id: h.id,
        age: h.age_years(),
        education: format!("{:?}", h.education),
        status: format!("{:?}", h.status),
        tenure: format!("{:?}", h.tenure),
        wage_annual: h.wage * 12.0,
        income_annual: m.income.total() * 12.0,
        deposits: h.deposits,
        fund_value: h.shares * w.fund.share_price,
        home_value,
        mortgage_balance,
        mortgage_rate,
        rent: h.rent,
        net_worth: common::net_worth(w, h.id, biz_eq),
        consumption_monthly: m.consumption.iter().sum(),
        employer_sector: h
            .employer
            .map(|f| w.firms[f as usize].sector.name().to_string())
            .unwrap_or_default(),
    }
}

/// Firm size histogram upper bounds (persons = employees x 1.75, inclusive; the same unit as the size classes).
const FIRM_SIZE_BOUNDS: [u32; 10] = [0, 4, 9, 19, 49, 99, 249, 499, 999, u32::MAX];

/// Build a distribution snapshot of the current state (O(n log n); call on demand, not every month).
pub fn snapshot(w: &World) -> Snapshot {
    let scale = w.cfg.effective_scale();
    let biz_eq = common::owner_equity(w);
    let mut s = Snapshot {
        tick: w.tick,
        ..Default::default()
    };
    s.age_buckets = AGE_BUCKETS
        .iter()
        .map(|&(lo, hi)| if hi >= 200 { format!("{lo}+") } else { format!("{lo}-{hi}") })
        .collect();

    // mortgage rates by borrower age bucket
    let mut mr_sum = [0.0f64; 6];
    let mut mr_n = [0usize; 6];
    for (_, l) in w.loans.iter() {
        if let (LoanKind::Mortgage { .. }, Borrower::Household(hid)) = (l.kind, l.borrower) {
            if let Some(b) = bucket_of(u32::from(w.households[hid as usize].age_months / 12)) {
                mr_sum[b] += l.annual_rate;
                mr_n[b] += 1;
            }
        }
    }

    for b in 0..6 {
        let members: Vec<(&Household, &HhMonth)> = w
            .households
            .iter()
            .zip(w.hh_month.iter())
            .filter(|(h, _)| h.alive && bucket_of(u32::from(h.age_months / 12)) == Some(b))
            .collect();
        let n = members.len() as f64;
        let mut inc_sum = 0.0f64;
        let mut nw: Vec<f64> = Vec::with_capacity(members.len());
        let mut employed = 0usize;
        let mut owners = 0usize;
        let mut base = 0usize;
        for &(h, m) in &members {
            inc_sum += m.income.total() * 12.0;
            nw.push(common::net_worth(w, h.id, &biz_eq));
            if h.status == LaborStatus::Employed {
                employed += 1;
            }
            if h.tenure == Tenure::Owner {
                owners += 1;
            }
            if in_tenure_base(h) {
                base += 1;
            }
        }
        let mean_nw = div(nw.iter().sum::<f64>(), n);
        nw.sort_unstable_by(|a, b| a.total_cmp(b));
        s.by_age_mean_income.push(div(inc_sum, n));
        s.by_age_mean_net_worth.push(mean_nw);
        s.by_age_median_net_worth.push(median_sorted(&nw));
        s.by_age_ownership.push(100.0 * div(owners as f64, base as f64));
        s.by_age_employment_rate.push(100.0 * div(employed as f64, n));
        s.by_age_mortgage_rate.push(100.0 * div(mr_sum[b], mr_n[b] as f64));
    }

    // deciles over alive households
    let alive_ids: Vec<usize> = (0..w.households.len()).filter(|&i| w.households[i].alive).collect();
    let mut nw_all: Vec<f64> = alive_ids
        .iter()
        .map(|&i| common::net_worth(w, i as HhId, &biz_eq))
        .collect();
    nw_all.sort_unstable_by(|a, b| a.total_cmp(b));
    s.net_worth_deciles = deciles(&nw_all);
    let mut inc_all: Vec<f64> = alive_ids
        .iter()
        .map(|&i| w.hh_month[i].income.total() * 12.0)
        .collect();
    inc_all.sort_unstable_by(|a, b| a.total_cmp(b));
    s.income_deciles = deciles(&inc_all);

    // firms
    let mut size_counts = [0u32; 10];
    for f in w.firms.iter().filter(|f| f.alive) {
        // Size in persons, the unit of the observation size classes.
        let size = firm_persons(f.employees());
        let i = FIRM_SIZE_BOUNDS
            .iter()
            .position(|&b| size <= b as f64)
            .unwrap_or(FIRM_SIZE_BOUNDS.len() - 1);
        size_counts[i] += 1;
    }
    s.firm_size_hist = FIRM_SIZE_BOUNDS.iter().zip(size_counts).map(|(&b, c)| (b, c)).collect();

    let mut sec_emp = [0.0f64; N_SECTORS];
    for h in w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed) {
        if let Some(f) = h.employer {
            sec_emp[w.firms[f as usize].sector.idx()] += 1.0;
        }
    }
    s.sector_employment = sec_emp.iter().map(|x| x * scale / 1e6).collect();
    s.sector_prices = w.prices.sector.to_vec();

    let mut mk = [0.0f64; N_SECTORS];
    let mut mkn = [0usize; N_SECTORS];
    for f in w.firms.iter().filter(|f| f.alive) {
        mk[f.sector.idx()] += f.markup;
        mkn[f.sector.idx()] += 1;
    }
    s.sector_markups = (0..N_SECTORS).map(|i| 100.0 * div(mk[i], mkn[i] as f64)).collect();

    s.yield_curve = TENORS
        .iter()
        .zip(w.curve.yields)
        .map(|(&t, y)| (t, 100.0 * y))
        .collect();

    let mut house_vals: Vec<f64> = w
        .houses
        .iter()
        .filter(|h| h.completed)
        .map(|h| common::house_value(w, h.id))
        .collect();
    house_vals.sort_unstable_by(|a, b| a.total_cmp(b));
    s.house_price_deciles = deciles(&house_vals);

    // evenly spaced sample of alive households for the agent explorer
    let take = alive_ids.len().min(200);
    s.sample_households = (0..take)
        .map(|k| {
            let i = alive_ids[k * alive_ids.len() / take];
            household_view(w, &w.households[i], &w.hh_month[i], &biz_eq)
        })
        .collect();

    s
}

// ---- b5-firmhet: firm-distribution observations (S0). Read-only; nothing here feeds back into the model. ----

/// One alive firm's inputs to the firm-distribution statistics (`firm_dist`).
#[derive(Clone, Copy, Debug)]
pub struct FirmDistRow {
    /// Sector index (0..N_SECTORS).
    pub sector: usize,
    /// Workers on the books.
    pub heads: f64,
    /// Log labour productivity; NaN when productivity is not positive.
    pub lnlp: f64,
    /// Revenue this month (nominal).
    pub revenue: f64,
    /// Markup over variable cost, percent; NaN when the variable unit cost is not positive.
    pub markup_avc: f64,
    /// ln(K / k_star_smooth); NaN when either is not positive.
    pub capital_gap: f64,
    /// Real output this month and real capacity (capital × capital productivity).
    pub output: f64,
    pub capacity: f64,
}

/// The firm-distribution statistics of one month (percentages where named).
#[derive(Clone, Debug)]
pub struct FirmDist {
    pub n_by_sector: [f64; N_SECTORS],
    pub hhi_by_sector: [f64; N_SECTORS],
    pub top1: f64,
    pub top034: f64,
    pub lp_sd: f64,
    pub lp_p90_p10: f64,
    pub op_gap: f64,
    pub size_slope: f64,
    pub markup_p: [f64; 3],
    pub gap_p: [f64; 3],
    pub gap_sd: f64,
    pub gap_median_by_sector: [f64; N_SECTORS],
    pub cu_by_sector: [f64; N_SECTORS],
}

/// Linear-interpolated quantile of an ascending-sorted slice; NaN for an empty slice.
fn quantile_interp(xs: &[f64], q: f64) -> f64 {
    if xs.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (xs.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    xs[lo] + (xs[hi] - xs[lo]) * (pos - lo as f64)
}

/// Percent of the total held by the largest `frac` of the values (at least one value); 0 when the total is not
/// positive. The result lies in [0, 100].
fn top_frac_share(xs: &[f64], frac: f64) -> f64 {
    let total: f64 = xs.iter().sum();
    if xs.is_empty() || total <= 0.0 {
        return 0.0;
    }
    let mut desc = xs.to_vec();
    desc.sort_by(|a, b| b.total_cmp(a));
    let k = ((frac * desc.len() as f64).ceil() as usize).clamp(1, desc.len());
    (100.0 * desc[..k].iter().sum::<f64>() / total).clamp(0.0, 100.0)
}

/// Firm-distribution statistics from the alive firms' rows. Pure, so the tests can build rows directly.
pub fn firm_dist(rows: &[FirmDistRow]) -> FirmDist {
    let mut d = FirmDist {
        n_by_sector: [0.0; N_SECTORS],
        hhi_by_sector: [0.0; N_SECTORS],
        top1: 0.0,
        top034: 0.0,
        lp_sd: f64::NAN,
        lp_p90_p10: f64::NAN,
        op_gap: f64::NAN,
        size_slope: f64::NAN,
        markup_p: [f64::NAN; 3],
        gap_p: [f64::NAN; 3],
        gap_sd: f64::NAN,
        gap_median_by_sector: [0.0; N_SECTORS],
        cu_by_sector: [0.0; N_SECTORS],
    };
    let mut rev_tot = [0.0f64; N_SECTORS];
    let mut rev_sq = [0.0f64; N_SECTORS];
    let mut out_s = [0.0f64; N_SECTORS];
    let mut cap_s = [0.0f64; N_SECTORS];
    let mut lp_n = [0usize; N_SECTORS];
    let mut lp_sum = [0.0f64; N_SECTORS];
    let mut hd_tot = [0.0f64; N_SECTORS];
    let mut hd_lp = [0.0f64; N_SECTORS];
    for r in rows {
        let s = r.sector;
        d.n_by_sector[s] += 1.0;
        rev_tot[s] += r.revenue;
        rev_sq[s] += r.revenue * r.revenue;
        out_s[s] += r.output;
        cap_s[s] += r.capacity;
        if r.lnlp.is_finite() {
            lp_n[s] += 1;
            lp_sum[s] += r.lnlp;
            hd_tot[s] += r.heads;
            hd_lp[s] += r.heads * r.lnlp;
        }
    }
    for s in 0..N_SECTORS {
        if rev_tot[s] > 0.0 {
            d.hhi_by_sector[s] = rev_sq[s] / (rev_tot[s] * rev_tot[s]);
        }
        if cap_s[s] > 0.0 {
            d.cu_by_sector[s] = 100.0 * out_s[s] / cap_s[s];
        }
    }
    // Employment concentration over all alive firms.
    let heads: Vec<f64> = rows.iter().map(|r| r.heads).collect();
    d.top1 = top_frac_share(&heads, 0.01);
    d.top034 = top_frac_share(&heads, 0.0034);

    // Labour productivity: sector means of the log (unweighted), then pooled deviations.
    let mean_lp: [f64; N_SECTORS] = std::array::from_fn(|s| if lp_n[s] > 0 { lp_sum[s] / lp_n[s] as f64 } else { 0.0 });
    let mut dev: Vec<f64> = Vec::new();
    let mut xy: Vec<(f64, f64)> = Vec::new();
    for r in rows.iter().filter(|r| r.lnlp.is_finite()) {
        let x = r.lnlp - mean_lp[r.sector];
        dev.push(x);
        if r.heads > 0.0 {
            xy.push((x, r.heads.ln()));
        }
    }
    if !dev.is_empty() {
        let n = dev.len() as f64;
        d.lp_sd = (dev.iter().map(|x| x * x).sum::<f64>() / n).sqrt();
        dev.sort_by(|a, b| a.total_cmp(b));
        d.lp_p90_p10 = quantile_interp(&dev, 0.9) - quantile_interp(&dev, 0.1);
    }
    // Olley-Pakes gap, employment-weighted across sectors.
    let (mut gw, mut gs) = (0.0f64, 0.0f64);
    for s in 0..N_SECTORS {
        if hd_tot[s] > 0.0 && lp_n[s] > 0 {
            let gap = hd_lp[s] / hd_tot[s] - lp_sum[s] / lp_n[s] as f64;
            gw += hd_tot[s];
            gs += hd_tot[s] * gap;
        }
    }
    if gw > 0.0 {
        d.op_gap = gs / gw;
    }
    // OLS slope of ln heads on relative ln productivity.
    if xy.len() >= 2 {
        let n = xy.len() as f64;
        let mx = xy.iter().map(|p| p.0).sum::<f64>() / n;
        let my = xy.iter().map(|p| p.1).sum::<f64>() / n;
        let sxx: f64 = xy.iter().map(|p| (p.0 - mx) * (p.0 - mx)).sum();
        let sxy: f64 = xy.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        if sxx > 0.0 {
            d.size_slope = sxy / sxx;
        }
    }

    // Markup over variable cost.
    let mut mk: Vec<f64> = rows.iter().map(|r| r.markup_avc).filter(|x| x.is_finite()).collect();
    mk.sort_by(|a, b| a.total_cmp(b));
    d.markup_p = [quantile_interp(&mk, 0.1), quantile_interp(&mk, 0.5), quantile_interp(&mk, 0.9)];

    // Capital gap: percentiles, pooled sd around sector means, and sector medians.
    let mut gap_all: Vec<f64> = rows.iter().map(|r| r.capital_gap).filter(|x| x.is_finite()).collect();
    gap_all.sort_by(|a, b| a.total_cmp(b));
    d.gap_p = [quantile_interp(&gap_all, 0.1), quantile_interp(&gap_all, 0.5), quantile_interp(&gap_all, 0.9)];
    let mut gap_by: Vec<Vec<f64>> = vec![Vec::new(); N_SECTORS];
    for r in rows.iter().filter(|r| r.capital_gap.is_finite()) {
        gap_by[r.sector].push(r.capital_gap);
    }
    let mut ss = 0.0f64;
    let mut ng = 0usize;
    for (s, g) in gap_by.iter_mut().enumerate() {
        if g.is_empty() {
            continue;
        }
        let m = g.iter().sum::<f64>() / g.len() as f64;
        ss += g.iter().map(|x| (x - m) * (x - m)).sum::<f64>();
        ng += g.len();
        g.sort_by(|a, b| a.total_cmp(b));
        d.gap_median_by_sector[s] = quantile_interp(g, 0.5);
    }
    if ng > 0 {
        d.gap_sd = (ss / ng as f64).sqrt();
    }
    d
}

/// Fill the firm-distribution observations of the month from the alive firms and the household owner states. Read
/// only: the RNG, the ledger and the firms are untouched.
fn fill_firm_dist(w: &World, o: &mut Observations) {
    let cfg = &w.cfg;
    let p_k = role_sum(&cfg.capital_goods_mix, |s| common::sector_price(w, Sector::ALL[s]));
    let dep_m = cfg.depreciation_annual / 12.0;
    let u_n = cfg.target_utilization;
    let mut rows = Vec::with_capacity(w.firms.len());
    // Owner status: [in own firm, employed elsewhere, unemployed, not in the labour force].
    let mut own = [0.0f64; 4];
    for f in w.firms.iter().filter(|f| f.alive) {
        let cap_cost = if f.capital_productivity > 0.0 && u_n > 0.0 {
            dep_m * p_k / (f.capital_productivity * u_n)
        } else {
            f64::NAN
        };
        let vc = f.unit_cost - cap_cost;
        let markup_avc = if f.price > 0.0 && vc > 0.0 { 100.0 * (f.price / vc - 1.0) } else { f64::NAN };
        let capital_gap = if f.capital > 0.0 && f.k_star_smooth > 0.0 {
            (f.capital / f.k_star_smooth).ln()
        } else {
            f64::NAN
        };
        if let Some(hh) = f.owner.and_then(|h| w.households.get(h as usize)).filter(|h| h.alive) {
            match hh.status {
                LaborStatus::Employed if hh.employer == Some(f.id) => own[0] += 1.0,
                LaborStatus::Employed => own[1] += 1.0,
                LaborStatus::Unemployed => own[2] += 1.0,
                _ => own[3] += 1.0,
            }
        }
        rows.push(FirmDistRow {
            sector: f.sector.idx(),
            heads: f.employees() as f64,
            lnlp: if f.labor_productivity > 0.0 { f.labor_productivity.ln() } else { f64::NAN },
            revenue: f.revenue,
            markup_avc,
            capital_gap,
            output: f.output,
            capacity: f.capital * f.capital_productivity,
        });
    }
    let d = firm_dist(&rows);
    o.firms_by_sector = d.n_by_sector;
    o.firm_hhi_by_sector = d.hhi_by_sector;
    o.firm_emp_top1_share = d.top1;
    o.firm_emp_top034_share = d.top034;
    o.firm_lp_sd_within = d.lp_sd;
    o.firm_lp_p90_p10 = d.lp_p90_p10;
    o.firm_op_gap_lp = d.op_gap;
    o.firm_size_lp_slope = d.size_slope;
    o.markup_avc_p10 = d.markup_p[0];
    o.markup_avc_p50 = d.markup_p[1];
    o.markup_avc_p90 = d.markup_p[2];
    o.capital_gap_p10 = d.gap_p[0];
    o.capital_gap_p50 = d.gap_p[1];
    o.capital_gap_p90 = d.gap_p[2];
    o.capital_gap_sd_within = d.gap_sd;
    o.capital_gap_median_by_sector = d.gap_median_by_sector;
    o.cu_by_sector = d.cu_by_sector;
    o.owners_in_own_firm = own[0];
    o.owners_employed_elsewhere = own[1];
    o.owners_unemployed = own[2];
    o.owners_not_in_lf = own[3];
}

#[cfg(test)]
mod firm_dist_tests {
    use super::*;

    fn row(sector: usize, heads: f64, lnlp: f64) -> FirmDistRow {
        FirmDistRow {
            sector,
            heads,
            lnlp,
            revenue: 1.0,
            markup_avc: 10.0,
            capital_gap: 0.0,
            output: 1.0,
            capacity: 2.0,
        }
    }

    #[test]
    fn equal_productivity_gives_zero_dispersion_and_op_gap() {
        let rows: Vec<FirmDistRow> = (0..40).map(|i| row(i % 3, 1.0 + (i % 5) as f64, 0.7)).collect();
        let d = firm_dist(&rows);
        assert!(d.lp_sd.abs() < 1e-12);
        assert!(d.lp_p90_p10.abs() < 1e-12);
        assert!(d.op_gap.abs() < 1e-12);
    }

    #[test]
    fn top_shares_lie_in_zero_to_hundred() {
        let rows: Vec<FirmDistRow> = (0..300).map(|i| row(i % 9, (i % 7) as f64 * 10.0, 0.0)).collect();
        let d = firm_dist(&rows);
        for v in [d.top1, d.top034] {
            assert!((0.0..=100.0).contains(&v), "share {v} out of range");
        }
        let one = firm_dist(&[row(0, 5.0, 0.0)]);
        assert!((one.top1 - 100.0).abs() < 1e-12);
    }

    #[test]
    fn variable_cost_markup_is_at_least_the_effective_markup() {
        // price P, unit cost uc, capital cost cc: P/(uc - cc) >= P/uc whenever 0 < uc - cc <= uc.
        for (p, uc, cc) in [(1.3f64, 1.0f64, 0.2f64), (2.0, 1.5, 0.0), (1.1, 1.0, 0.5)] {
            let effective = p / uc - 1.0;
            let variable = p / (uc - cc) - 1.0;
            assert!(variable >= effective - 1e-12);
        }
    }

    #[test]
    fn olley_pakes_gap_is_positive_when_big_firms_are_more_productive() {
        let rows = vec![row(0, 1.0, 0.0), row(0, 9.0, 0.5)];
        let d = firm_dist(&rows);
        assert!(d.op_gap > 0.0);
        assert!(d.size_slope > 0.0);
    }

    #[test]
    fn hhi_of_one_firm_is_one() {
        let d = firm_dist(&[row(4, 3.0, 0.0)]);
        assert!((d.hhi_by_sector[4] - 1.0).abs() < 1e-12);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    // rz1 F1b: the pure margin logic.
    #[test]
    fn fisim_real_first_call_is_legacy_then_follows_stocks_at_the_base_margins() {
        let mut b = FisimBase::default();
        // Month 0: deposits 1000 at margin 1.0 %, loans 800 at margin 0.5 %: nominal (10 + 4) / 12. Legacy deflator 1.04, general 1.10.
        let nominal0 = (1000.0 * 0.010 + 800.0 * 0.005) / 12.0;
        let f0 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.010, 800.0, 800.0 * 0.005, nominal0, 1.04, 1.10);
        assert_eq!(f0.to_bits(), (nominal0 / 1.04).to_bits(), "month 0 is the legacy value, bit for bit");
        assert!(b.captured);
        assert_eq!((b.dep, b.loan), (Some(0.010), Some(0.005)));
        assert!((b.anchor - 1.10 / 1.04).abs() < 1e-15);
        // Later: margins moved (deposit 2.0 %, loan 0.1 %), stocks +10 %, general deflator 1.10 -> 1.21 (+10 %): the deflated stocks at
        // the base margins are unchanged, so the real flow equals its month-0 value whatever the current margins and legacy price.
        let nominal1 = (1100.0 * 0.020 + 880.0 * 0.001) / 12.0;
        let f1 = fisim_real_at_base(&mut b, 1100.0, 1100.0 * 0.020, 880.0, 880.0 * 0.001, nominal1, 1.05, 1.21);
        let want = nominal0 / 1.04;
        assert!((f1 - want).abs() < 1e-12, "{f1} vs {want}");
        assert!((f1 - nominal1 / 1.05).abs() > 1e-6, "differs from the current-margin value");
        // Same stocks and deflator, different margins: same flow (margins never enter after the base).
        let f2 = fisim_real_at_base(&mut b, 1100.0, 1100.0 * 0.050, 880.0, 880.0 * 0.0, 1.0, 1.05, 1.21);
        assert_eq!(f2.to_bits(), f1.to_bits());
    }

    #[test]
    fn fisim_real_uses_the_general_deflator_not_the_sector_price() {
        let mut b = FisimBase::default();
        let n0 = (1000.0 * 0.01 + 800.0 * 0.005) / 12.0;
        fisim_real_at_base(&mut b, 1000.0, 10.0, 800.0, 4.0, n0, 1.0, 1.0);
        // Same stocks and margins later; the OtherServices price (legacy deflator) doubles, the general index does not move.
        let f = fisim_real_at_base(&mut b, 1000.0, 10.0, 800.0, 4.0, n0, 2.0, 1.0);
        assert!((f - n0).abs() < 1e-15, "{f} vs {n0}");
        // The general index doubles: the real flow halves.
        let g = fisim_real_at_base(&mut b, 1000.0, 10.0, 800.0, 4.0, n0, 1.0, 2.0);
        assert!((g - n0 / 2.0).abs() < 1e-15, "{g} vs {}", n0 / 2.0);
    }

    // RZ-R8 correction 2: a leg with a zero margin at month 0 is not dropped for the run.
    #[test]
    fn fisim_real_keeps_a_leg_whose_margin_is_zero_at_month_0() {
        let mut b = FisimBase::default();
        // Month 0: the deposit margin is zero (deposit rate at the reference rate), the loan margin 0.5 %.
        let n0 = (800.0 * 0.005) / 12.0;
        let f0 = fisim_real_at_base(&mut b, 1000.0, 0.0, 800.0, 4.0, n0, 1.0, 1.0);
        assert_eq!(f0.to_bits(), n0.to_bits());
        assert_eq!((b.dep, b.loan), (None, Some(0.005)));
        // Still zero: the deposit leg adds nothing (its nominal is zero too); the loan leg follows its base margin.
        let f1 = fisim_real_at_base(&mut b, 1000.0, 0.0, 800.0, 800.0 * 0.020, 1.0, 1.0, 1.0);
        assert!((f1 - n0).abs() < 1e-15);
        // The deposit margin turns positive (0.8 %): the leg latches that margin and joins the flow, now and later.
        let f2 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.008, 800.0, 800.0 * 0.020, 1.0, 1.0, 1.0);
        assert!((f2 - (8.0 + 4.0) / 12.0).abs() < 1e-12, "{f2}");
        assert_eq!(b.dep, Some(0.008));
        let f3 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.030, 800.0, 0.0, 1.0, 1.0, 1.0);
        assert!((f3 - (8.0 + 4.0) / 12.0).abs() < 1e-12, "{f3}");
        // No stocks and no margin at all: zero, not NaN, and no latch.
        let mut z = FisimBase::default();
        assert_eq!(fisim_real_at_base(&mut z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0), 0.0);
        assert_eq!((z.dep, z.loan), (None, None));
        assert_eq!(fisim_real_at_base(&mut z, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0), 0.0);
    }

    // RZ-R8 correction 2, hardened: a margin of a few basis points at month 0 (a deposit leg at the policy floor) is not a base either.
    #[test]
    fn fisim_real_does_not_latch_a_negligible_margin() {
        let mut b = FisimBase::default();
        // Month 0: the deposit margin is 0.5 bp, the loan margin 0.5 %.
        let n0 = (1000.0 * 0.00005 + 800.0 * 0.005) / 12.0;
        let f0 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.00005, 800.0, 4.0, n0, 1.0, 1.0);
        assert_eq!(f0.to_bits(), n0.to_bits());
        assert_eq!((b.dep, b.loan), (None, Some(0.005)), "0.5 bp is below the 10 bp threshold: no deposit base yet");
        // The deposit margin is 9 bp: still below; the leg follows its current margin.
        let f1 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.0009, 800.0, 4.0, 1.0, 1.0, 1.0);
        assert!((f1 - (0.9 + 4.0) / 12.0).abs() < 1e-12, "{f1}");
        assert_eq!(b.dep, None);
        // The deposit margin reaches 10 bp: it latches there, and later margins no longer matter.
        let f2 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.001, 800.0, 4.0, 1.0, 1.0, 1.0);
        assert!((f2 - (1.0 + 4.0) / 12.0).abs() < 1e-12, "{f2}");
        assert_eq!(b.dep, Some(0.001));
        let f3 = fisim_real_at_base(&mut b, 1000.0, 1000.0 * 0.02, 800.0, 4.0, 1.0, 1.0, 1.0);
        assert!((f3 - (1.0 + 4.0) / 12.0).abs() < 1e-12, "{f3}");
    }

    #[test]
    fn fisim_real_falls_back_to_the_legacy_deflator_when_the_general_index_is_unusable() {
        let mut b = FisimBase::default();
        let n0 = (1000.0 * 0.01) / 12.0;
        fisim_real_at_base(&mut b, 1000.0, 10.0, 0.0, 0.0, n0, 1.25, f64::NAN);
        assert_eq!(b.anchor, 1.0);
        let f = fisim_real_at_base(&mut b, 2000.0, 20.0, 0.0, 0.0, 1.0, 1.25, 0.0);
        assert!((f - 2.0 * n0).abs() < 1e-15, "{f}");
    }

    // rz1 F1b: the key changes the recorded real consumption and GDP and nothing else, and in the direction the margins imply.
    // The reported window starts after a 24-month warm-up (as `compare` does: the base is reset there), so the base is the margins
    // of reported month 0 and month 0 is identical with the key on and off.
    #[test]
    fn fisim_constant_margin_changes_only_the_real_consumption_and_gdp_series() {
        // squeeze = from month 2 on the deposit rate follows the policy rate almost one for one, which shrinks the deposit margin
        // (r_ref - deposit rate). The economy is the same with the key on and off; only the recorded real series can differ.
        let run_with = |on: bool, squeeze: bool| {
            // As `compare` does: build and warm up with the key off, then start the reported window with the key as asked.
            let cfg = Config { n_households: 1000, fisim_constant_margin: false, ..Default::default() };
            let mut w = tiny_world(cfg, 5);
            crate::sim::warm_up(&mut w, 24);
            crate::sim::begin_reported_window(&mut w, on);
            let h0 = w.history.len();
            crate::sim::run(&mut w, 2);
            if squeeze {
                w.cfg.deposit_beta = 0.95;
            }
            crate::sim::run(&mut w, 8);
            w.history[h0..].iter().map(|m| serde_json::to_value(m).unwrap()).collect::<Vec<_>>()
        };
        let (off, on) = (run_with(false, true), run_with(true, true));
        assert_eq!(off.len(), 10);
        assert_eq!(off.len(), on.len());
        let mut differing = std::collections::BTreeSet::new();
        for (t, (a, b)) in off.iter().zip(on.iter()).enumerate() {
            for (k, va) in a.as_object().unwrap() {
                if va != &b[k] {
                    differing.insert(k.clone());
                    assert!(t >= 1, "reported month 0 must be identical (the base is captured there), key {k}");
                }
            }
        }
        let allowed: std::collections::BTreeSet<String> =
            ["consumption_real_bn_ar", "gdp_real_bn_ar", "gdp_real_growth_yoy", "real_consumption_per_hh"].iter().map(|s| s.to_string()).collect();
        assert!(differing.is_subset(&allowed), "unexpected fields differ: {:?}", differing.difference(&allowed).collect::<Vec<_>>());
        assert!(differing.contains("gdp_real_bn_ar"), "the key must change something");
        // Direction, by difference in differences (the squeeze also moves the economy, identically in both arms): with the key OFF
        // the squeeze cuts recorded real consumption through the deposit margin, with the key ON it does not. A swapped branch or a
        // wrong sign in the flow fails here.
        let (off0, on0) = (run_with(false, false), run_with(true, false));
        let cons = |h: &Vec<serde_json::Value>, t: usize| h[t]["consumption_real_bn_ar"].as_f64().unwrap();
        let dd: f64 = (3..10).map(|t| (cons(&on, t) - cons(&on0, t)) - (cons(&off, t) - cons(&off0, t))).sum();
        assert!(dd > 0.0, "the squeeze must cut current-margin real consumption more than constant-margin: {dd}");
    }

    /// b5-udur: duration statistics on a hand-built set of households.
    #[test]
    fn duration_stats_match_hand_values() {
        let mut w = tiny_world(Config { n_households: 40, ..Default::default() }, 2);
        for h in w.households.iter_mut() {
            h.alive = true;
            h.status = LaborStatus::Employed;
        }
        // No unemployed: every statistic is 0.
        assert_eq!(duration_stats(&w.households), DurationStats::default());
        // Four unemployed at months 0, 2, 6, 10: mean (4.5 + 0.5)*4.345 weeks, LTU 50%.
        for (h, m) in w.households.iter_mut().zip([0u16, 2, 6, 10]) {
            h.status = LaborStatus::Unemployed;
            h.months_unemployed = m;
        }
        let d = duration_stats(&w.households);
        assert!((d.mean_weeks - 5.0 * 4.345).abs() < 1e-9, "mean {}", d.mean_weeks);
        assert!((d.ltu_share - 50.0).abs() < 1e-9);
        // Grouped median of {0,2,6,10} is the bin [2,3) at position 2 of 4 halves: 2 + (2-1)/1 = 3 months.
        assert!((d.median_weeks - 3.0 * 4.345).abs() < 1e-9, "median {}", d.median_weeks);
    }

    fn all_finite(v: &serde_json::Value) -> bool {
        match v {
            serde_json::Value::Number(n) => n.as_f64().map_or(false, |x| x.is_finite()),
            serde_json::Value::Array(a) => a.iter().all(all_finite),
            serde_json::Value::Object(o) => o.values().all(all_finite),
            _ => true,
        }
    }

    #[test]
    fn record_is_finite_and_unemployment_matches_manual_count() {
        let mut w = tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            1,
        );
        record(&mut w);
        assert_eq!(w.history.len(), 1);
        let m = &w.history[0];
        assert!(all_finite(&serde_json::to_value(m).unwrap()), "non-finite metric");

        let (mut emp, mut unemp) = (0usize, 0usize);
        for h in w.households.iter().filter(|h| h.alive) {
            match h.status {
                LaborStatus::Employed => emp += 1,
                LaborStatus::Unemployed => unemp += 1,
                _ => {}
            }
        }
        let manual = 100.0 * unemp as f64 / (emp + unemp) as f64;
        assert!((m.unemployment_rate - manual).abs() < 1e-9, "{} vs {}", m.unemployment_rate, manual);
        assert!(m.gdp_nominal_bn_ar >= 0.0);
        assert!((m.real_wage_index - 100.0).abs() < 1e-9);
    }

    #[test]
    fn overdraft_share_matches_direct_count() {
        let mut w = tiny_world(Config { n_households: 500, ..Default::default() }, 1);
        // Overdraw every seventh alive household, and leave a dead one overdrawn (it must not count).
        let mut k = 0usize;
        for h in w.households.iter_mut() {
            if h.alive {
                if k % 7 == 0 {
                    h.deposits = -1.0;
                }
                k += 1;
            }
        }
        if let Some(h) = w.households.iter_mut().find(|h| !h.alive) {
            h.deposits = -5.0;
        }
        let direct_n = w.households.iter().filter(|h| h.alive && h.deposits < 0.0).count();
        let n_alive = w.households.iter().filter(|h| h.alive).count();
        record(&mut w);
        let m = &w.history[0];
        let want = 100.0 * direct_n as f64 / n_alive as f64;
        assert!((m.obs.overdraft_share - want).abs() < 1e-9, "{} vs {}", m.obs.overdraft_share, want);
        let want_bn = {
            let total: f64 = w.households.iter().filter(|h| h.alive && h.deposits < 0.0).map(|h| -h.deposits).sum();
            total * w.cfg.effective_scale() / 1e9
        };
        assert!((m.obs.overdraft_balance_bn - want_bn).abs() <= 1e-9 * (1.0 + want_bn.abs()));
    }

    #[test]
    fn stepping_keeps_metrics_finite() {
        let mut w = tiny_world(
            Config {
                n_households: 300,
                ..Default::default()
            },
            2,
        );
        crate::sim::run(&mut w, 14);
        assert_eq!(w.history.len(), 14);
        for m in &w.history {
            assert!(all_finite(&serde_json::to_value(m).unwrap()));
        }
        // distributional metrics refreshed at month 12 must be carried forward at month 13
        assert_eq!(w.history[13].wealth_gini, w.history[12].wealth_gini);
    }

    #[test]
    fn seed_mean_keeps_tick_and_averages_floats() {
        let a = Metrics {
            tick: 5,
            oil_price: 60.0,
            ..Default::default()
        };
        let b = Metrics {
            tick: 5,
            oil_price: 80.0,
            ..Default::default()
        };
        let m = Metrics::mean([&a, &b].into_iter(), 2.0);
        assert_eq!(m.tick, 5);
        assert!((m.oil_price - 70.0).abs() < 1e-12);
    }

    #[test]
    fn seed_mean_with_null_observations_round_trips() {
        // Stayer fields are NaN (null) before their first valid January; averaging must not fail on them.
        let a = Metrics { tick: 3, obs: Observations { stayer_wage_change_p50: f64::NAN, ..Default::default() }, ..Default::default() };
        let b = a.clone();
        let m = Metrics::mean([&a, &b].into_iter(), 2.0);
        assert!(m.obs.stayer_wage_change_p50.is_nan());
        let v = serde_json::to_value(&m).unwrap();
        assert!(v["stayer_wage_change_p50"].is_null());
        let back: Metrics = serde_json::from_value(v).unwrap();
        assert!(back.obs.stayer_wage_change_p50.is_nan());
        // a valid value in one seed and null in the other is undefined
        let c = Metrics { tick: 3, obs: Observations { stayer_wage_change_p50: 2.0, ..Default::default() }, ..Default::default() };
        let m = Metrics::mean([&c, &a].into_iter(), 2.0);
        assert!(m.obs.stayer_wage_change_p50.is_nan());
    }

    /// J4 (R-19): the top-level `r_star_est` and the observation copy used to share one JSON key (the flattened `Observations`
    /// field was written as a second `r_star_est`), so a seed mean returned the second copy as 0.0. The copy is `obs_r_star_est`
    /// now: each key appears once and both survive the seed mean.
    #[test]
    fn r_star_est_has_one_key_and_survives_the_seed_mean() {
        let a = Metrics { tick: 2, r_star_est: 0.75, obs: Observations { obs_r_star_est: 0.75, ..Default::default() }, ..Default::default() };
        let b = Metrics { tick: 2, r_star_est: 1.25, obs: Observations { obs_r_star_est: 1.25, ..Default::default() }, ..Default::default() };
        let text = serde_json::to_string(&a).unwrap();
        assert_eq!(text.matches("\"r_star_est\":").count(), 1);
        assert_eq!(text.matches("\"obs_r_star_est\":").count(), 1);
        let m = Metrics::mean([&a, &b].into_iter(), 2.0);
        assert!((m.r_star_est - 1.0).abs() < 1e-12);
        assert!((m.obs.obs_r_star_est - 1.0).abs() < 1e-12);
        let again = serde_json::to_string(&m).unwrap();
        assert_eq!(again.matches("\"r_star_est\":").count(), 1);
    }

    #[test]
    fn investment_window_keeps_sums_when_firms_are_added() {
        let mut w = tiny_world(Config::default(), 1);
        let mut st = MetricsState::default();
        for f in w.firms.iter_mut() {
            f.alive = true;
            f.investment_order = 1.0;
        }
        accumulate_investment(&mut st, &w.firms);
        let n0 = w.firms.len();
        let mut born = w.firms[0].clone();
        born.investment_order = 5.0;
        w.firms.push(born);
        accumulate_investment(&mut st, &w.firms);
        // existing slots keep their first-month sum plus the second month; the new slot holds only its own month
        assert!(st.inv_acc[..n0].iter().all(|&x| (x - 2.0).abs() < 1e-12));
        assert!((st.inv_acc[n0] - 5.0).abs() < 1e-12);
        assert_eq!(st.inv_slot_months[0], 2);
        assert_eq!(st.inv_slot_months[n0], 1);
    }

    #[test]
    fn radix_sort_matches_total_cmp_sort() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut v: Vec<f64> = (0..5000)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                match i % 7 {
                    0 => 0.0,
                    1 => -0.0,
                    2 => (x % 1000) as f64 - 500.0,
                    3 => f64::INFINITY,
                    _ => ((x >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 1e6,
                }
            })
            .collect();
        let mut expect = v.clone();
        expect.sort_unstable_by(|a, b| a.total_cmp(b));
        radix_sort_total(&mut v);
        assert!(v.iter().zip(&expect).all(|(a, b)| a.to_bits() == b.to_bits()));
    }

    /// Large inputs with heavy ties (zeros, repeated values) match the sequential definitions exactly.
    #[test]
    fn parallel_helpers_match_sequential_definitions() {
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let n = 40_001usize;
        let rows: Vec<(f64, f64, f64)> = (0..n)
            .map(|_| {
                let inc = match next() % 5 {
                    0 => 0.0,
                    1 => (next() % 7) as f64,
                    _ => (next() % 100_000) as f64 / 3.0,
                };
                let cons = (next() % 1000) as f64 / 7.0;
                (inc, cons, (next() % 50) as f64 - 10.0)
            })
            .collect();
        // median
        let vals: Vec<f64> = rows.iter().map(|r| r.0).collect();
        let mut sorted = vals.clone();
        sorted.sort_unstable_by(|a, b| a.total_cmp(b));
        assert_eq!(median_mut(&mut vals.clone()).to_bits(), median_sorted(&sorted).to_bits());
        // sort
        let mut p = vals.clone();
        par_sort_total(&mut p);
        assert!(p.iter().zip(&sorted).all(|(a, b)| a.to_bits() == b.to_bits()));
        // quintiles: rank by (income, household order), quintile q = ranks [ceil(q n/5), ceil((q+1) n/5))
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by(|&a, &b| rows[a].0.total_cmp(&rows[b].0).then(a.cmp(&b)));
        let ends: [usize; 5] = std::array::from_fn(|q| ((q + 1) * n + 4) / 5);
        let mut want_sum = [0.0f64; 5];
        let mut want_cnt = [0usize; 5];
        for (rank, &h) in idx.iter().enumerate() {
            let q = ends.iter().position(|&e| rank < e).unwrap();
            want_sum[q] += rows[h].1;
            want_cnt[q] += 1;
        }
        let (got_sum, got_cnt) = quintile_sums(&rows, &sorted);
        assert_eq!(got_cnt, want_cnt);
        for q in 0..5 {
            assert!((got_sum[q] - want_sum[q]).abs() <= 1e-9 * (1.0 + want_sum[q].abs()), "quintile {q}");
        }
    }

    #[test]
    fn gini_of_equal_wealth_is_zero() {
        assert_eq!(gini_sorted(&[5.0; 10]), 0.0);
        // one holder of everything among n approaches (n-1)/n
        let mut v = vec![0.0; 9];
        v.push(1.0);
        assert!((gini_sorted(&v) - 0.9).abs() < 1e-12);
    }

    #[test]
    fn snapshot_shapes() {
        let w = tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            3,
        );
        let s = snapshot(&w);
        assert_eq!(s.age_buckets.len(), 6);
        assert_eq!(s.by_age_ownership.len(), 6);
        assert_eq!(s.net_worth_deciles.len(), 10);
        assert_eq!(s.yield_curve.len(), N_TENORS);
        assert_eq!(s.sample_households.len(), 200);
        assert!(s.net_worth_deciles.windows(2).all(|p| p[0] <= p[1]));
    }
}

#[cfg(test)]
mod obs_helper_tests {
    use super::*;

    #[test]
    fn wage_variance_split_is_exact_and_matches_direct_computation() {
        // Firm A pays log wages 1 and 2; firm B pays 3, 4 and 5. Direct: total 2, between 1.5, within 0.5.
        let groups = [[2.0, 3.0, 5.0], [3.0, 12.0, 50.0]];
        let (total, between, within) = wage_variance_split(&groups);
        assert!((total - 2.0).abs() < 1e-12, "total {total}");
        assert!((between - 1.5).abs() < 1e-12, "between {between}");
        assert!((within - 0.5).abs() < 1e-12, "within {within}");
        assert!((total - between - within).abs() < 1e-12);
    }

    #[test]
    fn wage_variance_split_is_zero_without_dispersion() {
        let groups = [[2.0, 2.0, 2.0], [0.0, 0.0, 0.0]];
        assert_eq!(wage_variance_split(&groups), (0.0, 0.0, 0.0));
        assert_eq!(wage_variance_split(&[]), (0.0, 0.0, 0.0));
    }

    #[test]
    fn size_classes_are_in_persons() {
        // 2 heads = 3.5 persons (class 0); 3 heads = 5.25 persons (class 1); 286 heads = 500.5 persons (500+ class).
        assert_eq!(size_class_persons(firm_persons(2)), 0);
        assert_eq!(size_class_persons(firm_persons(3)), 1);
        assert_eq!(size_class_persons(firm_persons(286)), 7);
        assert_eq!(size_class_persons(499.0), 6);
        assert_eq!(size_class_persons(500.0), 7);
        assert_eq!(size_class_persons(1000.0), 8);
    }

    #[test]
    fn exit_age_classes_split_under_two_years() {
        assert_eq!(exit_age_class(0), 0);
        assert_eq!(exit_age_class(11), 0);
        assert_eq!(exit_age_class(12), 1);
        assert_eq!(exit_age_class(23), 1);
        assert_eq!(exit_age_class(24), 2);
        assert_eq!(exit_age_class(59), 2);
        assert_eq!(exit_age_class(60), 3);
        assert_eq!(exit_age_class(119), 3);
        assert_eq!(exit_age_class(120), 4);
    }
}

#[cfg(test)]
mod b7_cipi_tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    /// b7-spikes F1: CIPI (real, annualised) and the per-sector diagnostic after one month in which only a stock in
    /// `pick` (`true` = nonstorable sector, `false` = storable) changes by 10 units. Returns (cipi, per-sector, sector idx).
    fn cipi_after_stock_change(on: bool, nonstorable: bool) -> (f64, [f64; N_SECTORS], usize) {
        let cfg = Config { cipi_excludes_nonstorable: on, ..Default::default() };
        let mut w = tiny_world(cfg, 2);
        let mut st = MetricsState::default();
        let _ = compute(&w, &mut st); // first record: captures inventory_prev
        let i = (0..w.firms.len())
            .find(|&i| w.firms[i].alive && w.cfg.nonstorable_sectors[w.firms[i].sector.idx()] == nonstorable)
            .expect("a firm in the requested sector class");
        let s = w.firms[i].sector.idx();
        w.firms[i].inventory += 10.0;
        let m = compute(&w, &mut st);
        (m.inventory_change_real_bn_ar, m.cipi_by_sector_real_bn_ar, s)
    }

    #[test]
    fn nonstorable_stock_change_is_not_investment_when_on() {
        let (on, by_on, s) = cipi_after_stock_change(true, true);
        let (off, by_off, _) = cipi_after_stock_change(false, true);
        assert!(on.abs() < 1e-9, "switch on: CIPI from a perishable sector must be 0, got {on}");
        assert!(off.abs() > 1e-6, "switch off: legacy CIPI must record the perishable change, got {off}");
        // Diagnostic key is unchanged by the switch: it still shows the change in the perishable sector.
        assert!((by_on[s] - by_off[s]).abs() < 1e-9 * by_off[s].abs().max(1.0));
        assert!(by_on[s] > 0.0);
        // Legacy CIPI equals the sum of the per-sector diagnostics (write-offs are zero in this test).
        let sum_off: f64 = by_off.iter().sum();
        assert!((sum_off - off).abs() < 1e-9 * off.abs().max(1.0), "sum {sum_off} vs legacy {off}");
    }

    #[test]
    fn storable_stock_change_is_identical_with_switch_on_or_off() {
        let (on, by_on, _) = cipi_after_stock_change(true, false);
        let (off, by_off, _) = cipi_after_stock_change(false, false);
        assert!((on - off).abs() < 1e-9 * off.abs().max(1.0), "storable CIPI moved with the switch: {on} vs {off}");
        assert!(off > 0.0);
        let sum_on: f64 = by_on.iter().sum();
        assert!((sum_on - on).abs() < 1e-9 * on.abs().max(1.0), "sum {sum_on} vs CIPI {on}");
        for s in 0..N_SECTORS {
            assert!((by_on[s] - by_off[s]).abs() < 1e-9 * by_off[s].abs().max(1.0));
        }
    }
}
