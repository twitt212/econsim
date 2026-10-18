#!/usr/bin/env python3
"""Print key metrics from `econsim-cli run N M seed --json` output. Usage: diag.py file.json [keys...]"""
import json, sys
h = json.loads(open(sys.argv[1]).read().strip().splitlines()[-1])
keys = sys.argv[2:] or ['gdp_real_growth_yoy','unemployment_rate','inflation_yoy','core_inflation_yoy','policy_rate',
  'consumption_real_bn_ar','investment_real_bn_ar','government_bn_ar','imports_bn_ar','profit_share','deficit_gdp',
  'gov_debt_gdp','fund_share_price','median_house_price','house_sales_millions_ar','housing_starts_millions_ar',
  'homeownership_rate','avg_wage_annual','wage_growth_yoy','firms_alive','bankruptcies_ar','capacity_utilization','avg_markup']
steps = sorted(set([0,1,2,3,6,9,12,18,24,36,48,60,84,119,len(h)-1]))
print('key'.ljust(28) + ''.join(f'{t:>9}' for t in steps if t < len(h)))
for k in keys:
    row = k[:27].ljust(28)
    for t in steps:
        if t < len(h):
            v = h[t][k]
            row += f'{v:>9.3g}' if isinstance(v,(int,float)) else f'{str(v)[:8]:>9}'
    print(row)
