#!/usr/bin/env python3
"""scripts/world_events.py: stage-4 and stage-5 event rows read from the compare files of scripts/world_runs.py events (S4-R45).

Argument-driven copy of a list-B pre-flight script with two changes (stage 5, spec rev 11):
  * W4-26 P1-P5 and P2 are gated on the SEED-MEAN IRF (D56 vi, the convention of irf_check.py); the per-seed values are printed
    beside them. P7 stays paired against the matched cell E (per seed, jackknife SE).
  * W4-07 and W4-08 are read on `pol100l` at month 24 (F; S5-R5: they stay on pol100l and gate nothing). The pin scenarios are
    read by scripts/world_s5.py w407 and reported as W4-39 (R) only.

Layout (written by world_runs.py events): DIR/CELL_SCENARIO/SCENARIO_sSEED.json.gz, a compare response {baseline, shocked}.
Horizon convention (spec 3.0): month h after a shock at reported month 0 is record index h - 1. Bands are printed; nothing is decided
here beyond the printed PASS / FAIL against the band of the spec. Rows marked [assumed] carry a definition the spec leaves open.

  riskoff   --dir DIR --cell C --seeds A-B    W4-02 (broad dollar, RiskOff 14), W4-03 (yen, 3 months)
  uscred    --dir DIR --cell C --seeds A-B    W4-04 (dollar half, UsCredibility; the 10-year half in bp at h = 1..3, cell F)
  china     --dir DIR --cell C --seeds A-B    W4-15, W4-16 (China shortfall), W4-17 (ECB, read from the ecb100 cell)
  pol100l   --dir DIR --cell C --seeds A-B    W4-07, W4-08 (F): month 24 per pp of the mean US policy gap over months 1-12
  fund      --dir DIR --cell C --seeds A-B    W4-09 (pol100): US equity, fund share price, impact
  dollarm   --dir DIR --cell C --seeds A-B    W4-19 (R): US net exports per 1 % dollar move, months 24-36 (dollarm10)
  irf26     --dir DIR --cell C --matched M --seeds A-B   W4-26 P1-P7 (pol100): seed-mean IRF gates P1-P5, P2; P7 paired vs M
"""
import argparse, gzip, json, math, os, sys
import numpy as np

AE = [0, 2, 3, 4, 9]          # EA, JP, UK, CA, AU  (AE5)
EME = [1, 5, 6, 7, 8]         # CN, MX, IN, KR, BR  (EME5, D56)


def seeds_of(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def load(d, cell, scen, s):
    path = os.path.join(d, '%s_%s' % (cell, scen), '%s_s%d.json.gz' % (scen, s))
    with gzip.open(path, 'rt') as f:
        r = json.load(f)
    return r['baseline'], r['shocked']


def se(x):
    x = np.asarray(x, float)
    return float(x.std(ddof=1) / math.sqrt(len(x))) if len(x) > 1 else float('nan')


def show(label, vals, band, kind):
    # a single value is a statistic of the seed-mean path: its SE is not defined here and is printed as n/a
    v = np.asarray(vals, float)
    if band is None:
        tag = 'reported'
        bt = 'reported'
    else:
        ok = band[0] <= v.mean() <= band[1]
        tag = 'PASS' if ok else 'FAIL'
        bt = '[%g, %g]' % band
    sev = se(v) if len(v) > 1 else float('nan')
    print('%-62s %s %9.3f (SE %s) band %s %s' % (label, kind, v.mean(), ('%.3f' % sev) if len(v) > 1 else 'n/a', bt, tag))


def dlog_fx(d, cell, scen, seeds):
    out = []
    for s in seeds:
        b, sh = load(d, cell, scen, s)
        out.append(100.0 * (np.log([r['world']['fx_broad'] for r in sh]) - np.log([r['world']['fx_broad'] for r in b])))
    return np.array(out)


def paired_gdp_pct(d, cell, scen, seeds):
    out = []
    for s in seeds:
        b, sh = load(d, cell, scen, s)
        g0 = np.array([r['gdp_real_bn_ar'] for r in b], float)
        g1 = np.array([r['gdp_real_bn_ar'] for r in sh], float)
        out.append(100.0 * (g1 - g0) / g0)
    return np.array(out)


def paired(d, cell, scen, seeds, fn):
    out = []
    for s in seeds:
        b, sh = load(d, cell, scen, s)
        out.append(np.array([fn(r) for r in sh], float) - np.array([fn(r) for r in b], float))
    return np.array(out)


def cmd_riskoff(a):
    seeds = seeds_of(a.seeds)
    dl = dlog_fx(a.dir, a.cell, 'riskoff14', seeds)
    show('W4-02 impact (h 1): broad dollar %', dl[:, 0], (2.0, 4.0), 'F')
    show('W4-02 6-month (h 6): broad dollar %', dl[:, 5], None, 'R (band [2.0, 5.0] reported)')
    show('W4-02 12-month (h 12): broad dollar %', dl[:, 11], (-0.5, 2.5), 'F')
    jp = []
    for s in seeds:
        b, sh = load(a.dir, a.cell, 'riskoff14', s)
        jp.append(100.0 * (math.log(sh[2]['world']['s_bil'][2]) - math.log(b[2]['world']['s_bil'][2])))
    show('W4-03 yen: USD/JPY change, 3 months (%, paired)', jp, (-3.0, 1.0), 'G')


def cmd_uscred(a):
    seeds = seeds_of(a.seeds)
    dc = dlog_fx(a.dir, a.cell, 'uscred', seeds)
    show('W4-04 dollar, impact (h 1): broad dollar %', dc[:, 0], None, 'F (reported)')
    show('W4-04 dollar, 3 months (h 3): broad dollar %', dc[:, 2], (-4.0, -2.5), 'F')
    y = paired(a.dir, a.cell, 'uscred', seeds, lambda r: r['yields'][3])
    for h in (1, 2, 3):
        show('W4-04 10-year, h %d (bp, paired)' % h, 100.0 * y[:, h - 1], (10.0, 30.0), 'G (sign gate)')


def cmd_china(a):
    seeds = seeds_of(a.seeds)
    gc = paired_gdp_pct(a.dir, a.cell, 'china3', seeds)
    gap = paired(a.dir, a.cell, 'china3', seeds, lambda r: r['world']['y_gap'][1])
    dc = dlog_fx(a.dir, a.cell, 'china3', seeds)
    mg, mgap, md = gc.mean(axis=0), gap.mean(axis=0), dc.mean(axis=0)
    t = 11 + int(np.argmin(mg[11:36]))
    p = 11 + int(np.argmax(md[11:36]))
    k = 11 + int(np.argmin(mgap[11:36]))
    print('   china3 mean paths: US GDP trough %.3f%% at month %d (CN gap %.3f pp); dollar peak %.3f%% at month %d (CN gap %.3f pp)' % (
        mg[t], t + 1, mgap[t], md[p], p + 1, mgap[p]))
    show('W4-15 US GDP per 1 % China shortfall (GDP trough / |CN trough|, def 3) [assumed]',
         [mg[t] / abs(mgap[k])], (-0.35, -0.07), 'F')
    show('W4-16 broad dollar per 1 % China shortfall (dollar peak / |CN trough|, def 3) [assumed]',
         [md[p] / abs(mgap[k])], (0.75, 1.75), 'F')
    show('W4-15 (S4-R12) US GDP trough m12-36 / |China trough, full path|',
         [mg[11:36].min() / abs(mgap.min())], (-0.35, -0.07), 'F')
    show('W4-16 (S4-R12) impact-month dollar peak / |China trough, full path|',
         [md[0] / abs(mgap.min())], (0.75, 1.75), 'F')
    ge = paired_gdp_pct(a.dir, a.cell, 'ecb100', seeds)
    show('W4-17 US GDP trough, months 1-36 (%, per seed)', [ge[i, :36].min() for i in range(len(seeds))], None, 'R')


def cmd_pol100l(a):
    seeds = seeds_of(a.seeds)
    pg = paired(a.dir, a.cell, 'pol100l', seeds, lambda r: r['policy_rate'])
    gap_mean = pg[:, 0:12].mean(axis=1)
    dAE, dEM = [], []
    for s in seeds:
        b, sh = load(a.dir, a.cell, 'pol100l', s)
        dAE.append(np.mean([100.0 * (math.log(sh[23]['world']['s_bil'][j]) - math.log(b[23]['world']['s_bil'][j])) for j in AE]))
        dEM.append(np.mean([100.0 * (math.log(sh[23]['world']['s_bil'][j]) - math.log(b[23]['world']['s_bil'][j])) for j in EME]))
    dAE, dEM = np.array(dAE), np.array(dEM)
    show('W4-07 (F, pol100l) AE5 currencies per pp of US policy gap, month 24 (%)', dAE / gap_mean, (2.0, 10.0), 'F')
    show('W4-08 (F, pol100l) EME5 currencies per pp of US policy gap, month 24 (%)', dEM / gap_mean, (2.0, 16.0), 'F')


def cmd_fund(a):
    seeds = seeds_of(a.seeds)
    fund = []
    for s in seeds:
        b, sh = load(a.dir, a.cell, 'pol100', s)
        fund.append(100.0 * (sh[0]['fund_share_price'] / b[0]['fund_share_price'] - 1.0))
    show('W4-09 fund share price, impact (month 1, %)', fund, (-12.0, -5.0), 'F')


def cmd_dollarm(a):
    seeds = seeds_of(a.seeds)
    per, dd = [], []
    for s in seeds:
        b, sh = load(a.dir, a.cell, 'dollarm10', s)
        nx_s = np.array([(r['exports_bn_ar'] - r['imports_bn_ar']) / r['gdp_nominal_bn_ar'] for r in sh[23:36]])
        nx_b = np.array([(r['exports_bn_ar'] - r['imports_bn_ar']) / r['gdp_nominal_bn_ar'] for r in b[23:36]])
        per.append(100.0 * (nx_s - nx_b).mean())
        dd.append(100.0 * math.log(sh[23]['dollar_index'] / b[23]['dollar_index']))
    per, dd = np.array(per), np.array(dd)
    print('   dollarm10 net exports diff (pp of GDP, mean months 24-36) %.3f (SE %.3f); dollar at m24 %.2f %%' % (per.mean(), se(per), dd.mean()))
    if dd.mean():
        print('   per 1 %% dollar depreciation (sign flipped): %.3f pp of GDP (reported band 0.05-0.20)' % (-per.mean() / (-dd.mean())))


def cmd_irf26(a):
    seeds = seeds_of(a.seeds)
    H = [3, 6, 12, 24]
    ix = lambda hs: [h - 1 for h in hs]
    gL = paired_gdp_pct(a.dir, a.cell, 'pol100', seeds)
    uL = paired(a.dir, a.cell, 'pol100', seeds, lambda r: r['unemployment_rate'])
    yL = paired(a.dir, a.cell, 'pol100', seeds, lambda r: r['yields'][3])
    cL = paired(a.dir, a.cell, 'pol100', seeds, lambda r: r['core_pce_inflation_yoy'])
    invL = np.array([[100.0 * (sh[t]['investment_real_bn_ar'] / b[t]['investment_real_bn_ar'] - 1.0) for t in range(60)]
                     for b, sh in [load(a.dir, a.cell, 'pol100', s) for s in seeds]])
    # seed-mean IRF (D56 vi): the statistics of the mean path
    mg, mu, mc, my, mi = gL.mean(axis=0), uL.mean(axis=0), cL.mean(axis=0), yL.mean(axis=0), invL.mean(axis=0)
    P1 = mg[ix(H)].min()
    P2 = mi[ix(H)].min() / P1
    P3 = mu[ix(H)].max()
    P4 = mc[ix([6, 12, 24])].min()
    P5 = my[2]
    inv3 = mi[2]
    print('W4-26 seed-mean IRF (gated, D56 vi), %d seeds, cell %s, pol100 (100bp, 12 months)' % (len(seeds), a.cell))
    print('  P1 GDP trough, sample months 3,6,12,24: %.3f%%  band [-1.6, -0.4] %s' % (P1, 'PASS' if -1.6 <= P1 <= -0.4 else 'FAIL'))
    print('  P2 investment trough / GDP trough: %.3f; inv m3 %.3f  band [2.0, 5.0] and inv m3 <= -0.3: %s' % (
        P2, inv3, 'PASS' if (2.0 <= P2 <= 5.0 and inv3 <= -0.3) else 'FAIL'))
    print('  P3 unemployment peak, sample months: %.3f pp  band [0.2, 1.0] %s' % (P3, 'PASS' if 0.2 <= P3 <= 1.0 else 'FAIL'))
    print('  P4 core inflation trough m6-m24: %.3f pp  sign gate <= -0.03 %s' % (P4, 'PASS' if P4 <= -0.03 else 'FAIL'))
    print('  P5 10-year at month 3: %.3f pp  band [0.15, 0.45] %s' % (P5, 'PASS' if 0.15 <= P5 <= 0.45 else 'FAIL'))
    # per-seed values, reported beside the gates
    p1s = np.array([gL[i, ix(H)].min() for i in range(len(seeds))])
    p3s = np.array([uL[i, ix(H)].max() for i in range(len(seeds))])
    p5s = yL[:, 2]
    print('  per seed (reported): P1 mean %.3f (SE %.3f); P3 mean %.3f (SE %.3f); P5 mean %.3f (SE %.3f)' % (
        p1s.mean(), se(p1s), p3s.mean(), se(p3s), p5s.mean(), se(p5s)))
    # P6 reported
    dol = np.array([[100.0 * (sh[t]['dollar_index'] / b[t]['dollar_index'] - 1.0) for t in range(60)]
                    for b, sh in [load(a.dir, a.cell, 'pol100', s) for s in seeds]])
    pol = paired(a.dir, a.cell, 'pol100', seeds, lambda r: r['policy_rate'])
    P6 = dol.mean(axis=0)[ix([3, 6, 12])].max() / pol.mean(axis=0)[ix([3, 6, 12])].max()
    print('  P6 dollar peak / policy peak, m3-m12 (floor >= 1.2, R): %.3f' % P6)
    # P7: GDP overshoot after month 24 over |P1 trough| (per seed), paired against the matched cell
    P7 = np.array([max(0.0, gL[i, 24:60].max()) / abs(gL[i, ix(H)].min()) for i in range(len(seeds))])
    gE = paired_gdp_pct(a.dir, a.matched, 'pol100', seeds)
    P7e = np.array([max(0.0, gE[i, 24:60].max()) / abs(gE[i, ix(H)].min()) for i in range(len(seeds))])
    dP7 = P7 - P7e
    n = len(dP7)
    jk = [np.mean(np.delete(dP7, i)) for i in range(n)]
    se_jk = math.sqrt((n - 1) / n * sum((x - np.mean(jk)) ** 2 for x in jk))
    limit = max(0.5, P7e.mean() + 2 * se_jk)
    print('  P7 overshoot / |P1 trough|: L %.3f, matched %s %.3f, paired difference %.3f (jackknife SE %.3f); limit max(0.5, E + 2 SE) = %.3f: %s' % (
        P7.mean(), a.matched, P7e.mean(), dP7.mean(), se_jk, limit, 'PASS' if P7.mean() <= limit else 'FAIL'))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    for name, fn in (('riskoff', cmd_riskoff), ('uscred', cmd_uscred), ('china', cmd_china), ('pol100l', cmd_pol100l),
                     ('fund', cmd_fund), ('dollarm', cmd_dollarm)):
        p = sub.add_parser(name)
        p.add_argument('--dir', required=True); p.add_argument('--cell', required=True); p.add_argument('--seeds', required=True)
        p.set_defaults(func=fn)
    q = sub.add_parser('irf26')
    q.add_argument('--dir', required=True); q.add_argument('--cell', required=True); q.add_argument('--matched', required=True)
    q.add_argument('--seeds', required=True)
    q.set_defaults(func=cmd_irf26)
    a = ap.parse_args()
    a.func(a)


if __name__ == '__main__':
    main()
