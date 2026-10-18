#!/usr/bin/env python3
"""Firm dynamics and labour-market structure: model vs US stylised facts.

Usage:  python3 scripts/facts_firms.py [path/to/econsim-cli]
        (or ECONSIM_CLI=...; default ./target/release/econsim-cli). Honours ECONSIM_CFG (passed through to the model).
Env:    FACTS_SEEDS (8)  FACTS_MONTHS (240)  FACTS_HH (20000)  FACTS_JOBS (8 parallel runs, RAYON_NUM_THREADS=1 each)
        FACTS_CACHE     directory to reuse/save run_<seed>.json (development convenience; off by default)
        FACTS_TARGETS_OUT  also write the target table (values, samples, sources) to this JSON path.
Prints: fact | US target | model | gap flag (ok / minor / material / severe / unmeasurable).

US targets: author's calculations from Census BDS (bds2022*.csv, firm-level, March-to-March years), FRED (JOLTS,
CPS gross flows, G.17, NIPA) with the same filters as applied to the model, plus published micro estimates (Axtell
2001; Cooper & Haltiwanger 2006; Song et al. 2019; De Loecker, Eeckhout & Unger 2020; Shimer 2005; Fujita & Ramey
2009; Petrongolo & Pissarides 2001; Fallick & Fleischman 2004; Knaup 2005). Quarterly cycles are HP(1600) unless
noted; Shimer-style volatilities use HP(1e5) on log levels as in Shimer (2005). Only numpy and the stdlib are used.
"""
import json, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
import numpy as np

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
CLI = sys.argv[1] if len(sys.argv) > 1 else os.environ.get('ECONSIM_CLI', os.path.join(ROOT, 'target', 'release', 'econsim-cli'))
SEEDS = int(os.environ.get('FACTS_SEEDS', 8))
MONTHS = int(os.environ.get('FACTS_MONTHS', 240))
HH = int(os.environ.get('FACTS_HH', 20000))
JOBS = int(os.environ.get('FACTS_JOBS', 8))
CACHE = os.environ.get('FACTS_CACHE')
CFG = json.loads(os.environ['ECONSIM_CFG']) if os.environ.get('ECONSIM_CFG') else {}
# Exogenous E->U separation probability per month (config.rs `separation_rate`; not reported in the JSON).
SEP_RATE = 100.0 * CFG.get('separation_rate', 0.009)
SKIP = 12  # first reported year discarded
PERSONS_PER_EMPLOYED_HEAD = 1.75  # metrics.rs: employment_persons_millions = employed heads x 1.75

# ------------------------------------------------------------------------------------------------------------------
# Targets: (key, label, lo, hi, tol, target text, US sample, source). Gap = distance outside [lo, hi] in units of tol:
# 0 ok, <=1 minor, <=3 material, >3 severe. tol None = not measurable from master outputs (reported as unmeasurable).
# ------------------------------------------------------------------------------------------------------------------
T = [
    # --- firm dynamics (BDS) ---
    ('exit_rate', 'firm exit (death) rate, %/yr', 7.5, 10.5, 2.0, '8.8 (2010s 7.8; 1979-2019 9.0)',
     'BDS 1985-2019', 'Census BDS 2022 release: firmdeath_firms(t)/firms(t-1); author calc'),
    ('entry_rate', 'firm entry (startup) rate, %/yr', 8.0, 11.5, 2.0, '9.7 (2010s 8.2; 1980s 12)',
     'BDS 1985-2019', 'Census BDS 2022: age-0 firms / firms; author calc'),
    ('exit_drift', 'exit-rate drift: last 5y - first 5y of run, pp', -3.0, 1.0, 2.0, 'stationary/declining: -1.9pp (1985-89 to 2000-04)',
     'BDS 1985-2004', 'Census BDS 2022 (10.3% in 1985-89, 8.4% 2000-04, 7.6% 2015-19); Decker et al. (2014 JEP)'),
    ('entry_cyc', 'corr(entry rate, GDP), annual HP(6.25)', 0.25, 0.75, 0.25, '+0.50',
     'BDS 1985-2019', 'Census BDS 2022 vs FRED GDPC1 Q1 levels; author calc. Procyclical entry: Lee & Mukoyama (2015 RED)'),
    ('exit_cyc', 'corr(exit rate, GDP), annual HP(6.25)', -0.6, -0.15, 0.25, '-0.36',
     'BDS 1985-2019', 'Census BDS 2022 vs FRED GDPC1; author calc (exit rises with unemployment: corr(exit, du) +0.31)'),
    ('exit_young_share', 'share of firm exits aged <5 years, %', 40.0, 60.0, 10.0, '48 (BDS deaths at ages 1-5)',
     'BDS 2010-2019', 'Census BDS 2022 by firm age; author calc. Death rate age 1: 21.7%, ages 6-10: 8.1%, 26+: 4.9%'),
    ('entrant_exit_2y', 'entrants exiting within 2 years, %', 25.0, 40.0, 10.0, '34 (44% survive 4 years)',
     'BLS BED 1998 cohort', 'Knaup (2005 MLR 128(9)); BLS BED establishment age tables (5-yr survival ~50%)'),
    ('exit_size', 'employees per exiting firm', 4.5, 8.0, 1.5, '6.2 (2010s 5.8)',
     'BDS 1985-2019', 'Census BDS 2022: firmdeath_emp / firmdeath_firms; author calc'),
    ('death_jd', 'employment in exiting firms, % of employment/yr', 1.8, 3.0, 0.5, '2.5 (2010s 1.9)',
     'BDS 1985-2019', 'Census BDS 2022: firmdeath_emp(t) / emp(t-1); author calc'),
    ('firm_size', 'mean employer firm size, employees', 21.0, 26.0, 3.0, '24.7 (1990 21.0)',
     'BDS 2019', 'Census BDS 2022: emp/firms'),
    ('size_tail', 'Zipf tail exponent of firm size', 0.9, 1.1, 0.1, '1.06 (BDS 2019 counting-fn slope, 20-5000 emp: 0.99)',
     'Census 1997; BDS 2019', 'Axtell (2001 Science 293:1818); author calc from BDS size classes'),
    ('emp500', 'employment share of firms with 500+ employees, %', 46.0, 56.0, 4.0, '52.9 (1990 46.1)',
     'BDS 2019', 'Census BDS 2022 by firm size; SUSB similar'),
    ('jc_jd', 'gross job creation / destruction, %/yr (mean of JC and JD)', 13.0, 16.0, 1.5, 'JC 15.4, JD 14.0 (2010s 13.0 / 11.5)',
     'BDS 1985-2019', 'Census BDS 2022; Davis, Haltiwanger & Schuh (1996)'),
    ('startup_jc', 'startups share of gross job creation, %', 14.0, 20.0, 3.0, '16 (firms age 0-5: 32)',
     'BDS 1985-2019', 'Census BDS 2022; Haltiwanger, Jarmin & Miranda (2013 REStat)'),
    # --- markups, profits, pay ---
    ('markup', 'aggregate markup over unit cost, %', 10.0, 30.0, 5.0, '~10-25 cost-weighted; 61 sales-weighted P/MC (2016)',
     'Compustat 1980-2016', 'De Loecker, Eeckhout & Unger (2020 QJE): 1.21 (1980) -> 1.61 (2016) sales-weighted; '
     'cost-weighted about half the rise (Edmond, Midrigan & Xu 2023 JPE). Model markup is over average (normal) cost'),
    ('markup_disp', 'markup dispersion (P90/P50 of P/MC = 1+markup, all alive firms)', 1.25, 2.1, 0.15, 'P50 ~flat, P90 1.5 (1980) -> 2.5 (2016)',
     'Compustat 1980-2016', 'De Loecker, Eeckhout & Unger (2020); Eeckhout (2021) The Profit Paradox'),
    ('profit_share', 'after-tax profits, % of GDP', 6.0, 10.0, 2.0, '7.6 (1967-2019 7.1; 2010s ~9)',
     'NIPA 1985-2019', 'FRED CPATAX/GDP (corporate profits after tax with IVA & CCAdj); author calc'),
    ('ls_lag', 'labour share lags output: corr(ls(t+4q), GDP(t))', 0.2, 0.7, 0.2, '+0.43 (contemporaneous +0.06)',
     'NFB 1985-2019', 'FRED PRS85006173 vs GDPC1, HP(1600); author calc (1967-2019: +0.53). Rios-Rull & Santaeulalia-Llopis (2010 JME)'),
    # --- income-side profits and factor shares (metrics: corp_profits_bn_ar, bank_net_income_bn_ar, owner_*, markup_eff_*;
    # firm_profits_by_sector_bn_ar). Ported from the diag-profits study (9a64209); n/a on binaries without the metrics.
    ('ls_cyc', 'labour share, contemporaneous: corr(ls(t), GDP(t)), quarterly HP', -0.3, 0.15, 0.2,
     '+0.06 NFB (NFC -0.14; 1967-2019 NFB -0.10, NFC -0.07)',
     'NFB 1985-2019', 'FRED PRS85006173, PRS88003173 vs GDPC1, HP(1600) on logs; author calc. Rios-Rull & Santaeulalia-Llopis '
     '(2010 JME): mildly countercyclical on impact, overshooting later'),
    ('prof_vol', 'profit volatility: sd HP log real profits / sd HP log real GDP', 5.0, 8.5, 1.5,
     '7.2 (1967-2019 5.8; NFC after-tax 10.8)',
     'NIPA 1985-2019', 'FRED CPATAX/GDPDEF vs GDPC1, quarterly HP(1600); author calc. Model: after-tax profits of all firms '
     '(profit_share x nominal GDP), deflated by the GDP deflator'),
    ('prof_cyc', 'profits procyclical: corr(HP profits(t), HP GDP(t))', 0.1, 0.6, 0.15, '+0.12 (1967-2019 +0.46; NFC pretax +0.46)',
     'NIPA 1985-2019', 'FRED CPATAX/GDPDEF vs GDPC1, HP(1600); author calc'),
    ('prof_lead4', 'profits lead GDP: corr(HP profits(t), HP GDP(t+4q))', 0.3, 0.65, 0.15,
     '+0.45 (1967-2019 +0.51; NFC pretax +0.62); corr with GDP(t-4q) -0.47',
     'NIPA 1985-2019', 'FRED CPATAX/GDPDEF vs GDPC1, HP(1600); author calc. Profits peak 2-4 quarters before output'),
    ('corp_profit_share', 'corporate profits after tax (shareholder firms + banks after tax), % of GDP', 6.0, 10.0, 2.0,
     '7.6 (2010s 10.0; NFC after-tax/NFC GVA 8.6)',
     'NIPA 1985-2019', 'FRED CPATAX/GDP; author calc. Model: corp_profits_bn_ar + bank_net_income_bn_ar (owner-operated firms '
     'excluded: their profits are proprietors\' income in NIPA); n/a without those metrics'),
    ('corp_profit_drift', 'corporate profit share drift: years 16-20 minus years 2-6, pp', -1.0, 1.0, 0.5,
     'n/a (stationary profit share over the cycle of the US record)',
     'NIPA 1985-2019', 'FRED CPATAX/GDP; author calc. Model: mean of corp_profit_share over months 180-239 minus months 12-71'),
    ('energy_profit_gdp', 'Energy after-tax profit, % of GDP (utilities, petroleum, oil and gas extraction)', 0.6, 1.1, 0.25,
     'approximate: 0.7-1.0 (BEA NIPA Table 6.16D, energy industries)',
     'BEA NIPA 6.16D, approximate', 'approximate; the source note says "approximate". Model: firm_profits_by_sector_bn_ar[Energy] over GDP'),
    ('proprietors_nipa', "proprietors' income, NIPA concept (owner-firm profit + owner's own pay), % of GDP", 6.0, 8.0, 1.5, '6.9',
     'NIPA 1985-2019', 'FRED PROPINC/GDP; model: owner_firm_profits_bn_ar + owner_own_wage_bn_ar; n/a without those metrics'),
    ('markup_disp_eff', 'markup dispersion on realised P/AC (P90/P50 of price / unit cost)', 1.25, 2.1, 0.15,
     'P50 ~flat, P90 1.5 (1980) -> 2.5 (2016)',
     'Compustat 1980-2016', 'De Loecker, Eeckhout & Unger (2020). Model: markup_eff_p* (price over current unit cost) instead '
     'of the markup state markup_p*; n/a without those metrics'),
    ('inv_inaction', 'investment inaction (|i/k|<1%) share of plant-years, %', 5.0, 11.0, 2.0, '8.1 (negative investment 10.4%)',
     'LRD 1972-1988', 'Cooper & Haltiwanger (2006 REStud) Table 1. Model: investment_inaction_share (annual, January)'),
    ('proprietors', "proprietors' income, % of GDP", 6.0, 8.0, 1.5, '6.9',
     'NIPA 1985-2019', 'FRED PROPINC/GDP; author calc (NIPA includes the owner\'s labour income)'),
    ('pay_premia', 'between-employer share of log-wage variance (raw, no worker FE), %', 30.0, 40.0, 5.0,
     '30-40 (AKM firm-effect share 8.7-11.9)',
     'SSA 1980-2013', 'Song, Price, Guvenen, Bloom & von Wachter (2019 QJE), AKM decomposition table. The model measures the '
     'raw between-employer share, which is not the AKM firm-effect share; compare with the between-firm benchmark only'),
    # --- investment and utilisation ---
    ('inv_spikes', 'investment spikes (i/k>20%) share of plant-years, %', 14.0, 22.0, 4.0, '18 (inaction |i/k|<1%: 8.1%; mean i/k 12.2%)',
     'LRD 1972-1988', 'Cooper & Haltiwanger (2006 REStud) Table 1; spikes ~ half of investment'),
    ('cu_level', 'capacity utilisation, %', 77.0, 81.5, 2.0, '79.3 (1967-2019 80.3; manufacturing 77.9)',
     'G.17 1985-2019', 'FRED TCU (Federal Reserve G.17); author calc'),
    ('cu_cyc', 'corr(capacity utilisation, GDP), quarterly HP', 0.65, 0.92, 0.15, '+0.79 (1967-2019 +0.86); sd 2.0pp',
     'G.17 1985-2019', 'FRED TCU vs GDPC1; author calc'),
    # --- worker flows (JOLTS, CPS) ---
    ('hires', 'hires rate, %/month', 3.3, 4.0, 0.5, '3.62', 'JOLTS 2001-2019', 'FRED JTSHIR; author calc'),
    ('layoffs', 'layoffs & discharges, %/month (model: layoffs + exogenous E->U + bankruptcy displacement)', 1.2, 1.6, 0.3, '1.39',
     'JOLTS 2001-2019', 'FRED JTSLDR; author calc'),
    ('quits', 'quits + other separations, %/month', 1.9, 2.5, 0.5, '2.16 (quits 1.91); EE moves 2.6%/month (CPS)',
     'JOLTS 2001-2019', 'FRED JTSTSR - JTSLDR; Fallick & Fleischman (2004 FEDS 2004-34)'),
    ('hires_cyc', 'corr(hires rate, GDP), quarterly HP', 0.6, 0.95, 0.3, '+0.86', 'JOLTS 2001-2019', 'FRED JTSHIR vs GDPC1; author calc'),
    ('layoffs_cyc', 'corr(layoffs rate, GDP), quarterly HP', -0.8, -0.35, 0.3, '-0.58', 'JOLTS 2001-2019', 'FRED JTSLDR vs GDPC1; author calc'),
    ('quits_cyc', 'corr(quits + other, GDP), quarterly HP', 0.6, 0.95, 0.3, '+0.87 (quits +0.88)', 'JOLTS 2001-2019', 'FRED JTSTSR-JTSLDR; author calc'),
    ('openings', 'job openings rate, %', 2.7, 3.6, 0.5, '3.13', 'JOLTS 2001-2019', 'FRED JTSJOR; author calc'),
    ('beveridge', 'Beveridge: corr(log v, log u), quarterly HP', -0.97, -0.85, 0.1, '-0.93 (Shimer: -0.89 1951-2003)',
     'JOLTS 2001-2019', 'FRED JTSJOL, UNEMPLOY; Shimer (2005 AER) Table 1'),
    ('vu_sd', 'sd log(v/u), quarterly HP(1e5)', 0.30, 0.45, 0.1, '0.39 (Shimer: 0.38 1951-2003)',
     'JOLTS 2001-2019', 'FRED JTSJOL/UNEMPLOY; Shimer (2005 AER) Table 1'),
    ('vu_prod', 'sd log(v/u) / sd log labour productivity, HP(1e5)', 15.0, 30.0, 5.0, '25 (Shimer: 19 1951-2003)',
     'JOLTS/NFB 2001-2019', 'FRED OPHNFB; Shimer (2005 AER): the unemployment volatility puzzle'),
    ('inflow_share', 'inflow (separation) share of unemployment variance', 0.25, 0.55, 0.15, '0.46 (Shimer 2012: ~0.25 post-1987)',
     'CPS flows 1990-2019', 'FRED LNS17100000, LNS17400000; steady-state decomposition; Fujita & Ramey (2009 IER) 0.4-0.5'),
    ('match_elast', 'elasticity of job-finding to v/u (1 - matching elasticity wrt u)', 0.25, 0.5, 0.1, '0.37 (first differences 0.28)',
     'CPS/JOLTS 2001-2019', 'author calc; Shimer (2005) 0.28; Petrongolo & Pissarides (2001 JEL) u-elasticity 0.5-0.7'),
    # --- inventories ---
    ('inv_cyc', 'corr(change in inventories / GDP, GDP), quarterly HP', 0.35, 0.7, 0.15, '+0.52 (1967-2019 +0.57)',
     'NIPA 1985-2019', 'FRED CBIC1, GDPC1; Blinder & Maccini (1991)'),
    ('prod_sales', 'sd(GDP) / sd(final sales), quarterly HP', 1.05, 1.25, 0.1, '1.15 (1967-2019 1.18)',
     'NIPA 1985-2019', 'FRED GDPC1, FINSLC1; Ramey & West (1999)'),
    # --- b5-firmhet firm-distribution rows (S0). Metrics fields added in S0; these read n/a on older binaries. ---
    ('markup_disp_avc', 'markup dispersion on variable cost (P90/P50 of P/VC = 1+markup, P VC = price / (unit cost - capital cost))',
     1.25, 2.1, 0.15, 'P50 ~flat, P90 1.5 (1980) -> 2.5 (2016)', 'Compustat 1980-2016',
     'De Loecker, Eeckhout & Unger (2020), sales/COGS concept. Model: markup_avc_p* (capital cost at normal utilisation, '
     'target-return term excluded)'),
    ('top1_share', 'employment share of the largest 1% of firms, %', 30.0, 52.0, 5.0, 'finite-sample p25-p75 at N ~ 760: 32-52',
     'Census SUSB 2019; finite-sample', 'scripts/finite_size_benchmark.py (Pareto tail above 500 calibrated to 52.9% emp500). '
     'Model: firm_emp_top1_share'),
    ('emp500_fs', 'employment share of firms with 500+ employees, finite-sample band, %', 13.0, 43.0, 5.0,
     'finite-sample p25-p75 at N ~ 760: 13-43 (emp500 kept as a reported row)', 'Census SUSB 2019; finite-sample',
     'scripts/finite_size_benchmark.py. Model value = emp500 (firm_share_500plus) tested against the finite-sample band'),
    ('lp_disp_9010', 'within-sector 90-10 range of log labour productivity (pooled, sector-demeaned)', 0.65, 1.4, 0.2,
     'log TFP 90-10 about 0.65 (Syverson); labour-productivity ratios are wider', 'Syverson (2004 REStat; 2011 JEL)',
     'Model: firm_lp_p90_p10 (labour productivity, not TFP)'),
    ('hhi_drift', 'concentration drift: mean over sectors of revenue HHI, years 16-20 minus years 2-6', 0.0, 0.03, 0.03,
     'concentration rises slowly', 'Autor et al. (2020 QJE)',
     'Model: firm_hhi_by_sector. Run months 24-83 vs 192-239 (the run is 240 months, so years 16-20 are truncated to 16-19)'),
]

# ------------------------------------------------------------------------------------------------------------------
def run(seed):
    if CACHE:
        p = os.path.join(CACHE, f'run_{seed}.json')
        if os.path.exists(p):
            return json.load(open(p))
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', str(HH), str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    h = json.loads(out.strip().splitlines()[-1])
    if CACHE:
        os.makedirs(CACHE, exist_ok=True)
        json.dump(h, open(os.path.join(CACHE, f'run_{seed}.json'), 'w'))
    return h


def hp(y, lam=1600.0):
    y = np.asarray(y, float); n = len(y); I = np.eye(n); D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)


def qa(v):
    v = np.asarray(v, float); return v[: len(v) // 3 * 3].reshape(-1, 3).mean(1)


def corr(a, b):
    return float(np.corrcoef(a, b)[0, 1])


def series(h, k):
    # Keys added by metrics-out are missing from older binaries: those give NaN (reported as n/a), not an error.
    return np.array([m.get(k, np.nan) for m in h], float)


def model_moments(H):
    R = {k: [] for k, *_ in T}
    for h in H:
        fa, br = series(h, 'firms_alive'), series(h, 'bankruptcies_ar') / 12.0
        gdp = series(h, 'gdp_real_bn_ar'); gnom = series(h, 'gdp_nominal_bn_ar')
        u = series(h, 'unemployment_rate'); emp = series(h, 'employment_millions')
        empp = series(h, 'employment_persons_millions')
        hires, lay = series(h, 'hires_rate'), series(h, 'layoffs_rate')
        # --- annual firm entry/exit (12-month windows after the first year) ---
        # Year y runs from month a to b = a + 12; exits are bankruptcies in months a+1..b, entries the rest of the
        # change in the firm count (dead slots are reborn after 3 months, plus growth entry).
        ny = (len(h) - SKIP - 1) // 12
        ex, en, gy = [], [], []
        for y in range(ny):
            a = SKIP + 12 * y; b = a + 12
            exits = br[a + 1:b + 1].sum()
            ex.append(100 * exits / fa[a])
            en.append(100 * (fa[b] - fa[a] + exits) / fa[a])
            gy.append(np.log(gdp[b - 2:b + 1].mean()))
        ex, en, gy = np.array(ex), np.array(en), np.array(gy)
        R['exit_rate'].append(ex.mean()); R['entry_rate'].append(en.mean())
        R['exit_drift'].append(ex[-5:].mean() - ex[:5].mean())
        gc = hp(gy, 6.25)
        R['entry_cyc'].append(corr(hp(en, 6.25), gc)); R['exit_cyc'].append(corr(hp(ex, 6.25), gc))
        R['firm_size'].append(np.mean(empp[SKIP:] * 1e6 / fa[SKIP:]))
        # --- markups, profits, pay ---
        R['markup'].append(series(h, 'avg_markup')[SKIP:].mean())
        R['profit_share'].append(series(h, 'profit_share')[SKIP:].mean())
        R['proprietors'].append(np.mean(100 * series(h, 'proprietors_income_bn_ar')[SKIP:] / gnom[SKIP:]))
        hs = h[SKIP:]
        # --- firm size, job flows, markups and investment (metrics-out fields; annual flows change each January) ---
        R['size_tail'].append(series(hs, 'firm_size_tail_exponent').mean())
        R['emp500'].append(series(hs, 'firm_share_500plus').mean())
        ann = list(range(SKIP, len(h), 12))
        R['inv_inaction'].append(np.nanmean([h[i].get('investment_inaction_share', np.nan) for i in ann]))
        nan3 = [np.nan] * 3
        jc = np.array([h[i].get('job_creation_by_age', nan3) for i in ann], float)
        jd = np.array([h[i].get('job_destruction_by_age', nan3) for i in ann], float)
        R['jc_jd'].append(0.5 * (jc.sum(1).mean() + jd.sum(1).mean()))
        # Startup share: job creation by firms born in the 12 months before each January (BDS age 0), over total.
        ent = np.array([h[i].get('job_creation_entrants_pct', np.nan) for i in ann], float)
        R['startup_jc'].append(100 * ent.mean() / jc.sum(1).mean())
        # markup_p* are 100 x the markup rate; P/MC = 1 + markup, so the dispersion ratio is on P/MC levels.
        R['markup_disp'].append(np.mean((100 + series(hs, 'markup_p90')) / (100 + series(hs, 'markup_p50'))))
        # Investment rates are annual (12-month order sums over capital), read at each January.
        R['inv_spikes'].append(np.nanmean([h[i].get('investment_spike_share', np.nan) for i in ann]))
        # Realised pay: raw between-employer share of the log-wage variance of employed heads (annual, January).
        R['pay_premia'].append(np.nanmean([h[i].get('wage_between_firm_share', np.nan) for i in ann]))
        # Firm exits by age and bankruptcy displacement (monthly counts, summed over the years after the first).
        ex = np.array([m.get('firm_exits_by_age_ar', [np.nan] * 5) for m in hs], float) / 12.0
        ex_tot = ex.sum()
        ent_m = series(hs, 'firm_entries_ar').sum() / 12.0
        disp = series(hs, 'bankruptcy_displaced_heads_ar').sum() / 12.0
        R['exit_young_share'].append(100 * ex[:, :3].sum() / ex_tot)        # exits under 5 years of age
        R['entrant_exit_2y'].append(100 * ex[:, :2].sum() / ent_m)          # exits under 2 years per entrant
        R['exit_size'].append(PERSONS_PER_EMPLOYED_HEAD * disp / ex_tot)    # persons per exiting firm
        R['death_jd'].append(12 * series(hs, 'flow_bankruptcy_displaced_pct').mean())  # % of employment per year
        yq = hp(np.log(qa(series(hs, 'gdp_real_bn_ar'))))
        wshare = qa(series(hs, 'avg_wage_annual') * series(hs, 'employment_millions') / 1e3 / series(hs, 'gdp_nominal_bn_ar'))
        lc = hp(np.log(wshare)); k = 4
        R['ls_lag'].append(corr(lc[k:], yq[:-k]))
        # Income-side rows (9a64209): contemporaneous labour share; profit volatility, cyclicality and lead (after-tax
        # profits of all firms, deflated by the GDP deflator); NIPA-consistent levels when the metrics exist.
        R['ls_cyc'].append(corr(lc, yq))
        gnq = qa(series(hs, 'gdp_nominal_bn_ar')); grq = qa(series(hs, 'gdp_real_bn_ar'))
        profq = qa(series(hs, 'profit_share') / 100.0 * series(hs, 'gdp_nominal_bn_ar')) / (gnq / grq)
        pc = hp(np.log(np.maximum(profq, 1e-9)))
        R['prof_vol'].append(pc.std() / yq.std()); R['prof_cyc'].append(corr(pc, yq))
        R['prof_lead4'].append(corr(pc[:-4], yq[4:]))
        # Level rows use months 12-239 (hs), as every other row; the drift row keeps absolute month indices.
        gn_all = series(h, 'gdp_nominal_bn_ar')
        cps_all = 100 * (series(h, 'corp_profits_bn_ar') + series(h, 'bank_net_income_bn_ar')) / gn_all
        gn = series(hs, 'gdp_nominal_bn_ar')
        cps = 100 * (series(hs, 'corp_profits_bn_ar') + series(hs, 'bank_net_income_bn_ar')) / gn
        R['corp_profit_share'].append(np.mean(cps))
        R['corp_profit_drift'].append(cps_all[180:240].mean() - cps_all[12:72].mean())
        R['energy_profit_gdp'].append(np.mean(100 * np.array([m.get('firm_profits_by_sector_bn_ar', [np.nan] * 9)[0]
                                                            for m in hs], float) / gn))
        R['proprietors_nipa'].append(np.mean(100 * (series(hs, 'owner_firm_profits_bn_ar') + series(hs, 'owner_own_wage_bn_ar')) / gn))
        R['markup_disp_eff'].append(np.mean((100 + series(hs, 'markup_eff_p90')) / (100 + series(hs, 'markup_eff_p50'))))
        # --- utilisation ---
        cu = series(hs, 'capacity_utilization')
        R['cu_level'].append(cu.mean()); R['cu_cyc'].append(corr(hp(qa(cu)), yq))
        # --- worker flows ---
        hr, ly = hires[SKIP:], lay[SKIP:]
        # Bankruptcy displacement (employed heads whose employer exited this month), % of employed heads at the
        # previous record: a JOLTS-style layoff and discharge, so it sits in the layoffs row, not in the residual.
        dis = series(hs, 'flow_bankruptcy_displaced_pct')
        e = emp[SKIP - 1:]
        demp = 100 * np.diff(np.log(e))            # monthly employment growth, %
        seps = hr - demp                           # implied total separations, % of employment
        qo = seps - ly - SEP_RATE - dis            # job-to-job moves, retirements, deaths
        R['hires'].append(hr.mean()); R['layoffs'].append((ly + SEP_RATE + dis).mean()); R['quits'].append(qo.mean())
        R['hires_cyc'].append(corr(hp(qa(hr)), yq)); R['layoffs_cyc'].append(corr(hp(qa(ly)), yq))
        R['quits_cyc'].append(corr(hp(qa(qo)), yq))
        R['openings'].append(series(hs, 'vacancy_rate').mean())
        uu = series(hs, 'unemployment_rate') / 100
        U = series(hs, 'employment_millions') * uu / (1 - uu)
        V = series(hs, 'vacancies_millions')
        lv, lu = np.log(qa(V)), np.log(qa(U))
        R['beveridge'].append(corr(hp(lv), hp(lu)))
        th = hp(lv, 1e5) - hp(lu, 1e5)
        prod = hp(np.log(qa(series(hs, 'gdp_real_bn_ar') / series(hs, 'employment_persons_millions'))), 1e5)
        R['vu_sd'].append(th.std()); R['vu_prod'].append(th.std() / prod.std())
        # Steady-state flow proxy: E->U inflow s = layoffs + exogenous separations (incl. bankruptcy displacement, which
        # moves workers to unemployment); job finding f = s(1-u)/u.
        s = qa((ly + SEP_RATE + dis) / 100); uq = qa(uu); f = s * (1 - uq) / uq
        lus = hp(np.log(s / (s + f))); us_s = hp(np.log(s / (s + f.mean())))
        R['inflow_share'].append(np.cov(us_s, lus)[0, 1] / lus.var(ddof=1))
        R['match_elast'].append(np.polyfit(th, hp(np.log(f), 1e5), 1)[0])
        # --- inventories ---
        gq = qa(series(hs, 'gdp_real_bn_ar')); di = qa(series(hs, 'inventory_change_real_bn_ar'))
        R['inv_cyc'].append(corr(hp(di / gq.mean() * 100), yq))
        R['prod_sales'].append(yq.std() / hp(np.log(gq - di)).std())
        # --- b5-firmhet firm-distribution rows (S0 fields; NaN on older binaries) ---
        R['markup_disp_avc'].append(np.mean((100 + series(hs, 'markup_avc_p90')) / (100 + series(hs, 'markup_avc_p50'))))
        R['top1_share'].append(series(hs, 'firm_emp_top1_share').mean())
        R['emp500_fs'].append(series(hs, 'firm_share_500plus').mean())
        R['lp_disp_9010'].append(series(hs, 'firm_lp_p90_p10').mean())
        hhi = np.array([m.get('firm_hhi_by_sector', [np.nan] * 9) for m in h], float)
        R['hhi_drift'].append(np.nanmean(hhi[192:240].mean(0) - hhi[24:84].mean(0)))
    return R


def flag(v, lo, hi, tol):
    if tol is None or v is None or not np.isfinite(v):
        return 'unmeasurable'
    d = max(lo - v, v - hi, 0.0) / tol
    return 'ok' if d == 0 else 'minor' if d <= 1 else 'material' if d <= 3 else 'severe'


def main():
    t0 = time.time()
    with ThreadPoolExecutor(min(JOBS, SEEDS)) as ex:
        H = list(ex.map(run, range(1, SEEDS + 1)))
    wall = time.time() - t0
    R = model_moments(H)
    model, note = {}, {}
    for k, *_ in T:
        if R[k]:
            a = np.array(R[k], float); model[k] = float(np.nanmean(a)); note[k] = f'{model[k]:.2f} (sd {np.nanstd(a):.2f})'
    # Firm exits by age, entrant early exit, exit size and bankruptcy displacement come from the metrics-out fields
    # (summed over all seeds' months after the first year). Pay premia is the raw between-employer share.
    note['pay_premia'] = note.get('pay_premia', 'n/a') + ' (raw between-employer share, no worker FE)'
    note['layoffs'] = note['layoffs'] + f" [planned layoffs alone {np.mean([series(h, 'layoffs_rate')[SKIP:].mean() for h in H]):.2f}]"

    print(f"Firm dynamics & labour-market structure: {SEEDS} seeds x {MONTHS} months x {HH} households "
          f"(first year dropped); wall {wall:.0f}s")
    print(f"{'fact':58s} | {'US target':44s} | {'model':40s} | gap")
    print('-' * 160)
    counts = {}
    for k, label, lo, hi, tol, txt, sample, src in T:
        g = flag(model.get(k), lo, hi, tol)
        counts[g] = counts.get(g, 0) + 1
        print(f"{label[:58]:58s} | {(txt + ' [' + sample + ']')[:44]:44s} | {note.get(k, 'n/a')[:40]:40s} | {g}")
    print('-' * 160)
    print('flags: ' + ', '.join(f'{k} {v}' for k, v in sorted(counts.items())))
    print('long model notes:')
    for k in ('layoffs',):
        if k in note:
            print(f'  {k}: {note[k]}')
    out = os.environ.get('FACTS_TARGETS_OUT')
    if out:
        js = [dict(key=k, fact=label, target=txt, range=[lo, hi], tolerance=tol, sample=sample, source=src,
                   model_master=note.get(k, 'n/a'), gap=flag(model.get(k), lo, hi, tol))
              for k, label, lo, hi, tol, txt, sample, src in T]
        with open(out, 'w') as fh:
            json.dump({'dimension': 'firm dynamics and labour-market structure', 'cli': CLI,
                       'runs': f'{SEEDS} seeds x {MONTHS} months x {HH} households', 'wall_s': round(wall),
                       'facts': js}, fh, indent=1)
        print(f'targets written to {out}')


if __name__ == '__main__':
    main()
