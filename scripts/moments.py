#!/usr/bin/env python3
"""Business-cycle stylised facts from baseline runs vs US data (quarterly, HP-filtered λ=1600).
Usage: python3 scripts/moments.py [seeds=8] [months=240]   (honours ECONSIM_CFG; build --release first).
US reference moments: Stock & Watson (1999), Ravn & Uhlig (2002), Shimer (2005), 1954–2019 quarterly data."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np
CLI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240

def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', os.environ.get('MOMENTS_HH', '5000'), str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])

def hp(y, lam=1600.0):
    n = len(y); I = np.eye(n); D = np.diff(I, 2, axis=0)
    trend = np.linalg.solve(I + lam * D.T @ D, y)
    return y - trend

def quarterly(h, key, f=lambda m, k: m[k]):
    v = np.array([f(m, key) for m in h])
    v = v[: len(v) // 3 * 3].reshape(-1, 3).mean(axis=1)
    return v

with ThreadPoolExecutor(10) as ex:
    runs = list(ex.map(run, range(1, SEEDS + 1)))

rows = {k: [] for k in ['sd_y', 'sd_c', 'sd_i', 'sd_u', 'sd_pi', 'corr_cy', 'corr_iy', 'corr_uy', 'corr_vu', 'okun', 'ac_y', 'ac_u', 'corr_piy', 'lag_piy', 'lead_piy']}
for h in runs:
    h = h[12:]  # skip the first year after burn-in
    y = hp(np.log(quarterly(h, 'gdp_real_bn_ar')))
    c = hp(np.log(quarterly(h, 'consumption_real_bn_ar')))
    i = hp(np.log(quarterly(h, 'investment_real_bn_ar')))
    u = hp(quarterly(h, 'unemployment_rate'))
    v = hp(quarterly(h, 'vacancy_rate'))
    pi = hp(quarterly(h, 'core_inflation_yoy'))
    rows['sd_y'].append(100 * y.std()); rows['sd_c'].append(c.std() / y.std()); rows['sd_i'].append(i.std() / y.std())
    rows['sd_u'].append(u.std()); rows['sd_pi'].append(pi.std())
    rows['corr_cy'].append(np.corrcoef(c, y)[0, 1]); rows['corr_iy'].append(np.corrcoef(i, y)[0, 1])
    rows['corr_uy'].append(np.corrcoef(u, y)[0, 1]); rows['corr_vu'].append(np.corrcoef(v, u)[0, 1])
    rows['okun'].append(np.polyfit(100 * y, u, 1)[0])
    rows['ac_y'].append(np.corrcoef(y[1:], y[:-1])[0, 1]); rows['ac_u'].append(np.corrcoef(u[1:], u[:-1])[0, 1])
    rows['corr_piy'].append(np.corrcoef(pi, y)[0, 1])
    lagged = [np.corrcoef(y[:len(y) - k], pi[k:])[0, 1] for k in range(1, 9)]
    leading = [np.corrcoef(y[k:], pi[:len(pi) - k])[0, 1] for k in range(1, 9)]
    rows['lag_piy'].append(max(lagged)); rows['lead_piy'].append(min(leading))

US = {'sd_y': ('GDP cycle sd, %', 1.5), 'sd_c': ('consumption sd / GDP sd', 0.8), 'sd_i': ('investment sd / GDP sd', 3.0),
      'sd_u': ('unemployment cycle sd, pp', 0.75), 'sd_pi': ('core inflation cycle sd, pp', 0.6),
      'corr_cy': ('corr(C, GDP)', 0.85), 'corr_iy': ('corr(I, GDP)', 0.90), 'corr_uy': ('corr(u, GDP)', -0.85),
      'corr_vu': ('corr(vacancies, u) — Beveridge', -0.90), 'okun': ('Okun: d u per 1% GDP gap', -0.45),
      'ac_y': ('GDP autocorrelation (1q)', 0.85), 'ac_u': ('unemployment autocorrelation (1q)', 0.90), 'corr_piy': ('corr(core inflation, GDP), same quarter', 0.2),
      'lag_piy': ('max corr(GDP_t, inflation_t+k), k=1..8', 0.5), 'lead_piy': ('min corr(GDP_t, inflation_t-k), k=1..8', -0.5)}
print(f"{'moment':40s} {'model':>8s} {'(sd over seeds)':>16s} {'US':>7s}")
for k, (name, us) in US.items():
    a = np.array(rows[k])
    print(f"{name:40s} {a.mean():8.2f} {a.std():16.2f} {us:7.2f}")
