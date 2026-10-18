#!/usr/bin/env python3
"""world_yus_offset (N1), cross-check from the ABM's monthly output: the mean over months 0-239 and over seeds of the US output
gap that the blocs see, y_US(t) = 100 okun (u*_t - mean(u_{t-1}, u_{t-2}, u_{t-3})) in pp, with okun = 2 (`world_okun_ratio`)
and u in fractions. The fields are the reported rows `unemployment_rate` and `u_star` (percent) of a run file.

The formula is exact (to 1e-15 against the block's own y_US) for months 3-239, where the three lags are reported rows. The
pre-run lags of months 0-2 are not in the output, so they are replaced by u_0 here; the value used as the config default is the
block's own mean (test `world::tests::yus_offset_measure`, which reads the block's y_US over months 0-239). This script prints
both the mean over months 0-239 (with that replacement) and over months 3-239 (exact), so the two can be compared.

Usage:  python3 -B -I measure_yus_offset.py --files S/base/s*.json [--seeds 12]     (files written by  econsim-cli run 20000 240 SEED --json | tail -n 1 )
"""
import sys, json, glob, argparse, re
import numpy as np


def load(path):
    return json.loads(open(path).read().strip().splitlines()[-1])


def yus_series(path):
    d = load(path)
    assert len(d) == 240, 'expected 240 reported months, got %d' % len(d)
    u = np.array([m['unemployment_rate'] for m in d]) / 100.0
    us = np.array([m['u_star'] for m in d]) / 100.0
    y = np.zeros(len(d))
    for t in range(len(d)):
        lags = [u[max(0, t - k)] for k in (1, 2, 3)]     # rows before 0 are not in the output: u_0 is used in their place
        y[t] = 100.0 * 2.0 * (us[t] - np.mean(lags))
    return y


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('--files', nargs='*', required=True)
    ap.add_argument('--seeds', type=int, default=12, help='number of files (in the order given) used for the first mean')
    a = ap.parse_args()
    files = sorted(a.files, key=lambda p: int(re.search(r'(\d+)\.json', p).group(1)))
    ys = [yus_series(f) for f in files]
    m_all = np.array([y.mean() for y in ys])                 # months 0-239
    m_exact = np.array([y[3:].mean() for y in ys])           # months 3-239, exact
    n = len(ys)
    print('seeds %d: mean y_US months 0-239, first %d seeds: %.3f pp (standard error %.3f)' % (n, min(a.seeds, n), m_all[:a.seeds].mean(), m_all[:a.seeds].std(ddof=1) / np.sqrt(min(a.seeds, n))))
    print('mean over all %d seeds: %.3f pp, across-seed sd %.3f' % (n, m_all.mean(), m_all.std(ddof=1)))
    print('exact months 3-239, first %d seeds: %.3f pp; all seeds %.3f pp' % (min(a.seeds, n), m_exact[:a.seeds].mean(), m_exact.mean()))
    print('per-seed means (pp, months 0-239):', np.round(m_all, 3).tolist())
