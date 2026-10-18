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
    /// b8-world: bloc shocks that fired, with their firing month (the block takes each one once, in `world::step`).
    #[serde(default)]
    pub world_shocks: Vec<(Shock, u32)>,
}

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
}
