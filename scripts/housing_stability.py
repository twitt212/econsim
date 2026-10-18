#!/usr/bin/env python3
"""Multi-seed, no-shock housing stability and burn-in parity test (fix-housing).

Usage: python3 scripts/housing_stability.py [seeds=16] [months=240] [master_cli=<path>]
  - Runs the branch CLI (target/release/econsim-cli) and, if given, a master CLI for comparison.
  - Stability: sd of monthly HPI growth (annualised pp), mean and worst per-seed HPI drawdown,
    rent level vs master at y10 / y20, shelter and core at y10 / y20.
  - Parity: month-0 (first reported month) policy, mortgage, core PCE, rent and HPI levels, master vs branch.
Honours ECONSIM_CFG and HOUSING_HH (default 20000). Build --release first.
"""
import json, math, os, statistics, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
CLI = os.path.join(HERE, '..', 'target', 'release', 'econsim-cli')
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 16
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
MASTER = next((a.split('=', 1)[1] for a in sys.argv[3:] if a.startswith('master_cli=')), None)
HH = os.environ.get('HOUSING_HH', '20000')


def run(binary, seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([binary, 'run', HH, str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])


def runs(binary):
    with ThreadPoolExecutor(8) as ex:
        return list(ex.map(lambda s: run(binary, s), range(1, SEEDS + 1)))


def drawdown(series):
    peak, worst = series[0], 0.0
    for x in series:
        peak = max(peak, x)
        worst = min(worst, x / peak - 1.0)
    return worst * 100.0


def stats(H):
    sds, dds, hpa_max, hpa_min = [], [], [], []
    for h in H:
        g = [m['hpi_growth_yoy'] for m in h]
        sds.append(statistics.pstdev(g))
        dds.append(drawdown([m['hpi'] for m in h]))
        hpa_max.append(max(g))
        hpa_min.append(min(g))
    rent = lambda k: statistics.mean(h[k]['rent_index'] for h in H)
    shelter = lambda a, b: statistics.mean(m['shelter_inflation_yoy'] for h in H for m in h[a:b])
    core = lambda a, b: statistics.mean(m['core_inflation_yoy'] for h in H for m in h[a:b])
    out = {
        'hpa_sd_pp': statistics.mean(sds),
        'hpa_worst_sd_pp': max(sds),
        'hpa_range_pp': [min(hpa_min), max(hpa_max)],
        'drawdown_mean_pct': statistics.mean(dds),
        'drawdown_worst_pct': min(dds),
        'shelter_y10': shelter(108, 120), 'shelter_y20': shelter(228, 240),
        'core_y10': core(108, 120), 'core_y20': core(228, 240),
        'rent_y10': rent(119), 'rent_y20': rent(min(239, len(H[0]) - 1)),
    }
    m0 = {k: statistics.mean(h[0][k] for h in H) for k in ['policy_rate', 'mortgage_rate', 'core_pce_inflation_yoy', 'rent_index', 'hpi', 'shelter_inflation_yoy']}
    out['m0'] = m0
    return out


def fmt(label, s):
    print(f"{label}: hpa sd {s['hpa_sd_pp']:.2f} (worst seed {s['hpa_worst_sd_pp']:.2f}); hpa range {s['hpa_range_pp'][0]:.1f}..{s['hpa_range_pp'][1]:.1f}; "
          f"drawdown mean {s['drawdown_mean_pct']:.1f}% worst {s['drawdown_worst_pct']:.1f}%")
    print(f"   shelter y10/y20 {s['shelter_y10']:.2f}/{s['shelter_y20']:.2f}; core y10/y20 {s['core_y10']:.2f}/{s['core_y20']:.2f}; "
          f"rent index y10 {s['rent_y10']:.1f} y20 {s['rent_y20']:.1f}")
    print('   m0: ' + ', '.join(f'{k} {v:.3f}' for k, v in s['m0'].items()))


B = runs(CLI)
b = stats(B)
fmt(f'branch ({SEEDS} seeds, {MONTHS}m)', b)
if MASTER:
    M = runs(MASTER)
    m = stats(M)
    fmt('master', m)
    print(f"rent vs master: y10 {100*(b['rent_y10']/m['rent_y10']-1):+.2f}%  y20 {100*(b['rent_y20']/m['rent_y20']-1):+.2f}%")
    print('m0 diff (branch - master): ' + ', '.join(f"{k} {b['m0'][k]-m['m0'][k]:+.3f}" for k in b['m0']))
print(json.dumps({'branch': b}, indent=1))
