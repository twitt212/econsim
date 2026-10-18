#!/usr/bin/env python3
"""build_usnow_ref.py [--repo PATH] [--vintage 2026-08-24] [--out PATH]

Builds the dated reference table of the Today's-start trade block (docs/CALIBRATION.md, section world-usnow) from the recorded input extracts in scripts/world/usnow_inputs/
and the model's own 2024 trade matrices (crates/econsim-core/src/world/data/world_params.json of --repo). Python 3 standard library only.
Run it with `python3 -I`. The output is written to --out (default usnow_ref.json next to this script); copy it to
crates/econsim-core/src/world/data/usnow_ref.json.

What it computes (all rates are decimal in the output):
  duty[j][s]        US duty on goods group s from bloc j: Yale Budget Lab statutory rates by trading-partner group and by product category on the vintage
                    date, combined at the resolution of the published data by iterative proportional fitting, weighted by today's import matrix.
  omega[j][s]       bloc j's share of US imports of goods group s: the 2025 Census-basis bloc totals, spread over the goods groups by the model's 2024
                    matrix (fitted to the 2024 group totals); each column sums to one.
  export_dest[j][s] the same for US exports by destination bloc.
  ret[j]            the duty bloc j puts on US goods (total, goods-weighted): retaliation_reference.csv.
Goods groups: 0 Energy, 1 Food, 2 Durables (cars, machinery, appliances), 3 Nondurables (clothes, household goods, chemicals), 4 Tech (electronics).
Blocs: EA CN JP UK CA MX IN KR BR AU OPEC+ RoW.
"""
import argparse, csv, json, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
BL = ["EA", "CN", "JP", "UK", "CA", "MX", "IN", "KR", "BR", "AU", "OPEC+", "RoW"]
S = 5
# bloc -> Yale trading-partner group (the eight groups of the published data). The RoW bloc is split between the free-trade-partner group and the
# rest-of-world group by FTA_SHARE_ROW (an estimate: Singapore, Israel, CAFTA-DR, Chile, Colombia, Peru and a few more are about 13% of its imports).
GROUPS = {"CN": "China", "CA": "Canada", "MX": "Mexico", "EA": "EU", "JP": "Japan", "UK": "UK", "KR": "Free-trade partners", "AU": "Free-trade partners",
          "IN": "Rest of World", "BR": "Rest of World", "OPEC+": "Rest of World"}
GNAMES = ["China", "Canada", "Mexico", "EU", "Japan", "UK", "Free-trade partners", "Rest of World"]
FTA_SHARE_ROW = 0.13


def read_rates(name, date):
    out = {}
    with open(os.path.join(HERE, "usnow_inputs", name)) as f:
        for r in csv.DictReader(f):
            out[r[list(r.keys())[0]]] = float(r[date])
    return out


def ipf(seed, rows, cols, it=500):
    m = [r[:] for r in seed]
    sr, sc = sum(rows), sum(cols)
    cols = [c * sr / sc for c in cols]
    for _ in range(it):
        for j in range(len(m)):
            t = sum(m[j]); f = rows[j] / t if t > 0 else 0.0
            m[j] = [x * f for x in m[j]]
        for s in range(len(cols)):
            t = sum(m[j][s] for j in range(len(m))); f = cols[s] / t if t > 0 else 0.0
            for j in range(len(m)):
                m[j][s] *= f
    return m


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=os.path.abspath(os.path.join(HERE, "..", "..")))
    ap.add_argument("--vintage", default="2026-08-24")
    ap.add_argument("--out", default=os.path.join(HERE, "usnow_ref.json"))
    ap.add_argument("--shares-year", default="2025", help="calendar year of the Census bloc totals (2024 for the early-2025 back-cast vintage)")
    ap.add_argument("--retaliation", default="retaliation_reference.csv", help="file in inputs/ with the partners' duty on US goods")
    ap.add_argument("--scale", type=float, default=1.0, help="multiply every duty (sensitivity vintages only)")
    ap.add_argument("--rule", default="ipf", choices=["ipf", "product"], help="ipf (the design): the two-margin fit; product (VALIDATION R2): the separable rule partner mean x goods mean / overall mean, no fit")
    a = ap.parse_args()
    wp = json.load(open(os.path.join(a.repo, "crates/econsim-core/src/world/data/world_params.json")))["trade"]
    imp24 = [[wp["imports_base"][j][s] for s in range(S)] for j in range(12)]
    exp24 = [[wp["exports_base"][j][s] for s in range(S)] for j in range(12)]
    partner = read_rates("yale_partner_rates.csv", a.vintage)
    product = read_rates("yale_product_rates.csv", a.vintage)
    # 1. sector rates: Yale product categories mapped to the five goods groups, weighted by approximate 2025 import values (category_map.csv)
    cats = list(csv.DictReader(open(os.path.join(HERE, "usnow_inputs", "category_map.csv"))))
    product["TECH_TELECOM_TV"] = product["Electronics & electrical equipment"]
    sec_rate = []
    for s in range(S):
        num = sum(float(c["weight_usd_bn_2025"]) * product[c["yale_category"]] for c in cats if int(c["model_sector"]) == s)
        den = sum(float(c["weight_usd_bn_2025"]) for c in cats if int(c["model_sector"]) == s)
        sec_rate.append(num / den)
    # 2. today's trade matrices: 2025 bloc totals (Census basis) spread over the goods groups by the model's 2024 matrices
    tb = {}
    for r in csv.DictReader(open(os.path.join(HERE, "usnow_inputs", "census_bloc_trade.csv"))):
        if r["year"] == a.shares_year:
            tb[r["bloc"]] = (float(r["goods_imports_usd_bn"]), float(r["goods_exports_usd_bn"]))
    row_m = [tb[b][0] for b in BL]; row_x = [tb[b][1] for b in BL]
    col_m = [sum(imp24[j][s] for j in range(12)) for s in range(S)]
    col_x = [sum(exp24[j][s] for j in range(12)) for s in range(S)]
    V = ipf(imp24, row_m, col_m); X = ipf(exp24, row_x, col_x)
    # 3. duty matrix: IPF on rates at the resolution of the published data (eight partner groups x five goods groups), weights V
    Vg = {g: [0.0] * S for g in GNAMES}
    for j, b in enumerate(BL):
        for s in range(S):
            if b == "RoW":
                Vg["Free-trade partners"][s] += FTA_SHARE_ROW * V[j][s]
                Vg["Rest of World"][s] += (1 - FTA_SHARE_ROW) * V[j][s]
            else:
                Vg[GROUPS[b]][s] += V[j][s]
    gr = [partner[g] for g in GNAMES]
    Vgj = [sum(Vg[g]) for g in GNAMES]; Vs = [sum(Vg[g][s] for g in GNAMES) for s in range(S)]
    mean_p = sum(Vgj[k] * gr[k] for k in range(8)) / sum(Vgj)
    c = mean_p / (sum(Vs[s] * sec_rate[s] for s in range(S)) / sum(Vs))
    sr = [x * c for x in sec_rate]
    G = [[gr[k] * sr[s] / mean_p for s in range(S)] for k in range(8)]
    for _ in range(3000 if a.rule == "ipf" else 0):
        for k in range(8):
            m = sum(Vg[GNAMES[k]][s] * G[k][s] for s in range(S)) / Vgj[k]; f = gr[k] / m
            G[k] = [x * f for x in G[k]]
        for s in range(S):
            m = sum(Vg[GNAMES[k]][s] * G[k][s] for k in range(8)) / Vs[s]; f = sr[s] / m
            for k in range(8):
                G[k][s] *= f
    gi = {g: k for k, g in enumerate(GNAMES)}
    T = []
    for j, b in enumerate(BL):
        if b == "RoW":
            T.append([FTA_SHARE_ROW * G[gi["Free-trade partners"]][s] + (1 - FTA_SHARE_ROW) * G[gi["Rest of World"]][s] for s in range(S)])
        else:
            T.append(G[gi[GROUPS[b]]][:])
    omega = [[V[j][s] / sum(V[k][s] for k in range(12)) for s in range(S)] for j in range(12)]
    xdest = [[X[j][s] / sum(X[k][s] for k in range(12)) for s in range(S)] for j in range(12)]
    ret = {r["bloc"]: float(r["duty_on_us_goods"]) for r in csv.DictReader(open(os.path.join(HERE, "usnow_inputs", a.retaliation)))}
    doc = {
        "meta": {"note": "Generated by build_usnow_ref.py from scripts/world/usnow_inputs/. Do not edit by hand.", "blocs": BL,
                 "goods_groups": ["Energy", "Food", "Durables", "Nondurables", "Tech"], "rates": "decimal",
                 "sector_rates_pct_after_fit": [round(x, 3) for x in sr]},
        "vintages": {a.vintage: {
            "as_of": a.vintage,
            "duty": [[round(a.scale * T[j][s] / 100, 8) for s in range(S)] for j in range(12)],
            "omega": [[round(omega[j][s], 10) for s in range(S)] for j in range(12)],
            "export_dest": [[round(xdest[j][s], 10) for s in range(S)] for j in range(12)],
            "ret": [ret[b] for b in BL]}},
        "latest": a.vintage,
    }
    with open(a.out, "w") as f:
        json.dump(doc, f, indent=1)
        f.write("\n")
    print("wrote", a.out)
    print("partner mean duty (%):", {b: round(sum(V[j][s] * T[j][s] for s in range(S)) / sum(V[j]), 2) for j, b in enumerate(BL)})


if __name__ == "__main__":
    main()
