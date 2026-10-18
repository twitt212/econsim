#!/usr/bin/env python3
"""Finite-sample US firm-size benchmark: what a random sample of N firms looks like if the population matches US size.

Usage:  PYTHONDONTWRITEBYTECODE=1 python3 -I scripts/finite_size_benchmark.py [N ...]   (default N = 760)

Population: six approximate BDS / SUSB 2019 size classes (employees 1-4, 5-9, 10-19, 20-99, 100-499, 500+). The class
shares below are approximate (labelled as such). The 500+ class is a Pareto tail with exponent alpha500, calibrated so
that the population share of employment in firms with 500+ staff is 52.9%, and capped at 2.3 million employees.

For each sample size, the script draws random samples and reports the p10/p25/p50/p75/p90 of:
  emp500   employment share of firms with 500+ staff, %
  top1     employment share of the largest 1% of firms, %   (top034: largest 0.34%)
  zipf     OLS Zipf exponent of the rank-size counting function over firms with >= 10 employed heads
  max      largest firm, employed heads

Model rows in facts_firms.py: top1_share (band 30-52) and emp500_fs (band 13-43). The sampling is numpy only.
Sizes are employed heads (the model unit; persons = heads x 1.75 would scale only the largest-firm row).
"""
import sys
import numpy as np

SEED = 7
CLASSES = [  # (lo, hi, approximate share of firms); hi None = open-ended Pareto tail
    (1, 4, 0.615), (5, 9, 0.185), (10, 19, 0.110), (20, 99, 0.0855), (100, 499, 0.0161), (500, None, 0.0034),
]
EMP500_TARGET = 0.529
CAP = 2.3e6
XM = 500.0


def pareto_truncated_mean(a):
    """Mean of Pareto(XM, a) capped at CAP (cap mass goes to CAP)."""
    F = (XM / CAP) ** a
    return a * XM / (a - 1) * (1 - (XM / CAP) ** (a - 1)) + CAP * F


def loguniform_mean(lo, hi):
    return (hi + 1 - lo) / np.log((hi + 1) / lo)


def calibrate(w):
    """Bisect the Pareto exponent so the population 500+ employment share matches EMP500_TARGET."""
    base = sum(wi * loguniform_mean(lo, hi) for (lo, hi, _), wi in zip(CLASSES[:-1], w[:-1]))
    lo_a, hi_a = 1.01, 2.0
    a = 1.5
    for _ in range(60):
        a = 0.5 * (lo_a + hi_a)
        m = w[-1] * pareto_truncated_mean(a)
        share = m / (m + base)
        if share > EMP500_TARGET:
            lo_a = a
        else:
            hi_a = a
    return a


def draw(rng, n, w, a):
    k = rng.choice(len(CLASSES), size=n, p=w)
    s = np.empty(n)
    for i, (lo, hi, _) in enumerate(CLASSES):
        mm = k == i
        c = int(mm.sum())
        if hi is None:
            s[mm] = np.minimum(XM * (1 - rng.random(c)) ** (-1 / a), CAP)
        else:
            s[mm] = np.exp(rng.uniform(np.log(lo), np.log(hi + 1), c))
    return s


def zipf_exponent(s):
    x = np.sort(s[s >= 10])[::-1]
    if len(x) < 10:
        return np.nan
    return -np.polyfit(np.log(x), np.log(np.arange(1, len(x) + 1)), 1)[0]


def main(argv):
    sizes = [int(v) for v in argv] or [760]
    w = np.array([c[2] for c in CLASSES])
    w = w / w.sum()
    a = calibrate(w)
    base = sum(wi * loguniform_mean(lo, hi) for (lo, hi, _), wi in zip(CLASSES[:-1], w[:-1]))
    m = w[-1] * pareto_truncated_mean(a)
    print(f"Pareto exponent above 500 staff: {a:.4f}; population emp500 {100 * m / (m + base):.1f}%; "
          f"mean firm size {base + m:.1f} staff (approximate BDS/SUSB 2019 class shares)")
    rng = np.random.default_rng(SEED)
    for n in sizes:
        reps = 20000 if n < 5000 else 300
        rows = []
        for _ in range(reps):
            s = draw(rng, n, w, a)
            t = s.sum()
            ss = np.sort(s)[::-1]
            rows.append((
                100 * s[s >= 500].sum() / t,
                100 * ss[: max(1, round(n * 0.01))].sum() / t,
                100 * ss[: max(1, round(n * 0.0034))].sum() / t,
                zipf_exponent(s),
                ss[0],
            ))
        R = np.array(rows)
        print(f"N={n} ({reps} samples)")
        for j, name in enumerate(["emp500 %", "top1 %", "top034 %", "zipf", "max heads"]):
            q = np.nanpercentile(R[:, j], [10, 25, 50, 75, 90])
            print(f"   {name:12s} p10/p25/p50/p75/p90 = " + " / ".join(f"{v:.3f}" for v in q)
                  + f"   mean {np.nanmean(R[:, j]):.3f}")


if __name__ == "__main__":
    main(sys.argv[1:])
