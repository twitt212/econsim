#!/usr/bin/env python3
"""b9-rates M-M measurement: the mortgage-10y spread over the cycle (b9_rates_spec section 4).

usage: mtg_spread_check.py RUNDIR CELL [--seeds 1-32] [--base CELL] [--json OUT]

For each seed read RUNDIR/<CELL>_s<seed>.json.gz (the file holds the JSON list of 240 monthly records), drop the first
12 months and compute, in percent, s = mortgage_rate - yields[3] (mortgage rate minus the 10y yield). Per seed: mean,
population sd, min, max, lag-1 autocorrelation of s, and correlations of s with credit_spread and lender_premium.

Prints the mean over seeds of each per-seed statistic with SE = sample sd / sqrt(n), the pooled 5th/50th/95th
percentiles and min/max over all seed-months, and one PASS/FAIL line per row:
  RM-1  mean of (mortgage rate - 10y) in [1.60, 1.80]
  RM-2  sd of the spread in [0.28, 0.42]
  RM-3  corr(spread, lender_premium) >= 0.30
  RM-4  pooled p5 >= 1.15, p95 <= 2.40, min >= 0.95, max <= 3.40
RM-5 (lag-1 autocorrelation) and corr(spread, credit_spread) are printed as reports.
With --base CELL the same statistics are printed for the base cell and as paired differences (mean of the per-seed
differences, SE = sd(differences) / sqrt(n)). Standard library plus numpy only.

Self-check: on the cell M9 (the constant 1.70 pp spread) the sd is 0.001 or less (RM-2 FAIL) and the pooled p5 = p95 = 1.700.
"""
import argparse, gzip, json, math, os, sys

import numpy as np

SKIP = 12
END = 240
BANDS = {'RM-1': (1.60, 1.80), 'RM-2': (0.28, 0.42), 'RM-3_min': 0.30,
         'RM-4': {'p5_min': 1.15, 'p95_max': 2.40, 'min_min': 0.95, 'max_max': 3.40}}


def parse_seeds(text):
    out = []
    for part in text.split(','):
        if '-' in part:
            a, b = part.split('-')
            out += list(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def load(rundir, cell, seed):
    path = os.path.join(rundir, f'{cell}_s{seed}.json.gz')
    with gzip.open(path, 'rt') as fh:
        rec = json.loads(fh.read())
    if len(rec) < END:
        raise SystemExit(f'{path}: {len(rec)} records, need {END}')
    return rec


def corr(a, b):
    if np.std(a) == 0.0 or np.std(b) == 0.0:
        return float('nan')
    return float(np.corrcoef(a, b)[0, 1])


def seed_stats(rec):
    h = rec[SKIP:END]
    s = np.array([m['mortgage_rate'] - m['yields'][3] for m in h], float)
    cs = np.array([m['credit_spread'] for m in h], float)
    lp = np.array([m['lender_premium'] for m in h], float)
    st = {
        'mean': float(np.mean(s)),
        'sd': float(np.std(s)),
        'min': float(np.min(s)),
        'max': float(np.max(s)),
        'ac1': corr(s[:-1], s[1:]),
        'corr_credit': corr(s, cs),
        'corr_lp': corr(s, lp),
    }
    return st, s


def se_of(vals):
    v = np.array(vals, float)
    n = len(v)
    if n < 2:
        return float('nan')
    return float(np.std(v, ddof=1) / math.sqrt(n))


def cell_summary(rundir, cell, seeds):
    rows, pooled = [], []
    for sd_ in seeds:
        st, s = seed_stats(load(rundir, cell, sd_))
        rows.append(st)
        pooled.append(s)
    allv = np.concatenate(pooled)
    out = {'n': len(seeds)}
    for k in ('mean', 'sd', 'min', 'max', 'ac1', 'corr_credit', 'corr_lp'):
        vals = [r[k] for r in rows]
        out[k] = float(np.nanmean(vals))
        out[k + '_se'] = se_of([v for v in vals if not math.isnan(v)])
        out[k + '_seeds'] = vals
    out['p5'] = float(np.percentile(allv, 5))
    out['p50'] = float(np.percentile(allv, 50))
    out['p95'] = float(np.percentile(allv, 95))
    out['pooled_min'] = float(np.min(allv))
    out['pooled_max'] = float(np.max(allv))
    return out


def verdicts(c):
    rm1 = BANDS['RM-1']
    rm2 = BANDS['RM-2']
    r4 = BANDS['RM-4']
    v = {
        'RM-1': rm1[0] <= c['mean'] <= rm1[1],
        'RM-2': rm2[0] <= c['sd'] <= rm2[1],
        'RM-3': (not math.isnan(c['corr_lp'])) and c['corr_lp'] >= BANDS['RM-3_min'],
        'RM-4': (c['p5'] >= r4['p5_min'] and c['p95'] <= r4['p95_max']
                 and c['pooled_min'] >= r4['min_min'] and c['pooled_max'] <= r4['max_max']),
    }
    return v


def fmt(x, nd=4):
    return 'nan' if (x is None or (isinstance(x, float) and math.isnan(x))) else f'{x:.{nd}f}'


def print_cell(label, c):
    print(f'== {label}: n={c["n"]} seeds, months {SKIP}-{END - 1}')
    print(f'  mean(mortgage - 10y) = {fmt(c["mean"])} (SE {fmt(c["mean_se"])})   [RM-1 band 1.60-1.80]')
    print(f'  sd of spread         = {fmt(c["sd"])} (SE {fmt(c["sd_se"])})   [RM-2 band 0.28-0.42]')
    print(f'  corr(spread, lender_premium) = {fmt(c["corr_lp"])} (SE {fmt(c["corr_lp_se"])})   [RM-3 >= 0.30]')
    print(f'  lag-1 autocorr       = {fmt(c["ac1"])} (SE {fmt(c["ac1_se"])})   [RM-5 report, band 0.80-0.98]')
    print(f'  corr(spread, credit_spread) = {fmt(c["corr_credit"])} (SE {fmt(c["corr_credit_se"])})   [report]')
    print(f'  per-seed min / max   = {fmt(c["min"])} / {fmt(c["max"])}')
    print(f'  pooled p5 / p50 / p95 = {fmt(c["p5"])} / {fmt(c["p50"])} / {fmt(c["p95"])}   '
          f'pooled min / max = {fmt(c["pooled_min"])} / {fmt(c["pooled_max"])}   [RM-4]')
    v = verdicts(c)
    print(f'  RM-1 {"PASS" if v["RM-1"] else "FAIL"}  RM-2 {"PASS" if v["RM-2"] else "FAIL"}  '
          f'RM-3 {"PASS" if v["RM-3"] else "FAIL"}  RM-4 {"PASS" if v["RM-4"] else "FAIL"}')
    return v


def print_diff(c, b):
    print('== paired difference (cell minus base), mean over seeds, SE = sd(diff)/sqrt(n)')
    for k in ('mean', 'sd', 'corr_lp', 'corr_credit', 'ac1', 'min', 'max'):
        d = [x - y for x, y in zip(c[k + '_seeds'], b[k + '_seeds'])]
        d = [x for x in d if not math.isnan(x)]
        m = float(np.mean(d)) if d else float('nan')
        print(f'  {k:12s} diff = {fmt(m)} (SE {fmt(se_of(d))})')


def main():
    ap = argparse.ArgumentParser(description='mortgage-10y spread statistics and RM-1..RM-4 verdicts')
    ap.add_argument('rundir')
    ap.add_argument('cell')
    ap.add_argument('--seeds', default='1-32')
    ap.add_argument('--base', default=None)
    ap.add_argument('--json', default=None)
    a = ap.parse_args()
    seeds = parse_seeds(a.seeds)
    c = cell_summary(a.rundir, a.cell, seeds)
    v = print_cell(a.cell, c)
    out = {'cell': c, 'verdicts': v}
    if a.base:
        b = cell_summary(a.rundir, a.base, seeds)
        vb = print_cell(a.base, b)
        print_diff(c, b)
        out['base'] = b
        out['base_verdicts'] = vb
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(out, fh, indent=1)
    return 0


if __name__ == '__main__':
    sys.exit(main())
