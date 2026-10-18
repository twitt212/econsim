#!/usr/bin/env python3
"""Firm-distribution readout (b5-firmhet, S0): per-seed levels and paired T - B differences.

Runs are gzipped JSON histories written as RUNDIR/<cell>_s<seed>.json.gz by `econsim-cli run 20000 240 <seed> --json`
(the layout scripts/paired.py uses). Statistics use months 12..239 unless noted.

Usage:
  firm_check.py levels RUNDIR CELL [--seeds 1-16]
      Per-seed table and mean +- SE of every firm row below.
  firm_check.py pair RUNDIR_T CELL_T RUNDIR_B CELL_B [--seeds 1-8]
      Paired difference T - B per row: mean d, SE = sd(d)/sqrt(n), and the per-seed d.

Rows (monthly fields from metrics.rs, b5-firmhet block):
  markup_disp_avc   P90/P50 of 1 + markup over variable cost (facts_firms.py row)
  markup_disp_eff   P90/P50 of price / unit cost (existing field, markup_eff_p*)
  top1_share        employment share of the largest 1% of firms, %
  emp500            employment share of firms with 500+ staff, % (firm_share_500plus; reported, not a gate)
  lp_disp_9010      pooled within-sector 90-10 range of log labour productivity
  lp_sd_within      within-sector sd of log labour productivity
  op_gap            Olley-Pakes gap (log points)
  size_lp_slope     OLS slope of ln heads on relative ln productivity
  hhi_drift         mean over sectors of revenue HHI, months 24-83 vs 192-239 (run truncation: years 16-19)
  trap_count        seed-sector pairs whose January median ln(K/K*) is below -1 in any year (count per seed)
  sector_floor      share of seed-sector pairs whose firm count ever falls below 25% of the CALIBRATED count, %
  sector_floor_m0   same, against the month-0 count (secondary; cannot see the burn-in traps), %
  entry_rate, exit_rate   %/yr, annual flow over the mean alive firm count (proxy; facts_firms.py is authoritative)
  owner_own_share   owners working in their own firm, % of owner-operated firms (counts over months 12-239)
  cu_level          capacity utilisation, %
Gates in the branch rules are applied by the caller; this script prints the numbers only.
"""
import argparse
import gzip
import json
import math
import os
import sys
import warnings

import numpy as np

warnings.filterwarnings('ignore', category=RuntimeWarning)
SKIP = 12
TRAP_MEDIAN = -1.0
FLOOR_SHARE = 0.25
# Calibrated firm count per sector, as init.rs build_firms sizes it:
#   k_s = max(2, round(round(n / households_per_firm) * employment_share[s]))
# Defaults from config.rs (households_per_firm 25.0, employment_share table). n = 20000 households.
N_HOUSEHOLDS = 20000
HOUSEHOLDS_PER_FIRM = 25.0
EMPLOYMENT_SHARE = [0.020, 0.070, 0.090, 0.100, 0.040, 0.105, 0.200, 0.325, 0.050]


def load(rundir, cell, seed):
    path = os.path.join(rundir, f'{cell}_s{seed}.json.gz')
    return json.loads(gzip.open(path).read())


def parse_seeds(text):
    out = []
    for part in text.split(','):
        if '-' in part:
            a, b = part.split('-')
            out.extend(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def arr(recs, key, default=np.nan):
    return np.array([r.get(key, default) if r.get(key) is not None else default for r in recs], float)


def mat(recs, key, width=9):
    return np.array([r.get(key) if r.get(key) is not None else [np.nan] * width for r in recs], float)


def ratio_disp(recs, p90, p50):
    return float(np.nanmean((100 + arr(recs, p90)[SKIP:]) / (100 + arr(recs, p50)[SKIP:])))


def seed_rows(recs):
    """All per-seed scalars for one run."""
    hs = recs[SKIP:]
    out = {}
    out['markup_disp_avc'] = ratio_disp(recs, 'markup_avc_p90', 'markup_avc_p50')
    out['markup_disp_eff'] = ratio_disp(recs, 'markup_eff_p90', 'markup_eff_p50')
    out['top1_share'] = float(np.nanmean(arr(hs, 'firm_emp_top1_share')))
    out['emp500'] = float(np.nanmean(arr(hs, 'firm_share_500plus')))
    out['lp_disp_9010'] = float(np.nanmean(arr(hs, 'firm_lp_p90_p10')))
    out['lp_sd_within'] = float(np.nanmean(arr(hs, 'firm_lp_sd_within')))
    out['op_gap'] = float(np.nanmean(arr(hs, 'firm_op_gap_lp')))
    out['size_lp_slope'] = float(np.nanmean(arr(hs, 'firm_size_lp_slope')))
    hhi = mat(recs, 'firm_hhi_by_sector')
    out['hhi_drift'] = float(np.nanmean(np.nanmean(hhi[192:240], 0) - np.nanmean(hhi[24:84], 0)))
    # Trap count: January values of the median ln(K/K*) by sector, below -1 in any year.
    gap_med = mat(recs, 'capital_gap_median_by_sector')
    jan = list(range(SKIP, len(recs), 12))
    out['trap_count'] = float(np.sum(np.nanmin(gap_med[jan], 0) < TRAP_MEDIAN)) if jan else np.nan
    # Sector floor: share of seed-sector pairs whose firm count ever falls below 25% of the CALIBRATED
    # per-sector count (init.rs build_firms). The month-0 variant is secondary: month 0 is already after
    # the burn-in collapse, so it cannot see the worst traps.
    fbs = mat(recs, 'firms_by_sector')
    nf_target = round(N_HOUSEHOLDS / HOUSEHOLDS_PER_FIRM)
    calib = np.array([max(2, round(nf_target * sh)) for sh in EMPLOYMENT_SHARE], dtype=float)
    ok_c = calib > 0
    floor_c = np.nanmin(fbs, 0) < FLOOR_SHARE * calib
    out['sector_floor'] = float(100 * np.sum(floor_c & ok_c) / max(1, np.sum(ok_c)))
    base = fbs[0]
    ok = base > 0
    floor_m0 = np.nanmin(fbs, 0) < FLOOR_SHARE * base
    out['sector_floor_m0'] = float(100 * np.sum(floor_m0 & ok) / max(1, np.sum(ok)))
    alive = arr(hs, 'firms_alive')
    out['entry_rate'] = float(100 * np.nanmean(arr(hs, 'firm_entries_ar')) / np.nanmean(alive))
    out['exit_rate'] = float(100 * np.nanmean(arr(hs, 'bankruptcies_ar')) / np.nanmean(alive))
    own = np.array([[r.get('owners_in_own_firm', np.nan), r.get('owners_employed_elsewhere', np.nan),
                     r.get('owners_unemployed', np.nan), r.get('owners_not_in_lf', np.nan)] for r in hs], float)
    tot = np.nansum(own, 1)
    out['owner_own_share'] = float(100 * np.nansum(own[:, 0]) / np.nansum(tot)) if np.nansum(tot) > 0 else np.nan
    out['cu_level'] = float(np.nanmean(arr(hs, 'capacity_utilization')))
    return out


ROWS = ['markup_disp_avc', 'markup_disp_eff', 'top1_share', 'emp500', 'lp_disp_9010', 'lp_sd_within', 'op_gap',
        'size_lp_slope', 'hhi_drift', 'trap_count', 'sector_floor', 'sector_floor_m0', 'entry_rate', 'exit_rate', 'owner_own_share',
        'cu_level']


def se(x):
    x = np.asarray(x, float)
    x = x[np.isfinite(x)]
    if len(x) < 2:
        return float('nan')
    return float(np.std(x, ddof=1) / math.sqrt(len(x)))


def cmd_levels(args):
    seeds = parse_seeds(args.seeds)
    R = {s: seed_rows(load(args.rundir, args.cell, s)) for s in seeds}
    print(f"{'row':16s} " + ' '.join(f's{s:>2d}'.rjust(9) for s in seeds) + '   mean +- SE')
    for k in ROWS:
        vals = [R[s][k] for s in seeds]
        m = float(np.nanmean(vals))
        print(f"{k:16s} " + ' '.join(f'{v:9.3f}' for v in vals) + f'   {m:.3f} +- {se(vals):.3f}')


def cmd_pair(args):
    seeds = parse_seeds(args.seeds)
    T = {s: seed_rows(load(args.rundir_t, args.cell_t, s)) for s in seeds}
    B = {s: seed_rows(load(args.rundir_b, args.cell_b, s)) for s in seeds}
    print(f'paired T - B: {args.cell_t} vs {args.cell_b}, seeds {args.seeds}')
    print(f"{'row':16s} {'mean d':>10s} {'SE':>8s}   T mean     B mean")
    for k in ROWS:
        d = [T[s][k] - B[s][k] for s in seeds]
        print(f"{k:16s} {float(np.nanmean(d)):10.3f} {se(d):8.3f}   "
              f"{float(np.nanmean([T[s][k] for s in seeds])):9.3f} {float(np.nanmean([B[s][k] for s in seeds])):9.3f}")


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    lv = sub.add_parser('levels')
    lv.add_argument('rundir')
    lv.add_argument('cell')
    lv.add_argument('--seeds', default='1-16')
    pr = sub.add_parser('pair')
    pr.add_argument('rundir_t')
    pr.add_argument('cell_t')
    pr.add_argument('rundir_b')
    pr.add_argument('cell_b')
    pr.add_argument('--seeds', default='1-8')
    args = ap.parse_args(argv)
    if args.cmd == 'levels':
        cmd_levels(args)
    else:
        cmd_pair(args)


if __name__ == '__main__':
    main(sys.argv[1:])
