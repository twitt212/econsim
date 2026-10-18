#!/usr/bin/env bash
# Speed benchmark: construction and burn-in time, and months per second, at several household counts.
#
#   scripts/bench_speed.sh <threads> <out.tsv> <label=path/to/profile-binary> [<label=path> ...]
#
# Uses the `profile` bin (crates/econsim-cli/src/bin/profile.rs), which runs sim::new_world: construction
# (init::build_world) followed by cfg.burn_in_months of burn-in. Three timings per build and household count:
#   construct_s  0-month run with ECONSIM_CFG={"burn_in_months":0}: world construction only
#   init_s       0-month run with the build's own default burn-in: construction plus burn-in
#   months_s     24-month run with the build's own default config: months per second after init
# Each build's burn-in is its own Config default (read it from docs/CALIBRATION.md, Config ledger), so a build with
# a longer burn-in shows a longer init_s; construct_s separates the two.
# Builds are interleaved within each repeat (one process at a time) so that load on the shared machine hits
# both builds alike. Each timing keeps the best of REPEATS, since other load only ever slows a run down.
# bash 3.2 compatible (macOS default): no associative arrays.
set -euo pipefail
threads="$1"; out="$2"; shift 2
builds=("$@")
nb=${#builds[@]}
months=24
repeats=3
now() { python3 -c 'import time;print(time.time())'; }
best() { # best MODE(min|max) CURRENT NEW  -> prints the better of the two (empty CURRENT means NEW)
  python3 -c "import sys; a,b=sys.argv[2],float(sys.argv[3]); print(b if a=='' else (min(float(a),b) if sys.argv[1]=='min' else max(float(a),b)))" "$1" "$2" "$3"
}
for n in 20000 50000 100000; do
  bc=(); bi=(); bm=()
  for ((k = 0; k < nb; k++)); do bc[k]=""; bi[k]=""; bm[k]=""; done
  for _ in $(seq "$repeats"); do
    for ((k = 0; k < nb; k++)); do
      bin="${builds[k]#*=}"
      t0=$(now); RAYON_NUM_THREADS=$threads ECONSIM_CFG='{"burn_in_months":0}' "$bin" "$n" 0 > /dev/null; t1=$(now)
      RAYON_NUM_THREADS=$threads "$bin" "$n" 0 > /dev/null; t2=$(now)
      ms=$(RAYON_NUM_THREADS=$threads "$bin" "$n" "$months" | tail -1 | sed -E 's/.*\(([0-9.]+) months\/s\).*/\1/')
      c=$(python3 -c "print(round($t1-$t0,2))"); i=$(python3 -c "print(round($t2-$t1,2))")
      bc[k]=$(best min "${bc[k]}" "$c"); bi[k]=$(best min "${bi[k]}" "$i"); bm[k]=$(best max "${bm[k]}" "$ms")
    done
  done
  for ((k = 0; k < nb; k++)); do
    printf "%s\t%d\t%d\t%s\t%s\t%s\n" "${builds[k]%%=*}" "$threads" "$n" "${bc[k]}" "${bi[k]}" "${bm[k]}" >> "$out"
  done
done
