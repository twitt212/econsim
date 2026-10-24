//! Model parameters (docs/DESIGN.md §13). All rates annual decimals unless noted.

use serde::{Deserialize, Serialize};

use crate::types::{N_CONSUMER, N_SECTORS, N_TENORS};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // --- scale ---
    pub n_households: usize,
    pub households_per_firm: f64,
    pub n_banks: usize,
    /// US households represented by one agent (computed from n_households if 0).
    pub scale: f64,
    /// Months simulated (and discarded from history) inside `init::build_world` to settle.
    pub burn_in_months: u32,

    // --- exogenous ---
    pub oil_base: f64,
    pub stochastic_oil: bool,
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_ar: f64,
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_sigma: f64,
    // p0-exog: exogenous stochastic factors. Every new sigma, intensity and loading defaults to 0 (process off), so
    // the outputs are bit-identical to the pre-p0 model. See docs/DESIGN.md "Exogenous factors (p0-exog)".
    /// Financial risk factor f_t: AR(1) persistence per month (Bekaert, Engstrom & Xu 2022 use ≈0.98 monthly).
    /// Re-fit Step 10a (stop record, not adopted): the candidate is 0.95 with the jumps below. Default 0.98 = legacy.
    pub fin_factor_rho: f64,
    /// Financial risk factor: innovation sd per month (0 = off).
    pub fin_factor_sigma: f64,
    /// Volatility loading γ: the innovation sd is σ·exp(γ·f_{t−1}), clamped to [0.2, 5]. 0 = constant volatility.
    pub fin_factor_vol_gamma: f64,
    /// Poisson intensity of risk jumps per month added to f_t (0 = off). Re-fit Step 10a candidate 1/96; off by default.
    pub fin_jump_intensity: f64,
    /// Mean of a risk jump; positive = risk-off (f_t rises). Re-fit Step 10a candidate 1.0; off by default.
    pub fin_jump_mean: f64,
    /// Sd of a risk jump. Re-fit Step 10a candidate 0.25; off by default.
    pub fin_jump_sd: f64,
    /// Term-premium level factor: AR(1) persistence per month (Diebold & Li 2006; ≈0.98).
    pub tp_level_rho: f64,
    /// Term-premium level factor: innovation sd per month (0 = off).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub tp_level_sigma: f64,
    /// Loading of the level factor on the current f_t (flight to quality; negative = premium falls in risk-off).
    pub tp_level_fin_loading: f64,
    /// Term-premium curvature factor: AR(1) persistence per month (≈0.95).
    pub tp_curv_rho: f64,
    /// Term-premium curvature factor: innovation sd per month (0 = off).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub tp_curv_sigma: f64,
    /// Dollar risk premium ψ_t: AR(1) persistence per month (Itskhoki & Mukhin 2021; ≈0.99).
    pub fx_psi_rho: f64,
    /// Dollar risk premium: innovation sd per month (0 = off).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub fx_psi_sigma: f64,
    /// Correlation of the ψ innovation with the diffusion innovation ε_t of f_t (corr in [−1, 1]; 0 = independent). It is
    /// not a correlation with the level of f_t, and it does not see f's jumps (see `fx_psi_fin_jump_loading`).
    pub fx_psi_fin_corr: f64,
    /// Dollar risk premium: loading of ψ_t on this month's risk-jump sum J_t of f_t, so ψ_t = ρψ_{t−1} + σ z_t + λ·J_t
    /// (λ in ψ units per unit of f jump; 0 = ψ ignores f's jumps, the default). Lets a risk-off jump move the dollar.
    pub fx_psi_fin_jump_loading: f64,
    /// Oil: Student-t degrees of freedom ν for the innovations (0 = Gaussian, the pre-p0 process). Values in (0, 2.5) are
    /// raised to 2.5. The t draw is scaled to unit variance and then winsorised at ±8 sd (exog::OIL_SHOCK_CAP), which
    /// cuts 0.035% of draws at ν = 4 and 0.007% at ν = 6. The winsorised variance is 0.73, 0.89 and 0.98 at ν = 2.5, 3
    /// and 4, and about 1.00 by ν = 10; excess kurtosis is 5.6 at ν = 4 and 2.45 at ν = 6. The winsorising applies only
    /// when ν > 0. The oil log deviation is capped at ±1.5 (exog::OIL_LOG_DEV_CAP) whenever an extension is on.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_t_nu: f64,
    /// Oil GARCH(1,1): ARCH coefficient α on the last squared innovation (Bollerslev 1986). With α = β = 0 the
    /// innovation sd is the constant `oil_sigma` (the pre-p0 process). The long-run sd is `oil_sigma`.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_garch_alpha: f64,
    /// Oil GARCH(1,1): GARCH coefficient β on the last conditional variance (α + β < 1; oil_spec panics otherwise). The
    /// conditional sd is clamped to [0.2, 5] × `oil_sigma` (exog::OIL_VOL_MULT_RANGE), so an explosive cluster is bounded.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_garch_beta: f64,
    /// Oil: Poisson intensity of supply/geopolitical jumps per month in the log price (Hamilton 2009; 0 = off). Jumps
    /// draw from their own stream, so changing `oil_t_nu` does not redraw them.
    pub oil_jump_intensity: f64,
    /// Oil: sd of a jump in the log price (jumps are symmetric; they then mean-revert with the AR(1)).
    pub oil_jump_sd: f64,
    /// Index the oil price to the domestic price level at `inflation_target`: oil_price = oil_base × (1+π*)^((t − B)/12) ×
    /// e^(ar + shock), normalised to reported month 0 (= calendar month `burn_in_months`, so the index is 1 at month 0 and
    /// is continuous across the burn-in reset). `oil_base` stays the anchor that `oil_ratio` divides by, so the index
    /// reaches the nominal consumers of the oil ratio (firms' crude cost, the Energy oil leg, gasoline, world energy
    /// price). Extraction labour uses the real ratio (oil ratio / index, `common::real_oil_ratio`), so the index creates no
    /// drift in labour per unit of energy. Real oil (oil / CPI) then has no drift when CPI tracks the target (Alquist,
    /// Kilian & Vigfusson 2013). It is
    /// not indexed to `world_goods_inflation` (0.5%/yr, import prices), which would leave a real drift of about −1.5%/yr.
    /// false = pre-p0 behaviour.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub oil_index_to_world: bool,
    /// Aggregate risk-premium shock: monthly AR(1) coefficient and innovation sd (annual rate units). It raises the
    /// real rate households save at and firms discount investment at, moving output and inflation together — the
    /// dominant demand shock in estimated DSGE models (Smets & Wouters 2007). 0 sd disables it.
    pub risk_premium_ar: f64,
    pub risk_premium_sigma: f64,
    /// Weight of the risk-premium wedge in firms' cost of capital (households get the full wedge).
    pub risk_premium_firm_weight: f64,
    /// Re-fit Step 10a: loading of the demand premium on the centred financial factor, f_t − f̄ with
    /// f̄ = λμ/(1 − ρ): risk_premium_t = clamp(rp_ar_t + rp_fin_loading × (f_t − f̄), ±0.05), where rp_ar is the AR(1)
    /// state of the demand shock alone. The centring is exact while `fin_factor_vol_gamma` = 0. 0 = off (legacy).
    pub rp_fin_loading: f64,
    /// Monthly sd of a common random-walk shock to log productivity (permanent technology shocks; utilisation-adjusted
    /// TFP growth has a quarterly sd ≈0.6–0.8%, Fernald 2014). 0 disables it.
    pub tfp_shock_sigma: f64,
    pub tfp_growth_annual: f64,
    /// Sector offsets to annual productivity growth (one per sector, `Sector::ALL` order), added to
    /// `tfp_growth_annual`. US labour productivity has grown much faster in goods than in services and construction
    /// (Baumol 1967; BLS productivity by industry), so goods prices fall relative to services. Zeros = uniform growth.
    pub tfp_sector_offset: [f64; N_SECTORS],
    /// Legacy: no longer drives entry (native entrants follow births, plus `immigration_annual`) and no longer
    /// sizes the burn-in population (`init::build_world` builds `n_households` directly).
    pub pop_growth_annual: f64,
    /// Net immigrant households per household per year (US net international migration ≈1M people/yr, CBO),
    /// arriving aged 22–45 as finished-schooling job seekers.
    pub immigration_annual: f64,
    /// Log change in native household formation per pp of unemployment gap (u* − u); 2008–11: a ≈5pp gap cut
    /// formation ≈60% (JCHS), ≈0.18. 0 makes formation acyclical.
    pub household_formation_cyclicality: f64,
    /// An entrant placed in a reused household slot (`demography::add_entrants`) starts with none of the previous
    /// occupant's per-slot memory: last month's capital income and taxable distributions (otherwise taxed in the
    /// entrant's name and spent at the capital-income MPC), the windfall stock, UI-spell flag and impatience type (redrawn
    /// even when the entrant is not younger than the decedent, which the "age falls" test missed), the labour-market
    /// sector that targets job applications, the job's pay multiplier, and the tax arrears, year-to-date wages, Fund cost
    /// basis and loss carry-forward. Inheritance is unchanged: the estate still passes to the heirs at death. Legacy
    /// `false` (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub entrant_slot_reset: bool,

    // --- labour ---
    pub min_wage_monthly: f64,
    pub separation_rate: f64,
    pub applications: usize,
    /// μ in the matching function M = μ·U^½·V^½ (monthly job-finding ≈ μ at V/U = 1).
    pub matching_efficiency: f64,
    /// Fill rate assumed in the first month, before any hires and openings have been observed: JOLTS hires ÷
    /// openings is ≈0.5–0.8 a month. Firms post openings = hires needed ÷ expected fill rate (firms::plan).
    pub initial_fill_rate: f64,
    /// Lower bound on the expected fill rate used to scale openings, so a collapse in hiring cannot blow up postings.
    pub min_fill_rate: f64,
    pub on_the_job_search: f64,
    pub benefit_replacement: f64,
    pub benefit_cap: f64,
    /// Round 7 step 1: during the frozen burn-in and at its anchor, pensions, brackets and nominal dollar parameters
    /// keep their 2026 relation to the wage index (not CPI), so no benefit or threshold erodes against wages.
    pub burn_in_nominal_neutral: bool,
    /// Round 7 step 1: the UI weekly cap follows the wage index (SAWW rule) rather than the nominal scale.
    pub ui_cap_wage_indexed: bool,
    /// Round 7 step 1: entrants' reservation wages follow the wage index (current wage levels).
    pub entry_wage_indexed: bool,
    /// Round 7 step 2: means-tested dollar maxima and EITC thresholds follow the wage index (AWI), not the CPI index,
    /// so caseload shares stay roughly stable as in US law. The SSI asset limit is not indexed (see `nominal_scale`).
    pub wage_indexed_thresholds: bool,
    pub benefit_months: u32,
    pub edu_premium: [f64; 3],

    // --- firms ---
    pub inventory_target: [f64; N_SECTORS],
    pub reprice_prob: [f64; N_SECTORS],
    pub initial_markup: [f64; N_SECTORS],
    pub depreciation_annual: f64,
    pub payout_ratio: f64,
    pub investment_hurdle: f64,
    pub investment_sensitivity: f64,
    /// Fastest planned run-down of excess capital per month (share of capital) beyond depreciation: how far gross
    /// investment can fall below replacement when a firm has too much capital (US equipment and software investment fell
    /// ≈20–30% in 2001–02 and 2008–09).
    pub capital_rundown_max_monthly: f64,
    /// Re-fit Step 8(a): firms' user cost of capital uses the households' mean expected inflation (`mean_expected_
    /// inflation`) instead of the firm nowcast blend, so the investment term no longer carries the 3-month core
    /// nowcast noise (Coibion, Gorodnichenko & Kumar 2018; Sharpe & Suarez 2021). The nowcast blend stays in price
    /// resets and wage norms. Legacy false (scripts/profiles/legacy_p1off.json and legacy_p1all.json).
    pub user_cost_household_expectation: bool,
    /// Re-fit Step 8(b): partial adjustment of desired capital. Each month the firm's desired capital moves this share
    /// of the way from its last smoothed value to the raw k* (Eisner & Strotz 1963; Abel & Blanchard 1986; Khan &
    /// Thomas 2008). 0 = raw k* (the legacy path).
    pub kstar_adapt: f64,
    /// Mean lag (months) with which firms' investment plans and households' spending respond to interest rates:
    /// they act on an exponentially smoothed rate (time-to-plan, Christiano & Todd 1996; habit-like inertia,
    /// Christiano, Eichenbaum & Evans 2005). 0 = respond to the current rate.
    pub rate_perception_months: f64,
    /// Target capacity utilisation of the firms' desired-output rule. Default since joint2 (docs/CALIBRATION.md, Joint
    /// calibration v2): 0.83 with upstream_margin 0.05, the S6 level calibration (32-seed hard bands and gates pass;
    /// energy_profit_gdp 1.07 and cu_level 80.70 meet T1). Legacy 0.85 in scripts/profiles/legacy_j2off.json.
    pub target_utilization: f64,
    /// Input–output coefficients A[from][to].
    pub io: [[f64; N_SECTORS]; N_SECTORS],
    /// Crude input as share of Energy output value at base oil price.
    pub crude_intensity: f64,
    /// Share of Energy employment in oil and gas extraction, whose labour needs scale with the lagged real oil price (oil ÷ price-level index) to the
    /// power `energy_extraction_elasticity` (0 disables; BLS: mining and oil & gas ≈40% of energy employment).
    pub energy_extraction_labor_share: f64,
    pub energy_extraction_elasticity: f64,
    /// Share of crude purchases paid abroad (US is roughly a net exporter: small).
    pub oil_import_share: f64,
    pub employment_share: [f64; N_SECTORS],

    // --- sector roles ---
    // Which sectors play each economic role, per sector (indexed as `Sector::ALL`). Split roles (`capital_goods_mix`,
    // `gov_purchase_split`, `household_services_mix`) are used as weights of the split; price-index roles are
    // normalised by `types::role_mean`, so their weights need not sum to 1.
    /// Split of firms' investment orders across capital-goods sectors (durables 37%, Tech 17%, other services 26%, Construction 20%). It also
    /// prices capital goods bought by firms (`unit_inv_cost`).
    pub capital_goods_mix: [f64; N_SECTORS],
    /// Price index at which firms' capital is valued (balance sheets, owner equity, liquidation) and the investment
    /// deflator is taken (the same capital-goods mix as `capital_goods_mix`).
    pub capital_valuation_mix: [f64; N_SECTORS],
    /// Weight of each sector in traded goods: import competition and import shares of household and firm purchases,
    /// the import price index and the purchasing-power anchor of the dollar (durables, nondurables, Food and Tech).
    pub tradable_weight: [f64; N_SECTORS],
    /// Sectors whose output is non-storable: unsold output perishes at month end instead of entering inventory
    /// (LeisureHospitality, HealthEducation and OtherServices today).
    pub nonstorable_sectors: [bool; N_SECTORS],
    /// Sectors whose firms are typically owner-operated, so an owner-household can start them (Food, Leisure,
    /// HealthEducation, OtherServices and Construction today).
    pub proprietor_sectors: [bool; N_SECTORS],
    /// Split of household service purchases across sectors: owner maintenance, landlords' operating costs and
    /// owner maintenance and landlords' operating costs (OtherServices); employer health benefits go to `health_sector_mix`.
    pub household_services_mix: [f64; N_SECTORS],
    /// Sectors that in-kind health care (Medicare/Medicaid, recorded as personal consumption) is bought from and
    /// deflated by (HealthEducation today).
    pub health_sector_mix: [f64; N_SECTORS],
    /// Sectors that financial services furnished without payment (FISIM) are priced in (OtherServices today).
    pub financial_services_mix: [f64; N_SECTORS],
    /// Split of total government purchases (health included, planned in `fiscal::plan_spending`) across sectors.
    /// Its nonzero sectors also weight the government purchase deflator.
    pub gov_purchase_split: [f64; N_SECTORS],
    /// Weights of sector prices in headline CPI. All sectors but Construction are CPI components; Construction is
    /// not. Also the salience weight of Energy in expected inflation (`consumption`).
    pub cpi_weights: [f64; N_SECTORS],
    /// Weight of the rent index (shelter) in CPI.
    pub cpi_weight_shelter: f64,
    /// Sectors included in core CPI (headline excluding food and energy).
    pub cpi_core_sectors: [bool; N_SECTORS],
    /// Weights of sector prices in core PCE (BEA PCE excluding food and energy).
    pub pce_core_weights: [f64; N_SECTORS],
    /// Weight of the rent index (shelter) in core PCE.
    pub pce_core_weight_shelter: f64,

    // --- households / consumption ---
    pub subsistence: [f64; N_CONSUMER],
    pub supernumerary_share: [f64; N_CONSUMER],
    pub mpc_income: f64,
    /// Fall in the income MPC per unit of real interest rate above neutral (intertemporal substitution).
    pub consumption_rate_sens: f64,
    pub mpc_wealth_above: f64,
    pub mpc_wealth_below: f64,
    pub mpc_housing_wealth_annual: f64,
    pub mpc_fund_wealth_annual: f64,
    // fix-households: transitory one-off payments (helicopter money). Share of the payment spent in the month received
    // when pre-receipt liquid wealth is below the buffer (Parker et al. 2013; Fagereng, Holm & Natvik 2021) or above it.
    pub mpc_windfall_low: f64,
    pub mpc_windfall_high: f64,
    /// Monthly share of the unspent part of earlier one-off payments that is spent (the remainder is spent slowly).
    pub windfall_decay: f64,
    // fix-households: the windfall MPC is `mpc_windfall_low` when pre-receipt liquid wealth is below
    // max(buffer, this many months of permanent income), so hand-to-mouth households get the high MPC.
    pub windfall_liquid_months: f64,
    // fix-households: sector split of one-off payments that are spent (sums to 1). Durables-tilted (Parker et al. 2013:
    // about 45% of stimulus spending goes to durables and nondurables); leisure is excluded because it is
    // capacity-constrained (its fill rate falls short of orders). Round 7 step 9: durables 0.12, between the measured
    // points (0.45 gives +62% durables at month 3; 0.20 gives +29%), with the remainder spread to energy, nondurables
    // and other services. Aggregate 2021 data are about +10% with checks twice this size; a PCE-proportional share is
    // the neutral prior. The legacy mix is [0.0, 0.10, 0.45, 0.20, 0.10, 0.10, 0.0, 0.05].
    pub windfall_sector_mix: [f64; N_CONSUMER],
    // fix-households: necessity demand for energy and food responds to price with a low short-run elasticity that rises
    // over time. Real subsistence quantity is multiplied by exp(-e_long * (A - A_ref)), where A is an exponentially
    // smoothed log price (speed `necessity_adapt` a month). Short-run elasticity is about e_long * necessity_adapt
    // (Hughes, Knittel & Sperling 2008: gasoline about -0.03 to -0.08); long-run is e_long (Labandeira et al. 2017:
    // -0.2 to -0.3). Higher prices then squeeze other discretionary spending.
    pub energy_elasticity_long: f64,
    pub food_elasticity_long: f64,
    pub necessity_adapt: f64,
    // fix-households: impatient types keep this weight on last month's permanent income (patient types 0.9), so their
    // spending follows current income (KVW 2014).
    pub impatient_yp_weight: f64,
    // fix-households: monthly pull of permanent income towards current income after UI exhaustion (patient types use
    // 0.1 while employed; the slower pull keeps spending flat after exhaustion, Ganong & Noel 2019).
    pub exhaust_yp_pull: f64,
    // fix-households: reduction of the income MPC of patient households (not impatient ones), so that the extra
    // spending of impatient types is offset in aggregate.
    pub patient_mpc_offset: f64,
    // fix-households: hand-to-mouth heterogeneity (Kaplan, Violante & Weidner 2014). A persistent impatient type (drawn
    // from the keyed RNG by household id) keeps a small buffer and spends liquid wealth above it quickly.
    /// Share of households that are impatient.
    pub impatient_share: f64,
    /// Liquid buffer of impatient households, in months of permanent income (patient types use `k` by age).
    pub impatient_buffer_months: f64,
    /// Monthly MPC out of liquid wealth above the buffer for impatient households.
    pub impatient_mpc_above: f64,
    /// Impatient households buy Fund shares only from deposits above this many months of permanent income.
    pub impatient_fund_floor_months: f64,
    // fix-households: job losers (Ganong & Noel 2019; Davis & von Wachter 2011). Permanent income on UI is the pre-loss
    // level times `ui_yp_onset` in the first month of the spell, then falls by `ui_yp_drift` a month during receipt.
    /// Factor on permanent income in the first month of a UI spell (about -6% spending at onset).
    pub ui_yp_onset: f64,
    /// Monthly decline of permanent income during UI receipt (about -1% spending a month).
    pub ui_yp_drift: f64,
    /// Weight on permanent income (rest on current income) in the one-month credit line after UI exhaustion. 0 = current
    /// income only (the largest step at exhaustion); 1 = permanent income.
    pub exhaust_credit_share: f64,
    // fix-households: retirees' drawdown (De Nardi 2004; De Nardi, French & Jones 2010).
    /// Years of bequest and longevity horizon added to remaining life expectancy in the retirees' annuity.
    pub retiree_horizon_years: f64,
    /// Real discount rate of retirees' drawdown (slow-moving, so policy-rate moves do not change spending).
    pub retiree_real_rate: f64,
    pub credit_card_spread: f64,
    /// Share of each consumer sector's supernumerary (above-subsistence) spending that is discretionary. The
    /// discretionary multiplier D (consumption.rs, DESIGN §8.3) scales only this part. Durables, leisure and much of
    /// tech are the cyclical categories (Stock & Watson 1999: durables consumption sd ≈2.9× GDP's, services ≈0.4×);
    /// food, energy and health barely move.
    pub discretionary_weight: [f64; N_CONSUMER],
    /// Discretionary multiplier D = exp(θ_r·(r_neutral − r_real) + θ_m·excess_money + θ_u·(u* − u) + θ_c·credit_gap),
    /// clamped to [`disc_mult_min`, `disc_mult_max`]. θ_r: interest sensitivity of durables and big-ticket spending
    /// (Erceg & Levin 2006: durables several times more rate-sensitive than other spending).
    pub disc_theta_rate: f64,
    /// θ_m: hot-potato effect of excess money, excess_money = ln(deposits ÷ desired money) with desired money = the
    /// liquid buffer target (Friedman & Schwartz 1963: excess balances are spent first on durables and leisure).
    pub disc_theta_money: f64,
    /// Norm subtracted from ln(deposits ÷ desired money) so that excess money is zero at the calibrated baseline
    /// (average log ratio across households, ≈0.6 in the baseline; the money term then moves D only when money
    /// balances move relative to the buffer target).
    pub disc_money_norm: f64,
    /// θ_u: labour-market confidence and precautionary saving: u below u* raises discretionary spending
    /// (Carroll 1992; Mian & Sufi 2014 on household demand in the bust).
    pub disc_theta_unemp: f64,
    /// θ_c: bank-credit term, the household's bank's capital shortfall below `capital_requirement` (a credit-crunch
    /// proxy: banks below the requirement cut credit; Mian & Sufi 2014; Gertler & Karadi 2011 on bank-capital credit
    /// supply), clamped to −5pp. Zero when the bank is above the requirement.
    pub disc_theta_credit: f64,
    pub disc_mult_min: f64,
    pub disc_mult_max: f64,

    // --- goods market ---
    pub import_share: [f64; N_SECTORS],
    pub export_base_share_gdp: [f64; N_SECTORS],
    pub switch_intensity: f64,
    /// Monthly probability that a customer compares its preferred firm with a randomly drawn rival (per sector).
    pub switch_prob: f64,
    /// Log appreciation of the dollar per unit of US-minus-world real 2y rate differential (0.03 = 3% per 100bp).
    /// Uncovered interest parity: the dollar appreciates when US real rates rise relative to the rest of the world
    /// (Engel 2016: responses of roughly 2–5% per 100bp; Faust & Rogers 2003 on monetary-policy surprises and the
    /// dollar). Anchored in the long run by purchasing-power parity (see `fx::exchange_rate`).
    /// Round 7 step 9: 4.0 (was 3.0). Set to 4.0 by the step-9 guard: at 4.5 the pinned-7% GDP at month 12 was 1.0pp
    /// worse than Checkpoint B (limit 0.7pp). A realised 100bp policy surprise moves the dollar by about 2.4% per pp of
    /// realised deviation at 4.0 (Engel 2016: 2–5%). Legacy 3.0.
    pub fx_rate_sensitivity: f64,
    /// Annual inflation of world prices of traded goods (US import prices of consumer goods were roughly flat over
    /// 1995–2019 as low-cost producers gained share; 0.02 = same as the US target).
    pub world_goods_inflation: f64,
    /// Purchasing-power anchor of the dollar on US goods prices (tradables) rather than the whole CPI, so cheaper
    /// imports are not offset by a falling dollar when nontradable (shelter, services) inflation is higher
    /// (Balassa–Samuelson).
    pub fx_ppp_tradables: bool,
    /// Weight of the purchasing-power anchor in the dollar's target (1 = full relative PPP; PPP deviations decay
    /// slowly in the data, with a 3–5-year half-life: Rogoff 1996).
    pub fx_ppp_weight: f64,
    /// Monthly share of the gap between the current and target log exchange rate that closes each month (partial
    /// adjustment toward the PPP-plus-differential anchor; Engel 2016 documents slow, persistent deviations).
    pub fx_adjust: f64,
    /// Imported share of firms' intermediate purchases of manufactured goods (industrial supplies, parts). Scaled
    /// with relative import prices as household import shares are (see `import_price_elasticity`).
    pub firm_input_import_share: f64,
    /// Imported share of firms' equipment purchases (≈⅓ of US equipment investment is imported; BEA end-use imports).
    pub firm_equipment_import_share: f64,
    /// Elasticity of import spending shares to the relative price of domestic to imported goods (Armington-type
    /// substitution; Hooper, Johnson & Marquez 2000 estimate short-run US import price elasticities well below 1).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub import_price_elasticity: f64,
    // fix-trade: import prices pass through partially and with a lag (Campa & Goldberg 2005: US import-price
    // pass-through about 0.25 short run, 0.4-0.5 long run; Gopinath et al. 2010). Only the exchange-rate move is
    // passed through partially; world inflation passes through fully (see goods::import_pass_through).
    // fix-trade
    pub import_pass_through_long: f64,
    /// Monthly share of the gap to the long-run pass-through target closed (1 = no lag). 0.08 closes about 22% of the
    /// gap in the first quarter.
    // fix-trade
    pub import_pass_through_speed: f64,
    /// Weight of each sector in the CPI / PCE tradables composite (domestic goods and imports priced off the dollar).
    /// Food is excluded (it is priced on its own, so core and headline keep food out of the tradables index).
    // fix-trade
    pub cpi_tradable_weight: [f64; N_SECTORS],
    /// Elasticity of the world price of energy to the oil price relative to its base (1 = world energy price moves
    /// one-for-one with oil; 0 = oil absent from world prices, the previous behaviour). Used for Energy-sector trade.
    // fix-trade
    pub world_energy_oil_elasticity: f64,

    // p1-energy (plan items 1 and 9). Both switches are on in the defaults since the re-fit (p1-all). The pre-re-fit
    // state (switches off, the integrate model at a06a973) is scripts/profiles/legacy_p1off.json. The overlay scripts/profiles/p1-energy.json, which also sets the oil process
    // (`oil_ar`, `oil_sigma`, `oil_t_nu`, `oil_garch_*`, `oil_index_to_world`, the fields above in the p0-exog block).
    //
    /// Motor-fuel / utilities split of the Energy price (item 1). false = integrate's Energy price (½ oil ratio + ½
    /// unit-cost ratio), gasoline = 1.25 × Energy price + oil/42, and the world energy price = oil ratio^elasticity.
    /// true: the Energy price is a blend of motor fuel and utilities (below); gasoline is the fuel line; the world
    /// energy price follows real oil through the fuel line's crude share only.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub energy_fuel_split: bool,
    /// Household inflation-expectations salience follows the 12-month change in the pump price (item 1). false =
    /// integrate's term, `EXP_INFL_SALIENCE` (0.03) × the 12-month change in the Energy CPI. true: `pump_salience` ×
    /// the pump-price change (`common::pump_yoy`). The pump price is the gasoline line, so this is meaningful with
    /// `energy_fuel_split` on.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub pump_expectations: bool,
    // The Energy price is a blend of motor fuel (share `energy_fuel_share`) and utilities. Motor fuel is
    // priced per gallon: `gas_tax` ($/gal, fixed taxes) + `gas_margin` ($/gal, indexed to the non-energy price level)
    // + `gas_crude_yield` × oil ($/bbl) / 42. Utilities move with their non-crude cost and with a 12-month average of the
    // oil ratio, elasticity `util_oil_elasticity` (slow, regulated pass-through). The fuel line is what the world energy
    // price and the household inflation-expectations salience (`pump_salience`) follow.
    /// Motor fuel's share of the Energy price index (≈ motor fuel's share of CPI energy; BLS relative importance).
    pub energy_fuel_share: f64,
    /// Fixed motor-fuel taxes and distribution charges, $/gallon (EIA: federal and state taxes ≈ $0.57 in 2024).
    pub gas_tax: f64,
    /// Refining, distribution and retail margin, $/gallon at the non-energy price level of 1 (EIA breakdowns).
    pub gas_margin: f64,
    /// Crude pass-through to the pump, gallons of gasoline per barrel ÷ 42 (≈ 1.1–1.15 × oil/42, EIA).
    pub gas_crude_yield: f64,
    /// Elasticity of utilities' price to the 12-month average oil ratio (electricity and utility gas; Borenstein et al.).
    pub util_oil_elasticity: f64,
    /// Monthly increment to household expected inflation per unit of the 12-month pump-price change, used when
    /// `pump_expectations` is on (`common::pump_yoy`). It is added every month, and the adaptive update
    /// (`EXP_INFL_ADAPT` = 0.1) relaxes the gap back, so the long-run gap is about 10 × `pump_salience` × the change
    /// (0.04 × 10% = 0.4pp), and a 12-month change in expectations is about 72% of that (0.29pp per +10% pump price).
    /// Measured on the profile (8 seeds): 0.29pp per +10% (long runs 0.26; data 0.25 per Coibion & Gorodnichenko 2015;
    /// base 0.13). The literature 0.025 in the same units would give 6–7 times that slope (about 1.8pp per +10%): at
    /// 0.025 food CPI sd rose to 4.4pp (ok range 1.2–2.3), so 0.004 is the on-target value, not a deviation.
    pub pump_salience: f64,
    /// rz2: weight of the pump-price (energy-salience) part of household expected inflation in the real rate that sets the
    /// income MPC and the discretionary multiplier (`consumption::decide_one`). 1 = the legacy real rate (the whole expectation,
    /// the default); 0 = the gasoline-driven part is dropped from the real rate, so a pump-price surge no longer lowers the
    /// real rate households face and pulls spending forward. The expectation itself (wages, firms, the Fed) is untouched.
    /// Units: weight, 0 to 1. Evidence: Coibion, Georgarakos, Gorodnichenko & van Rooij (2023), Bachmann, Berg & Sims (2015).
    pub hh_pump_real_rate_weight: f64,
    /// rz2: weight of the pump-price (energy-salience) part of household expected inflation in the expectation that enters the
    /// investment user cost when `user_cost_pi_e_mode` is 0 and `user_cost_household_expectation` is true. 1 = the legacy user cost
    /// (the whole household mean expectation, the default); 0 = the gasoline-driven part is dropped, so a pump-price surge no longer
    /// lowers the real user cost of capital and lifts investment in the months before the Fed moves. The anchored part of the
    /// expectation (target and recent inflation) stays, so the baseline user cost keeps its link to inflation (modes 2 and 3 cut it).
    /// Units: weight, 0 to 1. Evidence: Sharpe & Suarez (2021), Coibion, Gorodnichenko & Kumar (2018), Blanchard & Gali (2010).
    pub firm_pump_user_cost_weight: f64,

    // --- housing ---
    pub houses_per_household: f64,
    pub median_house_price: f64,
    pub gross_rental_yield: f64,
    pub property_tax: f64,
    pub maintenance: f64,
    pub max_ltv: f64,
    pub max_ltv_investor: f64,
    pub max_dti: f64,
    pub mortgage_term_months: u32,
    pub construction_months: u32,
    /// Real construction-sector units needed per average-quality house.
    pub construction_units_per_house: f64,
    pub land_share: f64,
    pub base_starts_monthly_share: f64,
    /// During burn-in (demography frozen), retire one vacant rental per completed house so the stock does not grow.
    pub burn_in_hold_stock: bool,
    /// Rent growth reacts to Census-HVS rental vacancy relative to its level at the first reported month (rather than
    /// to vacant rentals over renter households relative to the first burn-in month).
    pub rent_vacancy_hvs: bool,
    /// Share of nominal income growth in excess of expected inflation that passes into rents each year. Rents grow
    /// with nominal incomes in the long run (BLS CPI shelter ≈ core + 0.5–1pp, 1990–2024).
    pub rent_income_pass: f64,

    // --- banks ---
    pub deposit_beta: f64,
    pub mortgage_spread: f64,
    pub business_spread: f64,
    pub capital_requirement: f64,
    pub bank_payout_ratio: f64,
    /// Loan losses booked after the month's bank P&L are recognised in the next month's P&L. Business write-offs of
    /// failed firms are booked in `firms::close_books`, which runs after `banking::service` has computed profit and
    /// dividends and after the bank tax, and `sim::begin_month` used to zero them. They cut bank equity but never
    /// bank profit, dividends or the bank tax base (bank_profits_bn_ar left out the business charge-offs that
    /// loan_losses_bn_ar counts). With the key on, `sim::begin_month` keeps the part of `Bank.loan_losses` that the
    /// P&L has not recognised, so every loss reaches the P&L exactly once, one month late when it is booked late.
    /// Legacy `false` (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub bank_loss_carry: bool,
    pub discount_spread: f64,
    /// Lender balance-sheet premium (excess bond premium), added to a bank's business loan base rate. Loss term:
    /// percentage points of premium per pp by which the expected loss rate on the bank's business loans (loan-weighted
    /// LGD × PD of current borrowers) exceeds its slow norm. Spreads move by more than expected losses because risk
    /// appetite falls with them (Gilchrist & Zakrajšek 2012). 3 gives a spread cycle sd ≈0.34pp, countercyclical
    /// and slightly leading, and investment volatility ≈3× GDP's (US ≈3×).
    pub ebp_loss_sens: f64,
    /// Capital term: premium per unit of capital-ratio shortfall below `ebp_capital_buffer` above the requirement
    /// (lenders pay more to fund themselves as their capital thins; Gertler & Karadi 2011 leverage constraint).
    pub ebp_capital_sens: f64,
    pub ebp_capital_buffer: f64,
    /// Monthly partial-adjustment speed of the premium toward its target (≈2.5-month half-life).
    pub ebp_adjust: f64,
    /// Bounds on the premium (about −0.5pp to +4pp).
    pub ebp_min: f64,
    pub ebp_max: f64,
    /// Monthly adaptation of a bank's slow-moving norm for the business loss rate (≈6-year half-life).
    pub loss_norm_adapt: f64,
    /// Initial value of that norm: the C&I charge-off rate in normal times (≈0.5% a year; Federal Reserve charge-off
    /// data, 1990s–2000s average).
    pub loss_norm_initial: f64,
    /// Default-probability pricing of firm loans: annual PD = logistic(pd_intercept + pd_leverage × leverage
    /// + pd_coverage × ln(1/interest coverage)). Spread = pd_lgd × (PD − PD at the reference firm), so the average
    /// firm's premium is about zero (Merton-style expected-loss pricing; loss-given-default 0.6, Altman & Kishore 1996).
    pub pd_intercept: f64,
    pub pd_leverage: f64,
    pub pd_coverage: f64,
    pub pd_lgd: f64,
    /// Credit standards (SLOOS tightening): each pp of bank premium above zero lowers the maximum post-loan leverage
    /// (and rollover leverage) by this many points, and raises the minimum interest coverage by this share.
    pub standards_leverage_per_pp: f64,
    pub standards_coverage_per_pp: f64,
    /// Months a business loan may stay in arrears before the firm is treated as in default (Config doc §11.3).
    pub business_default_months: u32,

    // --- p1-firmsentry: entry, customers and exit of firms (firms.rs, goods.rs; docs/CALIBRATION.md "Firm entry and exit").
    // Switches are on in the defaults since the re-fit (p1-all). The pre-Phase-1 behaviour is reproduced by
    // scripts/profiles/legacy_p1off.json; scripts/profiles/p1-firmsentry.json is the historical overlay.
    /// Customer floor for young firms: in the household switching draw a firm younger than `entrant_intro_months`
    /// carries at least this share of the mean expected demand of its sector (0 = off).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entrant_customer_floor: f64,
    /// Introductory price discount of a firm younger than `entrant_intro_months` (share of its posted price; 0 = off).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entrant_intro_discount: f64,
    /// Months of the introductory price discount and customer floor.
    pub entrant_intro_months: u32,
    /// A household switching between equally priced firms switches with probability ½ (false = a tie blocks it).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entrant_tie_switch: bool,
    /// Entrant start-up equity from the Fund, in months of a two-worker payroll (incl. employer tax).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entrant_startup_months: f64,
    /// Log-sd of the productivity draw of an entrant (mean-one lognormal; incumbents' sector average is the mean).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entrant_productivity_sigma: f64,
    /// Entry gate: a potential entrant enters with probability logistic(slope × (sector profit rate − hurdle)); slope 0
    /// admits every candidate (no endogenous entry). Profit rate = 12 × monthly profit / capital at replacement cost.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entry_gate_slope: f64,
    pub entry_gate_hurdle: f64,
    /// Cap on gated re-entry per sector per month, as a share of the sector's living firms (at least one slot; 0 = off,
    /// every dead slot whose gate admits it is reborn in the same month). Without the cap a sector whose gate opens
    /// after a long closed spell reborns its whole backlog at once (seeds 5 and 7 of p1-all, 34-36% one-month GDP moves).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entry_reborn_max_share: f64,
    /// Growth entry targets the firm count in proportion to the labour force (employed + unemployed) instead of the
    /// households per firm ratio, which counts retirees and gives entry regardless of demand.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub entry_labour_force: bool,
    /// In proprietor sectors an employed household founds each entrant, paying up to its deposits towards the start-up
    /// equity; the founder becomes the owner and receives the firm's payouts (proprietors' income).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub owner_founders: bool,
    /// Fixed monthly operating cost of every firm, as a share of the mean monthly revenue of living firms; paid to the
    /// Fund. Keeps the exit hazard of established low-margin firms positive (Hopenhayn 1992; Ericson & Pakes 1995).
    pub firm_fixed_cost_share: f64,
    /// Merton-style default test: a firm's capital is valued at q × replacement cost, q = smoothed return on capital ÷
    /// cost of capital, clamped to [merton_q_min, merton_q_max]. Cost of capital 0 = off (replacement-cost values).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub merton_cost_of_capital: f64,
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub merton_q_min: f64,
    pub merton_q_max: f64,
    /// Fire-sale recovery with the Merton test on: LIQUIDATION_RECOVERY × clamp(q, merton_recovery_floor, 1). The
    /// test (default) uses q, the loss does not have to: 1 = a fixed recovery share, so charge-off losses do not rise
    /// in slumps merely because q falls.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub merton_recovery_floor: f64,
    /// Idiosyncratic productivity of established firms, relative to the sector TFP index: monthly innovation sd of the
    /// log (0 = off) and the monthly reversion of log productivity to the sector's anchor (its geometric mean when the
    /// process first ran, then fixed, so exits do not ratchet the level up). Stationary sd of log productivity =
    /// sigma / sqrt(2 × revert).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub firm_prod_sigma: f64,
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub firm_prod_revert: f64,

    // --- b5-firmhet: entry viability (S1; firms.rs reborn, plan_firm, entry_accepts). Source: Hopenhayn (1992); the
    // zero-capital trap in docs/CALIBRATION.md "b5-firmhet". Every field is switched off at its legacy value.
    /// Entrant capital floor in workers: K_e = max(0.5 × median K, n_w × lp_e / (cp × u_n)), so a new firm can equip
    /// n_w workers at normal utilisation even where the sector's median capital is near zero. 0 = legacy 0.5 × median.
    pub entrant_capital_floor_workers: f64,
    /// Expansion cap base: the monthly capital order is capped at 2% of max(K, K_lab), with K_lab the capital that
    /// equips the current workforce at normal utilisation. A firm with K = 0 and workers can then order capital.
    /// false = legacy cap of 2% of K.
    pub inv_expand_labour_ref: bool,
    /// Entry gate signal: the capital-weighted mean of the sector's profit rate (weights = capital value) instead of
    /// the unweighted mean, which lets many small loss-making firms shut entry into a sector. false = legacy mean.
    pub entry_gate_capital_weighted: bool,
    /// S1 fallback (contestability bypass): a sector with fewer living firms than this share of its calibrated count
    /// admits every entrant, whatever the gate says. 0 = off (legacy gate).
    pub entry_contestability_share: f64,
    // --- b6-firmsize (S3 cyclical entry gate; S5 Pareto appeal with price-elastic demand). Every field is switched
    // off at its legacy value. Sources: Gabaix (1999), Luttmer (2007), Anderson, de Palma & Thisse (1992).
    /// S3: cyclical entry gate p = logistic(logit(p0) + β (π̃_j − π̄_j)), with π̃_j a 3-month EMA of the sector's
    /// capital-weighted profit rate and π̄_j a slow EMA (`entry_gate_anchor_rate`). false = legacy gate.
    pub entry_gate_cyclical: bool,
    /// S3: base entry probability p0 (its logit is the gate intercept).
    pub entry_gate_base_prob: f64,
    /// S3: sensitivity β of the entry probability to the profit-rate gap (π̃ − π̄).
    pub entry_gate_cyc_slope: f64,
    /// S3: monthly rate of the slow profit-rate EMA π̄_j (0.0115 ≈ 5-year half-life).
    pub entry_gate_anchor_rate: f64,
    /// S3 variant: level term γ in the cyclical logit, + γ (π̃_j − hurdle) on the fast EMA (0 = off, pure gap gate).
    /// Lets the gate keep structurally unprofitable sectors closed, which the gap-only gate does not.
    pub entry_gate_level_slope: f64,
    /// S3 fallback: aggregate u-gap term in the cyclical logit, + β_u · 100 · (u* − u) with u the current unemployment
    /// rate and u* the natural rate (0 = off). Sector-independent.
    pub entry_gate_u_beta: f64,
    /// S5: household choice weights v_f = a_f · p_f^(−ε) (appeal times price elasticity) instead of expected demand;
    /// the appeal a_f follows a reflected Gibrat process. false = legacy expected-demand sampling.
    pub demand_logit: bool,
    /// S5: price elasticity ε of the choice weights (grid {3, 4.5}).
    pub demand_elasticity: f64,
    /// S5: monthly probability that a household re-draws its preferred firm in proportion to v, with no price test.
    pub demand_rechoice_prob: f64,
    /// S5: monthly volatility σ_a of the log appeal (reflected Gibrat step).
    pub appeal_sigma: f64,
    /// S5: Pareto tail exponent ζ of the appeal; the drift is μ_a = ζ σ_a² / 2 so the stationary tail is A^(−ζ).
    pub appeal_zeta: f64,
    /// S5: log appeal of an entrant (the barrier at which a new firm starts).
    pub entrant_appeal_ln: f64,
    // S2 (size-linked markup norm; firms.rs plan). Atkeson & Burstein (2008) / Kimball: a firm with expected demand
    // above its sector mean gets a higher norm, κ·max(0, ln q), q = ed / sector mean. Energy is excluded.
    /// κ: slope of the markup norm on ln of relative expected demand (0 = legacy norm m̄_j + rent).
    pub markup_size_kappa: f64,
    /// Capped compensation (Edmond, Midrigan & Xu 2023 style): the sector norm is lowered by
    /// min(c_max, κ · Σ ω_f max(0, ln q_f)), so the sector's average norm is roughly unchanged. 0 = no compensation.
    pub markup_size_comp_max: f64,
    /// S2 fallback: the premium applies only above this relative demand, κ·max(0, ln(q / q_min)) (the large-firm
    /// segment when q_min = 3). 1 = legacy form κ·max(0, ln q).
    pub markup_size_q_min: f64,
    // S4 (lumpy investment; firms.rs plan_firm investment block). Cooper & Haltiwanger (2006): inaction bands and
    // one-time catch-up instead of years of spikes; Abel & Eberly (1996) partial irreversibility; Caballero & Engel (1999).
    /// Lumpy investment on the firm-specific gap z = ln(K/k*) − (sector capital-weighted mean). false = legacy rule.
    pub inv_lumpy: bool,
    /// Inaction band s_hi: a firm with z above it orders no capital (not even replacement). Used only with `inv_lumpy`.
    pub inv_inaction_band: f64,
    /// Catch-up band s_lo: a firm with z below −s_lo closes its gap in `inv_catchup_months` months. 0 = off.
    pub inv_catchup_band: f64,
    /// Months over which a catch-up firm closes its gap to desired capital.
    pub inv_catchup_months: f64,
    /// Catch-up cap: the monthly gap is at most this share of max(K, K_lab).
    pub inv_catchup_cap: f64,

    // --- central bank ---
    pub inflation_target: f64,
    pub r_star: f64,
    /// Annual adjustment of the central bank's real-time r* estimate per pp of persistent (core) inflation miss: when
    /// inflation stays below target at the rule's rate, the neutral rate must be lower (the logic of real-time
    /// Holston–Laubach–Williams estimates). 0 keeps r* fixed. Bounded to [−1%, 3%]; frozen under rate overrides.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json. Re-fit Step 3: the default is 0
    /// (r* fixed at `r_star`). At the zero bound, integrating below-target inflation wound r* down to its −1% floor
    /// (seed 8 of the post-merge p1-all runs: a 20-year policy average of 0.63), a ZLB trap after recovery.
    /// p1-all's 0.3 is scripts/profiles/legacy_p1all.json.
    pub r_star_adapt: f64,
    /// Re-fit Step 3 fallback: skip negative-gap integration of r* while the policy rate is at or below 0.25%.
    /// Legacy false (the integration runs at all rates), so the legacy path is bit-identical.
    pub r_star_antiwindup: bool,
    /// Re-fit Step 3: lower bound on the adaptive r* estimate (real, decimal). Legacy −0.01, the former constant
    /// R_STAR_EST_MIN.
    pub r_star_est_min: f64,
    pub natural_unemployment: f64,
    pub phi_pi: f64,
    pub phi_u: f64,
    pub rate_smoothing: f64,
    pub initial_policy_rate: f64,
    pub rule_uses_core: bool,
    /// With `rule_uses_core`, react to core PCE-weighted inflation (the FOMC's target measure) rather than core CPI.
    pub rule_uses_pce: bool,
    pub tp_base: [f64; N_TENORS],
    pub tp_duration_kappa: f64,
    pub initial_curve: [f64; N_TENORS],
    /// Base equity risk premium (decimal) over the 10y yield. Damodaran's implied ERP for the S&P 500 is ≈ 4–4.5%, but
    /// with the model's profit growth (nominal GDP, ≈3.9%) and payout, 3.5% gives a trailing P/E of ≈18 and equity
    /// worth ≈2.3× GDP (3.0% gives ≈19–20 but pushes bank reserves and the 10-year yield too high). The time-varying premium is `erp + erp_unemp_sens × (u − u*) − erp_extrap × (R12 −
    /// erp_return_anchor)`, clipped as above and bounded to [erp_min, erp_max] (equity::equity_risk_premium).
    pub erp: f64,
    /// Long-run growth of equity cash flows in the Gordon denominator (decimal). Set to the nominal trend growth
    /// (inflation target + TFP + labour force ≈ 3.9%), because the 10y yield is nominal.
    pub long_growth: f64,
    /// Mean lag (months) of the long rate in the equity discount rate (0 = current 10-year yield). With 6, a +100bp
    /// term-premium shock lowers equities ≈6% at 3 months and ≈9% at 6 months (2022: ≈8% per 100bp of 10-year yield),
    /// and a +100bp policy shock ≈2–4% (Bernanke & Kuttner 2005: ≈4%); with 0 the falls were ≈14% and ≈5%.
    pub equity_rate_months: f64,
    /// Countercyclical premium: ERP rises by this much per unit of unemployment gap (decimal on decimal). Campbell &
    /// Shiller (1988), Cochrane (2011) imply ERP up ≈ 1–2pp in a recession with u up ≈ 3pp (≈ 0.3–0.7 per unit).
    /// Default 0: with the valuation gain 1/cap ≈ 25, even 0.05 raises the GDP cycle sd from ≈1.56 to ≈1.7 and the
    /// unemployment sd from ≈1.0 to ≈1.1 through the Fund wealth channel, and pushes corr(ERP, GDP) to ≈ −0.8
    /// (target −0.3 to −0.6). See docs/DESIGN.md §11.4.
    pub erp_unemp_sens: f64,
    /// p1-equity: loading of the equity risk premium on the financial risk factor f_t (exog.rs; positive = risk-off).
    /// ERP_t = erp + erp_fin_loading × f_t, unsmoothed, so equities price a risk-off move as it arrives (Campbell &
    /// Cochrane 1999; Bekaert, Engstrom & Xu 2022). f_t also feeds the term-premium level (tp_level_fin_loading) and the
    /// dollar (fx_psi_fin_corr, fx_psi_fin_jump_loading) when those are non-zero. 0 = no factor in the premium.
    /// Re-fit Step 10a: with `erp_fin_build_months` > 0 the term is erp_fin_loading × fund.fin_erp_build, a build-up of
    /// (f_t − f̄) instead of f_t itself, so the premium moves over months rather than in one step.
    pub erp_fin_loading: f64,
    /// Re-fit Step 10a (off by default): months over which the financial-factor term in the ERP builds up (EWMA weight
    /// 1/months). 0 = unsmoothed (legacy: erp_fin_loading × f_t).
    pub erp_fin_build_months: f64,
    /// p1-equity: loading of the equity risk premium on the demand (risk-premium) shock exog.risk_premium, unsmoothed:
    /// ERP_t = ... + erp_rp_loading × risk_premium_t. The shock is contractionary (it raises the household real rate and
    /// the firms' user cost), so a positive loading reprices equities as a demand slump arrives. The loading is what
    /// gives the return volatility (vol_monthly) and the excess volatility; it does not by itself make returns lead GDP
    /// (see docs/CALIBRATION.md "p1-equity"). 0 = off.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub erp_rp_loading: f64,
    /// p1-equity: the demand loading erp_rp_loading is multiplied by 1 + erp_rp_gap_scale × max(u − u*, 0), so a demand
    /// shock moves the premium more in a slump. Swept at 20 and 40 in the item-5 screens: it did not make realised
    /// volatility countercyclical (vol_unemp_corr stayed near 0) and lowered the GDP-cycle fit, so it is off (0).
    pub erp_rp_gap_scale: f64,
    /// p1-equity: loading of the ERP on the slow demand-premium state Fund::erp_slow (a persistent component: it moves
    /// D/P slowly and so raises the persistence of log D/P without adding much return volatility). 0 = off.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub erp_slow_loading: f64,
    /// p1-equity: monthly decay of the slow demand-premium state (EWMA weight on the previous state). Default 0.98.
    pub erp_slow_decay: f64,
    /// p1-equity: the households' Fund-wealth term reads the 12-month EWMA of the Fund share price (equity::
    /// smoothed_share_price) instead of the current price. false = current price (base behaviour).
    pub fund_wealth_ewma: bool,
    /// p1-equity: the Tech boom discount (Pástor & Veronesi 2009) is phased in through Tech's productivity-news EWMA,
    /// scaled by news / tech_boom_extra_growth, instead of switching on with the boom flag. false = boom flag (base).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub tech_disc_news_phase: bool,
    /// p1-equity: floor on the equity capitalisation rate, decimal, in the 1/cap valuation (equity_valuation_log =
    /// false): the denominator y_eq + ERP + premium − g is never below max(this, 0.02), so a sector's value is at most
    /// 12 × smoothed dividends ÷ floor. Its only purpose is to bound the 1/cap convexity. It is a hard floor, so with
    /// the 0.03 setting it binds in about a quarter of sector-months under the 1/cap form, and while it binds equity
    /// does not respond to rates or premia (docs/CALIBRATION.md "p1-equity"). Default 0.02 is the base floor
    /// (equity::MIN_CAP_RATE), so the base output is unchanged. Not used by the log form.
    pub equity_cap_floor: f64,
    /// p1-equity: smoothing scale (decimal) of the capitalisation-rate floor, equity::bounded_cap. 0 = hard floor
    /// (max(cap, floor): the slope in rates and premia is zero below the floor). s > 0 gives the smooth bound
    /// floor + s × softplus((cap − floor) ÷ s), which keeps a slope below the floor. Default 0 = base behaviour.
    pub equity_cap_softness: f64,
    /// p1-equity: value sectors in log-linear form, E_s = 12·sd_s ÷ c_s × exp((c_s − r_s) ÷ c_s), where r_s is the raw
    /// capitalisation rate (y + ERP + premium − g) and c_s the sector's reference rate fixed at seeding (equity::
    /// value_sectors). ln E is then linear in r, as in the Campbell–Shiller log-price-dividend approximation, so the
    /// 1/cap convexity that makes returns positively skewed is gone. There is no capitalisation-rate floor, but the raw
    /// rate is bounded below at 0.005 (equity::LOG_CAP_GUARD): while it binds (1.8% of sector-months on the profile, about
    /// 61% of Tech boom months) the sector does not respond to rates or premia. false = the 1/cap form with
    /// `equity_cap_floor` (base behaviour).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub equity_valuation_log: bool,
    /// Extrapolative premium: ERP falls by this much per unit of trailing 12-month equity return above its anchor.
    /// Greenwood & Shleifer (2014) find investor return expectations track past returns; Barberis, Greenwood, Jin &
    /// Shleifer (2015) model the resulting low-premium booms. The loop price → premium → price has gain ≈ θ/cap, so
    /// θ ≥ 0.02 is unstable here (premium runs to its bounds); 0.005 keeps the gain ≈ 0.1 (docs/DESIGN.md §11.4).
    pub erp_extrap: f64,
    /// Anchor for the trailing 12-month total equity return (nominal, decimal). The long-run US nominal total return
    /// is ≈ 9–10%; 8% is the model's typical level (dividend yield ≈ 4.5% plus nominal growth ≈ 3.9%).
    pub erp_return_anchor: f64,
    /// Clip on the extrapolation signal (trailing return minus anchor), decimal. Without it the loop
    /// price → premium → price has gain erp_extrap / cap ≈ 0.5–1 and the premium runs to its bounds.
    pub erp_momentum_cap: f64,
    /// Lower and upper bounds on the time-varying equity risk premium (decimal).
    pub erp_min: f64,
    pub erp_max: f64,

    // fix-monetary
    /// Share of an announced QE programme's remaining purchases priced into the term premium at announcement
    /// (monetary::duration_ratio). 1 = the full stock effect is priced on the announcement date (Gagnon et al. 2011;
    /// Krishnamurthy & Vissing-Jorgensen 2011); 0 = the old flow-only pricing, where each purchase moves yields as it
    /// is bought. 0.5 puts a 10y programme at about −17bp per $1T of face at announcement (inside the −15 to −20bp
    /// target) and deepens it as purchases execute, without the drift-back that full pricing produced.
    pub qe_announcement_weight: f64,
    /// Term-premium response to a monetary-policy innovation: decimal term premium per decimal policy gap at the 10y
    /// (scaled by the tenor slope). 0.2 = +20bp of 10y term premium per 100bp on impact, decaying with the gap
    /// (Hanson & Stein 2015; Gertler & Karadi 2015 put the 10y at +20–40bp in total).
    pub policy_tp_response: f64,
    /// Monthly EWMA persistence of the sector Tobin's q signal that investment reads (sector_q_rel). 0.9 ≈ 10-month
    /// memory: a boom's q rise reaches investment over months rather than in one step. 0 = unsmoothed.
    pub q_signal_decay: f64,
    /// Mortgage-spread effect of central-bank agency-MBS holdings, as a decimal spread change per unit of MBS holdings
    /// divided by annual GDP (0.05 = -5bp per 1% of GDP; $1.25T, about 8% of GDP, gives about -40bp, Hancock & Passmore
    /// 2011). Read through MonetaryState::mbs_spread_adj by the banking module.
    pub mbs_spread_per_gdp: f64,
    // end fix-monetary

    // p1-curve
    /// Speed-limit weight in the policy target (Orphanides & Williams 2002): i* falls by this much per unit (decimal) rise
    /// of the three-month average of u over its level six months earlier. The average keeps single-month noise out.
    /// 0 = off (the p1-curve profile uses 0: with weight 0.5 to 1.5 the cycle's persistence fell, docs/CALIBRATION.md).
    pub speed_limit_weight: f64,
    /// Monthly gain of the long-run expected inflation π^LR toward core inflation (the curve's long-run anchor).
    /// 0 keeps π^LR at the target.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub lr_infl_gain: f64,
    /// Weight of the rule's target i* in the curve's expected short-rate path (1 = priced; 0 = the path decays from
    /// the current rate as before). The target priced is the EWMA `i_star_gain` of the rule's raw target. The p1-curve
    /// profile uses 0: pricing the projection cut the policy-rate response and the cycle's persistence (docs/CALIBRATION.md).
    pub rule_path_weight: f64,
    /// Monthly EWMA gain of the rule target i* (1 = the raw target, unsmoothed). set_policy always keeps `i_star_smooth`
    /// at this gain for the report; the curve prices it only when `rule_path_weight` > 0.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub i_star_gain: f64,
    /// Monthly decay of the rule target's projected path toward i_inf in the curve (the target's own persistence). 0.97
    /// is the base PATH_DECAY; a higher value keeps the priced target further out, so the 10-year loads more on it.
    /// Only read when `rule_path_weight` > 0.
    pub rule_path_decay: f64,
    /// Report the p1-curve state (π^LR, the rule target i*, the term-premium factor states) in the monthly metrics.
    /// Off in the pre-re-fit state (scripts/profiles/legacy_p1off.json); on in the defaults since the re-fit (p1-all).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub curve_report: bool,
    // end p1-curve

    // --- technology booms and sector equity (stage 4, docs/SECTORS.md §Stage 4, docs/DESIGN.md §4.1, §11.4) ---
    /// Markov tech-boom regime on (draws from the EXOG stream each month). Off: no draws, results unchanged.
    pub tech_boom_enabled: bool,
    /// Rise in the Tech sector's norm markup during a boom (innovation rents), reached after three years.
    pub tech_boom_markup_bonus: f64,
    /// Extra annual Tech productivity growth during a boom (decimal).
    pub tech_boom_extra_growth: f64,
    /// Extra annual productivity growth for adopters during a boom (DurableGoods, OtherServices).
    pub tech_boom_spillover: f64,
    /// Monthly probability of entering a boom (≈ 1/240: one boom every ≈20 years).
    pub tech_boom_entry_monthly: f64,
    /// Monthly probability of a boom ending (≈ 1/96: booms last ≈8 years).
    pub tech_boom_exit_monthly: f64,
    /// Pástor & Veronesi (2009) channel: Tech's equity premium is lower by this much (decimal) during a boom.
    pub tech_boom_erp_discount: f64,
    /// Sector equity risk premium over the economy-wide premium (decimal, by Sector::ALL order).
    pub erp_sector: [f64; N_SECTORS],
    /// Weight on a sector's trailing dividend growth above trend in its expected growth (extrapolation, Barberis et
    /// al. 2015; Greenwood & Shleifer 2014).
    pub sector_growth_extrapolation: f64,
    /// Monthly EWMA decay of the slow leg of a sector's dividend growth signal (0.99 ≈ 8-year memory).
    pub sector_growth_decay: f64,
    /// News weight: expected earnings growth rises by this much per unit of the sector's (EWMA of) excess productivity
    /// growth during a tech boom (Beaudry & Portier 2006 news; Pástor & Veronesi 2009). Realised sector dividends do
    /// not accelerate in this model (cost-plus pricing passes productivity into prices), so this is the channel that
    /// lets booms re-rate Tech.
    pub sector_productivity_news: f64,
    /// Monthly EWMA decay of the productivity-news signal (0.98 ≈ 4-year memory).
    pub sector_news_decay: f64,
    /// Bounds on a sector's expected nominal earnings growth g^e (decimal, per year).
    pub sector_growth_min: f64,
    pub sector_growth_max: f64,
    /// Monthly persistence of productivity news after a boom ends (faster unwinding than the build-up).
    pub sector_news_decay_bust: f64,
    /// Elasticity of a sector's desired capital to its Tobin's q (sector equity ÷ replacement value of its capital,
    /// relative to its normal level). 0 = no investment link. Applied per firm sector, bounded in firms.rs.
    pub investment_q_sens: [f64; N_SECTORS],
    /// Years of recent excess demand growth that firms extrapolate when sizing capacity, by sector (0 = size capital
    /// to current demand). Extrapolation produces overbuilding in booms and investment busts afterwards.
    pub investment_growth_horizon_years: [f64; N_SECTORS],

    // --- fiscal ---
    pub gov_purchases_share: f64,
    pub tax_brackets: Vec<(f64, f64)>,
    pub state_income_tax: f64,
    pub standard_deduction: f64,
    pub payroll_tax: f64,
    /// Employer-paid health insurance and similar benefits, as a share of wages (spent on health care; untaxed).
    pub employer_benefits: f64,
    pub payroll_cap_annual: f64,
    pub corporate_tax: f64,
    /// A firm reborn in a dead slot (`firms::reborn`, which reuses the slot's id) starts with no corporate-tax loss
    /// carry-forward. The dead predecessor's `FiscalState::firm_loss_cf` was kept, so the entrant paid no corporate tax
    /// until it had used up the failed firm's losses (households in a reused slot already start clean). Legacy `false`
    /// (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub entrant_loss_cf_reset: bool,
    /// Monthly pull of wage offers toward the firm's initial labour share (log gap).
    pub labor_share_pull: f64,
    /// Share of the gap to target inventory that production closes each month (partial adjustment ≈20–30% a
    /// quarter; Blinder & Maccini 1991).
    pub inventory_adjust: f64,
    /// Monthly adaptation of firms' expected demand toward recent sales (plus half of unmet demand).
    pub demand_adapt: f64,
    /// Firms plan layoffs only when labour exceeds need by this ratio, and then shed this share of the excess a month.
    /// Some labour hoarding (Fay & Medoff 1985; Biddle 2014): 1.08 and 0.4 bring Okun's coefficient to ≈ −0.52 (US
    /// ≈ −0.45) and the unemployment cycle sd to ≈0.9pp (US ≈0.75) once equity wealth amplifies the cycle.
    pub layoff_trigger: f64,
    pub layoff_speed: f64,
    /// Markup response per unit of aggregate output gap at each reprice. Default 0 since the re-fit (Step 7): the
    /// marginal cost already carries the gap through wages, so this term double-counts it and, with the lagged wage
    /// offers, creates a resonance of about 30 months. Only safe with `firm_prod_sigma` > 0: without it the legacy 0.3
    /// produces a ten-year boom-bust (y_sd 3.5). Legacy value 0.3 is in scripts/profiles/legacy_p1off.json and
    /// legacy_p1all.json.
    pub markup_demand_pull: f64,
    /// Weight of the inflation target in wage norms; the rest is 70% expected and 30% realised inflation.
    pub wage_norm_target_weight: f64,
    /// Weight of a 3-month annualised core nowcast (rest: the inflation target) in firms' inflation expectations, and
    /// switch from yoy headline to that nowcast for wage catch-up. 0 = firms use households' adaptive expectations.
    pub firm_nowcast_weight: f64,
    /// Monthly wage-offer rise for a firm that missed its hiring need, and cut for a firm planning layoffs.
    pub wage_unfilled_bonus: f64,
    pub wage_layoff_cut: f64,
    /// Measure those firm signals against their usual (slowly averaged) employment-weighted frequency, so they
    /// reallocate wages across firms without a built-in aggregate drift; aggregate tightness enters via the slope.
    pub wage_signal_relative: bool,
    /// Share of the change in the long-run cost of funds that firms build into the normal return in prices.
    pub target_return_pass: f64,
    /// Business property tax, annual share of firms' capital value (taxes on production; US ≈ $0.3T on commercial property).
    pub business_property_tax: f64,
    pub sales_tax: f64,
    pub auction_shares: [f64; 3],
    pub initial_debt_gdp: f64,
    /// Round 7 step 3: the fiscal rules respond to debt/GDP above a target path that starts at `initial_debt_gdp`
    /// and rises by this much per reported year (CBO current-law path, about 1.25pp a year at the default). Legacy 0.
    pub debt_target_slope: f64,
    /// Round 7 step 3: scale of the debt-stabilising tax and purchases rules (1.0 = the round-6 rules; 0.5 at the
    /// default, about 0.037 of GDP per unit of debt gap, inside Bohn-type estimates of 0.03–0.05).
    pub fiscal_rule_strength: f64,
    /// Round 7 step 4: households pay this month's scheduled mortgage payments and pass-through spending (landlord
    /// operating costs, employer health benefits) before discretionary consumption. Legacy false (the HEAD budget,
    /// which could overdraw by one month of permanent income and so pushed mortgages into arrears).
    pub housing_first_budget: bool,
    /// Round 7 step 4: months of permanent income of credit line the budget may use beyond deposits under
    /// `housing_first_budget`. Default 0 (no default consumer credit line). Ignored in the legacy branch.
    pub consumer_credit_months: f64,
    /// Round 7 step 5: under-62 retirees at init receive this share of their PIA (a proxy for SSDI, employer pensions
    /// and drawdowns, which the model lacks). Default 0.7, the lowest benefit in the SS schedule. Legacy 0.
    pub early_retiree_pia_factor: f64,
    /// Round 7 step 6: the first-time-buyer gate (six months employed) counts continuous employment, which a
    /// job-to-job move does not reset (`Household::months_employed`). Legacy false: months in the current job.
    pub first_buy_continuous_employment: bool,
    /// Round 7 step 7: no foreclosure starts in burn-in (tenure flows that frozen demography cannot offset wait for the
    /// reported run), and household mortgage delinquency is reset at the anchor. Legacy false.
    pub burn_in_freeze_foreclosures: bool,
    /// Round 7 step 10a: upper bound of the real-time natural-unemployment estimate (monetary). Default 0.044 (CBO
    /// noncyclical rate 2026); legacy 0.048.
    pub u_star_max: f64,
    /// Re-fit Step 11b: lower bound of the real-time natural-unemployment estimate (monetary). Legacy 0.038 (the former
    /// `U_STAR_MIN` constant); 0.042 by default, set because seeds in the 16-seed card had more than 6 months with u < 2.5.
    pub u_star_min: f64,
    /// Re-fit Step 9 (off by default, option (c) of D12): share of the net hiring gap that firms post each month
    /// (recruiting capacity: expansion hiring is spread over months). 1.0 = legacy (post the whole gap, rounded up),
    /// which is the default. Values below 1 are the measured candidate; they fail the firm guard and core PCE (see
    /// docs/CALIBRATION.md, Step 9 record).
    pub vacancy_net_share: f64,
    /// Re-fit Step 9 (off by default): with `vacancy_net_share` < 1, firms also post replacement hiring for their
    /// separations (`separation_rate` × headcount), which is filled before the net gap. Default and legacy false.
    pub vacancy_replace: bool,
    // b4-asym
    /// b4-asym M1 (off by default): how a firm rounds its posted vacancies when `vacancy_net_share` < 1.
    /// 0 = Step 9 stochastic rounding of `x` (the default); 1 = round the throttled gap up; 2 = hybrid: the free band
    /// `vacancy_free_units` is posted whole (rounded up) and only the excess is throttled. Legacy 0 (unused at share 1).
    pub vacancy_rounding: u32,
    /// b4-asym M1: gap in heads posted in full before the share applies (a free band, so steady-state hiring is
    /// neutral). Legacy 0.
    pub vacancy_free_units: f64,
    /// b4-asym M1: firms younger than this many months post their whole gap (legacy path). Legacy 0 (none exempt).
    pub vacancy_young_exempt_months: u32,
    /// Round 7 step 8: the rental affordability floor is 0.35 × the rent index (about $600 at t=0) rather than a nominal
    /// $600, and seekers may spend 10pp more of income on rent. Apply after `housing_first_budget`. Legacy false.
    pub rent_floor_indexed: bool,
    // fix-fiscal
    /// SSI-like floor for aged or disabled households: monthly maximum (single, partnered), in base-year $, indexed
    /// by the COLA index; paid only when liquid wealth (deposits + Fund) is below `ssi_asset_limit`.
    pub ssi_max_single: f64,
    pub ssi_max_couple: f64,
    pub ssi_asset_limit: f64,
    /// SNAP/TANF-like benefit for working-age households: monthly maximum (single, partnered), reduced by
    /// `snap_phaseout` × net income (wage + UI + pension), zero above `snap_asset_limit` of liquid wealth.
    pub snap_max_single: f64,
    pub snap_max_couple: f64,
    pub snap_phaseout: f64,
    /// Full SNAP benefit below `snap_asset_limit` of liquid wealth; linear phase-out to zero at `snap_asset_end`.
    pub snap_asset_limit: f64,
    pub snap_asset_end: f64,
    /// Multiplier on the SNAP benefit for students and households living with parents (fix-fiscal).
    pub snap_student_discount: f64,
    /// Medicare share of health spending; the rest is Medicaid (fix-fiscal).
    pub medicare_share: f64,
    /// EITC-like earnings credit (monthly wage): `eitc_rate` × wage up to `eitc_plateau`, phased out at
    /// `eitc_phaseout_rate` above `eitc_phaseout_start`.
    pub eitc_rate: f64,
    pub eitc_plateau: f64,
    pub eitc_phaseout_start: f64,
    pub eitc_phaseout_rate: f64,
    /// Tax rate on realised gains on Fund shares (losses carried forward).
    pub cap_gains_tax: f64,
    /// Averaging horizon (months) of debt/GDP in the fiscal and income-tax rules.
    pub fiscal_rule_avg_months: f64,
    /// Share of the gap between the target and actual stock by tenor that is closed by issuance each month.
    pub issuance_feedback: f64,
    /// Composition of the debt stock by tenor (fix-fiscal). Used for the t=0 stock (init) and as the target that issuance
    /// refinances toward; see `debt_mix`.
    pub maturity_stock_mix: [f64; N_TENORS],
    /// Share of unrealised Fund gains realised each year by deemed sale (fix-fiscal); sales of shares are taxed too.
    pub cap_gains_realisation: f64,
    pub cb_bonds_gdp: f64,
    pub reserves_gdp: f64,

    // --- initial macro targets ---
    pub initial_unemployment: f64,
    pub initial_inflation: f64,
    /// Unused by the build (kept for config compatibility); the build targets the CPS earner-household level, see init.rs.
    pub median_earnings_annual: f64,

    // --- fix-labour: labour-market incidence, search effort and participation (labor.rs) ---
    /// Multiplier on the exogenous separation rate by education (HighSchool, College, Graduate). Normalised so the
    /// employment-weighted mean (55/35/10) is 1, so the aggregate separation rate is unchanged (CPS: less-than-HS
    /// unemployment ≈1.9× BA+, from separations not job finding; Cairo & Cajner 2018, Hoynes et al. 2012).
    pub separation_mult_by_edu: [f64; 3],
    /// Search effort of an unemployed household falls by this factor times its benefit replacement (benefit / last
    /// wage) while benefits last (Chetty 2008 moral hazard). Contact is divided by the fixed `search_effort_norm`
    /// (not renormalised), so these cuts lower the mean job-finding rate unless the norm is recalibrated.
    pub ui_search_cut: f64,
    /// Extra search effort in the last benefit month (benefit exhaustion spike, Katz & Meyer 1990).
    pub ui_exhaustion_boost: f64,
    /// Monthly decay of employers' contact with the unemployed per month of unemployment
    /// (negative duration dependence, Kroft, Lange & Notowidigdo 2013). 1.0 = none. Compounds for
    /// `duration_contact_cap` months at most. Default since joint3 (docs/CALIBRATION.md, Joint calibration 3): 0.90
    /// (DC90; passes the duration targets T2 and J1 on the F1 base; the BLS band is not reached, 13.5 weeks). Legacy 0.92
    /// in scripts/profiles/legacy_dc92.json.
    pub duration_contact_decay: f64,
    /// fix-labour: months of unemployment after which the contact decay stops compounding (cap of the exponent). 12 (was
    /// 24). The earlier note said this brings the mean unemployment duration into the 17–23 week band; that is not what
    /// it does. The measured mean on the default (BLS clock, 32 seeds) is 12.9 weeks (docs/CALIBRATION.md, Joint
    /// calibration v2). Job-to-job premium 1.04.
    pub duration_contact_cap: f64,
    /// Contact probability of out-of-labour-force households of working age, relative to the unemployed's (direct
    /// N->E hires; 0 = none). Used with `applications_nonparticipant` applications.
    pub nonparticipant_contact: f64,
    pub applications_nonparticipant: usize,
    /// Log-sd of the match-specific pay shock (mean-one lognormal) drawn for each new hire: pay differs across
    /// firms for the same worker (Card, Heining & Kline 2013: sd ≈0.15). Lower values keep more offers above
    /// reservation wages, so unemployment falls and the policy rate rises: u 5.41 / policy 1.75 at 0.08, 5.07 / 2.47
    /// at 0.04, 4.97 / 2.70 at 0.03 (headline band). Set to 0.03 for the policy band; firm pay premia (integration)
    /// should restore wage dispersion without this lever. 0 = none.
    pub match_wage_dispersion: f64,
    /// Search effort of an unemployed household is divided by this constant (not the monthly mean), so UI and
    /// duration effects change aggregate contact (a fixed normaliser: the mean is not renormalised each month).
    pub search_effort_norm: f64,
    /// Multiplier on `on_the_job_search` (employed households' monthly search probability: 0.04 × 16 = 0.64). Raised
    /// from 12 so on-the-job moves (JOLTS quits proxy) reach 2.2–2.5%/month once the premium is 1.04.
    pub on_the_job_search_mult: f64,
    /// Offers to employed workers must beat their current wage by this factor (job-to-job premium). 1.04 gives EE
    /// ≈2.4%/month. Lower values raise EE and hires but also unemployment, and the policy rate falls with it (the
    /// policy band binds): 1.03 gives EE ≈3.0 and u ≈5.2 (reviewer run); 1.05 gives EE ≈1.9 with the other settings here.
    pub job_to_job_premium: f64,
    /// Monthly probability that an unemployed household leaves the labour force: base + `unemp_exit_duration`
    /// × min(months unemployed, 12)/12 + `unemp_exit_exhaust` once benefits are exhausted, all scaled by
    /// (f_ref / f_t)^`unemp_exit_cyc`. The cycle exponent is negative: U->N falls when job finding falls, as in CPS
    /// gross flows (Elsby, Hobijn & Sahin 2015; Krusell et al. 2017), so exits do not damp unemployment's cycle.
    /// A positive exponent (discouraged workers) made U->N countercyclical and halved the unemployment response.
    pub unemp_exit_base: f64,
    pub unemp_exit_duration: f64,
    pub unemp_exit_exhaust: f64,
    pub unemp_exit_cyc: f64,

    // p1-fx: dollar risk premium shape and the distributed-lag trade elasticities (plan item 13). All inert at the
    // defaults (the premium is off, and the elasticities equal the p0 values with no lag), so the default run is
    // bit-identical to integrate. The on-state is the committed overlay scripts/profiles/p1-fx.json.
    /// Weight w of this month's risk premium ψ_t in the premium-driven part of ln fx, q_t = (1 − w)ψ_t + wψ_{t−1}
    /// (`fx::psi_step`). w = 0 is a pure level premium. w > 0 gives the monthly AR(1) of dollar changes its
    /// time-averaging value: a premium that moves over two months enters the level once in each month.
    pub fx_psi_weight: f64,
    /// Short-run (impact) price elasticity of export demand: RoW demand ∝ (P_world / (P_s × fx))^ε. Hooper, Johnson &
    /// Marquez (2000) put US export price elasticities well below one in the first year.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub export_elasticity_sr: f64,
    /// Long-run export price elasticity. `None` means the same as `export_elasticity_sr`, so the lag drops out and an
    /// override of the short-run value takes effect (the p0 path). `Some(v)` reaches v through the lagged price state
    /// (`export_lag_speed`); at speed 1 the lag is instant and v applies at once. Hooper et al. (2000): about one after
    /// 2–3 years.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub export_elasticity_lr: Option<f64>,
    /// Monthly speed at which the lagged export price state moves toward the current price, in (0, 1]. 1 = no lag.
    /// 0.08 closes about 8% of the gap a month, a mean lag of about a year (the J-curve; Magee 1973).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub export_lag_speed: f64,
    /// Long-run elasticity of import spending shares to the terms ratio. `None` means the same as
    /// `import_price_elasticity` (the short-run value), so the lag drops out and an override of
    /// `import_price_elasticity` takes effect (the p0 path). `Some(v)` reaches v through the lagged terms ratio
    /// (`import_lag_speed`); at speed 1 the lag is instant and v applies at once. Hooper et al. (2000) and Chinn (2005):
    /// about 0.2 short run and about 0.6 long run.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub import_elasticity_lr: Option<f64>,
    /// Monthly speed at which the lagged terms ratio moves toward the current one, in (0, 1]. 1 = no lag.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub import_lag_speed: f64,
    // --- p1-distquick: distribution quick fixes (plan item 14; docs/CALIBRATION.md "Item 14"). The switches are on in
    // the defaults since the re-fit. The pre-p1 base model (byte-identical to the pre-p1 binary) is reproduced by
    // scripts/profiles/legacy_p1off.json; scripts/profiles/p1-distquick.json is the historical overlay.
    /// Log-sd of skill for new entrants (demography `fill_entrant`). Base 0.35; the initial population uses 0.45.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub distquick_entrant_skill_sigma: f64,
    /// Initial top-decile share of household net worth that `init::allocate_fund` targets. Base 0.67; profile 0.72
    /// (the SCF concept is about 0.73; 0.73 moved the median net worth in some burn-in seeds, see docs/CALIBRATION.md).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub distquick_top10_nw_target: f64,
    /// Wage-index-scaled consumption kinks (`consumption.rs`). false = the base $1M and $20k kinks, in nominal dollars.
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub distquick_kink_wage_indexed: bool,
    /// Wealth-assortative heir choice with an education penalty (`demography::pick_heir_matched`). false = random
    /// heir (base).
    /// default since the re-fit; legacy value in scripts/profiles/legacy_p1off.json
    pub distquick_heir_match: bool,
    /// Heirs per estate. 1 = the whole estate to one heir (base); 2 or 3 = deposits, Fund shares and houses split
    /// among that many heirs (`demography::process_death`).
    pub distquick_estate_heirs: usize,

    // fix-housing
    /// Rent-to-star elasticity θ to the user cost (rent-to-price ∝ (user cost / user cost at the anchor)^θ), shared by
    /// the rent arbitrage and the price term (one reference). 0 restores the fixed rent-to-price ratio. Default 0.75
    /// (fix-housing, review 5): at 0.5 a +100bp term-premium shock lifted house prices only about 4% at the trough (goal
    /// −5 to −10%); at 1.0 the rent arbitrage lifted core inflation after tightening (price puzzle).
    pub housing_uc_theta: f64,
    /// Refinancing: gap (pp) between the old rate and the current mortgage rate that must be exceeded.
    pub refi_threshold_pp: f64,
    /// Refinancing: monthly probability per pp of gap above the threshold (0 disables refinancing).
    pub refi_prob_per_pp: f64,
    /// Refinancing: cap on the monthly probability.
    pub refi_prob_cap: f64,
    /// Construction starts: elasticity of starts to the financing cost, per pp of 6-month mortgage rate (0 disables).
    pub construction_rate_elasticity: f64,
    /// After the anchor: monthly probability that a delinquent (3+ months) borrower in negative equity (net of selling
    /// costs and arrears) is foreclosed. The double trigger: employment does not enter.
    pub housing_foreclosure_prob: f64,
    /// After the anchor: monthly probability that a delinquent (3+ months) borrower with equity lists the house for a
    /// distressed sale. A borrower's own home is vacated at listing (an owner exit); a let unit is only listed, and its
    /// owner keeps the home they live in.
    pub housing_distress_sale_prob: f64,
    /// A repossession's defaulted loan (`HousingState::reo_loan`, held from foreclosure until the bank sells the house) is
    /// a non-accruing bank asset: the evicted borrower, or an entrant who later takes that household slot, is not charged
    /// interest or principal on it, has no shortfall, miss or Fund-share sale for it, and it does not count as that
    /// household's loan in servicing. It also leaves the household-mortgage measures (mortgage debt, average outstanding
    /// rate, delinquency counts and shares, the FISIM loan leg and the mortgage rate by age), since the borrower no
    /// longer owns the house; it stays in the bank's loan book and capital until the sale writes it off net of proceeds.
    /// Legacy `false` (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub housing_reo_nonaccrual: bool,
    /// A repossession whose listing reaches the end of its life stays on the market (`housing::update_listings`): it is
    /// re-listed at the lower of its decayed ask and `REO_ASK_FACTOR` × current value, instead of being let as a
    /// bank-owned rental that nothing ever offers for sale again. The bank keeps cutting the price until a buyer takes it,
    /// so every repossession is resolved through the sale path that writes the loan off net of proceeds. With
    /// `housing_reo_nonaccrual` off the evicted borrower can repay the loan before the sale, which then writes nothing off:
    /// with either key the sale checks that the loan id is still a mortgage on the house. Legacy `false`
    /// (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub housing_reo_relist: bool,
    /// Owner-to-renter exit hazard elasticity to the user cost relative to its anchor: exits scale by (uc / uc0)^e.
    /// Higher mortgage rates raise exits, so ownership falls and rental demand rises when rates rise.
    pub housing_exit_uc_elasticity: f64,
    /// Annual price gain on the deviation of the log rent-to-star gap from its anchor level. Prices absorb the
    /// rate-driven user-cost change (Gallin 2008); rents follow through the same gap (arbitrage). Default 0.9.
    pub housing_price_uc_gain: f64,
    /// Annual rent arbitrage speed on the same gap (rents converge to the rent-to-star level). Default 0.08.
    pub housing_rent_price_anchor: f64,
    /// Buyer-affordability sensitivity: annual house-price response per log-point of the bid ratio relative to its
    /// anchor (the main channel from mortgage rates to prices). Lowered from 0.3 to avoid price dynamics dominated by
    /// the pull; 0.2 (fix-housing, review 5) puts the +100bp term-premium trough at about −5%.
    pub housing_affordability_sens: f64,
    /// After the anchor: monthly probability that an unemployed owner on a high-LTV mortgage (LTV above `housing_shock_ltv`)
    /// misses a payment, at most this value. With the shipped grading off (see `housing_shock_liquidity_months`) every
    /// such owner misses at this probability, whatever their liquidity; it is a reduced-form, LTV-gated payment shock
    /// (the Ganong & Noel 2023 illiquidity channel is represented only if grading is switched on). 0 disables it.
    pub housing_income_miss_prob: f64,
    /// Liquid assets (deposits only; Fund shares are not counted), in months of mortgage payments, at or above which the
    /// income-shock miss is zero. The probability falls linearly from `housing_income_miss_prob` at
    /// `housing_shock_full_miss_months`. Grading is available but off by default (1e6): it cannot bind for this population
    /// at realistic thresholds, so the shipped channel is an LTV-gated payment shock without a liquidity gradient.
    pub housing_shock_liquidity_months: f64,
    /// Liquid assets, in months of mortgage payments, at or below which the income-shock miss takes its full probability.
    pub housing_shock_full_miss_months: f64,
    /// Idiosyncratic house values: monthly persistence of each house's log deviation from the index (AR(1); Case & Shiller
    /// 1989, Landvoigt, Piazzesi & Schneider 2015). The deviations are cross-sectionally mean zero, so the index holds.
    pub housing_idio_persistence: f64,
    /// Idiosyncratic house values: monthly innovation sd of the log deviation. The stationary sd is this divided by
    /// sqrt(1 - persistence^2). 0 removes the deviations (every house is worth its index value).
    pub housing_idio_innovation_sd: f64,
    /// After the anchor: monthly probability that an owner-occupier with a mortgage above `housing_shock_ltv` starts an
    /// income-shock episode (job loss, a pay cut or a family event; the Ganong & Noel 2023 illiquidity channel). It is
    /// drawn per house on its own stream. 0 disables episodes.
    pub housing_shock_hazard: f64,
    /// Months an income-shock episode lasts (the owner's payments are missed with the episode miss probability, which is
    /// not liquidity-graded while `housing_shock_liquidity_months` is at its default).
    pub housing_shock_episode_months: u32,
    /// Loan-to-value above which an owner in an income-shock episode (or unemployed) can miss a payment.
    pub housing_shock_ltv: f64,
    /// Monthly probability that an owner in an income-shock episode (and employed) misses the payment, for owners with LTV
    /// above `housing_shock_ltv`. Liquidity grading (`housing_shock_liquidity_months`) applies to it only when grading is
    /// switched on; with the shipped default every such owner misses at this probability. The unemployed keep
    /// `housing_income_miss_prob`.
    pub housing_shock_episode_miss_prob: f64,
    /// Multiplier on the episode hazard for an owner whose loan exceeds the house value (negative equity).
    pub housing_shock_underwater_mult: f64,
    /// fix-housing: elasticity of the episode hazard to labour-market stress: the monthly hazard scales by
    /// (u / u_natural)^e, so episodes (job loss, pay cuts) cluster in recessions (review 10). 0 gives a flat hazard.
    pub housing_shock_u_elasticity: f64,
    /// Re-fit Step 4a: the bank corporate-tax base is the bank P&L after operating expenses (`Bank.profit`, which
    /// `banking::service_bank_pnl` computes). Legacy false taxes interest income less interest expense and loan losses,
    /// before operating expenses. Literature: FDIC Quarterly Banking Profile (efficiency ratio 55–60%).
    pub bank_tax_after_opex: bool,
    /// Re-fit Step 4b: a founder of a reborn firm is hired at the market wage like any other hire
    /// (`common::hire_worker` at the wage offer times human capital), so the founder does not job-hop out of the firm.
    /// Legacy false attaches the founder at no wage. Literature: BEA NIPA proprietors' income includes owners' labour
    /// income; Hurst & Pugsley (2011).
    pub founder_wage_hc: bool,
    /// Re-fit Step 4c: share of domestic crude value (`crude_intensity × (1 − oil_import_share) × oil_ratio × output`)
    /// paid as production taxes and public royalties to the Treasury, ad valorem (state severance and ad valorem taxes
    /// ≈ 4–6% of wellhead value; federal, state and tribal royalties ≈ 3–4%, ONRR and EIA). Legacy 0; see
    /// `upstream_split`.
    pub upstream_tax_share: f64,
    /// Re-fit Step 4c: share of domestic crude value paid as private mineral royalties to the Fund (household property
    /// income). Legacy 0; see `upstream_split`.
    pub upstream_royalty_share: f64,
    /// Re-fit Step 4c: upstream margin above Energy's normal markup, kept in Energy profit (it absorbs the windfall when
    /// oil is above base). Legacy 1.0, which switches the upstream split off: the domestic crude is then the old
    /// unpaid rent (see `upstream_split`). Default since joint2 (docs/CALIBRATION.md, Joint calibration v2): 0.05, the
    /// S6 level calibration (energy_profit_gdp 1.13 to 1.07 and cu_level 82.6 to 80.7 on the 16-seed card). Legacy
    /// 0.10 in scripts/profiles/legacy_j2off.json.
    pub upstream_margin: f64,
    /// Accounting fix (recorded real series): real consumption and real government purchases are deflated component by
    /// component, each sector's nominal purchases over that sector's price relative to the base month and housing services
    /// over the rent index, so the implied deflators (nominal over real) are Paasche indices: the expenditure-weighted
    /// harmonic means of the relatives (`metrics::paasche_volume`). The headline then agrees with its own breakdown
    /// (`household_purchases_real_bn_ar`, `consumption_by_sector_real`). The same applies to in-kind health care (split by
    /// `health_sector_mix`), to the household deflator behind `consumption_by_quintile`, and, through the PCE deflator, to
    /// the constant-margin real FISIM. Legacy: the aggregates are divided by the arithmetic mean of the relatives weighted by
    /// current nominal spending, which overstates the deflator whenever relative prices diverge (4% of spending on energy,
    /// energy prices +50%, quantities unchanged: real consumption -0.9%) and understates real consumption, real government
    /// purchases and real GDP. Nominal series and the economy are unchanged, except that with the world block's trade link and
    /// US import channel on `world/trade.rs` reads last month's `gdp_real_bn_ar`. The investment (fixed capital-goods
    /// basket), residential (one sector), export (by sector) and import (fixed tradable weights) deflators are not this
    /// form and are unchanged. Legacy `false` (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub paasche_deflators: bool,
    /// Stock-flow consistency is enforced in every build: `sim::step` panics, naming the tick and the failing identity, when
    /// the month's `sfc_error` is set (the checks of `ledger::check_consistency` that `metrics::record` computes in its passes:
    /// bank deposit liabilities against customer deposits, loan books against loans, finite balances, Fund shares against
    /// household shares, mortgage links) or when the world block's identities fail (`world::check_identities`: the NIIP
    /// residual and the bilateral current accounts). Debug builds also panic on the full `ledger::check_consistency`, as
    /// before. The checks are computed anyway, so the cost is nil. Legacy: a release build (server, CLI, `cargo test
    /// --release`) only records `sfc_error` and goes on with an inconsistent economy. Legacy `false`
    /// (scripts/profiles/legacy_acctfix_off.json) reproduces the release.
    pub sfc_enforce: bool,
    /// Re-fit Step 5a: the CIPI fold adds back the inventory written off in bankruptcies (`Flows.bk_inventory_units`),
    /// since a write-off is an "other change in volume", not a fall in inventory investment. Legacy false.
    pub cipi_excludes_writeoffs: bool,
    // b7-spikes F1: unsold perishable output is not inventory investment (other change in volume); the CIPI fold skips
    // the nonstorable sectors. Default since joint3 (docs/CALIBRATION.md, Joint calibration 3): true. Accounting fix; the
    // per-seed policy counts on seeds 3, 21, 28 are the known miss (PASS-F1). Legacy false in scripts/profiles/legacy_b7off.json.
    pub cipi_excludes_nonstorable: bool,
    /// b7-spikes F2: pooled firm-order routing (no random supplier draws; each supplier settled once on its share of the
    /// sector's pooled order). Default false; scripts/profiles/b7-pool.json sets it true.
    ///
    /// Open item (joint3): pooled routing is not wired to b7-tariff's covered-order duty bookkeeping (`Draw.cov`,
    /// `st.unmet_cov`). With an active tariff, `firm_order_pool` = true under-collects the duty on covered unmet orders.
    pub firm_order_pool: bool,
    /// Re-fit Step 5b: monthly default hazard for a firm whose Merton net worth is below zero (drawn on the FIRM_DEFAULT
    /// stream). 1/6 gives the same mean distress spell as the deterministic `DISTRESS_MONTHS` = 6 counter. Legacy 0 keeps
    /// the counter (`firms::close_books`).
    pub distress_default_hazard: f64,
    /// Re-fit Step 6: native entry tracks the dissolutions of single households. Each month's native due is the base
    /// rate times `native_formation_index`, plus (current expected dissolutions − those at the calibration month), so
    /// entry follows the ageing of the population. Legacy false keeps the base rate fixed at its calibration.
    pub native_entry_tracks_dissolutions: bool,
    /// Re-fit Step 6: natural household formation net of deaths, per year, as a share of households (documented
    /// constant, moved from `HH_FORMATION_ANNUAL`; legacy 0.0055). JCHS projects ≈0.65%/yr household growth over
    /// 2025–35, of which ≈0.35pp is immigration (`immigration_annual`) and the rest is native formation net of the
    /// dissolutions that native entry replaces. The value is the native-formation excess over those dissolutions, set at
    /// the first post-burn-in month (demography.rs). JCHS household projections (McCue 2018/2023/2025); CBO Demographic
    /// Outlook 2025/26.
    pub hh_formation_annual: f64,
    /// Re-fit Step 6: trend labour-force growth per year in the nominal trend (`nominal_trend_growth`, and so trend GDP
    /// and the central bank's balance-sheet target). Default: the measured labour-force growth of the Step 6 model
    /// (`TREND_LABOR_FORCE_GROWTH`). Legacy 0.004 (CBO) in scripts/profiles/legacy_p1off.json.
    pub trend_labor_force_growth: f64,
    // b4-rent (housing rental side; Dias & Duarte 2019/2022, Greenwald & Guren 2025). All off by default: the
    // off-state reproduces the re-fit master bit for bit. Profiles: scripts/profiles/b4-rent*.json.
    /// b4-rent: β_b, the first-time-buyer bid-hazard sensitivity per percentage point of the 6-month mortgage-rate
    /// average above its slow reference (`buy_rate_mult`: multiplier exp(−β_b·d), clamped to [0.5, 1.5]). Legacy 0
    /// (no tenure response). Dias & Duarte (2022, Table 3): renter→owner −14.6% per 25bp, which this term targets.
    pub housing_buy_rate_semi: f64,
    /// b4-rent: switches the re-anchored vacant-rental sale hazard on (`vacant_sell_prob`). Legacy false (the master
    /// hazard `max(0.2·clamp(4/ms−1,0,1), 0.03)`, which is inactive at ms ≈ 5.4 and steep at the re-fit's ms ≈ 3).
    /// Greenwald & Guren (2025) for the segmented rental market; Dias & Duarte (2019) for the rental-vacancy sign.
    pub housing_vacant_sell_reanchor: bool,
    /// b4-rent: η, the elasticity of the re-anchored vacant-rental sale hazard to months' supply (inert while
    /// `housing_vacant_sell_reanchor` is false). Legacy hazard elasticity near the re-fit state is about −4; η ≥ 1 keeps
    /// the unit-level sign (Dias & Duarte 2022, Table 3: ownership→rental +3.4% per 25bp).
    pub housing_vacant_sell_ms_elast: f64,
    /// b4-rent: κ_s, sensitivity of the vacant-rental sale hazard to last month's HVS rental vacancy above its 120-month
    /// reference, per percentage point (landlords sell more out of a loose rental market: a stabilising exit). Legacy 0.
    /// Rosen & Smith (1983) on natural vacancy; Greenwald & Guren (2025).
    pub housing_rental_gap_sell_semi: f64,
    /// b4-rent: κ_e, sensitivity of the probability that an expiring vacant for-sale listing is let (rather than re-listed
    /// for sale) to the HVS rental vacancy gap, per percentage point (`expiry_let_prob`; one-sided: 1 when v ≤ v̄).
    /// Legacy 0 (every expiry goes to rent, master). Mills, Molloy & Zarutskie (2019) on owner↔rental conversions.
    pub housing_expiry_let_gap_semi: f64,


    // b5-udur: unemployment duration mechanisms (docs/CALIBRATION.md, b5-udur). Every field is legacy-neutral at
    // its default (off), so the default run is bit-identical to master.
    /// M2a: contact profile c(m) = d1^min(m, m1) × d2^min(max(m − m1, 0), max(m2 − m1, 0)) replaces the legacy
    /// `duration_contact_decay`/`_cap` when on, and contact is divided by `udur_effort_norm`. With the legacy values
    /// (d1 = decay, m1 = cap, d2 = 1) and norm = `search_effort_norm`, on is bit-identical to off.
    pub udur_contact_on: bool,
    pub udur_contact_d1: f64,
    pub udur_contact_m1: f64,
    pub udur_contact_d2: f64,
    pub udur_contact_m2: f64,
    pub udur_effort_norm: f64,
    /// M1: employers rank unemployed applicants by hc × (1 + bonus) × exp(−κ·min(m, M)/12) (Blanchard & Diamond 1994),
    /// so long-duration applicants are hired last when applicants exceed openings. κ = 0 is off (legacy ranking).
    pub udur_rank_kappa: f64,
    pub udur_rank_cap: f64,
    /// M5: extended UI. u3 (mean of the last ≤3 monthly unemployment rates) ≥ t2 gives m2 extra benefit months, else
    /// ≥ t1 gives m1 (EB/EUC-style triggers; 99 = off). `udur_ui_ext_search_scale` scales the UI search cut for
    /// recipients past `benefit_months`.
    pub udur_ui_ext_t1: f64,
    pub udur_ui_ext_t2: f64,
    pub udur_ui_ext_m1: f64,
    pub udur_ui_ext_m2: f64,
    pub udur_ui_ext_search_scale: f64,
    /// M3: human-capital scarring. Each unemployed month multiplies the scar by (1 − δ), floored at 1 − L; employed
    /// months recover it by ρ(1 − scar). Human capital (ranking, offers, stayer resets) is multiplied by the scar.
    /// Production is unchanged. δ = 0 is off.
    pub udur_hc_loss: f64,
    pub udur_hc_loss_max: f64,
    pub udur_hc_recover: f64,
    /// M4a: U→N exit hazard gains udur_exit_base_shift + udur_exit_dur × min(m, M)/M (Kroft et al. 2016 duration
    /// dependence in exits). Zero is off; the shift re-centres the level (−0.29 × d_u).
    pub udur_exit_base_shift: f64,
    pub udur_exit_dur: f64,
    pub udur_exit_dur_months: f64,
    /// M6: monthly decay of the reservation wage (replaces the constant 0.97 in `reservation_wage` and the
    /// non-participant offer). 0.97 is the legacy value.
    pub udur_resv_decay: f64,
    // b6-reentry: participation re-entry with spell memory (Kudlyak & Lange 2017; Elsby, Hobijn & Şahin 2015).
    /// M4b: monthly re-entry hazard for non-participants who left unemployment less than `udur_reentry_window`
    /// months ago (prev spell > 0). 0 is off (the legacy hazard OLF_OUTFLOW applies).
    pub udur_reentry_recent: f64,
    /// M4b: months after a U→N exit during which `udur_reentry_recent` applies.
    pub udur_reentry_window: u16,
    /// M4b: a re-entrant resumes its previous unemployment spell length (months_unemployed = prev).
    pub udur_resume_spell: bool,
    /// M4c: scarring. Non-participants whose previous spell reached `udur_reentry_scar_spell` months and who have
    /// been out for more than the window re-enter at OLF_OUTFLOW × `udur_reentry_scar`. 1 is off.
    pub udur_reentry_scar: f64,
    pub udur_reentry_scar_spell: u16,
    /// b6-reentry M4d: attachment of long-spell leavers (Krueger, Cramer & Cho 2014; Kudlyak & Lange 2018). Off by
    /// default. When on, a non-participant whose previous unemployment spell reached `udur_att_spell` months has
    /// attachment a = [scar + (1 − scar)(1 − e^{−n/τ})] × θ-factor, where n is months out and θ-factor =
    /// clamp((θ_{t−1}/θ̄)^η, 0.5, 2). The N→U hazard is OLF_OUTFLOW × udur_att_base_mult × a, and the N→E contact of
    /// that household is weighted by a (Hornstein, Kudlyak & Lange 2014 non-employment index).
    pub udur_att_on: bool,
    /// M4d: previous spell (months) from which attachment applies.
    pub udur_att_spell: u16,
    /// M4d: attachment right after exit (1 = no scarring).
    pub udur_att_scar: f64,
    /// M4d: months out for attachment to recover halfway-plus (1 − e^{−n/τ}).
    pub udur_att_tau: f64,
    /// M4d: elasticity of attachment to tightness (0 = none).
    pub udur_att_eta: f64,
    /// M4d: multiplier on the base N→U hazard (re-centring; 1 = legacy level).
    pub udur_att_base_mult: f64,
    // --- b7-tariff ---
    /// b7-tariff: tariff pass-through (border duty on customs value, lagged retail pass-through, importer absorption,
    /// tariff-elastic import shares, CPI import component, BEA duty treatment). `false` = master semantics (the
    /// ImportTariff shock is a lump-sum payment from households to Government). Source: docs/CALIBRATION.md §b7-tariff.
    pub tariff_pass_through: bool,
    /// b7-tariff: λ_R, monthly speed of retail pass-through of the duty (r_t = r_{t-1} + λ_R (φ_R τ_t − r_{t-1})).
    /// Source: Cavallo, Llamas and Vazquez (2025): partial, gradual retail pass-through; 0.075 gives 61% at m12.
    pub tariff_retail_speed: f64,
    /// b7-tariff: φ_R, long-run retail pass-through of the statutory duty. Source: Amiti, Redding and Weinstein (2019),
    /// Fajgelbaum et al. (2020): full border pass-through (1.0).
    pub tariff_retail_long: f64,
    /// b7-tariff: ε_S, short-run elasticity of household and firm import shares to the tariff-inclusive price. Source:
    /// Boehm, Levchenko and Pandalai-Nayar (2023): customs-volume elasticity about 0.76 short run.
    pub tariff_share_elasticity_sr: f64,
    /// b7-tariff: ε_L, long-run elasticity of import shares to the tariff. Source: Boehm, Levchenko and Pandalai-Nayar
    /// (2023): about 2 long run; Fontagné, Martin and Orefice (2018): tariff elasticities exceed exchange-rate ones.
    pub tariff_share_elasticity_lr: f64,
    /// b7-tariff: λ_S, monthly speed of the lagged tariff state that drives the long-run share response.
    pub tariff_share_lag_speed: f64,

    // b7-transmission
    /// b7-transmission M-Q: λ, the QE scarcity/signalling weight. Each purchased lot's remaining duration counts λ times
    /// in the priced duration ratio (the announced-but-unexecuted share is scaled the same way), so a programme of the
    /// same size prices a deeper term premium while issuance pricing is unchanged. Units: multiple of the portfolio-
    /// balance duration. Default since joint3 (docs/CALIBRATION.md, Joint calibration 3): 1.5 (partial adoption, T-R3: the
    /// QE duration and dollar targets TC and TD are met; neutral on runs without a QE or PolicyShift shock). Legacy 1.0 in
    /// scripts/profiles/legacy_b7txoff.json. Literature: Bauer & Rudebusch 2014 IJCB; Krishnamurthy & Vissing-Jorgensen
    /// 2011 BPEA; D'Amico & King 2013 JFE. Target follows Gagnon et al. 2011 and Bernanke 2020 AER.
    pub qe_duration_weight: f64,
    /// b7-transmission M-X: κ_mp, the monetary-surprise dollar premium: z_t = κ_mp·L·tanh(gap_t / L) (log points), added to
    /// the dollar's risk-premium state, so a tightening surprise appreciates the dollar on impact beyond UIP and decays
    /// with the gap. Units: log points per decimal policy gap. Default since joint3 (docs/CALIBRATION.md, Joint calibration 3):
    /// 1.25. It acts on PolicyShift only (review F5): rule-driven tightening and Taylor-rule shocks get no premium. Legacy 0.0
    /// in scripts/profiles/legacy_b7txoff.json. Literature: Faust & Rogers 2003 JME;
    /// Eichenbaum & Evans 1995 QJE; Engel 2016 AER; Rogers, Scotti & Wright 2018.
    pub fx_mp_premium: f64,
    /// b7-transmission M-X: L, the saturation scale of the monetary-surprise premium (decimal policy gap). Units:
    /// decimal. Used only when `fx_mp_premium` is non-zero; the premium is bounded by κ_mp·L.
    pub fx_mp_cap: f64,
    /// b7-transmission M-Es: χ_mp, the sensitivity of firms' price-setting inflation expectations to the policy gap:
    /// π^e_f −= χ_mp·L_e·tanh(gap_t / L_e). A tightening lowers the reset-price and wage-norm expectations. Units:
    /// dimensionless χ (the term is decimal inflation). 0 = off (legacy). Literature: Enders, Hünnekes & Müller 2019 JME;
    /// Coibion, Gorodnichenko & Kumar 2018 AER; Christiano, Eichenbaum & Evans 2005 JPE; Miranda-Agrippino & Ricco 2021.
    pub firm_mp_expect: f64,
    /// b7-transmission M-Es: L_e, saturation scale of the firms' policy-expectation term (decimal inflation). Units:
    /// decimal. Used only when `firm_mp_expect` is non-zero.
    pub firm_mp_expect_cap: f64,
    /// b7-transmission M-E: χ_s, the sensitivity of firms' expectations to the policy stance S_t (the 2y real rate less
    /// its EWMA reference): π^e_f −= χ_s·L_s·tanh(S_t / L_s). Units: dimensionless χ. 0 = off (legacy). Literature as
    /// `firm_mp_expect`.
    pub firm_stance_weight: f64,
    /// b7-transmission M-E: L_s, saturation scale of the stance term (decimal). Units: decimal. Used only when
    /// `firm_stance_weight` is non-zero.
    pub firm_stance_cap: f64,
    /// b7-transmission M-E: months over which the stance reference adapts (EWMA gain 1/months). Units: months. Used
    /// only when `firm_stance_weight` is non-zero.
    pub firm_stance_months: f64,
    /// b7-transmission M-R: k, the knee of the bounded discretionary rate response (decimal rate gap NEUTRAL − real rate).
    /// Below k the gap saturates at k − L_eff (`consumption::saturating_rate_gap`). Units: decimal. Used only when
    /// `disc_rate_sat_scale` is non-zero. Literature: Tenreyro & Thwaites 2016 AEJ:Macro; Berger & Vavra 2015 Econometrica;
    /// Ascari & Haber 2022 EJ.
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): 0.005 (was
    /// 0.0). R3 slump bias: at a neutral real rate the bounded response adds h(0) = +0.12 pp at zero slack and +0.26 pp
    /// at 1 pp of slack, that is +0.54% and +1.2% on the discretionary multiplier (theta_r 4.5), and it also amplifies
    /// easing just below the knee (review F2). Legacy 0.0 in scripts/profiles/legacy_b7pinoff.json.
    pub disc_rate_knee: f64,
    /// b7-transmission M-R: L, the saturation scale of the discretionary rate response (decimal). Units: decimal.
    /// 0 = off (legacy: the rate term is linear in the real rate).
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): 0.005 (was
    /// 0.0). Part of the PbR3F0 package with `disc_rate_knee` and `disc_rate_sat_slack`. Legacy 0.0 in
    /// scripts/profiles/legacy_b7pinoff.json.
    pub disc_rate_sat_scale: f64,
    /// b7-transmission M-R: γ, how much unemployment slack (u − u*, in pp) narrows the saturation scale:
    /// L_eff = L/(1 + γ·slack/0.01). Units: per pp of u − u*. Used only when `disc_rate_sat_scale` is non-zero.
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): 1.0 (was
    /// 0.0). Part of the PbR3F0 package with `disc_rate_knee` and `disc_rate_sat_scale`. Legacy 0.0 in
    /// scripts/profiles/legacy_b7pinoff.json.
    pub disc_rate_sat_slack: f64,
    /// b7-transmission M-F: k_f, the knee of the bounded UIP response to large rate differentials d = r_us − r_world
    /// (decimal). Above k_f the differential saturates at k_f + L_f·tanh((d − k_f)/L_f). Units: decimal. Used only when
    /// `fx_rate_sat_scale` is non-zero. Literature: Volcker-episode dollar (+18% in 12 months) against the pin-10 case.
    pub fx_rate_knee: f64,
    /// b7-transmission M-F: L_f, the saturation scale of the bounded UIP response (decimal). Units: decimal. 0 = off
    /// (legacy: the dollar moves linearly in the rate differential).
    pub fx_rate_sat_scale: f64,
    /// b7-transmission M-P: rule-consistent expected path under a pinned policy rate (`pinned_expected`). true prices the
    /// pin as the rule releases it (decay `pin_exit_decay`, exit hazard `pin_exit_hazard`); false keeps the legacy
    /// flat-average path. Units: bool. Literature: Campbell, Evans, Fisher & Justiniano 2012 BPEA; Del Negro, Giannoni &
    /// Patterson, "The forward guidance puzzle".
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): true (was
    /// false). The pinned-rate path is priced as the rule releases it; neutral without a pinned rate. Legacy false in
    /// scripts/profiles/legacy_b7pinoff.json.
    pub pin_path_on: bool,
    /// b7-transmission M-P: h, the monthly hazard that an open-ended pin exits, so the priced expectation averages over
    /// a finite pin. Units: per month. 0 = the pin lasts the whole priced horizon. Used only when `pin_path_on`.
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): 0.0417 (was
    /// 0.0). Neutral without a pinned rate. Legacy 0.0 in scripts/profiles/legacy_b7pinoff.json.
    pub pin_exit_hazard: f64,
    /// b7-transmission M-P: ρ, the monthly release speed of the pinned rate after exit (the rule's smoothing, 0.9 in
    /// the model). Units: per month. With h = 0 and ρ = 0.97 the result equals the legacy `expected_average`. Used only
    /// when `pin_path_on`.
    /// Default since joint calibration 4 (docs/CALIBRATION.md, Joint calibration 4, PbR3F0, RULINGS R-14): 0.9 (was
    /// 0.97). Neutral without a pinned rate. Legacy 0.97 in scripts/profiles/legacy_b7pinoff.json.
    pub pin_exit_decay: f64,
    // b8-world (stage 1): the world block. Off by default; with it off the output is byte-identical to M3 (spec 1.5).
    /// b8-world master switch: builds the twelve bloc models and steps them in shadow mode (spec 0.1). Units: bool.
    pub world_block_enabled: bool,
    /// b8-world D7: OPEC+ composition for the GDP weights, true = 21 members with the UAE (blocs global.composition_notes).
    pub world_uae_in_opec: bool,
    /// b8-world: multiplies every bloc shock sd (blocs.<b>.shocks_monthly_sd.*); zero draws are still taken. Units: x.
    pub world_shock_scale: f64,
    /// b8-world: runtime multiplier on the P* theta of every bloc (BlocDemand and the financial channel). Units: x.
    pub world_theta_mult: f64,
    /// b8-world: runtime multiplier on the P* lambda_rp (UIP risk premium on G). Units: x.
    pub world_lam_rp_mult: f64,
    /// b8-world: runtime multiplier on the P* k_us (pass-through of the US policy deviation). Units: x.
    pub world_kus_mult: f64,
    /// b8-world: runtime multiplier on the oil IS coefficients. Units: x.
    pub world_oil_is_mult: f64,
    /// b8-world D17: projection persistence of the foreign-activity input F (realised lags, then AR(1)). Units: per month.
    pub world_rho_f: f64,
    /// b8-world D17: projection persistence of the partner real exchange rates in Peff. Units: per month.
    pub world_rho_p: f64,
    /// b8-world D17: projection persistence of the oil gap. Units: per month.
    pub world_rho_oil: f64,
    /// b8-world D17: projection persistence of the OPEC+ output deviation. Units: per month.
    pub world_rho_qty: f64,
    /// b8-world D18: Okun ratio of the US output gap, y_US = 100 okun (u* - mean u). Units: x. Source: the ABM's Okun rule.
    pub world_okun_ratio: f64,
    /// b8-world D28: fixed offset (pp) subtracted from the US policy deviation, measured once on the M3 binary by
    /// scripts/world/measure_ius_offset.py (seeds 1-12: -0.431, rounded). Units: pp.
    pub world_ius_offset: f64,
    /// b8-world N1: fixed offset (pp) subtracted from the US output gap y_US = 100 okun (u* - mean u) before it enters the
    /// blocs (`net::us_inputs`). Measured once on the M3 binary as the mean of the block's y_US over months 0-239, seeds 1-12
    /// (test `world::tests::yus_offset_measure`: -0.901, SE 0.165, rounded to -0.90; cross-check `scripts/world/measure_yus_offset.py`).
    /// Units: pp.
    pub world_yus_offset: f64,
    /// b8-world: oil value-added share of OPEC+ GDP for the production channel [assumed]. Units: share.
    pub world_opec_va_share: f64,
    // b8-world (stage 2): trade and prices. The link is off by default; with it off the block only reports (shadow).
    /// b8-world stage 2: trade link (bilateral imports and exports, origin import prices, partner-sector tariffs into b7,
    /// exports on foreign income). Off: the legacy goods and trade expressions run unchanged. Units: bool.
    pub world_link_trade: bool,
    /// b8-world D27: US-to-bloc import-demand channel in the bloc network (`world_us_import_channel`; coupled mode only).
    /// Units: bool.
    pub world_us_import_channel: bool,
    /// b8-world origin substitution, short run: sigma_SR of the origin shares (spec 2.5; `elasticities.us_imports`
    /// origin substitution, 2.53 long run, 2.0 used at the Boehm et al. profile). Units: elasticity.
    pub world_origin_sigma_sr: f64,
    /// b8-world origin substitution, long run: sigma_LR (`elasticities.us_imports.origin_substitution_elasticity` 2.53; the
    /// stage's value 2.0 hits the Boehm et al. bilateral profile). Units: elasticity.
    pub world_origin_sigma_lr: f64,
    /// b8-world origin substitution: monthly speed of the lagged origin state zw (spec 2.5). Units: per month.
    pub world_origin_speed: f64,
    /// b8-world dollar pass-through, USD-invoiced origins: long-run response kappa_usd of Phi^d
    /// (`pass_through.us_import_price_from_bloc_formula.G_usd_long_run`). Units: fraction.
    pub world_dcp_kappa_usd: f64,
    /// b8-world dollar pass-through, USD-invoiced origins: monthly speed of Phi^d (`pass_through.us_nonfuel_import_price_to_broad_dollar.monthly_speed_to_long_run`). Units: per month.
    pub world_dcp_speed_usd: f64,
    /// b8-world dollar pass-through, non-USD origins: long-run response kappa_nonusd of Phi^nd
    /// (`pass_through.us_import_price_from_bloc_formula.G_nonusd_long_run`). Units: fraction.
    pub world_dcp_kappa_nonusd: f64,
    /// b8-world dollar pass-through, non-USD origins: monthly speed of Phi^nd [assumed]: month-1 pass-through 0.86 against 0.96
    /// (`pass_through.by_invoicing_nondollar_priced_goods`). Units: per month.
    pub world_dcp_speed_nonusd: f64,
    /// b8-world exports: foreign-income elasticity eps_y (`elasticities.us_exports.foreign_income_elasticity`). Units: elasticity.
    pub world_export_income_elast: f64,
    /// b8-world exports: short-run and long-run price elasticities of the competitor-price term (calibrated to W2-02). Units: fraction.
    pub world_export_price_sr: f64,
    pub world_export_price_lr: f64,
    /// b8-world exports: short-run dollar elasticity (`pass_through.dominant_currency_volume_response`, sign reversed) and long run
    /// [assumed]. Units: fraction.
    pub world_export_fx_sr: f64,
    pub world_export_fx_lr: f64,
    /// b8-world exports: monthly speed of the lagged importer-depreciation and competitor-price states. Units: per month.
    pub world_export_speed: f64,
    // b8-world (stage 3): the oil market (spec 2.4, 1.5, 1.8). The link is off by default; with it off the block only reports.
    /// b8-world stage 3: oil link (the market sets `exog.oil_price` and the oil channels of the blocs and OPEC+). Units: bool.
    pub world_link_oil: bool,
    /// b8-world stage 3: oil profile (`oil.alternatives.<name>.changes`, D13/D4): default, supply_heavy, low_demand,
    /// high_demand, shale_boom, shale_discipline. Units: name.
    pub world_oil_profile: String,
    /// b8-world stage 3: OPEC+ regime, `price_defence` (floor and ceiling rule) or `market_share` (`supply.opec.market_share_unwind_rate`).
    pub world_opec_regime: String,
    /// b8-world stage 3: legacy OilPrice presets pin the monthly-average price (exact path, spec 2.4). Units: bool.
    pub world_oil_exact_path: bool,
    /// b8-world stage 3 (S3-R2, fix round): the market reads the blocs' output gaps and real exchange rates. True is the
    /// structural demand feedback (default); false is the exact-path interface test (the market ignores the blocs' activity).
    /// Units: bool.
    pub world_oil_feedback: bool,
    /// b8-world stage 3: subtracted from the market log price when `stochastic_oil` is true (D6, `anchors.stochastic_mean_offset`). Units: log points.
    pub world_oil_mean_offset: f64,
    /// b8-world stage 3: multiplies the oil shock scales (sig_I, sig_O, sig_y); the legacy `oil_sigma` is ignored. Units: x.
    pub world_oil_sigma_mult: f64,
    /// b8-world stage 3 (D5 lever): persistence of the consumption-demand shift q (`shocks.oil_specific.rho_q`). Units: per month.
    pub world_oil_rho_q: f64,
    /// b8-world stage 3 (D5 lever): multiplier on the oil-specific shock scale sig_I (`shocks.oil_specific.sigma`). The field and
    /// the JSON key are both `world_oil_sig_i_mult` (stage-3 review R10). Units: x. The server path also accepts the retired
    /// spelling `world_oil_sigI_mult` (stage-4 step 0, D3); the CLI overlay (`from_env`) still rejects it.
    #[serde(alias = "world_oil_sigI_mult")]
    pub world_oil_sig_i_mult: f64,
    /// b8-world stage 3 (S3-R2 fix round, W3-19): which structural oil shock sources are drawn into the market: any of `I`
    /// (oil-specific), `D` (supply disruption), `O` (OPEC+ surprise). The draws are always taken; a source not listed is zeroed.
    /// The default `IDO` is the model; the others are the W3-19 variance-share cells. Units: string of I, D, O.
    pub world_oil_shock_mask: String,
    /// b8-world stage 3 (S3-R8 fix round, option b): stochastic mean of the OPEC+ quantity input (% of OPEC+ liquids), subtracted
    /// from the blocs' OPEC+ channel when `stochastic_oil` is true (the rule of D6). Measured on the shipped profile with the oil
    /// link on, 1000 households, seeds 1-200, as the mean over months 0-239 of the market's deviation (`scripts/world/oil_gates.py
    /// qty`): -6.627 (SE 0.601). The check on seeds 201-400 gives -7.631 (SE 0.610). Units: % of OPEC+ liquids.
    pub world_oil_qty_offset: f64,
    /// b8-world stage 3 (D5): sd of the prototype activity process, used by the isolated `oil-check` market only.
    /// Units: % per month.
    pub world_oilcheck_activity_sigma: f64,
    /// b8-world stage 3 (D5): persistence of the prototype activity process (`oil-check` only). Units: per month.
    pub world_oilcheck_activity_rho: f64,
    /// b8-world stage 3 (S3-R1): persistence of the decaying splice of the ABM oil price onto the market (spec 2.4 as revised).
    /// -1 means follow the legacy `oil_ar` (the default); 1 is the permanent anchor, 0 is a month-0 jump. Units: per month.
    pub world_oil_splice_rho: f64,
    /// b8-world stage 3 (D7): share of OPEC+ spare capacity held by the UAE, removed when `world_uae_in_opec` is false.
    /// [assumed]. Units: share.
    pub world_opec_uae_spare_share: f64,
    /// b8-world stage 4-1 (spec 2.6, D30): the bilateral dollar link. When on, the broad dollar `w.row.fx` is the blocs' index
    /// (`ln D` of 2.6) and the monetary `exchange_rate` is skipped in reported months. Off: byte-identical to ccdcc48. Units: bool.
    pub world_link_dollar: bool,
    /// b8-world stage 4-1 (spec 1.5, source: VIX units of the finance params `global_financial_cycle`): VIX points per unit of
    /// the ABM's financial factor `f_t`. Units: VIX points per unit.
    pub world_vix_per_unit: f64,
    /// b8-world stage 4-1 (spec 2.7, source: finance `risk_off` 13 VIX points per 100 bp): VIX points per 100 bp of policy
    /// surprise. Units: VIX points per 100 bp.
    pub world_vix_per_100bp: f64,
    /// b8-world stage 4-1 (spec 2.6): multiplier on the risk-off loadings gamma_j in the dollar's risk term. Units: x.
    pub world_fx_risk_mult: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance `global_financial_cycle` persistence of the risk factor): AR(1) persistence of
    /// Frisk_t. Units: per month.
    pub world_fx_risk_rho: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance asymmetry of the risk factor innovation): multiplier on falls of F. Units: x.
    pub world_fx_risk_asym: f64,
    /// b8-world stage 4-1 (spec 2.6, D30 lever): multiplier on the bloc risk-premium innovation sd (`sd.risk_premium` in `draws`).
    /// Units: x.
    pub world_fx_rp_sd_mult: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance dollar idiosyncratic sd, advanced economies): innovation sd of the idiosyncratic
    /// bilateral AR(1) `zeta`, % per month. Units: % per month.
    pub world_fx_idio_sd_ae: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance dollar idiosyncratic sd, emerging economies). Units: % per month.
    pub world_fx_idio_sd_eme: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance dollar idiosyncratic sd, China). Units: % per month.
    pub world_fx_idio_sd_cn: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance dollar idiosyncratic sd, OPEC+). Units: % per month.
    pub world_fx_idio_sd_opec: f64,
    /// b8-world stage 4-1 (spec 2.6): AR(1) persistence of the bilateral idiosyncratic `zeta`. Units: per month.
    pub world_fx_idio_rho: f64,
    /// b8-world stage 4-1 (spec 2.7, D30): the risk-factor loading of the bloc's financial demand, as a fraction of theta_i. Units: x.
    pub world_theta_f_ratio: f64,
    /// b8-world stage 4-1 (spec 2.6, source: finance China risk-off points per percent of demand shortfall). Units: VIX points per %.
    pub world_china_risk_pts_per_pct: f64,
    /// b8-world stage 4-1 (spec 2.7, source: finance GDP, real-world equivalent): US nominal GDP base in $bn for the official-block
    /// units (D39). Units: $bn.
    pub world_gdp_rw_bn: f64,
    /// b8-world list B (spec 2.6, rev 9, D47): baseline neutrality of the bloc base state. When on (and `world_link_dollar` is on), the
    /// real exchange rate the link reads, in the broad index and in the trade pass-through, is the bloc's `z` less its conditional
    /// forecast from the base month (`z_fc`), a forecast-error state that is zero at the base; off: the 4-1 form (change since the
    /// base month). Units: bool.
    pub world_dollar_neutral: bool,
    /// b8-world list B (spec 2.2.2, rev 9, D48): knee of the bound on the total US-rate excess return `ex[k] + lambda_rp gp[k]` in
    /// the bloc dollar equation, decimal (the b7-transmission M-F knee, `fx_rate_knee`). Used only with `world_link_dollar` and a
    /// non-zero `world_rp_sat_scale`. Units: decimal (0.02 = 2 pp a year).
    pub world_rp_sat_knee: f64,
    /// b8-world list B (rev 9, D48): scale of that bound (the M-F scale, `fx_rate_sat_scale` of `b8-world-s4a-mf.json`); the total is
    /// bounded above by knee + scale = 3.5 pp a year. 0 = off (the 4-1 form). Units: decimal (0.015 = 1.5 pp).
    pub world_rp_sat_scale: f64,
    /// b8-world stage 4-2 (spec 2.7, D19, D43): the official-demand and term-premium link. When on (reported months), the official
    /// Treasury block executes flows with the banks, adds `tp_official` to every tenor's term premium and `UsCredibility`'s term-premium
    /// part. Off: byte-identical to the 4-1 list-A head. Units: bool.
    pub world_link_finance: bool,
    /// b8-world stage 4-2 (finance `treasury_demand.official_demand_term_premium_coefficients.theta_stock_bp_per_1pct_gdp`): bp of the
    /// 10y term premium per 1% of GDP of official holdings. Units: bp per % of GDP.
    pub world_fo_theta_stock: f64,
    /// b8-world stage 4-2 (finance `theta_transitory_bp_per_1pct_gdp`): bp of the 10y term premium per 1% of GDP of net official
    /// purchases, transitory. Units: bp per % of GDP.
    pub world_fo_theta_trans: f64,
    /// b8-world stage 4-2 (finance `transitory_half_life_months` 12): monthly persistence of the transitory term. Units: per month.
    pub world_fo_trans_rho: f64,
    /// b8-world stage 4-2 (finance `reserve_manager_response_to_dollar.fraction_of_official_holdings_per_1pct_dollar_6m`): fraction of
    /// official holdings bought per +1% of the dollar, summed over six months (applied to the monthly change of ln D, D32). Units: fraction per %.
    pub world_fo_dollar_resp: f64,
    /// b8-world stage 4-2 (finance `global_financial_cycle.foreign_official_ust_holdings_per_vix_point_usd_bn_per_month`): real-world
    /// $bn per month per VIX point of the six-month mean of F - Fbar. Units: $bn per month per VIX point.
    pub world_fo_vix_resp: f64,
    /// b8-world stage 4-2 (finance `treasury_demand.china_secular_decline`): China's trend net purchases of Treasuries, real-world $bn a
    /// year (negative = sales). Units: $bn per year. Lever {-80, 0} (S4-h).
    pub world_fo_trend_cn: f64,
    /// b8-world stage 4-3 (spec 2.5, D9, D47): tariff retaliation. When on, a `Tariff` shock flagged `retaliate` raises the partner's
    /// duty on US exports by the ratchet rule and the export factor R multiplies the bilateral exports. Needs `world_link_trade`.
    /// Off, or on with no flagged shock: byte-identical. Units: bool.
    pub world_retaliation: bool,
    /// b8-world stage 4-3 (trade `retaliation.export_response_to_retaliation`, calibrated to W4-24): value elasticity of US exports on
    /// the duty-inclusive price, impact. Units: elasticity.
    pub world_ret_eps_sr: f64,
    /// b8-world stage 4-3 (same, long run). Lever {2.0, 3.0, 4.0} (S4-k). Units: elasticity.
    pub world_ret_eps_lr: f64,
    /// b8-world stage 4-3 (assumed, W4-24): monthly speed of the lagged price state zr. Lever {0.05, 0.10, 0.20} (S4-k). Units: per month.
    pub world_ret_speed: f64,
    /// b8-world stage 4-3 (spec 2.8, D22, D40): memo cross-border positions (nine stocks by bloc), the net international investment
    /// position, the current account by bloc and the stock-flow identities. Memo only: no ABM quantity changes. Units: bool.
    pub world_positions: bool,
    /// b8-world stage 5 (S2-R7): per-bloc centring constants of the coupled US import-demand term, pp, in bloc order (empty: none). Subtracted from the clamped term.
    pub world_import_gap_offset: Vec<f64>,
    /// b8-world stage 5-T2 (D62, S5-R21 pre-flight): drift of the block's import price relative to the tradables price index PT, decimal per year
    /// (positive: imports get cheaper relative to PT, as the legacy import price does through world goods inflation and the nominal dollar).
    /// The block's import price is `imp_level x PT_ratio x exp(rbar)` (D26); this key multiplies it by `exp(-drift x months since the base / 12)`,
    /// so the baseline relative price follows the ABM's legacy path on average and a shock moves it from there. 0 = off (the stage-5 behaviour).
    pub world_import_rel_drift: f64,

    // b9-rates
    /// b9-rates R-B: w, the weight of the policy rate in the business-loan base rate: base = w x policy + (1 - w) x
    /// (0.5 y2 + 0.5 y5) + spread + credit + `business_policy_offset`. Units: weight; values above 1 act as 1. 0 = off
    /// (legacy: base = 0.5 y2 + 0.5 y5 + spread + credit). Literature: bank C&I loans reprice with the policy rate
    /// (prime and SOFR-linked) while corporate bonds follow the 2-10y curve; the target is a business-rate pass-through of
    /// about 0.7 for a 12-month +100 bp policy shift (0.41 before this key).
    pub business_policy_weight: f64,
    /// b9-rates R-B: constant added to the business base when `business_policy_weight` > 0, so that the level is neutral:
    /// w x mean(0.5 y2 + 0.5 y5 - policy) over months 12-239 of the baseline, about 0.0019-0.0038 for w = 0.3-0.6. Units:
    /// decimal. 0 = off (legacy); no effect while `business_policy_weight` is 0.
    pub business_policy_offset: f64,
    /// b9-rates M-M: k_v, the mortgage-spread response to realised 10y volatility: the spread adjustment is
    /// k_v x (EWMA of |monthly change of the 10y yield| - its slow norm). Units: decimal spread per decimal volatility.
    /// 0 = off (legacy: the spread is the constant `mortgage_spread`). Literature: Gabaix, Krishnamurthy & Vigneron (2007,
    /// J. Finance 62(2)) and Malkhozov, Mueller, Vedolin & Venter (2016, Rev. Financial Studies 29(5)): MBS prepayment and
    /// convexity premia rise with interest-rate volatility.
    pub mortgage_spread_vol_gain: f64,
    /// b9-rates M-M: k_c, the mortgage-spread response to the mean lender premium above its slow norm (the banks' excess
    /// bond premium, `bank.ebp`). Units: decimal spread per decimal premium. 0 = off (legacy). Literature: Boyarchenko,
    /// Fuster & Lucca (2019, Rev. Financial Studies): the time-series variation of MBS spreads is mostly non-prepayment
    /// (credit and supply) risk; Fuster, Goodman, Lucca, Madar, Molloy & Willen (2013, FRBNY Economic Policy Review).
    pub mortgage_spread_stress_gain: f64,
    /// b9-rates M-M: memory of the realised-volatility EWMA, in months (gain 1 / (1 + m)). Units: months. Used only when a
    /// mortgage-spread gain is non-zero.
    pub mortgage_spread_vol_months: f64,
    /// b9-rates M-M: memory of the slow norms of volatility and lender premium, in months (gain 1 / (1 + m)). Units: months.
    /// Used only when a mortgage-spread gain is non-zero.
    pub mortgage_spread_norm_months: f64,
    /// b9-rates M-M: initial realised-volatility EWMA and its norm, decimal (the model's mean absolute monthly change of the
    /// 10y yield is 0.0012). Units: decimal. Used only when a mortgage-spread gain is non-zero.
    pub mortgage_spread_vol_ref: f64,
    /// b9-rates M-M: initial norm of the mean lender premium, decimal (the model's mean is 0.0031). Units: decimal. Used
    /// only when a mortgage-spread gain is non-zero.
    pub mortgage_spread_stress_ref: f64,
    /// b9-rates M-M: lower bound of the spread adjustment, decimal (spread floor 1.10 pp with the default
    /// `mortgage_spread`). Units: decimal. Used only when a mortgage-spread gain is non-zero.
    pub mortgage_spread_min_adj: f64,
    /// b9-rates M-M: upper bound of the spread adjustment, decimal (spread ceiling 3.30 pp; the US 1985-2019 range is
    /// 1.2-3.2 pp). Units: decimal. Used only when a mortgage-spread gain is non-zero.
    pub mortgage_spread_max_adj: f64,

    // b9-oil
    /// b9-oil U: which expected-inflation series enters the investment user cost (`PlanCtx::pi_e_capital`). 0 = legacy
    /// (the household mean if `user_cost_household_expectation`, else the firm expectation); 1 = firm expectation (the
    /// pre-p1 behaviour, same as the bool false); 2 = slow long-run expectation `monetary.pi_lr` (target if None) plus
    /// `user_cost_pi_e_offset`; 3 = slow EWMA of the household mean with memory `user_cost_pi_e_months`. Any value above 3
    /// behaves as 0. Units: mode code (u8). Capital spending responds to persistent, not gasoline-driven, expectations.
    /// Literature: Coibion, Gorodnichenko & Kumar 2018; Sharpe & Suarez 2021.
    pub user_cost_pi_e_mode: u8,
    /// b9-oil U: decimal added to pi^LR in mode 2 (re-centres the long-run expectation on the household mean). Units:
    /// decimal. 0 = no offset. Used only in mode 2.
    pub user_cost_pi_e_offset: f64,
    /// b9-oil U: memory of the mode-3 EWMA of the household mean expectation, gain 1/(1+m). Units: months. Used only in
    /// mode 3.
    pub user_cost_pi_e_months: f64,

    // b9-inflation
    /// b9-inflation M-H: χ_h, the household policy term on expected inflation: π^e_h −= χ_h·L_h·tanh(gap_t / L_h), a
    /// bounded calibration device keyed to the PolicyShift gap (a tightening lowers household expectations). Units:
    /// dimensionless χ (the term is decimal inflation). 0 = off (legacy). The sign of the survey evidence is mixed: Doh,
    /// Lee & Park 2024 (KC Fed RWP 24-06) find a fall; Ferreira & Pica 2024 and Grigoli, Sandri, Gorodnichenko & Coibion
    /// 2026 find a rise. Adopted only if every gate passes (batch 9 ruling 5).
    pub hh_mp_expect: f64,
    /// b9-inflation M-H: L_h, saturation scale of the household policy term (decimal). Units: decimal. Used only when
    /// `hh_mp_expect` is non-zero.
    pub hh_mp_expect_cap: f64,
    /// b9-inflation M-D: κ, the demand-induced markup pull. The deviation d_t = (1 − ν)·d_{t−1} + κ·(g_t − n_{t−1}) moves
    /// the markup anchor of each repricing non-Energy firm to m0 + d_t. Units: markup points (decimal, as the markup) per
    /// unit Okun gap per month. 0 = off (legacy). Literature: Christiano, Eichenbaum & Evans 2005 JPE; Galí & Gertler 1999
    /// JME; Rotemberg & Woodford 1999.
    pub markup_dev_pull: f64,
    /// b9-inflation M-D: ν, the monthly decay of the demand-induced markup deviation. Units: per month. Used only when
    /// `markup_dev_pull` is non-zero.
    pub markup_dev_decay: f64,
    /// b9-inflation M-D: bound on |d| (decimal, as the markup). Units: decimal. Used only when `markup_dev_pull` is non-zero.
    pub markup_dev_max: f64,
    /// b9-inflation M-D: initial value n_0 of the slow norm n of the Okun gap g = 2(u* − u), clamped to ±0.1 (decimal;
    /// the mean gap of the stored baseline runs is −0.0098). Units: decimal. Used only when `markup_dev_pull` is non-zero.
    pub markup_dev_gap_ref: f64,
    /// b9-inflation M-D: M, the memory of the gap norm in months (EWMA gain 1/(1 + M)). Units: months. Used only when
    /// `markup_dev_pull` is non-zero.
    pub markup_dev_norm_months: f64,

    // b9-warmup
    /// b9-warmup: months the common world is stepped in the reported regime, after the burn-in reset and before the
    /// shocks are scheduled, by `sim::compare` only. Reported months start after the warm-up (their `tick` and `date`
    /// are rebased so month 0 stays tick 0, date 2026.0). 0 = off (legacy). Should be a multiple of 12, because annual
    /// events are keyed to `tick % 12`; the server accepts 0 or a multiple of 12 up to 120. Units: months. Live
    /// simulations (`POST /api/sims`) ignore it.
    pub warmup_months: u32,

    // us-now
    /// us-now: where month 0 of a comparison comes from. `typical` (default) = the model's own state; `us_now` = steered to
    /// `start_targets` (`sim::compare` only; see `start.rs`). The warm-up is then the targets' window, and `warmup_months` is ignored.
    pub start_state: crate::start::StartState,
    /// us-now: the data the `us_now` start follows (window of monthly values plus the unemployment rate). Required with
    /// `start_state = us_now`, rejected with `typical`.
    pub start_targets: Option<crate::start::StartTargets>,

    // rz1
    /// rz1 F1: with `rate_perception_months` > 0 and this on, the deposit-rate leg of the households' real rate
    /// (`0.5 * deposit_rate` in `consumption::decide_one`) is smoothed with the same `rate_perception_months` as the 2-year
    /// leg and the firms' investment rate. Off = the leg reads the current deposit rate (legacy). No effect while
    /// `rate_perception_months` is 0.
    pub rate_perception_deposit: bool,
    /// rz1 F2: half-life (months) of the decaying tail on a `CreditSpread` shock. The spread holds for the shock's `months` and
    /// then decays geometrically: spread(until + k) = bps x 0.5^((k + 1) / half-life), k = 0, 1, ... 0 = off (the spread is a
    /// rectangle and is zero from month `until`, legacy). Range [0, 60]. Units: months.
    pub credit_tail_half_life_months: f64,
    /// rz1 F1b (recorded real series only): real FISIM (the imputed financial services in recorded consumption and GDP) is the
    /// stock of household deposits and loans at the margins of the first reported month, deflated by the general price index (the
    /// PCE deflator excluding FISIM) and indexed to the legacy value at that month, as the SNA volume measure prescribes (SNA 2008
    /// 15.114), instead of the current month's margins over the 3-month yield divided by the OtherServices price. Nominal FISIM and
    /// the economy are unchanged; only `consumption_real_bn_ar`, `gdp_real_bn_ar`, `gdp_real_growth_yoy` and
    /// `real_consumption_per_hh` can differ. With the world block's trade link and US import channel on, `world/trade.rs` reads
    /// last month's `gdp_real_bn_ar`, so the key then reaches the economy. Off = the current-margin real FISIM (legacy).
    pub fisim_constant_margin: bool,
    /// rz1 F1: mean lag (months) of the firms' investment user cost, when different from the households' lag. A negative value
    /// (the default, -1) means "the same as `rate_perception_months`" (legacy: one lag for firms and households). A value >= 0
    /// replaces `rate_perception_months` in the firms' smoother only (`firms::plan`); the households keep `rate_perception_months`.
    /// Range [-1, 36]. Units: months.
    pub rate_perception_firm_months: f64,

    // rz12
    /// rz12: number of exponential smoothers in series in the households' rate perception (the 2-year leg and, with
    /// `rate_perception_deposit`, the deposit leg). Each stage has the lag `rate_perception_months / stages`, so the mean delay
    /// stays `rate_perception_months` and the delay becomes gamma-shaped (no weight peak on the first month). 1 = the single
    /// smoother (legacy). Range [1, 4].
    pub rate_perception_stages: u32,
    /// rz12: the same for the firms' perceived business lending rate. A negative value (the default, -1) means "the same as
    /// `rate_perception_stages`". Range [-1, 4] without 0.
    pub rate_perception_firm_stages: i32,
    /// rz12: the share of the `CreditSpread` shock (with its tail) in the banks' business base rate that bypasses the firms' perception
    /// smoother: firms see that part of their cost of credit as it is, and only the rest (the yield curve, i.e. the policy path) is smoothed.
    /// 0 = the whole base rate is smoothed (legacy). Range [0, 1]. No effect without `rate_perception_months` > 0 or without a `CreditSpread` shock.
    pub rate_perception_credit_bypass: f64,

    // rz19
    /// rz19: the efficiency wedge of a tariff. While a tariff is in force the productivity of every firm (the sector TFP indices
    /// `exog.tfp` and each firm's labour productivity) is lower by `exp(-tariff_tfp_loss * tau)`, where `tau` is the effective duty
    /// on imported inputs and equipment (`world::tau_for(w, Channel::Firm)`: the `ImportTariff` level, or with the world block's trade
    /// link the import-weighted duty over the tradable sectors, in both cases the change since the start of the run, so today's duty
    /// in a Today's-US-economy start carries no wedge), as a decimal (0.25 for 25 points). The wedge phases in linearly over
    /// `tariff_tfp_phase_in_months` and phases out the same way when the duty falls; it never raises productivity above the start
    /// (a duty cut gives no gain). Units: log points of productivity per 1.0 (= 100 points) of duty. 0 = off (legacy): the code path
    /// is not entered and the run is bit-identical. Range [0, 2]. Source of the calibration: docs/CALIBRATION.md (RZ21).
    pub tariff_tfp_loss: f64,
    /// rz19: months in which the efficiency wedge builds up from zero to its full size `tariff_tfp_loss * tau` (and falls back to zero),
    /// a linear ramp; the ramp's speed follows the size of the target, so any size takes the same time. No effect while
    /// `tariff_tfp_loss` is 0. Range [1, 120]. Units: months.
    pub tariff_tfp_phase_in_months: f64,

    // rz21
    /// rz21: the policy rule looks through the first round of a tariff. The inflation the rule reads (12-month core PCE, core CPI or
    /// headline, as `rule_uses_core` and `rule_uses_pce` choose) is replaced by `pi - rule_lookthrough * (pi - pi_xt)`, where `pi_xt` is
    /// the same index's 12-month inflation with the retail tariff wedge held where it was (the wedge's own contribution over the last
    /// 12 months removed: inflation "at a constant tariff", as the ECB's HICP at constant taxes removes a VAT change). It takes the
    /// duty's direct retail price effect only; the domestic price rises and wage rises that follow stay in the rule's inflation. 0 = off
    /// (legacy): nothing is computed and the run is bit-identical; also inert in any run without a tariff wedge. Range [0, 1]. Units: share.
    /// Source of the calibration: docs/CALIBRATION.md (RZ21).
    pub rule_lookthrough: f64,

    // rz20
    /// rz20: weight on Fund wealth above the consumption kink (`WEALTH_KINK` = $1 million, wage-indexed with `distquick_kink_wage_indexed`) in the
    /// Fund-wealth term of the household budget (`consumption.rs`). 0.35 = the legacy `WEALTH_TAIL_WEIGHT` (bit-identical); 1 = no concavity for
    /// Fund wealth. Range [0.05, 1]. Units: weight (decimal). Used only while `mpc_fund_wealth_annual` > 0.
    pub fund_wealth_tail_weight: f64,
    /// rz20: span, in months, of the EWMA of the Fund share price that the Fund-wealth term reads when `fund_wealth_ewma` is on (weight 2 / (span + 1) on the
    /// newest close). 12 = the legacy 12-month span (mean lag 6.5 months). Range [2, 60]. Units: months. No effect while `fund_wealth_ewma` is false.
    pub fund_wealth_ewma_span: f64,
    /// rz20: the q-sensitivity of desired capital in the sectors that have none of their own (`investment_q_sens[j]` = 0; Tech keeps its 0.2 on its own
    /// q). Those sectors' desired capital is multiplied by exp(sens x L), L the Fund's aggregate q: the equity-value-weighted mean over the sectors of
    /// ln q_rel, q_rel being the sector's Tobin's q relative to its normal level (`banking::fund_q_log`); the factor stays in [0.75, 1.33], as the
    /// sector factor does. 0 = off (legacy). Range [0, 1]. Units: elasticity of desired capital with respect to the aggregate q.
    pub investment_q_sens_all: f64,
    /// rz20: financial accelerator from equity values to the lender premium (excess bond premium). The target of every bank's premium
    /// (`step_premium`) gains `ebp_equity_sens` times the fall of the Fund's aggregate q below its normal level,
    /// max(0, -L), L the Fund's current aggregate q (`Fund::q_raw_log`: the equity-value-weighted mean of ln(q / q0) over the sectors, not smoothed) (decimal premium per unit of log fall; 0.024 is 2.4 percentage points per unit). The premium feeds business
    /// loan rates and credit standards. 0 = off (legacy). Range [0, 0.05]. Units: decimal per unit of ln q.
    pub ebp_equity_sens: f64,
    /// rz20 (re-rating form of the three equity levers: they act on the part of the Fund's equity value that a `SharePrices` re-rating moves and on nothing else, so a
    /// run with no re-rating is bit-identical with these keys on; each is 0 = off by default).
    /// `rerate_wealth_mpc_annual`: an extra annual marginal propensity to consume out of the wealth that a re-rating takes from (or adds to) a household's Fund shares: the
    /// budget loses m / 12 x (the household's Fund shares) x W, W the EWMA (span `fund_wealth_ewma_span`, whatever `fund_wealth_ewma` says) of the
    /// wealth that the re-rating takes from one Fund share (dollars; negative for a rise). Not concave. Range [0, 0.2]. Units: per year, per dollar.
    pub rerate_wealth_mpc_annual: f64,
    /// rz20 (re-rating form): the elasticity of desired capital, in the sectors with no q-sensitivity of their own, to the EWMA (`q_signal_decay`) of the re-rating's log:
    /// factor exp(sens x it), bounded to [0.75, 1.33]. Range [0, 1].
    pub rerate_q_sens: f64,
    /// rz20 (re-rating form): the lender premium's target gains `rerate_ebp_sens` x max(0, -L), L the (unsmoothed) log of the re-rating in force. Range [0, 0.05]. Units: decimal per unit of ln.
    pub rerate_ebp_sens: f64,

    // rz24
    /// rz24: the equity risk premium loads on a `CreditSpread` shock. While the shock (with its decaying tail, `credit_tail_half_life_months`)
    /// is in force the premium is higher by `credit_shock_erp` x the spread added to bank loan rates (decimal: 0.02 for +200 bp), in every
    /// sector (`equity::equity_risk_premium`), so valuations fall by the premium times the sectors' duration. The loading is on the shock only:
    /// the lender premium and the baseline cycle are untouched, so a run with no `CreditSpread` shock is bit-identical with the key on. The
    /// premium stays inside [`erp_min`, `erp_max`]. A negative shock (credit easing) lowers the premium by the same loading. Units: points of
    /// equity premium per point of spread. 0 = off (legacy): nothing is computed. Range [0, 5]. Source of the calibration: docs/CALIBRATION.md (RZ24).
    /// rz24-r1: a sector that an active `SharePrices` re-rating covers (a shock aimed at all sectors or at that sector, whatever its size) is valued without
    /// the loading: the reader's typed share-price path already sets that market, so the model's own estimate of what credit does to equities is not added
    /// to it. Sectors the re-rating does not cover keep the loading. Docs: docs/CALIBRATION.md (RZ24-R1).
    pub credit_shock_erp: f64,
    // rz27
    /// rz27: the Energy sector's equity value follows the oil price at a bounded elasticity instead of its own dividends. With this on, the sector is valued
    /// (`equity::value_sectors`) on its seeded share of the other eight sectors' smoothed dividends, with no extrapolated growth, times
    /// (the real oil price over its value at seeding) raised to `energy_equity_oil_elasticity`. Its own dividends (the oil windfall in the sector's
    /// profit, which the smoothed dividends and the growth extrapolation pass on at about twice the oil move) no longer set its price; they are still
    /// paid out and recorded. Valuation only: no payment and no stock-flow identity is touched. false = off (legacy: bit-identical); also inert before
    /// the first valuation seeds the share and the oil anchor. Source of the calibration: docs/CALIBRATION.md (RZ27).
    pub energy_equity_oil_on: bool,
    /// rz27: the elasticity of the Energy sector's equity value to the real oil price while `energy_equity_oil_on` is set. Range [0, 1]. Units: elasticity.
    /// 0.24 is the equal-weight mean of Kanzig's (2021, PDF p.55, Figure A.8) impact responses of the oil and gas stock index (+3.3% for an oil price
    /// +10%: 0.33) and of the electricity stock index (+1.5%: 0.15).
    pub energy_equity_oil_elasticity: f64,
}

impl Config {
    /// Re-fit Step 4c: the upstream split of domestic crude as (τ, ρ, m, κ) = (production taxes, private royalties,
    /// margin, upstream capital). κ = 1 − τ − ρ − m is Energy's own capital cost, carried by `testkit::capital_output_months`
    /// and not by Energy's crude price. `None` when `upstream_margin >= 1.0`: the legacy path, in which domestic crude
    /// is unpaid and τ and ρ are ignored.
    pub fn upstream_split(&self) -> Option<(f64, f64, f64, f64)> {
        if self.upstream_margin >= 1.0 {
            return None;
        }
        let tau = self.upstream_tax_share.clamp(0.0, 1.0);
        let rho = self.upstream_royalty_share.clamp(0.0, 1.0);
        let m = self.upstream_margin.clamp(0.0, 1.0);
        Some((tau, rho, m, (1.0 - tau - rho - m).max(0.0)))
    }

    /// Normalised debt composition by tenor (`maturity_stock_mix` scaled to sum to one), used for the t=0 stock.
    pub fn debt_mix(&self) -> [f64; N_TENORS] {
        let total: f64 = self.maturity_stock_mix.iter().sum();
        if total > 0.0 {
            std::array::from_fn(|j| self.maturity_stock_mix[j] / total)
        } else {
            [1.0 / N_TENORS as f64; N_TENORS]
        }
    }
}

impl Config {
    /// Rejects unknown names of the stage-3 oil keys (stage-3 review R10). Before this check an unknown `world_oil_profile`
    /// panicked inside the run, and any other `world_opec_regime` silently meant `price_defence`; an unknown mask character
    /// was ignored. The server calls this before it runs a request.
    pub fn validate_world_oil(&self) -> Result<(), String> {
        let profiles: Vec<String> = crate::world::params::WorldParams::load().oil.profiles.keys().cloned().collect();
        if !profiles.iter().any(|p| p == &self.world_oil_profile) {
            return Err(format!("unknown world_oil_profile {:?} (one of {})", self.world_oil_profile, profiles.join(", ")));
        }
        if !matches!(self.world_opec_regime.as_str(), "price_defence" | "market_share") {
            return Err(format!("unknown world_opec_regime {:?} (price_defence or market_share)", self.world_opec_regime));
        }
        if let Some(c) = self.world_oil_shock_mask.chars().find(|c| !matches!(c, 'I' | 'D' | 'O')) {
            return Err(format!("world_oil_shock_mask {:?} holds {c:?}; only I, D and O are allowed", self.world_oil_shock_mask));
        }
        Ok(())
    }

    /// Refuses the bilateral dollar link (`world_link_dollar`) with the trade link (`world_link_trade`) off (stage 4-1 review, D7;
    /// S4-R14). The dollar link then acts through the legacy nominal channels and flips the sign of the baseline shift (core -0.45,
    /// unemployment +0.46 in the review's 24-seed check). Then the stage 4-2 finance ranges (`validate_world_finance`). The server and
    /// the CLI call this before they run a config.
    pub fn validate_world_links(&self) -> Result<(), String> {
        if self.world_link_dollar && !self.world_link_trade {
            return Err(
                "world_link_dollar needs world_link_trade: with the trade link off the dollar link flips the sign of the baseline shift \
                 (stage 4-1 review); set world_link_trade true or world_link_dollar false"
                    .to_string(),
            );
        }
        let (k, l) = (self.world_rp_sat_knee, self.world_rp_sat_scale);
        if !(k.is_finite() && (0.0..=0.2).contains(&k)) {
            return Err(format!("world_rp_sat_knee = {k} is out of range: 0 to 0.2 (decimal, a year)"));
        }
        if !(l.is_finite() && (0.0..=0.2).contains(&l)) {
            return Err(format!("world_rp_sat_scale = {l} is out of range: 0 to 0.2 (decimal, a year); 0 turns the bound off"));
        }
        if self.world_retaliation && !self.world_link_trade {
            return Err("world_retaliation needs world_link_trade: the retaliation factor multiplies the trade block's bilateral exports".to_string());
        }
        self.validate_world_ret()?;
        self.validate_world_import_gap_offset()?;
        self.validate_world_import_rel_drift()?;
        self.validate_world_finance()
    }

    /// Stage 5-T2 (D62): `world_import_rel_drift` is finite and within +-0.05 a year.
    pub fn validate_world_import_rel_drift(&self) -> Result<(), String> {
        let g = self.world_import_rel_drift;
        if !g.is_finite() || g.abs() > 0.05 {
            return Err(format!("world_import_rel_drift {g} is out of range: finite and within +-0.05 a year (decimal)"));
        }
        Ok(())
    }

    /// rz2: range checks of the oil-fix keys. The off value of every key passes. A bad value is refused (400 / exit 2) instead of
    /// running as a silent legacy case (`user_cost_pi_e_mode` above 3 behaves as 0).
    pub fn validate_rz2(&self) -> Result<(), String> {
        fn within(name: &str, v: f64, ok: bool, want: &str) -> Result<(), String> {
            if ok {
                Ok(())
            } else {
                Err(format!("{name} = {v} is out of range: {want}"))
            }
        }
        let w = self.hh_pump_real_rate_weight;
        within("hh_pump_real_rate_weight", w, w.is_finite() && (0.0..=1.0).contains(&w), "0 to 1")?;
        let f = self.firm_pump_user_cost_weight;
        within("firm_pump_user_cost_weight", f, f.is_finite() && (0.0..=1.0).contains(&f), "0 to 1")?;
        let x = self.energy_extraction_labor_share;
        within("energy_extraction_labor_share", x, x.is_finite() && (0.0..=1.0).contains(&x), "0 to 1")?;
        let e = self.energy_extraction_elasticity;
        within("energy_extraction_elasticity", e, e.is_finite() && (0.0..=4.0).contains(&e), "0 to 4")?;
        within("user_cost_pi_e_mode", f64::from(self.user_cost_pi_e_mode), self.user_cost_pi_e_mode <= 3, "0, 1, 2 or 3")?;
        let o = self.user_cost_pi_e_offset;
        within("user_cost_pi_e_offset", o, o.is_finite() && o.abs() <= 0.02, "within +-0.02 (decimal)")?;
        let m = self.user_cost_pi_e_months;
        within("user_cost_pi_e_months", m, m.is_finite() && (0.0..=120.0).contains(&m), "0 to 120 months")?;
        Ok(())
    }

    /// Bound pairs that reach `f64::clamp`, which panics when the lower bound is above the upper one (before this check the
    /// server answered such a config with a 500 from the panic): `u_star_min` and `u_star_max` (the natural-rate estimate,
    /// every month), `ebp_min` and `ebp_max` (the lender premium, every month), `mortgage_spread_min_adj` and
    /// `mortgage_spread_max_adj` (only with a mortgage-spread gain), and `r_star_est_min` against the fixed upper bound of the r*
    /// estimate, `monetary::R_STAR_EST_MAX` (only with `r_star_adapt` > 0). A pair the run does not use is not checked. Equal
    /// bounds pass. The server calls this before it runs a request, and the CLI before it runs a config.
    pub fn validate_clamp_bounds(&self) -> Result<(), String> {
        fn ordered(lo_name: &str, lo: f64, hi_name: &str, hi: f64) -> Result<(), String> {
            if lo <= hi {
                Ok(())
            } else {
                Err(format!("{lo_name} = {lo} is above {hi_name} = {hi}"))
            }
        }
        ordered("u_star_min", self.u_star_min, "u_star_max", self.u_star_max)?;
        ordered("ebp_min", self.ebp_min, "ebp_max", self.ebp_max)?;
        if self.mortgage_spread_vol_gain != 0.0 || self.mortgage_spread_stress_gain != 0.0 {
            let (lo, hi) = (self.mortgage_spread_min_adj, self.mortgage_spread_max_adj);
            ordered("mortgage_spread_min_adj", lo, "mortgage_spread_max_adj", hi).map_err(|e| format!("{e} (a mortgage-spread gain is set)"))?;
        }
        if self.r_star_adapt > 0.0 {
            let max = crate::monetary::R_STAR_EST_MAX;
            ordered("r_star_est_min", self.r_star_est_min, "the r* estimate's upper bound", max).map_err(|e| format!("{e} (r_star_adapt > 0)"))?;
        }
        Ok(())
    }

    /// Stage 5 (S2-R7, D57): `world_import_gap_offset` is empty or holds twelve finite values in [-10, 10] (pp; the term it centres is clamped to +-10).
    pub fn validate_world_import_gap_offset(&self) -> Result<(), String> {
        let v = &self.world_import_gap_offset;
        if !(v.is_empty() || v.len() == 12) {
            return Err(format!("world_import_gap_offset has {} values: empty or twelve (one per bloc, pp)", v.len()));
        }
        if let Some(x) = v.iter().find(|x| !x.is_finite() || x.abs() > 10.0) {
            return Err(format!("world_import_gap_offset value {x} is out of range: finite and within +-10 pp"));
        }
        Ok(())
    }

    /// Range checks of the stage 4-3 retaliation keys (list-A' style: a bad value cannot wait for the switch).
    pub fn validate_world_ret(&self) -> Result<(), String> {
        let (sr, lr, sp) = (self.world_ret_eps_sr, self.world_ret_eps_lr, self.world_ret_speed);
        if !((0.0..=10.0).contains(&sr) && (0.0..=10.0).contains(&lr) && lr >= sr) {
            return Err(format!("world_ret_eps_sr = {sr}, world_ret_eps_lr = {lr}: each 0 to 10 and the long run at least the short run"));
        }
        if !(0.0..=1.0).contains(&sp) {
            return Err(format!("world_ret_speed = {sp} is out of range: 0 to 1"));
        }
        Ok(())
    }

    /// Range checks of the stage 4-2 official-demand keys and of `world_gdp_rw_bn` (list A', S4-R22, review D2). The bounds refuse the
    /// values that made the output non-finite or ran the yields away (`world_fo_trans_rho` 1.5, `world_fo_theta_stock` 1e9,
    /// `world_gdp_rw_bn` 0, `world_fo_dollar_resp` -5), and every default and profile of the repository lies inside them. They hold
    /// whatever the link state, so a bad value cannot wait for the link to be switched on. `world_link_finance` is a bool (typed by serde).
    /// NaN and infinite values fail every check.
    pub fn validate_world_finance(&self) -> Result<(), String> {
        fn within(name: &str, v: f64, ok: bool, want: &str) -> Result<(), String> {
            if ok {
                Ok(())
            } else {
                Err(format!("{name} = {v} is out of range: {want}"))
            }
        }
        let (s, t, rho) = (self.world_fo_theta_stock, self.world_fo_theta_trans, self.world_fo_trans_rho);
        within("world_fo_theta_stock", s, (0.0..=100.0).contains(&s), "0 to 100 bp per 1 % of GDP")?;
        within("world_fo_theta_trans", t, (0.0..=100.0).contains(&t), "0 to 100 bp per 1 % of GDP")?;
        within("world_fo_trans_rho", rho, (0.0..1.0).contains(&rho), "0 up to below 1")?;
        let eta = self.world_fo_dollar_resp;
        within("world_fo_dollar_resp", eta, eta.abs() <= 0.05, "absolute value at most 0.05")?;
        let v = self.world_fo_vix_resp;
        within("world_fo_vix_resp", v, (0.0..=10.0).contains(&v), "0 to 10 $bn a month per VIX point")?;
        let trend = self.world_fo_trend_cn;
        within("world_fo_trend_cn", trend, trend.abs() <= 500.0, "absolute value at most 500 $bn a year")?;
        let gdp = self.world_gdp_rw_bn;
        within("world_gdp_rw_bn", gdp, gdp > 0.0 && gdp <= 1.0e6, "above 0 and at most 1e6 $bn")?;
        Ok(())
    }
}

/// rz1: upper bounds of the lag and tail keys (months).
pub const RATE_PERCEPTION_MAX_MONTHS: f64 = 36.0;
pub const CREDIT_TAIL_MAX_HALF_LIFE_MONTHS: f64 = 60.0;
/// rz19: upper bounds of the tariff efficiency wedge (log points per 1.0 of duty) and of its phase-in (months).
pub const TARIFF_TFP_MAX_LOSS: f64 = 2.0;
pub const TARIFF_TFP_MAX_PHASE_IN_MONTHS: f64 = 120.0;
/// rz20: bounds of the equity-channel keys.
pub const FUND_WEALTH_TAIL_WEIGHT_MIN: f64 = 0.05;
pub const INVESTMENT_Q_SENS_ALL_MAX: f64 = 1.0;
pub const EBP_EQUITY_SENS_MAX: f64 = 0.05;
pub const FUND_WEALTH_EWMA_SPAN_RANGE: (f64, f64) = (2.0, 60.0);
pub const RERATE_WEALTH_MPC_MAX: f64 = 0.2;
pub const RERATE_Q_SENS_MAX: f64 = 1.0;
pub const RERATE_EBP_SENS_MAX: f64 = 0.05;
/// rz24: upper bound of the equity risk premium's loading on a credit-spread shock (points of premium per point of spread).
pub const CREDIT_SHOCK_ERP_MAX: f64 = 5.0;

impl Config {
    /// rz1: range check of the rate-perception lag, the credit-spread tail and the desired-capital adjustment share.
    /// `rate_perception_months` must be finite and in [0, 36]; `rate_perception_firm_months` finite and in [-1, 36];
    /// `credit_tail_half_life_months` finite and in [0, 60]; `kstar_adapt` finite and in [0, 1]; (rz19) `tariff_tfp_loss` finite and
    /// in [0, 2], `tariff_tfp_phase_in_months` finite and in [1, 120]. The off values (0, -1, 0, 0, 0 and the default 60) always pass. The server calls this
    /// before it runs a request; `from_env` calls it for the CLI.
    pub fn validate_rz1(&self) -> Result<(), String> {
        let lag = self.rate_perception_months;
        if !lag.is_finite() || !(0.0..=RATE_PERCEPTION_MAX_MONTHS).contains(&lag) {
            return Err(format!("rate_perception_months must be between 0 and {RATE_PERCEPTION_MAX_MONTHS} (got {lag})"));
        }
        let hl = self.credit_tail_half_life_months;
        if !hl.is_finite() || !(0.0..=CREDIT_TAIL_MAX_HALF_LIFE_MONTHS).contains(&hl) {
            return Err(format!("credit_tail_half_life_months must be between 0 and {CREDIT_TAIL_MAX_HALF_LIFE_MONTHS} (got {hl})"));
        }
        let fl = self.rate_perception_firm_months;
        if !fl.is_finite() || !(-1.0..=RATE_PERCEPTION_MAX_MONTHS).contains(&fl) {
            return Err(format!("rate_perception_firm_months must be between -1 and {RATE_PERCEPTION_MAX_MONTHS} (got {fl})"));
        }
        let ka = self.kstar_adapt;
        if !ka.is_finite() || !(0.0..=1.0).contains(&ka) {
            return Err(format!("kstar_adapt must be between 0 and 1 (got {ka})"));
        }
        // rz12: the number of smoothing stages.
        if !(1..=4).contains(&self.rate_perception_stages) {
            return Err(format!("rate_perception_stages must be 1, 2, 3 or 4 (got {})", self.rate_perception_stages));
        }
        if !(self.rate_perception_firm_stages == -1 || (1..=4).contains(&self.rate_perception_firm_stages)) {
            return Err(format!("rate_perception_firm_stages must be -1, 1, 2, 3 or 4 (got {})", self.rate_perception_firm_stages));
        }
        let cb = self.rate_perception_credit_bypass;
        if !cb.is_finite() || !(0.0..=1.0).contains(&cb) {
            return Err(format!("rate_perception_credit_bypass must be between 0 and 1 (got {cb})"));
        }
        // rz19: the tariff efficiency wedge.
        let tl = self.tariff_tfp_loss;
        if !tl.is_finite() || !(0.0..=TARIFF_TFP_MAX_LOSS).contains(&tl) {
            return Err(format!("tariff_tfp_loss must be between 0 and {TARIFF_TFP_MAX_LOSS} (got {tl})"));
        }
        let tm = self.tariff_tfp_phase_in_months;
        if !tm.is_finite() || !(1.0..=TARIFF_TFP_MAX_PHASE_IN_MONTHS).contains(&tm) {
            return Err(format!("tariff_tfp_phase_in_months must be between 1 and {TARIFF_TFP_MAX_PHASE_IN_MONTHS} (got {tm})"));
        }
        // rz21: the rule's look-through of a tariff's first round.
        let lt = self.rule_lookthrough;
        if !lt.is_finite() || !(0.0..=1.0).contains(&lt) {
            return Err(format!("rule_lookthrough must be between 0 and 1 (got {lt})"));
        }
        // rz20: the equity channel.
        let tw = self.fund_wealth_tail_weight;
        if !tw.is_finite() || !(FUND_WEALTH_TAIL_WEIGHT_MIN..=1.0).contains(&tw) {
            return Err(format!("fund_wealth_tail_weight must be between {FUND_WEALTH_TAIL_WEIGHT_MIN} and 1 (got {tw})"));
        }
        let sp = self.fund_wealth_ewma_span;
        if !sp.is_finite() || !(FUND_WEALTH_EWMA_SPAN_RANGE.0..=FUND_WEALTH_EWMA_SPAN_RANGE.1).contains(&sp) {
            return Err(format!("fund_wealth_ewma_span must be between {} and {} months (got {sp})", FUND_WEALTH_EWMA_SPAN_RANGE.0, FUND_WEALTH_EWMA_SPAN_RANGE.1));
        }
        let qs = self.investment_q_sens_all;
        if !qs.is_finite() || !(0.0..=INVESTMENT_Q_SENS_ALL_MAX).contains(&qs) {
            return Err(format!("investment_q_sens_all must be between 0 and {INVESTMENT_Q_SENS_ALL_MAX} (got {qs})"));
        }
        for (name, v, max) in [
            ("rerate_wealth_mpc_annual", self.rerate_wealth_mpc_annual, RERATE_WEALTH_MPC_MAX),
            ("rerate_q_sens", self.rerate_q_sens, RERATE_Q_SENS_MAX),
            ("rerate_ebp_sens", self.rerate_ebp_sens, RERATE_EBP_SENS_MAX),
        ] {
            if !v.is_finite() || !(0.0..=max).contains(&v) {
                return Err(format!("{name} must be between 0 and {max} (got {v})"));
            }
        }
        let es = self.ebp_equity_sens;
        if !es.is_finite() || !(0.0..=EBP_EQUITY_SENS_MAX).contains(&es) {
            return Err(format!("ebp_equity_sens must be between 0 and {EBP_EQUITY_SENS_MAX} (got {es})"));
        }
        // rz24: the equity premium's loading on a credit-spread shock.
        let ce = self.credit_shock_erp;
        if !ce.is_finite() || !(0.0..=CREDIT_SHOCK_ERP_MAX).contains(&ce) {
            return Err(format!("credit_shock_erp must be between 0 and {CREDIT_SHOCK_ERP_MAX} (got {ce})"));
        }

        // rz27: the Energy equity's oil elasticity.
        let ee = self.energy_equity_oil_elasticity;
        if !ee.is_finite() || !(0.0..=1.0).contains(&ee) {
            return Err(format!("energy_equity_oil_elasticity must be between 0 and 1 (got {ee})"));
        }
        Ok(())
    }
}

impl Default for Config {
    fn default() -> Self {
        Config {
            n_households: 20_000,
            households_per_firm: 25.0,
            n_banks: 5,
            scale: 0.0,
            // months simulated before t=0 so analyses start after the initial adjustment. 144 (round 6, was 72): at 72 the
            // reported run starts mid-disinflation (core PCE 0.6 in year 1, policy 0.9); at 144 year 1 is about 1.8.
            burn_in_months: 144,

            oil_base: 65.0,
            stochastic_oil: true,
            oil_ar: 0.985,
            oil_sigma: 0.085,
            // p0-exog: the pre-re-fit defaults had the extra processes off (sigmas, intensities, loadings 0); the
            // current defaults carry the values set in the fields below. Persistences are the literature values.
            fin_factor_rho: 0.98,
            fin_factor_sigma: 0.0,
            fin_factor_vol_gamma: 0.0,
            // Re-fit Step 10a (stop record): the compensated-jump candidate is off by default; see docs/CALIBRATION.md.
            fin_jump_intensity: 0.0,
            fin_jump_mean: 0.0,
            fin_jump_sd: 0.0,
            tp_level_rho: 0.98,
            tp_level_sigma: 0.0014,
            tp_level_fin_loading: 0.0,
            tp_curv_rho: 0.95,
            tp_curv_sigma: 0.002,
            fx_psi_rho: 0.99,
            fx_psi_sigma: 0.0215,
            fx_psi_fin_corr: 0.0,
            fx_psi_fin_jump_loading: 0.0,
            oil_t_nu: 4.0,
            oil_garch_alpha: 0.15,
            oil_garch_beta: 0.8,
            oil_jump_intensity: 0.0,
            oil_jump_sd: 0.0,
            oil_index_to_world: true,
            risk_premium_ar: 0.97,
            // re-fitted to GDP cycle sd ≈1.5% after the equity wealth and discretionary-spending channels were added;
            // integration (round 4): 0.0015 -> 0.0007, because the merged channels add cycle volatility (GDP sd 2.01 -> 1.59)
            // Re-fit Step 10c: 0.0007 -> 0.0002 (16-seed HP sd 1.35 at 0.0002 in the accept band 1.20-1.45; flat above
            // 0.0005, see docs/CALIBRATION.md). The loading is rescaled so the ERP demand term is unchanged (erp 2.5 x 0.0007/0.0002).
            risk_premium_sigma: 0.0002,
            risk_premium_firm_weight: 1.0,
            // integration (round 4): 0.0015 -> 0.0008 (with risk_premium_sigma: GDP sd 1.59, u sd 0.89)
            // Re-fit Step 10b: 0.0008 -> 0.002 (largest grid value with corr_emp >= 0.72 and lp_cyc >= 0.15; legacy 0.0008).
            tfp_shock_sigma: 0.002,
            // labour-productivity trend (no capital deepening in the model; US ≈1.5%/yr)
            tfp_growth_annual: 0.015,
            // sector offsets in pp over the 1.5% trend (docs/SECTORS.md; BLS labour productivity by industry, 1990–2019)
            // productivity growth offsets (BLS by industry); health and leisure penalties softened because PCE health prices are largely administered (PCE health inflation ≈1.5–2% in the 2010s)
            tfp_sector_offset: [0.0, 0.0, 0.015, -0.003, 0.03, -0.005, -0.005, 0.002, -0.015],
            // Legacy (see `pop_growth_annual` above).
            pop_growth_annual: 0.009,
            // Net immigrant households per household per year (US net international migration ≈1M people/yr, CBO).
            immigration_annual: 0.0035,
            household_formation_cyclicality: 0.18,
            entrant_slot_reset: true,

            min_wage_monthly: 1_260.0,
            // exogenous E→U separations (CPS flows ≈1.1–1.3%/month incl. layoffs, which are endogenous here)
            separation_rate: 0.009,
            applications: 6,
            matching_efficiency: 0.30,
            initial_fill_rate: 0.6,
            min_fill_rate: 0.2,
            on_the_job_search: 0.04,
            benefit_replacement: 0.45,
            benefit_cap: 2_600.0,
            burn_in_nominal_neutral: true,
            ui_cap_wage_indexed: true,
            entry_wage_indexed: true,
            wage_indexed_thresholds: true,
            benefit_months: 6,
            edu_premium: [1.0, 1.6, 1.95],

            inventory_target: [0.3, 0.5, 1.5, 1.0, 0.3, 0.05, 0.0, 0.0, 0.5],
            reprice_prob: [1.0, 0.35, 0.20, 0.20, 0.15, 0.10, 0.06, 0.12, 0.25],
            // Tech margins: software and platforms (information-sector profit share; De Loecker, Eeckhout & Unger 2020)
            initial_markup: [0.14, 0.10, 0.12, 0.12, 0.40, 0.10, 0.12, 0.14, 0.11],
            depreciation_annual: 0.12,
            // dividends + buybacks ≈ 90% of earnings (S&P 500, 2015–2024)
            payout_ratio: 0.9,
            investment_hurdle: 0.02,
            // elasticity of desired capital to the user cost (σ)
            investment_sensitivity: 0.4,
            capital_rundown_max_monthly: 0.0015,
            user_cost_household_expectation: true,
            kstar_adapt: 0.0,
            rate_perception_months: 0.0,
            target_utilization: 0.83,
            io: [
                [0.10, 0.04, 0.02, 0.04, 0.01, 0.03, 0.01, 0.02, 0.04],
                [0.00, 0.20, 0.00, 0.01, 0.00, 0.12, 0.01, 0.00, 0.00],
                [0.02, 0.01, 0.20, 0.03, 0.05, 0.02, 0.02, 0.02, 0.15],
                [0.04, 0.06, 0.05, 0.15, 0.02, 0.04, 0.05, 0.02, 0.08],
                [0.01, 0.01, 0.02, 0.01, 0.15, 0.02, 0.02, 0.04, 0.01],
                [0.00, 0.00, 0.00, 0.00, 0.01, 0.02, 0.01, 0.02, 0.00],
                [0.00, 0.00, 0.00, 0.00, 0.00, 0.00, 0.03, 0.01, 0.00],
                [0.08, 0.12, 0.10, 0.10, 0.12, 0.12, 0.12, 0.20, 0.12],
                [0.03, 0.00, 0.01, 0.01, 0.00, 0.01, 0.01, 0.02, 0.02],
            ],
            crude_intensity: 0.35,
            // oil & gas extraction ≈35% of energy employment (BLS); rig counts move more than one-for-one with oil (2014–16)
            energy_extraction_labor_share: 0.35,
            energy_extraction_elasticity: 1.7,
            oil_import_share: 0.15,
            // Input–output-consistent with the model's own final demand at t=0 (x = (I − A − A_cap)⁻¹ f, refined by one
            // fixed-point step on realised sector demand; see docs/CALIBRATION.md).
            employment_share: [0.020, 0.070, 0.090, 0.100, 0.040, 0.105, 0.200, 0.325, 0.050],

            // Sector roles (see the struct docs). Sector order: Energy, Food, DurableGoods, NondurableGoods, Tech,
            // LeisureHospitality, HealthEducation, OtherServices, Construction (docs/SECTORS.md, role vectors).
            // Capital goods: equipment 37% and software/IP/structures split across Durable, Tech, Other and Construction.
            // nonresidential investment: equipment, software, R&D and other IP, structures (BEA)
            capital_goods_mix: [0.0, 0.0, 0.30, 0.0, 0.17, 0.0, 0.0, 0.38, 0.15],
            // Investment deflator and capital valuation use the same capital-goods price mix (the doc gives no separate
            // valuation split; the old Goods-only value was the 80% goods share of the same mix).
            capital_valuation_mix: [0.0, 0.0, 0.30, 0.0, 0.17, 0.0, 0.0, 0.38, 0.15],
            tradable_weight: [0.0, 0.3, 1.0, 1.0, 0.3, 0.0, 0.0, 0.0, 0.0],
            nonstorable_sectors: [false, false, false, false, false, true, true, true, false],
            proprietor_sectors: [false, true, false, false, false, true, true, true, true],
            // Household service purchases (owner maintenance, landlords' operating costs): OtherServices. Employer health
            // benefits are spent on health care (`health_sector_mix`), per docs/SECTORS.md.
            household_services_mix: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            health_sector_mix: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0],
            financial_services_mix: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            // Government purchases: HealthEducation 0.45, Other 0.25, Durable 0.10 (defence), Nondur 0.03, Tech 0.04,
            // Construction 0.13.
            // government purchases: health and education, administration and defence services, equipment, structures (BEA)
            gov_purchase_split: [0.0, 0.0, 0.08, 0.03, 0.04, 0.02, 0.47, 0.30, 0.06],
            // BEA/BLS CPI relative importance, Dec 2024 (docs/SECTORS.md): shelter 0.36 separately; Construction is not
            // a CPI component.
            cpi_weights: [0.065, 0.080, 0.110, 0.080, 0.030, 0.106, 0.090, 0.079, 0.0],
            cpi_weight_shelter: 0.36,
            // Core CPI excludes Energy and Food (Construction is not a CPI component, so it is excluded too).
            cpi_core_sectors: [false, false, true, true, true, true, true, true, false],
            // Core PCE weights (BEA PCE excluding food and energy): shelter 0.17 separately.
            pce_core_weights: [0.0, 0.0, 0.12, 0.14, 0.03, 0.12, 0.21, 0.21, 0.0],
            pce_core_weight_shelter: 0.17,

            subsistence: [180.0, 480.0, 50.0, 210.0, 80.0, 230.0, 120.0, 300.0],
            // marginal budget shares calibrated with subsistence so sector employment matches BLS CES shares (docs/SECTORS.md)
            supernumerary_share: [0.04, 0.07, 0.09, 0.11, 0.06, 0.29, 0.10, 0.24],
            // typical households save little out of labour income beyond mortgage principal and buffers (US personal
            // saving ≈ 4–5% of disposable income, concentrated at the top)
            mpc_income: 0.97,
            // durables carry most of the interest sensitivity (discretionary channel); +100bp lowers total consumption
            // ≈0.7% (VAR evidence ≈0.3–0.5%)
            consumption_rate_sens: 0.3,
            // monthly MPCs out of liquid wealth above / below the buffer target (≈5%/yr; rebuild buffer in ~1–2 yrs)
            mpc_wealth_above: 0.005,
            mpc_wealth_below: 0.05,
            mpc_housing_wealth_annual: 0.03,
            mpc_fund_wealth_annual: 0.02,
            // fix-households: a one-off payment is spent at once at `mpc_windfall_low` when liquid wealth is below the buffer, at `mpc_windfall_high`
            // otherwise. RZ23 (adopted 2026-10-23 in v2026.10.23.1; docs/CALIBRATION.md, "RZ23"): 0.39 and 0.09 with no decaying tail (was 0.65, 0.15 and 0.03), so the
            // three-month non-durable spending out of a payment is the median of the identified estimates (0.21: Parker et al. 2013 and 2022,
            // Sahm, Shapiro & Slemrod, Orchard, Ramey & Wieland) and the 12-month response lands in 0.25-0.40. A run with no payment is unchanged.
            mpc_windfall_low: 0.39,
            mpc_windfall_high: 0.09,
            windfall_decay: 0.0,
            windfall_liquid_months: 2.0,
            windfall_sector_mix: [0.0, 0.15, 0.12, 0.33, 0.10, 0.10, 0.0, 0.20],
            energy_elasticity_long: 0.9,
            food_elasticity_long: 0.3,
            necessity_adapt: 0.02,
            impatient_yp_weight: 0.75,
            exhaust_yp_pull: 0.02,
            patient_mpc_offset: 0.1,
            // fix-households: 30% impatient types (KVW 2014: about 31% hand-to-mouth). Realised HtM share (deposits below half
            // a month's income) is about 23%: more impatient types push GDP cycle sd above the 1.9 band (the cyclicality
            // trade-off, see the commit message). Spending out of excess deposits is slow (5%/month): at 15%/month the
            // initial drawdown is a demand shock that pushes the policy rate to zero and shrinks GDP by a fifth.
            impatient_share: 0.33,
            impatient_buffer_months: 0.1,
            impatient_mpc_above: 0.05,
            impatient_fund_floor_months: 0.5,
            ui_yp_onset: 0.965,
            ui_yp_drift: 0.002,
            exhaust_credit_share: 0.75,
            retiree_horizon_years: 12.0,
            retiree_real_rate: 0.02,
            credit_card_spread: 0.15,
            // Discretionary shares of supernumerary spending (docs/SECTORS.md, stage 3): durables and leisure are the
            // cyclical categories; food, energy and health are necessities.
            discretionary_weight: [0.0, 0.1, 1.0, 0.3, 0.6, 0.9, 0.1, 0.3],
            // Calibrated so that +100bp on the policy rate lowers discretionary spending by a few percent (see
            // docs/DESIGN.md §8.3 and the IRF check); the sign and form follow the sources above.
            disc_theta_rate: 4.5,
            disc_theta_money: 0.10,
            disc_money_norm: 0.6,
            disc_theta_unemp: 0.5,
            disc_theta_credit: 4.0,
            disc_mult_min: 0.5,
            disc_mult_max: 1.6,

            // household import content by sector, calibrated so imports ≈12% of GDP (BEA import content of PCE; services imports: travel, transport, business services)

            import_share: [0.10, 0.08, 0.55, 0.45, 0.25, 0.06, 0.03, 0.08, 0.0],
            export_base_share_gdp: [0.012, 0.008, 0.035, 0.020, 0.012, 0.008, 0.002, 0.013, 0.0],
            switch_intensity: 4.0,
            switch_prob: 0.25,
            fx_rate_sensitivity: 4.0,
            // US import prices of consumer goods ≈ flat 1995–2019 (BLS import price index)
            world_goods_inflation: 0.005,
            fx_ppp_tradables: false,
            // weak purchasing-power anchor (PPP deviations decay over years; Rogoff 1996)
            fx_ppp_weight: 0.3,
            fx_adjust: 0.3,
            firm_input_import_share: 0.10,
            firm_equipment_import_share: 0.25,
            import_price_elasticity: 0.2,
            // fix-trade
            import_pass_through_long: 0.45,
            // fix-trade
            import_pass_through_speed: 0.08,
            // fix-trade: Food is priced on its own; durables, nondurables and Tech carry the tradables index.
            // fix-trade
            cpi_tradable_weight: [0.0, 0.0, 1.0, 1.0, 0.3, 0.0, 0.0, 0.0, 0.0],
            // fix-trade
            world_energy_oil_elasticity: 1.0,
            // p1-energy: motor fuel is about half of the energy index; $0.57 taxes + $1.0 margin + 1.12 × $65/42 gives a
            // pump price of $3.30 at $65 WTI and the US level of $3.26 (EIA, 2024) at the reported start, where the
            // non-energy index is at its burn-in reference (`common::fuel_margin_ref`).
            energy_fuel_split: true,
            pump_expectations: true,
            energy_fuel_share: 0.5,
            gas_tax: 0.57,
            gas_margin: 1.0,
            gas_crude_yield: 1.12,
            util_oil_elasticity: 0.1,
            pump_salience: 0.004,
            hh_pump_real_rate_weight: 1.0,
            firm_pump_user_cost_weight: 1.0,

            // ≈3% of the stock vacant and available (Census HVS, excluding seasonal/other vacant)
            houses_per_household: 1.03,
            median_house_price: 410_000.0,
            gross_rental_yield: 0.045,
            property_tax: 0.011,
            maintenance: 0.01,
            // FHA-style first-time-buyer limit (3.5% down)
            max_ltv: 0.965,
            max_ltv_investor: 0.75,
            max_dti: 0.31,
            mortgage_term_months: 360,
            construction_months: 8,
            construction_units_per_house: 250_000.0,
            land_share: 0.25,
            // Gross additions to the stock at balance: ≈0.8%/yr of the stock (≈0.00068 a month). Household growth
            // is ≈0.55–0.65%/yr (HH_FORMATION_ANNUAL; Census CPS/HVS household counts) and withdrawals ≈0.2–0.3M units
            // a year (Census/AHS units lost, ≈0.2%/yr of stock), so starts ≈ 1.1–1.2M a year, not the 1.35M
            // of the US starts series, which includes replacement. Lower than the former 0.00085: at 0.00085 the
            // stock outgrew households and rental vacancy rose without limit.
            base_starts_monthly_share: 0.00068,
            burn_in_hold_stock: true,
            rent_vacancy_hvs: true,
            rent_income_pass: 0.5,

            // deposit beta ≈ 0.5 over the 2022–23 tightening cycle
            deposit_beta: 0.5,
            mortgage_spread: 0.017,
            // 1.4% base: the lender premium averages ≈+0.6pp (it is bounded at −0.5pp below), so the mean business spread
            // over 2–5y Treasuries stays ≈2.2%.
            business_spread: 0.014,
            capital_requirement: 0.105,
            bank_payout_ratio: 0.5,
            bank_loss_carry: true,
            discount_spread: 0.001,
            ebp_loss_sens: 3.0,
            ebp_capital_sens: 1.0,
            ebp_capital_buffer: 0.02,
            ebp_adjust: 0.25,
            ebp_min: -0.005,
            ebp_max: 0.04,
            loss_norm_adapt: 0.01,
            loss_norm_initial: 0.005,
            pd_intercept: -5.5,
            pd_leverage: 4.0,
            pd_coverage: 1.0,
            pd_lgd: 0.6,
            standards_leverage_per_pp: 0.05,
            standards_coverage_per_pp: 0.05,
            business_default_months: 6,

            inflation_target: 0.02,
            r_star: 0.0075,
            r_star_adapt: 0.0,
            r_star_antiwindup: false,
            r_star_est_min: -0.01,
            // the model's frictional rate (separations / (separations + job finding)), close to CBO's ≈4.4%
            natural_unemployment: 0.05,
            phi_pi: 0.5,
            // Taylor (1993): 0.5 on the output gap ≈ 1pp of rate per 1pp unemployment gap (Okun 2:1)
            phi_u: 0.5,
            rate_smoothing: 0.9,
            initial_policy_rate: 0.0375,
            rule_uses_core: true,
            rule_uses_pce: true,
            // term premia ≈ ACM (NY Fed) estimates for 2025: 10y ≈ 0.7–1.0%
            tp_base: [0.0, 0.0015, 0.005, 0.010, 0.014],
            // term-premium sensitivity to log private duration supply (QE ≈3% of GDP ≈ −15 to −20bp on the 10y)
            tp_duration_kappa: 0.025,
            initial_curve: [0.038, 0.035, 0.037, 0.041, 0.047],
            erp: 0.035,
            long_growth: 0.039,
            equity_rate_months: 6.0,
            erp_unemp_sens: 0.0,
            // p1-equity: off in the pre-re-fit state (scripts/profiles/legacy_p1off.json); on in the defaults since the
            // re-fit. scripts/profiles/p1-equity.json is the historical overlay (docs/CALIBRATION.md "p1-equity").
            erp_fin_loading: 0.0,
            erp_fin_build_months: 0.0,
            rp_fin_loading: 0.0,
            erp_rp_loading: 8.75,
            erp_rp_gap_scale: 0.0,
            erp_slow_loading: 2.0,
            erp_slow_decay: 0.98,
            fund_wealth_ewma: false,
            tech_disc_news_phase: true,
            equity_cap_floor: 0.02,
            equity_cap_softness: 0.0,
            equity_valuation_log: true,
            erp_extrap: 0.005,
            erp_return_anchor: 0.08,
            erp_momentum_cap: 0.20,
            erp_min: 0.01,
            erp_max: 0.09,
            // fix-monetary
            qe_announcement_weight: 0.5,
            policy_tp_response: 0.2,
            q_signal_decay: 0.9,
            mbs_spread_per_gdp: 0.05,
            // p1-curve
            speed_limit_weight: 0.0,
            lr_infl_gain: 0.02,
            rule_path_weight: 0.0,
            i_star_gain: 0.6,
            rule_path_decay: 0.97,
            curve_report: true,

            // one tech revolution every ≈20 years on average, lasting ≈6–8 years (1995–2004 IT; AI)
            tech_boom_enabled: true,
            // innovation rents: Tech's norm markup rises by up to 0.3 during a boom (software and platform margins)
            tech_boom_markup_bonus: 0.3,
            tech_boom_extra_growth: 0.05,
            tech_boom_spillover: 0.01,
            tech_boom_entry_monthly: 1.0 / 240.0,
            tech_boom_exit_monthly: 1.0 / 96.0,
            tech_boom_erp_discount: 0.015,
            erp_sector: [0.008; N_SECTORS],
            sector_growth_extrapolation: 0.3,
            sector_growth_decay: 0.99,
            sector_growth_min: -0.02,
            sector_growth_max: 0.09,
            sector_productivity_news: 1.0,
            sector_news_decay: 0.98,
            // optimism unwinds within quarters after a boom (Nasdaq 2000–02: most of the −78% within 12 months)
            sector_news_decay_bust: 0.85,
            investment_q_sens: [0.0, 0.0, 0.0, 0.0, 0.2, 0.0, 0.0, 0.0, 0.0],
            // Tech firms extrapolate two years of recent excess demand growth (Gennaioli, Ma & Shleifer 2016)
            investment_growth_horizon_years: [0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0],

            // ≈ government consumption + investment (17%) + in-kind health (≈3–7%), as a share of BEA-style GDP
            gov_purchases_share: 0.20,
            tax_brackets: vec![
                (11_600.0, 0.10),
                (47_150.0, 0.12),
                (100_525.0, 0.22),
                (191_950.0, 0.24),
                (243_725.0, 0.32),
                (609_350.0, 0.35),
                (1.0e15, 0.37),
            ],
            // state/local: ≈1.5% income tax plus a 5% sales tax (US states rely on both)
            state_income_tax: 0.015,
            standard_deduction: 15_000.0,
            payroll_tax: 0.0765,
            // employer health premiums ≈ 10% of wages (BLS ECEC: insurance ≈ 7–8% of compensation)
            employer_benefits: 0.10,
            payroll_cap_annual: 176_100.0,
            // effective rate on book profits (US corporate receipts ≈ 1.7% of GDP; statutory 21%)
            corporate_tax: 0.13,
            entrant_loss_cf_reset: true,
            labor_share_pull: 0.0,
            inventory_adjust: 0.12,
            demand_adapt: 0.4,
            layoff_trigger: 1.08,
            layoff_speed: 0.4,
            markup_demand_pull: 0.0,
            wage_norm_target_weight: 0.0,
            firm_nowcast_weight: 0.5,
            wage_unfilled_bonus: 0.001,
            wage_layoff_cut: 0.001,
            wage_signal_relative: true,
            target_return_pass: 1.0,
            business_property_tax: 0.012,
            sales_tax: 0.05,
            auction_shares: [0.30, 0.45, 0.25],
            initial_debt_gdp: 1.0,
            // Calibration (round 7 Checkpoint A): 0.015 gave deficit 7.7 and debt 169 at year 20; 0.0125 per the spec rule.
            debt_target_slope: 0.0125,
            fiscal_rule_strength: 0.5,
            housing_first_budget: true,
            consumer_credit_months: 0.0,
            early_retiree_pia_factor: 0.70,
            first_buy_continuous_employment: true,
            burn_in_freeze_foreclosures: true,
            u_star_max: 0.044,
            u_star_min: 0.042,
            // Re-fit Step 9 is committed off by default (legacy-neutral): recruiting capacity returns later with a re-anchored wage curve.
            vacancy_net_share: 1.0,
            vacancy_replace: false,
            // b4-asym (legacy-neutral)
            vacancy_rounding: 0,
            vacancy_free_units: 0.0,
            vacancy_young_exempt_months: 0,
            rent_floor_indexed: true,
            // fix-fiscal
            ssi_max_single: 967.0,
            ssi_max_couple: 1_450.0,
            ssi_asset_limit: 3_000.0,
            snap_max_single: 298.0,
            snap_max_couple: 546.0,
            snap_phaseout: 0.30,
            // Round 7 step 2: SNAP asset test off (broad-based categorical eligibility, about 44 states).
            snap_asset_limit: 1.0e12,
            snap_asset_end: 1.0e12,
            snap_student_discount: 0.5,
            medicare_share: 0.55,
            eitc_rate: 0.40,
            eitc_plateau: 1_490.0,
            eitc_phaseout_start: 1_946.0,
            eitc_phaseout_rate: 0.2106,
            cap_gains_tax: 0.18,
            fiscal_rule_avg_months: 36.0,
            issuance_feedback: 0.02,
            // fix-fiscal: Treasury profile (WAM about 5-6 years, 30y at least 18%). Uniform vintages give WAM about 56
            // months; growing debt adds about 10 (measured WAM 65 on 10k households).
            maturity_stock_mix: [0.16, 0.14, 0.28, 0.22, 0.20],
            // fix-fiscal: about 35% of unrealised Fund gains realised a year (household equity turnover). Measured mean
            // realised CGT over 6 seeds, 10k households: 0.21% of GDP at 10%, 0.38% at 35% (heavy-tailed across seeds).
            cap_gains_realisation: 0.35,
            cb_bonds_gdp: 0.15,
            reserves_gdp: 0.10,

            initial_unemployment: 0.043,
            initial_inflation: 0.028,
            median_earnings_annual: 75_000.0,

            // fix-labour
            separation_mult_by_edu: [1.456, 0.473, 0.337],
            ui_search_cut: 0.5,
            ui_exhaustion_boost: 0.3,
            duration_contact_decay: 0.90,
            duration_contact_cap: 12.0,
            nonparticipant_contact: 0.5,
            applications_nonparticipant: 2,
            match_wage_dispersion: 0.03,
            search_effort_norm: 0.5,
            on_the_job_search_mult: 16.0,
            job_to_job_premium: 1.04,
            unemp_exit_base: 0.10,
            unemp_exit_duration: 0.0,
            unemp_exit_exhaust: 0.0,
            unemp_exit_cyc: -0.5,
            // p1-fx: inert while fx_psi_sigma = 0 with no trade lag (the pre-re-fit state, legacy_p1off.json). The
            // defaults carry the trade lag and long-run elasticities below.
            fx_psi_weight: 0.28,
            export_elasticity_sr: 0.3,
            export_elasticity_lr: Some(1.0),
            export_lag_speed: 0.08,
            import_elasticity_lr: Some(0.6),
            import_lag_speed: 0.08,
            // p1-distquick: current defaults (on since the re-fit; pre-re-fit values in legacy_p1off.json)
            distquick_entrant_skill_sigma: 0.45,
            distquick_top10_nw_target: 0.72,
            distquick_kink_wage_indexed: true,
            distquick_heir_match: true,
            distquick_estate_heirs: 1,
            // p1-firmsentry: current defaults (on since the re-fit; see the field docs and legacy_p1off.json).
            entrant_customer_floor: 3.0,
            entrant_intro_discount: 0.05,
            entrant_intro_months: 12,
            entrant_tie_switch: true,
            entrant_startup_months: 12.0,
            entrant_productivity_sigma: 0.2,
            entry_gate_slope: 40.0,
            entry_gate_hurdle: 0.03,
            entry_reborn_max_share: 0.002,
            entry_labour_force: true,
            owner_founders: true,
            firm_fixed_cost_share: 0.0,
            merton_cost_of_capital: 0.1,
            merton_q_min: 0.2,
            merton_q_max: 2.0,
            merton_recovery_floor: 0.7,
            firm_prod_sigma: 0.05,
            firm_prod_revert: 0.01,
            // b5-firmhet S1 (all off at legacy values; see the field docs).
            entrant_capital_floor_workers: 0.0,
            inv_expand_labour_ref: false,
            entry_gate_capital_weighted: false,
            entry_contestability_share: 0.0,
            entry_gate_cyclical: false,
            entry_gate_base_prob: 0.5,
            entry_gate_cyc_slope: 300.0,
            entry_gate_anchor_rate: 0.0115,
            entry_gate_level_slope: 0.0,
            entry_gate_u_beta: 0.0,
            demand_logit: false,
            demand_elasticity: 4.5,
            demand_rechoice_prob: 0.03,
            appeal_sigma: 0.03,
            appeal_zeta: 1.1,
            entrant_appeal_ln: 0.0,
            markup_size_kappa: 0.0,
            markup_size_comp_max: 0.0,
            markup_size_q_min: 1.0,
            inv_lumpy: false,
            inv_inaction_band: 0.40,
            inv_catchup_band: 0.0,
            inv_catchup_months: 12.0,
            inv_catchup_cap: 0.05,

            // fix-housing
            housing_uc_theta: 0.75,
            refi_threshold_pp: 0.75,
            refi_prob_per_pp: 0.04,
            refi_prob_cap: 0.09,
            construction_rate_elasticity: 0.16,
            housing_foreclosure_prob: 0.45,
            housing_distress_sale_prob: 0.10,
            housing_reo_nonaccrual: true,
            housing_reo_relist: true,
            housing_exit_uc_elasticity: 2.0,
            housing_price_uc_gain: 0.9,
            housing_rent_price_anchor: 0.08,
            housing_affordability_sens: 0.2,
            housing_income_miss_prob: 0.7,
            // Liquidity grading is OFF by default (1e6 months: every owner in an episode, or unemployed, with LTV above
            // housing_shock_ltv misses at the full probability). The grade reads deposits only (banking.rs
            // income_shock_miss_prob). It cannot bind for this population at realistic thresholds: with deposits only,
            // 3/1 months gives about 12k foreclosures a year, 6/2 about 11k, 24/6 about 24k, 36/12 about 52k (8 seeds,
            // m12+), against about 216k a year when off. Households hold far more deposits than the thresholds.
            housing_shock_liquidity_months: 1e6,
            housing_shock_full_miss_months: 1e6,
            housing_idio_persistence: 0.99,
            housing_idio_innovation_sd: 0.026,
            housing_shock_hazard: 0.003,
            housing_shock_episode_months: 12,
            housing_shock_ltv: 0.9,
            housing_shock_episode_miss_prob: 0.7,
            housing_shock_underwater_mult: 3.0,
            // fix-housing
            housing_shock_u_elasticity: 2.0,
            // Re-fit Step 4 (accounting and calibration fixes); legacy values in scripts/profiles/legacy_p1off.json.
            bank_tax_after_opex: true,
            founder_wage_hc: true,
            upstream_tax_share: 0.08,
            upstream_royalty_share: 0.10,
            upstream_margin: 0.05,
            // Accounting fixes of 2026-10-20 to the real-series deflators and the stock-flow check; legacy values in
            // scripts/profiles/legacy_acctfix_off.json.
            paasche_deflators: true,
            sfc_enforce: true,
            // Re-fit Step 5 (bankruptcy realism); legacy values in scripts/profiles/legacy_p1off.json.
            cipi_excludes_writeoffs: true,
            cipi_excludes_nonstorable: true,
            firm_order_pool: false,
            distress_default_hazard: 1.0 / 6.0,
            // Re-fit Step 6 (native entry tracks dissolutions); legacy values in scripts/profiles/legacy_p1off.json.
            native_entry_tracks_dissolutions: true,
            hh_formation_annual: 0.0030,
            trend_labor_force_growth: TREND_LABOR_FORCE_GROWTH,
            // b4-rent: off (legacy values; see the field docs).
            housing_buy_rate_semi: 0.0,
            housing_vacant_sell_reanchor: false,
            housing_vacant_sell_ms_elast: 1.0,
            housing_rental_gap_sell_semi: 0.0,
            housing_expiry_let_gap_semi: 0.0,

            // b5-udur (off; legacy-equivalent values in the comments above)
            udur_contact_on: false,
            udur_contact_d1: 0.92,
            udur_contact_m1: 12.0,
            udur_contact_d2: 1.0,
            udur_contact_m2: 12.0,
            udur_effort_norm: 0.5,
            udur_rank_kappa: 0.0,
            udur_rank_cap: 24.0,
            udur_ui_ext_t1: 99.0,
            udur_ui_ext_t2: 99.0,
            udur_ui_ext_m1: 0.0,
            udur_ui_ext_m2: 0.0,
            udur_ui_ext_search_scale: 1.0,
            udur_hc_loss: 0.0,
            udur_hc_loss_max: 0.2,
            udur_hc_recover: 0.05,
            udur_exit_base_shift: 0.0,
            udur_exit_dur: 0.0,
            udur_exit_dur_months: 12.0,
            udur_resv_decay: 0.97,
            // b6-reentry (off; legacy-neutral values)
            udur_reentry_recent: 0.0,
            udur_reentry_window: 3,
            udur_resume_spell: false,
            udur_reentry_scar: 1.0,
            udur_reentry_scar_spell: 12,
            udur_att_on: false,
            udur_att_spell: 6,
            udur_att_scar: 1.0,
            udur_att_tau: 12.0,
            udur_att_eta: 0.0,
            udur_att_base_mult: 1.0,
            // --- b7-tariff ---
            tariff_pass_through: true,
            tariff_retail_speed: 0.075,
            tariff_retail_long: 1.0,
            tariff_share_elasticity_sr: 0.30,
            tariff_share_elasticity_lr: 1.2,
            tariff_share_lag_speed: 0.03,

            // b7-transmission
            qe_duration_weight: 1.5,
            fx_mp_premium: 1.25,
            fx_mp_cap: 0.03,
            firm_mp_expect: 0.0,
            firm_mp_expect_cap: 0.03,
            firm_stance_weight: 0.0,
            firm_stance_cap: 0.01,
            firm_stance_months: 60.0,
            disc_rate_knee: 0.005,
            disc_rate_sat_scale: 0.005,
            disc_rate_sat_slack: 1.0,
            fx_rate_knee: 0.02,
            fx_rate_sat_scale: 0.0,
            pin_path_on: true,
            pin_exit_hazard: 0.0417,
            pin_exit_decay: 0.9,
            // b8-world (stage 1)
            world_block_enabled: false,
            world_uae_in_opec: true,
            world_shock_scale: 1.0,
            world_theta_mult: 1.0,
            world_lam_rp_mult: 1.0,
            world_kus_mult: 1.0,
            world_oil_is_mult: 1.0,
            world_rho_f: 0.93,
            world_rho_p: 0.95,
            world_rho_oil: 0.98,
            world_rho_qty: 0.97,
            world_okun_ratio: 2.0,
            world_ius_offset: -0.43,
            world_yus_offset: -0.90,
            world_opec_va_share: 0.20,
            world_link_trade: false,
            world_us_import_channel: true,
            world_origin_sigma_sr: 0.5,
            world_origin_sigma_lr: 2.0,
            world_origin_speed: 0.05,
            world_dcp_kappa_usd: 0.47,
            world_dcp_speed_usd: 0.08,
            world_dcp_kappa_nonusd: 0.95,
            world_dcp_speed_nonusd: 0.9,
            world_export_income_elast: 1.4,
            world_export_price_sr: 0.10,
            world_export_price_lr: 0.90,
            world_export_fx_sr: 0.15,
            world_export_fx_lr: 0.30,
            world_export_speed: 0.04,
            world_link_oil: false,
            world_oil_profile: "default".to_string(),
            world_opec_regime: "price_defence".to_string(),
            world_oil_exact_path: true,
            world_oil_feedback: true,
            world_oil_mean_offset: 8.355,
            world_oil_sigma_mult: 1.0,
            world_oil_rho_q: 0.99,
            world_oil_sig_i_mult: 1.0,
            world_oil_shock_mask: "IDO".to_string(),
            world_oil_qty_offset: -6.627,
            world_oilcheck_activity_sigma: 1.6657,
            world_oilcheck_activity_rho: 0.9943,
            world_oil_splice_rho: -1.0,
            world_opec_uae_spare_share: 0.30,
            world_link_dollar: false,
            world_vix_per_unit: 10.0,
            world_vix_per_100bp: 13.0,
            world_fx_risk_mult: 1.0,
            world_fx_risk_rho: 0.9,
            world_fx_risk_asym: 0.588,
            world_fx_rp_sd_mult: 1.0,
            world_fx_idio_sd_ae: 1.5,
            world_fx_idio_sd_eme: 2.0,
            world_fx_idio_sd_cn: 0.3,
            world_fx_idio_sd_opec: 0.3,
            world_fx_idio_rho: 0.99,
            world_theta_f_ratio: 0.5,
            world_china_risk_pts_per_pct: 6.0,
            world_gdp_rw_bn: 32560.0,
            world_dollar_neutral: true,
            world_rp_sat_knee: 0.02,
            world_rp_sat_scale: 0.015,
            world_link_finance: false,
            world_fo_theta_stock: 13.0,
            world_fo_theta_trans: 20.0,
            world_fo_trans_rho: 0.944,
            world_fo_dollar_resp: -0.0023,
            world_fo_vix_resp: 1.05,
            world_fo_trend_cn: -80.0,
            world_retaliation: false,
            world_ret_eps_sr: 1.0,
            world_ret_eps_lr: 3.0,
            world_ret_speed: 0.10,
            world_positions: false,
            world_import_gap_offset: Vec::new(),
            world_import_rel_drift: 0.0,

            // b9-rates
            business_policy_weight: 0.0,
            business_policy_offset: 0.0,
            mortgage_spread_vol_gain: 0.0,
            mortgage_spread_stress_gain: 0.0,
            mortgage_spread_vol_months: 6.0,
            mortgage_spread_norm_months: 120.0,
            mortgage_spread_vol_ref: 0.0012,
            mortgage_spread_stress_ref: 0.0031,
            mortgage_spread_min_adj: -0.006,
            mortgage_spread_max_adj: 0.016,

            // b9-oil
            user_cost_pi_e_mode: 0,
            user_cost_pi_e_offset: 0.0,
            user_cost_pi_e_months: 24.0,

            // b9-inflation
            hh_mp_expect: 0.0,
            hh_mp_expect_cap: 0.03,
            markup_dev_pull: 0.0,
            markup_dev_decay: 0.02,
            markup_dev_max: 0.08,
            markup_dev_gap_ref: 0.0,
            markup_dev_norm_months: 120.0,

            // b9-warmup
            warmup_months: 0,

            // us-now
            start_state: crate::start::StartState::Typical,
            start_targets: None,

            // rz1
            rate_perception_deposit: false,
            credit_tail_half_life_months: 0.0,
            fisim_constant_margin: false,
            rate_perception_firm_months: -1.0,

            // rz12
            rate_perception_stages: 1,
            rate_perception_firm_stages: -1,
            rate_perception_credit_bypass: 0.0,

            // rz19
            tariff_tfp_loss: 0.0,
            tariff_tfp_phase_in_months: 60.0,

            // rz21
            rule_lookthrough: 0.0,

            // rz20. The general form (tail weight, investment_q_sens_all, ebp_equity_sens) stays off: it failed its guards. The re-rating form is on
            // (adopted 2026-10-23 in v2026.10.23.3; docs/CALIBRATION.md, "RZ20"): a SharePrices re-rating moves spending (2.5 cents a year per dollar,
            // Chodorow-Reich, Nenov & Simsek 2021), investment through q (0.16, Barro 1990; Blanchard, Rhee & Summers 1993) and the credit premium (0.024,
            // Gilchrist & Zakrajsek 2012), with the wealth term smoothed over 24 months. A run with no SharePrices re-rating is unchanged.
            fund_wealth_tail_weight: 0.35,
            fund_wealth_ewma_span: 24.0,
            investment_q_sens_all: 0.0,
            ebp_equity_sens: 0.0,
            rerate_wealth_mpc_annual: 0.025,
            rerate_q_sens: 0.16,
            rerate_ebp_sens: 0.024,

            // rz24
            credit_shock_erp: 0.0,
            // rz27: off (legacy). The elasticity is the registered calibration (Kanzig 2021, Figure A.8); it does nothing while the switch is off.
            energy_equity_oil_on: false,
            energy_equity_oil_elasticity: 0.24,
        }
    }
}

/// Trend labour-force growth per year used for trend GDP (`Config::trend_labor_force_growth`). Re-fit Step 6: the measured
/// labour-force growth of the Step 6 model (16 seeds, months 12–239, 0.27–0.31%/yr; the 0.4% CBO projection, 2026–36,
/// differed from it by more than 0.1pp). The pre-re-fit value 0.004 is in scripts/profiles/legacy_p1off.json.
pub const TREND_LABOR_FORCE_GROWTH: f64 = 0.00274;

impl Config {
    /// Default config with an optional JSON override from `ECONSIM_CFG` (for sweeps and diagnostics), e.g.
    /// `ECONSIM_CFG='{"labor_share_pull":0.01}'`. Panics on invalid JSON or unknown keys, so typos are not ignored.
    pub fn from_env() -> Config {
        let Ok(js) = std::env::var("ECONSIM_CFG") else { return Config::default() };
        let mut merged = serde_json::to_value(Config::default()).expect("Config serialises");
        let over: serde_json::Value = serde_json::from_str(&js).expect("ECONSIM_CFG is not valid JSON");
        if let (Some(m), Some(o)) = (merged.as_object_mut(), over.as_object()) {
            for (k, v) in o {
                assert!(m.contains_key(k), "unknown config key {k}");
                m.insert(k.clone(), v.clone());
            }
        }
        let cfg: Config = serde_json::from_value(merged).expect("ECONSIM_CFG does not fit Config");
        if let Err(e) = cfg.validate_rz1() {
            panic!("ECONSIM_CFG: {e}");
        }
        cfg
    }

    pub fn effective_scale(&self) -> f64 {
        if self.scale > 0.0 {
            self.scale
        } else {
            132.0e6 / self.n_households.max(1) as f64
        }
    }

    /// Nominal trend growth per year (trend GDP and central-bank balance-sheet target): inflation target plus
    /// productivity growth plus trend labour-force growth.
    pub fn nominal_trend_growth(&self) -> f64 {
        self.inflation_target + self.tfp_growth_annual + self.trend_labor_force_growth
    }
}

#[cfg(test)]
mod rz1_tests {
    use super::*;

    #[test]
    fn rz1_keys_are_off_by_default_and_serialise() {
        let c = Config::default();
        assert!(!c.rate_perception_deposit);
        assert_eq!(c.credit_tail_half_life_months, 0.0);
        assert!(!c.fisim_constant_margin);
        assert_eq!(c.rate_perception_firm_months, -1.0);
        assert_eq!(c.rate_perception_months, 0.0);
        assert_eq!(c.kstar_adapt, 0.0);
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["rate_perception_deposit"], serde_json::json!(false));
        assert_eq!(v["credit_tail_half_life_months"], serde_json::json!(0.0));
    }

    #[test]
    fn rz1_validation_ranges() {
        let with = |lag: f64, hl: f64| Config { rate_perception_months: lag, credit_tail_half_life_months: hl, ..Config::default() };
        for (lag, hl) in [(0.0, 0.0), (9.0, 9.0), (36.0, 60.0), (6.0, 0.5)] {
            assert!(with(lag, hl).validate_rz1().is_ok(), "{lag} {hl}");
        }
        for (lag, hl) in [(-0.1, 0.0), (36.1, 0.0), (f64::NAN, 0.0), (f64::INFINITY, 0.0), (0.0, -0.1), (0.0, 60.1), (0.0, f64::NAN)] {
            assert!(with(lag, hl).validate_rz1().is_err(), "{lag} {hl}");
        }
        for fl in [-1.0, -0.5, 0.0, 4.0, 36.0] {
            assert!(Config { rate_perception_firm_months: fl, ..Config::default() }.validate_rz1().is_ok(), "firm lag {fl}");
        }
        for fl in [-1.1, 36.1, f64::NAN, f64::INFINITY] {
            assert!(Config { rate_perception_firm_months: fl, ..Config::default() }.validate_rz1().is_err(), "firm lag {fl}");
        }
        for ka in [0.0, 0.05, 1.0] {
            assert!(Config { kstar_adapt: ka, ..Config::default() }.validate_rz1().is_ok(), "kstar_adapt {ka}");
        }
        for ka in [-0.01, 1.01, f64::NAN] {
            assert!(Config { kstar_adapt: ka, ..Config::default() }.validate_rz1().is_err(), "kstar_adapt {ka}");
        }
    }
}

#[cfg(test)]
mod rz2_tests {
    use super::Config;

    #[test]
    fn rz2_key_is_off_by_default_and_serialises() {
        let c = Config::default();
        assert_eq!(c.hh_pump_real_rate_weight, 1.0);
        assert_eq!(c.firm_pump_user_cost_weight, 1.0);
        let js = serde_json::to_value(&c).unwrap();
        assert_eq!(js["hh_pump_real_rate_weight"], serde_json::json!(1.0));
        // the oil switches that RZ2 re-uses keep their legacy defaults
        assert_eq!((c.energy_extraction_labor_share, c.energy_extraction_elasticity), (0.35, 1.7));
        assert_eq!((c.user_cost_pi_e_mode, c.user_cost_pi_e_offset, c.user_cost_pi_e_months), (0, 0.0, 24.0));
        assert!(c.validate_rz2().is_ok());
    }

    #[test]
    fn rz2_validation_ranges() {
        let ok = |f: &dyn Fn(&mut Config)| {
            let mut c = Config::default();
            f(&mut c);
            c.validate_rz2().is_ok()
        };
        // accepted, edges included
        assert!(ok(&|c| c.hh_pump_real_rate_weight = 0.0));
        assert!(ok(&|c| c.hh_pump_real_rate_weight = 1.0));
        assert!(ok(&|c| c.firm_pump_user_cost_weight = 0.0));
        assert!(ok(&|c| c.firm_pump_user_cost_weight = 1.0));
        assert!(!ok(&|c| c.firm_pump_user_cost_weight = 1.2));
        assert!(!ok(&|c| c.firm_pump_user_cost_weight = f64::NAN));
        assert!(ok(&|c| c.energy_extraction_labor_share = 0.0));
        assert!(ok(&|c| c.energy_extraction_labor_share = 1.0));
        assert!(ok(&|c| c.energy_extraction_elasticity = 4.0));
        assert!(ok(&|c| c.user_cost_pi_e_mode = 3));
        assert!(ok(&|c| c.user_cost_pi_e_offset = -0.02));
        assert!(ok(&|c| c.user_cost_pi_e_months = 120.0));
        // refused
        assert!(!ok(&|c| c.hh_pump_real_rate_weight = 1.0001));
        assert!(!ok(&|c| c.hh_pump_real_rate_weight = -0.0001));
        assert!(!ok(&|c| c.hh_pump_real_rate_weight = f64::NAN));
        assert!(!ok(&|c| c.energy_extraction_labor_share = 1.01));
        assert!(!ok(&|c| c.energy_extraction_labor_share = -0.1));
        assert!(!ok(&|c| c.energy_extraction_elasticity = 4.01));
        assert!(!ok(&|c| c.energy_extraction_elasticity = f64::INFINITY));
        assert!(!ok(&|c| c.user_cost_pi_e_mode = 4));
        assert!(!ok(&|c| c.user_cost_pi_e_offset = 0.0201));
        assert!(!ok(&|c| c.user_cost_pi_e_months = 120.5));
        assert!(!ok(&|c| c.user_cost_pi_e_months = -1.0));
        // the message starts with the key name
        let mut c = Config::default();
        c.hh_pump_real_rate_weight = 2.0;
        assert!(c.validate_rz2().unwrap_err().starts_with("hh_pump_real_rate_weight"));
    }
}

#[cfg(test)]
mod rz12_tests {
    use super::*;

    #[test]
    fn rz12_keys_are_off_by_default_and_serialise() {
        let c = Config::default();
        assert_eq!(c.rate_perception_stages, 1);
        assert_eq!(c.rate_perception_firm_stages, -1);
        assert_eq!(c.rate_perception_credit_bypass, 0.0);
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["rate_perception_stages"], serde_json::json!(1));
        assert_eq!(v["rate_perception_firm_stages"], serde_json::json!(-1));
    }

    #[test]
    fn rz12_stage_validation_ranges() {
        for s in [1u32, 2, 3, 4] {
            assert!(Config { rate_perception_stages: s, ..Config::default() }.validate_rz1().is_ok(), "stages {s}");
        }
        for s in [0u32, 5, 100] {
            let e = Config { rate_perception_stages: s, ..Config::default() }.validate_rz1().unwrap_err();
            assert!(e.starts_with("rate_perception_stages"), "{e}");
        }
        for s in [-1i32, 1, 2, 3, 4] {
            assert!(Config { rate_perception_firm_stages: s, ..Config::default() }.validate_rz1().is_ok(), "firm stages {s}");
        }
        for s in [-2i32, 0, 5] {
            let e = Config { rate_perception_firm_stages: s, ..Config::default() }.validate_rz1().unwrap_err();
            assert!(e.starts_with("rate_perception_firm_stages"), "{e}");
        }
        for b in [0.0, 0.5, 1.0] {
            assert!(Config { rate_perception_credit_bypass: b, ..Config::default() }.validate_rz1().is_ok(), "bypass {b}");
        }
        for b in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
            let e = Config { rate_perception_credit_bypass: b, ..Config::default() }.validate_rz1().unwrap_err();
            assert!(e.starts_with("rate_perception_credit_bypass"), "{e}");
        }
    }
}

// rz13: c3 = c2 with three existing keys at new values (`phi_u` 0.5 -> 0.25, `speed_limit_weight` 0.5 -> 0.75, `credit_tail_half_life_months` 5 -> 4). There is no new key and no
// new code path; these tests pin the profile (what c3 changes, nothing else) and the identity of the defaults.
#[cfg(test)]
mod rz13_tests {
    use super::*;

    /// The c2 profile (a candidate profile of the realism study, md5 0d59ebfdf0d0438e4d37b226d18c817d) and the three values c3 changes.
    const C2: &str = r#"{"rate_perception_months": 11.0, "rate_perception_firm_months": 7.0, "rate_perception_stages": 3, "rate_perception_deposit": true,
        "rate_perception_credit_bypass": 0.7, "credit_tail_half_life_months": 5.0, "fisim_constant_margin": true, "rate_smoothing": 0.88,
        "speed_limit_weight": 0.5, "energy_extraction_labor_share": 0.11, "energy_extraction_elasticity": 1.0, "user_cost_pi_e_mode": 2,
        "user_cost_pi_e_offset": 0.0035, "hh_pump_real_rate_weight": 0.0}"#;
    const C3_CHANGES: &str = r#"{"phi_u": 0.25, "speed_limit_weight": 0.75, "credit_tail_half_life_months": 4.0}"#;

    /// `Config::from_env`'s merge, without the environment variable.
    fn merged(overlays: &[&str]) -> Config {
        let mut m = serde_json::to_value(Config::default()).unwrap();
        for js in overlays {
            let o: serde_json::Value = serde_json::from_str(js).unwrap();
            for (k, v) in o.as_object().unwrap() {
                assert!(m.as_object().unwrap().contains_key(k), "unknown config key {k}");
                m.as_object_mut().unwrap().insert(k.clone(), v.clone());
            }
        }
        serde_json::from_value(m).unwrap()
    }

    #[test]
    fn c3_is_c2_with_three_existing_keys_at_new_values() {
        let (d, c2, c3) = (Config::default(), merged(&[C2]), merged(&[C2, C3_CHANGES]));
        // the defaults of the three keys are untouched (the default fingerprint is the other half of this proof)
        assert_eq!((d.phi_u, d.speed_limit_weight, d.credit_tail_half_life_months), (0.5, 0.0, 0.0));
        assert_eq!((c2.phi_u, c2.speed_limit_weight, c2.credit_tail_half_life_months), (0.5, 0.5, 5.0));
        assert_eq!((c3.phi_u, c3.speed_limit_weight, c3.credit_tail_half_life_months), (0.25, 0.75, 4.0));
        // everything else is equal between c2 and c3, field by field
        let (a, b) = (serde_json::to_value(&c2).unwrap(), serde_json::to_value(&c3).unwrap());
        let mut diff: Vec<&String> = a.as_object().unwrap().keys().filter(|k| a[k.as_str()] != b[k.as_str()]).collect();
        diff.sort();
        assert_eq!(diff, vec!["credit_tail_half_life_months", "phi_u", "speed_limit_weight"], "c3 differs from c2 in these three keys only");
        assert!(c3.validate_rz1().is_ok() && c3.validate_rz2().is_ok());
    }
}

// rz19: the tariff efficiency wedge. Two new keys, both off by default: `tariff_tfp_loss` 0 (the wedge is never entered) and
// `tariff_tfp_phase_in_months` 60 (inert while the loss is 0). The default fingerprint is checked by the identity cases of the
// acceptance identity checks of the realism program, not here.
#[cfg(test)]
mod rz19_tests {
    use super::Config;

    #[test]
    fn rz19_keys_are_off_by_default_and_serialise() {
        let c = Config::default();
        assert_eq!(c.tariff_tfp_loss, 0.0);
        assert_eq!(c.tariff_tfp_phase_in_months, 60.0);
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["tariff_tfp_loss"], serde_json::json!(0.0));
        assert_eq!(v["tariff_tfp_phase_in_months"], serde_json::json!(60.0));
        // a request that names neither key gets the defaults; one that names them gets its values
        let over: Config = serde_json::from_str(r#"{"tariff_tfp_loss": 0.067, "tariff_tfp_phase_in_months": 36}"#).unwrap();
        assert_eq!((over.tariff_tfp_loss, over.tariff_tfp_phase_in_months), (0.067, 36.0));
        let none: Config = serde_json::from_str("{}").unwrap();
        assert_eq!((none.tariff_tfp_loss, none.tariff_tfp_phase_in_months), (0.0, 60.0));
    }

    #[test]
    fn rz19_validation_ranges() {
        let with = |l: f64, m: f64| Config { tariff_tfp_loss: l, tariff_tfp_phase_in_months: m, ..Config::default() };
        for (l, m) in [(0.0, 60.0), (0.067, 60.0), (2.0, 1.0), (0.5, 120.0), (0.0, 1.0)] {
            assert!(with(l, m).validate_rz1().is_ok(), "{l} {m}");
        }
        for (l, m) in [(-0.01, 60.0), (2.01, 60.0), (f64::NAN, 60.0), (f64::INFINITY, 60.0), (0.1, 0.99), (0.1, 120.1), (0.1, 0.0), (0.1, f64::NAN), (0.1, -5.0)] {
            assert!(with(l, m).validate_rz1().is_err(), "{l} {m}");
        }
    }
}

// rz21: the rule's look-through of a tariff's first round. One new key, off by default: `rule_lookthrough` 0 (nothing is computed). The default
// fingerprint is checked by the identity checks of the realism program's acceptance runs, not here.
#[cfg(test)]
mod rz21_tests {
    use super::Config;

    #[test]
    fn rz21_key_is_off_by_default_serialises_and_is_range_checked() {
        let c = Config::default();
        assert_eq!(c.rule_lookthrough, 0.0);
        assert!(c.validate_rz1().is_ok());
        assert_eq!(serde_json::to_value(&c).unwrap()["rule_lookthrough"], serde_json::json!(0.0));
        let over: Config = serde_json::from_str(r#"{"rule_lookthrough": 0.75}"#).unwrap();
        assert_eq!(over.rule_lookthrough, 0.75);
        let none: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(none.rule_lookthrough, 0.0);
        let with = |l: f64| Config { rule_lookthrough: l, ..Config::default() };
        for l in [0.0, 0.5, 1.0] {
            assert!(with(l).validate_rz1().is_ok(), "{l}");
        }
        for l in [-0.01, 1.01, f64::NAN, f64::INFINITY, -1.0] {
            assert!(with(l).validate_rz1().is_err(), "{l}");
        }
    }
}

// rz20: the equity channel. Three new keys, all off by default: `fund_wealth_tail_weight` 0.35 (the legacy constant), `investment_q_sens_all` 0 and
// `ebp_equity_sens` 0. The candidate also sets the existing `mpc_fund_wealth_annual` (0.02 -> 0.032) and `fund_wealth_ewma` (false -> true). The default
// fingerprint is checked by the identity checks of the realism program's acceptance runs, not here.
#[cfg(test)]
mod rz20_config_tests {
    use super::Config;

    #[test]
    fn rz20_keys_are_off_by_default_and_serialise() {
        let c = Config::default();
        assert_eq!((c.fund_wealth_tail_weight, c.investment_q_sens_all, c.ebp_equity_sens), (0.35, 0.0, 0.0));
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["fund_wealth_tail_weight"], serde_json::json!(0.35));
        assert_eq!(v["investment_q_sens_all"], serde_json::json!(0.0));
        assert_eq!(v["ebp_equity_sens"], serde_json::json!(0.0));
        assert_eq!(c.fund_wealth_ewma_span, 24.0);
        // The re-rating form is on by default since its adoption (v2026.10.23.3); it is inert without a SharePrices re-rating.
        assert_eq!((c.rerate_wealth_mpc_annual, c.rerate_q_sens, c.rerate_ebp_sens), (0.025, 0.16, 0.024));
        let over: Config = serde_json::from_str(r#"{"fund_wealth_tail_weight": 1, "fund_wealth_ewma_span": 24, "investment_q_sens_all": 0.16, "ebp_equity_sens": 0.024}"#).unwrap();
        assert_eq!((over.fund_wealth_tail_weight, over.fund_wealth_ewma_span, over.investment_q_sens_all, over.ebp_equity_sens), (1.0, 24.0, 0.16, 0.024));
        let none: Config = serde_json::from_str("{}").unwrap();
        assert_eq!((none.fund_wealth_tail_weight, none.investment_q_sens_all, none.ebp_equity_sens), (0.35, 0.0, 0.0));
    }

    #[test]
    fn rz20_validation_ranges() {
        let with = |t: f64, q: f64, e: f64| Config { fund_wealth_tail_weight: t, investment_q_sens_all: q, ebp_equity_sens: e, ..Config::default() };
        for (m, q, e) in [(0.0, 0.0, 0.0), (0.025, 0.16, 0.024), (0.2, 1.0, 0.05)] {
            assert!(Config { rerate_wealth_mpc_annual: m, rerate_q_sens: q, rerate_ebp_sens: e, ..Config::default() }.validate_rz1().is_ok(), "{m} {q} {e}");
        }
        for (m, q, e, name) in [(-0.01, 0.0, 0.0, "rerate_wealth_mpc_annual"), (0.21, 0.0, 0.0, "rerate_wealth_mpc_annual"), (0.0, -0.1, 0.0, "rerate_q_sens"), (0.0, 1.1, 0.0, "rerate_q_sens"),
                               (0.0, 0.0, -0.001, "rerate_ebp_sens"), (0.0, 0.0, 0.06, "rerate_ebp_sens"), (f64::NAN, 0.0, 0.0, "rerate_wealth_mpc_annual")] {
            let err = Config { rerate_wealth_mpc_annual: m, rerate_q_sens: q, rerate_ebp_sens: e, ..Config::default() }.validate_rz1().unwrap_err();
            assert!(err.starts_with(name), "{m} {q} {e}: {err}");
        }
        for sp in [2.0, 12.0, 24.0, 60.0] {
            assert!(Config { fund_wealth_ewma_span: sp, ..Config::default() }.validate_rz1().is_ok(), "span {sp}");
        }
        for sp in [1.99, 60.1, 0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert!(Config { fund_wealth_ewma_span: sp, ..Config::default() }.validate_rz1().unwrap_err().starts_with("fund_wealth_ewma_span"), "span {sp}");
        }
        for (t, q, e) in [(0.35, 0.0, 0.0), (1.0, 0.16, 0.024), (0.05, 1.0, 0.05), (0.5, 0.5, 0.0)] {
            assert!(with(t, q, e).validate_rz1().is_ok(), "{t} {q} {e}");
        }
        for (t, q, e) in [
            (0.049, 0.0, 0.0), (1.01, 0.0, 0.0), (f64::NAN, 0.0, 0.0), (0.0, 0.0, 0.0), (-0.1, 0.0, 0.0),
            (0.35, -0.01, 0.0), (0.35, 1.01, 0.0), (0.35, f64::INFINITY, 0.0),
            (0.35, 0.0, -0.001), (0.35, 0.0, 0.0501), (0.35, 0.0, f64::NAN),
        ] {
            assert!(with(t, q, e).validate_rz1().is_err(), "{t} {q} {e}");
        }
        assert!(with(0.01, 0.0, 0.0).validate_rz1().unwrap_err().starts_with("fund_wealth_tail_weight"));
        assert!(with(0.35, 2.0, 0.0).validate_rz1().unwrap_err().starts_with("investment_q_sens_all"));
        assert!(with(0.35, 0.0, 0.1).validate_rz1().unwrap_err().starts_with("ebp_equity_sens"));
    }
}

// rz24: the credit-tightening key. `credit_shock_erp` (0, off) acts only while a `CreditSpread` shock is in force. The default fingerprint is checked by
// the identity cases of the acceptance, not here.
#[cfg(test)]
mod rz24_tests {
    use super::Config;

    #[test]
    fn rz24_key_is_off_by_default_and_serialises() {
        let c = Config::default();
        assert_eq!(c.credit_shock_erp, 0.0);
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["credit_shock_erp"], serde_json::json!(0.0));
        let over: Config = serde_json::from_str(r#"{"credit_shock_erp": 0.56}"#).unwrap();
        assert_eq!(over.credit_shock_erp, 0.56);
        let none: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(none.credit_shock_erp, 0.0);
    }

    #[test]
    fn rz24_validation_ranges() {
        let with = |e: f64| Config { credit_shock_erp: e, ..Config::default() };
        for e in [0.0, 0.56, 5.0, 2.5, 1e-9] {
            assert!(with(e).validate_rz1().is_ok(), "{e}");
        }
        for e in [-0.01, 5.01, f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -5.0] {
            let err = with(e).validate_rz1().unwrap_err();
            assert!(err.starts_with("credit_shock_erp"), "{e}: {err}");
        }
    }
}

// rz27: the Energy equity's oil elasticity. Two new keys, off by default: `energy_equity_oil_on` false (nothing is computed or anchored). The default fingerprint is checked by
// the identity cases of the acceptance scripts, not here.
#[cfg(test)]
mod rz27_config_tests {
    use super::Config;

    #[test]
    fn rz27_keys_are_off_by_default_serialise_and_are_range_checked() {
        let c = Config::default();
        assert!(!c.energy_equity_oil_on);
        assert_eq!(c.energy_equity_oil_elasticity, 0.24);
        assert!(c.validate_rz1().is_ok());
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["energy_equity_oil_on"], serde_json::json!(false));
        assert_eq!(v["energy_equity_oil_elasticity"], serde_json::json!(0.24));
        let over: Config = serde_json::from_str(r#"{"energy_equity_oil_on": true, "energy_equity_oil_elasticity": 0.3}"#).unwrap();
        assert!(over.energy_equity_oil_on);
        assert_eq!(over.energy_equity_oil_elasticity, 0.3);
        let none: Config = serde_json::from_str("{}").unwrap();
        assert!(!none.energy_equity_oil_on);
        let with = |e: f64| Config { energy_equity_oil_elasticity: e, ..Config::default() };
        for e in [0.0, 0.15, 0.24, 1.0] {
            assert!(with(e).validate_rz1().is_ok(), "{e}");
        }
        for e in [-0.01, 1.01, f64::NAN, f64::INFINITY] {
            assert!(with(e).validate_rz1().unwrap_err().starts_with("energy_equity_oil_elasticity"), "{e}");
        }
    }
}
