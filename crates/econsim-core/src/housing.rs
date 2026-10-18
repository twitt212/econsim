//! Housing market (docs/DESIGN.md §10, §16): rents and property tax, arrears and eviction, foreclosures,
//! leaving parents, listings, bids and double-auction matching, purchase mortgages and refinancing,
//! rental matching, house-price and rent indices, and construction.
//!
//! Money moves only through `crate::ledger` (and `banking::originate_mortgage` for new loans).
//! Per-agent state that the shared types do not hold lives in `HousingState`, indexed by id.

use std::cmp::Ordering;

use rayon::prelude::*;

use crate::banking::{self, MortgageQuote};
use crate::common::{self, sector_price};
use crate::ledger::{self, Account};
use crate::rng::{self, stream, Rng, SimRng};
use crate::types::*;

// ---------------------------------------------------------------- parameters

/// Rng phase tags mixed into the agent id so that each decision draws from its own stream.
const PH_LEAVE: u64 = 1;
const PH_LIST: u64 = 2;
const PH_BID: u64 = 3;
const PH_RENT_UNIT: u64 = 4;
const PH_LEASE: u64 = 5;
const PH_REFI: u64 = 6;
const PH_FORECLOSE: u64 = 7;
const PH_DEMOLISH: u64 = 8;
const PH_CURE: u64 = 9;
/// Distressed-sale draw for delinquent borrowers with equity (after the anchor).
const PH_DISTRESS: u64 = 10;
/// b4-rent: expiry routing draw for an unlet vacant listing (keyed `(PH_EXPIRE << 40) | house_id` on `stream::HOUSING`).
const PH_EXPIRE: u64 = 11;

/// Foreclosure triggers (§10, burn-in): ≥3 months delinquent and unemployed or LTV > 0.9. After the anchor, see `foreclosures`.
const FORECLOSURE_DELINQUENT_MONTHS: u32 = 3;
/// Monthly probability that an eligible delinquent borrower actually starts foreclosure in burn-in (master's rule, so
/// the month-0 state is unchanged). After the anchor the draw is `Config::housing_foreclosure_prob`.
const FORECLOSURE_START_PROB: f64 = 0.15;
/// fix-housing (review 8): the post-anchor foreclosure draw ramps in over this many months from the anchor, so the
/// delinquency stock built under master's rule does not turn into a burst in the first months.
const FORECLOSURE_RAMP_MONTHS: f64 = 6.0;
/// Burn-in foreclosure rule (master): LTV above this, with delinquency and unemployment or equity.
const FORECLOSURE_LTV: f64 = 0.9;
/// Selling costs (agent commission and closing) deducted from value when testing negative equity after the anchor.
const SELL_COST: f64 = 0.06;
/// Monthly probability that a delinquent borrower who is not foreclosed this month cures (catches up, is modified or
/// is re-employed). Without it, 3+ month delinquencies with equity never resolve.
const DELINQUENCY_CURE_PROB: f64 = 0.25;
/// Eviction when rent owed reaches this many months of rent (§10: 2+ months in arrears).
const EVICTION_MONTHS_OWED: f64 = 2.0;
/// Lease length and exit / increase rules (§10).
const LEASE_MONTHS: u32 = 12;
/// Households per parallel chunk in rent and property-tax collection (fixed, so results are thread-count independent).
const RENT_CHUNK: usize = 1024;
const LEASE_EXIT_PROB: f64 = 0.2;
const LEASE_MAX_INCREASE: f64 = 1.10;
/// Transfer tax paid by the buyer; closing costs are a down-payment requirement (§10).
const TRANSFER_TAX: f64 = 0.01;
const CLOSING_COSTS: f64 = 0.02;
/// Bank-owned foreclosed houses are listed at this share of value.
const REO_ASK_FACTOR: f64 = 0.85;
/// Asking price and listing life (§10).
const ASK_MARKUP: f64 = 1.04;
const ASK_NOISE: f64 = 0.05;
const ASK_DECAY: f64 = 0.97;
/// Extra ask-cut sensitivity to a slack market, and the months of supply above which sellers cut faster (the
/// calibrated market's ≈5.4 months; the cut is unchanged at or below it).
const ASK_SLACK_SENS: f64 = 2.0;
const NORMAL_MONTHS_SUPPLY: f64 = 5.4;
const LISTING_LIFE_MONTHS: u32 = 6;
/// Listing rates (§10). Owners under 75 list to move (buy-sell chain) at OWNER_LIST_PROB a month.
const OWNER_LIST_PROB: f64 = 0.0028;
/// Listing-probability reduction per percentage point of mortgage-rate lock-in, and its floor.
const LOCK_IN_PER_PP: f64 = 0.10;
const LOCK_IN_FLOOR: f64 = 0.4;
const RETIREE_AGE: f64 = 75.0;
/// Owner→renter exit: an owner lists the home to leave ownership for a rental (divorce, financial distress, moves to
/// rentals and assisted living). Annual listing hazard by age band (lower bound, rate); a listing is also gated by
/// lock-in and the market, so realised exits run below the table (≈ half of it under 35, ≈ table at 35–64). A sale of
/// an exit listing makes the household a renter seeking a lease, and it does not bid to buy again for
/// EXIT_REBUY_COOLDOWN_MONTHS. Sources: PSID/SIPP owner→renter exits ≈4–5% a year under 35, ≈1.5–2% at 35–64,
/// ≈2–3% at 65+ (≈2% overall). Deviation: 65+ rates are set at 0.75% (65–74) and 1% (75+). At the PSID rates the
/// 65+ homeownership profile falls 10–20pp below HVS (Census 2024: 79% at 65+) because renter→owner inflows at 65+
/// are ≈2% a year (PSID) and ageing and deaths move owners into the band. The former retiree listing (0.3% a month)
/// sold homes as moves and let retirees buy again; that path is gone.
const OWNER_EXIT_BANDS: [(f64, f64); 4] = [(0.0, 0.042), (35.0, 0.01225), (65.0, 0.00525), (75.0, 0.007)];
/// fix-housing: the rates above are scaled by 0.7 because an exit now takes effect at listing (unsold exit listings
/// become rentals too), which otherwise pulled homeownership down ~3pp over 16 years (66 -> 62; US 65.6, avg 65.5 after).
/// Months a former owner who sold to rent waits before bidding to buy again.
const EXIT_REBUY_COOLDOWN_MONTHS: u16 = 36;
// fix-housing: investor sales follow a smoothed expected-return margin (see `investor_sell_prob`), not a one-month
// yield gate. No sales while the margin is above −INVESTOR_SELL_TRIGGER; the monthly probability then ramps linearly
// (over INVESTOR_SELL_RAMP) to INVESTOR_SELL_CAP.
/// Margin (pp below zero, smoothed) at which investors start to sell.
const INVESTOR_SELL_TRIGGER: f64 = 0.015;
/// Margin range over which the sale probability ramps from zero to the cap.
const INVESTOR_SELL_RAMP: f64 = 0.02;
/// Cap on the monthly sale probability of an investor property.
const INVESTOR_SELL_CAP: f64 = 0.008;
/// Monthly weight of the investor margin's exponential average (12-month memory: a persistent fall, not a blip).
const INVESTOR_MARGIN_EMA_WEIGHT: f64 = 1.0 / 12.0;
/// Investors own more than one property; cap portfolio size to avoid runaway accumulation.
const INVESTOR_MIN_PROPERTIES: usize = 2;
const MAX_PROPERTIES: usize = 10;
/// First-purchase bid hazard for renters by age band (lower bound, annual rate of bids). Bids must still clear the
/// affordability, down-payment and tenure-logit tests, so realised renter→owner transitions run below these rates.
/// Sources: PSID/SIPP renter→owner transitions a year, 18–24: 4%, 25–34: 10%, 35–44: 8%, 45–54: 5%, 55–64: 3.5%,
/// 65+: 2%; Census HVS 2024 homeownership by age (≈26% under 25, 38% at 25–34, 62% at 35–44, 70% at 45–54, 75% at
/// 55–64, 79% at 65+). The rates are set so the stock matches HVS at 25–64 (the realised rates run ≈1.2–1.5× PSID:
/// with PSID-level inflows the 25–34 stock falls to ≈30%). Renters must have rented first (see `bid_for`).
const FIRST_BUY_BANDS: [(f64, f64); 6] = [(0.0, 0.10), (25.0, 0.27), (35.0, 0.18), (45.0, 0.10), (55.0, 0.07), (65.0, 0.05)];
/// Desired purchase price as a multiple of gross annual income (Baptista et al. 2016 use ≈4.5; US median price ≈4×
/// median buyer income, NAR 2024).
const BID_INCOME_MULTIPLE: f64 = 4.0;
/// Rent growth per unit of rental-vacancy gap (annual).
const RENT_VACANCY_SENS: f64 = 0.3;
/// Monthly weight of the rent-inflation trend: an EMA over about ten years (fix-housing, review 6 fix 3). Over the 36-month
/// IRF it absorbs about a fifth of a shock, so an oil shock barely reaches rents through expectations (a 4-year EMA
/// absorbed about 40% by month 24).
const RENT_TREND_EMA_WEIGHT: f64 = 1.0 / 120.0;
/// Monthly weight of the income-growth average (≈ one year's memory).
const INCOME_GROWTH_EMA: f64 = 1.0 / 12.0;
/// Monthly asking-rent cut for a vacant unit, and the floor relative to market rent.
const VACANT_RENT_DECAY: f64 = 0.97;
const VACANT_RENT_FLOOR: f64 = 0.7;
/// Household landlords put a rental unit that has been empty this long up for sale, with a monthly probability of up to
/// VACANT_SELL_PROB that rises as the for-sale market tightens (zero at or above the balanced months of supply).
const VACANT_SELL_MONTHS: u16 = 6;
const VACANT_SELL_PROB: f64 = 0.2;
const VACANT_SELL_BASE_PROB: f64 = 0.03;
// b4-rent: bounds of the re-anchored vacant-rental sale hazard, the expiry let probability and the first-time-buyer
// multiplier (see `vacant_sell_prob`, `expiry_let_prob`, `buy_rate_mult`).
const VSELL_P_MIN: f64 = 0.01;
const VSELL_P_MAX: f64 = 0.25;
const EXPIRY_LET_MIN: f64 = 0.25;
const BUY_MULT_MIN: f64 = 0.5;
const BUY_MULT_MAX: f64 = 1.5;
/// Normal share of all completed units that are vacant (for-sale, for-rent, between occupants).
/// House-price growth per month of supply below a balanced market (≈ 6 months; NAR).
/// House-price growth per log-point of (balanced / actual) months of supply (annual).
const HPI_LOG_SUPPLY_SENS: f64 = 0.045;
const BALANCED_MONTHS_SUPPLY: f64 = 4.0;
/// Sale-to-hedonic-value ratio that the index holds in a balanced market. The sales are divided by each house's
/// idiosyncratic value multiplier (`match_bids`), so the ratio is measured on the house-quality index. Asks start at
/// 1.04× value and are cut 3%/month, so a house selling after 2–4 months goes for ≈0.96–1.0× value. Calibrated (review
/// 10) so the burn-in level matches master's month-0 HPI (the removed selection on the multiplier had been holding the
/// level down at 0.98).
const SALE_TO_VALUE_NORM: f64 = 1.045;
/// Monthly probability that a recent seller bids, and how long they keep searching.
const MOVER_BUY_PROB: f64 = 0.6;
/// Rent multiple households will pay in user cost to own rather than rent. Chosen with the logit slope below to keep
/// homeownership at the calibrated level (68.7% vs 68.65% before the change).
const OWNERSHIP_PREMIUM: f64 = 1.6;
/// Slope of the tenure-choice logit in the relative rent–user-cost gap. A flatter logit means wider dispersion in
/// unobserved tenure preferences, so a 1pp rate rise moves ≈5% of the buy probability (rather than ≈16%).
const TENURE_LOGIT_SLOPE: f64 = 1.0;
/// Monthly probability that an idle (empty, unlisted) household-owned unit is listed for sale (else let). Most
/// inherited homes are sold rather than let.
const IDLE_LIST_PROB: f64 = 0.85;
const MOVER_MONTHS: u8 = 24;
const INVESTOR_PROB: f64 = 0.003;
const INVESTOR_WEALTH_QUANTILE: f64 = 0.85;
const INVESTOR_MIN_YIELD_SPREAD: f64 = 0.015;
/// Scale of the logistic in the buy-to-let yield margin: dispersion of investors' required returns (≈ 2.6 pp s.d.).
const INVESTOR_MARGIN_SCALE: f64 = 0.025;
/// Property tax + maintenance + management as a share of value (annual).
const INVESTOR_RUNNING_COSTS: f64 = 0.025;
/// Months of spending kept liquid when bidding.
const RESERVE_MONTHS: f64 = 1.0;
/// Rental affordability: rent ≤ 40% of monthly income, with a $600 floor (§10). Round 7 step 8 (`rent_floor_indexed`):
/// seekers may spend a further `SEEKER_EXTRA_BURDEN` of income (about a quarter of US renters pay over half their income,
/// JCHS), and the floor is `RENT_FLOOR_INDEX_SHARE` of the rent index, so it erodes with rents rather than staying nominal.
const RENT_BUDGET_SHARE: f64 = 0.40;
const YOUNG_RENT_BUDGET_SHARE: f64 = 0.50;
const MIN_RENT_BUDGET: f64 = 600.0;
const RENT_FLOOR_INDEX_SHARE: f64 = 0.35;
const SEEKER_EXTRA_BURDEN: f64 = 0.10;
/// Refinancing (§10): gap, threshold and probability are in `Config` (`refi_*`); the owner must be employed, current
/// on the loan and below this LTV.
const REFI_MAX_LTV: f64 = 0.8;
// fix-housing: user cost of ownership = mortgage rate + property tax (config) + maintenance and depreciation − expected
// appreciation. The rent anchor moves with it (Poterba 1984; Dias & Duarte 2019). Monthly weight of its 24-month EMA (1/24).
const UC_MAINTENANCE: f64 = 0.01;
const UC_EMA_WEIGHT: f64 = 1.0 / 24.0;
/// Monthly weight of the expected-appreciation EMA in the user cost (36-month memory).
const UC_EHPA_EMA_WEIGHT: f64 = 1.0 / 36.0;
/// The user cost is taken relative to its first-reported-month level and clamped to this ratio before the elasticity:
/// a deep rate cut or price boom cannot drive the anchor to zero, and the factor is continuous.
const UC_RATIO_MIN: f64 = 0.6;
const UC_RATIO_MAX: f64 = 1.6;
/// Monthly weight of the 6-month EMA of the mortgage rate that drives construction financing costs.
const BUILD_RATE_EMA_WEIGHT: f64 = 1.0 / 6.0;
/// Scale on the owner-exit bands (`OWNER_EXIT_BANDS`), calibrated so baseline homeownership stays near the US level
/// now that exits are no longer scaled by the lock-in and market factors.
const OWNER_EXIT_SCALE: f64 = 0.7;
/// Monthly weight with which the rent anchor and the construction norm drift toward the current level (10-year EMA).
const UC_REF_EMA_WEIGHT: f64 = 1.0 / 120.0;
/// Headline expected inflation EMA netted off the real user cost (rents and prices; fix-housing review 10). Its weight
/// matches the mortgage-rate EMA (`UC_EMA_WEIGHT`), so an inflation-matched rate rise leaves the real user cost unchanged.
const UC_PI_HEAD_EMA_WEIGHT: f64 = 1.0 / 24.0;
/// Rent arbitrage speed and buyer-affordability sensitivity during burn-in (master's values). The reported run uses
/// `Config::housing_rent_price_anchor` and `Config::housing_affordability_sens` after the anchor, so burn-in is unchanged.
const BURN_IN_RENT_PRICE_ANCHOR: f64 = 0.08;
const BURN_IN_AFFORDABILITY_SENS: f64 = 0.15;
/// Wage pass-through used during burn-in (master's value). The reported run uses `Config::rent_income_pass`.
const BURN_IN_INCOME_PASS: f64 = 1.0;
/// Index updates need this many observations and move toward the sample median (§10).
const MIN_INDEX_SAMPLES: usize = 5;
const HPI_WEIGHT: f64 = 0.5;
const HPI_HISTORY_MAX: usize = 600;
/// Expected HPA (§10): 0.7 × trailing 12m growth + 0.3 × 3%.
/// Weight on trailing house-price growth in expectations (extrapolation is partial; Case, Shiller & Thompson 2012).
const EHPA_TRAILING_WEIGHT: f64 = 0.25;
const EHPA_LONG_RUN: f64 = 0.03;
/// Construction (§10).
const CONSTRUCTION_QUALITY_SIGMA: f64 = 0.45;
/// Physical cap on starts per month as a share of the housing stock (guards the cubic response).
const START_CAP_SHARE: f64 = 0.02;
/// Withdrawal from the stock (demolition or conversion): an unmortgaged unit that has been vacant and for rent for
/// this many months is withdrawn with this monthly probability. Without it the stock only grows, so starts above
/// household growth leave vacancy rising without limit. US units lost are ≈0.2–0.3M a year (Census/AHS
/// units-lost estimates); the rate is set so withdrawals offset the stock growth that household formation does not
/// absorb.
const DEMOLISH_MIN_VACANT_MONTHS: u16 = 12;
const DEMOLISH_PROB: f64 = 0.01;

/// True during burn-in (`init::build_world`): demography is frozen, so no household dies, ages or enters. Tenure
/// flows that depend on those events are then frozen too (see `bid_for` and `list_plan`).
fn in_burn_in(w: &World) -> bool {
    w.tick_offset == 0 && w.tick < w.cfg.burn_in_months
}

/// Monthly probability for an annual rate (compounding: 1 − (1 − a)^(1/12)).
fn monthly_rate(annual: f64) -> f64 {
    1.0 - (1.0 - annual).powf(1.0 / 12.0)
}

/// Index of the age band (lower-bound table) containing `age`; the first band when none does.
fn band_index(bands: &[(f64, f64)], age: f64) -> usize {
    bands.iter().rposition(|(lo, _)| age >= *lo).unwrap_or(0)
}

/// Monthly hazards of the age bands: `monthly_rate` of each band's annual rate (owner→renter exit listings in
/// `OWNER_EXIT_BANDS`, renter→owner first purchase in `FIRST_BUY_BANDS`). They are piecewise constant in age, so
/// they are computed once per call site and indexed per household instead of calling `powf` for every household.
struct BandHazards {
    owner_exit: [f64; OWNER_EXIT_BANDS.len()],
    first_buy: [f64; FIRST_BUY_BANDS.len()],
}

impl BandHazards {
    fn new() -> Self {
        BandHazards {
            owner_exit: OWNER_EXIT_BANDS.map(|(_, annual)| monthly_rate(annual)),
            first_buy: FIRST_BUY_BANDS.map(|(_, annual)| monthly_rate(annual)),
        }
    }
}

// ---------------------------------------------------------------- state

/// Housing-market state (module-owned; DESIGN §16). Vectors are indexed by household or house id and grown
/// lazily, because the household and house vectors can grow or have slots recycled.
#[derive(Clone, Debug, Default)]
pub struct HousingState {
    /// Rent owed and unpaid per household (nominal $).
    pub arrears: Vec<f64>,
    /// Households actively searching for a rental (they hold no home).
    pub seeking: Vec<bool>,
    /// Per house: the defaulted mortgage the bank holds while the foreclosed house is unsold (REO).
    pub reo_loan: Vec<Option<LoanId>>,
    /// Houses under construction.
    pub projects: Vec<ConstructionProject>,
    /// Construction start ratio (HPI / build cost) at the first month; starts scale with its ratio to this.
    pub start_ratio0: f64,
    /// Builders' vacancy-plus-pipeline rate in the first reported month (the norm starts respond to; 0 until set).
    pub supply_rate0: f64,
    /// Smoothed monthly house-sales count (months-supply denominator).
    pub sales_ema: f64,
    /// Smoothed median owner-occupier bid relative to the HPI (buyer affordability), and its first-month anchor.
    pub bid_ratio: f64,
    pub bid_ratio0: f64,
    /// Steady-state anchors captured at the first month: rental vacancy and months of supply.
    pub natural_rental_vacancy: f64,
    /// True once the natural rental vacancy has been fixed at the first reported month (`rent_vacancy_hvs`).
    pub natural_vacancy_fixed: bool,
    /// Gross rental yield (annual rent / price) at the first month: the own-vs-rent arbitrage anchor.
    pub rent_yield0: f64,
    pub balanced_months_supply: f64,
    /// Consecutive months each house has been vacant and for rent (asking-rent discount).
    pub vacant_months: Vec<u16>,
    /// Months left during which a household that sold its own home is an active (move-up/move-across) buyer.
    pub recent_seller: Vec<u8>,
    /// Per household: months left before a former owner who sold to rent may bid to buy again.
    pub exit_cooldown: Vec<u16>,
    /// Last month's wage index and an exponential average of its annual growth rate (rent income pass-through).
    pub last_wage_index: f64,
    pub income_growth: f64,
    /// fix-housing (review 6, fix 3): slow trend of expected inflation (EMA of `pi_e` over ~10 years), which the rent
    /// equation uses from the anchor. Headline expectations move with oil and food, and rents should not follow them
    /// one for one. `pi_trend_set` once it has a value.
    pub pi_trend: f64,
    pub pi_trend_set: bool,
    /// fix-housing (review 9, fix 2): real rent user cost. Headline expected inflation (EMA over ~6 months), which the
    /// nominal mortgage rate embeds, is netted off the rate and off the appreciation expectation; the appreciation is
    /// held in real terms at its anchor value (`ehpa_real0`). A rate rise that only matches higher inflation then leaves
    /// the rent user cost unchanged. `uc0_rent` is the anchor of that real user cost (0 before the anchor).
    pub pi_head_ema: f64,
    pub pi_head_set: bool,
    pub ehpa_real0: f64,
    pub uc0_rent: f64,
    /// p1-housing: anchor of the price-side user cost, netted by the slow expectation trend `pi_trend` (not the
    /// headline EMA), so a transitory oil or food spike does not lower the real user cost of owning (oil -> prices).
    pub uc0_price: f64,
    /// True from the anchor month on (the post-anchor mechanisms are then live), and the month it was set.
    pub anchored: bool,
    pub anchor_tick: u32,
    /// Idiosyncratic log value deviation of each house (AR(1), see `update_house_values`), and the log of the mean of
    /// its exponential over the houses: subtracting it makes the value multiplier average one across houses, so the
    /// index is not biased by the dispersion. Empty until the first month (every multiplier is then 1).
    pub house_idio: Vec<f64>,
    pub idio_norm: f64,
    /// Per house: the tick until which the owner is in an income-shock episode (0 when none; `update_income_shocks`).
    pub income_shock_until: Vec<u32>,
    /// Per house: the household that started the current episode (`HHID_NONE` when none). An episode belongs to its
    /// owner at the start, so a buyer or heir of the house does not inherit it (p1-housing).
    pub income_shock_owner: Vec<HhId>,
    // fix-housing
    /// EMA of the market mortgage rate (rent anchor's user cost), and the user cost at the first reported month (uc0,
    /// zero until then: the anchor is neutral before it). The EMAs run through burn-in, so they are populated at m0.
    pub uc_mortgage_ema: f64,
    pub uc0: f64,
    /// EMA of expected appreciation in the user cost (slow, so a price boom or bust does not swing the anchor).
    pub uc_ehpa_ema: f64,
    pub uc_ehpa_set: bool,
    /// EMA of the market mortgage rate (construction financing) and its value at the first reported month.
    pub build_rate_ema: f64,
    pub build_rate0: f64,
    /// Log rent-to-star gap at the anchor month (the calibrated level); the price term acts on its deviation.
    pub rent_gap0: f64,
    /// 12-month EMA of the investor margin (see `investor_margin`); `investor_margin_set` once it has a value.
    pub investor_margin_ema: f64,
    pub investor_margin_set: bool,
    /// Houses torn down so far. A demolished house keeps its slot with `completed = false` and no project.
    pub demolished: usize,
    /// Scratch buffer reused across months (no state carried between months).
    scratch_lists: Vec<ListPlan>,
    // b4-rent
    /// Census HVS rental vacancy (fraction) last month, and its 120-month reference from the anchor month.
    pub rental_vac_hvs: f64,
    pub rental_vac_ref: f64,
    /// Months of supply last month (unclamped, as `rental_market` reads it), and its 120-month reference from the anchor.
    pub ms_last: f64,
    pub ms_ref: f64,
}

/// One household's listing decisions for the month. Drawn in parallel from its own stream, applied in household order.
#[derive(Clone, Debug, Default)]
struct ListPlan {
    hh: HhId,
    /// Owner-occupier's home listed, with the ask-noise draw.
    home: Option<(HouseId, f64)>,
    /// Whether that listing starts a buy-sell chain (sellers under retirement age).
    mover: bool,
    /// Whether that listing is an owner leaving ownership for a rental (see `OWNER_EXIT_BANDS`).
    exit: bool,
    /// Investor properties listed, with their ask-noise draws, in property order.
    lets: Vec<(HouseId, f64)>,
}

/// A house under construction by a construction firm (DESIGN §10). Consumes `units_left` from its firm.
#[derive(Clone, Copy, Debug)]
pub struct ConstructionProject {
    pub house: HouseId,
    pub firm: FirmId,
    /// Real construction units still to be delivered.
    pub units_left: f64,
}

/// A household's purchase bid for this month.
#[derive(Clone, Copy, Debug)]
struct Bid {
    hh: HhId,
    price: f64,
    /// Mortgage principal the quote supports at `price` (0 for cash).
    principal: f64,
    quote: Option<MortgageQuote>,
    investor: bool,
}

/// Clear housing bookkeeping for a household slot that demography has recycled (call on death or rebirth).
pub fn reset_household(state: &mut HousingState, h: HhId) {
    let i = h as usize;
    if let Some(a) = state.arrears.get_mut(i) {
        *a = 0.0;
    }
    if let Some(s) = state.seeking.get_mut(i) {
        *s = false;
    }
    if let Some(s) = state.recent_seller.get_mut(i) {
        *s = 0;
    }
    if let Some(s) = state.exit_cooldown.get_mut(i) {
        *s = 0;
    }
}

/// Months of supply: listed houses divided by the smoothed monthly sales count.
pub fn months_supply(w: &World) -> f64 {
    let n = listings_count(w) as f64;
    if n == 0.0 {
        0.0
    } else {
        n / w.housing.sales_ema.max(0.5)
    }
}

/// Number of completed houses currently listed for sale.
pub fn listings_count(w: &World) -> usize {
    w.houses.par_iter().filter(|h| h.for_sale && h.completed).count()
}

/// b4-rent: the post-anchor, reported-run flag of the re-anchored and tenure mechanisms (`live = !in_burn_in && anchored`).
fn b4_live(w: &World) -> bool {
    !in_burn_in(w) && w.housing.anchored
}

/// b4-rent: Census HVS rental vacancy as a fraction, vacant for-rent units ÷ (renter-occupied + vacant for-rent units),
/// counted from the houses. No side effects.
fn hvs_rental_vacancy(w: &World) -> f64 {
    let (vac, rented) = w
        .houses
        .par_iter()
        .filter(|h| h.completed)
        .map(|h| match h.occupant {
            None if h.for_rent => (1usize, 0usize),
            Some(o) if !matches!(h.owner, Owner::Household(x) if x == o) => (0, 1),
            _ => (0, 0),
        })
        .reduce(|| (0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
    vac as f64 / (rented as f64 + vac as f64).max(1.0)
}

/// b4-rent: the master vacant-rental sale hazard at months of supply `x` (`max(0.2·clamp(4/ms−1,0,1), 0.03)`).
fn master_vacant_sell_prob(x: f64) -> f64 {
    (VACANT_SELL_PROB * (BALANCED_MONTHS_SUPPLY / x.max(0.25) - 1.0).clamp(0.0, 1.0)).max(VACANT_SELL_BASE_PROB)
}

/// b4-rent: monthly probability that a household landlord sells a vacant rental (M-V, Greenwald & Guren 2025). Off or not
/// live: the master hazard at `ms`. On: anchored at the 120-month references, `p_leg(M̄)·(M̄/ms)^η·exp(κ_s·100·(v₋₁−v̄))`,
/// clamped to [VSELL_P_MIN, VSELL_P_MAX]. At the references it equals `p_leg(M̄)`, so baseline flows barely move, and the
/// elasticity to months of supply is −η rather than the master's ≈ −4 near ms = 3. κ_s > 0 raises the hazard when rental
/// vacancy is above its reference (landlords sell out of a loose rental market). `ms_ref ≤ 0` (reference not yet set)
/// falls back to the master hazard.
fn vacant_sell_prob(cfg: &crate::config::Config, live: bool, ms: f64, ms_ref: f64, v: f64, v_ref: f64) -> f64 {
    if !cfg.housing_vacant_sell_reanchor || !live || ms_ref <= 0.0 {
        return master_vacant_sell_prob(ms);
    }
    let gap = if v_ref > 0.0 { (cfg.housing_rental_gap_sell_semi * 100.0 * (v - v_ref)).exp() } else { 1.0 };
    let p = master_vacant_sell_prob(ms_ref) * (ms_ref / ms.max(0.25)).powf(cfg.housing_vacant_sell_ms_elast) * gap;
    p.clamp(VSELL_P_MIN, VSELL_P_MAX)
}

/// b4-rent: first-time buyers' bid-hazard multiplier (M-T, Dias & Duarte 2022). `d = 100·(build_rate_ema − build_rate0)` is
/// the 6-month mortgage-rate average above its slow reference in pp; the multiplier is `exp(−β_b·d)` clamped to
/// [BUY_MULT_MIN, BUY_MULT_MAX]. It is 1 when β_b = 0 or before the anchor. No draws.
fn buy_rate_mult(w: &World) -> f64 {
    let beta = w.cfg.housing_buy_rate_semi;
    let st = &w.housing;
    if beta == 0.0 || !b4_live(w) || st.build_rate0 <= 0.0 {
        return 1.0;
    }
    let d = 100.0 * (st.build_rate_ema - st.build_rate0);
    (-beta * d).exp().clamp(BUY_MULT_MIN, BUY_MULT_MAX)
}

/// b4-rent: probability that an expiring unlet vacant listing is let (M-G(b), one-sided). `exp(−κ_e·100·(v₋₁−v̄))` clamped to
/// [EXPIRY_LET_MIN, 1]: 1 when κ_e = 0, when not live, or when the HVS rental vacancy is at or below its reference.
fn expiry_let_prob(cfg: &crate::config::Config, live: bool, v: f64, v_ref: f64) -> f64 {
    if cfg.housing_expiry_let_gap_semi <= 0.0 || !live || v_ref <= 0.0 {
        return 1.0;
    }
    (-cfg.housing_expiry_let_gap_semi * 100.0 * (v - v_ref)).exp().clamp(EXPIRY_LET_MIN, 1.0)
}

// ---------------------------------------------------------------- driver

/// Run this month's housing market (DESIGN §10 steps 1–10, in the order given there).
pub fn step(w: &mut World) {
    ensure_sizes(w);
    update_house_values(w);
    update_income_shocks(w);
    settle_own_rental(w);
    for m in w.housing.recent_seller.iter_mut() {
        *m = m.saturating_sub(1);
    }
    for m in w.housing.exit_cooldown.iter_mut() {
        *m = m.saturating_sub(1);
    }
    rents_and_taxes(w);
    foreclosures(w);
    leave_parents(w);
    update_listings(w);
    offer_idle_units(w);
    list_homes(w);
    let bids = compute_bids(w);
    track_buyer_budgets(w, &bids);
    let ppq = match_bids(w, bids);
    let mut rent_per_q = Vec::new();
    rental_market(w, &mut rent_per_q);
    let renters = refinance(w);
    update_indices_with(w, &ppq, &rent_per_q, renters);
    demolish(w);
    construction(w);
    let sales = w.flows.house_sales as f64;
    w.housing.sales_ema = 0.9 * w.housing.sales_ema + 0.1 * sales;
}

/// A tenant who comes to own the home they rent (their landlord died and they inherited it) is an owner-occupier from
/// that month: the tenancy and its rent end, and the house is not let. Tenure and ownership then agree, as
/// `check_invariants` requires.
fn settle_own_rental(w: &mut World) {
    for i in 0..w.households.len() {
        let hh = &w.households[i];
        let Some(hm) = hh.home else { continue };
        if !hh.alive || hh.tenure != Tenure::Renter || w.houses[hm as usize].owner != Owner::Household(i as HhId) {
            continue;
        }
        let hh = &mut w.households[i];
        hh.tenure = Tenure::Owner;
        hh.rent = 0.0;
        hh.lease_months_left = 0;
        w.housing.arrears[i] = 0.0;
        w.housing.seeking[i] = false;
        let hs = &mut w.houses[hm as usize];
        hs.for_rent = false;
        hs.rent = 0.0;
    }
}

// ---------------------------------------------------------------- helpers

/// Grow the per-household and per-house vectors to the current population sizes.
fn ensure_sizes(w: &mut World) {
    let nh = w.households.len();
    let nho = w.houses.len();
    let st = &mut w.housing;
    if st.arrears.len() < nh {
        st.arrears.resize(nh, 0.0);
    }
    if st.seeking.len() < nh {
        st.seeking.resize(nh, false);
    }
    if st.recent_seller.len() < nh {
        st.recent_seller.resize(nh, 0);
    }
    if st.exit_cooldown.len() < nh {
        st.exit_cooldown.resize(nh, 0);
    }
    if st.reo_loan.len() < nho {
        st.reo_loan.resize(nho, None);
    }
    if st.vacant_months.len() < nho {
        st.vacant_months.resize(nho, 0);
    }
    if st.income_shock_until.len() < nho {
        st.income_shock_until.resize(nho, 0);
    }
    if st.income_shock_owner.len() < nho {
        st.income_shock_owner.resize(nho, HHID_NONE);
    }
}

/// Sentinel for "no household" in per-house household ids.
pub const HHID_NONE: HhId = HhId::MAX;

/// True when house `house` is in an income-shock episode that `h` started (i.e. `h` is the owner it was started for).
pub fn income_shock_active(w: &World, house: HouseId, h: HhId) -> bool {
    let hu = house as usize;
    w.housing.income_shock_until.get(hu).is_some_and(|&u| u > w.tick)
        && w.housing.income_shock_owner.get(hu).is_some_and(|&o| o == h)
}

/// Deterministic per-agent stream for one housing decision (DESIGN §15, §16).
#[inline]
fn rng_for(w: &World, phase: u64, id: u64) -> SimRng {
    rng::agent_rng(w.seed, w.tick, stream::HOUSING, (phase << 40) | id)
}

/// Mortgage rate used when no quote exists: the bank's posted rate, or 30y yield plus spread before banks set rates.
fn mortgage_rate_of(w: &World, bank: BankId) -> f64 {
    let r = w.banks.get(bank as usize).map_or(0.0, |b| b.mortgage_rate);
    if r > 0.0 {
        r
    } else {
        w.curve.yield_at(360.0) + w.cfg.mortgage_spread
    }
}

/// fix-housing: market mortgage rate (annual): the mean posted rate of the live banks, or the fallback rate.
fn market_mortgage_rate(w: &World) -> f64 {
    let (sum, n) = w
        .banks
        .iter()
        .filter(|b| b.alive && b.mortgage_rate.is_finite() && b.mortgage_rate > 0.0)
        .fold((0.0, 0usize), |(s, n), b| (s + b.mortgage_rate, n + 1));
    if n > 0 {
        sum / n as f64
    } else {
        mortgage_rate_of(w, 0)
    }
}

/// fix-housing: rent-anchor factor (uc / uc0)^theta with the ratio clamped to [UC_RATIO_MIN, UC_RATIO_MAX], so the
/// factor is continuous and bounded (a non-positive user cost sits on the floor). 1 until uc0 is set.
fn user_cost_factor(uc: f64, uc0: f64, theta: f64) -> f64 {
    if uc0 > 0.0 {
        (uc / uc0).clamp(UC_RATIO_MIN, UC_RATIO_MAX).powf(theta)
    } else {
        1.0
    }
}

/// Expected house-price appreciation (§10): 0.7 × trailing 12m HPI growth + 0.3 × 3%.
pub fn expected_hpa(w: &World) -> f64 {
    let hist = &w.prices.hpi_history;
    let n = hist.len();
    let trailing = if n >= 13 && hist[n - 13] > 0.0 {
        hist[n - 1] / hist[n - 13] - 1.0
    } else {
        0.0
    };
    let e = EHPA_TRAILING_WEIGHT * trailing + (1.0 - EHPA_TRAILING_WEIGHT) * EHPA_LONG_RUN;
    if e.is_finite() {
        e.clamp(-0.03, 0.08)
    } else {
        EHPA_LONG_RUN
    }
}

/// True when `x` is finite and strictly positive (NaN-safe guard).
#[inline]
fn pos(x: f64) -> bool {
    x.is_finite() && x > 0.0
}

/// Value multiplier of one house from its idiosyncratic deviation: exp(deviation) normalised to average one over the
/// houses (`HousingState::idio_norm`). 1 before the deviations exist.
#[inline]
fn idio_mult(st: &HousingState, hid: usize) -> f64 {
    st.house_idio.get(hid).map_or(1.0, |x| (x - st.idio_norm).exp())
}

/// Value of a house to its owner, lender and seller: the index value times its idiosyncratic multiplier (Case & Shiller
/// 1989; Giacoletti 2021). This is the value used for equity, LTV, sale reservation and listing prices and bank collateral.
/// The aggregate index (`common::house_value`) is unchanged on average, since the multiplier averages one.
pub fn house_value_i(w: &World, house: HouseId) -> f64 {
    common::house_value(w, house) * idio_mult(&w.housing, house as usize)
}

/// Draw each house's starting deviation from its stationary distribution, keyed by house id (`stream::HOUSE_VALUE`).
/// Houses are created at initialisation, so the vector only grows if the house count does.
fn ensure_idio(w: &mut World) {
    let nho = w.houses.len();
    if w.housing.house_idio.len() >= nho {
        return;
    }
    let rho = w.cfg.housing_idio_persistence.clamp(0.0, 0.9999);
    let stat_sd = w.cfg.housing_idio_innovation_sd.max(0.0) / (1.0 - rho * rho).sqrt();
    let seed = w.seed;
    let st = &mut w.housing;
    for hid in st.house_idio.len()..nho {
        let mut r = rng::agent_rng(seed, 0, stream::HOUSE_VALUE, hid as u64);
        st.house_idio.push(stat_sd * rng::normal(&mut r));
    }
}

/// Income-shock episodes (after the anchor): an owner-occupier with a mortgage above `housing_shock_ltv` starts an
/// episode with monthly probability `housing_shock_hazard` × (u / u_natural)^`housing_shock_u_elasticity`, drawn on the
/// house's own `INCOME_SHOCK` stream. The hazard rises with unemployment, so episodes cluster in recessions (review 10).
/// During an episode the owner's payments are missed with the episode miss probability (banking.rs). With liquidity
/// grading off (the shipped default) that probability does not depend on liquidity, so the channel is LTV-gated but not
/// concentrated among illiquid owners; the Ganong & Noel 2023 gradient needs grading switched on, and then it binds only
/// weakly at realistic thresholds.
fn update_income_shocks(w: &mut World) {
    ensure_sizes(w);
    let base_hazard = if w.housing.anchored { w.cfg.housing_shock_hazard.clamp(0.0, 1.0) } else { 0.0 };
    if base_hazard <= 0.0 {
        return;
    }
    // Labour-market stress multiplier, relative to the natural rate and bounded so a deep recession cannot saturate it.
    let u_nat = w.natural_unemployment().max(1e-3);
    let stress = (w.unemployment_rate() / u_nat).clamp(0.0, 5.0).powf(w.cfg.housing_shock_u_elasticity.max(0.0));
    let hazard = (base_hazard * stress).min(1.0);
    let (seed, tick) = (w.seed, w.tick);
    let ltv_min = w.cfg.housing_shock_ltv;
    let underwater_mult = w.cfg.housing_shock_underwater_mult.max(1.0);
    let episode = w.cfg.housing_shock_episode_months.max(1);
    for hid in 0..w.houses.len() {
        let hs = &w.houses[hid];
        let Owner::Household(h) = hs.owner else { continue };
        if !hs.completed || hs.occupant != Some(h) {
            continue;
        }
        let Some(lid) = hs.mortgage else { continue };
        let Some(l) = w.loans.get(lid) else { continue };
        let v = house_value_i(w, hid as HouseId);
        if !(v > 0.0) || !(l.principal / v > ltv_min) {
            continue;
        }
        // Negative equity raises the hazard (Foote, Gerardi & Willen 2008: an underwater owner is far more likely to stop
        // paying than one with equity).
        let hz = if l.principal > v { (hazard * underwater_mult).min(1.0) } else { hazard };
        let mut r = rng::agent_rng(seed, tick, stream::INCOME_SHOCK, hid as u64);
        if r.gen::<f64>() < hz {
            w.housing.income_shock_until[hid] = tick.saturating_add(episode);
            w.housing.income_shock_owner[hid] = h;
        }
    }
}

/// Monthly update of the idiosyncratic house values: an AR(1) per house with independent innovations, keyed by (tick,
/// house id), so a shocked clone and its baseline draw the same innovations. The deviations are then demeaned across
/// houses (so the index is not moved by them) and the normaliser is recomputed.
fn update_house_values(w: &mut World) {
    ensure_idio(w);
    let rho = w.cfg.housing_idio_persistence.clamp(0.0, 0.9999);
    let sd = w.cfg.housing_idio_innovation_sd.max(0.0);
    if sd == 0.0 {
        w.housing.house_idio.iter_mut().for_each(|x| *x = 0.0);
        w.housing.idio_norm = 0.0;
        return;
    }
    let (seed, tick) = (w.seed, w.tick);
    let st = &mut w.housing;
    st.house_idio.par_iter_mut().enumerate().for_each(|(hid, x)| {
        let mut r = rng::agent_rng(seed, tick, stream::HOUSE_VALUE, hid as u64);
        *x = rho * *x + sd * rng::normal(&mut r);
    });
    let n = st.house_idio.len().max(1) as f64;
    let mean = st.house_idio.iter().sum::<f64>() / n;
    st.house_idio.iter_mut().for_each(|x| *x -= mean);
    let mean_exp = st.house_idio.iter().map(|x| x.exp()).sum::<f64>() / n;
    st.idio_norm = mean_exp.max(1e-300).ln();
}

/// Median of a slice (sorts it in place). NaN for an empty slice.
fn median(v: &mut [f64]) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Pick the better of two (value, index) candidates; ties keep the first.
#[inline]
fn better(a: (f64, usize), b: (f64, usize)) -> (f64, usize) {
    if b.0 > a.0 {
        b
    } else {
        a
    }
}

/// Max segment tree over items in sorted order: prefix-maximum queries and point removal in O(log n).
struct MaxTree {
    size: usize,
    nodes: Vec<(f64, usize)>,
}

impl MaxTree {
    fn new(vals: &[f64]) -> Self {
        let size = vals.len().max(1).next_power_of_two();
        let mut nodes = vec![(f64::NEG_INFINITY, usize::MAX); 2 * size];
        for (i, &v) in vals.iter().enumerate() {
            nodes[size + i] = (v, i);
        }
        for i in (1..size).rev() {
            nodes[i] = better(nodes[2 * i], nodes[2 * i + 1]);
        }
        MaxTree { size, nodes }
    }

    fn remove(&mut self, i: usize) {
        let mut p = self.size + i;
        self.nodes[p] = (f64::NEG_INFINITY, usize::MAX);
        p /= 2;
        while p > 0 {
            self.nodes[p] = better(self.nodes[2 * p], self.nodes[2 * p + 1]);
            p /= 2;
        }
    }

    /// Index of the best (largest value) live item among positions `0..k`.
    fn query_prefix(&self, k: usize) -> Option<usize> {
        let mut best = (f64::NEG_INFINITY, usize::MAX);
        let mut l = self.size;
        let mut r = self.size + k;
        while l < r {
            if l & 1 == 1 {
                best = better(best, self.nodes[l]);
                l += 1;
            }
            if r & 1 == 1 {
                r -= 1;
                best = better(best, self.nodes[r]);
            }
            l >>= 1;
            r >>= 1;
        }
        if best.0.is_finite() {
            Some(best.1)
        } else {
            None
        }
    }
}

/// The occupant of `hid` leaves: the house becomes vacant (for rent unless it is for sale) and the household
/// becomes a renter-seeker with no home. Used for eviction, foreclosure and sale.
fn release_occupant(w: &mut World, hid: HouseId) {
    ensure_sizes(w);
    let hu = hid as usize;
    let Some(o) = w.houses[hu].occupant.take() else {
        return;
    };
    w.houses[hu].for_rent = !w.houses[hu].for_sale;
    let ou = o as usize;
    let hh = &mut w.households[ou];
    if hh.home == Some(hid) {
        hh.home = None;
    }
    hh.tenure = Tenure::WithParents;
    hh.rent = 0.0;
    hh.lease_months_left = 0;
    w.housing.seeking[ou] = true;
    w.housing.arrears[ou] = 0.0;
}

// ---------------------------------------------------------------- 1. rents, taxes, leases

/// Rent collection with arrears and eviction, lease countdown and renewal, property tax, and the committed
/// monthly housing cost `hh.housing_cost` (DESIGN §10 step 1, §16).
///
/// Property tax and the scheduled mortgage payment depend only on house values, property lists and loans, none of
/// which change during this step. Pass 1 (parallel, fixed chunks) debits every tenant's rent and every owner's
/// property tax from its own deposits, and finishes the housing cost of every household that does not pay rent.
/// Rent credits to household landlords are then applied in parallel per landlord chunk (pass 1b), in tenant order
/// within each bucket. Only the renters' evictions and lease steps, which touch shared house records, run in
/// household order (pass 2).
fn rents_and_taxes(w: &mut World) {
    ensure_sizes(w);
    let tax_rate = w.cfg.property_tax / 12.0;
    let nh = w.households.len();
    let nb = w.banks.len();
    let nchunks = nh.div_ceil(RENT_CHUNK);
    let hpi = w.prices.hpi;
    // Pass 1, parallel over fixed chunks. Per-chunk bank deposit/reserve deltas, tax, renters and rent credits are
    // reduced in chunk order, so the result is independent of the thread count.
    let parts: Vec<RentPart> = {
        let houses = &w.houses;
        let loans = &w.loans;
        let households = &mut w.households[..nh];
        let arrears = &mut w.housing.arrears[..nh];
        let seeking = &mut w.housing.seeking[..nh];
        households
            .par_chunks_mut(RENT_CHUNK)
            .zip(arrears.par_chunks_mut(RENT_CHUNK))
            .zip(seeking.par_chunks_mut(RENT_CHUNK))
            .enumerate()
            .map(|(c, ((hhs, arrs), seeks))| {
                let mut part = RentPart {
                    bank: vec![0.0; nb],
                    tax: 0.0,
                    renters: Vec::new(),
                    credits: vec![Vec::new(); nchunks],
                    external: Vec::new(),
                };
                for (k, ((hh, arr), seek)) in hhs.iter_mut().zip(arrs.iter_mut()).zip(seeks.iter_mut()).enumerate() {
                    let i = c * RENT_CHUNK + k;
                    if !hh.alive {
                        *arr = 0.0;
                        *seek = false;
                        hh.housing_cost = 0.0;
                        continue;
                    }
                    // Same arithmetic as `common::house_value(w, p) * tax_rate` (assessments use the index, not the idiosyncratic value).
                    let mut tax_due = 0.0;
                    for &p in &hh.properties {
                        tax_due += hpi * houses[p as usize].quality * tax_rate;
                    }
                    // Scheduled mortgage payment on the home (banking::service collects it).
                    let mut mpmt = 0.0;
                    if hh.tenure == Tenure::Owner {
                        if let Some(hid) = hh.home {
                            if let Some(l) = houses[hid as usize].mortgage.and_then(|l| loans.get(l)) {
                                if l.borrower == Borrower::Household(i as HhId) {
                                    mpmt = l.payment;
                                }
                            }
                        }
                    }
                    if let (true, Some(hid)) = (hh.tenure == Tenure::Renter, hh.home) {
                        let landlord = houses[hid as usize].owner;
                        let own = landlord == Owner::Household(i as HhId);
                        let owed = *arr + hh.rent;
                        // Same as `ledger::pay_up_to` from the tenant (never overdrawing).
                        let rent_paid = if own {
                            owed
                        } else {
                            let x = owed.min(hh.deposits.max(0.0));
                            if x > 0.0 && x.is_finite() {
                                hh.deposits -= x;
                                part.bank[hh.bank as usize] -= x;
                                x
                            } else {
                                0.0
                            }
                        };
                        *arr = (owed - rent_paid).max(0.0);
                        // A renter who stays with the lease running on (no eviction, no expiry) only counts it down:
                        // its own record, so it is done here. Evictions and expiries go to the sequential pass.
                        let rent = hh.rent;
                        let evict = rent > 0.0 && *arr >= EVICTION_MONTHS_OWED * rent;
                        if !evict && hh.lease_months_left.saturating_sub(1) > 0 {
                            hh.lease_months_left -= 1;
                            hh.housing_cost = mpmt + rent + tax_due;
                        } else {
                            part.renters.push((i as HhId, hid, tax_due, mpmt));
                        }
                        // A household renting its own house is treated as paid and credits nobody.
                        if !own && rent_paid > 0.0 && rent_paid.is_finite() {
                            match landlord {
                                Owner::Household(l) => part.credits[l as usize / RENT_CHUNK].push((l, rent_paid)),
                                other => part.external.push((other, rent_paid)),
                            }
                        }
                    } else {
                        // No rent this month: committed = 0.
                        hh.housing_cost = mpmt + tax_due;
                    }
                    // Property tax on every owned property (DESIGN §10: 1.1%/yr of value, paid monthly).
                    let t = tax_due.min(hh.deposits.max(0.0));
                    if t > 0.0 && t.is_finite() {
                        hh.deposits -= t;
                        part.bank[hh.bank as usize] -= t;
                        part.tax += t;
                    }
                }
                part
            })
            .collect()
    };
    // Pass 1b, parallel over the same chunks: each landlord chunk receives its bucket of rent from every tenant
    // chunk, in tenant order. The bank side (deposits and reserves) is reduced per chunk, in chunk order.
    let credit_banks: Vec<Vec<f64>> = {
        let parts = &parts;
        w.households[..nh]
            .par_chunks_mut(RENT_CHUNK)
            .zip(w.hh_month[..nh].par_chunks_mut(RENT_CHUNK))
            .enumerate()
            .map(|(t, (hhs, mons))| {
                let base = t * RENT_CHUNK;
                let mut bank = vec![0.0; nb];
                for part in parts {
                    for &(l, paid) in &part.credits[t] {
                        let hh = &mut hhs[l as usize - base];
                        let mon = &mut mons[l as usize - base];
                        hh.deposits += paid;
                        mon.income.rent_received += paid;
                        bank[hh.bank as usize] += paid;
                    }
                }
                bank
            })
            .collect()
    };
    let mut bank_delta = vec![0.0; nb];
    let mut tax_total = 0.0;
    for part in &parts {
        for (a, x) in bank_delta.iter_mut().zip(&part.bank) {
            *a += x;
        }
        tax_total += part.tax;
    }
    for bank in &credit_banks {
        for (a, x) in bank_delta.iter_mut().zip(bank) {
            *a += x;
        }
    }
    for (b, &x) in bank_delta.iter().enumerate() {
        if x != 0.0 {
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
    }
    w.cb.tga += tax_total;
    w.gov.revenue += tax_total;
    for part in &parts {
        for &(owner, paid) in &part.external {
            credit_external(w, owner, paid);
        }
    }

    // Pass 2, in household order: evictions and lease expiries (renters only; see pass 1).
    for part in &parts {
        for &(h, hid, tax_due, mpmt) in &part.renters {
            let i = h as usize;
            let rent = w.households[i].rent;
            let mut committed = 0.0;
            if rent > 0.0 && w.housing.arrears[i] >= EVICTION_MONTHS_OWED * rent {
                w.flows.evictions += 1;
                release_occupant(w, hid);
            } else {
                committed = rent;
                lease_step(w, h, hid);
            }
            w.households[i].housing_cost = mpmt + committed + tax_due;
        }
    }
}

/// Per-chunk output of the parallel pass in `rents_and_taxes`.
struct RentPart {
    /// Bank deposit and reserve change from this chunk's debits (rent and tax).
    bank: Vec<f64>,
    /// Property tax collected.
    tax: f64,
    /// Renters with a home whose eviction or lease expiry is decided in pass 2, in household order:
    /// (household, home, property tax due, mortgage payment).
    renters: Vec<(HhId, HouseId, f64, f64)>,
    /// Rent credits to household landlords, bucketed by the landlord's chunk: (landlord, amount), in tenant order.
    credits: Vec<Vec<(HhId, f64)>>,
    /// Rent credits to firm and bank landlords, in tenant order.
    external: Vec<(Owner, f64)>,
}

/// Credit rent already debited from a tenant to a firm or bank landlord (the receiving half of `collect_rent`).
/// Household landlords are credited in the parallel pass of `rents_and_taxes`.
fn credit_external(w: &mut World, landlord: Owner, paid: f64) {
    match landlord {
        Owner::Firm(f) => {
            let fm = &mut w.firms[f as usize];
            fm.deposits += paid;
            fm.revenue += paid;
            let b = fm.bank as usize;
            w.banks[b].deposits += paid;
            w.banks[b].reserves += paid;
        }
        Owner::Bank(b) => w.banks[b as usize].reserves += paid,
        Owner::Household(_) => {}
    }
}

/// Lease countdown for a renter. At expiry: 20% chance the renter moves out (becomes a seeker); otherwise renew at
/// max(current rent, rent index × quality), capped at +10%.
fn lease_step(w: &mut World, h: HhId, hid: HouseId) {
    let i = h as usize;
    let left = w.households[i].lease_months_left.saturating_sub(1);
    if left > 0 {
        w.households[i].lease_months_left = left;
        return;
    }
    let mut r = rng_for(w, PH_LEASE, h as u64);
    let u: f64 = r.gen();
    if u < LEASE_EXIT_PROB {
        release_occupant(w, hid);
        return;
    }
    let cur = w.households[i].rent;
    let market = w.prices.rent_index * w.houses[hid as usize].quality;
    let new = if cur > 0.0 {
        market.max(cur).min(cur * LEASE_MAX_INCREASE)
    } else {
        market
    };
    w.households[i].rent = new;
    w.households[i].lease_months_left = LEASE_MONTHS as u16;
    w.houses[hid as usize].rent = new;
}

// ---------------------------------------------------------------- 2. foreclosures

/// Round 7 step 7: no foreclosure starts during burn-in when `burn_in_freeze_foreclosures` is set (the frozen pre-sample
/// does not record history). Otherwise, before the anchor (burn-in), master's rule: at least 3 months delinquent and
/// unemployed or LTV > 0.9, foreclosed with a monthly draw of FORECLOSURE_START_PROB.
///
/// After the anchor (fix-housing), delinquent borrowers (at least 3 months) take one of three paths, a month at a time:
/// - foreclosure, when they are in negative equity after selling costs and arrears (value × (1 − SELL_COST) − principal −
///   months delinquent × payment < 0), with probability `housing_foreclosure_prob`. Employment does not enter: this is
///   the double trigger (Foote, Gerardi & Willen 2008; Ganong & Noel 2023). The bank repossesses;
/// - a distressed sale, when they still have equity after those costs: the house is listed, and when it is the
///   household's own home the owner leaves ownership at listing (as an owner exit). The sale repays the loan. Probability
///   `housing_distress_sale_prob` a month. A let unit is a plain distressed listing (review 6);
/// - otherwise the delinquency cures with DELINQUENCY_CURE_PROB a month.
/// A listed house is not foreclosed on in the meantime: its sale repays the loan.
fn foreclosures(w: &mut World) {
    ensure_sizes(w);
    if w.cfg.burn_in_freeze_foreclosures && in_burn_in(w) {
        return;
    }
    let post_anchor = !in_burn_in(w) && w.housing.uc0 > 0.0;
    // Months since the anchor. A delinquency that began before the anchor (burn-in, under master's rule) has more months
    // owed than this, and is not subject to the post-anchor paths: it only cures. Without this the burn-in stock is
    // foreclosed in one burst in the first months after the anchor.
    let since_anchor = if w.housing.anchored { w.tick.saturating_sub(w.housing.anchor_tick) as u64 + 1 } else { 0 };
    let ramp = (since_anchor as f64 / FORECLOSURE_RAMP_MONTHS).min(1.0);
    let mut cands: Vec<(LoanId, HouseId)> = Vec::new();
    let mut cures: Vec<LoanId> = Vec::new();
    let mut sales: Vec<(HouseId, HhId, f64)> = Vec::new();
    for (lid, l) in w.loans.iter() {
        let LoanKind::Mortgage { house } = l.kind else {
            continue;
        };
        let Borrower::Household(h) = l.borrower else {
            continue;
        };
        if l.months_delinquent == 0 {
            continue;
        }
        let hh = &w.households[h as usize];
        if !hh.alive || w.houses[house as usize].owner != Owner::Household(h) {
            continue;
        }
        if w.housing.reo_loan[house as usize].is_some() {
            continue;
        }
        // After the anchor a listed house is being sold, and its sale repays the loan (no foreclosure meanwhile).
        if post_anchor && w.houses[house as usize].for_sale {
            continue;
        }
        let v = house_value_i(w, house);
        let ltv = if v > 0.0 { l.principal / v } else { f64::INFINITY };
        let delinquent = l.months_delinquent >= FORECLOSURE_DELINQUENT_MONTHS;
        let unemployed = hh.status == LaborStatus::Unemployed;
        let legacy = l.months_delinquent as u64 > since_anchor;
        if post_anchor {
            if delinquent && !legacy {
                // Double trigger (Foote, Gerardi & Willen 2008; Ganong & Noel 2023): a delinquent borrower who still
                // has equity after selling costs and the arrears owed sells the house (whatever their employment);
                // only a delinquent borrower who is underwater on that test is foreclosed.
                let payment = if l.payment.is_finite() { l.payment.max(0.0) } else { 0.0 };
                let arrears = l.months_delinquent as f64 * payment;
                let net_equity = v * (1.0 - SELL_COST) - l.principal - arrears;
                if net_equity < 0.0 {
                    let mut r = rng::agent_rng(w.seed, w.tick, stream::HOUSING, (PH_FORECLOSE << 40) | house as u64);
                    if r.gen::<f64>() < w.cfg.housing_foreclosure_prob * ramp {
                        cands.push((lid, house));
                        continue;
                    }
                } else {
                    let mut r = rng::agent_rng(w.seed, w.tick, stream::HOUSING, (PH_DISTRESS << 40) | house as u64);
                    if r.gen::<f64>() < w.cfg.housing_distress_sale_prob {
                        let eps = rng::normal(&mut r);
                        sales.push((house, h, eps));
                        continue;
                    }
                }
            }
            let mut r = rng::agent_rng(w.seed, w.tick, stream::HOUSING, (PH_CURE << 40) | lid as u64);
            if r.gen::<f64>() < DELINQUENCY_CURE_PROB {
                cures.push(lid);
            }
        } else if delinquent && (unemployed || ltv > FORECLOSURE_LTV) {
            let mut r = rng::agent_rng(w.seed, w.tick, stream::HOUSING, (PH_FORECLOSE << 40) | house as u64);
            if r.gen::<f64>() < FORECLOSURE_START_PROB {
                cands.push((lid, house));
            }
        }
    }
    for lid in cures {
        if let Some(l) = w.loans.get_mut(lid) {
            l.months_delinquent = 0;
        }
    }
    for (house, h, eps) in sales {
        list_house_at(w, house, eps);
        // Only the household's own home is an owner exit. A let unit is a plain distressed listing, and its owner keeps
        // the home they live in (fix-housing review 6).
        if w.households[h as usize].home == Some(house) {
            exit_at_listing(w, house, h);
        }
    }
    for (lid, house) in cands {
        foreclose(w, lid, house);
    }
}

/// The bank takes the house, the borrower leaves, and the bank lists it at 0.85 × value. The loan stays on the
/// books until the house sells; the write-off is then net of sale proceeds.
fn foreclose(w: &mut World, loan: LoanId, house: HouseId) {
    ensure_sizes(w);
    let Some(l) = w.loans.get(loan) else {
        return;
    };
    let bank = l.bank;
    // Read before the borrower is moved out: an owner-occupied foreclosure is one where the borrower lives in the house.
    let owner_occ = match l.borrower {
        Borrower::Household(b) => w.households.get(b as usize).is_some_and(|h| h.home == Some(house)),
        Borrower::Firm(_) => false,
    };
    let v = house_value_i(w, house);
    release_occupant(w, house);
    ledger::transfer_house(w, house, Owner::Bank(bank));
    let hs = &mut w.houses[house as usize];
    hs.for_sale = true;
    hs.for_rent = false;
    hs.ask_price = REO_ASK_FACTOR * v;
    hs.months_listed = 0;
    w.housing.reo_loan[house as usize] = Some(loan);
    w.flows.foreclosures += 1;
    if owner_occ {
        w.flows.foreclosures_owner_occ += 1;
    }
}

// ---------------------------------------------------------------- 3. leaving parents

/// WithParents households aged ≥ 20 start a rental search with monthly probability 1%, plus 2% if employed and
/// plus 3% if aged ≥ 25 (DESIGN §16).
fn leave_parents(w: &mut World) {
    ensure_sizes(w);
    let nh = w.households.len();
    let households = &w.households;
    let (seed, tick) = (w.seed, w.tick);
    w.housing.seeking[..nh].par_iter_mut().enumerate().for_each(|(i, seeking)| {
        let hh = &households[i];
        if !hh.alive || hh.tenure != Tenure::WithParents || *seeking || hh.age_years() < 19.0 {
            return;
        }
        // Household formation: median move-out age ≈ 21–22 for the potential household heads modelled here.
        let mut p = 0.02;
        if hh.status == LaborStatus::Employed {
            p += 0.04;
        }
        if hh.age_years() >= 22.0 {
            p += 0.04;
        }
        // Same key as `rng_for(w, PH_LEAVE, i)`.
        let mut r = rng::agent_rng(seed, tick, stream::HOUSING, (PH_LEAVE << 40) | i as u64);
        let u: f64 = r.gen();
        if u < p {
            *seeking = true;
        }
    });
}

// ---------------------------------------------------------------- 4. listings

/// Age existing listings: the ask decays 2.5% a month, and a listing is withdrawn after 9 months.
fn update_listings(w: &mut World) {
    ensure_sizes(w);
    // Sellers cut asks faster in a slack market: the monthly cut is ASK_DECAY^(1 + ASK_SLACK_SENS × slack), where
    // slack = ln(months of supply / NORMAL_MONTHS_SUPPLY)⁺. Sellers who see many listings and few sales lower their
    // prices sooner (Genesove & Mayer 2001: price cuts and time on market rise with the stock of sellers), so part of
    // a demand fall shows up as lower prices rather than only as fewer sales. In the normal state the cut is 3%/month.
    let ms = months_supply(w).max(0.25);
    let slack = (ms / NORMAL_MONTHS_SUPPLY).ln().max(0.0);
    let decay = ASK_DECAY.powf(1.0 + ASK_SLACK_SENS * slack);
    // b4-rent (M-G(b)): an unlet vacant listing that expires goes to rent with probability `p_let`, and is otherwise
    // re-listed for sale. Only the reported run with κ_e > 0 draws (PH_EXPIRE); otherwise every expiry is let, as in master.
    let live = b4_live(w);
    let route = w.cfg.housing_expiry_let_gap_semi > 0.0 && live;
    let p_let = expiry_let_prob(&w.cfg, live, w.housing.rental_vac_hvs, w.housing.rental_vac_ref);
    let (seed, tick) = (w.seed, w.tick);
    let hpi = w.prices.hpi;
    let idio = &w.housing.house_idio;
    let norm = w.housing.idio_norm;
    // Each house is aged on its own record only, so the houses are processed in parallel.
    w.houses.par_iter_mut().enumerate().for_each(|(i, hs)| {
        if !hs.for_sale {
            return;
        }
        hs.months_listed += 1;
        hs.ask_price *= decay;
        if hs.months_listed >= LISTING_LIFE_MONTHS {
            hs.for_sale = false;
            hs.ask_price = 0.0;
            hs.months_listed = 0;
            if hs.occupant.is_none() {
                if route {
                    let mut r = rng::agent_rng(seed, tick, stream::HOUSING, (PH_EXPIRE << 40) | i as u64);
                    if r.gen::<f64>() >= p_let {
                        let eps = rng::normal(&mut r);
                        let mult = idio.get(i).map_or(1.0, |x| (x - norm).exp());
                        list_at(hs, hpi * hs.quality * mult, eps);
                        return;
                    }
                }
                hs.for_rent = true;
            }
        }
    });
}

/// Owner-to-renter exit takes effect at listing (fix d): the seller moves out when the home goes on the market, becomes a
/// renter at once and seeks a rental (so rental demand rises as ownership falls). A house still occupied by the seller
/// is vacated; a sale later only closes the loan. The rebuy cooldown also starts at listing.
fn exit_at_listing(w: &mut World, hid: HouseId, s: HhId) {
    if w.houses[hid as usize].occupant != Some(s) {
        return;
    }
    release_occupant(w, hid);
    let su = s as usize;
    w.households[su].tenure = Tenure::Renter;
    w.housing.exit_cooldown[su] = EXIT_REBUY_COOLDOWN_MONTHS;
}

/// New listings: owner-occupiers (a move at 0.28% a month under 75, and an exit to renting at the age-band rate in
/// `OWNER_EXIT_BANDS`), and investors' non-home properties at the smoothed-margin sale probability (`investor_sell_prob`).
///
/// Each household's decisions read only its own stream and the state at the start of the step (a house has one
/// owner, so no two households touch the same house). They are computed in parallel and applied in household order.
fn list_homes(w: &mut World) {
    let gross_yield = w.prices.rent_index * 12.0 / w.prices.hpi.max(1e-9);
    // Loss aversion / market timing (Genesove & Mayer 2001): fewer owners list into a glutted market, more into a
    // tight one.
    let ms = months_supply(w).max(0.25);
    let market = (BALANCED_MONTHS_SUPPLY / ms).sqrt().clamp(0.4, 1.5);
    // fix-housing: investors sell on the smoothed margin, and the sale probability is the same for all of them.
    let margin = investor_margin(w, gross_yield);
    let st = &mut w.housing;
    if st.investor_margin_set {
        st.investor_margin_ema += INVESTOR_MARGIN_EMA_WEIGHT * (margin - st.investor_margin_ema);
    } else {
        st.investor_margin_ema = margin;
        st.investor_margin_set = true;
    }
    let sell_prob = investor_sell_prob(st.investor_margin_ema);
    let exit_mult = owner_exit_uc_factor(w);
    let hz = BandHazards::new();
    let mut plans = std::mem::take(&mut w.housing.scratch_lists);
    {
        let ww: &World = w;
        plans.clear();
        plans.par_extend(
            (0..ww.households.len())
                .into_par_iter()
                .filter_map(|i| list_plan(ww, i, sell_prob, market, exit_mult, &hz)),
        );
    }
    for plan in &plans {
        if let Some((hid, eps)) = plan.home {
            list_house_at(w, hid, eps);
            if plan.exit {
                exit_at_listing(w, hid, plan.hh);
            }
            if plan.mover {
                // Most sellers are moving within the market: they search for the next home at once
                // (buy-sell chain), bridging until the old home sells.
                w.housing.recent_seller[plan.hh as usize] = MOVER_MONTHS;
            }
        }
        for &(hid, eps) in &plan.lets {
            list_house_at(w, hid, eps);
        }
    }
    w.housing.scratch_lists = plans;
}

/// One household's listing decisions this month (None when it does nothing). Draw order within the household's
/// stream matches the sequential loop: the home test, then one test per investor property, each listing followed by
/// its ask-noise draw.
fn list_plan(w: &World, i: usize, sell_prob: f64, market: f64, exit_mult: f64, hz: &BandHazards) -> Option<ListPlan> {
    let hh = &w.households[i];
    if !hh.alive {
        return None;
    }
    let h = i as HhId;
    let age = hh.age_years();
    let home = hh.home;
    let owner_occupier = hh.tenure == Tenure::Owner;
    let bank = hh.bank;
    let nprops = hh.properties.len();
    // Without an owned home or two or more properties the body draws nothing, so no stream is needed.
    if !(owner_occupier && home.is_some()) && nprops < INVESTOR_MIN_PROPERTIES {
        return None;
    }
    let mut r = rng_for(w, PH_LIST, i as u64);
    let mut plan = ListPlan {
        hh: h,
        home: None,
        mover: false,
        exit: false,
        lets: Vec::new(),
    };

    if owner_occupier {
        if let Some(hid) = home {
            let hs = &w.houses[hid as usize];
            if hs.owner == Owner::Household(h) && hs.completed && !hs.for_sale {
                // Mortgage lock-in (Fonseca & Liu 2023): owners whose mortgage rate is below the market rate
                // move less — mobility falls ≈10% per percentage point of rate gap.
                let own_rate = hs.mortgage.and_then(|l| w.loans.get(l)).map(|l| l.annual_rate);
                let gap_pp = own_rate.map_or(0.0, |r0| (mortgage_rate_of(w, bank) - r0).max(0.0) * 100.0);
                let scale = (1.0 - LOCK_IN_PER_PP * gap_pp).max(LOCK_IN_FLOOR) * market;
                // Two independent listing decisions: a move (under 75) and an exit to renting (any age). Exits are
                // a tenure change that demography cannot offset, so they wait for the reported run like other
                // tenure changes. Moves are locked in (and follow the market); exits follow the user cost, not the
                // lock-in (fix d, review 5): higher mortgage rates raise exits to renting.
                let p_move = if age < RETIREE_AGE { OWNER_LIST_PROB * scale } else { 0.0 };
                let p_exit = if in_burn_in(w) {
                    0.0
                } else {
                    hz.owner_exit[band_index(&OWNER_EXIT_BANDS, age)] * OWNER_EXIT_SCALE * exit_mult
                };
                let u: f64 = r.gen();
                let u_exit: f64 = r.gen();
                if u < p_move {
                    let eps = rng::normal(&mut r);
                    plan.home = Some((hid, eps));
                    plan.mover = true;
                } else if u_exit < p_exit {
                    let eps = rng::normal(&mut r);
                    plan.home = Some((hid, eps));
                    plan.exit = true;
                }
            }
        }
    }

    if nprops >= INVESTOR_MIN_PROPERTIES {
        for k in 0..nprops {
            let p = hh.properties[k];
            let u: f64 = r.gen();
            if home == Some(p) {
                continue;
            }
            let hs = &w.houses[p as usize];
            if !hs.completed || hs.for_sale {
                continue;
            }
            if u < sell_prob && !investor_locked_in(w, p) {
                let eps = rng::normal(&mut r);
                plan.lets.push((p, eps));
            }
        }
    }

    if plan.home.is_none() && plan.lets.is_empty() {
        None
    } else {
        Some(plan)
    }
}

/// Owner-exit multiplier (uc / uc0)^e from the user cost relative to its anchor (fix d): 1 before the anchor.
fn owner_exit_uc_factor(w: &World) -> f64 {
    let st = &w.housing;
    if in_burn_in(w) || st.uc0 <= 0.0 {
        return 1.0;
    }
    let uc = st.uc_mortgage_ema + w.cfg.property_tax + UC_MAINTENANCE - st.uc_ehpa_ema;
    user_cost_factor(uc, st.uc0, w.cfg.housing_exit_uc_elasticity)
}

/// fix-housing: an investor's expected total return less its hurdle: net rental yield plus expected appreciation, minus
/// the 10-year yield and the minimum spread. The buy side uses the same margin (see `bid_for`).
fn investor_margin(w: &World, gross_yield: f64) -> f64 {
    gross_yield - INVESTOR_RUNNING_COSTS + expected_hpa(w) - w.curve.yield_at(120.0) - INVESTOR_MIN_YIELD_SPREAD
}

/// fix-housing: lock-in (Fonseca-Liu 2023 analogue). A let investor unit whose rent covers its own mortgage payment
/// (or that carries no mortgage) is held through a downturn; vacant units and cash-flow-negative landlords sell.
fn investor_locked_in(w: &World, p: HouseId) -> bool {
    let hs = &w.houses[p as usize];
    hs.occupant.is_some() && hs.mortgage.and_then(|l| w.loans.get(l)).map_or(true, |l| hs.rent >= l.payment)
}

/// fix-housing: monthly sale probability of each investor property, from the smoothed margin. Zero while the margin
/// is above −INVESTOR_SELL_TRIGGER (the original market, where investors do not sell), ramping to the cap as the
/// margin stays persistently negative (no 2022-style mass sell-off on a one-month rate rise).
fn investor_sell_prob(margin_ema: f64) -> f64 {
    let stress = ((-margin_ema - INVESTOR_SELL_TRIGGER) / INVESTOR_SELL_RAMP).clamp(0.0, 1.0);
    INVESTOR_SELL_CAP * stress
}

/// Completed houses that are empty and neither let nor listed (e.g. inherited, or left behind by a move) are
/// offered: listed for sale with probability IDLE_LIST_PROB, else let. Household owners only; firm/bank stock is handled elsewhere.
/// Each unit's draw uses its own stream and only its own record, so the units are processed in parallel.
fn offer_idle_units(w: &mut World) {
    let (seed, tick) = (w.seed, w.tick);
    let hpi = w.prices.hpi;
    let idio = &w.housing.house_idio;
    let norm = w.housing.idio_norm;
    w.houses.par_iter_mut().for_each(|hs| {
        if !(hs.completed
            && hs.occupant.is_none()
            && !hs.for_sale
            && !hs.for_rent
            && matches!(hs.owner, Owner::Household(_)))
        {
            return;
        }
        // Same key as `rng_for(w, PH_LIST, u64::MAX - hid)`.
        let mut r = rng::agent_rng(seed, tick, stream::HOUSING, (PH_LIST << 40) | (u64::MAX - hs.id as u64));
        if r.gen::<f64>() < IDLE_LIST_PROB {
            let eps = rng::normal(&mut r);
            let mult = idio.get(hs.id as usize).map_or(1.0, |x| (x - norm).exp());
            list_at(hs, hpi * hs.quality * mult, eps);
        } else {
            hs.for_rent = true;
        }
    });
}

/// The listing of `list_house` for a given ask-noise draw ε.
fn list_house_at(w: &mut World, hid: HouseId, eps: f64) {
    let v = house_value_i(w, hid);
    list_at(&mut w.houses[hid as usize], v, eps);
}

/// The listing itself: ask = value × 1.04 × exp(0.05 ε) with the house's value `v`.
#[inline]
fn list_at(hs: &mut House, v: f64, eps: f64) {
    hs.for_sale = true;
    hs.ask_price = v * ASK_MARKUP * (ASK_NOISE * eps).exp();
    hs.months_listed = 0;
    hs.for_rent = false;
}

// ---------------------------------------------------------------- 5. bids

/// Wealth cut-off (deposits + Fund shares) for investor eligibility: the top 15%.
fn investor_wealth_threshold(w: &World) -> f64 {
    let mut v: Vec<f64> = w
        .households
        .par_iter()
        .filter(|h| h.alive)
        .map(|h| h.deposits + h.shares * w.fund.share_price)
        .collect();
    if v.is_empty() {
        return f64::INFINITY;
    }
    let k = ((v.len() as f64 * INVESTOR_WEALTH_QUANTILE) as usize).min(v.len() - 1);
    if v.iter().any(|x| x.is_nan()) {
        v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
        v[k]
    } else {
        kth_smallest(v, k)
    }
}

/// The `k`-th smallest value (0-based) of `v`, which must not contain NaN. Exact: a strided sample brackets the
/// answer, a parallel count and collect of the bracket locates it, and a full selection is the fallback.
fn kth_smallest(mut v: Vec<f64>, k: usize) -> f64 {
    const SAMPLES: usize = 1024;
    const MARGIN: usize = 48;
    let n = v.len();
    let cmp = |a: &f64, b: &f64| a.partial_cmp(b).unwrap_or(Ordering::Equal);
    if n < 8 * SAMPLES {
        v.select_nth_unstable_by(k, cmp);
        return v[k];
    }
    let stride = n / SAMPLES;
    let mut sample: Vec<f64> = (0..SAMPLES).map(|j| v[j * stride]).collect();
    sample.sort_unstable_by(cmp);
    let centre = k * SAMPLES / n;
    let lo = sample[centre.saturating_sub(MARGIN)];
    let hi = sample[(centre + MARGIN).min(SAMPLES - 1)];
    let below = v.par_iter().filter(|&&x| x < lo).count();
    if below <= k {
        let mut mid: Vec<f64> = v.par_iter().copied().filter(|&x| x >= lo && x <= hi).collect();
        let j = k - below;
        if j < mid.len() {
            mid.select_nth_unstable_by(j, cmp);
            return mid[j];
        }
    }
    v.select_nth_unstable_by(k, cmp);
    v[k]
}

/// This month's bids, computed in parallel from the immutable world. Output order is by household id.
fn compute_bids(w: &World) -> Vec<Bid> {
    let thr = investor_wealth_threshold(w);
    let ehpa = expected_hpa(w);
    let gross_yield = w.prices.rent_index * 12.0 / w.prices.hpi.max(1e-9);
    let hz = BandHazards::new();
    let buy_mult = buy_rate_mult(w);
    (0..w.households.len())
        .into_par_iter()
        .filter_map(|i| bid_for(w, i, thr, ehpa, gross_yield, &hz, buy_mult))
        .collect()
}

/// One household's bid, if it evaluates this month and the bid passes the affordability and user-cost tests (§10).
fn bid_for(w: &World, i: usize, wealth_thr: f64, ehpa: f64, gross_yield: f64, hz: &BandHazards, buy_mult: f64) -> Option<Bid> {
    let hh = &w.households[i];
    if !hh.alive {
        return None;
    }
    let h = i as HhId;
    let age = hh.age_years();
    let wealth = hh.deposits + hh.shares * w.fund.share_price;
    // Households that just sold their own home are re-buying (most US existing-home sales are repeat buyers).
    let mover = w.housing.recent_seller.get(i).copied().unwrap_or(0) > 0
        && (hh.tenure != Tenure::Owner || hh.home.is_some_and(|hm| w.houses[hm as usize].for_sale));
    // First-time purchases come from renters: a household must rent before it buys (entrants leave their parents
    // into a lease, not straight into a purchase). A former owner who sold to rent waits out the re-buy cooldown.
    let cooling = w.housing.exit_cooldown.get(i).copied().unwrap_or(0) > 0;
    let owner_buyer = mover
        || (hh.tenure == Tenure::Renter
            && !cooling
            && age >= 18.0
            && hh.status == LaborStatus::Employed
            && (if w.cfg.first_buy_continuous_employment { hh.months_employed } else { hh.months_in_job }) >= 6);
    // Each test below needs one of these conditions (the investor's yield test is checked later). Other households
    // cannot bid whatever their draws, so they skip the RNG setup.
    let investor_possible = age >= 25.0 && wealth >= wealth_thr && hh.properties.len() < MAX_PROPERTIES;
    if !owner_buyer && !investor_possible {
        return None;
    }
    // First-time purchases are tenure changes that the frozen demography cannot offset (no deaths free homes), so
    // they wait for the reported run. Movers rebuying and investors are unaffected. (Known gap, see DESIGN §10: a
    // renter who is investor-eligible still reaches the owner-bid test in burn-in.)
    if in_burn_in(w) && !mover && !investor_possible {
        return None;
    }
    let mut r = rng_for(w, PH_BID, i as u64);
    let u_inv: f64 = r.gen();
    let u_own: f64 = r.gen();
    // The two uniforms of `rng::normal`, drawn here in stream order; the transform runs only when a price is needed.
    let n1: f64 = r.gen::<f64>().max(1e-12);
    let n2: f64 = r.gen();
    let u_band: f64 = r.gen();

    // Equity in a home the mover has listed (bridge-financed down payment).
    let listed_equity = if mover && hh.tenure == Tenure::Owner {
        hh.home
            .map(|hm| (0.8 * house_value_i(w, hm) - common::mortgage_balance(w, hm)).max(0.0))
            .unwrap_or(0.0)
    } else {
        0.0
    };
    // b4-rent (M-T): the first-time buyer hazard scales with `buy_mult` (1 in master); movers are unchanged.
    let buy_prob = if mover { MOVER_BUY_PROB } else { hz.first_buy[band_index(&FIRST_BUY_BANDS, age)] * buy_mult };
    let rate_est = mortgage_rate_of(w, hh.bank);
    // Buy-to-let when the expected total return (net rental yield + expected appreciation) beats the 10-year
    // Treasury plus a risk premium. Investors differ in the return they require, so the share of investors who
    // pass the test rises smoothly with the margin (logistic, INVESTOR_MARGIN_SCALE) instead of switching all
    // investors on and off at once. The margin is market-wide, so a hard threshold would turn investor demand on
    // and off as a block (the baseline margin itself moves around zero).
    let investor_margin = gross_yield - INVESTOR_RUNNING_COSTS + ehpa - w.curve.yield_at(120.0) - INVESTOR_MIN_YIELD_SPREAD;
    let investor_pass = 1.0 / (1.0 + (-investor_margin / INVESTOR_MARGIN_SCALE).exp());
    let investor = age >= 25.0
        && wealth >= wealth_thr
        && hh.properties.len() < MAX_PROPERTIES
        && u_inv < INVESTOR_PROB * investor_pass;
    let _ = rate_est;
    if !investor && !(owner_buyer && u_own < buy_prob) {
        return None;
    }

    // Desired price: 4.5 × annual gross income × exp(0.5 ε) (Baptista et al.); movers also roll over their equity.
    let monthly_gross = if hh.wage > 0.0 { hh.wage } else { hh.permanent_income / 0.78 };
    let annual = 12.0 * monthly_gross.max(0.0);
    let eps = (-2.0 * n1.ln()).sqrt() * (2.0 * std::f64::consts::PI * n2).cos();
    let p0 = (BID_INCOME_MULTIPLE * annual + if mover { 0.8 * (wealth.max(0.0) + listed_equity) } else { 0.0 }) * (0.5 * eps).exp();
    // Liquid resources for the down payment after a spending reserve; Fund shares can be sold.
    let reserve = RESERVE_MONTHS * hh.permanent_income.max(1_000.0);
    let down = hh.deposits + hh.shares * w.fund.share_price + listed_equity - reserve;
    if !pos(down) || !pos(p0) {
        return None;
    }
    let cost = 1.0 + TRANSFER_TAX + CLOSING_COSTS;
    let (price, principal, quote) = match banking::mortgage_quote(w, h, p0, investor) {
        Some(q0) => {
            // Price so that loan plus down payment (incl. closing costs and tax) covers it; re-quote at that price.
            let p1 = p0.min((q0.max_loan + down) / cost);
            let q1 = banking::mortgage_quote(w, h, p1, investor).unwrap_or(q0);
            let p2 = p0.min((q1.max_loan + down) / cost);
            (p2, q1.max_loan.min(p2).max(0.0), Some(q1))
        }
        None => (p0.min(down / cost), 0.0, None),
    };
    if !pos(price) {
        return None;
    }

    if !investor && !mover {
        // User cost of owning (§10) against rent: P × (mortgage rate + 1.1% tax & upkeep − E[HPA]) / 12.
        let rate = quote.map_or(rate_est, |q| q.rate);
        let uc = price * (rate + 0.011 + 0.01 - ehpa) / 12.0;
        // Rent on the same house (Poterba 1984): the price at the market rent-to-price ratio, not the rent on the
        // (typically smaller) unit the household rents now.
        let rent_equiv = price * w.prices.rent_index / w.prices.hpi.max(1e-9);
        // Owner-occupation premium: households pay above user-cost parity to own (Glaeser & Gyourko 2007).
        // Probabilistic tenure choice (logit in the rent-vs-own gap, as in housing ABMs): buying stays possible when
        // owning is somewhat dearer than renting, and becomes likely when it is cheaper.
        let gap = (rent_equiv * OWNERSHIP_PREMIUM - uc) / rent_equiv.max(1.0);
        let p_buy = 1.0 / (1.0 + (-TENURE_LOGIT_SLOPE * gap).exp());
        if uc.is_nan() || u_band >= p_buy {
            return None;
        }
    }
    Some(Bid {
        hh: h,
        price,
        principal,
        quote,
        investor,
    })
}

// ---------------------------------------------------------------- 6. matching

/// Monthly double auction (§10): bids in descending order; each buyer takes the highest-quality listing with
/// ask ≤ bid, at the ask. Returns price/(quality × idiosyncratic value multiplier) for non-foreclosure sales (for the
/// HPI). Dividing out the multiplier keeps the index a constant-quality measure, as repeat-sales indices are (review 10).
fn match_bids(w: &mut World, mut bids: Vec<Bid>) -> Vec<f64> {
    let mut listings: Vec<(f64, HouseId)> = w
        .houses
        .par_iter()
        .enumerate()
        .filter(|(_, h)| h.for_sale && h.completed && h.ask_price > 0.0)
        .map(|(i, h)| (h.ask_price, i as HouseId))
        .collect();
    // Keys are unique (house index) and never NaN (ask_price > 0), so an unstable sort gives the same order.
    listings.par_sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    let asks: Vec<f64> = listings.iter().map(|l| l.0).collect();
    let quals: Vec<f64> = listings.iter().map(|l| w.houses[l.1 as usize].quality).collect();
    let mut tree = MaxTree::new(&quals);
    // Prices are finite and positive (bid_for) and household ids are unique, so the order is total.
    bids.par_sort_unstable_by(|a, b| {
        b.price
            .partial_cmp(&a.price)
            .unwrap_or(Ordering::Equal)
            .then(a.hh.cmp(&b.hh))
    });

    let mut ppq = Vec::new();
    for bid in &bids {
        let k = asks.partition_point(|&a| a <= bid.price);
        let Some(idx) = tree.query_prefix(k) else {
            continue;
        };
        let house = listings[idx].1;
        let q = w.houses[house as usize].quality;
        let price = w.houses[house as usize].ask_price;
        let mult = idio_mult(&w.housing, house as usize);
        if let Some(reo) = execute_purchase(w, bid, house) {
            tree.remove(idx);
            if !reo && q > 0.0 {
                ppq.push(price / (q * mult));
            }
        }
    }
    ppq
}

/// Complete one purchase at the ask: originate the mortgage (if quoted), pay the price and transfer tax, settle the
/// seller's mortgage, move ownership, and move an owner-occupier in. Returns `Some(true)` when the seller is a bank
/// selling a foreclosed house. Returns `None`, with no state change, when the buyer cannot fund the purchase.
fn execute_purchase(w: &mut World, bid: &Bid, house: HouseId) -> Option<bool> {
    ensure_sizes(w);
    let buyer = bid.hh;
    let bu = buyer as usize;
    let hu = house as usize;
    let price = w.houses[hu].ask_price;
    let seller = w.houses[hu].owner;
    if seller == Owner::Household(buyer) || !pos(price) || !w.households[bu].alive {
        return None;
    }
    let principal = if bid.quote.is_some() { bid.principal.min(price).max(0.0) } else { 0.0 };
    let quote = bid.quote.filter(|_| principal > 0.0);
    let tax = TRANSFER_TAX * price;

    // Funding test (§10): down payment plus closing costs covered by deposits and Fund shares.
    let fund_value = w.households[bu].shares * w.fund.share_price;
    let bridge = w.households[bu]
        .home
        .filter(|&hm| hm != house && w.houses[hm as usize].for_sale && w.houses[hm as usize].owner == Owner::Household(buyer))
        .map(|hm| (0.8 * house_value_i(w, hm) - common::mortgage_balance(w, hm)).max(0.0))
        .unwrap_or(0.0);
    if w.households[bu].deposits + fund_value + bridge < (price - principal) + CLOSING_COSTS * price {
        return None;
    }

    // The seller's mortgage is unlinked while the buyer's loan is originated (a bank will not lend against a
    // house that is already encumbered); it is settled from the sale proceeds below.
    let old_loan = w.houses[hu].mortgage.take();
    if let Some(q) = quote {
        // The new loan credits the buyer's deposits and links the house to it.
        if banking::originate_mortgage(w, buyer, house, principal, q).is_none() {
            w.houses[hu].mortgage = old_loan;
            return None;
        }
        w.flows.new_mortgages += 1;
        w.flows.new_mortgage_value += principal;
    }

    // Raise any shortfall from Fund shares, then pay the price and the transfer tax.
    let need = price + tax - w.households[bu].deposits;
    if need > 0.0 {
        ledger::fund_sell(w, buyer, need);
    }
    let seller_acct = match seller {
        Owner::Household(s) => Account::Household(s),
        Owner::Firm(f) => Account::Firm(f),
        Owner::Bank(b) => Account::Bank(b),
    };
    ledger::pay(w, Account::Household(buyer), seller_acct, price);
    let tax_paid = ledger::pay_up_to(w, Account::Household(buyer), Account::Government, tax);
    w.gov.revenue += tax_paid;

    // Seller side.
    let mut reo = false;
    // An owner-occupier who sells starts a buy-sell chain (a mover). An exit listing is not a move, and it needs no
    // check here: its occupant was released when it was listed (`exit_at_listing`), so the occupant guard below is
    // false. The tenure change happened at listing, so the sale does not change the seller's tenure.
    match seller {
        Owner::Household(s) => {
            if w.houses[hu].occupant == Some(s) {
                w.housing.recent_seller[s as usize] = MOVER_MONTHS;
            }
            if let Some(l) = old_loan.filter(|&l| w.loans.contains(l)) {
                let bal = w.loans.get(l).map_or(0.0, |x| x.principal);
                ledger::repay_principal(w, l, bal.min(price));
                if w.loans.contains(l) {
                    // Short sale: the seller covers the remainder from deposits; any rest is written off.
                    let rem = w.loans.get(l).map_or(0.0, |x| x.principal);
                    let from_dep = w.households[s as usize].deposits.max(0.0).min(rem);
                    ledger::repay_principal(w, l, from_dep);
                    if w.loans.contains(l) {
                        ledger::write_off(w, l);
                    }
                }
            }
        }
        Owner::Firm(f) => {
            w.firms[f as usize].revenue += price;
        }
        Owner::Bank(_) => {
            reo = true;
            // Foreclosed house: the defaulted loan is written off net of proceeds (DESIGN §16).
            if let Some(l) = w.housing.reo_loan.get_mut(hu).and_then(|x| x.take()) {
                if let Some(loan) = w.loans.get(l) {
                    let (lb, p) = (loan.bank as usize, loan.principal);
                    let rec = p.min(price);
                    ledger::write_off(w, l);
                    w.banks[lb].loan_losses -= rec;
                    w.flows.loan_losses -= rec;
                    if price > p {
                        w.banks[lb].interest_income += price - p;
                    }
                }
            }
        }
    }
    // Any occupant (the seller living there, or a tenant of an investor) leaves.
    if w.houses[hu].occupant.is_some() {
        release_occupant(w, house);
    }

    // Buyer side. An owner-occupier moves in and vacates any rental; an investor's purchase is let out.
    if !bid.investor {
        if let Some(old) = w.households[bu].home {
            let ou = old as usize;
            if ou != hu && w.houses[ou].occupant == Some(buyer) {
                w.houses[ou].occupant = None;
                w.houses[ou].for_rent = !w.houses[ou].for_sale;
            }
        }
        let hh = &mut w.households[bu];
        hh.tenure = Tenure::Owner;
        hh.home = Some(house);
        hh.rent = 0.0;
        hh.lease_months_left = 0;
        w.housing.arrears[bu] = 0.0;
        w.housing.seeking[bu] = false;
    }
    ledger::transfer_house(w, house, Owner::Household(buyer));
    let hs = &mut w.houses[hu];
    hs.for_sale = false;
    hs.ask_price = 0.0;
    hs.months_listed = 0;
    hs.last_sale_price = price;
    hs.rent = 0.0;
    if bid.investor {
        hs.occupant = None;
        hs.for_rent = true;
    } else {
        hs.occupant = Some(buyer);
        hs.for_rent = false;
    }
    w.flows.house_sales += 1;
    w.flows.house_sale_value += price;
    Some(reo)
}

// ---------------------------------------------------------------- 6b. demolition

/// Withdraw long-vacant, unmortgaged rental units from the stock (`DEMOLISH_PROB`). The house keeps its id but leaves
/// the stock: `completed = false`, no flags, no project, no money. A household landlord's unit passes to the bank
/// slot, since the house must have an owner and no household holds it any more.
fn demolish(w: &mut World) {
    ensure_sizes(w);
    let (seed, tick) = (w.seed, w.tick);
    let mut n = 0usize;
    for hid in 0..w.houses.len() {
        let hs = &w.houses[hid];
        if !(hs.completed
            && hs.occupant.is_none()
            && hs.for_rent
            && !hs.for_sale
            && matches!(hs.owner, Owner::Firm(_) | Owner::Household(_))
            && hs.mortgage.is_none()
            && w.housing.reo_loan[hid].is_none()
            && w.housing.vacant_months[hid] >= DEMOLISH_MIN_VACANT_MONTHS)
        {
            continue;
        }
        let mut r = rng::agent_rng(seed, tick, stream::HOUSING, (PH_DEMOLISH << 40) | hid as u64);
        if r.gen::<f64>() < DEMOLISH_PROB {
            if matches!(w.houses[hid].owner, Owner::Household(_)) {
                ledger::transfer_house(w, hid as HouseId, Owner::Bank(0));
            }
            let hs = &mut w.houses[hid];
            hs.completed = false;
            hs.for_rent = false;
            hs.rent = 0.0;
            w.housing.vacant_months[hid] = 0;
            n += 1;
        }
    }
    w.housing.demolished += n;
}

/// Withdraw up to `k` vacant, unmortgaged rental units (lowest house ids first) from the stock, as `demolish` does.
fn withdraw_vacant_rentals(w: &mut World, k: usize) {
    let mut n = 0usize;
    for hid in 0..w.houses.len() {
        if n >= k {
            break;
        }
        let hs = &w.houses[hid];
        if !(hs.completed
            && hs.occupant.is_none()
            && hs.for_rent
            && !hs.for_sale
            && matches!(hs.owner, Owner::Firm(_) | Owner::Household(_))
            && hs.mortgage.is_none()
            && w.housing.reo_loan[hid].is_none())
        {
            continue;
        }
        if matches!(w.houses[hid].owner, Owner::Household(_)) {
            ledger::transfer_house(w, hid as HouseId, Owner::Bank(0));
        }
        let hs = &mut w.houses[hid];
        hs.completed = false;
        hs.for_rent = false;
        hs.rent = 0.0;
        w.housing.vacant_months[hid] = 0;
        n += 1;
    }
    w.housing.demolished += n;
}

// ---------------------------------------------------------------- 7. rental market

/// Vacant for-rent units are matched to seekers (richest first) at the best quality with rent ≤ budget (§10).
/// Pushes rent / quality for each new lease.
fn rental_market(w: &mut World, rent_per_q: &mut Vec<f64>) {
    ensure_sizes(w);
    let rent_index = w.prices.rent_index;
    let hpi = w.prices.hpi;
    let idio = &w.housing.house_idio;
    let norm = w.housing.idio_norm;
    // Landlords sell empty units into a tight for-sale market and hold them in a slack one.
    // A unit vacant for half a year is also sold at a small base rate in any market, so the rental stock shrinks when
    // renter households decline (rental-to-owner conversions; AHS components of inventory change: ≈2–3% of rentals
    // a year move to owner occupancy).
    // b4-rent (M-V): `vacant_sell_prob` (the master hazard unless the re-anchored form is on). Last month's HVS vacancy is
    // the stored `rental_vac_hvs`, since `update_indices_with` runs after this step.
    let sell_prob = vacant_sell_prob(
        &w.cfg,
        b4_live(w),
        months_supply(w),
        w.housing.ms_ref,
        w.housing.rental_vac_hvs,
        w.housing.rental_vac_ref,
    );
    let (seed, tick) = (w.seed, w.tick);
    let mut units: Vec<(f64, HouseId)> = w
        .houses
        .par_iter_mut()
        .zip(w.housing.vacant_months.par_iter_mut())
        .enumerate()
        .filter_map(|(i, (hs, vm_slot))| {
            if !hs.completed || hs.occupant.is_some() || !hs.for_rent || hs.for_sale {
                return None;
            }
            let q = hs.quality;
            // Same key as `rng_for(w, PH_RENT_UNIT, i)`.
            let mut r = rng::agent_rng(seed, tick, stream::HOUSING, (PH_RENT_UNIT << 40) | i as u64);
            let jitter = 0.95 + 0.15 * r.gen::<f64>();
            // Landlords cut the asking rent the longer a unit sits empty, and sell units that stay empty.
            let vm = *vm_slot;
            if vm >= VACANT_SELL_MONTHS && matches!(hs.owner, Owner::Household(_)) && r.gen::<f64>() < sell_prob {
                *vm_slot = 0;
                hs.for_rent = false;
                let eps = rng::normal(&mut r);
                let mult = idio.get(i).map_or(1.0, |x| (x - norm).exp());
                list_at(hs, hpi * q * mult, eps);
                return None;
            }
            *vm_slot = vm.saturating_add(1);
            let discount = VACANT_RENT_DECAY.powi(vm as i32).max(VACANT_RENT_FLOOR);
            let rent = rent_index * q * jitter * discount;
            hs.rent = rent;
            Some((rent, i as HouseId))
        })
        .collect();
    // Keys are unique (house index); with no NaN rent the unstable sort gives the same order as a stable one.
    if units.iter().any(|u| u.0.is_nan()) {
        units.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    } else {
        units.par_sort_unstable_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    }
    let rents: Vec<f64> = units.iter().map(|u| u.0).collect();
    let quals: Vec<f64> = units.iter().map(|u| w.houses[u.1 as usize].quality).collect();
    let mut tree = MaxTree::new(&quals);

    let mut seekers: Vec<(f64, HhId)> = {
        let ww: &World = w;
        (0..ww.households.len())
            .into_par_iter()
            .filter(|&i| {
                // Check the compact `seeking` flag first so only seekers touch their household record.
                ww.housing.seeking[i] && {
                    let hh = &ww.households[i];
                    // Parents' children and former owners who sold to rent (tenure Renter, no lease yet).
                    hh.alive && hh.tenure != Tenure::Owner && hh.home.is_none()
                }
            })
            .map(|i| {
                let hh = &ww.households[i];
                // Young adults share units (roommates) and stretch further on rent.
                let share = if hh.age_years() < 30.0 { YOUNG_RENT_BUDGET_SHARE } else { RENT_BUDGET_SHARE };
                (hh.permanent_income * share / RENT_BUDGET_SHARE, i as HhId)
            })
            .collect()
    };
    // Keys are unique (household id) and never NaN (permanent income), so an unstable sort gives the same order.
    if seekers.iter().any(|s| s.0.is_nan()) {
        seekers.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    } else {
        seekers.par_sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    }

    let indexed = w.cfg.rent_floor_indexed;
    for (income, h) in seekers {
        let budget = if indexed {
            ((RENT_BUDGET_SHARE + SEEKER_EXTRA_BURDEN) * income).max(RENT_FLOOR_INDEX_SHARE * rent_index)
        } else {
            (RENT_BUDGET_SHARE * income).max(MIN_RENT_BUDGET)
        };
        let k = rents.partition_point(|&r| r <= budget);
        let Some(idx) = tree.query_prefix(k) else {
            continue;
        };
        let (rent, hid) = units[idx];
        if w.houses[hid as usize].owner == Owner::Household(h) {
            continue;
        }
        tree.remove(idx);
        let q = w.houses[hid as usize].quality;
        w.housing.vacant_months[hid as usize] = 0;
        let hs = &mut w.houses[hid as usize];
        hs.occupant = Some(h);
        hs.for_rent = false;
        hs.rent = rent;
        let hh = &mut w.households[h as usize];
        hh.tenure = Tenure::Renter;
        hh.home = Some(hid);
        hh.rent = rent;
        hh.lease_months_left = LEASE_MONTHS as u16;
        w.housing.seeking[h as usize] = false;
        w.housing.arrears[h as usize] = 0.0;
        w.flows.new_leases += 1;
        if q > 0.0 {
            rent_per_q.push(rent / q);
        }
    }
}

// ---------------------------------------------------------------- 8. refinancing

/// A refinance that passes the pre-origination tests: the house, its loan, the balance to repay and the new quote.
#[derive(Clone)]
struct RefiPlan {
    house: HouseId,
    loan: LoanId,
    balance: f64,
    quote: MortgageQuote,
}

/// Market-rate gap in percentage points for household `h`'s home loan (old rate less the bank's posted rate), or None
/// when it has no home loan of its own. `fix-housing`.
fn refi_gap_pp(w: &World, h: HhId) -> Option<f64> {
    let hh = &w.households[h as usize];
    let hid = hh.home?;
    let lid = w.houses[hid as usize].mortgage?;
    let loan = w.loans.get(lid)?;
    if loan.borrower != Borrower::Household(h) {
        return None;
    }
    Some((loan.annual_rate - mortgage_rate_of(w, hh.bank)) * 100.0)
}

/// The tests before a refinance for household `h` (DESIGN §10), in order; None when it does not refinance. `floor` is
/// the lowest mortgage rate of a live bank: a quote can never be below it. `fix-housing`: the household must be
/// employed and current on the loan, and the new loan replaces the old one at the current rate with a full term.
fn refi_plan(w: &World, h: HhId, floor: f64) -> Option<RefiPlan> {
    let thr = w.cfg.refi_threshold_pp / 100.0;
    let hh = &w.households[h as usize];
    if hh.status != LaborStatus::Employed {
        return None;
    }
    let hid = hh.home?;
    let lid = w.houses[hid as usize].mortgage?;
    let loan = w.loans.get(lid)?;
    if loan.borrower != Borrower::Household(h) || loan.months_delinquent > 0 {
        return None;
    }
    let (bal, old_rate) = (loan.principal, loan.annual_rate);
    if old_rate - thr <= floor {
        return None;
    }
    let v = house_value_i(w, hid);
    if !pos(v) || bal / v >= REFI_MAX_LTV {
        return None;
    }
    // The old payment is excluded from the DTI test (the new loan replaces it).
    let q = banking::mortgage_quote_refi(w, h, v, lid)?;
    if q.rate >= old_rate - thr || bal > q.max_loan {
        return None;
    }
    Some(RefiPlan { house: hid, loan: lid, balance: bal, quote: q })
}

/// Owners refinance their home mortgage when its rate exceeds the market mortgage rate by more than
/// `refi_threshold_pp`. The monthly probability rises with the gap (`refi_prob_per_pp`, capped at `refi_prob_cap`).
/// The candidate draw uses each household's own stream, so the result does not depend on thread count.
///
/// Returns the number of living renter households. Refinancing changes no tenure or liveness, so the count taken
/// in the same parallel pass as the candidates is the one `update_indices` needs.
fn refinance(w: &mut World) -> f64 {
    let (thr, per_pp, cap) = (w.cfg.refi_threshold_pp, w.cfg.refi_prob_per_pp, w.cfg.refi_prob_cap);
    // fix-housing: refinancing is a reported-run mechanism (like owner exits), so burn-in leaves the calibrated
    // mortgage stock as it was; the candidates are none during burn-in but the renter count is still taken.
    let burn = in_burn_in(w);
    // Only owners whose gap clears the threshold and whose draw passes their probability can act. Each draw depends
    // only on its household key, so the candidates are found in parallel; the remaining checks run in household order.
    let (candidates, renters): (Vec<usize>, usize) = {
        let ww: &World = w;
        (0..ww.households.len())
            .into_par_iter()
            .fold(
                || (Vec::new(), 0usize),
                |(mut cands, mut renters), i| {
                    let hh = &ww.households[i];
                    if hh.alive && hh.tenure == Tenure::Renter {
                        renters += 1;
                    }
                    if !burn && hh.alive && hh.tenure == Tenure::Owner && hh.home.is_some() {
                        if let Some(gap) = refi_gap_pp(ww, i as HhId).filter(|g| *g > thr) {
                            let p = (per_pp * gap).min(cap);
                            let mut r = rng_for(ww, PH_REFI, i as u64);
                            let u: f64 = r.gen();
                            if u < p {
                                cands.push(i);
                            }
                        }
                    }
                    (cands, renters)
                },
            )
            .reduce(
                || (Vec::new(), 0usize),
                |(mut a, ra), (b, rb)| {
                    a.extend(b);
                    (a, ra + rb)
                },
            )
    };
    // The checks and the quote for each candidate read only its own record, house and loan, and the banks' capital
    // and rates; they are computed in parallel against the state at the start of the loop. Until a refinance changes
    // balances, those results are the ones the sequential checks would give, so they are used as they are; after
    // the first refinance each candidate is planned again in household order.
    // Every quote carries the rate of a live bank, so no quote can pass when the old rate less the threshold is at or
    // below the lowest of those rates (rates are not changed in this step).
    let floor = w
        .banks
        .iter()
        .filter(|b| b.alive && b.mortgage_rate.is_finite())
        .map(|b| b.mortgage_rate)
        .fold(f64::INFINITY, f64::min);
    let plans: Vec<Option<RefiPlan>> = {
        let ww: &World = w;
        candidates.par_iter().map(|&i| refi_plan(ww, i as HhId, floor)).collect()
    };
    let mut changed = false;
    for (k, &i) in candidates.iter().enumerate() {
        let h = i as HhId;
        let plan = if changed { refi_plan(w, h, floor) } else { plans[k].clone() };
        let Some(p) = plan else {
            continue;
        };
        // fix-housing: the house's mortgage link is released so the new loan can attach, and restored if origination
        // fails. The new loan credits deposits; repaying the old loan from them leaves the household's cash unchanged.
        w.houses[p.house as usize].mortgage = None;
        if banking::originate_mortgage(w, h, p.house, p.balance, p.quote).is_some() {
            ledger::repay_principal(w, p.loan, p.balance);
            w.flows.refinances += 1;
            changed = true;
        } else {
            w.houses[p.house as usize].mortgage = Some(p.loan);
        }
    }
    renters as f64
}

// ---------------------------------------------------------------- 9. indices

/// HPI moves toward the median price/quality of this month's sales (weight 0.5); the rent index moves toward the
/// median rent/quality of new leases (weight 0.3). Each needs at least 5 observations. The HPI is pushed monthly.
/// Smoothed buyer affordability (as in the NAR Housing Affordability Index): the mortgage the median earner can carry
/// at the current mortgage rate under the DTI limit, relative to the HPI. It falls when mortgage rates rise or DTI
/// limits tighten and rises with incomes. A representative buyer is used rather than this month's bids so that
/// changes in the mix of bidders (movers with equity vs first-time buyers) do not move prices.
fn track_buyer_budgets(w: &mut World, _bids: &[Bid]) {
    let Some(last) = w.last() else {
        return;
    };
    let income = last.median_wage_annual / 12.0;
    let r = last.mortgage_rate / 100.0 / 12.0;
    let n = w.cfg.mortgage_term_months.max(1) as i32;
    let pf = if r > 1e-9 { r / (1.0 - (1.0 + r).powi(-n)) } else { 1.0 / n as f64 };
    let m = w.cfg.max_dti * income / pf / w.prices.hpi.max(1.0);
    if !m.is_finite() || m <= 0.0 {
        return;
    }
    let st = &mut w.housing;
    st.bid_ratio = if st.bid_ratio > 0.0 { 0.8 * st.bid_ratio + 0.2 * m } else { m };
    if st.bid_ratio0 <= 0.0 {
        st.bid_ratio0 = st.bid_ratio;
    }
}

#[cfg(test)]
fn update_indices(w: &mut World, ppq: &[f64], rent_per_q: &[f64]) {
    let renters = w.households.par_iter().filter(|h| h.alive && h.tenure == Tenure::Renter).count() as f64;
    update_indices_with(w, ppq, rent_per_q, renters);
}

/// `update_indices` given this month's living-renter count (see `refinance`).
fn update_indices_with(w: &mut World, ppq: &[f64], _rent_per_q: &[f64], renters: f64) {
    // Transaction medians are selection-biased (leases go to the cheapest units per quality; houses sell after
    // their asks have been cut), so index growth is driven by market tightness, as in the empirical literature:
    // rents by the rental vacancy gap, house prices by months of supply, plus a sale-to-value correction.
    let pi_e = w
        .last()
        .map(|m| m.expected_inflation / 100.0)
        .unwrap_or(w.cfg.inflation_target)
        .clamp(-0.05, 0.2);
    // Slow trend of expected inflation (fix 3): updated every month, used for rents from the anchor on.
    {
        let st = &mut w.housing;
        if st.pi_trend_set {
            st.pi_trend += RENT_TREND_EMA_WEIGHT * (pi_e - st.pi_trend);
        } else {
            st.pi_trend = pi_e;
            st.pi_trend_set = true;
        }
        if st.pi_head_set {
            st.pi_head_ema += UC_PI_HEAD_EMA_WEIGHT * (pi_e - st.pi_head_ema);
        } else {
            st.pi_head_ema = pi_e;
            st.pi_head_set = true;
        }
    }
    // b4-rent: the HVS rental vacancy is always computed (it feeds the rental references below).
    let v_hvs = hvs_rental_vacancy(w);
    let v = if w.cfg.rent_vacancy_hvs {
        // Census HVS rental vacancy (`hvs_rental_vacancy`). Fixed at the first reported month; during burn-in the norm
        // follows the current rate, so the vacancy term is neutral.
        let v = v_hvs;
        if !w.housing.natural_vacancy_fixed {
            w.housing.natural_rental_vacancy = v.clamp(0.03, 0.15);
            if !in_burn_in(w) {
                w.housing.natural_vacancy_fixed = true;
            }
        }
        v
    } else {
        let vacant = w.houses.par_iter().filter(|h| h.completed && h.occupant.is_none() && h.for_rent).count() as f64;
        let v = vacant / (renters + vacant).max(1.0);
        // Natural levels are anchored to the calibrated initial state (its own steady state), within plausible bounds.
        if w.housing.natural_rental_vacancy <= 0.0 {
            w.housing.natural_rental_vacancy = v.clamp(0.05, 0.15);
        }
        v
    };
    // Rents are sticky: US CPI rent's trough was ≈ −0.4% yoy (2010) despite vacancy above 10%.
    // Own-vs-rent arbitrage: rents converge slowly toward the calibrated rent-to-price ratio (so rents track house
    // prices and incomes in the long run instead of drifting with vacancy alone).
    if w.housing.rent_yield0 <= 0.0 && w.prices.hpi > 0.0 {
        w.housing.rent_yield0 = 12.0 * w.prices.rent_index / w.prices.hpi;
    }
    // fix-housing: user-cost anchor. Rent-to-price moves with the user cost of ownership (mortgage rate, property tax,
    // maintenance, less expected appreciation): when mortgages get dearer, renting becomes relatively cheaper and
    // rents rise (Poterba 1984; Dias & Duarte 2019). θ = 0 restores the fixed rent-to-price ratio.
    let mort = market_mortgage_rate(w);
    let ehpa = expected_hpa(w);
    let burn = in_burn_in(w);
    // fix-housing: the post-anchor mechanisms (price term and the halved wage pass-through) act only in the
    // reported run, after the anchor is set. Burn-in keeps master's rent and price dynamics, so the month-0 state matches
    // master (the burn-in outcome is then unchanged).
    let live = !burn && w.housing.uc0 > 0.0;
    let mut anchored_now = false;
    {
        let st = &mut w.housing;
        // The anchors are read from the EMAs as they stood before this month's update, so a shock scheduled at m0
        // (which has already moved the market rate) cannot move the reference: shocked and baseline clones share it.
        if !burn && st.uc0 <= 0.0 && st.rent_yield0 > 0.0 && st.uc_mortgage_ema > 0.0 {
            let uc_pre = st.uc_mortgage_ema + w.cfg.property_tax + UC_MAINTENANCE - st.uc_ehpa_ema;
            if uc_pre > 0.0 {
                st.uc0 = uc_pre;
                // Real anchor: the appreciation expectation net of headline inflation. The real user cost at the anchor
                // equals `uc_pre` (the nominal one), so the anchor is the same.
                st.ehpa_real0 = st.uc_ehpa_ema - st.pi_head_ema;
                st.uc0_rent = uc_pre;
                st.uc0_price = st.uc_mortgage_ema - st.pi_trend + w.cfg.property_tax + UC_MAINTENANCE - st.ehpa_real0;
                st.build_rate0 = st.build_rate_ema;
                st.rent_gap0 = (w.prices.rent_index * 12.0 / (st.rent_yield0 * w.prices.hpi)).ln();
                anchored_now = true;
                st.anchored = true;
                st.anchor_tick = w.tick;
            }
        }
        if mort > 0.0 {
            if st.uc_mortgage_ema <= 0.0 {
                st.uc_mortgage_ema = mort;
                st.build_rate_ema = mort;
            } else {
                st.uc_mortgage_ema += UC_EMA_WEIGHT * (mort - st.uc_mortgage_ema);
                st.build_rate_ema += BUILD_RATE_EMA_WEIGHT * (mort - st.build_rate_ema);
            }
        }
        // Expected appreciation enters the anchor's user cost only through burn-in. From the anchor month it is frozen,
        // so a price boom cannot lower the user cost and feed back into the rent-to-star gap and prices (fix a).
        if !st.uc_ehpa_set {
            st.uc_ehpa_ema = ehpa;
            st.uc_ehpa_set = true;
        } else if burn {
            st.uc_ehpa_ema += UC_EHPA_EMA_WEIGHT * (ehpa - st.uc_ehpa_ema);
        }
    }
    // Ratio to the reference user cost, clamped (a non-positive user cost lands on the floor, so the factor is continuous).
    let uc = w.housing.uc_mortgage_ema + w.cfg.property_tax + UC_MAINTENANCE - w.housing.uc_ehpa_ema;
    // The rent side uses the real user cost (see `HousingState::pi_head_ema`); the nominal one still drives owner exits.
    let uc_rent = w.housing.uc_mortgage_ema - w.housing.pi_head_ema + w.cfg.property_tax + UC_MAINTENANCE - w.housing.ehpa_real0;
    let uc_factor = user_cost_factor(uc_rent, w.housing.uc0_rent, w.cfg.housing_uc_theta);
    // The reference drifts slowly (10-year EMA) toward the current user cost and construction rate, so trend drift in
    // baseline rates does not move the rent-to-price ratio or starts permanently. It absorbs about 20-30% of a shock over
    // 36 months. The anchor month itself is not drifted, so it is identical in shocked and baseline clones.
    // Price-side user cost: the mortgage rate net of the slow expectation trend (same real anchor as the rent side otherwise).
    let uc_price = w.housing.uc_mortgage_ema - w.housing.pi_trend + w.cfg.property_tax + UC_MAINTENANCE - w.housing.ehpa_real0;
    {
        let st = &mut w.housing;
        if st.uc0 > 0.0 && !anchored_now {
            st.uc0 += UC_REF_EMA_WEIGHT * (uc - st.uc0);
            st.uc0_rent += UC_REF_EMA_WEIGHT * (uc_rent - st.uc0_rent);
            st.uc0_price += UC_REF_EMA_WEIGHT * (uc_price - st.uc0_price);
            st.build_rate0 += UC_REF_EMA_WEIGHT * (st.build_rate_ema - st.build_rate0);
        }
    }
    let rent_star = w.housing.rent_yield0 * uc_factor * w.prices.hpi / 12.0;
    // One reference for rents and prices (fix c, restored in review 10): both act on the log rent-to-star gap measured
    // from its calibrated level (`rent_gap0`, zero in burn-in), and that gap uses the same real user cost. Arbitrage closes
    // the gap from the rent side and the price term from the price side.
    let gap = if rent_star > 0.0 && w.prices.rent_index > 0.0 { (w.prices.rent_index / rent_star).ln() } else { f64::NAN };
    let gap_ref = if live { w.housing.rent_gap0 } else { 0.0 };
    let arbitrage_speed = if live { w.cfg.housing_rent_price_anchor } else { BURN_IN_RENT_PRICE_ANCHOR };
    let arbitrage = if gap.is_finite() { -arbitrage_speed * (gap - gap_ref) } else { 0.0 };
    // Nominal income growth (annualised, smoothed) passes into rents beyond expected inflation.
    let wage = w.prices.wage_index;
    if wage > 0.0 {
        if w.housing.last_wage_index > 0.0 {
            let g_month = (wage / w.housing.last_wage_index).powi(12) - 1.0;
            w.housing.income_growth += INCOME_GROWTH_EMA * (g_month - w.housing.income_growth);
        }
        w.housing.last_wage_index = wage;
    }
    // Rents grow with the slow trend of expected inflation (not the headline expectation, which an oil or food shock
    // moves for a year). Burn-in keeps the headline term, so the month-0 state is unchanged.
    let pi_rent = if live { w.housing.pi_trend } else { pi_e };
    let pass_coef = if live { w.cfg.rent_income_pass } else { BURN_IN_INCOME_PASS };
    let income_pass = pass_coef * (w.housing.income_growth - pi_rent);
    let vacancy_gap = RENT_VACANCY_SENS * (w.housing.natural_rental_vacancy - v);
    let g_rent = (pi_rent + income_pass + vacancy_gap + arbitrage).clamp(-0.03, 0.10);
    if w.prices.rent_index > 0.0 {
        w.prices.rent_index *= 1.0 + g_rent / 12.0;
    }

    if w.housing.sales_ema <= 0.0 {
        w.housing.sales_ema = listings_count(w) as f64 / BALANCED_MONTHS_SUPPLY;
    }
    let ms = months_supply(w).min(24.0);
    if w.housing.balanced_months_supply <= 0.0 {
        w.housing.balanced_months_supply = BALANCED_MONTHS_SUPPLY;
    }
    // Supply imbalance in log terms (doubling the months of supply ≈ −3%/yr), bounded at ±4%/yr.
    let imbalance = (w.housing.balanced_months_supply / ms.max(0.25)).ln();
    let mut g_hpi = pi_e + (HPI_LOG_SUPPLY_SENS * imbalance).clamp(-0.04, 0.04);
    // Price side of the user cost: prices move to keep the same real rent-to-star gap at its calibrated level (the gap is
    // measured relative to rent_gap0, so the baseline is unaffected and only the rate-driven deviation counts).
    if live && gap.is_finite() {
        // The price gap is the rent gap with its user-cost netting swapped: the rent side nets the headline-real cost
        // (in `gap`), the price side the slow-trend-real cost. Both are zero at the anchor, so the reference is shared.
        let log_ratio = |uc: f64, uc0: f64| if uc0 > 0.0 { (uc / uc0).clamp(UC_RATIO_MIN, UC_RATIO_MAX).ln() } else { 0.0 };
        let theta = w.cfg.housing_uc_theta;
        let rent_log = log_ratio(uc_rent, w.housing.uc0_rent);
        let price_log = log_ratio(uc_price, w.housing.uc0_price);
        let gap_price = (gap - w.housing.rent_gap0) + theta * (rent_log - price_log);
        g_hpi += w.cfg.housing_price_uc_gain * gap_price;
    }
    // Buyer affordability: prices are pulled toward what buyers can bid (mortgage rates, incomes, down payments)
    // relative to the calibrated initial ratio — the main channel from interest rates to house prices, and an anchor
    // against drift away from incomes.
    if w.housing.bid_ratio > 0.0 && w.housing.bid_ratio0 > 0.0 {
        let afford = (w.housing.bid_ratio / w.housing.bid_ratio0).ln();
        let sens = if live { w.cfg.housing_affordability_sens } else { BURN_IN_AFFORDABILITY_SENS };
        g_hpi += (sens * afford).clamp(-0.06, 0.06);
    }
    if ppq.len() >= MIN_INDEX_SAMPLES && w.prices.hpi > 0.0 {
        let mut s = ppq.to_vec();
        let ratio = median(&mut s) / w.prices.hpi;
        if ratio.is_finite() {
            g_hpi += 12.0 * HPI_WEIGHT * 0.1 * (ratio - SALE_TO_VALUE_NORM);
        }
    }
    w.prices.hpi *= 1.0 + g_hpi.clamp(-0.10, 0.12) / 12.0;
    w.prices.hpi_history.push(w.prices.hpi);
    let n = w.prices.hpi_history.len();
    if n > HPI_HISTORY_MAX {
        w.prices.hpi_history.drain(..n - HPI_HISTORY_MAX);
    }
    // b4-rent: rental references. In the anchor month they are set from last month's stored values (before this month's
    // update), so shocked and baseline clones share them; afterwards they follow the 120-month average. Then this month's
    // values are stored for next month's lagged reads.
    let ms_now = months_supply(w);
    let live_b4 = b4_live(w);
    {
        let st = &mut w.housing;
        if anchored_now {
            st.rental_vac_ref = st.rental_vac_hvs;
            st.ms_ref = st.ms_last;
        } else if live_b4 {
            st.rental_vac_ref += UC_REF_EMA_WEIGHT * (st.rental_vac_hvs - st.rental_vac_ref);
            st.ms_ref += UC_REF_EMA_WEIGHT * (st.ms_last - st.ms_ref);
        }
        st.rental_vac_hvs = v_hvs;
        st.ms_last = ms_now;
    }
}

// ---------------------------------------------------------------- 10. construction

/// Construction (DESIGN §10). In-progress houses consume `units / construction_months` from their firm's inventory
/// each month (slower when short). Completed houses are listed at value. Expected starts are
/// base_share × stock × (HPI / build cost, relative to t=0)³, capped at a share of stock, and go to the construction
/// firms with the most spare inventory. Each firm's backlog is set to next month's requirement.
fn construction(w: &mut World) {
    ensure_sizes(w);
    let units = w.cfg.construction_units_per_house.max(1.0);
    let months = w.cfg.construction_months.max(1) as f64;
    let per_month = units / months;
    let p_c = sector_price(w, Sector::Construction);
    let build_cost = units * p_c + w.cfg.land_share * w.prices.hpi;
    let ratio = if build_cost > 0.0 {
        (w.prices.hpi / build_cost).max(0.0)
    } else {
        0.0
    };
    if !pos(w.housing.start_ratio0) && pos(ratio) {
        w.housing.start_ratio0 = ratio;
    }

    // Advance projects, drawing on inventory.
    let mut projects = std::mem::take(&mut w.housing.projects);
    let mut residential = 0.0;
    let mut done: Vec<HouseId> = Vec::new();
    let mut keep = Vec::with_capacity(projects.len());
    for mut p in projects {
        let fm = &mut w.firms[p.firm as usize];
        let used = per_month.min(p.units_left).min(fm.inventory.max(0.0));
        fm.inventory -= used;
        p.units_left -= used;
        residential += used * p_c;
        if p.units_left <= 1e-6 * units {
            done.push(p.house);
        } else {
            keep.push(p);
        }
    }
    projects = keep;
    w.flows.residential_investment += residential;

    // Completions: the developer owns the house and lists it at value.
    for &hid in &done {
        let v = house_value_i(w, hid);
        let hs = &mut w.houses[hid as usize];
        hs.completed = true;
        hs.for_sale = true;
        hs.for_rent = false;
        hs.ask_price = v;
        hs.months_listed = 0;
        w.flows.housing_completions += 1;
    }

    // Burn-in freezes demography, so (with `burn_in_hold_stock`) the stock must not grow: each completion retires
    // one vacant rental (in house order, deterministic). Construction itself continues, so builders and their workers
    // stay at their calibrated scale, but the reported run does not start with years of surplus vacant units.
    if w.cfg.burn_in_hold_stock && in_burn_in(w) && !done.is_empty() {
        withdraw_vacant_rentals(w, done.len());
    }

    // Starts.
    let stock = (w.houses.len() - w.housing.demolished) as f64;
    let rel = if w.housing.start_ratio0 > 0.0 {
        ratio / w.housing.start_ratio0
    } else {
        1.0
    };
    // Builders also respond to vacancy, counting units already under construction (housing inventory includes the
    // pipeline — Census new-residential "for sale, under construction"), so starts do not keep rising until
    // completions arrive (the cobweb overbuild of a vacancy-only rule). The norm is the calibrated starting state.
    let vacant = w.houses.par_iter().filter(|h| h.completed && h.occupant.is_none()).count() as f64;
    let supply_rate = (vacant + projects.len() as f64) / stock.max(1.0);
    if !in_burn_in(w) && !pos(w.housing.supply_rate0) && pos(supply_rate) {
        w.housing.supply_rate0 = supply_rate;
    }
    // Before the norm is set (burn-in) the vacancy term is neutral.
    let norm = if pos(w.housing.supply_rate0) { w.housing.supply_rate0 } else { supply_rate };
    let vac_factor = (norm / supply_rate.max(0.01)).powi(2).min(2.0);
    // fix-housing: financing cost. Starts fall with the 6-month mortgage rate above its first-month level
    // (elasticity per pp; +100bp is about −15% within a year, given the lag).
    let fin_factor = if w.housing.build_rate0 > 0.0 && w.cfg.construction_rate_elasticity > 0.0 {
        let pp = 100.0 * (w.housing.build_rate_ema - w.housing.build_rate0);
        (-w.cfg.construction_rate_elasticity * pp).exp().min(2.0)
    } else {
        1.0
    };
    let expected = (w.cfg.base_starts_monthly_share * stock * rel.powi(3) * vac_factor * fin_factor)
        .min(START_CAP_SHARE * stock)
        .max(0.0);
    let expected = if expected.is_finite() { expected } else { 0.0 };
    let mut r = rng::module_rng(w.seed, w.tick, stream::HOUSING);
    let whole = expected.floor();
    let mut n_starts = whole as usize;
    if r.gen::<f64>() < expected - whole {
        n_starts += 1;
    }
    let builders: Vec<FirmId> = w
        .firms
        .iter()
        .filter(|f| f.alive && f.sector == Sector::Construction)
        .map(|f| f.id)
        .collect();
    if n_starts > 0 && !builders.is_empty() {
        let mut committed = vec![0.0f64; w.firms.len()];
        for p in &projects {
            committed[p.firm as usize] += per_month;
        }
        let mut spare: Vec<f64> = builders
            .iter()
            .map(|&f| w.firms[f as usize].inventory - committed[f as usize])
            .collect();
        for _ in 0..n_starts {
            let bi = spare
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(Ordering::Equal))
                .map(|(i, _)| i)
                .unwrap_or(0);
            let f = builders[bi];
            spare[bi] -= per_month;
            let quality = rng::lognormal_mean1(&mut r, CONSTRUCTION_QUALITY_SIGMA);
            let hid = w.houses.len() as HouseId;
            w.houses.push(House {
                id: hid,
                quality,
                owner: Owner::Firm(f),
                occupant: None,
                rent: 0.0,
                for_sale: false,
                ask_price: 0.0,
                months_listed: 0,
                for_rent: false,
                last_sale_price: 0.0,
                mortgage: None,
                completed: false,
            });
            projects.push(ConstructionProject {
                house: hid,
                firm: f,
                units_left: units,
            });
            w.flows.housing_starts += 1;
        }
    }

    // Firm backlog: next month's construction units for each builder.
    for f in w.firms.iter_mut() {
        if f.sector == Sector::Construction {
            f.backlog = 0.0;
        }
    }
    for p in &projects {
        w.firms[p.firm as usize].backlog += per_month.min(p.units_left);
    }
    w.housing.projects = projects;
}

// ---------------------------------------------------------------- invariants

/// Structural checks on housing links (DESIGN §16): each house has at most one occupant; every household's home is a
/// house whose occupant is that household; each owner's `properties` match `house.owner`. Used by tests and debugging.
pub fn check_invariants(w: &World) -> Result<(), String> {
    for (i, h) in w.households.iter().enumerate() {
        if !h.alive {
            continue;
        }
        match h.home {
            Some(hid) => {
                let Some(hs) = w.houses.get(hid as usize) else {
                    return Err(format!("household {i}: home {hid} does not exist"));
                };
                if hs.occupant != Some(i as HhId) {
                    return Err(format!("household {i}: home {hid} has occupant {:?}", hs.occupant));
                }
                if h.tenure == Tenure::WithParents {
                    return Err(format!("household {i}: has a home but tenure WithParents"));
                }
                if h.tenure == Tenure::Owner && hs.owner != Owner::Household(i as HhId) {
                    return Err(format!("household {i}: owner tenure but house {hid} owned by {:?}", hs.owner));
                }
                if h.tenure == Tenure::Renter && hs.owner == Owner::Household(i as HhId) {
                    return Err(format!("household {i}: renter tenure in its own house {hid}"));
                }
            }
            None => {
                // Without a home: a parent's child, or a former owner who sold to rent and is seeking a lease (a
                // Renter with the seeking flag set).
                let renter_seeker = h.tenure == Tenure::Renter && w.housing.seeking.get(i).copied().unwrap_or(false);
                if h.tenure != Tenure::WithParents && !renter_seeker {
                    return Err(format!("household {i}: no home but tenure {:?}", h.tenure));
                }
            }
        }
        for &p in &h.properties {
            let Some(ps) = w.houses.get(p as usize) else {
                return Err(format!("household {i}: property {p} does not exist"));
            };
            if ps.owner != Owner::Household(i as HhId) {
                return Err(format!("household {i}: lists house {p} owned by {:?}", ps.owner));
            }
        }
    }
    for (j, hs) in w.houses.iter().enumerate() {
        if let Some(o) = hs.occupant {
            let ok = w
                .households
                .get(o as usize)
                .is_some_and(|x| x.alive && x.home == Some(j as HouseId));
            if !ok {
                return Err(format!("house {j}: occupant {o} does not list it as home"));
            }
        }
        if let Owner::Household(h) = hs.owner {
            if let Some(x) = w.households.get(h as usize) {
                if x.alive && !x.properties.contains(&(j as HouseId)) {
                    return Err(format!("house {j}: owner {h} does not list it as a property"));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::banking::MortgageQuote;
    use crate::common::house_value;
    use crate::config::Config;
    use crate::sim;
    use crate::testkit::tiny_world;

    fn cfg(n: usize) -> Config {
        Config {
            n_households: n,
            ..Default::default()
        }
    }

    /// An owner-occupier (owner of its home), with or without a mortgage on the home.
    fn find_owner_occupier(w: &World, with_mortgage: bool) -> (usize, HouseId) {
        for (i, h) in w.households.iter().enumerate() {
            let Some(hid) = h.home else { continue };
            let hs = &w.houses[hid as usize];
            if h.tenure == Tenure::Owner
                && hs.occupant == Some(i as HhId)
                && hs.owner == Owner::Household(i as HhId)
                && hs.mortgage.is_some() == with_mortgage
            {
                return (i, hid);
            }
        }
        panic!("no suitable owner-occupier");
    }

    /// A renter with a home, other than `exclude`.
    /// The highest-wage renter (other than `exclude`): underwriting needs income, and the first renter in index order
    /// may not qualify for the mortgage at a given calibration.
    /// Mark the world as anchored a year ago (the clock moves on by a year), so the 3-month delinquencies in these tests
    /// began after the anchor and are not pre-anchor legacy.
    fn anchor_for_test(w: &mut World) {
        w.tick += 12;
        w.housing.anchored = true;
        w.housing.anchor_tick = w.tick - 12;
    }

    fn find_renter(w: &World, exclude: usize) -> usize {
        w.households
            .iter()
            .enumerate()
            .filter(|(i, h)| {
                h.alive
                    && h.tenure == Tenure::Renter
                    && h.home.is_some_and(|hm| w.houses[hm as usize].owner != Owner::Household(*i as HhId))
                    && h.id as usize != exclude
            })
            .max_by(|(_, a), (_, b)| a.wage.total_cmp(&b.wage))
            .map(|(i, _)| i)
            .expect("no renter")
    }

    fn cash_bid(buyer: usize) -> Bid {
        Bid {
            hh: buyer as HhId,
            price: 0.0,
            principal: 0.0,
            quote: None,
            investor: false,
        }
    }

    #[test]
    fn rent_and_property_tax_are_paid() {
        let mut w = tiny_world(cfg(500), 3);
        sim::begin_month(&mut w);
        let gov0 = w.gov.revenue;
        step(&mut w);
        let rent_in: f64 = w.hh_month.iter().map(|m| m.income.rent_received).sum();
        assert!(rent_in > 0.0, "landlords received no rent");
        assert!(w.gov.revenue > gov0, "no property tax collected");
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    #[test]
    fn unpaid_rent_leads_to_eviction() {
        let mut w = tiny_world(cfg(500), 4);
        ensure_sizes(&mut w);
        let r = find_renter(&w, usize::MAX);
        w.households[r].lease_months_left = 12;
        let d = w.households[r].deposits;
        ledger::pay(&mut w, Account::Household(r as HhId), Account::CentralBank, d);
        sim::begin_month(&mut w);
        rents_and_taxes(&mut w);
        assert_eq!(w.households[r].tenure, Tenure::Renter, "evicted after one month");
        sim::begin_month(&mut w);
        rents_and_taxes(&mut w);
        assert_eq!(w.households[r].tenure, Tenure::WithParents);
        assert!(w.households[r].home.is_none());
        assert!(w.housing.seeking[r]);
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    /// An owner who lists to exit ownership becomes a renter seeking a lease at the sale, is not a mover, and cannot
    /// bid to buy again during the cooldown. A mover's sale still starts the buy-sell chain.
    #[test]
    fn owner_exit_sale_makes_a_renter_without_a_rebuy() {
        let mut w = tiny_world(cfg(500), 7);
        ensure_sizes(&mut w);
        let (seller, house) = find_owner_occupier(&w, false);
        let buyer = find_renter(&w, seller);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 10_000_000.0);
        let v = house_value(&w, house);
        let hs = &mut w.houses[house as usize];
        hs.for_sale = true;
        hs.completed = true;
        hs.ask_price = v;
        exit_at_listing(&mut w, house, seller as HhId);
        assert_eq!(w.households[seller].tenure, Tenure::Renter, "the exit takes effect at listing");
        assert_eq!(w.housing.exit_cooldown[seller], EXIT_REBUY_COOLDOWN_MONTHS);
        check_invariants(&w).unwrap();

        assert_eq!(execute_purchase(&mut w, &cash_bid(buyer), house), Some(false));

        assert_eq!(w.households[seller].tenure, Tenure::Renter);
        assert!(w.households[seller].home.is_none());
        assert!(w.housing.seeking[seller], "former owner should be seeking a lease");
        assert_eq!(w.housing.recent_seller[seller], 0, "an exit sale is not a move");
        assert_eq!(w.housing.exit_cooldown[seller], EXIT_REBUY_COOLDOWN_MONTHS);
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    /// Fix-housing review 6: a delinquent investor's let unit is a plain distressed listing. Its sale leaves the
    /// investor's own home and tenure untouched.
    #[test]
    fn distressed_sale_of_a_let_unit_is_not_an_owner_exit() {
        let mut c = cfg(500);
        c.housing_distress_sale_prob = 1.0;
        c.housing_foreclosure_prob = 0.0;
        let mut w = tiny_world(c, 8);
        ensure_sizes(&mut w);
        let (h, home) = find_owner_occupier(&w, true);
        // Another house, owned by someone else and not anyone's home, becomes the investor's let unit with a mortgage.
        let let_unit = (0..w.houses.len() as HouseId)
            .find(|&p| {
                let hs = &w.houses[p as usize];
                p != home && hs.owner != Owner::Household(h as HhId) && !matches!(hs.owner, Owner::Household(x) if Some(x) == hs.occupant)
            })
            .expect("tiny world has a house that is not an owner's home");
        release_occupant(&mut w, let_unit);
        ledger::transfer_house(&mut w, let_unit, Owner::Household(h as HhId));
        let loan = ledger::create_loan(&mut w, 0, Borrower::Household(h as HhId), LoanKind::Mortgage { house: let_unit }, 1.0, 0.06, 300);
        w.houses[let_unit as usize].mortgage = Some(loan);
        w.houses[let_unit as usize].occupant = None;
        let principal = 0.5 * house_value(&w, let_unit);
        // Only the let unit's loan is in arrears (the test world may have other delinquent loans).
        let others: Vec<LoanId> = w.loans.iter().map(|(lid, _)| lid).filter(|&lid| lid != loan).collect();
        for lid in others {
            w.loans.get_mut(lid).unwrap().months_delinquent = 0;
        }
        {
            let l = w.loans.get_mut(loan).unwrap();
            l.months_delinquent = 3;
            l.principal = principal;
        }
        w.households[h].status = LaborStatus::Employed;
        w.tick = w.cfg.burn_in_months;
        w.housing.uc0 = 0.05;
        anchor_for_test(&mut w);
        foreclosures(&mut w);
        assert!(w.houses[let_unit as usize].for_sale, "delinquent let unit with equity should list");
        assert_eq!(w.houses[let_unit as usize].occupant, None);
        assert_eq!(w.households[h].tenure, Tenure::Owner, "the investor keeps the tenure of the home they live in");
        assert_eq!(w.households[h].home, Some(home));
        check_invariants(&w).unwrap();
    }

    /// A tenant who inherits the home they rent becomes an owner-occupier (`settle_own_rental`), so tenure and ownership
    /// agree afterwards.
    #[test]
    fn tenant_who_inherits_their_rental_becomes_an_owner() {
        let mut w = tiny_world(cfg(500), 7);
        ensure_sizes(&mut w);
        let r = find_renter(&w, usize::MAX);
        let home = w.households[r].home.unwrap();
        ledger::transfer_house(&mut w, home, Owner::Household(r as HhId));
        assert!(check_invariants(&w).is_err(), "a renter who owns the rented home is a stale state");
        settle_own_rental(&mut w);
        assert_eq!(w.households[r].tenure, Tenure::Owner);
        assert_eq!(w.households[r].rent, 0.0);
        check_invariants(&w).unwrap();
    }

    /// Fix-housing review 6: an exit listing that passes to an heir by death does not make the heir's sale a renter
    /// transition.
    #[test]
    fn inherited_exit_listing_does_not_change_the_heirs_tenure() {
        let mut w = tiny_world(cfg(500), 7);
        ensure_sizes(&mut w);
        let (seller, house) = find_owner_occupier(&w, false);
        let buyer = find_renter(&w, seller);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 10_000_000.0);
        let v = house_value(&w, house);
        let hs = &mut w.houses[house as usize];
        hs.for_sale = true;
        hs.completed = true;
        hs.ask_price = v;
        exit_at_listing(&mut w, house, seller as HhId);
        // The house passes to a child living with parents (no home of their own), as at a death.
        let heir = (0..w.households.len())
            .find(|&i| i != seller && i != buyer && w.households[i].alive && w.households[i].home.is_none())
            .expect("tiny world has a household without a home");
        w.households[heir].tenure = Tenure::WithParents;
        ledger::transfer_house(&mut w, house, Owner::Household(heir as HhId));
        update_listings(&mut w);
        assert_eq!(execute_purchase(&mut w, &cash_bid(buyer), house), Some(false));
        assert_eq!(w.households[heir].tenure, Tenure::WithParents, "the heir's tenure is unchanged by the sale");
        check_invariants(&w).unwrap();
    }

    /// fix-housing (review 9, fix 1): the idiosyncratic house values are cross-sectionally mean zero, their value
    /// multipliers average one (so the index is not biased), and they move from month to month.
    #[test]
    fn idiosyncratic_values_are_mean_zero_and_average_one() {
        let mut w = tiny_world(cfg(500), 3);
        ensure_sizes(&mut w);
        ensure_idio(&mut w);
        let before = w.housing.house_idio.clone();
        for _ in 0..24 {
            w.tick += 1;
            update_house_values(&mut w);
        }
        let n = w.housing.house_idio.len() as f64;
        let mean = w.housing.house_idio.iter().sum::<f64>() / n;
        assert!(mean.abs() < 1e-9, "deviations are demeaned across houses");
        let avg_mult = (0..w.houses.len()).map(|h| idio_mult(&w.housing, h)).sum::<f64>() / n;
        assert!((avg_mult - 1.0).abs() < 1e-9, "value multipliers average one");
        assert!(w.housing.house_idio.iter().zip(&before).any(|(a, b)| (a - b).abs() > 1e-6), "deviations move");
    }

    #[test]
    fn cash_purchase_transfers_title_and_money() {
        let mut w = tiny_world(cfg(500), 5);
        ensure_sizes(&mut w);
        let (seller, house) = find_owner_occupier(&w, false);
        let buyer = find_renter(&w, seller);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 10_000_000.0);
        let v = house_value(&w, house);
        let hs = &mut w.houses[house as usize];
        hs.for_sale = true;
        hs.completed = true;
        hs.ask_price = v;
        let s0 = w.households[seller].deposits;
        let b0 = w.households[buyer].deposits;
        let tax0 = w.gov.revenue;

        assert_eq!(execute_purchase(&mut w, &cash_bid(buyer), house), Some(false));

        assert_eq!(w.houses[house as usize].owner, Owner::Household(buyer as HhId));
        assert!((w.households[seller].deposits - (s0 + v)).abs() < 1e-6, "seller not paid");
        assert!((w.households[buyer].deposits - (b0 - 1.01 * v)).abs() < 1e-6, "buyer cash wrong");
        assert!((w.gov.revenue - tax0 - 0.01 * v).abs() < 1e-6, "transfer tax wrong");
        assert_eq!(w.households[seller].tenure, Tenure::WithParents);
        assert!(w.households[seller].home.is_none());
        assert_eq!(w.households[buyer].tenure, Tenure::Owner);
        assert_eq!(w.households[buyer].home, Some(house));
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    #[test]
    fn sale_repays_seller_mortgage() {
        let mut w = tiny_world(cfg(500), 6);
        ensure_sizes(&mut w);
        let (seller, house) = find_owner_occupier(&w, true);
        let buyer = find_renter(&w, seller);
        let loan = w.houses[house as usize].mortgage.unwrap();
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 10_000_000.0);
        let v = house_value(&w, house);
        let hs = &mut w.houses[house as usize];
        hs.for_sale = true;
        hs.completed = true;
        hs.ask_price = v;

        assert_eq!(execute_purchase(&mut w, &cash_bid(buyer), house), Some(false));

        assert!(!w.loans.contains(loan), "seller's loan not paid off");
        assert!(w.houses[house as usize].mortgage.is_none());
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    #[test]
    fn hpi_moves_toward_sale_prices() {
        // Same market tightness, different sale prices: high sale-to-value ratios raise the HPI more.
        let base = tiny_world(cfg(500), 7);
        let h0 = base.prices.hpi;
        let mut hi = base.clone();
        let mut lo = base.clone();
        let n0 = hi.prices.hpi_history.len();
        update_indices(&mut hi, &[h0 * 1.2; 10], &[]);
        update_indices(&mut lo, &[h0 * 0.8; 10], &[]);
        assert!(hi.prices.hpi > lo.prices.hpi);
        assert_eq!(hi.prices.hpi_history.len(), n0 + 1);
        // Monthly moves are bounded (±15%/yr).
        assert!((hi.prices.hpi / h0 - 1.0).abs() <= 0.15 / 12.0 + 1e-12);
        // Too few observations: only tightness and expected inflation move the index (no sale-price effect).
        let mut a = base.clone();
        let mut b = base.clone();
        update_indices(&mut a, &[h0 * 2.0; 3], &[]);
        update_indices(&mut b, &[h0 * 0.5; 3], &[]);
        assert!((a.prices.hpi - b.prices.hpi).abs() < 1e-9);
    }

    // Round 7 step 7: a distressed borrower is not foreclosed during burn-in.
    #[test]
    fn burn_in_does_not_foreclose() {
        let mut w = tiny_world(cfg(500), 8);
        w.cfg.burn_in_months = 10_000;
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.households[h].status = LaborStatus::Unemployed;
        sim::begin_month(&mut w);
        for _ in 0..200 {
            foreclosures(&mut w);
        }
        assert_eq!(w.houses[house as usize].owner, Owner::Household(h as HhId));
        assert_eq!(w.flows.foreclosures, 0);
    }

    #[test]
    fn foreclosure_repossesses_and_sale_writes_off_loan() {
        let mut w = tiny_world(cfg(500), 8);
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.households[h].status = LaborStatus::Unemployed;
        sim::begin_month(&mut w);

        // A foreclosure start is a monthly draw (FORECLOSURE_START_PROB), so the eligible loan is re-tested month by
        // month until the draw fires: 0.85^200 ≈ 1e-14 for it not to fire within 200 months.
        for _ in 0..200 {
            if w.houses[house as usize].owner != Owner::Household(h as HhId) {
                break;
            }
            w.tick += 1;
            foreclosures(&mut w);
        }

        let bank = w.loans.get(loan).unwrap().bank;
        assert_eq!(w.houses[house as usize].owner, Owner::Bank(bank));
        assert!(w.households[h].home.is_none());
        assert_eq!(w.households[h].tenure, Tenure::WithParents);
        assert_eq!(w.housing.reo_loan[house as usize], Some(loan));
        assert!(w.flows.foreclosures >= 1);
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();

        // The bank sells to a cash buyer: the loan is written off net of proceeds.
        let buyer = find_renter(&w, h);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 10_000_000.0);
        let price = w.houses[house as usize].ask_price;
        let principal = w.loans.get(loan).unwrap().principal;
        let loss0 = w.banks[bank as usize].loan_losses;
        assert_eq!(execute_purchase(&mut w, &cash_bid(buyer), house), Some(true));
        assert!(!w.loans.contains(loan));
        let loss = w.banks[bank as usize].loan_losses - loss0;
        assert!((loss - (principal - price).max(0.0)).abs() < 1e-6, "loss {loss}");
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    #[test]
    fn construction_starts_and_completes() {
        let mut w = tiny_world(cfg(500), 9);
        sim::begin_month(&mut w);
        step(&mut w);
        assert!(w.housing.start_ratio0 > 0.0);

        // HPI triples: starts run well above the base rate.
        w.prices.hpi *= 3.0;
        sim::begin_month(&mut w);
        step(&mut w);
        assert!(w.flows.housing_starts > 0, "no starts during a boom");
        assert!(!w.housing.projects.is_empty());
        let p = w.housing.projects[0];
        assert!(w.firms[p.firm as usize].backlog > 0.0);
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();

        // A project with almost nothing left completes, and the developer lists the house.
        let hid = p.house;
        w.firms[p.firm as usize].inventory = 1e9;
        w.housing.projects[0].units_left = 1.0;
        sim::begin_month(&mut w);
        construction(&mut w);
        assert!(w.houses[hid as usize].completed);
        assert!(w.houses[hid as usize].for_sale);
        assert!(w.flows.housing_completions >= 1);
    }

    #[test]
    fn seeker_rents_a_vacant_unit() {
        let mut w = tiny_world(cfg(500), 10);
        ensure_sizes(&mut w);
        let h = w
            .households
            .iter()
            .position(|x| x.alive && x.tenure == Tenure::WithParents && x.home.is_none())
            .unwrap();
        let k = w
            .houses
            .iter()
            .position(|x| {
                x.occupant.is_none() && x.completed && !x.for_sale && x.owner != Owner::Household(h as HhId)
            })
            .unwrap();
        w.houses[k].for_rent = true;
        w.housing.seeking[h] = true;
        w.households[h].permanent_income = 20_000.0;
        sim::begin_month(&mut w);
        let mut leases = Vec::new();
        rental_market(&mut w, &mut leases);
        assert!(w.households[h].home.is_some(), "seeker found nothing");
        assert_eq!(w.households[h].tenure, Tenure::Renter);
        assert!(!leases.is_empty());
        check_invariants(&w).unwrap();
    }

    #[test]
    fn mortgage_financed_purchase() {
        // Integration with banking: a quoted mortgage funds the purchase and is linked to the house.
        let mut w = tiny_world(cfg(500), 12);
        ensure_sizes(&mut w);
        let (seller, house) = find_owner_occupier(&w, false);
        let buyer = find_renter(&w, seller);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(buyer as HhId), 200_000.0);
        let v = house_value(&w, house);
        let hs = &mut w.houses[house as usize];
        hs.for_sale = true;
        hs.completed = true;
        hs.ask_price = v;
        let quote = MortgageQuote {
            bank: 0,
            rate: 0.05,
            max_loan: 0.8 * v,
        };
        let bid = Bid {
            hh: buyer as HhId,
            price: v,
            principal: 0.8 * v,
            quote: Some(quote),
            investor: false,
        };
        let loans0 = w.loans.len();
        let _ = execute_purchase(&mut w, &bid, house);
        assert_eq!(w.houses[house as usize].owner, Owner::Household(buyer as HhId));
        let m = w.houses[house as usize].mortgage.expect("purchase should be mortgage-financed");
        assert_eq!(w.loans.get(m).unwrap().borrower, Borrower::Household(buyer as HhId));
        assert!(w.loans.len() >= loans0);
        assert_eq!(w.flows.house_sales, 1);
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    /// fix-housing: a mortgagor whose rate is well above the market refinances: the old loan is repaid, the house links
    /// to a new loan at the market rate, and the ledger stays consistent.
    #[test]
    fn refinance_replaces_old_loan_at_market_rate() {
        let mut w = tiny_world(cfg(500), 21);
        ensure_sizes(&mut w);
        let (owner, house) = find_owner_occupier(&w, true);
        let old = w.houses[house as usize].mortgage.unwrap();
        // Old rate 2pp above a market rate 1pp lower than the bank's current one: far outside the threshold.
        let market = w.banks.iter().filter(|b| b.alive).map(|b| b.mortgage_rate).fold(f64::INFINITY, f64::min);
        let market = if market.is_finite() && market > 0.0 { market } else { 0.06 };
        for b in w.banks.iter_mut() {
            b.mortgage_rate = market;
        }
        w.loans.get_mut(old).unwrap().annual_rate = market + 0.02;
        w.households[owner].status = LaborStatus::Employed;
        w.households[owner].months_unemployed = 0;
        w.households[owner].bls_unemployed = 0;
        let refi0 = w.flows.refinances;
        // Refinancing is probabilistic: the candidate gate is the monthly draw, so try enough ticks for one to pass.
        for _ in 0..200 {
            if w.flows.refinances > refi0 {
                break;
            }
            w.houses[house as usize].mortgage = Some(old);
            w.loans.get_mut(old).unwrap().annual_rate = market + 0.02;
            w.loans.get_mut(old).unwrap().months_delinquent = 0;
            w.tick += 1;
            let _ = refinance(&mut w);
        }
        assert!(w.flows.refinances > refi0, "no refinance was accepted");
        let new = w.houses[house as usize].mortgage.expect("house lost its mortgage");
        assert_ne!(new, old, "the old loan is still linked to the house");
        assert!(!w.loans.contains(old), "the old loan was not repaid");
        let l = w.loans.get(new).unwrap();
        assert_eq!(l.borrower, Borrower::Household(owner as HhId));
        assert!((l.annual_rate - market).abs() < 1e-12, "new loan not at the market rate");
        ledger::check_consistency(&w).unwrap();
        check_invariants(&w).unwrap();
    }

    /// Post-anchor delinquency (fix-housing review 5): a delinquent borrower with equity lists the house for a distressed
    /// sale and leaves ownership at listing, without a foreclosure.
    #[test]
    fn post_anchor_delinquent_owner_with_equity_lists_for_distressed_sale() {
        let mut c = cfg(500);
        c.housing_distress_sale_prob = 1.0;
        c.housing_foreclosure_prob = 1.0;
        let mut w = tiny_world(c, 8);
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.loans.get_mut(loan).unwrap().principal = 0.5 * house_value(&w, house);
        w.households[h].status = LaborStatus::Employed;
        w.tick = w.cfg.burn_in_months;
        w.housing.uc0 = 0.05;
        anchor_for_test(&mut w);
        foreclosures(&mut w);
        assert!(w.houses[house as usize].for_sale, "delinquent owner with equity should list the house");
        assert!(w.houses[house as usize].occupant.is_none(), "the owner's home is vacated at listing");
        assert_eq!(w.households[h].tenure, Tenure::Renter);
        assert_eq!(w.flows.foreclosures, 0);
        assert!(w.housing.reo_loan[house as usize].is_none());
    }

    /// Post-anchor foreclosure trigger: delinquency in negative equity (net of selling costs) forecloses even while
    /// employed, and delinquency with equity does not (it sells instead).
    #[test]
    fn post_anchor_foreclosure_needs_negative_equity() {
        let mut c = cfg(500);
        c.housing_distress_sale_prob = 0.0;
        c.housing_foreclosure_prob = 1.0;
        let mut w = tiny_world(c, 8);
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.households[h].status = LaborStatus::Employed;
        w.tick = w.cfg.burn_in_months;
        w.housing.uc0 = 0.05;
        anchor_for_test(&mut w);
        // Positive equity: no foreclosure (the distressed-sale draw is off in this config). The cure draw may reset the
        // delinquency, so it is set again before the negative-equity test.
        w.loans.get_mut(loan).unwrap().principal = 0.8 * house_value(&w, house);
        foreclosures(&mut w);
        assert_eq!(w.flows.foreclosures, 0, "employed borrower with equity was foreclosed");
        // Negative equity: foreclosed at once (the draw is 1).
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.loans.get_mut(loan).unwrap().principal = 1.2 * house_value(&w, house);
        foreclosures(&mut w);
        assert_eq!(w.flows.foreclosures, 1, "negative-equity delinquency should foreclose");
        assert!(w.housing.reo_loan[house as usize].is_some());
    }

    /// A delinquency that began before the anchor (more months owed than months since the anchor) is not subject to the
    /// post-anchor paths in its first months: it is not foreclosed in a burst at the anchor.
    #[test]
    fn pre_anchor_delinquency_is_not_foreclosed_at_the_anchor() {
        let mut c = cfg(500);
        c.housing_distress_sale_prob = 1.0;
        c.housing_foreclosure_prob = 1.0;
        let mut w = tiny_world(c, 8);
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 20;
        w.loans.get_mut(loan).unwrap().principal = 1.2 * house_value(&w, house);
        w.households[h].status = LaborStatus::Employed;
        w.tick = w.cfg.burn_in_months;
        w.housing.uc0 = 0.05;
        w.housing.anchored = true;
        w.housing.anchor_tick = w.tick;
        foreclosures(&mut w);
        assert_eq!(w.flows.foreclosures, 0, "a burn-in delinquency is not foreclosed in the anchor month");
        assert!(!w.houses[house as usize].for_sale);
    }

    /// A delinquency that is not foreclosed or sold cures at DELINQUENCY_CURE_PROB a month, and the loan is kept.
    #[test]
    fn post_anchor_delinquency_cures_without_foreclosure() {
        let mut c = cfg(500);
        c.housing_distress_sale_prob = 0.0;
        c.housing_foreclosure_prob = 0.0;
        let mut w = tiny_world(c, 8);
        ensure_sizes(&mut w);
        let (h, house) = find_owner_occupier(&w, true);
        let loan = w.houses[house as usize].mortgage.unwrap();
        w.loans.get_mut(loan).unwrap().months_delinquent = 3;
        w.households[h].status = LaborStatus::Unemployed;
        w.loans.get_mut(loan).unwrap().principal = 0.8 * house_value(&w, house);
        w.tick = w.cfg.burn_in_months;
        w.housing.uc0 = 0.05;
        anchor_for_test(&mut w);
        let mut cured = false;
        for _ in 0..200 {
            w.tick += 1;
            foreclosures(&mut w);
            if w.loans.get(loan).map_or(false, |l| l.months_delinquent == 0) {
                cured = true;
                break;
            }
        }
        assert!(cured, "delinquency never cured");
        assert_eq!(w.flows.foreclosures, 0);
        assert!(w.loans.contains(loan));
    }

    /// fix-housing: a shock in the first reported month (already in the market rate) must not move the anchors, so a
    /// shocked clone and its baseline share the same user-cost and construction references.
    #[test]
    fn m0_shock_does_not_move_anchors() {
        let mut base = tiny_world(cfg(500), 9);
        ensure_sizes(&mut base);
        for b in base.banks.iter_mut().filter(|b| b.alive) {
            b.mortgage_rate = 0.06;
        }
        // Two updates populate the EMAs (the first sets them, the second may set the anchors from them).
        update_indices(&mut base, &[], &[]);
        let mut shocked = base.clone();
        for b in shocked.banks.iter_mut() {
            b.mortgage_rate += 0.01;
        }
        for w in [&mut base, &mut shocked] {
            update_indices(w, &[], &[]);
        }
        assert!(base.housing.uc0 > 0.0, "anchor not set");
        assert_eq!(base.housing.uc0, shocked.housing.uc0, "user-cost anchor moved by an m0 shock");
        assert_eq!(base.housing.build_rate0, shocked.housing.build_rate0, "construction norm moved by an m0 shock");
        assert!(shocked.housing.uc_mortgage_ema > base.housing.uc_mortgage_ema, "shock did not reach the EMA");
    }

    /// fix-housing: the rent-anchor factor is bounded, continuous through zero user cost, and neutral before uc0 is set.
    #[test]
    fn user_cost_factor_is_bounded_and_continuous() {
        let theta = 0.5;
        assert_eq!(user_cost_factor(0.05, 0.0, theta), 1.0, "neutral before uc0");
        let uc0 = 0.05;
        let lo = UC_RATIO_MIN.powf(theta);
        let hi = UC_RATIO_MAX.powf(theta);
        let mut prev = user_cost_factor(-0.10, uc0, theta);
        assert!((prev - lo).abs() < 1e-12, "non-positive user cost sits on the floor");
        let mut uc = -0.10;
        while uc <= 0.20 {
            let f = user_cost_factor(uc, uc0, theta);
            assert!(f >= lo - 1e-12 && f <= hi + 1e-12, "factor out of bounds at uc {uc}");
            assert!((f - prev).abs() < 0.01, "factor jumps at uc {uc}: {prev} -> {f}");
            prev = f;
            uc += 0.0005;
        }
        assert!((user_cost_factor(uc0, uc0, theta) - 1.0).abs() < 1e-12, "factor is 1 at the reference user cost");
    }

    #[test]
    fn invariants_hold_over_months() {
        let mut w = tiny_world(cfg(500), 11);
        for _ in 0..24 {
            sim::step(&mut w);
            check_invariants(&w).unwrap();
        }
        assert!(months_supply(&w).is_finite());
    }

    // ---------------------------------------------------------------- b4-rent (rental side)

    #[test]
    fn vacant_sell_prob_is_master_when_off_or_not_live() {
        let off = cfg(10);
        let mut on = cfg(10);
        on.housing_vacant_sell_reanchor = true;
        on.housing_vacant_sell_ms_elast = 1.0;
        for ms in [0.5, 2.0, 3.0, 4.0, 6.0] {
            let leg = master_vacant_sell_prob(ms);
            assert_eq!(vacant_sell_prob(&off, true, ms, 3.2, 0.08, 0.08), leg, "off at ms {ms}");
            assert_eq!(vacant_sell_prob(&on, false, ms, 3.2, 0.08, 0.08), leg, "not live at ms {ms}");
        }
    }

    #[test]
    fn vacant_sell_prob_at_references_and_elasticity() {
        let mut c = cfg(10);
        c.housing_vacant_sell_reanchor = true;
        c.housing_vacant_sell_ms_elast = 1.5;
        let m = 3.2;
        // At the references the hazard is the master hazard at the reference months of supply.
        assert!((vacant_sell_prob(&c, true, m, m, 0.08, 0.08) - master_vacant_sell_prob(m)).abs() < 1e-12);
        // Log-slope in months of supply is −η away from the clamps.
        let (a, b) = (3.2, 3.2 * 1.01);
        let (pa, pb) = (vacant_sell_prob(&c, true, a, m, 0.08, 0.08), vacant_sell_prob(&c, true, b, m, 0.08, 0.08));
        let slope = (pb / pa).ln() / (b / a).ln();
        assert!((slope + 1.5).abs() < 1e-9, "slope {slope}");
        // Clamps.
        assert_eq!(vacant_sell_prob(&c, true, 0.25, m, 0.08, 0.08), VSELL_P_MAX);
        assert_eq!(vacant_sell_prob(&c, true, 100.0, m, 0.08, 0.08), VSELL_P_MIN);
        // Loose rentals (vacancy above its reference) raise the sale hazard when κ_s > 0: a stabilising exit.
        c.housing_rental_gap_sell_semi = 0.2;
        assert!(vacant_sell_prob(&c, true, m, m, 0.10, 0.08) > vacant_sell_prob(&c, true, m, m, 0.08, 0.08));
    }

    #[test]
    fn buy_rate_mult_is_neutral_without_beta_or_anchor_and_bounded() {
        let mut w = tiny_world(cfg(200), 3);
        w.housing.build_rate0 = 0.06;
        w.housing.build_rate_ema = 0.07;
        w.cfg.housing_buy_rate_semi = 0.2;
        assert_eq!(buy_rate_mult(&w), 1.0, "not anchored");
        anchor_for_test(&mut w);
        let up = buy_rate_mult(&w);
        assert!((up - (-0.2f64).exp()).abs() < 1e-12, "one pp above reference: {up}");
        w.cfg.housing_buy_rate_semi = 0.0;
        assert_eq!(buy_rate_mult(&w), 1.0, "beta zero");
        w.cfg.housing_buy_rate_semi = 10.0;
        w.housing.build_rate_ema = 0.20;
        assert_eq!(buy_rate_mult(&w), BUY_MULT_MIN, "floor");
        w.housing.build_rate_ema = 0.0;
        assert_eq!(buy_rate_mult(&w), BUY_MULT_MAX, "ceiling");
    }

    #[test]
    fn expiry_let_prob_is_one_when_off_or_not_above_reference_and_floored() {
        let mut c = cfg(10);
        assert_eq!(expiry_let_prob(&c, true, 0.12, 0.08), 1.0, "kappa zero");
        c.housing_expiry_let_gap_semi = 0.5;
        assert_eq!(expiry_let_prob(&c, true, 0.07, 0.08), 1.0, "vacancy below reference");
        assert_eq!(expiry_let_prob(&c, false, 0.12, 0.08), 1.0, "not live");
        assert!((expiry_let_prob(&c, true, 0.09, 0.08) - (-0.5f64).exp()).abs() < 1e-12);
        assert_eq!(expiry_let_prob(&c, true, 0.5, 0.08), EXPIRY_LET_MIN, "floor");
    }

    #[test]
    fn b4_rent_mechanisms_at_grid_maximum_keep_invariants() {
        let mut c = cfg(400);
        c.housing_buy_rate_semi = 0.4;
        c.housing_vacant_sell_reanchor = true;
        c.housing_vacant_sell_ms_elast = 2.0;
        c.housing_rental_gap_sell_semi = 0.2;
        c.housing_expiry_let_gap_semi = 0.5;
        let mut w = tiny_world(c, 5);
        for _ in 0..36 {
            sim::step(&mut w);
            check_invariants(&w).unwrap();
            crate::ledger::check_consistency(&w).unwrap();
        }
        assert!(w.housing.anchored, "the mechanisms must have run");
        assert!(w.housing.ms_ref > 0.0 && w.housing.rental_vac_ref > 0.0);
    }
}

/// Diagnostics: (listings, bids, median ask, median bid, max bid, median ask/quality, bids from movers).
pub fn market_diagnostics(w: &World) -> (usize, usize, f64, f64, f64, f64, usize) {
    let mut asks: Vec<f64> = w.houses.iter().filter(|h| h.for_sale && h.completed && h.ask_price > 0.0).map(|h| h.ask_price).collect();
    let mut apq: Vec<f64> = w.houses.iter().filter(|h| h.for_sale && h.completed && h.ask_price > 0.0).map(|h| h.ask_price / h.quality).collect();
    let bids = compute_bids(w);
    let movers = bids.iter().filter(|b| w.housing.recent_seller.get(b.hh as usize).copied().unwrap_or(0) > 0).count();
    let mut bp: Vec<f64> = bids.iter().map(|b| b.price).collect();
    let mx = bp.iter().cloned().fold(0.0, f64::max);
    (asks.len(), bids.len(), median(&mut asks), median(&mut bp), mx, median(&mut apq), movers)
}

/// Diagnostics: tenure counts (owners, renters, with-parents, homeless-renters) and vacant completed units by
/// state (for_rent, for_sale, idle) and owner type (household, firm, bank).
pub fn tenure_diagnostics(w: &World) -> String {
    let alive = || w.households.iter().filter(|h| h.alive);
    let owners = alive().filter(|h| h.tenure == Tenure::Owner).count();
    let renters = alive().filter(|h| h.tenure == Tenure::Renter && h.home.is_some()).count();
    let renters_no_home = alive().filter(|h| h.tenure == Tenure::Renter && h.home.is_none()).count();
    let parents = alive().filter(|h| h.tenure == Tenure::WithParents).count();
    let parents25 = alive().filter(|h| h.tenure == Tenure::WithParents && h.age_years() >= 25.0).count();
    let vac = || w.houses.iter().filter(|h| h.completed && h.occupant.is_none());
    let (fr, fs) = (vac().filter(|h| h.for_rent).count(), vac().filter(|h| h.for_sale).count());
    let idle = vac().filter(|h| !h.for_rent && !h.for_sale).count();
    let (vh, vf, vb) = (
        vac().filter(|h| matches!(h.owner, Owner::Household(_))).count(),
        vac().filter(|h| matches!(h.owner, Owner::Firm(_))).count(),
        vac().filter(|h| matches!(h.owner, Owner::Bank(_))).count(),
    );
    let seeking = w.housing.seeking.iter().filter(|&&x| x).count();
    format!(
        "owners {owners} renters {renters} renters_unhoused {renters_no_home} with_parents {parents} (25+: {parents25}) seeking {seeking} | vacant: for_rent {fr} for_sale {fs} idle {idle} | by owner hh {vh} firm {vf} bank {vb} | houses {} building {}",
        w.houses.len(),
        w.housing.projects.len()
    )
}
