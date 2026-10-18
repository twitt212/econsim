#!/usr/bin/env python3
"""usnow_accept.py: the pre-registered checks of the Today's-start validation note on one block of seeds, from run files written by uprobe.py.

usage:
  uprobe.py SERVER PORT0 CELL SCEN FIRST LAST MONTHS      (one call per cell and scenario; writes runs/CELL_SCEN/sSEED.json under $SCR)
  usnow_accept.py [--rows S,T,O] [--seeds A-B] [--first-year-only]   (reads $SCR/runs; prints a PASS/FAIL table; writes nothing else)

Python 3 standard library only; run with `python3 -I`. $SCR is the work folder (default: this file's folder). Rows and rules are copied from
the validation note (not published; summarised in docs/CALIBRATION.md, `world-usnow`); a row that cannot be read because its cell was not run prints n/a. A number in brackets is the standard error over seeds.
"""
import json, os, glob, math, sys, argparse

HERE = os.path.dirname(os.path.abspath(__file__))
SCR = os.environ.get("SCR", HERE)
BL = ["EA", "CN", "JP", "UK", "CA", "MX", "IN", "KR", "BR", "AU", "OPEC+", "RoW"]
# the table the model starts from (data/usnow_ref.json, vintage 2026-08-24): import shares 2025 (%), mean duty by bloc on 2025 weights (%), partners' duty on US goods (%)
SHARE25 = dict(zip(BL, [16.6, 9.0, 4.3, 1.9, 11.2, 15.6, 3.0, 3.7, 1.2, 0.8, 3.2, 29.5]))
DUTY25 = dict(zip(BL, [7.9, 26.5, 12.9, 7.1, 7.6, 6.9, 11.4, 10.3, 11.3, 9.1, 7.6, 10.9]))
RET = dict(zip(BL, [0, 31.9, 0, 0, 3.0, 0, 0, 0, 0, 0, 0, 0]))
GOODS_SHARE_TOL = 2.0   # pp; per bloc max(0.4, 15% of the share), at most 2.0; China 0.7


def load(cell, scen, seeds=None):
    out = {}
    for f in sorted(glob.glob(f"{SCR}/runs/{cell}_{scen}/s*.json")):
        s = int(os.path.basename(f)[1:-5])
        if seeds and not any(lo <= s <= hi for lo, hi in seeds):   # seeds: a list of (lo, hi) blocks
            continue
        out[s] = json.load(open(f))
    return out


def mean(x): return sum(x) / len(x) if x else float("nan")
def sd(x):
    if len(x) < 2: return float("nan")
    m = mean(x); return math.sqrt(sum((v - m) ** 2 for v in x) / (len(x) - 1))
def se(x): return sd(x) / math.sqrt(len(x)) if len(x) > 1 else float("nan")
def get(r, arm, key, m, j=None):
    v = r[arm][key][m]
    return v[j] if j is not None else v
def vals(runs, arm, key, m, j=None): return [get(r, arm, key, m, j) for r in runs.values()]
def paired(runs, key, m, j=None, mode="diff"):
    d = []
    for r in runs.values():
        b, s = get(r, "b", key, m, j), get(r, "s", key, m, j)
        d.append(100.0 * (s / b - 1.0) if mode == "pct" else s - b)
    return mean(d), se(d)
def paired_avg(runs, key, m0, m1, j=None, mode="diff"):
    d = []
    for r in runs.values():
        v = []
        for m in range(m0, m1 + 1):
            b, s = get(r, "b", key, m, j), get(r, "s", key, m, j)
            v.append(100.0 * (s / b - 1.0) if mode == "pct" else s - b)
        d.append(mean(v))
    return mean(d), se(d)
def between_cells(a, b, key, m, j=None):
    """UR minus U0 style difference of the cell means with the unpaired SE (the cells do not share a common world: the profile moves ABM keys)."""
    x, y = vals(a, "b", key, m, j), vals(b, "b", key, m, j)
    return mean(x) - mean(y), math.hypot(se(x), se(y))

ROWS = []
def row(rid, text, value, lo, hi, se_=float("nan"), unit=""):
    ok = value == value and (lo is None or value >= lo) and (hi is None or value <= hi)
    near = False
    if se_ == se_ and se_ > 0:
        for bnd in (lo, hi):
            if bnd is not None and abs(value - bnd) <= se_: near = True
    if rid.startswith("R-"):   # reported rows: never a pass or a fail
        verdict = "n/a" if value != value else ("REPORT inside the reading band" if ok else "REPORT outside the reading band") + (" (within 1 SE of a bound)" if near else "")
    else:
        verdict = "n/a" if value != value else ("PASS" if ok else "FAIL") + (" (within 1 SE of a bound)" if near else "")
    ROWS.append((rid, text, value, se_, lo, hi, verdict, unit))


def start_rows(a):
    R = load("UR", "none", a)
    if not R:
        return
    n = len(R)
    # S1a duty by origin at month 0, goods-weighted by the model's own sector weights: within 1.0 pp of the table's mean on 2025 weights
    for j, b in enumerate(BL):
        v = [100 * x for x in vals(R, "b", "w_tau_by_origin", 0, j)]
        row(f"S1a {b}", f"US duty on {b} at month 0 (table {DUTY25[b]} on 2025 weights)", mean(v), DUTY25[b] - 1.0, DUTY25[b] + 1.0, se(v), "%")
    # S1b retaliation equals the table
    for j, b in enumerate(BL):
        v = [100 * x for x in vals(R, "b", "w_ret_tau", 0, j)]
        row(f"S1b {b}", f"{b}'s duty on US goods at month 0 (table {RET[b]})", mean(v), RET[b] - 0.01, RET[b] + 0.01, se(v), "%")
    # S1c goods import shares within 2.0 pp of 2025 (China 0.7)
    tot = [sum(get(r, "b", "w_imports_goods_bn", 0, j) for j in range(12)) for r in R.values()]
    for j, b in enumerate(BL):
        sh = [100 * get(r, "b", "w_imports_goods_bn", 0, j) / t for r, t in zip(R.values(), tot)]
        tol = 0.7 if b == "CN" else min(GOODS_SHARE_TOL, max(0.4, 0.15 * SHARE25[b]))
        row(f"S1c {b}", f"{b} share of US goods imports at month 0 (2025: {SHARE25[b]}%)", mean(sh), SHARE25[b] - tol, SHARE25[b] + tol, se(sh), "%")
    # S1d the ABM's duty is the realised ratio
    row("S1d", "tariff_rate_pct at month 0 = the snapshot's last value 8.22", mean(vals(R, "b", "tariff_rate_pct", 0)), 8.2199, 8.2201, 0.0, "%")
    # S2 steered fit (month-0 means against the snapshot: us_now_accept.py bands)
    for key, tgt, tol, nm in [("policy_rate", 4.00, 0.05, "policy rate"), ("y10", 5.22, 0.12, "10-year"), ("mortgage_rate", 7.40, 0.12, "mortgage rate"), ("oil_price", 96.24, 6.0, "oil"), ("unemployment_rate", 4.20, 0.25, "unemployment")]:
        v = vals(R, "b", key, 0)
        row(f"S2 {key}", f"month-0 mean {nm} against the data {tgt}", mean(v), tgt - tol, tgt + tol, se(v))
    # S3 stability months 0 -> 6 (bands of us_now_accept.py, calibrated world-off)
    for key, lo, hi in [("policy_rate", -0.60, 0.40), ("y10", -0.35, 0.35), ("mortgage_rate", -0.35, 0.35), ("unemployment_rate", -0.20, 1.20), ("inflation_yoy", -2.30, 0.60)]:
        d = [get(r, "b", key, 6) - get(r, "b", key, 0) for r in R.values()]
        row(f"S3 {key}", f"mean change month 0 to 6 of {key}", mean(d), lo, hi, se(d))
    # S4 the install is continuous: UR minus UN at month 0 (same seeds); the blocs' baseline gaps do not collapse
    N = load("UN", "none", a)
    if N:
        common = [s for s in R if s in N]
        for key, tol in [("exports_bn_ar", 0.1), ("imports_bn_ar", 0.1), ("import_price_index", 0.1)]:
            d = [100 * (get(R[s], "b", key, 0) / get(N[s], "b", key, 0) - 1) for s in common]
            row(f"S4a {key}", f"UR minus UN at month 0, % (the install is continuous)", mean(d), -tol, tol, se(d), "%")
        for j, b in enumerate(BL):
            d = [mean([get(R[s], "b", "w_y_gap", m, j) - get(N[s], "b", "w_y_gap", m, j) for m in range(1, 25)]) for s in common]
            row(f"S4b {b}", f"{b} baseline output gap, mean of months 1-24, UR minus UN (no collapse from the share change)", mean(d), -1.0, 1.0, se(d), "pts")
    drift_rows(R)
    # S5 level agreement UR against U0 (the Compare rule): |diff| <= max(2 SE, tolerance)
    U = load("U0", "none", a)
    if U:
        for key, tol in [("unemployment_rate", 0.3), ("inflation_yoy", 0.3), ("core_pce_inflation_yoy", 0.3), ("policy_rate", 0.3), ("y10", 0.15), ("mortgage_rate", 0.15), ("gdp_real_growth_yoy", 0.6)]:
            for m in (0, 6, 12):
                d, s_ = between_cells(R, U, key, m)
                lim = max(2 * s_, tol)
                row(f"S5 {key} m{m}", f"UR minus U0 baseline level (limit max(2 SE, {tol}))", d, -lim, lim, s_)


def drift_rows(R):
    """S6 (VALIDATION section 3): the world block's baseline drift, no change, months 0 to 23: the duty by origin moves by less than 0.3 pp, the goods import shares by less than 0.8 pp"""
    for j, b in enumerate(BL):
        d = [100 * (get(r, "b", "w_tau_by_origin", 23, j) - get(r, "b", "w_tau_by_origin", 0, j)) for r in R.values()]
        row(f"S6 duty {b}", f"drift of the US duty on {b}, month 0 to 23 (baseline arm)", mean(d), -0.3, 0.3, se(d), "pp")
    for j, b in enumerate(BL):
        d = []
        for r in R.values():
            t0 = sum(get(r, "b", "w_imports_goods_bn", 0, k) for k in range(12)); t23 = sum(get(r, "b", "w_imports_goods_bn", 23, k) for k in range(12))
            d.append(100 * (get(r, "b", "w_imports_goods_bn", 23, j) / t23 - get(r, "b", "w_imports_goods_bn", 0, j) / t0))
        row(f"S6 share {b}", f"drift of {b}'s share of US goods imports, month 0 to 23 (baseline arm)", mean(d), -0.8, 0.8, se(d), "pp")


def converted_band(lo, hi, tau0, delta):
    """The typical-start band [lo, hi] % for an increment `delta` (decimal) on a duty of 0, moved to a base duty tau0: (1 + x)^f - 1, f = ln((1+tau0+delta)/(1+tau0)) / ln(1+delta)."""
    f = math.log((1 + tau0 + delta) / (1 + tau0)) / math.log(1 + delta)
    return 100 * ((1 + lo / 100) ** f - 1), 100 * ((1 + hi / 100) ** f - 1), f


def event_rows(a):
    R = load("UR", "tarCN25", a)
    if R:
        tau0 = mean(vals(R, "b", "w_tau_by_origin", 11, 1))
        lo, hi, f = converted_band(-40, -20, tau0, 0.25)
        m, s_ = paired(R, "w_imports_bn", 11, 1, "pct")
        row("T-W2-07", f"US imports from China, month 12, +25 on China (band [-40,-20] moved to the base duty {100 * tau0:.1f}%: factor {f:.3f})", m, lo, hi, s_, "%")
        oth = []
        for r in R.values():
            lost = get(r, "b", "w_imports_bn", 11, 1) - get(r, "s", "w_imports_bn", 11, 1)
            gain = sum(get(r, "s", "w_imports_bn", 11, j) - get(r, "b", "w_imports_bn", 11, j) for j in range(12) if j != 1)
            oth.append(gain / lost if lost else float("nan"))
        row("R-W2-08", "diversion: change in imports from other origins over the loss from China, month 12 (reported, [0.3, 1.0])", mean(oth), 0.3, 1.0, se(oth))
    R = load("UR", "tarCN15r", a)
    if R:
        def el_exp(r):
            b, s = get(r, "b", "w_exports_goods_bn", 11, 1), get(r, "s", "w_exports_goods_bn", 11, 1)
            rb, rs = get(r, "b", "w_ret_tau", 11, 1), get(r, "s", "w_ret_tau", 11, 1)
            return math.log(s / b) / math.log((1 + rs) / (1 + rb))
        def el_imp(r):
            b, s = get(r, "b", "w_imports_bn", 11, 1), get(r, "s", "w_imports_bn", 11, 1)
            tb, ts = get(r, "b", "w_tau_by_origin", 11, 1), get(r, "s", "w_tau_by_origin", 11, 1)
            return math.log(s / b) / math.log((1 + ts) / (1 + tb))
        x = [el_exp(r) for r in R.values()]; y = [el_imp(r) for r in R.values()]
        row("T-W4-24i", "goods exports to China per ln of the retaliatory duty ratio, month 12 (band [-5,-2])", mean(x), -5.0, -2.0, se(x))
        row("T-W4-24ii", "imports from China per ln of the US duty ratio, month 12 (band [-1.7,-0.8])", mean(y), -1.7, -0.8, se(y))
    R = load("UR", "tarwar", a)
    if R:
        cpi, cs = paired_avg(R, "inflation_yoy", 0, 11)
        g, gs = paired(R, "gdp_real_bn_ar", 11, mode="pct")
        # the CPI band [0.5, 1.3] is a typical-start band whose lower edge includes the China increment's share of the effect;
        # rule (4) of VALIDATION section 4 moves that part by the exposure ratio 9.0/13.1: 0.50 - 0.06 x (1 - 0.69) = 0.48
        row("T-W5-04 CPI", "tariff war: US CPI year-1 average, pp (typical band [0.5, 1.3]; moved by rule (4) to [0.48, 1.3])", cpi, 0.48, 1.3, cs)
        row("R-W5-04 CPI unmoved", "tariff war: US CPI year-1 average, pp against the UNMOVED typical band [0.5, 1.3] (printed beside the moved edge, rule (4))", cpi, 0.5, 1.3, cs)
        row("T-W5-04 GDP", "tariff war: US real GDP, month 12, % (band [-1.6, -0.5])", g, -1.6, -0.5, gs)
        m, s2 = paired(R, "w_imports_bn", 11, 1, "pct")
        row("T-W5-04 imports", "tariff war: US imports from China, month 12, % (typical band [-40, -20], moved to [-34.0, -16.6])", m, -34.0, -16.6, s2)
        row("R-W5-04 imports unmoved", "tariff war: US imports from China, month 12, % against the UNMOVED typical band [-40, -20] (printed beside the moved edge)", m, -40.0, -20.0, s2)
        ex = []
        for r in R.values():
            b, s_ = get(r, "b", "w_exports_goods_bn", 11, 1), get(r, "s", "w_exports_goods_bn", 11, 1)
            rb, rs = get(r, "b", "w_ret_tau", 11, 1), get(r, "s", "w_ret_tau", 11, 1)
            ex.append(math.log(s_ / b) / math.log((1 + rs) / (1 + rb)))
        row("T-W5-04 elasticity", "tariff war: goods exports to China per ln of the retaliatory duty ratio, month 12 (band [-5,-2])", mean(ex), -5.0, -2.0, se(ex))
        yg, ys = paired_avg(R, "w_y_gap", 0, 23, 1)
        row("R-W5-04 China gap", "tariff war: China's output gap, mean of months 1-24, pts (reported; reading band [-0.8, -0.2], typical [-1.2, -0.3] x 0.67)", yg, -0.8, -0.2, ys)
    # O1 research ranges (first-year averages), the tariff card at 10 and 25 points
    for scen, k, lo_i, hi_i, lo_g, hi_g in (("tarU10", 1.0, 0.19, 0.79, -0.17, -0.05), ("tarU25", 2.5, 0.19, 0.79, -0.17, -0.05)):
        R = load("UR", scen, a)
        if not R: continue
        c, cs = paired_avg(R, "inflation_yoy", 0, 11)
        g, gs = paired_avg(R, "gdp_real_bn_ar", 0, 11, mode="pct")
        row(f"O1 {scen} inflation", f"first-year average inflation effect, pp (research {lo_i * k:.2f} to {hi_i * k:.2f})", c, lo_i * k, hi_i * k, cs)
        row(f"O1 {scen} GDP", f"first-year average GDP effect, % (research {lo_g * k:.3f} to {hi_g * k:.3f})", g, lo_g * k, hi_g * k, gs)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", default="1-64")
    ap.add_argument("--rows", default="S,T")
    a = ap.parse_args()
    seeds = [tuple(map(int, blk.split("-"))) for blk in a.seeds.split(",")]   # e.g. 41201-41216,41249-41296
    if "S" in a.rows: start_rows(seeds)
    if "T" in a.rows or "O" in a.rows: event_rows(seeds)
    npass = sum(1 for r in ROWS if r[6].startswith("PASS")); nfail = sum(1 for r in ROWS if r[6].startswith("FAIL")); nrep = sum(1 for r in ROWS if r[6].startswith("REPORT"))
    for rid, text, v, s_, lo_, hi_, verdict, unit in ROWS:
        b = f"[{'' if lo_ is None else f'{lo_:.3g}'}, {'' if hi_ is None else f'{hi_:.3g}'}]"
        print(f"{rid:16s} {v:9.3f} ({s_:.3f}) {b:18s} {verdict:34s} {text}")
    print(f"\n{npass} PASS, {nfail} FAIL, {nrep} reported, {len(ROWS) - npass - nfail - nrep} n/a")
    return 1 if nfail else 0


if __name__ == "__main__":
    sys.exit(main())
