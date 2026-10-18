#!/usr/bin/env python3
"""Distribution and household-finance stylised facts: baseline runs vs US targets.

Usage:
  python3 scripts/facts_distribution.py [seeds=8] [months=240] [--cli PATH] [--targets-out FILE] [--json-out FILE]

  CLI binary: --cli PATH, else $ECONSIM_CLI, else ./target/release/econsim-cli (relative to the repo root).
  Households: $DIST_HH (default 20000). $ECONSIM_CFG is passed through to the binary. $DIST_CACHE (a directory)
  caches each seed's JSON so the table can be re-printed without re-running.
  Only numpy and the standard library are used. Each seed runs in its own process with RAYON_NUM_THREADS=1.

Conventions: the first 12 reported months are skipped for levels (as in scripts/moments.py); trends are slopes of
annual means over the whole run; cycles are quarterly HP(1600) cycles. Money is converted to calibration dollars
(≈2026 $) by dividing by cpi/100. Ratios to GDP use the model's BEA-equivalent GDP (measured GDP ≈ 83% of BEA GDP,
README "Known limitations"), and the raw ratio is shown in brackets.

Gap flag: distance outside the target range divided by the fact's tolerance `tol`: 0 → ok, ≤1 → minor,
≤3 → material, >3 → severe. Facts the CLI JSON cannot measure are listed as unmeasurable with their targets.
"""
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
import numpy as np

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
BEA_SCALE = 0.83  # model measured GDP / BEA GDP (README)

# ---------------------------------------------------------------------------------------------------------------
# Targets. lo/hi: accepted range; tol: tolerance unit for the gap flag; central: preferred point value.
# ---------------------------------------------------------------------------------------------------------------
TARGETS = [
    dict(id='wealth_gini', name='Wealth Gini (net worth, households)', lo=0.82, hi=0.86, central=0.83, tol=0.02,
         sample='SCF 2019 / 2022 / 2025', unit='index',
         source='Federal Reserve SCF summary extract public data (scfp2019/2022/2025excel.zip), weighted Gini of '
                'NETWORTH computed here: 0.852 (2019), 0.830 (2022), 0.824 (2025). Kuhn, Schularick & Steins (2020, '
                'JPE) SCF+: 0.80-0.86 since 1989.'),
    dict(id='top10', name='Top-10% wealth share, %', lo=67.0, hi=76.5, central=72.0, tol=3.0,
         sample='SCF 2019-2025; DFA 1989-2026', unit='%',
         source='SCF extract (computed): 76.5 (2019), 73.4 (2022), 72.1 (2025). Fed Distributional Financial Accounts '
                '(dfa-networth-levels.csv, includes DB pensions and durables, so lower): 69.9 (2019Q4), 66.2 '
                '(2022Q4), 68.9 (2026Q2); 60.8 in 1989Q3.'),
    dict(id='top10_trend', name='Top-10% wealth share trend, pp per decade', lo=0.0, hi=3.5, central=2.9, tol=1.5,
         sample='DFA 1989Q3-2026Q2', unit='pp/decade',
         source='Fed DFA top-10% share 60.8% (1989Q3) -> 68.9% (2026Q2); OLS on Q4 values +0.29 pp/yr. SCF: 67% '
                '(1989) -> 72-76% (2019-2025) (Bricker et al. 2020, Fed Bulletin). Rising, not falling.'),
    dict(id='nw_to_income', name='Median net worth / median household income', lo=2.0, hi=2.8, central=2.6, tol=0.3,
         sample='SCF 2019 / 2022 / 2025', unit='ratio',
         source='SCF extract (computed, medians of NETWORTH and INCOME): 2.06 (2019), 2.74 (2022), 2.63 (2025). '
                'Published 2022: $192,900 / $70,260 (Aladangady et al. 2023, Fed Bulletin).'),
    dict(id='median_nw_real', name='Median net worth, real (calibration $ ~2026), $k', lo=155.0, hi=225.0,
         central=222.0, tol=30.0, sample='SCF 2019-2025 in 2025 $', unit='$k',
         source='SCF extract (2025 dollars): $154.5k (2019), $210.9k (2022), $216.1k (2025); ~$222k in 2026 $.'),
    dict(id='income_gini', name='Income Gini (pre-tax money income incl. transfers)', lo=0.47, hi=0.50,
         central=0.49, tol=0.02, sample='Census CPS ASEC 2015-2025', unit='index',
         source='Census household money-income Gini (FRED GINIALLRH): 0.484 (2019), 0.488 (2022), 0.489 (2024), '
                '0.490 (2025); Census P60-286. Model income = wage+benefits+pension+transfers+rent+interest+'
                'dividends+business income, i.e. the Census money-income concept.'),
    dict(id='income_gini_trend', name='Income Gini trend, per decade', lo=0.0, hi=0.025, central=0.018, tol=0.015,
         sample='Census 1984-2025', unit='/decade',
         source='FRED GINIALLRH OLS slope 1984-2025: +0.018 per decade (0.415 -> 0.490; includes the 2013 CPS '
                'redesign step of ~+0.01).'),
    dict(id='earn_mean_median', name='Household earnings mean/median (earner households)', lo=1.30, hi=1.55,
         central=1.45, tol=0.08, sample='SCF 2019-2025; CPS ASEC', unit='ratio',
         source='SCF extract WAGEINC>0, head working 25-64 (computed): 1.46 (2019), 1.54 (2022), 1.44 (2025); all '
                'earner households 1.51-1.64. CPS ASEC household earnings (top-coded) ~1.3. SSA individual net '
                'compensation mean/median ~1.4-1.5.'),
    dict(id='wage_age_peak', name='Earnings 45-54 / 25-34 (age profile peak)', lo=1.15, hi=1.30, central=1.23,
         tol=0.06, sample='BLS CPS 2026Q2', unit='ratio',
         source='BLS Usual Weekly Earnings Table 3 (median, full-time W&S, 2026Q2): 16-24 $806, 25-34 $1,160, 35-44 '
                '$1,436, 45-54 $1,421, 55-64 $1,367, 65+ $1,233 -> 45-54/25-34 = 1.23, 35-44/25-34 = 1.24 (peak '
                '35-54; Guvenen et al. 2021 Econometrica: cross-section peaks at 45-55).'),
    dict(id='wage_age_young', name='Earnings 18-24 / 25-34', lo=0.62, hi=0.78, central=0.70, tol=0.05,
         sample='BLS CPS 2026Q2', unit='ratio', source='BLS Usual Weekly Earnings Table 3: 16-24 / 25-34 = 0.695.'),
    dict(id='wage_age_old', name='Earnings 65+ / 45-54', lo=0.80, hi=0.92, central=0.87, tol=0.05,
         sample='BLS CPS 2026Q2', unit='ratio', source='BLS Usual Weekly Earnings Table 3: 65+ / 45-54 = 0.868; '
                                                      '55-64 / 45-54 = 0.962.'),
    dict(id='own_u25', name='Homeownership, head <25, %', lo=20.0, hi=26.0, central=23.0, tol=4.0,
         sample='Census HVS 2015-2024', unit='%',
         source='Census CPS/HVS annual Table 17 (ann25t_17.xlsx): <25 21.8 (2015), 23.2 (2019), 25.4 (2022), 23.0 '
                '(2024).'),
    dict(id='own_25_34', name='Homeownership, head 25-34, %', lo=38.0, hi=43.0, central=40.5, tol=3.0,
         sample='Census HVS 2019-2024', unit='%',
         source='HVS Table 17, 2024: 25-29 33.5, 30-34 47.4 (mean 40.5); 2019: 32.9 / 48.0; 2022: 35.6 / 48.8.'),
    dict(id='own_45_54', name='Homeownership, head 45-54, %', lo=69.5, hi=71.5, central=70.6, tol=2.0,
         sample='Census HVS 2019-2026', unit='%', source='HVS Table 17/19: 70.1 (2019), 70.5 (2022-23), 70.6 (2024).'),
    dict(id='own_65', name='Homeownership, head 65+, %', lo=78.0, hi=80.0, central=79.0, tol=2.0,
         sample='Census HVS 2015-2026', unit='%',
         source='HVS Table 17/19: 78.9 (2015), 78.6 (2019), 79.1 (2022), 79.0 (2024), 78.4-78.6 (2026H1).'),
    dict(id='u_youth_ratio', name='Unemployment 18-24 / 25-54 (level)', lo=1.8, hi=2.4, central=2.0, tol=0.3,
         sample='BLS CPS 1984-2019; 2025', unit='ratio',
         source='FRED LNS14000036 / LNS14000060 (20-24 vs 25-54) mean ratio 1.99 (1984-2019), 2.04 (1955-2019); BLS '
                'CPS Table 3 2025: 8.3 / 3.6 = 2.3. Model: heads aged 18-24 vs mean of the 25-34/35-44/45-54 rates.'),
    dict(id='u_youth_cycle', name='Unemployment cycle beta, 18-24 on 25-54', lo=1.25, hi=1.7, central=1.4, tol=0.25,
         sample='BLS CPS 1984-2019 (HP 1600, quarterly)', unit='slope',
         source='FRED LNS14000036 (20-24) on LNS14000060 (25-54), HP-filtered quarterly cycles: slope 1.36 '
                '(1984-2019), 1.54 (1955-2019), 1.64 (1984-2025); sd ratio 1.45 / 1.62. Jaimovich & Siu (2009): the '
                'young carry a disproportionate share of cyclical volatility.'),
    dict(id='mort_gdp', name='Mortgage debt / GDP (BEA-equivalent)', lo=0.43, hi=0.53, central=0.48, tol=0.05,
         sample='Fed Z.1 / BEA 2015-2025', unit='ratio',
         source='FRED HHMSDODNS / GDP: mean 0.48 (2015-2025, range 0.44-0.53), 0.44 (2024-26), 0.49 mean '
                '1984-2025 (peak 0.73 in 2007; 0.27 mean 1955-83).'),
    dict(id='mort_per_owner', name='Mortgage debt per owner / median owner home value', lo=0.40, hi=0.52,
         central=0.45, tol=0.06, sample='SCF 2019-2025', unit='ratio',
         source='SCF extract (computed: mean MRTHEL over owners / median HOUSES of owners): 0.52 (2019), 0.42 '
                '(2022), 0.41 (2025). Aggregate debt/value among owners 0.29-0.34; Z.1 owners equity share 71.9% '
                '(2026Q2), 62% mean 1984-2025 (FRED HOEREPHRE).'),
    dict(id='foreclosures', name='Foreclosures, thousands per year', lo=120.0, hi=350.0, central=220.0, tol=100.0,
         sample='NY Fed CCP 2018-2025 (non-crisis)', unit='k/yr',
         source='NY Fed Household Debt & Credit Report: new foreclosure notations ~53-58k per quarter in 2025 '
                '(~0.22M/yr), ~70-90k per quarter in 2018-19 (~0.3M/yr); peak ~0.55M per quarter in 2009. ATTOM '
                'foreclosure starts ~0.27M (2023).'),
    dict(id='foreclosure_cycle', name='corr(unemployment, foreclosures), quarterly HP cycles', lo=0.4, hi=0.9,
         central=0.7, tol=0.2, sample='US 1991-2019 (proxy: mortgage delinquency)', unit='corr',
         source='FRED DRSFRMACBS (single-family mortgage delinquency, commercial banks) vs UNRATE, HP-filtered '
                'quarterly 1991-2019: corr +0.70 same quarter, +0.56 one quarter later. Mian & Sufi (2009): defaults '
                'concentrate where unemployment and house prices move together.'),
    dict(id='cons_beta_top', name='Consumption exposure: beta(Q5) / mean beta(Q1-Q3)', lo=1.5, hi=5.0, central=2.5,
         tol=0.75, sample='CEX 1982-2004', unit='ratio',
         source='Parker & Vissing-Jorgensen (2009, AER P&P 99(2)): top-10% (consumption-ranked) exposure to '
                'aggregate consumption growth ~5x the bottom 80%; their income exposure is also higher since the '
                'early 1980s. Income-ranked quintiles dilute this, so 1.5-5.'),
    dict(id='cons_ineq_trend', name='Consumption growth gap Q5 - Q1, pp/yr', lo=0.0, hi=1.0, central=0.5, tol=0.5,
         sample='CEX 1980-2007; CPS 1984-2025', unit='pp/yr',
         source='Aguiar & Bils (2015, AER): consumption inequality between high- and low-income households tracked '
                'the rise in income inequality 1980-2007; Meyer & Sullivan (2023, JPE): smaller but non-negative '
                'rise. Census income Gini rose +0.018/decade. A falling top/bottom ratio is the wrong sign.'),
    # ---- not measurable from the CLI JSON ----
    dict(id='top1', name='Top-1% wealth share, %', lo=31.0, hi=37.0, central=33.0, tol=3.0, 
         sample='SCF 2019-2025; DFA 2026Q2', unit='%',
         source='SCF extract (computed): 37.2 (2019), 35.1 (2022), 31.3 (2025); DFA 30.4 (2019Q4), 32.5 (2026Q2); '
                'Saez & Zucman (2016, QJE) ~37-42% (tax-capitalised).'),
    dict(id='bottom50', name='Bottom-50% wealth share, %; share with NW<=0', lo=1.5, hi=2.6, central=2.2, tol=0.5,
         sample='SCF 2019-2025; DFA', unit='%',
         source='SCF extract (computed): 1.5 (2019), 2.2 (2022), 2.1 (2025); DFA 2.3-2.6 (2022-26). Households with '
                'NW<=0: 10.8% (2019), 7.9% (2022), 8.6% (2025).'),
    dict(id='nw_by_age', name='Median NW by age / overall median (<35,35-44,45-54,55-64,65-74,75+): mean abs error', lo=0.0, hi=0.10, central=[0.15, 0.72, 1.18, 1.90, 2.00, 2.34], tol=0.10, 
         sample='SCF 2025 (2019: 0.11,0.75,1.39,1.75,2.19,2.09)', unit='ratio',
         source='SCF extract (computed): medians $32.8k, $156k, $255k, $411k, $432k, $505k vs $216k overall. '
                'Elderly decumulate slowly (De Nardi, French & Jones 2010, JPE).'),
    dict(id='htm', name='Hand-to-mouth share of households, %', lo=25.0, hi=40.0, central=31.0, tol=5.0,
         sample='SCF 1989-2010; SCF 2025', unit='%',
         source='Kaplan, Violante & Weidner (2014, BPEA): ~31% HtM, about two-thirds of them wealthy HtM (own '
                'illiquid assets). SCF 2025 extract, liquid assets net of card debt <= half monthly income: 40% '
                '(20% wealthy HtM).'),
    dict(id='disp_gini', name='Disposable-income Gini (after taxes and transfers)', lo=0.39, hi=0.44, central=0.43,
         tol=0.02, sample='CBO 2022; OECD IDD 2021-22', unit='index',
         source='CBO, The Distribution of Household Income 2022: Gini after transfers and federal taxes 0.434 '
                '(before 0.51); OECD IDD equivalised disposable ~0.39.'),
    dict(id='consumer_credit', name='Consumer credit / GDP (revolving / GDP)', lo=0.16, hi=0.19, central=0.18,
         tol=0.02, sample='Fed G.19 / BEA 2015-2025', unit='ratio',
         source='FRED TOTALSL / GDP: mean 0.183 (2015-25), 0.158 (2026Q2); REVOLSL / GDP 0.047 (2015-25), 0.041 '
                '(2026Q2). The model has only an overdraft line (negative deposits), not reported.'),
    dict(id='mort_delinq', name='Mortgage delinquency rate, % (normal times; peak 11.5)', lo=1.5, hi=4.0, central=2.5,
         tol=1.0, sample='Fed (commercial banks) 1991-2026', unit='%',
         source='FRED DRSFRMACBS: mean 3.7% 1991-2025, trough 1.86% (2026Q2), peak 11.5% (2010Q1); NY Fed CCP 90+ '
                'days ~0.5-1% (2019-25).'),
    dict(id='personal_bankruptcy', name='Personal bankruptcy filings, % of households per year', lo=0.35,
         hi=0.6, central=0.41, tol=0.1, measurable=False, sample='US Courts 2015-2025', unit='%/yr',
         source='US Courts: 549,577 non-business filings in CY2025 (0.41% of ~133M households), 494,201 (2024), '
                '~750k (2019); 1.5M in 2010; 2.0M in 2005 (pre-BAPCPA spike). SCF: 1.3-2.0% filed in last 5 years.'),
    dict(id='income_risk', name='Earnings risk: left skewness countercyclical, dispersion acyclical', lo=None,
         hi=None, central=None, tol=None, measurable=False, sample='SSA panel 1978-2010', unit='sign',
         source='Guvenen, Ozkan & Song (2014, JPE): the variance of idiosyncratic earnings shocks is roughly '
                'acyclical while left skewness rises in recessions (upper tail thins, lower tail thickens); '
                'Guvenen et al. (2021, Econometrica): 1-yr log earnings changes are negatively skewed and leptokurtic '
                '(kurtosis >> 3).'),
]


def cli_path(args):
    if '--cli' in args:
        return args[args.index('--cli') + 1]
    return os.environ.get('ECONSIM_CLI', os.path.join(ROOT, 'target', 'release', 'econsim-cli'))


def opt(args, name):
    return args[args.index(name) + 1] if name in args else None


def positional(args):
    out, skip = [], False
    for i, a in enumerate(args):
        if skip:
            skip = False
            continue
        if a.startswith('--'):
            skip = a in ('--cli', '--targets-out', '--json-out')
            continue
        out.append(a)
    return out


def run(cli, hh, months, seed, cache):
    path = os.path.join(cache, f'dist_hh{hh}_m{months}_s{seed}.json') if cache else None
    if path and os.path.exists(path):
        with open(path) as f:
            return json.load(f)
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([cli, 'run', str(hh), str(months), str(seed), '--json'], capture_output=True, text=True,
                         env=env, check=True).stdout
    h = json.loads(out.strip().splitlines()[-1])
    if path:
        with open(path, 'w') as f:
            json.dump(h, f)
    return h


def hp(y, lam=1600.0):
    n = len(y)
    I = np.eye(n)
    D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)


def quarterly(v):
    v = np.asarray(v, float)
    return v[: len(v) // 3 * 3].reshape(-1, 3).mean(axis=1)


def series(h, key, idx=None):
    # Keys added by metrics-out are missing from older binaries: those give NaN (reported as n/a), not an error.
    return np.array([np.nan if key not in m else (m[key] if idx is None else m[key][idx]) for m in h], float)


def annual_slope(v):
    """OLS slope per year of annual means."""
    a = np.asarray(v, float)[: len(v) // 12 * 12].reshape(-1, 12).mean(axis=1)
    return np.polyfit(np.arange(len(a)), a, 1)[0]


def measure(h):
    """Model moments for one seed."""
    r = {}
    L = h[12:]  # levels: skip the first reported year
    mean = lambda k, i=None: series(L, k, i).mean()
    r['wealth_gini'] = mean('wealth_gini')
    r['wealth_gini_first_last'] = (series(h[:12], 'wealth_gini').mean(), series(h[-12:], 'wealth_gini').mean())
    r['top10'] = mean('top10_wealth_share')
    r['top10_first_last'] = (series(h[:12], 'top10_wealth_share').mean(), series(h[-12:], 'top10_wealth_share').mean())
    r['top10_trend'] = 10 * annual_slope(series(h, 'top10_wealth_share'))
    med_inc = series(L, 'median_house_price') / np.maximum(series(L, 'price_to_income'), 1e-9)
    r['nw_to_income'] = np.mean(series(L, 'median_net_worth') / med_inc)
    # real levels are compared in the first reported year (the calibration date); the last year shows the drift
    real_nw = lambda hh: np.mean(series(hh, 'median_net_worth') / (series(hh, 'cpi') / 100.0)) / 1e3
    r['median_nw_real'] = real_nw(h[:12])
    r['median_nw_real_last'] = real_nw(h[-12:])
    r['median_income_real'] = np.mean(med_inc / (series(L, 'cpi') / 100.0)) / 1e3
    r['income_gini'] = mean('income_gini')
    r['income_gini_first_last'] = (series(h[:12], 'income_gini').mean(), series(h[-12:], 'income_gini').mean())
    r['income_gini_trend'] = 10 * annual_slope(series(h, 'income_gini'))
    r['earn_mean_median'] = np.mean(series(L, 'avg_wage_annual') / series(L, 'median_wage_annual'))
    wa = np.array([m['wage_by_age'] for m in L], float).mean(axis=0)
    r['wage_by_age'] = list(wa)
    r['wage_age_peak'] = wa[3] / wa[1]
    r['wage_age_young'] = wa[0] / wa[1]
    r['wage_age_old'] = wa[5] / wa[3]
    own = np.array([m['homeownership_by_age'] for m in L], float)
    r['own_by_age'] = list(own.mean(axis=0))
    r['own_u25'], r['own_25_34'], r['own_45_54'], r['own_65'] = own[:, 0].mean(), own[:, 1].mean(), own[:, 3].mean(), own[:, 5].mean()
    r['own_65_first_last'] = (np.mean([m['homeownership_by_age'][5] for m in h[:12]]),
                              np.mean([m['homeownership_by_age'][5] for m in h[-12:]]))
    ua = np.array([m['unemployment_by_age'] for m in L], float)
    prime = ua[:, 1:4].mean(axis=1)
    r['u_by_age'] = list(ua.mean(axis=0))
    r['u_youth_ratio'] = ua[:, 0].mean() / prime.mean()
    r['u_old_ratio'] = ua[:, 4].mean() / prime.mean()
    # cyclical sensitivity as the slope of the youth cycle on the prime-age cycle: the 18-24 group is small, and
    # agent-sampling noise in it inflates a ratio of standard deviations but does not bias the slope
    cy, cp = hp(quarterly(ua[:, 0])), hp(quarterly(prime))
    r['u_youth_cycle'] = np.polyfit(cp, cy, 1)[0]
    r['u_youth_sd_ratio'] = cy.std() / cp.std()
    gdp_bea = series(L, 'gdp_nominal_bn_ar') / BEA_SCALE
    r['mort_gdp'] = np.mean(series(L, 'mortgage_debt_bn') / gdp_bea)
    r['mort_gdp_raw'] = np.mean(series(L, 'mortgage_debt_bn') / series(L, 'gdp_nominal_bn_ar'))
    owners = series(L, 'homeownership_rate') / 100.0 * series(L, 'population_households_m') * 1e6
    r['mort_per_owner'] = np.mean(series(L, 'mortgage_debt_bn') * 1e9 / owners / series(L, 'median_house_price'))
    r['foreclosures'] = mean('foreclosures_thousands_ar')
    fq, uq = hp(quarterly(series(L, 'foreclosures_thousands_ar'))), hp(quarterly(series(L, 'unemployment_rate')))
    r['foreclosure_cycle'] = np.corrcoef(uq, fq)[0, 1]
    # consumption by income quintile: yoy growth of real purchases per household (refreshed every 3 months)
    Q = [m for m in L if m['tick'] % 3 == 0]
    g = np.array([m['consumption_by_quintile'] for m in Q], float)
    pur = series(h, 'household_purchases_real_bn_ar') / series(h, 'population_households_m')
    ticks = {m['tick']: i for i, m in enumerate(h)}
    agg = np.array([100 * (pur[ticks[m['tick']]] / pur[ticks[m['tick']] - 12] - 1) for m in Q])
    betas = [np.cov(g[:, q], agg)[0, 1] / agg.var(ddof=1) for q in range(5)]
    r['cons_betas'] = betas
    r['cons_beta_top'] = betas[4] / np.mean(betas[:3])
    r['cons_ineq_trend'] = (g[:, 4] - g[:, 0]).mean()
    r['cons_growth_by_q'] = list(g.mean(axis=0))
    # --- metrics-out fields (wealth, hand-to-mouth, disposable income, credit, delinquency, NW by age) ---
    r['top1'] = mean('wealth_top1_share')
    r['bottom50'] = mean('wealth_bottom50_share')
    r['htm'] = mean('htm_share')
    r['disp_gini'] = mean('disposable_income_gini')
    # revolving credit proxy: consumer loans plus household overdrafts, $bn stock over BEA-scaled nominal GDP
    r['consumer_credit'] = np.mean((series(L, 'consumer_credit_bn') + series(L, 'overdraft_balance_bn')) / gdp_bea)
    r['mort_delinq'] = mean('mortgage_delinquent_30_share')
    # model age buckets are 18-24, 25-34, 35-44, 45-54, 55-64, 65+; the target groups map to <35 = 25-34, 65-74 and
    # 75+ both = 65+. Error is the mean absolute gap between model and target ratios to the overall median.
    mb = np.array([m.get('median_nw_by_age', [np.nan] * 6) for m in L], float).mean(axis=0)  # NaN on older binaries
    ratio = np.array([mb[1], mb[2], mb[3], mb[4], mb[5], mb[5]]) / mean('median_net_worth')
    r['nw_by_age'] = float(np.mean(np.abs(ratio - np.array([0.15, 0.72, 1.18, 1.90, 2.00, 2.34]))))
    return r


def flag(t, v):
    if t.get('measurable', True) is False or v is None or not np.isfinite(v):
        return 'unmeasurable'
    lo, hi, tol = t['lo'], t['hi'], t['tol']
    d = max(lo - v, v - hi, 0.0) / tol
    return 'ok' if d == 0 else 'minor' if d <= 1 else 'material' if d <= 3 else 'severe'


def fmt(v):
    if isinstance(v, (list, tuple)):
        return '[' + ', '.join(fmt(x) for x in v) + ']'
    if v is None:
        return '-'
    a = abs(v)
    return f'{v:.0f}' if a >= 100 else f'{v:.1f}' if a >= 10 else f'{v:.2f}' if a >= 1 else f'{v:.3f}'


def main():
    args = sys.argv[1:]
    pos = positional(args)
    seeds = int(pos[0]) if len(pos) > 0 else 8
    months = int(pos[1]) if len(pos) > 1 else 240
    hh = int(os.environ.get('DIST_HH', '20000'))
    cli = cli_path(args)
    cache = os.environ.get('DIST_CACHE')
    if cache:
        os.makedirs(cache, exist_ok=True)
    tout = opt(args, '--targets-out')
    if tout:
        with open(tout, 'w') as f:
            json.dump({'dimension': 'distribution and household finance', 'targets': TARGETS}, f, indent=1)
    t0 = time.time()
    with ThreadPoolExecutor(min(seeds, 8)) as ex:
        runs = list(ex.map(lambda s: run(cli, hh, months, s, cache), range(1, seeds + 1)))
    wall = time.time() - t0
    per = [measure(h) for h in runs]
    agg = {}
    for k in per[0]:
        vals = [p[k] for p in per]
        if isinstance(vals[0], (list, tuple)):
            agg[k] = list(np.mean(np.array(vals, float), axis=0))
        else:
            agg[k] = float(np.mean(vals))
            agg[k + '_sd'] = float(np.std(vals))
    extra = {
        'wealth_gini': f"first/last yr {fmt(agg['wealth_gini_first_last'])}",
        'top10': f"first/last yr {fmt(agg['top10_first_last'])}",
        'income_gini': f"first/last yr {fmt(agg['income_gini_first_last'])}",
        'nw_to_income': f"median income {fmt(agg['median_income_real'])}k",
        'median_nw_real': f"first year; last year {fmt(agg['median_nw_real_last'])}k",
        'u_youth_cycle': f"sd ratio {fmt(agg['u_youth_sd_ratio'])} (inflated by agent sampling)",
        'wage_age_peak': f"by age {fmt([x / agg['wage_by_age'][1] for x in agg['wage_by_age']])}",
        'own_65': f"first/last yr {fmt(agg['own_65_first_last'])}; all {fmt(agg['own_by_age'])}",
        'u_youth_ratio': f"u by age {fmt(agg['u_by_age'])}; 55-64/prime {fmt(agg['u_old_ratio'])}",
        'mort_gdp': f"raw {fmt(agg['mort_gdp_raw'])}",
        'cons_beta_top': f"betas Q1-Q5 {fmt(agg['cons_betas'])}",
        'cons_ineq_trend': f"growth Q1-Q5 {fmt(agg['cons_growth_by_q'])}",
    }
    rows = []
    for t in TARGETS:
        v = agg.get(t['id']) if t.get('measurable', True) else None
        sd = agg.get(t['id'] + '_sd')
        tgt = (f"{fmt(t['lo'])}-{fmt(t['hi'])}" if t['lo'] is not None else fmt(t['central']))
        model = '-' if v is None else f"{fmt(v)} (sd {fmt(sd)})"
        if t['id'] in extra and v is not None:
            model += ' ' + extra[t['id']]
        rows.append((t['name'], tgt, model, flag(t, v), t['sample']))
    print(f"Distribution & household finance: {seeds} seeds x {months} months x {hh} households; "
          f"runs {wall:.0f}s wall; cli {cli}")
    w0 = max(len(r[0]) for r in rows)
    print(f"{'fact':{w0}s} | {'target':>11s} | {'gap':12s} | model (mean over seeds)  [target sample]")
    for name, tgt, model, fl, sample in rows:
        print(f"{name:{w0}s} | {tgt:>11s} | {fl:12s} | {model}  [{sample}]")
    jout = opt(args, '--json-out')
    if jout:
        with open(jout, 'w') as f:
            json.dump({'seeds': seeds, 'months': months, 'households': hh, 'wall_s': wall, 'model': agg,
                       'flags': {t['id']: flag(t, agg.get(t['id']) if t.get('measurable', True) else None)
                                 for t in TARGETS}}, f, indent=1, default=float)


if __name__ == '__main__':
    main()
