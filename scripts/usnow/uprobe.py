#!/usr/bin/env python3
"""uprobe.py: run compare requests against probe servers (two ports from PORT0), keep compact per-seed series in a work folder.
usage: uprobe.py SERVER_BIN PORT0 CELL SCEN FIRST LAST MONTHS [EXTRA_CFG_JSON]
cells: T0 (typical, warm-up 24, c3 + t2a), U0 (us_now, c3, no world), UW (us_now, c3 + t2a)
Seeds 1-64 only unless USNOW_REGISTERED=1 (the registered blocks of the validation note; set by the validation run, never by a design run).
Ports: PORT0 and PORT0+1 must be free."""
import sys, os, json, time, subprocess, threading, urllib.request
SCR = os.environ.get("SCR", os.path.dirname(os.path.abspath(__file__)))   # run summaries go to $SCR/runs
WT = os.environ.get("ECONSIM_REPO", os.path.abspath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")))   # the repository (profiles and the Today's snapshot are read from it)
C3 = {"rate_perception_months": 11, "rate_perception_firm_months": 7, "rate_perception_stages": 3, "rate_perception_deposit": True,
      "rate_perception_credit_bypass": 0.7, "credit_tail_half_life_months": 4, "fisim_constant_margin": True, "rate_smoothing": 0.88,
      "speed_limit_weight": 0.75, "energy_extraction_labor_share": 0.11, "energy_extraction_elasticity": 1, "user_cost_pi_e_mode": 2,
      "user_cost_pi_e_offset": 0.0035, "hh_pump_real_rate_weight": 0, "phi_u": 0.25}
T2A = json.load(open(WT + "/scripts/profiles/b8-world-t2a.json"))
TARGETS = json.load(open(WT + "/scripts/data/us_now.json"))["targets"]
L0 = TARGETS["window"]["tariff_pct"][-1]

def cell_cfg(cell, hh=5000):
    c = dict(C3); c["n_households"] = hh
    if cell.startswith("T"):
        c.update(T2A); c["warmup_months"] = 24
    elif cell.startswith("UR"):   # us_now + world + the Today's trade reference
        c.update(T2A); c["start_state"] = "us_now"; c["start_targets"] = dict(TARGETS, trade_ref="2026-08-24")
    elif cell.startswith("UN"):   # us_now + world, trade_ref "none": the uniform 8.22% start, 2024 shares, no retaliation in place (option 0)
        c.update(T2A); c["start_state"] = "us_now"; c["start_targets"] = dict(TARGETS, trade_ref="none")
    elif cell.startswith("UC"):   # sensitivity: the blocs' centring offsets moved by the displacement of the steered US inputs (ius +1.5, y_us +2.1)
        c.update(T2A); c["world_ius_offset"] = 1.07; c["world_yus_offset"] = 1.2; c["start_state"] = "us_now"; c["start_targets"] = dict(TARGETS)
    elif cell.startswith("US"):   # sensitivity: the reference duties scaled to the realised level (x0.768)
        c.update(T2A); c["start_state"] = "us_now"; c["start_targets"] = dict(TARGETS, trade_ref="2026-08-24-s77")
    elif cell.startswith("UB"):   # back-cast: Today's macro state, the Jan-2025 trade reference, then the 2025-26 increments as Tariff components
        c.update(T2A); c["start_state"] = "us_now"; c["start_targets"] = dict(TARGETS, trade_ref="2025-01-01")
    elif cell.startswith("UW"):
        c.update(T2A); c["start_state"] = "us_now"; c["start_targets"] = TARGETS
    elif cell.startswith("U0"):
        c["start_state"] = "us_now"; c["start_targets"] = TARGETS
    elif cell.startswith("V0"):   # c3 only, typical, no world (reference)
        c["warmup_months"] = 24
    return c

def tar(partner, sector, pct, r): return {"at_month": 0, "shock": {"type": "Tariff", "partner": partner, "sector": sector, "pct": pct, "retaliate": r}}
def legacy(p, base): return {"at_month": 0, "shock": {"type": "ImportTariff", "pct": round(p + base, 2)}}
SC = {
    "none": lambda base: [],
    "tarU10": lambda base: [tar(None, None, 10.0, False)],
    "tarU25": lambda base: [tar(None, None, 25.0, False)],
    "tarU25r": lambda base: [tar(None, None, 25.0, True)],
    "tarCN25": lambda base: [tar("CN", None, 25.0, False)],
    "tarCN25r": lambda base: [tar("CN", None, 25.0, True)],
    "tarCN15r": lambda base: [tar("CN", None, 15.0, True)],
    "tarUK25r": lambda base: [tar("UK", None, 25.0, True)],
    "tarUK25": lambda base: [tar("UK", None, 25.0, False)],
    "tarCA25r": lambda base: [tar("CA", None, 25.0, True)],
    "tarMX25": lambda base: [tar("MX", None, 25.0, False)],
    "tarDT25": lambda base: [tar(None, 2, 25.0, False), tar(None, 4, 25.0, False)],
    "tarwar": lambda base: [tar(None, None, 10.0, True), tar("CN", None, 15.0, True)],
    "leg10": lambda base: [legacy(10.0, base)],
    "uimp10": lambda base: [legacy(10.0, L0)],     # ImportTariff at the level 8.22 + 10 in a world cell (W5-06 on Today's start)
    "uimp25": lambda base: [legacy(25.0, L0)],
    "leg25": lambda base: [legacy(25.0, base)],
}
def backcast_shocks(avg=False):
    """the 2025 back-cast (VALIDATION O2): the increments between the vintages 2025-01-01 and 2026-08-24 by bloc and goods group, as Tariff components.
    avg=True scales each bloc's increments to the 2025 time-average duty (data/backcast_k.json, made by data/build_backcast_k.py)."""
    tab = json.load(open(os.environ.get("PROBE4_TABLE", WT + "/crates/econsim-core/src/world/data/usnow_ref.json")))["vintages"]   # needs the 2025-01-01 vintage: see DESIGN 8 and data/usnow_ref_with_probe_vintages.json
    a, b = tab["2025-01-01"]["duty"], tab["2026-08-24"]["duty"]
    names = ["EA", "CN", "JP", "UK", "CA", "MX", "IN", "KR", "BR", "AU", "OPEC+", "RoW"]
    k = json.load(open(os.environ.get("USNOW_BACKCAST_K", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "data", "backcast_k.json"))))["k"] if avg else {}   # USNOW_BACKCAST_K: path of backcast_k.json
    out = []
    for j in range(12):
        for s_ in range(5):
            d = 100.0 * (b[j][s_] - a[j][s_]) * k.get(names[j], 1.0)
            if d > 0.05:
                out.append({"at_month": 0, "shock": {"type": "Tariff", "partner": names[j], "sector": s_, "pct": round(d, 3), "retaliate": j in (1, 4)}})
    return out
SC["backcast"] = lambda base: backcast_shocks()
SC["backcast_avg"] = lambda base: backcast_shocks(avg=True)
SCAL = ["unemployment_rate", "inflation_yoy", "core_inflation_yoy", "core_pce_inflation_yoy", "policy_rate", "mortgage_rate", "oil_price",
        "tariff_rate_pct", "tariff_retail_pct", "gdp_real_bn_ar", "gdp_real_growth_yoy", "exports_bn_ar", "imports_bn_ar", "dollar_index",
        "import_price_index", "consumption_real_bn_ar", "investment_real_bn_ar", "firms_alive", "tariff_revenue_bn_ar", "sfc_error", "cpi", "hh_import_share_pct"]
WARR = ["y_gap", "us_import_gap", "i_gap", "imports_bn", "imports_goods_bn", "exports_bn", "exports_goods_bn", "ret_tau", "tau_by_origin", "z", "policy", "infl", "r_import"]
WSCAL = ["tau_eff_hh", "tau_eff_firm", "import_volume_idx", "export_volume_idx", "import_price_idx", "c_t", "fx_broad"]

def pack(arm):
    out = {k: [m.get(k) for m in arm] for k in SCAL}
    out["y10"] = [m["yields"][3] for m in arm]
    out["y2"] = [m["yields"][1] for m in arm]
    if arm and arm[0].get("world"):
        for k in WARR: out["w_" + k] = [m["world"].get(k, [0.0]*12) for m in arm]
        for k in WSCAL: out["w_" + k] = [m["world"][k] for m in arm]
    return out

def main():
    server, port0, cell, scen, a, b, months = sys.argv[1], int(sys.argv[2]), sys.argv[3], sys.argv[4], int(sys.argv[5]), int(sys.argv[6]), int(sys.argv[7])
    extra = json.loads(sys.argv[8]) if len(sys.argv) > 8 else {}
    assert (1 <= a <= b <= 64) or os.environ.get("USNOW_REGISTERED") == "1", "design runs use seeds 1-64 only"
    cfg = cell_cfg(cell); cfg.update(extra)
    base = L0 if (cell.startswith("U0") or cell.startswith("UL")) else 0.0
    shocks = SC[scen](base)
    out = f"{SCR}/runs/{cell}_{scen}"; os.makedirs(out, exist_ok=True)
    env = dict(os.environ, RAYON_NUM_THREADS="1", ECONSIM_CACHE_MB="0", ECONSIM_WARMUP_MONTHS="24")
    ports = [port0, port0 + 1]
    procs = [subprocess.Popen([server], env=dict(env, PORT=str(p)), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for p in ports]
    time.sleep(2.0)
    jobs = [s for s in range(a, b + 1) if not os.path.exists(f"{out}/s{s}.json")]
    lock = threading.Lock(); errors = []
    def worker(port):
        while True:
            with lock:
                if not jobs: return
                s = jobs.pop(0)
            body = {"config": cfg, "shocks": shocks, "months": months, "seeds": [s]}
            try:
                req = urllib.request.Request(f"http://127.0.0.1:{port}/api/compare", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
                res = json.loads(urllib.request.urlopen(req, timeout=3600).read())
                rec = {"b": pack(res["baseline"]), "s": pack(res["shocked"]), "start_info": res.get("start_info"), "warm": res.get("warmup_months")}
                json.dump(rec, open(f"{out}/s{s}.json", "w"))
            except Exception as e:
                errors.append((s, str(e)[:300]))
    try:
        ts = [threading.Thread(target=worker, args=(p,)) for p in ports]
        [t.start() for t in ts]; [t.join() for t in ts]
    finally:
        for p in procs: p.terminate(); p.wait(timeout=30)
    print(cell, scen, a, b, "errors", errors)

if __name__ == "__main__":
    main()
