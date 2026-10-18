# Stylised facts: US targets and the model's gaps

> **Note.** Scorecards and logs cited here by short name (for example `final_16s`) are run records from working folders and are not part of this release; `scripts/stylized_facts.py` regenerates the scorecard from the model. Commit hashes name commits of the development history, which is not published.

This is the reference for calibration work on realism. It lists 208 empirical stylised facts in six dimensions. For
each fact it gives the US target (range, sample, source), the value on master, and a gap flag. It also gives the
diagnosed cause of each gap. Each dimension has its own measurement script, and one runner prints a combined
scorecard.

| Dimension | Script | What it covers |
|---|---|---|
| cycle | `scripts/facts_cycle.py` | Recession frequency, length and depth (BBQ, two negative quarters, Sahm rule); plucking and asymmetry; persistence; volatility and comovement of expenditure components, employment, productivity and wages; Okun; Phillips curve |
| rates | `scripts/facts_rates.py` | Policy rate and rule, yield-curve level, slope, volatility and factors, term premium, curve inversions before downturns, zero bound, credit and mortgage spreads, charge-offs, deposit betas, bank capital and profits |
| equity | `scripts/facts_equity.py` | Return level, volatility, tails, autocorrelation, volatility clustering, leverage effect, countercyclical volatility, drawdowns, valuation ratios and predictability, stock–bond correlation, sector returns, tech booms and busts |
| shocks | `scripts/facts_shocks.py` | Oil price process, gasoline and energy CPI pass-through, food CPI, household inflation expectations, the dollar, house-price dynamics, productivity |
| distribution | `scripts/facts_distribution.py` | Wealth and income inequality (levels and trends), earnings age profile, homeownership by age, unemployment by age, mortgage debt, foreclosures, consumption by income quintile |
| firms | `scripts/facts_firms.py` | Firm entry and exit (BDS), size, markups, profit and proprietors' shares, capacity utilisation, JOLTS worker flows, Beveridge curve, Shimer volatility, inventories |

## Running the scorecard

```sh
python3 scripts/stylized_facts.py path/to/econsim-cli                 # all six dimensions, ~100 s on a loaded machine
python3 scripts/stylized_facts.py CLI --json card --detail      # every fact, plus a JSON scorecard
python3 scripts/stylized_facts.py NEW_CLI --baseline card       # which facts changed flag against card
ECONSIM_CFG='{"oil_sigma":0.085}' python3 scripts/stylized_facts.py CLI --only shocks   # a config experiment
python3 scripts/stylized_facts.py CLI --markdown > tables.md           # regenerate the tables in this document
```

- **Runs.** The reference run set is 8 seeds × 240 months × 20,000 households, after the CLI's 72-month burn-in. The
  first 12 reported months are dropped. `equity` and `shocks` add 4 long runs each (480 months × 10,000 households),
  used for monthly return statistics, and `firms` runs the `exits` diagnostic binary on seed 1.
- **Shared cache.** The six scripts mostly need the same runs, so the runner hands each script a wrapper in place of
  the binary. The wrapper computes each distinct run once under a file lock and caps concurrent simulations
  (`--max-sims`, default 8). On master this needs 17 simulations; 40 further requests are served from the cache.
  Wall time is 96 s at a load average of about 21 on the 10-core development machine.
- **Determinism.** The model is deterministic given the binary, `ECONSIM_CFG` and the run arguments. A re-run
  reproduces every value in this document exactly. `--cache DIR` keeps runs between invocations. Cache entries are
  keyed on the binary's path, size and mtime, so a rebuild never reuses a stale run.
- **Single dimension.** Each script also runs on its own. Its docstring lists its knobs, for example
  `RATES_SEEDS` or `EQF_LONG_MONTHS`, and its option for writing its target table with sources as JSON.
- **Smoke tests.** `--seeds/--months/--hh` override the standard run set, for smoke tests only. The flags assume the
  reference set: statistics such as recession counts or the zero-bound share are noisy in short runs.

## Gap flags

A value inside the target range is **ok**. Otherwise the distance outside the range, divided by the fact's tolerance,
sets the flag: **minor**, **material** or **SEVERE**. The thresholds are set per script and documented in each
script's docstring:

| Script | minor | material | severe |
|---|---|---|---|
| equity, shocks, distribution, firms | ≤ 1 tolerance | ≤ 3 | > 3 |
| rates | ≤ 0.5 tolerance | ≤ 2 | > 2, or the wrong sign on a sign fact |
| cycle | ≤ 0.5 of half the range (in logs for counts, durations and ratios) | ≤ 2 | > 2 |

Facts that the CLI JSON cannot measure are listed as **n/a** with their targets, so they are ready once the outputs
listed under "Missing outputs" exist. Each target has a numeric range, a US sample and a citation. Where possible,
targets are post-1984 (Great Moderation) US moments; the 1950–2023 value is given where it differs materially. Most
targets were computed by the researchers from FRED, BLS, BEA, Census BDS, the SCF, the Fed DFA, the Ken French
library and Shiller's data, using the same code as is applied to the model. Published values are cross-checked
against the literature cited in each row.

## Scorecard

Master at commit 8910b23 (release binary, default config); 8 seeds x 240 months x 20000 households (+ each script's own long runs); measured 2026-10-10 00:05; wall 96 s (17 simulations, 623 s of single-thread simulation).

| Dimension | Facts | ok | minor | material | severe | n/a | Mean severity |
|---|---|---|---|---|---|---|---|
| cycle | 45 | 10 | 4 | 21 | 10 | 0 | 1.69 |
| rates | 36 | 11 | 4 | 13 | 8 | 0 | 1.50 |
| equity | 33 | 7 | 12 | 5 | 9 | 0 | 1.48 |
| shocks | 25 | 5 | 2 | 15 | 3 | 0 | 1.64 |
| distribution | 32 | 9 | 4 | 7 | 3 | 9 | 1.17 |
| firms | 37 | 8 | 9 | 10 | 3 | 7 | 1.27 |
| **total** | 208 | 50 | 35 | 71 | 36 | 16 | 1.48 |

85 of 192 measurable facts (44%) are ok or minor. Mean severity scores ok 0, minor 1, material 2 and severe 3, over measurable facts.

## Cross-cutting diagnosis

Most of the flagged facts trace back to eight root causes.

1. **Financial prices are deterministic, smoothed functions of the policy rate.**
   - The 10y yield's monthly change has sd 0.046pp (US 0.24). It moves almost one-for-one with the policy rate (corr
     0.86 vs 0.26), and the curve has no curvature factor (0.01% vs 3%).
   - The term premium changes by 0.014pp a month (US 0.11).
   - Monthly equity returns have 5.7% annualised volatility (US 15.6%) and an AC1 of 0.63 (US 0.02).
   - The dollar's monthly volatility is 0.19% (US 1.45%), and the mortgage–10y spread is constant.
   - Apart from user shocks, the only financial disturbance is the Smets–Wouters risk-premium shock. It enters
     households' and firms' real rates (consumption.rs, firms.rs), but never the term premium, the equity risk
     premium, credit spreads or the dollar.
2. **Too many shallow recessions.**
   - The model has 26 BBQ recessions per century (US 6–15), 30% of quarters with falling GDP (US 7–14%) and an HP GDP
     sd of 1.45 (US 0.99 after 1984).
   - The shock sigmas were fitted to 1954–2019 volatility, on a 1.8% trend.
   - Finite-population noise adds about a quarter of the cycle variance at 20,000 households: with every aggregate
     shock off, the HP sd is still 0.69 and there are 32 Sahm onsets per century.
   - The shocks are Gaussian and symmetric, so there is no negative skew.
3. **Labour adjusts symmetrically and too fast.**
   - firms.rs plan_firm posts vacancies for the whole hiring gap at once. Layoffs start only beyond an 8% excess band
     and then shed 40% of the excess a month.
   - Unemployment therefore behaves like a symmetric oscillator: the plucking slope is 0.26 (US 1.12), the ratio of
     rise to fall speed is 1.0 (US 2.2), u-falling phases last 24 months (US 59) and the quarterly AR(1) of u is 0.88
     (US 0.97).
   - Labour productivity is too procyclical (0.81 vs 0.23–0.48).
4. **Oil passes through too strongly into energy prices, and oil itself is too calm.**
   - The energy CPI elasticity to oil is 0.68 (US 0.28), and the gasoline elasticity is 0.85 (US 0.60).
   - Real oil volatility is 5.9% a month (US 9.2%), with no fat tails or clustering.
   - oil_base is fixed in nominal dollars, so the real oil price falls 2.5% a year.
5. **Credit and housing look backward.**
   - Spreads lag GDP instead of leading it: corr(GDP_t, spread_t−2q) is +0.09 (US −0.57). Charge-offs are acyclical
     (+0.10 vs −0.68).
   - Residential investment lags GDP by 4 quarters (US leads by 2) and is countercyclical.
   - House-price momentum is 0.38 (US 0.75), and 20-year house-price cycles are a fifth of the US size.
6. **Household demand is too smooth.**
   - sd(C)/sd(Y) is 0.52 (US 0.80), and durables are 1.48× as volatile as GDP (US 3.0×).
   - Spending follows a heavily smoothed permanent income, and durables are a flow share with no stock adjustment.
7. **The distribution compresses over time.**
   - The wealth Gini is 0.73 (US 0.82–0.86) and falls. The income Gini trend is −0.025 per decade (US +0.018).
   - Top-quintile consumption grows 1.8pp a year slower than bottom-quintile consumption.
   - Causes: retirees fully annuitise their wealth, heirs are chosen at random, everyone holds a common portfolio,
     entrants draw skill with a lower sigma (0.35 vs 0.45), and the consumption kinks are nominal.
8. **Firms are caught in a churn trap.**
   - The exit rate is 14% a year (US 8.8%) and rises by 8pp over 20 years (US: falling).
   - 76% of entrants exit within 2 years (US 34%), and entry is countercyclical (−0.21 vs +0.50).
   - Entrants are mechanical rebirths of dead slots. They start without customers and cannot build a customer base,
     because customers switch only to strictly cheaper firms.

## Business cycle (`scripts/facts_cycle.py`)

Quarterly moments use 3-month averages and HP(1600) cycles. Recessions are dated with Harding-Pagan BBQ on quarterly real GDP. The script's dating code reproduces the Dupraz-Nakamura-Steinsson (2019) Table A.1 dates exactly. US columns: postwar 1949-2019 and Great Moderation (GM) 1984-2019, both computed with the script's own code via `--fred`.

45 facts: ok 10, minor 4, material 21, severe 10

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| recessions per century (BBQ, quarterly GDP) | 6.00 .. 15.00; point 12.70 | 26.32 | material | postwar 12.7, GM 5.6 (NBER: 14.8 in 1945-2019, 8.3 in 1984-2019) | NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating); Harding & Pagan (2002 JME) BBQ rule; FRED, this script (--fred) GDPC1 |
| two-negative-quarter episodes per century | 5.50 .. 12.00; point 11.30 | 30.26 | SEVERE | postwar 11.3, GM 5.6 | FRED, this script (--fred) GDPC1 (the rule misses mild NBER recessions such as 1960 and 2001) |
| Sahm-rule onsets per century | 9.00 .. 18.00; point 16.90 | 36.18 | SEVERE | postwar 16.9, GM 11.1 | Sahm (2019, Hamilton Project 'Recession Ready'): the 0.5pp rule caught every recession since 1970 without a false positive; FRED, this script (--fred) UNRATE (the 1959 steel strike is the postwar false positive) |
| share of quarters in recession (BBQ), % | 4.00 .. 13.00; point 9.90 | 23.19 | material | postwar 9.9, GM 4.2 (NBER months: 13% 1945-2019, 7% 1984-2019) | NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating); FRED, this script (--fred) GDPC1 |
| mean expansion length (BBQ), months | 60.00 .. 120.0; point 73.50 | 30.00 | material | postwar 73.5 (NBER 1945-2020: 64.2; 1982-2020: 103) | NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating); FRED, this script (--fred) GDPC1 |
| mean recession length (BBQ), months | 8.00 .. 14.00; point 10.30 | 10.57 | ok | postwar 9.3 (NBER 1945-2020: 10.3) | NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating); FRED, this script (--fred) GDPC1 |
| mean peak-to-trough real GDP fall, % | -3.50 .. -1.80; point -2.40 | -1.76 | minor | postwar -2.4 (median -2.6; 1980 -2.2, 1990-91 -1.4, 2007-09 -4.0) | FRED, this script (--fred) GDPC1 BBQ; NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating) |
| u rise over a BBQ recession (+4q), pp | 2.20 .. 4.00; point 2.90 | 2.00 | minor | postwar 2.9, GM 3.1 | FRED, this script (--fred) UNRATE over GDPC1 BBQ windows |
| inventory share of recession GDP fall, % | 30.00 .. 90.00; point 47.00 | 43.16 | ok | postwar median 47, GM 33 (Blinder & Maccini: 87% of the fall in 1948-82 recessions) | Blinder & Maccini (1991 JEP 5:1); FRED, this script (--fred) CBIC1, GDPC1 |
| plucking: next u fall on u rise (slope) | 0.7 .. 1.40; point 1.12 | 0.255 | material | 1948-2020: 1.12 (R2 0.59) | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020 (this script reproduces their dates exactly) |
| plucking: next u rise on u fall (slope) | -0.7 .. 0.2; point -0.38 | 0.556 | material | 1948-2020: -0.38 (R2 0.22, insignificant) | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020 |
| speed of u rises / u falls (DNS cycles) | 1.70 .. 2.80; point 2.17 | 1.03 | material | 1948-2020: 1.89 vs 0.87 pp/yr; GM 2.2 | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020; Neftci (1984 JPE); McKay & Reis (2008 JME) |
| u-falling phase length (DNS), months | 45.00 .. 75.00; point 59.10 | 24.03 | SEVERE | 1948-2020: 59.1 (rising phases 26.9) | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020 |
| u decline in recoveries, log points/yr | 0.07 .. 0.17; point 0.1 | 0.39 | material | H&K 1949-2019 ~0.1; postwar 0.16, GM 0.10 | Hall & Kudlyak (2022 NBER Macro Annual 'The inexorable recoveries of unemployment'); FRED, this script (--fred) UNRATE on DNS phases |
| skewness of u (monthly level) | 0.3 .. 1.00; point 0.64 | -0.205 (seed sd 0.212) | material | 1948-2020: 0.64; GM 0.74 | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020; Sichel (1993 Econ. Inquiry) 'deepness' |
| skewness of quarterly changes in u | 0.8 .. 2.20; point 1.27 | -0.13 (seed sd 0.142) | material | postwar 1.27, GM 2.1 (19y windows 1.2-1.5) | Sichel (1993) 'steepness'; McKay & Reis (2008); FRED, this script (--fred) UNRATE |
| skewness of q/q GDP growth | -1.30 .. -0.05; point -0.3 | -0.0311 (seed sd 0.275) | minor | postwar -0.07 (19y windows -0.6), GM -1.3 | Jensen, Petrella, Ravn & Santoro (2020 AEJ:Macro): negative and deepening after 1984; FRED, this script (--fred) GDPC1 |
| sd of HP GDP cycle, % | 0.9 .. 1.30; point 1.00 | 1.45 (seed sd 0.218) | material | GM 0.99, 1990-2019 1.03 (postwar 1.53) | McConnell & Perez-Quiros (2000 AER); Stock & Watson (2002 NBER Macro Annual): volatility halved after 1984; Stock & Watson (1999, Handbook of Macroeconomics ch. 1); FRED, this script (--fred) GDPC1 |
| sd of q/q annualised GDP growth, % | 1.90 .. 2.70; point 2.25 | 3.28 (seed sd 0.349) | material | GM 2.25, 1990-2019 2.32 (postwar 3.7) | McConnell & Perez-Quiros (2000); Stock & Watson (2002); FRED, this script (--fred) GDPC1 |
| share of quarters with falling GDP, % | 7.00 .. 14.00; point 10.00 | 29.50 (seed sd 3.86) | SEVERE | postwar 13.4, GM 8.4 (1990-2019 10.1) | FRED, this script (--fred) GDPC1 |
| AR(1) of q/q GDP growth | 0.25 .. 0.45; point 0.35 | 0.576 (seed sd 0.095) | material | postwar 0.35, GM 0.37 | Cogley & Nason (1995 AER): positive autocorrelation at lags 1-2; FRED, this script (--fred) GDPC1 |
| AR(1) of HP GDP cycle | 0.82 .. 0.92; point 0.86 | 0.846 (seed sd 0.03) | ok | postwar 0.84, GM 0.88 | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) 0.84; FRED, this script (--fred) |
| AR(1) of quarterly u | 0.95 .. 0.99; point 0.97 | 0.879 (seed sd 0.0335) | SEVERE | postwar 0.97, GM 0.98 (19y windows 0.96-0.98) | FRED, this script (--fred) UNRATE |
| sd of monthly change in u, pp | 0.13 .. 0.22; point 0.17 | 0.281 (seed sd 0.00861) | material | postwar 0.21, GM 0.15 | FRED, this script (--fred) UNRATE (BLS: CPS sampling error of a monthly change ~0.1pp) |
| AR(1) of q/q core CPI inflation | 0.6 .. 0.9; point 0.75 | 0.518 (seed sd 0.133) | material | 1960-2019 0.88, GM 0.83 (19y windows 0.65-0.73) | Fuhrer (2010 Handbook of Monetary Econ. ch. 9); Stock & Watson (2007 JMCB); FRED, this script (--fred) CPILFESL |
| sum of AR(4) coefficients, core CPI inflation | 0.65 .. 0.95; point 0.9 | 0.843 (seed sd 0.0617) | ok | 1960-2019 0.91, GM 0.90 | Pivetta & Reis (2007 JEDC); FRED, this script (--fred) CPILFESL |
| sd(C)/sd(Y) | 0.72 .. 0.9; point 0.8 | 0.522 (seed sd 0.0487) | SEVERE | postwar 0.80, GM 0.83 | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) 0.74; FRED, this script (--fred) PCECC96 |
| corr(C, Y) | 0.78 .. 0.92; point 0.86 | 0.682 (seed sd 0.0879) | material | postwar 0.79, GM 0.87 | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) 0.88; FRED, this script (--fred) PCECC96 |
| lead of C over Y at peak corr, quarters | -0.5 .. 0.5; point 0 | 0.875 (seed sd 0.331) | minor | postwar 0, GM 0 (coincident) | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); FRED, this script (--fred) |
| sd(durables C)/sd(Y) | 2.60 .. 3.60; point 3.00 | 1.48 (seed sd 0.117) | SEVERE | postwar 3.1, GM 3.0 | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); FRED, this script (--fred) DDURRA3Q086SBEA |
| sd(nonresidential fixed I)/sd(Y) | 2.80 .. 4.30; point 3.10 | 3.68 (seed sd 0.227) | ok | postwar 2.9, GM 4.2 | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); FRED, this script (--fred) A008RA3Q086SBEA |
| sd(residential I)/sd(Y) | 5.00 .. 7.50; point 6.30 | 3.14 (seed sd 1.10) | SEVERE | postwar 6.2, GM 6.9 | Davis & Heathcote (2005 IER): residential ~2x as volatile as nonresidential; FRED, this script (--fred) A011RA3Q086SBEA |
| corr(residential I, Y) | 0.45 .. 0.75; point 0.6 | -0.138 (seed sd 0.217) | SEVERE | postwar 0.56, GM 0.57 | Davis & Heathcote (2005); FRED, this script (--fred) |
| lead of residential I over Y, quarters | 1.00 .. 3.00; point 2.00 | -3.88 (seed sd 0.331) | SEVERE | postwar 2, GM 2 | Leamer (2007 Jackson Hole) 'Housing IS the business cycle'; Fisher (2007 JPE); FRED, this script (--fred) |
| corr(inventory change/Y, Y) | 0.45 .. 0.7; point 0.6 | 0.342 (seed sd 0.0471) | material | postwar 0.62, GM 0.50 | Blinder & Maccini (1991); FRED, this script (--fred) CBIC1 |
| sd(employment)/sd(Y) | 0.8 .. 1.10; point 0.9 | 0.619 (seed sd 0.0396) | material | postwar 0.89, GM 1.06 (hours HOANBS ~1.0) | King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) hours 0.99; FRED, this script (--fred) PAYEMS |
| corr(employment, Y) | 0.72 .. 0.9; point 0.8 | 0.802 (seed sd 0.0336) | ok | postwar 0.81, GM 0.80 | King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1); FRED, this script (--fred) PAYEMS |
| corr(output per worker, Y) | 0 .. 0.5; point 0.25 | 0.806 (seed sd 0.0288) | material | postwar 0.48, GM 0.23 | Gali & van Rens (2021 EJ) 'vanishing procyclicality of labour productivity'; FRED, this script (--fred) GDPC1/PAYEMS |
| corr(output per worker, employment) | -0.5 .. -0.05; point -0.25 | 0.296 (seed sd 0.0519) | material | postwar -0.13, GM -0.40 | Gali & van Rens (2021); Stiroh (2009 JEDC); FRED, this script (--fred) |
| sd(real wage)/sd(Y) | 0.35 .. 1.10; point 0.6 | 0.381 (seed sd 0.0683) | ok | postwar 0.62, GM 1.07 (real comp./hour) | King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) 0.38 (avg hourly earnings); FRED, this script (--fred) COMPRNFB |
| corr(real wage, Y) | -0.1 .. 0.4; point 0.15 | -0.0776 (seed sd 0.131) | ok | postwar 0.18, GM -0.02 | King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1) 0.12; Solon, Barsky & Parker (1994 QJE): procyclical net of composition; FRED, this script (--fred) COMPRNFB |
| Okun: u cycle per 1% GDP cycle (HP) | -0.55 .. -0.4; point -0.45 | -0.479 (seed sd 0.0465) | ok | postwar -0.44, GM -0.54 | Ball, Leigh & Loungani (2017 JMCB) US levels -0.4 to -0.5; FRED, this script (--fred) |
| Okun: 4q change in u per 1pp 4q growth | -0.45 .. -0.3; point -0.38 | -0.493 (seed sd 0.0497) | material | postwar -0.37, GM -0.41 | Ball, Leigh & Loungani (2017); Knotek (2007 KC Fed Econ. Review); FRED, this script (--fred) |
| Phillips: core PCE on u gap given lagged π | -0.3 .. -0.03; point -0.1 | -0.212 (seed sd 0.109) | ok | 1960-2019 -0.19, GM -0.07 | Blanchard (2016 AER P&P); Del Negro, Lenza, Primiceri & Tambalotti (2020 BPEA); Hazell, Herreno, Nakamura & Steinsson (2022 QJE): flat since the 1990s; FRED, this script (--fred) PCEPILFE, NROU |
| corr(core PCE infl. cycle, u cycle) | -0.6 .. -0.25; point -0.45 | -0.104 (seed sd 0.0912) | material | GM -0.46 (1960-2019 -0.04: 1970s) | Stock & Watson (1999, Handbook of Macroeconomics ch. 1) (inflation procyclical with a lag); FRED, this script (--fred) PCEPILFE, UNRATE |

### Diagnosis of the flagged facts

- **Recession frequency, two-quarter declines, Sahm onsets, expansion length, negative-quarter share, GDP volatility.**
  - The risk-premium and TFP sigmas (exog.rs step; config `risk_premium_sigma`, `tfp_shock_sigma`) were re-fitted
    to the 1954-2019 cycle sd of about 1.5, not to post-1984 volatility, on a 1.8% trend.
  - Agent-sampling noise adds to this. With all aggregate shocks off, the HP GDP sd is 0.69 at 20k households and
    0.99 at 10k (proportional to 1/√N), and there are 15 BBQ recessions and 32 Sahm onsets per century.
  - Lumpy firm-level layoffs, at about 25 households per firm, are granular noise. The monthly change in u has sd
    0.28 (US 0.15-0.21) and a negative AR(1) (-0.15 vs +0.1).
- **No plucking, symmetric unemployment, fast recoveries** (pluck and anti-pluck slopes, speed ratio, u-falling phase
  length, Hall-Kudlyak rate, u skewness, quarterly AR(1) of u).
  - firms.rs plan_firm posts the whole labour gap l* − l as vacancies at once, while layoffs wait for 8% excess
    (`layoff_trigger` 1.08) and then shed 40% a month. Hiring is therefore faster than firing, the reverse of the US.
  - The wage Phillips term is linear. Downward nominal wage rigidity caps only stayers' wage cuts and never causes
    layoffs.
  - u* adapts toward the actual rate.
  - Booms overshoot: in seed 1, u falls from 7.0% to 2.3% in 8 quarters.
- **Consumption and durables too smooth** (sd(C)/sd(Y), corr(C,Y), C lead, durables).
  - consumption.rs spends out of a heavily smoothed permanent income (yp = 0.9·yp + 0.1·y each month).
  - Durables are a linear-expenditure-system flow share with no stock adjustment.
  - The discretionary multiplier reacts to the real rate at once, so C leads GDP.
- **Residential investment half as volatile, countercyclical and lagging.** housing.rs construction() drives starts
  from (HPI/build cost)³ times a vacancy and pipeline factor. HPI lags the cycle, and there is no sales or months'
  supply channel.
- **Employment and productivity.**
  - There is a hoarding band and no hours margin (employment counts heads).
  - TFP shocks are common to all firms, so output per worker co-moves with GDP (0.81) and with employment (+0.30).
- **Inflation.** Core inflation is short-lived (q/q AR(1) 0.52) and weakly tied to the u cycle in the same quarter
  (-0.10).
  - Price resets and wage norms anchor on the target and expected inflation.
  - About 800 Calvo price-setters add sector price noise: the shock-free AR(1) is 0.20.
  - Inflation lags output by 4-6 quarters.
- **Okun, difference form** (-0.49). Only heads move, and participation and hours absorb none of the cycle.

## Rates, yield curve and credit (`scripts/facts_rates.py`)

Recessions are dated from unemployment (Sahm-rule onsets, and rises of at least 1pp within 18 months), the same way in the model and the US data, because two-quarter GDP declines in the model are mostly noise. US sample 1985-2019 unless stated. Model statistics are means over seeds; recession statistics and the policy-rule regression pool all seeds.

36 facts: ok 11, minor 4, material 13, severe 8

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| Policy rate mean, % | 3.00 [2.50 .. 3.50]; 1985-2019 FEDFUNDS mean 3.59% (higher-inflation early years) | 3.07 (seed sd 0.275) | ok | 2019-2025 FOMC SEP longer run | FOMC Summary of Economic Projections, longer-run fed funds median 2.5% (2019-23) to 3.0% (2025); Holston-Laubach-Williams r* ~0.75% + 2% target |
| 10y - 3m slope mean, pp | 1.74 [1.40 .. 2.10]; 1962-2019 (TB3MS): 1.52 | 1.43 (seed sd 0.121) | ok | 1985:01-2019:12 | FRED GS10, GS3M (H.15 constant maturity); own calculation |
| 30y - 10y slope mean, pp | 0.48 [0.3 .. 0.7] | 0.518 (seed sd 0.0328) | ok | 1985:01-2019:12 (30y gap 2002-06) | FRED GS30, GS10; own calculation |
| sd of 10y - 3m slope, pp | 1.12 [0.9 .. 1.40]; 1962-2019: 1.23 | 0.555 (seed sd 0.124) | material | 1985:01-2019:12 | FRED GS10, GS3M; own calculation |
| Share of months with 10y < 3m | 0.062 [0.03 .. 0.12]; 1962-2019 (TB3MS): 0.10 | 0.0115 (seed sd 0.0233) | material | 1985:01-2019:12 | FRED GS10, GS3M; own calculation |
| sd of monthly change, 3m yield, pp | 0.2 [0.15 .. 0.25]; 1962-2019 (TB3MS): 0.43 (Volcker era) | 0.132 (seed sd 0.0119) | minor | 1985:01-2019:12 | FRED GS3M; own calculation (1990-2007: 0.205) |
| sd of monthly change, 10y yield, pp | 0.24 [0.2 .. 0.28]; 1962-2019: 0.28 | 0.0458 (seed sd 0.00408) | SEVERE | 1985:01-2019:12 | FRED GS10; own calculation (1990-2007: 0.223) |
| sd(d10y) / sd(d3m) | 1.21 [0.9 .. 1.40]; 1962-2019: 0.66 (1979-82 short-rate volatility) | 0.348 (seed sd 0.0149) | material | 1985:01-2019:12 | FRED; excess volatility of long rates: Shiller (1979) JPE 87(6); Gurkaynak, Sack & Swanson (2005) AER 95(1) |
| corr(d3m, d10y), monthly | 0.45 [0.35 .. 0.6]; 1962-2019: 0.58 | 0.859 (seed sd 0.0287) | material | 1985:01-2019:12 | FRED GS3M, GS10; own calculation (1990-2007: 0.41) |
| corr(d policy rate, d10y), monthly | 0.26 [0.1 .. 0.4] | 0.857 (seed sd 0.0294) | SEVERE | 1985:01-2019:12 | FRED FEDFUNDS, GS10; own calculation (1990-2007: 0.16; corr with d2y 0.50): long yields move mostly on news other than the current policy move (Gurkaynak, Sack & Swanson 2005) |
| PCA of monthly yield changes: level share | 0.83 [0.78 .. 0.9] | 0.959 (seed sd 0.00969) | material | 1985:01-2019:12, tenors 3m/2y/5y/10y/30y | FRED; own calculation. Litterman & Scheinkman (1991) J. Fixed Income 1(1): level ~89%, slope ~8%, curvature ~2% |
| PCA of monthly yield changes: curvature share | 0.032 [0.015 .. 0.05] | 0.00013 (seed sd 3.45e-05) | SEVERE | 1985:01-2019:12 | FRED; own calculation (slope share 0.13). Litterman & Scheinkman (1991) |
| Campbell-Shiller beta, 10y (m = 3m) | -1.35 [-3.00 .. -0.5]; 1962-2019: -2.9 | -3.49 (seed sd 0.429) | minor | 1985:01-2019:12 | Campbell & Shiller (1991) REStud 58(3): beta < 0 and falling with maturity (expectations hypothesis implies +1); FRED GS10/GS3M own calculation with y(117m) ~ y(120m) |
| 10y term premium mean, pp | 0.97 [0.5 .. 1.50] | 1.21 (seed sd 0.0524) | ok | 1990:01-2019:12 | Kim & Wright (2005) FEDS 2005-33, FRED THREEFYTP10 (2000-2019 mean 0.60; ACM similar order) |
| sd of monthly change, 10y term premium, pp | 0.11 [0.08 .. 0.15] | 0.0141 (seed sd 0.000792) | SEVERE | 1990:01-2019:12 | FRED THREEFYTP10 (Kim-Wright); own calculation |
| cov(dTP10, d10y) / var(d10y) | 0.48 [0.35 .. 0.65] | 0.107 (seed sd 0.0187) | material | 1990:01-2019:12 | FRED THREEFYTP10, GS10; own calculation (corr 0.94): about half of long-rate moves are term-premium moves |
| corr(TP10 cycle, GDP cycle) | -0.18 [-0.6 .. -0.05] | -0.719 (seed sd 0.113) | minor | 1990Q1-2019Q4, HP cycles | FRED THREEFYTP10, GDPC1; own calculation; countercyclical term premia: Adrian, Crump & Moench (2013) JFE 110(1); Cochrane & Piazzesi (2005) AER 95(1) |
| AUC: -slope_t predicts u +1pp within 18m | 0.81 [0.7 .. 0.9] | 0.749 | ok | 1962:01-2019:12 (1985-2019: 0.73) | FRED UNRATE (3-month mean), GS10, TB3MS/GS3M; own calculation (NBER USREC 12m ahead: 0.87). Estrella & Mishkin (1998) REStat 80(1); Estrella & Hardouvelis (1991) JF 46(2) |
| Share of Sahm-rule onsets after inversion (24m) | 0.83 [0.7 .. 1.00] | 0.0435 | SEVERE | 1962-2019 (1985-2019: 3/3) | FRED UNRATE, GS10, TB3MS; own calculation, leads 10-18 months. Bauer & Mertens (2018) FRBSF Economic Letter 2018-07: every recession in 60 years preceded by a 10y-3m inversion, one false positive |
| Policy change around Sahm onsets, pp | -4.00 [-5.50 .. -2.50] | -0.679 | material | 1962-2019 (1985-2019: -4.5) | FRED FEDFUNDS: rate 12 months after a Sahm-rule onset minus its 12-month pre-onset maximum (1990 -3.6, 2001 -4.8, 2008 -5.0); own calculation |
| Policy rule smoothing rho (quarterly) | 0.85 [0.75 .. 0.92] | 0.91 | ok | 1987Q3-2007Q4 | FRED FEDFUNDS, PCEPILFE, UNRATE: i_t = c + rho i_t-1 + b pi_t + d u_t (own estimate 0.85; model: pooled over seeds with seed intercepts, quarters above the zero bound); Clarida, Gali & Gertler (2000) QJE 115(1): rho ~0.79 |
| Long-run response to core inflation | 2.27 [1.50 .. 2.50] | 2.83 | material | 1987Q3-2007Q4 | Own estimate as above (2.27); Clarida, Gali & Gertler (2000): ~2.15; Taylor (1993): 1.5 |
| Long-run response to unemployment | -2.20 [-2.50 .. -1.00] | -2.22 | ok | 1987Q3-2007Q4 | Own estimate as above (-2.20); CGG output-gap 0.93 x Okun 2 ~ -1.9; Taylor (1993) 0.5 x 2 = -1.0 |
| Share of months at the zero bound | 0.13 [0.05 .. 0.4] | 0 (seed sd 0) | material | 1954-2023 / 1985-2023 | FRED FEDFUNDS < 0.25: 13% of 1954-2023, 23% of 1985-2023; Kiley & Roberts (2017) BPEA: 30-40% when the neutral nominal rate is ~3% |
| Corporate credit spread mean, pp | 2.00 [1.70 .. 2.50] | 1.56 (seed sd 0.182) | minor | 1985:01-2019:12 | Gilchrist & Zakrajsek (2012) AER 102(4) GZ spread, Fed ebp_csv (mean 2.01); Moody's Baa-10y FRED BAA10YM 2.32 |
| Credit spread cycle sd (HP, quarterly), pp | 0.55 [0.4 .. 0.7] | 0.201 (seed sd 0.0768) | material | 1985Q1-2019Q4 | GZ spread 0.64, Baa-10y 0.48; own calculation |
| corr(spread cycle, GDP cycle), same quarter | -0.45 [-0.6 .. -0.25] | -0.336 (seed sd 0.148) | ok | 1985Q1-2019Q4 | Baa-10y -0.45, GZ -0.28 (1962-2019 Baa -0.58); own calculation; Gilchrist & Zakrajsek (2012) |
| corr(GDP_t, spread_t-2q): spreads lead | -0.57 [-0.7 .. -0.4] | 0.086 (seed sd 0.133) | SEVERE | 1985Q1-2019Q4 | Baa-10y -0.61, GZ -0.53, EBP -0.46 (corr with GDP 2q later; lagging corr ~0 to +0.2); Gilchrist & Zakrajsek (2012): spreads and EBP predict activity |
| 30y mortgage - 10y spread mean, pp | 1.69 [1.50 .. 1.90] | 1.70 (seed sd 0.00014) | ok | 1985:01-2019:12 | FRED MORTGAGE30US (Freddie Mac PMMS) - GS10; own calculation (1990-2019: 1.66; 2022-23: 2.61) |
| sd of mortgage - 10y spread, pp | 0.31 [0.25 .. 0.45] | 0.000723 (seed sd 0.00134) | SEVERE | 1985:01-2019:12 | FRED; own calculation: range 1.2-3.2, 2008 peak 2.9, 2022-23 peak 3.0; corr with Baa-10y 0.61. Fuster, Lo & Willen (2017) on time-varying intermediation spreads |
| C&I loan charge-off rate mean, %/yr | 0.82 [0.5 .. 1.10] | 0.871 (seed sd 0.122) | ok | 1985Q1-2019Q4 | Federal Reserve charge-off rates, FRED CORBLACBS (median 0.45, p90 1.7, max 2.6) |
| corr(charge-off cycle, GDP cycle) | -0.68 [-0.8 .. -0.45] | 0.105 (seed sd 0.267) | SEVERE | 1985Q1-2019Q4 | FRED CORBLACBS, GDPC1, HP cycles; own calculation (peaks 0-1 quarters after GDP) |
| Deposit beta, 3-month changes | 0.15 [0.05 .. 0.25] | 0.5 (seed sd 2.56e-16) | material | 2022 tightening | S&P Global Market Intelligence (2023): quarterly deposit-cost betas 18% (IB, 2022Q2), 24% (2022Q3), 33% (2022Q4); cumulative ~0.22 after three quarters (+90bp costs vs ~+4pp fed funds) |
| Deposit beta, 24-month changes | 0.4 [0.3 .. 0.5] | 0.5 (seed sd 3.1e-16) | ok | 1994-2013 Call Reports | Drechsler, Savov & Schnabl (2017) QJE 132(4): mean deposit spread beta 0.54 with 4 quarterly lags, so deposit rate beta ~0.46 (savings deposits 0.24-0.34); 2022-23 cycle cumulative ~0.35-0.45 |
| Bank risk-based capital ratio, % | 15.00 [13.00 .. 16.00] | 18.13 (seed sd 1.33) | material | 2010-2024 | FDIC Quarterly Banking Profile 2024Q1: total risk-based 15.4%, tier 1 14.0%, leverage 9.2%; equity/assets 2010-19 11.2% (FRED EQTA) |
| Bank net income / GDP, % | 1.00 [0.8 .. 1.20] | 1.57 (seed sd 0.0553) | material | 2015-2023 | FDIC QBP: net income $233.1bn (2019) and $256.9bn (2023) = 1.08% and 0.93% of GDP; ROA ~1.0-1.1% (FRED USROA) |

### Diagnosis of the flagged facts

- **Long rates far too smooth; one-factor curve** (sd of Δ10y, sd(Δ10y)/sd(Δ3m), corr(Δ3m, Δ10y), corr(Δpolicy, Δ10y),
  PCA level and curvature, sd of ΔTP10, the term premium's share of 10y moves).
  - In monetary.rs update_curve, each yield is a deterministic geometric path from the current policy rate to
    i_inf = r*_est + 2% (PATH_DECAY 0.97), plus a term premium.
  - The term premium moves only with duration supply (tanh of log D/D0, which tracks debt/GDP with corr 0.94) and
    with |π_e − π*|. It has no stochastic factor, and long-run expectations ignore news.
  - The 10y loads about 0.27 on the policy gap.
  - The weights and the TP_SLOPE loadings are both monotone in maturity, so no curvature factor can arise.
- **Term premium too countercyclical** (-0.72). It is tied to the duration of privately held debt, which deficits
  make countercyclical, and it drifts up with debt (0.85 → 1.35).
- **Curve rarely inverts; easing too small; zero bound never reached** (slope sd, inversion share, inversions before
  Sahm onsets, policy change around onsets, zero-bound share).
  - Policy sd is 0.8 (US 1.7-2.6). Cuts around downturns average 0.7pp (US about 4).
  - set_policy reacts to realised year-on-year core PCE, which lags output by about 4 quarters.
  - An inversion needs the policy rate about 1.7pp above neutral, given a near-constant TP10 of 1.2.
  - So the zero-bound and QE scenarios start from states the baseline never visits.
- **Long-run response to inflation 2.83** (US 2.27). The adaptive r* (`r_star_adapt` 0.5/yr per pp of inflation miss)
  adds integral action on top of the direct 1.5 coefficient.
- **Credit spreads lag the cycle** (spread lead, cycle sd).
  - banking.rs expected_loss_rates and step_premium price expected losses from current leverage and coverage. These
    worsen only after output falls.
  - The exogenous risk-premium shock never reaches spreads.
  - The loss norm adapts 1% a month, so the spread drifts down from 2.26 to 1.45 over 10 years.
- **Mortgage spread constant** (sd 0.0007). set_rates sets the mortgage rate to 10y + 1.7 + a capital shortfall
  premium that never binds. There is no link to the lender premium, to rate volatility (the prepayment option) or to
  MBS QE.
- **Charge-offs acyclical** (+0.10). firms.rs fails firms on book net worth and cash. Failures are dominated by
  idiosyncratic young-firm churn, and the metric is a trailing 12-month sum.
- **Deposit beta 0.5 at 3 months.** set_rates sets deposit_rate = 0.5 × policy at once, with no partial adjustment or
  asymmetry.
- **Bank capital 18% and net income 1.6% of GDP.**
  - Risk-weighted assets count only mortgages (0.5) and business loans (1.0).
  - Banks retain earnings until they are 2pp above requirement.
  - A wide fixed mortgage spread and a 0.5 deposit beta give a high net interest margin.

## Equity market (`scripts/facts_equity.py`)

Monthly total returns are rebuilt exactly from equity_return_12m. US targets were computed from CRSP (Ken French library), Shiller's data and FRED, at monthly frequency to match the model's tick. Recession statistics use BBQ dating on the 20k-household runs. Model values show the mean, the min..max over runs and the number of runs or episodes.

33 facts: ok 7, minor 12, material 5, severe 9

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| Real total return, geometric, %/yr | 6.00 .. 9.00; US 8.5 (1984-2024); 7.7 (1950-2024); 7.0 (1927-2024) | 5.27 [1.83..7.64, n=12] | minor | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Siegel (2014) Stocks for the Long Run ~6.7% 1802-2012; Dimson, Marsh & Staunton (2002) |
| Volatility of monthly returns, annualised, % | 14.00 .. 17.00; US 15.6 (1984-2024); 14.9 (1950-2024); 18.4 (1927-2024) | 5.68 [3.28..7.56, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Schwert (1989 JF); Campbell, Lo & MacKinlay (1997) ch. 1 |
| Sd of 12-month real returns (non-overlapping), % | 15.00 .. 20.00; US 16.9 (1984-2024); 17.5 (1950-2024); 19.8 (1927-2024) | 13.99 [7.64..20.63, n=12] | minor | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here deflated by CPI-U; Shiller (1981 AER); Campbell & Cochrane (1999) ~18% |
| Excess return over 3m bill, arithmetic, %/yr | 4.00 .. 8.50; US realised 8.9 (1984-2024), 8.1 (1950-2024); ex-ante 4.0 (Damodaran implied 1961-2012), 2.6-4.3 (Fama-French 2002, 1951-2000) | 4.38 [1.89..6.70, n=12] | ok | 1984-2024 / 1951-2000 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Mehra & Prescott (1985); Fama & French (2002 JF); Damodaran histimpl (NYU Stern) |
| Skewness of monthly returns | -1.00 .. -0.3; US -0.72 (1984-2024); -0.49 (1950-2024); +0.13 (1927-2024, 1930s rallies); log returns -0.99 / -0.73 | 0.963 [-0.824..5.19, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Cont (2001 QF) stylised fact 3 (gain/loss asymmetry); Campbell & Hentschel (1992 JFE) |
| Excess kurtosis of monthly returns | 1.00 .. 3.50; US 2.1 (1984-2024); 1.7 (1950-2024); 7.3 (1927-2024) | 9.79 [-0.0413..58.86, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Cont (2001) heavy tails, aggregational Gaussianity (monthly kurtosis << daily) |
| Share of months with negative return, % (supplementary) | 33.00 .. 40.00; US 35.0 (1984-2024); 36.3 (1950-2024); 37.1 (1927-2024) | 33.01 [22.03..38.76, n=12] | ok | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here |
| Autocorrelation of monthly returns, lag 1 | -0.05 .. 0.1; US 0.02 (1984-2024); 0.04 (1950-2024); 0.09 (1927-2024) | 0.626 [0.359..0.854, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Cont (2001) absence of linear autocorrelation; Lo & MacKinlay (1988 RFS) |
| Volatility clustering: AC1 of \|r - mean\| | 0.12 .. 0.3; US 0.22 (1984-2024); 0.16 (1950-2024); 0.26 (1927-2024); daily-RV log AC1 0.65-0.69 | 0.418 [0.143..0.759, n=12] | material | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Cont (2001) volatility clustering; Schwert (1989); Bollerslev (1986) |
| Volatility clustering: mean AC of \|r - mean\|, lags 1-12 (supplementary) | 0.06 .. 0.22; US 0.10 (1984-2024); 0.10 (1950-2024); 0.21 (1927-2024) | 0.117 [0.0554..0.381, n=12] | ok | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Ding, Granger & Engle (1993 JEF) long memory of \|r\| |
| Leverage effect: corr(r_t, \|r_t+1 - mean\|) | -0.35 .. -0.1; US -0.26 (1984-2024); -0.25 (1950-2024); daily RV: corr(r_t, log RV_t+1) -0.32 (1984-2024) | -0.0574 [-0.289..0.161, n=12] | minor | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here and daily factors; Black (1976); Christie (1982 JFE); Cont (2001) leverage effect |
| Return sd in recessions / expansions | 1.40 .. 2.20; US 1.71 (1984-2024); 1.56 (1950-2024); daily-RV ratio 2.07 / 1.54 | 0.904 [0.383..1.67, n=8] | material | 1984-2024 (NBER months) | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here, FRED USREC; Schwert (1989 JF) volatility higher in recessions; Hamilton & Lin (1996 JAE) |
| Countercyclical vol: corr(12m realised vol, 12m change in unemployment) | 0.25 .. 0.55; US 0.39 (1984-2024); 0.50 (1984-2019); 0.41 (1950-2024) | -0.123 [-0.314..0.124, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here, FRED UNRATE; Schwert (1989 JF); Officer (1973 JB); Mele (2007 JFE) asymmetric volatility over the cycle |
| Median real drawdown around recessions, % | -45.00 .. -18.00; US median -27 post-1984 (-20, -35, -51, -20); -20 for 1948-2024 (12 recessions, -8 to -55) | -8.16 [-37.70..-0.0355, n=37] | material | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here monthly real total-return index, window [NBER peak-12m, trough+6m] |
| Real bear markets (>=20% below prior peak) per decade (supplementary) | 0.8 .. 1.60; US 1.22 (1984-2024: 2000-02, 2008-09, 2020, 2022, 1987); 0.93 (1950-2024) | 0.586 [0..1.06, n=12] | minor | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here monthly real total-return index; Pagan & Sossounov (2003 JAE) bull/bear dating |
| P/E on trailing earnings, mean | 15.00 .. 23.00; US mean 22.9, median 20.7 (1984-2024); mean 18.5 (1950-2024); ~25-28 in 2024-25; CAPE median 24.9 / 19.7 | 19.55 [16.60..24.58, n=12] | ok | 1984-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Campbell & Shiller (1998 JPM) |
| Persistence of log D/P: 12-month autocorrelation | 0.75 .. 0.95; US 0.81 (1984-2024, half-life 3.3y); 0.89 (1950-2024, half-life 6.2y); annual AR(1) ~0.94 (Cochrane 2008, 1926-2004) | 0.724 [0.303..0.872, n=12] | minor | 1950-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Cochrane (2008 RFS); Campbell & Shiller (1988 RFS) |
| Payout yield (dividends incl. buybacks), % (supplementary) | 3.00 .. 5.50; US D/P 2.3 (1984-2024), 3.1 (1950-2024); total payout yield ~ D/P + 2pp after 1985 (repurchases) | 4.55 [2.84..6.78, n=12] | ok | 1984-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Boudoukh, Michaely, Richardson & Roberts (2007 JF); Grullon & Michaely (2002 JF) |
| 1-yr return predictability: R2 of fwd 12m real return on log D/P (supplementary) | 0.03 .. 0.15; US 0.15 (1984-2024); 0.06 (1950-2024); slope 0.18 / 0.09 | 0.127 [0.0223..0.316, n=12] | ok | 1950-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Fama & French (1988 JFE); Cochrane (2008); Goyal & Welch (2008 RFS) out-of-sample caveat |
| 5-yr return predictability: R2 of fwd 60m real return on log D/P | 0.15 .. 0.45; US 0.45 (1984-2024); 0.19 (1950-2024); slope 0.68 / 0.37 (CAPE: R2 0.34 / 0.14) | 0.451 [0.145..0.948, n=12] | minor | 1950-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Campbell & Shiller (1988 JF); Fama & French (1988); Cochrane (2011 JF presidential address) |
| 5-yr predictive slope on log D/P (>0: high D/P -> high returns) | 0.25 .. 0.8; US 0.68 (1984-2024); 0.37 (1950-2024) | 1.16 [0.606..1.94, n=12] | material | 1950-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Cochrane (2008, 2011) |
| Excess volatility: sd dlog real price / sd dlog real dividends (annual) | 2.00 .. 3.20; US 2.85 (1950-2024: 16.7% vs 5.9%); 2.39 (1984-2024); 1.62 (1881-2024) | 1.86 [1.32..2.62, n=12] | minor | 1950-2024 | Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here; Shiller (1981 AER); LeRoy & Porter (1981); Campbell & Shiller (1988) |
| Variance ratio of real returns, 60 months (mean reversion < 1) | 0.75 .. 1.05; US 0.84 (1984-2024); 1.03 (1950-2024); 0.81 (1927-2024) | 3.58 [0.786..11.29, n=12] | SEVERE | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here; Poterba & Summers (1988 JFE); Fama & French (1988 JPE); Lo & MacKinlay (1988) |
| Monthly stock-10y Treasury return correlation | -0.2 .. 0.3; US +0.05 (1984-2024); +0.24 (1984-1999); -0.11 (2000-2024); +0.16 (1962-2024) | 0.451 [0.197..0.698, n=12] | material | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here, FRED GS10 (duration 7.5); Baele, Bekaert & Inghelbrecht (2010 RFS); Campbell, Sunderam & Viceira (2017 CFR) |
| corr(past 4q real return, next 4q real GDP growth) | 0.15 .. 0.45; US 0.27 (1984-2024); 0.32 (1950-2024); same-4q 0.41 / 0.29 | 0.0674 [-0.14..0.449, n=12] | minor | 1984-2024 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here, FRED GDPC1; Fama (1990 JF); Schwert (1990 JF); Stock & Watson (2003 JEL) |
| Market value of corporate equity / GDP (supplementary) | 1.30 .. 2.30; US 2.10 (2024); 1.64 (2015-2024 mean); 1.07 (1984-2024 mean); 0.84 (1950-2024) | 2.30 [1.94..2.84, n=12] | minor | 2015-2024 | Fed Z.1 nonfinancial corporate equities at market value (FRED NCBEILQ027S) / FRED GDP |
| Tech equity return sd / market sd (monthly) | 1.30 .. 1.70; US 1.48 (1984-2024); 1.42 (1950-2024); utilities 0.90, nondurables 0.92 | 2.74 [1.23..3.54, n=12] | SEVERE | 1984-2024 | Kenneth French 10 industry portfolios (HiTec, value-weighted), computed here |
| Sector return sd: max / min across sectors | 1.60 .. 2.40; US 2.01 (1984-2024: durables 1.81 vs utilities 0.90 of market); 1.79 (1950-2024) | 8.30 [5.94..11.01, n=12] | SEVERE | 1984-2024 | Kenneth French 10 industry portfolios, computed here |
| Mean corr(sector return, market return), monthly (supplementary) | 0.65 .. 0.9; US 0.77 (1984-2024; range 0.50 utilities .. 0.92 manufacturing); 0.79 (1950-2024); HiTec 0.86 | 0.345 [0.243..0.514, n=12] | SEVERE | 1984-2024 | Kenneth French 10 industry portfolios vs CRSP VW market, computed here |
| Tech boom: real Tech equity run-up, start to peak, % | 250.0 .. 800.0; US +579 (HiTec real total return Dec 1994-Mar 2000); market +209; Nasdaq Composite ~+570 nominal | 191.8 [22.40..365.1, n=13] | minor | 1995-2000 | Kenneth French HiTec portfolio, CPI-U; Pastor & Veronesi (2009 AER); Ofek & Richardson (2003 JF) |
| Tech bust: real Tech drawdown from peak (36m), % | -85.00 .. -60.00; US -81 (HiTec real, Mar 2000-Sep 2002); Nasdaq -78 nominal | -64.10 [-81.14..-32.08, n=11] | ok | 2000-2002 | Kenneth French HiTec portfolio; Pastor & Veronesi (2009) |
| Market real drawdown after a tech boom (36m), % | -55.00 .. -30.00; US -48 (CRSP VW real total return Mar 2000-Sep 2002); S&P 500 -49 nominal price | -28.84 [-42.17..-13.02, n=11] | minor | 2000-2002 | Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here |
| Tech share of equity at boom peak, % (supplementary) | 29.00 .. 37.00; US 32.9 (S&P 500 IT, Mar 2000; Yardeni); 33.6 (Aug 2000, S&P DJI); ~5-10 in early 1990s | 38.79 [27.75..46.20, n=13] | minor | 2000 | Yardeni Research S&P 500 sector weights; S&P Dow Jones Indices "Not your father's tech sector" |

### Diagnosis of the flagged facts

- **Monthly returns three times too smooth and strongly autocorrelated** (volatility, AC1, 60-month variance ratio,
  the 5-year predictive slope, excess volatility).
  - equity.rs value_sectors sets E_s = 12 × EWMA₀.₉(dividends) / (y_eq + ERP + erp_sector − g).
  - y_eq is a 6-month EWMA of a 10y yield that itself moves 0.046pp a month.
  - The ERP is nearly constant: its monthly change has sd 0.011pp, `erp_unemp_sens` is 0 and `erp_extrap` is 0.005.
  - The discount-rate term explains 49% of return variance and has an AC1 of 0.93. Nothing gives equities
    discount-rate news, although discount-rate news dominates US price-dividend variance (Cochrane 2011).
- **Wrong-signed tails** (skew +0.96, excess kurtosis 9.8).
  - The only large moves are +10% to +22% jumps in the first month of a tech boom, when `tech_boom_erp_discount`
    (1.5pp) switches on in one step.
  - Without booms, returns are close to Gaussian (skew -0.08, excess kurtosis 0.1). There is no crash mechanism.
- **No leverage effect, no countercyclical volatility, shallow recession drawdowns** (-8% vs -27%), and too few bear
  markets.
  - The ERP does not respond to the cycle. `erp_unemp_sens` ≥ 0.05 destabilised the economy through the Fund wealth
    consumption channel.
  - In recessions, dividends fall only gradually (EWMA) and the 10y falls, which raises valuations.
- **Stock-bond correlation +0.45** (US +0.05; -0.11 since 2000). The 10y is the only common driver. There is no
  risk-premium or flight-to-quality shock that moves stocks and bonds in opposite directions.
- **Equities do not anticipate GDP** (0.07 vs 0.27). The valuation looks back. The Smets-Wouters risk-premium shock
  moves households' and firms' real rates but not the ERP.
- **Sector returns dominated by payout noise** (Tech/market sd ratio, max/min sector sd, sector-market correlation).
  - Sectors are valued on their own realised payouts. Payouts are lumpy: firms.rs pays only when profit > 0 and
    deposits exceed 2 months of costs.
  - Payouts are smoothed only by an EWMA of 0.9, and the extrapolative growth term (up to ±1.5pp) amplifies them.
  - Small sectors have few firms at 10-20k households: Food is 7.4× and Construction 8.0× as volatile as the market.
- **Tech booms too mild across the market.** The run-up is +192% (US +579%), and the market drawdown after a boom is
  -29% (US -48%).
  - The re-rating comes from the one-step discount plus slow news.
  - Realised Tech dividends do not accelerate, because cost-plus pricing passes productivity into prices
    (SECTORS.md).
- **Real return 5.3%/yr** (US 8.5% for 1984-2024). P/E is stationary, so the return is about D/P plus real growth. The
  US post-1984 sample also gained from P/E rising from about 10 to 25.

## Exogenous and price processes (`scripts/facts_shocks.py`)

US targets use the same estimators over 20-year windows stepping 5 years, so they compare directly with a 20-year run. The range is the min..max across windows. The model value is the mean over the main runs, with the seed range and the mean over 240-month windows of the long runs.

25 facts: ok 5, minor 2, material 15, severe 3

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| sd monthly dln real oil price, % | 9.20 (8.00 .. 10.50) | 5.91 [5.72..6.53]; long runs 5.92 | material | WTI monthly average (FRED MCOILWTICO) / CPI-U, 1986:01-2026:08, 20y windows; 1986-2019 8.5 | EIA/FRED MCOILWTICO, BLS CPIAUCSL (computed); Baumeister & Kilian (2016) JEP 30(1); Alquist, Kilian & Vigfusson (2013) Handbook of Econ. Forecasting 2A |
| excess kurtosis of monthly dln real oil | 4.60 (1.20 .. 8.60) | 0.114 [-0.131..0.26]; long runs -0.00551 | material | WTI 1986-2026 20y windows (full sample 7.3; 1986-2019 2.5; spot 1974-2026 8.8) | FRED MCOILWTICO, WTISPLC (computed); jumps 1986, 1990, 2008, 2014-16, 2020, 2022 (Baumeister & Kilian 2016; Hamilton 2009 Brookings PEA) |
| oil variance ratio VR(12) = var(12m dln)/(12 var(1m dln)) | 1.10 (0.86 .. 1.42) | 0.774 [0.49..1.16]; long runs 0.754 | minor | WTI real 1986-2026 20y windows (1986-2019 1.05); 1 = random walk | FRED MCOILWTICO (computed); real oil price close to a random walk / no-change forecast hard to beat (Hamilton 2009; Alquist, Kilian & Vigfusson 2013) |
| share of months with \|12m dln real oil\| > 0.5 | 0.14 (0.05 .. 0.2) | 0.00694 [0..0.0278]; long runs 0.00694 | material | WTI real 1986-2026 20y windows (full 0.12); episodes 1986, 1990-91, 1998-99, 2008-09, 2014-16, 2020-21, 2022 | FRED MCOILWTICO (computed); Baumeister & Kilian (2016); Kilian (2009) AER 99(3) |
| sd of ln real oil level within a 20-year window | 0.4 (0.27 .. 0.52) | 0.216 [0.168..0.316]; long runs 0.211 | material | WTI real 1986-2026 20y windows (full 0.45): $20-$140 swings | FRED MCOILWTICO / CPIAUCSL (computed); Hamilton (2009) |
| mean drift of ln real oil, %/yr | 0.4 (-1.00 .. 2.00) | -2.51 [-5.03..0.559]; long runs -1.64 | material | 1986-2026 0.46 (monthly avg), 0.45 (spot); 1974-2026 0.29; single 20y windows -3.1..+3.9, the model mean over 8 runs should be near the long-run drift | FRED MCOILWTICO, WTISPLC / CPIAUCSL (computed); no deterministic trend in the real oil price (Hamilton 2009; Baumeister & Kilian 2016) |
| gasoline pass-through, $/gal per $/bbl (dgas on doil, lags 0+1) | 0.0287 (0.0235 .. 0.029) | 0.0381 [0.0379..0.0382]; long runs 0.0381 | material | BLS avg retail regular (APU000074714) on WTI, monthly differences, 20y windows 1990-2026 0.028-0.029; 1990-2004 0.0235 | FRED APU000074714, MCOILWTICO (computed); EIA Today in Energy 6850 (2012): $1/bbl = 2.4 c/gal, half within 2 weeks, 80% within 4 weeks; Borenstein, Cameron & Gilbert (1997) QJE 112(1) |
| gasoline elasticity to oil (dln gas on dln oil, lags 0+1) | 0.6 (0.44 .. 0.72) | 0.849 [0.837..0.859]; long runs 0.823 | material | APU000074714 on WTI, 20y windows 1990-2026 0.57-0.72; 2010-2026 0.49; 1990-2004 0.44 | FRED APU000074714, MCOILWTICO (computed); EIA (2019, Today in Energy 41453): crude ≈53% of the retail price |
| sd of 12m energy CPI inflation, pp | 11.40 (7.90 .. 12.90) | 12.81 [9.64..15.47]; long runs 11.80 | ok | CPI energy (CPIENGSL) 1986-2026 20y windows (1986-2019 10.2) | BLS via FRED CPIENGSL (computed) |
| energy CPI elasticity to oil (dln on dln WTI, lags 0+1) | 0.28 (0.22 .. 0.33) | 0.68 [0.661..0.697]; long runs 0.638 | SEVERE | CPIENGSL on MCOILWTICO 1986-2026 20y windows (full 0.25) | FRED CPIENGSL, MCOILWTICO (computed); energy CPI = motor fuel ≈half + electricity and utility gas (BLS relative importance); Kilian (2008) JEL 46(4) |
| sd of 12m food CPI inflation, pp | 1.50 (1.20 .. 2.30) | 1.60 [1.31..1.85]; long runs 1.89 | ok | CPI food (CPIUFDSL) 1986-2026 20y windows (1986-2019 1.45; 1957-2026 3.0) | BLS via FRED CPIUFDSL (computed) |
| sd of 12m-ahead household inflation forecast error, pp | 1.40 (0.93 .. 1.54) | 1.04 [0.737..1.30]; long runs 0.906 | ok | realised CPI-U 12m change from t to t+12 minus Michigan 1y expectation (MICH) at t, 1986-2026 20y windows; 1986-2019 1.39 (mean error -0.46) | U. Michigan Surveys of Consumers via FRED MICH, CPIAUCSL (computed); Souleles (2004) JMCB 36(1); Ang, Bekaert & Wei (2007) JME 54(4) |
| corr(12m dln gasoline, 12m change in household expected inflation) | 0.68 (0.57 .. 0.78) | 0.561 [0.493..0.728]; long runs 0.541 | minor | APU000074714 and MICH 1986-2026 20y windows (full 0.56; slope 0.25pp per 10% gas) | FRED APU000074714, MICH (computed); Coibion & Gorodnichenko (2015) AEJ:Macro 7(1); Binder (2018) J. Macroecon. 58; Kilian & Zhou (2022) JAE 37(5) |
| sd monthly dln dollar index, % | 1.45 (1.20 .. 1.80) | 0.19 [0.119..0.234]; long runs 0.208 | SEVERE | real broad BIS (RBUSBIS) 1994-2026 1.22; Fed real major currencies (TWEXMPA) 1973-2019 1.70; nominal broad 2006-2026 1.27 (annualised 4.2-5.8%) | BIS/Fed via FRED RBUSBIS, TWEXMPA, TWEXBGSMTH (computed); Meese & Rogoff (1983) JIE 14; Engel & West (2005) JPE 113(3) |
| sd of ln dollar level within a 20-year window | 0.11 (0.087 .. 0.132) | 0.0288 [0.0168..0.0385]; long runs 0.0336 | material | TWEXMPA 1973-2019 20y windows 0.098-0.132; RBUSBIS 1994-2026 0.087 (swings 1980-85 +50%, 1995-2002 +35%, 2011-22 +30%) | FRED TWEXMPA, RBUSBIS (computed); PPP half-life 3-5 years (Rogoff 1996 JEL 34(2)) |
| corr(monthly dln dollar, monthly change in 2y yield) | 0.18 (0.09 .. 0.33) | 0.597 [0.527..0.648]; long runs 0.676 | material | TWEXMPA vs GS2 1976-2019 0.18-0.33; RBUSBIS 1994-2026 0.09-0.14 | FRED GS2, TWEXMPA, RBUSBIS (computed); exchange-rate disconnect (Meese & Rogoff 1983; Itskhoki & Mukhin 2021 JPE 129(8)) |
| corr(12m real HPI growth, same 12 months earlier) - momentum | 0.75 (0.6 .. 0.8) | 0.38 [0.187..0.509]; long runs 0.365 | material | S&P CoreLogic Case-Shiller national (CSUSHPISA)/CPI 1987-2026 20y windows 0.70-0.80; annual-average growth AR1 0.72; FHFA 1975-2026 0.62 | FRED CSUSHPISA, USSTHPI (computed); Case & Shiller (1989) AER 79(1); Cutler, Poterba & Summers (1991) AER P&P; Glaeser, Gyourko, Morales & Nathanson (2014) JUE 81 |
| sd of 12m real HPI growth, % | 5.90 (4.20 .. 6.70) | 1.48 [1.18..1.86]; long runs 2.59 | material | Case-Shiller national real 1987-2026 20y windows (full 5.4); FHFA all-transactions 1975-2026 4.0 | FRED CSUSHPISA, USSTHPI, CPIAUCSL (computed); Shiller (2015) Irrational Exuberance, 3rd ed., long-run real HPI |
| sd of linearly detrended ln real HPI within 20 years (long cycles) | 0.126 (0.116 .. 0.138) | 0.0243 [0.0146..0.0379]; long runs 0.0401 | SEVERE | Case-Shiller national real 1987-2026 20y windows; FHFA 1975-2026 0.03-0.12 (1989-97 -15%, 2006-12 -35% real) | FRED CSUSHPISA (computed); Glaeser et al. (2014): mean reversion over 5-year horizons |
| sd of linearly detrended ln(price/rent) within 20 years | 0.116 (0.098 .. 0.132) | 0.032 [0.0164..0.0544]; long runs 0.0465 | material | Case-Shiller national / CPI rent of primary residence (CUSR0000SEHA) 1987-2026 20y windows | FRED CSUSHPISA, CUSR0000SEHA (computed); Davis, Lehnert & Martin (2008) RIW 54(2); Campbell, Davis, Gallin & Martin (2009) JUE 66(2) |
| price-to-income (median owner-occupied value / median household income) | 3.90 (3.50 .. 5.00) | 4.34 [4.22..4.79]; long runs 3.99 | ok | ACS 2019-23: $303,400 / $78,538 = 3.9 (stock values); JCHS sales-price basis 3.2 (1990s avg), 4.1 (2019), 5.0 (2024); MSPUS new homes / median income 1984-2025 3.6-5.8 | Census ACS 5-year 2019-2023 (B25077, B19013); Harvard JCHS State of the Nation's Housing 2025; FRED MSPUS, MEHOINUSA646N |
| corr(HP home sales_t, HP real GDP_t+2q), quarterly - sales lead | 0.6 (0.4 .. 0.69) | 0.162 [-0.00341..0.344]; long runs 0.196 | material | new single-family home sales (HSN1F) vs GDPC1, HP 1600: 1984-2019 0.60, 1963-2019 0.69, 1984-2026 0.40; peak at a 2-3 quarter lead; contemporaneous 0.2-0.4 | Census/HUD via FRED HSN1F, BEA GDPC1 (computed); Leamer (2007) Jackson Hole "Housing IS the business cycle"; Leamer (2015) JMCB 47(S1) |
| corr(12m dln home sales_t, 12m real HPI growth_t+12) - volume leads price | 0.63 (0.3 .. 0.79) | 0.122 [0.00701..0.195]; long runs 0.18 | material | HSN1F vs Case-Shiller real 1987-2026 (20y windows 0.30-0.79); contemporaneous 0.44 | FRED HSN1F, CSUSHPISA (computed); Stein (1995) QJE 110(2); Genesove & Mayer (2001) QJE 116(4); Clayton, Miller & Peng (2010) JREFE 40(1) |
| sd quarterly labour productivity growth (GDP/employed), % annualised | 2.00 (1.80 .. 2.80) | 2.29 [1.88..2.57]; long runs 2.66 | ok | GDPC1 / CPS employment (CE16OV) 1984-2019 2.0 (payroll 1.8); Fernald utilisation-adjusted TFP 1984-2019 2.75 (1948-2025 3.3), AR1 ≈ 0 | FRED GDPC1, CE16OV, PAYEMS (computed); Fernald (2014) FRBSF WP 2012-19, data vintage 2026-10-01 |
| corr(HP labour productivity, HP real GDP), quarterly | 0.45 (0.23 .. 0.5) | 0.806 [0.771..0.857]; long runs 0.816 | material | GDP/CPS employment 1984-2019 0.50 (20y windows 0.44-0.73), GDP/payroll 0.23; pre-1984 0.83 | FRED GDPC1, CE16OV, PAYEMS (computed); Gali & van Rens (2021) EJ 131(633): the vanishing procyclicality of labour productivity |

### Diagnosis of the flagged facts

- **Oil too calm, Gaussian and mean-reverting** (volatility, kurtosis, VR(12), large-episode share, 20-year level sd).
  - exog.rs step draws oil as a Gaussian AR(1), x = 0.95x + 0.06ε, with a half-life of 13.5 months.
  - There are no jumps, no t-tails and no volatility clustering: the AC1 of |Δ| is -0.04 (US 0.36).
- **Real oil drifts down 2.5%/yr.** init.rs sets `exog.oil_base` to `cfg.oil_base` ($65 nominal) and never indexes
  it, so energy inflation runs about 2pp/yr below core.
- **Energy CPI elasticity to oil 0.68** (US 0.28), so headline CPI responds about twice as much as in the data.
  - In firms.rs, the Energy price is 0.5 × the oil ratio (ENERGY_OIL_WEIGHT) + 0.5 × uc/uc0, and uc itself contains
    crude_intensity 0.35 × the oil ratio.
  - One Energy good stands in for motor fuel and for electricity/utility gas, which have little oil pass-through.
- **Energy CPI volatility looks ok only by coincidence.** Oil volatility is too low (×0.64) and the energy beta too
  high (×2.4). With realistic oil (`oil_ar` 0.985, `oil_sigma` 0.085), energy CPI volatility rises to 19pp, a severe
  gap.
- **Gasoline pass-through and elasticity too high** (0.038 $/gal per $/bbl vs 0.029; 0.85 vs 0.60).
  - common.rs gasoline_price = 1.25·P_Energy + oil/42 counts crude twice.
  - There is no fixed per-gallon tax.
  - The level is $2.80 at $65 WTI (US $3.26 in 2025).
- **Expectations react to gasoline at half the US slope** (0.135 vs 0.25pp per 10%). consumption.rs EXP_INFL_SALIENCE
  acts on the Energy sector price times the CPI weight, not on the pump price.
- **Dollar six to eight times too smooth, with no disconnect from rates** (volatility, level sd, correlation with Δ2y).
  fx.rs exchange_rate is deterministic partial adjustment (`fx_adjust` 0.3) toward PPP plus 3 × the real 2y
  differential. There is no exogenous UIP or risk-premium shock.
- **House prices: no momentum or long cycles** (momentum, volatility, 20-year cycle, price/rent cycle, sales leading
  GDP and prices).
  - housing.rs update_indices sets HPI growth to π_e plus bounded supply-imbalance and affordability terms.
  - The strong affordability anchor makes prices mean-revert within 1-2 years.
  - Extrapolation is weak: EHPA_TRAILING_WEIGHT is 0.25, not the 0.7 the docs state.
  - The index is a rule, not built from transaction prices, so volume does not lead prices.
- **Productivity too procyclical** (0.81 vs 0.45 post-1984). Demand shocks dominate, labour is hoarded, and the
  common TFP random walk is small (about 1.0% vs 2.75% annualised quarterly sd in Fernald's series). The model shows
  the pre-1984 pattern.

## Distribution and household finance (`scripts/facts_distribution.py`)

Levels skip the first reported year. Trends are slopes of annual means over the whole run. Money is in calibration dollars (about 2026 $). Ratios to GDP use BEA-equivalent GDP (model GDP ≈ 83% of BEA).

32 facts: ok 9, minor 4, material 7, severe 3, n/a 9

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| Wealth Gini (net worth, households) | 0.82 .. 0.86 | 0.733 (seed sd 0.00479) | SEVERE | SCF 2019 / 2022 / 2025 | Federal Reserve SCF summary extract public data (scfp2019/2022/2025excel.zip), weighted Gini of NETWORTH computed here: 0.852 (2019), 0.830 (2022), 0.824 (2025). Kuhn, Schularick & Steins (2020, JPE) SCF+: 0.80-0.86 since 1989. |
| Top-10% wealth share, % | 67.00 .. 76.50 | 62.71 (seed sd 0.564) | material | SCF 2019-2025; DFA 1989-2026 | SCF extract (computed): 76.5 (2019), 73.4 (2022), 72.1 (2025). Fed Distributional Financial Accounts (dfa-networth-levels, includes DB pensions and durables, so lower): 69.9 (2019Q4), 66.2 (2022Q4), 68.9 (2026Q2); 60.8 in 1989Q3. |
| Top-10% wealth share trend, pp per decade | 0 .. 3.50 | -2.25 (seed sd 0.993) | material | DFA 1989Q3-2026Q2 | Fed DFA top-10% share 60.8% (1989Q3) -> 68.9% (2026Q2); OLS on Q4 values +0.29 pp/yr. SCF: 67% (1989) -> 72-76% (2019-2025) (Bricker et al. 2020, Fed Bulletin). Rising, not falling. |
| Median net worth / median household income | 2.00 .. 2.80 | 3.23 (seed sd 0.106) | material | SCF 2019 / 2022 / 2025 | SCF extract (computed, medians of NETWORTH and INCOME): 2.06 (2019), 2.74 (2022), 2.63 (2025). Published 2022: $192,900 / $70,260 (Aladangady et al. 2023, Fed Bulletin). |
| Median net worth, real (calibration $ ~2026), $k | 155.0 .. 225.0 | 282.4 (seed sd 13.70) | material | SCF 2019-2025 in 2025 $ | SCF extract (2025 dollars): $154.5k (2019), $210.9k (2022), $216.1k (2025); ~$222k in 2026 $. |
| Income Gini (pre-tax money income incl. transfers) | 0.47 .. 0.5 | 0.45 (seed sd 0.00396) | minor | Census CPS ASEC 2015-2025 | Census household money-income Gini (FRED GINIALLRH): 0.484 (2019), 0.488 (2022), 0.489 (2024), 0.490 (2025); Census P60-286. Model income = wage+benefits+pension+transfers+rent+interest+dividends+business income, i.e. the Census money-income concept. |
| Income Gini trend, per decade | 0 .. 0.025 | -0.0252 (seed sd 0.00339) | material | Census 1984-2025 | FRED GINIALLRH OLS slope 1984-2025: +0.018 per decade (0.415 -> 0.490; includes the 2013 CPS redesign step of ~+0.01). |
| Household earnings mean/median (earner households) | 1.30 .. 1.55 | 1.14 (seed sd 0.0028) | material | SCF 2019-2025; CPS ASEC | SCF extract WAGEINC>0, head working 25-64 (computed): 1.46 (2019), 1.54 (2022), 1.44 (2025); all earner households 1.51-1.64. CPS ASEC household earnings (top-coded) ~1.3. SSA individual net compensation mean/median ~1.4-1.5. |
| Earnings 45-54 / 25-34 (age profile peak) | 1.15 .. 1.30 | 1.25 (seed sd 0.00556) | ok | BLS CPS 2026Q2 | BLS Usual Weekly Earnings Table 3 (median, full-time W&S, 2026Q2): 16-24 $806, 25-34 $1,160, 35-44 $1,436, 45-54 $1,421, 55-64 $1,367, 65+ $1,233 -> 45-54/25-34 = 1.23, 35-44/25-34 = 1.24 (peak 35-54; Guvenen et al. 2021 Econometrica: cross-section peaks at 45-55). |
| Earnings 18-24 / 25-34 | 0.62 .. 0.78 | 0.79 (seed sd 0.00685) | minor | BLS CPS 2026Q2 | BLS Usual Weekly Earnings Table 3: 16-24 / 25-34 = 0.695. |
| Earnings 65+ / 45-54 | 0.8 .. 0.92 | 0.83 (seed sd 0.0168) | ok | BLS CPS 2026Q2 | BLS Usual Weekly Earnings Table 3: 65+ / 45-54 = 0.868; 55-64 / 45-54 = 0.962. |
| Homeownership, head <25, % | 20.00 .. 26.00 | 6.04 (seed sd 0.506) | SEVERE | Census HVS 2015-2024 | Census CPS/HVS annual Table 17 (ann25t_17.xlsx): <25 21.8 (2015), 23.2 (2019), 25.4 (2022), 23.0 (2024). |
| Homeownership, head 25-34, % | 38.00 .. 43.00 | 38.85 (seed sd 0.31) | ok | Census HVS 2019-2024 | HVS Table 17, 2024: 25-29 33.5, 30-34 47.4 (mean 40.5); 2019: 32.9 / 48.0; 2022: 35.6 / 48.8. |
| Homeownership, head 45-54, % | 69.50 .. 71.50 | 71.49 (seed sd 0.521) | ok | Census HVS 2019-2026 | HVS Table 17/19: 70.1 (2019), 70.5 (2022-23), 70.6 (2024). |
| Homeownership, head 65+, % | 78.00 .. 80.00 | 72.11 (seed sd 0.29) | material | Census HVS 2015-2026 | HVS Table 17/19: 78.9 (2015), 78.6 (2019), 79.1 (2022), 79.0 (2024), 78.4-78.6 (2026H1). |
| Unemployment 18-24 / 25-54 (level) | 1.80 .. 2.40 | 2.66 (seed sd 0.0547) | minor | BLS CPS 1984-2019; 2025 | FRED LNS14000036 / LNS14000060 (20-24 vs 25-54) mean ratio 1.99 (1984-2019), 2.04 (1955-2019); BLS CPS Table 3 2025: 8.3 / 3.6 = 2.3. Model: heads aged 18-24 vs mean of the 25-34/35-44/45-54 rates. |
| Unemployment cycle beta, 18-24 on 25-54 | 1.25 .. 1.70 | 1.65 (seed sd 0.145) | ok | BLS CPS 1984-2019 (HP 1600, quarterly) | FRED LNS14000036 (20-24) on LNS14000060 (25-54), HP-filtered quarterly cycles: slope 1.36 (1984-2019), 1.54 (1955-2019), 1.64 (1984-2025); sd ratio 1.45 / 1.62. Jaimovich & Siu (2009): the young carry a disproportionate share of cyclical volatility. |
| Mortgage debt / GDP (BEA-equivalent) | 0.43 .. 0.53 | 0.506 (seed sd 0.0158) | ok | Fed Z.1 / BEA 2015-2025 | FRED HHMSDODNS / GDP: mean 0.48 (2015-2025, range 0.44-0.53), 0.44 (2024-26), 0.49 mean 1984-2025 (peak 0.73 in 2007; 0.27 mean 1955-83). |
| Mortgage debt per owner / median owner home value | 0.4 .. 0.52 | 0.381 (seed sd 0.00433) | minor | SCF 2019-2025 | SCF extract (computed: mean MRTHEL over owners / median HOUSES of owners): 0.52 (2019), 0.42 (2022), 0.41 (2025). Aggregate debt/value among owners 0.29-0.34; Z.1 owners equity share 71.9% (2026Q2), 62% mean 1984-2025 (FRED HOEREPHRE). |
| Foreclosures, thousands per year | 120.0 .. 350.0 | 161.3 (seed sd 12.60) | ok | NY Fed CCP 2018-2025 (non-crisis) | NY Fed Household Debt & Credit Report: new foreclosure notations ~53-58k per quarter in 2025 (~0.22M/yr), ~70-90k per quarter in 2018-19 (~0.3M/yr); peak ~0.55M per quarter in 2009. ATTOM foreclosure starts ~0.27M (2023). |
| corr(unemployment, foreclosures), quarterly HP cycles | 0.4 .. 0.9 | 0.47 (seed sd 0.0895) | ok | US 1991-2019 (proxy: mortgage delinquency) | FRED DRSFRMACBS (single-family mortgage delinquency, commercial banks) vs UNRATE, HP-filtered quarterly 1991-2019: corr +0.70 same quarter, +0.56 one quarter later. Mian & Sufi (2009): defaults concentrate where unemployment and house prices move together. |
| Consumption exposure: beta(Q5) / mean beta(Q1-Q3) | 1.50 .. 5.00 | 2.05 (seed sd 0.272) | ok | CEX 1982-2004 | Parker & Vissing-Jorgensen (2009, AER P&P 99(2)): top-10% (consumption-ranked) exposure to aggregate consumption growth ~5x the bottom 80%; their income exposure is also higher since the early 1980s. Income-ranked quintiles dilute this, so 1.5-5. |
| Consumption growth gap Q5 - Q1, pp/yr | 0 .. 1.00 | -1.77 (seed sd 0.121) | SEVERE | CEX 1980-2007; CPS 1984-2025 | Aguiar & Bils (2015, AER): consumption inequality between high- and low-income households tracked the rise in income inequality 1980-2007; Meyer & Sullivan (2023, JPE): smaller but non-negative rise. Census income Gini rose +0.018/decade. A falling top/bottom ratio is the wrong sign. |
| Top-1% wealth share, % | 31.00 .. 37.00 | n/a | n/a | SCF 2019-2025; DFA 2026Q2 | SCF extract (computed): 37.2 (2019), 35.1 (2022), 31.3 (2025); DFA 30.4 (2019Q4), 32.5 (2026Q2); Saez & Zucman (2016, QJE) ~37-42% (tax-capitalised). |
| Bottom-50% wealth share, %; share with NW<=0 | 1.50 .. 2.60 | n/a | n/a | SCF 2019-2025; DFA | SCF extract (computed): 1.5 (2019), 2.2 (2022), 2.1 (2025); DFA 2.3-2.6 (2022-26). Households with NW<=0: 10.8% (2019), 7.9% (2022), 8.6% (2025). |
| Median NW by age / overall median (<35,35-44,45-54,55-64,65-74,75+) | [0.15, 0.72, 1.18, 1.90, 2.00, 2.34] | n/a | n/a | SCF 2025 (2019: 0.11,0.75,1.39,1.75,2.19,2.09) | SCF extract (computed): medians $32.8k, $156k, $255k, $411k, $432k, $505k vs $216k overall. Elderly decumulate slowly (De Nardi, French & Jones 2010, JPE). |
| Hand-to-mouth share of households, % | 25.00 .. 40.00 | n/a | n/a | SCF 1989-2010; SCF 2025 | Kaplan, Violante & Weidner (2014, BPEA): ~31% HtM, about two-thirds of them wealthy HtM (own illiquid assets). SCF 2025 extract, liquid assets net of card debt <= half monthly income: 40% (20% wealthy HtM). |
| Disposable-income Gini (after taxes and transfers) | 0.39 .. 0.44 | n/a | n/a | CBO 2022; OECD IDD 2021-22 | CBO, The Distribution of Household Income 2022: Gini after transfers and federal taxes 0.434 (before 0.51); OECD IDD equivalised disposable ~0.39. |
| Consumer credit / GDP (revolving / GDP) | 0.16 .. 0.19 | n/a | n/a | Fed G.19 / BEA 2015-2025 | FRED TOTALSL / GDP: mean 0.183 (2015-25), 0.158 (2026Q2); REVOLSL / GDP 0.047 (2015-25), 0.041 (2026Q2). The model has only an overdraft line (negative deposits), not reported. |
| Mortgage delinquency rate, % (normal times; peak 11.5) | 1.50 .. 4.00 | n/a | n/a | Fed (commercial banks) 1991-2026 | FRED DRSFRMACBS: mean 3.7% 1991-2025, trough 1.86% (2026Q2), peak 11.5% (2010Q1); NY Fed CCP 90+ days ~0.5-1% (2019-25). |
| Personal bankruptcy filings, % of households per year | 0.35 .. 0.6 | n/a | n/a | US Courts 2015-2025 | US Courts: 549,577 non-business filings in CY2025 (0.41% of ~133M households), 494,201 (2024), ~750k (2019); 1.5M in 2010; 2.0M in 2005 (pre-BAPCPA spike). SCF: 1.3-2.0% filed in last 5 years. |
| Earnings risk: left skewness countercyclical, dispersion acyclical | see source | n/a | n/a | SSA panel 1978-2010 | Guvenen, Ozkan & Song (2014, JPE): the variance of idiosyncratic earnings shocks is roughly acyclical while left skewness rises in recessions (upper tail thins, lower tail thickens); Guvenen et al. (2021, Econometrica): 1-yr log earnings changes are negatively skewed and leptokurtic (kurtosis >> 3). |

### Diagnosis of the flagged facts

- **Wealth concentration too low and falling** (Gini, top-10 share and its trend).
  - init.rs calibrates initial wealth to the DFA concept (TARGET_TOP10_NW_SHARE 0.67), while the model's net worth
    is the SCF concept (no DB pensions or durables).
  - The distribution then decays, for several reasons:
    - Retirees annuitise all liquid and Fund wealth over their remaining life expectancy, with no bequest motive.
    - Estates go to a random unrelated heir aged 25-60 (demography.rs pick_heir).
    - Everyone holds the same Fund and keeps at least 40% of financial wealth in deposits (LIQUID_SHARE).
    - Saving out of labour income is uniform (mpc_income 0.97).
  - Halving the wealth MPCs in a config test left the end-of-run top-10 share at 61.5%.
- **Income and earnings compress** (income Gini trend; earnings mean/median 1.14 vs 1.30-1.55).
  - Entrants draw skill with sigma 0.35 (demography.rs fill_entrant), the initial population with SKILL_SIGMA 0.45
    (init.rs).
  - There is no Pareto top tail in earnings.
- **Middle-class wealth too high** (median NW / income 3.2 vs 2.0-2.8; real median $282k vs about $222k).
  - The 72-month burn-in freezes demography while saving continues. A run without burn-in dips to 2.6.
  - The precautionary deposit target is 2-8 months of income × 1.5.
  - There is no non-mortgage household debt.
- **Homeownership age profile.**
  - Under-25s are at 6% (US 23%). Entrants arrive with about $3k and need full down payments, and there is no FHA or
    parental-help path.
  - 65+ is at 72% and falling (US 79%). Realised first-buy rates run below FIRST_BUY_BANDS, and the exits in
    OWNER_EXIT_BANDS keep the profile flat from 45.
- **Top-quintile consumption grows 1.8pp/yr slower than the bottom.** consumption.rs WEALTH_KINK ($1M) and
  CAPITAL_INCOME_KINK ($20k/month) are nominal and never indexed, so the low-MPC segment spreads down the
  distribution as nominal values grow.
- **Youth unemployment 2.7× prime-age** (US 2.0×). labor.rs ranks applicants strictly by human capital, and entrants
  arrive unemployed.

## Firm dynamics and labour flows (`scripts/facts_firms.py`)

BDS rates are annual and firm-level. JOLTS rates are monthly. Exits by firm age come from the `exits` binary on seed 1. Model employment counts household heads, so persons = heads × 1.75.

37 facts: ok 8, minor 9, material 10, severe 3, n/a 7

| Fact | US target | Model | Gap | Sample | Source |
|---|---|---|---|---|---|
| firm exit (death) rate, %/yr | 8.8 (2010s 7.8; 1979-2019 9.0) [7.50 .. 10.50] | 14.14 (sd 2.67) | material | BDS 1985-2019 | Census BDS 2022 release: firmdeath_firms(t)/firms(t-1); author calc |
| firm entry (startup) rate, %/yr | 9.7 (2010s 8.2; 1980s 12) [8.00 .. 11.50] | 14.58 (sd 2.62) | material | BDS 1985-2019 | Census BDS 2022: age-0 firms / firms; author calc |
| exit-rate drift: last 5y - first 5y of run, pp | stationary/declining: -1.9pp (1985-89 to 2000-04) [-3.00 .. 1.00] | 8.33 (sd 3.31) | SEVERE | BDS 1985-2004 | Census BDS 2022 (10.3% in 1985-89, 8.4% 2000-04, 7.6% 2015-19); Decker et al. (2014 JEP) |
| corr(entry rate, GDP), annual HP(6.25) | +0.50 [0.25 .. 0.75] | -0.21 (sd 0.26) | material | BDS 1985-2019 | Census BDS 2022 vs FRED GDPC1 Q1 levels; author calc. Procyclical entry: Lee & Mukoyama (2015 RED) |
| corr(exit rate, GDP), annual HP(6.25) | -0.36 [-0.6 .. -0.15] | -0.32 (sd 0.12) | ok | BDS 1985-2019 | Census BDS 2022 vs FRED GDPC1; author calc (exit rises with unemployment: corr(exit, du) +0.31) |
| share of firm exits aged <5 years, % | 48 (BDS deaths at ages 1-5) [40.00 .. 60.00] | 90.0 (seed 1; by age <3,3-6,6-12,12-24,24-60,60+ m: 0/1/9/69/11/10%) | material | BDS 2010-2019 | Census BDS 2022 by firm age; author calc. Death rate age 1: 21.7%, ages 6-10: 8.1%, 26+: 4.9% |
| entrants exiting within 2 years, % | 34 (44% survive 4 years) [25.00 .. 40.00] | 76.4 (seed 1; exits aged <24 months / entrants) | SEVERE | BLS BED 1998 cohort | Knaup (2005 MLR 128(9)); BLS BED establishment age tables (5-yr survival ~50%) |
| employees per exiting firm | 6.2 (2010s 5.8) [4.50 .. 8.00] | 3.8 (seed 1; 2.2 heads x 1.75) | minor | BDS 1985-2019 | Census BDS 2022: firmdeath_emp / firmdeath_firms; author calc |
| employment in exiting firms, % of employment/yr | 2.5 (2010s 1.9) [1.80 .. 3.00] | 2.01 (exit rate x exit size / mean size) | ok | BDS 1985-2019 | Census BDS 2022: firmdeath_emp(t) / emp(t-1); author calc |
| mean employer firm size, employees | 24.7 (1990 21.0) [21.00 .. 26.00] | 26.92 (sd 0.12) | minor | BDS 2019 | Census BDS 2022: emp/firms |
| Zipf tail exponent of firm size | 1.06 (BDS 2019 counting-fn slope, 20-5000 emp: 0.99) [0.9 .. 1.10] | n/a: no firm-size output; t=0 Pareto(1.1) truncated at 65x the smallest weight | n/a | Census 1997; BDS 2019 | Axtell (2001 Science 293:1818); author calc from BDS size classes |
| employment share of firms with 500+ employees, % | 52.9 (1990 46.1) [46.00 .. 56.00] | n/a: no firm-size output; t=0 construction: largest firm ~400 employees, 500+ share 0% | n/a | BDS 2019 | Census BDS 2022 by firm size; SUSB similar |
| gross job creation / destruction, %/yr | JC 15.4, JD 14.0 (2010s 13.0 / 11.5) [13.00 .. 16.00] | n/a: no firm-level employment changes in the JSON | n/a | BDS 1985-2019 | Census BDS 2022; Davis, Haltiwanger & Schuh (1996) |
| startups share of gross job creation, % | 16 (firms age 0-5: 32) [14.00 .. 20.00] | n/a: no employment by firm age in the JSON | n/a | BDS 1985-2019 | Census BDS 2022; Haltiwanger, Jarmin & Miranda (2013 REStat) |
| aggregate markup over unit cost, % | ~10-25 cost-weighted; 61 sales-weighted P/MC (2016) [10.00 .. 30.00] | 14.86 (sd 0.37) | ok | Compustat 1980-2016 | De Loecker, Eeckhout & Unger (2020 QJE): 1.21 (1980) -> 1.61 (2016) sales-weighted; cost-weighted about half the rise (Edmond, Midrigan & Xu 2023 JPE). Model markup is over average (normal) cost |
| within-industry markup dispersion (P90/P50) | P50 ~flat, P90 1.5 (1980) -> 2.5 (2016) [1.25 .. 2.10] | n/a: only the output-weighted mean markup is reported | n/a | Compustat 1980-2016 | De Loecker, Eeckhout & Unger (2020); Eeckhout (2021) The Profit Paradox |
| after-tax profits, % of GDP | 7.6 (1967-2019 7.1; 2010s ~9) [6.00 .. 10.00] | 11.97 (sd 0.38) | minor | NIPA 1985-2019 | FRED CPATAX/GDP (corporate profits after tax with IVA & CCAdj); author calc |
| labour share lags output: corr(ls(t+4q), GDP(t)) | +0.43 (contemporaneous +0.06) [0.2 .. 0.7] | 0.49 (sd 0.14) | ok | NFB 1985-2019 | FRED PRS85006173 vs GDPC1, HP(1600); author calc (1967-2019: +0.53). Rios-Rull & Santaeulalia-Llopis (2010 JME) |
| proprietors' income, % of GDP | 6.9 [6.00 .. 8.00] | 1.83 (sd 0.26) | material | NIPA 1985-2019 | FRED PROPINC/GDP; author calc (NIPA includes the owner's labour income) |
| firm pay premia: var(firm FE) share of var(log earnings), % | 8.7-11.9 (between-firm share 30-40%) [8.00 .. 13.00] | n/a: no firm wage-offer dispersion reported (all firms start at BASE_WAGE_OFFER) | n/a | SSA 1980-2013 | Song, Price, Guvenen, Bloom & von Wachter (2019 QJE), AKM decomposition table |
| investment spikes (i/k>20%) share of plant-years, % | 18 (inaction \|i/k\|<1%: 8.1%; mean i/k 12.2%) [14.00 .. 22.00] | n/a: no firm investment rates reported (gap closed over 48 months, <=2% of K a month) | n/a | LRD 1972-1988 | Cooper & Haltiwanger (2006 REStud) Table 1; spikes ~ half of investment |
| capacity utilisation, % | 79.3 (1967-2019 80.3; manufacturing 77.9) [77.00 .. 81.50] | 82.58 (sd 1.04) | minor | G.17 1985-2019 | FRED TCU (Federal Reserve G.17); author calc |
| corr(capacity utilisation, GDP), quarterly HP | +0.79 (1967-2019 +0.86); sd 2.0pp [0.65 .. 0.92] | 0.93 (sd 0.02) | minor | G.17 1985-2019 | FRED TCU vs GDPC1; author calc |
| hires rate, %/month | 3.62 [3.30 .. 4.00] | 1.63 (sd 0.02) | SEVERE | JOLTS 2001-2019 | FRED JTSHIR; author calc |
| layoffs & discharges, %/month (model: layoffs + exogenous E->U) | 1.39 [1.20 .. 1.60] | 1.05 (sd 0.01) [planned layoffs alone 0.15] | minor | JOLTS 2001-2019 | FRED JTSLDR; author calc |
| quits + other separations, %/month | 2.16 (quits 1.91); EE moves 2.6%/month (CPS) [1.90 .. 2.50] | 0.57 (sd 0.02) | material | JOLTS 2001-2019 | FRED JTSTSR - JTSLDR; Fallick & Fleischman (2004 FEDS 2004-34) |
| corr(hires rate, GDP), quarterly HP | +0.86 [0.6 .. 0.95] | 0.34 (sd 0.05) | minor | JOLTS 2001-2019 | FRED JTSHIR vs GDPC1; author calc |
| corr(layoffs rate, GDP), quarterly HP | -0.58 [-0.8 .. -0.35] | -0.69 (sd 0.04) | ok | JOLTS 2001-2019 | FRED JTSLDR vs GDPC1; author calc |
| corr(quits + other, GDP), quarterly HP | +0.87 (quits +0.88) [0.6 .. 0.95] | -0.14 (sd 0.09) | material | JOLTS 2001-2019 | FRED JTSTSR-JTSLDR; author calc |
| job openings rate, % | 3.13 [2.70 .. 3.60] | 4.36 (sd 0.08) | material | JOLTS 2001-2019 | FRED JTSJOR; author calc |
| Beveridge: corr(log v, log u), quarterly HP | -0.93 (Shimer: -0.89 1951-2003) [-0.97 .. -0.85] | -0.69 (sd 0.06) | material | JOLTS 2001-2019 | FRED JTSJOL, UNEMPLOY; Shimer (2005 AER) Table 1 |
| sd log(v/u), quarterly HP(1e5) | 0.39 (Shimer: 0.38 1951-2003) [0.3 .. 0.45] | 0.39 (sd 0.06) | ok | JOLTS 2001-2019 | FRED JTSJOL/UNEMPLOY; Shimer (2005 AER) Table 1 |
| sd log(v/u) / sd log labour productivity, HP(1e5) | 25 (Shimer: 19 1951-2003) [15.00 .. 30.00] | 36.42 (sd 4.90) | material | JOLTS/NFB 2001-2019 | FRED OPHNFB; Shimer (2005 AER): the unemployment volatility puzzle |
| inflow (separation) share of unemployment variance | 0.46 (Shimer 2012: ~0.25 post-1987) [0.25 .. 0.55] | 0.11 (sd 0.01) | minor | CPS flows 1990-2019 | FRED LNS17100000, LNS17400000; steady-state decomposition; Fujita & Ramey (2009 IER) 0.4-0.5 |
| elasticity of job-finding to v/u (1 - matching elasticity wrt u) | 0.37 (first differences 0.28) [0.25 .. 0.5] | 0.40 (sd 0.04) | ok | CPS/JOLTS 2001-2019 | author calc; Shimer (2005) 0.28; Petrongolo & Pissarides (2001 JEL) u-elasticity 0.5-0.7 |
| corr(change in inventories / GDP, GDP), quarterly HP | +0.52 (1967-2019 +0.57) [0.35 .. 0.7] | 0.34 (sd 0.04) | minor | NIPA 1985-2019 | FRED CBIC1, GDPC1; Blinder & Maccini (1991) |
| sd(GDP) / sd(final sales), quarterly HP | 1.15 (1967-2019 1.18) [1.05 .. 1.25] | 1.06 (sd 0.03) | ok | NIPA 1985-2019 | FRED GDPC1, FINSLC1; Ramey & West (1999) |

### Diagnosis of the flagged facts

- **Churn trap: exit and entry 14-15%/yr, exit drifting up 8pp in 20 years, 76% of entrants dead within 2 years, 90%
  of exits under age 5.**
  - Entry is mechanical: dead slots are reborn after 3 months (firms.rs reborn, ENTRY_DELAY_MONTHS), plus
    growth_entry tied to the household count, including the growing retiree share.
  - Entrants get 0.5× the sector median capital, startup equity of 3 months of a 2-worker payroll, and no customers.
  - Households sample switching candidates in proportion to expected demand and switch only to strictly cheaper
    firms (goods.rs), so entrants cannot build a customer base. They fail at 12-24 months on the distress or arrears
    rules.
  - Mature firms almost never exit (about 1%/yr).
- **Entry countercyclical** (-0.21 vs +0.50). Entry inherits the timing of exits; there is no expected-profit entry
  decision.
- **Proprietors' income 1.8% of GDP** (US 6.9%).
  - Owners are assigned only at t=0 (init.rs assign_owners), and reborn() never makes an entrant owner-operated.
  - NIPA proprietors' income includes the owner's labour income.
- **Worker flows too small** (hires 1.6%/month vs 3.6; quits and other separations 0.57 vs 2.16, acyclical; openings
  4.4% vs 3.1; Beveridge correlation -0.69).
  - A constant 0.9% exogenous E→U draw dominates separations.
  - On-the-job search reaches 4% of workers with 2 applications and a 10% wage-premium hurdle, against nearly
    homogeneous offers.
  - The per-firm ceil in openings (firms.rs) and default postings by churning entrants inflate vacancies.
  - The fix-labour job ladder addresses this. On the integrate snapshot (see the log below), hires are 3.9%/month,
    quits and other separations 2.8%, their cyclicality +0.88 and +0.79, and the Beveridge correlation -0.85.
- **The heterogeneity facts are not measurable yet** (size tail, 500+ share, job creation and destruction, markup
  dispersion, pay premia, investment spikes). The code implies large gaps:
  - At t=0, sizes are a Pareto(1.1) truncated at 65× the smallest weight. The largest firm has about 400 employees
    and the 500+ share is 0% (US 53%).
  - MARKUP_ANCHOR 0.45 keeps the markup P90/P50 near 1.03 (US about 2).
  - Investment closes the capital gap over 48 months with no fixed cost, so there is no inaction and no spikes.
- **Capacity utilisation 82.6% and too procyclical** (0.93). Desired capital is sized for 85% of capacity
  (`target_utilization`) with smooth adjustment.


## Missing outputs

These are series to add to `metrics.rs` (Metrics struct and CLI JSON) so that the n/a facts become measurable and the
flagged ones can be decomposed. Each is listed under the dimension that needs it.

- **equity:**
  - Monthly total return (`equity_return_1m`), and Fund dividends paid, regular and special separately
    (`fund_equity_dividends_bn_ar`).
  - The equity discount rate y_eq (`monetary.equity_long_rate`), smoothed dividends, the ERP state, and sector
    expected growth and productivity news.
  - After-tax profits and dividends by sector, and sector Tobin's q.
  - Fund bond portfolio value or bond total return, for a model-consistent stock–bond correlation.
- **rates:**
  - The rule's unrounded target i* and shadow rate, the real-time r* and u* estimates, and the expected short-rate
    component by tenor (`curve.expected`).
  - The lender premium (`bank.ebp`) and the expected business loss rate.
  - A loan-weighted average firm borrowing rate including the default premium, plus its 90th percentile.
  - A quarterly, non-trailing business charge-off rate, and mortgage delinquency and charge-off rates.
  - Total bank assets, net interest income, deposit interest expense, and credit refusals as a share of requests.
  - A 10-year inflation expectation and the 10y real yield.
- **cycle:**
  - Hours worked (an intensive margin), and monthly U→E and E→U probabilities split into layoffs and quits.
  - Real residential investment, the core PCE price level, the real-time u* and r*, and the TFP level with potential
    output.
  - The real durable-goods stock and new-home sales.
  - The employed-head count, as a sampling-noise indicator.
- **shocks:**
  - The common TFP level, and the oil AR state and user-shock component.
  - Energy CPI split into motor fuel and household energy, once Energy is split.
  - A real dollar index and the world price, existing vs new home sales, a transaction-price HPI before smoothing,
    expected house-price appreciation, and expected inflation by income quintile.
- **distribution:**
  - Top-1% and bottom-50% wealth shares, the share with net worth ≤ 0, and median net worth by age (computed in
    `Snapshot::by_age_median_net_worth` but not exported).
  - Hand-to-mouth and wealthy hand-to-mouth shares.
  - Disposable-income, earnings and consumption Ginis, and the top-1% earnings share.
  - Household overdrafts, mortgage delinquency 30/90, and household defaults.
  - Moments of the 12-month log wage change for stayers (P90–P10, Kelley skewness).
  - Counts of owners and of the tenure base.
- **firms:**
  - `firm_entries_ar` (already in `flows.firm_entries`).
  - Exits and firm counts by age class, and the firm-size distribution with the 500+ employment share.
  - Annual job creation and destruction, split by firm age.
  - Markup quantiles, the between-firm wage variance, and the firm investment-rate distribution (spikes, inaction).
  - Worker flows split into EE, exogenous E→U, bankruptcy displacement and U→E.

Two things cannot be measured at a monthly tick, so they are out of scope: daily or intra-month realised volatility,
and implied volatility.

## Re-measurement log

Counts are ok / minor / material / severe / n/a, out of 208 facts.

| Date | Binary | ok | minor | material | severe | n/a | Note |
|---|---|---|---|---|---|---|---|
| 2026-10-10 | master 8910b23 | 50 | 35 | 71 | 36 | 16 | Reference values in this document |
| 2026-10-10 | integrate eb779ca (snapshot) | 51 | 45 | 54 | 42 | 16 | Eight fix branches merged, before the final retune; fix-housing not yet merged |
| 2026-10-10 | p0-metrics (base f8fadf8, after second review fixes) | 50 | 43 | 63 | 49 | 3 | Metrics-out fields measured here; not like for like with the master reference tables above |

The integrate snapshot measured 41 facts better and 40 worse than master. Its baseline had not yet been retuned.

- **Better.**
  - Labour flows: hires, quits, their cyclicality and the Beveridge curve.
  - Employment volatility (0.81) and productivity cyclicality (0.59).
  - Inflation: core q/q AR(1) 0.66, and the inflation-u comovement (-0.28).
  - Rates: zero bound reached in 11% of months, inversions 6.6% of months, bank capital 14.0%.
  - Equity: excess kurtosis 2.0, recession drawdown -17%, bear markets 1.1 per decade.
  - Distribution: wealth Gini 0.77, top-10 share 65%, median NW/income 2.9, income Gini 0.50.
  - Housing: HPI momentum 0.52.
- **Worse.**
  - GDP volatility: HP sd 2.01, q/q sd 4.8, expansions 23 months.
  - Policy rate mean 1.66.
  - Equity valuation: P/E 27.5, equity/GDP 3.1, real return 3.0%, and returns now lag GDP (-0.25).
  - Firm churn: exit 18%/yr, entry cyclicality -0.51.
  - Proprietors' income 1.2% of GDP.
  - Real wage countercyclical (-0.61).
  - Foreclosures 448k/yr, youth unemployment beta 2.5, and the dollar's correlation with the 2y (0.73).
  - The residential-investment volatility ratio of 6.4 comes from one outlier seed (seed sd 9.7).

### metrics-out (branch p0-metrics, base f8fadf8)

- The observation fields listed under "Missing outputs" are in the monthly JSON (`Metrics.obs`, flattened, 70 fields;
  see `docs/API.md`). The model is unchanged: existing fields are bit-identical for seeds 1-3 at 5000 × 120 with the
  new keys stripped (`scripts/json_strip_compare.py`), and the IRF table is identical.
- The equity facts read `equity_return_1m` directly instead of rebuilding returns from the 12-month series. Fund and
  sector dividends are the firms' declarations for the month; bank dividends are not in the split.
- Review fixes. (1) The household overdraft count was added twice in the household pass, so both overdraft fields
  were exactly 2× the true value. It is now one count, with a unit test against a direct count. Consumer credit is
  therefore 1.1% of GDP (overdrafts only, since the model makes no consumer loans), not 2.2%; the overdraft share is
  about half the earlier value. (2) Business charge-offs: `biz_chargeoff_3m_ar` is the current and two previous
  months' write-offs, annualised (×4). The rates facts use it: corr(charge-off cycle, GDP cycle) is -0.15, against a
  target of -0.8 to -0.45 (it was -0.04 on the trailing 12-month loss rate). (3) Cost: the distribution statistics that
  need sorts over households and firms (Gini, top shares, quantiles, age medians, markup, firm-size tail) are refreshed
  every 12th month instead of every quarter. The expected business loss rate and firm loan rate p90 are not sorts and are
  computed monthly. The existing
  quarterly fields are unchanged. (4) Scope: the first pass's writes into `monetary.rs`, `firms.rs` and `types.rs` are
  reverted. The rule target i* (needs the rule's inputs recomputed), bankruptcy displacement (needs a counter) and the
  bank regular/special dividend split are not measured.
- Facts that are now measurable: the firm-size tail (Zipf 1.49 vs 0.9-1.1), the 500+ share (0.07% vs 46-56%), gross
  job creation and destruction (6.5%/yr vs 13-16%), the startup share of creation (22% vs 14-20%), markup dispersion
  (P90/P50 of P/MC, 1+markup: 1.02 vs 1.25-2.1), investment spikes (11.6% vs 14-22%), top-1% wealth (19.7% vs 31-37%), bottom-50% wealth
  (4.5% vs 1.5-2.6%), hand-to-mouth share (42% vs 25-40%), disposable-income Gini (0.48 vs 0.39-0.44), consumer credit
  (1.1% of GDP vs 16-19%), mortgage delinquency 30+ days (9.4% vs 1.5-4%), and median net worth by age (mean absolute
  error 0.47 of the overall median, against a 0.10 band).
- Still unmeasurable: pay premia (needs worker-firm fixed effects), the earnings-risk skewness fact, and personal
  bankruptcy filings (the model has no bankruptcy flow; foreclosures are reported instead).
- Scorecard on p0-metrics after the second review fixes: 208 facts, 50 ok, 43 minor, 63 material, 49 severe, 3 n/a. The
  markup dispersion moved from minor to material after the P/MC units fix. Before the first review fixes it was 50 / 44 / 61 / 50 / 3. The master reference tables above are unchanged; this branch's per-fact values are
  in the scorecard JSON, not in those tables.

## Re-fit rows (Step 13, final HEAD)

These rows were added in the re-fit (spec section 1.5 and the profit gates, section 8D). They are reported, not part of the 208-row severity sum (section 8F). The 8-seed column is the scorecard run (`final`). The 16-seed column is the scorecard run on 16 seeds (`final_16s`). `u_updown` and `du12_skew` are report-only (decision D16).

| Id | Fact | US target | Model, 8 seeds | Gap, 8 seeds | Model, 16 seeds | Gap, 16 seeds | Source |
|---|---|---|---|---|---|---|---|
| u_updown | mean rise / mean fall of 12-month change in u (pooled) | 1.20 .. 2.70; point 1.85 | 0.95 (seed sd 0.14) | minor | 1.02 (seed sd 0.182) | minor | Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020; FRED, this script (--fred) UNRATE. Pooled over seeds in paired.py |
| du12_skew | skewness of 12-month change in u | 0.4 .. 1.50; point 0.97 | 0.0253 (seed sd 0.275) | material | -0.0203 (seed sd 0.293) | material | FRED, this script (--fred) UNRATE |
| corr_rw_eff | corr(CPI-deflated wage per efficiency unit, Y), HP | 0 .. 0.5; point 0.25 | -0.199 (seed sd 0.271) | material | -0.162 (seed sd 0.266) | material | Stock & Watson (1999, Handbook of Macroeconomics ch. 1); FRED, this script (--fred) |
| emp_growth | employment growth, %/yr (months 12-239) | 0.3 .. 0.6; point 0.45 | 0.351 (seed sd 0.279) | ok | 0.253 (seed sd 0.24) | minor | BLS Employment Projections 2024-34; CBO |
| bank_ni_aftertax | Bank net income after tax / GDP, % | 1.00 [0.8 .. 1.20] | 1.09 (seed sd 0.0315) | ok | 1.07 (seed sd 0.0488) | ok | FDIC QBP net income (after taxes), as bank_profit_gdp; model: bank_net_income_bn_ar = bank profit less the bank corporate tax paid that month |
| ls_cyc | labour share, contemporaneous: corr(ls(t), GDP(t)), quarterly HP | +0.06 NFB (NFC -0.14; 1967-2019 NFB -0.10, NFC -0.07) [-0.3 .. 0.15] | -0.21 (sd 0.17) | ok | -0.15 (sd 0.18) | ok | FRED PRS85006173, PRS88003173 vs GDPC1, HP(1600) on logs; author calc. Rios-Rull & Santaeulalia-Llopis (2010 JME): mildly countercyclical on impact, overshooting later |
| prof_vol | profit volatility: sd HP log real profits / sd HP log real GDP | 7.2 (1967-2019 5.8; NFC after-tax 10.8) [5.00 .. 8.50] | 6.28 (sd 1.19) | ok | 6.00 (sd 1.04) | ok | FRED CPATAX/GDPDEF vs GDPC1, quarterly HP(1600); author calc. Model: after-tax profits of all firms (profit_share x nominal GDP), deflated by the GDP deflator |
| prof_cyc | profits procyclical: corr(HP profits(t), HP GDP(t)) | +0.12 (1967-2019 +0.46; NFC pretax +0.46) [0.1 .. 0.6] | 0.43 (sd 0.10) | ok | 0.42 (sd 0.08) | ok | FRED CPATAX/GDPDEF vs GDPC1, HP(1600); author calc |
| prof_lead4 | profits lead GDP: corr(HP profits(t), HP GDP(t+4q)) | +0.45 (1967-2019 +0.51; NFC pretax +0.62); corr with GDP(t-4q) -0.47 [0.3 .. 0.65] | 0.34 (sd 0.22) | ok | 0.41 (sd 0.21) | ok | FRED CPATAX/GDPDEF vs GDPC1, HP(1600); author calc. Profits peak 2-4 quarters before output |
| corp_profit_share | corporate profits after tax (shareholder firms + banks after tax), % of GDP | 7.6 (2010s 10.0; NFC after-tax/NFC GVA 8.6) [6.00 .. 10.00] | 9.93 (sd 0.83) | ok | 10.06 (sd 0.69) | minor | FRED CPATAX/GDP; author calc. Model: corp_profits_bn_ar + bank_net_income_bn_ar (owner-operated firms excluded: their profits are proprietors' income in NIPA); n/a without those metrics |
| corp_profit_drift | corporate profit share drift: years 16-20 minus years 2-6, pp | n/a (stationary profit share over the cycle of the US record) [-1.00 .. 1.00] | -0.26 (sd 1.05) | ok | -0.11 (sd 0.92) | ok | FRED CPATAX/GDP; author calc. Model: mean of corp_profit_share over months 180-239 minus months 12-71 |
| energy_profit_gdp | Energy after-tax profit, % of GDP (utilities, petroleum, oil and gas extraction) | approximate: 0.7-1.0 (BEA NIPA Table 6.16D, energy industries) [0.6 .. 1.10] | 1.12 (sd 0.26) | minor | 1.13 (sd 0.25) | minor | approximate; the source note says "approximate". Model: firm_profits_by_sector_bn_ar[Energy] over GDP |
| proprietors_nipa | proprietors' income, NIPA concept (owner-firm profit + owner's own pay), % of GDP | 6.9 [6.00 .. 8.00] | 3.07 (sd 0.48) | material | 3.03 (sd 0.43) | material | FRED PROPINC/GDP; model: owner_firm_profits_bn_ar + owner_own_wage_bn_ar; n/a without those metrics |
| markup_disp_eff | markup dispersion on realised P/AC (P90/P50 of price / unit cost) | P50 ~flat, P90 1.5 (1980) -> 2.5 (2016) [1.25 .. 2.10] | 1.14 (sd 0.02) | minor | 1.13 (sd 0.02) | minor | De Loecker, Eeckhout & Unger (2020). Model: markup_eff_p* (price over current unit cost) instead of the markup state markup_p*; n/a without those metrics |
| inv_inaction | investment inaction (/i/k/<1%) share of plant-years, % | 8.1 (negative investment 10.4%) [5.00 .. 11.00] | 2.13 (sd 0.17) | material | 2.16 (sd 0.16) | material | Cooper & Haltiwanger (2006 REStud) Table 1. Model: investment_inaction_share (annual, January) |


## Labour duration (scripts/facts_labour.py)

b5-udur rows, 8 seeds × 20k × 240, months 12–239. `m0` is the branch at defaults (equal to master). `candidate` is `scripts/profiles/b5-udur.json` (contact d .89, m1 10, norm .52, rank κ 2). Both are off by default; the flags are the scorecard gate (`scripts/facts_cycle.flag`).

| id | fact | band | m0 | gap | candidate | gap |
|---|---|---|---|---|---|---|
| dur_mean | mean unemployment duration, weeks | 16-21 | 13.1138 | material | 14.541 | material |
| dur_median | median unemployment duration, weeks | 8-11 | 8.8925 | ok | 9.3243 | ok |
| ltu_share | long-term unemployed (27+ weeks), % of unemployed | 15-23 | 12.3657 | material | 15.3482 | ok |
| ltu_peak | LTU share, 3-month average peak per seed, % | 26-46 | 23.307 | minor | 25.9353 | minor |
| dur_slope | slope of median duration (weeks) on u, per pp | 1.2-2.5 | 0.9922 | minor | 1.1049 | minor |
| ltu_lag | LTU share lag behind u, months (argmax corr) | 6-12 | 4.875 | minor | 5.125 | minor |
| med_lag | median duration lag behind u, months (argmax corr) | 3-8 | 2.125 | minor | 2.125 | minor |
| dur_ratio | mean ÷ median duration | 1.7-2.3 | 1.4747 | material | 1.5595 | minor |
| jf_lt_st | job finding 13+ months ÷ 1-3 months (pooled) | 0.33-0.42 | 0.4616 | material | 0.3654 | ok |

Sources: `UEMPMEAN`, `UEMPMED`, `UEMP27OV/UNEMPLOY`, `UNRATE` (CPS via FRED), with the 19-year-window statistics in docs/CALIBRATION.md (b5-udur). `jf_lt_st` is Krueger, Cramer & Cho (2014) and Kroft, Lange, Notowidigdo & Katz (2016). The `--fred` option that reproduces the US columns with the same code is not implemented in this batch.

## b5-firmhet: firm-distribution rows (S0, measurement only)

Rows added by `scripts/facts_firms.py` (ids new in b5-firmhet S0; existing rows unchanged). The 16-seed values are
from `scripts/firm_check.py levels` on the S0 build at defaults (B0, 20k x 240, seeds 1-16), which equals master on
every master key. The 8-seed column was not run for these rows. Older binaries read n/a.

| Id | Fact | US target | Model, 8 seeds | Gap, 8 seeds | Model, 16 seeds | Gap, 16 seeds | Source |
|---|---|---|---|---|---|---|---|
| markup_disp_avc | markup dispersion on variable cost (P90/P50 of P/VC = 1+markup) | P50 ~flat, P90 1.5 (1980) -> 2.5 (2016) [1.25 .. 2.10] | not run | not run | 1.17 (sd 0.03) | minor | De Loecker, Eeckhout & Unger (2020), sales/COGS concept. Model: markup_avc_p* |
| top1_share | employment share of the largest 1% of firms, % | finite-sample p25-p75 at N ~ 760: 32-52 [30 .. 52] | not run | not run | 18.1 (sd 3.5) | material | scripts/finite_size_benchmark.py (Pareto tail calibrated to 52.9% emp500). Model: firm_emp_top1_share |
| emp500_fs | employment share of firms with 500+ staff, finite-sample band, % | finite-sample p25-p75 at N ~ 760: 13-43 [13 .. 43] | not run | not run | 10.3 (sd 3.9) | minor | scripts/finite_size_benchmark.py. Model value = emp500 (firm_share_500plus), kept as a reported row |
| lp_disp_9010 | within-sector 90-10 range of log labour productivity, pooled | log TFP 90-10 about 0.65 (Syverson); labour-productivity ratios wider [0.65 .. 1.40] | not run | not run | 0.79 (sd 0.01) | ok | Syverson (2004 REStat; 2011 JEL). Model: firm_lp_p90_p10 |
| hhi_drift | concentration drift: mean over sectors of revenue HHI, years 16-20 minus years 2-6 | concentration rises slowly [0.00 .. 0.03] | not run | not run | 0.071 (sd 0.04) | material | Autor et al. (2020 QJE). Run is 240 months, so years 16-20 are truncated to 16-19 (months 192-239) |

Master-side reference (16 seeds, B0): trap count (seed-sector pairs with January median ln(K/K*) below -1, any year)
8 of 72 pairs over seeds 1-8, the same count as the diagnosis in `docs/CALIBRATION.md`; Olley-Pakes gap 0.010 (SE 0.003);
size-productivity slope 0.073 (SE 0.014); within-sector sd of log labour productivity 0.310.

## Joint calibration (b6-joint)

Scorecards at P0 (= master b3f95ff, no defaults changed) reproduce the archived master cards exactly on the shared ids.
16 seeds: 208-id severity 204 (archive 204), all-facts 235 (archive 217 without the new facts_labour rows). 8 seeds: 208-id
severity 209 (archive 209). `cardcmp` --worse` against the archive gives 0 worse and 0 better on shared ids.

S6 (upstream_margin 0.05, target_utilization 0.83; not adopted) at 16 seeds: 208-id severity 203, all-facts 232. Changed
flags against P0: 10 worse (u_skew, pi_ac1, pi_arsum, leads_gdp: ok to minor; y_sd, pc_corr, top10, inv_spikes: minor to
material; vol_unemp_corr: material to severe), and improved cu_level, energy_profit_gdp and corp_profit_share (to ok).

Facts_labour rows at P0 (16 seeds, card): dur_mean 13.11 (material, band 16–21), dur_median 8.89 (ok), ltu_share 12.37
(material, 15–23), ltu_peak 23.31 (minor, 26–46), dur_slope 0.99 (minor, 1.2–2.5), ltu_lag 4.88 (minor, 6–12), med_lag 2.13
(minor, 3–8), dur_ratio 1.47 (material, 1.7–2.3), jf_lt_st 0.46 (material, 0.33–0.42). See `docs/CALIBRATION.md`
(Joint calibration) for the paired D_J values.

## Joint calibration v2 (joint2)

The scorecard is re-run at the joint2 defaults (S6: `upstream_margin` 0.05, `target_utilization` 0.83). Commands and
numbers from the joint2 record in [CALIBRATION.md](CALIBRATION.md) (Joint calibration v2).

- **16-seed card (208 ids), master against the joint2 defaults.** Master: ok 91, minor 44, material 53, severe 18 (severity
  204). joint2: ok 91, minor 45, material 52, severe 18 (severity 203, Δ −1). All-facts severity 235 → 232. Firm entry 8.12 →
  8.27 %/yr and exit 8.03 → 8.22 %/yr (both above the 7 % guard). Energy profit / GDP 1.13 → 1.07 (minor → ok), `cu_level`
  82.56 → 80.70 (minor → ok).
- **Changed flags** (`--baseline` against the master card, `P0_16s`): better (12), `pluck_slope`,
  `antipluck_slope`, `speed_ratio`, `rec_auc`, `dep_beta_24m`, `neg_share`, `pred_r2_1y`, `entry_cyc`, `ls_lag`,
  `corp_profit_share`, `energy_profit_gdp`, `cu_level`. Worse (10): `y_sd`, `pc_corr`, `top10`, `inv_spikes`
  (minor → material), `vol_unemp_corr` (material → severe), `u_skew`, `pi_ac1`, `pi_arsum`, `leads_gdp`, `foreclosure_cycle`
  (ok → minor).
- **8-seed card** (report): severity 206 against master's 209.
- **Labour facts** (`facts_labour.py`, 8 seeds, 20,000 households; the BLS clock is used when present, which it is here):

  | Row | Band | Master | joint2 |
  |---|---|---|---|
  | dur_mean (weeks) | 16–21 | 13.11 (material) | 12.52 (material) |
  | dur_median (weeks) | 8–11 | 8.89 (ok) | 8.54 (ok) |
  | ltu_share (%) | 15–23 | 12.37 (material) | 11.32 (material) |
  | ltu_peak (%) | 26–46 | 23.31 (minor) | 21.78 (minor) |
  | dur_slope | 1.2–2.5 | 0.99 (minor) | 0.99 (minor) |
  | ltu_lag (months) | 6–12 | 4.88 (minor) | 4.88 (minor) |
  | med_lag (months) | 3–8 | 2.13 (minor) | 2.13 (minor) |
  | dur_ratio | 1.7–2.3 | 1.47 (material) | 1.47 (material) |
  | jf_lt_st | 0.33–0.42 | 0.46 (material) | 0.46 (material) |

  The duration rows are not moved by the S6 step. They are the reason the duration package was tried (see the joint2 record).

## Joint calibration 3 (joint3)

Final defaults: F1 (`cipi_excludes_nonstorable` on), P2 (`qe_duration_weight` 1.5, `fx_mp_premium` 1.25), DC90 (`duration_contact_decay` 0.90). The pin package was not adopted. The 16-seed card for the final binary (`stylized_facts.py --seeds 16`, `final_16s`) against the M card (`M_16s`, `cardcmp` --worse`): 208-id severity 202 (ok 90, minor 47, material 52, severe 17) against 203 (ok 91, minor 45, material 52, severe 18): ok 91 → 90, minor 45 → 47, material unchanged, severe 18 → 17 (see `cardcmp_final`). Firm entry 8.15 and exit 8.14 a year (M 8.27 and 8.22; guard ≥ 7%). Recession frequency (`rec_per_century`) 17.11 (M 19.41; minor). BLS duration on the card: mean 13.60 weeks (`dur_mean`, CPS band 16–21, material) and jf_lt_st 0.389 (ok; M 0.455). Facts that change category against M (cardcmp `--worse`, final card): `inv_share` ok to material, `pluck_slope` minor to material, `antipluck_slope` ok to material, and `g_ac1`, `corr_lp_emp`, `spread_mean`, `food_vol`, `corp_profit_share` ok to minor (open items). Under F1 alone `okun_gap` (minor to material) and `okun_diff` (material to severe) also worsened; DC90 restores both to their master categories.

## Joint calibration 4 (j4)

Final binary (`4561051`, the PbR3F0 defaults), 16-seed card (`stylized_facts.py --seeds 16`, `stylized_final16` (external archive, not in this repository), `final_16s`), compared with the joint3 M9 card (`S3_16s`) with `cardcmp` --worse`:

- Severity over the 208 ids: 201 (ok 91, minor 47, material 50, severe 18), against 202 (90, 47, 52, 17) on M9: a change of -1. The stored PbR3F0 card of batch 9 reproduces exactly (no id worse or better against it).
- Worse against M9 (4): speed_ratio material -> severe; u_skew ok -> minor; rec_auc ok -> minor; entry_cyc minor -> material.
- Better against M9 (4): inv_share material -> ok; pluck_slope material -> minor; g_ac1 minor -> ok; spread_mean minor -> ok.
- Firm entry 8.17 and exit 8.21 per year (target 9.7 and 8.8; guard >= 7%; gap ok on both).
- Recession frequency 16.1 per century on the card (BBQ rule; target 6-15; gap minor, as on M9).
- BLS duration (`facts_labour`): dur_mean 13.57 weeks against 13.54 on M9 (+0.03); long-term-unemployed share 13.33% against 13.29%. Both stay outside the CPS band (16-21 weeks; 15-23%), as on M9.
