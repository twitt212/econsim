#!/usr/bin/env python3
"""Housing-market baseline paths vs US targets (8 seeds, default 20,000 households, 20 years).
Usage: python3 scripts/housing_check.py [seeds=8] [months=240]   (honours ECONSIM_CFG and HOUSING_HH; build --release first)."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
CLI = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = int(sys.argv[1]) if len(sys.argv) > 1 else 8
MONTHS = int(sys.argv[2]) if len(sys.argv) > 2 else 240
HH = os.environ.get('HOUSING_HH', '20000')

def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([CLI, 'run', HH, str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])

with ThreadPoolExecutor(10) as ex:
    H = list(ex.map(run, range(1, SEEDS + 1)))
COLS = [('own%HVS', 'homeownership_rate_hvs', 65.6), ('own%hh', 'homeownership_rate', None),
        ('rvac%HVS', 'rental_vacancy_rate_hvs', 6.8), ('totvac%', 'vacancy_rate_housing', None),
        ('unhoused%', 'unhoused_share', None), ('dlq90%', 'mortgage_delinquent_90', 0.8),
        ('evict k', 'evictions_thousands_ar', 3600), ('fcl k', 'foreclosures_thousands_ar', 200),
        ('shelter%', 'shelter_inflation_yoy', 3.0), ('core%', 'core_inflation_yoy', 2.0), ('hpa%', 'hpi_growth_yoy', 4.0),
        ('p/inc', 'price_to_income', 5.0), ('mo.sup', 'months_supply', 4.5), ('salesM', 'house_sales_millions_ar', 4.7),
        ('startsM', 'housing_starts_millions_ar', 1.35), ('mtg%', 'mortgage_rate', None), ('u%', 'unemployment_rate', 4.3)]
# Relabelling (Round 7): own%HVS and rvac%HVS use the HVS definitions (owner-occupied / occupied units; vacant for rent /
# (renter-occupied + vacant for rent)). own%hh is the household-basis rate and totvac% the stock vacancy rate. The old
# rentvac% column was total stock vacancy; its 6.8 target now sits on rvac%HVS. evict k and fcl k are annualised
# thousands; the US targets are about 3.6M eviction filings a year and 100-300k foreclosure starts a year.
def avg(k, a, b): return sum(sum(x[k] for x in h[a:b]) / (b - a) for h in H) / len(H)
print('yr  ' + ' '.join(f'{c[0]:>8}' for c in COLS))
for y in range(0, MONTHS // 12, 2):
    print(f'{y:2d}  ' + ' '.join(f'{avg(k, y * 12, y * 12 + 12):8.2f}' for _, k, _ in COLS))
print('avg ' + ' '.join(f'{avg(k, 12, MONTHS):8.2f}' for _, k, _ in COLS))
print('US  ' + ' '.join(f'{t:8.2f}' if t is not None else f'{"":>8}' for *_, t in COLS))
