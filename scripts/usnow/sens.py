#!/usr/bin/env python3
"""sens.py: the sensitivity rows R1, R3, R4 and the UK check of the registered validation note (not published), from run files of uprobe.py (seeds 1-64).
usage: SCR=/path/to/work/folder python3 -I sens.py > results/sensitivities.txt
Every comparison pairs cells on the same seeds (a difference of two paired effects, SE over seeds)."""
import os, sys
HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
SCR = os.environ.get("SCR", HERE)
import an
an.SCR = SCR
from an import load, mean, se, get, paired, paired_avg


def per_seed(runs, stat):
    return {s: stat(r) for s, r in runs.items()}


def diff_of(a, b, stat):
    """mean (SE) of stat(a seed) - stat(b seed) over the common seeds"""
    sa, sb = per_seed(a, stat), per_seed(b, stat)
    d = [sa[s] - sb[s] for s in sa if s in sb]
    return mean(d), se(d), len(d)


def pct(key, m, j=None):
    return lambda r: 100.0 * (get(r, "s", key, m, j) / get(r, "b", key, m, j) - 1.0)


def avg_pct(key, m0, m1, j=None):
    return lambda r: mean([100.0 * (get(r, "s", key, m, j) / get(r, "b", key, m, j) - 1.0) for m in range(m0, m1 + 1)])


def avg_diff(key, m0, m1, j=None):
    return lambda r: mean([get(r, "s", key, m, j) - get(r, "b", key, m, j) for m in range(m0, m1 + 1)])


def show(title, cellA, cellB, scen, rows):
    A, B = load(cellA, scen), load(cellB, scen)
    if not A or not B:
        print(f"{title}: n/a (cell not run)")
        return
    print(f"{title}: {cellA} against {cellB}, scenario {scen}, {len(set(A) & set(B))} common seeds; per-seed paired effects, mean (SE); then their difference")
    for name, stat in rows:
        ma = mean(list(per_seed(A, stat).values())); sa = se(list(per_seed(A, stat).values()))
        mb = mean(list(per_seed(B, stat).values())); sb = se(list(per_seed(B, stat).values()))
        d, sd_, n = diff_of(A, B, stat)
        print(f"   {name:46s} {cellA} {ma:+8.3f} ({sa:.3f})   {cellB} {mb:+8.3f} ({sb:.3f})   difference {d:+8.3f} ({sd_:.3f})")


TE_ROWS = [("gdp_real_bn_ar", "%", 0.10), ("cpi", "%", 0.10), ("inflation_yoy", "pp", 0.05), ("core_pce_inflation_yoy", "pp", 0.05), ("unemployment_rate", "pp", 0.05),
           ("policy_rate", "pp", 0.05), ("consumption_real_bn_ar", "%", 0.10), ("dollar_index", "%", 0.20)]   # scripts/world_s5.py, tariff-eq


def w506(cell, a_tag, b_tag, seeds):
    """W5-06 as scripts/world_s5.py tariff-eq reads it: the largest paired-mean difference at months 3, 12, 24 over eight indicators, as a share of each indicator's site floor (rule <= 0.5)"""
    A, B = load(cell, a_tag), load(cell, b_tag)
    common = sorted(s for s in A if s in B and s in seeds)
    if not common:
        print(f"W5-06 {cell} {a_tag} against {b_tag}: n/a")
        return
    worst, where = 0.0, None
    for key, unit, floor in TE_ROWS:
        for m in (2, 11, 23):
            va = [(100.0 * (get(A[s], "s", key, m) / get(A[s], "b", key, m) - 1.0)) if unit == "%" else get(A[s], "s", key, m) - get(A[s], "b", key, m) for s in common]
            vb = [(100.0 * (get(B[s], "s", key, m) / get(B[s], "b", key, m) - 1.0)) if unit == "%" else get(B[s], "s", key, m) - get(B[s], "b", key, m) for s in common]
            d = abs(mean(va) - mean(vb))
            if d / floor > worst:
                worst, where = d / floor, (key, m + 1, d, floor)
    print(f"W5-06 {cell}: {a_tag} (Tariff form) against {b_tag} (ImportTariff form), {len(common)} seeds: largest difference {worst:.2f} of the site floor (rule <= 0.5) at {where}")


def o2(scen, label):
    """O2: the 2025 back-cast. Shares of US goods imports by origin in the shocked arm against the no-change arm (same seeds), months 12 and 24, against the Census 2024 to 2025 change in share"""
    import csv
    R = load("UB", scen)
    if not R:
        print(f"O2 {label}: n/a")
        return
    rows = list(csv.DictReader(open(os.path.join(os.environ.get("USNOW_INPUTS", os.path.join(HERE, "..", "world", "usnow_inputs")), "census_bloc_trade.csv"))))   # USNOW_INPUTS: the folder of the input csv files

    def shares(y):
        d = {r["bloc"]: float(r["goods_imports_usd_bn"]) for r in rows if r["year"] == y}
        t = sum(d.values())
        return {k: 100.0 * v / t for k, v in d.items()}
    s24, s25 = shares("2024"), shares("2025")
    print(f"O2 {label}: UB cell, scenario {scen}, {len(R)} seeds; change in the share of US goods imports (points), model (SE) against Census 2024 to 2025")
    tot = {11: 0.0, 23: 0.0}
    for j, nm in enumerate(an.BL):
        cells = []
        for m in (11, 23):
            ds = []
            for r in R.values():
                b = [get(r, "b", "w_imports_goods_bn", m, k) for k in range(12)]
                sh = [get(r, "s", "w_imports_goods_bn", m, k) for k in range(12)]
                ds.append(100.0 * sh[j] / sum(sh) - 100.0 * b[j] / sum(b))
            tot[m] += abs(mean(ds))
            cells.append(f"m{m + 1} {mean(ds):+6.2f} ({se(ds):.2f})")
        print(f"   {nm:6s} {'   '.join(cells)}   Census {s25[nm] - s24[nm]:+6.2f}")
    print(f"   sum of absolute changes: model m12 {tot[11]:.2f}, m24 {tot[23]:.2f}; Census {sum(abs(s25[k] - s24[k]) for k in an.BL):.2f}")
    d = [100.0 * (get(r, "s", "w_imports_bn", 11, 1) / get(r, "b", "w_imports_bn", 11, 1) - 1.0) for r in R.values()]
    print(f"   US imports from China, month 12: {mean(d):+.1f}% (SE {se(d):.1f}); Census 2025 against 2024: -29.9%")


def main():
    cn = [("US imports from China, month 12, %", pct("w_imports_bn", 11, 1)),
          ("US goods exports to China, month 12, %", pct("w_exports_goods_bn", 11, 1)),
          ("China output gap, mean months 1-24, pts", avg_diff("w_y_gap", 0, 23, 1)),
          ("US real GDP, first-year average, %", avg_pct("gdp_real_bn_ar", 0, 11)),
          ("US CPI inflation, first-year average, pp", avg_diff("inflation_yoy", 0, 11))]
    print("R1: duty reference at the realised level (every cell x 0.768), cell US, against the statutory shape, cell UR")
    show("  China 25 points, they hit back", "US", "UR", "tarCN25r", cn)
    show("  everyone 25 points, flagged", "US", "UR", "tarU25r", [("US real GDP, first-year average, %", avg_pct("gdp_real_bn_ar", 0, 11)), ("US CPI inflation, first-year average, pp", avg_diff("inflation_yoy", 0, 11)),
                                                           ("US imports, month 12, %", pct("imports_bn_ar", 11)), ("US exports, month 12, %", pct("exports_bn_ar", 11))])
    print()
    print("R2: the duty table from the separable rule (partner mean x goods mean / overall mean, no fit; build_usnow_ref.py --rule product), cell UR2, against the design (UR)")
    show("  China 25 points, they hit back", "UR2", "UR", "tarCN25r", cn)
    show("  two goods groups 25 points (Durables and Nondurables)", "UR2", "UR", "tarDT25", [("US real GDP, first-year average, %", avg_pct("gdp_real_bn_ar", 0, 11)), ("US CPI inflation, first-year average, pp", avg_diff("inflation_yoy", 0, 11)),
                                                           ("US imports, month 12, %", pct("imports_bn_ar", 11)), ("US real GDP, month 12, %", pct("gdp_real_bn_ar", 11))])
    print()
    print("R3: blocs re-centred (world_ius_offset +1.07, world_yus_offset +1.2), cell UC, against the shipped centring, cell UR")
    show("  China 25 points, they hit back", "UC", "UR", "tarCN25r", cn)
    A, B = load("UC", "none"), load("UR", "none")
    if A and B:
        print("  baseline output gaps, mean of months 1-24 (UC minus UR, same seeds), pts:")
        for j, nm in enumerate(an.BL):
            d, s_, n = diff_of(A, B, lambda r, j=j: mean([get(r, "b", "w_y_gap", m, j) for m in range(0, 24)]))
            print(f"     {nm:6s} {d:+6.2f} ({s_:.2f})")
    print()
    o2("backcast", "end-state duties of 24 Aug 2026 against 1 Jan 2025")
    print()
    o2("backcast_avg", "2025 time-average duties (data/backcast_k.json) against 1 Jan 2025")
    print()
    print("W5-06: tariff form identity on Today's start")
    w506("UR", "tarU10", "uimp10", range(1, 65))
    w506("UR", "tarU25", "uimp25", range(1, 65))
    print()
    print("R4: the UK's response (kappa 0.2, lag 2 months): cell TY with the binary that has it (T0_*) against the binary without it (T0_*_noUK), same seeds")
    for scen in ("tarU25r", "tarwar"):
        a, b = load("T0", scen), load("T0", scen + "_noUK")
        if not a or not b:
            print(f"  {scen}: n/a")
            continue
        rows = [("US goods exports to the UK, month 12, %", pct("w_exports_goods_bn", 11, 3)),
                ("US exports (all), month 12, %", pct("exports_bn_ar", 11)),
                ("US real GDP, first-year average, %", avg_pct("gdp_real_bn_ar", 0, 11)),
                ("US real GDP, month 12, %", pct("gdp_real_bn_ar", 11)),
                ("US CPI inflation, first-year average, pp", avg_diff("inflation_yoy", 0, 11)),
                ("UK tariff on US goods (ret_tau), month 12", lambda r: get(r, "s", "w_ret_tau", 11, 3)),
                ("UK output gap, mean months 1-24, pts", avg_diff("w_y_gap", 0, 23, 3))]
        print(f"  scenario {scen}")
        sa = {s: v for s, v in a.items() if s in b}
        sb = {s: v for s, v in b.items() if s in sa}
        for name, stat in rows:
            va = [stat(sa[s]) for s in sorted(sa)]; vb = [stat(sb[s]) for s in sorted(sa)]
            d = [x - y for x, y in zip(va, vb)]
            print(f"     {name:46s} with {mean(va):+8.3f}  without {mean(vb):+8.3f}  difference {mean(d):+8.3f} ({se(d):.3f})  n={len(d)}")


if __name__ == "__main__":
    main()
