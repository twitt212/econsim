# Nine-sector calibration (stage 2 of the sector expansion)

The model moves from five sectors (Energy, Food, Goods, Services, Construction) to nine, so that sectors can have
different trend productivity, price stickiness, income elasticities, inventory behaviour, trade exposure and business
cycles. Construction stays last and is not a consumer sector (N_CONSUMER = 8). Numbers are rounded calibration
targets from the sources listed; the model is a 20,000-household economy, so they are shares and ratios, not levels.

Order: 0 Energy, 1 Food, 2 DurableGoods, 3 NondurableGoods, 4 Tech, 5 LeisureHospitality, 6 HealthEducation,
7 OtherServices, 8 Construction.

## Sector definitions and employment shares (BLS CES 2024, plus public education in HealthEducation)

| Sector | Contents | Employment share |
|---|---|---|
| Energy | mining and oil & gas, utilities, refining, gasoline stations | 0.020 |
| Food | food manufacturing, grocery and food wholesale/retail (food away from home is Leisure) | 0.070 |
| DurableGoods | durable manufacturing, motor-vehicle and furniture/electronics/building-material dealers, durable wholesale | 0.090 |
| NondurableGoods | nondurable manufacturing ex food, general merchandise/apparel/drug stores, nondurable wholesale, freight | 0.100 |
| Tech | information (software publishing, telecom, data processing, media), computer systems design | 0.040 |
| LeisureHospitality | restaurants, accommodation, arts and recreation, air travel | 0.105 |
| HealthEducation | private health care and social assistance, private and public education | 0.200 |
| OtherServices | professional/business, finance and insurance, real estate services, transport, other services, non-education government | 0.325 |
| Construction | residential and non-residential construction | 0.050 |

## Price-index weights

CPI (BLS relative importance, Dec 2024; shelter 0.36 separately): Energy 0.065, Food at home 0.080, Durables 0.110,
Nondurables 0.080, Tech (communication and IT) 0.030, Leisure (food away, recreation services, lodging, airfares)
0.106, Health and education 0.090, Other services 0.079. Core CPI excludes Energy and Food.

Core PCE (BEA, PCE excluding food and energy): shelter 0.17, Durables 0.12, Nondurables 0.14, Tech 0.03, Leisure 0.12,
Health and education 0.21 (includes care paid by government and insurers), Other services 0.21.

## Input-output coefficients (BEA use tables, rounded; input of row sector per unit of column sector's output)

| input \ user | Energy | Food | Durable | Nondur | Tech | Leisure | HealthEd | Other | Constr |
|---|---|---|---|---|---|---|---|---|---|
| Energy | 0.10 | 0.04 | 0.02 | 0.04 | 0.01 | 0.03 | 0.01 | 0.02 | 0.04 |
| Food | 0.00 | 0.20 | 0.00 | 0.01 | 0.00 | 0.12 | 0.01 | 0.00 | 0.00 |
| Durable | 0.02 | 0.01 | 0.20 | 0.03 | 0.05 | 0.02 | 0.02 | 0.02 | 0.15 |
| Nondur | 0.04 | 0.06 | 0.05 | 0.15 | 0.02 | 0.04 | 0.05 | 0.02 | 0.08 |
| Tech | 0.01 | 0.01 | 0.02 | 0.01 | 0.15 | 0.02 | 0.02 | 0.04 | 0.01 |
| Leisure | 0.00 | 0.00 | 0.00 | 0.00 | 0.01 | 0.02 | 0.01 | 0.02 | 0.00 |
| HealthEd | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.00 | 0.03 | 0.01 | 0.00 |
| Other | 0.08 | 0.12 | 0.10 | 0.10 | 0.12 | 0.12 | 0.12 | 0.20 | 0.12 |
| Constr | 0.03 | 0.00 | 0.01 | 0.01 | 0.00 | 0.01 | 0.01 | 0.02 | 0.02 |

## Behavioural parameters by sector

| Parameter | Energy | Food | Durable | Nondur | Tech | Leisure | HealthEd | Other | Constr | Source |
|---|---|---|---|---|---|---|---|---|---|---|
| Productivity growth offset (pp/yr over 1.5%) | 0.0 | 0.0 | +2.0 | −0.3 | +3.0 | −1.0 | −1.2 | +0.45 | −1.5 | BLS labour productivity by industry 1990–2019; Baumol |
| Initial markup | 0.14 | 0.10 | 0.12 | 0.12 | 0.25 | 0.10 | 0.12 | 0.14 | 0.11 | De Loecker, Eeckhout & Unger 2020 (sector dispersion) |
| Monthly repricing probability | 1.00 | 0.35 | 0.20 | 0.20 | 0.15 | 0.10 | 0.06 | 0.12 | 0.25 | Nakamura & Steinsson 2008 (regular prices) |
| Inventory target (months of sales) | 0.3 | 0.5 | 1.5 | 1.0 | 0.3 | 0.05 | 0.0 | 0.0 | 0.5 | Census M3 / retail inventory-sales ratios |
| Household import share | 0.05 | 0.10 | 0.40 | 0.30 | 0.10 | 0.03 | 0.02 | 0.02 | 0.0 | BEA import content of PCE |
| Export base (share of GDP) | 0.012 | 0.008 | 0.035 | 0.020 | 0.012 | 0.008 | 0.002 | 0.013 | 0.0 | BEA trade by product (total ≈11%) |
| Tradable weight | 0 | 0.3 | 1 | 1 | 0.3 | 0 | 0 | 0 | 0 | goods and digital goods compete with imports |
| Owner-operated (proprietor) firms | no | yes | no | no | no | yes | yes | yes | yes | BEA proprietors' income by industry |

Consumer demand (linear expenditure system, eight consumer sectors; income elasticities after Aguiar & Bils 2015:
necessities food and energy, luxuries durables and leisure):

| | Energy | Food | Durable | Nondur | Tech | Leisure | HealthEd | Other |
|---|---|---|---|---|---|---|---|---|
| Subsistence ($/month per household) | 180 | 330 | 60 | 200 | 80 | 120 | 120 | 400 |
| Marginal budget share | 0.04 | 0.06 | 0.14 | 0.10 | 0.06 | 0.16 | 0.10 | 0.34 |

Role vectors (stage 1 Config fields):
- capital goods (new nonresidential investment: equipment 37%, software 17%, R&D and other IP 26%, structures 20%):
  Durable 0.37, Tech 0.17, Other 0.26, Construction 0.20;
- government purchases: HealthEducation 0.45 (health in kind and education), Other 0.25, Durable 0.10 (defence
  equipment), Nondur 0.03, Tech 0.04, Construction 0.13;
- in-kind health: HealthEducation; employer health benefits: HealthEducation; owner maintenance and landlord
  operating costs: OtherServices; FISIM: OtherServices.

## Sector business-cycle targets (BLS CES employment, HP-filtered quarterly, 1990–2019, approximate)

| Sector | Employment cycle sd ÷ total nonfarm sd | corr with GDP cycle |
|---|---|---|
| Construction | ≈3.0 | ≈0.8 |
| DurableGoods (durable manufacturing) | ≈2.5–3.0 | ≈0.85 |
| Energy (mining) | ≈2.5 (oil-driven) | ≈0.3 |
| Tech (information) | ≈1.5 | ≈0.6 |
| LeisureHospitality | ≈1.2 | ≈0.7 |
| NondurableGoods | ≈1.2 | ≈0.7 |
| OtherServices (professional/business, financial) | ≈1.2 | ≈0.8 |
| Food | ≈0.5 | ≈0.3 |
| HealthEducation | ≈0.3 | ≈0 (acyclical) |

Literature: Long & Plosser (1983) and Horvath (2000) on multi-sector cycles; Foerster, Sarte & Watson (2011) on the
sectoral sources of industrial-production fluctuations; Acemoglu, Carvalho, Ozdaglar & Tahbaz-Salehi (2012) on network
propagation; Stock & Watson (1999) on the relative volatility of durables and construction.

Later stages: discretionary demand (durables, leisure, tech) that responds to rates, credit and excess money balances
(Mian & Sufi 2014; Aguiar & Bils 2015; Friedman & Schwartz on excess money); a regime-switching tech productivity
process with sector equity valuation and extrapolative expectations (Pástor & Veronesi 2009; Greenwood & Jovanovic
1999; Jermann & Quadrini 2007; Beaudry & Portier 2006).

## Stage 3: discretionary spending (consumption.rs, DESIGN §8.2a)

Consumer-sector discretionary weights (share of supernumerary spending that responds to the discretionary
multiplier): Energy 0, Food 0.1, DurableGoods 1.0, NondurableGoods 0.3, Tech 0.6, LeisureHospitality 0.9,
HealthEducation 0.1, OtherServices 0.3. Necessities (Energy, Food, HealthEducation) are almost acyclical in US
data; durables, leisure and tech are the cyclical categories.

Consumption cycle targets (real household purchases by sector, HP-filtered quarterly, λ=1600; relative sd to GDP
and corr with GDP; Stock & Watson 1999): DurableGoods ≈2.9 / 0.8; NondurableGoods ≈0.6 / 0.7; Food, Energy and
HealthEducation ≈0.5 (necessities); OtherServices ≈0.5 / 0.7; LeisureHospitality ≈1.0 / 0.7; Tech ≈1.5 / 0.6;
total consumption ≈0.8 / 0.85. Measured by `python3 scripts/consumption_cycle.py`.

## Stage 4: technology booms and sector equity cycles

Technological revolutions come in waves (the 1995–2004 IT boom, the AI boom). Productivity growth in the producing
sector jumps for several years, aggregate TFP growth rises by about 1pp (Jorgenson, Ho & Stiroh 2008; Fernald 2014),
and the new sector's equity overshoots and then falls (Nasdaq −78% and S&P 500 −49% in 2000–02; Pástor & Veronesi
2009; Greenwood & Jovanovic 1999; Jermann & Quadrini 2007; Beaudry & Portier 2006 on news; Barberis et al. 2015 on
extrapolation).

**Productivity regime (exog.rs).** A two-state Markov chain, normal and boom, drawn once a month from the EXOG stream
only when `tech_boom_enabled` is set. Entry 1/240 per month (one boom per ≈20 years), exit 1/96 (booms ≈8 years;
stationary boom share ≈29%). In a boom Tech productivity growth rises by `tech_boom_extra_growth` = 5pp a year, and
DurableGoods and OtherServices (adopters) by `tech_boom_spillover` = 1pp. Level gains persist after a boom; growth
returns to trend. `Shock::TechBoom { months }` forces a boom for scenarios and IRFs.

**Sector equity (equity.rs).** Firms' regular dividends are attributed to the paying sector (bank dividends to
OtherServices). Each sector is valued as
`E_s = 12 × smoothed dividends_s ÷ max(0.02, y_eq + ERP_t + erp_sector_s − tech_boom_erp_discount·[Tech, boom] − g_s^e)`,
with `g_s^e = clamp(long_growth + ext_s + news_weight × news_s, [−2%, +6%])`. The three terms are:
- `ext_s = clamp(sector_growth_extrapolation × (x_s − x_agg), ±1.5pp)`, where `x_s` is the sector's annualised
  dividend growth from a fast-over-slow EWMA ratio (`12·ln(F/S)/(lag_S − lag_F)`, slow decay 0.99), and `x_agg` is the
  same signal for total dividends, so the common business cycle cancels;
- `news_s` is an EWMA (decay 0.98) of the sector's excess productivity growth during a boom (Beaudry & Portier news);
- the Pástor & Veronesi channel lowers Tech's premium by 1pp during a boom.

The total Fund equity is Σ E_s. The wealth channel and the Fund share price are unchanged in form. Sector Tobin's
q = E_s ÷ (capital of sector s × capital price), relative to its normal level at the first valuation, feeds back to
investment (firms.rs): desired capital is multiplied by `q_rel^investment_q_sens`, clamped to [0.75, 1.33]. Only Tech
has a nonzero sensitivity (0.5).

**Round 7 (final HEAD; `sector_cycle.py` and `consumption_cycle.py`, 8 seeds).** Energy employment relative sd is 4.62 (BLS 2.50) and Energy consumption relative sd is 1.14 (US 0.50). Equity (`equity_check.py`, 8 seeds): P/E 20.1, equity/GDP 2.33, price-return sd 10.0% and total-return sd 12.7%. The stage-4 figures below are the calibration history; the round-7 values are these.

**Calibration and findings (stage 4 values).** `erp_sector` = 0.8pp in every sector. With the regime off (8 seeds x 20 years),
P/E is 19.7 and equity/GDP 2.10 (the stage-3 model gave 22 and 2.4). The price-return standard deviation is 9.9%, against 11.2% before. The
extrapolative term is noisy: with it on at full weight, the Fund price return standard deviation rose from 11% to 25%.
Measuring growth relative to the economy-wide signal and capping the term brought it back to about 12%.

**Known gap.** In this model, realised Tech dividends do not accelerate in a boom, because cost-plus pricing passes
productivity into prices. The re-rating therefore comes from the news channel, not from realised earnings growth.

## Integration round 4

The sector expansion is unchanged by the merge. Two points are new. Energy's realised exports are deflated by the Energy
price, and Energy's labour requirement follows the lagged oil ratio through the extraction factor (firms.rs). Measured
Energy cyclicality after round 7 (8 seeds, `sector_cycle.py`): relative sd 4.62 (BLS 2.5), corr with GDP 0.07 (BLS 0.30).
The round-6 figure of 3.83 is superseded. See
docs/CALIBRATION.md for the aggregate results.
