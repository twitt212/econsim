#!/usr/bin/env python3
"""Statistical-equivalence check for optimisations that change floating-point order.

  stat_check.py record  ref.json     # run reference (8 seeds x 120 months, 5000 households; 4-seed means are too noisy for the policy rate) and save means
  stat_check.py compare ref.json     # run current build and compare with tolerances

Means are over seeds and months 12-120 of key metrics. Exit code 1 if any metric is outside tolerance."""
import json, subprocess, sys
import os
SEEDS = [int(x) for x in os.environ.get("STAT_SEEDS", "1,2,3,4,5,6,7,8").split(",")]
KEYS = {  # metric: (tolerance, kind) — 'abs' in metric units, 'rel' relative
    'unemployment_rate': (0.6, 'abs'), 'inflation_yoy': (0.4, 'abs'), 'core_inflation_yoy': (0.4, 'abs'),
    'policy_rate': (0.5, 'abs'), 'gdp_real_bn_ar': (0.02, 'rel'), 'consumption_real_bn_ar': (0.02, 'rel'),
    'investment_real_bn_ar': (0.04, 'rel'), 'median_house_price': (0.05, 'rel'), 'homeownership_rate': (1.5, 'abs'),
    'gov_debt_gdp': (4.0, 'abs'), 'deficit_gdp': (1.0, 'abs'), 'wealth_gini': (0.02, 'abs'),
    'house_sales_millions_ar': (0.15, 'rel'), 'avg_wage_annual': (0.02, 'rel'),
}
def means():
    acc = {k: 0.0 for k in KEYS}
    for s in SEEDS:
        out = subprocess.run(['./target/release/econsim-cli', 'run', '5000', '120', str(s), '0', '--json'],
                             capture_output=True, text=True, check=True).stdout
        h = json.loads(out.strip().splitlines()[-1])[12:]
        for k in KEYS:
            acc[k] += sum(m[k] for m in h) / len(h) / len(SEEDS)
    return acc
if __name__ == '__main__':
    mode, path = sys.argv[1], sys.argv[2]
    cur = means()
    if mode == 'record':
        json.dump(cur, open(path, 'w'), indent=1); print(json.dumps(cur, indent=1)); sys.exit(0)
    ref = json.load(open(path)); bad = 0
    for k, (tol, kind) in KEYS.items():
        d = cur[k] - ref[k] if kind == 'abs' else (cur[k] / ref[k] - 1 if ref[k] else 0)
        ok = abs(d) <= tol; bad += not ok
        print(f"{'OK ' if ok else 'BAD'} {k:28s} ref {ref[k]:12.4f} now {cur[k]:12.4f} diff {d:+.4f} (tol {tol} {kind})")
    sys.exit(1 if bad else 0)
