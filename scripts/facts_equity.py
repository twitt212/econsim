#!/usr/bin/env python3
"""Equity-market stylised facts: model vs US data (fact | target | model | gap flag).

Usage:
  python3 scripts/facts_equity.py [path to econsim-cli] [--targets-json PATH]
  The CLI path can also come from ECONSIM_CLI (default ./target/release/econsim-cli). ECONSIM_CFG is passed through to
  every run, so config overrides apply. Only numpy and the standard library are used.

Runs (seeds in parallel, RAYON_NUM_THREADS=1 per process):
  standard set  EQF_SEEDS (8) seeds 1..N    x EQF_MONTHS (240)      x EQF_HH (20000) households: levels, cycles, sectors
  long set      EQF_LONG_SEEDS (4) seeds 101.. x EQF_LONG_MONTHS (480) x EQF_LONG_HH (10000): monthly return moments
                and long-horizon predictability (pooled with the standard set where the fact uses all runs)
  EQF_JOBS (8) parallel processes; EQF_CACHE=dir keeps each run's JSON there and reuses it (same CLI/config only!).
  The first EQF_DROP (12) reported months of each run are dropped. Seeds 1..N and 101.. have different exogenous paths.

Model definitions (all from the per-month JSON):
  E_t       equity_value_bn (the Fund's equity in all firms and banks, ex-dividend).
  r_t       monthly nominal total return. equity_return_12m is the trailing compounded 12-month total return
            (E_t + dividends paid)/E_{t-1}, so (1+r_t) = (1+R12_t)/(1+R12_{t-1}) * (1+r_{t-12}); the recursion starts
            from the price return plus the run's mean dividend yield for the first 12 months (error < 0.1pp/month).
            Implied dividends D_t = (1+r_t) E_{t-1} - E_t; D/P = trailing 12-month D / E_t.
  real      deflated by the cpi level; risk-free = 3-month yield (yields[0]) at t-1; 10y bond return
            = y10_{t-1}/12 - 7.5 * (y10_t - y10_{t-1}) (constant modified duration; the same formula is used on FRED GS10).
  recessions  Harding-Pagan (2002) BBQ dating on quarterly real GDP (local peaks/troughs over +-2 quarters, alternating,
            phases >= 2 quarters, peak-to-trough fall >= 0.3%), standard (20k) runs only; the US benchmarks use NBER
            months (FRED USREC). The dating-free check corr(12m realised vol, 12m change in unemployment) uses all runs.
  tech booms  runs of tech_boom == 1 (the Markov regime); Tech = sector index 4 of equity_value_by_sector.

US targets were computed by the author from: Kenneth French Data Library (CRSP value-weighted market = Mkt-RF + RF,
monthly and daily, and the 10 industry portfolios, file built from the 202608 CRSP database), Robert Shiller's monthly
S&P composite data (price, trailing dividends and earnings, CPI, CAPE), FRED (USREC, UNRATE, GS10, CPIAUCSL, GDPC1,
GDP, NCBEILQ027S). Each target lists its sample, citation and the literature it reproduces. Samples: post-1984 (Great
Moderation) unless noted; 1950-2024 values are given where they differ materially.

The table prints 25 key facts, then supplementary checks (core=False).
Gap flags: ok = inside the target range; otherwise the distance outside the range in units of the fact's tolerance:
  minor <= 1 tol, material <= 3 tol, severe > 3 tol; n/a = not measurable in these runs (e.g. no tech boom).
"""
import json, math, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

# ----------------------------------------------------------------------------------------------------------------------
# Targets: key -> (fact, lo, hi, tol, unit, us_values, sample, source)
# ----------------------------------------------------------------------------------------------------------------------
FF = 'Kenneth French Data Library, CRSP VW market (Mkt-RF+RF), computed here'
SH = 'Shiller monthly S&P composite data (Irrational Exuberance; shillerdata.com), computed here'
TARGETS = [
    dict(key='real_return', fact='Real total return, geometric, %/yr', lo=6.0, hi=9.0, tol=1.5, unit='%/yr',
         us='8.5 (1984-2024); 7.7 (1950-2024); 7.0 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Siegel (2014) Stocks for the Long Run ~6.7% 1802-2012; Dimson, Marsh & Staunton (2002)'),
    dict(key='vol_monthly', fact='Volatility of monthly returns, annualised, %', lo=14.0, hi=17.0, tol=2.0, unit='%',
         us='15.6 (1984-2024); 14.9 (1950-2024); 18.4 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Schwert (1989 JF); Campbell, Lo & MacKinlay (1997) ch. 1'),
    dict(key='vol_annual', fact='Sd of 12-month real returns (non-overlapping), %', lo=15.0, hi=20.0, tol=2.5, unit='%',
         us='16.9 (1984-2024); 17.5 (1950-2024); 19.8 (1927-2024)', sample='1984-2024',
         source=f'{FF} deflated by CPI-U; Shiller (1981 AER); Campbell & Cochrane (1999) ~18%'),
    dict(key='equity_premium', fact='Excess return over 3m bill, arithmetic, %/yr', lo=4.0, hi=8.5, tol=1.5, unit='pp/yr',
         us='realised 8.9 (1984-2024), 8.1 (1950-2024); ex-ante 4.0 (Damodaran implied 1961-2012), 2.6-4.3 (Fama-French 2002, 1951-2000)',
         sample='1984-2024 / 1951-2000', source=f'{FF}; Mehra & Prescott (1985); Fama & French (2002 JF); Damodaran histimpl (NYU Stern)'),
    dict(key='skew', fact='Skewness of monthly returns', lo=-1.0, hi=-0.3, tol=0.3, unit='',
         us='-0.72 (1984-2024); -0.49 (1950-2024); +0.13 (1927-2024, 1930s rallies); log returns -0.99 / -0.73',
         sample='1984-2024', source=f'{FF}; Cont (2001 QF) stylised fact 3 (gain/loss asymmetry); Campbell & Hentschel (1992 JFE)'),
    dict(key='kurt', fact='Excess kurtosis of monthly returns', lo=1.0, hi=3.5, tol=1.0, unit='',
         us='2.1 (1984-2024); 1.7 (1950-2024); 7.3 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Cont (2001) heavy tails, aggregational Gaussianity (monthly kurtosis << daily)'),
    dict(key='neg_share', core=False, fact='Share of months with negative return, %', lo=33.0, hi=40.0, tol=3.0, unit='%',
         us='35.0 (1984-2024); 36.3 (1950-2024); 37.1 (1927-2024)', sample='1984-2024', source=FF),
    dict(key='ac1', fact='Autocorrelation of monthly returns, lag 1', lo=-0.05, hi=0.10, tol=0.05, unit='',
         us='0.02 (1984-2024); 0.04 (1950-2024); 0.09 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Cont (2001) absence of linear autocorrelation; Lo & MacKinlay (1988 RFS)'),
    dict(key='ac1_abs', fact='Volatility clustering: AC1 of |r - mean|', lo=0.12, hi=0.30, tol=0.06, unit='',
         us='0.22 (1984-2024); 0.16 (1950-2024); 0.26 (1927-2024); daily-RV log AC1 0.65-0.69', sample='1984-2024',
         source=f'{FF}; Cont (2001) volatility clustering; Schwert (1989); Bollerslev (1986)'),
    dict(key='ac_abs_12', core=False, fact='Volatility clustering: mean AC of |r - mean|, lags 1-12', lo=0.06, hi=0.22, tol=0.04, unit='',
         us='0.10 (1984-2024); 0.10 (1950-2024); 0.21 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Ding, Granger & Engle (1993 JEF) long memory of |r|'),
    dict(key='leverage', fact='Leverage effect: corr(r_t, |r_t+1 - mean|)', lo=-0.35, hi=-0.10, tol=0.08, unit='',
         us='-0.26 (1984-2024); -0.25 (1950-2024); daily RV: corr(r_t, log RV_t+1) -0.32 (1984-2024)', sample='1984-2024',
         source=f'{FF} and daily factors; Black (1976); Christie (1982 JFE); Cont (2001) leverage effect'),
    dict(key='vol_rec_ratio', fact='Return sd in recessions / expansions', lo=1.4, hi=2.2, tol=0.25, unit='x',
         us='1.71 (1984-2024); 1.56 (1950-2024); daily-RV ratio 2.07 / 1.54', sample='1984-2024 (NBER months)',
         source=f'{FF}, FRED USREC; Schwert (1989 JF) volatility higher in recessions; Hamilton & Lin (1996 JAE)'),
    dict(key='vol_unemp_corr', fact='Countercyclical vol: corr(12m realised vol, 12m change in unemployment)', lo=0.25, hi=0.55, tol=0.10, unit='',
         us='0.39 (1984-2024); 0.50 (1984-2019); 0.41 (1950-2024)', sample='1984-2024',
         source=f'{FF}, FRED UNRATE; Schwert (1989 JF); Officer (1973 JB); Mele (2007 JFE) asymmetric volatility over the cycle'),
    dict(key='rec_drawdown', fact='Median real drawdown around recessions, %', lo=-45.0, hi=-18.0, tol=7.0, unit='%',
         us='median -27 post-1984 (-20, -35, -51, -20); -20 for 1948-2024 (12 recessions, -8 to -55)',
         sample='1984-2024', source=f'{FF} monthly real total-return index, window [NBER peak-12m, trough+6m]'),
    dict(key='bear_freq', core=False, fact='Real bear markets (>=20% below prior peak) per decade', lo=0.8, hi=1.6, tol=0.4, unit='/decade',
         us='1.22 (1984-2024: 2000-02, 2008-09, 2020, 2022, 1987); 0.93 (1950-2024)', sample='1984-2024',
         source=f'{FF} monthly real total-return index; Pagan & Sossounov (2003 JAE) bull/bear dating'),
    dict(key='pe_level', fact='P/E on trailing earnings, mean', lo=15.0, hi=23.0, tol=3.0, unit='x',
         us='mean 22.9, median 20.7 (1984-2024); mean 18.5 (1950-2024); ~25-28 in 2024-25; CAPE median 24.9 / 19.7',
         sample='1984-2024', source=f'{SH}; Campbell & Shiller (1998 JPM)'),
    dict(key='dp_persist', fact='Persistence of log D/P: 12-month autocorrelation', lo=0.75, hi=0.95, tol=0.06, unit='',
         us='0.81 (1984-2024, half-life 3.3y); 0.89 (1950-2024, half-life 6.2y); annual AR(1) ~0.94 (Cochrane 2008, 1926-2004)',
         sample='1950-2024', source=f'{SH}; Cochrane (2008 RFS); Campbell & Shiller (1988 RFS)'),
    dict(key='payout_yield', core=False, fact='Payout yield (dividends incl. buybacks), %', lo=3.0, hi=5.5, tol=1.0, unit='%',
         us='D/P 2.3 (1984-2024), 3.1 (1950-2024); total payout yield ~ D/P + 2pp after 1985 (repurchases)',
         sample='1984-2024', source=f'{SH}; Boudoukh, Michaely, Richardson & Roberts (2007 JF); Grullon & Michaely (2002 JF)'),
    dict(key='pred_r2_1y', core=False, fact='1-yr return predictability: R2 of fwd 12m real return on log D/P', lo=0.03, hi=0.15, tol=0.04, unit='',
         us='0.15 (1984-2024); 0.06 (1950-2024); slope 0.18 / 0.09', sample='1950-2024',
         source=f'{SH}; Fama & French (1988 JFE); Cochrane (2008); Goyal & Welch (2008 RFS) out-of-sample caveat'),
    dict(key='pred_r2_5y', fact='5-yr return predictability: R2 of fwd 60m real return on log D/P', lo=0.15, hi=0.45, tol=0.10, unit='',
         us='0.45 (1984-2024); 0.19 (1950-2024); slope 0.68 / 0.37 (CAPE: R2 0.34 / 0.14)', sample='1950-2024',
         source=f'{SH}; Campbell & Shiller (1988 JF); Fama & French (1988); Cochrane (2011 JF presidential address)'),
    dict(key='pred_slope_5y', fact='5-yr predictive slope on log D/P (>0: high D/P -> high returns)', lo=0.25, hi=0.80, tol=0.15, unit='',
         us='0.68 (1984-2024); 0.37 (1950-2024)', sample='1950-2024', source=f'{SH}; Cochrane (2008, 2011)'),
    dict(key='excess_vol', fact='Excess volatility: sd dlog real price / sd dlog real dividends (annual)', lo=2.0, hi=3.2, tol=0.5, unit='x',
         us='2.85 (1950-2024: 16.7% vs 5.9%); 2.39 (1984-2024); 1.62 (1881-2024)', sample='1950-2024',
         source=f'{SH}; Shiller (1981 AER); LeRoy & Porter (1981); Campbell & Shiller (1988)'),
    dict(key='vr60', fact='Variance ratio of real returns, 60 months (mean reversion < 1)', lo=0.75, hi=1.05, tol=0.15, unit='',
         us='0.84 (1984-2024); 1.03 (1950-2024); 0.81 (1927-2024)', sample='1984-2024',
         source=f'{FF}; Poterba & Summers (1988 JFE); Fama & French (1988 JPE); Lo & MacKinlay (1988)'),
    dict(key='stock_bond_corr', fact='Monthly stock-10y Treasury return correlation', lo=-0.20, hi=0.30, tol=0.15, unit='',
         us='+0.05 (1984-2024); +0.24 (1984-1999); -0.11 (2000-2024); +0.16 (1962-2024)', sample='1984-2024',
         source=f'{FF}, FRED GS10 (duration 7.5); Baele, Bekaert & Inghelbrecht (2010 RFS); Campbell, Sunderam & Viceira (2017 CFR)'),
    dict(key='leads_gdp', fact='corr(past 4q real return, next 4q real GDP growth)', lo=0.15, hi=0.45, tol=0.10, unit='',
         us='0.27 (1984-2024); 0.32 (1950-2024); same-4q 0.41 / 0.29', sample='1984-2024',
         source=f'{FF}, FRED GDPC1; Fama (1990 JF); Schwert (1990 JF); Stock & Watson (2003 JEL)'),
    dict(key='eq_gdp', core=False, fact='Market value of corporate equity / GDP', lo=1.3, hi=2.3, tol=0.3, unit='x',
         us='2.10 (2024); 1.64 (2015-2024 mean); 1.07 (1984-2024 mean); 0.84 (1950-2024)', sample='2015-2024',
         source='Fed Z.1 nonfinancial corporate equities at market value (FRED NCBEILQ027S) / FRED GDP'),
    dict(key='tech_vol_ratio', fact='Tech equity return sd / market sd (monthly)', lo=1.3, hi=1.7, tol=0.2, unit='x',
         us='1.48 (1984-2024); 1.42 (1950-2024); utilities 0.90, nondurables 0.92', sample='1984-2024',
         source='Kenneth French 10 industry portfolios (HiTec, value-weighted), computed here'),
    dict(key='sector_vol_spread', fact='Sector return sd: max / min across sectors', lo=1.6, hi=2.4, tol=0.3, unit='x',
         us='2.01 (1984-2024: durables 1.81 vs utilities 0.90 of market); 1.79 (1950-2024)', sample='1984-2024',
         source='Kenneth French 10 industry portfolios, computed here'),
    dict(key='sector_mkt_corr', core=False, fact='Mean corr(sector return, market return), monthly', lo=0.65, hi=0.90, tol=0.10, unit='',
         us='0.77 (1984-2024; range 0.50 utilities .. 0.92 manufacturing); 0.79 (1950-2024); HiTec 0.86', sample='1984-2024',
         source='Kenneth French 10 industry portfolios vs CRSP VW market, computed here'),
    dict(key='boom_runup', fact='Tech boom: real Tech equity run-up, start to peak, %', lo=250.0, hi=800.0, tol=150.0, unit='%',
         us='+579 (HiTec real total return Dec 1994-Mar 2000); market +209; Nasdaq Composite ~+570 nominal',
         sample='1995-2000', source='Kenneth French HiTec portfolio, CPI-U; Pastor & Veronesi (2009 AER); Ofek & Richardson (2003 JF)'),
    dict(key='boom_bust', fact='Tech bust: real Tech drawdown from peak (36m), %', lo=-85.0, hi=-60.0, tol=10.0, unit='%',
         us='-81 (HiTec real, Mar 2000-Sep 2002); Nasdaq -78 nominal', sample='2000-2002',
         source='Kenneth French HiTec portfolio; Pastor & Veronesi (2009)'),
    dict(key='boom_bust_mkt', fact='Market real drawdown after a tech boom (36m), %', lo=-55.0, hi=-30.0, tol=10.0, unit='%',
         us='-48 (CRSP VW real total return Mar 2000-Sep 2002); S&P 500 -49 nominal price', sample='2000-2002', source=FF),
    dict(key='boom_tech_share', core=False, fact='Tech share of equity at boom peak, %', lo=29.0, hi=37.0, tol=4.0, unit='%',
         us='32.9 (S&P 500 IT, Mar 2000; Yardeni); 33.6 (Aug 2000, S&P DJI); ~5-10 in early 1990s', sample='2000',
         source='Yardeni Research S&P 500 sector weights; S&P Dow Jones Indices "Not your father\'s tech sector"'),
]

# ----------------------------------------------------------------------------------------------------------------------
args = [a for a in sys.argv[1:]]
TARGETS_OUT = os.environ.get('EQF_TARGETS_OUT')
if '--targets-json' in args:
    i = args.index('--targets-json')
    TARGETS_OUT = args[i + 1]
    del args[i:i + 2]
CLI = args[0] if args else os.environ.get('ECONSIM_CLI', './target/release/econsim-cli')
SEEDS = int(os.environ.get('EQF_SEEDS', '8'))
MONTHS = int(os.environ.get('EQF_MONTHS', '240'))
HH = os.environ.get('EQF_HH', '20000')
LSEEDS = int(os.environ.get('EQF_LONG_SEEDS', '4'))
LMONTHS = int(os.environ.get('EQF_LONG_MONTHS', '480'))
LHH = os.environ.get('EQF_LONG_HH', '10000')
JOBS = int(os.environ.get('EQF_JOBS', '8'))
DROP = int(os.environ.get('EQF_DROP', '12'))
CACHE = os.environ.get('EQF_CACHE')
DUR10 = 7.5


def run(spec):
    hh, months, seed = spec
    path = os.path.join(CACHE, f'eq_{hh}_{months}_{seed}.json') if CACHE else None
    if path and os.path.exists(path):
        with open(path) as fh:
            return json.load(fh)
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', str(hh), str(months), str(seed), '--json'], capture_output=True, text=True, env=env)
    if out.returncode != 0:
        sys.exit(f'econsim-cli failed (seed {seed}): {out.stderr[-500:]}')
    h = json.loads(out.stdout.strip().splitlines()[-1])
    if path:
        os.makedirs(CACHE, exist_ok=True)
        with open(path, 'w') as fh:
            json.dump(h, fh)
    return h


# ----------------------------------------------------------------------------------------------------------------------
def ac(x, k):
    x = np.asarray(x, float)
    x = x - x.mean()
    den = (x * x).sum()
    return float((x[k:] * x[:-k]).sum() / den) if den > 0 and len(x) > k else np.nan


def moments(r):
    z = (r - r.mean()) / r.std(ddof=1)
    return float((z ** 3).mean()), float((z ** 4).mean() - 3.0)


def corr(a, b):
    a, b = np.asarray(a, float), np.asarray(b, float)
    if len(a) < 3 or a.std() == 0 or b.std() == 0:
        return np.nan
    return float(np.corrcoef(a, b)[0, 1])


def bbq(y, k=2, min_phase=2, min_cycle=5, min_amp=0.003):
    """Harding-Pagan quarterly turning points on log level y. Returns a boolean recession mask (quarters after a peak
    up to and including the trough)."""
    n = len(y)
    tp = []
    for t in range(k, n - k):
        w = y[t - k:t + k + 1]
        if y[t] == w.max():
            tp.append((t, 'P'))
        elif y[t] == w.min():
            tp.append((t, 'T'))
    # alternate: of consecutive same-type points keep the more extreme
    alt = []
    for t, s in tp:
        if alt and alt[-1][1] == s:
            if (s == 'P' and y[t] > y[alt[-1][0]]) or (s == 'T' and y[t] < y[alt[-1][0]]):
                alt[-1] = (t, s)
        else:
            alt.append((t, s))
    # censor short phases / shallow contractions (drop the offending pair and re-alternate)
    changed = True
    while changed and len(alt) > 1:
        changed = False
        for i in range(len(alt) - 1):
            (t0, s0), (t1, s1) = alt[i], alt[i + 1]
            shallow = s0 == 'P' and (y[t0] - y[t1]) < min_amp
            short = (t1 - t0) < min_phase
            cyc = i + 2 < len(alt) and (alt[i + 2][0] - t0) < min_cycle
            if shallow or short or cyc:
                del alt[i:i + 2]
                merged = []
                for t, s in alt:
                    if merged and merged[-1][1] == s:
                        if (s == 'P' and y[t] > y[merged[-1][0]]) or (s == 'T' and y[t] < y[merged[-1][0]]):
                            merged[-1] = (t, s)
                    else:
                        merged.append((t, s))
                alt = merged
                changed = True
                break
    rec = np.zeros(n, bool)
    for i in range(len(alt) - 1):
        if alt[i][1] == 'P' and alt[i + 1][1] == 'T':
            rec[alt[i][0] + 1:alt[i + 1][0] + 1] = True
    return rec


def max_drawdown(x):
    peak = np.maximum.accumulate(x)
    return float((x / peak - 1.0).min())


def bear_count(x, thr=0.20):
    peak, inbear, n = x[0], False, 0
    for v in x:
        if v > peak:
            peak, inbear = v, False
        if v / peak - 1 <= -thr and not inbear:
            inbear, n = True, n + 1
    return n


def episodes(mask):
    out, i = [], 0
    while i < len(mask):
        if mask[i]:
            j = i
            while j < len(mask) and mask[j]:
                j += 1
            out.append((i, j - 1))
            i = j
        else:
            i += 1
    return out


def series(h):
    """Per-run monthly arrays over the full report (drop applied by the caller)."""
    g = lambda k: np.array([m[k] for m in h], float)
    E, R12, cpi = g('equity_value_bn'), g('equity_return_12m') / 100.0, g('cpi')
    n = len(E)
    p = np.full(n, np.nan)
    p[1:] = E[1:] / E[:-1] - 1.0
    dy = np.array([R12[t] - (E[t] / E[t - 12] - 1.0) for t in range(12, n)])  # trailing-12m dividend yield, approx
    d0 = float(np.mean(dy)) / 12.0 if len(dy) else 0.0
    r = np.full(n, np.nan)
    if 'equity_return_1m' in h[0]:
        # metrics-out: the model reports the monthly total return directly, (E_t + dividends paid) / E_{t-1} - 1.
        r[1:] = g('equity_return_1m')[1:] / 100.0
    else:
        for t in range(1, min(13, n)):
            r[t] = p[t] + d0
        for t in range(13, n):
            r[t] = (1 + R12[t]) / (1 + R12[t - 1]) * (1 + r[t - 12]) - 1.0
    div = np.full(n, np.nan)
    div[1:] = (1 + r[1:]) * E[:-1] - E[1:]
    if np.nanmin(div[1:]) < -0.002 * np.nanmean(E):  # recursion broken (e.g. a build without equity_return_12m)
        r[1:] = p[1:] + d0
        div[1:] = d0 * E[:-1]
    infl = np.full(n, np.nan)
    infl[1:] = cpi[1:] / cpi[:-1] - 1.0
    y = np.array([m['yields'] for m in h], float) / 100.0
    rf = np.full(n, np.nan)
    rf[1:] = y[:-1, 0] / 12.0
    rb = np.full(n, np.nan)
    rb[1:] = y[:-1, 3] / 12.0 - DUR10 * (y[1:, 3] - y[:-1, 3])
    D12 = np.full(n, np.nan)
    for t in range(12, n):
        D12[t] = div[t - 11:t + 1].sum()
    sect = np.array([m['equity_value_by_sector'] for m in h], float)
    return dict(E=E, p=p, r=r, rr=(1 + r) / (1 + infl) - 1.0, rf=rf, rb=rb, cpi=cpi, D12=D12, pe=g('pe_ratio'),
                gdp=g('gdp_real_bn_ar'), gdpn=g('gdp_nominal_bn_ar'), boom=g('tech_boom'), sect=sect,
                u=g('unemployment_rate'))


def measure(runs_std, runs_long):
    res = {k['key']: [] for k in TARGETS}
    weights = {k['key']: [] for k in TARGETS}
    diag = dict(rec_share=[], n_booms=0, n_busts=0, price_ac1=[], price_vol=[], d_sd=[], n_recessions=0)
    rec_dd = []
    all_runs = [(h, 'std') for h in runs_std] + [(h, 'long') for h in runs_long]

    def add(key, v, w=1.0):
        if v is not None and np.isfinite(v):
            res[key].append(v)
            weights[key].append(w)

    for h, kind in all_runs:
        s = series(h)
        n = len(s['E'])
        a = DROP + 1  # first month with a valid return after the drop
        r, rr, rf = s['r'][a:], s['rr'][a:], s['rf'][a:]
        w = len(r)
        # --- return distribution (all runs)
        add('real_return', 100 * (math.exp(12 * np.log1p(rr).mean()) - 1), w)
        add('vol_monthly', 100 * r.std(ddof=1) * math.sqrt(12), w)
        blocks = [np.prod(1 + rr[i:i + 12]) - 1 for i in range(0, w - 11, 12)]
        add('vol_annual', 100 * np.std(blocks, ddof=1), w)
        add('equity_premium', 1200 * np.mean(r - rf), w)
        sk, ku = moments(r)
        add('skew', sk, w)
        add('kurt', ku, w)
        add('neg_share', 100 * np.mean(r < 0), w)
        add('ac1', ac(r, 1), w)
        dev = np.abs(r - r.mean())
        add('ac1_abs', ac(dev, 1), w)
        add('ac_abs_12', float(np.mean([ac(dev, j) for j in range(1, 13)])), w)
        add('leverage', corr(r[:-1], dev[1:]), w)
        lrr = np.log1p(rr)
        c60 = np.convolve(lrr, np.ones(60), 'valid')
        add('vr60', c60.var() / (60 * lrr.var()), w)
        add('stock_bond_corr', corr(r, s['rb'][a:]), w)
        diag['price_ac1'].append(ac(s['p'][a:], 1))
        diag['price_vol'].append(100 * np.std(s['p'][a:], ddof=1) * math.sqrt(12))
        # --- countercyclical volatility without recession dating: rolling 12-month RMS of demeaned returns vs the
        #     12-month change in the unemployment rate (same construction on CRSP and FRED UNRATE)
        u = s['u'][a:]
        rv = np.array([math.sqrt(np.mean(dev[t - 11:t + 1] ** 2)) for t in range(11, w)])
        add('vol_unemp_corr', corr(rv[1:], u[12:] - u[:-12]), w)
        # --- recessions (BBQ on quarterly real GDP). Standard (20k-household) runs only: at 10k households agent noise
        #     in quarterly GDP (growth sd ~1.1% vs ~0.8%) creates spurious shallow contractions.
        nq = (n - DROP) // 3
        gq = np.log(s['gdp'][DROP:DROP + 3 * nq].reshape(-1, 3).mean(axis=1))
        ridx = np.cumprod(1 + rr)
        if kind == 'std':
            recq = bbq(gq)
            recm = np.repeat(recq, 3)
            rm = np.zeros(w, bool)  # months aligned with r (r starts at DROP+1); months past the last full quarter stay False
            rm[:len(recm) - 1] = recm[1:w + 1]
            diag['rec_share'].append(100 * recm.mean())
            if rm.sum() >= 6 and (~rm).sum() >= 6:
                add('vol_rec_ratio', r[rm].std(ddof=1) / r[~rm].std(ddof=1), rm.sum())
            for (s0, e0) in episodes(rm):
                lo_, hi_ = max(0, s0 - 12), min(w - 1, e0 + 6)
                rec_dd.append(100 * max_drawdown(ridx[lo_:hi_ + 1]))
                diag['n_recessions'] += 1
        add('bear_freq', bear_count(ridx) / (w / 120.0), w)
        # --- valuation
        pe = s['pe'][DROP:]
        pe = pe[pe > 0]
        add('pe_level', float(np.mean(pe)), w)
        E = s['E']
        dp = s['D12'][DROP:] / E[DROP:]
        ok = np.isfinite(dp) & (dp > 0)
        ldp = np.log(dp[ok])
        add('dp_persist', ac(ldp, 12), w)
        add('payout_yield', 100 * float(np.mean(dp[ok])), w)
        add('eq_gdp', float(np.mean(E[DROP:] / s['gdpn'][DROP:])), w)
        # predictability: forward h-month log real return from t+1 on log D/P_t (long runs, and any run >= 20 years)
        ldp_full = np.log(s['D12'] / E)
        lr_full = np.log1p(s['rr'])
        for hz, key in ((12, 'pred_r2_1y'), (60, 'pred_r2_5y')):
            xs, ys = [], []
            for t in range(max(DROP, 12), n - hz):
                if np.isfinite(ldp_full[t]):
                    xs.append(ldp_full[t])
                    ys.append(lr_full[t + 1:t + hz + 1].sum())
            if len(xs) >= 60:
                xs, ys = np.array(xs), np.array(ys)
                A = np.vstack([np.ones_like(xs), xs]).T
                beta = np.linalg.lstsq(A, ys, rcond=None)[0]
                r2 = 1 - (ys - A @ beta).var() / ys.var()
                add(key, r2, len(xs))
                if hz == 60:
                    add('pred_slope_5y', beta[1], len(xs))
        # excess volatility: annual changes of log real price and log real trailing dividends
        idx = list(range(max(DROP, 12), n, 12))
        lp = np.log(E[idx] / s['cpi'][idx])
        ld = np.log(s['D12'][idx] / s['cpi'][idx])
        if len(idx) >= 6 and np.all(np.isfinite(ld)):
            sdp, sdd = np.diff(lp).std(ddof=1), np.diff(ld).std(ddof=1)
            diag['d_sd'].append(100 * sdd)
            add('excess_vol', sdp / sdd, len(idx))
        # equity leads GDP (quarterly)
        lq = np.log1p(rr)
        lq = lq[2: 2 + 3 * ((len(lq) - 2) // 3)].reshape(-1, 3).sum(axis=1)  # quarters aligned with gq (from DROP+3)
        gq2 = gq[1:1 + len(lq)]
        m = len(gq2) - 7
        if m > 10:
            past = np.array([lq[i:i + 4].sum() for i in range(m)])
            nxt = np.array([gq2[i + 7] - gq2[i + 3] for i in range(m)])
            add('leads_gdp', corr(past, nxt), m)
        # sectors: monthly price returns
        sect = s['sect'][DROP:]
        tot = E[DROP:]
        ok_s = np.all(sect > 0, axis=0)
        sret = sect[1:] / sect[:-1] - 1
        mret = tot[1:] / tot[:-1] - 1
        sds = np.array([sret[:, j].std() for j in range(sect.shape[1])]) / mret.std()
        add('tech_vol_ratio', sds[4], w)
        add('sector_mkt_corr', float(np.mean([corr(sret[:, j], mret) for j in range(sect.shape[1]) if ok_s[j]])), w)
        if ok_s.sum() >= 2:
            add('sector_vol_spread', sds[ok_s].max() / sds[ok_s].min(), w)
        # tech booms (full report, need a pre-boom month)
        tech_real = s['sect'][:, 4] / s['cpi']
        mkt_real = E / s['cpi']
        for (b0, b1) in episodes(s['boom'] > 0):
            if b0 < 1 or b1 - b0 + 1 < 24:
                continue
            diag['n_booms'] += 1
            win = slice(b0, min(n, b1 + 7))
            ip = b0 + int(np.argmax(tech_real[win]))
            add('boom_runup', 100 * (tech_real[ip] / tech_real[b0 - 1] - 1), 1)
            add('boom_tech_share', 100 * float(np.max(s['sect'][win, 4] / E[win])), 1)
            if b1 + 24 < n:  # bust observable
                diag['n_busts'] += 1
                end = min(n, ip + 37)
                add('boom_bust', 100 * (tech_real[ip:end].min() / tech_real[ip] - 1), 1)
                ipm = b0 + int(np.argmax(mkt_real[win]))
                endm = min(n, ipm + 37)
                add('boom_bust_mkt', 100 * (mkt_real[ipm:endm].min() / mkt_real[ipm] - 1), 1)
    out = {}
    for k in res:
        v, wt = np.array(res[k]), np.array(weights[k])
        out[k] = (float((v * wt).sum() / wt.sum()), float(v.min()), float(v.max()), len(v)) if len(v) else None
    out['rec_drawdown'] = (float(np.median(rec_dd)), float(np.min(rec_dd)), float(np.max(rec_dd)), len(rec_dd)) if rec_dd else None
    return out, diag


def flag(t, v):
    if v is None:
        return 'n/a'
    x = v[0]
    dist = max(t['lo'] - x, x - t['hi'], 0.0)
    if dist == 0:
        return 'ok'
    q = dist / t['tol']
    return 'minor' if q <= 1 else ('material' if q <= 3 else 'severe')


def main():
    if TARGETS_OUT:
        os.makedirs(os.path.dirname(os.path.abspath(TARGETS_OUT)), exist_ok=True)
        with open(TARGETS_OUT, 'w') as fh:
            json.dump(dict(dimension='equity market stylised facts', script='scripts/facts_equity.py',
                           gap_rule='ok inside [lo,hi]; else distance/tol: minor<=1, material<=3, severe>3',
                           targets=TARGETS), fh, indent=1)
    specs_std = [(HH, MONTHS, s) for s in range(1, SEEDS + 1)]
    specs_long = [(LHH, LMONTHS, 100 + s) for s in range(1, LSEEDS + 1)]
    import time
    t0 = time.time()
    with ThreadPoolExecutor(JOBS) as ex:
        allruns = list(ex.map(run, specs_std + specs_long))
    wall = time.time() - t0
    out, diag = measure(allruns[:len(specs_std)], allruns[len(specs_std):])
    print(f'CLI {CLI}; {SEEDS} seeds x {MONTHS} months x {HH} hh + {LSEEDS} seeds x {LMONTHS} months x {LHH} hh; '
          f'first {DROP} months dropped; ECONSIM_CFG={os.environ.get("ECONSIM_CFG", "")!r}; runs wall {wall:.0f}s')
    print(f'diagnostics: recession share {np.mean(diag["rec_share"]):.1f}% of months (US 1984-2024 7%, 1950-2024 13%), '
          f'{diag["n_recessions"]} recessions; tech booms >=24m {diag["n_booms"]} (busts observable {diag["n_busts"]}); '
          f'price-return AC1 {np.mean(diag["price_ac1"]):.2f}, ann. vol {np.mean(diag["price_vol"]):.1f}%; '
          f'sd dlog real dividends {np.mean(diag["d_sd"]) if diag["d_sd"] else float("nan"):.1f}% (US 1950-2024 5.9%)')
    w1, w2 = 72, 14
    for title, core in (('KEY FACTS', True), ('SUPPLEMENTARY CHECKS', False)):
        print(f'\n{title}\n{"fact":<{w1}} | {"target (US)":<{w2}} | {"model [min..max over runs, n]":<34} | gap')
        print('-' * (w1 + w2 + 48))
        counts = {}
        for t in TARGETS:
            if t.get('core', True) != core:
                continue
            v = out.get(t['key'])
            tgt = f'{t["lo"]:g} .. {t["hi"]:g}'
            mv = 'not measurable' if v is None else f'{v[0]:8.3g}  [{v[1]:.3g}..{v[2]:.3g}, {v[3]}]'
            f = flag(t, v)
            counts[f] = counts.get(f, 0) + 1
            print(f'{t["fact"]:<{w1}} | {tgt:<{w2}} | {mv:<34} | {f}')
        print('summary: ' + ', '.join(f'{k} {v}' for k, v in sorted(counts.items())))
    if os.environ.get('EQF_JSON_OUT'):
        with open(os.environ['EQF_JSON_OUT'], 'w') as fh:
            json.dump(dict(model=out, diag={k: (v if not isinstance(v, list) else float(np.mean(v)) if v else None) for k, v in diag.items()},
                           flags={t['key']: flag(t, out.get(t['key'])) for t in TARGETS}), fh, indent=1)


if __name__ == '__main__':
    main()
