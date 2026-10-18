#!/usr/bin/env python3
"""Joint-calibration gate table for one candidate cell T against its base B (joint2, Joint calibration v2).

Usage:
  joint_gates.py RUNDIR_T CELL_T RUNDIR_B CELL_B [--seeds 1-32] [--json OUT] [--duration]

Runs are RUNDIR/<cell>_s<seed>.json.gz (see paired.py). Statistics come from paired.py (seed_stats, pooled_stats,
jackknife_replicates, jk_se, jf_ratio), so the numbers match `paired.py pair`.

Verdicts (spec 2.2, 2.3, 2.4):
  G1 hard bands (32 seeds): policy 1.6-3.6 (joint2 decision), u 4.3-5.5, core PCE 1.7-2.3, HP GDP sd 1.1-1.9.
  G2 per-seed: policy < 1.0 in at most 2 seeds, policy < 0.5 in at most 1, max one-month |dlnGDP| <= 3.00%,
     non-finite 0, sfc 0.
  G3 corporate profit share 9-11 % of GDP.
  G4 cycle not worse vs B: seven statistics, each Z >= -2.0 (exp_dur_m, hk_rate, u_updown, u_ac1, du12_skew,
     tot_lt25, and the u<2.5 sign test Zc).
  REPORT: per-seed u<2.5 target (>= ceil(0.875 n) seeds with <= 6 months of u<2.5), reported cycle rows.
  J1 (b7-spikes, duration cells only): jf_lt_st in [0.30, 0.42]. Computed, printed and written to the JSON
     verdicts only with --duration; otherwise the line reads "J1 n/a (not a duration cell)" and "J1" is null.
  T2: dD_J <= -max(0.30, 2 SE). T4: BLS dur_mean in [17, 23], BLS ltu in [15, 23], and dD_J <= +max(0.30, 2 SE).

Duration rows are read on the BLS clock when the record has the BLS keys, else on the resumed clock.

--stage2 (b8-world trade link, S2-R3 and S2-R4): the policy counts of G2 scale with the seed count (limits 2 and 1 per 32 seeds,
so 4 and 2 on 64); the one-month |dlnGDP| > 3 % guard is a paired count, the link-on count at most the matched cell's count + 1
(the matched cell is the block on, link off, same ABM parameters, given as the base); du12_skew is reported and not gated in G4.
"""
import argparse, json, math, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import numpy as np  # noqa: E402
import paired as P  # noqa: E402

SKIP = 12
# joint2 policy band (decision in docs/CALIBRATION.md, Joint calibration v2, section 0.1)
BANDS = {'policy': (1.6, 3.6), 'u_mean': (4.3, 5.5), 'core': (1.7, 2.3), 'y_sd': (1.1, 1.9)}
BAND_LABEL = {'policy': 'policy', 'u_mean': 'u', 'core': 'core PCE', 'y_sd': 'y_sd'}
NEAR = 0.05
CORP_BAND = (9.0, 11.0)
U25_TARGET_FRAC = 0.875

# G4 rows: name, sign ('pool' = pooled jackknife, 'seed' = per-seed paired), key
CYCLE_G4 = [
    ('exp_dur_m', +1, 'pool'),
    ('hk_rate', -1, 'pool'),
    ('u_updown', 0, 'pool'),  # sign from the base level, see sign_updown
    ('u_ac1', +1, 'seed'),
    ('du12_skew', 0, 'pool'),  # sign from the base level, see sign_du12
    ('tot_lt25', -1, 'seed'),
]
REPORTED = [('two_q_per_century', 'pool'), ('dns_dur_exp', 'pool'), ('g_sd', 'seed'), ('speed_ratio', 'pool'),
            ('rec_per_century', 'pool')]

# b7-spikes J1: jf_lt_st band for duration cells (spec 5 Stage D).
J1_BAND = (0.30, 0.42)
DUR = [  # row, band lo, hi
    ('dur_mean', 16, 21), ('dur_med', 8, 11), ('ltu', 15, 23), ('ltu_peak3', 26, 46), ('dur_slope', 1.2, 2.5),
    ('ltu_lag', 6, 12), ('med_lag', 3, 8), ('dur_ratio', 1.7, 2.3), ('jf_lt_st', 0.33, 0.42),
]

DECOMP = [('unemployment_rate', None), ('u_star', None), ('i_star', None), ('policy_rate', None),
          ('core_pce_inflation_yoy', None), ('yields', 3), ('mortgage_rate', None), ('participation_rate', None)]


def sign_updown(u):
    """u_updown: + while the base sits below 1.2, - above 2.7, otherwise 0."""
    if u < 1.2:
        return +1
    if u > 2.7:
        return -1
    return 0


def sign_du12(s):
    """du12_skew: + while the base sits below 0.4, - above 1.5 (US 0.97, band 0.4-1.5), otherwise 0."""
    if s < 0.4:
        return +1
    if s > 1.5:
        return -1
    return 0


def se_seed(d):
    d = np.asarray(d, float)
    return float(d.std(ddof=1) / math.sqrt(len(d))) if len(d) > 1 else float('nan')


def dist(x, lo, hi):
    return max(0.0, lo - x, x - hi) / (hi - lo)


def bls_available(rec):
    return all(k in rec[SKIP] for k in P.BLS_KEYS)


def seed_row(rec, seed):
    """Per-seed statistics plus the duration rows on the BLS clock (or the resumed clock when BLS keys are absent)."""
    st, smp, dl = P.seed_stats(rec, -1.0)
    st['seed'] = seed
    h = rec[SKIP:240]
    u = np.array([m['unemployment_rate'] for m in h], float)
    if bls_available(rec):
        bls = P.dur_stats(h, u, keys=P.BLS_KEYS)
        dm_res = np.array([m['dur_mean_weeks'] for m in h], float)
        dm_bls = np.array([m['dur_mean_weeks_bls'] for m in h], float)
        st['clock_differs'] = bool(np.any(dm_res != dm_bls))
        clock = 'bls'
    else:
        bls = P.dur_stats(h, u, keys=P.RESUMED_KEYS)
        st['clock_differs'] = False
        clock = 'resumed'
    st['clock'] = clock
    st['dur'] = {'dur_mean': bls['dur_mean'], 'dur_med': bls['dur_med'], 'ltu': bls['ltu'],
                 'ltu_peak3': bls['ltu_peak3'], 'dur_slope': bls['dur_slope'], 'med_lag': bls['med_lag'],
                 'ltu_lag': bls['ltu_lag']}
    st['jf_h'] = bls['jf_h']; st['jf_s'] = bls['jf_s']
    return st, smp, dl, rec


def loadset(rundir, cell, seeds):
    rows, samples, full = [], [], []
    for s in seeds:
        rec = P.load(rundir, cell, s)
        st, smp, dl, rec = seed_row(rec, s)
        rows.append(st); samples.append(smp); full.append(rec)
    return rows, samples, full


def dur_values(rows):
    """Seed-mean duration rows, the dur_ratio and jf_lt_st, as dict name -> value."""
    v = {}
    for name in ('dur_mean', 'dur_med', 'ltu', 'ltu_peak3', 'dur_slope', 'ltu_lag', 'med_lag'):
        v[name] = float(np.mean([r['dur'][name] for r in rows]))
    v['dur_ratio'] = float(np.mean([r['dur']['dur_mean'] for r in rows]) / np.mean([r['dur']['dur_med'] for r in rows]))
    v['jf_lt_st'] = P.jf_ratio(rows)
    return v


def D_J(rows):
    v = dur_values(rows)
    return sum(dist(v[name], lo, hi) for name, lo, hi in DUR)


def decomp_mean(full, key, idx):
    out = []
    for rec in full:
        vals = []
        for m in rec[SKIP:240]:
            v = m.get(key)
            if isinstance(v, list):
                v = v[idx] if idx is not None and len(v) > idx else None
            if v is None:
                continue
            vals.append(float(v))
        out.append(float(np.mean(vals)) if vals else float('nan'))
    return np.array(out, float)


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('rundir_t'); ap.add_argument('cell_t'); ap.add_argument('rundir_b'); ap.add_argument('cell_b')
    ap.add_argument('--seeds', default='1-32')
    ap.add_argument('--json', default=None)
    ap.add_argument('--duration', action='store_true',
                    help='duration cell: compute, print and write the J1 verdict (jf_lt_st in [0.30, 0.42])')
    ap.add_argument('--stage4', action='store_true',
                    help='stage-4 scoring: as --stage2 (count scaling, paired spike count) with du12_skew gated, and the policy counts '
                         'at most max(int(2n/32), base + 1) and max(int(n/32), base_05 + 1) against the matched base cell')
    ap.add_argument('--stage2', action='store_true',
                    help='trade-link scoring: G2 per S2-R3 (scaled policy limits, paired spike count), du12_skew reported (S2-R4)')
    a = ap.parse_args(argv)
    seeds = P.parse_seeds(a.seeds)
    n = len(seeds)
    need_u25 = math.ceil(U25_TARGET_FRAC * n)
    out = {'T': a.cell_t, 'B': a.cell_b, 'seeds': seeds}
    T, sT, fT = loadset(a.rundir_t, a.cell_t, seeds)
    B, sB, fB = loadset(a.rundir_b, a.cell_b, seeds)
    clocks = {r['clock'] for r in T + B}
    if any(r['clock_differs'] for r in T + B):
        print('WARNING: resumed and BLS clocks differ')
    out['clock'] = sorted(clocks)

    print(f'joint_gates v2  T={a.cell_t} ({a.rundir_t})  vs  B={a.cell_b} ({a.rundir_b})  seeds {seeds[0]}-{seeds[-1]} (n={n})')
    print(f'duration clock: {", ".join(sorted(clocks))}')

    # 1. levels and hard bands (G1)
    print('\n1. levels (mean +- SE over seeds) and hard bands (G1)')
    lev = {}
    for k in ['policy', 'u_mean', 'core', 'y_sd', 'corp_share', 'energy_share']:
        xt = np.array([r[k] for r in T], float)
        xb = np.array([r[k] for r in B], float)
        lev[k] = {'T': float(xt.mean()), 'T_se': se_seed(xt), 'B': float(xb.mean()), 'B_se': se_seed(xb)}
        if k in BANDS:
            lo, hi = BANDS[k]
            inb = lo <= xt.mean() <= hi
            near = (0 <= xt.mean() - lo < NEAR) or (0 <= hi - xt.mean() < NEAR)
            lev[k].update(band=[lo, hi], in_band=bool(inb), near=bool(near))
            print(f'  {k:12s} T {xt.mean():8.3f} +- {se_seed(xt):.3f}  B {xb.mean():8.3f}  band {lo}-{hi}  '
                  f"{'IN' if inb else 'OUT'}{'  near-bound' if near else ''}")
        else:
            print(f'  {k:12s} T {xt.mean():8.3f} +- {se_seed(xt):.3f}  B {xb.mean():8.3f}')
    b1 = all(lev[k]['in_band'] for k in BANDS)
    out['levels'] = lev

    # 2. per-seed rows (G2)
    print('\n2. per-seed rows (G2)')
    per = {}
    for lab, rows in (('T', T), ('B', B)):
        p1 = [r['seed'] for r in rows if r['policy'] < 1.0]
        p05 = [r['seed'] for r in rows if r['policy'] < 0.5]
        gate = [r['seed'] for r in rows if r['tot_lt25'] <= 6]
        mx = max(r['max_dlng'] for r in rows)
        mx_seed = max(rows, key=lambda r: r['max_dlng'])
        nf = sum(r['nonfinite'] for r in rows)
        sfc = max(r['sfc_max'] for r in rows)
        per[lab] = {'policy_lt1': p1, 'policy_lt05': p05, 'lt25_le6': gate, 'max_dlng': float(mx),
                    'max_dlng_seed': int(mx_seed['seed']), 'max_dlng_month': int(mx_seed['max_dlng_tick']),
                    'nonfinite': int(nf), 'sfc_max': float(sfc)}
        print(f'  {lab}: policy<1.0 {len(p1)} {p1}; policy<0.5 {len(p05)} {p05}')
        print(f'      u<2.5 <=6 months (target, REPORT): {len(gate)}/{n} (target >= {need_u25})')
        print(f'      max one-month |dlnGDP| {mx:.2f}% (seed {mx_seed["seed"]}, month {mx_seed["max_dlng_tick"]}); '
              f'nonfinite {nf}; sfc {sfc:.3g}')
    if a.stage4:
        lim_p1 = max(int(2 * n / 32), len(per['B']['policy_lt1']) + 1)
        lim_p05 = max(int(n / 32), len(per['B']['policy_lt05']) + 1)
    else:
        lim_p1, lim_p05 = (2, 1) if not a.stage2 else (int(2 * n / 32), int(n / 32))
    g2_p1 = len(per['T']['policy_lt1']) <= lim_p1
    g2_p05 = len(per['T']['policy_lt05']) <= lim_p05
    if a.stage2 or a.stage4:
        cnt_t = sum(1 for r in T if r['max_dlng'] > 3.0)
        cnt_b = sum(1 for r in B if r['max_dlng'] > 3.0)
        g2_mx = cnt_t <= cnt_b + 1
        mx_txt = f"seeds with one-month |dlnGDP| > 3%: T {cnt_t} <= B {cnt_b} + 1"
        per['T']['spike_count'] = cnt_t
        per['B']['spike_count'] = cnt_b
    else:
        g2_mx = per['T']['max_dlng'] <= 3.0
        mx_txt = f"max |dlnGDP| {per['T']['max_dlng']:.2f}%<=3.00"
    g2_nf = per['T']['nonfinite'] == 0 and per['T']['sfc_max'] == 0
    g2 = g2_p1 and g2_p05 and g2_mx and g2_nf
    print(f"  G2 verdict: {'PASS' if g2 else 'FAIL'}  (policy<1.0 {len(per['T']['policy_lt1'])}<={lim_p1} "
          f"{'ok' if g2_p1 else 'X'}, policy<0.5 {len(per['T']['policy_lt05'])}<={lim_p05} {'ok' if g2_p05 else 'X'}, "
          f"{mx_txt} {'ok' if g2_mx else 'X'}, nonfinite/sfc "
          f"{'ok' if g2_nf else 'X'})")
    if a.stage4:
        # Stage 5 (S4-R32, W4-32): the exact one-sided binomial probability of the (link, matched) spike counts, printed beside the rule.
        # The formula is `cmd_binom` of world_s5.py; the rule above is unchanged and this line is not a gate.
        nn = cnt_t + cnt_b
        pb = sum(math.comb(nn, k) for k in range(cnt_t, nn + 1)) / 2 ** nn if nn else 1.0
        print(f'  G2 spike guard: link {cnt_t}, matched {cnt_b}; exact one-sided binomial p = {pb:.3f} (printed, not a gate)')
    out['per_seed'] = per

    # 3. G4 cycle set (d = T - B)
    print('\n3. G4 cycle set: d = T - B, Z = sign * d / SE, clamped to [-3, 3]')
    pT = P.pooled_stats(sT); pB = P.pooled_stats(sB)
    repT = P.jackknife_replicates(sT); repB = P.jackknife_replicates(sB)
    cyc = []
    for name, sign, kind in CYCLE_G4:
        if name == 'u_updown':
            sg = sign_updown(pB[name])
        elif name == 'du12_skew':
            sg = sign_du12(pB[name])
        else:
            sg = sign
        if kind == 'pool':
            d = pT[name] - pB[name]
            se = P.jk_se([{'d': rt[name] - rb[name]} for rt, rb in zip(repT, repB)], 'd')
            vt, vb = pT[name], pB[name]
        else:
            xt = np.array([r[name] for r in T], float); xb = np.array([r[name] for r in B], float)
            d = float(xt.mean() - xb.mean()); se = se_seed(xt - xb); vt, vb = xt.mean(), xb.mean()
        z = 0.0 if (sg == 0 or se == 0 or not math.isfinite(se)) else float(np.clip(sg * d / se, -3, 3))
        cyc.append({'row': name, 'sign': sg, 'B': float(vb), 'T': float(vt), 'd': float(d), 'SE': float(se), 'Z': z})
    # u<2.5 count: sign test over seeds with <= 6 months of u<2.5
    gT = {r['seed'] for r in T if r['tot_lt25'] <= 6}
    gB = {r['seed'] for r in B if r['tot_lt25'] <= 6}
    g = len(gT - gB); l = len(gB - gT)
    zc = float((g - l) / math.sqrt(g + l)) if g + l else 0.0
    cyc.append({'row': 'u25_count', 'sign': +1, 'B': float(len(gB)), 'T': float(len(gT)), 'd': float(len(gT) - len(gB)),
                'SE': float('nan'), 'Z': float(np.clip(zc, -3, 3)), 'gained': sorted(gT - gB), 'lost': sorted(gB - gT)})
    for c in cyc:
        extra = ''
        if c['row'] == 'u25_count':
            extra = f"  gained {c['gained']} lost {c['lost']}  (g {g}, l {l})"
            print(f"  {c['row']:16s} B {int(c['B']):3d}/{n}  T {int(c['T']):3d}/{n}  Zc {c['Z']:+.2f}{extra}")
        else:
            print(f"  {c['row']:16s} sign {c['sign']:+d}  B {c['B']:9.3f}  T {c['T']:9.3f}  d {c['d']:+9.3f} "
                  f"SE {c['SE']:7.3f}  Z {c['Z']:+.2f}")
    zs = [c['Z'] for c in cyc]
    sumz, minz = float(sum(zs)), float(min(zs))
    gated = [c for c in cyc if not (a.stage2 and not a.stage4 and c['row'] == 'du12_skew')]
    worse = [c['row'] for c in gated if c['Z'] < -2.0]
    g4 = len(worse) == 0
    print(f"  G4 verdict: {'PASS' if g4 else 'FAIL'}  (Z < -2.0: {worse if worse else 'none'})")
    if a.stage2 and not a.stage4:
        d12 = [c for c in cyc if c['row'] == 'du12_skew'][0]
        print(f"  du12_skew (reported, not gated, S2-R4): Z {d12['Z']:+.2f}")
    print(f'  reported (not in G4): sum Z {sumz:+.2f}  min Z {minz:+.2f}')
    rep = []
    for name, kind in REPORTED:
        if kind == 'pool':
            d = pT[name] - pB[name]
            se = P.jk_se([{'d': rt[name] - rb[name]} for rt, rb in zip(repT, repB)], 'd')
            vt, vb = pT[name], pB[name]
        else:
            xt = np.array([r[name] for r in T], float); xb = np.array([r[name] for r in B], float)
            d = float(xt.mean() - xb.mean()); se = se_seed(xt - xb); vt, vb = xt.mean(), xb.mean()
        rep.append({'row': name, 'B': float(vb), 'T': float(vt), 'd': float(d), 'SE': float(se)})
        print(f'    {name:18s} B {vb:9.3f}  T {vt:9.3f}  d {d:+9.3f} SE {se:7.3f}')
    out['cycle'] = {'rows': cyc, 'worse': worse, 'G4': g4, 'sumZ': sumz, 'minZ': minz, 'reported': rep,
                    'u25': {'T': len(gT), 'B': len(gB), 'gained': g, 'lost': l, 'Zc': zc,
                            'target': need_u25, 'target_met': len(gT) >= need_u25}}
    print(f"  u<2.5 target (REPORT): T {len(gT)}/{n} vs >= {need_u25}: {'MET' if len(gT) >= need_u25 else 'not met'}")

    # 4. duration rows on the BLS clock and D_J (target T2 and T4)
    print('\n4. duration rows (BLS clock) and D_J')
    vT = dur_values(T); vB = dur_values(B)
    dj_t = dj_b = 0.0
    drows = []
    for name, lo, hi in DUR:
        dt, db = dist(vT[name], lo, hi), dist(vB[name], lo, hi)
        dj_t += dt; dj_b += db
        drows.append({'row': name, 'B': vB[name], 'T': vT[name], 'band': [lo, hi], 'dist_T': dt, 'dist_B': db})
        print(f'  {name:12s} B {vB[name]:9.3f}  T {vT[name]:9.3f}  band {lo}-{hi}  distance T {dt:.3f}  B {db:.3f}')
    d_dj = dj_t - dj_b
    reps = []
    for i in range(n):
        tt = [T[j] for j in range(n) if j != i]; bb = [B[j] for j in range(n) if j != i]
        reps.append(D_J(tt) - D_J(bb))
    v = np.array(reps, float)
    se_dj = float(math.sqrt((n - 1) / n * np.sum((v - v.mean()) ** 2)))
    print(f'  D_J(T) {dj_t:.3f}   D_J(B) {dj_b:.3f}   dD_J {d_dj:+.3f} +- {se_dj:.3f}')
    t2 = d_dj <= -max(0.30, 2 * se_dj)
    t4_mean = 17 <= vT['dur_mean'] <= 23
    t4_ltu = 15 <= vT['ltu'] <= 23
    t4_dj = d_dj <= max(0.30, 2 * se_dj)
    t4 = t4_mean and t4_ltu and t4_dj
    print(f"  T2 (dD_J <= -max(0.30, 2SE) = {-max(0.30, 2 * se_dj):+.3f}): {'PASS' if t2 else 'FAIL'}")
    print(f"  T4 (dur_mean {vT['dur_mean']:.2f} in 17-23 {'ok' if t4_mean else 'X'}; ltu {vT['ltu']:.2f} in 15-23 "
          f"{'ok' if t4_ltu else 'X'}; dD_J <= +max(0.30, 2SE) {'ok' if t4_dj else 'X'}): {'PASS' if t4 else 'FAIL'}")
    out['duration'] = {'rows': drows, 'DJ_T': dj_t, 'DJ_B': dj_b, 'dDJ': d_dj, 'SE_dDJ': se_dj,
                       'dur_mean_T': vT['dur_mean'], 'ltu_T': vT['ltu'], 'dur_mean_B': vB['dur_mean'],
                       'ltu_B': vB['ltu']}

    # 5. policy and participation decomposition (report)
    print('\n5. decomposition, d +- SE over seeds (months 12-239), report only')
    dec = []
    for key, idx in DECOMP:
        xt = decomp_mean(fT, key, idx); xb = decomp_mean(fB, key, idx)
        ok = np.isfinite(xt) & np.isfinite(xb)
        lab = key if idx is None else f'{key}[{idx}]'
        if not ok.any():
            print(f'  {lab} (null)'); continue
        d = xt[ok] - xb[ok]
        dec.append({'key': lab, 'B': float(xb[ok].mean()), 'T': float(xt[ok].mean()), 'd': float(d.mean()),
                    'SE': se_seed(d)})
        print(f'  {lab:24s} B {xb[ok].mean():8.3f}  T {xt[ok].mean():8.3f}  d {d.mean():+8.3f} +- {se_seed(d):.3f}')
    ct = np.array([sum(1 for m in r[SKIP:240] if m['policy_rate'] <= 0.25) for r in fT], float)
    cb = np.array([sum(1 for m in r[SKIP:240] if m['policy_rate'] <= 0.25) for r in fB], float)
    dec.append({'key': 'months policy<=0.25', 'B': float(cb.mean()), 'T': float(ct.mean()),
                'd': float((ct - cb).mean()), 'SE': se_seed(ct - cb)})
    print(f"  {'months policy<=0.25':24s} B {cb.mean():8.2f}  T {ct.mean():8.2f}  d {(ct - cb).mean():+8.2f} "
          f"+- {se_seed(ct - cb):.2f}")
    out['decomp'] = dec

    # 6. verdicts
    print('\n6. verdicts')
    b4 = CORP_BAND[0] <= lev['corp_share']['T'] <= CORP_BAND[1]
    g1 = b1
    print(f"  G1 hard bands: {'PASS' if g1 else 'FAIL'}  (" + ', '.join(
        f"{BAND_LABEL[k]} {lev[k]['T']:.3f}" for k in BANDS) + ')')
    print(f"  G2 per-seed: {'PASS' if g2 else 'FAIL'}")
    print(f"  G3 corp share 9-11: {'PASS' if b4 else 'FAIL'}  ({lev['corp_share']['T']:.2f}% of GDP)")
    print(f"  G4 cycle not worse: {'PASS' if g4 else 'FAIL'}")
    print(f"  REPORT u<2.5 target: {len(gT)}/{n} (>= {need_u25})")
    print(f"  T2: {'PASS' if t2 else 'FAIL'}   T4: {'PASS' if t4 else 'FAIL'}")
    # b7-spikes J1 (duration cells only, --duration): long-term/short-term job-finding ratio in [0.30, 0.42].
    # Master is 0.461. Without --duration the verdict is not computed: the line says n/a and the JSON has null.
    if a.duration:
        j1 = J1_BAND[0] <= vT['jf_lt_st'] <= J1_BAND[1]
        print(f"  J1 (duration cells only; jf_lt_st {vT['jf_lt_st']:.3f} in [{J1_BAND[0]:.2f}, {J1_BAND[1]:.2f}]): "
              f"{'PASS' if j1 else 'FAIL'}")
    else:
        j1 = None
        print('  J1 n/a (not a duration cell)')
    out['verdicts'] = {'G1': g1, 'G2': g2, 'G3': b4, 'G4': g4, 'u25_target_report': len(gT) >= need_u25,
                       'T2': t2, 'T4': t4, 'J1': j1, 'jf_lt_st_T': vT['jf_lt_st'], 'sumZ': sumz, 'minZ': minz,
                       'dDJ': d_dj, 'SE_dDJ': se_dj, 'DJ_T': dj_t}
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(out, fh, indent=1, default=float)
    return 0


if __name__ == '__main__':
    sys.exit(main())
