#!/usr/bin/env python3
"""Labour-market check (fix-labour): flow rates, unemployment by education, job-to-job moves, duration and UI,
participation and the wage lag, from the `labflows` diagnostic, against CPS / JOLTS / literature targets.

Usage:
  python3 scripts/labour_check.py --run [--bin PATH] [--households 20000] [--months 240] [--seeds 1 2 3 4] [--out DIR]
  python3 scripts/labour_check.py FILE.csv [FILE.csv ...]        # analyse existing labflows output
  add --json PATH to write the per-seed and averaged metrics as JSON.

The first 12 months of each run are dropped. Rates are per start-of-month stock. Each seed is analysed separately and
the table shows the mean across seeds with the spread (sd) in brackets.
"""
import argparse
import csv
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

import numpy as np

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
DROP = 12
WEEKS_PER_MONTH = 4.345


def hp(y, lam=14400.0):
    """Hodrick-Prescott cycle (monthly lambda)."""
    y = np.asarray(y, dtype=float)
    n = len(y)
    if n < 5:
        return np.zeros(n)
    D = np.zeros((n - 2, n))
    for i in range(n - 2):
        D[i, i:i + 3] = [1.0, -2.0, 1.0]
    trend = np.linalg.solve(np.eye(n) + lam * D.T @ D, y)
    return y - trend


def corr(a, b):
    a, b = np.asarray(a, float), np.asarray(b, float)
    if a.std() < 1e-12 or b.std() < 1e-12:
        return float('nan')
    return float(np.corrcoef(a, b)[0, 1])


def load(path):
    with open(path) as fh:
        rows = list(csv.DictReader(fh))
    cols = {}
    for k in rows[0].keys():
        cols[k] = np.array([float(r[k]) for r in rows])
    return {k: v[DROP:] for k, v in cols.items()}


def group(c, prefix, n):
    return np.stack([c[f'{prefix}{i}'] for i in range(n)], axis=1)


def analyse(c):
    """Return an ordered dict of metrics for one run (arrays already trimmed)."""
    m = {}
    emp_edu = group(c, 'emp_edu', 3)
    u_edu = group(c, 'u_edu', 3)
    hu_edu = group(c, 'hu_edu', 3)
    sep_edu = group(c, 'sep_edu', 3)
    E0 = emp_edu.sum(1)
    U0 = u_edu.sum(1)
    LF0 = E0 + U0
    # Unemployment by education uses post-step stocks (the headline definition); start-of-step stocks include the
    # month's entrants before they are matched and are used only for flow rates.
    pe = group(c, 'pe_edu', 3)
    pu = group(c, 'pu_edu', 3)
    u_agg_m = pu.sum(1) / (pe.sum(1) + pu.sum(1))
    u_agg = 100 * u_agg_m.mean()
    m['u_mean'] = u_agg
    m['u_sd_hp'] = float(hp(c['u_pct']).std())
    m['u_pct_mean'] = float(c['u_pct'].mean())
    m['core_sd_hp'] = float(hp(c['core_yoy']).std())

    # --- education: unemployment, cyclical betas, separation and job-finding rates ---
    ur = pu / (pe + pu)
    ur_mean = 100 * ur.mean(0)
    ratio = ur_mean / u_agg
    for k, name in enumerate(['HS', 'College', 'Grad']):
        m[f'u_{name}'] = float(ur_mean[k])
        m[f'u_ratio_{name}'] = float(ratio[k])
    # BA+ = College + Graduate
    ba_lf = (pe[:, 1] + pu[:, 1]) + (pe[:, 2] + pu[:, 2])
    ba_rate = 100 * (pu[:, 1] + pu[:, 2]) / ba_lf
    m['u_BAplus'] = float(ba_rate.mean())
    m['u_ratio_BAplus'] = float(ba_rate.mean() / u_agg)
    m['u_ratio_HS_over_BAplus'] = float(ur_mean[0] / ba_rate.mean())
    cyc_all = hp(100 * u_agg_m)
    for k, name in enumerate(['HS', 'College', 'Grad']):
        cyc = hp(100 * ur[:, k])
        m[f'beta_{name}'] = float(np.dot(cyc, cyc_all) / np.dot(cyc_all, cyc_all))
    cyc_ba = hp(ba_rate)
    m['beta_BAplus'] = float(np.dot(cyc_ba, cyc_all) / np.dot(cyc_all, cyc_all))
    sep_rate = sep_edu.sum(0) / emp_edu.sum(0)
    sep_agg = sep_edu.sum() / emp_edu.sum()
    for k, name in enumerate(['HS', 'College', 'Grad']):
        m[f'sep_ratio_{name}'] = float(sep_rate[k] / sep_agg)
    jf_edu = hu_edu.sum(0) / u_edu.sum(0)
    jf_agg = hu_edu.sum() / u_edu.sum()
    for k, name in enumerate(['HS', 'College', 'Grad']):
        m[f'jf_ratio_{name}'] = float(jf_edu[k] / jf_agg)

    # --- flows (monthly, % of the start-of-month stock) ---
    sumE = E0.sum()
    sumU = U0.sum()
    sumN = c['N0'].sum()
    # EE: CPS-style employer change between month-start and month-end snapshots (includes layoff-and-rehire at
    # another firm within the month). On-the-job EE is the flow count of employed households hired elsewhere; it is
    # what JOLTS quits approximate.
    m['EE_pct'] = 100 * c['ee_cps'].sum() / sumE
    m['EE_onjob_pct'] = 100 * c['EE'].sum() / sumE
    m['hires_pct'] = 100 * c['HIRES'].sum() / sumE
    m['quits_pct'] = m['EE_onjob_pct']
    m['layoffs_pct'] = 100 * c['LAY'].sum() / sumE
    m['sepx_pct'] = 100 * c['SEPX'].sum() / sumE
    m['EU_pct'] = m['layoffs_pct'] + m['sepx_pct']
    # Job finding and direct hiring from N: month-start snapshot flows over month-start stocks (U_PRE, N_PRE).
    m['UE_pct'] = 100 * c['hu_snap'].sum() / max(c['U_PRE'].sum(), 1)
    m['NE_pct'] = 100 * c['hn_snap'].sum() / max(c['N_PRE'].sum(), 1)
    m['NE_share_hires'] = 100 * c['hn_snap'].sum() / max(c['HIRES'].sum(), 1)
    m['UN_pct'] = 100 * c['UN'].sum() / max(c['U_PRE'].sum(), 1)
    m['NU_pct'] = 100 * c['NU'].sum() / max(c['N_PRE'].sum(), 1)
    m['vacancy_pct_of_needed'] = 100 * c['VAC'].sum() / max(c['NEEDED'].sum(), 1)

    # cyclicality of the job-to-job rate and hires with unemployment (positive = procyclical)
    ee_rate = c['EE'] / E0
    m['corr_EE_u'] = corr(hp(ee_rate), cyc_all)
    m['corr_hires_u'] = corr(hp(c['HIRES'] / E0), cyc_all)

    # --- duration and UI ---
    # A household in its m-th month of unemployment has been unemployed (m + 0.5) months on average.
    # Mean, median and 27+ week share read the BLS clock (labflows bls_dur_* columns) when present; the
    # resumed clock is the fallback. dur_med_cont keeps the resumed clock (grouped median, not on the BLS clock).
    dm_col = 'bls_dur_mean' if 'bls_dur_mean' in c else 'dur_mean'
    dd_col = 'bls_dur_med' if 'bls_dur_med' in c else 'dur_med'
    d27_col = 'bls_dur_27w' if 'bls_dur_27w' in c else 'dur_27w'
    m['dur_mean_wk'] = float((c[dm_col].mean() + 0.5) * WEEKS_PER_MONTH)
    m['dur_median_wk'] = float((c[dd_col].mean() + 0.5) * WEEKS_PER_MONTH)
    # continuous (grouped) median, resumed clock: the integer median above can only be 6.5 or 10.9 weeks near the 9-10 target
    m['dur_median_cont_wk'] = float(c['dur_med_cont'].mean() * WEEKS_PER_MONTH)
    m['dur_27w_share'] = float(100 * c[d27_col].mean())
    sd = group(c, 'u_dur', 4).sum(0)
    hd = group(c, 'hu_dur', 4).sum(0)
    for b, name in enumerate(['1_3', '4_6', '7_12', '13p']):
        m[f'jf_dur_{name}'] = float(100 * hd[b] / max(sd[b], 1))
    ui_s = group(c, 'u_ui', 3).sum(0)
    ui_h = group(c, 'hu_ui', 3).sum(0)
    for b, name in enumerate(['benefits', 'last_month', 'exhausted']):
        m[f'jf_ui_{name}'] = float(100 * ui_h[b] / max(ui_s[b], 1))

    # --- participation ---
    lf = c['lfpr_pct']
    m['lfpr_mean'] = float(lf.mean())
    m['lfpr_sd_hp'] = float(hp(lf).std())
    m['corr_lfpr_minus_u'] = corr(hp(lf), -cyc_all)

    # --- wage growth response to unemployment ---
    for prefix, series in [('wage', c['wage_g_yoy']), ('stay', c['wage_stay_yoy'])]:
        cors = lag_corrs(hp(series), cyc_all)
        m[f'{prefix}_u_corr_lag0'] = float(cors[0])
        m[f'{prefix}_u_peak_lag_m'] = int(np.nanargmin(cors))
        m[f'{prefix}_u_peak_corr'] = float(np.nanmin(cors))
    # composition-fixed (stayer) wage cycle: its sd relative to the employed-mean wage cycle
    m['stay_wage_sd_hp'] = float(hp(c['wage_stay_yoy']).std())
    m['wage_mean_sd_hp'] = float(hp(c['wage_g_yoy']).std())
    return m


def lag_corrs(wg, cyc):
    """corr(wage cycle_t, u cycle_{t-k}) for k = 0..24 (negative = wage growth falls when unemployment rises)."""
    cors = []
    for k in range(0, 25):
        if k == 0:
            cors.append(corr(wg, cyc))
        else:
            cors.append(corr(wg[k:], cyc[:-k]))
    return np.array(cors)


def run_seed(args, seed, outdir):
    path = os.path.join(outdir, f'lab_s{seed}.csv')
    env = dict(os.environ)
    env.setdefault('RAYON_NUM_THREADS', '2')
    with open(path, 'w') as fh:
        subprocess.run([args.bin, str(args.households), str(args.months), str(seed)], stdout=fh,
                       stderr=subprocess.DEVNULL, env=env, check=True)
    return path


TARGETS = [
    # key, label, low, high (None = no bound), source
    ('u_ratio_HS_over_BAplus', 'HS / BA+ unemployment ratio', 1.8, 2.2, 'CPS HS-or-less / BA+ ~1.9'),
    ('u_ratio_BAplus', 'BA+ unemployment / average', 0.5, 0.7, 'CPS BA+ 0.5-0.6 (model BA+ = College+Grad)'),
    ('u_ratio_HS', 'HS group unemployment / average', 1.15, 1.45, 'model HS = less-than-BA (blend of CPS groups)'),
    ('beta_HS', 'HS cyclical beta', 1.15, None, 'low-skill rise in recessions (HMS 2012)'),
    ('beta_BAplus', 'BA+ cyclical beta', None, 0.85, 'BA+ rise smaller than HS'),
    ('EE_pct', 'employer-to-employer (CPS definition), % of employed / month', 2.0, 2.5, 'CPS EE 2.0-2.3 (FMP 2020)'),
    ('hires_pct', 'hires, % of employment / month', 3.5, 4.0, 'JOLTS hires 3.5-4.0'),
    ('quits_pct', 'on-the-job moves (quits proxy), % / month', 2.2, 2.5, 'JOLTS quits 2.2-2.5'),
    ('EU_pct', 'separations to unemployment, % / month', 1.0, 1.4, 'JOLTS layoffs+discharges ~1.2'),
    ('UE_pct', 'job-finding, % of unemployed / month', 20.0, 30.0, 'CPS ~25-30 (start-of-month snapshot)'),
    ('NE_share_hires', 'N->E share of hires, %', 5.0, None, 'large share of hires (CPS)'),
    ('NE_pct', 'N->E, % of non-participants / month', 0.5, None, 'direct N->E hires (reported)'),
    ('UN_pct', 'U->N, % of unemployed / month', 20.0, 25.0, 'CPS 20-25%; needs demography.rs (reported only)'),
    ('dur_mean_wk', 'mean duration, weeks', 17.0, 23.0, 'BLS 2017-19 ~21-22; CPS normal ~20'),
    ('dur_median_cont_wk', 'median duration, weeks (grouped, continuous)', 8.0, 11.0, 'CPS median 9-10'),
    ('dur_median_wk', 'median duration, weeks (whole months)', 8.0, None, 'granular: 6.5 or 10.9 wk per month (reported)'),
    ('dur_27w_share', 'share unemployed 27+ weeks, %', 17.0, 23.0, 'CPS ~20 (months >= 6)'),
    ('corr_lfpr_minus_u', 'LFPR cycle vs -u cycle (procyclical > 0)', 0.0, None, 'Aaronson et al. 2014'),
    ('corr_EE_u', 'EE cycle vs u cycle (procyclical < 0)', None, -0.0, 'EE procyclical'),
    ('wage_u_peak_lag_m', 'wage growth peak lag vs u, months', 0, 6, 'ECI/AWT: lag <= 6m'),
    ('stay_u_peak_lag_m', 'stayer (composition-fixed) wage peak lag vs u, months', 0, 6, 'ECI/AWT: lag <= 6m'),
    ('u_mean', 'unemployment rate, mean %', 4.3, 5.5, 'headline band 4.3-5.5'),
    ('u_sd_hp', 'unemployment HP sd, pp', 0.6, 1.0, 'master ~0.77; a 25% drop fails'),
]


def status(v, lo, hi):
    if v is None or (isinstance(v, float) and np.isnan(v)):
        return 'n/a'
    if lo is not None and v < lo:
        return 'LOW'
    if hi is not None and v > hi:
        return 'HIGH'
    return 'ok'


def summarise(per_seed):
    keys = list(per_seed[0].keys())
    avg, sdv = {}, {}
    for k in keys:
        vals = np.array([float(s[k]) for s in per_seed])
        avg[k] = float(np.nanmean(vals))
        sdv[k] = float(np.nanstd(vals))
    return avg, sdv


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('csv', nargs='*', help='labflows CSV files to analyse')
    ap.add_argument('--run', action='store_true', help='run labflows for each seed first')
    ap.add_argument('--bin', default=os.path.join(ROOT, 'target', 'release', 'labflows'))
    ap.add_argument('--households', type=int, default=20000)
    ap.add_argument('--months', type=int, default=240)
    ap.add_argument('--seeds', type=int, nargs='+', default=[1, 2, 3, 4])
    ap.add_argument('--out', default=os.path.join(ROOT, 'target', 'labour'))
    ap.add_argument('--json', default=None, help='write metrics as JSON to this path')
    args = ap.parse_args()

    paths = list(args.csv)
    if args.run:
        os.makedirs(args.out, exist_ok=True)
        with ThreadPoolExecutor(len(args.seeds)) as ex:
            paths = list(ex.map(lambda s: run_seed(args, s, args.out), args.seeds))
    if not paths:
        ap.error('give CSV files or --run')

    per_seed = [analyse(load(p)) for p in paths]
    avg, sdv = summarise(per_seed)

    print(f'labour check: {len(paths)} run(s), {DROP} burn-in months dropped\n')
    print(f"{'metric':<46}{'mean':>10}{'(sd)':>9}   {'band':<14}{'status':<6}source")
    for key, label, lo, hi, src in TARGETS:
        v = avg.get(key)
        band = f"[{'' if lo is None else lo}, {'' if hi is None else hi}]"
        st = status(v, lo, hi)
        vs = 'n/a' if v is None else f'{v:10.3f}'
        ss = '' if key not in sdv else f'({sdv[key]:.3f})'
        print(f'{label:<46}{vs:>10}{ss:>9}   {band:<14}{st:<6}{src}')
    print('\nother diagnostics (mean across seeds)')
    shown = set(t[0] for t in TARGETS)
    for k in avg:
        if k not in shown:
            print(f'  {k:<26}{avg[k]:12.4f}  (sd {sdv[k]:.4f})')

    if args.json:
        with open(args.json, 'w') as fh:
            json.dump({'mean': avg, 'sd': sdv, 'per_seed': per_seed, 'files': paths}, fh, indent=1)
    return 0


if __name__ == '__main__':
    sys.exit(main())
