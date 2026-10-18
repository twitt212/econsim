#!/usr/bin/env python3
"""Grid sweep of config parameters (8 seeds x 180 months, 5,000 households) scored on long-run stationarity:
unemployment and core-inflation levels and 15-year trends, policy-rate and consumption-share trends.
Usage: python3 scripts/sweep.py ['{"param":[v1,v2],...}']  (uses ECONSIM_CFG overrides; build --release first)."""
import json, subprocess, itertools, os, sys
from concurrent.futures import ThreadPoolExecutor
CLI=os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
grid = {
 'labor_share_pull':[0.02,0.01,0.005],
 'wage_norm_target_weight':[0.0,0.5],
 'markup_demand_pull':[0.3,0.1],
 'pop_growth_annual':[0.009,0.006],
}
if len(sys.argv)>1: grid=json.loads(sys.argv[1])
keys=list(grid)
combos=[dict(zip(keys,v)) for v in itertools.product(*[grid[k] for k in keys])]
SEEDS=range(1,9)
def run(job):
    ci,cfg,seed=job
    env=dict(os.environ, RAYON_NUM_THREADS='1', ECONSIM_CFG=json.dumps(cfg))
    out=subprocess.run([CLI,'run','5000','180',str(seed),'--json'],capture_output=True,text=True,env=env).stdout
    return ci,json.loads(out.strip().splitlines()[-1])
jobs=[(i,c,s) for i,c in enumerate(combos) for s in SEEDS]
res={}
with ThreadPoolExecutor(10) as ex:
    for ci,h in ex.map(run,jobs): res.setdefault(ci,[]).append(h)
def stats(H):
    def avg(k,a,b): return sum(sum(h[t][k] for t in range(a,b))/(b-a) for h in H)/len(H)
    d=dict(u=avg('unemployment_rate',0,180), core=avg('core_inflation_yoy',12,180), pol=avg('policy_rate',0,180),
           du=avg('unemployment_rate',144,180)-avg('unemployment_rate',0,36),
           dcore=avg('core_inflation_yoy',144,180)-avg('core_inflation_yoy',12,48),
           dpol=avg('policy_rate',144,180)-avg('policy_rate',0,36),
           debt=avg('gov_debt_gdp',168,180),
           dC=100*(avg('consumption_nominal_bn_ar',144,180)/avg('gdp_nominal_bn_ar',144,180)-avg('consumption_nominal_bn_ar',0,36)/avg('gdp_nominal_bn_ar',0,36)))
    d['score']=abs(d['u']-4.2)+abs(d['du'])+abs(d['core']-2.0)+abs(d['dcore'])+0.5*abs(d['dpol'])+0.3*abs(d['dC'])
    return d
rows=[(stats(res[i]),combos[i]) for i in range(len(combos))]
rows.sort(key=lambda r:r[0]['score'])
for d,c in rows:
    print(f"score {d['score']:5.2f} | u {d['u']:4.2f} du {d['du']:+5.2f} | core {d['core']:4.2f} dcore {d['dcore']:+5.2f} | pol {d['pol']:4.2f} dpol {d['dpol']:+5.2f} | dC {d['dC']:+5.2f} debt {d['debt']:5.1f} | {c}")
