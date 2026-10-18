#!/usr/bin/env python3
"""Monetary-transmission harness (b7-transmission): paired per-seed IRF-style statistics from `econsim-server`.

Usage:
  python3 -B scripts/transmission_irf.py run   --server BIN --cell C --cfg FILE --scenarios pol100,pin7,pin10,qe05,qe1 \
                                               --seeds 1-32 --households 10000 --months 36 --servers 6 --port0 8930 --out DIR
  python3 -B scripts/transmission_irf.py stats --out DIR --cell C [--base B] [--seeds 1-32] [--json FILE]

`run` starts `--servers` copies of econsim-server (PORT set, RAYON_NUM_THREADS=1), sends one POST /api/compare per
(scenario, seed) with a single seed, and stores DIR/<C>/<scenario>_s<seed>.json.gz (baseline and shocked monthly
series of the fields in FIELDS). Existing files are skipped. Overlay keys must exist in GET /api/config/default.

`stats` averages the per-seed paths over seeds (percent for GDP and the dollar, differences otherwise, $bn for the
CB balance sheet), computes each statistic on the mean path, and gives the leave-one-seed-out jackknife SE. With
--base the paired difference d = stat(cell) - stat(base) uses the same left-out seed. The target block (TA-TD) and
the +100bp non-regression block (N1-N3) are printed with PASS/FAIL.

Units. Rates (policy_rate, yields, business_loan_rate, mortgage_rate, deposit_rate) are in percent (pp) in the
Metrics output, so differences are already pp. The dollar and GDP are percent (100*(s/b-1)).
Month t (1..36) is record t-1 of the series.
"""
import argparse, atexit, gzip, json, os, queue, subprocess, sys, time, urllib.error, urllib.request
from concurrent.futures import ThreadPoolExecutor

MONTHS_DEFAULT = 36
FIELDS = [
    'gdp_real_bn_ar', 'consumption_real_bn_ar', 'investment_real_bn_ar', 'government_real_bn_ar',
    'exports_bn_ar', 'imports_bn_ar', 'gdp_nominal_bn_ar', 'unemployment_rate', 'core_inflation_yoy',
    'core_pce_inflation_yoy', 'inflation_yoy', 'expected_inflation', 'policy_rate', 'yields', 'term_premium',
    'dollar_index', 'business_loan_rate', 'deposit_rate', 'mortgage_rate', 'cb_bonds_bn',
    'discretionary_multiplier_mean', 'avg_markup', 'wage_growth_yoy', 'median_house_price',
]
SCENARIOS = {
    'pol100': [{'type': 'PolicyShift', 'bps': 100.0, 'months': 12}],
    'pin7': [{'type': 'PolicyRate', 'rate': 0.07, 'months': 24}],
    'pin10': [{'type': 'PolicyRate', 'rate': 0.10, 'months': 24}],
    'qe05': [{'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 0.5, 'months': 12}],
    'qe1': [{'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 1.0, 'months': 12}],
}
# Tenor indices: 0=3m, 1=2y, 2=5y, 3=10y, 4=30y.
Y2, Y10 = 1, 3


def parse_seeds(text):
    out = []
    for part in text.split(','):
        part = part.strip()
        if '-' in part:
            a, b = part.split('-')
            out.extend(range(int(a), int(b) + 1))
        elif part:
            out.append(int(part))
    return out


def base_url(port):
    return f'http://127.0.0.1:{port}'


def http_json(url, payload=None, timeout=900):
    if payload is None:
        req = urllib.request.Request(url)
    else:
        req = urllib.request.Request(url, data=json.dumps(payload).encode(), headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.load(r)


def health(port):
    try:
        http_json(base_url(port) + '/api/health', timeout=1)
        return True
    except (urllib.error.URLError, OSError, ValueError):
        return False


# ---------------------------------------------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------------------------------------------

def extract(records, months):
    if len(records) < months:
        raise SystemExit(f'compare returned {len(records)} records, need {months}')
    out = {}
    for f in FIELDS:
        out[f] = [r[f] for r in records[:months]]
    return out


def run_cmd(a):
    scen_names = [s for s in a.scenarios.split(',') if s]
    for s in scen_names:
        if s not in SCENARIOS:
            raise SystemExit(f'unknown scenario {s}')
    seeds = parse_seeds(a.seeds)
    overlay = json.load(open(a.cfg)) if a.cfg else {}
    if not isinstance(overlay, dict):
        raise SystemExit('cfg must be a JSON object')
    ports = [a.port0 + i for i in range(a.servers)]
    busy = [p for p in ports if health(p)]
    if busy:
        raise SystemExit(f'ports already answering /api/health: {busy}; choose another --port0')

    cell_dir = os.path.join(a.out, a.cell)
    os.makedirs(cell_dir, exist_ok=True)
    os.makedirs(os.path.join(a.out, 'logs'), exist_ok=True)

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
            env = dict(os.environ, PORT=str(p), RAYON_NUM_THREADS='1', ECONSIM_WARMUP_MONTHS='0')
            log = open(os.path.join(a.out, 'logs', f'server_{p}.log'), 'w')
            procs.append(subprocess.Popen([a.server], env=env, stdout=log, stderr=subprocess.STDOUT))
        for p in ports:
            t0 = time.time()
            while not health(p):
                if time.time() - t0 > 60:
                    raise SystemExit(f'server on port {p} did not start')
                time.sleep(0.2)

        defaults = http_json(base_url(ports[0]) + '/api/config/default')
        unknown = [k for k in overlay if k not in defaults]
        if unknown:
            raise SystemExit(f'overlay keys not in /api/config/default: {unknown}')

        jobs = []
        for sc in scen_names:
            for seed in seeds:
                path = os.path.join(cell_dir, f'{sc}_s{seed}.json.gz')
                if not os.path.exists(path):
                    jobs.append((sc, seed, path))
        print(f'cell {a.cell}: {len(jobs)} jobs to run ({len(scen_names) * len(seeds) - len(jobs)} already present)',
              flush=True)

        free = queue.Queue()
        for p in ports:
            free.put(p)

        def job(item):
            sc, seed, path = item
            port = free.get()
            try:
                cfg = dict(overlay)
                cfg['n_households'] = a.households
                cfg['warmup_months'] = 0      # stage 4 protocol (spec 3.0 item 6): the server default is 24 since J4
                payload = {'config': cfg, 'shocks': [{'at_month': 0, 'shock': s} for s in SCENARIOS[sc]],
                           'months': a.months, 'seeds': [seed]}
                cmp = http_json(base_url(port) + '/api/compare', payload)
                doc = {'meta': {'cell': a.cell, 'scenario': sc, 'seed': seed, 'months': a.months,
                                'households': a.households},
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
# stats
# ---------------------------------------------------------------------------------------------------------------

def load_seed(path):
    with gzip.open(path, 'rt') as fh:
        return json.load(fh)


def pct(s, b):
    return [100.0 * (x / y - 1.0) for x, y in zip(s, b)]


def diff(s, b):
    return [x - y for x, y in zip(s, b)]


def series_of(doc):
    """Transformed per-seed series, keyed by name, each a list of 36 values (index t-1)."""
    B, S = doc['baseline'], doc['shocked']
    out = {}
    out['gdp'] = pct(S['gdp_real_bn_ar'], B['gdp_real_bn_ar'])
    out['dol'] = pct(S['dollar_index'], B['dollar_index'])
    out['u'] = diff(S['unemployment_rate'], B['unemployment_rate'])
    out['core'] = diff(S['core_inflation_yoy'], B['core_inflation_yoy'])
    out['cpce'] = diff(S['core_pce_inflation_yoy'], B['core_pce_inflation_yoy'])
    out['pol'] = diff(S['policy_rate'], B['policy_rate'])
    out['biz'] = diff(S['business_loan_rate'], B['business_loan_rate'])
    out['cb'] = diff(S['cb_bonds_bn'], B['cb_bonds_bn'])
    for k in range(5):
        out[f'y{k}'] = diff([row[k] for row in S['yields']], [row[k] for row in B['yields']])
    return out


def mean_series(per_seed, seeds):
    keys = per_seed[seeds[0]].keys()
    n = len(seeds)
    return {k: [sum(per_seed[s][k][t] for s in seeds) / n for t in range(len(per_seed[seeds[0]][k]))] for k in keys}


def stat_pol100(m):
    g, u, c, cp, dol, pol, y10 = m['gdp'], m['u'], m['core'], m['cpce'], m['dol'], m['pol'], m['y3']
    p1h = min(g[t - 1] for t in (3, 6, 12, 24))
    trm = min(g[0:24])
    upk = max(u[0:24])
    core = min(c[5:24])
    cpce_v = min(cp[5:24])
    pmax = max(pol[t - 1] for t in (3, 6, 12))
    dmax = max(dol[t - 1] for t in (3, 6, 12))
    return {
        'P1h': p1h, 'TRm': trm, 'UPK': upk, 'CORE': core, 'CPCE': cpce_v,
        'C12': c[11], 'C24': c[23], 'P6h': dmax / pmax, 'DOL3': dol[2], 'Y10_3': m['y3'][2],
    }


def stat_pin(m):
    g = m['gdp']
    tr = min(g[0:36])
    trm = g.index(tr) + 1
    return {
        'TR': tr, 'TRM': float(trm), 'UPK': max(m['u'][0:36]), 'CORE': min(m['core'][5:24]),
        'DOL12': m['dol'][11], 'Y2_3': m['y1'][2], 'Y10_3': m['y3'][2], 'BIZ_3': m['biz'][2],
    }


def stat_qe(m):
    y10_12 = m['y3'][11]
    cb12 = m['cb'][11] / 1000.0
    return {
        'Y10_3': m['y3'][2], 'Y10_12': y10_12, 'CB12': cb12, 'BPT': 100.0 * y10_12 / cb12, 'GDP12': m['gdp'][11],
    }


def stats_for(sc, m):
    if sc == 'pol100':
        return stat_pol100(m)
    if sc in ('pin7', 'pin10'):
        return stat_pin(m)
    return stat_qe(m)


def jackknife_se(vals):
    n = len(vals)
    mean = sum(vals) / n
    return ((n - 1) / n * sum((v - mean) ** 2 for v in vals)) ** 0.5


def load_cell(out, cell, scen, seeds):
    per = {}
    for s in seeds:
        path = os.path.join(out, cell, f'{scen}_s{s}.json.gz')
        if not os.path.exists(path):
            raise SystemExit(f'missing {path}')
        per[s] = series_of(load_seed(path))
    return per


def stat_with_se(per, scen, seeds):
    """Statistic dict with (value, jackknife SE) on the mean path."""
    point = stats_for(scen, mean_series(per, seeds))
    loo = {}
    for left in seeds:
        rest = [s for s in seeds if s != left]
        st = stats_for(scen, mean_series(per, rest))
        for k, v in st.items():
            loo.setdefault(k, []).append(v)
    return {k: (point[k], jackknife_se(loo[k])) for k in point}, point, loo


def paired_stat(per_t, per_b, scen, seeds):
    """d = stat(T) - stat(B) on the mean path, with the jackknife over the same left-out seed."""
    pt = stats_for(scen, mean_series(per_t, seeds))
    pb = stats_for(scen, mean_series(per_b, seeds))
    dloo = {}
    for left in seeds:
        rest = [s for s in seeds if s != left]
        st = stats_for(scen, mean_series(per_t, rest))
        sb = stats_for(scen, mean_series(per_b, rest))
        for k in pt:
            dloo.setdefault(k, []).append(st[k] - sb[k])
    return {k: (pt[k] - pb[k], jackknife_se(dloo[k])) for k in pt}


def fmt(v, se=None):
    return f'{v:9.3f}' if se is None else f'{v:9.3f} ± {se:6.3f}'


def stats_cmd(a):
    seeds = parse_seeds(a.seeds)
    result = {'cell': a.cell, 'base': a.base, 'n_seeds': len(seeds), 'stats': {}, 'paired': {}, 'targets': []}
    per = {}
    for sc in SCENARIOS:
        per[sc] = load_cell(a.out, a.cell, sc, seeds)
    print(f'=== cell {a.cell} (n={len(seeds)} seeds) ===')
    for sc in SCENARIOS:
        st, point, loo = stat_with_se(per[sc], sc, seeds)
        result['stats'][sc] = {k: {'value': v[0], 'se': v[1]} for k, v in st.items()}
        print(f'-- {sc}')
        for k, (v, se) in st.items():
            print(f'   {k:8s} {fmt(v, se)}')
    if a.base:
        perb = {}
        for sc in SCENARIOS:
            perb[sc] = load_cell(a.out, a.base, sc, seeds)
        print(f'=== paired d = {a.cell} - {a.base} ===')
        for sc in SCENARIOS:
            d = paired_stat(per[sc], perb[sc], sc, seeds)
            result['paired'][sc] = {k: {'value': v[0], 'se': v[1]} for k, v in d.items()}
            print(f'-- {sc}')
            for k, (v, se) in d.items():
                print(f'   d{k:7s} {fmt(v, se)}')

    # Target block (section 2.1) and non-regression (section 2.2).
    st = {sc: {k: d['value'] for k, d in result['stats'][sc].items()} for sc in SCENARIOS}
    ok = lambda lo, hi, x: lo <= x <= hi
    rows = []

    def row(name, value, rule, passed):
        rows.append((name, value, rule, passed))
        result['targets'].append({'id': name, 'value': value, 'rule': rule, 'pass': bool(passed)})

    row('TA core', st['pol100']['CORE'], '[-0.50,-0.20]', ok(-0.50, -0.20, st['pol100']['CORE']))
    row('TB pin7 TR', st['pin7']['TR'], '[-7.0,-4.0]', ok(-7.0, -4.0, st['pin7']['TR']))
    row('TB pin7 UPK', st['pin7']['UPK'], '[2.5,5.5]', ok(2.5, 5.5, st['pin7']['UPK']))
    row('TB pin10 TR', st['pin10']['TR'], '[-9.5,-5.0]', ok(-9.5, -5.0, st['pin10']['TR']))
    row('TB pin10 UPK', st['pin10']['UPK'], '[3.5,6.5]', ok(3.5, 6.5, st['pin10']['UPK']))
    ratio = st['pin10']['TR'] / st['pin7']['TR'] if st['pin7']['TR'] else float('nan')
    row('TB TR10/TR7', ratio, '[1.10,1.60]', ok(1.10, 1.60, ratio))
    brief = (-6 <= st['pin7']['TR'] <= -3 and -6 <= st['pin10']['TR'] <= -3 and
             3 <= st['pin7']['UPK'] <= 6 and 3 <= st['pin10']['UPK'] <= 6)
    result['brief_reading_pass'] = bool(brief)
    rows.append(('TB brief reading', brief, 'both TR in [-6,-3], UPK in [3,6]', brief))
    row('TC qe05 BPT', st['qe05']['BPT'], '[-20,-15]', ok(-20, -15, st['qe05']['BPT']))
    row('TC qe1 BPT', st['qe1']['BPT'], '[-20,-15]', ok(-20, -15, st['qe1']['BPT']))
    row('TD dollar P6h', st['pol100']['P6h'], '[2.0,5.0]', ok(2.0, 5.0, st['pol100']['P6h']))
    if a.base:
        dp = result['paired']
        row('N1 P1h', st['pol100']['P1h'], '>= -1.00', st['pol100']['P1h'] >= -1.00)
        row('N1 dP1h', dp['pol100']['P1h']['value'], '<= +0.10', dp['pol100']['P1h']['value'] <= 0.10)
        row('N2 dUPK', dp['pol100']['UPK']['value'], '>= -0.05', dp['pol100']['UPK']['value'] >= -0.05)
        row('N2 UPK', st['pol100']['UPK'], '<= 1.0', st['pol100']['UPK'] <= 1.0)
        row('N3 |dY10_3|', abs(dp['pol100']['Y10_3']['value']), '<= 0.03', abs(dp['pol100']['Y10_3']['value']) <= 0.03)
    print('=== targets ===')
    for name, value, rule, passed in rows:
        vs = f'{value:.3f}' if isinstance(value, float) else str(value)
        print(f'  {name:18s} {vs:>10s}  {rule:36s} {"PASS" if passed else "FAIL"}')
    if a.json:
        with open(a.json, 'w') as fh:
            json.dump(result, fh, indent=1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('run')
    r.add_argument('--server', required=True)
    r.add_argument('--cell', required=True)
    r.add_argument('--cfg', default=None, help='JSON overlay file; omit for defaults')
    r.add_argument('--scenarios', default='pol100,pin7,pin10,qe05,qe1')
    r.add_argument('--seeds', default='1-32')
    r.add_argument('--households', type=int, default=10000)
    r.add_argument('--months', type=int, default=MONTHS_DEFAULT)
    r.add_argument('--servers', type=int, default=6)
    r.add_argument('--port0', type=int, default=8930)
    r.add_argument('--out', required=True)
    r.set_defaults(fn=run_cmd)
    s = sub.add_parser('stats')
    s.add_argument('--out', required=True)
    s.add_argument('--cell', required=True)
    s.add_argument('--base', default=None)
    s.add_argument('--seeds', default='1-32')
    s.add_argument('--json', default=None)
    s.set_defaults(fn=stats_cmd)
    a = ap.parse_args()
    a.fn(a)


if __name__ == '__main__':
    sys.exit(main())
