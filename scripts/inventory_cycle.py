#!/usr/bin/env python3
"""Inventory facts (Blinder & Maccini 1991): cyclicality of the change in private inventories and production vs final
sales volatility (quarterly HP cycles, 8 seeds x 20 years, 20,000 households).
Usage: python3 scripts/inventory_cycle.py [path to econsim-cli]   (honours ECONSIM_CFG; build --release first)."""
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
def qa(v): v=np.array(v); return v[:len(v)//3*3].reshape(-1,3).mean(1)
with ThreadPoolExecutor(8) as ex: R=list(ex.map(run,range(1,9)))
res=[]
for h in R:
    gdp=qa([m['gdp_real_bn_ar'] for m in h]); dinv=qa([m['inventory_change_real_bn_ar'] for m in h])
    sales=gdp-dinv
    y=hp(np.log(gdp)); s_=hp(np.log(sales)); di=hp(dinv/gdp.mean()*100)
    c=hp(np.log(qa([m['consumption_real_bn_ar'] for m in h])))
    res.append([np.corrcoef(di,y)[0,1], np.std(y)/np.std(s_), np.corrcoef(c,s_)[0,1], np.corrcoef(s_,y)[0,1], np.std(di)])
r=np.mean(res,0)
print(f"corr(dInv/Y, GDP) {r[0]:+.2f} (US ≈+0.6) | sd(GDP)/sd(final sales) {r[1]:.2f} (US >1, ≈1.2) | corr(C, final sales) {r[2]:+.2f} | corr(final sales, GDP) {r[3]:+.2f} | sd dInv %Y {r[4]:.2f}")
