# Scenario presets

The EconSim website's Scenario Lab (a separate project, not part of this repository) has one-click presets for the
questions the model is used to answer. Each preset is a list of shocks scheduled at month 0, and the column "Shock sent
to the API" gives it in the form `POST /api/compare` takes (see [API.md](API.md)). The Lab runs the preset and its baseline with
the same seeds (common random numbers), so the difference between the two lines is the effect of the shock, not sampling noise.

## How to read the table

- **Measured at 4561051** is the re-measurement at the current defaults (joint calibration 4, step 3: PbR3F0, F1, P2 and DC90; warm-up 24): one `POST /api/compare` per seed (the website's measurement script, not published), 10,000
  households, 6 seeds, 24 months, shocked minus baseline over the same seeds. The full output is in
  [SCENARIOS_MEASURED_4561051.txt](SCENARIOS_MEASURED_4561051.txt). Values are quoted at month 3 (m3), month 12 (m12)
  or the month named.
- **Earlier validation row** is the row of the validation table of rounds 6 and 7 (`econsim-cli irf`; the table was in the
  project's earlier README, which is not part of this release, so its values are quoted in the cell) for the same shock, where one
  exists. Those rows were measured before the re-fit in [CALIBRATION.md](CALIBRATION.md) (2e966a5),
  so their sizes are older than the measured column. They are cited for comparison only.
- **Literature** is the benchmark range for the same shock. Where neither a re-measurement nor an earlier validation row covers a
  preset, the table says so and gives only the expected sign.
- *Since joint calibration 4 the compare endpoint steps the common world 24 months before month 0 (`config.warmup_months`, server default 24) so the baseline starts from a state without the start-up drift; the same rows at warm-up 0 (the earlier behaviour) are in `SCENARIOS_MEASURED_4561051_warmup0.txt`.*
- Units: rates are percent or basis points as stated; money is $bn at annual rates; GDP, prices and house prices are
  % change, and inflation and unemployment are pp.

## Presets

| Preset (Lab group) | Shock sent to the API | What it does in the model | Expected sign and size (literature) | Measured at 4561051 (10k households, 6 seeds, warm-up 24) | Earlier validation row |
|---|---|---|---|---|---|
| **Oil +50%** (Energy) | `OilPrice { pct: 50, half_life_months: 12 }` | World oil price +50% on impact, decaying with a 12-month half-life. Gasoline, energy prices and energy-firm profits follow. | Headline CPI up about 1.5–2pp; core small; GDP down about 0.5–1%; dollar down (Kilian 2009; Blanchard and Galí 2007). | m3 CPI +0.96pp, GDP −0.48%, u +0.19pp, policy +0bp, 10y −1bp, house px +0.04%; m12 CPI +1.19pp, GDP +0.29%, u −0.35pp, policy +29bp, 10y +14bp, house px +0.49%; m24 CPI −0.47pp, GDP −0.72%, u +0.37pp, policy +46bp, 10y +20bp, house px +0.15% | Yes, from round 6/7: headline CPI +1.9pp (m3), GDP −0.7% (m3). The 2e966a5 response is lower (above). |
| **Oil +100%** (Energy) | `OilPrice { pct: 100, half_life_months: 12 }` | Same mechanism at twice the size. | Expected sign as for +50%, about twice the size. | m3 CPI +1.91pp, GDP −0.89%, u −0.01pp, policy +4bp, 10y −1bp, house px +0.06%; m12 CPI +2.34pp, GDP +0.22%, u −0.70pp, policy +63bp, 10y +31bp, house px +0.84%; m24 CPI −0.92pp, GDP −1.45%, u +0.57pp, policy +83bp, 10y +36bp, house px +0.15% | No row for +100%. |
| **2y term premium +100bp** (Rates) | `TermPremium { tenor_idx: 1, bps: 100, half_life_months: 12 }` | Term premium +100bp at the 2-year point; the 3m and 5y neighbours get 50%. The expected short path is unchanged, so the 2y yield rises by the premium. Business loans are priced off 0.5 × 2y + 0.5 × 5y (`banking.rs`), so they move. Mortgages follow the 10y and move little. | The 2y yield rises by the premium at impact, fading with the half-life; business-loan rates and investment fall; unemployment rises (sign expectation; the size is not from literature). | m3 CPI −0.04pp, GDP −0.38%, u +0.20pp, policy −4bp, 10y −0bp, house px +0.01%; m12 CPI −0.13pp, GDP −0.71%, u +0.48pp, policy −29bp, 10y −8bp, house px +0.12%; m24 CPI −0.10pp, GDP −0.00%, u −0.01pp, policy −33bp, 10y −11bp, house px +0.38% | No row. |
| **10y term premium +100bp** (Rates) | `TermPremium { tenor_idx: 3, bps: 100, half_life_months: 12 }` | Term premium +100bp at 10y, neighbours at 50%. Mortgage rates follow the 10y yield; home sales and house prices respond through the mortgage channel. | 10y and mortgage rates +70 to +95bp (m3–m12); home sales −8 to −10%; house prices −2.5% (m12) and −3.6% (m24) (2022–23 episode: −3 to −8% per 100bp over two years). | m3 CPI −0.00pp, GDP −0.09%, u +0.16pp, policy −4bp, 10y +88bp, house px −0.36%; m12 CPI −0.01pp, GDP −0.25%, u +0.07pp, policy −4bp, 10y +53bp, house px −2.36%; m24 CPI +0.07pp, GDP −0.04%, u +0.08pp, policy +4bp, 10y +30bp, house px −3.52% | Yes, round 6/7: 10y +95bp (m3); house prices −2.5% (m12), −3.6% (m24); home sales −8 to −10%. |
| **30y term premium +100bp** (Rates) | `TermPremium { tenor_idx: 4, bps: 100, half_life_months: 12 }` | Term premium +100bp at 30y; the 10y neighbour gets 50%. Mortgages follow the 10y yield, not the 30y, so a 30y shock reaches mortgages only through the spill onto the 10y. | 30y yield up by the premium; 10y up by about half; mortgage rates up by about half of the 10y move. Housing effect follows the 10y row at about half size. | m3 CPI +0.01pp, GDP +0.00%, u +0.09pp, policy +0bp, 10y +45bp, house px −0.20%; m12 CPI −0.02pp, GDP +0.00%, u +0.06pp, policy +0bp, 10y +27bp, house px −1.21%; m24 CPI −0.01pp, GDP +0.38%, u +0.08pp, policy −8bp, 10y +12bp, house px −1.85% | No row for 30y. |
| **Policy rate +100bp (12m)** (Rates) | `PolicyShift { bps: 100, months: 12 }` | Innovation of +100bp to the Taylor rule. The gap decays as an AR(1) with a 12-month mean; the rule keeps responding to the economy. Joint3: the dollar carries a monetary-surprise premium (`fx_mp_premium` 1.25) on PolicyShift shocks only; rule-driven tightening gets none. | Policy rate +83bp (m3), GDP −0.6% (m3), −1.1% (m6 trough) in the round 6/7 row; unemployment +0.4pp; core −0.01 to −0.15pp; dollar +1 to 1.2%; 10y +26bp (m3). Literature: GDP −0.5 to −1%; 10y +20 to 40bp; credit spreads +10 to 40bp (Gertler and Karadi 2015); dollar +2 to 5% (Faust and Rogers 2003); equities about −4% per 100bp (Bernanke and Kuttner 2005). | m3 CPI −0.03pp, GDP −0.46%, u +0.23pp, policy +75bp, 10y +26bp, house px −0.08%; m12 CPI −0.11pp, GDP −0.38%, u +0.32pp, policy +8bp, 10y +5bp, house px −0.53%; m24 CPI −0.05pp, GDP +0.43%, u −0.12pp, policy +8bp, 10y +2bp, house px −0.61% | Yes (first IRF row, round 6/7): policy +83bp (m3), GDP −0.6% (m3), −1.1% (m6). |
| **Policy rate +200bp (12m)** (Rates) | `PolicyShift { bps: 200, months: 12 }` | As above at twice the impact. | Expected sign as for +100bp, about twice the size. | m3 CPI −0.07pp, GDP −0.91%, u +0.19pp, policy +138bp, 10y +45bp, house px −0.21%; m12 CPI −0.19pp, GDP −1.10%, u +0.56pp, policy +21bp, 10y +12bp, house px −1.07%; m24 CPI +0.03pp, GDP +0.25%, u −0.32pp, policy +0bp, 10y +0bp, house px −1.16% | No row for +200bp. |
| **Volcker: 10% for 24m** (Rates) | `PolicyRate { rate: 0.10, months: 24 }` | Policy rate pinned at 10% for 24 months, overriding the Taylor rule. The pinned rate is priced as the rule releases it (exit hazard), and the discretionary response is bounded in slack (R3; docs/CALIBRATION.md, Joint calibration 4). | Large recession; output loss, unemployment and house-price falls; sharp dollar appreciation (Volcker-type disinflation). | m3 CPI −0.18pp, GDP −1.93%, u +0.80pp, policy +796bp, 10y +181bp, house px −0.59%; m12 CPI −0.97pp, GDP −7.93%, u +4.89pp, policy +850bp, 10y +176bp, house px −5.38%; m24 CPI −0.72pp, GDP −8.69%, u +5.11pp, policy +700bp, 10y +64bp, house px −10.17% | Partly: the README row is a 7% pin for 24 months (GDP −12% at m12, unemployment +6.1pp, house prices −10.8% at m12). The 10% case is not in the README. |
| **QE Treasuries 1% GDP/m x 12 (10y)** (Balance sheet) | `QE { tenor_idx: 3, monthly_face_pct_gdp: 1, months: 12 }` | Central bank buys 1% of annual GDP per month of 10y Treasuries for 12 months (about 12% of GDP in total). The term premium falls through the preferred-habitat channel. Joint3: each purchased lot counts 1.5 times in the priced duration (`qe_duration_weight` 1.5). | 10y yield falls by about 15–20bp per $1T of purchases (Gagnon et al. 2011; Krishnamurthy and Vissing-Jorgensen 2011; Bernanke 2020); GDP and unemployment effects small. | m3 CPI +0.02pp, GDP +0.12%, u +0.03pp, policy +0bp, 10y −46bp, house px +0.15%; m12 CPI −0.03pp, GDP +0.53%, u −0.26pp, policy +21bp, 10y −60bp, house px +1.56%; m24 CPI +0.03pp, GDP +1.04%, u −0.63pp, policy +38bp, 10y −47bp, house px +3.65% | Round 6/7 row is the 0.5% × 12 case (10y −14 to −18bp; GDP +0.1 to +0.3%; unemployment −0.2pp at m12). The 1% case is not in the README. |
| **MBS QE 0.5% GDP/m x 12** (Balance sheet) | `MbsQE { monthly_face_pct_gdp: 0.5, months: 12 }` | Central bank buys agency MBS. It compresses the mortgage spread (`mbs_spread_adj`) and moves no money into the Treasury curve. | Mortgage rate falls; house prices and home sales rise; the 10y Treasury effect is small (sign expectation; no literature size used). | m3 CPI +0.00pp, GDP +0.00%, u +0.00pp, policy +0bp, 10y +0bp, house px −0.01%; m12 CPI −0.01pp, GDP −0.07%, u −0.07pp, policy +4bp, 10y +1bp, house px +0.29%; m24 CPI −0.02pp, GDP +0.62%, u −0.10pp, policy −4bp, 10y −2bp, house px +1.34% | No row (the README says the MbsQE shock is not in the QE table). |
| **Helicopter $2,000 x 3** (Fiscal and money) | `Helicopter { per_household: 2000, months: 3 }` | $2,000 per household per month for 3 months, central-bank financed. Money printing: the transfer is not wage-indexed. | Consumption up sharply at impact (durables most), then reverses; GDP up about 2% for a few months; unemployment down about 1pp (large MPC out of transfers; Parker et al. 2013; 2020–21 stimulus). | m3 CPI +0.18pp, GDP +1.96%, u −0.56pp, policy +8bp, 10y −2bp, house px −0.00%; m12 CPI +0.18pp, GDP +0.56%, u −0.40pp, policy +71bp, 10y +20bp, house px −0.07%; m24 CPI +0.07pp, GDP +0.05%, u +0.26pp, policy +4bp, 10y +3bp, house px −0.44% | Yes, round 6/7: consumption +7.3% (m3), −0.4% (m12); durables +17.5% (m3); GDP +2.3% (m3); unemployment −0.7pp (m3), −1.4pp (m6). |
| **Gov. spending +5% x 12** (Fiscal and money) | `GovSpending { pct: 5, months: 12 }` | Government purchases 5% above baseline for 12 months. The debt-stabilising rule and the policy rule respond. | GDP up on impact; unemployment down; debt/GDP up. Multiplier sign positive; size not stated here. | m3 CPI +0.04pp, GDP +0.54%, u −0.12pp, policy +0bp, 10y −0bp, house px −0.00%; m12 CPI +0.08pp, GDP +1.11%, u −0.65pp, policy +33bp, 10y +10bp, house px −0.03%; m24 CPI +0.04pp, GDP −0.16%, u +0.49pp, policy +8bp, 10y +6bp, house px −0.43% | No row. |
| **Import tariff 10%** (Trade) | `ImportTariff { pct: 10 }` | Permanent border duty of 10% on the customs value of imported consumer goods (Durable, Nondurable, Tech) and of firms' imported inputs and equipment (Food is covered as an input). Energy and services are exempt. Mechanism (b7-tariff): the retail price carries a lagged share of the duty (speed 0.075 a month, 61% after 12 months); importers absorb the rest; household import shares fall with elasticity 0.30 short run and 1.2 long run; the duty is government revenue; the import price and CPI carry the retail wedge; nominal consumption includes the duty, real volumes do not (BEA). Policy rule unchanged (core PCE, no look-through). Re-measured at joint3 (tariff check, 24 seeds, 60 months): CPI at m12 +0.74pp, core PCE at m12 +0.99pp, revenue 0.80% of GDP, dollar +0.80% at m12. | Amiti, Redding and Weinstein (2019) and Fajgelbaum et al. (2020): full border pass-through; Cavallo et al. (2021) and Cavallo, Llamas and Vazquez (2025): partial, lagged retail pass-through, margins absorb part; Boehm, Levchenko and Pandalai-Nayar (2023): trade elasticity about 0.76 short run, about 2 long run. Expected: CPI +0.5 to +1.0pp at m12 (one-off), GDP small negative, revenue 0.5 to 1.1% of GDP, dollar up modestly. | m3 CPI +0.24pp, GDP +0.02%, u +0.07pp, policy +8bp, 10y +1bp, house px −0.01%; m12 CPI +0.72pp, GDP −0.37%, u +0.20pp, policy +58bp, 10y +22bp, house px −0.15%; m24 CPI +0.29pp, GDP −0.32%, u +0.39pp, policy +58bp, 10y +28bp, house px −1.00% | No row. |
| **Tech boom (72 months)** (Technology) | `TechBoom { months: 72 }` | Tech productivity growth up, adopters (durables, other services) up a little, Tech equity re-rated on expected earnings growth. | Tech investment and Tech equity up sharply; Tech P/E up; the boom shows as a shaded band in the Equity panel. | m3 CPI +0.00pp, GDP −0.04%, u +0.06pp, policy +0bp, 10y +0bp, house px −0.01%; m12 CPI −0.09pp, GDP +0.33%, u +0.06pp, policy +0bp, 10y −1bp, house px +0.05%; m24 CPI +0.05pp, GDP +1.00%, u −0.02pp, policy −4bp, 10y −4bp, house px +0.18% | Yes, in part: `irf 10000 36 12`, 12 seeds: Tech investment +3% (m3), +8% (m6), +14% (m12); Tech equity +59% (m3), +73% (m6). |
| **LTV cap 80%** (Housing) | `MacroPrudential { max_ltv: 0.8, max_dti: 0.31 }` | Caps the maximum loan-to-value for new mortgages at 80% (from the model's 96.5%) and keeps the model's maximum debt-to-income at 31%. Permanent. | Tighter mortgage credit: fewer new mortgages, lower house prices on the margin, home sales down. | m3 CPI −0.00pp, GDP +0.03%, u +0.03pp, policy +0bp, 10y +0bp, house px −0.06%; m12 CPI −0.01pp, GDP +0.35%, u −0.05pp, policy +4bp, 10y +1bp, house px −0.12%; m24 CPI −0.01pp, GDP +0.49%, u −0.08pp, policy +4bp, 10y +1bp, house px −0.19% | No row. Sign only in the README. |

### Note on the earlier LTV preset

Until this round the LTV preset sent `max_dti = 0.43`, which loosened the DTI limit (the model's default is 0.31), so
new mortgages rose. The preset now keeps DTI at the model default, and the smoke test checks that new mortgages fall.

## Mechanics the presets rely on

- **Units.** Shock sizes in the Lab are in the units shown under each preset name. The API takes decimals for rates
  (`PolicyRate.rate` = 0.10 for 10%) and percent for `pct`/`bps` fields; see `docs/API.md` for the exact shapes.
- **Half-lives.** `OilPrice`, `TermPremium` and `Dollar` take a half-life in months (0 means permanent). The
  `PolicyShift` gap uses a mean duration, not a half-life.
- **Tenor index.** `TermPremium`, `QE` and the curve use `tenor_idx`: 0 = 3m, 1 = 2y, 2 = 5y, 3 = 10y, 4 = 30y.
  A `TermPremium` shock puts 50% on the neighbouring tenors.
- **Mortgage and loan pricing.** The mortgage rate is the 10y yield plus a spread (`banking.rs`, `yield_at(120)`);
  business loans are priced off 0.5 × 2y + 0.5 × 5y. So a 2y shock works through business loans and investment, and a
  30y shock reaches mortgages only through its spill onto the 10y.
- **Tariffs.** `ImportTariff` is a border duty on the customs value of covered imports (consumer goods, firms' imported
  inputs and equipment). The retail price carries a lagged share of it (`tariff_retail_speed` 0.075 a month), importers
  absorb the rest, household and firm import shares respond with elasticities 0.30 short run and 1.2 long run, and the
  duty is government revenue. Nominal consumption includes the duty; real volumes exclude it (BEA). `tariff_pass_through:
  false` gives the legacy lump-sum path (`scripts/profiles/b7-tariff-legacy.json`).
- **Common random numbers.** `POST /api/compare` runs the baseline and the shocked scenario with the same seeds.
  The website's compare view and its measurement scripts all rely on this.
- **Burn-in.** Compare runs start after burn-in (`burn_in_months`, 144 at 2e966a5), so the baseline is already in its
  stationary state. Presets scheduled at month 0 hit a converged economy.

## Limits

- **Tariffs: policy and scope.** The policy rule (core PCE yoy, coefficient 1.5) does not look through the one-off
  price-level shift, so the tariff raises the policy rate: about +56bp at m12 at 10% (12 seeds; +54bp in the 6-seed preset), and +140bp at 25% (12-seed means). There
  is no retaliation in the default profile (stage 4-3: with `world_retaliation` on, the flagged duties of `tarwar` and of the `Tariff` shocks with `retaliate` true raise the partner's duty on US exports; the duties have no end date, open item 32) and no foreign-exporter absorption (domestic firms absorb part of the duty, as in Cavallo et al.
  2021). Food and Energy are exempt at retail (Food is covered as an input). Household-level real-consumption metrics
  (quintiles, Gini) still include duties. A border pass-through below 1 is not modelled.
- **Oil pass-through is below the literature range.** At 2e966a5 the headline CPI response to oil +50% is about
  1.2pp, against the 1.5–2pp range the doc cites. This is flagged to the orchestrator and is not tuned here.
- Rows marked "no row" or "sign only" are not measured against the earlier validation table. The Measured column is the only
  current number; the earlier validation rows are older.
- Linear scaling is an approximation: oil +100% is about 2× oil +50% at m3, and +200bp is about 2× +100bp at m3.
  Both are read from the measured runs.
- Presets are single shocks. Combinations (for example oil plus a rate rise) are not presets; send them as several entries of `shocks` in one request.

## Measuring a preset

Send the shock to `POST /api/compare` for seeds 1 to 6 at 10,000 households and 24 months (the server's default warm-up of 24 months
applies), average shocked minus baseline over the seeds, and add a row here with the literature range and the earlier validation row it
matches, if any.

## Limits added with the world block's trade link (stage 2)

- Tariffs (world block, `world_link_trade` on): partner-sector duties (`Tariff`) enter b7 as one weighted scalar per channel
  (household and firm); a uniform set-level tariff gives the b7 scalar exactly. Sector heterogeneity of the retail wedge is lost to
  first order.
- Dollar shocks at the first reported month are partly absorbed by the trade base (the base is captured after the month's
  exchange-rate step): the block's dollar responses are measured from month 1 (open item in CALIBRATION "b8-world S2").

- Oil shocks (stage 3, `world_link_oil` on): `oil50` places the +50 % shock at reported month 12 and `oil50m0` at month 0 (the `irf` convention). With `world_oil_exact_path` true (the default), copy U pins the monthly-average price to the structural price plus the shock ("exact path"), and copy U is dropped once the shock has decayed. The shipped profile sets the flag false, so its shocks enter the ABM log price directly. A one-month pass-through jump leaves the 12-month yoy window at month 12, which is a base effect (CALIBRATION "b8-world S3", W3-11).

- Dollar link (stage 4-1, `world_link_dollar` on): `riskoff14` places `RiskOff` 14 VIX points at reported month 0; `uscred` places `UsCredibility {tp_bp 22, months 3, dollar_pct 3.3}` at month 0 (its term-premium part is applied from 4-2 when `world_link_finance` is on, and ignored when it is off); `china3` places `BlocDemand {CN, -3, half-life 12, risk_off}`; `ecb100` places `BlocPolicy {EA, 100, 12}`; `pin7` and `pin10` hold the policy rate at 7 % and 10 % for 24 months. Limits: `RiskOff` vix_points in [0, 100]; `UsCredibility` dollar_pct in [0, 50) and months at least 1; the broad dollar moves by the realised step of the blocs (in dollar mode the blocs' uncovered-interest offset is part of the response to `dollar10`: the broad rate moves by the realised step, spec 2.6, not by the full 10 %).
- List B (stage 4-1b, `world_link_dollar` on in `b8-world-s4d.json`, shipped off by default): the dollar responses are read against the blocs' conditional forecast (D47) and the total US-rate excess return is bounded at 3.5 pp a year (D48). Limit: the first-year dollar response to a +100 bp surprise is smaller than in 4-1 by 9-24 % (pre-flight measurement on exploratory seeds, the list-B pre-flight note (not published) 4.3; not re-measured against the 4-1 cells on the fresh seeds). `pol100` and `pol100l` are unchanged in definition; the W4-07 and W4-08 values are reported as measured (see CALIBRATION `## b8-world S4 (4-1b)`).
- Finance link (stage 4-2, `world_link_finance` on, dollar link off): `fo100` places `OfficialTreasury {bloc CN, bn 100, months 1}` at month 0 (the 10y term falls about 10 bp at the first month after the flow and about 4 bp in the long run); `fo400` places `OfficialTreasury {bloc CN, bn -400, months 6}` at month 0 (a reserve-manager sale; with the CN trend at -80 the CN tranche cannot carry both, so the response decays; the preset row is read with `world_fo_trend_cn` 0 as the mechanism cross-check); `dollar10` (above) is the realised-dollar row W4-12 with the ABM's own dollar; `qe05` and `qe1` place `QE {tenor_idx 3, monthly_face_pct_gdp 0.5 (1.0), months 12}` at month 0. Limits: `OfficialTreasury` bn in [-2000, 2000] real-world $bn in total, months in [1, 60], bloc null or a bloc name; the tranche cannot go negative (each bloc's sales stop at its holdings, D44).

## Stage 5 world cards (typical start, world profile `b8-world-s5a`, warm-up 24, reported)

Stage 5 measured the world cards of spec section 5 on the typical start only (`start_state` typical; the "Today's US economy" start and the world block exclude each other, D60, sub-round 5-U). The trade link ships **off** (CALIBRATION `b8-world S5`: W2-15 dM/GDP on the pooled 128 seeds), so every card that needs the trade link is withdrawn from the site, not failed. A card that needs a link that does not ship is withdrawn (spec section 5, "Needs"). The readings below are reported; none is a gate.

| Card | Shock sent to the API | Needs | Status | Reading (16 seeds, cell T, warm-up 24 unless stated) |
|---|---|---|---|---|
| **ECB +100bp** | `BlocPolicy { bloc: "EA", bps: 100, months: 12 }` | trade | withdrawn: fails its other-bloc peak gap (MX 0.185, CA 0.325 against 0.15) (warm-up 0; the gated EA rows pass: gap trough -0.55, inflation -0.40 pp at m12) | US GDP trough -0.32 % (reported, D56 ix) |
| **BoJ +100bp** | `BlocPolicy { bloc: "JP", bps: 100, months: 12 }` | trade | withdrawn with the trade link (its gated rows pass at warm-up 0: JP gap trough -0.43 %, inflation -0.33 pp) | US GDP trough -0.19 % (reported) |
| **PBoC -100bp** | `BlocPolicy { bloc: "CN", bps: -100, months: 12 }` | trade | withdrawn with the trade link (all rows pass at warm-up 0) | CN gap peak +0.83 %, CN inflation +0.52 pp, US exports +0.10 % |
| **China slowdown** | `BlocDemand { bloc: "CN", pct: -3, half_life_months: 12, risk_off: true }` | trade and dollar | withdrawn: the dollar rung does not ship | trade-only rows: EA gap trough -0.31 %, UK -0.28 % |
| **Tariff war with retaliation** | `Tariff { partner: null, retaliate: true, pct: 10 }` + `Tariff { partner: "CN", retaliate: true, pct: 15 }` | trade and retaliation | withdrawn: imports from CN -20.2 % sit on their edge (-20 %) at 48 seeds, and the trade link is off | warm-up 24, 16 seeds: imports from CN -20.05 %, goods exports elasticity -2.44, real GDP m12 -1.42 %, CPI m12 +0.82 pp, unemployment m12 +0.92 pp |
| **US-China tariff war** | `Tariff { partner: "CN", pct: 25, retaliate: true }` | trade and retaliation | withdrawn with the trade link | warm-up 24, 16 seeds: imports from CN -22.4 %, goods exports elasticity -2.44, real GDP m12 -0.53 %, CPI m12 +0.17 pp, CN gap trough -0.68 % |
| **Machinery and electronics** | `Tariff { sector: 2, pct: 25 }` + `Tariff { sector: 4, pct: 25 }` (hit-back: `retaliate` true) | trade (and retaliation) | withdrawn with the trade link (the model applies one duty per product group; no product prices) | not measured at stage 5 |
| **Global risk-off** | `RiskOff { vix_points: 14 }` | dollar | withdrawn: the dollar link does not ship | not measured at stage 5 (the dollar rung is not run) |
| **Reserve-manager sell-off** | `OfficialTreasury { bloc: "CN", bn: -400, months: 6 }` | finance | withdrawn: the long-run 10-year reading (months 49-60, +5.1 bp) is below its +8 bp floor; the first-year reading (+25.3 bp) is inside its band | finance-only cell against T (16 seeds, 60 months) |
| **OPEC+ cut, Global demand boom, Hormuz-type loss** | `OpecCut`, `GlobalDemand`, `OilSupply` | oil | withdrawn: the oil link ships off (its early gates fail: CPI at m12 0.98 pp against 1.0-1.4, OPEC+ output -1.34 mb/d against -2.0 to -1.4) | `OpecCut` gates: Brent +4.6 % at m6 (pass), OPEC+ output -1.34 mb/d (fail) |

The readings and their seeds are in CALIBRATION `b8-world S5`; the warm-up 0 readings there are the registered ones.

## Stage 5-T2 world cards (typical start, world profile `b8-world-t2a`, reported)

Sub-round 5-T2 repaired the trade link (CALIBRATION `b8-world 5-T2`): the trade link ships on rung T, so the trade-only cards below are readable with `b8-world-t2a.json`. They are not on the default set (the cost row W5-05 is not run), and the site keeps the legacy `ImportTariff` preset until the orchestrator rules (S5-R25). The dollar and finance cards need profiles `t2b` and `t2c`, which are profiles (D55) and are not on the default set. Readings are reported, none is a gate. Warm-up: 0 unless the row says 24 (16 events seeds 4601-4616 unless stated; 48 seeds for `tarwar` imports).

| Card | Shock sent to the API | Needs | Status | Reading (T2, 10,000 households) |
|---|---|---|---|---|
| **Tariff war with retaliation** | `Tariff { partner: null, retaliate: true, pct: 10 }` + `Tariff { partner: "CN", retaliate: true, pct: 15 }` (`tarwar`) | trade and retaliation | available with `t2a`; warm-up 0, 48 seeds: imports from CN -20.65 % (0.16), real GDP m12 -1.23 %, CPI m12 +0.87 pp, unemployment m12 +0.86 pp, goods exports to CN -41.8 %; warm-up 24, 16 seeds: imports from CN -20.47 % (0.22), real GDP m12 -1.35 %, CPI m12 +0.86 pp | imports from CN -20.65 % sit inside [-40, -20] by 0.65 (close to the edge; 48 seeds) |
| **US-China tariff war** | `Tariff { partner: "CN", pct: 25, retaliate: true }` (`tarCN25r`) | trade and retaliation | available with `t2a`; warm-up 0: imports from CN -22.81 % (0.25), real GDP m12 -0.34 %, CPI m12 +0.18 pp, unemployment m12 +0.18 pp, CN output-gap trough -0.51 % (month 24); warm-up 24: imports from CN -22.75 % (0.19), real GDP m12 -0.45 % | |
| **Uniform 25 % duty, retaliating** | `Tariff { partner: null, sector: null, pct: 25, retaliate: true }` (`tarU25r`) | trade and retaliation | available with `t2a`; warm-up 0: imports from CN -17.45 % (0.33), real GDP m12 -2.53 %, CPI m12 +1.80 pp, unemployment m12 +1.73 pp | the uniform form and `ImportTariff` are bit-identical without retaliation (W5-06) |
| **Machinery and electronics** | `Tariff { sector: 2, pct: 25 }` + `Tariff { sector: 4, pct: 25 }`, hit-back (`retaliate` true) (`tarDT25r`) | trade and retaliation | available with `t2a`; warm-up 0: exports to CN -33.13 % (0.03), imports from CN -11.93 %, real GDP m12 -1.49 %, CPI m12 +1.15 pp, unemployment m12 +1.01 pp | without retaliation (`tarDT25`): imports from CN -11.26 %, real GDP m12 -0.85 % |
| **ECB +100bp** | `BlocPolicy { bloc: "EA", bps: 100, months: 12 }` (`ecb100`) | trade | available with `t2a`; warm-up 0: EA output-gap trough -0.55 % (month 15), US real GDP m12 +0.07 % (0.09), CPI m12 -0.01 pp | the gated EA rows are the stage-5 readings (CALIBRATION `b8-world S5`) |
| **BoJ +100bp** | `BlocPolicy { bloc: "JP", bps: 100, months: 12 }` (`boj100`) | trade | available with `t2a`; warm-up 0: JP output-gap trough -0.42 % (month 17), US real GDP m12 +0.03 % | |
| **PBoC -100bp** | `BlocPolicy { bloc: "CN", bps: -100, months: 12 }` (`pboc-100`) | trade | available with `t2a`; warm-up 0: US goods exports to CN +0.87 % (0.02), CPI m12 -0.02 pp, real GDP m12 +0.05 % | |
| **China slowdown** | `BlocDemand { bloc: "CN", pct: -3, half_life_months: 12, risk_off: true }` (`china3`) | trade and dollar | the dollar rung is a profile: the card is readable with `t2b` only; warm-up 0, cell `D2`: US GDP per 1 % China shortfall -0.18 (band [-0.35, -0.07]), dollar per 1 % 0.82 (band [0.75, 1.75]) | |
| **Global risk-off** | `RiskOff { vix_points: 14 }` (`riskoff14`) | dollar | profile (`t2b`): impact broad dollar +2.57 % (band [2, 4]), 12-month +0.62 % (band [-0.5, 2.5]), USD/JPY 3-month +0.05 % (band [-3, 1]) | |
| **Reserve-manager sell-off** | `OfficialTreasury { bloc: "CN", bn: -400, months: 6 }` (`fo400`) | finance | profile (`t2c`): 10-year yield at months 1-12 +31.6 bp at record 5 (reported), mean months 48-59 +4.2 bp | the stage-5 long-run floor (+8 bp) is not re-read here |
| **QE, 10-year** | `QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 }` (`qe05`) | finance | profile (`t2c`), 32 seeds: 10-year yield change at month 12 -0.35 pp (band [-0.48, -0.18]); the term-premium channel at month 12 +0.12 bp (band [-0.5, 0.5]); the matched dollar cell `t2b` gives the difference | |
| **OPEC+ cut, Global demand boom, Hormuz-type loss** | `OpecCut`, `GlobalDemand`, `OilSupply` | oil | off: the oil link ships off (stage 5, S5-o) | not measured at 5-T2 |

The readings and their seeds are in CALIBRATION `b8-world 5-T2`. The 16-seed event readings carry the SEs shown in the record.

## Today's start tariff cards (world-usnow, 2026-10-15)

On Today's start the tariff cards run the trade model from the tariffs and trade shares of 24 August 2026 (the Today's-start design note (not published; its content is summarised in CALIBRATION, `world-usnow`)), with the 24-month window as the warm-up. The readings are from the registered validation run (CALIBRATION `world-usnow`, the Today's-start registration note (not published)).

| Card | Shock sent to the API | Needs | Status | Reading (Today's start, world profile) |
|---|---|---|---|---|
| Tariff on imports (`tariff`, page controls) | `Tariff { partner, sector, pct, retaliate }` | trade and retaliation | available on Today's start: registered 2026-10-15, class Ship (the Today's-start registration note (not published)) | defaults Everyone, Everything, +10, off (96 seeds, 5,000 households, 36 months): first-year CPI +0.48 pp, unemployment +0.09 pp, real GDP -0.08 % (-0.25 % at month 12); +25 points: +1.17 pp, +0.29 pp, -0.25 %; +25 with hit-back (48 seeds): +1.13 pp, +0.60 pp, -0.87 %. Research ranges (CPI +0.19 to +0.79 pp, GDP -0.17 to -0.05 % per 10 points): inside at 10 and at 25 points (the first 16 seeds read GDP -0.19 at 10 points, then 48 seeds -0.10). 5,000 households read GDP about 0.1 more negative than 10,000 |
| Tariffs on China (country card) | `Tariff { partner: "CN", pct, retaliate }` | trade and retaliation | available on Today's start: registered 2026-10-15, class Ship | +25 points with hit-back (48 seeds): imports from China -19.6 % (SE 0.16) at month 12, goods exports to China -34.4 %, first-year real GDP -0.12 %, CPI +0.10 pp, unemployment +0.07 pp; China's output gap -0.18 points (a damped response: the typical start reads about -0.3 to -0.4). The war (+10 on all and +15 on China, hit-back; 96 seeds): imports from China -16.7 %, CPI +0.52 pp, GDP -0.88 % at month 12 |
| Tariffs on Canada (country card) | `Tariff { partner: "CA", pct, retaliate }` | trade and retaliation | available on Today's start: registered 2026-10-15, class Ship. Canada's 3% duty on US goods is an estimate, not a published figure | +25 points with hit-back (48 seeds): imports from Canada -18.4 % (SE 0.16) at month 12, goods exports to Canada -32.8 %, first-year real GDP -0.32 %, CPI +0.07 pp, unemployment +0.17 pp; Canada's output gap -4.3 points (mean 24 months) |

## Web cards for tariffs by country and product (R12-W, typical start, warm-up 24, 32 simulations)

The site runs these with `b8-world-t2a.json` (scripts/profiles, the 5-T2 repair): a website script writes the
config into the website's source. Figures are Year 1 averages from the website, built
against a world-capable server at the card's own size.

| Card (id) | Shock the page sends | Year 1 (live card) | Pre-flight reading (16 seeds, month 12) |
|---|---|---|---|
| Tariff on imports (`tariff`, page controls) | `Tariff { partner, sector, pct, retaliate }`, defaults Everyone, Everything, +10, off | inflation +0.48 pts, unemployment +0.14 pts, output -0.23 % (regenerated, R12-W fix round) | +10 points: GDP -0.59 %, inflation +0.76 pts |
| US and China fight a tariff war (`us-china-tariff-war`) | `Tariff { partner: "CN", sector: null, pct: 25, retaliate: true }` | inflation +0.14 pts, unemployment +0.15 pts, output -0.25 % | +25 points with hit-back: GDP -0.48 %, inflation +0.18 pts, unemployment +0.30 pts |
| Tariff on machinery and electronics (`tariff-machinery`) | `Tariff { partner: null, sector: 2 }` + `Tariff { partner: null, sector: 4 }`, same pct and hit-back flag | inflation +0.76 pts, unemployment +0.23 pts, output -0.33 % | +25 points: GDP -0.71 %, inflation +1.21 pts, unemployment +0.50 pts |

The live and pre-flight figures differ by sampling (32 simulations, the Year 1 average against month 12) and are not
compared as a check. No research row covers a single country or a single kind of good, so these cards carry no research range.
