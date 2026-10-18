import json, os, glob, math, sys
SCR = os.environ.get("SCR", os.path.dirname(os.path.abspath(__file__)))
BL = ["EA","CN","JP","UK","CA","MX","IN","KR","BR","AU","OPEC+","RoW"]
def load(cell, scen, seeds=None):
    out = {}
    for f in sorted(glob.glob(f"{SCR}/runs/{cell}_{scen}/s*.json")):
        s = int(os.path.basename(f)[1:-5])
        if seeds and s not in seeds: continue
        out[s] = json.load(open(f))
    return out
def mean(x): return sum(x)/len(x) if x else float('nan')
def sd(x):
    if len(x) < 2: return float('nan')
    m = mean(x); return math.sqrt(sum((v-m)**2 for v in x)/(len(x)-1))
def se(x): return sd(x)/math.sqrt(len(x)) if len(x) > 1 else float('nan')
def get(rec, arm, key, m, j=None):
    v = rec[arm][key][m]
    return v[j] if j is not None else v
def seedvals(runs, arm, key, m, j=None): return [get(r, arm, key, m, j) for r in runs.values()]
def paired(runs, key, m, j=None, mode="diff"):
    """per-seed (shocked - baseline) at month m; mode diff or pct (100*(s/b-1)); returns (mean, se, n)"""
    d = []
    for r in runs.values():
        b = get(r, "b", key, m, j); s = get(r, "s", key, m, j)
        if mode == "pct":
            if b == 0: continue
            d.append(100.0*(s/b-1.0))
        else: d.append(s-b)
    return mean(d), se(d), len(d)
def paired_avg(runs, key, m0, m1, j=None, mode="diff"):
    """mean over months m0..m1 inclusive of the per-seed paired effect"""
    d = []
    for r in runs.values():
        vals = []
        for m in range(m0, m1+1):
            b = get(r, "b", key, m, j); s = get(r, "s", key, m, j)
            vals.append(100.0*(s/b-1.0) if mode == "pct" else s-b)
        d.append(mean(vals))
    return mean(d), se(d), len(d)
