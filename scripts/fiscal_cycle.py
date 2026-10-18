#!/usr/bin/env python3
"""Fiscal cyclicality: elasticity of federal tax revenue to real GDP over the business cycle (fix-fiscal).
HP-filters (quarterly, lambda=1600) log tax revenue and log real GDP, then regresses the revenue cycle on the GDP
cycle. The slope is the revenue elasticity; US federal evidence is about 1.5-2.0 (Mourre et al., OECD; CBO), and the
target here is at least 1.2. Also reports the corporate and personal-income shares of revenue, cycle correlations, and
the UI + means-tested proxy as a share of GDP.
Usage: MOMENTS_HH=20000 python3 scripts/fiscal_cycle.py [seeds=8] [months=240]
(honours ECONSIM_CFG and ECONSIM_CLI for another binary; build --release first, as in moments.py)."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

CLI = os.environ.get('ECONSIM_CLI', os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli'))
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240


def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', os.environ.get('MOMENTS_HH', '20000'), str(MONTHS), str(seed), '--json'],
                         capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])


def hp(y, lam=1600.0):
    n = len(y)
    I = np.eye(n)
    D = np.diff(I, 2, axis=0)
    trend = np.linalg.solve(I + lam * D.T @ D, y)
    return y - trend


def quarterly(h, key):
    v = np.array([m[key] for m in h], dtype=float)
    return v[: len(v) // 3 * 3].reshape(-1, 3).mean(axis=1)


with ThreadPoolExecutor(10) as ex:
    runs = list(ex.map(run, range(1, SEEDS + 1)))

elast, corr, sd_ratio, rev_gdp, means_gdp = [], [], [], [], []
for h in runs:
    h = h[12:]  # skip the first year after burn-in
    y = hp(np.log(quarterly(h, 'gdp_real_bn_ar')))
    r = hp(np.log(quarterly(h, 'gov_revenue_bn_ar')))
    slope = np.polyfit(y, r, 1)[0]
    elast.append(slope)
    corr.append(np.corrcoef(r, y)[0, 1])
    sd_ratio.append(r.std() / y.std())
    # Means-tested support: outlays less interest, purchases (incl. in-kind health), pensions and unemployment
    # insurance (ui_gdp, reported in % of GDP by the JSON). The residual is the means-tested and other transfers.
    gdp = np.array([m['gdp_nominal_bn_ar'] for m in h], dtype=float)
    mt = np.array([m['gov_outlays_bn_ar'] - m['gov_interest_bn_ar'] - m['government_bn_ar']
                   - m['health_gdp'] / 100 * m['gdp_nominal_bn_ar']
                   - m['social_security_gdp'] / 100 * m['gdp_nominal_bn_ar']
                   - m['ui_gdp'] / 100 * m['gdp_nominal_bn_ar'] for m in h], dtype=float)
    means_gdp.append(100 * mt.mean() / gdp.mean())
    rev_gdp.append(100 * np.array([m['gov_revenue_bn_ar'] / m['gdp_nominal_bn_ar'] for m in h]).mean())

e = np.array(elast)
print(f"{'moment':48s} {'model':>8s} {'(sd over seeds)':>16s}")
rows = [
    ('tax revenue elasticity to real GDP cycle (target >= 1.2)', e),
    ('corr(tax-revenue cycle, GDP cycle)', np.array(corr)),
    ('sd(tax-revenue cycle) / sd(GDP cycle)', np.array(sd_ratio)),
    ('mean revenue / GDP, %', np.array(rev_gdp)),
    ('means-tested + other transfers (excl. UI, SS), % of GDP', np.array(means_gdp)),
]
for name, a in rows:
    print(f"{name:48s} {a.mean():8.2f} {a.std():16.2f}")
