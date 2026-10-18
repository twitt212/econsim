#!/usr/bin/env python3
"""b9-warmup drift test: start-up drift of the reported run, with and without a warm-up.

usage: warmup_drift.py RUNDIR CELL [--seeds 1-300] [--window 60] [--shift 24]
       warmup_drift.py --selftest

RUNDIR/<CELL>_s<seed>.json.gz is a plain CLI run (`econsim-cli run N MONTHS SEED --json`, gzipped); its last line is a
JSON array of monthly records, at least shift + window long. A run without warm-up shows months 0..window-1 in window A;
its months shift..shift+window-1 are exactly what a warm-up of `shift` months reports (the warm-up steps the same world).

Per seed and variable (percent, pp):
  drift0     = mean(x[0:6])                 - mean(x[window-6:window])          (no warm-up)
  drift_shift = mean(x[shift:shift+6])      - mean(x[shift+window-6:shift+window])  (warm-up = shift)
  red        = drift0 - drift_shift

Prints, per variable, the mean over seeds with SE = sample sd / sqrt(n) of drift0, drift_shift and red, and the gate
rows of the spec (section 3, Step 2):
  W2a  u and policy: |drift_shift| <= 1.5 x SE (not distinguishable from zero)
  W2b  u, policy and wage growth: red > 2 x SE of red
  core PCE, hires rate and vacancy rate are reported only.
Exit status 1 when a W2a or W2b row fails, 0 otherwise.
Standard library plus numpy only.
"""
import gzip
import json
import math
import os
import sys

import numpy as np

VARS = [
    # (label, record key, gate membership)
    ("u", "unemployment_rate", {"W2a", "W2b"}),
    ("policy", "policy_rate", {"W2a", "W2b"}),
    ("core_pce", "core_pce_inflation_yoy", set()),
    ("wage", "wage_growth_yoy", {"W2b"}),
    ("hires", "hires_rate", set()),
    ("vac", "vacancy_rate", set()),
]
SEED_RANGE_HELP = "a-b (inclusive)"


def mean_of(xs):
    xs = [x for x in xs if x is not None and math.isfinite(x)]
    return float(np.mean(xs)) if len(xs) == 6 else float("nan")


def seed_drifts(series, window, shift):
    """series: list of floats (percent) with at least shift + window items. Returns (drift0, drift_shift, red)."""
    d0 = mean_of(series[0:6]) - mean_of(series[window - 6:window])
    ds = mean_of(series[shift:shift + 6]) - mean_of(series[shift + window - 6:shift + window])
    return d0, ds, d0 - ds


def read_records(path):
    with gzip.open(path, "rt") as f:
        lines = [ln for ln in f.read().splitlines() if ln.strip()]
    if not lines:
        raise SystemExit(f"{path}: empty")
    return json.loads(lines[-1])


def parse_range(text):
    a, b = text.split("-")
    return list(range(int(a), int(b) + 1))


def se(xs):
    xs = np.asarray([x for x in xs if math.isfinite(x)], dtype=float)
    n = len(xs)
    if n < 2:
        return float("nan"), n
    return float(np.std(xs, ddof=1) / math.sqrt(n)), n


def analyse(rundir, cell, seeds, window, shift):
    need = shift + window
    per = {lab: {"d0": [], "ds": [], "red": []} for lab, _, _ in VARS}
    missing = []
    for s in seeds:
        path = os.path.join(rundir, f"{cell}_s{s}.json.gz")
        if not os.path.exists(path):
            missing.append(s)
            continue
        recs = read_records(path)
        if len(recs) < need:
            raise SystemExit(f"{path}: {len(recs)} records, need at least {need}")
        for lab, key, _ in VARS:
            series = [None if r.get(key) is None else float(r[key]) for r in recs[:need]]
            d0, ds, red = seed_drifts(series, window, shift)
            per[lab]["d0"].append(d0)
            per[lab]["ds"].append(ds)
            per[lab]["red"].append(red)
    if missing:
        raise SystemExit(f"{len(missing)} seed files missing in {rundir} for {cell}, e.g. seed {missing[0]}")
    return per


def report(per, cell, n_seeds, window, shift):
    lines = [f"cell {cell}, seeds n={n_seeds}, window {window}, shift {shift} (values in pp; mean over seeds, SE = sd/sqrt(n))"]
    lines.append(f"{'var':<9}{'drift0':>12}{'SE':>8}{'drift_shift':>14}{'SE':>8}{'reduction':>12}{'SE':>8}{'n':>5}")
    stats = {}
    for lab, _, _ in VARS:
        d0, n0 = se(per[lab]["d0"])
        ds, n1 = se(per[lab]["ds"])
        red, n2 = se(per[lab]["red"])
        m0, ms, mr = (float(np.mean(per[lab][k])) for k in ("d0", "ds", "red"))
        stats[lab] = {"m0": m0, "se0": d0, "ms": ms, "ses": ds, "mr": mr, "ser": red}
        lines.append(f"{lab:<9}{m0:>+12.3f}{d0:>8.3f}{ms:>+14.3f}{ds:>8.3f}{mr:>+12.3f}{red:>8.3f}{min(n0, n1, n2):>5}")
    fail = False
    for lab, _, gates in VARS:
        st = stats[lab]
        if "W2a" in gates:
            ok = abs(st["ms"]) <= 1.5 * st["ses"]
            fail |= not ok
            lines.append(f"W2a {lab}: |drift_shift| {abs(st['ms']):.3f} <= 1.5 x SE {1.5 * st['ses']:.3f}: {'PASS' if ok else 'FAIL'}")
        if "W2b" in gates:
            ok = st["mr"] > 2.0 * st["ser"]
            fail |= not ok
            lines.append(f"W2b {lab}: reduction {st['mr']:+.3f} > 2 x SE {2.0 * st['ser']:.3f}: {'PASS' if ok else 'FAIL'}")
    lines.append("W2c core_pce, hires, vac: reported only")
    return "\n".join(lines), fail


def selftest():
    # u 5 for months 0-5, 4 afterwards: drift0 = +1; the shifted window (months 24-29 vs 78-83) is flat at 4: drift_shift = 0.
    series = [5.0] * 6 + [4.0] * 78
    d0, ds, red = seed_drifts(series, 60, 24)
    assert abs(d0 - 1.0) < 1e-12, d0
    assert abs(ds - 0.0) < 1e-12, ds
    assert abs(red - 1.0) < 1e-12, red
    # a series with a missing value in the window is not a number for that seed
    bad = [5.0, 5.0, None, 5.0, 5.0, 5.0] + [4.0] * 78
    d0b, _, _ = seed_drifts(bad, 60, 24)
    assert math.isnan(d0b), d0b
    print("selftest ok: drift0 +1, drift_shift 0; missing value gives NaN")


def main(argv):
    if argv[1:] == ["--selftest"]:
        selftest()
        return 0
    if len(argv) < 3:
        raise SystemExit(__doc__)
    rundir, cell = argv[1], argv[2]
    opts = {"--seeds": "1-300", "--window": "60", "--shift": "24"}
    rest = argv[3:]
    while rest:
        k = rest.pop(0)
        if k not in opts or not rest:
            raise SystemExit(f"unknown or incomplete option {k}; seeds take {SEED_RANGE_HELP}")
        opts[k] = rest.pop(0)
    seeds = parse_range(opts["--seeds"])
    window, shift = int(opts["--window"]), int(opts["--shift"])
    if window < 12 or shift < 0:
        raise SystemExit("window must be >= 12 and shift >= 0")
    per = analyse(rundir, cell, seeds, window, shift)
    text, fail = report(per, cell, len(seeds), window, shift)
    print(text)
    return 1 if fail else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
