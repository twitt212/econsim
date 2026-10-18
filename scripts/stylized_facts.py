#!/usr/bin/env python3
"""Combined stylised-facts scorecard: run every scripts/facts_*.py against one econsim-cli binary.

Usage:
  python3 scripts/stylized_facts.py [path/to/econsim-cli] [options]

  The binary is the first positional argument, else $ECONSIM_CLI, else ./target/release/econsim-cli (relative to the
  repository root). ECONSIM_CFG is passed through to every script, and so to every model run. Only numpy (used by the
  facts scripts) and the standard library are needed.

Options:
  --only a,b       run only these dimensions (equity, rates, cycle, shocks, distribution, firms, or any facts_<dim>.py)
  --skip a,b       skip these dimensions
  --jobs N         facts scripts running at the same time (default: all of them)
  --max-sims N     model runs at the same time, summed over all scripts (default 8; each run is single-threaded)
  --cache DIR      keep the shared run cache in DIR and reuse it later (default: a temporary directory removed at
                   exit). Entries are keyed on the binary (real path, size, mtime), ECONSIM_CFG and the run arguments,
                   so a rebuilt binary or a different config never reuses a stale run. A 240-month run is ~1.5 MB.
  --no-share       no shared cache and no run limit: every script calls the binary directly
  --logs DIR       save each script's full printed table to DIR/<dimension>.txt
  --verbose        also print each script's full table when it finishes
  --detail         list every fact (default: only the severe ones)
  --json OUT       write the scorecard (counts, run info, and every fact with target, model, gap, sample, source)
  --markdown OUT   write every fact as Markdown tables, one per dimension (used for docs/STYLIZED_FACTS.md)
  --baseline FILE  compare with an earlier --json scorecard and list every fact whose gap flag changed
  --seeds N / --months N / --hh N
                   override the standard run set of every script. For smoke tests only: the targets and the
                   reference scorecard in docs/STYLIZED_FACTS.md assume 8 seeds x 240 months x 20,000 households.

How runs are shared. The six scripts mostly simulate the same runs (seeds 1-8, 240 months, 20,000 households), and
the model is deterministic given binary, config and arguments. Unless --no-share is given, each script is handed a
small wrapper in place of the binary. The wrapper serves `run ... --json` (and the `exits` diagnostic binary) from a
cache, computing each distinct run once under a per-run file lock, and holds one of --max-sims slots while the real
binary runs. Every other command goes straight to the real binary. Simulations are therefore never duplicated, even
when scripts run concurrently, and the machine never runs more than --max-sims of them at once.

Gap flags are each script's own (ok inside the target range; otherwise minor / material / severe by the distance
outside the range in units of that fact's tolerance; see the script docstrings). Facts that the CLI JSON cannot
measure are counted as n/a. The exit status is 1 if any script failed, otherwise 0 (gaps never fail the run).

New dimensions: any scripts/facts_<dim>.py is picked up. It receives the binary as its first argument and
$FACTS_RESULT_JSON, a path where it should write {"facts": [{"id", "name", "target", "lo", "hi", "model", "gap",
"sample", "source"}, ...]}. If it writes nothing, rows whose last word is a gap flag are counted from its stdout.
"""
import fcntl
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
ORDER = ['cycle', 'rates', 'equity', 'shocks', 'distribution', 'firms']
FLAGS = ['ok', 'minor', 'material', 'severe', 'n/a']
SEV = {'ok': 0, 'minor': 1, 'material': 2, 'severe': 3}


# ---------------------------------------------------------------------------------------------------------------
# Shared-run wrapper (invoked as: stylized_facts.py --shim-exec REAL CACHE NSLOTS args...)
# ---------------------------------------------------------------------------------------------------------------
def shim_main(argv):
    real, cache, nslots, args = argv[0], argv[1], int(argv[2]), argv[3:]
    cacheable = (args[:1] == ['run'] and '--json' in args) or os.path.basename(real) == 'exits'
    if not cacheable:
        os.execv(real, [real] + args)
    st = os.stat(real)
    ident = json.dumps([os.path.realpath(real), st.st_size, st.st_mtime_ns, os.environ.get('ECONSIM_CFG', ''), args])
    key = hashlib.sha256(ident.encode()).hexdigest()[:24]
    out_path = os.path.join(cache, 'runs', key + '.out')
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    with open(out_path + '.lock', 'w') as lk:
        fcntl.flock(lk, fcntl.LOCK_EX)  # one computation per distinct run; others wait and then read the result
        if not os.path.exists(out_path):
            slot = acquire_slot(cache, nslots)
            t0 = time.time()
            tmp = f'{out_path}.{os.getpid()}.tmp'
            try:
                with open(tmp, 'wb') as fo:
                    rc = _run_to(real, args, fo)
            finally:
                fcntl.flock(slot, fcntl.LOCK_UN)
                slot.close()
            if rc != 0:
                os.unlink(tmp)
                sys.exit(rc)
            os.replace(tmp, out_path)
            log(cache, f'miss {time.time() - t0:.1f} {os.path.basename(real)} {" ".join(args)}')
        else:
            log(cache, f'hit 0 {os.path.basename(real)} {" ".join(args)}')
    with open(out_path, 'rb') as fi:
        shutil.copyfileobj(fi, sys.stdout.buffer)
    sys.stdout.flush()


def _run_to(real, args, fo):
    p = subprocess.run([real] + args, stdout=fo, stderr=subprocess.PIPE)
    if p.returncode != 0:
        sys.stderr.buffer.write(p.stderr[-4000:])
    return p.returncode


def acquire_slot(cache, nslots):
    d = os.path.join(cache, 'slots')
    os.makedirs(d, exist_ok=True)
    while True:
        for i in range(max(1, nslots)):
            f = open(os.path.join(d, f'slot{i}'), 'w')
            try:
                fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
                return f
            except OSError:
                f.close()
        time.sleep(0.2)


def log(cache, line):
    with open(os.path.join(cache, 'shim.log'), 'a') as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        f.write(line + '\n')


def make_shim(real, cache, nslots, name):
    d = os.path.join(cache, 'bin')
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, name)
    q = lambda s: "'" + s.replace("'", "'\\''") + "'"
    with open(path, 'w') as f:
        f.write(f'#!/bin/sh\nexec {q(sys.executable)} {q(os.path.abspath(__file__))} --shim-exec {q(real)} {q(cache)} '
                f'{nslots} "$@"\n')
    os.chmod(path, 0o755)
    return path


# ---------------------------------------------------------------------------------------------------------------
# Per-script adapters: how to ask each script for machine-readable output, and how to read it back.
# Each parser returns rows dict(id, name, target, lo, hi, model, model_text, gap, sample, source).
# ---------------------------------------------------------------------------------------------------------------
def num(x):
    return x if isinstance(x, (int, float)) and x == x else None


def fmt(v):
    if v is None:
        return 'n/a'
    if isinstance(v, (list, tuple)):
        return '[' + ', '.join(fmt(x) for x in v) + ']'
    a = abs(v)
    return f'{v:.0f}' if a >= 1000 else f'{v:.1f}' if a >= 100 else f'{v:.2f}' if a >= 1 else f'{v:.3g}'


def rng_text(lo, hi):
    return f'{fmt(lo)} .. {fmt(hi)}' if lo is not None and hi is not None else ''


def norm_flag(g):
    g = (g or 'n/a').lower()
    return g if g in SEV else 'n/a'


def parse_equity(tj, rj):
    T, R = load(tj)['targets'], load(rj)
    out = []
    for t in T:
        v = R['model'].get(t['key'])
        core = t.get('core', True)
        out.append(dict(id=t['key'], name=t['fact'] + ('' if core else ' (supplementary)'),
                        target=f"{rng_text(t['lo'], t['hi'])}; US {t['us']}", lo=t['lo'], hi=t['hi'],
                        model=v[0] if v else None,
                        model_text='n/a' if not v else f'{fmt(v[0])} [{fmt(v[1])}..{fmt(v[2])}, n={v[3]}]',
                        gap=norm_flag(R['flags'].get(t['key'])), sample=t.get('sample', ''), source=t['source']))
    return out


def parse_rates(tj, rj):
    T = {t['id']: t for t in load(tj)['targets']}
    out = []
    for r in load(rj)['rows']:
        t = T.get(r['id'], {})
        m = num(r.get('model'))
        sd = num(r.get('model_sd'))
        tgt = f"{fmt(r['target'])} [{rng_text(r['lo'], r['hi'])}]" + (f"; {t['alt']}" if t.get('alt') else '')
        out.append(dict(id=r['id'], name=r['label'], target=tgt, lo=r['lo'], hi=r['hi'], model=m,
                        model_text=fmt(m) + (f' (seed sd {fmt(sd)})' if sd is not None else ''),
                        gap=norm_flag(r['gap']), sample=t.get('sample', ''), source=t.get('source', '')))
    return out


def parse_cycle(tj, rj):
    out = []
    for r in load(rj)['rows']:
        m, sd = num(r.get('model')), num(r.get('model_seed_sd'))
        out.append(dict(id=r['key'], name=r['label'], target=f"{rng_text(r['lo'], r['hi'])}; point {fmt(r['point'])}",
                        lo=r['lo'], hi=r['hi'], model=m,
                        model_text=fmt(m) + (f' (seed sd {fmt(sd)})' if sd else ''),
                        gap=norm_flag(r['gap']), sample=r.get('sample', ''), source=r.get('source', '')))
    return out


def parse_shocks(tj, rj):
    T = load(tj)['targets']
    R = {r['id']: r for r in load(rj)['results']}
    out = []
    for t in T:
        r = R.get(t['id'], {})
        m, ml, seeds = num(r.get('model')), num(r.get('model_long')), r.get('seeds') or []
        txt = fmt(m) + (f' [{fmt(min(seeds))}..{fmt(max(seeds))}]' if seeds else '') + \
            (f'; long runs {fmt(ml)}' if ml is not None else '')
        out.append(dict(id=t['id'], name=t['name'], target=f"{fmt(t['target'])} ({rng_text(t['lo'], t['hi'])})",
                        lo=t['lo'], hi=t['hi'], model=m, model_text=txt, gap=norm_flag(r.get('gap')),
                        sample=t.get('sample', ''), source=t['source']))
    return out


def parse_distribution(tj, rj):
    T, R = load(tj)['targets'], load(rj)
    out = []
    for t in T:
        measurable = t.get('measurable', True)
        m = num(R['model'].get(t['id'])) if measurable else None
        sd = num(R['model'].get(t['id'] + '_sd')) if measurable else None
        tgt = rng_text(t['lo'], t['hi']) or (fmt(t['central']) if t.get('central') is not None else 'see source')
        out.append(dict(id=t['id'], name=t['name'], target=tgt, lo=t['lo'], hi=t['hi'], model=m,
                        model_text=fmt(m) + (f' (seed sd {fmt(sd)})' if sd is not None else ''),
                        gap=norm_flag(R['flags'].get(t['id'])), sample=t.get('sample', ''), source=t['source']))
    return out


def parse_firms(tj, rj):
    out = []
    for f in load(rj)['facts']:
        lo, hi = f['range']
        mm = re.match(r'\s*([-+]?\d+(?:\.\d+)?)', f.get('model_master') or '')
        out.append(dict(id=f['key'], name=f['fact'], target=f"{f['target']} [{rng_text(lo, hi)}]", lo=lo, hi=hi,
                        model=float(mm.group(1)) if mm else None, model_text=f.get('model_master') or 'n/a',
                        gap=norm_flag(f['gap']), sample=f.get('sample', ''), source=f.get('source', '')))
    return out


def parse_generic(tj, rj, stdout=''):
    if rj and os.path.exists(rj):
        out = []
        for f in load(rj)['facts']:
            out.append(dict(id=f.get('id', f.get('name')), name=f.get('name', f.get('id')),
                            target=str(f.get('target', rng_text(f.get('lo'), f.get('hi')))), lo=f.get('lo'),
                            hi=f.get('hi'), model=num(f.get('model')), model_text=fmt(num(f.get('model'))),
                            gap=norm_flag(f.get('gap')), sample=f.get('sample', ''), source=f.get('source', '')))
        return out
    out = []
    for line in stdout.splitlines():
        w = line.strip().split()
        if len(w) > 2 and w[-1].lower() in ('ok', 'minor', 'material', 'severe', 'n/a', 'unmeasurable'):
            out.append(dict(id=line[:40].strip(), name=line[:60].strip(' |'), target='', lo=None, hi=None, model=None,
                            model_text='', gap=norm_flag(w[-1]), sample='', source='(parsed from stdout)'))
    return out


def load(p):
    with open(p) as f:
        return json.load(f)


# args(cli, targets_path, result_path, size) -> argv; env(...) -> extra environment; pre: argv run first (no sims)
ADAPTERS = {
    'equity': dict(
        args=lambda c, t, r, s: [c, '--targets-json', t],
        env=lambda t, r, s: dict({'EQF_JSON_OUT': r}, **sized(s, 'EQF_SEEDS', 'EQF_MONTHS', 'EQF_HH')),
        parse=parse_equity),
    'rates': dict(
        pre=lambda t: ['--dump-targets', t],
        args=lambda c, t, r, s: [c],
        env=lambda t, r, s: dict({'RATES_OUT': r}, **sized(s, 'RATES_SEEDS', 'RATES_MONTHS', 'RATES_HH')),
        parse=parse_rates),
    'cycle': dict(
        args=lambda c, t, r, s: [c, '--json', r] + sum((['--' + k, str(s[k])] for k in ('seeds', 'months', 'hh')
                                                         if s.get(k)), []),
        env=lambda t, r, s: {},
        parse=parse_cycle),
    'shocks': dict(
        args=lambda c, t, r, s: [c, '--targets-json', t],
        env=lambda t, r, s: dict({'SHOCKS_RESULTS': r}, **sized(s, 'SHOCKS_SEEDS', 'SHOCKS_MONTHS', 'SHOCKS_HH')),
        parse=parse_shocks),
    'distribution': dict(
        args=lambda c, t, r, s: [str(s.get('seeds') or 8), str(s.get('months') or 240), '--cli', c,
                                 '--targets-out', t, '--json-out', r],
        env=lambda t, r, s: sized(s, None, None, 'DIST_HH'),
        parse=parse_distribution),
    'firms': dict(
        args=lambda c, t, r, s: [c],
        env=lambda t, r, s: dict({'FACTS_TARGETS_OUT': r}, **sized(s, 'FACTS_SEEDS', 'FACTS_MONTHS', 'FACTS_HH')),
        parse=parse_firms),
}


def sized(s, ks, km, kh):
    e = {}
    for k, name in (('seeds', ks), ('months', km), ('hh', kh)):
        if name and s.get(k):
            e[name] = str(s[k])
    return e


# ---------------------------------------------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------------------------------------------
def opt(args, name, default=None, flag=False):
    if name not in args:
        return default
    i = args.index(name)
    if flag:
        del args[i]
        return True
    v = args[i + 1]
    del args[i:i + 2]
    return v


def run_dimension(dim, script, cli, workdir, size, extra_env, logs, verbose, lock):
    ad = ADAPTERS.get(dim)
    tj = os.path.join(workdir, f'{dim}_targets.json')
    rj = os.path.join(workdir, f'{dim}_result.json')
    env = dict(os.environ, **extra_env)
    env['FACTS_RESULT_JSON'] = rj
    t0 = time.time()
    text, err = '', None
    try:
        if ad and ad.get('pre'):
            subprocess.run([sys.executable, script] + ad['pre'](tj), capture_output=True, text=True, env=env, check=True)
        argv = ad['args'](cli, tj, rj, size) if ad else [cli]
        if ad:
            env.update(ad['env'](tj, rj, size))
        p = subprocess.run([sys.executable, script] + argv, capture_output=True, text=True, env=env, cwd=ROOT)
        text = p.stdout + (('\n[stderr]\n' + p.stderr[-3000:]) if p.returncode != 0 else '')
        if p.returncode != 0:
            last = [l.strip() for l in p.stderr.splitlines() if l.strip()][-1:] or ['(no stderr)']
            raise RuntimeError(f'exit status {p.returncode}: {last[0][:300]}')
        rows = ad['parse'](tj, rj) if ad else parse_generic(tj, rj, p.stdout)
        if not rows:
            raise RuntimeError('no facts found in the output')
    except Exception as e:  # noqa: BLE001 - report and carry on with the other dimensions
        rows, err = [], str(e)
    wall = time.time() - t0
    if logs:
        with open(os.path.join(logs, f'{dim}.txt'), 'w') as f:
            f.write(text)
    if verbose:
        with lock:
            print(f'\n===== {dim} ({os.path.basename(script)}, {wall:.0f}s) =====\n{text}', flush=True)
    else:
        with lock:
            print(f'  {dim:<13s} {"FAILED" if err else "done":6s} {wall:6.0f}s', flush=True)
    return dict(dim=dim, script=os.path.relpath(script, ROOT), wall_s=round(wall, 1), error=err, facts=rows,
                counts={g: sum(r['gap'] == g for r in rows) for g in FLAGS})


def markdown(results, info):
    esc = lambda s: str(s).replace('|', '\\|').replace('\n', ' ')
    lines = [f"<!-- generated by scripts/stylized_facts.py: {info['runs']}; ECONSIM_CFG={info['cfg']!r} -->", '']
    for r in results:
        c = r['counts']
        lines += [f"### {r['dim']} (`{r['script']}`)", '',
                  f"{len(r['facts'])} facts: " + ', '.join(f'{g} {c[g]}' for g in FLAGS if c[g]), '',
                  '| Fact | US target | Model | Gap | Sample | Source |', '|---|---|---|---|---|---|']
        for f in r['facts']:
            g = f['gap'].upper() if f['gap'] == 'severe' else f['gap']
            lines.append(f"| {esc(f['name'])} | {esc(f['target'])} | {esc(f['model_text'])} | {g} | "
                         f"{esc(f['sample'])} | {esc(f['source'])} |")
        lines.append('')
    return '\n'.join(lines)


def main():
    args = sys.argv[1:]
    if args[:1] == ['--shim-exec']:
        return shim_main(args[1:])
    if '-h' in args or '--help' in args:
        print(__doc__)
        return 0
    only = opt(args, '--only')
    skip = opt(args, '--skip')
    jobs = opt(args, '--jobs')
    max_sims = int(opt(args, '--max-sims', '8'))
    cache = opt(args, '--cache')
    no_share = opt(args, '--no-share', flag=True)
    logs = opt(args, '--logs')
    verbose = opt(args, '--verbose', flag=True)
    detail = opt(args, '--detail', flag=True)
    jout = opt(args, '--json')
    mdout = opt(args, '--markdown')
    base = opt(args, '--baseline')
    size = {k: int(opt(args, '--' + k)) for k in ('seeds', 'months', 'hh') if '--' + k in args}
    bad = [a for a in args if a.startswith('-')]
    if bad or len(args) > 1:
        sys.exit(f'unexpected arguments {bad or args[1:]} (see --help)')
    cli = os.path.abspath(args[0] if args else os.environ.get('ECONSIM_CLI',
                                                              os.path.join(ROOT, 'target', 'release', 'econsim-cli')))
    if not os.access(cli, os.X_OK):
        sys.exit(f'econsim-cli not found or not executable: {cli}')

    scripts = {os.path.basename(p)[6:-3]: p for p in sorted(glob.glob(os.path.join(HERE, 'facts_*.py')))}
    dims = [d for d in ORDER if d in scripts] + sorted(d for d in scripts if d not in ORDER)
    if only:
        want = [d.strip() for d in only.split(',')]
        missing = [d for d in want if d not in scripts]
        if missing:
            sys.exit(f'no scripts/facts_<dim>.py for: {", ".join(missing)} (have {", ".join(dims)})')
        dims = [d for d in dims if d in want]
    if skip:
        dims = [d for d in dims if d not in [x.strip() for x in skip.split(',')]]

    tmp_root = tempfile.mkdtemp(prefix='stylized_facts_')
    cache_dir = os.path.abspath(cache) if cache else os.path.join(tmp_root, 'cache')
    os.makedirs(cache_dir, exist_ok=True)
    if logs:
        os.makedirs(logs, exist_ok=True)
    extra_env = {}
    run_cli = cli
    exits_real = os.environ.get('ECONSIM_EXITS', os.path.join(os.path.dirname(cli), 'exits'))
    log_start = 0
    if not no_share:
        run_cli = make_shim(cli, cache_dir, max_sims, 'econsim-cli')
        if os.path.exists(exits_real):
            extra_env['ECONSIM_EXITS'] = make_shim(os.path.abspath(exits_real), cache_dir, max_sims, 'exits')
        sl = os.path.join(cache_dir, 'shim.log')
        log_start = os.path.getsize(sl) if os.path.exists(sl) else 0
    elif os.path.exists(exits_real):
        extra_env['ECONSIM_EXITS'] = exits_real

    cfg = os.environ.get('ECONSIM_CFG', '')
    runs_txt = (f"{size.get('seeds', 8)} seeds x {size.get('months', 240)} months x {size.get('hh', 20000)} households "
                f"(+ each script's own long runs)")
    print(f'EconSim stylised facts: {len(dims)} dimensions; cli {cli}; ECONSIM_CFG={cfg!r}; {runs_txt}; '
          f'{"no shared cache" if no_share else f"shared run cache {cache_dir}, <= {max_sims} runs at once"}',
          flush=True)
    lock = threading.Lock()
    t0 = time.time()
    try:
        with ThreadPoolExecutor(int(jobs) if jobs else max(1, len(dims))) as ex:
            futs = [ex.submit(run_dimension, d, scripts[d], run_cli, tmp_root, size, extra_env, logs, verbose, lock)
                    for d in dims]
            results = [f.result() for f in futs]
        wall = time.time() - t0
        sims = sim_stats(cache_dir, log_start) if not no_share else None
    finally:
        shutil.rmtree(tmp_root, ignore_errors=True)  # removes the cache too unless --cache DIR was given

    report(results, wall, sims, detail)
    info = dict(cli=cli, cfg=cfg, runs=runs_txt, wall_s=round(wall, 1), sims=sims,
                date=time.strftime('%Y-%m-%d %H:%M'))
    if base:
        compare(results, load(base))
    if jout:
        with open(jout, 'w') as f:
            json.dump(dict(info=info, dimensions=results), f, indent=1, default=float)
        print(f'scorecard written to {jout}')
    if mdout:
        with open(mdout, 'w') as f:
            f.write(markdown(results, info) + '\n')
        print(f'markdown tables written to {mdout}')
    return 1 if any(r['error'] for r in results) else 0


def sim_stats(cache_dir, start):
    p = os.path.join(cache_dir, 'shim.log')
    if not os.path.exists(p):
        return dict(computed=0, reused=0, sim_s=0.0)
    with open(p) as f:
        f.seek(start)
        lines = [l.split() for l in f if l.strip()]
    miss = [l for l in lines if l[0] == 'miss']
    return dict(computed=len(miss), reused=sum(l[0] == 'hit' for l in lines),
                sim_s=round(sum(float(l[1]) for l in miss), 1))


def report(results, wall, sims, detail):
    print()
    hdr = f"{'dimension':<14s} {'facts':>5s} {'ok':>4s} {'minor':>6s} {'material':>9s} {'severe':>7s} {'n/a':>4s} " \
          f"{'mean sev':>8s} {'wall s':>7s}"
    print(hdr)
    print('-' * len(hdr))
    tot = {g: 0 for g in FLAGS}
    for r in results:
        c = r['counts']
        for g in FLAGS:
            tot[g] += c[g]
        if r['error']:
            print(f"{r['dim']:<14s} FAILED: {r['error'][:200]}")
            continue
        print(f"{r['dim']:<14s} {len(r['facts']):5d} {c['ok']:4d} {c['minor']:6d} {c['material']:9d} {c['severe']:7d} "
              f"{c['n/a']:4d} {mean_sev(c):>8s} {r['wall_s']:7.0f}")
    print('-' * len(hdr))
    n = sum(tot.values())
    print(f"{'TOTAL':<14s} {n:5d} {tot['ok']:4d} {tot['minor']:6d} {tot['material']:9d} {tot['severe']:7d} "
          f"{tot['n/a']:4d} {mean_sev(tot):>8s} {wall:7.0f}")
    meas = n - tot['n/a']
    if meas:
        print(f"ok or minor: {tot['ok'] + tot['minor']} of {meas} measurable facts "
              f"({100 * (tot['ok'] + tot['minor']) / meas:.0f}%); mean severity 0 = all ok, 3 = all severe")
    if sims:
        print(f"model runs: {sims['computed']} computed ({sims['sim_s']:.0f} s of single-thread simulation), "
              f"{sims['reused']} served from the shared cache; wall {wall:.0f} s")
    sel = [(r['dim'], f) for r in results for f in r['facts'] if detail or f['gap'] == 'severe']
    if sel:
        print('\nall facts:' if detail else '\nsevere gaps:')
        w = max(len(f['name'][:70]) for _, f in sel)
        for dim, f in sel:
            print(f"  {dim:<12s} {f['name'][:70]:<{w}s} | target {f['target'][:42]:<42s} | model "
                  f"{f['model_text'][:34]:<34s} | {f['gap']}")


def mean_sev(c):
    n = sum(c[g] for g in SEV)
    return f'{sum(SEV[g] * c[g] for g in SEV) / n:.2f}' if n else '-'


def compare(results, old):
    prev = {(d['dim'], f['id']): f for d in old['dimensions'] for f in d['facts']}
    better, worse = [], []
    for r in results:
        for f in r['facts']:
            p = prev.get((r['dim'], f['id']))
            if not p or p['gap'] == f['gap']:
                continue
            a, b = SEV.get(p['gap'], -1), SEV.get(f['gap'], -1)
            (better if 0 <= b < a else worse).append((r['dim'], f, p))
    print(f"\nchanges against {old['info'].get('cli', 'baseline')} ({old['info'].get('date', '')}): "
          f"{len(better)} better, {len(worse)} worse")
    for title, lst in (('better', better), ('worse', worse)):
        for dim, f, p in lst:
            print(f"  {title:6s} {dim:<12s} {f['name'][:64]:<64s} {p['gap']:>8s} -> {f['gap']:<8s} "
                  f"model {p.get('model_text', '')[:22]} -> {f['model_text'][:22]}")


if __name__ == '__main__':
    sys.exit(main())
