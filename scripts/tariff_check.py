#!/usr/bin/env python3
"""b7-tariff paired tariff measurement (docs/CALIBRATION.md, "b7-tariff: tariff pass-through").

One /api/compare request per seed (common random numbers within a seed). The config overlay applies to both arms,
so `{"tariff_pass_through": false}` measures the legacy path and `{"tariff_retail_speed": 1.0}` measures a
no-absorption channel decomposition cell. Archive format matches an older measurement script: {pct: {seed: {"b": rows, "s": rows}}}.

usage:
  tariff_check.py OUT.json --pct 10 [--pct 25] [--seeds 1-12] [--months 60] [--hh 10000]
                  [--ports 8811,8812,8813,8814,8815,8816] [--config '{}'] [--summary OUT.txt]
                  [--compare-archive ARCHIVE.json]
  tariff_check.py OUT.json --analyse-only --pct 10 --pct 25 [--summary OUT.txt]

Prints mean and seed-SE of shocked-minus-baseline at m1, m3, m6, m12, m24, m36, m60; the per-seed share elasticities
eps_share = -ln(sh_s/sh_b)/ln(1+r_s) and eps_vol = -(ln(sh_s/sh_b) - ln(1+r_s))/ln(1+tau_s); the revenue share of
baseline nominal GDP; and a PASS/FAIL table for T1-T11 that flags any row within 1 SE of a bound.
Exit status: 0 on success; 1 if --compare-archive finds any difference.
"""
import argparse
import concurrent.futures as cf
import json
import math
import sys
import time
import urllib.request

FIELDS = [
    "inflation_yoy", "core_inflation_yoy", "core_pce_inflation_yoy", "cpi", "core_pce_level",
    "gdp_real_bn_ar", "gdp_nominal_bn_ar", "consumption_real_bn_ar", "consumption_nominal_bn_ar",
    "imports_bn_ar", "exports_bn_ar", "gov_revenue_bn_ar", "corp_profits_bn_ar", "firm_profits_by_sector_bn_ar",
    "dollar_index", "real_dollar_index", "unemployment_rate", "policy_rate", "yields", "firms_alive",
    "bankruptcies_ar", "firm_entries_ar", "investment_real_bn_ar", "deficit_gdp", "sector_prices",
    "consumption_by_sector_real", "gov_debt_gdp", "profit_share", "real_wage_index", "employment_millions",
    # b7-tariff outputs
    "tariff_rate_pct", "tariff_retail_pct", "tariff_revenue_bn_ar", "tariff_absorbed_bn_ar", "import_price_index",
    "hh_import_share_pct", "firm_input_import_share_pct", "sfc_error",
]
HORIZONS = [1, 3, 6, 12, 24, 36, 60]


def post(port, body):
    req = urllib.request.Request(f"http://localhost:{port}/api/compare", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=7200) as r:
        return json.loads(r.read())


def slim(rows):
    return [{k: m.get(k) for k in FIELDS} for m in rows]


def parse_seeds(text):
    lo, hi = map(int, text.split("-"))
    return list(range(lo, hi + 1))


def run_measurement(args):
    overlay = json.loads(args.config)
    ports = args.ports.split(",")
    seeds = parse_seeds(args.seeds)
    jobs = [(p, s) for p in args.pct for s in seeds]
    # One worker per server port, jobs dealt round-robin, so each server runs one request at a time.
    per_port = {i: [] for i in range(len(ports))}
    for k, job in enumerate(jobs):
        per_port[k % len(ports)].append(job)
    out = {}
    t0 = time.time()

    def worker(i):
        res = []
        for pct, seed in per_port[i]:
            body = {
                # J4: pin the warm-up (the server default is 24 since joint calibration 4); --config may override it.
                "config": {"n_households": args.hh, "warmup_months": 0, **overlay},
                "shocks": [{"at_month": 0, "shock": {"type": "ImportTariff", "pct": pct}}],
                "months": args.months,
                "seeds": [seed],
            }
            c = post(ports[i], body)
            res.append((pct, seed, {"b": slim(c["baseline"]), "s": slim(c["shocked"])}))
            print(f"done pct={pct} seed={seed} port={ports[i]} {time.time() - t0:.0f}s", flush=True)
        return res

    with cf.ThreadPoolExecutor(len(ports)) as ex:
        for res in ex.map(worker, range(len(ports))):
            for pct, seed, rows in res:
                out.setdefault(str(float(pct)), {})[str(seed)] = rows
    with open(args.out, "w") as fh:
        json.dump(out, fh)
    return out


def load_data(path):
    with open(path) as fh:
        return json.load(fh)


# ---- statistics -------------------------------------------------------------------------------------------------

def finite(x):
    return x is not None and isinstance(x, (int, float)) and math.isfinite(x)


def mean_se(xs):
    xs = [x for x in xs if finite(x)]
    n = len(xs)
    if n == 0:
        return float("nan"), float("nan"), 0
    m = sum(xs) / n
    if n == 1:
        return m, 0.0, 1
    var = sum((x - m) ** 2 for x in xs) / (n - 1)
    return m, math.sqrt(var / n), n


def seed_diff(b, s, key, mode, h):
    """Per-seed shocked-minus-baseline at horizon h (1-based month)."""
    bv, sv = b[h - 1][key], s[h - 1][key]
    if not (finite(bv) and finite(sv)):
        return float("nan")
    if mode == "pct":
        return 100.0 * (sv / bv - 1.0) if bv else float("nan")
    if mode == "bp":
        return 100.0 * (sv - bv)
    if mode == "lvl":
        return sv
    return sv - bv  # pp / level


def share_elasticities(b, s, h):
    """Per-seed eps_share and eps_vol at horizon h, from the household import share and the wedge and tariff."""
    sh_b, sh_s = b[h - 1]["hh_import_share_pct"], s[h - 1]["hh_import_share_pct"]
    r = s[h - 1]["tariff_retail_pct"] / 100.0
    tau = s[h - 1]["tariff_rate_pct"] / 100.0
    if not (finite(sh_b) and finite(sh_s) and sh_b > 0 and sh_s > 0 and r > 0 and tau > 0):
        return float("nan"), float("nan")
    ln_ratio = math.log(sh_s / sh_b)
    eps_share = -ln_ratio / math.log(1.0 + r)
    eps_vol = -(ln_ratio - math.log(1.0 + r)) / math.log(1.0 + tau)
    return eps_share, eps_vol


def gather(data, pct, fn):
    """Per-seed values of fn(baseline rows, shocked rows); NaN where a horizon is beyond the run's months."""
    out = []
    for v in data[pct].values():
        try:
            out.append(fn(v["b"], v["s"]))
        except IndexError:
            out.append(float("nan"))
    return out


ROWS = [
    ("CPI yoy pp", "inflation_yoy", "pp"),
    ("core PCE yoy pp", "core_pce_inflation_yoy", "pp"),
    ("CPI level %", "cpi", "pct"),
    ("real GDP %", "gdp_real_bn_ar", "pct"),
    ("real C %", "consumption_real_bn_ar", "pct"),
    ("nominal C %", "consumption_nominal_bn_ar", "pct"),
    ("nominal GDP %", "gdp_nominal_bn_ar", "pct"),
    ("imports %", "imports_bn_ar", "pct"),
    ("tariff revenue $bn", "tariff_revenue_bn_ar", "lvl"),
    ("policy bp", "policy_rate", "bp"),
    ("dollar %", "dollar_index", "pct"),
    ("firms alive %", "firms_alive", "pct"),
    ("corp profits %", "corp_profits_bn_ar", "pct"),
    ("hh import share %", "hh_import_share_pct", "pct"),
    ("firm input imp sh %", "firm_input_import_share_pct", "pct"),
    ("import price index %", "import_price_index", "pct"),
]


def horizon_table(data, pct, out):
    seeds = data[pct]
    n = len(seeds)
    T = min(len(v["b"]) for v in seeds.values())
    hs = [h for h in HORIZONS if h <= T]
    out.append(f"\n=== ImportTariff {pct}% ({n} seeds, {T} months): shocked minus baseline, mean (SE)")
    out.append(f"{'':22}" + "".join(f"{'m' + str(h):>16}" for h in hs))
    for name, key, mode in ROWS:
        line = f"{name:22}"
        for h in hs:
            m, se, _ = mean_se(gather(data, pct, lambda b, s: seed_diff(b, s, key, mode, h)))
            line += f"{m:>9.3f} ({se:4.2f})"
        out.append(line)
    if key_exists(data, pct):
        for label, idx in (("eps_share", 0), ("eps_vol", 1)):
            line = f"{label:22}"
            for h in hs:
                m, se, _ = mean_se(gather(data, pct, lambda b, s: share_elasticities(b, s, h)[idx]))
                line += f"{m:>9.3f} ({se:4.2f})"
            out.append(line)
        line = f"{'revenue % GDP':22}"
        for h in hs:
            m, se, _ = mean_se(gather(data, pct, lambda b, s: 100.0 * s[h - 1]["tariff_revenue_bn_ar"] / b[h - 1]["gdp_nominal_bn_ar"]))
            line += f"{m:>9.3f} ({se:4.2f})"
        out.append(line)


def key_exists(data, pct):
    return any("tariff_revenue_bn_ar" in v["s"][0] and v["s"][0]["tariff_revenue_bn_ar"] is not None
               for v in data[pct].values())


# ---- gates ------------------------------------------------------------------------------------------------------

class Gate:
    def __init__(self, gid, text, value, se, lo, hi):
        self.gid, self.text, self.value, self.se, self.lo, self.hi = gid, text, value, se, lo, hi

    def verdict(self):
        if not finite(self.value):
            return "n/a"
        ok = (self.lo is None or self.value >= self.lo) and (self.hi is None or self.value <= self.hi)
        near = False
        if finite(self.se) and self.se > 0:
            for bound in (self.lo, self.hi):
                if bound is not None and abs(self.value - bound) <= self.se:
                    near = True
        return ("PASS" if ok else "FAIL") + (" (near bound)" if near else "")


def gates(data):
    g = []
    has = {p: p in data for p in ("10.0", "25.0")}
    if has["10.0"]:
        d = "10.0"
        m3, _, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "inflation_yoy", "pp", 3)))
        m12, se12, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "inflation_yoy", "pp", 12)))
        m36, se36, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "inflation_yoy", "pp", 36)))
        lvl60, sel60, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "cpi", "pct", 60)))
        g.append(Gate("T1", "CPI yoy diff m12 (10%) in [0.50, 1.00] pp", m12, se12, 0.50, 1.00))
        g.append(Gate("T1b", "CPI yoy m3 < m12 (10%): m12 - m3", m12 - m3, se12, 1e-12, None))
        g.append(Gate("T2a", "CPI yoy m36 (10%) <= 0.30 pp", m36, se36, None, 0.30))
        g.append(Gate("T2b", "CPI level m60 (10%) <= 2.0 %", lvl60, sel60, None, 2.0))
        m12_real_c, se_real_c, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "consumption_real_bn_ar", "pct", 12)))
        g.append(Gate("T7a", "real C m12 (10%) <= -0.2 %", m12_real_c, se_real_c, None, -0.2))
        nc1, nse1, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "consumption_nominal_bn_ar", "pct", 1)))
        g.append(Gate("T7b", "|nominal C m1| (10%) <= 0.2 %", abs(nc1), nse1, None, 0.2))
        gr1, gse1, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "gdp_real_bn_ar", "pct", 1)))
        g.append(Gate("T6a", "|real GDP m1| (10%) <= 0.15 %", abs(gr1), gse1, None, 0.15))
        gr12, gse12, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "gdp_real_bn_ar", "pct", 12)))
        g.append(Gate("T6b", "real GDP m12 (10%) in [-1.25, +0.05] %", gr12, gse12, -1.25, 0.05))
        gmid, gmse, _ = mean_se(gather(data, d, lambda b, s: sum(seed_diff(b, s, "gdp_real_bn_ar", "pct", h) for h in range(6, 25)) / 19.0))
        g.append(Gate("T6c", "mean real GDP m6-m24 (10%) in [-1.25, +0.05] %", gmid, gmse, -1.25, 0.05))
        dm, dse, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "dollar_index", "pct", 12)))
        g.append(Gate("T8", "dollar m12 (10%) in [-1.5, +3.0] % (sign reported)", dm, dse, -1.5, 3.0))
        fa, fase, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "firms_alive", "pct", 12)))
        g.append(Gate("T9a", "firms alive m12 (10%) >= -0.75 %", fa, fase, -0.75, None))
        for h in (6, 12):
            cp, cpse, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "corp_profits_bn_ar", "pct", h)))
            g.append(Gate(f"T9b{h}", f"corp profits m{h} (10%) >= -10 %", cp, cpse, -10.0, None))
        rv, rvse, _ = mean_se(gather(data, d, lambda b, s: 100.0 * s[11]["tariff_revenue_bn_ar"] / b[11]["gdp_nominal_bn_ar"]))
        g.append(Gate("T5a", "revenue m12 / nominal GDP (10%) in [0.5, 1.1] %", rv, rvse, 0.5, 1.1))
        es12, ses12, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 12)[0]))
        es60, ses60, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 60)[0]))
        ev12, sev12, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 12)[1]))
        ev60, sev60, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 60)[1]))
        g.append(Gate("T4a", "eps_share m12 (10%) in [0.30, 0.60]", es12, ses12, 0.30, 0.60))
        g.append(Gate("T4b", "eps_share m60 (10%) >= 0.85", es60, ses60, 0.85, None))
        g.append(Gate("T4c", "eps_vol m12 (10%) in [0.5, 1.3]", ev12, sev12, 0.5, 1.3))
        g.append(Gate("T4d", "eps_vol m60 (10%) >= 1.5", ev60, sev60, 1.5, None))
        fi, fise, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "firm_input_import_share_pct", "pct", 12)))
        g.append(Gate("T4e", "firm input import share m12 (10%) below baseline", fi, fise, None, -1e-12))
    if has["25.0"]:
        d = "25.0"
        c12, cse12, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "inflation_yoy", "pp", 12)))
        g.append(Gate("T3a", "CPI yoy diff m12 (25%) in [1.25, 2.50] pp", c12, cse12, 1.25, 2.50))
        if has["10.0"]:
            c10, _, _ = mean_se(gather(data, "10.0", lambda b, s: seed_diff(b, s, "inflation_yoy", "pp", 12)))
            ratio = c12 / c10 if c10 else float("nan")
            g.append(Gate("T3b", "CPI m12 ratio 25%/10% in [2.0, 3.0]", ratio, float("nan"), 2.0, 3.0))
        es12, ses12, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 12)[0]))
        es60, ses60, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 60)[0]))
        ev12, sev12, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 12)[1]))
        ev60, sev60, _ = mean_se(gather(data, d, lambda b, s: share_elasticities(b, s, 60)[1]))
        g.append(Gate("T4f", "eps_share m12 (25%) in [0.30, 0.60]", es12, ses12, 0.30, 0.60))
        g.append(Gate("T4g", "eps_share m60 (25%) >= 0.85", es60, ses60, 0.85, None))
        g.append(Gate("T4h", "eps_vol m12 (25%) in [0.5, 1.3]", ev12, sev12, 0.5, 1.3))
        g.append(Gate("T4i", "eps_vol m60 (25%) >= 1.5", ev60, sev60, 1.5, None))
        rv, rvse, _ = mean_se(gather(data, d, lambda b, s: 100.0 * s[11]["tariff_revenue_bn_ar"] / b[11]["gdp_nominal_bn_ar"]))
        g.append(Gate("T5b", "revenue m12 / nominal GDP (25%) in [1.1, 2.6] %", rv, rvse, 1.1, 2.6))
        gr12, gse12, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "gdp_real_bn_ar", "pct", 12)))
        g.append(Gate("T6d", "real GDP m12 (25%) in [-3.0, 0.0] %", gr12, gse12, -3.0, 0.0))
        gmid, gmse, _ = mean_se(gather(data, d, lambda b, s: sum(seed_diff(b, s, "gdp_real_bn_ar", "pct", h) for h in range(6, 25)) / 19.0))
        g.append(Gate("T6e", "mean real GDP m6-m24 (25%) in [-3.0, 0.0] %", gmid, gmse, -3.0, 0.0))
        fa, fase, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "firms_alive", "pct", 12)))
        g.append(Gate("T9c", "firms alive m12 (25%) >= -2.0 %", fa, fase, -2.0, None))
        for h in (6, 12):
            cp, cpse, _ = mean_se(gather(data, d, lambda b, s: seed_diff(b, s, "corp_profits_bn_ar", "pct", h)))
            g.append(Gate(f"T9d{h}", f"corp profits m{h} (25%) >= -25 %", cp, cpse, -25.0, None))
    return g


def sfc_and_finite(data):
    bad_sfc = 0
    nonfinite = 0
    for pct, seeds in data.items():
        for v in seeds.values():
            for arm in ("b", "s"):
                for row in v[arm]:
                    if row.get("sfc_error") not in (0, 0.0):
                        bad_sfc += 1
                    for k in FIELDS:
                        x = row.get(k)
                        # A missing value (None) counts as non-finite too: the gates read these fields.
                        if x is None or (isinstance(x, float) and not math.isfinite(x)):
                            nonfinite += 1
    return bad_sfc, nonfinite


def compare_archive(data, path):
    arch = load_data(path)
    mismatches = 0
    checked = 0
    for pct, seeds in arch.items():
        if pct not in data:
            print(f"archive pct {pct} missing from run")
            mismatches += 1
            continue
        for seed, v in seeds.items():
            if seed not in data[pct]:
                print(f"archive seed {seed} (pct {pct}) missing from run")
                mismatches += 1
                continue
            for arm in ("b", "s"):
                ra, rn = v[arm], data[pct][seed][arm]
                if len(ra) != len(rn):
                    print(f"length differs pct {pct} seed {seed} {arm}: {len(ra)} vs {len(rn)}")
                    mismatches += 1
                    continue
                for mo, (x, y) in enumerate(zip(ra, rn)):
                    for k in x:
                        checked += 1
                        if x[k] != y.get(k):
                            mismatches += 1
                            if mismatches <= 10:
                                print(f"mismatch pct {pct} seed {seed} {arm} month {mo + 1} {k}: {x[k]} vs {y.get(k)}")
    print(f"compare vs {path}: {checked} values checked, {mismatches} mismatches")
    return mismatches


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--pct", type=float, action="append", required=True)
    ap.add_argument("--seeds", default="1-12")
    ap.add_argument("--months", type=int, default=60)
    ap.add_argument("--hh", type=int, default=10000)
    ap.add_argument("--ports", default="8811,8812,8813,8814,8815,8816")
    ap.add_argument("--config", default="{}", help="JSON config overlay applied to both arms")
    ap.add_argument("--summary", help="also write the printed summary to this file")
    ap.add_argument("--compare-archive", help="check every archived field, seed and month for exact equality")
    ap.add_argument("--analyse-only", action="store_true", help="skip the runs; read OUT.json")
    args = ap.parse_args()

    if args.analyse_only:
        data = load_data(args.out)
    else:
        data = run_measurement(args)

    lines = []
    for pct in sorted(data, key=float):
        horizon_table(data, pct, lines)
    bad_sfc, nonfinite = sfc_and_finite(data)
    lines.append(f"\nT10: sfc_error months nonzero = {bad_sfc}; non-finite monthly values = {nonfinite}")
    lines.append("\n=== Acceptance gates (12-seed means; SE in brackets is not used for the verdict)")
    for g in gates(data):
        m = f"{g.value:.3f}" if finite(g.value) else "nan"
        lines.append(f"{g.gid:6} {g.text:58} value {m:>9}  SE {g.se:.3f}  -> {g.verdict()}"
                     if finite(g.se) else f"{g.gid:6} {g.text:58} value {m:>9}  -> {g.verdict()}")
    lines.append("T11: I1-I4 are checked outside this script (identity proofs; I4 via --compare-archive).")
    text = "\n".join(lines)
    print(text)
    if args.summary:
        with open(args.summary, "w") as fh:
            fh.write(text + "\n")
    rc = 0
    if args.compare_archive:
        rc = 1 if compare_archive(data, args.compare_archive) else 0
    return rc


if __name__ == "__main__":
    sys.exit(main())
