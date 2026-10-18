#!/usr/bin/env python3
"""Technology-boom cycle diagnostics (stage 4; docs/SECTORS.md §Stage 4).

Runs 8 seeds x 40 years with the Markov tech-boom regime on (ECONSIM_CFG overrides are honoured), finds boom
episodes, and reports, around each boom:
  * productivity growth: Tech TFP growth in boom vs normal months, and aggregate labour-productivity growth
    (real GDP per employed person, year on year) in boom vs normal months (pp);
  * the Tech P/E path (sector equity ÷ 12 × sector monthly profit, 6-month average) and its peak relative to the
    24 months before the boom;
  * drawdowns after the boom ends (peak-to-trough within 36 months): Tech equity, total equity;
  * unemployment (pp change, max over 36 months) and GDP growth (average over the 24 months after the end, vs the
    24 months before the start) around busts;
  * Tech gross investment (ratio after the end vs the 24 months before the start).

Usage: python3 scripts/tech_cycle.py [path to econsim-cli]   (build --release first). Use -I when running it.
"""
import json, os, subprocess, sys
from concurrent.futures import ThreadPoolExecutor
import numpy as np

CLI = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'target', 'release', 'econsim-cli')
SEEDS = 8
MONTHS = 480
HH = os.environ.get('TECH_HH', '20000')
TECH = 4


def run(seed):
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    env.setdefault('ECONSIM_CFG', json.dumps({'tech_boom_enabled': True}))
    out = subprocess.run([CLI, 'run', HH, str(MONTHS), str(seed), '--json'], capture_output=True, text=True, env=env).stdout
    return json.loads(out.strip().splitlines()[-1])


def episodes(flag):
    eps, i = [], 0
    while i < len(flag):
        if flag[i]:
            j = i
            while j < len(flag) and flag[j]:
                j += 1
            eps.append((i, j))  # [start, end) months
            i = j
        else:
            i += 1
    return eps


def series(h):
    gdp = np.array([m['gdp_real_bn_ar'] for m in h])
    emp = np.array([m['employment_millions'] for m in h])
    lp = gdp / np.maximum(emp, 1e-9)
    lp_yoy = np.full(len(h), np.nan)
    lp_yoy[12:] = 100 * (lp[12:] / lp[:-12] - 1)
    tech_pe = np.array([m['pe_by_sector'][TECH] for m in h])
    k = 6
    tech_pe_s = np.convolve(tech_pe, np.ones(k) / k, mode='same')
    eq = np.array([m['equity_value_bn'] for m in h])
    eq_tech = np.array([m['equity_value_by_sector'][TECH] for m in h])
    return dict(
        flag=np.array([m['tech_boom'] > 0.5 for m in h]),
        tfp_tech=np.array([m['tfp_growth_tech'] for m in h]),
        lp_yoy=lp_yoy,
        tech_pe=tech_pe_s,
        eq=eq,
        eq_tech=eq_tech,
        u=np.array([m['unemployment_rate'] for m in h]),
        gdp_g=np.array([m['gdp_real_growth_yoy'] for m in h]),
        inv_tech=np.array([m['tech_investment_bn_ar'] for m in h]),
    )


def stats(runs):
    rows = {k: [] for k in ['n_boom', 'boom_len', 'tfp_gain', 'lp_gain', 'pe_peak_ratio', 'tech_dd', 'tot_dd', 'u_rise', 'gdp_dip', 'inv_ratio']}
    for s in runs:
        flag = s['flag']
        normal = ~flag
        rows.setdefault('tfp_boom', []).append(np.nanmean(s['tfp_tech'][flag]) if flag.any() else np.nan)
        rows.setdefault('tfp_norm', []).append(np.nanmean(s['tfp_tech'][normal]))
        rows.setdefault('lp_boom', []).append(np.nanmean(s['lp_yoy'][flag]) if flag.any() else np.nan)
        rows.setdefault('lp_norm', []).append(np.nanmean(s['lp_yoy'][normal & ~np.isnan(s['lp_yoy'])]))
        rows.setdefault('tech_pe_norm', []).append(np.nanmedian(s['tech_pe'][normal]))
        rows.setdefault('eq_share_norm', []).append(100 * np.nanmean(s['eq_tech'][normal] / s['eq'][normal]))
        rows.setdefault('eq_share_boom', []).append(100 * np.nanmean(s['eq_tech'][flag] / s['eq'][flag]) if flag.any() else np.nan)
        for (a, b) in episodes(flag):
            rows['n_boom'].append(1)
            rows['boom_len'].append(b - a)
            pre = slice(max(0, a - 24), a)
            base_pe = np.nanmedian(s['tech_pe'][pre]) if a >= 1 else np.nan
            rows['pe_peak_ratio'].append(np.nanmax(s['tech_pe'][a:min(b + 1, len(flag))]) / base_pe if base_pe > 0 else np.nan)
            post = slice(b, min(b + 36, len(flag)))
            if b < len(flag) - 1 and post.stop - post.start > 1:
                for key, arr in [('tech_dd', s['eq_tech']), ('tot_dd', s['eq'])]:
                    seg = arr[post]
                    peak = np.nanmax(arr[max(0, b - 3):b + 1])
                    rows[key].append(100 * (np.nanmin(seg) / peak - 1))
                u_base = s['u'][max(0, a - 1)]
                rows['u_rise'].append(np.nanmax(s['u'][post]) - u_base)
                if a < 24:
                    continue
                g_pre = np.nanmean(s['gdp_g'][max(0, a - 24):a])
                g_post = np.nanmean(s['gdp_g'][b:min(b + 24, len(flag))])
                rows['gdp_dip'].append(g_post - g_pre)
                i_peak = np.nanmax(s['inv_tech'][a:b])
                i_post = np.nanmin(s['inv_tech'][b:min(b + 24, len(flag))])
                rows['inv_ratio'].append(i_post / i_peak if i_peak > 0 else np.nan)
    return rows


def fmt(name, v, unit=''):
    v = np.array(v, dtype=float)
    v = v[~np.isnan(v)]
    if v.size == 0:
        return f"{name:<46} n/a"
    return f"{name:<46} mean {v.mean():8.2f}{unit}   median {np.median(v):8.2f}{unit}   n={v.size}"


with ThreadPoolExecutor(8) as ex:
    runs = [series(h[12:]) for h in ex.map(run, range(1, SEEDS + 1))]

rows = stats(runs)
n_boom = len(rows['boom_len'])
print(f"seeds {SEEDS} x {MONTHS // 12} years, {HH} households, tech regime on; boom episodes: {n_boom} "
      f"(about one per {20 * SEEDS / max(n_boom, 1):.0f} seed-years)")
print(fmt('boom length (months)', rows['boom_len']))
print(fmt('Tech TFP growth, boom months (%/yr)', rows['tfp_boom'], ''))
print(fmt('Tech TFP growth, normal months (%/yr)', rows['tfp_norm'], ''))
print(fmt('aggregate labour productivity growth, boom (%/yr yoy)', rows['lp_boom']))
print(fmt('aggregate labour productivity growth, normal (%/yr yoy)', rows['lp_norm']))
print(fmt('Tech P/E, normal (median)', rows['tech_pe_norm']))
print(fmt('Tech P/E peak ÷ P/E 24m before boom', rows['pe_peak_ratio'], 'x'))
print(fmt('Tech equity share of Fund equity, normal', rows['eq_share_norm'], '%'))
print(fmt('Tech equity share of Fund equity, boom', rows['eq_share_boom'], '%'))
print(fmt('Tech equity drawdown after boom (36m, %)', rows['tech_dd'], '%'))
print(fmt('Total equity drawdown after boom (36m, %)', rows['tot_dd'], '%'))
print(fmt('unemployment rise after boom, max 36m (pp)', rows['u_rise'], ' pp'))
print(fmt('GDP growth, 24m after vs 24m before (pp)', rows['gdp_dip'], ' pp'))
print(fmt('Tech investment, trough in 24m after end ÷ boom peak', rows['inv_ratio'], 'x'))
