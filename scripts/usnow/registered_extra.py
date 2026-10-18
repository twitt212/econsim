#!/usr/bin/env python3
"""registered_extra.py: the rows of the registered validation note that usnow_accept.py does not compute: W5-04 US-China (tarCN25r), W5-04 machinery (R), W4-25 (R), O1b (R), O3 (R), R5 (R).
usage: SCR=... python3 -I registered_extra.py [LO-HI[,LO-HI]]   (seed blocks; default all seeds found)"""
import os, sys, math
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import an
from an import load, mean, se, get, BL

blocks = [tuple(map(int, b.split("-"))) for b in sys.argv[1].split(",")] if len(sys.argv) > 1 else None


def L(cell, scen):
    r = load(cell, scen)
    return {s: v for s, v in r.items() if blocks is None or any(lo <= s <= hi for lo, hi in blocks)}


def pct(r, key, m, j=None): return 100.0 * (get(r, "s", key, m, j) / get(r, "b", key, m, j) - 1.0)
def avgd(r, key, j=None, m0=0, m1=11): return mean([get(r, "s", key, m, j) - get(r, "b", key, m, j) for m in range(m0, m1 + 1)])
def avgp(r, key, j=None, m0=0, m1=11): return mean([pct(r, key, m, j) for m in range(m0, m1 + 1)])
def ms(x): return f"{mean(x):+.3f} ({se(x):.3f})"


def block(title, cell, scen):
    R = L(cell, scen)
    print(f"{title}: {cell} {scen}, {len(R)} seeds")
    return R


R = block("W5-04 US-China", "UR", "tarCN25r")
if R:
    imp = [pct(r, "w_imports_bn", 11, 1) for r in R.values()]
    ex = [math.log(get(r, "s", "w_exports_goods_bn", 11, 1) / get(r, "b", "w_exports_goods_bn", 11, 1)) / math.log((1 + get(r, "s", "w_ret_tau", 11, 1)) / (1 + get(r, "b", "w_ret_tau", 11, 1))) for r in R.values()]
    print(f"   imports from China m12 {ms(imp)} % in [-34.0, -16.6]; goods exports per ln retaliatory ratio {ms(ex)} in [-5, -2]")
    print(f"   GDP first year {ms([avgp(r, 'gdp_real_bn_ar') for r in R.values()])} %; CPI first year {ms([avgd(r, 'inflation_yoy') for r in R.values()])} pp; unemployment first year {ms([avgd(r, 'unemployment_rate') for r in R.values()])} pp (R); China output gap, mean months 1-24 {ms([avgd(r, 'w_y_gap', 1, 0, 23) for r in R.values()])} pts (R)")
R = block("W5-04 machinery (R)", "UR", "tarDT25")
if R:
    print(f"   GDP m12 {ms([pct(r, 'gdp_real_bn_ar', 11) for r in R.values()])} %; GDP first year {ms([avgp(r, 'gdp_real_bn_ar') for r in R.values()])} %; CPI first year {ms([avgd(r, 'inflation_yoy') for r in R.values()])} pp; dollar m12 {ms([pct(r, 'dollar_index', 11) for r in R.values()])} %")
for cell in ("UR", "T0"):
    R = block("W4-25 broad dollar after the tariff war (R, band abs <= 8%)", cell, "tarwar")
    if R:
        print(f"   dollar m12 {ms([pct(r, 'dollar_index', 11) for r in R.values()])} %, m24 {ms([pct(r, 'dollar_index', 23) for r in R.values()])} %")
print("O1b (R): peak GDP (minimum over months 1-36 of the paired GDP effect) and peak price level (maximum of the paired CPI level effect), 10 points; UR against TY, unpaired SE")
for scen in ("tarU10",):
    U, T = L("UR", scen), L("T0", scen)
    if U and T:
        for W in (24, 36):
            def peak_gdp(r, W=W): return min(pct(r, "gdp_real_bn_ar", m) for m in range(W))
            def peak_cpi(r, W=W): return max(pct(r, "cpi", m) for m in range(W))
            for nm, f, rng in (("peak GDP, %", peak_gdp, "[-0.63, -0.25]"), ("peak price level, %", peak_cpi, "[+0.25, +1.05]")):
                a, b = [f(r) for r in U.values()], [f(r) for r in T.values()]
                d = mean(a) - mean(b); s_ = math.sqrt(se(a) ** 2 + se(b) ** 2)
                print(f"   within {W} months, {nm}: UR {ms(a)} ({len(a)} seeds), TY {ms(b)} ({len(b)} seeds); difference {d:+.3f} (SE {s_:.3f}); research range {rng}")
print("O3 (R): UR tarU10 / tarU25 effects against the legacy card on U0 (leg10 / leg25) at the same size, first-year averages; unpaired SE; rule: within 2 SE or 0.1")
for sc_u, sc_l in (("tarU10", "leg10"), ("tarU25", "leg25")):
    U, G = L("UR", sc_u), L("U0", sc_l)
    if U and G:
        for nm, key, mode in (("inflation, pp", "inflation_yoy", "d"), ("GDP, %", "gdp_real_bn_ar", "p"), ("unemployment, pp", "unemployment_rate", "d"), ("policy rate, pp", "policy_rate", "d")):
            f = (lambda r, key=key: avgd(r, key)) if mode == "d" else (lambda r, key=key: avgp(r, key))
            a, b = [f(r) for r in U.values()], [f(r) for r in G.values()]
            d = mean(a) - mean(b); s_ = math.sqrt(se(a) ** 2 + se(b) ** 2)
            ok = abs(d) <= max(2 * s_, 0.1)
            print(f"   {sc_u} vs {sc_l} {nm:18s}: UR {ms(a)} ({len(a)}), U0 {ms(b)} ({len(G)}); difference {d:+.3f} (SE {s_:.3f}) {'inside' if ok else 'OUTSIDE'}")
print("R5 (R): 5,000 against 10,000 households (URH), first-set seeds")
for scen in ("tarU10", "tarCN25r", "tarwar"):
    A, B = L("UR", scen), L("URH", scen)
    common = sorted(set(A) & set(B))
    if common:
        for nm, f in (("GDP first year, %", lambda r: avgp(r, "gdp_real_bn_ar")), ("CPI first year, pp", lambda r: avgd(r, "inflation_yoy")), ("imports from China m12, %", lambda r: pct(r, "w_imports_bn", 11, 1))):
            a, b = [f(A[s]) for s in common], [f(B[s]) for s in common]
            d = [x - y for x, y in zip(a, b)]
            print(f"   {scen} {nm:28s}: 5,000 {ms(a)}  10,000 {ms(b)}  difference {mean(d):+.3f} ({se(d):.3f}) n={len(common)}")
