#!/usr/bin/env python3
"""Per-seed statistics, 16-seed levels and paired differences for re-fit runs (spec sections 1.4 and 8).

Runs are gzipped JSON histories written as RUNDIR/<cell>_s<seed>.json.gz by `econsim-cli run 20000 240 <seed> --json`.

Usage:
  paired.py levels RUNDIR CELL [--seeds 1-16] [--sanity] [--rstar-floor PCT]
      Per-seed table, mean +- SE of the level statistics, and the per-seed sanity gates (spec 8B).
  paired.py pair RUNDIR_T CELL_T RUNDIR_B CELL_B [--seeds 1-8] [--rstar-floor PCT] [--rstar-floor-b PCT]
      Paired difference T - B: mean d, SE = sd(d)/sqrt(n), and the pooled episode statistics. The pooled difference
      d = stat(T) - stat(B) has a paired leave-one-seed-out jackknife SE: for each left-out seed i, d_(i) =
      stat(T without i) - stat(B without i), and SE = sqrt((n-1)/n * sum_i (d_(i) - mean d_(.))^2).

Statistics use months 12..239 (record indices 12..239) unless noted. Episode statistics and the moment definitions
reuse scripts/facts_cycle.py (sample_moments, episode_stats, model_sample) so the numbers match the scorecard.
Only numpy and the stdlib are used, plus facts_cycle from this directory.
"""
import argparse, gzip, json, math, os, sys, warnings

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np  # noqa: E402
warnings.filterwarnings('ignore', category=RuntimeWarning)
import facts_cycle as fc  # noqa: E402

SKIP = 12
POOLED = ['rec_per_century', 'two_q_per_century', 'exp_dur_m', 'dns_dur_exp', 'speed_ratio', 'hk_rate']
# Keys that the simulator writes as JSON null (serde_json writes NaN as null) by design, before they are defined.
# Anything else that is null or non-finite, at any depth (including list elements), counts as a failure.
NAN_ALLOWED_PREFIXES = ('stayer_wage_change_',)
NAN_ALLOWED_KEYS = {'wage_per_eff', 'newhire_wage_per_eff', 'investment_inaction_share'}


def nonfinite_scan(full):
    """Return (bad, allowed): dicts key -> list of ticks, for null or non-finite values anywhere in the record."""
    bad, allowed = {}, {}

    def walk(key, tick, v, sink):
        if v is None or (isinstance(v, float) and not math.isfinite(v)):
            sink.setdefault(key, []).append(tick)
        elif isinstance(v, (list, tuple)):
            for x in v:
                walk(key, tick, x, sink)
        elif isinstance(v, dict):
            for k2, x in v.items():
                walk(f'{key}.{k2}', tick, x, sink)

    for tick, m in enumerate(full):
        for k, v in m.items():
            ok = k in NAN_ALLOWED_KEYS or k.startswith(NAN_ALLOWED_PREFIXES)
            walk(k, tick, v, allowed if ok else bad)
    return bad, allowed


def load(rundir, cell, seed):
    path = os.path.join(rundir, f'{cell}_s{seed}.json.gz')
    return json.loads(gzip.open(path).read())


def parse_seeds(text):
    out = []
    for part in text.split(','):
        if '-' in part:
            a, b = part.split('-')
            out += list(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def acf(x, k):
    x = np.asarray(x, float)
    return float(np.corrcoef(x[k:], x[:-k])[0, 1])


def longest_run(flags):
    best = run = 0
    for f in flags:
        run = run + 1 if f else 0
        best = max(best, run)
    return best


def acf_peak_2436(acf_by_lag):
    """Secondary-peak height at lags 24-36: max over L in 24..36 of acf[L] minus the lowest acf in 12..L.

    A monotone decay gives zero or less (the lowest point up to L is at L itself), so a value of +0.15 or more means
    the ACF rose by that much from a trough between lags 12 and L, which is a secondary peak (spec 8C, Step 7)."""
    return max(acf_by_lag[L] - min(acf_by_lag[K] for K in range(12, L + 1)) for L in range(24, 37))


def seed_stats(full, rstar_floor_val=-1.0):
    """Per-seed values over months 12..239, plus the samples used for the pooled statistics."""
    h = full[SKIP:240]
    u = np.array([m['unemployment_rate'] for m in h], float)
    lt = u < 2.5
    gdp = np.array([m['gdp_real_bn_ar'] for m in full[:240]], float)
    dl = np.abs(np.diff(np.log(gdp)))  # index t-1 holds |ln gdp[t]/gdp[t-1]|, t = 1..239
    gn = np.array([m['gdp_nominal_bn_ar'] for m in h], float)
    samples = fc.model_sample(h, skip=0)
    Q, um = samples
    mom = fc.sample_moments(Q, um)
    heads = np.array([m['employed_heads_n'] for m in h], float)
    lf = heads / (1 - u / 100)
    yr = np.arange(len(h)) / 12.0
    cap = np.array([m['bank_capital_ratio'] for m in h], float)
    # income-side metrics exist only in binaries from Step 2 on: NaN for older runs
    corp = np.array([m.get('corp_profits_bn_ar', np.nan) + m.get('bank_net_income_bn_ar', np.nan) for m in h], float)
    energy = np.array([m.get('firm_profits_by_sector_bn_ar', [np.nan])[0] for m in h], float)
    # growth ACF (quarterly annualised real GDP growth), investment q/q annualised sd, monthly u ACF
    gq = 400 * np.diff(np.log(Q['y']))
    gi = 400 * np.diff(np.log(Q['i']))
    uacf = {L: acf(u, L) for L in range(6, 37)}
    st = dict(
        g_acf1=acf(gq, 1), g_acf2=acf(gq, 2), g_acf3=acf(gq, 3), g_acf4=acf(gq, 4), g_acf5=acf(gq, 5),
        g_acf6=acf(gq, 6), inv_sd=float(gi.std()),
        u_acf6=uacf[6], u_acf12=uacf[12], u_acf18=uacf[18], u_acf24=uacf[24], u_acf30=uacf[30], u_acf36=uacf[36],
        u_peak2436=acf_peak_2436(uacf),
        dum_sd=mom['dum_sd'],
        u_mean=u.mean(), u_min=u.min(), tot_lt25=int(lt.sum()), longest_lt25=longest_run(lt),
        policy=np.mean([m['policy_rate'] for m in h]),
        core=np.mean([m['core_pce_inflation_yoy'] for m in h]),
        y_sd=mom['y_sd'], g_sd=mom['g_sd'], g_ac1=mom['g_ac1'], u_ac1=mom['u_ac1'],
        corr_emp=mom.get('corr_emp', float('nan')), rel_i=mom.get('rel_i', float('nan')),
        rel_c=mom.get('rel_c', float('nan')), corr_rw=mom.get('corr_rw', float('nan')),
        corr_lp=mom.get('corr_lp', float('nan')), corr_lp_emp=mom.get('corr_lp_emp', float('nan')),
        du12_skew=mom.get('du12_skew', float('nan')), u_updown=mom.get('u_updown', float('nan')),
        corr_rw_eff=mom.get('corr_rw_eff', float('nan')), emp_growth=mom.get('emp_growth', float('nan')),
        max_dlng=100 * dl.max(), max_dlng_tick=int(np.argmax(dl)) + 1,
        # months within 0.01 pp of the r* estimate's floor (the floor is set by r_star_est_min, in percent)
        rstar_floor=int(sum(1 for m in h if m['r_star_est'] <= rstar_floor_val + 0.01)),
        rstar_min=float(min(m['r_star_est'] for m in h)),
        hpa=np.mean([m['hpi_growth_yoy'] for m in h]),
        shelter=np.mean([m['shelter_inflation_yoy'] for m in h]),
        mortgage=np.mean([m['mortgage_rate'] for m in h]),
        fc_k=np.mean([m['foreclosures_thousands_ar'] for m in h]),
        corp_share=100 * np.nanmean(corp / gn), energy_share=100 * np.nanmean(energy / gn),
        lf_growth=100 * ((lf[-1] / lf[0]) ** (12 / (len(h) - 1)) - 1),
        bank_cap_slope=float(np.polyfit(yr, cap, 1)[0]),
    )
    bad, allowed = nonfinite_scan(full)
    st['nonfinite'] = sum(len(v) for v in bad.values())
    st['nonfinite_keys'] = {k: (len(v), v[0], v[-1]) for k, v in bad.items()}
    st['nonfinite_allowed'] = sum(len(v) for v in allowed.values())
    st['sfc_max'] = max(abs(m.get('sfc_error', 0) or 0) for m in full)
    st.update(dur_stats(h, u))
    bls = dur_stats(h, u, keys=BLS_KEYS)
    st['dur_mean_bls'] = bls['dur_mean']
    st['ltu_bls'] = bls['ltu']
    st['ltu_bls_peak3'] = bls['ltu_peak3']
    return st, samples, dl


DUR_ROWS = ['dur_mean', 'dur_med', 'ltu', 'ltu_peak3', 'dur_slope', 'med_lag', 'ltu_lag']
# b6-reentry: the BLS search-spell clock (restarts at every U->N re-entry, resumed or not). Reported next to the
# resumed clock above; the CPS-band reading is the BLS one, because resumption adds months by counting alone.
DUR_ROWS_BLS = ['dur_mean_bls', 'ltu_bls', 'ltu_bls_peak3']


BLS_KEYS = ('dur_mean_weeks_bls', 'dur_median_weeks_bls', 'ltu_share_bls')
RESUMED_KEYS = ('dur_mean_weeks', 'dur_median_weeks', 'ltu_share')


def dur_stats(h, u, keys=RESUMED_KEYS):
    """b5-udur duration rows (months 12..239): NaN on runs without the duration keys.

    keys = (mean weeks, median weeks, share unemployed > 26 weeks) selects the clock: RESUMED_KEYS (default,
    the resumed-spell clock) or BLS_KEYS (the BLS clock, b6-reentry). The default output is unchanged."""
    out = {}
    dmean = np.array([m.get(keys[0], np.nan) for m in h], float)
    dmed = np.array([m.get(keys[1], np.nan) for m in h], float)
    ltu = np.array([m.get(keys[2], np.nan) for m in h], float)
    out['dur_mean'] = float(np.nanmean(dmean))
    out['dur_med'] = float(np.nanmean(dmed))
    out['ltu'] = float(np.nanmean(ltu))
    ma = np.convolve(ltu, np.ones(3) / 3, mode='valid') if np.all(np.isfinite(ltu)) else np.array([np.nan])
    out['ltu_peak3'] = float(np.nanmax(ma))
    ok = np.isfinite(dmed) & np.isfinite(u)
    out['dur_slope'] = float(np.polyfit(u[ok], dmed[ok], 1)[0]) if ok.sum() > 2 else float('nan')

    def lag_of(x):
        # argmax over k = 0..24 of corr(x_t, u_{t-k}), t = k..end
        best, bk = -2.0, float('nan')
        for k in range(25):
            a, b = x[k:], u[:len(u) - k]
            good = np.isfinite(a) & np.isfinite(b)
            if good.sum() < 10 or np.std(a[good]) == 0 or np.std(b[good]) == 0:
                continue
            c = float(np.corrcoef(a[good], b[good])[0, 1])
            if c > best:
                best, bk = c, float(k)
        return bk

    out['med_lag'] = lag_of(dmed) if np.all(np.isfinite(dmed)) else float('nan')
    out['ltu_lag'] = lag_of(ltu) if np.all(np.isfinite(ltu)) else float('nan')
    hires = np.array([m.get('hires_u_dur_n', [np.nan] * 4) for m in h], float)
    stock = np.array([m.get('u_dur_n', [np.nan] * 4) for m in h], float)
    out['jf_h'] = np.nansum(hires, axis=0)
    out['jf_s'] = np.nansum(stock, axis=0)
    return out


def jf_ratio(rows):
    """Pooled finding ratio (Σ hires 13+ / Σ stock 13+) / (Σ hires 1-3 / Σ stock 1-3) over the given seed rows."""
    h = np.sum([r['jf_h'] for r in rows], axis=0)
    s = np.sum([r['jf_s'] for r in rows], axis=0)
    with np.errstate(all='ignore'):
        return float((h[3] / s[3]) / (h[0] / s[0]))


def jf_ratio_jk(rows):
    """Leave-one-seed-out jackknife SE of the pooled finding ratio."""
    n = len(rows)
    reps = [jf_ratio([rows[j] for j in range(n) if j != i]) for i in range(n)]
    v = np.array(reps, float)
    return float(math.sqrt((n - 1) / n * np.sum((v - v.mean()) ** 2)))


def pooled_stats(samples_list):
    """Pooled episode statistics over seeds (point estimate, not a jackknife)."""
    ep = fc.episode_stats(samples_list)
    out = {k: ep.get(k, float('nan')) for k in POOLED}
    d12 = [np.asarray(um[12:], float) - np.asarray(um[:-12], float) for _, um in samples_list]
    alld = np.concatenate(d12)
    out['u_updown'] = fc.u_updown(alld)
    out['du12_skew'] = float(fc.skew(alld))
    return out


def jackknife_replicates(samples_list):
    """Leave-one-seed-out replicates of the pooled statistics: one dict per left-out seed."""
    n = len(samples_list)
    return [pooled_stats([samples_list[j] for j in range(n) if j != i]) for i in range(n)]


def jk_se(replicates, key):
    """Jackknife SE of a statistic from its leave-one-out replicates: sqrt((n-1)/n * sum (x_(i) - mean)^2)."""
    v = np.array([r[key] for r in replicates], float)
    n = len(v)
    return float(math.sqrt((n - 1) / n * np.sum((v - v.mean()) ** 2)))


POOLED_KEYS = POOLED + ['u_updown', 'du12_skew']


def cmd_levels(a):
    seeds = parse_seeds(a.seeds)
    rows, samples, dls = [], [], []
    for s in seeds:
        st, smp, dl = seed_stats(load(a.rundir, a.cell, s), a.rstar_floor)
        st['seed'] = s
        rows.append(st)
        samples.append(smp)
        dls.append(dl)
    pool = pooled_stats(samples)
    rep_ = jackknife_replicates(samples)
    pse = {k: jk_se(rep_, k) for k in POOLED_KEYS}
    print(f'{a.cell}: {len(seeds)} seeds, months 12-239')
    if a.sanity:
        print(f"{'seed':>4} {'u':>5} {'tot<2.5':>7} {'longest':>7} {'policy':>6} {'max|dlnY|':>9} {'tick':>4} "
              f"{'r*min':>6} {'r*floor':>7} {'nonfin':>6} {'sfc':>7}")
        for r in rows:
            print(f"{r['seed']:4d} {r['u_mean']:5.2f} {r['tot_lt25']:7d} {r['longest_lt25']:7d} {r['policy']:6.2f} "
                  f"{r['max_dlng']:9.2f} {r['max_dlng_tick']:4d} {r['rstar_min']:6.3f} {r['rstar_floor']:7d} "
                  f"{r['nonfinite']:6d} {r['sfc_max']:7.2g}")
    keys = ['u_mean', 'policy', 'core', 'y_sd', 'g_sd', 'g_ac1', 'u_ac1', 'corr_emp', 'rel_i', 'rel_c', 'corr_rw',
            'corr_lp', 'emp_growth', 'lf_growth', 'du12_skew', 'u_updown', 'corr_rw_eff', 'hpa', 'shelter', 'mortgage',
            'fc_k', 'corp_share', 'energy_share', 'rstar_floor', 'rstar_min', 'bank_cap_slope', 'tot_lt25', 'max_dlng',
            'g_acf1', 'g_acf2', 'g_acf3', 'g_acf4', 'g_acf5', 'g_acf6', 'inv_sd', 'dum_sd',
            'u_acf6', 'u_acf12', 'u_acf18', 'u_acf24', 'u_acf30', 'u_acf36', 'u_peak2436']
    print('mean +- SE over seeds:')
    for k in keys:
        v = np.array([r[k] for r in rows], float)
        se = v.std(ddof=1) / math.sqrt(len(v)) if len(v) > 1 else float('nan')
        print(f'  {k:16s} {np.nanmean(v):10.3f} +- {se:.3f}')
    print('pooled episode statistics (leave-one-seed-out jackknife SE):')
    for k in POOLED_KEYS:
        print(f'  {k:18s} {pool[k]:10.3f} +- {pse[k]:.3f}')
    print(f"  {'jf_ratio':18s} {jf_ratio(rows):10.3f} +- {jf_ratio_jk(rows):.3f}")
    print('b5-udur duration rows (mean +- SE over seeds):')
    for k in DUR_ROWS + DUR_ROWS_BLS:
        v = np.array([r[k] for r in rows], float)
        se = v.std(ddof=1) / math.sqrt(len(v)) if len(v) > 1 else float('nan')
        print(f'  {k:16s} {np.nanmean(v):10.3f} +- {se:.3f}')
    tot = np.array([r['tot_lt25'] for r in rows])
    lg = np.array([r['longest_lt25'] for r in rows])
    pol = np.array([r['policy'] for r in rows])
    mx = np.array([r['max_dlng'] for r in rows])
    print(f'SANITY 8B: months u<2.5 (total) mean {tot.mean():.1f}, max {tot.max()} (seeds >6: '
          f"{[r['seed'] for r in rows if r['tot_lt25'] > 6]}); longest spell max {lg.max()}")
    print(f"           lowest policy average {pol.min():.2f} (seed {rows[int(pol.argmin())]['seed']}; "
          f"seeds <1.0: {[r['seed'] for r in rows if r['policy'] < 1.0]})")
    print(f"           max one-month |dlnGDP| {mx.max():.2f}% (seeds >3%: {[r['seed'] for r in rows if r['max_dlng'] > 3]})")
    print(f"           non-finite values: {sum(r['nonfinite'] for r in rows)}; max sfc_error {max(r['sfc_max'] for r in rows):.3g}")
    for r in rows:
        if r['nonfinite']:
            print(f"           seed {r['seed']} non-finite keys (count, first tick, last tick): {r['nonfinite_keys']}")
    print(f"           allow-listed NaN/null values (not counted): {sum(r['nonfinite_allowed'] for r in rows)}")


def cmd_pair(a):
    seeds = parse_seeds(a.seeds)
    T, B = [], []
    smT, smB, dlT, dlB = [], [], [], []
    for s in seeds:
        st, smp, dl = seed_stats(load(a.rundir_t, a.cell_t, s), a.rstar_floor)
        sb, smpb, dlb = seed_stats(load(a.rundir_b, a.cell_b, s),
                                   a.rstar_floor if a.rstar_floor_b is None else a.rstar_floor_b)
        T.append(st); B.append(sb); smT.append(smp); smB.append(smpb); dlT.append(dl); dlB.append(dlb)
    n = len(seeds)
    print(f'paired T={a.cell_t} ({a.rundir_t}) minus B={a.cell_b} ({a.rundir_b}), seeds {seeds}')
    print(f"{'statistic':16s} {'B':>10s} {'T':>10s} {'d':>10s} {'SE':>8s}")
    for k in ['u_mean', 'policy', 'core', 'y_sd', 'g_sd', 'g_ac1', 'u_ac1', 'corr_emp', 'rel_i',
                           'rel_c', 'corr_rw', 'corr_lp', 'corr_lp_emp', 'emp_growth', 'lf_growth', 'tot_lt25', 'max_dlng',
                           'rstar_floor', 'rstar_min', 'corp_share', 'energy_share', 'fc_k', 'hpa', 'shelter', 'mortgage',
                           'bank_cap_slope', 'du12_skew', 'u_updown', 'g_acf1', 'g_acf2', 'g_acf3', 'g_acf4',
                           'g_acf5', 'g_acf6', 'inv_sd', 'dum_sd', 'u_acf6', 'u_acf12', 'u_acf18', 'u_acf24',
                           'u_acf30', 'u_acf36', 'u_peak2436']:
        xt = np.array([r[k] for r in T], float)
        xb = np.array([r[k] for r in B], float)
        d = xt - xb
        se = d.std(ddof=1) / math.sqrt(n) if n > 1 else float('nan')
        print(f'{k:16s} {xb.mean():10.3f} {xt.mean():10.3f} {d.mean():+10.3f} {se:8.3f}')
    for k in DUR_ROWS + DUR_ROWS_BLS:
        xt = np.array([r[k] for r in T], float)
        xb = np.array([r[k] for r in B], float)
        d = xt - xb
        se = d.std(ddof=1) / math.sqrt(n) if n > 1 else float('nan')
        print(f'{k:16s} {xb.mean():10.3f} {xt.mean():10.3f} {d.mean():+10.3f} {se:8.3f}')
    jt, jb = jf_ratio(T), jf_ratio(B)
    jd = [jf_ratio([T[j] for j in range(n) if j != i]) - jf_ratio([B[j] for j in range(n) if j != i]) for i in range(n)]
    dv = np.array(jd, float)
    print(f"{'jf_ratio':16s} {jb:10.3f} {jt:10.3f} {jt - jb:+10.3f} "
          f"{math.sqrt((n - 1) / n * np.sum((dv - dv.mean()) ** 2)):8.3f}   (pooled, jackknife SE)")
    pt = pooled_stats(smT)
    pb = pooled_stats(smB)
    rt = jackknife_replicates(smT)
    rb = jackknife_replicates(smB)
    print('pooled episode statistics (SE: paired leave-one-seed-out jackknife on the difference):')
    for k in POOLED_KEYS:
        d_loo = [r_t[k] - r_b[k] for r_t, r_b in zip(rt, rb)]
        pair_rep = [{'d': v} for v in d_loo]
        d = pt[k] - pb[k]
        print(f'{k:18s} {pb[k]:10.3f} {pt[k]:10.3f} {d:+10.3f} {jk_se(pair_rep, "d"):8.3f}')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    lv = sub.add_parser('levels')
    lv.add_argument('rundir'); lv.add_argument('cell')
    lv.add_argument('--seeds', default='1-16'); lv.add_argument('--sanity', action='store_true')
    lv.add_argument('--rstar-floor', type=float, default=-1.0, dest='rstar_floor',
                    help='r_star_est_min in percent (default -1.0); rstar_floor counts months within 0.01 of it')
    lv.set_defaults(fn=cmd_levels)
    pr = sub.add_parser('pair')
    pr.add_argument('rundir_t'); pr.add_argument('cell_t'); pr.add_argument('rundir_b'); pr.add_argument('cell_b')
    pr.add_argument('--seeds', default='1-8')
    pr.add_argument('--rstar-floor', type=float, default=-1.0, dest='rstar_floor',
                    help='treatment r_star_est_min in percent (default -1.0)')
    pr.add_argument('--rstar-floor-b', type=float, default=None, dest='rstar_floor_b',
                    help='baseline r_star_est_min in percent (default: the treatment value)')
    pr.set_defaults(fn=cmd_pair)
    a = ap.parse_args()
    a.fn(a)


if __name__ == '__main__':
    main()
