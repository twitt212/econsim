#!/bin/bash
# Stage 5 fingerprint tests (brief 5 and 7): run 5000 60 3 md5 of the block-off default (I1), the stage-5 profiles at RAYON 1 and 8
# (determinism, profiles_threads_equal), the three profiles without their import-gap key (the stage-4 values), and legacy_b8off (I9).
# usage: stage5_fingerprints.sh [CLI]   (default: target/release/econsim-cli of this checkout). Exit 0 when every value matches.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-$ROOT/target/release/econsim-cli}"
P="$ROOT/scripts/profiles"
TMP="$(mktemp -d)"
md5v() { if command -v md5sum >/dev/null 2>&1; then md5sum | cut -d' ' -f1; else md5 -q; fi; }
fail=0
check() { # name want file-or-none
  local name=$1 want=$2 cfg=$3 r got
  for r in 1 8; do
    if [ "$cfg" = none ]; then
      got=$(RAYON_NUM_THREADS=$r "$BIN" run 5000 60 3 --json 2>/dev/null | tail -n 1 | md5v)
    else
      got=$(ECONSIM_CFG="$(cat "$cfg")" RAYON_NUM_THREADS=$r "$BIN" run 5000 60 3 --json 2>/dev/null | tail -n 1 | md5v)
    fi
    if [ "$got" = "$want" ]; then echo "PASS $name rayon=$r $got"; else echo "FAIL $name rayon=$r got $got want $want"; fail=1; fi
  done
}
# Strip the gap key from a profile (the stage-4 reference of the with-constants profiles).
nokey() { python3 -B -I -c "import json,sys; d=json.load(open(sys.argv[1])); d.pop('world_import_gap_offset', None); json.dump(d, open(sys.argv[2], 'w'))" "$1" "$2"; }
check block-off 9f90c88a2c8b653857029d235cd6b0a6 none
check legacy_b8off-I9 9f90c88a2c8b653857029d235cd6b0a6 "$P/legacy_b8off.json"
check s5a 51001607e6a3cac587c84c367adf8158 "$P/b8-world-s5a.json"
check s5b 78287b98bfb85a02faa20f36805c5c2e "$P/b8-world-s5b.json"
check s5c d67ad754615146788900b02fbbad42b7 "$P/b8-world-s5c.json"
check s5a-lean e20bc1a491c678af60cd0c87ec15fd8c "$P/b8-world-s5a-lean.json"
nokey "$P/b8-world-s5a.json" "$TMP/s5a.json"; check s5a-nokey 8d26861f4ce69352e7316ae244955631 "$TMP/s5a.json"
nokey "$P/b8-world-s5b.json" "$TMP/s5b.json"; check s5b-nokey cb8025afc3fd9f88bad922b332f3a797 "$TMP/s5b.json"
nokey "$P/b8-world-s5c.json" "$TMP/s5c.json"; check s5c-nokey 294c25af44689d58c0ef3ae24cbebb97 "$TMP/s5c.json"
# 5-T2 (D62, D63): the repaired profiles (sketch fingerprints of the record), RAYON 1 and 8 each; the stage-5 checks above stay (I10).
check t2a 19bea986120687d021d98723a1c97725 "$P/b8-world-t2a.json"
check t2b cccdf5ada5c32c6db48dd83b04afaad1 "$P/b8-world-t2b.json"
check t2c a262a0a2108dd2ae444d0324520d3573 "$P/b8-world-t2c.json"
check t2a-nc fedb5a661b7b88ae8c8ad9213b2a7159 "$P/b8-world-t2a-nc.json"
check t2c-nd 4eb2d4f830637c0bf48fb8112b24039f "$P/b8-world-t2c-nd.json"
# t2a without the drift key and with the stage-5 constants is the stage-5 profile s5a (51001607...).
python3 -B -I -c "import json,sys; d=json.load(open(sys.argv[1])); d.pop('world_import_rel_drift', None); d['world_import_gap_offset']=[1.983,1.858,1.802,2.933,1.163,1.885,3.007,2.102,2.125,1.822,1.814,2.577]; json.dump(d, open(sys.argv[2], 'w'))" "$P/b8-world-t2a.json" "$TMP/t2a-s5const.json"
check t2a-nodrift-s5const 51001607e6a3cac587c84c367adf8158 "$TMP/t2a-s5const.json"
rm -rf "$TMP"
if [ $fail = 0 ]; then echo "STAGE5_FINGERPRINTS PASS"; else echo "STAGE5_FINGERPRINTS FAIL"; fi
exit $fail
