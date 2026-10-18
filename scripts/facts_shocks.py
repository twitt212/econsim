#!/usr/bin/env python3
"""Exogenous and price processes: oil, gasoline, energy/food CPI, inflation-expectation surprises, the dollar, house
prices and productivity, measured on baseline runs with the same estimators as the US data targets.

Usage
  python3 scripts/facts_shocks.py [path/to/econsim-cli] [--targets-json PATH]
  ECONSIM_CLI=... python3 scripts/facts_shocks.py

The CLI path is argv[1], else $ECONSIM_CLI, else ./target/release/econsim-cli. ECONSIM_CFG is passed through to every
run. Environment knobs (defaults in brackets):
  SHOCKS_HH [20000], SHOCKS_SEEDS [8], SHOCKS_MONTHS [240]      main sample (one 228-month window per seed)
  SHOCKS_LONG_HH [10000], SHOCKS_LONG_SEEDS [4], SHOCKS_LONG_MONTHS [480]   long sample (240-month windows, step 114)
  SHOCKS_JOBS [8]   parallel runs (each with RAYON_NUM_THREADS=1)
  SHOCKS_CACHE      directory to cache run JSON (keyed by CLI, ECONSIM_CFG, households, months, seed)
--targets-json PATH writes the targets with sources (no runs needed when it is the only work: add --no-run).

Method. The data targets were computed from FRED/BLS/BIS/S&P CoreLogic/SF Fed series with exactly the estimators
below, over 20-year (240-month) windows stepping 5 years (the 'range' is the min..max across windows; 'target' the
window mean unless noted), so they are comparable with a 20-year model run. The model value is the mean over the
main-sample runs (first 12 months after the burn-in dropped); 'long' repeats it on 240-month windows of the long
runs; 'seeds' is the min..max across main-sample runs.

Gap flag: ok inside the target range; otherwise d = distance to the nearest bound / the fact's tolerance 'tol':
minor d < 1, material 1 <= d < 3, severe d >= 3. 'n/a' when the model does not report the series.
"""
import hashlib
import json
import os
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
BURN = 12

# ----------------------------------------------------------------------------------------------------------------
# Targets (US data). Each: id, group, name, target, lo, hi, tol, unit, sample, source.
# ----------------------------------------------------------------------------------------------------------------
TARGETS = [
    # ---- oil ----
    dict(id='oil_vol', group='oil', name='sd monthly dln real oil price, %', target=9.2, lo=8.0, hi=10.5, tol=1.0,
         sample='WTI monthly average (FRED MCOILWTICO) / CPI-U, 1986:01-2026:08, 20y windows; 1986-2019 8.5',
         source='EIA/FRED MCOILWTICO, BLS CPIAUCSL (computed); Baumeister & Kilian (2016) JEP 30(1); Alquist, Kilian & Vigfusson (2013) Handbook of Econ. Forecasting 2A'),
    dict(id='oil_kurt', group='oil', name='excess kurtosis of monthly dln real oil', target=4.6, lo=1.2, hi=8.6, tol=1.0,
         sample='WTI 1986-2026 20y windows (full sample 7.3; 1986-2019 2.5; spot 1974-2026 8.8)',
         source='FRED MCOILWTICO, WTISPLC (computed); jumps 1986, 1990, 2008, 2014-16, 2020, 2022 (Baumeister & Kilian 2016; Hamilton 2009 Brookings PEA)'),
    dict(id='oil_vr12', group='oil', name='oil variance ratio VR(12) = var(12m dln)/(12 var(1m dln))', target=1.10, lo=0.86, hi=1.42, tol=0.1,
         sample='WTI real 1986-2026 20y windows (1986-2019 1.05); 1 = random walk',
         source='FRED MCOILWTICO (computed); real oil price close to a random walk / no-change forecast hard to beat (Hamilton 2009; Alquist, Kilian & Vigfusson 2013)'),
    dict(id='oil_big', group='oil', name='share of months with |12m dln real oil| > 0.5', target=0.14, lo=0.05, hi=0.20, tol=0.02,
         sample='WTI real 1986-2026 20y windows (full 0.12); episodes 1986, 1990-91, 1998-99, 2008-09, 2014-16, 2020-21, 2022',
         source='FRED MCOILWTICO (computed); Baumeister & Kilian (2016); Kilian (2009) AER 99(3)'),
    dict(id='oil_lvl', group='oil', name='sd of ln real oil level within a 20-year window', target=0.40, lo=0.27, hi=0.52, tol=0.05,
         sample='WTI real 1986-2026 20y windows (full 0.45): $20-$140 swings',
         source='FRED MCOILWTICO / CPIAUCSL (computed); Hamilton (2009)'),
    dict(id='oil_drift', group='oil', name='mean drift of ln real oil, %/yr', target=0.4, lo=-1.0, hi=2.0, tol=1.0,
         sample='1986-2026 0.46 (monthly avg), 0.45 (spot); 1974-2026 0.29; single 20y windows -3.1..+3.9, the model mean over 8 runs should be near the long-run drift',
         source='FRED MCOILWTICO, WTISPLC / CPIAUCSL (computed); no deterministic trend in the real oil price (Hamilton 2009; Baumeister & Kilian 2016)'),
    # ---- gasoline ----
    dict(id='gas_pt', group='gasoline', name='gasoline pass-through, $/gal per $/bbl (dgas on doil, lags 0+1)', target=0.0287, lo=0.0235, hi=0.0290, tol=0.004,
         sample='BLS avg retail regular (APU000074714) on WTI, monthly differences, 20y windows 1990-2026 0.028-0.029; 1990-2004 0.0235',
         source='FRED APU000074714, MCOILWTICO (computed); EIA Today in Energy 6850 (2012): $1/bbl = 2.4 c/gal, half within 2 weeks, 80% within 4 weeks; Borenstein, Cameron & Gilbert (1997) QJE 112(1)'),
    dict(id='gas_elast', group='gasoline', name='gasoline elasticity to oil (dln gas on dln oil, lags 0+1)', target=0.60, lo=0.44, hi=0.72, tol=0.05,
         sample='APU000074714 on WTI, 20y windows 1990-2026 0.57-0.72; 2010-2026 0.49; 1990-2004 0.44',
         source='FRED APU000074714, MCOILWTICO (computed); EIA (2019, Today in Energy 41453): crude ≈53% of the retail price'),
    # ---- CPI components ----
    dict(id='energy_vol', group='cpi', name='sd of 12m energy CPI inflation, pp', target=11.4, lo=7.9, hi=12.9, tol=1.5,
         sample='CPI energy (CPIENGSL) 1986-2026 20y windows (1986-2019 10.2)',
         source='BLS via FRED CPIENGSL (computed)'),
    dict(id='energy_beta', group='cpi', name='energy CPI elasticity to oil (dln on dln WTI, lags 0+1)', target=0.28, lo=0.22, hi=0.33, tol=0.05,
         sample='CPIENGSL on MCOILWTICO 1986-2026 20y windows (full 0.25)',
         source='FRED CPIENGSL, MCOILWTICO (computed); energy CPI = motor fuel ≈half + electricity and utility gas (BLS relative importance); Kilian (2008) JEL 46(4)'),
    dict(id='food_vol', group='cpi', name='sd of 12m food CPI inflation, pp', target=1.5, lo=1.2, hi=2.3, tol=0.3,
         sample='CPI food (CPIUFDSL) 1986-2026 20y windows (1986-2019 1.45; 1957-2026 3.0)',
         source='BLS via FRED CPIUFDSL (computed)'),
    # ---- expectations ----
    dict(id='exp_err', group='expectations', name='sd of 12m-ahead household inflation forecast error, pp', target=1.4, lo=0.93, hi=1.54, tol=0.3,
         sample='realised CPI-U 12m change from t to t+12 minus Michigan 1y expectation (MICH) at t, 1986-2026 20y windows; 1986-2019 1.39 (mean error -0.46)',
         source='U. Michigan Surveys of Consumers via FRED MICH, CPIAUCSL (computed); Souleles (2004) JMCB 36(1); Ang, Bekaert & Wei (2007) JME 54(4)'),
    dict(id='exp_gas', group='expectations', name='corr(12m dln gasoline, 12m change in household expected inflation)', target=0.68, lo=0.57, hi=0.78, tol=0.1,
         sample='APU000074714 and MICH 1986-2026 20y windows (full 0.56; slope 0.25pp per 10% gas)',
         source='FRED APU000074714, MICH (computed); Coibion & Gorodnichenko (2015) AEJ:Macro 7(1); Binder (2018) J. Macroecon. 58; Kilian & Zhou (2022) JAE 37(5)'),
    # ---- dollar ----
    dict(id='fx_vol', group='dollar', name='sd monthly dln dollar index, %', target=1.45, lo=1.20, hi=1.80, tol=0.2,
         sample='real broad BIS (RBUSBIS) 1994-2026 1.22; Fed real major currencies (TWEXMPA) 1973-2019 1.70; nominal broad 2006-2026 1.27 (annualised 4.2-5.8%)',
         source='BIS/Fed via FRED RBUSBIS, TWEXMPA, TWEXBGSMTH (computed); Meese & Rogoff (1983) JIE 14; Engel & West (2005) JPE 113(3)'),
    dict(id='fx_lvl', group='dollar', name='sd of ln dollar level within a 20-year window', target=0.11, lo=0.087, hi=0.132, tol=0.02,
         sample='TWEXMPA 1973-2019 20y windows 0.098-0.132; RBUSBIS 1994-2026 0.087 (swings 1980-85 +50%, 1995-2002 +35%, 2011-22 +30%)',
         source='FRED TWEXMPA, RBUSBIS (computed); PPP half-life 3-5 years (Rogoff 1996 JEL 34(2))'),
    dict(id='fx_disc', group='dollar', name='corr(monthly dln dollar, monthly change in 2y yield)', target=0.18, lo=0.09, hi=0.33, tol=0.1,
         sample='TWEXMPA vs GS2 1976-2019 0.18-0.33; RBUSBIS 1994-2026 0.09-0.14',
         source='FRED GS2, TWEXMPA, RBUSBIS (computed); exchange-rate disconnect (Meese & Rogoff 1983; Itskhoki & Mukhin 2021 JPE 129(8))'),
    # ---- house prices ----
    dict(id='hpi_mom', group='housing', name='corr(12m real HPI growth, same 12 months earlier) - momentum', target=0.75, lo=0.60, hi=0.80, tol=0.1,
         sample='S&P CoreLogic Case-Shiller national (CSUSHPISA)/CPI 1987-2026 20y windows 0.70-0.80; annual-average growth AR1 0.72; FHFA 1975-2026 0.62',
         source='FRED CSUSHPISA, USSTHPI (computed); Case & Shiller (1989) AER 79(1); Cutler, Poterba & Summers (1991) AER P&P; Glaeser, Gyourko, Morales & Nathanson (2014) JUE 81'),
    dict(id='hpi_vol', group='housing', name='sd of 12m real HPI growth, %', target=5.9, lo=4.2, hi=6.7, tol=1.0,
         sample='Case-Shiller national real 1987-2026 20y windows (full 5.4); FHFA all-transactions 1975-2026 4.0',
         source='FRED CSUSHPISA, USSTHPI, CPIAUCSL (computed); Shiller (2015) Irrational Exuberance, 3rd ed., long-run real HPI'),
    dict(id='hpi_cycle', group='housing', name='sd of linearly detrended ln real HPI within 20 years (long cycles)', target=0.126, lo=0.116, hi=0.138, tol=0.03,
         sample='Case-Shiller national real 1987-2026 20y windows; FHFA 1975-2026 0.03-0.12 (1989-97 -15%, 2006-12 -35% real)',
         source='FRED CSUSHPISA (computed); Glaeser et al. (2014): mean reversion over 5-year horizons'),
    dict(id='ptr_cycle', group='housing', name='sd of linearly detrended ln(price/rent) within 20 years', target=0.116, lo=0.098, hi=0.132, tol=0.03,
         sample='Case-Shiller national / CPI rent of primary residence (CUSR0000SEHA) 1987-2026 20y windows',
         source='FRED CSUSHPISA, CUSR0000SEHA (computed); Davis, Lehnert & Martin (2008) RIW 54(2); Campbell, Davis, Gallin & Martin (2009) JUE 66(2)'),
    dict(id='pti', group='housing', name='price-to-income (median owner-occupied value / median household income)', target=3.9, lo=3.5, hi=5.0, tol=0.5,
         sample='ACS 2019-23: $303,400 / $78,538 = 3.9 (stock values); JCHS sales-price basis 3.2 (1990s avg), 4.1 (2019), 5.0 (2024); MSPUS new homes / median income 1984-2025 3.6-5.8',
         source='Census ACS 5-year 2019-2023 (B25077, B19013); Harvard JCHS State of the Nation\'s Housing 2025; FRED MSPUS, MEHOINUSA646N'),
    dict(id='sales_gdp', group='housing', name='corr(HP home sales_t, HP real GDP_t+2q), quarterly - sales lead', target=0.60, lo=0.40, hi=0.69, tol=0.15,
         sample='new single-family home sales (HSN1F) vs GDPC1, HP 1600: 1984-2019 0.60, 1963-2019 0.69, 1984-2026 0.40; peak at a 2-3 quarter lead; contemporaneous 0.2-0.4',
         source='Census/HUD via FRED HSN1F, BEA GDPC1 (computed); Leamer (2007) Jackson Hole "Housing IS the business cycle"; Leamer (2015) JMCB 47(S1)'),
    dict(id='sales_price', group='housing', name='corr(12m dln home sales_t, 12m real HPI growth_t+12) - volume leads price', target=0.63, lo=0.30, hi=0.79, tol=0.15,
         sample='HSN1F vs Case-Shiller real 1987-2026 (20y windows 0.30-0.79); contemporaneous 0.44',
         source='FRED HSN1F, CSUSHPISA (computed); Stein (1995) QJE 110(2); Genesove & Mayer (2001) QJE 116(4); Clayton, Miller & Peng (2010) JREFE 40(1)'),
    # ---- productivity ----
    dict(id='lp_vol', group='productivity', name='sd quarterly labour productivity growth (GDP/employed), % annualised', target=2.0, lo=1.8, hi=2.8, tol=0.4,
         sample='GDPC1 / CPS employment (CE16OV) 1984-2019 2.0 (payroll 1.8); Fernald utilisation-adjusted TFP 1984-2019 2.75 (1948-2025 3.3), AR1 ≈ 0',
         source='FRED GDPC1, CE16OV, PAYEMS (computed); Fernald (2014) FRBSF WP 2012-19, data vintage 2026-10-01'),
    dict(id='lp_cyc', group='productivity', name='corr(HP labour productivity, HP real GDP), quarterly', target=0.45, lo=0.23, hi=0.50, tol=0.15,
         sample='GDP/CPS employment 1984-2019 0.50 (20y windows 0.44-0.73), GDP/payroll 0.23; pre-1984 0.83',
         source='FRED GDPC1, CE16OV, PAYEMS (computed); Gali & van Rens (2021) EJ 131(633): the vanishing procyclicality of labour productivity'),
]


# ----------------------------------------------------------------------------------------------------------------
# Estimators (identical for data and model)
# ----------------------------------------------------------------------------------------------------------------
def ac1(x):
    x = np.asarray(x, float)
    return float(np.corrcoef(x[1:], x[:-1])[0, 1])


def kurt_ex(x):
    x = np.asarray(x, float) - np.mean(x)
    return float((x ** 4).mean() / (x ** 2).mean() ** 2 - 3.0)


def vr(lx, k):
    return float((lx[k:] - lx[:-k]).var() / (k * np.diff(lx).var()))


def g(lx, k):
    return lx[k:] - lx[:-k]


def lag01(y, x):
    """sum of OLS coefficients of y_t on x_t, x_{t-1} (with constant), both first differences already"""
    X = np.column_stack([x[1:], x[:-1], np.ones(len(x) - 1)])
    b = np.linalg.lstsq(X, y[1:], rcond=None)[0]
    return float(b[0] + b[1])


def detr_sd(x):
    t = np.arange(len(x))
    return float((x - np.polyval(np.polyfit(t, x, 1), t)).std())


def hp(y, lam=1600.0):
    n = len(y)
    I = np.eye(n)
    D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)


def quarterly(x):
    n = len(x) // 3 * 3
    return x[:n].reshape(-1, 3).mean(axis=1)


def corr(a, b):
    return float(np.corrcoef(a, b)[0, 1])


# ----------------------------------------------------------------------------------------------------------------
# Model series and facts
# ----------------------------------------------------------------------------------------------------------------
def prepare(h):
    """monthly arrays from one run's metric records"""
    def s(k, i=None):
        if k not in h[0]:
            return None
        return np.array([m[k] if i is None else m[k][i] for m in h], float)
    cpi = s('cpi')
    d = dict(cpi=cpi, core=s('core_cpi'), oil=s('oil_price'), gas=s('gasoline_price'), pi_e=s('expected_inflation'),
             fx=s('dollar_index'), hpi=s('hpi'), rent=s('rent_index'), pti=s('price_to_income'),
             sales=s('house_sales_millions_ar'), gdp=s('gdp_real_bn_ar'), emp=s('employment_persons_millions'))
    sp = s('sector_prices', 0)
    d['energy'] = sp
    d['food'] = s('sector_prices', 1)
    y = [m['yields'][1] for m in h] if 'yields' in h[0] else None
    d['y2'] = np.array(y, float) if y is not None else None
    return d


def need(*xs):
    for x in xs:
        if x is None or not np.all(np.isfinite(x)):
            raise KeyError('series missing')


def f_oil(d):
    need(d['oil'], d['cpi'])
    return np.log(d['oil'] / d['cpi'])


def sales_gdp_lead(d, k=2):
    need(d['sales'], d['gdp'])
    s = hp(np.log(quarterly(np.maximum(d['sales'], 1e-6))))
    y = hp(np.log(quarterly(d['gdp'])))
    return corr(s[:len(s) - k], y[k:])


def sales_price(d):
    need(d['sales'], d['hpi'], d['cpi'])
    ls = np.log(np.maximum(d['sales'], 1e-6))
    lr = np.log(d['hpi'] / d['cpi'])
    gs, gp = g(ls, 12), g(lr, 12)
    return corr(gs[:-12], gp[12:])


def exp_err(d):
    need(d['pi_e'], d['cpi'])
    real12 = 100 * g(np.log(d['cpi']), 12)
    return float((real12 - d['pi_e'][:-12]).std())


def exp_gas(d):
    need(d['gas'], d['pi_e'])
    return corr(g(np.log(d['gas']), 12), g(d['pi_e'], 12))


def lp(d):
    need(d['gdp'], d['emp'])
    return np.log(quarterly(d['gdp']) / quarterly(d['emp']))


def real_hpi(d):
    need(d['hpi'], d['cpi'])
    return np.log(d['hpi'] / d['cpi'])


def yoy_sd(x):
    need(x)
    return float((100 * g(np.log(x), 12)).std())


def elast(y, x):
    need(y, x)
    return lag01(np.diff(np.log(y)), np.diff(np.log(x)))


def fx(d):
    need(d['fx'])
    return np.log(d['fx'])


def gas_pt(d):
    need(d['gas'], d['oil'])
    return lag01(np.diff(d['gas']), np.diff(d['oil']))


def head_core(d):
    need(d['cpi'], d['core'])
    return float(g(np.log(d['cpi']), 12).std() / g(np.log(d['core']), 12).std())


def fx_disc(d):
    need(d['y2'])
    return corr(np.diff(fx(d)), np.diff(d['y2']))


def hpi_mom(d):
    x = g(real_hpi(d), 12)
    return corr(x[12:], x[:-12])


def ptr_cycle(d):
    need(d['hpi'], d['rent'])
    return detr_sd(np.log(d['hpi'] / d['rent']))


def pti(d):
    need(d['pti'])
    return float(np.mean(d['pti']))


FACTS = {
    'oil_vol': lambda d: 100 * float(np.diff(f_oil(d)).std()),
    'oil_kurt': lambda d: kurt_ex(np.diff(f_oil(d))),
    'oil_vr12': lambda d: vr(f_oil(d), 12),
    'oil_big': lambda d: float(np.mean(np.abs(g(f_oil(d), 12)) > 0.5)),
    'oil_lvl': lambda d: float(f_oil(d).std()),
    'oil_drift': lambda d: 1200 * float(np.diff(f_oil(d)).mean()),
    'gas_pt': gas_pt,
    'gas_elast': lambda d: elast(d['gas'], d['oil']),
    'energy_vol': lambda d: yoy_sd(d['energy']),
    'energy_beta': lambda d: elast(d['energy'], d['oil']),
    'food_vol': lambda d: yoy_sd(d['food']),
    'exp_err': exp_err,
    'exp_gas': exp_gas,
    'fx_vol': lambda d: 100 * float(np.diff(fx(d)).std()),
    'fx_lvl': lambda d: float(fx(d).std()),
    'fx_disc': fx_disc,
    'hpi_mom': hpi_mom,
    'hpi_vol': lambda d: float(100 * g(real_hpi(d), 12).std()),
    'hpi_cycle': lambda d: detr_sd(real_hpi(d)),
    'ptr_cycle': ptr_cycle,
    'pti': pti,
    'sales_gdp': sales_gdp_lead,
    'sales_price': sales_price,
    'lp_vol': lambda d: float((400 * np.diff(lp(d))).std()),
    'lp_cyc': lambda d: corr(hp(lp(d)), hp(np.log(quarterly(d['gdp'])))),
}

# Supplementary diagnostics (printed, not flagged): (name, data value, function)
DIAG = [
    ('oil: AR1 of monthly dln (data 0.25, monthly averaging)', '0.25', lambda d: ac1(np.diff(f_oil(d)))),
    ('oil: sd of 12m dln real, % (data 32.9, 26.9-36.4)', '32.9', lambda d: 100 * float(g(f_oil(d), 12).std())),
    ('oil: AR1 |dln| - volatility clustering (data 0.36, 0.22-0.56)', '0.36', lambda d: ac1(np.abs(np.diff(f_oil(d))))),
    ('gasoline: margin gas - oil/42, $/gal (data 2015-26 1.46; 2023-25 1.71)', '1.46', lambda d: float(np.mean(d['gas'] - d['oil'] / 42.0))),
    ('gasoline: sd monthly dln, % (data 6.4, 5.9-6.7)', '6.4', lambda d: 100 * float(np.diff(np.log(d['gas'])).std())),
    ('energy CPI: sd monthly dln, % (data 2.9, 2.3-3.2)', '2.9', lambda d: 100 * float(np.diff(np.log(d['energy'])).std())),
    ('energy minus core CPI inflation, pp/yr (data 0.1 full, -0.6..1.4)', '0.1', lambda d: 1200 * float(np.diff(np.log(d['energy'])).mean() - np.diff(np.log(d['core'])).mean())),
    ('core CPI: sd 12m, pp (data 0.75, 0.42-1.23)', '0.75', lambda d: float((100 * g(np.log(d['core']), 12)).std())),
    ('sd(12m headline CPI) / sd(12m core CPI) (data 2.0, 1.0-2.9; 1986-2019 1.31)', '2.0', head_core),
    ('headline CPI: AR1 monthly inflation (data 0.43, 0.35-0.54)', '0.43', lambda d: ac1(np.diff(np.log(d['cpi'])))),
    ('expectations: mean forecast error, pp (data -0.46 1986-2019)', '-0.46', lambda d: float((100 * g(np.log(d['cpi']), 12) - d['pi_e'][:-12]).mean())),
    ('expectations: slope of 12m d pi_e on 12m dln gas, pp per 10% (data 0.25)', '0.25', lambda d: 10 * float(np.polyfit(100 * g(np.log(d['gas']), 12), g(d['pi_e'], 12), 1)[0])),
    ('dollar: AR1 monthly dln (data 0.31-0.39)', '0.35', lambda d: ac1(np.diff(np.log(d['fx'])))),
    ('dollar: sd 12m dln, % (data 5.2-7.6)', '6.4', lambda d: 100 * float(g(np.log(d['fx']), 12).std())),
    ('HPI: AR1 monthly real dln (data CS 0.83, index smoothing)', '0.83', lambda d: ac1(np.diff(np.log(d['hpi'] / d['cpi'])))),
    ('HPI: corr(36m real growth, previous 36m) (data CS 0.22, -0.09..0.85)', '0.22', lambda d: (lambda x: corr(x[36:], x[:-36]))(g(np.log(d['hpi'] / d['cpi']), 36))),
    ('sales: quarter lead of peak corr(sales_t, GDP_t+k), k=0..8 (data 2-3)', '2-3', lambda d: float(np.argmax([sales_gdp_lead(d, k) for k in range(9)]))),
    ('sales: sd HP log sales / sd HP log GDP (new home sales 8.5-10; existing lower)', '~5-10', lambda d: float(hp(np.log(quarterly(d['sales']))).std() / hp(np.log(quarterly(d['gdp']))).std())),
    ('productivity: AR1 quarterly growth (data -0.09..0.2; Fernald TFP ≈ 0)', '0.0', lambda d: ac1(400 * np.diff(lp(d)))),
]


# ----------------------------------------------------------------------------------------------------------------
# Running the model
# ----------------------------------------------------------------------------------------------------------------
def parse_args():
    """(cli path, targets-json path or None, no-run flag)"""
    pos, tj, norun = [], None, False
    it = iter(sys.argv[1:])
    for a in it:
        if a == '--targets-json':
            tj = next(it, None)
        elif a == '--no-run':
            norun = True
        else:
            pos.append(a)
    cli = pos[0] if pos else os.environ.get('ECONSIM_CLI', os.path.join('.', 'target', 'release', 'econsim-cli'))
    return cli, tj, norun


def run(cli, hh, months, seed):
    cfg = os.environ.get('ECONSIM_CFG', '')
    cache = os.environ.get('SHOCKS_CACHE')
    path = None
    if cache:
        st = os.stat(cli)
        key = hashlib.md5(f'{os.path.realpath(cli)}|{st.st_mtime}|{st.st_size}|{cfg}'.encode()).hexdigest()[:12]
        os.makedirs(cache, exist_ok=True)
        path = os.path.join(cache, f'shocks_{key}_{hh}_{months}_{seed}.json')
        if os.path.exists(path):
            with open(path) as f:
                return json.load(f)
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([cli, 'run', str(hh), str(months), str(seed), '--json'], capture_output=True, text=True, env=env)
    if out.returncode != 0:
        raise RuntimeError(f'econsim-cli failed (seed {seed}): {out.stderr[-500:]}')
    h = json.loads(out.stdout.strip().splitlines()[-1])
    if path:
        with open(path, 'w') as f:
            json.dump(h, f)
    return h


def evaluate(windows, fn):
    vals = []
    for d in windows:
        try:
            v = fn(d)
        except (KeyError, ValueError, FloatingPointError, IndexError, np.linalg.LinAlgError):
            return None
        if v is None or not np.isfinite(v):
            continue
        vals.append(float(v))
    return vals or None


def flag(t, v):
    if v is None:
        return 'n/a', None
    if t['lo'] <= v <= t['hi']:
        return 'ok', 0.0
    dist = (t['lo'] - v) if v < t['lo'] else (v - t['hi'])
    dd = dist / t['tol']
    return ('minor' if dd < 1 else 'material' if dd < 3 else 'severe'), dd


def fmt(v):
    if v is None:
        return 'n/a'
    a = abs(v)
    return f'{v:.4f}' if a < 0.1 else f'{v:.3f}' if a < 10 else f'{v:.2f}' if a < 100 else f'{v:.1f}'


def main():
    cli, tj, norun = parse_args()
    if tj:
        p = tj
        with open(p, 'w') as f:
            json.dump({'dimension': 'exogenous and price processes', 'script': 'scripts/facts_shocks.py',
                       'method': __doc__.split('Method.')[1].strip(), 'targets': TARGETS}, f, indent=1, ensure_ascii=False)
        print(f'wrote {len(TARGETS)} targets to {p}')
        if norun:
            return
    hh = int(os.environ.get('SHOCKS_HH', '20000'))
    seeds = int(os.environ.get('SHOCKS_SEEDS', '8'))
    months = int(os.environ.get('SHOCKS_MONTHS', '240'))
    lhh = int(os.environ.get('SHOCKS_LONG_HH', '10000'))
    lseeds = int(os.environ.get('SHOCKS_LONG_SEEDS', '4'))
    lmonths = int(os.environ.get('SHOCKS_LONG_MONTHS', '480'))
    jobs = int(os.environ.get('SHOCKS_JOBS', '8'))
    t0 = time.time()
    tasks = [(hh, months, s) for s in range(1, seeds + 1)] + [(lhh, lmonths, s) for s in range(1, lseeds + 1)]
    with ThreadPoolExecutor(jobs) as ex:
        res = list(ex.map(lambda a: run(cli, *a), tasks))
    wall = time.time() - t0
    main_w = [prepare(h[BURN:]) for h in res[:seeds]]
    long_w = []
    for h in res[seeds:]:
        h = h[BURN:]
        for s0 in range(0, max(1, len(h) - 240 + 1), 114):
            long_w.append(prepare(h[s0:s0 + 240]))

    print(f'EconSim exogenous & price processes vs US data — cli={cli}')
    print(f'main: {seeds} seeds x {months} months x {hh} households; long: {lseeds} x {lmonths} x {lhh} '
          f'({len(long_w)} windows of 240m); ECONSIM_CFG={os.environ.get("ECONSIM_CFG", "")!r}; wall {wall:.0f}s\n')
    hdr = f'{"fact":66s} | {"target (range)":24s} | {"model":>8s} {"seeds min..max":>17s} {"long":>8s} | gap'
    print(hdr)
    print('-' * len(hdr))
    results = []
    group = None
    for t in TARGETS:
        if t['group'] != group:
            group = t['group']
            print(f'[{group}]')
        vm = evaluate(main_w, FACTS[t['id']])
        vl = evaluate(long_w, FACTS[t['id']])
        m = float(np.mean(vm)) if vm else None
        ml = float(np.mean(vl)) if vl else None
        fl, dd = flag(t, m)
        rng = f'{fmt(min(vm))}..{fmt(max(vm))}' if vm else 'n/a'
        tgt = f'{fmt(t["target"])} ({fmt(t["lo"])}..{fmt(t["hi"])})'
        print(f'{t["name"][:66]:66s} | {tgt:24s} | {fmt(m):>8s} {rng:>17s} {fmt(ml):>8s} | {fl}')
        results.append(dict(id=t['id'], model=m, model_long=ml, seeds=vm, gap=fl, d=dd))
    print('\nSupplementary diagnostics (not flagged)')
    for name, dv, fn in DIAG:
        vm = evaluate(main_w, fn)
        vl = evaluate(long_w, fn)
        print(f'  {name[:84]:84s} model {fmt(float(np.mean(vm)) if vm else None):>8s}  long {fmt(float(np.mean(vl)) if vl else None):>8s}')
    counts = {k: sum(r['gap'] == k for r in results) for k in ('ok', 'minor', 'material', 'severe', 'n/a')}
    print('\nsummary: ' + ', '.join(f'{k} {v}' for k, v in counts.items()))
    out = os.environ.get('SHOCKS_RESULTS')
    if out:
        with open(out, 'w') as f:
            json.dump(dict(wall_s=wall, results=results), f, indent=1)


if __name__ == '__main__':
    main()
