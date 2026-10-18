#!/usr/bin/env python3
"""scripts/world_s5.py: analysis commands of stage 5 (spec rev 11). Reads the files written by scripts/world_runs.py; decides nothing by itself
except where a band is printed. Horizon convention (spec 3.0): month h after a shock at reported month 0 is record index h - 1.

  gap-offset  --rundir DIR --cell C --seeds A-B [--from 13] [--expect v1,..,v12]   D57: per-bloc mean of world.us_import_gap over months FROM..239
  shifts      --rundir DIR --base C0 --cells C1,C2,.. --seeds A-B                  W5-07 (R): paired baseline shifts, months 12-239
  w504        --dir DIR --cell C --scenario SC --seeds A-B [--bloc CN]             tariff-preset rows (spec 5): trade, duty, prices, output
  w407        --dir DIR --cell C --scenario SC --seeds A-B [--month 24]           W4-07 / W4-08 (F, scenario pol100l, S5-R5) per pp of the mean policy gap over months 1-12; a pin scenario (pin7, pin10) prints W4-39 (R) only
  w210        --dir DIR --cell C --scenario pol100 --seeds A-B                     W2-10 restated (R): through-origin slope, months 1-12
  tariff-eq   --dir DIR --cell C --a tar10 --b tarU10 --seeds A-B                  W5-06: ImportTariff against Tariff{null,null}
  binom       --rundir DIR --link C --matched C --seeds A-B                        exact one-sided binomial p of the G2 spike counts (printed beside the S4-R32 rule)
  fo-drift    --rundir DIR --cell-on C --cell-off C --seeds A-B                    S4-R29: tp_fo_bp and executed official flow, dollar link on against off
Long-run files are DIR/CELL_sSEED.json.gz (last JSON line of `econsim-cli run`); event files are DIR/CELL_SCENARIO/SCENARIO_sSEED.json.gz (a compare response)."""
import argparse, gzip, json, math, os, sys
import numpy as np

BLOC = ['EA', 'CN', 'JP', 'UK', 'CA', 'MX', 'IN', 'KR', 'BR', 'AU', 'OPEC+', 'RoW']
AE5 = [0, 2, 3, 4, 9]
EME5 = [1, 5, 6, 7, 8]          # CN, MX, IN, KR, BR (D56: the harness EME5)


def seeds_of(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def load_long(rundir, cell, s):
    with gzip.open(os.path.join(rundir, '%s_s%d.json.gz' % (cell, s)), 'rt') as f:
        return json.loads(f.read())


def load_ev(d, cell, scen, s):
    with gzip.open(os.path.join(d, '%s_%s' % (cell, scen), '%s_s%d.json.gz' % (scen, s)), 'rt') as f:
        r = json.load(f)
    return r['baseline'], r['shocked']


def se(x):
    x = np.asarray(x, float)
    return float(x.std(ddof=1) / math.sqrt(len(x))) if len(x) > 1 else float('nan')


def cmd_gap_offset(a):
    M = []
    for s in seeds_of(a.seeds):
        g = np.array([r['world']['us_import_gap'] for r in load_long(a.rundir, a.cell, s)])
        M.append(g[a.from_month:240].mean(axis=0))
    M = np.array(M)
    mu = M.mean(axis=0)
    sem = np.array([se(M[:, j]) for j in range(12)])
    print('us_import_gap, months %d-239, cell %s, seeds %s (n=%d)' % (a.from_month, a.cell, a.seeds, len(M)))
    print('offset = [%s]' % ', '.join('%.3f' % x for x in mu))
    print('SE     = [%s]' % ', '.join('%.3f' % x for x in sem))
    if a.expect:
        want = np.array([float(x) for x in a.expect.split(',')])
        z = (mu - want) / np.maximum(sem, 1e-9)
        bad = [BLOC[j] for j in range(12) if abs(z[j]) > 2.0]
        print('pre-registered = [%s]; z = [%s]' % (', '.join('%.3f' % x for x in want), ', '.join('%.1f' % x for x in z)))
        print('S4-R10 trigger (|z| > 2): %s' % (('FIRED for ' + ', '.join(bad)) if bad else 'not fired'))
        sys.exit(1 if bad else 0)


SHIFT_KEYS = [('policy_rate', 'pp'), ('unemployment_rate', 'pp'), ('core_pce_inflation_yoy', 'pp'), ('gdp_real_growth_yoy', 'pp'), ('y10', 'pp'),
              ('mortgage_rate', 'pp'), ('hpi', '%'), ('fund_share_price', '%'), ('bank_capital_ratio', 'pt'), ('gov_debt_gdp', 'pt'),
              ('profit_share', 'pt'), ('dollar_index', '%'), ('exports_bn_ar', '%'), ('imports_bn_ar', '%'), ('zb', 'months')]


def _series(rec, key):
    if key == 'y10':
        return np.array([r['yields'][3] for r in rec], float)
    if key == 'zb':
        return (np.array([r['policy_rate'] for r in rec], float) <= 0.25).astype(float)
    return np.array([r[key] for r in rec], float)


def cmd_shifts(a):
    seeds = seeds_of(a.seeds)
    base = {s: load_long(a.rundir, a.base, s) for s in seeds}
    cells = a.cells.split(',')
    out = {}
    for c in cells:
        for s in seeds:
            rec = load_long(a.rundir, c, s)
            for k, u in SHIFT_KEYS:
                x, y = _series(rec, k)[12:240], _series(base[s], k)[12:240]
                v = 100.0 * (x.mean() / y.mean() - 1.0) if u == '%' else (x.sum() - y.sum() if k == 'zb' else x.mean() - y.mean())
                out.setdefault((c, k), []).append(v)
    print('paired baseline shift (cell - %s), months 12-239, seeds %s (n=%d); mean (SE)' % (a.base, a.seeds, len(seeds)))
    print('%-26s %-7s' % ('series', 'unit') + ''.join('%-22s' % c for c in cells))
    for k, u in SHIFT_KEYS:
        print('%-26s %-7s' % (k, u) + ''.join('%+8.3f (%5.3f)      ' % (np.mean(out[(c, k)]), se(out[(c, k)])) for c in cells))


def cmd_w504(a):
    J = BLOC.index(a.bloc)
    rows, gap = {}, []
    add = lambda k, v: rows.setdefault(k, []).append(v)
    for s in seeds_of(a.seeds):
        B, S = load_ev(a.dir, a.cell, a.scenario, s)
        pct = lambda key, h: 100.0 * (S[h - 1][key] / B[h - 1][key] - 1.0)
        wp = lambda key, h: 100.0 * (S[h - 1]['world'][key][J] / B[h - 1]['world'][key][J] - 1.0)
        add('imports from %s m12 %%' % a.bloc, wp('imports_bn', 12))
        add('goods exports to %s m12 %%' % a.bloc, wp('exports_goods_bn', 12))
        add('total exports to %s m12 %%' % a.bloc, wp('exports_bn', 12))
        rt = S[11]['world']['ret_tau'][J]
        add('ret_tau m12', rt)
        add('goods exports elasticity per ln(1+ret_tau) m12', math.log(S[11]['world']['exports_goods_bn'][J] / B[11]['world']['exports_goods_bn'][J]) / math.log(1 + rt) if rt > 0 else float('nan'))
        add('CPI yoy diff m12 pp', S[11]['inflation_yoy'] - B[11]['inflation_yoy'])
        add('real GDP m12 %', pct('gdp_real_bn_ar', 12))
        add('real GDP m24 %', pct('gdp_real_bn_ar', min(24, len(S))))
        add('unemployment m12 pp', S[11]['unemployment_rate'] - B[11]['unemployment_rate'])
        add('broad dollar m3 %', pct('dollar_index', 3))
        add('broad dollar m12 %', pct('dollar_index', 12))
        gap.append([S[h]['world']['y_gap'][J] - B[h]['world']['y_gap'][J] for h in range(min(24, len(S)))])
    print('%s %s seeds %s (n=%d), partner %s' % (a.cell, a.scenario, a.seeds, len(gap), a.bloc))
    for k, v in rows.items():
        v = np.array(v, float)
        print('  %-48s %+9.3f (SE %.3f)' % (k, np.nanmean(v) if not np.all(np.isnan(v)) else float('nan'), se(v[~np.isnan(v)]) if np.any(~np.isnan(v)) else float('nan')))
    P = np.mean(gap, axis=0)
    print('  %s output-gap trough, min of the mean path over months 1-24: %+.3f at month %d' % (a.bloc, P.min(), int(P.argmin()) + 1))


def cmd_w407(a):
    H = a.month
    r_ae, r_em, gaps = [], [], []
    for s in seeds_of(a.seeds):
        B, S = load_ev(a.dir, a.cell, a.scenario, s)
        gap = np.mean([S[h]['policy_rate'] - B[h]['policy_rate'] for h in range(12)])
        ch = lambda grp: np.mean([100.0 * (math.log(S[H - 1]['world']['s_bil'][j]) - math.log(B[H - 1]['world']['s_bil'][j])) for j in grp])
        gaps.append(gap)
        r_ae.append(ch(AE5) / gap)
        r_em.append(ch(EME5) / gap)
    print('%s %s seeds %s, month %d: mean policy gap over months 1-12 %.3f pp' % (a.cell, a.scenario, a.seeds, H, np.mean(gaps)))
    ok = lambda v, lo, hi: 'PASS' if lo <= v <= hi else 'FAIL'
    if a.scenario.startswith('pin'):
        # S5-R5: the pins are reported only, as W4-39 (R); no band applies and no pin is a gate
        print('  W4-39 (R, reported) AE5 per pp: %.3f (SE %.3f), no band' % (np.mean(r_ae), se(r_ae)))
        print('  W4-39 (R, reported) EME5 per pp: %.3f (SE %.3f), no band' % (np.mean(r_em), se(r_em)))
    else:
        # S5-R5: W4-07 and W4-08 stay on pol100l (F; a failure is recorded and gates nothing)
        print('  W4-07 (F) AE5 per pp: %.3f (SE %.3f) band [2, 10] %s' % (np.mean(r_ae), se(r_ae), ok(np.mean(r_ae), 2, 10)))
        print('  W4-08 (F) EME5 per pp: %.3f (SE %.3f) band [2, 16] %s' % (np.mean(r_em), se(r_em), ok(np.mean(r_em), 2, 16)))


def cmd_w210(a):
    G, I = [], []
    for s in seeds_of(a.seeds):
        B, S = load_ev(a.dir, a.cell, a.scenario, s)
        G.append([100.0 * (S[h]['gdp_real_bn_ar'] / B[h]['gdp_real_bn_ar'] - 1.0) for h in range(12)])
        I.append([100.0 * (S[h]['imports_bn_ar'] / B[h]['imports_bn_ar'] - 1.0) for h in range(12)])
    G, I = np.array(G), np.array(I)
    sl = (G * I).sum() / (G * G).sum()
    jk = [(np.delete(G, k, 0) * np.delete(I, k, 0)).sum() / (np.delete(G, k, 0) ** 2).sum() for k in range(len(G))]
    sej = math.sqrt((len(jk) - 1) / len(jk) * sum((x - np.mean(jk)) ** 2 for x in jk))
    print('W2-10 restated (R): through-origin slope of the paired import response on the paired GDP response, months 1-12: %.3f (jackknife SE %.3f); reported band [1.5, 2.3]' % (sl, sej))


TE_ROWS = [('gdp_real_bn_ar', '%', 0.10), ('cpi', '%', 0.10), ('inflation_yoy', 'pp', 0.05), ('core_pce_inflation_yoy', 'pp', 0.05), ('unemployment_rate', 'pp', 0.05),
           ('policy_rate', 'pp', 0.05), ('consumption_real_bn_ar', '%', 0.10), ('dollar_index', '%', 0.20)]


def cmd_tariff_eq(a):
    worst, bad = 0.0, []
    for key, unit, floor in TE_ROWS:
        for h in (3, 12, 24):
            va, vb = [], []
            for tag, acc in ((a.a, va), (a.b, vb)):
                for s in seeds_of(a.seeds):
                    B, S = load_ev(a.dir, a.cell, tag, s)
                    acc.append(100.0 * (S[h - 1][key] / B[h - 1][key] - 1.0) if unit == '%' else S[h - 1][key] - B[h - 1][key])
            d = abs(np.mean(va) - np.mean(vb))
            worst = max(worst, d / floor)
            if d > 0.5 * floor:
                bad.append((key, h, d, floor))
    print('W5-06: %s against %s, %d seeds; largest |difference| as a share of the indicator\'s site floor: %.2f (rule <= 0.5): %s' % (
        a.a, a.b, len(seeds_of(a.seeds)), worst, 'PASS' if not bad else 'FAIL ' + str(bad)))
    sys.exit(0 if not bad else 1)


def cmd_binom(a):
    seeds = seeds_of(a.seeds)
    spikes = lambda cell: sum(1 for s in seeds if np.max(np.abs(np.diff(np.log([r['gdp_real_bn_ar'] for r in load_long(a.rundir, cell, s)])[12:]))) > 0.03)
    L, E = spikes(a.link), spikes(a.matched)
    n = L + E
    p = sum(math.comb(n, k) for k in range(L, n + 1)) / 2 ** n if n else 1.0
    print('G2 spike seeds (one-month |d ln GDP| > 3 %% after month 12), %d seeds: link %d, matched %d; rule L <= E + 1: %s; exact one-sided binomial p(L >= %d | n = %d) = %.3f (printed, not a gate)' % (
        len(seeds), L, E, 'PASS' if L <= E + 1 else 'FAIL', L, n, p))


def cmd_fo_drift(a):
    seeds = seeds_of(a.seeds)
    for rec_i in (11, 59, 119, 239):
        d = [load_long(a.rundir, a.cell_on, s)[rec_i]['world']['tp_fo_bp'] - load_long(a.rundir, a.cell_off, s)[rec_i]['world']['tp_fo_bp'] for s in seeds]
        print('tp_fo_bp record %3d: on - off = %+.2f bp (SE %.2f)' % (rec_i, np.mean(d), se(d)))
    f = lambda c, s: np.mean([r['world']['fo_flow_bn'] for r in load_long(a.rundir, c, s)[12:240]])
    d = [f(a.cell_on, s) - f(a.cell_off, s) for s in seeds]
    print('executed official flow, mean over months 12-239: on - off = %+.3f bn a month (SE %.3f)' % (np.mean(d), se(d)))


# ---------------------------------------------------------------- stage 5 oil early gates (D59; S5-o)
# Shock at reported month 12 (record index 12 = h 1): month h is record index 11 + h. Event files of cell `To` (oil link on, s5a + oil)
# and of the legacy cell `Lo` (block off, stochastic_oil off) are read from DIR/CELL_SCENARIO as elsewhere in this file.

def _oil_paths(d, cell, scen, seeds, fn):
    """seeds x months array: shocked minus baseline of fn(record), per seed (paired inside compare)."""
    out = []
    for s in seeds:
        B, S = load_ev(d, cell, scen, s)
        out.append([fn(x) - fn(y) for x, y in zip(S, B)])
    return np.array(out, float)


def _oil_pct(d, cell, scen, seeds, fn):
    out = []
    for s in seeds:
        B, S = load_ev(d, cell, scen, s)
        out.append([100.0 * (fn(x) / fn(y) - 1.0) for x, y in zip(S, B)])
    return np.array(out, float)


def _verdict(v, lo, hi):
    return 'PASS' if lo <= v <= hi else 'FAIL'


def cmd_oil_w311(a):
    seeds = seeds_of(a.seeds)
    cpi = _oil_paths(a.dir, a.cell, 'oil50', seeds, lambda r: r['inflation_yoy']).mean(axis=0)
    core = _oil_paths(a.dir, a.cell, 'oil50', seeds, lambda r: r['core_pce_inflation_yoy']).mean(axis=0)
    gdp = _oil_pct(a.dir, a.cell, 'oil50', seeds, lambda r: r['gdp_real_bn_ar']).mean(axis=0)
    o1 = gdp[17:36].min()          # h 6..24
    o2 = cpi[14:24].max()          # h 3..12
    o3 = core[14:24].max()         # h 3..12
    print('W3-11 early gate: oil +50 %% (half-life 12) at month 12, cell %s, %d seeds (mean paths)' % (a.cell, len(seeds)))
    print('  CPI m12 (pp)                      %8.3f  band [1.0, 1.4] %s' % (cpi[23], _verdict(cpi[23], 1.0, 1.4)))
    print('  CPI m3 (pp)                       %8.3f  band [0.9, 1.6] %s (W3-11 m3, reported with the row)' % (cpi[14], _verdict(cpi[14], 0.9, 1.6)))
    print('  O1 GDP trough m6-m24 (%%)            %8.3f  band [-1.5, -0.25] %s' % (o1, _verdict(o1, -1.5, -0.25)))
    print('  O2 headline CPI peak m3-m12 (pp)  %8.3f  band [1.0, 1.8] %s' % (o2, _verdict(o2, 1.0, 1.8)))
    print('  O3 core peak m3-m12 (pp, floor <= 0.5, R)  %8.3f  %s' % (o3, 'PASS' if o3 <= 0.5 else 'FLOOR-FAIL'))
    gates = [_verdict(cpi[23], 1.0, 1.4), _verdict(o1, -1.5, -0.25), _verdict(o2, 1.0, 1.8)]
    print('  W3-11 gating rows (CPI m12, O1, O2): %s' % ('ALL PASS' if all(g == 'PASS' for g in gates) else 'FAIL'))


def cmd_oil_w310(a):
    seeds = seeds_of(a.seeds)
    on = _oil_paths(a.dir, a.on_cell, 'oil50', seeds, lambda r: r['inflation_yoy'])
    off = _oil_paths(a.dir, a.off_cell, 'oil50', seeds, lambda r: r['inflation_yoy'])
    d = on - off
    print('W3-10 early gate (feedback off, exact path, paired %d seeds): CPI yoy, %s against legacy %s' % (len(seeds), a.on_cell, a.off_cell))
    for name, h in (('m3', 3), ('m12', 12), ('m24', 24)):
        idx = 11 + h   # record index T0 + h - 1 with T0 = 12
        m = d[:, idx].mean()
        se_ = se(d[:, idx])
        ok = abs(m) <= 0.05 + 2.0 * se_
        print('  %-4s difference %+8.3f pp (SE %.3f); tolerance 0.05 + 2 SE = %.3f: %s' % (name, m, se_, 0.05 + 2 * se_, 'PASS' if ok else 'FAIL'))


def cmd_oil_w308(a):
    seeds = seeds_of(a.seeds)
    price = _oil_pct(a.dir, a.cell, 'opec2', seeds, lambda r: r['world']['brent_usd']).mean(axis=0)
    out = _oil_paths(a.dir, a.cell, 'opec2', seeds, lambda r: r['world']['opec_out_mbd']).mean(axis=0)
    p6, o6 = price[17], out[17]          # month 6 after the cut at month 12: record 17
    print('W3-08 early gate (ABM stochastic row): OpecCut 2.0 at month 12, cell %s, %d seeds (mean paths), month 6' % (a.cell, len(seeds)))
    print('  Brent change (%%)                     %8.3f  band [4, 9] %s' % (p6, _verdict(p6, 4.0, 9.0)))
    print('  OPEC+ output change (mb/d)            %8.3f  band [-2.0, -1.4] %s' % (o6, _verdict(o6, -2.0, -1.4)))



def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    g = sub.add_parser('gap-offset'); g.add_argument('--rundir', required=True); g.add_argument('--cell', required=True); g.add_argument('--seeds', required=True)
    g.add_argument('--from', type=int, default=13, dest='from_month'); g.add_argument('--expect', default='')
    s = sub.add_parser('shifts'); s.add_argument('--rundir', required=True); s.add_argument('--base', required=True); s.add_argument('--cells', required=True); s.add_argument('--seeds', required=True)
    for name in ('w504', 'w407', 'w210'):
        p = sub.add_parser(name); p.add_argument('--dir', required=True); p.add_argument('--cell', required=True); p.add_argument('--scenario', required=True); p.add_argument('--seeds', required=True)
        if name == 'w504':
            p.add_argument('--bloc', default='CN')
        if name == 'w407':
            p.add_argument('--month', type=int, default=24)
    t = sub.add_parser('tariff-eq'); t.add_argument('--dir', required=True); t.add_argument('--cell', required=True); t.add_argument('--a', default='tar10'); t.add_argument('--b', default='tarU10'); t.add_argument('--seeds', required=True)
    b = sub.add_parser('binom'); b.add_argument('--rundir', required=True); b.add_argument('--link', required=True); b.add_argument('--matched', required=True); b.add_argument('--seeds', required=True)
    f = sub.add_parser('fo-drift'); f.add_argument('--rundir', required=True); f.add_argument('--cell-on', required=True, dest='cell_on'); f.add_argument('--cell-off', required=True, dest='cell_off'); f.add_argument('--seeds', required=True)
    o = sub.add_parser('oil-w311'); o.add_argument('--dir', required=True); o.add_argument('--cell', required=True); o.add_argument('--seeds', required=True)
    o2 = sub.add_parser('oil-w310'); o2.add_argument('--dir', required=True); o2.add_argument('--on-cell', required=True, dest='on_cell'); o2.add_argument('--off-cell', required=True, dest='off_cell'); o2.add_argument('--seeds', required=True)
    o3 = sub.add_parser('oil-w308'); o3.add_argument('--dir', required=True); o3.add_argument('--cell', required=True); o3.add_argument('--seeds', required=True)
    a = ap.parse_args()
    {'gap-offset': cmd_gap_offset, 'shifts': cmd_shifts, 'w504': cmd_w504, 'w407': cmd_w407, 'w210': cmd_w210, 'tariff-eq': cmd_tariff_eq, 'binom': cmd_binom, 'fo-drift': cmd_fo_drift,
     'oil-w311': cmd_oil_w311, 'oil-w310': cmd_oil_w310, 'oil-w308': cmd_oil_w308}[a.cmd](a)


if __name__ == '__main__':
    main()
