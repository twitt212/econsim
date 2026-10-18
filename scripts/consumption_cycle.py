#!/usr/bin/env python3
"""Consumption-category cycles: real household purchases by consumer sector, cycle sd relative to the GDP cycle and
correlation with GDP (quarterly HP cycles, λ=1600; 8 seeds x 20 years, 20,000 households), against US targets
(Stock & Watson 1999, HP-filtered quarterly; docs/DESIGN.md §8). Usage: python3 scripts/consumption_cycle.py [seeds=8]
[months=240]   (honours ECONSIM_CFG and CONSUMPTION_HH; build --release first)."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

CLI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
HH = os.environ.get('CONSUMPTION_HH', '20000')
LABELS = ['Energy', 'Food', 'DurableGoods', 'NondurableGoods', 'Tech', 'LeisureHospitality', 'HealthEducation',
          'OtherServices']
# (relative sd, corr with GDP) US targets; services and leisure per the stage-3 brief
TARGETS = {'DurableGoods': (2.9, 0.8), 'NondurableGoods': (0.6, 0.7), 'Energy': (0.5, 0.3), 'Food': (0.5, 0.3),
           'Tech': (1.5, 0.6), 'LeisureHospitality': (1.0, 0.7), 'HealthEducation': (0.5, 0.3),
           'OtherServices': (0.5, 0.7), 'Total': (0.8, 0.85)}

def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', HH, str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])

def hp(y, lam=1600.0):
    n = len(y); I = np.eye(n); D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)

def q(v):
    v = np.array(v, float)
    return v[: len(v) // 3 * 3].reshape(-1, 3).mean(axis=1)

with ThreadPoolExecutor(10) as ex:
    runs = list(ex.map(run, range(1, SEEDS + 1)))
ns = len(LABELS)
rel = np.zeros(ns + 1); cor = np.zeros(ns + 1); rel_sd_pct = np.zeros(ns + 1)
for h in runs:
    h = h[12:]  # skip the first year after burn-in
    y = hp(np.log(q([m['gdp_real_bn_ar'] for m in h])))
    series = [[m['consumption_by_sector_real'][s] for m in h] for s in range(ns)]
    series.append([m['consumption_real_bn_ar'] for m in h])
    for s in range(ns + 1):
        c = hp(np.log(np.maximum(q(series[s]), 1e-9)))
        rel[s] += c.std() / y.std() / len(runs)
        cor[s] += np.corrcoef(c, y)[0, 1] / len(runs)
print(f"{'category':20s} {'rel sd':>7s} {'(US)':>6s} {'corr GDP':>8s} {'(US)':>6s}")
for s in range(ns + 1):
    name = LABELS[s] if s < ns else 'Total'
    t = TARGETS[name]
    print(f"{name:20s} {rel[s]:7.2f} {t[0]:6.2f} {cor[s]:8.2f} {t[1]:6.2f}")
