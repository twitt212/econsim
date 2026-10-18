#!/bin/bash
# Server identity proof: a typical-start compare is byte-identical between the base (master) server and the new server.
# usage: us_now_identity.sh BASE_SERVER_BIN NEW_SERVER_BIN OUTDIR     (BASE_PORT, default 8812, and NEW_PORT, default 8813, must be free; two server processes at most)
BASE="$1"; NEW="$2"; OUT="$3"; mkdir -p "$OUT"
BASE_PORT="${BASE_PORT:-8812}"; NEW_PORT="${NEW_PORT:-8813}"
cleanup() { [ -n "$PB" ] && kill "$PB" 2>/dev/null; [ -n "$PN" ] && kill "$PN" 2>/dev/null; }
trap cleanup EXIT
ECONSIM_CACHE_MB=0 RAYON_NUM_THREADS=2 PORT=$BASE_PORT "$BASE" > "$OUT/base_server.log" 2>&1 & PB=$!
ECONSIM_CACHE_MB=0 RAYON_NUM_THREADS=2 PORT=$NEW_PORT "$NEW"  > "$OUT/new_server.log" 2>&1 & PN=$!
sleep 2
i=0
for body in \
 '{"config":{"n_households":1000},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":6,"seeds":[1,2]}' \
 '{"config":{"n_households":1000,"warmup_months":0},"shocks":[],"months":12,"seeds":[3]}' \
 '{"config":{"n_households":1000,"warmup_months":12},"shocks":[{"at_month":0,"shock":{"type":"PolicyShift","bps":100,"months":12}}],"months":12,"seeds":[1]}'
do
  i=$((i+1))
  curl -s -X POST -H 'Content-Type: application/json' -d "$body" http://127.0.0.1:$BASE_PORT/api/compare > "$OUT/base_$i.json"
  curl -s -X POST -H 'Content-Type: application/json' -d "$body" http://127.0.0.1:$NEW_PORT/api/compare > "$OUT/new_$i.json"
  if cmp -s "$OUT/base_$i.json" "$OUT/new_$i.json"; then echo "IDENTICAL compare body $i ($(wc -c < "$OUT/base_$i.json") bytes)"; else echo "DIFFERENT compare body $i"; fi
done
curl -s http://127.0.0.1:$BASE_PORT/api/config/default > "$OUT/base_config.json"
curl -s http://127.0.0.1:$NEW_PORT/api/config/default > "$OUT/new_config.json"
python3 -I - "$OUT/base_config.json" "$OUT/new_config.json" <<'PY'
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
print("config/default keys only in new:", sorted(set(b) - set(a)))
print("config/default keys only in base:", sorted(set(a) - set(b)))
print("config/default shared keys with a different value:", [k for k in a if k in b and a[k] != b[k]])
print("start_state:", b.get("start_state"), "| start_states:", [(s["id"], s.get("fallback_as_of")) for s in b.get("start_states", [])])
PY
