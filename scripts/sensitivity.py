#!/usr/bin/env python3
"""One-at-a-time sensitivity of long-run trends to config parameters (8 seeds x 180 months, 5,000 households)."""
import json, subprocess, os
from concurrent.futures import ThreadPoolExecutor
CLI=os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
variants=[{}]
for k,vals in {'tfp_growth_annual':[0.0075,0.025],'pop_growth_annual':[0.012],'payout_ratio':[0.6],'investment_sensitivity':[0.0,1.0],
               'mpc_income':[0.99,0.94],'mpc_wealth_above':[0.0025,0.01],'mpc_fund_wealth_annual':[0.01,0.05],'mpc_housing_wealth_annual':[0.01,0.06],
               'deposit_beta':[0.25,0.75],'r_star':[0.0025,0.015],'phi_pi':[1.0],'phi_u':[1.0],'gov_purchases_share':[0.22]}.items():
    for v in vals: variants.append({k:v})
SEEDS=range(1,9)
def run(job):
    ci,cfg,seed=job
    env=dict(os.environ, RAYON_NUM_THREADS='1', ECONSIM_CFG=json.dumps(cfg))
    out=subprocess.run([CLI,'run','5000','180',str(seed),'--json'],capture_output=True,text=True,env=env).stdout
    return ci,json.loads(out.strip().splitlines()[-1])
jobs=[(i,c,s) for i,c in enumerate(variants) for s in SEEDS]
res={}
with ThreadPoolExecutor(10) as ex:
    for ci,h in ex.map(run,jobs): res.setdefault(ci,[]).append(h)
def stats(H):
    def avg(k,a,b): return sum(sum(h[t][k] for t in range(a,b))/(b-a) for h in H)/len(H)
    return dict(u=avg('unemployment_rate',0,180), core=avg('core_inflation_yoy',12,180), pol=avg('policy_rate',0,180),
           du=avg('unemployment_rate',144,180)-avg('unemployment_rate',0,36),
           dcore=avg('core_inflation_yoy',144,180)-avg('core_inflation_yoy',12,48),
           dpol=avg('policy_rate',144,180)-avg('policy_rate',0,36),
           dprof=avg('profit_share',144,180)-avg('profit_share',0,36),
           dC=100*(avg('consumption_nominal_bn_ar',144,180)/avg('gdp_nominal_bn_ar',144,180)-avg('consumption_nominal_bn_ar',0,36)/avg('gdp_nominal_bn_ar',0,36)))
for i,c in enumerate(variants):
    d=stats(res[i])
    print(f"u {d['u']:4.2f} du {d['du']:+5.2f} | core {d['core']:4.2f} dcore {d['dcore']:+5.2f} | pol {d['pol']:4.2f} dpol {d['dpol']:+5.2f} | dprof {d['dprof']:+5.2f} dC {d['dC']:+5.2f} | {c if c else 'DEFAULT'}")
