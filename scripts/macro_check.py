#!/usr/bin/env python3
"""Macro baseline check: unemployment, inflation (headline core CPI, core PCE, shelter), rates, housing and price
growth by sector, averaged over 8 seeds (20,000 households, 240 months, first year discarded).
Usage: SEEDS=8 python3 scripts/macro_check.py [path/to/econsim-cli]   (default: target/release/econsim-cli, 8 seeds)."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor

SEEDS = int(os.environ.get('SEEDS', '8'))
ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
CLI = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, 'target', 'release', 'econsim-cli')
LABELS = ['Energy', 'Food', 'DurableGoods', 'NondurableGoods', 'Tech', 'LeisureHospitality', 'HealthEducation',
          'OtherServices', 'Construction']


def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', '20000', '240', str(seed), '--json'], capture_output=True, text=True,
                         env=env).stdout
    return json.loads(out.strip().splitlines()[-1])


with ThreadPoolExecutor(8) as ex:
    H = list(ex.map(run, range(1, SEEDS + 1)))
yrs = (len(H[0]) - 13) / 12
ns = len(H[0][-1]['sector_prices'])
labels = LABELS if ns == len(LABELS) else [f'sector{i}' for i in range(ns)]


def m(k, a=12, b=None):
    return sum(sum(x[k] for x in h[a:b]) / len(h[a:b]) for h in H) / len(H)


def se(vals):
    n = len(vals)
    mu = sum(vals) / n
    sd = (sum((v - mu) ** 2 for v in vals) / (n - 1)) ** 0.5 if n > 1 else 0.0
    return mu, sd / n ** 0.5


def r(k):
    return sum(sum(x[k] / x['gdp_nominal_bn_ar'] for x in h[12:]) / len(h[12:]) for h in H) / len(H) * 100


# annualised price growth by sector over the sample after the first year
g = [sum(100 * ((h[-1]['sector_prices'][s] / h[12]['sector_prices'][s]) ** (1 / yrs) - 1) for h in H) / len(H)
     for s in range(ns)]
y10 = sum(sum(x['yields'][3] for x in h[12:]) / len(h[12:]) for h in H) / len(H)
fx = sum(100 * ((h[-1]['dollar_index'] / h[12]['dollar_index']) ** (1 / yrs) - 1) for h in H) / len(H)
gdp = sum(100 * ((h[-1]['gdp_real_bn_ar'] / h[12]['gdp_real_bn_ar']) ** (1 / yrs) - 1) for h in H) / len(H)

print(f"u {m('unemployment_rate'):.2f} coreCPI {m('core_inflation_yoy'):.2f} corePCE {m('core_pce_inflation_yoy'):.2f} "
      f"shelter {m('shelter_inflation_yoy'):.2f} policy {m('policy_rate'):.2f} 10y {y10:.2f} "
      f"mtg {m('mortgage_rate'):.2f} starts {m('housing_starts_millions_ar'):.2f} hpa {m('hpi_growth_yoy'):.2f} "
      f"debt_end {sum(h[-1]['gov_debt_gdp'] for h in H) / len(H):.0f} int/Y {r('gov_interest_bn_ar'):.1f}")
pol = [sum(x['policy_rate'] for x in h[12:]) / len(h[12:]) for h in H]
pm, ps = se(pol)
print("  policy per seed: " + " ".join(f"{v:.2f}" for v in pol) + f" | mean {pm:.2f} +- SE {ps:.2f}")
ss0 = [sum(x['social_security_gdp'] for x in h[0:12]) / 12 for h in H]
ss10 = [sum(x['social_security_gdp'] for x in h[120:132]) / 12 for h in H]
ssav = [sum(x['social_security_gdp'] for x in h) / len(h) for h in H]
dfc = [sum(x['deficit_gdp'] for x in h[12:]) / len(h[12:]) for h in H]
prim = [sum(x['deficit_gdp'] - 100 * x['gov_interest_bn_ar'] / x['gdp_nominal_bn_ar'] for x in h[12:]) / len(h[12:]) for h in H]
mt = [sum(x['means_tested_gdp'] for x in h[12:]) / len(h[12:]) for h in H]
pe = [sum(x['pe_ratio'] for x in h[12:]) / len(h[12:]) for h in H]
rse = [sum(x['r_star_est'] for x in h[12:]) / len(h[12:]) for h in H]
d0 = [h[11]['gov_debt_gdp'] for h in H]
d10 = [h[131]['gov_debt_gdp'] for h in H]
d20 = [h[-1]['gov_debt_gdp'] for h in H]
print(f"  deficit {sum(dfc)/len(dfc):.2f} primary {sum(prim)/len(prim):.2f} SS yr0 {sum(ss0)/len(ss0):.2f} "
      f"SS yr10 {sum(ss10)/len(ss10):.2f} SS avg {sum(ssav)/len(ssav):.2f} means-tested {sum(mt)/len(mt):.2f} "
      f"debt yr0/10/20 {sum(d0)/len(d0):.0f}/{sum(d10)/len(d10):.0f}/{sum(d20)/len(d20):.0f} "
      f"P/E {sum(pe)/len(pe):.1f} r*est {sum(rse)/len(rse):.2f}")
print(f"  GDP real growth {gdp:.2f}/yr | X/Y {r('exports_bn_ar'):.1f} M/Y {r('imports_bn_ar'):.1f} dollar %/yr {fx:+.2f}")
print("  price growth by sector (%/yr): " + ", ".join(f"{labels[s]} {g[s]:.2f}" for s in range(ns)))
