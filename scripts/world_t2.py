#!/usr/bin/env python3
"""scripts/world_t2.py: harness commands of sub-round 5-T2 (spec rev 12, D62-D64). Nothing here decides a row on its own; the rows are read from the printed lines.

  relprice   --rundir DIR --cell-on CELL --cell-off CELL --seeds A-B
             W2-18 (R) and the step-0 measurement of g (D62): per seed the OLS slope (per year, months 12-239) of 100 ln(import_price_index / PT), PT the
             `tradable_weight` role mean of `sector_prices` ([0, 0.3, 1, 1, 0.3, 0, 0, 0, 0]); the paired difference on - off over seeds (mean, SE), the mean level
             difference, the household import share slope, and the paired imports / GDP and exports / GDP means (W2-15 dM and dX, own formula). When the `on` cell is the
             block cell WITHOUT the drift (cell `T` of stage 5), g = difference / 100.
  tariff-bit --dir DIR --cell CELL --a CODE --b CODE --seeds A-B
             W5-06 (E): of the event files CELL_CODE/CODE_sSEED.json.gz, the number of seeds for which both arms of the two codes are identical in every key of every record
             (the `world` block included); the old statistic (largest difference of the paired means over eight indicators at months 3, 12, 24 as a share of the site floor)
             is printed beside it. Exit 1 when a seed differs.

Run files are DIR/CELL_sSEED.json.gz (long runs) and DIR/CELL_CODE/CODE_sSEED.json.gz (compare responses with `baseline` and `shocked`), as in world_runs.py."""
import argparse, gzip, json, math, os, sys
import numpy as np

TW = np.array([0.0, 0.3, 1.0, 1.0, 0.3, 0.0, 0.0, 0.0, 0.0])


def seeds_of(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def load_long(rundir, cell, s):
    with gzip.open(os.path.join(rundir, '%s_s%d.json.gz' % (cell, s)), 'rt') as f:
        return json.loads(f.read())


def load_ev(d, cell, code, s):
    with gzip.open(os.path.join(d, '%s_%s' % (cell, code), '%s_s%d.json.gz' % (code, s)), 'rt') as f:
        r = json.load(f)
    return r['baseline'], r['shocked']


def mean_se(x):
    x = np.asarray(x, float)
    return float(x.mean()), float(x.std(ddof=1) / math.sqrt(len(x))) if len(x) > 1 else float('nan')


def cmd_relprice(a):
    seeds = seeds_of(a.seeds)
    t = np.arange(12, 240) / 12.0
    sl = {a.cell_on: [], a.cell_off: []}
    sh = {a.cell_on: [], a.cell_off: []}
    lvl, mg, xg = [], {a.cell_on: [], a.cell_off: []}, {a.cell_on: [], a.cell_off: []}
    for s in seeds:
        y = {}
        for c in (a.cell_on, a.cell_off):
            h = load_long(a.rundir, c, s)
            imp = np.array([m['import_price_index'] for m in h], float)
            pt = (np.array([m['sector_prices'] for m in h], float) * TW).sum(axis=1) / TW.sum()
            y[c] = 100.0 * (np.log(imp) - np.log(pt))
            sl[c].append(np.polyfit(t, y[c][12:240], 1)[0])
            share = np.array([m['hh_import_share_pct'] for m in h[12:240]], float)
            sh[c].append(np.polyfit(t, share, 1)[0])
            gdp = np.array([m['gdp_nominal_bn_ar'] for m in h[12:240]], float)
            mg[c].append(float(np.mean(100.0 * np.array([m['imports_bn_ar'] for m in h[12:240]], float) / gdp)))
            xg[c].append(float(np.mean(100.0 * np.array([m['exports_bn_ar'] for m in h[12:240]], float) / gdp)))
        lvl.append(y[a.cell_on][12:240].mean() - y[a.cell_off][12:240].mean())
    d = np.array(sl[a.cell_on]) - np.array(sl[a.cell_off])
    print('W2-18 (R): relative import price, slope of 100 ln(import_price_index / PT) per year over months 12-239, seeds %s (n=%d)' % (a.seeds, len(seeds)))
    for c in (a.cell_on, a.cell_off):
        print('  %-6s %+.3f %%/yr (SE %.3f); household import share slope %+.3f pp/yr (SE %.3f)' % ((c,) + mean_se(sl[c]) + mean_se(sh[c])))
    print('  paired difference %s - %s: %+.3f %%/yr (SE %.3f); mean level difference of ln(imp/PT) %+.3f %% (SE %.3f)' % ((a.cell_on, a.cell_off) + mean_se(d) + mean_se(lvl)))
    print('  if %s is the block cell without the drift: g = difference / 100 = %+.4f (decimal a year)' % (a.cell_on, np.mean(d) / 100.0))
    dm = np.array(mg[a.cell_on]) - np.array(mg[a.cell_off])
    dx = np.array(xg[a.cell_on]) - np.array(xg[a.cell_off])
    print('  W2-15 (own formula): dM/GDP %+.3f pp (SE %.3f), dX/GDP %+.3f pp (SE %.3f)' % (mean_se(dm) + mean_se(dx)))


FLOORS = [('gdp_real_bn_ar', '%', 0.10), ('cpi', '%', 0.10), ('inflation_yoy', 'pp', 0.05), ('core_pce_inflation_yoy', 'pp', 0.05), ('unemployment_rate', 'pp', 0.05),
          ('policy_rate', 'pp', 0.05), ('consumption_real_bn_ar', '%', 0.10), ('dollar_index', '%', 0.20)]


def cmd_tariff_bit(a):
    seeds = seeds_of(a.seeds)
    same, bad = 0, []
    for s in seeds:
        ba, sa = load_ev(a.dir, a.cell, a.a, s)
        bb, sb = load_ev(a.dir, a.cell, a.b, s)
        if sa == sb and ba == bb:
            same += 1
        else:
            bad.append(s)
    worst = 0.0
    for key, unit, floor in FLOORS:
        for h in (3, 12, 24):
            va, vb = [], []
            for tag, acc in ((a.a, va), (a.b, vb)):
                for s in seeds:
                    b, sh = load_ev(a.dir, a.cell, tag, s)
                    acc.append(100.0 * (sh[h - 1][key] / b[h - 1][key] - 1.0) if unit == '%' else sh[h - 1][key] - b[h - 1][key])
            worst = max(worst, abs(np.mean(va) - np.mean(vb)) / floor)
    print('W5-06 (E): %s against %s in cell %s, seeds %s: %d of %d seeds identical in every key of both arms%s; old statistic %.2f of the floor' % (
        a.a, a.b, a.cell, a.seeds, same, len(seeds), '' if not bad else ' (differ: %s)' % bad, worst))
    sys.exit(0 if not bad else 1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('relprice')
    r.add_argument('--rundir', required=True); r.add_argument('--cell-on', required=True, dest='cell_on'); r.add_argument('--cell-off', required=True, dest='cell_off'); r.add_argument('--seeds', required=True)
    t = sub.add_parser('tariff-bit')
    t.add_argument('--dir', required=True); t.add_argument('--cell', required=True); t.add_argument('--a', required=True); t.add_argument('--b', required=True); t.add_argument('--seeds', required=True)
    a = ap.parse_args()
    {'relprice': cmd_relprice, 'tariff-bit': cmd_tariff_bit}[a.cmd](a)


if __name__ == '__main__':
    main()
