# EconSim — Design Specification

A monthly, stock-flow-consistent, agent-based model (AB-SFC) of a US-like economy, written in Rust,
with an HTTP API for running policy and price shocks against a common-random-numbers baseline.

This document is the **contract** every module implements. Section numbers are referenced from code.

---

## 0. Literature the design rests on

| Area | Source | What we take from it |
|---|---|---|
| AB-SFC macro core | Caiani, Godin, Caverzasi, Gallegati, Kinsella & Stiglitz (2016), *Agent based-stock flow consistent macroeconomics: Towards a benchmark model*, JEDC 69 | Every flow is a payment between two balance sheets; endogenous money (loans create deposits); consistency checks at micro, meso and macro level. |
| Firms, prices, credit | Assenza, Delli Gatti & Grazzini (2015), *Emergent dynamics of a macroeconomic ABM with capital and credit* (CATS), JEDC 50; Delli Gatti et al. (2011) *Macroeconomics from the Bottom-up* | Adaptive price/quantity rules driven by inventories and relative price; bank credit to firms with leverage-based risk premia; bankruptcy and entry. |
| Innovation / Keynesian demand / energy | Dosi, Fagiolo & Roventini (2010) *Schumpeter meeting Keynes*, JEDC 34; Lamperti, Dosi, Napoletano, Roventini & Sapio (2018) *Faraway, so close* (DSK), Ecol. Econ. 150 | Energy as a distinct sector whose price enters every other sector's unit cost; investment driven by expected demand and cost of capital. |
| Calibrated forecasting ABM | Poledna, Miess, Hommes & Rabitsch (2023), *Economic forecasting with an agent-based model*, EER 151 | Sector input-output structure, calibration to national accounts, adaptive (AR) expectations, Taylor-rule central bank; validation by out-of-sample forecasts against AR, VAR, VECM and DSGE benchmarks (not by impulse responses; the paper has none). |
| Housing market | Baptista, Farmer, Hinterschweiger, Low, Tang & Uluc (2016), BoE SWP 619; Geanakoplos et al. (2012) *Getting at systemic risk via an ABM of the housing market*, AER P&P | Households as first-time buyers / owners / buy-to-let investors / renters; bid = f(income), ask = markup over hedonic value with monthly price reductions; LTV/LTI/DTI constraints; double-auction matching. |
| Labor market | Lengnick (2013) *Agent-based macroeconomics: a baseline model*, JEBO 86; Riccetti, Russo & Gallegati (2015) | Decentralised search: unemployed visit a sample of vacancies; reservation wage decays with unemployment duration; on-the-job search. |
| Consumption | Carroll (1997) buffer-stock saving; Kaplan & Violante (2014) wealthy hand-to-mouth | Consumption = MPC × permanent income + MPC_w × (liquid wealth − buffer target); high MPC when liquid wealth is low. |
| Lifecycle earnings | Mincer (1974) earnings function | log human capital = education premium + β₁·exp − β₂·exp². |
| Expectations | Hommes (2011) *The heterogeneous expectations hypothesis*, JEDC 35 | Inflation expectations: credibility-weighted mix of target and adaptive (past) inflation. |
| Monetary policy | Taylor (1993); Clarida-Galí-Gertler (1999) inertial rule | i = ρ·i₋₁ + (1−ρ)·[r* + π + φπ(π−π*) + φu(u*−u)], floored at the ZLB. |
| Yield curve | Expectations hypothesis + preferred-habitat term premia (Vayanos & Vila 2021); Nelson & Siegel (1987) | y(m) = average expected short rate over m + term premium(m); term premium rises with duration supply held by private investors — QE lowers it. |
| Oil shocks | Kilian (2009) AER; Hamilton (2003) | Oil price is an exogenous AR(1) world price; energy sector prices index to it; low short-run demand elasticity for energy (subsistence term). |

Real-world calibration targets (US, 2024-2026 data, approximate): see §13.

---

## 1. Time, units, scale

- One tick = **one month**. `w.tick` starts at 0 (= January 2026 for labels).
- Every household agent represents `cfg.scale = 132e6 / n_households` US households. Monetary values on agents are
  **real-world dollars per household** (e.g. wage ≈ $6,000/month), so per-agent numbers look realistic;
  aggregates for display are multiplied by `scale`.
- Goods are measured in "2026 dollars": sector prices start at 1.0 at the start of the burn-in, so real quantity =
  nominal / price. The CPI base is set in the first goods market of the burn-in, so at the first reported month the price
  level has drifted (CPI 111–133 at 20,000 households, seeds 1–3; burn-in 0 gives CPI 100).
- **Compare and warm-up.** `POST /api/compare` (and `sim::compare`) can step the common world `warmup_months` more months after the
  burn-in before the arms split, so that the reported months start without the start-up transient (b9-warmup: u +0.31 pp, policy
  +0.39 pp, wage growth +0.54 pp above their later averages over the first 60 months); reported `tick` and `date` are rebased to
  month 0. The model default is 0; the server default is 24 since joint calibration 4; live simulations are not warmed up. The
  absolute-tick state (bond maturities, housing shock expiries, annual-event alignment) is not rebased, which is why the warm-up
  must be a multiple of 12.
- House prices are in dollars directly (median ≈ $410k at t=0).
- Rates are **annual decimals** (0.045 = 4.5%). Monthly rate = annual / 12.

## 2. Agents and balance sheets

Accounts that can hold money (`ledger::Account`):

| Account | Holds money at | Assets | Liabilities |
|---|---|---|---|
| Household | its bank (deposits) | deposits, fund shares, houses | loans (mortgages) |
| Firm | its bank | deposits, capital, inventory, WIP houses | business loans |
| Bank | the central bank (reserves) | reserves, loans, government bonds | deposits, CB loans |
| Fund | bank 0 (deposits) | deposits, government bonds, equity claim on all firms & banks | shares owned by households |
| Government | central bank (TGA) | TGA balance | government bonds |
| Central bank | — | government bonds, loans to banks | reserves, TGA |
| Rest of world | bank 0 (deposits) | deposits, government bonds | — |

**Rule:** all money moves through `ledger::pay(w, from, to, amount)`. Loans are created/repaid only via
`ledger::create_loan / repay_principal / write_off`. Bonds move only via `ledger::bond_transfer`.
No module may change `deposits`, `reserves`, `tga` or loan principal directly. `ledger::check_consistency`
runs each tick in debug builds and in tests.

## 3. Monthly tick order (sim.rs)

```
 1 exog::step                 oil price AR(1)+shocks, TFP trend, shock schedule activation
 2 world::after_exog          b8-world: empty in stage 1 (oil and policy-surprise injections come in stages 3 and 4)
 3 monetary::set_policy       Taylor rule / override; QE & helicopter flags
 4 monetary::update_curve     yield curve from expected path + term premia + shocks
 5 monetary::exchange_rate    nominal exchange rate and dollar premium (fx.rs)
 6 world::step                b8-world: bloc month (US inputs, bloc shocks, bloc models, world metrics); no-op when the block is off
 7 banking::set_rates         deposit, mortgage, business loan rates
 8 demography::step           aging, retirement, death+inheritance, new 18-year-olds, education
 9 firms::plan                expected demand, target output, labour demand, prices, wage offers, investment
10 labor::step                separations, layoffs, search & matching, wage setting for new hires
11 firms::produce             output, wage payments, intermediate & investment orders
12 fiscal::pay_transfers      unemployment benefits, pensions, helicopter money, other transfers
13 housing::step              rent, listings, bids, matching, mortgages, foreclosures, construction
14 consumption::decide        consumption budget & sector split per household; portfolio rebalancing
15 fiscal::plan_spending      government goods orders
16 goods::clear               match all orders to firm inventories, payments, imports/exports
17 banking::service           loan payments, delinquency, deposit interest, liquidity, bank dividends
18 fiscal::collect_and_finance  taxes, bond issuance/redemption
19 monetary::markets          coupons, maturities, QE purchases, Fund & RoW portfolio, equity valuation
20 firms::close_books         profits, dividends, bankruptcy, entry
21 consumption::record_capital_income  capital income of the month, used by next month's decide
22 world::settle              b8-world: empty in stage 1 (the flow settlement comes in stage 2 and later)
23 metrics::record            aggregates; ledger::check_consistency
```

Per-month flow fields on agents (`hh.income`, `firm.revenue`, …) are reset by `sim::begin_month` before step 1.

## 4. Exogenous block (exog.rs) — owned by orchestrator

- Oil: `ln(oil_t) = ln(oil_base) + x_t`, `x_t = 0.95·x_{t-1} + σ·ε` (σ = 0.06 monthly when `cfg.stochastic_oil`), plus
  user shock `s_t` with geometric decay. Gasoline/energy CPI follows via the Energy sector price (§7.4).
- TFP: sector productivity drifts at `cfg.tfp_growth_annual` (1.2%/yr) with idiosyncratic firm shocks.
- Tech-boom regime (stage 4, SECTORS.md): a two-state Markov chain (entry 1/240, exit 1/96 per month), drawn from the EXOG stream only when `tech_boom_enabled`. In a boom Tech productivity growth rises by 5pp/yr and DurableGoods and OtherServices by 1pp/yr; `Shock::TechBoom` forces one.

## 5. Demography (demography.rs)

- Ages in months; agents are household heads aged 18–100. Each agent is a household; the initial head-age distribution is the US householder distribution (CPS 2024: <25 4.5%, 25–34 15.5%, 35–44 17%, 45–54 16.5%, 55–64 18%, 65+ 28.5%, declining density above 65).
- **Burn-in freeze**: while `init::build_world` settles the economy (before the tick reset) demography does not age anyone or run deaths, retirements, school completion, partnership or entry; labour-force exits and re-entries continue. The population is built at its requested size, so no growth into it is needed. Tenure changes that need those frozen events are frozen too: first-time purchases (renters and WithParents who are not investors or movers) and non-mover owner listings (retirees' downsizing) wait for the reported run. Otherwise renter purchases ratchet homeownership from ≈65.6% at build to ≈72% by month 0, since no death frees a home.
- **Mortality**: Gompertz–Makeham monthly hazard `h(a) = (A + B·e^{c·a})/12` with A=0.0005, B=0.00003, c=0.094 (a in years) ≈ US life table (life expectancy at 18 ≈ 60 more years).
- **Partnership**: each household is partnered (married or cohabiting) or single. Initial shares by head age: <25 30%, 25–34 55%, 35–44 63%, 45–54 61%, 55–64 57%, 65–74 54%, 75+ 37%. A single head aged 20–59 partners at 5%/yr; a partnered head under 65 separates at 1.5%/yr (no new household).
- **Death**: a single household dissolves when its head dies (individual Gompertz–Makeham hazard): all deposits, fund shares and properties pass to an **heir**: a random alive household aged 25–60 (prefer age ≈ deceased − 30). With `distquick_heir_match` (p1-distquick profile) the heir is the closest in log net worth among up to 8 draws in the nearest age bucket, with an education penalty; with `distquick_estate_heirs` = k > 1 the estate is split among k heirs (deposits and shares in equal parts, houses round-robin). The pre-re-fit base model had both off; the defaults have `distquick_heir_match` on (p1-all), and the estate split stays at 1 heir. Mortgages on inherited houses transfer to the heir. Employment ends; the dead agent's slot is left empty (`alive = false`) until an entrant takes it. A **partnered** household does not dissolve: each month the partner is lost with probability ≈ 2 × the individual hazard (either partner may die), and the household continues as a single household with the same money, home and job, and no inheritance flow.
- **Entry** (pre-integration description; §17 supersedes the entry rules): native households at `base × births(Y−27)/births(2008)` per month, where Y is the calendar year (US births table, clamped to 1990–2025). `base` is set at the first post-burn-in month so that native entries equal the expected dissolutions of single households (their heads' monthly mortality) plus native formation of `HH_FORMATION_ANNUAL` = 0.3%/yr of alive households. JCHS projects ≈0.65%/yr household growth over 2025–35, of which ≈0.35pp is immigration (`cfg.immigration_annual`, 0.35%/yr, net immigrants aged 22–45 of households). Entrants fill the lowest dead slot first and append a new slot only when none is free.
- **New native household** (pre-integration description; §17 and demography.rs are authoritative): the N(27, 5) truncated to [19, 40] rule below is superseded. Education drawn: HighSchool 55%, College 35%, Graduate 10%. College = Student until 22, Graduate until 24; an entrant younger than that is a Student, otherwise arrives finished as a job seeker (Unemployed with a reservation wage). Skill ~ LogNormal(0, 0.35) normalised mean 1. Partnered with probability 0.35. Starts with deposits ~ LogNormal(mean $3k), tenure WithParents, no fund shares, and an imputed career from age 20.
- **Leaving parents**: WithParents agents move out (become Renter search) with monthly probability rising with age/income (median move-out age ≈ 24).
- **Retirement** (demography.rs `retirement_hazard`, fix-demog): 0.4%/m at 55–61; a one-off 4% at the 62nd birthday; 1.5%/m at 63–66; a one-off 8% at the 67th birthday; 1.0%/m at 68–74; 0.6%/m from 75 (no forced retirement). Retirement ends employment and sets no benefit. **No PIA is assigned at retirement**: claiming lives in `fiscal.rs` (§9.3). A retiree aged 62+ with no pension claims at once at the claim-age factor; at init, initial retirees aged 62+ take the claim-age walk and any not walked claim at their current age; under-62 retirees get nothing until 62. Non-retirees aged 62+ are seeded by one walk at the first reported month and claim monthly afterwards.
- **Out of labour force**: 3% of prime-age agents are OutOfLaborForce at any time (flows in/out at 0.2%/m).

## 6. Labour market (labor.rs)

- **Human capital** `h = edu_premium × skill × exp(0.035·x − 0.0006·x²)`, x = years since end of schooling;
  edu_premium = {HS: 1.0, College: 1.6, Graduate: 1.95}. Productivity of a worker = firm.labor_productivity × h.
- **Wage of a worker** = firm.wage_offer × h (wage_offer is $ per efficiency unit per month).
- **Separations** each month: exogenous quit/match-destruction rate 1.4%; layoffs executed when firm planned layoffs (lowest h/wage ratio first… use last-in-first-out with noise).
- **Search**: unemployed sample `cfg.applications` (6) random vacancies (70% within last sector, 30% any); employed search on-the-job with prob 4%/m and move if new wage > 1.1×current.
- **Hiring**: firm ranks applicants by h, hires up to vacancies if applicant's reservation wage ≤ offered wage.
- **Reservation wage** = max(benefit level, last_wage × 0.97^{months_unemployed}); floor at `cfg.min_wage_monthly`.
- **Wage dynamics for stayers**: annual (on job anniversary month) wage reset to current firm.wage_offer × h (so downward rigidity: never cut by more than 2% for stayers).
- **Benefits**: on job loss, benefit_months_left = 6; benefit = min(0.45 × last_wage, $2,600) paid by fiscal module.

## 7. Firms (firms.rs)

Sectors: Energy, Food, Goods, Services, Construction. Firm count ratio ≈ 1 firm per 25 households; employment shares Energy 2%, Food 5%, Goods 19%, Services 68%, Construction 6%. Size: Pareto(1.1) truncated.

### 7.1 Expected demand & output target (Assenza et al. 2015)
- `expected_demand_t = expected_demand_{t-1} + 0.4·(demand_{t-1} − expected_demand_{t-1})` (demand = real units ordered including unmet).
- Target inventory = `cfg.inventory_target[sector]` × expected demand (Services 0, Energy 0.3, Food 0.5, Goods 1.0, Construction 0.5 of a month).
- Desired output `Y* = max(0, expected_demand + 0.5·(target_inventory − inventory))`, capped by capital capacity `capital × capital_productivity × 1.0`.
- Labour demand `L* = Y* / labor_productivity` efficiency units; compare with current workers' Σh: vacancies = ceil((L* − L)/avg_h) if positive; planned layoffs = floor((L − L*)/avg_h × 0.5) if L > 1.05·L* (firing is gradual).

**Re-fit status (Block D, Step 9; off by default).** Recruiting capacity is committed as an off-by-default option. `vacancy_net_share` 1.0 and `vacancy_replace` false reproduce the Step 8 economy exactly. The candidate forms (`vacancy_net_share` 0.5 or 0.4, with or without `vacancy_replace`) failed the firm guard and the core-PCE gate, so the recruiting-capacity mechanism and the re-anchored wage curve are a separate later item. `u_star_max` stays 0.044. The u* floor is now the Config field `u_star_min`, 0.042 (legacy 0.038; Step 11b).

### 7.2 Price setting
- Unit cost `uc = wage_bill_per_unit + intermediate_cost_per_unit + energy_cost_per_unit + interest_per_unit`, where intermediate/energy cost per unit = Σ_s A[s][sector] × P_s (input-output coefficients, §13.3).
- Calvo-style repricing: each month a firm reprices with probability θ_sector (Energy 1.0, Food 0.35, Goods 0.2, Services 0.12, Construction 0.25).
- When repricing: target markup adjusts: if inventory < 0.5·target (or unmet demand > 5%) markup += U(0, 0.03); if inventory > 1.5·target and price > sector average, markup −= U(0, 0.03); markup clamped to [0.02, 0.6]. New price = (1+markup)·uc × (1 + expected_inflation/12·months_since_reprice) — firms build in expected inflation.
- **Energy sector**: price = `energy_base_price × (crude_share·oil/oil_0 + (1−crude_share)·uc/uc_0)`, crude_share = 0.5, updated monthly.

**Re-fit status (Step 7).** The output-gap term is removed from markups: `markup_demand_pull` is 0.0 (legacy 0.3). Marginal cost already carries the gap through wages, and with the lagged wage offers the term produced a resonance of about 30 months. Scaling `initial_markup` (Step 11c) was not triggered: corporate profits are 10.06% of GDP at 16 seeds, inside the 9–11 band and not above 11%.

### 7.3 Investment
- Capacity utilisation u = Y*/capacity. Desired capital K* = expected_demand / (capital_productivity × 0.85) (target utilisation 83% since joint2; 85% before, see `scripts/profiles/legacy_j2off.json`).
- Expected return r_K = expected profit per unit of capital (12-month moving average, annualised). User cost = `business_loan_rate − expected_inflation + depreciation(0.08/yr) + hurdle(0.02)`.
- Investment (real units) = `max(0, (K* − K)/12 + depreciation·K) × clamp(1 + 4·(r_K − user_cost), 0, 2)`.
  This is the main **interest-rate → investment** channel.
- Financing: internal deposits above 1 month of costs first; remainder via `banking::request_loan(w, Borrower::Firm, amount, LoanKind::Business)`. If refused, investment is scaled down.
- Capital goods are bought from the Goods sector (and 20% from Construction) via the goods market (`orders.firm_investment`).

**Re-fit status (Step 8).** The user-cost line uses the household expectation of inflation (`user_cost_household_expectation` true, legacy false). The nowcast blend stays in price resets and wage norms. Partial adjustment of desired capital (`kstar_adapt`) was tested over 0 to 0.20 and stays 0 (the raw k*): no tested value passed the gates.

### 7.4 Production, payments
- Output = min(Y*, labor_productivity × Σh_workers, capital × capital_productivity). Inventory += output.
- Pay wages: `pay(Firm → Household, wage)` per worker, plus employer payroll tax 7.65% to Government. Record hh.income.wage.
- Place intermediate input orders: nominal budget per supplying sector s = A[s][sector] × output × P_s (P_s = last month's sector price).
- Energy-sector firms additionally pay crude: `crude_cost = crude_intensity × output × oil/oil_0`; `cfg.oil_import_share` of it is paid to RestOfWorld, the rest stays as domestic upstream income (no payment).
- Depreciation: capital ×= (1 − 0.08/12).

**Re-fit status (Step 4, energy cost structure).** Domestic crude is split into an upstream tax (`upstream_tax_share` 0.08, paid to the TGA), a royalty (`upstream_royalty_share` 0.10, paid to the Fund) and an upstream margin (`upstream_margin` 0.10). Both the tax and the royalty are firm costs (`Config::upstream_split`, `firms::plan_firm`, `firms::produce`). Bank tax is collected after opex (`bank_tax_after_opex` true), and a founder is hired at the wage offer times human capital (`founder_wage_hc` true). The legacy values are 0, 0, 1.0, false and false. At 16 seeds the Energy profit share is 1.13 (gate 0.6–1.1): missed and recorded.

### 7.5 Books, dividends, bankruptcy, entry
- Profit = revenue − wage bill − payroll tax − inputs − crude imports − interest − depreciation×P_goods×capital.
- Corporate tax is collected by fiscal (§9). Dividends: if profit > 0 and deposits > 2 months of costs, pay `cfg.payout_ratio` (0.6) of after-tax profit to Fund.
- Bankruptcy: deposits < 0 after a refused loan, or net worth < 0 for 6 consecutive months. Workers laid off, loans written off at 60% loss (bank recovers 40% from liquidation of capital: liquidation pays the bank from firm deposits/capital sale to Fund), inventory destroyed. Firm slot is reborn as an entrant after 3 months: sector mean productivity × LogNormal(0, 0.1), capital = 50% sector median, startup equity paid by the Fund.

**Re-fit status (Steps 5–6, bankruptcy and entry).** `cipi_excludes_writeoffs` is true: the inventory of a bankrupt firm is recorded and added back to CIPI. `distress_default_hazard` is 1/6: a firm with negative Merton net worth defaults on a monthly Bernoulli draw from the `FIRM_DEFAULT` stream, in place of the six-month counter (legacy 0 and false). `native_entry_tracks_dissolutions` is true, so native household entry follows dissolutions, and `hh_formation_annual` is 0.0030 (legacy 0.0055).

**Joint3:** nonstorable (perishable) output is not inventory investment in GDP (F1; `cipi_excludes_nonstorable`, default on since joint3).

### 7.6 Firm-distribution observations (b5-firmhet, S0; read-only)

`metrics.rs` (`fill_firm_dist`, `firm_dist`) reports the firm distribution each month: firms per sector, revenue HHI
per sector, the employment share of the largest 1% and 0.34% of firms, within-sector dispersion of log labour
productivity (sd and 90-10), the Olley-Pakes gap, the size-productivity slope, markups over variable cost (price over
unit cost less capital cost at normal utilisation; De Loecker, Eeckhout & Unger 2020), capital gaps ln(K/K*) by sector,
capacity utilisation by sector, and owner-operated firms by the owner's labour status. These are observations only: no
RNG draw, no ledger entry and no firm state is touched. Legacy behaviour is unchanged (seed-1 and seed-2 runs at 20k x
240 match master bit for bit on every master key, and on both legacy profiles). The firm-distribution mechanisms
(productivity-driven share, markup norm, lumpy investment, demand elasticity) are not implemented yet; see
`docs/CALIBRATION.md`, section "b5-firmhet".

### 7.7 Entry viability switches (b5-firmhet, S1; off by default)

`firms.rs` has three switches for the zero-capital trap in sectors with few firms: `entrant_capital_floor_workers`
(entrant capital at least the capital that equips n_w workers at normal utilisation), `inv_expand_labour_ref` (the 2%
monthly expansion cap is on max(K, K_lab) rather than K), and `entry_gate_capital_weighted` (the entry gate reads the
capital-weighted profit rate). A fourth, `entry_contestability_share`, is the S1 fallback (admit entrants in sectors below
a share of their calibrated firm count). All four are off at their legacy values, and the on-state profile is
`scripts/profiles/b5-entry.json`. Measured on-state and why it is not a default: `docs/CALIBRATION.md`, section
"b5-firmhet". Legacy is bit-identical (seeds 1 and 2 at 20k x 240, both legacy profiles).

## 8. Goods market (goods.rs) and consumption (consumption.rs)

### 8.1 Consumption budget (buffer-stock)
*Round 7 (step 4, `housing_first_budget`; default on).* The budget is capped at deposits less this month's scheduled
mortgage payments (`banking::mortgage_payments_of`) and pass-through spending (landlords' operating costs and employer
health benefits), plus `consumer_credit_months` × the credit line (default 0, so no credit line). The subsistence floor no
longer overrides the cash constraint. Discretionary and windfall spending draw on the same headroom. The legacy budget
(deposits plus one month of permanent income) is kept under `housing_first_budget = false`.

Monthly after-tax income `y` = this month's income − expected taxes − housing_cost (§10).
Permanent income `yp ← 0.9·yp + 0.1·y_after_tax` (excl. housing cost). Liquid wealth `W = deposits + 0.5·fund_value`.
Buffer target `B = k(age)·yp`, k = 2 (age<30), 4 (30–54), 8 (55–64), and retirees target spending down over remaining life:
retiree budget = pension + annuity(W, remaining_life_expectancy, real deposit rate).

`C = max(C_min, mpc_y·yp + mpc_w·(W − B) + mpc_h·housing_equity_annualised + mpc_e·fund_value_annualised) − housing_cost`
with mpc_y = 0.90 − 0.5·(real_deposit_rate) (intertemporal substitution), mpc_w = 0.04/month if W > B else 0.10,
mpc_h = 0.03/yr, mpc_e = 0.03/yr, C_min = subsistence(§8.2). Budget cannot exceed deposits + 0 (no overdraft except
1 month of income credit-card line at `cfg.credit_card_rate`).

### 8.2 Sector split (Linear Expenditure System / Stone–Geary)
Subsistence real quantities γ = {Energy: 180, Food: 450, Goods: 300, Services: 600} $/month (2026 dollars).
Budget share of supernumerary spending β = {Energy: 0.05, Food: 0.10, Goods: 0.30, Services: 0.55}.
`spend_s = P_s·γ_s + β_s·(C − Σ P_j γ_j)`. If C < Σ P_jγ_j, scale subsistence proportionally.
This makes energy and food demand price-inelastic (an oil shock squeezes discretionary goods & services spending,
especially for low-income households).

### 8.2a Discretionary spending multiplier (consumption.rs, stage 3)
Discretionary categories (durables, leisure and hospitality, much of tech) are cut first in downturns and raised
first in booms (Stock & Watson 1999: durables consumption sd ≈2.9× GDP's). Each household's discretionary share of
supernumerary spending is `Config::discretionary_weight` (w = {Energy 0, Food 0.1, Durable 1.0, Nondur 0.3, Tech 0.6,
Leisure 0.9, Health 0.1, Other 0.3}). Each month the discretionary part is scaled by
`D = clamp(exp(θ_r·(r_neutral − r_real) + θ_m·x_money + θ_u·(u* − u) + θ_c·x_credit), D_min, D_max)`, with
r_neutral = 1%, r_real the household's real rate (§8.1), `x_money = ln(deposits ÷ B) − norm` (B the liquid buffer
target, clipped at ±2), `u* − u` the unemployment gap, and `x_credit` the household's bank capital shortfall below
`capital_requirement` (zero in normal times, clipped at −5pp). Parameters: θ_r = 6 (Erceg & Levin 2006: durables are
several times more rate-sensitive than other spending), θ_m = 0.10 (Friedman & Schwartz 1963 hot potato, excess money
goes first to durables and leisure), θ_u = 0.5 (Carroll 1992 precautionary saving; Mian & Sufi 2014), θ_c = 4
(Mian & Sufi 2014; Gertler & Karadi 2011), norm = 0.6, D ∈ [0.5, 1.6].

Budget: the change in spending is `Δ = (D − 1)·Σ_i w_i·β_i·extra`, where `extra` is the supernumerary budget. Each
sector's LES share becomes `β_i·extra·(1 + (D − 1)·w_i)`. The change moves total spending (not only its mix): extra
discretionary spending comes out of deposits, and lower spending stays in deposits. It is capped so that the budget
stays within deposits less scheduled mortgage payments and pass-through spending (§8.1), and it never takes spending below subsistence. All
purchases go through the goods market as before, so the ledger and determinism hold. Households' own deposits drive
`x_money`, so injections that land in some households' deposits (QE, helicopter money, credit) raise their own
discretionary spending. `consumption_by_sector_real` and `discretionary_multiplier_mean` are reported in the metrics.

### 8.3 Matching (goods.rs)
- Each household has a preferred firm per consumer sector. Each month with prob 0.25 it samples a random alternative firm in the sector; switches if `p_new < p_old` with prob `1 − exp(4·(p_new − p_old)/p_new)` (Delli Gatti et al. 2011).
- Orders are aggregated per firm. If a firm's inventory is insufficient it sells pro rata; unmet buyers try one more random firm in the sector; any remaining household budget for Goods is filled by imports at the sector price (import leakage); otherwise it is saved.
- Imports: a fixed share `cfg.import_share` = {Energy 0.05, Food 0.10, Goods 0.30, Services 0.03} of every order is routed to RestOfWorld at P_s.
- Import lag (p1-fx, default since the re-fit): the import share responds to the price with a lagged state (`import_lag_speed` 0.08 a month) that moves toward the long-run elasticity `import_elasticity_lr` 0.6; the short-run elasticity is `import_price_elasticity` 0.2. Legacy: no lag, `import_elasticity_lr` null (`scripts/profiles/legacy_p1off.json`).
- Exports: RestOfWorld demand per sector = `cfg.export_base[s] × (P_world/(P_s × fx))^ε`, paid from RoW deposits (RoW recycles via §11.4). The price is the dollar price (`fx` = the exchange rate, §11.4). ε is `export_elasticity_sr` (0.3 at the defaults); with `export_elasticity_lr` set (1.0 at the defaults), ε moves toward it through a lagged price state (`export_lag_speed`, 0.08), the p1-fx distributed lag. The pre-re-fit state is inert (no lag, the p0 value 1.5), which is scripts/profiles/legacy_p1off.json; scripts/profiles/p1-fx.json is the historical overlay.
- Government orders (fiscal) and firm investment/intermediate orders are cleared the same way (firm buyers pay from deposits).
- Record: firm.sales/revenue/demand; hh.consumption[s]; w.flows (C, I, G, X, M by sector).

## 9. Government (fiscal.rs)

### 9.1 Taxes (monthly, annualised brackets)
- Federal+state household income tax: progressive on annualised taxable income (wage + interest + dividends + rent_received + 85% of pension − standard deduction $15k): brackets 0–11.6k 10%, –47k 12%, –100k 22%, –191k 24%, –243k 32%, –609k 35%, above 37%; plus flat state 4%.
- Employee payroll tax 7.65% on wages up to $176k/yr (withheld here); employer side paid in firms::produce.
- Corporate tax 21% on positive firm & bank profits (monthly).
- Property tax 1.1%/yr of house market value, paid by owners (collected in housing, routed to Government).
- Sales tax 0 by default (configurable).

### 9.2 Spending
*Round 7 (step 3).* The fiscal rules respond to `debt_rule_gap`: smoothed debt/GDP less a target that starts at
`initial_debt_gdp` (100%) and rises by `debt_target_slope` (default 0.0125) a reported year, along the CBO current-law path.
`fiscal_rule_strength` (default 0.5) scales both rules: income-tax rates move 0.375pp per 10pp of gap at the default and
0.75pp at strength 1; non-health purchases move 1.25% per 10pp (2.5% at strength 1), clamped to [0.75, 1.3]. Half strength
gives about 0.037 of GDP per unit of debt gap, inside the Bohn-type range of 0.03–0.05. Without a target path the rule
cancelled realistic transfers; with the rule off, the round-7 diagnosis (at ed8cb0e, not re-run at HEAD) gave a deficit of 10.9% and debt 223%, so a rule stays.

- Government purchases = health + other. Other purchases = `(1 − HEALTH_SHARE)` × `cfg.gov_purchases_share` × trend nominal GDP / 12 × shock multiplier × debt rule. Health (Medicare + Medicaid) is mandatory: `HEALTH_SHARE` = 27% at t=0, outside the debt rule and shock multiplier. Health per month = health per household aged 65+ (set at the first reported month so that month 0 equals the t=0 share) × households aged 65+ × (trend nominal GDP per household ÷ its t=0 value) × (1 + `EXCESS_HEALTH_COST_GROWTH`)^(years since the start of the reported run), with `EXCESS_HEALTH_COST_GROWTH` = 0.8%/yr (CBO excess cost growth in Medicare). Simplification: all health spending is tied to 65+ households, since Medicare and long-term-care Medicaid dominate the growth. Purchases split Services 0.75, Goods 0.12, Construction 0.13. Metrics report health as in-kind personal consumption (`health_gdp`).
### 9.3 Transfers
*Round 7 indexation (steps 1–2, 5 and 8).* Frozen burn-in and the anchor month track the wage index, so no benefit erodes
against wages during the pre-sample; the reported run then follows US law.

| Quantity | Burn-in and anchor | Reported run | US law |
|---|---|---|---|
| Pensions (SS) and tax brackets | wage index (`nominal_scale`, COLA) | CPI COLA | CPI (SS), CPI (brackets) |
| UI weekly cap | wage index (SAWW rule) | wage index | state average weekly wage |
| Minimum wage (model) | wage index, scale frozen at anchor | scale frozen at anchor | nominal |
| Means-tested maxima (SSI, SNAP) and EITC thresholds | wage index | wage index (AWI proxy) | CPI (SNAP, EITC); SSI unindexed |
| SSI and SNAP asset limits | burn-in scale | burn-in scale | unindexed (SNAP has no asset test under BBCE) |
| Entrant reservation wage | wage index | wage index | n/a |
| Under-62 retirees at init | 0.7 × PIA (no SSDI or private pension in the model) | same | SSDI, employer pension, drawdowns |

Benefits per recipient stay at US maxima times the index. `mt_scale` is not used.

- Unemployment benefits (§6).
- Social security: career earnings are accumulated in base-year dollars (wage ÷ wage index × 100). At retirement AIME = career_earnings / max(career_months, 420) × wage_index / 100, with PIA bend points 90% to $1,226, 32% to $7,391, 15% above, both scaled by wage_index / 100 (wage indexing up to eligibility). The average wage index (100 at t=0, the first month of the world) is updated each January to 100 × (mean monthly wage of employed households over the previous 12 months) ÷ (that mean in the first month). After the first payment, pensions are indexed annually to CPI (COLA in January).
- Means-tested support: if household income < $1,200/m pay top-up of 50% of the gap (SNAP/EITC proxy).
- Helicopter money transfers (from central bank) are paid here when `cb.helicopter_per_household > 0`: `pay(CentralBank → Household)`.

### 9.4 Financing
- TGA target = 1 week of outlays. Deficit (TGA below target) → issue new bonds; surplus → buy back bills.
- Issuance mix by face value: 3m 25%, 2y 25%, 5y 20%, 10y 20%, 30y 10%. Coupon = current curve yield at tenor.
- Buyers at auction: banks (up to their liquidity desire), Fund, RoW in proportions `cfg.auction_shares` (0.3/0.45/0.25).

## 10. Housing (housing.rs)
*Round 7 (steps 6–8).* **Continuous employment (`first_buy_continuous_employment`).** `Household::months_employed`
counts continuous employment: a job-to-job hire keeps it, separation to any other status resets it, and the six-month
first-purchase gate reads it (the Fannie Mae Selling Guide's two-year history; `months_in_job` is unchanged elsewhere).
**Burn-in foreclosure freeze (`burn_in_freeze_foreclosures`).** No foreclosure starts in burn-in, as for other tenure flows
that frozen demography cannot offset; household mortgage delinquency is reset at the anchor so the burn-in stock does not
all fall due in month 0. **Indexed rent floor (`rent_floor_indexed`).** The affordability floor is 0.35 × the rent index
(about $600 at t=0), and seekers may spend 10pp more of income (JCHS: about a quarter of renters pay over half their
income). The rent budget share is 40% (the comment at line 141 previously said 35%).
Deferred to fix-housing, not in this round: the default rules, the tenure channel, lock-in and refinancing, user-cost rents
and prices, and construction financing (see the round-7 spec, section 4).


- Stock: houses = 1.06 × households at t=0 (≈ 6% vacant). Quality q ~ LogNormal(0, 0.45), mean 1. Hedonic value `V = HPI × q`, HPI_0 so that median V ≈ $410k.
- Market rent index `R` with rent ≈ V × 4.5%/yr gross yield at t=0 (median rent ≈ $1,550/month).
- **Owners**: pay mortgage (banking services the loan), property tax 1.1%/yr, maintenance 1%/yr (paid to Services firms as a consumption item via orders? — simplified: paid to Government? NO: maintenance is added to the household's Services budget in consumption).
- **Moving / selling**: owners under 75 list to move with prob 0.28%/m before the market and lock-in factors (`OWNER_LIST_PROB`; at the earlier 0.4%/m, months of supply stayed near 9 with many listings unsold); a move starts a buy-sell chain. Owners also list to leave ownership for a rental (owner→renter exit, `OWNER_EXIT_BANDS`, PSID/SIPP ≈4–5%/yr under 35, ≈1.5–2% at 35–64, ≈2–3% at 75+; a sale makes the household a renter seeking a lease and it may not bid to buy for 36 months, `EXIT_REBUY_COOLDOWN_MONTHS`). Calibrated 65+ exits are lower than PSID (0.75% at 65–74, 1% at 75+): at PSID rates the 65+ stock sits 10–20pp below HVS. Investors sell when rental yield < mortgage rate − 2% with prob 2%/m. Both kinds of listing are frozen in burn-in.
- **Buying decision (renters only)**: first-time purchases come from renters (a household rents before it buys; entrants leave their parents into a lease). The monthly bid hazard is age-specific (`FIRST_BUY_BANDS`, annual 10% at 18–24, 27% at 25–34, 18% at 35–44, 10% at 45–54, 7% at 55–64, 5% at 65+; the realised renter→owner rate runs below the bid hazard). Sources: PSID/SIPP renter→owner transitions (18–24: 4%, 25–34: 10%, 35–44: 8%, 45–54: 5%, 55–64: 3.5%, 65+: 2% a year) and Census HVS 2024 homeownership by age. Buy if employed ≥ 6 months, deposits ≥ down payment + closing costs, and
  user cost of owning `UC = P·(mortgage_rate + 0.011 + 0.01 − E[HPA])/12` against rent × OWNERSHIP_PREMIUM (1.6), with
  buy probability `1/(1+exp(−1.0 × gap))`, gap = (rent × 1.6 − UC)/rent (logit slope 1.0; Glaeser & Gyourko 2007). The flat
  slope reflects wide dispersion in tenure preferences: a +1pp rate moves ≈5% of the buy probability, not ≈16%.
  E[HPA] = 0.7 × trailing 12m HPI growth + 0.3 × 3%.
- **Bid (Baptista)**: desired price `P_bid = min( 4.5 × annual_income × exp(0.5·ε),  max_loan + down )`, max_loan from bank quote:
  LTV ≤ `cfg.max_ltv` (0.95 FTB / 0.80 investors), DTI (P&I + taxes)/gross income ≤ `cfg.max_dti` (0.43).
- **Ask**: `ask = V × 1.04 × exp(0.05ε)`; reduce by 3% each month unsold, faster in a slack market: the monthly cut is
  `0.97^(1 + 2 × ln(months_supply / 5.4)⁺)` (Genesove & Mayer 2001: sellers cut prices as time on market rises), withdraw after 6 months.
- **Matching**: monthly double auction: sort bids descending; each bidder buys the highest-quality listed house with ask ≤ bid; transaction price = ask. Then HPI updated as `HPI ← HPI × (median(price/q of month's sales)/HPI)^{0.5}` (smoothing), unchanged if < 5 sales.
- **Housing rate response (IRF, 8 seeds, 20k households)**: a +100bp term-premium shock moves mortgage rates ≈+0.9pp and
  sales −19% / −24% / −20% after 3 / 6 / 12 months, with house prices −0.8% / −2.7% / −7.1% at 3 / 6 / 12 months and −8.4% at 24
  (Jordà, Schularick & Taylor 2015; Williams 2015 give ≈−8 to −10% sales and −2 to −5% prices per 100bp). A +100bp policy shock
  gives sales −4% / −6% / −5% and prices −1.3% at 12 months, so it moves housing less than the term-premium shock. Lock-in
  (listings −10% per pp of rate gap) is kept. Known gap: the term-premium response is larger than before the recalibration
  (sales −11% and prices −2% at 12 months). Most of the price response runs through the buyer-affordability index
  (`AFFORDABILITY_SENS`) and the slack-market ask cuts, and months of supply sits near 6–7 rather than the balanced 4–5.
- **Transaction**: buyer pays seller: down payment from deposits + new mortgage (`banking::originate_mortgage`, 30y fixed at current mortgage rate, created as deposits then paid to seller). Seller's existing mortgage is paid off from proceeds (`ledger::repay_principal`). Closing costs 2% of price paid to Services firms (as Services spending) — simplified: paid to Government as transfer tax 1%.
- **Buy-to-let investors**: households in top 15% net worth with deposits ≥ 25% of a house bid, LTV ≤ 0.75, with probability
  0.3%/m (`INVESTOR_PROB`, set so investor purchases ≈0.4–0.5M a year, NAR investor share of purchases) × `logistic((gross yield − running costs + E[HPA] − 10y − 1.5pp) / 2.5pp)`. The yield margin is market-wide, so a hard
  threshold switched all investors on and off together (the baseline margin moves around zero); the logistic reflects
  dispersion in investors' required returns.
- **Rental market**: vacant investor-owned/bank-owned units listed at `R × q × U(0.95,1.1)`; renters seeking (new movers, WithParents leaving, foreclosed owners, lease ends with 20% prob) match with best quality they can afford at rent ≤ 30–40% of income. Leases 12 months; renewals at market rent × q. Rent index updated from new leases like HPI.
- **Mortgage default**: if a mortgage is ≥ 3 months delinquent (set by banking::service) **and** LTV > 0.9 or unemployed, the eligible loan starts foreclosure with probability 15% a month (`FORECLOSURE_START_PROB`; most 90+ day delinquencies cure or sell, and ungated starts were ≈0.5M a year against ≈0.2–0.3M in ATTOM data). Foreclosure: bank becomes owner, household becomes renter, loan written off minus sale proceeds; bank lists house at 0.85 × V.
- **Rent growth**: `g_rent = π_e + 0.5 × (g_income − π_e) + 0.3 × (natural vacancy − vacancy) + 0.08 × ln(rent*/rent)`, clamped to [−3%, 10%]. Rents grow with nominal incomes (BLS shelter runs ≈0.5–1pp above core in the long run); `g_income` is an exponential average of the wage index over 12 months. The natural rental vacancy is captured at the first month.
- **Withdrawals (demolition or conversion)**: an unmortgaged unit vacant and for rent for ≥ 12 months is withdrawn with probability 1%/m (`DEMOLISH_PROB`), at ≈40 units a year in the model (≈0.26M US, Census/AHS units lost). The unit keeps its id with `completed = false`; a household landlord's unit passes to the bank slot. Without withdrawals the stock only grows, and starts above household growth raise vacancy without limit.
- **Refinancing**: owners refinance if current mortgage rate < their rate − 0.75% and LTV < 0.8, prob 10%/m (US refinancing waves; lock-in effect when rates rise).
- **Construction**: housing starts per month = `base_starts × max(0, (HPI × q̄ / build_cost))^{3}`, base ≈ 0.068% of stock/month (`base_starts_monthly_share`; ≈0.8%/yr of stock, which is household growth plus withdrawals; ≈1.1–1.2M starts a year in the model). `build_cost = construction_units × P_construction + land(25% of V)`. Each start is assigned to a construction firm (by spare capacity), consumes `construction_units_per_house` real construction output over 8 months from the firm's inventory; on completion the firm owns and lists the house. Firm.backlog is set so firms::plan includes it in expected demand. Residential investment = construction output used.

**Re-fit status (Step 12, Block E).** The house-price block is unchanged at the defaults (`housing_uc_theta` 0.75, `housing_price_uc_gain` 0.9, `housing_affordability_sens` 0.2). Two bars fail and are recorded, not fixed. H2, the +100bp TP house-price trough, is −4.85 to −4.99 against a −5 to −10 band. H5, the policy HVS rental vacancy, rises to +0.12 to +0.15pp at m18–m24 against a maximum of +0.05pp. The 12b and 12c grids passed H1–H4 but not H5 at 16 seeds, and the one H5 pass fails the H6 guards. The structural switches (`housing_price_valuation_slow`, `housing_price_drift_trend`) are not committed. See CALIBRATION.md, Block E.

## 11. Banks and central bank

### 11.1 Rates (banking::set_rates)
- Deposit rate = max(0, β_d × policy_rate), β_d = 0.35.
- 30y fixed mortgage rate = y(10y) + `cfg.mortgage_spread` (1.7%) + bank-specific adj (capital shortfall → +).
- Business loan rate = bank base + firm default-risk premium. Bank base = 0.5·y(2y)+0.5·y(5y) + 2.0% + bank excess bond premium `bank.ebp`.
- Lender balance-sheet premium (excess bond premium, Gilchrist & Zakrajšek 2012): each month a bank rolls its business write-offs into a trailing 12-month window, and targets `ebp_loss_sens` × (annualised loss rate − slow norm, initial 0.5%, adapt 1%/month) + `ebp_capital_sens` × max(0, requirement + `ebp_capital_buffer` − capital ratio). The target is bounded to [−0.5pp, +4pp] and `ebp` closes 25% of the gap each month. (RZ20) With `ebp_equity_sens` > 0 the target also gains `ebp_equity_sens` × max(0, −L), L the Fund's current aggregate q (`Fund::q_raw_log`: the equity-value-weighted mean over sectors of ln(q/q0), unsmoothed), so a fall in equity values widens the premium, the business loan rate and, through the standards below, tightens credit; off by default (the premium's target reads losses and capital only). `rerate_ebp_sens` is the same term from the log of a `SharePrices` re-rating alone.
- Firm default-risk premium (Merton-style expected loss): annual PD = logistic(`pd_intercept` + `pd_leverage`·leverage + `pd_coverage`·ln(1/interest coverage)); premium = `pd_lgd` (0.6) × (PD − PD of the reference firm at leverage 0.5, coverage 3). Zero for the reference firm, convex in leverage, bounded to [−1pp, +6pp]. Coverage = (earnings + interest)/interest, capped at 50.
- Credit standards (SLOOS, §11.2): a bank's positive premium tightens its standards, lowering maximum leverage (approval and rollover) by 0.05 per pp and raising minimum coverage by 5% per pp.
- Credit-card / overdraft rate = policy + 15%.
- Credit card / overdraft rate = policy + 15%.
### 11.2 Credit supply
- Banks lend only if post-loan capital ratio (equity / risk-weighted assets: mortgages 0.5, business 1.0, bonds 0) ≥ `cfg.capital_requirement` (10.5%) + buffer; otherwise refuse (credit crunch).
- Firms: refuse if leverage (debt/assets) after loan > 0.75 or interest coverage < 1.5, each tightened by the bank's premium (above). Maturing business loans are rolled over only while the firm's leverage is within the tightened rollover limit (0.85 untightened); otherwise the firm must repay from deposits.
### 11.3 Servicing (banking::service)
- Business loans whose borrower has been in arrears for `business_default_months` (6) trigger a firm default (§7.5 bankruptcy); the write-off is booked to the bank's trailing business write-offs.
- For each loan: scheduled payment = annuity(principal, rate, months_remaining). If borrower deposits ≥ payment: pay interest (`pay(borrower→Bank)`) and principal (`ledger::repay_principal`); else pay what is possible and increment months_delinquent. Business loans in default 6 months → write off (firms module handles bankruptcy).
- Deposit interest monthly: `pay(Bank → depositor, deposits × deposit_rate/12)`.
- Interest on reserves: CB pays `reserves × policy_rate/12` to banks (`pay(CentralBank → Bank)`).
- Liquidity: if reserves < 0 → borrow from CB discount window at policy + 0.10%; repay when positive.
- Bank dividends: 50% of positive profit paid to Fund if capital ratio > requirement + 2%.
### 11.4 Central bank & markets (monetary.rs)

Code layout: `monetary.rs` (policy rule, r*/u*, yield curve, markets), `equity.rs` (equity risk premium, sector valuation, Fund equity return) and `fx.rs` (exchange rate). `monetary` re-exports `exchange_rate`, `fx_base`, `FxBase` and `trailing_equity_return`.
- **Policy rule**: `i* = r* + π_e + 0.5·(π − π*) + 1.0·(u* − u)·2` (Okun 2:1), smoothed `i = 0.9·i₋₁ + 0.1·i*`, rounded to 0.25%, floor 0. r*=0.75%, π*=2%, u*=4.2%. π = trailing 12m core-ish PCE (use CPI ex-energy for rule `cfg.rule_uses_core`), π_e = household mean expectation. User override pins the rate for N months.
- **Expected short path**: `E[i_{t+k}] = i_∞ + (i_t − i_∞)·0.97^k`, i_∞ = r* + π*.
- **Term premium** `TP(m) = tp_base(m) + κ·(D_priv/D_priv_0 − 1)·tp_slope(m) + shock(m)`; tp_base = {3m: 0, 2y: 0.10%, 5y: 0.30%, 10y: 0.55%, 30y: 0.85%}; D_priv = duration-weighted government bonds held outside the CB (QE reduces it ⇒ lower TP). κ = 1.0%. User term-premium shocks per tenor with half-life.
- `y(m) = mean_{k<m} E[i_{t+k}] + TP(m)`.
- **Bonds**: coupons paid monthly (coupon/12 × face) by Government to holders; at maturity, face paid. Market value = PV of remaining coupons and face at current curve yield of remaining maturity.
- **QE**: buy `qe_monthly` face of the chosen tenor from Fund first, then banks (pay CB → seller: creates reserves/deposits). QT sells.
- **Fund portfolio**: keep 5% of NAV in deposits; invest excess in new bond auctions; sell bonds when below. Equity valuation: `E = 12 × smoothed_dividends / max(0.02, y10 + ERP_t − g)` with g = 3.9% (nominal trend growth, since y10 is nominal) and a time-varying premium `ERP_t = erp + κ_u·(u − u*) − θ·clip(R12 − R̄, ±c)`, bounded to [1%, 9%], where erp = 3.0%, κ_u = 0 (default), θ = 0.005, R̄ = 8%, c = 20%. R12 is the trailing 12-month compounded total return on the Fund's equity (E_t + equity dividends paid, over E_{t−1}), kept from the last 12 monthly observations. Calibration: erp and g give a mean trailing P/E ≈ 19 on after-tax firm profit, below the ≈20 that the model's ~13%-of-GDP profit share and 3.9% growth imply at Damodaran's 4.3% ERP. Evidence for the terms: Campbell & Shiller (1988) and Cochrane (2011) for countercyclical discount rates; Greenwood & Shleifer (2014) and Barberis, Greenwood, Jin & Shleifer (2015) for extrapolation. Stability limits (measured): the valuation gain is 1/cap ≈ 25, so the loop price → premium → price has gain ≈ θ/cap, and θ ≥ 0.02 drives the premium to its bounds; κ_u ≥ 0.05 raises the GDP cycle sd from ≈1.56 to ≈1.7 through the Fund wealth channel. Hence the countercyclical term is parameterised but off by default, and the cyclical correlation of the premium comes from the extrapolative term and the yield (corr(ERP, GDP) ≈ −0.35). NAV = deposits + bond MV + E. Household fund share price = NAV / total_shares. (RZ20) The households' Fund-wealth term of the consumption rule is `mpc_fund_wealth_annual / 12 × concave_wealth_w(shares × price)` with the tail weight `fund_wealth_tail_weight` (0.35 = the legacy concavity above $1 million, 1 = none) and, with `fund_wealth_ewma`, the price smoothed with the span `fund_wealth_ewma_span` (12 months by default). `rerate_wealth_mpc_annual` adds a propensity out of the wealth that a `SharePrices` re-rating alone has taken from a household's shares (a smoothed loss per share), so that a run with no re-rating is bit-identical. A user share-price re-rating (`Shock::SharePrices`, §12) multiplies each sector's E_s after this valuation.
- **RoW**: holds deposits from net imports; each month spends 1/24 of deposits on exports (goods market) and invests 1/12 in bonds.
- **Helicopter money**: CB pays households directly (in fiscal::pay_transfers).
- **Sector equity (stage 4)**: the Fund's equity is Σ_s E_s, with E_s = 12 × smoothed dividends_s ÷ max(0.02, y_eq + ERP_t + erp_sector_s − boom discount − g_s^e). g_s^e = long_growth + an extrapolative term (a fast-over-slow dividend growth signal, relative to the economy-wide signal and capped at ±1.5pp) + a productivity-news term during booms, bounded to [−2%, +6%]. Dividends are attributed to the paying sector (bank dividends to OtherServices). Tobin's q by sector feeds back to firm investment (firms.rs §7.3). Details: SECTORS.md stage 4.

**Re-fit status (Steps 3 and 10; monetary r* and the ERP).** The r* estimate is fixed at the defaults: `r_star_adapt` 0.0, `r_star_antiwindup` false, `r_star_est_min` −0.01. The adaptive path is kept behind `r_star_adapt` (0.3 in the legacy profile, which keeps the windup at Step 3). Policy averages 2.25 at 16 seeds (band 2.2–3.6; 1.6–3.6 since joint2). The demand shock is `risk_premium_sigma` 0.0002 (from 0.0007), with `erp_rp_loading` 8.75 (2.5 × 0.0007 / 0.0002), so the ERP demand term is unchanged. The TFP shock is `tfp_shock_sigma` 0.002 (legacy 0.0008). The compensated financial-factor jumps (`fin_jump_intensity`, `fin_jump_mean`, `fin_jump_sd`, `fin_factor_rho`, `rp_fin_loading`, `erp_fin_loading`, `erp_fin_build_months`) are committed with legacy-neutral defaults, so jumps are off. The jump candidate (intensity 1/96, mean 1.0, sd 0.25, `fin_factor_rho` 0.95) is an option that is not calibrated. Its asymmetry gates need recruiting capacity, so the labour-market plucking item is separate.


## 12. Shocks (shocks.rs) — user-facing levers

| Shock | Effect |
|---|---|
| OilPrice { pct, half_life_months } | log oil price jump, decays |
| PolicyRate { rate, months } | overrides the Taylor rule. Since joint calibration 4 the pinned-rate expectations follow the rule's release path with an exit hazard (`pin_path_on`), and the discretionary rate response is bounded in slack (R3: `disc_rate_knee`, `disc_rate_sat_scale`, `disc_rate_sat_slack`; it adds h(0) > 0 to the discretionary multiplier at the neutral rate, +0.54% at zero slack) |
| TaylorRule { phi_pi, phi_u, r_star } | changes the rule |
| TermPremium { tenor_idx, bps, half_life_months } | shifts one tenor (neighbours get 50% interpolated) |
| QE { tenor_idx, monthly_face_pct_gdp, months } | CB bond purchases (negative = QT); the priced duration counts each purchased lot λ = `qe_duration_weight` times (1.5 since joint3) |
| Helicopter { per_household, months } | central-bank-financed transfers |
| Rebate { per_household, months } | Treasury-financed one-off payment (RZ23): paid from the TGA with the government's other payments, refilled by the bond auction; no central-bank money; spent by the same windfall rule as `Helicopter` (`consumption::decide`); reported in `helicopter_bn_ar` |
| GovSpending { pct, months } | purchases multiplier |
| IncomeTax { pp, months } | add pp to all brackets |
| MacroPrudential { max_ltv, max_dti } | mortgage limits |
| CreditSpread { bps, months } | add to business & mortgage spreads |
| Productivity { sector?, pct } | permanent TFP level shock |
| TechBoom { months } | forces a tech boom: Tech productivity growth +5pp/yr, adopters +1pp/yr, Tech equity re-rated (stage 4) |
| SharePrices { sector?, pct, half_life_months } | re-rates share prices (R51-E): the equity valuation of one sector (0-8, `Sector::ALL` order) or of all (sector null) jumps by ln(1 + pct/100) on impact, `pct` in (-95, 300], and the jump decays with `half_life_months` (0 = permanent), as `Dollar` and `OilPrice` do. A multiplicative wedge on each sector's model value, applied in `equity::value_sectors` after the valuation; see the paragraph below |
| ImportTariff { pct } | border duty on the customs value of imported consumer goods (Durable, Nondurable, Tech) and of firms' imported inputs and equipment; lagged retail pass-through (0.075 a month, 61% of the duty at m12), importer absorption, tariff-elastic import shares (0.30 short run, 1.2 long run), the duty is government revenue and sits in CPI, BEA treatment of duties in real volumes; `tariff_pass_through` = false restores the legacy lump-sum payment (docs/CALIBRATION.md, b7-tariff) |

**SharePrices (R51-E, `shocks.rs`, `equity.rs`).** The model had no input that sets share prices: they moved only through earnings,
rates and the risk premium, and the tech boom re-rates Tech upwards through a lower capitalisation rate (§11.4). `SharePrices` is the
re-rating in either direction. The shock state keeps one entry per shock (sector or all, log size, monthly decay factor `0.5^(1/half-life)`,
1 for a permanent one); each month `activate_due` decays every entry by its own factor, adds the shocks due, and drops an entry that has run
out. `ShockState::share_price_wedge` returns exp of the summed log sizes by sector, or `None` when no entry is active (every run without the
shock). `equity::value_sectors` multiplies each sector's value E_s by its wedge **after** E_s is formed from the smoothed dividends, the
growth signal, the discount rate and the premium, so the shock is a level wedge on the price and works the same in the 1/cap and the
log-linear forms and where `LOG_CAP_GUARD` binds (a cut to the capitalisation rate would not move Tech where the guard binds). The dividends,
the growth signals, the news state, the premium and the discount rate are exactly the ones the model would have had. The wedged value is the
one that reaches `sector_equity`, Tobin's q, the Fund's NAV and share price, and the month's equity return. The normal q (the reference for
the rebase at the end of the burn-in) is not updated while a re-rating is in force in the sector, as it is not during a boom. (RZ24-R1) With
`credit_shock_erp` on, a sector that an active re-rating covers (`ShockState::share_price_covered`: an entry aimed at all sectors or at that sector,
whatever its size) is valued at the premium without the credit loading, because the typed path already sets that market; the sectors it does not cover keep
the loading. Nothing else reads the shock.

What carries it on, all existing channels. (1) Household wealth: the Fund's share price falls by Tech's (or the sectors') share of the Fund's NAV, and
households' Fund wealth is read by the consumption rule (`mpc_fund_wealth_annual`, 2% a year, concave in wealth), the housing down-payment
and wealth tests and the capital-gains tax. Households decide before the valuation step, so consumption moves from the month after the impact.
(2) Tobin's q: the q that investment reads is an EWMA of the raw ratio (`q_signal_decay` 0.9), and only sectors with `investment_q_sens` set respond;
by default that is Tech (0.2, with q's factor clamped to [0.75, 1.33], so Tech's investment can fall by at most 25% through q). (3) The risk premium: the
12-month return enters the equity risk premium through the extrapolative term (`erp_extrap` × (R12 − anchor), the momentum capped at ±20%), so after a
fall the premium is higher for about a year and valuations fall a little more (about 0.08 to 0.11 pp of premium in the runs of
docs/CALIBRATION.md, R51-E). (4) The Fund's liquidity rule keeps deposits between 2% and 8% of NAV, so a lower NAV changes the months in which it
sells or buys bonds. A fall of the price with unchanged dividends is a re-rating (a higher dividend yield), not lower earnings: to ask for a bust that
also cuts profits, combine it with a demand or productivity shock. There is no equity issuance in the model, so firms' financing is not touched
directly. The shock adds no `Metrics` field (the records and the fingerprint are unchanged); read it in `equity_value_by_sector`, `fund_share_price`,
`equity_risk_premium` and `equity_return_12m`.

Shock comparison runs **baseline and shocked worlds with the same seed** (common random numbers) and reports
differences (impulse responses), averaged over several seeds in parallel (rayon).

## 13. Calibration targets (t=0, US ~2025/26)

### 13.1 Macro
Unemployment 4.3%; core inflation ~2.8%; policy rate 3.75%; curve 3m 3.8%, 2y 3.5%, 5y 3.7%, 10y 4.1%, 30y 4.7%;
30y mortgage 6.3%; WTI $65; gasoline $3.10; government debt held by public ≈ 100% GDP, CB holds ≈ 15% GDP of Treasuries;
bank reserves ≈ 10% GDP; homeownership 65.5%; median house $410k; median rent $1,550; average outstanding mortgage rate 4.3%.
### 13.2 Households
Head-age shares: 18–24 5%, 25–34 16%, 35–44 18%, 45–54 17%, 55–64 19%, 65+ 25%.
Median household earnings of working heads: the earnings target is not `Config::median_earnings_annual` (75,000; unused by the build, kept for config compatibility; the build targets the CPS earner-household level, see `init.rs`); wealth: median net worth ≈ $190k, top 10% hold ≈ 67% of net worth
(fund shares Pareto α≈1.5 concentrated in top decile); ownership by age: <35 37%, 35–44 62%, 45–54 70%, 55–64 75%, 65+ 79%.
62% of owners have a mortgage; mortgage age uniform over 0–15 years; outstanding rate drawn from historical 30y rates
of the origination year (2010–2021 ≈ 3–4.5%, 2022–2025 ≈ 6–7%).
### 13.3 Input–output coefficients A[s][j] (input from s per $ of output of j)

| from \ to | Energy | Food | Goods | Services | Construction |
|---|---|---|---|---|---|
| Energy | 0.10 | 0.05 | 0.04 | 0.02 | 0.04 |
| Food | 0.00 | 0.20 | 0.01 | 0.03 | 0.00 |
| Goods | 0.05 | 0.10 | 0.25 | 0.06 | 0.30 |
| Services | 0.08 | 0.12 | 0.12 | 0.20 | 0.15 |
| Construction | 0.03 | 0.00 | 0.01 | 0.02 | 0.02 |

Energy sector crude intensity: 0.35 of output value at baseline oil price.

### 13.4 CPI weights
Shelter 0.35 (rent index), Energy 0.07, Food 0.13, Goods 0.19, Services ex-shelter 0.26.

## 14. Validation (stylised facts the model should reproduce; Dosi et al. 2010; Caiani et al. 2016)
1. Unemployment fluctuating around 4–5% without shocks; inflation near 2–3%.
2. Investment more volatile than GDP, consumption less volatile than GDP.
3. Phillips curve (negative u–wage-inflation correlation), Okun's law, Beveridge curve.
4. Right-skewed firm size distribution; Pareto wealth tail; wealth Gini ≈ 0.8, income Gini ≈ 0.45.
5. **Impulse responses with correct signs**: +100bp policy → mortgage rate ↑, house prices ↓ (−3 to −8% over 2y), investment ↓, unemployment ↑ (+0.3–0.6pp), inflation ↓ (−0.2–0.5pp after 12–24m).
   +50% oil → headline CPI ↑ ~1.5pp quickly, core small, real consumption ↓, energy profits ↑.
   QE 5% of GDP at 10y → 10y yield ↓ 15–30bp, mortgage rate ↓, house prices ↑.
   Helicopter 5% GDP → consumption ↑ then inflation ↑.

## 15. Performance
- Target: 50,000 households, ~2,000 firms, 5 banks; ≥ 100 simulated months per second on a laptop core-set.
- Data layout: dense `Vec`s indexed by u32 ids; no HashMaps in hot loops; per-agent deterministic RNG
  `rng::agent_rng(seed, tick, stream, id)` so parallel (rayon) decisions are reproducible.
- Pattern: compute decisions in parallel (`par_iter` over immutable world → Vec of intents), apply them sequentially.

## 16. Cross-module responsibilities (authoritative — resolves any ambiguity above)

| Concern | Owner | Notes |
|---|---|---|
| Out-of-labour-force flows, retirement, death, inheritance, new 18-year-olds, students finishing school | demography | On death: `common::separate_worker`; move deposits (`ledger::pay` dead→heir), shares (`ledger::transfer_shares`), houses (`ledger::transfer_house`), mortgages (`ledger::reassign_loan`); set the dead's home `occupant = None`; if the dead was a renter just vacate. Leave the slot empty (`alive = false`, no money, housing or job); an entrant later reuses it as a new 18-year-old (WithParents, fresh preferred firms). |
| Leaving parents (WithParents → seeking a rental) | housing | probability rises with age & income. |
| `hh.career_earnings`, `career_months`, `months_in_job`, `months_unemployed`, `reservation_wage` | labor | once per month for each household. |
| Wage payment (`pay Firm→Household`, `hh.income.wage`, `firm.wage_bill` incl. employer payroll tax paid to Government) | firms::produce | `flows.wages` += gross wages. |
| `firm.depreciation_cost` (nominal, = depreciated units × Goods price) | firms::produce | |
| Firm working-capital: if deposits < wage bill + inputs, call `banking::request_business_loan` | firms::produce | Firms may overdraw; banking charges overdraft interest. |
| `firm.input_cost` | goods::clear (inputs bought) + firms::produce (crude imports paid to RoW) | Domestic crude share is not a payment (it is upstream profit retained by the Energy firm). |
| `firm.revenue`, `firm.sales`, `firm.demand`, `hh.consumption`, `fills.*`, `flows.consumption/investment/government/exports/imports/intermediate` | goods::clear | Also updates `w.prices.sector`, `cpi`, `core_cpi`, pushes `cpi_history`, `core_history`. Shelter component of CPI uses `prices.rent_index` relative to its t=0 value. |
| Export orders | goods::clear | computed from `cfg.export_base_share_gdp` × `common::gdp_month_model` × (P_world/P_s)^1.5, paid by RoW. |
| Household budget, sector split, expected inflation, permanent income, maintenance → Services order, deposits↔Fund rebalancing | consumption::decide | Store any per-household memory in `ConsumptionState`. |
| `hh.housing_cost` (scheduled mortgage payment + rent + property tax), rent & property-tax payments, `prices.hpi`, `rent_index`, `hpi_history`, all housing `flows` | housing::step | Mortgage payments themselves are made in banking::service. |
| Loan payments (all kinds), delinquency counters, `firm.interest_paid`, deposit interest (`hh.income.interest`), overdraft interest, IORB, discount window, bank P&L & dividends to Fund | banking::service | Households short of cash for a mortgage payment first sell Fund shares (`ledger::fund_sell`). |
| Write-off of business loans & bankruptcy; firm entry funded by the Fund (`pay Fund→Firm`) | firms::close_books | |
| Mortgage foreclosure & write-off (net of sale proceeds) | housing::step | |
| Corporate tax on `pretax = revenue − wage_bill − input_cost − interest_paid − depreciation_cost` (firms) and on bank profit | fiscal::collect_and_finance | sets `firm.taxes`; firms::close_books then computes `profit = pretax − taxes`. |
| Household income tax & employee payroll tax (`hh.taxes`), transfers, purchases, bond auctions | fiscal | |
| Coupons & maturities (`ledger::service_government_debt`, records `gov.interest_paid`), QE, bond market values (`bonds_value` on banks/CB/Fund/RoW), Fund dividend distribution to households (`hh.income.dividends`), Fund NAV & share price, RoW & Fund liquidity | monetary::markets | |
| Taylor rule, `cb.helicopter_per_household` from shocks | monetary::set_policy | Helicopter payments made by fiscal::pay_transfers via `pay(CentralBank → Household)`. |
| Aggregates & distributions | metrics | |

Model-unit helper: `common::gdp_month_model(w)` = nominal GDP per month in per-agent dollars (unscaled).

## 17. Integration round 4: new mechanisms (where to look)

Branches merged into `integrate` (see docs/CALIBRATION.md for the wiring, the knob changes and the open issues). Only the
mechanisms are listed here; numbers are in CALIBRATION.md.

- **Demography (demography.rs, §5):** retirement hazards 1.0%/month at 68–74 and 0.6%/month from 75; headship-weighted
  native entry ages; immigrant bands from CBO; household formation base averaged over 20 years. Retirement sets no benefit:
  claiming belongs to fiscal.rs.
- **Firms (firms.rs, §7):** Energy extraction labour scales with the lagged real oil ratio (`extraction_factor`), so drilling
  labour per unit of output follows oil. Planned vacancies and poach slots are split by hires (`LaborState::hires_at`).
  Imported inputs are priced off the lagged pass-through import price.
- **Trade (goods.rs, §8.3):** `import_pass_through` moves a lagged state z toward `import_pass_through_long` × the FX move
  at `import_pass_through_speed` a month (Campa & Goldberg 2005). Energy export prices are oil-linked (`world_sector_factor`).
  Realised exports are kept by sector, so metrics can deflate each by its own price.
- **Monetary (monetary.rs, §11.4; shocks.rs, §12):** `PolicyShift` is an AR(1) gap with a stated mean duration; the dollar carries a monetary-surprise premium κ = `fx_mp_premium` (1.25 since joint3) on PolicyShift shocks only (rule-driven tightening gets none); the rule's
  shadow rate carries the gap; `QE` and `MbsQE` buy at a tenor or in agency MBS. MBS purchases lower the mortgage base
  rate through `mbs_spread_adj` (banking.rs). Tech q is a 10-month EWMA, rebased at the end of burn-in.
- **Fiscal (fiscal.rs, §9):** Social Security is paid to any claimed head (62+, age hazard, claim-age factor with 70%
  at 62, 124% at 70), and early claimants count as pension income in mortgage qualifying income. Means-tested support
  is asset-tested and net of UI; EITC, SNAP and SSI are separate lines. The central bank has a deferred asset and a
  remittance. Government purchases split into health (in-kind, to consumption) and non-health (government deflator).
  The t=0 debt stock follows `maturity_stock_mix` (WAM about 5.9 years).
- **Households (consumption.rs, §8.1):** patient and impatient types drawn on `STREAM_HH_TYPE` at the first decision
  of burn-in. One-off payments (helicopter money, rebates) go to `IncomeMonth.windfall`, spent under a front-loaded
  durables-tilted windfall rule.
- **Labour (labor.rs, §6):** job ladder with on-the-job search (`on_the_job_search_mult`) and a job-to-job premium;
  match-specific pay draws on `LABOUR_MATCH_PAY`; unemployment exit draws on `LABOUR_UE_EXIT`; UI exit base and duration
  cap. Non-participants search at half intensity.

All new RNG draws use keyed streams registered in `rng::stream` (checked by a collision test).

## 18. Exogenous factors (p0-exog, exog.rs)

Stochastic states on `Exogenous`, advanced once a month by `exog::step` before the policy, curve and FX steps. Each
process draws from its own registered stream (`rng::stream::{FIN_FACTOR, TERM_LEVEL, TERM_CURV, FX_PSI, OIL_EXT, OIL_JUMP}`),
so turning one on never shifts another path. **Every new sigma, intensity and loading defaults to 0**, so with the defaults
the outputs are bit-identical to the pre-p0 model. Readers are accessors on `Exogenous`: `fin_factor()`, `tp_level()`,
`tp_curv()`, `fx_psi()`, `oil_vol_state()`. The consumers (equity, curve, FX, oil) are wired in later batch-3 steps.

- **Financial risk factor f_t** (`fin_factor_*`, `fin_jump_*`). f_t = ρ f_{t−1} + σ·exp(γ f_{t−1})·ε_t + J_t. ρ = 0.98
  (Bekaert, Engstrom & Xu 2022). The volatility multiplier is clamped to [0.2, 5]. J_t is Poisson with intensity λ and
  N(μ_J, σ_J) sizes; μ_J > 0 is risk-off. Defaults: σ = 0, γ = 0, λ = 0. Analytic checks: AR slope ρ, sd σ/√(1−ρ²),
  jump frequency 1−e^{−λ}, and E f = λμ_J/(1−ρ).
- **Term-premium factors** (`tp_level_*`, `tp_curv_*`). Level: AR(1) with ρ = 0.98 (Diebold & Li 2006), plus a loading
  `tp_level_fin_loading` on the current f_t for flight to quality (negative = premium falls in risk-off). The loading is
  added to the AR state `tp_level_ar` and is not fed back into it, so tp_level_t = tp_level_ar_t + L·f_t (slope on f_t = L,
  not compounded by ρ). Curvature: AR(1)
  with ρ = 0.95, the Nelson–Siegel hump factor (Diebold & Li 2006; Litterman & Scheinkman 1991). Defaults: σ = 0, loading 0.
- **Dollar risk premium ψ_t** (`fx_psi_*`). AR(1) with ρ = 0.99 (Itskhoki & Mukhin 2021), added to ln fx after the
  partial-adjustment step. Its innovation is corr·ε_t + √(1−corr²)·u, where ε_t is f's diffusion innovation (the draw
  before the jumps), so `fx_psi_fin_corr` is the correlation of ψ's innovation with ε_t, not with the level of f
  (the level correlation is somewhat lower, about 0.94 times it, when γ = 0 and there are no jumps). Default 0; a
  risk-off dollar rise needs corr > 0. ψ does not see f's jumps unless `fx_psi_fin_jump_loading` λ ≠ 0 (default 0), which
  gives ψ_t = ρψ_{t−1} + σz_t + λ·J_t, with J_t the month's f jump sum. Default σ = 0.
- **Oil options** (`oil_t_nu`, `oil_garch_*`, `oil_jump_*`, `oil_index_to_world`), applied to the existing oil AR(1)
  in logs (Hamilton 2009; Alquist, Kilian & Vigfusson 2013).
  - Student-t innovations: z·√((ν−2)/V), V ~ χ²(ν), which is t(ν) rescaled to unit variance; ν is raised to at least 2.5;
    ν = 0 is Gaussian (the pre-p0 process). Below ν = 4 the kurtosis is infinite, so the draw is winsorised at ±8 sd
    (`OIL_SHOCK_CAP`). Only the t draw is capped, so the Gaussian path is unchanged. The winsorising changes the moments.
    The cut is 0.105% of draws at ν = 2.5, 0.035% at ν = 4 and 0.007% at ν = 6. The winsorised variance is 0.73, 0.89 and
    0.98 at ν = 2.5, 3 and 4, and about 1.00 by ν = 10 (not 1 for every ν > 2). Excess kurtosis is 5.6 at ν = 4 and 2.45 at
    ν = 6, against 6/(ν−4) = 3 untruncated at ν = 6.
  - GARCH(1,1): h_t = σ²(1−α−β) + α ε²_{t−1} + β h_{t−1}, so the long-run sd is `oil_sigma`. α = β = 0 gives the constant
    sd. The conditional sd is clamped to [0.2, 5]·σ (`OIL_VOL_MULT_RANGE`, the same range as f_t) and the clamped variance
    is stored. `oil_spec` requires α, β ≥ 0 and α + β < 1. Check: lag-1 autocorrelation of ε² equals
    α(1−αβ−β²)/(1−2αβ−β²) (Bollerslev 1986). The 5σ cap leaves this check unchanged; a 3σ cap moves
    it to 0.26.
  - Level cap: the oil log deviation is clamped to ±1.5 (`OIL_LOG_DEV_CAP`), about ×4.5 either way. The cap applies
    whenever an extension (t, GARCH or jumps) is on. The plain Gaussian path is never capped, so it is the pre-p0
    recursion for any σ and ρ. At the base parameters (stationary sd 0.19) the cap is about 8 sd and does not bind.
    At the plan-9 parameters (σ 0.085, ρ 0.985, stationary sd 0.49) it binds in 0.16% of months. A fit of ρ to a sample
    that reaches the cap is biased, so calibrate with the cap in view.
  - Jumps: Poisson intensity `oil_jump_intensity` with symmetric N(0, `oil_jump_sd`) sizes in the log price. They mean-revert
    with the AR(1) and apply only when `stochastic_oil` is on. Jumps draw from OIL_JUMP, so changing ν leaves the jump
    months unchanged (`oil_jump_path_does_not_depend_on_t_nu`).
  - `oil_index_to_world`: oil_price_t = oil_base × (1+π*)^((t − B)/12) × e^(ar + shock), where π* is `inflation_target`,
    t is calendar time (tick + tick_offset) and B is `burn_in_months`. The index is normalised to reported month 0, so it
    is 1 there and continuous across the burn-in reset. `oil_base` stays the anchor that every `oil_ratio` consumer
    divides by (`common::oil_ratio`), so the index reaches firms' crude cost, the Energy oil leg, the world energy price
    (`world_energy_oil_elasticity`) and gasoline. Extraction labour is the exception: it is a productivity divisor, so
    firms use the real ratio `common::real_oil_ratio` (oil ratio ÷ index, smoothed in `firms_state.oil_ratio_lagged`).
    Without that, labour per unit of Energy would rise with the price level (about 2× by month 479 at the default target, a 40-year run).
    It grows at the domestic
    target, not at the import-price trend `world_goods_inflation` (0.5%/yr). With CPI at target, real oil (oil / CPI) has
    no drift. On in the config default since the re-fit (`oil_index_to_world` true; the pre-re-fit state is in legacy_p1off.json). Two consequences for item 9. (1) The world energy
    price follows the world goods trend and no longer drifts at π*. Before p1-energy the world factor used the nominal
    oil ratio, so with the index on the Energy world price rose at π* on top of the trend. With `energy_fuel_split` on,
    the factor is the real oil ratio raised to the fuel line's crude share (`common::world_sector_factor`), which is flat
    when CPI tracks the target (`world_energy_factor_has_no_trend_with_index_on`). With `energy_fuel_split` off the old
    nominal factor is kept, so the index must not be used with it. (2) oil_price is below `oil_base` during burn-in
    (index below 1).
  - The oil's base Gaussian draw stays on the EXOG stream. The Student-t scale draws use OIL_EXT and the jumps use
    OIL_JUMP. The risk-premium and TFP draws that follow on EXOG are therefore unchanged by the oil options.
- **Tests.** Each process runs alone for 200k months on its stream and is checked against the analytic values above.
  `all_defaults_reproduce_base_exog_path` replays the pre-p0 oil recursion bit for bit.
  `oil_index_grows_at_domestic_inflation_so_real_oil_has_no_drift` checks the index in `oil_ratio` (deterministic), and
  `extraction_labour_sees_no_real_oil_drift_with_index_on` checks that the extraction factor on the real ratio stays at 1 over
  240 months with the index on, while the nominal ratio would raise it.
  `real_oil_cost_seen_by_firms_has_no_trend_with_index_on` checks that the stochastic real oil cost has no OLS trend with
  the index on and about −π* a year with it off. `gaussian_oil_path_is_the_pre_p0_recursion_for_any_sigma_and_rho` checks
  the Gaussian path bit for bit. `dollar_psi_responds_to_fin_jumps_only_through_the_loading` checks the ψ jump loading.

### 7.8 Firm heterogeneity switches (b5-firmhet S2, S4, S6; all off)

Three further switches, each legacy-off in the `// b5-firmhet` block of `config.rs`, with its measured on-state in
`docs/CALIBRATION.md` under `## b5-firmhet`:

- **S2, size-linked markup norm** (`markup_size_kappa`, `markup_size_comp_max`, `markup_size_q_min`). The norm gains
  κ·max(0, ln(q/q_min)) for relative expected demand q, less a capped sector compensation. Energy is excluded. Off:
  `scripts/profiles/b5-markup.json`.
- **S4, lumpy investment** (`inv_lumpy`, `inv_inaction_band`, `inv_catchup_band`, `inv_catchup_months`,
  `inv_catchup_cap`). Firms above the sector's capital-weighted mean gap by more than s_hi order nothing. Firms below
  it by more than s_lo catch up within a capped monthly rate. Off: `scripts/profiles/b5-lumpy.json`.
- **S6, level calibration** (`upstream_margin` 0.05, `target_utilization` 0.83): the default since joint2. Legacy values
  0.10 and 0.85 are in `scripts/profiles/legacy_j2off.json`; `scripts/profiles/b5-calib.json` is the same overlay.

S1 (entry viability, §7.7) and S0 (§7.6) are unchanged.

## 19. World block (b8-world, stage 1: blocs, offline solution, shadow coupling)

Off by default (`world_block_enabled` false). With it off the output is byte-identical to the base (the default md5 of
`run 5000 60 3 --json | tail -n 1` is unchanged, and `Metrics.world` is not serialised). Stage 1 adds twelve semi-structural
bloc models (EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW) that see the US and do not feed back into the ABM yet.
Specification: `docs/world/world_spec.md` (rev 3, sections 1, 2, 3 stage 1, 4 W1-*); decisions D1-D36 and the orchestrator rulings
are in `docs/world/DECISIONS.md` (copies of the review files). Calibration record: CALIBRATION.md, "b8-world S1".

**Files.** `crates/econsim-core/src/world/`: `mod.rs` (`WorldBlock`, `Bloc`, `init_if_enabled`, `after_exog` and `settle`
(empty in stage 1), `step`, base capture, replay record), `params.rs` (the two embedded JSON files, `Par` = P* times the
`world_*` multipliers), `bloc.rs` (one month of one bloc: forcing paths over K = 240, kernel sum with the tail, state shift,
histories, user paths, `demand_scale`), `net.rs` (`UsInputs`, `us_inputs` from the ABM, the cross-bloc `Net`), `wmetrics.rs`,
`tests.rs`. Data: `data/world_params.json` and `data/bloc_solution.json` (generated by `scripts/world/build_world_params.py`
and `scripts/world/solve_blocs.py`; both `--check` modes verify the calibration hash and the numbers), `data/golden/*` (Python
reference outputs, reproduced by `scripts/world/make_golden.py --check`). Research inputs: `data/world/research/`.

**State.** Per bloc: the 33-state `s` (y lags 1-3, i lag 1, z lags 1-13, pi lags 1-13, mrr lags 1-3), the 14-month output
histories, G (global financial state), the risk-premium state u, the Okun gap state, and ln P (accumulated from creation).
Per block: the US history (`ius`, `y_us`, unemployment, Dollar level, oil gap), the bloc clock `n`, the activated bloc shocks
with their activation month on the bloc clock, and the bases (ln P, ln S and ln PT at the first reported month, D36).

**Tick order (stage 1).** `exog::step`, `world::after_exog` (empty), `set_policy`, `update_curve` (records `CurveInputs`),
`exchange_rate`, `world::step`, then the rest of the month as before; `firms.close` ends with `world::settle` (empty).
`world::step` reads the policy rate, the curve state and the expected path of this month, and the unemployment of the
previous month (`us_inputs`). The blocs run through burn-in in shadow; the bases are captured at the first reported month.

**Equations as implemented.** Forcing: IS (activity through F and the partner term Peff, the financial term theta G, the oil
and OPEC+ terms, the user demand paths, the eps innovation at k = 0), PC (the REER-gap change in Peff), rule (k_us times the
M-F-bounded expected path, the policy pin paths, the rule innovation), UIP (-(bounded expected path + u rho^k + lambda_rp G
projected)). The solution is the period-0 map C plus the kernels IR[eq][k] (k < 240) and the tail sums: x = C s + sum_eq
sum_k IR f + sum_eq tail f[K-1]. The M-F bound is b7-transmission's `bounded_rate_differential` (identity at scale 0).
Peff uses the Dollar addend a_t (realised, then a_t d^(j-t)) and the partner real rates of the previous months.
US inputs (`us_inputs`): `ius = 100 (policy - i_inf) - world_ius_offset`, `ex[k] = 100 (path_k - i_inf) - offset 0.97^k`
(path from `monetary::expected_rate_path`, which mirrors `update_curve`'s three branches), `y_us = 100 okun (u* - mean of the
last three unemployment rates) - world_yus_offset` (N1: the raw gap has a permanent negative mean, because the real-time u* is
capped below the mean unemployment rate; the constant is measured once, see CALIBRATION "b8-world S1").

**Shocks** (`shocks.rs`, H14): `BlocPolicy{bloc, bps, months}` (pin table of the bloc; months in {3, 6, 12, 18, 24}),
`BlocDemand{bloc, pct, half_life_months, risk_off}` (isolated run scales the unit path to the peak `pct`), `GlobalDemand`.
They are validated always and ignored when the block is off. The block takes each fired bloc shock once, on the bloc clock.

**Metrics.** `Metrics.world` (H11), first group: `y_gap`, `pi_gap`, `i_gap`, `z`, `infl`, `policy` (floored), `s_index`,
`p_index` (1 at the first reported month), `headline` (= infl in stage 1), `u_gap` (zero for OPEC+ and RoW, which have no Okun
data). Serialised only when the block is on.

**Hooks (complete list of stage 1).** H1 `pub mod world`; H2 `sim::step`/`step_timed` (`world::after_exog`, `world::step`,
`world::settle`, PHASES 21 with "world" after "fx"); H4 `World.world`; H5 `monetary::update_curve` (`record_curve_inputs`,
`#[inline(never)]`, arithmetic-free) and `expected_rate_path`; H11 `metrics::record`; H12 `build_world` (after
`calibrate_purchases`) and `tiny_world`; H13 the four stream ids (`WORLD_BLOC` is drawn, the three others reserved); H14 shock
variants; H16 the fourteen `world_*` keys; H18 `econsim-cli world-dump`. Hooks H3, H6-H10 belong to stages 2-4. Each hook kept
the default md5 (I1) when added. Hot-path helpers are `#[inline(never)]`.

**Identity and parity.** I1: default md5 a9c633911b83e823bb069e291ba30a61 (the master default when stage 1 was built; since joint calibration 4 the default is raw 9f90c88a2c8b653857029d235cd6b0a6, normalised e11bff85…, and I1 is checked against that). I2: 32 seeds, block off, identical to base. I4: 32
seeds, block on with every link off, every non-world key bit-identical (`json_strip_compare`: "stripped 1 new keys"). W1-23: a
dumped ABM run replayed by the Python reference matches to 6e-14. The reference replay of the stored golden inputs matches to
1e-10. The `world` key is absent when the block is off.

**Determinism.** The block is serial. Draws come from `agent_rng(seed, tick, WORLD_BLOC, bloc)` as four normals per bloc and
month, always taken (scale zero only zeroes the innovations). Thread-count independence is tested (`shock_draws_keyed_and_thread_independent`).

### 19.1 Stage 2: trade and prices (`world/trade.rs`, link `world_link_trade`, off by default)

Specification: `docs/world/world_spec.md` section 2.5, stage 2 in section 3, W2-* in section 4; rulings in `docs/world/DECISIONS.md`
(D-R8 set-level `ImportTariff` with bilateral `Tariff` components added on top, decimal tau; D-R9 import elasticities in the profile,
not in the defaults; D36 bases at the end of the last burn-in month, fix round). Calibration record: CALIBRATION.md, "b8-world S2".

**Files.** `world/trade.rs`: `TradeTables` (base origin shares omega0 = column shares of the 2024 imports, export base shares
x0 = exports over the 2024 GDP of 29,381 bn, theta, m_jk = the import-share matrix oriented as importer by exporter, the
normalised H10 weights), `TradePar` (the stage-2 `world_*` keys), `TradeState` (state and this month's outputs), `TradeBase`
(bases captured at the end of the last burn-in month), `TradeOut` (imp_rel by sector, imp_rel_cpi, tau_eff per channel, exports,
tau_by_origin). `world/tests_trade.rs`: 28 tests. `params.rs`: the `trade` block of `world_params.json`, written by
`scripts/world/build_world_params.py` (`build_trade`; the calibration hash is unchanged, `--check` passes).

**Tick (stage 2).** `world::step` takes the block and runs: capture of the trade bases at the end of the last burn-in month (in `settle`, after that month's settlement; a world without burn-in
at its first step; no ABM state is overwritten, D36); `pre_bloc` (the three-month ring of the US import-demand term, its deviation
from the slow benchmark of S2-R5, read by the network only in coupled mode); the bloc step (`core`, network with the
import-demand term when coupled); `post_bloc`. `post_bloc` computes, in order: c_t = 100 [ln(fx/fx0) + ln(PT/PT0) - ln(P/P0)]
(the ABM's own real dollar against the base), z~_j = c_t + (z_j - sum_k wH10_k z_k), the dominant-currency and non-dollar
states Phi^d, Phi^nd and the origin log prices r_j (Energy r = 0), the duties tau_{j,s} = min(5, T + bilateral components), the
duty-inclusive prices, the lagged origin states zw and the origin shares omega, the import prices imp_rel_s = imp_level (PT/PT0) exp(rbar_s)
and imp_rel_cpi, the channel duties tau_eff^hh and tau_eff^firm (weights: last month's imports by sector over the CPI-tradable
and tradable sectors), and the exports X_{s,j} = x0 trend GDP exp(eps_y yhat_j + lnA^fx_j + lnA^rp_{s,j}) with R = 1. Energy's
competitor term in the export price is ln(world energy factor), not the bloc competitor index. `settle` (after `goods::clear`)
allocates the month's customs imports by origin (omega times imports by sector) and the realised exports by destination (pro rata
to X_{s,j}), updates the real-import states Q of the import-demand term, the volume indices and last month's import shares.
Nothing in `settle` touches a balance (the ledger identity holds; W2-11).

**ABM hooks (stage 2).** H6 (`goods::clear`, the import-price block): with `trade_active` the terms ratio uses the block's
imp_rel by sector and `st.import_rel` takes imp_rel_cpi; the CPI tradables price takes imp_rel_cpi too (the CPI import component);
the legacy `import_pass_through` and its state are not touched. H7 (`goods::clear`, step 8): exports are the block's sum; the
legacy lagged export state `trade_export_z` is left alone. H8 (`world::tau_for`, `#[inline(never)]`): the household duty of
`goods::clear` and `metrics` (`tariff_rate_pct`) read the household channel, the firm duty of `firm_stage` and of
`firms.rs` `goods_input_mult` read the firm channel; with the link off each site takes the literal b7 expression.
H9 (`imports_by_sector`, `Flows`, serde default): household imports by sector beside `imp` (`PayTotals.imp_s`), firm unmet imports by
sector beside every `st.unmet` increment (`Draw.sector`, `GoodsState.imp_s`, flushed in `finish_firm_stage`), and imported crude
(`firms.rs` close_books) credited to Energy. The sum equals `row.imports_this_month`. Duties are excluded (customs value).

**Shocks.** `Tariff{partner, sector, pct, retaliate}` (`shocks.rs`): validated (bloc name, sector 0-8, pct in [0, 500]), pushed to
`world_shocks` by `activate_due` (the block consumes it with its index), a bilateral component in decimal (pct/100) that adds on top of
the set-level `ImportTariff` level (replaced, not added, D34). Components are permanent. `retaliate` is recorded (the rule is stage 4).

**Metrics.** `Metrics.world` (stage-2 keys, all finite, absent with the block off): `imports_bn`, `exports_bn` (annualised $bn,
bilateral), `r_import` (percent), `tau_eff_hh`, `tau_eff_firm`, `tau_by_origin` (decimal), `import_volume_idx`,
`export_volume_idx` (100 at the month before the base; nominal over the PT ratio, relative), `import_price_idx` (100 x imp_rel_cpi),
`ret_tau` (zero until stage 4), `us_import_gap` (percent). Diagnostics beyond spec 1.6: `z_tilde[12]` and `c_t`, used by W2-15 and W2-16.

**Deviations from the spec text (each recorded in CALIBRATION "b8-world S2").** (1) Imported crude is credited to Energy in
`imports_by_sector` (otherwise the sum would miss the crude imports that `row.imports_this_month` includes). (2) The firm share factor
of b7 (`tariff_step`) is computed from the household channel; the firm channel enters the duty (`tau_eff_firm`), so the two channels
differ only through their sector weights. (3) wH10 is normalised to sum one (the table sums to 100.01). (4) The 2024 US GDP used
for the export base shares is 29,381 bn as in the spec; the research openness table gives 29,298 bn. (5) The trade states start at
zero at the base month (the end of burn-in) and do not step through burn-in; the initial omega is omega0 and m_s is the base import total
until the first settled month. (6) The import-demand term deflates imports by the PT ratio and the origin price; Y is last month's
metrics real GDP; its benchmark is the mean of the first twelve observations, then an EWMA with a 60-month half-life (S2-R5).
(8) Fix round (S2-R1): the dynamic terms read the change of z~ since the base month and Energy's competitor term the change of
ln world_f since the base month; the export level has a per-sector scale kappa_s; the import level starts at the base-month
legacy level. The deflator of real imports is the post_bloc PT ratio (F4). (7) Energy's export competitor term follows the spec; `trade_hand.py` applies the bloc term to all sectors (see
the W2-02 note in CALIBRATION).

### 19.2 Stage 3: world oil market (`world/oil.rs`, `world/oil_check.rs`, link `world_link_oil`, off by default)

Specification: `docs/world/world_spec.md` section 2.4, stage 3 in section 3, W3-* in section 4, and the rev 5 change log (ruling S3-R1).

Shipped state: the link is off and `world_oil_exact_path` is false in `scripts/profiles/b8-world-s3.json` (stop rules S3-c and S3-d fired; CALIBRATION "b8-world S3").
Calibration record: CALIBRATION.md, "b8-world S3". The market is one world price for the real Brent price, cleared every month in
closed form (`OilState::step`, the research prototype's order of updates) against thirteen demand regions (the US and the twelve
blocs, D12 coefficients), OPEC+ (price floor and ceiling, spare-capacity cap, the market-share regime as an assumption), US tight oil
(two lags), other conventional supply, an inventory sector, and the monthly-average weight w = 0.789. The shocks are the research's
oil-specific innovation (GARCH Student-t, compensated jumps; `WORLD_OIL_I`), disruptions and OPEC+ surprises (`WORLD_OIL_S`), and the
user shocks `OilSupply` (ramp, hold, recover; the Hormuz path) and `OpecCut` (a phased policy surprise). The activity of each region
is the bloc output gap of the previous month, so the market draws no activity noise of its own (D5).

Interface (per month, `world::after_exog`, hook H3 in `exog.rs`): the market steps in shadow through the burn-in and writes the ABM oil
price only in reported months. The ABM log deviation is the decaying splice `A_t = p_t - offset + G_t`, `G_t = kappa rho^(t+1)`,
with `kappa` set at the last burn-in month from the legacy level and the shadow price (S3-R1; the reasons and the sensitivities are in
CALIBRATION.md). The blocs receive `A_t` through their oil channel and the OPEC+ output deviation through their OPEC+ channel. A live
legacy `OilPrice` shock is pinned on the exact path by copy U (the monthly-average price equals the structural price plus the shock),
which is dropped once the shock has decayed (1e-4). `world_oil_exact_path` false adds the shock to the ABM log price instead.

The legacy oil draws are still taken with the link on (H3 guards only the price assignment), so every later EXOG draw is unchanged
(test `legacy_oil_draws_still_taken`); with the link off the output is the legacy output bit for bit (test
`oil_active_off_leaves_exog_untouched`, and I4 in CALIBRATION.md).

Metrics (`Metrics.world`, stage-3 group, present only when the block is on): `p_bar_pct`, `brent_usd`, `oil_supply_pct`,
`oil_demand_pct`, `opec_out_mbd`, `us_prod_mbd`, `stocks`, `spare_pct`, and the diagnostic `oil_dev_pct` (the ABM deviation).

Isolated checks: `econsim-cli oil-check [SAMPLES]` prints K1 (identity), K2 and K3 (the research IRF bands, scaled as in the
research), K4 and K5 (moments and variance shares over keyed samples, in parallel), K6 (the 2026 replay), K7 (the scenarios against the
golden file) and K8 (elasticities). The golden file `world/data/golden/oil_golden.json` is written by
`scripts/world/oil_reference.py --golden`, which keeps the research prototype unchanged above its golden section.

**Fix round (2026-10-12, spec rev 6).** The splice's base-month gap no longer contains the user shock (`kappa`, S3-R4). The bloc feedback into the market is a switch, `world_oil_feedback` (default true); false is the exact legacy path. The OPEC+ quantity input has its own splice and a measured stochastic offset (`world_oil_qty_offset` -6.627, applied only when `stochastic_oil` is on, S3-R8 option b). The market's US-region demand deviation is an output (`oil_us_demand_pct`, W3-18), and `world_oil_shock_mask` chooses the structural sources drawn into the market (W3-19). The link ships off: W3-14 fails at -4.89 % on 400 fresh seeds and at -13.70 % on the 20000-household check, and the research offset 8.355 is kept rather than fitted. The isolated-market output is unchanged by the round.

### 19.3 Stage 4-1: bilateral dollar and risk factor (`world/dollar.rs`, `world/finance.rs`, link `world_link_dollar`, off by default)

- Hook H2 (`sim.rs`, `step` and `step_timed`): `monetary::exchange_rate` is skipped in reported months when `world::dollar_active`; `world::step` then writes `w.row.fx` from the blocs' bilateral rates. Burn-in runs the ABM's `exchange_rate` as before.
- Order of a reported month: `exog::step` then `world::after_exog` (oil; then the financial factor filter and the injections into `f_t`, `finance::after_exog`) then `monetary::set_policy`, `update_curve`, the bloc step in `world::step` (dollar phase A before `core`, phase B after it, then `trade::post_bloc` with the bilateral addends), `banking`, and the rest of the tick.
- Phase A reproduces the premium step of `fx::exchange_rate` (the same calls and state updates), adds the `UsCredibility` dollar term, the risk addend `gamma_j (Frisk_t - Frisk_b)`, the idiosyncratic AR(1) `zeta_j` (one `WORLD_FX` draw per bloc and month), and sets `UsInputs.{dollar, dollar_delta, proj_on, dollar_proj, fex}`. The blocs see the common addend `a_t` and its projection (`step_bloc`, `jrel > 0`); the non-policy part of F enters the IS equation through `theta_f fex rho_fin^k` (skipped when `fex` is zero).
- Phase B: `lnS_j = (z_j - z_{j,b})/100 + (lnP_j - lnP_{j,b}) - (lnPT_{t-1} - lnPT_b) + a_{j,t} + zeta_j`, `ln D = sum_j wH10_j lnS_j`, `w.row.fx = fx_0 exp(ln D)`. The base values are captured once, at the end of the last burn-in month (`WorldBlock::capture_bases` through `settle`), or at the first step of a world without burn-in; no shock captures a base.
- The financial factor (`finance.rs`): every month, burn-in included, `Frisk_t = world_fx_risk_rho Frisk_{t-1} + eps_t` with `eps_t = e_t` if `e_t > 0` else `world_fx_risk_asym e_t`, `e_t = F_t - fin_factor_rho F_{t-1}`, `F = world_vix_per_unit f`. Reported months only: a `PolicyShift` adds `world_vix_per_100bp bps/100` VIX points to `Fpol` (the policy-induced part, excluded from `fex`), `RiskOff` adds its points to `F` exactly, and `BlocDemand{risk_off}` adds `world_china_risk_pts_per_pct |pct|`.
- With the link off nothing in this section runs: the output is byte-identical to ccdcc48 (identity I1, I2, I4 in CALIBRATION `## b8-world S4`). `finance.gamma_pct_per_vix` is the 12-bloc table copied from the research finance file by `scripts/world/build_world_params.py` (`finance` section of `world_params.json`).

### 19.4 Stage 4-2: official Treasury demand and term premium (`world/finance.rs`, link `world_link_finance`, off by default)

- State (`FinState`, `WorldBlock.fin`, created with the link): the official tranche `H_j = h_b,j tau_t/tau_b + dh_j` (model units; `h_b` from the sourced TIC holdings and official shares of `finance.treasury_demand` in `world_params.json`, normalised to `FO^rw_b = 3773.1` $bn by `nu = 0.99652`; `k_FO = FO_b / 3773.1`), captured once at the end of the last burn-in month (`WorldBlock::settle`, in its base-month branch, and `capture_bases` for a world without burn-in). A shock never captures. Further states: the transitory term `T` and the stock `stock_pp = 100 sum dh / k_FO / GDP^rw`, both exactly 0.0 until a flow executes; `ln D_{t-1}`; `tau_{t-1}`; the F ring (the last six values of `world_vix_per_unit f_t - Fbar`, filled every month, burn-in included); `extra` (the cumulative net official sales at market value, model units); the programmes and the credibility components, registered from `world_shocks` by their activation tick.
- Flow rule (reported months, spec 2.7 official part, D32, D44): `N_j = eta 100 (ln D_t - ln D_{t-1}) H_j,t-1 / k_FO + v mean6(F - Fbar) H_j,t-1 / FO^rw_{t-1} + trend (CN only) + programme terms`, with `eta = world_fo_dollar_resp`, `v = world_fo_vix_resp`; each bloc's `N_j` is clipped at `-H_j,t-1 / k_FO` (a bloc cannot sell more than it holds). Purchases execute from the banks, then the Fund last; sales go to the banks (D45). The deviations, the transitory and the stock terms are updated with the executed flow; the executed share scales the deviations.
- Term premium (hook H5, `update_curve`): `tp_FO(j) = -TP_SLOPE[j] (theta_stock/1e4 stock + theta_trans/1e4 T)` with `theta_stock` 13 bp and `theta_trans` 20 bp per 1 % of GDP, plus the credibility part `sum_c tp_bp/1e4 p_c(age) TP_SLOPE[j]/TP_SLOPE[3]` (D31, D46: the 10y exact at the stated bp, the 3m zero). The hook is `world::world_tp_fo` (None with the link off or outside reported months), added to the explicit term before `expected` and `term_premium` are stored; the later factor pass adds its own terms to the result.
- Book adjustment (hook H5, `rebalance_row`, D45): with `x` the cumulative net official sales, the private RoW book works on `deposits - x` and `bonds_value + x` (`world::world_row_extra`). Without the adjustment the `bonds_value / 12` term of the target would give back 1/13 of every sale.
- Order of a reported month: `exog::step`, `world::after_exog` (registration of `OfficialTreasury` and of the credibility term, so a credibility shock moves the curve of its own month), `set_policy`, `update_curve` (H5 term, from the state of the previous month), `exchange_rate`, `world::step` (`step_month`, then `finance::step_flows` with the trade base's `fx0`; the F ring is filled every month), the rest of the tick, `markets` (H5 book), `settle` (the base capture; the diagnostics).
- Metrics (`world`, zero with the link off): `fo_bn` (FO^rw_t), `fo_flow_bn` (executed flow), `tp_fo_bp` and `tp_cred_bp` (the 10y terms in the month's curve), `fo_extra`, `f_ring_mean`, `row_mv_gdp_pct`, `row_dep_gdp_pct`, `fo_share_row_pct`, `row_face_share_pct`.
- Config keys (`config.rs`): `world_link_finance` (false), `world_fo_theta_stock` (13.0), `world_fo_theta_trans` (20.0), `world_fo_trans_rho` (0.944), `world_fo_dollar_resp` (-0.0023), `world_fo_vix_resp` (1.05), `world_fo_trend_cn` (-80.0). The finance link has no injection into `f_t` and does not fill `f_vix` (those are dollar-link functions, S4-R16 item 11). The dollar link stays off in every 4-2 profile; the finance link does not depend on it.
- Identities: with the link off the output is byte-identical to the 4-1 list-A head (I1, I2, I4; the new world keys are zero); with the link on and the three drivers at zero the output equals the link-off output in every non-world key (I5). The pair identity holds: a shock at reported month `T0` leaves every earlier output bit-identical (`tests_finance`).

### 19.5 Stage 4-1b: baseline neutrality and the bound on the total US-rate excess return (`world/dollar.rs`, `world/bloc.rs`, `world/trade.rs`, link `world_link_dollar`)

Spec 2.6 (rev 9, D47) and 2.2.2 (rev 9, D48). Three config keys, all inert with the dollar link off: `world_dollar_neutral` (true), `world_rp_sat_knee` (0.02, decimal a year) and `world_rp_sat_scale` (0.015). The keys are validated to `[0, 0.2]` whatever the link state (the server answers 400, the CLI exits 2). `legacy_s4a_dollar.json` (`world_dollar_neutral` false, `world_rp_sat_scale` 0) reproduces the 4-1 link bit for bit (identity I6).

- **Conditional forecast of z (D47).** Once per world, at the base capture (the settle call of the last burn-in month, or the first step of a world without burn-in), `WorldBlock::forecast` steps a light copy of the block (parameters, the twelve bloc states, the US history) `FC_MONTHS = 480` months through `core`, coupled, with no innovation, no shock, no dollar addend and no import-demand gap. The US inputs are the base month's expectations moved forward: the policy path `ex_m[k] = ex_b[skip + m + k]` and `ius_m = ex_b[skip + m]` (zero beyond K = 240), `y_US`, oil, quantity and `Fex` decaying at `rho_f`, `rho_oil`, `rho_qty` and `fin_rho`. The result is held in `DollarBase.fc`, shared by clones through an `Arc` (both arms of a pair), and is shock-independent.
- **Where it is read.** `DollarState::z_ref(n)` is the forecast of the bloc month `n` (held after its last row) when `world_dollar_neutral` is on, the base `z` otherwise. Phase B reads `lnS_j = (z_j - z_fc,j)/100 + (lnP_j - lnP_j,b) - (lnPT_{t-1} - lnPT_b) + a_j + zeta_j`; the trade pass-through (2.5) reads `zrel_j = (z_j - z_fc,j) + 100 a_j` through `TradeState::post_bloc_ref` (`post_bloc` keeps its signature and passes no reference). `z - z_fc` is an innovation-only state, zero at the base: the dollar-link analogue of the change since the base of D37.
- **The bound (D48).** In the bloc dollar equation the expected excess return the bloc currency prices is `T_k = ex[k] + lambda_rp gp[k]` (`fS` of 2.2.2). With the dollar link on, `fS = -(M(T_k) + u_rp rho_rp^k)` where `M(x) = x` up to the knee and `knee + L tanh((x - knee)/L)` above it, with knee 2.0 pp and scale 1.5 pp (`fx::bounded_rate_differential`, the b7-transmission M-F constants of D33): a ceiling of 3.5 pp a year. `fR` keeps `exb[k]`; `G` and `theta gp` are unbounded. The bound acts on the raw `us.ex[k]` (not `exb`). With the link off or the scale 0 the arithmetic is the 4-1 expression.
- **Tick order and call sites.** Unchanged: `phase_b` takes the bloc clock `n` (no new hook in `sim.rs`), `step_month` passes `z_ref` to the trade module, `capture_bases` and `settle` compute the forecast before the mutable borrow of the dollar state. `bound_rp`, `z_ref`, `forecast` and `set_forecast` are `#[inline(never)]`.
- **Diagnostic.** `world.z_fc` (per bloc, zero before the base and with the dollar link off) is the reference each `z_j` was read against in the month.
- **What it changes.** The month-0 level step of the 4-1 reference (the forecast's one-month expected drift of the base `z`) is removed: month-0 `fx_broad` moves by about 0.2 index points; the first-year dollar response to a +100 bp surprise is smaller (the bound binds when the total excess return of the baseline is above 2 pp); the baseline no longer carries the blocs' own reversion as dollar movement. The nominal price terms are unchanged (the nominal dollar still carries the blocs' inflation differential against US tradables: S4-R29).
- **Limits (stated, not tuned).** `FC_MONTHS`, the knee, the scale and the forecast laws are not levers. The finance link stays off in every list-B profile (`b8-world-s4d*.json`); the M-F keys are off in `b8-world-s4d.json` and on in `b8-world-s4d-mf.json` (the W4-28 pins); if the dollar link ever ships on, stage 5 ships them on with it (S4-R30).

### 19.6 Stage 4-3: tariff retaliation and cross-border positions (`world/trade.rs` `RetState`, `world/positions.rs`, links `world_retaliation` and `world_positions`, both off by default)

- Retaliation state (`RetState`, `TradeState.ret`, needs `world_link_trade`, S4-R35): for each bloc `j` the average US duty `taubar_j` over the active `Tariff` components flagged `retaliate` (weights: the fixed 2024 import values `imports_base`, so `taubar` depends on the components only); a 13-month history, read `lag_j` months back (a delay, not a maximum); target `tgt = min(cap_j, kappa_j w_s hist_j[lag_j])`; the sector duty `tau*_{j,s}` ratchets up at speed 1 and down at 0.1 per reported month; the lagged price state `zr` moves at `world_ret_speed`; the export factor is `R_{j,s} = exp(-(eps_sr x + (eps_lr - eps_sr) zr))` with `x = ln(1 + tau*)`. `post_bloc` multiplies each bloc's sector exports by `R` before the finiteness guard. Off, or on with no flagged component, `tau*` and `zr` stay 0.0 and `R` is exactly 1.0.
- Tables (`wp.trade.retaliation`, built by `scripts/world/build_world_params.py` from trade `retaliation.recommended_rule`): `kappa` (EA from the research key `EU`), `lag` (EA 2, CN 0, CA 0, MX 1, IN 12; blocs without an entry 1 month [assumed]), `cap` (CN 1.25, others 0.5), `w_s` (Energy 1.0, Food 1.5, Durables 1.2, Nondurables 1.0, Tech 0.5, services 0).
- Retaliation metrics (`world`): `ret_tau[j]` (goods-weighted mean duty on US exports to `j`, sectors 0..4) and `exports_goods_bn[j]` (annualised $bn of those sectors, always filled).
- Positions state (`PositionsState`, `WorldBlock.pos`, created with `world_positions`): nine stocks by bloc. Seven are memo stocks (assets: equity, debt, FDI, bank claims; liabilities: agency and corporate debt, equity, FDI) from the research `finance.cross_border` tables; two are ledger stocks (RoW's Treasuries at market value and RoW's deposits) split by fixed bloc shares (`tic_share`, `dep_share`). The base is captured once: at the base-month settle, or at the first step of a world without burn-in (`fallback`, the first residual is set to zero). The memo stocks are scaled by `k = 12 tau_b / world_gdp_rw_bn`, with `tau_b` the trend GDP (`fiscal::trend_gdp_month`), not the base month's GDP.
- Memo step (`memo_step`, spec 2.8): every memo stock grows by `1 + v + y/12`, where `v` is the valuation rate of the stock from the bloc's dollar increment `ds` (foreign-currency shares `fc_*`), the US equity return `r_us` (equity loadings) and the equity noise `eta` (normal, sd 0.02, drawn on `WORLD_FX` at index 16 + j, every reported month). The income of month `t` uses the stocks of `t-1`. Constant yields: 3.8 % on assets, 2.55 % on liabilities. With `world_link_dollar` off, `ds` is the change of the ABM's broad dollar (`ln fx`); with the link on, the change of the bilateral `ln S_j`. Nothing in `positions.rs` writes an ABM quantity.
- Ledger accumulators (hooks, inert with `world_positions` off; reset every month in `settle`): H19 `ledger::service_government_debt` adds the coupons paid to RoW (`note_row_coupon`); H20 `banking` adds the deposit interest paid to RoW (`note_row_dep_interest`); H21 `fiscal::auction_bonds` records RoW's primary purchase at its paid cash (`note_row_primary`; `monetary::issue_price` is `pub(crate)`, nothing else in `monetary.rs` changes); H22 is the debug-build check in `sim::step` (`world::check_identities`), after `ledger::check_consistency`.
- Identity (spec 2.8, checked every reported month): `NIIP_t - NIIP_{t-1} = CA_ledger + PI_memo + Val_memo - Val_hold - Val_prim`, with `CA_ledger = (exports - imports) - (coupons + deposit interest paid to RoW)`, `NIIP = memo assets - memo liabilities - RoW bonds value - RoW deposits`, and `Val_hold` the valuation of RoW's Treasuries at the month's prices (a matured bond at par). The residual is `sfc_world_error` (relative to monthly GDP). The bilateral current accounts `ca_j = x_j - m_j - led_j + pi_j` (with `led_j = coupons tic_share_j + deposit interest dep_share_j`) sum to `ca_total`.
- Metrics (`world`, zero with `world_positions` off): `niip_pct_gdp`, `ca_pct_gdp`, `ca_bilateral_bn[12]`, `sfc_world_error`.
- Config keys (`config.rs`): `world_retaliation` (false), `world_ret_eps_sr` (1.0), `world_ret_eps_lr` (3.0), `world_ret_speed` (0.10 [assumed]), `world_positions` (false). Refused: retaliation without `world_link_trade`; each elasticity in 0 to 10 with the long run at least the short run; speed in 0 to 1; NaN and infinity.
- Pre-registered values and the measured ones: CALIBRATION `## b8-world S4 (4-3)`.

### 19.7 Stage 5: centring constants, profiles, the ladder and the default decision (`world/trade.rs`, `crates/econsim-server/src/main.rs`, `scripts/profiles/`)

- Centring constants (D57, S2-R7): `world_import_gap_offset` (twelve values in pp, default empty). `TradeState::pre_bloc` subtracts the offset of bloc `j` from the clamped term `clamp(100 ewma_j, -10, +10)` once the term is active; before that the term is zero and no offset is subtracted. The metric `world.us_import_gap[j]` is the term as the blocs see it, so it shows the centred value. The constants are the per-bloc means of the term over months 13-239 measured on seeds 1-12 (step 0, z at most 1.1 against the pre-registered values: `1.983, 1.858, 1.802, 2.933, 1.163, 1.885, 3.007, 2.102, 2.125, 1.822, 1.814, 2.577`). They centre the mean only: the term sits on its clamp in 7-40 % of months.
- `Config::default` does not change (D61): the key defaults to empty, so every fingerprint of stages 1-4 holds (I1 `9f90c88a2c8b653857029d235cd6b0a6`; I9 for `legacy_b8off.json`).
- `Tariff` check (D60): `validate_scenario` on `/api/compare` refuses a bilateral `Tariff` unless `world_block_enabled` and `world_link_trade` are both on. `ImportTariff` is untouched.
- Profiles (D58): see API.md, stage 5. Each profile is a set of keys over the stage-4 profiles, so the stage-4 cells are reproduced by removing the offset key (the sketch fingerprints of CALIBRATION `b8-world S5`).
- The ladder (D55, spec 3.0): `W'` (block off with the ABM keys), then `T` (trade, retaliation, positions), `D` (plus the dollar link), `F` (plus finance). Each rung is judged against the cell below it on the same seeds; a rung that ends off stops the rungs above it when they need it. The dollar and retaliation links need the trade link (validation: `world_link_dollar` and `world_retaliation` require `world_link_trade`); the finance link does not.
- Outcome (CALIBRATION `b8-world S5`): the trade link ships off, because `W2-15` (dM/GDP against W') reads -0.433 pp (SE 0.032) on the pooled 128 default seeds against +-0.3 (S5-c). Retaliation ships off with it (it needs the trade link). The dollar rung is not run (it needs the trade link); the finance rung runs on the finance-only cell against T and is a profile. Oil ships off on its early gates (S5-o). The default set is not proposed. `Config::default` is unchanged.
- Measurement tools: `scripts/world_runs.py` (long and event runs), `scripts/world_s5.py` (gap-offset, shifts, w504, w407, w210, tariff-eq, binom, fo-drift, oil-w311, oil-w310, oil-w308), `scripts/world_events.py` (the event rows of stages 4 and 5), `scripts/world_presets.py` (the bloc-policy presets of section 5), `scripts/stage5_fingerprints.sh` and `scripts/stage5_checks.py` (the stage-5 script tests).

### 19.8 Stage 5-T2: the import price drift (D62), the uniform-duty fold (D63), re-measured constants (`world/trade.rs`, `world/tests_t2.rs`, `scripts/profiles/b8-world-t2*.json`)

- Why (CALIBRATION `b8-world 5-T2`): stage 5 failed W2-15 dM/GDP (-0.433 pp against +-0.3). The block's import price is flat against the tradables price index PT (D26), while the legacy import price falls 0.38 % a year against PT; with the D13 import elasticities that drifts the household import share down by 0.15 pp a year and the imports / GDP ratio with it. The band was not moved (S5-R22).
- Drift (D62): `world_import_rel_drift` (decimal per year, default 0, finite and within +-0.05). `TradeState::post_bloc_ref` multiplies `imp_rel` of every sector (Energy included) by `exp(-g n / 12)`, `n` the months since the base (`since_base`, 0 in the first reported month). The factor is exactly 1 at the base and when the key is 0, so the stage-5 behaviour is bit for bit. The factor enters the CPI import component with the price. The value 0.0032 a year is the measured difference of the import-price slopes of the cell with and without the drift (+0.320 %/yr on seeds 1-64, +0.277 (SE 0.058) on seeds 1-12 with the new build: no stop, S5T-b); it is a continuity constant of the same kind as D57, not a lever.
- Uniform-duty fold (D63): a component with neither partner nor sector (`Tariff{null, null, p}`) is added to the set-level duty `t_level` before the origin-by-sector duties are formed; the bilateral sum takes only components with a partner or a sector. With no uniform component `t_uni` is 0.0 and `t_level + 0.0` is `t_level`. Retaliation (`RetState`) reads the tariff list directly and is unchanged. `ImportTariff{p}` and `Tariff{null, null, p, false}` (and the flagged form with retaliation off) give bit-identical runs (W5-06, an E row).
- Constants (D57, re-measured): the mean of the centred term in the drift cell on seeds 1-64 (`F1` in CALIBRATION) gives the values 2.381, 2.485, 2.419, 3.338, 1.838, 2.748, 3.678, 2.837, 2.568, 2.169, 2.264, 3.056 (pp, bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW). The step-0 check on seeds 1-12 (raw clamped term, cell `t2a` without the offset key) gives |z| at most 1.1 against them: kept.
- Profiles (`scripts/profiles/`): `b8-world-t2a.json` (the trade cell), `-t2b` (dollar), `-t2c` (finance), `-t2a-nc` (fallback: the coupled channel off), `-t2c-nd` (finance without the dollar link); each is the stage-5 profile of the same name with the two keys replaced (see API.md).
- Defaults: `Config::default` does not change (the key defaults to 0). I1 (`9f90c88a2c8b653857029d235cd6b0a6`) and the stage-5 profile fingerprints hold at RAYON 1 and 8 and under the alternative codegen. The server is unchanged; `validate_config` refuses a bad drift through `validate_world_links`.
- Class (CALIBRATION `b8-world 5-T2`): the trade link ships on rung T (provisional on TA1-TA7, see there). The dollar and finance rungs are profiles (D55).
