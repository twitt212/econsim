#!/usr/bin/env python3
"""b7-spikes: decompose one-month real GDP spikes by component, from `econsim-cli run ... --json` records.

Usage:
  spike_decomp.py RUNDIR CELL [--seeds 1-32] [--thr 2.0] [--top 8] [--cf] [--json OUT]

RUNDIR/CELL_s<seed>.json.gz holds the last line of `econsim-cli run 20000 240 <seed> --json | tail -n 1 | gzip`.
Records must carry the b7-spikes diagnostic keys (cipi_by_sector_real_bn_ar, firm_shortfall_imports_bn_ar,
hh_unmet_imports_bn_ar, bk_max_heads_pct).

Components of the month-on-month change in real GDP (c_k = 100 * dK / Y_{t-1}):
  C, I, Res, G, CIPI_goods (sectors 0-4), CIPI_constr (sector 8), CIPI_ns (sectors 5-7, raw key), NX.
NX is the lumped net-export residual Y - C - I - Res - G - recorded CIPI (exports minus imports).
If the recorded CIPI differs from the raw sector sum over all sectors but matches the sum over non-nonstorable
sectors, the F1 fix is on and CIPI_ns contributes 0 to GDP.
--cf subtracts raw CIPI_ns from Y (the first-order F1 counterfactual) and sets the CIPI_ns component to 0.

Spike types (applied in this order; k* = largest same-sign contribution):
  B  k* = C and bk_max_heads_pct at t or t-1 >= 0.5           (large-firm liquidation)
  C  max_s |dln sector_price_s| >= 10% and k* in {I, Res, G, C}  (deflator jump)
  A  k* = CIPI_ns, or k* = NX and sign * c_ns <= -0.5         (unsold services booked as inventory)
  D  k* in {CIPI_goods, CIPI_constr}                          (storable inventory)
  E  k* = NX                                                  (imports / net exports)
  F  anything else
"""
import argparse
import gzip
import json
import math
import os
import sys
from collections import Counter

import numpy as np

COMPS = ["C", "I", "Res", "G", "CIPI_goods", "CIPI_constr", "CIPI_ns", "NX"]
NS_IDX = [5, 6, 7]
GOODS_IDX = [0, 1, 2, 3, 4]
CONSTR_IDX = [8]
TYPE_NAMES = {"A": "A unsold-services", "B": "B bankruptcy", "C": "C deflator-jump",
              "D": "D storable-inventory", "E": "E net-exports", "F": "F other"}


def parse_seeds(text):
    out = []
    for part in text.split(","):
        if "-" in part:
            a, b = part.split("-")
            out.extend(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def load_run(path):
    with gzip.open(path, "rt") as fh:
        return json.loads(fh.read().strip().splitlines()[-1])


def acf1(x):
    if len(x) < 3:
        return float("nan")
    a, b = x[:-1], x[1:]
    return float(np.corrcoef(a, b)[0, 1])


def seed_series(recs, cf):
    """Return the per-month arrays used by the decomposition for one seed."""
    g = lambda k: np.array([float(m[k]) for m in recs])
    Y = g("gdp_real_bn_ar")
    C, I = g("consumption_real_bn_ar"), g("investment_real_bn_ar")
    Res, G = g("real_residential_investment_bn_ar"), g("government_real_bn_ar")
    CI = g("inventory_change_real_bn_ar")
    cip = np.array([m["cipi_by_sector_real_bn_ar"] for m in recs], dtype=float)  # T x 9
    raw_all = cip.sum(axis=1)
    raw_nonns = cip[:, GOODS_IDX + CONSTR_IDX].sum(axis=1)
    tol = 1e-6 * np.maximum(1.0, np.abs(CI))
    f1_on = bool(np.all(np.abs(CI - raw_nonns) <= tol) and np.any(np.abs(CI - raw_all) > tol))
    ns = cip[:, NS_IDX].sum(axis=1)
    NX = Y - C - I - Res - G - CI
    cg = cip[:, GOODS_IDX].sum(axis=1)
    cc = cip[:, CONSTR_IDX].sum(axis=1)
    if f1_on:
        ns_con = np.zeros_like(ns)
    elif cf:
        Y = Y - ns
        CI = CI - ns
        ns_con = np.zeros_like(ns)
    else:
        ns_con = ns
    parts = dict(C=C, I=I, Res=Res, G=G, CIPI_goods=cg, CIPI_constr=cc, CIPI_ns=ns_con, NX=NX)
    sp = np.array([m["sector_prices"] for m in recs], dtype=float)
    dlp = np.abs(np.diff(np.log(sp), axis=0))  # (T-1) x 9
    u = g("unemployment_rate")
    bkp = g("bk_max_heads_pct")
    return dict(Y=Y, parts=parts, dlp=dlp, u=u, bkp=bkp, f1_on=f1_on,
                fs=g("firm_shortfall_imports_bn_ar"), hu=g("hh_unmet_imports_bn_ar"),
                M=g("imports_bn_ar"), GDPn=g("gdp_nominal_bn_ar"))


def classify(sd, t, thr_frac):
    """Return (type, con dict, dl) for month t (1..239), or None if below the threshold."""
    Y = sd["Y"]
    dl = math.log(Y[t]) - math.log(Y[t - 1])
    if abs(dl) < thr_frac:
        return None
    sgn = 1.0 if dl > 0 else -1.0
    con = {k: 100.0 * (v[t] - v[t - 1]) / Y[t - 1] for k, v in sd["parts"].items()}
    kstar = max(COMPS, key=lambda k: sgn * con[k])
    jump = 100.0 * float(np.max(sd["dlp"][t - 1]))
    bk = max(sd["bkp"][t], sd["bkp"][t - 1])
    if kstar == "C" and bk >= 0.5:
        typ = "B"
    elif jump >= 10.0 and kstar in ("I", "Res", "G", "C"):
        typ = "C"
    elif kstar == "CIPI_ns" or (kstar == "NX" and sgn * con["CIPI_ns"] <= -0.5):
        typ = "A"
    elif kstar in ("CIPI_goods", "CIPI_constr"):
        typ = "D"
    elif kstar == "NX":
        typ = "E"
    else:
        typ = "F"
    return typ, kstar, con, dl, jump


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("rundir")
    ap.add_argument("cell")
    ap.add_argument("--seeds", default="1-32")
    ap.add_argument("--thr", type=float, default=2.0)
    ap.add_argument("--top", type=int, default=8)
    ap.add_argument("--cf", action="store_true")
    ap.add_argument("--json", dest="json_out", default=None)
    a = ap.parse_args(argv)

    seeds_out, spikes, du_all, fs_all, hu_all, mm_all, mgdp_all = [], [], [], [], [], [], []
    sd_list, ac_list, ism_sd, ism_ac, imp_gdp = [], [], [], [], []
    bk1 = bk05 = 0
    f1_seeds = []
    used = 0
    for s in parse_seeds(a.seeds):
        path = os.path.join(a.rundir, f"{a.cell}_s{s}.json.gz")
        if not os.path.exists(path):
            continue
        recs = load_run(path)
        sd = seed_series(recs, a.cf)
        used += 1
        f1_seeds.append(sd["f1_on"])
        Y = sd["Y"]
        dl = np.diff(np.log(Y))
        seg = dl[11:239]  # months 12-239
        sd_list.append(float(np.std(seg, ddof=1)))
        ac_list.append(acf1(seg))
        dM = np.diff(np.log(sd["M"]))[11:239]
        ism_sd.append(float(np.std(dM, ddof=1)))
        ism_ac.append(acf1(dM))
        imp_gdp.append(float(np.mean(sd["M"][12:240] / sd["GDPn"][12:240])))
        du = np.diff(sd["u"])
        du_all.append(du)
        bk1 += int(np.sum(sd["bkp"][1:] >= 1.0))
        bk05 += int(np.sum(sd["bkp"][1:] >= 0.5))
        fs_all.append(np.diff(sd["fs"]))
        hu_all.append(np.diff(sd["hu"]))
        mm_all.append(np.diff(sd["M"]))
        for t in range(1, 240):
            # Every month is classified (threshold 0): counts at 2%, 2.5% and 3% never depend on --thr.
            typ, kstar, con, d, jump = classify(sd, t, 0.0)
            spikes.append(dict(seed=s, t=t, dln=100.0 * d, type=typ, kstar=kstar, jump=jump,
                               con={k: round(v, 3) for k, v in con.items()}))
        seeds_out.append(s)

    if used == 0:
        print(f"no runs found for {a.cell} in {a.rundir}", file=sys.stderr)
        return 1

    # --thr selects only the listing (top seed-months and the threshold type count); all counts use every month.
    listed = [x for x in spikes if abs(x["dln"]) >= a.thr]
    big = [x for x in spikes if abs(x["dln"]) >= 2.0]
    n2, n25 = len(big), sum(1 for x in spikes if abs(x["dln"]) >= 2.5)
    # Max of |dlnY| over all seed-months (not only spikes above thr).
    maxes = []
    for s in seeds_out:
        recs = load_run(os.path.join(a.rundir, f"{a.cell}_s{s}.json.gz"))
        Ys = np.array([float(m["gdp_real_bn_ar"]) for m in recs])
        if a.cf:
            ns = np.array([float(np.sum(m["cipi_by_sector_real_bn_ar"][5:8])) for m in recs])
            f1 = seed_series(recs, True)["f1_on"]
            Ys = Ys - (0.0 if f1 else ns)
        dls = 100.0 * np.abs(np.diff(np.log(Ys)))
        i = int(np.argmax(dls))
        maxes.append((float(dls[i]), s, i + 1))
    maxes.sort(reverse=True)
    mx = maxes[0]
    gt3 = sum(1 for x in spikes if abs(x["dln"]) > 3.0)  # all months, not only listed
    types = Counter(x["type"] for x in big)
    tcount = Counter(x["type"] for x in listed)
    du_cat = np.concatenate(du_all)
    fsv = np.concatenate(fs_all)
    huv = np.concatenate(hu_all)
    mmv = np.concatenate(mm_all)
    varM = float(np.var(mmv))
    share_fs = float(np.cov(fsv, mmv)[0, 1] / varM) if varM > 0 else float("nan")
    share_hu = float(np.cov(huv, mmv)[0, 1] / varM) if varM > 0 else float("nan")

    res = dict(
        cell=a.cell, cf=a.cf, seeds=len(seeds_out), f1_on_seeds=int(sum(f1_seeds)),
        sd_dlny_pct=100.0 * float(np.mean(sd_list)), ac1_dlny=float(np.mean(ac_list)),
        n_ge2=n2, n_ge25=n25, n_gt3=gt3, types_ge2=dict(types), thr=a.thr, n_listed=len(listed), types_thr=dict(tcount),
        max_pct=mx[0], max_seed=mx[1], max_month=mx[2],
        top=sorted(listed, key=lambda x: -abs(x["dln"]))[: a.top],
        max_du_pp=float(np.max(du_cat)), n_du_ge1=int(np.sum(du_cat >= 1.0)),
        imports_mom_sd_pct=100.0 * float(np.mean(ism_sd)), imports_mom_ac1=float(np.mean(ism_ac)),
        imports_gdp_pct=100.0 * float(np.mean(imp_gdp)),
        var_share_firm_shortfall=share_fs, var_share_hh_unmet=share_hu,
        bk_ge05=bk05, bk_ge1=bk1,
    )

    print(f"== {a.cell}{' (cf)' if a.cf else ''}: {len(seeds_out)} seeds, F1 on in {res['f1_on_seeds']}")
    print(f"  sd dlnY {res['sd_dlny_pct']:.3f}%  AC1 {res['ac1_dlny']:+.3f}  (per-seed mean, months 12-239)")
    print(f"  months |dlnY|>=2%: {n2}  >=2.5%: {n25}  >3%: {gt3}   types(>=2%): {dict(types)}")
    print(f"  listed (>=thr {a.thr}%): {len(listed)} months, types {dict(tcount)}")
    print(f"  max |dlnY|: {mx[0]:.2f}% (seed {mx[1]}, month {mx[2]})  mean of per-seed max: "
          f"{np.mean([m[0] for m in maxes]):.2f}%")
    print(f"  max one-month du: {res['max_du_pp']:+.2f} pp; months du>=1pp: {res['n_du_ge1']}")
    print(f"  imports mom sd {res['imports_mom_sd_pct']:.2f}%  AC1 {res['imports_mom_ac1']:+.3f}  "
          f"mean imports/GDP {res['imports_gdp_pct']:.2f}%")
    print(f"  variance share of dM: firm shortfall {share_fs:+.3f}, household unmet {share_hu:+.3f}")
    print(f"  bankruptcies with bk_max_heads_pct >=0.5: {bk05}; >=1: {bk1}")
    print(f"  top seed-months:")
    for x in res["top"]:
        comps = " ".join(f"{k}{v:+.2f}" for k, v in x["con"].items())
        print(f"    s{x['seed']:2d} m{x['t']:3d} {x['dln']:+6.2f}% {x['type']} k*={x['kstar']:<10s} "
              f"pjump {x['jump']:5.1f}% | {comps}")
    if a.json_out:
        with open(a.json_out, "w") as fh:
            json.dump(res, fh, indent=1, default=float)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
