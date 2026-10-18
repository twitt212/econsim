#!/usr/bin/env python3
"""Stage-3 oil gate rows, computed from stored runs (spec section 4, W3-08, W3-10, W3-11, W3-14, W3-15, W3-18, W3-19).

Stage-3 fix round (docs/world/DECISIONS.md S3-R2..S3-R9; section 7, step 1 of the stage-3 review note, not published).

Horizon convention (S3-R2, as `econsim-cli irf`): horizon h after a shock at reported index T0 is index T0 + h - 1, where
index T0 is the shock month. W3-08 keeps the index T0 + 6 for "m6" (six months after the shock month, as the stage-3 review
measured it) and also prints T0 + 5 (the irf convention) beside it.

Two file formats are read:
  compare files  DIR/<scenario>_s<seed>.json.gz    {"baseline": [records], "shocked": [records], "seeds": [...]}   (world_check run)
  run files      DIR/<cell>_s<seed>.json.gz        one JSON line: the list of monthly records (econsim-cli run ... --json),
                                                   or the compact extract of `run_cell_ex.sh` (a dict of lists, `world` nested)
Records carry the world block under "world". Seeds are given as A-B.

Subcommands
  w310  --on DIR --nofb DIR --leg DIR --seeds 1-12   W3-10 (oil50, stochastic off, 10k hh). "on" is W3-10r (default feedback),
                                                      "nofb" is W3-10 (feedback off, the gated row). Headline CPI yoy.
  w311  --dir DIR --seeds 1-16 [--m0 DIR]            W3-11 (oil50 at month 12 in DIR, oil50m0 in --m0 or DIR). CPI m3, m12, m24,
                                                      O1 GDP trough, O2 CPI peak, O3 core peak.
  w308  --dir DIR --seeds 1-48 [--scen opec2]        W3-08 OPEC+ output and Brent at m6, with the baseline-zone split.
  w314  --on DIR --off DIR --seeds A-B [--on-cell C] [--off-cell C]
                                                     W3-14 (mean oil price, months 12-239): ratio of cross-seed means against
                                                     the matched link-off cell, delta-method SE (S3-R3).
  w315  --on DIR --off DIR --seeds A-B [--on-cell C] [--off-cell C] [--offset 8.355287]
                                                     W3-15 facts rows (ABM series) and the oil_lvl market-only series.
  w318  --dir DIR --seeds A-B                        W3-18 ratio of the ABM energy response to the market US demand response.
  w319  --dir DIR --seeds A-B                        W3-19 variance shares of 24-month price changes (reported row).
  qty   --dir DIR --seeds A-B [--cell C]             OPEC+ quantity input (% of OPEC+ liquids): stochastic mean over months 0-239.
"""
import argparse
import gzip
import json
import math
import os
import sys

import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.abspath(os.path.join(HERE, '..')))

T0 = 12                                  # index of the shock month in the oil50 and opec2 cells (at_month 12)
UAE_IN_LIQUIDS_MBD = 47.688755663246255  # oil.opec.uae_in_liquids_mbd (world_params.json); the default world_uae_in_opec true
OFFSET_DEFAULT = 8.355287233989602       # oil.stochastic_mean_offset


def seed_list(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def load_json(path):
    with gzip.open(path, 'rt') as f:
        return json.load(f)


def compare(dirn, scen, seed):
    d = load_json(os.path.join(dirn, '%s_s%d.json.gz' % (scen, seed)))
    return d['baseline'], d['shocked']


def run_file(dirn, cell, seed):
    return load_json(os.path.join(dirn, '%s_s%d.json.gz' % (cell, seed)))


def series(recs, key):
    """Top-level monthly series from a list of records (compare or run) or from a compact extract (dict of lists)."""
    if isinstance(recs, dict):
        return np.array(recs[key], float)
    return np.array([r[key] for r in recs], float)


def wseries(recs, key):
    if isinstance(recs, dict):
        return np.array(recs['world'][key], float)
    return np.array([r['world'][key] for r in recs], float)


def mean_se(x):
    x = np.asarray(x, float)
    n = len(x)
    se = x.std(ddof=1) / math.sqrt(n) if n > 1 else float('nan')
    return float(x.mean()), float(se)


def verdict(ok):
    return 'PASS' if ok else 'FAIL'


# ---------------------------------------------------------------------------------------------------------------------------
def w310(a):
    sd = seed_list(a.seeds)
    E = {}
    for cell, dirn in (('on', a.on), ('nofb', a.nofb), ('leg', a.leg)):
        E[cell] = []
        for s in sd:
            base, sh = compare(dirn, 'oil50', s)
            E[cell].append(series(sh, 'inflation_yoy') - series(base, 'inflation_yoy'))
        E[cell] = np.array(E[cell])
    print('W3-10 headline CPI yoy, link minus legacy (paired, %d seeds; index T0+h-1 with T0=%d)' % (len(sd), T0))
    rows = []
    for h in (3, 12, 24):
        i = T0 + h - 1
        for cell, label in (('nofb', 'W3-10 (feedback off, gated)'), ('on', 'W3-10r (feedback on, reported)')):
            d = E[cell][:, i] - E['leg'][:, i]
            m, se = mean_se(d)
            bound = 0.05 + 2 * se
            ok = abs(m) <= bound
            rows.append((h, label, m, se, bound, ok))
            print('  h%-2d %-30s diff %+.3f (SE %.3f) bound %.3f %s' % (h, label, m, se, bound, verdict(ok)))
    return rows


def w311(a):
    sd = seed_list(a.seeds)
    m0dir = a.m0 or a.dir
    out = {}
    for name, dirn, scen, t0 in (('on', a.dir, 'oil50', T0), ('m0', m0dir, 'oil50m0', 0)):
        cpi, core, gdp = [], [], []
        for s in sd:
            base, sh = compare(dirn, scen, s)
            cpi.append(series(sh, 'inflation_yoy') - series(base, 'inflation_yoy'))
            core.append(series(sh, 'core_inflation_yoy') - series(base, 'core_inflation_yoy'))
            gb, gs = series(base, 'gdp_real_bn_ar'), series(sh, 'gdp_real_bn_ar')
            gdp.append(100 * (gs / gb - 1))
        cpi, core, gdp = np.array(cpi), np.array(core), np.array(gdp)
        out[name] = dict(cpi=cpi, core=core, gdp=gdp, t0=t0, n=len(sd))
    for name, label in (('on', 'shock at month 12'), ('m0', 'shock at month 0')):
        r = out[name]
        t0 = r['t0']
        print('W3-11 %s (%s, %d seeds; index T0+h-1 with T0=%d)' % (name, label, r['n'], t0))
        at = {}
        for h in (3, 6, 12, 24):
            at[('cpi', h)] = mean_se(r['cpi'][:, t0 + h - 1])
            at[('core', h)] = mean_se(r['core'][:, t0 + h - 1])
            at[('gdp', h)] = mean_se(r['gdp'][:, t0 + h - 1])
        for h in (3, 12, 24):
            m, se = at[('cpi', h)]
            print('  CPI yoy m%-2d %+.3f (SE %.3f)' % (h, m, se))
        o1 = min(at[('gdp', h)][0] for h in (6, 12, 24))
        o2 = max(at[('cpi', h)][0] for h in (3, 6, 12))
        o3 = max(at[('core', h)][0] for h in (3, 6, 12))
        print('  O1 GDP trough over m6,m12,m24 %+.3f ; O2 CPI peak over m3,m6,m12 %+.3f ; O3 core peak %+.3f' % (o1, o2, o3))
        r['summary'] = dict(cpi3=at[('cpi', 3)], cpi12=at[('cpi', 12)], cpi24=at[('cpi', 24)], o1=o1, o2=o2, o3=o3)
    return out


def w308(a):
    sd = seed_list(a.seeds)
    d6, d5, brent6, pb = [], [], [], []
    for s in sd:
        base, sh = compare(a.dir, a.scen, s)
        ob, os_ = wseries(base, 'opec_out_mbd'), wseries(sh, 'opec_out_mbd')
        brb, brs = wseries(base, 'brent_usd'), wseries(sh, 'brent_usd')
        pbase = wseries(base, 'p_bar_pct')
        d6.append(os_[T0 + 6] - ob[T0 + 6])
        d5.append(os_[T0 + 5] - ob[T0 + 5])
        brent6.append(100 * (brs[T0 + 6] / brb[T0 + 6] - 1))
        pb.append(pbase[T0])
    d6, d5, brent6, pb = map(np.array, (d6, d5, brent6, pb))
    m6, se6 = mean_se(d6)
    print('W3-08 %s, %d seeds: OPEC+ output at T0+6 (m6, measured convention) %+.3f mb/d (SE %.3f), median %+.3f, min %+.3f, max %+.3f'
          % (a.scen, len(sd), m6, se6, np.median(d6), d6.min(), d6.max()))
    print('  irf convention T0+5: %+.3f mb/d (SE %.3f)' % mean_se(d5))
    mb, seb = mean_se(brent6)
    print('  Brent at T0+6: %+.2f %% (SE %.2f)' % (mb, seb))
    zones = (('baseline p_bar < -12 (floor zone)', pb < -12), ('-12 <= p_bar <= 25 (middle)', (pb >= -12) & (pb <= 25)),
             ('p_bar > 25', pb > 25))
    for name, mask in zones:
        if mask.sum():
            print('  %-34s n=%2d mean OPEC+ m6 %+.3f mb/d' % (name, int(mask.sum()), d6[mask].mean()))
    return dict(m6=(m6, se6), d5=mean_se(d5), brent6=(mb, seb), zones={n: int(m.sum()) for n, m in zones})


def w314(a):
    sd = seed_list(a.seeds)
    on, off = [], []
    for s in sd:
        don = run_file(a.on, a.on_cell, s)
        doff = run_file(a.off, a.off_cell, s)
        on.append(series(don, 'oil_price')[12:240].mean())
        off.append(series(doff, 'oil_price')[12:240].mean())
    on, off = np.array(on), np.array(off)
    r = on.mean() / off.mean() - 1.0
    se = np.std(on - (1 + r) * off, ddof=1) / math.sqrt(len(on)) / off.mean()
    res = dict(n=len(sd), ratio_pct=100 * r, se_pct=100 * se, mean_on=on.mean(), mean_off=off.mean())
    print('W3-14 %s vs %s, %d seeds, months 12-239: ratio of means %+.2f %% (delta SE %.2f); means %.3f / %.3f; PASS band +-3: %s'
          % (a.on_cell, a.off_cell, len(sd), 100 * r, 100 * se, on.mean(), off.mean(), verdict(abs(100 * r) <= 3)))
    return res


def w315(a):
    sd = seed_list(a.seeds)
    rows = {k: [] for k in ('oil_vol', 'oil_kurt', 'oil_vr12', 'oil_big', 'oil_lvl', 'oil_drift', 'oil_lvl_mkt')}
    import facts_shocks as F  # same definitions as the stage-2/3 gate tables
    for s in sd:
        d = run_file(a.on, a.on_cell, s)
        fo = F.f_oil(F.prepare(d))
        rows['oil_vol'].append(100 * float(np.diff(fo).std()))
        rows['oil_kurt'].append(F.kurt_ex(np.diff(fo)))
        rows['oil_vr12'].append(F.vr(fo, 12))
        rows['oil_big'].append(float(np.mean(np.abs(F.g(fo, 12)) > 0.5)))
        rows['oil_lvl'].append(float(fo.std()))
        rows['oil_drift'].append(1200 * float(np.diff(fo).mean()))
        # Market-only level (R6 of the stage-3 review): the splice term is removed from the ABM deviation inside log(oil/cpi):
        # A_market = p_bar - offset, so log(oil/cpi) + (A_market - A)/100 with A = oil_dev_pct (the spliced deviation).
        pbar = wseries(d, 'p_bar_pct')
        dev = wseries(d, 'oil_dev_pct')
        lk = np.log(series(d, 'oil_price') / series(d, 'cpi'))
        rows['oil_lvl_mkt'].append(float((lk + (pbar - a.offset - dev) / 100).std()))
    bands = dict(oil_vol=(8.0, 10.5), oil_kurt=(1.2, 8.6), oil_vr12=(0.86, 1.42), oil_big=(0.05, 0.20), oil_lvl=(0.27, 0.52),
                 oil_drift=(-1.0, 2.0), oil_lvl_mkt=(0.27, 0.52))
    res = {}
    print('W3-15 %s, %d seeds (ABM series unless stated)' % (a.on_cell, len(sd)))
    for k in rows:
        m, se = mean_se(rows[k])
        lo, hi = bands[k]
        res[k] = (m, se)
        print('  %-12s %+.4f (SE %.4f) band [%.2f, %.2f] %s' % (k, m, se, lo, hi, verdict(lo <= m <= hi)))
    return res


def w318(a):
    sd = seed_list(a.seeds)
    dd, ec, dp = [], [], []
    for s in sd:
        base, sh = compare(a.dir, 'oil50', s)
        dd.append(wseries(sh, 'oil_us_demand_pct') - wseries(base, 'oil_us_demand_pct'))
        cb = np.array([r['consumption_by_sector_real'][0] for r in base], float)
        cs = np.array([r['consumption_by_sector_real'][0] for r in sh], float)
        ec.append(100 * (cs / cb - 1))
        dp.append(wseries(sh, 'p_bar_pct') - wseries(base, 'p_bar_pct'))
    dd, ec, dp = np.array(dd), np.array(ec), np.array(dp)
    print('W3-18 oil50 at month %d, %d seeds (ratio = ABM energy %% change / market US demand change, pp)' % (T0, len(sd)))
    out = {}
    for h in (3, 6, 12, 24):
        i = T0 + h - 1
        m = dd[:, i].mean()
        ae, se = mean_se(ec[:, i])
        ratio = ae / m if m != 0 else float('nan')
        out[h] = ratio
        print('  m%-2d market US demand %+.3f, ABM energy %+.3f%% (SE %.3f), ratio %.2f, band [0.5, 2] %s'
              % (h, m, ae, se, ratio, verdict(0.5 <= ratio <= 2)))
    return out


def w319(a):
    sd = seed_list(a.seeds)
    cells = {'I': 'nofb_I', 'D': 'nofb_D', 'O': 'nofb_O', 'activity': 'mask_none'}
    pooled, per_seed = {}, {}
    for tag, cell in cells.items():
        x = []
        for s in sd:
            p = wseries(run_file(a.dir, cell, s), 'p_bar_pct')[12:240]
            x.append(p[24:] - p[:-24])
        per_seed[tag] = x
        pooled[tag] = float(np.concatenate(x).var())
    tot = sum(pooled.values())
    shares = {k: v / tot for k, v in pooled.items()}
    supply = (pooled['D'] + pooled['O']) / tot
    rng = np.random.default_rng(1)
    boot = []
    for _ in range(200):
        idx = rng.integers(0, len(sd), len(sd))
        vv = {k: float(np.concatenate([per_seed[k][i] for i in idx]).var()) for k in cells}
        tt = sum(vv.values())
        boot.append((vv['I'] / tt, vv['activity'] / tt, (vv['D'] + vv['O']) / tt))
    boot = np.array(boot)
    print('W3-19 24-month price change variance shares (reported), %d seeds, months 12-239' % len(sd))
    print('  oil-specific (I, feedback off) %.3f (boot SE %.3f)' % (shares['I'], boot[:, 0].std()))
    print('  activity (no structural shocks, feedback on) %.3f (boot SE %.3f)' % (shares['activity'], boot[:, 1].std()))
    print('  supply (D + O, feedback off) %.3f (boot SE %.3f)' % (supply, boot[:, 2].std()))
    print('  sum of single-source variances over the all-source variance: see the all-sources cell (mask_IDO)')
    return dict(I=shares['I'], activity=shares['activity'], supply=supply)


def qty(a):
    sd = seed_list(a.seeds)
    means = []
    for s in sd:
        d = run_file(a.dir, a.cell, s)
        opec = wseries(d, 'opec_out_mbd')[0:240]
        means.append(float((100.0 * opec / UAE_IN_LIQUIDS_MBD).mean()))
    m, se = mean_se(means)
    print('OPEC+ quantity input (%% of OPEC+ liquids, market, months 0-239), %s, seeds %s: mean %+.3f (SE %.3f), sd across seeds %.3f'
          % (a.cell, a.seeds, m, se, float(np.std(means, ddof=1))))
    return m, se


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest='cmd', required=True)
    c = sub.add_parser('w310')
    c.add_argument('--on', required=True); c.add_argument('--nofb', required=True); c.add_argument('--leg', required=True)
    c.add_argument('--seeds', required=True); c.set_defaults(fn=w310)
    c = sub.add_parser('w311')
    c.add_argument('--dir', required=True); c.add_argument('--m0'); c.add_argument('--seeds', required=True); c.set_defaults(fn=w311)
    c = sub.add_parser('w308')
    c.add_argument('--dir', required=True); c.add_argument('--seeds', required=True); c.add_argument('--scen', default='opec2')
    c.set_defaults(fn=w308)
    c = sub.add_parser('w314')
    c.add_argument('--on', required=True); c.add_argument('--off', required=True); c.add_argument('--seeds', required=True)
    c.add_argument('--on-cell', dest='on_cell', default='on'); c.add_argument('--off-cell', dest='off_cell', default='off')
    c.set_defaults(fn=w314)
    c = sub.add_parser('w315')
    c.add_argument('--on', required=True); c.add_argument('--seeds', required=True); c.add_argument('--on-cell', dest='on_cell', default='on')
    c.add_argument('--offset', type=float, default=OFFSET_DEFAULT); c.set_defaults(fn=w315)
    c = sub.add_parser('w318')
    c.add_argument('--dir', required=True); c.add_argument('--seeds', required=True); c.set_defaults(fn=w318)
    c = sub.add_parser('w319')
    c.add_argument('--dir', required=True); c.add_argument('--seeds', required=True); c.set_defaults(fn=w319)
    c = sub.add_parser('qty')
    c.add_argument('--dir', required=True); c.add_argument('--seeds', required=True); c.add_argument('--cell', required=True)
    c.set_defaults(fn=qty)
    a = p.parse_args()
    a.fn(a)


if __name__ == '__main__':
    main()
