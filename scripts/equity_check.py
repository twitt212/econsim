#!/usr/bin/env python3
"""Equity-market realism (8 seeds x 20 years, 20,000 households, first year after burn-in dropped).
Usage: python3 scripts/equity_check.py [path to econsim-cli]   (honours ECONSIM_CFG; build --release first).

Definitions
  P/E          = Fund equity value / annual after-tax corporate profit. Profit is recovered from profit_share (percent of
                 nominal GDP) x gdp_nominal_bn_ar, so the check runs on baseline builds too.
  equity/GDP   = equity_value_bn / gdp_nominal_bn_ar.
  real return  = non-overlapping 12-month real returns (sampled every 12 months), deflated by CPI inflation_yoy.
                 Two measures: 'price' = fund_share_price ratio (ex-dividend, includes the Fund's deposits and bonds);
                 'total' = equity_return_12m (capital gain + equity dividends paid, equity only). 'total' is only
                 reported by builds that expose equity_return_12m.
  cycles       = quarterly averages, HP filter (lambda 1600) on logs (GDP, P/E) or levels (ERP, percent).

US benchmarks: S&P 500 trailing P/E ~ 15-16 long run, ~25 in 2025-26; real total-return sd ~16-18% a year
  (Shiller data); ERP countercyclical (Campbell & Shiller 1988; Cochrane 2011), corr with GDP cycle ~ -0.3 to -0.6.
"""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

CLI = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = 8
MONTHS = 240
HH = os.environ.get('EQUITY_HH', '20000')
BURN = 12


def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', HH, str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])


def hp(y, lam=1600.0):
    n = len(y)
    I = np.eye(n)
    D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)


def quarterly(h, f):
    v = np.array([f(m) for m in h])
    return v[: len(v) // 3 * 3].reshape(-1, 3).mean(axis=1)


def pe(m):
    profit_bn = m['profit_share'] / 100.0 * m['gdp_nominal_bn_ar']
    return m['equity_value_bn'] / profit_bn if profit_bn > 0 else np.nan


with ThreadPoolExecutor(8) as ex:
    runs = [h[BURN:] for h in ex.map(run, range(1, SEEDS + 1))]

has_erp = 'equity_risk_premium' in runs[0][0]
has_total = 'equity_return_12m' in runs[0][0]

pe_mean, pe_sd_time, eq_gdp, sd_price, sd_total, corr_erp, corr_pe, erp_mean, erp_sd, pe_cyc_sd = ([] for _ in range(10))
for h in runs:
    p = np.array([pe(m) for m in h])
    pe_mean.append(np.nanmean(p))
    pe_sd_time.append(np.nanstd(quarterly(h, pe)))  # sd of quarterly-average P/E over time (levels, not cycle)
    eq_gdp.append(np.mean([m['equity_value_bn'] / m['gdp_nominal_bn_ar'] for m in h]))
    yrs = list(range(11, len(h), 12))  # months 12, 24, ... (end of each year) within the sample
    ret = [(h[t]['fund_share_price'] / h[t - 12]['fund_share_price']) / (1 + h[t]['inflation_yoy'] / 100.0) - 1 for t in yrs if t >= 12]
    sd_price.append(np.std(ret, ddof=1))
    if has_total:
        tot = [(1 + h[t]['equity_return_12m'] / 100.0) / (1 + h[t]['inflation_yoy'] / 100.0) - 1 for t in yrs]
        sd_total.append(np.std(tot, ddof=1))
    y = hp(np.log(quarterly(h, lambda m: m['gdp_real_bn_ar'])))
    c_pe = hp(np.log(quarterly(h, pe)))
    corr_pe.append(np.corrcoef(c_pe, y)[0, 1])
    pe_cyc_sd.append(100 * c_pe.std())
    if has_erp:
        e = quarterly(h, lambda m: m['equity_risk_premium'])
        erp_mean.append(e.mean())
        erp_sd.append(hp(e).std())  # percent points
        corr_erp.append(np.corrcoef(hp(e), y)[0, 1])


def f(xs, d=2):
    return f'{np.mean(xs):{d + 4}.{d}f} (sd over seeds {np.std(xs):.{d}f})'


print(f'seeds {SEEDS} x {MONTHS - BURN} months, {HH} households, CLI {os.path.basename(CLI)}')
print(f'mean P/E                           {f(pe_mean)}')
print(f'P/E sd over time (quarterly levels) {f(pe_sd_time)}')
print(f'P/E cycle sd (HP, percent)         {f(pe_cyc_sd)}')
print(f'equity value / GDP                 {f(eq_gdp)}')
print(f'annual real return sd, price       {f(sd_price, 3)}  (ex-dividend Fund share price)')
if has_total:
    print(f'annual real return sd, total       {f(sd_total, 3)}  (equity capital gain + dividends)')
if has_erp:
    print(f'mean ERP, percent                  {f(erp_mean)}')
    print(f'ERP cycle sd, pp (HP)              {f(erp_sd)}')
    print(f'corr(ERP cycle, GDP cycle)         {f(corr_erp, 3)}')
print(f'corr(P/E cycle, GDP cycle)         {f(corr_pe, 3)}')
