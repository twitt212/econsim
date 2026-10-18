#!/usr/bin/env python3
"""make_golden.py [--check]

Stage 1 golden vectors (spec 1.9): regenerates them from the committed defaults (world_ref.goldens --write), or with --check
compares a fresh regeneration against the committed files in crates/econsim-core/src/world/data/golden/ (structure equal,
numbers within 1e-9 absolute, the replay files byte-identical). The trade_hand golden (stage 2) and the oil golden (stage 3)
are not produced by stage 1. Run as: python3 -B -I scripts/world/make_golden.py [--check]
"""
import os, sys, argparse
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import json, tempfile
import paths
import world_ref

TOL = 1e-9


def check():
    worst = 0.0
    for name, obj in world_ref.golden_docs().items():
        committed = json.load(open(paths.golden(name)))
        worst = max(worst, world_ref._flat(json.loads(json.dumps(obj, default=float)), committed))
    print('max abs difference of the regenerated JSON goldens: %.3e' % worst)
    ok = worst <= TOL
    for kind in ('clean', 'stress'):
        fd, tmp = tempfile.mkstemp(suffix='.jsonl'); os.close(fd)
        world_ref.gen_inputs(kind, tmp)
        same = open(tmp).read() == open(paths.golden('inputs_' + kind)).read()
        os.remove(tmp)
        print('replay file inputs_%s byte-identical: %s' % (kind, same))
        ok = ok and same
    return ok


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--check', action='store_true')
    a = ap.parse_args()
    if a.check:
        ok = check()
        print('make_golden --check', 'OK' if ok else 'FAILED')
        sys.exit(0 if ok else 1)
    world_ref.goldens(write=True)
    print('make_golden: goldens written')


if __name__ == '__main__':
    main()
