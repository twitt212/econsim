#!/usr/bin/env python3
"""solve_blocs.py [--out PATH] [--check] [--bloc B]

Offline solution of the 12 blocs (spec 2.2.3, schema 1.9): for each bloc, the stacked perfect-foresight solve with the P* table
(world_ref.solve_horizon with Kk = 480, T = 720) gives C (5 x 33), the period-0 responses IR[e][k] for k < 240 and the tail
sum over k = 240..479 for the four equations (IS, PC, rule, V; the solver key "S"), and the rule innovations of the pin
tables for L in {3, 6, 12, 18, 24} (pin_eps with T = 480).

Output: crates/econsim-core/src/world/data/bloc_solution.json. Layout of IR: ir.<eq>[k][o] is the period-0 response of
(y, pi, i, z, mrr) = outputs o to a unit impulse in the equation at period k (k = 0..239).

--check: exit 0 iff the calibration hash equals the committed one and the regenerated numbers agree within 1e-10 absolute plus
1e-10 relative (the stacked LU solves need not be bit-identical across BLAS builds).
"""
import os, sys, json, argparse
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import numpy as np
import paths
import world_ref as wr
from bloc_params import BLOCS12
from pin_tables import pin_eps
from build_world_params import calibration_hash, compare, K, K_TAIL, T_KERNEL, T_PIN

OUT_DEFAULT = os.path.join(paths.ROOT, 'crates', 'econsim-core', 'src', 'world', 'data', 'bloc_solution.json') if paths.LAYOUT == 'repo' else None
PIN_LENGTHS = (3, 6, 12, 18, 24)
EQ_KEYS = (('is', 'IS'), ('pc', 'PC'), ('rule', 'rule'), ('v', 'S'))


def solve_one(b, cfg):
    c = cfg[b]
    q = wr.bloc_q(b, c)
    g_z = c['g_z']
    full = wr.solve_horizon(q, g_z, Kk=K_TAIL, T=T_KERNEL)
    out = {'c': [[float(x) for x in row] for row in full['C']], 'ir': {}, 'tail': {}, 'pin': {}}
    for key, eq in EQ_KEYS:
        arr = full['IR'][eq]
        out['ir'][key] = [[float(x) for x in arr[k]] for k in range(K)]
        out['tail'][key] = [float(x) for x in arr[K:].sum(0)]
    for L in PIN_LENGTHS:
        e, _ = pin_eps(q, L, g_z, T=T_PIN)
        out['pin'][str(L)] = [float(x) for x in e]
    return out


def build(only=None):
    cfg = wr.load_pstar()
    blocs = {}
    for b in BLOCS12:
        if only and b != only:
            continue
        blocs[b] = solve_one(b, cfg)
        print('solved', b, file=sys.stderr)
    return {
        'meta': {'calibration_hash': calibration_hash(), 'k': K, 'k_tail': K_TAIL, 't_kernel': T_KERNEL, 't_pin': T_PIN,
                 'pstar_sha': wr.pstar_sha(), 'outputs': ['y', 'pi', 'i', 'z', 'mrr'],
                 'state': 'y lags 1..3, i lag 1, z lags 1..13, pi lags 1..13, mrr lags 1..3 (33)',
                 'ir_layout': 'ir[eq][k][o]: row k = period-0 response of outputs o to a unit impulse in eq at period k',
                 'eqs': {'is': 'IS', 'pc': 'PC', 'rule': 'rule', 'v': 'UIP (solver key S)'},
                 'pin_lengths': list(PIN_LENGTHS)},
        'blocs': blocs,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--out', default=OUT_DEFAULT)
    ap.add_argument('--check', action='store_true')
    ap.add_argument('--bloc', default=None)
    a = ap.parse_args()
    if a.out is None:
        raise SystemExit('no default output path in this layout; pass --out')
    doc = build(a.bloc)
    if a.check:
        committed = json.load(open(a.out))
        if committed['meta']['calibration_hash'] != doc['meta']['calibration_hash']:
            print('calibration_hash differs from the committed file'); sys.exit(1)
        ref = committed if a.bloc is None else {'meta': committed['meta'], 'blocs': {a.bloc: committed['blocs'][a.bloc]}}
        d = compare(json.loads(json.dumps(doc)), ref)
        print('solve_blocs --check OK: hash equal; max abs difference %.3e' % d)
        return
    os.makedirs(os.path.dirname(a.out), exist_ok=True)
    with open(a.out, 'w') as f:
        json.dump(doc, f)
        f.write('\n')
    print('wrote', a.out, os.path.getsize(a.out), 'bytes')


if __name__ == '__main__':
    main()
