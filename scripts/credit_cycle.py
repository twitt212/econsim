#!/usr/bin/env python3
"""Cyclicality of credit variables (quarterly HP cycles vs real GDP, 8 seeds x 20 years, 20,000 households).
Usage: python3 scripts/credit_cycle.py [path to econsim-cli]   (honours ECONSIM_CFG; build --release first).
US benchmarks: credit spread (GZ) corr with GDP cycle ~ -0.5 and leading; business exits and loan losses countercyclical."""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np
CLI=sys.argv[1] if len(sys.argv)>1 else os.path.join(os.path.dirname(os.path.abspath(__file__)),'..','target','release','econsim-cli')
def run(seed):
    env=dict(os.environ, RAYON_NUM_THREADS='1')
    out=subprocess.run([CLI,'run','20000','240',str(seed),'--json'],capture_output=True,text=True,env=env).stdout
    return json.loads(out.strip().splitlines()[-1])
def hp(y, lam=1600.0):
    n=len(y); I=np.eye(n); D=np.diff(I,2,axis=0); return y-np.linalg.solve(I+lam*D.T@D,y)
def q(h,f): v=np.array([f(m) for m in h]); return v[:len(v)//3*3].reshape(-1,3).mean(1)
with ThreadPoolExecutor(8) as ex: R=list(ex.map(run,range(1,9)))
series={'spread':lambda m:m.get('credit_spread',m['business_loan_rate']-m['policy_rate']),'bankrupt':lambda m:m['bankruptcies_ar'],'losses/Y':lambda m:100*m['loan_losses_bn_ar']/m['gdp_nominal_bn_ar'],
 'bank cap':lambda m:m['bank_capital_ratio'],'biz debt/Y':lambda m:100*m['business_debt_bn']/m['gdp_nominal_bn_ar'],'cap util':lambda m:m['capacity_utilization'],'profit sh':lambda m:m['profit_share']}
for k,f in series.items():
    lv=np.mean([np.mean([f(m) for m in h]) for h in R])
    cs=[];sd=[]
    for h in R:
        y=hp(np.log(q(h,lambda m:m['gdp_real_bn_ar']))); x=q(h,f); c=hp(x)
        cs.append([np.corrcoef(y[:len(y)-kk] if kk>=0 else y[-kk:], c[kk:] if kk>=0 else c[:len(c)+kk])[0,1] for kk in (-4,-2,0,2,4)]); sd.append(c.std())
    print(f"{k:11s} mean {lv:9.3f} cyc sd {np.mean(sd):7.3f}  corr(GDP_t, x_t+k) k=-4,-2,0,2,4:",' '.join(f"{v:+.2f}" for v in np.mean(cs,0)))
