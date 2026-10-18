#!/usr/bin/env python3
"""Stage 5 script tests (task description 5), re-pointed at the 5-T2 profile t2a: presets_parse_as_shocks, preset_bands_smoke, docs_examples_run.

usage: stage5_checks.py SERVER_BIN [--port 8800]   (a free port; one server process at a time)

presets_parse_as_shocks  every code of world_check.SCENARIOS is accepted by the server (HTTP 200 on /api/compare, 6 months, 300
                          households, the typical start with the 5-T2 profile t2a) and its records are finite;
preset_bands_smoke       the section-5 preset codes run 6 months at 300 households: every number of every record is finite and the
                          SFC residual (sfc_world_error) is below 1e-6 in every month;
docs_examples_run        every request body in the fenced blocks of docs/API.md that follow the marker line
                          "<!-- stage5-example: POST /api/compare -->" is posted and answers 200 (or 400 when the block says so).
Exit status 0 when every check passes."""
import json, math, os, re, subprocess, sys, time, urllib.error, urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..'))
sys.path.insert(0, HERE)
import world_check as wc  # noqa: E402

PRESETS = ['ecb100', 'boj100', 'pboc-100', 'china3', 'opec2', 'tarwar', 'tarCN25r', 'tarU25r', 'tarD25', 'tarDT25r', 'tarCNDT25r',
           'riskoff14', 'fo400', 'oil50', 'hormuz', 'pin10', 'dollar10', 'uscred', 'qe05', 'tarCN15r']


def post(port, body, timeout=600):
    req = urllib.request.Request('http://127.0.0.1:%d/api/compare' % port, data=json.dumps(body).encode(),
                                 headers={'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


def finite_tree(x):
    if isinstance(x, float):
        return math.isfinite(x)
    if isinstance(x, dict):
        return all(finite_tree(v) for v in x.values())
    if isinstance(x, list):
        return all(finite_tree(v) for v in x)
    return True


def base_cfg(profile='b8-world-t2a', hh=300):  # 5-T2: the presets run on the repaired profile t2a
    cfg = json.load(open(os.path.join(ROOT, 'scripts', 'profiles', profile + '.json')))
    cfg.update({'n_households': hh, 'warmup_months': 0})
    return cfg


def start(server, port):
    env = dict(os.environ, RAYON_NUM_THREADS='1', ECONSIM_CACHE_MB='0', ECONSIM_WARMUP_MONTHS='0', PORT=str(port))
    proc = subprocess.Popen([server], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2.0)
    return proc


def presets_parse(port):
    ok = True
    for code in sorted(wc.SCENARIOS):
        shocks = wc.SCENARIOS[code]
        status, data = post(port, {'config': base_cfg(), 'shocks': shocks, 'months': 6, 'seeds': [1]})
        good = status == 200
        if good:
            res = json.loads(data)
            good = len(res['shocked']) == 6 and finite_tree(res['shocked'])
        ok = ok and good
        print('presets_parse_as_shocks %-12s HTTP %d %s' % (code, status, 'PASS' if good else 'FAIL'))
    print('presets_parse_as_shocks: %s (%d codes)' % ('PASS' if ok else 'FAIL', len(wc.SCENARIOS)))
    return ok


def preset_bands_smoke(port):
    ok = True
    for code in PRESETS:
        status, data = post(port, {'config': base_cfg(), 'shocks': wc.SCENARIOS[code], 'months': 6, 'seeds': [2]})
        good = status == 200
        sfc = 0.0
        if good:
            res = json.loads(data)
            good = finite_tree(res['baseline']) and finite_tree(res['shocked'])
            for arm in ('baseline', 'shocked'):
                for rec in res[arm]:
                    w = rec.get('world', {})
                    if 'sfc_world_error' in w:
                        sfc = max(sfc, abs(w['sfc_world_error']))
            good = good and sfc < 1e-6
        ok = ok and good
        print('preset_bands_smoke %-12s HTTP %d sfc %.2e %s' % (code, status, sfc, 'PASS' if good else 'FAIL'))
    print('preset_bands_smoke: %s (%d presets, 6 months, 300 households)' % ('PASS' if ok else 'FAIL', len(PRESETS)))
    return ok


def docs_examples(port):
    text = open(os.path.join(ROOT, 'docs', 'API.md')).read()
    marker = '<!-- stage5-example: POST /api/compare -->'
    blocks = []
    for m in re.finditer(re.escape(marker) + r'\s*```json\n(.*?)```', text, re.S):
        head = m.group(1)
        blocks.append(head)
    ok = True
    if not blocks:
        print('docs_examples_run: FAIL (no marked examples in docs/API.md)')
        return False
    for i, b in enumerate(blocks):
        body = json.loads(b)
        want = body.pop('expect_status', 200)
        status, _ = post(port, body)
        good = status == want
        ok = ok and good
        print('docs_examples_run example %d: HTTP %d (expected %d) %s' % (i + 1, status, want, 'PASS' if good else 'FAIL'))
    print('docs_examples_run: %s (%d examples)' % ('PASS' if ok else 'FAIL', len(blocks)))
    return ok


def main():
    server = sys.argv[1]
    port = int(sys.argv[sys.argv.index('--port') + 1]) if '--port' in sys.argv else 8800
    proc = start(server, port)
    try:
        results = [presets_parse(port), preset_bands_smoke(port), docs_examples(port)]
    finally:
        proc.terminate()
        proc.wait(timeout=30)
    sys.exit(0 if all(results) else 1)


if __name__ == '__main__':
    main()
