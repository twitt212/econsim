#!/usr/bin/env python3
"""scripts/world_presets.py: W5-04 bloc-policy and China-demand preset rows (spec section 5) from the compare files of
scripts/world_runs.py events (DIR/CELL_SCENARIO/SCENARIO_sSEED.json.gz). Paired shocked-minus-baseline mean paths over the seeds;
the statistic is taken on the mean path (spec 3.0). Horizon: month h is record index h - 1. Bands are the section-5 bands and are
printed with PASS / FAIL; nothing is decided here beyond that. Output gaps are in percent (world.y_gap), inflation gaps in pp
(world.pi_gap), US GDP in percent of the baseline.

  ecb    --dir DIR --cell C --seeds A-B   ECB +100 bp (ecb100): EA gap trough months 1-48, EA inflation m12, US GDP trough months 1-36,
                                          largest other-bloc peak gap months 1-48 (EA excluded; the bound 0.15 is [assumed])
  boj    --dir DIR --cell C --seeds A-B   BoJ +100 bp (boj100): JP gap trough months 1-48, JP inflation m12, US GDP trough months 1-36
  pboc   --dir DIR --cell C --seeds A-B   PBoC -100 bp (pboc-100): CN gap peak months 1-48, CN inflation peak, AU/KR/OPEC+ peak gaps,
                                          US exports at m12 (> 0)
  china  --dir DIR --cell C --seeds A-B   China slowdown (china3, needs the dollar link for its US rows): EA and UK gap troughs
                                          (reported with the trade link alone; the US and dollar rows are read in the dollar cell)
"""
import argparse, gzip, json, math, os, sys
import numpy as np

BLOC = ['EA', 'CN', 'JP', 'UK', 'CA', 'MX', 'IN', 'KR', 'BR', 'AU', 'OPEC+', 'RoW']


def seeds_of(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def load(d, cell, scen, s):
    with gzip.open(os.path.join(d, '%s_%s' % (cell, scen), '%s_s%d.json.gz' % (scen, s)), 'rt') as f:
        r = json.load(f)
    return r['baseline'], r['shocked']


def paired(d, cell, scen, seeds, fn):
    """seeds x months array of fn(shocked) - fn(baseline) per record."""
    out = []
    for s in seeds:
        b, sh = load(d, cell, scen, s)
        out.append([fn(x) - fn(y) for x, y in zip(sh, b)])
    return np.array(out, float)


def pct_paired(d, cell, scen, seeds, key):
    out = []
    for s in seeds:
        b, sh = load(d, cell, scen, s)
        out.append([100.0 * (x[key] / y[key] - 1.0) for x, y in zip(sh, b)])
    return np.array(out, float)


def se(x):
    x = np.asarray(x, float)
    return float(x.std(ddof=1) / math.sqrt(len(x))) if len(x) > 1 else float('nan')


def row(label, value, band, unit):
    if band is None:
        tag, bt = 'reported', 'reported'
    else:
        tag = 'PASS' if band[0] <= value <= band[1] else 'FAIL'
        bt = '[%g, %g]' % band
    print('  %-58s %10.3f %s band %s %s' % (label, value, unit, bt, tag))
    return tag


def gap_path(d, cell, scen, seeds, j):
    return paired(d, cell, scen, seeds, lambda r: r['world']['y_gap'][j])


def pi_path(d, cell, scen, seeds, j):
    return paired(d, cell, scen, seeds, lambda r: r['world']['pi_gap'][j])


def cmd_ecb(a):
    seeds = seeds_of(a.seeds)
    print('W5-04 ECB +100bp, cell %s, %d seeds (mean paths)' % (a.cell, len(seeds)))
    G = gap_path(a.dir, a.cell, 'ecb100', seeds, 0).mean(axis=0)
    row('EA gap trough, months 1-48 (%)', G[:48].min(), (-0.75, -0.40), '%')
    P = pi_path(a.dir, a.cell, 'ecb100', seeds, 0).mean(axis=0)
    row('EA inflation gap, month 12 (pp)', P[11], (-0.60, -0.20), 'pp')
    gdp = pct_paired(a.dir, a.cell, 'ecb100', seeds, 'gdp_real_bn_ar').mean(axis=0)
    row('US GDP trough, months 1-36 (%)', gdp[:36].min(), (-0.10, 0.10), '%')
    peaks = []
    for j in range(1, 12):
        pk = gap_path(a.dir, a.cell, 'ecb100', seeds, j).mean(axis=0)[:48].max()
        peaks.append((BLOC[j], pk))
    for name, pk in peaks:
        row('peak output gap, %s, months 1-48 (%%) [other-bloc bound 0.15 assumed]' % name, pk, (-1e9, 0.15), '%')


def cmd_boj(a):
    seeds = seeds_of(a.seeds)
    print('W5-04 BoJ +100bp, cell %s, %d seeds (mean paths)' % (a.cell, len(seeds)))
    G = gap_path(a.dir, a.cell, 'boj100', seeds, 2).mean(axis=0)
    row('JP gap trough, months 1-48 (%) [assumed band]', G[:48].min(), (-1.6, -0.3), '%')
    P = pi_path(a.dir, a.cell, 'boj100', seeds, 2).mean(axis=0)
    row('JP inflation gap, month 12 (pp)', P[11], (-0.9, -0.2), 'pp')
    gdp = pct_paired(a.dir, a.cell, 'boj100', seeds, 'gdp_real_bn_ar').mean(axis=0)
    row('US GDP trough, months 1-36 (%)', gdp[:36].min(), (-0.05, 0.05), '%')


def cmd_pboc(a):
    seeds = seeds_of(a.seeds)
    print('W5-04 PBoC -100bp, cell %s, %d seeds (mean paths)' % (a.cell, len(seeds)))
    G = gap_path(a.dir, a.cell, 'pboc-100', seeds, 1).mean(axis=0)
    row('CN gap peak, months 1-48 (%) [assumed band]', G[:48].max(), (0.40, 1.40), '%')
    P = pi_path(a.dir, a.cell, 'pboc-100', seeds, 1).mean(axis=0)
    row('CN inflation gap peak, months 1-48 (pp) [assumed band]', P[:48].max(), (0.2, 1.0), 'pp')
    for name, j in (('AU', 9), ('KR', 7), ('OPEC+', 10)):
        pk = gap_path(a.dir, a.cell, 'pboc-100', seeds, j).mean(axis=0)[:48].max()
        row('%s gap peak, months 1-48 (%%) [assumed band]' % name, pk, (0.0, 0.4), '%')
    ex = pct_paired(a.dir, a.cell, 'pboc-100', seeds, 'exports_bn_ar').mean(axis=0)
    row('US exports, month 12 (%) (sign: > 0)', ex[11], (0.0, 1e9), '%')


def cmd_china(a):
    seeds = seeds_of(a.seeds)
    print('China slowdown (china3), cell %s, %d seeds: trade-only rows (reported; the US and dollar rows need the dollar cell)' % (
        a.cell, len(seeds)))
    for name, j, band in (('EA', 0, (-0.60, -0.15)), ('UK', 3, (-0.45, -0.05))):
        G = gap_path(a.dir, a.cell, 'china3', seeds, j).mean(axis=0)
        row('%s gap trough, months 1-48 (%%)' % name, G[:48].min(), band, '%')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    for name, fn in (('ecb', cmd_ecb), ('boj', cmd_boj), ('pboc', cmd_pboc), ('china', cmd_china)):
        p = sub.add_parser(name)
        p.add_argument('--dir', required=True); p.add_argument('--cell', required=True); p.add_argument('--seeds', required=True)
        p.set_defaults(func=fn)
    a = ap.parse_args()
    a.func(a)


if __name__ == '__main__':
    main()
