//! Core agent and world types. Shared by every module — see docs/DESIGN.md §2.
//!
//! Modules must NOT mutate money fields (`deposits`, `reserves`, `tga`, loan `principal`,
//! bond holdings) directly — use `crate::ledger`.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::slab::Slab;

pub type HhId = u32;
pub type FirmId = u32;
pub type BankId = u16;
pub type HouseId = u32;
pub type LoanId = u32;
pub type BondId = u32;

pub const N_SECTORS: usize = 9;
/// Consumer-facing sectors are the first eight (all but Construction): Energy, Food, DurableGoods, NondurableGoods,
/// Tech, LeisureHospitality, HealthEducation, OtherServices. Construction is last and sells only to firms and
/// government (investment, housing), so it has no household consumption.
pub const N_CONSUMER: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Sector {
    Energy = 0,
    Food = 1,
    DurableGoods = 2,
    NondurableGoods = 3,
    Tech = 4,
    LeisureHospitality = 5,
    HealthEducation = 6,
    OtherServices = 7,
    Construction = 8,
}

impl Sector {
    pub const ALL: [Sector; N_SECTORS] = [
        Sector::Energy,
        Sector::Food,
        Sector::DurableGoods,
        Sector::NondurableGoods,
        Sector::Tech,
        Sector::LeisureHospitality,
        Sector::HealthEducation,
        Sector::OtherServices,
        Sector::Construction,
    ];
    pub const CONSUMER: [Sector; N_CONSUMER] = [
        Sector::Energy,
        Sector::Food,
        Sector::DurableGoods,
        Sector::NondurableGoods,
        Sector::Tech,
        Sector::LeisureHospitality,
        Sector::HealthEducation,
        Sector::OtherServices,
    ];
    #[inline]
    pub fn idx(self) -> usize {
        self as usize
    }
    pub fn from_idx(i: usize) -> Sector {
        Sector::ALL[i]
    }
    pub fn name(self) -> &'static str {
        match self {
            Sector::Energy => "Energy",
            Sector::Food => "Food",
            Sector::DurableGoods => "DurableGoods",
            Sector::NondurableGoods => "NondurableGoods",
            Sector::Tech => "Tech",
            Sector::LeisureHospitality => "LeisureHospitality",
            Sector::HealthEducation => "HealthEducation",
            Sector::OtherServices => "OtherServices",
            Sector::Construction => "Construction",
        }
    }
}

/// Normalised weighted mean of `x(s)` over the sectors that hold a nonzero weight in a role (`Config::tradable_weight`
/// and similar), summed in sector order. A role held by a single sector with weight 1 returns that sector's value
/// exactly; a role with no weight returns 0.
#[inline]
pub fn role_mean(weights: &[f64; N_SECTORS], x: impl Fn(usize) -> f64) -> f64 {
    let (mut num, mut den) = (0.0, 0.0);
    for (s, &wt) in weights.iter().enumerate() {
        if wt != 0.0 {
            num += wt * x(s);
            den += wt;
        }
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

/// Sum of `weights[s] * x(s)` over the sectors with a nonzero weight, in sector order (an unnormalised split).
#[inline]
pub fn role_sum(weights: &[f64; N_SECTORS], x: impl Fn(usize) -> f64) -> f64 {
    role_sum_from(0.0, weights, x)
}

/// As `role_sum`, but accumulating onto `init` (so a leading term keeps its place in the evaluation order).
#[inline]
pub fn role_sum_from(init: f64, weights: &[f64; N_SECTORS], x: impl Fn(usize) -> f64) -> f64 {
    let mut acc = init;
    for (s, &wt) in weights.iter().enumerate() {
        if wt != 0.0 {
            acc += wt * x(s);
        }
    }
    acc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Education {
    HighSchool,
    College,
    Graduate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LaborStatus {
    Student,
    Employed,
    Unemployed,
    Retired,
    OutOfLaborForce,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tenure {
    WithParents,
    Renter,
    Owner,
}

/// Income received during the current month (nominal $). Reset at the start of every tick.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct IncomeMonth {
    pub wage: f64,
    pub unemployment_benefit: f64,
    pub pension: f64,
    pub transfers: f64,
    pub rent_received: f64,
    pub interest: f64,
    pub dividends: f64,
    /// Proprietors' income: profit distributed by an owner-operated firm to its owner household.
    pub business: f64,
    // fix-households: one-off payments (helicopter money, rebates). Transitory: kept out of permanent income and
    // spent under the windfall rule in consumption.rs (Parker et al. 2013; Fagereng, Holm & Natvik 2021).
    #[serde(default)]
    pub windfall: f64,
}

impl IncomeMonth {
    #[inline]
    pub fn total(&self) -> f64 {
        self.wage
            + self.unemployment_benefit
            + self.pension
            + self.transfers
            + self.rent_received
            + self.interest
            + self.dividends
            + self.business
            + self.windfall
    }
}

/// Per-month flow record of a household, kept in `World::hh_month` (indexed like `World::households`) so
/// that phases which never read flows stream less memory.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct HhMonth {
    pub income: IncomeMonth,
    pub taxes: f64,
    /// Nominal amount actually spent this month per consumer sector.
    pub consumption: [f64; N_CONSUMER],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Household {
    pub id: HhId,
    pub alive: bool,
    pub age_months: u16,
    /// Married or cohabiting with a partner who shares the household. A partnered household survives the death of
    /// one partner (it continues as a single household); only a single household dissolves on death.
    pub partnered: bool,
    pub education: Education,
    /// Idiosyncratic productivity multiplier, mean 1.
    pub skill: f32,
    pub status: LaborStatus,
    pub employer: Option<FirmId>,
    /// Monthly gross wage when employed.
    pub wage: f64,
    /// Wage in last job (benefit & reservation-wage base).
    pub last_wage: f64,
    pub months_unemployed: u16,
    /// b6-reentry: months in the current unemployment spell as the BLS counts it (a month out of the labour force
    /// breaks the spell). Re-entrants resumed from spell memory (`udur_resume_spell`) restart it at 0, so it is the
    /// reported search-spell duration; `months_unemployed` keeps the resumed length for ranking and callbacks.
    #[serde(default)]
    pub bls_unemployed: u16,
    pub months_in_job: u16,
    /// Round 7 step 6: consecutive months in continuous employment, kept across job-to-job moves (same-line
    /// employment history, as in the Fannie Mae Selling Guide). Reset on separation to any non-employed status.
    #[serde(default)]
    pub months_employed: u16,
    pub reservation_wage: f32,
    pub benefit_months_left: u16,
    /// Cumulative nominal wage earnings (for pension).
    pub career_earnings: f64,
    pub career_months: u16,
    /// Monthly social security pension once retired.
    pub pension: f64,

    // --- money & assets (mutate via ledger only) ---
    pub bank: BankId,
    pub deposits: f64,
    /// Units of the Fund (equities + bonds). Change only via ledger::fund_buy/fund_sell.
    pub shares: f64,

    // --- housing ---
    pub tenure: Tenure,
    /// Dwelling the household lives in (None when WithParents).
    pub home: Option<HouseId>,
    /// All houses owned (includes `home` if Owner).
    pub properties: Vec<HouseId>,
    /// Monthly rent paid when Renter.
    pub rent: f64,
    pub lease_months_left: u16,
    /// Committed housing outflow this month (scheduled mortgage P&I + rent + property tax). Set by housing::step.
    /// Maintenance (cfg.maintenance × home value / 12) is NOT included: consumption adds it to the OtherServices order.
    pub housing_cost: f64,

    // --- expectations & consumption ---
    /// Smoothed monthly after-tax income.
    pub permanent_income: f64,
    /// Annual expected inflation.
    pub expected_inflation: f64,
    pub consumption_budget: f64,
    pub preferred_firm: [FirmId; N_CONSUMER],
}

impl Household {
    pub fn age_years(&self) -> f64 {
        self.age_months as f64 / 12.0
    }
    pub fn is_working_age(&self) -> bool {
        !matches!(self.status, LaborStatus::Retired | LaborStatus::Student)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Firm {
    pub id: FirmId,
    pub sector: Sector,
    pub alive: bool,
    pub age_months: u32,
    pub months_dead: u32,

    pub bank: BankId,
    pub deposits: f64,
    pub loans: Vec<LoanId>,

    pub workers: Vec<HhId>,
    /// Household that owns and runs this firm (proprietors' income); None for shareholder-owned firms.
    #[serde(default)]
    pub owner: Option<HhId>,
    /// Real output per efficiency unit of labour per month.
    pub labor_productivity: f64,
    /// Real capital stock (units of 2026 output of the capital-goods mix).
    pub capital: f64,
    /// Real output per unit of capital per month (capacity).
    pub capital_productivity: f64,
    /// Nominal monthly wage per efficiency unit.
    pub wage_offer: f64,
    /// Hires the firm needs this month (its shortfall of workers); matching hires at most this many.
    pub vacancies: u32,
    /// Openings posted this month = vacancies ÷ expected fill rate (JOLTS job openings). Drives the applicant
    /// sampling weights and the reported vacancy statistics; the number of hires remains capped by `vacancies`.
    #[serde(default)]
    pub openings: u32,
    pub planned_layoffs: u32,
    /// Desired output this month (real).
    pub target_output: f64,

    pub price: f64,
    pub markup: f64,
    pub months_since_reprice: u32,
    /// Unit cost estimated in last plan.
    pub unit_cost: f64,
    pub inventory: f64,
    /// Real output produced this month.
    pub output: f64,
    /// Real units sold this month.
    pub sales: f64,
    /// Real units demanded this month (including unmet).
    pub demand: f64,
    pub expected_demand: f64,
    /// Construction only: real construction units needed for houses in progress next months.
    pub backlog: f64,

    // flows this month (nominal)
    pub revenue: f64,
    pub wage_bill: f64,
    pub input_cost: f64,
    pub interest_paid: f64,
    pub depreciation_cost: f64,
    pub taxes: f64,
    pub profit: f64,
    pub dividends: f64,
    /// Smoothed annualised profit rate on capital.
    pub profit_rate: f64,
    /// Real capital goods ordered this month.
    pub investment_order: f64,

    pub months_distressed: u32,
    /// Re-fit Step 8(b): smoothed desired capital (`kstar_adapt`), carried month to month. 0 = unset.
    #[serde(default)]
    pub k_star_smooth: f64,
}

impl Firm {
    pub fn employees(&self) -> usize {
        self.workers.len()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Owner {
    Household(HhId),
    Firm(FirmId),
    Bank(BankId),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct House {
    pub id: HouseId,
    /// Hedonic quality multiplier, mean 1.
    pub quality: f64,
    pub owner: Owner,
    pub occupant: Option<HhId>,
    /// Current monthly rent if let.
    pub rent: f64,
    pub for_sale: bool,
    pub ask_price: f64,
    pub months_listed: u32,
    pub for_rent: bool,
    pub last_sale_price: f64,
    pub mortgage: Option<LoanId>,
    /// false while under construction.
    pub completed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Borrower {
    Household(HhId),
    Firm(FirmId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoanKind {
    Mortgage { house: HouseId },
    Business,
    Consumer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Loan {
    pub id: LoanId,
    pub bank: BankId,
    pub borrower: Borrower,
    pub kind: LoanKind,
    pub principal: f64,
    pub annual_rate: f64,
    pub term_months: u32,
    pub months_remaining: u32,
    /// Scheduled monthly payment (interest + principal).
    pub payment: f64,
    pub months_delinquent: u32,
    pub origination_tick: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Bank {
    pub id: BankId,
    pub alive: bool,
    pub reserves: f64,
    /// Sum of customer deposits (liability). Maintained by ledger.
    pub deposits: f64,
    /// Sum of outstanding loan principal (asset). Maintained by ledger.
    pub loans: f64,
    /// Discount-window borrowing from the central bank.
    pub cb_borrowing: f64,
    /// Market value of government bonds held (updated by monetary::markets).
    pub bonds_value: f64,
    pub mortgage_loans: f64,
    pub business_loans: f64,
    // monthly P&L
    pub interest_income: f64,
    pub interest_expense: f64,
    pub loan_losses: f64,
    /// The part of `loan_losses` recognised in this month's P&L (set by `banking::service_bank_pnl`). Losses booked
    /// after it (firm bankruptcies in `firms::close_books`) are the remainder, which `sim::begin_month` carries into the
    /// next month's P&L when `bank_loss_carry` is on.
    pub loan_losses_recognised: f64,
    pub profit: f64,
    pub dividends: f64,
    // rates offered (annual)
    pub deposit_rate: f64,
    pub mortgage_rate: f64,
    pub business_rate_base: f64,
    /// Lender balance-sheet premium on business loans (fraction; banking::set_rates adapts it monthly).
    pub ebp: f64,
    /// Business write-offs booked this month (firms::bankrupt), rolled into `biz_writeoff_hist` by banking.
    pub biz_writeoffs: f64,
    /// Business write-offs of the last 12 completed months (ring buffer; `biz_hist_pos` is the next slot).
    pub biz_writeoff_hist: [f64; 12],
    pub biz_hist_pos: u8,
    /// Slow-moving norm for the annualised business loss rate (set to the first observed rate).
    pub biz_loss_norm: f64,
}

impl Bank {
    pub fn equity(&self) -> f64 {
        self.reserves + self.loans + self.bonds_value - self.deposits - self.cb_borrowing
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BondHolder {
    Bank(BankId),
    CentralBank,
    Fund,
    RestOfWorld,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BondIssue {
    pub id: BondId,
    pub tenor_months: u32,
    /// Annual coupon rate.
    pub coupon: f64,
    pub issue_tick: u32,
    pub maturity_tick: u32,
    /// Face value held by each holder (mutate via ledger::bond_transfer / issue / redeem).
    pub holdings: Vec<(BondHolder, f64)>,
}

impl BondIssue {
    pub fn face_outstanding(&self) -> f64 {
        self.holdings.iter().map(|(_, f)| *f).sum()
    }
    pub fn held_by(&self, h: BondHolder) -> f64 {
        self.holdings
            .iter()
            .find(|(k, _)| *k == h)
            .map(|(_, f)| *f)
            .unwrap_or(0.0)
    }
}

/// Yield-curve tenors in months: 3m, 2y, 5y, 10y, 30y.
pub const TENORS: [u32; 5] = [3, 24, 60, 120, 360];
pub const N_TENORS: usize = 5;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct YieldCurve {
    /// Annual yields at TENORS.
    pub yields: [f64; N_TENORS],
    /// Expectations component at TENORS.
    pub expected: [f64; N_TENORS],
    /// Term premium at TENORS (incl. shocks).
    pub term_premium: [f64; N_TENORS],
}

impl YieldCurve {
    /// Linear interpolation in maturity (months); flat extrapolation.
    pub fn yield_at(&self, months: f64) -> f64 {
        let t = &TENORS;
        if months <= t[0] as f64 {
            return self.yields[0];
        }
        for i in 1..N_TENORS {
            if months <= t[i] as f64 {
                let a = t[i - 1] as f64;
                let b = t[i] as f64;
                let x = (months - a) / (b - a);
                return self.yields[i - 1] * (1.0 - x) + self.yields[i] * x;
            }
        }
        self.yields[N_TENORS - 1]
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CentralBank {
    pub policy_rate: f64,
    /// Government bonds' market value held (updated by monetary::markets).
    pub bonds_value: f64,
    pub discount_loans: f64,
    /// Treasury General Account balance (government's money). Mutate via ledger only.
    pub tga: f64,
    /// Cumulative remittance-relevant P&L (informational).
    pub income: f64,
    /// QE purchases this month (face) — informational.
    pub qe_this_month: f64,
    /// Helicopter transfer per household this month (set by monetary from shocks; paid by fiscal).
    pub helicopter_per_household: f64,
    pub helicopter_total: f64,
    // fix-fiscal
    /// Central-bank net income not yet remitted (negative = a deferred asset, as at the Fed in 2023-24). Positive
    /// income first pays this down before any remittance to the Treasury.
    #[serde(default)]
    pub deferred_asset: f64,
    /// Remittance to the Treasury this month, and cumulative (model units).
    #[serde(default)]
    pub remittance_month: f64,
    #[serde(default)]
    pub remittance_total: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Government {
    pub revenue: f64,
    pub outlays: f64,
    pub interest_paid: f64,
    pub purchases: f64,
    pub transfers: f64,
    pub deficit: f64,
    /// Face value of government debt outstanding (all holders).
    pub debt_face: f64,
    /// Social-security cost-of-living index (1.0 at t=0).
    pub cola_index: f64,
    /// Round 7 step 1: nominal scale of the dollar parameters (minimum wage, UI cap, asset limits, entrant
    /// reservation wage) at the last wage-tracked January; 1.0 at t=0. Set during the nominal-neutral burn-in.
    pub nominal_scale: f64,
}

/// The Fund: household-owned investment vehicle holding equity claims on all firms & banks plus bonds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Fund {
    pub bank: BankId,
    pub deposits: f64,
    pub total_shares: f64,
    pub bonds_value: f64,
    pub equity_value: f64,
    pub nav: f64,
    pub share_price: f64,
    // p1-equity: 12-month EWMA of the share price, the Fund-wealth input to the households' wealth effect (updated in
    // equity::revalue_fund_equity; 0 = not yet set, then the current price is used).
    #[serde(default)]
    pub share_price_ewma: f64,
    /// p1-equity: slow state of the demand risk premium, an EWMA of exog.risk_premium with decay `erp_slow_decay`
    /// (equity::revalue_fund_equity). It adds a persistent component to the ERP (`erp_slow_loading`).
    #[serde(default)]
    pub erp_slow: f64,
    /// Re-fit Step 10a: build-up of the centred financial factor in the ERP, an EWMA of f_t − f̄ with weight
    /// 1/`erp_fin_build_months` (equity::revalue_fund_equity). 0 until the first update.
    #[serde(default)]
    pub fin_erp_build: f64,
    /// Dividends received from firms & banks not yet distributed to households (NOT reset by begin_month;
    /// monetary::markets distributes and zeroes it).
    pub dividends_received: f64,
    /// Special payouts (surplus-cash buybacks, excess bank capital) pending distribution: paid to households but not
    /// capitalised into the equity valuation. Zeroed by monetary::markets.
    #[serde(default)]
    pub special_dividends: f64,
    pub smoothed_dividends: f64,
    /// Regular dividends received this month by paying sector (firms only; bank dividends are the residual of
    /// `dividends_received` and go to OtherServices). Zeroed by monetary::markets.
    #[serde(default)]
    pub sector_dividends_received: [f64; N_SECTORS],
    /// Smoothed dividends by sector (sum = `smoothed_dividends`), monetary::markets.
    #[serde(default)]
    pub sector_smoothed_dividends: [f64; N_SECTORS],
    /// Sector equity values (sum = `equity_value`), monetary::markets.
    #[serde(default)]
    pub sector_equity: [f64; N_SECTORS],
    /// EWMA of the sector's annualised log growth of smoothed dividends (extrapolative signal).
    #[serde(default)]
    pub sector_growth: [f64; N_SECTORS],
    /// EWMA of the sector's excess productivity growth during tech booms (decimal per year; equity::value_sectors).
    #[serde(default)]
    pub sector_news: [f64; N_SECTORS],
    /// Slow EWMA of smoothed sector dividends (the long leg of the growth signal; see equity::value_sectors).
    #[serde(default)]
    pub sector_slow_dividends: [f64; N_SECTORS],
    /// Sector Tobin's q relative to its normal level (1.0 at the first valuation).
    #[serde(default = "unit_q")]
    pub sector_q_rel: [f64; N_SECTORS],
    /// Normal q per sector (set at the first valuation); 0 until set.
    #[serde(default)]
    pub sector_q0: [f64; N_SECTORS],
    /// p1-equity: reference capitalisation rate per sector of the log-linear valuation (`equity_valuation_log`), set at
    /// seeding; 0 until set. Not used when the switch is off.
    #[serde(default)]
    pub sector_cap_ref: [f64; N_SECTORS],
}

fn unit_q() -> [f64; N_SECTORS] {
    [1.0; N_SECTORS]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RestOfWorld {
    pub bank: BankId,
    pub deposits: f64,
    pub bonds_value: f64,
    pub imports_this_month: f64,
    pub exports_this_month: f64,
    /// Trade-weighted dollar index (1.0 at t=0; higher = stronger dollar). Foreign currency per dollar is 1/fx.
    /// Set monthly by `fx::exchange_rate`.
    #[serde(default = "default_fx")]
    pub fx: f64,
}

fn default_fx() -> f64 {
    1.0
}

impl Default for RestOfWorld {
    fn default() -> Self {
        RestOfWorld {
            bank: 0,
            deposits: 0.0,
            bonds_value: 0.0,
            imports_this_month: 0.0,
            exports_this_month: 0.0,
            fx: 1.0,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Exogenous {
    pub oil_price: f64,
    pub oil_base: f64,
    /// Log deviation from stochastic AR(1) component.
    pub oil_ar: f64,
    /// Log deviation from user shocks.
    pub oil_shock: f64,
    /// Productivity level multiplier per sector (1.0 at t=0).
    pub tfp: [f64; N_SECTORS],
    /// Aggregate risk-premium (demand) shock: an AR(1) wedge, in annual rate units, added to the real rates that
    /// households and firms act on (Smets & Wouters 2007).
    #[serde(default)]
    pub risk_premium: f64,
    /// Re-fit Step 10a: AR(1) state of the demand shock alone (without the financial-factor loading). With
    /// `rp_fin_loading` = 0 it equals `risk_premium` every month.
    #[serde(default)]
    pub rp_ar: f64,
    /// Effective tech-boom flag this month (Markov regime or a forced TechBoom shock).
    #[serde(default)]
    pub tech_boom: bool,
    /// Consecutive months of the current tech boom (0 outside booms).
    #[serde(default)]
    pub tech_boom_months: u32,
    /// Markov state of the tech-boom regime (independent of forced shocks).
    #[serde(default)]
    pub tech_regime: bool,
    /// Realised monthly productivity multiplier by sector this month (before pending level shocks).
    #[serde(default = "unit_mult")]
    pub tfp_growth_m: [f64; N_SECTORS],
    // p0-exog: exogenous factors (all zero when their processes are off; see docs/DESIGN.md).
    /// Financial risk factor f_t (AR(1) with level-dependent volatility and jumps; positive = risk-off).
    #[serde(default)]
    pub fin_factor: f64,
    /// Normalised N(0,1) innovation of f_t this month (the diffusion draw, before the volatility scale).
    #[serde(default)]
    pub fin_factor_eps: f64,
    /// Number of financial risk jumps this month.
    #[serde(default)]
    pub fin_jump_n: u32,
    /// Term-premium level factor (a loading of the 10y term premium; negative = flight to quality). Equals the AR(1)
    /// state `tp_level_ar` plus the contemporaneous loading `tp_level_fin_loading · f_t`.
    #[serde(default)]
    pub tp_level: f64,
    /// AR(1) state of the term-premium level factor, without the flight-to-quality loading (the loading is not fed
    /// back into the state, so it stays a loading on the current f_t).
    #[serde(default)]
    pub tp_level_ar: f64,
    /// Term-premium curvature factor (Nelson–Siegel hump loading).
    #[serde(default)]
    pub tp_curv: f64,
    /// Dollar risk premium ψ_t (log of the dollar, added after the partial-adjustment step).
    #[serde(default)]
    pub fx_psi: f64,
    /// Conditional sd of this month's oil innovation (the constant `oil_sigma` unless GARCH is on).
    #[serde(default)]
    pub oil_vol: f64,
    /// GARCH conditional variance of the oil innovation used this month (0 before the first GARCH month).
    #[serde(default)]
    pub oil_cond_var: f64,
    /// Squared oil innovation of the previous month (the GARCH ARCH term).
    #[serde(default)]
    pub oil_prev_shock_sq: f64,
    /// Number of oil jumps this month.
    #[serde(default)]
    pub oil_jump_n: u32,
}

// p0-exog: accessors for the new factors (read-only views for the modules that use them).
impl Exogenous {
    /// Financial risk factor f_t.
    pub fn fin_factor(&self) -> f64 {
        self.fin_factor
    }
    /// Term-premium level factor.
    pub fn tp_level(&self) -> f64 {
        self.tp_level
    }
    /// Term-premium curvature factor.
    pub fn tp_curv(&self) -> f64 {
        self.tp_curv
    }
    /// Dollar risk premium ψ_t.
    pub fn fx_psi(&self) -> f64 {
        self.fx_psi
    }
    /// Conditional sd of this month's oil innovation.
    pub fn oil_vol_state(&self) -> f64 {
        self.oil_vol
    }
}

fn unit_mult() -> [f64; N_SECTORS] {
    [1.0; N_SECTORS]
}

/// Aggregate price indices (updated by goods::clear and housing).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Prices {
    /// Sales-weighted average price per sector (1.0 at t=0).
    pub sector: [f64; N_SECTORS],
    /// House price index ($ per unit quality).
    pub hpi: f64,
    /// Market rent index ($/month per unit quality).
    pub rent_index: f64,
    pub cpi: f64,
    pub core_cpi: f64,
    /// CPI history for inflation calculations (one entry per month, oldest first).
    pub cpi_history: Vec<f64>,
    pub core_history: Vec<f64>,
    /// Core PCE-weighted price index and its history (shelter ≈17% of core PCE vs ≈44% of core CPI; BEA, BLS).
    #[serde(default)]
    pub core_pce: f64,
    #[serde(default)]
    pub core_pce_history: Vec<f64>,
    pub hpi_history: Vec<f64>,
    pub wage_index: f64,
}

/// Orders placed this month; consumed by goods::clear. Nominal $ budgets.
#[derive(Clone, Debug, Default)]
pub struct Orders {
    /// Indexed by household id; nominal budget per consumer sector.
    pub household: Vec<[f64; N_CONSUMER]>,
    /// Indexed by firm id; nominal budget for intermediate inputs per supplying sector.
    pub firm_inputs: Vec<[f64; N_SECTORS]>,
    /// Indexed by firm id; nominal budget for capital goods per supplying sector (`Config::capital_goods_mix`).
    pub firm_investment: Vec<[f64; N_SECTORS]>,
    pub government: [f64; N_SECTORS],
    pub exports: [f64; N_SECTORS],
}

/// Fulfilment results written by goods::clear.
#[derive(Clone, Debug, Default)]
pub struct Fills {
    /// Real capital goods received by each firm this month.
    pub firm_capital_received: Vec<f64>,
    /// Nominal unspent household budget (forced saving) this month.
    pub household_unspent: Vec<f64>,
}

/// National-accounts flow accumulators for this month (nominal, per-agent units, NOT scaled).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Flows {
    pub consumption: [f64; N_CONSUMER],
    pub investment: f64,
    pub residential_investment: f64,
    pub government: f64,
    pub exports: f64,
    /// Realised exports by sector, nominal (integration: deflated by each sector's own price in metrics).
    pub exports_by_sector: [f64; N_SECTORS],
    pub imports: f64,
    /// b8-world stage 2 (hook H9): customs value of imports by sector this month, duty excluded (household and firm sides).
    /// Filled on both paths (accounting only; it does not change any ABM quantity); its sum equals `row.imports_this_month`.
    #[serde(default)]
    pub imports_by_sector: [f64; N_SECTORS],
    // --- b7-tariff: duties by channel (zero with `tariff_pass_through` off or no active tariff) ---
    /// b7-tariff: household duty paid to Government by consumer sector (the retail wedge), nominal.
    #[serde(default)]
    pub tariff_hh: [f64; N_CONSUMER],
    /// b7-tariff: duty absorbed by domestic firms (signed; negative after a tariff cut below the retail wedge), nominal.
    #[serde(default)]
    pub tariff_absorbed: f64,
    /// b7-tariff: duty on firms' imported intermediate inputs, nominal.
    #[serde(default)]
    pub tariff_inputs: f64,
    /// b7-tariff: duty on firms' imported capital goods (equipment), nominal.
    #[serde(default)]
    pub tariff_equipment: f64,
    /// b7-tariff: duty collected on the legacy path (`tariff_pass_through` off: the lump-sum household duty and the
    /// firms' unmet-order duty), nominal. Counted in `tariff_revenue_bn_ar` only; not in `tariff_hh`.
    #[serde(default)]
    pub tariff_legacy: f64,
    /// b7-tariff (M4): duty above the retail wedge that no firm could be charged (no alive firm in the covered sector),
    /// signed, nominal. Not collected, so not in any revenue metric.
    #[serde(default)]
    pub tariff_uncollected: f64,
    /// Re-fit Step 4c: production taxes on domestic crude paid this month (to the Treasury); 0 with the split off.
    pub upstream_tax: f64,
    /// Re-fit Step 4c: private mineral royalties on domestic crude paid this month (to the Fund); 0 with the split off.
    pub upstream_royalty: f64,
    /// Re-fit Step 5a: inventory units written off with bankrupt firms this month, by sector (added back to the CIPI
    /// fold when `cipi_excludes_writeoffs` is on).
    pub bk_inventory_units: [f64; N_SECTORS],
    pub intermediate: f64,
    pub wages: f64,
    pub profits: f64,
    pub hires: u32,
    pub separations: u32,
    pub layoffs: u32,
    pub vacancies: u32,
    pub house_sales: u32,
    pub house_sale_value: f64,
    pub housing_starts: u32,
    pub housing_completions: u32,
    pub foreclosures: u32,
    /// Evictions of renters this month (housing.rs pass 2: arrears at or above the eviction threshold).
    pub evictions: u32,
    /// Foreclosures of owner-occupied homes this month (the borrower still lived in the house).
    pub foreclosures_owner_occ: u32,
    pub new_mortgages: u32,
    pub new_mortgage_value: f64,
    pub refinances: u32,
    pub new_leases: u32,
    pub firm_bankruptcies: u32,
    pub firm_entries: u32,
    pub deaths: u32,
    pub births: u32,
    pub retirements: u32,
    pub loan_losses: f64,
    pub new_business_loans: f64,
    pub qe_purchases: f64,
    pub helicopter: f64,

    // --- fix-labour: labour-market flow and stock counters (labor.rs), per month ---
    /// Employer-to-employer moves (employed households hired by another firm).
    pub ee: u32,
    /// Hires from unemployment and from out of the labour force (direct N->E hires).
    pub hires_from_u: u32,
    pub hires_from_n: u32,
    /// Exogenous match-destruction separations to unemployment (layoffs are counted in `layoffs`).
    pub sep_exo: u32,
    /// Employed, unemployed and out-of-labour-force households of working age at the start of the labour step,
    /// and separations to unemployment, by education (HighSchool, College, Graduate).
    pub emp_edu: [u32; 3],
    pub u_stock_edu: [u32; 3],
    pub hires_u_edu: [u32; 3],
    pub sep_edu: [u32; 3],
    /// Unemployed households by months unemployed (1-3, 4-6, 7-12, 13+) and hires from them.
    pub u_stock_dur: [u32; 4],
    pub hires_u_dur: [u32; 4],
    /// Unemployed households by UI status (benefits with more than one month left, last benefit month, exhausted)
    /// and hires from them.
    pub u_stock_ui: [u32; 3],
    pub hires_u_ui: [u32; 3],
    /// Out-of-labour-force households of working age at the start of the labour step.
    pub n_stock: u32,
    /// Unemployed households who left the labour force (U->N) this month.
    pub u_to_n: u32,
    // b5-udur: duration diagnostics (observation counters; no effect on the market at defaults).
    /// Applicants left unmatched at firms whose openings were all filled, and the number of such firms.
    pub udur_rank_leftover: u32,
    pub udur_rank_firms: u32,
    /// Applicants rejected by the callback screen (`udur_screen_on`).
    pub udur_screened: u32,
    /// Households paid an extended UI month this month (`udur_ui_ext_*`).
    pub udur_ui_ext_n: u32,
    /// Non-participants who re-entered unemployment with their spell resumed (`udur_resume_spell`).
    pub udur_resumed: u32,
    /// Sum of ln(offer / last wage) for hires from unemployment with a last wage, and its count, by duration bucket.
    pub udur_rewage_sum: [f64; 4],
    pub udur_rewage_n: [u32; 4],
    // b6-reentry: non-participants who re-entered unemployment, by hazard class (recent leaver / long-spell scar).
    pub udur_reentry_recent_n: u32,
    pub udur_reentry_scar_n: u32,
    // b7-spikes (diagnostic): firm-to-firm orders not paid by a supplier (nominal, before tariff), the shortfall that
    // goods::firm_stage imports. No effect on the economy.
    pub firm_shortfall_imports: f64,
    // b7-spikes (diagnostic): household budget imported as unmet alternative-supplier spending (tradables).
    pub hh_unmet_imports: f64,
    // b7-spikes (diagnostic): largest headcount (workers.len()) of a firm entering `firms::bankrupt` this month.
    pub bk_max_heads: u32,
}

#[derive(Clone, Debug)]
pub struct World {
    pub cfg: Config,
    pub seed: u64,
    pub tick: u32,

    pub households: Vec<Household>,
    /// Per-month flows, one per household (same index as `households`; always the same length).
    pub hh_month: Vec<HhMonth>,
    pub firms: Vec<Firm>,
    pub banks: Vec<Bank>,
    pub houses: Vec<House>,
    pub loans: Slab<Loan>,
    pub bonds: Vec<BondIssue>,
    pub next_bond_id: BondId,

    pub cb: CentralBank,
    pub gov: Government,
    pub fund: Fund,
    pub row: RestOfWorld,
    pub curve: YieldCurve,
    pub exog: Exogenous,
    // p1-fx: anchored dollar risk premium ψ'_{t−1} (`fx::anchored_premium`; zero through burn-in). fx::psi_step uses it
    // in q_t = (1 − w)ψ_t + wψ_{t−1}.
    pub fx_psi_state: f64,
    // p1-fx: the exogenous premium ψ_{t−1} of the previous applied month, for the innovation ψ_t − ρψ_{t−1}.
    pub fx_psi_exog_prev: f64,
    // p1-fx: the part of ln fx explained by the dollar risk premium (`fx::psi_step`); ln fx − this = premium-free level.
    pub fx_psi_component: f64,
    // p1-fx: lagged logs of the export price ratio (by sector) and of the import terms ratios (by sector, then the
    // tradables mean used by the firm channels), the distributed-lag states of goods.rs. None before the first clear.
    pub trade_export_z: Option<[f64; N_SECTORS]>,
    pub trade_import_z: Option<[f64; N_SECTORS + 1]>,
    pub prices: Prices,

    pub orders: Orders,
    pub fills: Fills,
    pub flows: Flows,

    pub shocks: crate::shocks::ShockState,

    // module-owned state
    pub labor: crate::labor::LaborState,
    pub housing: crate::housing::HousingState,
    pub firms_state: crate::firms::FirmsState,
    pub goods: crate::goods::GoodsState,
    pub monetary: crate::monetary::MonetaryState,
    pub fiscal: crate::fiscal::FiscalState,
    pub demography: crate::demography::DemographyState,
    pub banking: crate::banking::BankingState,
    pub consumption: crate::consumption::ConsumptionState,
    pub metrics_state: crate::metrics::MetricsState,

    /// One entry per completed month.
    pub history: Vec<crate::metrics::Metrics>,
    /// The last 12 months recorded before `history` starts (the end of burn-in), so year-on-year metrics and
    /// last-month lookups are continuous at month 0. Not part of the reported run.
    pub history_pre: Vec<crate::metrics::Metrics>,
    /// Months simulated before `tick` was reset to 0 at the end of burn-in. Deterministic trends in calendar time
    /// (e.g. the world export price) use `tick + tick_offset` so they continue smoothly across the reset.
    pub tick_offset: u32,
    /// b8-world H4: the world block (`world::WorldBlock`); `None` when `Config::world_block_enabled` is off.
    pub world: Option<Box<crate::world::WorldBlock>>,
    /// us-now: steering state of the "Today's US economy" start (all inactive for the typical start). See `start.rs`.
    pub steer: crate::start::SteerState,
}

/// Calibration of `World::approx_clone_bytes` (R13-S): bytes per household, and the fixed part outside the history.
pub const CLONE_BYTES_PER_HOUSEHOLD: usize = 1_122;
pub const CLONE_FIXED_BYTES: usize = 146_500;

impl World {
    /// Approximate heap bytes of a copy of this world, the figure the server's common-world cache charges (R13-S). A model, not a
    /// measurement: the per-household vectors of the household, firm, bank and module state cost about 1.1 KB per household, the
    /// history costs its record size per month, and a fixed 0.15 MB covers the rest. Calibrated with a counting allocator on a clone
    /// of the typical, world-profile and us-now worlds at 1,000, 5,000 and 20,000 households (`CLONE_*` below).
    pub fn approx_clone_bytes(&self) -> usize {
        let per_month = std::mem::size_of::<crate::metrics::Metrics>();
        CLONE_FIXED_BYTES + self.households.len() * CLONE_BYTES_PER_HOUSEHOLD + self.history.len() * per_month
    }

    /// An empty world with defaults; `init::build_world` fills it.
    pub fn empty(cfg: Config, seed: u64) -> World {
        World {
            cfg,
            seed,
            tick: 0,
            fx_psi_state: 0.0, // p1-fx
            fx_psi_exog_prev: 0.0, // p1-fx
            fx_psi_component: 0.0, // p1-fx
            trade_export_z: None, // p1-fx
            trade_import_z: None, // p1-fx
            households: Vec::new(),
            hh_month: Vec::new(),
            firms: Vec::new(),
            banks: Vec::new(),
            houses: Vec::new(),
            loans: Slab::new(),
            bonds: Vec::new(),
            next_bond_id: 0,
            cb: CentralBank::default(),
            gov: Government {
                cola_index: 1.0,
                nominal_scale: 1.0,
                ..Default::default()
            },
            fund: Fund::default(),
            row: RestOfWorld::default(),
            curve: YieldCurve::default(),
            exog: Exogenous::default(),
            prices: Prices::default(),
            orders: Orders::default(),
            fills: Fills::default(),
            flows: Flows::default(),
            shocks: Default::default(),
            labor: Default::default(),
            housing: Default::default(),
            firms_state: Default::default(),
            goods: Default::default(),
            monetary: Default::default(),
            fiscal: Default::default(),
            demography: Default::default(),
            banking: Default::default(),
            consumption: Default::default(),
            metrics_state: Default::default(),
            history: Vec::new(),
            history_pre: Vec::new(),
            tick_offset: 0,
            world: None,
            steer: Default::default(),
        }
    }

    /// Number of household agents (alive or slots).
    pub fn n_households(&self) -> usize {
        self.households.len()
    }

    /// Year-over-year CPI inflation (decimal), from price history; 0 if not enough history.
    pub fn inflation_yoy(&self) -> f64 {
        yoy(&self.prices.cpi_history)
    }
    pub fn core_inflation_yoy(&self) -> f64 {
        yoy(&self.prices.core_history)
    }
    /// Year-on-year core PCE-weighted inflation (core CPI before a year of PCE history exists).
    pub fn core_pce_inflation_yoy(&self) -> f64 {
        if self.prices.core_pce_history.len() >= 13 {
            yoy(&self.prices.core_pce_history)
        } else {
            self.core_inflation_yoy()
        }
    }
    /// Last recorded metrics (None before the first month completes).
    pub fn last(&self) -> Option<&crate::metrics::Metrics> {
        self.history.last().or_else(|| self.history_pre.last())
    }
    /// Real-time estimate of the natural unemployment rate (adaptive; set by monetary::set_policy), falling back to
    /// the configured value before the first estimate.
    pub fn natural_unemployment(&self) -> f64 {
        if self.monetary.u_star > 0.0 {
            self.monetary.u_star
        } else {
            self.cfg.natural_unemployment
        }
    }
    /// Unemployment rate as a decimal (metrics store percent); falls back to the config natural rate.
    pub fn unemployment_rate(&self) -> f64 {
        self.last()
            .map(|m| m.unemployment_rate / 100.0)
            .unwrap_or(self.cfg.natural_unemployment)
    }
}

fn yoy(h: &[f64]) -> f64 {
    if h.len() >= 13 {
        h[h.len() - 1] / h[h.len() - 13] - 1.0
    } else if h.len() >= 2 {
        // annualise what we have
        let n = (h.len() - 1) as f64;
        (h[h.len() - 1] / h[0]).powf(12.0 / n) - 1.0
    } else {
        0.0
    }
}

/// Annuity payment for a loan.
pub fn annuity_payment(principal: f64, annual_rate: f64, months: u32) -> f64 {
    if months == 0 {
        return principal;
    }
    let r = annual_rate / 12.0;
    if r.abs() < 1e-9 {
        return principal / months as f64;
    }
    principal * r / (1.0 - (1.0 + r).powi(-(months as i32)))
}

/// Present value of a loan given payment, rate and remaining months (inverse of annuity_payment).
pub fn annuity_principal(payment: f64, annual_rate: f64, months: u32) -> f64 {
    let r = annual_rate / 12.0;
    if r.abs() < 1e-9 {
        return payment * months as f64;
    }
    payment * (1.0 - (1.0 + r).powi(-(months as i32))) / r
}
