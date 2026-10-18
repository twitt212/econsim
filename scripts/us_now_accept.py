#!/usr/bin/env python3
"""Acceptance gates for the "Today's US economy" start state (month 0 fit and months 1-6 stability).
Python 3 standard library only.

  server mode : us_now_accept.py --server http://127.0.0.1:8787 --snapshot scripts/data/us_now.json [--households 5000] [--first-seed 1] [--n 12]
                one POST /api/compare per seed (seeds [s], months 12, no shocks, start_state us_now with the snapshot's targets);
                month 0 = baseline[0].
  files mode  : us_now_accept.py --dir DIR --prefix G4 --snapshot us_now.json [--row0 24] [--first-seed 1] [--n 12]
                reads DIR/PREFIX_s<seed>.json (a JSON array of Metrics rows, as the prototype CLI writes) with month 0 at index ROW0.
Exit status 0 when every gate passes, 1 otherwise. Prints one line per gate."""
import argparse, json, math, statistics as st, sys, urllib.request

ap = argparse.ArgumentParser()
ap.add_argument("--server"); ap.add_argument("--dir"); ap.add_argument("--prefix", default="G4")
ap.add_argument("--snapshot", required=True)
ap.add_argument("--households", type=int, default=5000)
ap.add_argument("--first-seed", type=int, default=1); ap.add_argument("--n", type=int, default=12)
ap.add_argument("--row0", type=int, default=24)
ap.add_argument("--no-start", action="store_true", help="server mode plumbing check: do not send start_state")
a = ap.parse_args()

snap = json.load(open(a.snapshot))
tg = snap["targets"]; W = tg["window"]
seeds = list(range(a.first_seed, a.first_seed + a.n))

def load_server(seed):
    cfg = {"n_households": a.households}
    if not a.no_start:
        cfg["start_state"] = "us_now"; cfg["start_targets"] = tg
    body = json.dumps({"config": cfg, "shocks": [], "months": 12, "seeds": [seed]}).encode()
    req = urllib.request.Request(a.server + "/api/compare", data=body, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=600) as r:
        return json.load(r)["baseline"]

def load_file(seed):
    return json.load(open(f"{a.dir}/{a.prefix}_s{seed}.json"))[a.row0:]

hist = [(load_server if a.server else load_file)(s) for s in seeds]
assert all(len(h) >= 7 for h in hist), "need months 0..6"

def val(m, k):
    if k == "y3m": return m["yields"][0]
    if k == "y2": return m["yields"][1]
    if k == "y5": return m["yields"][2]
    if k == "y10": return m["yields"][3]
    if k == "y30": return m["yields"][4]
    return m[k]

T = {  # model metric -> target
 "policy_rate": W["policy_rate"][-1], "y3m": W["y3m"][-1], "y2": W["y2"][-1], "y5": W["y5"][-1], "y10": W["y10"][-1], "y30": W["y30"][-1],
 "mortgage_rate": W["mortgage_rate"][-1], "oil_price": W["oil_wti"][-1], "tariff_rate_pct": W["tariff_pct"][-1],
 "unemployment_rate": tg["unemployment_rate"],
}
disp = {r["id"]: r["value"] for r in snap["display"]}
T["core_pce_inflation_yoy"] = disp["core_pce_yoy"]; T["inflation_yoy"] = disp["cpi_yoy"]; T["core_inflation_yoy"] = disp["core_cpi_yoy"]
T["wage_growth_yoy"] = disp["ahe_yoy"]; T["hpi_growth_yoy"] = disp["hpi_cs_yoy"]; T["gdp_real_growth_yoy"] = disp["gdp_yoy"]
T["gasoline_price"] = disp["gasoline"]

# month-0 gates: (metric, mean_lo, mean_hi, seed_lo, seed_hi) as offsets from the target (None = not gated)
G0 = [
 ("policy_rate",           -0.05, 0.05,  -0.25, 0.25),
 ("y3m",                   -0.06, 0.06,  -0.30, 0.30),
 ("y2",                    -0.08, 0.08,  -0.35, 0.35),
 ("y5",                    -0.10, 0.10,  -0.40, 0.40),
 ("y10",                   -0.12, 0.12,  -0.50, 0.50),
 ("y30",                   -0.14, 0.14,  -0.60, 0.60),
 ("mortgage_rate",         -0.12, 0.12,  -0.50, 0.50),
 ("oil_price",             -6.0,  6.0,   -35.0, 35.0),
 ("gasoline_price",        -0.40, 0.10,  -1.00, 1.00),
 ("tariff_rate_pct",       -0.01, 0.01,  -0.01, 0.01),
 ("unemployment_rate",     -0.25, 0.25,  -0.80, 0.80),
 ("core_pce_inflation_yoy",-0.90, 0.90,  -4.00, 4.00),
 ("inflation_yoy",         -0.60, 1.60,  -2.00, 4.50),
 ("core_inflation_yoy",    -0.60, 1.40,  -4.00, 4.00),
 ("wage_growth_yoy",       -0.50, 2.50,  None, None),
 ("hpi_growth_yoy",        -1.50, 3.00,  None, None),
 ("gdp_real_growth_yoy",   -2.00, 0.80,  None, None),
]
# stability gates: mean over seeds of (x[k] - x[0]) must lie in [lo, hi]
G1 = [  # metric, k, lo, hi
 ("policy_rate", 1, -0.10, 0.10), ("policy_rate", 6, -0.60, 0.40),
 ("y10", 1, -0.15, 0.15), ("y10", 6, -0.35, 0.35),
 ("mortgage_rate", 1, -0.15, 0.15), ("mortgage_rate", 6, -0.35, 0.35),
 ("y2", 6, -0.50, 0.30),
 ("oil_price", 1, -8.0, 8.0), ("oil_price", 6, -20.0, 20.0),
 ("unemployment_rate", 1, -0.10, 0.50), ("unemployment_rate", 6, -0.20, 1.20),
 ("core_pce_inflation_yoy", 1, -0.40, 0.40), ("core_pce_inflation_yoy", 6, -0.70, 0.70),
 ("inflation_yoy", 1, -0.40, 0.40), ("inflation_yoy", 6, -2.30, 0.60),
 ("hpi_growth_yoy", 6, -2.50, 0.40),
]
fails = 0
def report(ok, msg):
    global fails
    fails += 0 if ok else 1
    print(("PASS " if ok else "FAIL ") + msg)

print(f"{len(seeds)} seeds {seeds[0]}..{seeds[-1]}, month 0 = first reported month; targets from {a.snapshot} (as_of {snap['as_of']})")
for k, mlo, mhi, slo, shi in G0:
    xs = [val(h[0], k) for h in hist]
    assert all(math.isfinite(x) for x in xs), k
    t = T[k]; m = st.mean(xs)
    report(t + mlo <= m <= t + mhi, f"month 0 {k:24s} target {t:9.3f} mean {m:9.3f} (allowed {t+mlo:.3f}..{t+mhi:.3f})")
    if slo is not None:
        lo, hi = min(xs), max(xs)
        report(t + slo <= lo and hi <= t + shi, f"month 0 {k:24s} per-seed range {lo:9.3f}..{hi:9.3f} (allowed {t+slo:.3f}..{t+shi:.3f})")
for k, dm, lo, hi in G1:
    dv = [val(h[dm], k) - val(h[0], k) for h in hist]
    m = st.mean(dv)
    report(lo <= m <= hi, f"stability {k:24s} mean change month 0 -> {dm}: {m:+8.3f} (allowed {lo:+.2f}..{hi:+.2f})")
bad = [(s, i) for s, h in zip(seeds, hist) for i, m in enumerate(h[:7]) if not all(math.isfinite(v) for v in [m["unemployment_rate"], m["inflation_yoy"], m["policy_rate"], m["gdp_real_growth_yoy"]])]
report(not bad, f"all months 0..6 finite for every seed {bad[:3]}")
report(all(abs(h[0]["sfc_error"]) == 0 for h in hist), "sfc_error == 0 at month 0")
jump = max(abs(h[1]["unemployment_rate"] - h[0]["unemployment_rate"]) for h in hist)
report(jump <= 2.0, f"no single-seed u jump above 2.0pp between months 0 and 1 (max {jump:.2f})")
print("RESULT:", "ALL GATES PASS" if fails == 0 else f"{fails} GATE(S) FAILED")
sys.exit(1 if fails else 0)
