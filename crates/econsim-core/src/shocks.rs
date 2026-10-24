//! User-facing policy & price shocks (docs/DESIGN.md §12).
//!
//! Shocks are scheduled at a month; `ShockState::activate_due` (called from exog::step) turns them into
//! active effects that the relevant modules read through the accessor methods below.

use serde::{Deserialize, Serialize};

use crate::types::{N_SECTORS, N_TENORS, TENORS};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Shock {
    /// Jump in log oil price by ln(1+pct/100), decaying with the given half-life (0 = permanent).
    OilPrice { pct: f64, half_life_months: f64 },
    /// Pin the policy rate (annual decimal) for `months` months, overriding the Taylor rule.
    PolicyRate { rate: f64, months: u32 },
    /// Monetary-policy innovation of `bps` to the Taylor rule's shadow rate (VAR / CEE sense; fix-monetary). The policy
    /// rate jumps by `bps` on impact. The rate gap then decays as an AR(1) with persistence rho = 1 − 1/months, so
    /// `months` is the mean duration of the gap in months (12 gives rho ≈ 0.92). The rule's inertia carries the gap
    /// and the rule keeps responding to the economy underneath; there is no expiry.
    PolicyShift { bps: f64, months: u32 },
    /// Central-bank purchases of agency MBS of `monthly_face_pct_gdp` % of annual GDP per month for `months` months
    /// (negative = sales). Moves the mortgage spread through MonetaryState::mbs_spread_adj; no money is moved (the model
    /// has no MBS asset), so the ledger is unchanged.
    MbsQE { monthly_face_pct_gdp: f64, months: u32 },
    /// Change Taylor-rule coefficients permanently.
    TaylorRule { phi_pi: f64, phi_u: f64, r_star: f64 },
    /// Term-premium shift (basis points) at one tenor index (0=3m,1=2y,2=5y,3=10y,4=30y); neighbours get 50%.
    TermPremium { tenor_idx: usize, bps: f64, half_life_months: f64 },
    /// Central bank buys `monthly_face_pct_gdp` % of annual GDP per month of the tenor for `months` (negative = QT).
    QE { tenor_idx: usize, monthly_face_pct_gdp: f64, months: u32 },
    /// Central-bank-financed transfer of `per_household` $ per household per month for `months`.
    Helicopter { per_household: f64, months: u32 },
    /// RZ23: Treasury-financed one-off payment (a "stimulus check" or tax rebate) of `per_household` $ (t=0 dollars, as `Helicopter`) to
    /// every household per month for `months`. It is paid out of the Treasury's account and the account is refilled by the usual bond
    /// auctions (`fiscal::collect_and_finance`), so unlike `Helicopter` it creates no central-bank money: the deficit and the debt rise
    /// and the buyers' deposits fall. The households spend it by the same windfall rule as `Helicopter` (`consumption::decide`).
    Rebate { per_household: f64, months: u32 },
    /// Government purchases multiplied by (1+pct/100) for `months`.
    GovSpending { pct: f64, months: u32 },
    /// Add `pp` percentage points to every income-tax bracket for `months`.
    IncomeTax { pp: f64, months: u32 },
    /// Replace mortgage limits (permanent).
    MacroPrudential { max_ltv: f64, max_dti: f64 },
    /// Add bps to mortgage and business loan spreads for `months`.
    CreditSpread { bps: f64, months: u32 },
    /// Permanent productivity level shock (pct) to one sector index or all (None).
    Productivity { sector: Option<usize>, pct: f64 },
    /// Tariff on imports (pct) — raises domestic price of imported goods and import costs, permanent.
    ImportTariff { pct: f64 },
    /// Shift the exchange-rate target by ln(1+pct/100) (positive = stronger dollar), decaying with the given
    /// half-life (0 = permanent). Acts through `fx::exchange_rate`.
    Dollar { pct: f64, half_life_months: f64 },
    /// Force a technology boom (exog::step: Tech and adopter productivity growth up, sector equity re-rated) for
    /// `months` months, whether or not the Markov regime is enabled.
    TechBoom { months: u32 },
    /// R51-E: re-rate share prices. The equity valuation of one sector (`Sector::ALL` order 0-8) or of all sectors (None) jumps by
    /// ln(1 + pct/100) on impact (-80 gives a fall of 80%), and the jump decays with the given half-life (0 = permanent), as `Dollar` and
    /// `OilPrice` do. It acts in `equity::value_sectors` as a multiplicative wedge on each sector's model value, after the valuation:
    /// dividends, the growth signal, the discount rate and the premium are not touched, so the earnings, rate and risk-premium channels
    /// keep working underneath, and the re-rating reaches the Fund's NAV and share price (households' wealth and spending), Tobin's q
    /// (investment) and the 12-month return that feeds the risk premium.
    SharePrices { sector: Option<usize>, pct: f64, half_life_months: f64 },
    /// b8-world: the bloc's policy rate held `bps` above its rule for `months` months, through the pin table of the bloc's
    /// solution (spec 1.6). `bloc` is one of EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW. Ignored when the block is off.
    BlocPolicy { bloc: String, bps: f64, months: u32 },
    /// b8-world: IS demand shock on one bloc, scaled so that the isolated bloc's peak gap equals `pct`, decaying with
    /// `half_life_months` (0 is not allowed). `risk_off` (CN) adds the risk-off jump of stage 4. Ignored when the block is off.
    BlocDemand {
        bloc: String,
        pct: f64,
        half_life_months: f64,
        #[serde(default)]
        risk_off: bool,
    },
    /// b8-world: `BlocDemand` applied to all twelve blocs (never to the ABM). Ignored when the block is off.
    GlobalDemand { pct: f64, half_life_months: f64 },
    /// b8-world (stage 2, spec 2.5, D34): bilateral duty component on the origin `partner` (None: all origins) and `sector`
    /// (None: all sectors), in percentage points, added on top of the set-level `ImportTariff` level (replaced, not added).
    /// `retaliate` marks the duty as a retaliation trigger: with `world_retaliation` on (stage 4-3, spec 2.5) the partner's duty on US
    /// exports follows the ratchet rule of the retaliation state and the export factor R multiplies the bilateral exports; with it off
    /// the flag is stored and unused. Ignored when the trade link is off.
    Tariff {
        partner: Option<String>,
        sector: Option<usize>,
        pct: f64,
        #[serde(default)]
        retaliate: bool,
    },
    /// b8-world stage 3 (spec 2.4): oil supply disruption of `pct_of_world_supply` % of world consumption (a loss > 0; an SPR
    /// release < 0). Path: ramp to the full loss over `ramp_months`, hold it for `hold_months`, then restore linearly over
    /// `recover_months`. Ignored when the oil link is off (the block still reports).
    OilSupply { pct_of_world_supply: f64, ramp_months: u32, hold_months: u32, recover_months: u32 },
    /// b8-world stage 3 (spec 2.4): OPEC+ cut of `mbd` million barrels a day (a cut > 0), phased in over `months` months as
    /// policy surprises of -(mbd/103.1) x 100 / months % of world consumption each. Ignored when the oil link is off.
    OpecCut { mbd: f64, months: u32 },
    /// b8-world stage 4-1 (spec 2.7, D30): a risk-off jump of `vix_points` VIX points in the financial factor F (an injection into
    /// `f_t` of `vix_points / world_vix_per_unit` in the month it fires). The injection is a dollar-link function: it needs
    /// `world_link_dollar` and is ignored when that is off (`world_link_finance` does not inject; stage 4-2, S4-R16 item 11).
    RiskOff { vix_points: f64 },
    /// b8-world stage 4-1 (spec 2.6, D31): a US-originated credibility shock. Its dollar term is `ln(1 - dollar_pct/100)` times the
    /// profile p(age): 1 for `months`, then a three-month half-life; it needs `world_link_dollar` and is ignored when that is off.
    /// Its term-premium part `tp_bp` (stage 4-2, D31 and D46) needs `world_link_finance` and is ignored when that is off: the 10y
    /// carries `tp_bp` at ages 0 to 2 and the other tenors load it by `TP_SLOPE[j] / TP_SLOPE[3]` (the 2y gets 0.2 of it, the 3m none).
    UsCredibility { tp_bp: f64, months: u32, dollar_pct: f64 },
    /// b8-world stage 4-2 (spec 1.6, 2.7): an extra official Treasury flow of `bn` real-world $bn in total (purchases > 0), spread evenly
    /// over `months` months from the shock month, by `bloc` (None: pro rata to the blocs' official holdings). Ignored when
    /// `world_link_finance` is off.
    OfficialTreasury { bloc: Option<String>, bn: f64, months: u32 },
}

/// Bloc names accepted by the world shocks, in solution order (spec 1.9).
pub const BLOC_NAMES: [&str; 12] = ["EA", "CN", "JP", "UK", "CA", "MX", "IN", "KR", "BR", "AU", "OPEC+", "RoW"];

impl Shock {
    /// Check parameter ranges (rates are decimals, tenors 0–4, sectors 0–8, magnitudes bounded).
    pub fn validate(&self) -> Result<(), String> {
        let ok = |cond: bool, msg: &str| if cond { Ok(()) } else { Err(msg.to_string()) };
        let fin = |x: f64| x.is_finite();
        match self {
            Shock::OilPrice { pct, half_life_months } => ok(
                fin(*pct) && *pct > -95.0 && *pct <= 1000.0 && fin(*half_life_months) && *half_life_months >= 0.0,
                "OilPrice: pct must be in (-95, 1000] and half_life_months >= 0",
            ),
            Shock::PolicyRate { rate, months } => ok(
                fin(*rate) && (-0.05..=0.25).contains(rate) && *months >= 1,
                "PolicyRate: rate is a decimal in [-0.05, 0.25] (0.05 = 5%) and months >= 1",
            ),
            Shock::PolicyShift { bps, months } => ok(
                fin(*bps) && bps.abs() <= 2_000.0 && *months >= 1,
                "PolicyShift: |bps| <= 2000 and months >= 1",
            ),
            Shock::TaylorRule { phi_pi, phi_u, r_star } => ok(
                fin(*phi_pi) && fin(*phi_u) && fin(*r_star) && (0.0..=5.0).contains(phi_pi) && (0.0..=5.0).contains(phi_u)
                    && (-0.05..=0.10).contains(r_star),
                "TaylorRule: phi_pi, phi_u in [0, 5]; r_star decimal in [-0.05, 0.10]",
            ),
            Shock::TermPremium { tenor_idx, bps, half_life_months } => ok(
                *tenor_idx < N_TENORS && fin(*bps) && bps.abs() <= 2_000.0 && *half_life_months >= 0.0,
                "TermPremium: tenor_idx 0-4 (3m,2y,5y,10y,30y), |bps| <= 2000, half_life_months >= 0",
            ),
            Shock::QE { tenor_idx, monthly_face_pct_gdp, months } => ok(
                *tenor_idx < N_TENORS && fin(*monthly_face_pct_gdp) && monthly_face_pct_gdp.abs() <= 10.0 && *months >= 1,
                "QE: tenor_idx 0-4, |monthly_face_pct_gdp| <= 10, months >= 1",
            ),
            Shock::Helicopter { per_household, months } => ok(
                fin(*per_household) && (0.0..=100_000.0).contains(per_household) && *months >= 1,
                "Helicopter: per_household in [0, 100000] and months >= 1",
            ),
            Shock::Rebate { per_household, months } => ok(
                fin(*per_household) && (0.0..=100_000.0).contains(per_household) && *months >= 1,
                "Rebate: per_household in [0, 100000] and months >= 1",
            ),
            Shock::GovSpending { pct, months } => ok(
                fin(*pct) && *pct > -100.0 && *pct <= 500.0 && *months >= 1,
                "GovSpending: pct in (-100, 500] and months >= 1",
            ),
            Shock::IncomeTax { pp, months } => ok(
                fin(*pp) && pp.abs() <= 50.0 && *months >= 1,
                "IncomeTax: |pp| <= 50 and months >= 1",
            ),
            Shock::MacroPrudential { max_ltv, max_dti } => ok(
                fin(*max_ltv) && fin(*max_dti) && (0.1..=1.0).contains(max_ltv) && (0.05..=1.0).contains(max_dti),
                "MacroPrudential: max_ltv in [0.1, 1], max_dti in [0.05, 1] (decimals)",
            ),
            Shock::CreditSpread { bps, months } => ok(
                fin(*bps) && bps.abs() <= 2_000.0 && *months >= 1,
                "CreditSpread: |bps| <= 2000 and months >= 1",
            ),
            Shock::Productivity { sector, pct } => ok(
                sector.map_or(true, |s| s < N_SECTORS) && fin(*pct) && *pct > -90.0 && *pct <= 100.0,
                "Productivity: sector null or 0-8 (Sector::ALL order), pct in (-90, 100]",
            ),
            Shock::ImportTariff { pct } => ok(fin(*pct) && (0.0..=500.0).contains(pct), "ImportTariff: pct in [0, 500]"),
            Shock::Dollar { pct, half_life_months } => ok(
                fin(*pct) && *pct > -90.0 && *pct <= 300.0 && fin(*half_life_months) && *half_life_months >= 0.0,
                "Dollar: pct in (-90, 300] and half_life_months >= 0",
            ),
            Shock::SharePrices { sector, pct, half_life_months } => {
                // One message per field, with the value received, so a refused request says what to change.
                if let Some(s) = sector.filter(|&s| s >= N_SECTORS) {
                    return Err(format!(
                        "SharePrices: sector must be null (all sectors) or 0 to 8 (0 Energy, 1 Food, 2 DurableGoods, 3 NondurableGoods, 4 Tech, \
                         5 LeisureHospitality, 6 HealthEducation, 7 OtherServices, 8 Construction), got {s}"
                    ));
                }
                if !(fin(*pct) && *pct > -95.0 && *pct <= 300.0) {
                    return Err(format!("SharePrices: pct (the change in share prices, in percent) must be above -95 and at most 300, got {pct}"));
                }
                if !(fin(*half_life_months) && *half_life_months >= 0.0) {
                    return Err(format!("SharePrices: half_life_months must be 0 (permanent) or more, got {half_life_months}"));
                }
                Ok(())
            }
            Shock::MbsQE { monthly_face_pct_gdp, months } => ok(
                fin(*monthly_face_pct_gdp) && monthly_face_pct_gdp.abs() <= 10.0 && *months >= 1,
                "MbsQE: |monthly_face_pct_gdp| <= 10, months >= 1",
            ),
            Shock::TechBoom { months } => ok(*months >= 1 && *months <= 600, "TechBoom: months in [1, 600]"),
            Shock::BlocPolicy { bloc, bps, months } => ok(
                BLOC_NAMES.contains(&bloc.as_str()) && fin(*bps) && bps.abs() <= 2_000.0 && [3, 6, 12, 18, 24].contains(months),
                "BlocPolicy: bloc in EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW; |bps| <= 2000; months in {3, 6, 12, 18, 24}",
            ),
            Shock::BlocDemand { bloc, pct, half_life_months, .. } => ok(
                BLOC_NAMES.contains(&bloc.as_str()) && fin(*pct) && pct.abs() <= 20.0 && fin(*half_life_months)
                    && (1.0..=120.0).contains(half_life_months),
                "BlocDemand: bloc valid; |pct| <= 20; half_life_months in [1, 120]",
            ),
            Shock::GlobalDemand { pct, half_life_months } => ok(
                fin(*pct) && pct.abs() <= 20.0 && fin(*half_life_months) && (1.0..=120.0).contains(half_life_months),
                "GlobalDemand: |pct| <= 20; half_life_months in [1, 120]",
            ),
            Shock::Tariff { partner, sector, pct, .. } => ok(
                partner.as_deref().map_or(true, |p| BLOC_NAMES.contains(&p))
                    && sector.map_or(true, |s| s < N_SECTORS)
                    && fin(*pct)
                    && (0.0..=500.0).contains(pct),
                "Tariff: partner null or a bloc name, sector null or 0-8, pct in [0, 500] (percentage points)",
            ),
            Shock::OilSupply { pct_of_world_supply, ramp_months, hold_months, recover_months } => ok(
                fin(*pct_of_world_supply)
                    && (-30.0..=50.0).contains(pct_of_world_supply)
                    && *ramp_months >= 1
                    && *hold_months >= 1
                    && *recover_months >= 1,
                "OilSupply: pct_of_world_supply in [-30, 50]; ramp_months, hold_months, recover_months >= 1",
            ),
            Shock::OpecCut { mbd, months } => ok(fin(*mbd) && mbd.abs() <= 10.0 && *months >= 1, "OpecCut: |mbd| <= 10; months >= 1"),
            Shock::RiskOff { vix_points } => ok(fin(*vix_points) && (0.0..=100.0).contains(vix_points), "RiskOff: vix_points in [0, 100]"),
            Shock::UsCredibility { tp_bp, months, dollar_pct } => ok(
                fin(*tp_bp) && (0.0..=200.0).contains(tp_bp) && *months >= 1 && fin(*dollar_pct) && (0.0..50.0).contains(dollar_pct),
                "UsCredibility: tp_bp in [0, 200], months >= 1, dollar_pct in [0, 50)",
            ),
            Shock::OfficialTreasury { bloc, bn, months } => ok(
                bloc.as_deref().map_or(true, |b| BLOC_NAMES.contains(&b)) && fin(*bn) && bn.abs() <= 2_000.0 && (1..=60).contains(months),
                "OfficialTreasury: bloc null or a bloc name, |bn| <= 2000 (real-world $bn in total), months in [1, 60]",
            ),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ScheduledShock {
    pub at_month: u32,
    pub shock: Shock,
}

/// Parses a JSON array of `ScheduledShock` (the `econsim-cli world-dump` shock file) and validates every entry before any
/// use. The first error is returned as text: a bad bloc name must not reach the world block, where `bloc_index` panics.
pub fn parse_schedule(text: &str) -> Result<Vec<ScheduledShock>, String> {
    let list: Vec<ScheduledShock> = serde_json::from_str(text).map_err(|e| format!("not a ScheduledShock array: {e}"))?;
    for (i, s) in list.iter().enumerate() {
        s.shock.validate().map_err(|e| format!("entry {i}: {e}"))?;
    }
    Ok(list)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ShockState {
    pub schedule: Vec<ScheduledShock>,
    /// Shocks already activated (for display).
    pub applied: Vec<ScheduledShock>,

    pub oil_log_shock: f64,
    pub oil_decay: f64,
    pub policy_override: Option<(f64, u32)>,
    /// Active monetary-policy innovation: (bps as decimal, start tick, persistence rho).
    #[serde(default)]
    pub policy_shift: Option<(f64, u32, f64)>,
    pub taylor: Option<(f64, f64, f64)>,
    pub term_premium: [f64; N_TENORS],
    pub tp_decay: [f64; N_TENORS],
    /// (tenor_idx, monthly face as share of annual GDP, until_tick)
    pub qe: Vec<(usize, f64, u32)>,
    /// (monthly face as share of annual GDP, until_tick) for agency-MBS purchases (fix-monetary).
    #[serde(default)]
    pub mbs: Vec<(f64, u32)>,
    pub helicopter: Option<(f64, u32)>,
    /// RZ23: active Treasury-financed one-off payment: (per household $, until tick). None unless a `Rebate` shock has fired.
    #[serde(default)]
    pub rebate: Option<(f64, u32)>,
    pub gov_spending: Option<(f64, u32)>,
    pub income_tax_pp: Option<(f64, u32)>,
    pub macroprudential: Option<(f64, f64)>,
    pub credit_spread: Option<(f64, u32)>,
    /// Pending productivity jumps not yet applied by exog (multiplier per sector).
    pub productivity_pending: Vec<(Option<usize>, f64)>,
    pub tariff: f64,
    /// Log shift of the dollar exchange-rate target (sum of active shocks, decaying).
    #[serde(default)]
    pub dollar_log_shock: f64,
    #[serde(default)]
    pub dollar_decay: f64,
    /// Forced tech boom active while tick < this value (0 = none).
    #[serde(default)]
    pub tech_boom_until: u32,
    /// R51-E: active share-price re-ratings, one entry per `SharePrices` shock: (sector or None for all, log jump now, monthly decay
    /// factor; 1 = permanent). `activate_due` decays each entry once a month and drops the ones that have run out; empty unless a
    /// `SharePrices` shock has fired, so a run without one never reads it.
    #[serde(default)]
    pub share_price_log: Vec<(Option<usize>, f64, f64)>,
    /// b8-world: bloc shocks that fired, with their firing month (the block takes each one once, in `world::step`).
    #[serde(default)]
    pub world_shocks: Vec<(Shock, u32)>,
}

/// A decaying share-price re-rating is dropped once its log size is below this (about 1e-9 of a percent).
const SHARE_LOG_EPS: f64 = 1e-11;

fn decay_for(half_life: f64) -> f64 {
    if half_life <= 0.0 {
        1.0
    } else {
        0.5f64.powf(1.0 / half_life)
    }
}

impl ShockState {
    pub fn schedule(&mut self, s: ScheduledShock) {
        self.schedule.push(s);
    }

    /// Activate shocks scheduled at `tick` and decay the persistent ones. Called once per month by exog.
    pub fn activate_due(&mut self, tick: u32) {
        // decay
        self.oil_log_shock *= if self.oil_decay == 0.0 { 1.0 } else { self.oil_decay };
        self.dollar_log_shock *= if self.dollar_decay == 0.0 { 1.0 } else { self.dollar_decay };
        for i in 0..N_TENORS {
            self.term_premium[i] *= if self.tp_decay[i] == 0.0 { 1.0 } else { self.tp_decay[i] };
        }
        // R51-E: each share-price re-rating decays by its own factor (an exhausted one is dropped).
        if !self.share_price_log.is_empty() {
            for c in self.share_price_log.iter_mut() {
                c.1 *= c.2;
            }
            self.share_price_log.retain(|c| c.2 >= 1.0 || c.1.abs() > SHARE_LOG_EPS);
        }
        let due: Vec<ScheduledShock> = self
            .schedule
            .iter()
            .filter(|s| s.at_month == tick)
            .cloned()
            .collect();
        self.schedule.retain(|s| s.at_month != tick);
        for s in due {
            match &s.shock {
                Shock::OilPrice { pct, half_life_months } => {
                    self.oil_log_shock += (1.0 + pct / 100.0).max(0.01).ln();
                    self.oil_decay = decay_for(*half_life_months);
                }
                Shock::PolicyRate { rate, months } => {
                    self.policy_override = Some((*rate, tick + months));
                }
                Shock::PolicyShift { bps, months } => {
                    let rho = 1.0 - 1.0 / (*months).max(1) as f64;
                    self.policy_shift = Some((bps / 10_000.0, tick, rho));
                }
                Shock::TaylorRule { phi_pi, phi_u, r_star } => {
                    self.taylor = Some((*phi_pi, *phi_u, *r_star));
                }
                Shock::TermPremium { tenor_idx, bps, half_life_months } => {
                    let i = (*tenor_idx).min(N_TENORS - 1);
                    let d = decay_for(*half_life_months);
                    self.term_premium[i] += bps / 10_000.0;
                    self.tp_decay[i] = d;
                    if i > 0 {
                        self.term_premium[i - 1] += 0.5 * bps / 10_000.0;
                        self.tp_decay[i - 1] = d;
                    }
                    if i + 1 < N_TENORS {
                        self.term_premium[i + 1] += 0.5 * bps / 10_000.0;
                        self.tp_decay[i + 1] = d;
                    }
                }
                Shock::QE { tenor_idx, monthly_face_pct_gdp, months } => {
                    self.qe.push(((*tenor_idx).min(N_TENORS - 1), monthly_face_pct_gdp / 100.0, tick + months));
                }
                Shock::MbsQE { monthly_face_pct_gdp, months } => {
                    self.mbs.push((monthly_face_pct_gdp / 100.0, tick + months));
                }
                Shock::Helicopter { per_household, months } => {
                    self.helicopter = Some((*per_household, tick + months));
                }
                Shock::Rebate { per_household, months } => {
                    self.rebate = Some((*per_household, tick + months));
                }
                Shock::GovSpending { pct, months } => {
                    self.gov_spending = Some((1.0 + pct / 100.0, tick + months));
                }
                Shock::IncomeTax { pp, months } => {
                    self.income_tax_pp = Some((pp / 100.0, tick + months));
                }
                Shock::MacroPrudential { max_ltv, max_dti } => {
                    self.macroprudential = Some((*max_ltv, *max_dti));
                }
                Shock::CreditSpread { bps, months } => {
                    self.credit_spread = Some((bps / 10_000.0, tick + months));
                }
                Shock::Productivity { sector, pct } => {
                    self.productivity_pending.push((*sector, 1.0 + pct / 100.0));
                }
                Shock::ImportTariff { pct } => {
                    self.tariff = pct / 100.0;
                }
                Shock::Dollar { pct, half_life_months } => {
                    self.dollar_log_shock += (1.0 + pct / 100.0).max(0.01).ln();
                    self.dollar_decay = decay_for(*half_life_months);
                }
                Shock::TechBoom { months } => {
                    self.tech_boom_until = self.tech_boom_until.max(tick + months);
                }
                Shock::SharePrices { sector, pct, half_life_months } => {
                    let jump = (1.0 + pct / 100.0).max(0.01).ln();
                    self.share_price_log.push((*sector, jump, decay_for(*half_life_months)));
                }
                Shock::BlocPolicy { .. } | Shock::BlocDemand { .. } | Shock::GlobalDemand { .. } | Shock::Tariff { .. }
                | Shock::OilSupply { .. } | Shock::OpecCut { .. } | Shock::RiskOff { .. } | Shock::UsCredibility { .. } | Shock::OfficialTreasury { .. } => {
                    self.world_shocks.push((s.shock.clone(), tick));
                }
            }
            self.applied.push(s);
        }
    }

    // ---- accessors used by modules ----
    /// True while a forced TechBoom shock is in force.
    pub fn tech_boom_forced(&self, tick: u32) -> bool {
        tick < self.tech_boom_until
    }

    pub fn policy_rate_override(&self, tick: u32) -> Option<f64> {
        self.policy_override.and_then(|(r, until)| (tick < until).then_some(r))
    }
    /// Size of the policy innovation's rate gap in force at `tick` (decimal): bps × rho^k for k months since the shock.
    pub fn policy_gap(&self, tick: u32) -> f64 {
        match self.policy_shift {
            Some((b, t0, rho)) if tick >= t0 => b * rho.powi((tick - t0) as i32),
            _ => 0.0,
        }
    }
    /// Innovation added to the rule's shadow rate at `tick` (decimal). With smoothing `rate_smoothing`, the shadow gap
    /// g_k = gap_k − rate_smoothing × gap_{k−1} gives exactly gap_k = bps × rho^k, so the rate moves by the full bps
    /// on impact and then follows the AR(1) path while the inertia smooths the rule's own response.
    pub fn policy_innovation(&self, tick: u32, rate_smoothing: f64) -> f64 {
        match self.policy_shift {
            Some((_, t0, _)) if tick == t0 => self.policy_gap(tick),
            Some((_, t0, _)) if tick > t0 => self.policy_gap(tick) - rate_smoothing * self.policy_gap(tick - 1),
            _ => 0.0,
        }
    }
    /// Average of the known policy-gap path over the next `tenor` months, starting at `tick` (decimal). The yield curve
    /// adds this to its expected short-rate path, because the gap's decay is announced with the shock.
    pub fn policy_gap_average(&self, tick: u32, tenor: u32) -> f64 {
        let t = tenor.max(1) as f64;
        match self.policy_shift {
            Some((b, t0, rho)) if tick >= t0 => {
                let k0 = (tick - t0) as f64;
                let sum = if (1.0 - rho).abs() < 1e-12 { t } else { (1.0 - rho.powf(t)) / (1.0 - rho) };
                b * rho.powf(k0) * sum / t
            }
            _ => 0.0,
        }
    }
    /// Face-years (as a share of annual GDP) of QE purchases announced but not yet executed at `tick`: the sum over
    /// active programmes of monthly share × remaining months × tenor in years. Subtracted from the private duration
    /// when the term premium is priced (monetary::duration_ratio).
    pub fn qe_announced_duration(&self, tick: u32) -> f64 {
        self.qe
            .iter()
            .filter(|(_, _, until)| tick < *until)
            .map(|(i, s, until)| s * (*until - tick) as f64 * TENORS[(*i).min(N_TENORS - 1)] as f64 / 12.0)
            .sum()
    }
    /// Agency-MBS purchases this month, as a share of annual GDP (sum over active MbsQE shocks).
    pub fn mbs_share(&self, tick: u32) -> f64 {
        self.mbs.iter().filter(|(_, until)| tick < *until).map(|(s, _)| s).sum()
    }
    /// Additional term premium (decimal) at each tenor.
    pub fn term_premium_shock(&self) -> [f64; N_TENORS] {
        self.term_premium
    }
    /// QE purchases this month: Vec of (tenor_idx, face as share of annual GDP).
    pub fn qe_active(&self, tick: u32) -> Vec<(usize, f64)> {
        self.qe
            .iter()
            .filter(|(_, _, until)| tick < *until)
            .map(|(i, s, _)| (*i, *s))
            .collect()
    }
    pub fn helicopter_per_household(&self, tick: u32) -> f64 {
        self.helicopter
            .map(|(a, until)| if tick < until { a } else { 0.0 })
            .unwrap_or(0.0)
    }
    /// RZ23: the Treasury-financed payment per household this month (0 unless a `Rebate` shock is in force).
    pub fn rebate_per_household(&self, tick: u32) -> f64 {
        self.rebate.map(|(a, until)| if tick < until { a } else { 0.0 }).unwrap_or(0.0)
    }
    pub fn gov_spending_mult(&self, tick: u32) -> f64 {
        self.gov_spending
            .map(|(m, until)| if tick < until { m } else { 1.0 })
            .unwrap_or(1.0)
    }
    pub fn income_tax_add(&self, tick: u32) -> f64 {
        self.income_tax_pp
            .map(|(p, until)| if tick < until { p } else { 0.0 })
            .unwrap_or(0.0)
    }
    pub fn credit_spread_add(&self, tick: u32) -> f64 {
        self.credit_spread
            .map(|(p, until)| if tick < until { p } else { 0.0 })
            .unwrap_or(0.0)
    }
    /// rz1 F2: the credit spread with a decaying tail. Inside the shock's window (`tick < until`) the full spread; from `until`
    /// on, `p * 0.5^((tick - until + 1) / half_life)`, i.e. the first month after the window is already one step down
    /// (AR(1) decay with half-life `half_life_months`). `half_life_months <= 0` (or not finite) is `credit_spread_add`, the legacy rectangle.
    pub fn credit_spread_add_tail(&self, tick: u32, half_life_months: f64) -> f64 {
        if !(half_life_months > 0.0) || !half_life_months.is_finite() {
            return self.credit_spread_add(tick);
        }
        self.credit_spread
            .map(|(p, until)| {
                if tick < until {
                    p
                } else {
                    p * 0.5f64.powf((tick - until + 1) as f64 / half_life_months)
                }
            })
            .unwrap_or(0.0)
    }
    /// R51-E: the multiplicative wedge on each sector's equity valuation from the active `SharePrices` shocks, exp of the sum of their
    /// current log sizes (Sector::ALL order). `None` when no re-rating is active, which is every run without one: the valuation then
    /// takes its unchanged path.
    pub fn share_price_wedge(&self) -> Option<[f64; N_SECTORS]> {
        if self.share_price_log.is_empty() {
            return None;
        }
        let mut log = [0.0; N_SECTORS];
        for &(sector, l, _) in &self.share_price_log {
            match sector {
                Some(s) if s < N_SECTORS => log[s] += l,
                Some(_) => {}
                None => log.iter_mut().for_each(|x| *x += l),
            }
        }
        Some(log.map(f64::exp))
    }
    /// rz24-r1: the sectors that an active `SharePrices` re-rating covers, in Sector::ALL order: true for a sector with at least one active
    /// re-rating entry aimed at all sectors or at that sector, whatever its size or sign. All false when none is active (every run without
    /// one). A typed re-rating sets the share price of the sectors it covers, so the model's own estimate of what a credit shock does to
    /// equities (`credit_shock_erp`) is not added to them (`equity::revalue_fund_equity`).
    pub fn share_price_covered(&self) -> [bool; N_SECTORS] {
        let mut cov = [false; N_SECTORS];
        for &(sector, _, _) in &self.share_price_log {
            match sector {
                Some(s) if s < N_SECTORS => cov[s] = true,
                Some(_) => {}
                None => cov = [true; N_SECTORS],
            }
        }
        cov
    }
    /// Log shift of the dollar target from active `Dollar` shocks.
    pub fn dollar_log(&self) -> f64 {
        self.dollar_log_shock
    }
    /// Effective (max_ltv, max_dti) given config defaults.
    pub fn mortgage_limits(&self, default_ltv: f64, default_dti: f64) -> (f64, f64) {
        self.macroprudential.unwrap_or((default_ltv, default_dti))
    }
}

/// Sector-indexed helper for tariff application.
pub fn tariff_multiplier(tariff: f64) -> [f64; N_SECTORS] {
    [1.0 + tariff; N_SECTORS]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_innovation_gives_ar1_gap_with_full_impact_and_no_expiry() {
        let mut st = ShockState::default();
        st.activate_due(0);
        st.schedule(ScheduledShock { at_month: 5, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
        let rs = 0.9;
        // Simulate the shadow-rate gap through the rule's smoothing with the innovation, as monetary::set_policy does.
        let mut gap = 0.0;
        for t in 0..80u32 {
            st.activate_due(t);
            gap = rs * gap + st.policy_innovation(t, rs);
            if t >= 5 {
                let k = (t - 5) as i32;
                let want = 0.01 * (1.0 - 1.0 / 12.0f64).powi(k);
                assert!((gap - want).abs() < 1e-12, "t {t}: gap {gap} want {want}");
                assert!((st.policy_gap(t) - want).abs() < 1e-12);
            } else {
                assert_eq!(gap, 0.0);
            }
        }
        // The expected-average over a tenor equals the mean of the known path.
        let t0 = 5u32;
        let avg: f64 = (0..24).map(|k| st.policy_gap(t0 + k)).sum::<f64>() / 24.0;
        assert!((st.policy_gap_average(t0, 24) - avg).abs() < 1e-12);
    }

    #[test]
    fn qe_announcement_prices_remaining_purchases_and_expires_with_the_programme() {
        let mut st = ShockState::default();
        st.schedule(ScheduledShock { at_month: 0, shock: Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 } });
        st.activate_due(0);
        // 0.5% of GDP per month, 12 months left, 10y: 0.005 × 12 × 10 = 0.6 (share-years).
        assert!((st.qe_announced_duration(0) - 0.6).abs() < 1e-12);
        assert!((st.qe_announced_duration(6) - 0.3).abs() < 1e-12);
        assert_eq!(st.qe_announced_duration(12), 0.0);
    }

    // ---- RZ23: Rebate ----

    #[test]
    fn rebate_validation_and_json() {
        let ok = |a: f64, m: u32| Shock::Rebate { per_household: a, months: m }.validate();
        assert!(ok(0.0, 1).is_ok() && ok(2_000.0, 12).is_ok() && ok(100_000.0, 600).is_ok());
        for bad in [ok(-0.01, 1), ok(100_000.01, 1), ok(f64::NAN, 1), ok(f64::INFINITY, 1), ok(2_000.0, 0)] {
            assert!(bad.unwrap_err().starts_with("Rebate: per_household in [0, 100000] and months >= 1"));
        }
        let s: Shock = serde_json::from_str(r#"{"type":"Rebate","per_household":2000,"months":1}"#).unwrap();
        assert_eq!(s, Shock::Rebate { per_household: 2000.0, months: 1 });
        assert!(serde_json::to_string(&s).unwrap().contains(r#""type":"Rebate""#));
        let file = r#"[{"at_month":3,"shock":{"type":"Rebate","per_household":600,"months":1}}]"#;
        assert_eq!(parse_schedule(file).unwrap()[0].shock, Shock::Rebate { per_household: 600.0, months: 1 });
        assert!(parse_schedule(&file.replace("600", "-1")).unwrap_err().contains("entry 0: Rebate"));
    }

    #[test]
    fn rebate_state_follows_its_schedule_and_is_none_without_one() {
        let mut st = ShockState::default();
        assert_eq!(st.rebate_per_household(0), 0.0);
        st.schedule(ScheduledShock { at_month: 2, shock: Shock::Rebate { per_household: 2_000.0, months: 3 } });
        for t in 0..8u32 {
            st.activate_due(t);
            let want = if (2..5).contains(&t) { 2_000.0 } else { 0.0 };
            assert_eq!(st.rebate_per_household(t), want, "t {t}");
        }
        assert_eq!(st.helicopter_per_household(3), 0.0, "the helicopter input is a different shock");
    }

    fn credit_state(bps: f64, months: u32) -> ShockState {
        let mut st = ShockState::default();
        st.schedule(ScheduledShock { at_month: 0, shock: Shock::CreditSpread { bps, months } });
        st.activate_due(0);
        st
    }

    #[test]
    fn credit_tail_off_is_the_rectangle_bit_for_bit() {
        let st = credit_state(200.0, 12);
        for half_life in [0.0, -3.0, f64::NAN] {
            for t in 0..60u32 {
                assert_eq!(st.credit_spread_add_tail(t, half_life).to_bits(), st.credit_spread_add(t).to_bits(), "t {t} hl {half_life}");
            }
        }
        assert_eq!(st.credit_spread_add(11), 0.02);
        assert_eq!(st.credit_spread_add(12), 0.0);
    }

    #[test]
    fn credit_tail_holds_then_decays_with_the_half_life() {
        let st = credit_state(200.0, 12);
        let hl = 9.0;
        // Inside the window the full spread, exactly.
        for t in 0..12u32 {
            assert_eq!(st.credit_spread_add_tail(t, hl), 0.02);
        }
        // From month 12 on: p x 0.5^((t - 12 + 1) / hl). One half-life after the first decayed month the value is half of it... of p.
        assert!((st.credit_spread_add_tail(12, hl) - 0.02 * 0.5f64.powf(1.0 / hl)).abs() < 1e-15);
        assert!((st.credit_spread_add_tail(12 + 8, hl) - 0.01).abs() < 1e-15, "(20 - 12 + 1) / 9 = 1 half-life");
        assert!((st.credit_spread_add_tail(12 + 17, hl) - 0.005).abs() < 1e-15, "two half-lives");
        let mut prev = f64::INFINITY;
        for t in 12..80u32 {
            let v = st.credit_spread_add_tail(t, hl);
            assert!(v > 0.0 && v < prev && v < 0.02, "t {t}");
            prev = v;
        }
        // Geometric: the ratio of successive months is the constant decay factor.
        let r = st.credit_spread_add_tail(14, hl) / st.credit_spread_add_tail(13, hl);
        assert!((r - 0.5f64.powf(1.0 / hl)).abs() < 1e-12);
    }

    #[test]
    fn credit_tail_keeps_the_sign_and_needs_a_shock() {
        let eased = credit_state(-100.0, 6);
        assert!(eased.credit_spread_add_tail(3, 9.0) < 0.0 && eased.credit_spread_add_tail(9, 9.0) < 0.0);
        assert!(eased.credit_spread_add_tail(9, 9.0) > eased.credit_spread_add_tail(3, 9.0), "decaying towards zero");
        let none = ShockState::default();
        assert_eq!(none.credit_spread_add_tail(5, 9.0), 0.0);
    }

    // ---- R51-E: SharePrices ----

    fn share_prices(sector: Option<usize>, pct: f64, half_life_months: f64) -> Shock {
        Shock::SharePrices { sector, pct, half_life_months }
    }

    #[test]
    fn share_prices_validation_at_and_past_each_bound() {
        let bad = |s: Shock| s.validate().expect_err("out of range was accepted");
        // pct in (-95, 300]: the lower bound is open, the upper closed.
        assert!(share_prices(None, -94.999, 0.0).validate().is_ok());
        assert!(share_prices(None, 300.0, 0.0).validate().is_ok());
        assert!(share_prices(None, 0.0, 0.0).validate().is_ok());
        for pct in [-95.0, -95.001, -100.0, 300.001, 1000.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let e = bad(share_prices(None, pct, 0.0));
            assert!(e.starts_with("SharePrices: pct") && e.contains("above -95 and at most 300") && e.ends_with(&format!("got {pct}")), "{pct}: {e}");
        }
        // half_life_months >= 0 (0 = permanent), finite.
        assert!(share_prices(None, -30.0, 0.0).validate().is_ok());
        assert!(share_prices(None, -30.0, 1e-9).validate().is_ok());
        assert!(share_prices(None, -30.0, 600.0).validate().is_ok());
        for hl in [-1e-9, -1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let e = bad(share_prices(None, -30.0, hl));
            assert!(e.starts_with("SharePrices: half_life_months must be 0 (permanent) or more"), "{hl}: {e}");
        }
        // sector null or 0-8.
        assert!(share_prices(None, -30.0, 0.0).validate().is_ok());
        for s in 0..N_SECTORS {
            assert!(share_prices(Some(s), -30.0, 0.0).validate().is_ok(), "sector {s}");
        }
        for s in [N_SECTORS, N_SECTORS + 1, 99, usize::MAX] {
            let e = bad(share_prices(Some(s), -30.0, 0.0));
            assert!(e.starts_with("SharePrices: sector must be null (all sectors) or 0 to 8") && e.contains("4 Tech") && e.ends_with(&format!("got {s}")), "{s}: {e}");
        }
        // parse_schedule applies the same bounds to a shock file.
        let ok = r#"[{"at_month":2,"shock":{"type":"SharePrices","sector":4,"pct":-80,"half_life_months":24}}]"#;
        assert_eq!(parse_schedule(ok).unwrap()[0].shock, share_prices(Some(4), -80.0, 24.0));
        let past = ok.replace("-80", "-95");
        assert!(parse_schedule(&past).unwrap_err().contains("entry 0: SharePrices"), "{past}");
    }

    #[test]
    fn share_prices_json_uses_the_plain_name_and_a_missing_sector_means_all() {
        let all: Shock = serde_json::from_str(r#"{"type":"SharePrices","pct":-30,"half_life_months":0}"#).unwrap();
        assert_eq!(all, share_prices(None, -30.0, 0.0));
        let one: Shock = serde_json::from_str(r#"{"type":"SharePrices","sector":4,"pct":-80,"half_life_months":24}"#).unwrap();
        assert_eq!(one, share_prices(Some(4), -80.0, 24.0));
        let text = serde_json::to_string(&one).unwrap();
        assert!(text.contains(r#""type":"SharePrices""#) && text.contains(r#""sector":4"#), "{text}");
        assert!(serde_json::from_str::<Shock>(r#"{"type":"SharePrices","sector":"tech","pct":-80,"half_life_months":0}"#).is_err());
    }

    fn fire(st: &mut ShockState, at: u32, s: Shock) {
        st.schedule(ScheduledShock { at_month: at, shock: s });
    }

    #[test]
    fn share_price_wedge_is_none_until_a_shock_fires_and_never_before_its_month() {
        let mut st = ShockState::default();
        fire(&mut st, 3, share_prices(Some(4), -80.0, 0.0));
        for t in 0..3u32 {
            st.activate_due(t);
            assert_eq!(st.share_price_wedge(), None, "t {t}");
        }
        st.activate_due(3);
        assert!(st.share_price_wedge().is_some());
        assert!(ShockState::default().share_price_wedge().is_none());
    }

    #[test]
    fn share_price_covered_marks_the_sectors_an_active_re_rating_reaches_whatever_its_size() {
        let mut st = ShockState::default();
        assert_eq!(st.share_price_covered(), [false; N_SECTORS]);
        fire(&mut st, 1, share_prices(Some(4), -80.0, 0.0));
        st.activate_due(0);
        assert_eq!(st.share_price_covered(), [false; N_SECTORS], "not before its month");
        st.activate_due(1);
        let mut only_tech = [false; N_SECTORS];
        only_tech[4] = true;
        assert_eq!(st.share_price_covered(), only_tech);
        // a re-rating of 0% still covers: the reader typed where the price is
        fire(&mut st, 2, share_prices(Some(1), 0.0, 0.0));
        st.activate_due(2);
        only_tech[1] = true;
        assert_eq!(st.share_price_covered(), only_tech);
        fire(&mut st, 3, share_prices(None, -10.0, 0.0));
        st.activate_due(3);
        assert_eq!(st.share_price_covered(), [true; N_SECTORS]);
    }

    #[test]
    fn share_price_covered_ends_when_a_decaying_re_rating_has_run_out() {
        let mut st = ShockState::default();
        fire(&mut st, 0, share_prices(None, -10.0, 0.05));
        st.activate_due(0);
        assert_eq!(st.share_price_covered(), [true; N_SECTORS]);
        for t in 1..6u32 {
            st.activate_due(t);
        }
        assert_eq!(st.share_price_covered(), [false; N_SECTORS]);
        assert!(st.share_price_wedge().is_none());
    }

    #[test]
    fn share_price_jump_is_ln_of_one_plus_pct_on_impact_and_permanent_without_a_half_life() {
        let mut st = ShockState::default();
        fire(&mut st, 2, share_prices(Some(4), -80.0, 0.0));
        for t in 0..60u32 {
            st.activate_due(t);
            if t < 2 {
                continue;
            }
            let w = st.share_price_wedge().unwrap();
            // exp(ln(0.2)) is 0.2 to rounding, every month, with no decay.
            assert!((w[4] - 0.2).abs() < 1e-15, "t {t}: {}", w[4]);
            for s in (0..N_SECTORS).filter(|&s| s != 4) {
                assert_eq!(w[s], 1.0, "t {t} sector {s}: a one-sector shock leaves the others alone");
            }
        }
        // +300% is the upper bound: a factor of 4.
        let mut up = ShockState::default();
        fire(&mut up, 0, share_prices(None, 300.0, 0.0));
        up.activate_due(0);
        assert!(up.share_price_wedge().unwrap().iter().all(|&x| (x - 4.0).abs() < 1e-14));
    }

    #[test]
    fn share_price_gap_decays_with_its_half_life() {
        for hl in [1.0, 3.0, 12.0, 24.0] {
            let mut st = ShockState::default();
            fire(&mut st, 5, share_prices(None, -80.0, hl));
            let l = 0.2f64.ln();
            for t in 5..120u32 {
                st.activate_due(t);
                let k = (t - 5) as f64;
                // The log size is ln(0.2) x 0.5^(k / hl): half of it one half-life after impact.
                let want = l * 0.5f64.powf(k / hl);
                match st.share_price_wedge() {
                    Some(w) => assert!((w[4].ln() - want).abs() < 1e-12, "hl {hl} k {k}: {} vs {want}", w[4].ln()),
                    None => assert!(want.abs() < 1e-10, "hl {hl} k {k}: dropped while still {want}"),
                }
            }
            let mut half = ShockState::default();
            fire(&mut half, 0, share_prices(None, -80.0, hl));
            for t in 0..=(hl as u32) {
                half.activate_due(t);
            }
            let at_hl = half.share_price_wedge().unwrap()[0].ln();
            assert!((at_hl - l / 2.0).abs() < 1e-12, "hl {hl}: log at k = hl is {at_hl}, half of {l}");
        }
    }

    #[test]
    fn share_price_shocks_stack_each_on_its_own_decay_and_run_out() {
        let mut st = ShockState::default();
        fire(&mut st, 0, share_prices(Some(4), -50.0, 0.0));
        fire(&mut st, 0, share_prices(None, 100.0, 6.0));
        fire(&mut st, 12, share_prices(Some(4), -50.0, 12.0));
        for t in 0..=12u32 {
            st.activate_due(t);
        }
        let w = st.share_price_wedge().unwrap();
        let tech = 0.5f64.ln() + 2.0f64.ln() * 0.5f64.powf(12.0 / 6.0) + 0.5f64.ln();
        let other = 2.0f64.ln() * 0.5f64.powf(12.0 / 6.0);
        assert!((w[4].ln() - tech).abs() < 1e-12, "{} vs {tech}", w[4].ln());
        assert!((w[1].ln() - other).abs() < 1e-12);
        // A decaying shock is dropped once it is negligible; a permanent one stays.
        for t in 13..2000u32 {
            st.activate_due(t);
        }
        assert_eq!(st.share_price_log.len(), 1, "only the permanent one is left: {:?}", st.share_price_log);
        assert!((st.share_price_wedge().unwrap()[4] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn a_tiny_half_life_acts_on_impact_only() {
        let mut st = ShockState::default();
        fire(&mut st, 0, share_prices(None, -50.0, 1e-9));
        st.activate_due(0);
        assert!((st.share_price_wedge().unwrap()[0] - 0.5).abs() < 1e-15);
        st.activate_due(1);
        assert_eq!(st.share_price_wedge(), None, "gone after one month, no NaN");
    }
}
