#!/usr/bin/env python3
"""Shared batch-9 measurement harness (b9-tools): shock runs, year-window statistics, harness subsets, sacrifice ratio.

  python3 -B scripts/shock_irf.py run       --server BIN --cell C [--cfg FILE] --scenarios oil50,pol100 --seeds 1-48
                                            [--households 5000] [--months 60] [--servers 4] --port0 PORT [--warmup N] --out DIR
  python3 -B scripts/shock_irf.py windows   --out DIR --cell C [--base B] --scenario S --seeds 1-48
                                            [--rows ref|all] [--ratio biz:policy ...] [--json FILE]
  python3 -B scripts/shock_irf.py harness   --out DIR --cell C [--base B] --seeds 1-48 [--scenarios pol100,pin7,pin10,qe05,qe1]
                                            [--json FILE]
  python3 -B scripts/shock_irf.py sacrifice --out DIR --cell C [--base B] --scenario pin10 --seeds 1-48 [--horizons 36,60]

run        POST /api/compare with one seed per request, for each (scenario, seed). Stores
           DIR/<cell>/<scenario>_s<seed>.json.gz = {'meta', 'baseline', 'shocked'}, per-month lists of FIELDS.
windows    per seed, the gap (shocked - baseline; percent rows 100 * (s/b - 1)) averaged over each window of months.
           Cell value = mean over seeds; SE = sample sd / sqrt(n). --base adds per-seed paired differences.
           A statistics block (mean path over seeds, jackknife SE) follows the rows.
harness    the b7 TA-TD and N1-N3 statistics of transmission_irf.py for any subset of pol100, pin7, pin10, qe05, qe1.
sacrifice  Volcker sacrifice ratio on the mean path: SR_H = (-sum_{t<=H} gdp_gap_t / 12) / (-min_{t<=H} core_gap_t),
           gdp gap in percent, core gap in pp.

Month t is record t-1 of a stored series. No simulation change. Standard library only. The statistics of the
b7 harness are imported from transmission_irf.py (not copied); that file is not edited.
"""
import argparse, atexit, gzip, json, math, os, queue, statistics, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import transmission_irf as T  # noqa: E402  (parse_seeds, http_json, health, base_url, load_seed, load_cell,
#                                             stat_with_se, paired_stat, jackknife_se, pct, diff, fmt)

# Stored fields: the b7 field list plus the batch-9 additions (inflation_yoy is already in T.FIELDS).
EXTRA_FIELDS = [
    'employment_millions', 'employment_by_sector', 'firm_loan_rate', 'credit_spread', 'lender_premium',
    'housing_starts_millions_ar', 'house_sales_millions_ar', 'capacity_utilization', 'vacancy_rate',
    'hires_rate', 'real_wage_index', 'gov_debt_gdp', 'deficit_gdp', 'inflation_yoy',
]
FIELDS = list(dict.fromkeys(list(T.FIELDS) + EXTRA_FIELDS))

# Shock lists, all scheduled at month 0 (key -> list of shock dicts).
SCENARIOS = {
    'pol100': [{'type': 'PolicyShift', 'bps': 100.0, 'months': 12}],
    'pin7': [{'type': 'PolicyRate', 'rate': 0.07, 'months': 24}],
    'pin10': [{'type': 'PolicyRate', 'rate': 0.10, 'months': 24}],
    'qe05': [{'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 0.5, 'months': 12}],
    'qe1': [{'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 1.0, 'months': 12}],
    'oil50': [{'type': 'OilPrice', 'pct': 50.0, 'half_life_months': 12.0}],
    'oil100': [{'type': 'OilPrice', 'pct': 100.0, 'half_life_months': 12.0}],
    'tp10': [{'type': 'TermPremium', 'tenor_idx': 3, 'bps': 100.0, 'half_life_months': 24.0}],
    'credit200': [{'type': 'CreditSpread', 'bps': 200.0, 'months': 12}],
    'gov5': [{'type': 'GovSpending', 'pct': 5.0, 'months': 12}],
    'tax2': [{'type': 'IncomeTax', 'pp': 2.0, 'months': 12}],
}
HARNESS_SCENARIOS = ['pol100', 'pin7', 'pin10', 'qe05', 'qe1']

# Window rows. Source kinds: ('f', field) top-level field; ('y', i) yields[i]; ('s', i) employment_by_sector[i];
# ('mtg',) mortgage_rate - yields[3] computed inside each arm. Transform: 'pct' or 'diff'. ref = in an archived master reference run.
ROWS = [
    ('gdp %', ('f', 'gdp_real_bn_ar'), 'pct', True),
    ('u pp', ('f', 'unemployment_rate'), 'diff', True),
    ('policy', ('f', 'policy_rate'), 'diff', True),
    ('business_loan_rate', ('f', 'business_loan_rate'), 'diff', True),
    ('firm_loan_rate', ('f', 'firm_loan_rate'), 'diff', True),
    ('mortgage', ('f', 'mortgage_rate'), 'diff', True),
    ('10y', ('y', 3), 'diff', True),
    ('core yoy', ('f', 'core_inflation_yoy'), 'diff', True),
    ('core pce', ('f', 'core_pce_inflation_yoy'), 'diff', True),
    ('inv %', ('f', 'investment_real_bn_ar'), 'pct', True),
    ('exp infl (hh)', ('f', 'expected_inflation'), 'diff', True),
    ('energy emp %', ('s', 0), 'pct', True),
    ('employment M', ('f', 'employment_millions'), 'diff', True),
    ('cons %', ('f', 'consumption_real_bn_ar'), 'pct', False),
    ('2y', ('y', 1), 'diff', False),
    ('5y', ('y', 2), 'diff', False),
    ('mtg_spread', ('mtg',), 'diff', False),
    ('credit_spread', ('f', 'credit_spread'), 'diff', False),
    ('lender_prem', ('f', 'lender_premium'), 'diff', False),
    ('deposit', ('f', 'deposit_rate'), 'diff', False),
    ('cpi yoy', ('f', 'inflation_yoy'), 'diff', False),
    ('wage_gr', ('f', 'wage_growth_yoy'), 'diff', False),
    ('markup', ('f', 'avg_markup'), 'diff', False),
    ('vacancy', ('f', 'vacancy_rate'), 'diff', False),
    ('hires', ('f', 'hires_rate'), 'diff', False),
    ('cap_util', ('f', 'capacity_utilization'), 'diff', False),
    ('disc_mult %', ('f', 'discretionary_multiplier_mean'), 'pct', False),
    ('dollar %', ('f', 'dollar_index'), 'pct', False),
    ('house_px %', ('f', 'median_house_price'), 'pct', False),
    ('sales %', ('f', 'house_sales_millions_ar'), 'pct', False),
    ('starts %', ('f', 'housing_starts_millions_ar'), 'pct', False),
    ('real_wage %', ('f', 'real_wage_index'), 'pct', False),
    ('gov_debt_gdp', ('f', 'gov_debt_gdp'), 'diff', False),
]
ROW_BY_LABEL = {r[0]: r for r in ROWS}
RATIO_ALIAS = {'biz': 'business_loan_rate', 'firm_loan': 'firm_loan_rate', 'policy': 'policy',
               'mortgage': 'mortgage', '10y': '10y'}

# Windows over months (record a .. b-1).
WINDOWS = [('m1-3', 0, 3), ('y1', 0, 12), ('y2', 12, 24), ('y3', 24, 36), ('y4', 36, 48), ('y5', 48, 60),
           ('y3-5', 24, 60)]
WINDOW_NAMES = [w[0] for w in WINDOWS]
REF_WINDOWS = ['y1', 'y2', 'y3', 'y4', 'y5']
RATIO_WINDOWS = ['m1-3', 'y1', 'y2']

# Statistics on the mean path: (name, series key, first record, end record exclusive, reducer).
STAT_DEFS = [
    ('GDP_tr_6_24', 'gdp', 5, 24, min),
    ('GDP_tr_1_60', 'gdp', 0, 60, min),
    ('U_pk_3_24', 'u', 2, 24, max),
    ('U_pk_1_60', 'u', 0, 60, max),
    ('CORE_tr_6_24', 'core', 5, 24, min),
    ('CORE_pk_3_12', 'core', 2, 12, max),
    ('CPI_pk_3_12', 'cpi', 2, 12, max),
    ('C24', 'core', 23, 24, lambda v: v[0]),
    ('INV_y1', 'inv', 0, 12, lambda v: sum(v) / len(v)),
    ('ENEMP_pk_1_24', 'enemp', 0, 24, max),
]
STAT_ROWS = {'gdp': 'gdp %', 'u': 'u pp', 'core': 'core yoy', 'cpi': 'cpi yoy', 'inv': 'inv %',
             'enemp': 'energy emp %'}


# ---------------------------------------------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------------------------------------------

def extract(records, months):
    if len(records) < months:
        raise SystemExit(f'compare returned {len(records)} records, need {months}')
    missing = [f for f in FIELDS if f not in records[0]]
    if missing:
        raise SystemExit(f'server output lacks key(s): {", ".join(missing)}')
    return {f: [r[f] for r in records[:months]] for f in FIELDS}


def run_cmd(a):
    if a.warmup < 0:
        raise SystemExit('--warmup must be >= 0')
    scen_names = [s for s in a.scenarios.split(',') if s]
    bad = [s for s in scen_names if s not in SCENARIOS]
    if bad:
        raise SystemExit(f'unknown scenario(s): {bad}')
    seeds = T.parse_seeds(a.seeds)
    overlay = json.load(open(a.cfg)) if a.cfg else {}
    if not isinstance(overlay, dict):
        raise SystemExit('cfg must be a JSON object')
    ports = [a.port0 + i for i in range(a.servers)]
    busy = [p for p in ports if T.health(p)]
    if busy:
        raise SystemExit(f'ports already answering /api/health: {busy}; choose another --port0')

    cell_dir = os.path.join(a.out, a.cell)
    log_dir = os.path.join(a.out, 'logs')
    os.makedirs(cell_dir, exist_ok=True)
    os.makedirs(log_dir, exist_ok=True)

    procs = []

    def cleanup():
        for p in procs:
            if p.poll() is None:
                p.terminate()
        for p in procs:
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()

    atexit.register(cleanup)
    try:
        for p in ports:
            env = dict(os.environ, PORT=str(p), RAYON_NUM_THREADS='1', ECONSIM_COMPARE_CONCURRENCY='1',
                       ECONSIM_CACHE_MB='0', ECONSIM_WARMUP_MONTHS='0')
            log = open(os.path.join(log_dir, f'server_{p}.log'), 'w')
            proc = subprocess.Popen([a.server], env=env, stdout=log, stderr=subprocess.STDOUT)
            procs.append(proc)
            with open(os.path.join(log_dir, 'pids.txt'), 'a') as fh:
                fh.write(f'{proc.pid} port {p} cell {a.cell} started {time.strftime("%Y-%m-%d %H:%M:%S")}\n')
        for p in ports:
            t0 = time.time()
            while not T.health(p):
                if time.time() - t0 > 60:
                    raise SystemExit(f'server on port {p} did not start')
                time.sleep(0.2)

        defaults = T.http_json(T.base_url(ports[0]) + '/api/config/default')
        unknown = [k for k in overlay if k not in defaults]
        if unknown:
            raise SystemExit(f'overlay keys not in /api/config/default: {unknown}')
        if 'warmup_months' in defaults:
            if 'warmup_months' in overlay and overlay['warmup_months'] != a.warmup:
                raise SystemExit('cfg sets warmup_months; use --warmup instead')
            overlay['warmup_months'] = a.warmup
        elif a.warmup > 0:
            raise SystemExit('server has no warmup_months')

        jobs = []
        for sc in scen_names:
            for seed in seeds:
                path = seed_path(a.out, a.cell, sc, seed)
                if not os.path.exists(path):
                    jobs.append((sc, seed, path))
        print(f'cell {a.cell}: {len(jobs)} jobs to run ({len(scen_names) * len(seeds) - len(jobs)} already present); '
              f'ports {ports}; warmup {a.warmup}', flush=True)

        free = queue.Queue()
        for p in ports:
            free.put(p)

        def job(item):
            sc, seed, path = item
            port = free.get()
            try:
                cfg = dict(overlay)
                cfg['n_households'] = a.households
                payload = {'config': cfg, 'shocks': [{'at_month': 0, 'shock': s} for s in SCENARIOS[sc]],
                           'months': a.months, 'seeds': [seed]}
                cmp = T.http_json(T.base_url(port) + '/api/compare', payload)
                doc = {'meta': {'cell': a.cell, 'scenario': sc, 'seed': seed, 'months': a.months,
                                'households': a.households, 'warmup': a.warmup},
                       'baseline': extract(cmp['baseline'], a.months),
                       'shocked': extract(cmp['shocked'], a.months)}
                tmp = path + '.tmp'
                with gzip.open(tmp, 'wt') as fh:
                    json.dump(doc, fh)
                os.replace(tmp, path)
                return sc, seed
            finally:
                free.put(port)

        done = 0
        with ThreadPoolExecutor(max_workers=len(ports)) as ex:
            for sc, seed in ex.map(job, jobs):
                done += 1
                if done % 16 == 0 or done == len(jobs):
                    print(f'  {done}/{len(jobs)} done (last {sc} seed {seed})', flush=True)
    finally:
        cleanup()
    print(f'cell {a.cell}: complete', flush=True)


# ---------------------------------------------------------------------------------------------------------------
# shared helpers
# ---------------------------------------------------------------------------------------------------------------

def seed_path(out, cell, sc, seed):
    return os.path.join(out, cell, f'{sc}_s{seed}.json.gz')


def missing_paths(out, cell, sc, seeds):
    return [p for p in (seed_path(out, cell, sc, s) for s in seeds) if not os.path.exists(p)]


def load_docs(out, cell, sc, seeds):
    miss = missing_paths(out, cell, sc, seeds)
    if miss:
        raise SystemExit(f'missing {len(miss)} file(s) for cell {cell} scenario {sc}, first: {miss[0]}')
    return [T.load_seed(seed_path(out, cell, sc, s)) for s in seeds]


def arm_series(rec, src):
    kind = src[0]
    if kind == 'f':
        return rec.get(src[1])
    if kind == 'y':
        rows = rec.get('yields')
        return None if rows is None else [r[src[1]] for r in rows]
    if kind == 's':
        rows = rec.get('employment_by_sector')
        return None if rows is None else [r[src[1]] for r in rows]
    if kind == 'mtg':
        m, y = rec.get('mortgage_rate'), rec.get('yields')
        return None if m is None or y is None else [a - r[3] for a, r in zip(m, y)]  # tenor 3 = 10y
    raise SystemExit(f'bad source {src}')


def gap_series(doc, src, tr):
    b = arm_series(doc['baseline'], src)
    s = arm_series(doc['shocked'], src)
    if b is None or s is None:
        return None
    return T.pct(s, b) if tr == 'pct' else T.diff(s, b)


def row_gaps(docs, label):
    """List over seeds of the per-month gap series of one row, or None when a source key is missing."""
    _, src, tr, _ = ROW_BY_LABEL[label]
    out = []
    for d in docs:
        g = gap_series(d, src, tr)
        if g is None:
            return None
        out.append(g)
    return out


def window_means(g):
    out = {}
    for name, a, b in WINDOWS:
        out[name] = None if len(g) < b else sum(g[a:b]) / (b - a)
    return out


def row_window_table(docs, label):
    gaps = row_gaps(docs, label)
    return None if gaps is None else [window_means(g) for g in gaps]


def seed_mean_se(vals):
    """Mean over seeds and SE = sample sd / sqrt(n). Any None entry makes the cell None."""
    if vals is None or not vals or any(v is None for v in vals):
        return None, None
    n = len(vals)
    m = statistics.mean(vals)
    se = statistics.stdev(vals) / math.sqrt(n) if n >= 2 else None
    return m, se


def paired_seed_mean_se(tab, btab, w):
    if tab is None or btab is None:
        return None, None
    diffs = []
    for x, y in zip(tab, btab):
        if x[w] is None or y[w] is None:
            return None, None
        diffs.append(x[w] - y[w])
    return seed_mean_se(diffs)


def ratio_cell(num_tab, den_tab, w):
    """Ratio of seed-mean window gaps with the leave-one-seed-out jackknife SE."""
    if num_tab is None or den_tab is None:
        return None, None
    nums = [t[w] for t in num_tab]
    dens = [t[w] for t in den_tab]
    if any(v is None for v in nums + dens):
        return None, None

    def rat(ns, ds):
        md = statistics.mean(ds)
        return None if md == 0 else statistics.mean(ns) / md

    point = rat(nums, dens)
    n = len(nums)
    if point is None or n < 2:
        return point, None
    loo = []
    for i in range(n):
        r = rat(nums[:i] + nums[i + 1:], dens[:i] + dens[i + 1:])
        if r is None:
            return point, None
        loo.append(r)
    return point, T.jackknife_se(loo)


def mean_path(series):
    n = len(series)
    length = min(len(s) for s in series)
    return [sum(s[t] for s in series) / n for t in range(length)]


def leave_out_paths(gaps, skip):
    """Mean path per key over the seeds, optionally leaving out seed index `skip`."""
    out = {}
    for key, ser in gaps.items():
        if ser is None:
            out[key] = None
        elif skip is None:
            out[key] = mean_path(ser)
        else:
            out[key] = mean_path([s for j, s in enumerate(ser) if j != skip])
    return out


def stat_values(paths):
    out = {}
    for name, key, a, b, fn in STAT_DEFS:
        p = paths.get(key)
        out[name] = None if p is None or len(p) < b else fn(p[a:b])
    return out


def cell_stats(gaps, n):
    """{name: (value, jackknife SE)} on the mean path."""
    point = stat_values(leave_out_paths(gaps, None))
    loo = [stat_values(leave_out_paths(gaps, i)) for i in range(n)] if n >= 2 else None
    out = {}
    for name in point:
        se = None
        if loo is not None and point[name] is not None and all(x[name] is not None for x in loo):
            se = T.jackknife_se([x[name] for x in loo])
        out[name] = (point[name], se)
    return out


def paired_stats(gaps_t, gaps_b, n):
    """d = stat(T) - stat(B) on the mean paths, jackknife over the same left-out seed."""
    pt = stat_values(leave_out_paths(gaps_t, None))
    pb = stat_values(leave_out_paths(gaps_b, None))

    def dif(a, b):
        return None if a is None or b is None else a - b

    point = {k: dif(pt[k], pb[k]) for k in pt}
    if n < 2:
        return {k: (point[k], None) for k in point}
    loo = []
    for i in range(n):
        st = stat_values(leave_out_paths(gaps_t, i))
        sb = stat_values(leave_out_paths(gaps_b, i))
        loo.append({k: dif(st[k], sb[k]) for k in st})
    out = {}
    for k in point:
        se = None
        if point[k] is not None and all(x[k] is not None for x in loo):
            se = T.jackknife_se([x[k] for x in loo])
        out[k] = (point[k], se)
    return out


def fmt_pair(v, se):
    if v is None:
        return '      n/a'
    if se is None:
        return f'{v:9.3f}    n/a'
    return T.fmt(v, se)


def window_text(v, se):
    if v is None:
        return '-'
    if se is None:
        return '%+.3f(n/a)' % v
    return '%+.3f(%.3f)' % (v, se)


def pack(pair):
    return {'value': pair[0], 'se': pair[1]}


# ---------------------------------------------------------------------------------------------------------------
# windows
# ---------------------------------------------------------------------------------------------------------------

def windows_cmd(a):
    seeds = T.parse_seeds(a.seeds)
    if not seeds:
        raise SystemExit('no seeds')
    if a.scenario not in SCENARIOS:
        raise SystemExit(f'unknown scenario {a.scenario}')
    ratio_specs = []
    for spec in a.ratio:
        if ':' not in spec:
            raise SystemExit(f'--ratio wants NUM:DEN, got {spec}')
        names = []
        for part in spec.split(':', 1):
            lab = RATIO_ALIAS.get(part, part)
            if lab not in ROW_BY_LABEL:
                raise SystemExit(f'unknown ratio name {part}: use a Label of the table or biz, firm_loan, policy, '
                                 f'mortgage, 10y')
            names.append(lab)
        ratio_specs.append((spec, names[0], names[1]))

    n = len(seeds)
    docs = load_docs(a.out, a.cell, a.scenario, seeds)
    bdocs = load_docs(a.out, a.base, a.scenario, seeds) if a.base else None
    tables = {r[0]: row_window_table(docs, r[0]) for r in ROWS}
    cellv = {lab: {w: seed_mean_se(None if t is None else [x[w] for x in t]) for w in WINDOW_NAMES}
             for lab, t in tables.items()}
    btables = dv = None
    if bdocs is not None:
        btables = {r[0]: row_window_table(bdocs, r[0]) for r in ROWS}
        dv = {lab: {w: paired_seed_mean_se(tables[lab], btables[lab], w) for w in WINDOW_NAMES} for lab in tables}
    ratios = {spec: {w: ratio_cell(tables[num], tables[den], w) for w in RATIO_WINDOWS}
              for spec, num, den in ratio_specs}
    gaps = {k: row_gaps(docs, STAT_ROWS[k]) for k in STAT_ROWS}
    stv = cell_stats(gaps, n)
    pstv = bgaps = None
    if bdocs is not None:
        bgaps = {k: row_gaps(bdocs, STAT_ROWS[k]) for k in STAT_ROWS}
        pstv = paired_stats(gaps, bgaps, n)

    if a.rows == 'all':
        shown, wins, pad = ROWS, WINDOW_NAMES, 17
    else:
        shown, wins, pad = [r for r in ROWS if r[3]], REF_WINDOWS, 0

    print(f'{a.scenario} {n}')
    if a.rows == 'all':
        print(' ' * 27 + ' '.join(w.ljust(pad) for w in wins).rstrip())
    for label, _, _, _ in shown:
        tab = tables[label]
        if tab is None:
            print('%-26s %s' % (label, 'n/a'))
        else:
            cells = [window_text(*cellv[label][w]).ljust(pad) for w in wins]
            print(('%-26s %s' % (label, ' '.join(cells))).rstrip())
        if btables is not None:
            dlabel = '  d vs ' + a.base
            if tab is None or btables[label] is None:
                print('%-26s %s' % (dlabel, 'n/a'))
            else:
                cells = [window_text(*dv[label][w]).ljust(pad) for w in wins]
                print(('%-26s %s' % (dlabel, ' '.join(cells))).rstrip())

    for spec, num, den in ratio_specs:
        print(f'ratio {spec}   ({num} / {den}; jackknife SE of the ratio of seed means)')
        for w in RATIO_WINDOWS:
            v, se = ratios[spec][w]
            print(f'   {w:<6s} {window_text(v, se)}')
    if ratio_specs:
        print('   (n/a) = jackknife SE undefined: some leave-one-seed-out denominator is exactly 0 '
              '(e.g. the policy rate does not move in the window for most seeds)')

    print('-- statistics on the mean path over seeds (jackknife SE; month t = record t-1)')
    for name, _, _, _, _ in STAT_DEFS:
        print('   %-14s %s' % (name, fmt_pair(*stv[name])))
    if pstv is not None:
        print(f'-- paired statistics d = {a.cell} - {a.base} (jackknife SE, same left-out seed)')
        for name, _, _, _, _ in STAT_DEFS:
            print('   d%-13s %s' % (name, fmt_pair(*pstv[name])))

    if a.json:
        result = {'cell': a.cell, 'base': a.base, 'scenario': a.scenario, 'seeds': seeds, 'n_seeds': n,
                  'windows': WINDOW_NAMES, 'rows': {}, 'ratios': {}, 'statistics': {}, 'paired_statistics': {}}
        for label, _, _, ref in ROWS:
            ent = {'ref': ref, 'windows': {w: pack(cellv[label][w]) for w in WINDOW_NAMES}}
            if dv is not None:
                ent['d'] = {w: pack(dv[label][w]) for w in WINDOW_NAMES}
            result['rows'][label] = ent
        for spec, num, den in ratio_specs:
            result['ratios'][spec] = {'num': num, 'den': den,
                                      'windows': {w: pack(ratios[spec][w]) for w in RATIO_WINDOWS}}
        result['statistics'] = {name: pack(stv[name]) for name, _, _, _, _ in STAT_DEFS}
        if pstv is not None:
            result['paired_statistics'] = {name: pack(pstv[name]) for name, _, _, _, _ in STAT_DEFS}
        with open(a.json, 'w') as fh:
            json.dump(result, fh, indent=1)


# ---------------------------------------------------------------------------------------------------------------
# harness (b7 TA-TD and N1-N3, any subset of scenarios)
# ---------------------------------------------------------------------------------------------------------------

def harness_cmd(a):
    seeds = T.parse_seeds(a.seeds)
    if len(seeds) < 2:
        raise SystemExit('harness needs at least 2 seeds (jackknife)')
    req = [s for s in a.scenarios.split(',') if s]
    bad = [s for s in req if s not in HARNESS_SCENARIOS]
    if bad:
        raise SystemExit(f'harness scenarios must be among {HARNESS_SCENARIOS}; got {bad}')
    wanted = [s for s in HARNESS_SCENARIOS if s in req]

    result = {'cell': a.cell, 'base': a.base, 'n_seeds': len(seeds), 'stats': {}, 'paired': {}, 'targets': [],
              'skipped': []}
    present = []
    for sc in wanted:
        miss = missing_paths(a.out, a.cell, sc, seeds)
        if miss:
            msg = f'{sc}: {len(miss)} of {len(seeds)} files missing in cell {a.cell}'
            result['skipped'].append(msg)
            print(f'skipped {msg}')
        else:
            present.append(sc)

    per = {sc: T.load_cell(a.out, a.cell, sc, seeds) for sc in present}
    print(f'=== cell {a.cell} (n={len(seeds)} seeds) ===')
    for sc in present:
        st, _, _ = T.stat_with_se(per[sc], sc, seeds)
        result['stats'][sc] = {k: {'value': v[0], 'se': v[1]} for k, v in st.items()}
        print(f'-- {sc}')
        for k, (v, se) in st.items():
            print(f'   {k:8s} {T.fmt(v, se)}')

    if a.base:
        perb = {}
        for sc in present:
            miss = missing_paths(a.out, a.base, sc, seeds)
            if miss:
                msg = f'paired {sc}: {len(miss)} files missing in base cell {a.base}'
                result['skipped'].append(msg)
                print(f'skipped {msg}')
                continue
            perb[sc] = T.load_cell(a.out, a.base, sc, seeds)
        print(f'=== paired d = {a.cell} - {a.base} ===')
        for sc in present:
            if sc not in perb:
                continue
            d = T.paired_stat(per[sc], perb[sc], sc, seeds)
            result['paired'][sc] = {k: {'value': v[0], 'se': v[1]} for k, v in d.items()}
            print(f'-- {sc}')
            for k, (v, se) in d.items():
                print(f'   d{k:7s} {T.fmt(v, se)}')

    def cval(sc, k):
        return result['stats'][sc][k]['value'] if sc in result['stats'] else None

    def pval(sc, k):
        return result['paired'][sc][k]['value'] if sc in result['paired'] else None

    rows = []

    def row(name, value, rule, ok):
        if value is None:
            rows.append((name, None, rule, None))
            return
        passed = bool(ok(value))
        rows.append((name, value, rule, passed))
        result['targets'].append({'id': name, 'value': value, 'rule': rule, 'pass': passed})

    within = lambda lo, hi: (lambda x: lo <= x <= hi)  # noqa: E731
    row('TA core', cval('pol100', 'CORE'), '[-0.50,-0.20]', within(-0.50, -0.20))
    row('TB pin7 TR', cval('pin7', 'TR'), '[-7.0,-4.0]', within(-7.0, -4.0))
    row('TB pin7 UPK', cval('pin7', 'UPK'), '[2.5,5.5]', within(2.5, 5.5))
    row('TB pin10 TR', cval('pin10', 'TR'), '[-9.5,-5.0]', within(-9.5, -5.0))
    row('TB pin10 UPK', cval('pin10', 'UPK'), '[3.5,6.5]', within(3.5, 6.5))
    p7, p10 = cval('pin7', 'TR'), cval('pin10', 'TR')
    ratio = None if p7 is None or p10 is None else (p10 / p7 if p7 else float('nan'))
    row('TB TR10/TR7', ratio, '[1.10,1.60]', within(1.10, 1.60))
    brief = None
    if None not in (p7, p10, cval('pin7', 'UPK'), cval('pin10', 'UPK')):
        brief = (-6 <= p7 <= -3 and -6 <= p10 <= -3 and 3 <= cval('pin7', 'UPK') <= 6
                 and 3 <= cval('pin10', 'UPK') <= 6)
    result['brief_reading_pass'] = None if brief is None else bool(brief)
    rows.append(('TB brief reading', brief, 'both TR in [-6,-3], UPK in [3,6]', brief))
    row('TC qe05 BPT', cval('qe05', 'BPT'), '[-20,-15]', within(-20, -15))
    row('TC qe1 BPT', cval('qe1', 'BPT'), '[-20,-15]', within(-20, -15))
    row('TD dollar P6h', cval('pol100', 'P6h'), '[2.0,5.0]', within(2.0, 5.0))
    if a.base:
        row('N1 P1h', cval('pol100', 'P1h'), '>= -1.00', lambda x: x >= -1.00)
        row('N1 dP1h', pval('pol100', 'P1h'), '<= +0.10', lambda x: x <= 0.10)
        row('N2 dUPK', pval('pol100', 'UPK'), '>= -0.05', lambda x: x >= -0.05)
        row('N2 UPK', cval('pol100', 'UPK'), '<= 1.0', lambda x: x <= 1.0)
        dy = pval('pol100', 'Y10_3')
        row('N3 |dY10_3|', None if dy is None else abs(dy), '<= 0.03', lambda x: x <= 0.03)

    print('=== targets ===')
    for name, value, rule, passed in rows:
        if value is None:
            vs = 'n/a'
        elif isinstance(value, float):
            vs = f'{value:.3f}'
        else:
            vs = str(value)
        status = 'n/a' if passed is None else ('PASS' if passed else 'FAIL')
        print(f'  {name:18s} {vs:>10s}  {rule:36s} {status}')
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(result, fh, indent=1)


# ---------------------------------------------------------------------------------------------------------------
# sacrifice
# ---------------------------------------------------------------------------------------------------------------

def sr_from_paths(paths, H):
    """(numerator, denominator, SR_H) on mean paths; SR_H is None when the denominator is <= 0."""
    gp, cp = paths.get('gdp'), paths.get('core')
    if gp is None or cp is None or min(len(gp), len(cp)) < H:
        return None
    num = -sum(gp[:H]) / 12.0
    den = -min(cp[:H])
    return num, den, (num / den if den > 0 else None)


def sacrifice_cell(gaps, n, H):
    point = sr_from_paths(leave_out_paths(gaps, None), H)
    loo = [sr_from_paths(leave_out_paths(gaps, i), H) for i in range(n)] if n >= 2 else None
    comps = []
    for c in range(3):
        v = None if point is None else point[c]
        se = None
        if (loo is not None and v is not None and all(t is not None and t[c] is not None for t in loo)):
            se = T.jackknife_se([t[c] for t in loo])
        comps.append((v, se))
    return comps


def sacrifice_paired(gt, gb, n, H):
    pt = sr_from_paths(leave_out_paths(gt, None), H)
    pb = sr_from_paths(leave_out_paths(gb, None), H)
    dif = None if pt is None or pb is None or pt[2] is None or pb[2] is None else pt[2] - pb[2]
    if n < 2:
        return dif, None
    dl = []
    for i in range(n):
        st = sr_from_paths(leave_out_paths(gt, i), H)
        sb = sr_from_paths(leave_out_paths(gb, i), H)
        dl.append(None if st is None or sb is None or st[2] is None or sb[2] is None else st[2] - sb[2])
    se = T.jackknife_se(dl) if dif is not None and all(x is not None for x in dl) else None
    return dif, se


def sacrifice_cmd(a):
    seeds = T.parse_seeds(a.seeds)
    if len(seeds) < 1:
        raise SystemExit('no seeds')
    if a.scenario not in ('pin7', 'pin10'):
        raise SystemExit('--scenario must be pin7 or pin10')
    horizons = sorted({int(x) for x in a.horizons.split(',') if x.strip()})
    need = max(horizons)
    cells = [a.cell] + ([a.base] if a.base else [])
    gaps = {}
    for cell in cells:
        docs = load_docs(a.out, cell, a.scenario, seeds)
        gdp, core = [], []
        for d in docs:
            g = gap_series(d, ('f', 'gdp_real_bn_ar'), 'pct')
            c = gap_series(d, ('f', 'core_inflation_yoy'), 'diff')
            if g is None or c is None:
                raise SystemExit(f'cell {cell}: gdp_real_bn_ar or core_inflation_yoy missing')
            if min(len(g), len(c)) < need:
                raise SystemExit(f'cell {cell}: runs have {min(len(g), len(c))} months, need {need}')
            gdp.append(g)
            core.append(c)
        gaps[cell] = {'gdp': gdp, 'core': core}

    n = len(seeds)
    result = {'scenario': a.scenario, 'cell': a.cell, 'base': a.base, 'n_seeds': n, 'horizons': {}}
    print(f'=== sacrifice {a.scenario} (cell {a.cell}, n={n} seeds) ===')
    for H in horizons:
        comps = sacrifice_cell(gaps[a.cell], n, H)
        result['horizons'][str(H)] = {'cell': {'numerator': pack(comps[0]), 'denominator': pack(comps[1]),
                                               'SR': pack(comps[2])}}
        print(f'-- H={H}')
        print(f'   numerator   (%-years of GDP)   {fmt_pair(*comps[0])}')
        print(f'   denominator (pp core disinfl.) {fmt_pair(*comps[1])}')
        print(f'   SR_{H}                          {fmt_pair(*comps[2])}')
    if a.base:
        print(f'=== paired d = {a.cell} - {a.base} ===')
        for H in horizons:
            bcomps = sacrifice_cell(gaps[a.base], n, H)
            dsr, dse = sacrifice_paired(gaps[a.cell], gaps[a.base], n, H)
            result['horizons'][str(H)]['base'] = {'SR': pack(bcomps[2])}
            result['horizons'][str(H)]['paired_SR'] = {'value': dsr, 'se': dse}
            print(f'-- H={H}')
            print(f'   SR_{H} base {a.base}              {fmt_pair(*bcomps[2])}')
            print(f'   dSR_{H}                         {fmt_pair(dsr, dse)}')
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(result, fh, indent=1)


# ---------------------------------------------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------------------------------------------

def build_parser():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)

    r = sub.add_parser('run', help='run shock scenarios, one seed per request, store per-seed series')
    r.add_argument('--server', required=True, help='econsim-server binary')
    r.add_argument('--cell', required=True)
    r.add_argument('--cfg', default=None, help='JSON overlay (keys must be in GET /api/config/default)')
    r.add_argument('--scenarios', required=True, help='comma list, e.g. oil50,pol100')
    r.add_argument('--seeds', required=True, help='e.g. 1-48 or 1,5,9')
    r.add_argument('--households', type=int, default=5000)
    r.add_argument('--months', type=int, default=60)
    r.add_argument('--servers', type=int, default=4, help='server processes, one simulation each')
    r.add_argument('--port0', type=int, required=True, help='first port; the block is port0 .. port0+servers-1')
    r.add_argument('--warmup', type=int, default=0, help='warm-up months (server must have warmup_months)')
    r.add_argument('--out', required=True)
    r.set_defaults(fn=run_cmd)

    w = sub.add_parser('windows', help='year-window means of the gaps, pass-through ratios, statistics')
    w.add_argument('--out', required=True)
    w.add_argument('--cell', required=True)
    w.add_argument('--base', default=None)
    w.add_argument('--scenario', required=True)
    w.add_argument('--seeds', required=True)
    w.add_argument('--rows', choices=['ref', 'all'], default='all')
    w.add_argument('--ratio', action='append', default=[], metavar='NUM:DEN',
                   help='e.g. biz:policy (repeatable); names: table labels or biz, firm_loan, policy, mortgage, 10y')
    w.add_argument('--json', default=None)
    w.set_defaults(fn=windows_cmd)

    h = sub.add_parser('harness', help='b7 TA-TD and N1-N3 statistics for any subset of scenarios')
    h.add_argument('--out', required=True)
    h.add_argument('--cell', required=True)
    h.add_argument('--base', default=None)
    h.add_argument('--seeds', required=True)
    h.add_argument('--scenarios', default='pol100,pin7,pin10,qe05,qe1')
    h.add_argument('--json', default=None)
    h.set_defaults(fn=harness_cmd)

    s = sub.add_parser('sacrifice', help='Volcker sacrifice ratio SR_H on the mean path')
    s.add_argument('--out', required=True)
    s.add_argument('--cell', required=True)
    s.add_argument('--base', default=None)
    s.add_argument('--scenario', required=True, choices=['pin7', 'pin10'])
    s.add_argument('--seeds', required=True)
    s.add_argument('--horizons', default='36,60')
    s.add_argument('--json', default=None)
    s.set_defaults(fn=sacrifice_cmd)
    return ap


def main():
    a = build_parser().parse_args()
    a.fn(a)


if __name__ == '__main__':
    sys.exit(main())
