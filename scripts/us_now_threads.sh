#!/bin/bash
# A us_now compare is byte-identical at RAYON_NUM_THREADS=1 and 2 (cache off). usage: us_now_threads.sh SERVER_BIN SNAPSHOT.json OUTDIR   (PORT, default 8814, must be free)
BIN="$1"; SNAP="$2"; OUT="$3"; mkdir -p "$OUT"
PORT="${PORT:-8814}"
python3 -I - "$SNAP" "$OUT/body.json" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))["targets"]
json.dump({"config": {"n_households": 2000, "start_state": "us_now", "start_targets": t}, "shocks": [{"at_month": 0, "shock": {"type": "OilPrice", "pct": 30, "half_life_months": 6}}], "months": 12, "seeds": [4, 8]}, open(sys.argv[2], "w"))
PY
for n in 1 2; do
  ECONSIM_CACHE_MB=0 RAYON_NUM_THREADS=$n PORT=$PORT "$BIN" > "$OUT/server_$n.log" 2>&1 & PID=$!
  sleep 2
  curl -s -X POST -H 'Content-Type: application/json' -d @"$OUT/body.json" http://127.0.0.1:$PORT/api/compare > "$OUT/resp_$n.json"
  kill $PID; wait $PID 2>/dev/null
done
if cmp -s "$OUT/resp_1.json" "$OUT/resp_2.json"; then echo "IDENTICAL across RAYON 1 and 2 ($(wc -c < "$OUT/resp_1.json") bytes)"; else echo "DIFFERENT"; fi
