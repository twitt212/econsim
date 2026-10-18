#!/usr/bin/env python3
"""scripts/world_runs.py: run helper of the stage-5 ladder (spec rev 11, 3.0 "Stage-5 protocol"). Two subcommands; nothing here decides a row.

  long   --cell NAME --cfg FILE|none --seeds A-B [--households 20000] [--months 240] [--runs DIR] [--bin econsim-cli]
         `econsim-cli run HH MONTHS SEED --json | tail -n 1 | gzip` -> DIR/NAME_sSEED.json.gz, two processes at a time, RAYON_NUM_THREADS=1,
         `ECONSIM_CFG` = the contents of --cfg (none: the J4 default). Files that exist are skipped. This is the "default run" of every cell.
  events --cell NAME --cfg FILE --scenario CODE --seeds A-B --months M [--households 10000] [--runs DIR] [--server econsim-server]
         [--base-port 8800] [--warmup 0]
         Starts two `econsim-server` copies on base-port and base-port+1 (the ports must be free), one POST /api/compare per seed with one seed,
         stores DIR/NAME_CODE/CODE_sSEED.json.gz. The request config is the --cfg file with `n_households` and `warmup_months` set
         (warm-up 0 unless --warmup; with 0 the response must not carry `warmup_months`, with a positive value it must). Only the PIDs it
         started are stopped. CODE is a key of world_check.SCENARIOS.

Runs are two at a time (the stage's cap); never start a second invocation while one is running."""
import argparse, concurrent.futures as cf, gzip, json, os, subprocess, sys, threading, time, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
ROOT = os.path.abspath(os.path.join(HERE, '..'))


def seed_range(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def long_runs(a):
    cfg_text = None if a.cfg == 'none' else open(a.cfg).read()
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    env.pop('ECONSIM_CFG', None)
    if cfg_text is not None:
        env['ECONSIM_CFG'] = cfg_text
    os.makedirs(a.runs, exist_ok=True)

    def one(seed):
        path = os.path.join(a.runs, '%s_s%d.json.gz' % (a.cell, seed))
        if os.path.exists(path) and os.path.getsize(path) > 0:
            return seed, 0.0
        t0 = time.time()
        p = subprocess.run([a.bin, 'run', str(a.households), str(a.months), str(seed), '--json'], env=env, capture_output=True)
        if p.returncode != 0:
            raise RuntimeError('seed %d failed: %s' % (seed, p.stderr[-300:]))
        lines = [ln for ln in p.stdout.split(b'\n') if ln.strip()]
        with gzip.open(path + '.tmp', 'wb') as f:
            f.write(lines[-1] + b'\n')
        os.replace(path + '.tmp', path)
        return seed, time.time() - t0

    with cf.ThreadPoolExecutor(2) as ex:
        for seed, dt in ex.map(one, seed_range(a.seeds)):
            print(a.cell, seed, '%.1fs' % dt, flush=True)


def events(a):
    import world_check as wc
    shocks = wc.SCENARIOS[a.scenario]
    ports = [a.base_port, a.base_port + 1]
    cfg = json.load(open(a.cfg))
    cfg['n_households'] = a.households
    cfg['warmup_months'] = a.warmup
    out = os.path.join(a.runs, '%s_%s' % (a.cell, a.scenario))
    os.makedirs(out, exist_ok=True)
    env = dict(os.environ, RAYON_NUM_THREADS='1', ECONSIM_CACHE_MB='0', ECONSIM_WARMUP_MONTHS='0')
    procs = [subprocess.Popen([a.server], env=dict(env, PORT=str(p)), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) for p in ports]
    time.sleep(2.0)
    jobs = [s for s in seed_range(a.seeds) if not os.path.exists(os.path.join(out, '%s_s%d.json.gz' % (a.scenario, s)))]
    lock, errors = threading.Lock(), []

    def worker(port):
        while True:
            with lock:
                if not jobs:
                    return
                s = jobs.pop(0)
            body = {'config': cfg, 'shocks': shocks, 'months': a.months, 'seeds': [s]}
            try:
                req = urllib.request.Request('http://127.0.0.1:%d/api/compare' % port, data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
                res = json.loads(urllib.request.urlopen(req, timeout=3600).read())
                if ('warmup_months' in res) != (a.warmup > 0):
                    raise RuntimeError('warmup_months key does not match the request (%s)' % a.warmup)
                with gzip.open(os.path.join(out, '%s_s%d.json.gz' % (a.scenario, s)), 'wt') as f:
                    json.dump(res, f)
            except Exception as e:  # keep going; the exit status reports it
                errors.append((s, str(e)[:200]))

    try:
        ts = [threading.Thread(target=worker, args=(p,)) for p in ports]
        [t.start() for t in ts]
        [t.join() for t in ts]
    finally:
        for p in procs:
            p.terminate()
            p.wait(timeout=30)
    print(a.cell, a.scenario, a.seeds, 'errors', errors)
    sys.exit(1 if errors else 0)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='cmd', required=True)
    l = sub.add_parser('long')
    l.add_argument('--cell', required=True); l.add_argument('--cfg', required=True); l.add_argument('--seeds', required=True)
    l.add_argument('--households', type=int, default=20000); l.add_argument('--months', type=int, default=240)
    l.add_argument('--runs', default=os.path.join(ROOT, 'runs')); l.add_argument('--bin', default=os.path.join(ROOT, 'target', 'release', 'econsim-cli'))
    e = sub.add_parser('events')
    e.add_argument('--cell', required=True); e.add_argument('--cfg', required=True); e.add_argument('--scenario', required=True)
    e.add_argument('--seeds', required=True); e.add_argument('--months', type=int, required=True)
    e.add_argument('--households', type=int, default=10000); e.add_argument('--warmup', type=int, default=0)
    e.add_argument('--runs', default=os.path.join(ROOT, 'runs', 'ev')); e.add_argument('--base-port', type=int, default=8800, dest='base_port')
    e.add_argument('--server', default=os.path.join(ROOT, 'target', 'release', 'econsim-server'))
    a = ap.parse_args()
    {'long': long_runs, 'events': events}[a.cmd](a)


if __name__ == '__main__':
    main()
