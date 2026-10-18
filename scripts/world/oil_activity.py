#!/usr/bin/env python3
"""Isolated-market (prototype activity) against integrated-market (bloc activity proxy) price moments; levers rho_q and sig_I (D-R3).
Medians over N samples of 468 months of the prototype step function (the research prototype, published as oil_reference.py).
 'isolated'    : prototype activity AR(1), sigma_y 1.6657, rho_y 0.9943 (level sd 15.6%)   -> W3-05 / W3-06 (oil-check)
 'integrated'  : activity gap of the blocs is the activity: proxied here by an AR(1) with sd 2.0% (rho 0.9943) -> W3-15 expectation
 'none'        : no activity at all.
Usage: python3 -B oil_activity.py [N] [procs]
"""
import sys, os, json, dataclasses
import numpy as np
from multiprocessing import Pool
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE); sys.dont_write_bytecode = True
import paths, importlib.util
_spec = importlib.util.spec_from_file_location('oil_market_model', paths.OIL_PROTOTYPE)      # research prototype, or scripts/world/oil_reference.py in the repo layout
_m = importlib.util.module_from_spec(_spec); sys.modules['oil_market_model'] = _m; _spec.loader.exec_module(_m)
Params, simulate, moments = _m.Params, _m.simulate, _m.moments

CASES = {
    'isolated (prototype activity)':                 dict(sig_y=1.6657),
    'integrated proxy, activity sd 2%':               dict(sig_y=2.0 * np.sqrt(1 - 0.9943 ** 2)),
    'integrated proxy, no activity':                  dict(sig_y=0.0),
    'integrated, rho_q 0.995':                        dict(sig_y=2.0 * np.sqrt(1 - 0.9943 ** 2), rho_q=0.995),
    'integrated, sig_I x1.1':                         dict(sig_y=2.0 * np.sqrt(1 - 0.9943 ** 2), sig_I_mult=1.1),
    'integrated, rho_q 0.995 and sig_I x1.1':         dict(sig_y=2.0 * np.sqrt(1 - 0.9943 ** 2), rho_q=0.995, sig_I_mult=1.1),
}

def one(args):
    kw, seed = args
    kw = dict(kw); mult = kw.pop('sig_I_mult', 1.0)
    P = dataclasses.replace(Params(), **kw); P = dataclasses.replace(P, sig_I=P.sig_I * mult)
    o = simulate(P, 468, np.random.default_rng(seed))
    m = moments(o['pbar']); sd_share = None
    return m

def run_case(kw, n, procs, seed0=1):
    with Pool(procs) as p: res = p.map(one, [(kw, seed0 + i) for i in range(n)])
    return {k: round(float(np.median([r[k] for r in res])), 3) for k in res[0]}

if __name__ == '__main__':
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200; procs = int(sys.argv[2]) if len(sys.argv) > 2 else 3
    out = {}
    for name, kw in CASES.items():
        out[name] = run_case(kw, n, procs); print(name, out[name], flush=True)
    json.dump(out, open(paths.golden('oil_activity'), 'w'), indent=1)
