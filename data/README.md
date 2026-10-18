# Data and where it comes from

Everything under `data/` is research input to the world block (twelve foreign bloc models, trade, oil and finance
links). This file lists every data file in the repository, where its numbers come from, the address of the source,
and the terms that were read. The terms were read on 2026-10-18 by opening the pages named below. A line saying "not
opened" means the page could not be opened; nothing here is guessed.

The Apache-2.0 licence of the repository covers EconSim's own code and work. It does not change the terms of the
original data. The attribution lines are repeated in the root `NOTICE`.

**Do not edit the files under `data/world/research/`.** The four `params.json` files and the five bloc CSV files are
hashed into the `calibration_hash` of the compiled-in parameter file
(`crates/econsim-core/src/world/data/world_params.json`). `python3 scripts/world/build_world_params.py --check` fails if
any of them changes. The `_meta` blocks of the `params.json` files mention working files of the research stage
(a report, a `data/SOURCES` file, build scripts) that are not part of this release; they are kept as they are for that
reason. In the `params.json` files each number carries a `source` and, in the trade file, a status tag
(`read`, `secondary`, `own`, `assumed`, `unverified`): values tagged `secondary`, `assumed` or `unverified` were not
checked against a primary document.

## Files under `data/`

| File | What it is | Source of the numbers | Address | Terms |
|---|---|---|---|---|
| `world/research/blocs/params.json` | Calibration of the twelve bloc models: GPM6 coefficients per region, bloc GDP weights, steady states, targets from studies | IMF Working Paper WP/13/87 (GPM6), Tables 1 to 6; World Bank WDI GDP (NY.GDP.MKTP.CD, NY.GDP.MKTP.PP.CD); IMF WEO (gap fills for Taiwan, UAE 2025, South Sudan, Venezuela; medium-term average growth through the DataMapper API); NY Fed Holston-Laubach-Williams trend growth; published studies (Iacoviello and Navarro 2019, Degasperi and others, Georgiadis, and others), central-bank pages | https://www.imf.org/external/pubs/ft/wp/2013/wp1387.pdf ; https://datacatalog.worldbank.org/public-licenses ; https://www.imf.org/en/About/copyright-and-terms ; https://www.newyorkfed.org/research/policy/rstar | World Bank datasets: CC BY 4.0 with attribution (the licence page and the NY.GDP.MKTP.CD indicator page, https://data.worldbank.org/indicator/NY.GDP.MKTP.CD, opened). IMF data: reuse with attribution, but commercial reuse needs IMF permission (opened). IMF publications: limited extracts only (opened). NY Fed page: gives a citation line, no licence (opened). Studies: cited for the values they report |
| `world/research/blocs/data/bloc_spillover_coeffs_2024.csv` | 13 x 13 matrix of spillover coefficients between the US and the twelve blocs | Computed by the project: kappa x exports to the US / GDP, from OECD BIMTS 2024, World Bank GDP and the value-added multiplier of GPM6 Table 3 | OECD BIMTS (see the OECD row below) | Derived numbers; see the OECD and IMF rows |
| `world/research/blocs/data/bloc_trade_weights_2024.csv` | 13 x 13 matrix of bilateral trade shares (rows sum to 1) | Computed by the project from OECD BIMTS (balanced) 2024 | OECD | OECD: see below |
| `world/research/blocs/data/discretisation_check.csv` | Peak and half-life of each bloc's impulse responses, quarterly against monthly model | EconSim's own computation (no third-party data) | none | EconSim's own work |
| `world/research/blocs/data/own_shock_irfs_gpm.csv` | Own-shock impulse responses of the bloc models | EconSim's own computation from the GPM6 coefficients | none | EconSim's own work |
| `world/research/blocs/data/state_space_eigs.csv` | Eigenvalue summary of each bloc's state-space form | EconSim's own computation | none | EconSim's own work |
| `world/research/finance/params.json` | US financial links with the twelve blocs: Treasury holdings, direct investment, bank claims, dollar and risk-off sensitivities, validation targets | Treasury TIC system (SLT tables, SHL and SHC surveys); BEA international investment position and direct investment; BIS locational banking statistics; Federal Reserve Board H.10 trade weights; FRED series used in the project's own regressions (only the regression results are given, not the series); published studies | https://home.treasury.gov/data/treasury-international-capital-tic-system ; https://catalog.data.gov/dataset/treasury-international-capital-tic-tic-press-releases-annual-surveys-of-foreign-portfolio- ; https://www.bea.gov/help/faq/145 ; https://data.bis.org/help/legal ; https://www.federalreserve.gov/disclaimer.htm ; https://www.federalreserve.gov/releases/h10/ ; https://fred.stlouisfed.org/docs/api/terms_of_use.html | TIC: CC0 as listed by data.gov (opened). BEA: public domain, credit line requested (opened). BIS statistics: use unrestricted if BIS is cited as source (opened). Federal Reserve Board: public domain, cite the Board (opened). FRED: third-party series stay under their owners' terms (opened); no third-party series are republished |
| `world/research/oil/params.json` | Calibration of the monthly world oil market (demand, supply groups, OPEC+ rule, inventories, shocks) | EIA International Energy Statistics and Short-Term Energy Outlook (October 2026); FRED monthly Brent and WTI (EIA series MCOILBRENTEU, MCOILWTICO); EIA Monthly Energy Review; OPEC press releases (decisions, as facts); published studies (Kilian, Hamilton, Baumeister and Hamilton, Kanzig, and others) | https://www.eia.gov/outlooks/steo/ ; https://www.eia.gov/international/data/world ; https://www.eia.gov/about/copyrights_reuse.php | EIA content is in the public domain; credit EIA and give the publication date (opened) |
| `world/research/oil/data/eia_intl_annual_liquids_1990_2025.csv` | EIA petroleum and other liquids, annual, 1990-2025: 1,196 series (consumption, production by group and country), 34,974 rows | EIA International Energy Statistics (series ids `INTL.*`) | https://www.eia.gov/international/data/world | Public domain; credit EIA (EIA reuse page opened; the data page itself was opened only to the section level) |
| `world/research/oil/data/steo_monthly_tidy.csv` | EIA Short-Term Energy Outlook tables 1 to 10, monthly 2022-01 to 2027-12, tidy form, 68,726 rows, with a `forecast` flag | EIA Short-Term Energy Outlook, October 2026 release | https://www.eia.gov/outlooks/steo/ | Public domain; credit EIA and the publication date |
| `world/research/oil/data/oil_model_irf_check.csv` | Impulse-response checks of the oil model against the research bands | EconSim's own computation against bands from Kilian-type SVAR studies | none | EconSim's own work |
| `world/research/oil/data/oil_model_replay_2026.csv` | The oil model replayed over March to August 2026 against observed Brent and the EIA STEO path | EconSim's own computation; observed Brent from FRED (EIA series) and EIA STEO | https://www.eia.gov/outlooks/steo/ | EIA public domain; the model columns are EconSim's own work |
| `world/research/trade/params.json` | Trade and price block: bloc trade matrices, sector shares, value chains, invoicing currency, pass-through, elasticities, tariffs and retaliation, validation targets | OECD BIMTS and TiVA (2025 edition), aggregated by the project; Census Bureau foreign trade (Exhibit 13) and BEA tables; ECB "The international role of the euro" (June 2026); The Budget Lab at Yale, State of U.S. Tariffs (April and August 2026); Bown at PIIE, US-China trade war tariffs chart (August 2026); published studies (Gopinath and others, Fajgelbaum and others, Amiti and others, Cavallo and others, and many more) | https://www.oecd.org/en/about/oecd-open-by-default-policy.html ; https://www.census.gov/about/policies/citation.html ; https://www.ecb.europa.eu/pub/ire/html/index.en.html ; https://www.ecb.europa.eu/services/disclaimer/html/index.en.html ; https://budgetlab.yale.edu/research/state-us-tariffs ; https://www.piie.com/research/piie-charts/us-china-trade-war-tariffs-date-chart ; https://piie.com/terms-use | OECD: page not opened (HTTP 403); a web search of the OECD policy page gives CC BY 4.0 as the default. Census: asks to be cited as the source (opened). ECB: the report page lists the June 2026 edition; free use if reproduced accurately and the ECB is cited (both opened). Yale Budget Lab: page footer reads "All rights reserved", no data licence found (opened). PIIE: terms require permission to redistribute or make derivative works (opened). See "Items to confirm" |

## Data-bearing files outside `data/`

| File | What it is | Source | Terms |
|---|---|---|---|
| `scripts/data/us_now.json` | The snapshot of "Today's US economy" compiled into `econsim-server`: 24 monthly values of the policy rate (upper bound of the FOMC range), the 3m, 2y, 5y, 10y and 30y Treasury yields, the 30-year mortgage rate, WTI oil, the realised tariff rate, and the unemployment rate; vintage 2026-10-12 | Federal Reserve H.15 (via FRED DGS3MO, DGS2, DGS5, DGS10, DGS30, DFEDTARU); Freddie Mac Primary Mortgage Market Survey (FRED MORTGAGE30US); EIA spot prices (FRED DCOILWTICO); BEA customs duties over goods imports (FRED B235RC1Q027SBEA and A255RC1Q027SBEA); BLS unemployment rate (UNRATE) | Federal Reserve Board and BEA, EIA, BLS: public domain (opened). Freddie Mac: see "Items to confirm". The website's copy of this file also carries display values (S&P 500, ICE BofA credit spreads, University of Michigan expectations, Case-Shiller, a price-earnings ratio from multpl.com); they are not part of the model's input and were **left out of this repository** because their terms restrict redistribution |
| `scripts/world/usnow_inputs/census_bloc_trade.csv`, `category_map.csv` | US goods trade with the twelve blocs, 2024; Census end-use weights by tariff category, 2025 | U.S. Census Bureau foreign trade statistics (Exhibit 13 and end-use tables) | Credit the Census Bureau (opened) |
| `scripts/world/usnow_inputs/yale_overall_rate.csv`, `yale_partner_rates.csv`, `yale_product_rates.csv`, `yale_2025_mean.csv` | Effective US tariff rate on three dates (2025-01-01, 2025-12-31, 2026-08-24), by partner and product category, and the 2025 time average | The Budget Lab at Yale, The State of U.S. Tariffs (24 August 2026) | See "Items to confirm" |
| `scripts/world/usnow_inputs/retaliation_reference.csv`, `retaliation_reference_2025-01.csv` | Partners' average duty on US goods, current and January 2025 | Mainly Bown at PIIE (China: 31.9 percent, up 10.7 points since 20 January 2025); other rows are marked `unverified` or `assumed` in the file | See "Items to confirm" |
| `scripts/data/backcast_k.json`, `crates/econsim-core/src/world/data/usnow_ref.json` | Ratios and the dated tariff table of "Today's start", computed from the two groups above by `scripts/world/build_usnow_ref.py` | Derived | As the files they are computed from |
| `crates/econsim-core/src/world/data/world_params.json`, `bloc_solution.json`, `golden/*` | The compiled-in parameters, the solved bloc models and golden test vectors, generated by `scripts/world/` from the research files above | Derived from `data/` | As the files they are computed from; no other third-party data |
| `scripts/world/pstar.json`, `scripts/world/targets.json` | Steady-state table and the registry of validation rows | EconSim's own work, with values from the studies cited in the rows | Studies cited for the values they report |

The stylised-fact scorecard (`scripts/facts_*.py`, `docs/STYLIZED_FACTS.md`) and `docs/SECTORS.md` quote summary
statistics from public sources (BLS, BEA, Federal Reserve, CBO, Census, the Kenneth French Data Library, Robert
Shiller's data, published studies), each with its source. No raw series from those sources are included.

## Items to confirm

These were found while reading the terms. None of them changes the model's behaviour if the file stays as it is.

1. **IMF data and the GPM6 paper.** The IMF's terms allow reuse of its data with attribution but ask that commercial reuse
   be cleared with the IMF, and they limit reuse of its publications to short extracts. `data/world/research/blocs/params.json`
   holds IMF-derived numbers of two kinds. (a) WEO data: gap fills in the GDP weights (Taiwan, UAE, South Sudan, Venezuela)
   and medium-term average growth rates by country from the WEO DataMapper (the file marks the vintage as unverified).
   (b) About 270 of its 757 `source` entries cite the GPM6 paper (IMF WP/13/87) for coefficient values read from its
   tables (Tables 1 to 6: six regional coefficient sets and the posterior modes). They are model parameters, the compiled-in
   model needs them and they are covered by the calibration hash, so they were left in unchanged; Apache-2.0 allows
   commercial use downstream, which the IMF terms ask to be cleared.
2. **Freddie Mac PMMS.** The 24 monthly mortgage-rate values in `scripts/data/us_now.json` are PMMS numbers. The
   PMMS page allows use with attribution; the general freddiemac.com terms say data may not be redistributed or published
   without a licence, and the FRED series page says the data are copyrighted by Freddie Mac and reprinted with
   permission. They are compiled into `econsim-server` as the fallback for a request that sends no targets.
3. **The Budget Lab at Yale.** The tariff tables rest on one page whose footer says "All rights reserved"; no data
   licence was found. The figures are statistics (rates by partner and product on three dates), cited to the page.
   The Budget Lab's separate Tariff Rate Tracker code is MIT-licensed on GitHub and asks to be cited.
4. **PIIE.** The terms of the site require permission for redistribution and derivative works. Two numbers from the
   Bown chart are quoted with the source (China's average duty on US goods, and its rise since January 2025).
5. **OECD.** The OECD pages returned HTTP 403 to every fetch, so the licence was not read on oecd.org. A web search of
   the OECD's open-by-default policy reports CC BY 4.0 as the default licence for content from 1 July 2024, and terms
   close to CC BY 4.0 for earlier content. The two bloc CSV matrices are shares computed from OECD BIMTS; the trade file
   holds TiVA ratios (2025 edition).
6. **Unverified values.** Some numbers in the `params.json` files are tagged `secondary`, `assumed` or `unverified`
   (for example a value read from a search-engine summary of a page that was not opened). Treat them as assumptions.
