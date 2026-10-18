#!/usr/bin/env python3
"""Check that a new CLI run is bit-identical to a base run on every key the base has.

Usage:
  json_strip_compare.py BASE NEW [--keys-from-base-only]

BASE and NEW are files whose last line is the JSON array printed by `econsim-cli run ... --json` (the table
printed before it is ignored). Keys present only in NEW (the observation fields added in metrics-out) are stripped
from every monthly record before comparison. Every remaining key must match exactly (same float bits, via Python's
exact float round-trip), record by record and in the same order. Exit status 0 when identical, 1 otherwise.
"""
import json
import sys


def last_json(path):
    with open(path) as fh:
        lines = [ln for ln in fh.read().splitlines() if ln.strip()]
    return json.loads(lines[-1])


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    base = last_json(argv[1])
    new = last_json(argv[2])
    if len(base) != len(new):
        print(f"record count differs: base {len(base)} new {len(new)}")
        return 1
    extra = set()
    for b, n in zip(base, new):
        extra |= set(n) - set(b)
    if extra:
        print(f"stripped {len(extra)} new keys")
    bad = 0
    for i, (b, n) in enumerate(zip(base, new)):
        n = {k: v for k, v in n.items() if k not in extra}
        if list(b.keys()) != list(n.keys()):
            print(f"record {i}: key order or key set differs")
            bad += 1
            continue
        for k in b:
            if b[k] != n[k]:
                print(f"record {i} key {k}: base {b[k]!r} new {n[k]!r}")
                bad += 1
                break
        if bad > 20:
            break
    if bad:
        print(f"NOT identical ({bad} records differ)")
        return 1
    print(f"identical: {len(base)} records, {len(base[0]) if base else 0} base keys each")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
