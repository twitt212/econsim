#!/usr/bin/env python3
"""Paired H5 statistics from per-seed `hvs` output (b6-joint).

Usage:
  hvs_paired.py T.tsv [B.tsv]

T.tsv and B.tsv are written by `HVS_PERSEED_OUT=FILE hvs N MONTHS SEEDS` (header: scenario, seed, m3 m6 m12 m18 m24 m36;
each cell is the shock-minus-base rental-vacancy difference for one seed, or '-' past the run length).

For scenario "Policy +100bp 12m" let m(h) be the mean over seeds at horizon h:
  H5max  = max over h in {3, 6, 12, 18, 24} of m(h)
  H5mean = (m(12) + m(18) + m(24)) / 3
The same for "TP +100bp hl24" gives H5t (reported only).

With B: d = stat(T) - stat(B), with a paired leave-one-seed-out jackknife SE:
  d_(i) = stat(T without i) - stat(B without i);  SE = sqrt((n-1)/n * sum_i (d_(i) - mean d_(.))^2).
The seed lists of T and B must match.
"""
import csv
import math
import sys

POLICY = 'Policy +100bp 12m'
TP = 'TP +100bp hl24'
HS = ['m3', 'm6', 'm12', 'm18', 'm24', 'm36']


def load(path):
    """Return {scenario: {seed: {col: value or None}}} with the seed order kept."""
    out = {}
    with open(path) as fh:
        for row in csv.DictReader(fh, delimiter='\t'):
            vals = {}
            for h in HS:
                v = row[h].strip()
                vals[h] = None if v == '-' else float(v)
            out.setdefault(row['scenario'], {})[int(row['seed'])] = vals
    return out


def seed_mean(data, scen, h):
    """Mean over seeds of the horizon-h difference, for one scenario (seeds in order)."""
    vals = [s[h] for _, s in sorted(data[scen].items())]
    if any(v is None for v in vals):
        raise SystemExit(f'{scen} {h}: missing values')
    return sum(vals) / len(vals)


def stats(data, scen):
    """(H5max, H5mean) for the policy scenario, or the max-only statistic for TP."""
    m = {h: seed_mean(data, scen, h) for h in ('m3', 'm6', 'm12', 'm18', 'm24')}
    mx = max(m[h] for h in ('m3', 'm6', 'm12', 'm18', 'm24'))
    mean3 = (m['m12'] + m['m18'] + m['m24']) / 3
    return mx, mean3


def stat_per_seed(data, scen, seeds):
    """Stat over a subset of seeds (for the jackknife)."""
    def sub_mean(h):
        return sum(data[scen][s][h] for s in seeds) / len(seeds)
    mm = {h: sub_mean(h) for h in ('m3', 'm6', 'm12', 'm18', 'm24')}
    return max(mm.values()), (mm['m12'] + mm['m18'] + mm['m24']) / 3


def jk(tdata, bdata, scen, idx):
    seeds = sorted(tdata[scen])
    if sorted(bdata[scen]) != seeds:
        raise SystemExit('seed lists differ between T and B')
    n = len(seeds)
    reps = []
    for i in seeds:
        keep = [s for s in seeds if s != i]
        t = stat_per_seed(tdata, scen, keep)[idx]
        b = stat_per_seed(bdata, scen, keep)[idx]
        reps.append(t - b)
    mean = sum(reps) / n
    se = math.sqrt((n - 1) / n * sum((r - mean) ** 2 for r in reps))
    return se


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    t = load(argv[1])
    t_pol, t_tp = stats(t, POLICY), stats(t, TP)[0]
    print(f'T {argv[1]}')
    print(f'  H5max {t_pol[0]:+.4f}  H5mean {t_pol[1]:+.4f}  H5t {t_tp:+.4f}  (seeds {len(t[POLICY])})')
    if len(argv) >= 3:
        b = load(argv[2])
        b_pol, b_tp = stats(b, POLICY), stats(b, TP)[0]
        d_max, d_mean = t_pol[0] - b_pol[0], t_pol[1] - b_pol[1]
        se_max, se_mean = jk(t, b, POLICY, 0), jk(t, b, POLICY, 1)
        se_tp = jk(t, b, TP, 0)
        print(f'B {argv[2]}')
        print(f'  H5max {b_pol[0]:+.4f}  H5mean {b_pol[1]:+.4f}  H5t {b_tp:+.4f}')
        print(f'  dH5max {d_max:+.4f} +- {se_max:.4f}')
        print(f'  dH5mean {d_mean:+.4f} +- {se_mean:.4f}')
        print(f'  dH5t {t_tp - b_tp:+.4f} +- {se_tp:.4f}  (reported only)')
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv))
