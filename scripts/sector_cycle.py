#!/usr/bin/env python3
"""Sector business cycles: employment cycle volatility relative to total employment and correlation with the GDP
cycle, by sector (quarterly HP cycles, λ=1600; 8 seeds x 20 years, 20,000 households), against BLS CES targets
(docs/SECTORS.md). Usage: python3 scripts/sector_cycle.py [seeds=8] [months=240]   (honours ECONSIM_CFG)."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

CLI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
HH = os.environ.get('SECTOR_HH', '20000')
LABELS = {5: ['Energy', 'Food', 'Goods', 'Services', 'Construction'],
          9: ['Energy', 'Food', 'DurableGoods', 'NondurableGoods', 'Tech', 'LeisureHospitality', 'HealthEducation',
              'OtherServices', 'Construction']}
# (relative sd, corr with GDP) targets from docs/SECTORS.md
TARGETS = {'Construction': (3.0, 0.8), 'DurableGoods': (2.75, 0.85), 'Energy': (2.5, 0.3), 'Tech': (1.5, 0.6),
           'LeisureHospitality': (1.2, 0.7), 'NondurableGoods': (1.2, 0.7), 'OtherServices': (1.2, 0.8),
           'Food': (0.5, 0.3), 'HealthEducation': (0.3, 0.0), 'Goods': (1.8, 0.8), 'Services': (0.8, 0.7)}

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
ns = len(runs[0][0]['employment_by_sector'])
labels = LABELS.get(ns, [f'sector{i}' for i in range(ns)])
rel = np.zeros(ns); cor = np.zeros(ns); share = np.zeros(ns)
for h in runs:
    y = hp(np.log(q([m['gdp_real_bn_ar'] for m in h])))
    tot = hp(np.log(q([sum(m['employment_by_sector']) for m in h])))
    for s in range(ns):
        e = q([m['employment_by_sector'][s] for m in h])
        c = hp(np.log(np.maximum(e, 1e-9)))
        rel[s] += c.std() / tot.std() / len(runs)
        cor[s] += np.corrcoef(c, y)[0, 1] / len(runs)
        share[s] += np.mean([m['employment_by_sector'][s] / sum(m['employment_by_sector']) for m in h]) / len(runs)
print(f"{'sector':20s} {'emp share':>9s} {'rel sd':>7s} {'(BLS)':>6s} {'corr GDP':>8s} {'(BLS)':>6s}")
for s in range(ns):
    t = TARGETS.get(labels[s], (float('nan'), float('nan')))
    print(f"{labels[s]:20s} {share[s]:9.3f} {rel[s]:7.2f} {t[0]:6.2f} {cor[s]:8.2f} {t[1]:6.2f}")
