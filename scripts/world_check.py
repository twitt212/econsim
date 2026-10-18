#!/usr/bin/env python3
"""Measurement harness of the world block (spec 3.0, "Measurement harness scripts/world_check.py").

Subcommands (every failure exits non-zero):

  run    --scenario NAME --seeds A-B [--households N] --months M --out DIR [--servers K] [--base-port P] [--profile JSON] [--overlay JSON]
         Starts K copies of econsim-server (PORT P.., RAYON_NUM_THREADS=1, ECONSIM_CACHE_MB=0; P.. must be free ports), sends one
         POST /api/compare per (scenario, seed) with a single seed, and stores DIR/<scenario>_s<seed>.json.gz with the response
         (baseline and shocked arms, common random numbers). The request config is the profile (default
         scripts/profiles/b8-world-s2.json) with the --overlay keys applied on top; --config replaces the profile.
         --households sets n_households when the config has none. Only the PIDs this process started are terminated.
  stats  --dir DIR --scenario NAME --seeds A-B --kind {w1_21,w1_22}
         The spec 3.0 statistic (stage 1): the paired shocked-minus-baseline paths are averaged over the seeds first; the foreign
         group-mean trough over the US trough of the mean paths, with a leave-one-seed-out jackknife SE.
  gates  --stage 1 --dir DIR
         Stage 1: one PASS/FAIL line per ABM gate measured here (W1-21, W1-22).
  gates  --stage 2 --dir DIR
         Stage 2: the W2 rows measurable from compare files (W2-01, W2-02, W2-03, W2-07, W2-08, W2-10, W2-11), each with its label
         (G, F, R or E; D30). W2-01 and W2-02 are judged on the realised real-dollar step (S2-R6): the band is scaled by the paired
         mean of c_t over the window divided by the full step (9.53). Every line is printed; the exit status is non-zero on any FAIL.
  centring --rundir DIR --cell CELL [--seeds 13-32]
         W1-31 (G): the mean over months 12-239 of each bloc's y_gap and z, per seed, then over the holdout seeds; band 0.5 % and 3 %.
         The runs are `econsim-cli run 20000 240 SEED --json` histories, gzipped as DIR/CELL_sSEED.json.gz.
  centring2 --rundir DIR --cell-on CELL --cell-off CELL [--seeds 1-32]
         W2-15 (G, band 0.3 pp) and W2-16 (R): paired X/GDP and M/GDP over months 12-239 against the matched link-off cell (block on,
         link off, the same ABM parameters; S2-R2), the centring of z~_j - c_t per bloc (mean and trend), and c_t.
  continuity --rundir DIR --cell-on CELL --cell-off CELL [--seeds 1-64]
         E row (S2-R2, F8): months 0-2 of the link on against the matched link-off cell, |dX| and |dM| as a share of nominal GDP
         (pp), per seed the mean over months 0-2 of the absolute monthly difference. Band: mean over seeds <= 0.2 pp and every
         seed <= 0.5 pp. Also the sd across seeds of the net-export step (dX - dM).
  identity --base FILE --new FILE [--expect-extra KEYS] [--expect-world-extra KEYS]
         Compares two `econsim-cli run ... --json` outputs (last line). The non-world keys must be bit-identical; the top-level
         new keys must be exactly --expect-extra; the new keys of the world object must be exactly --expect-world-extra
         (json_strip_compare strips any new key silently; N5 asks for the exact set).
  cost   --profile BIN --households N --months M [--rounds R] [--threads 1,8] [--profile-json JSON]
         Interleaved (S4-R20): each round runs `profile` with the block off and on, alternating the order by round; the minimum over
         R rounds (at least 12) per cell; prints the ms/month totals and the overhead of the minima per RAYON value.
  fo-events --dir DIR [--seeds-events A-B]
         Stage 4-2 event rows (W4-04, W4-11, W4-12, W4-14 (i)-(iii) on seeds 1001-1032, and the fo400 mechanism rows); exit 1 on FAIL.
  continuity-dollar --rundir DIR --cell-on CELL [--cell-off CELL] [--seeds 1-64]
         W4-29 (E): the dollar continuity of months 0-2; with --cell-off the D49 reading of clause 2 (stage 4-1b).
  pins   --on-prefix P --off-prefix Q [--seeds 2201-2216]
         Stage 4-1b W4-28 (G): DOL12, TR, UPK of pin7 and pin10, link on against the block-off overlay; exit 1 on FAIL.
  g4-floor --rundir-t DIR --cell-t CELL --rundir-b DIR --cell-b CELL [--seeds 3201-3264]
         Stage 4-1b D49: the effect floor (0.2 sd of the matched cell) of the per-seed G4 statistics u_ac1 and tot_lt25.
  fo-pool --events DIR --confirm DIR [--first 1001-1032] [--pooled 1001-1048]
         Stage 4-2 W4-14 pooled decision (it replaces an earlier ad-hoc script); the exit status is the pooled decision.
  fo-moments --rundir DIR --cell CELL --seeds A-B
         Stage 4-2 default-run moments (W4-13, tp_fo_bp path, W4-20), reported.
  w410   --dir DIR [--seeds 1001-1032]
         Stage 4-2 W4-10 (R): the pre-registered statistic (on record) and the S4-R21 restatement (window slope over h = 1..12 with
         seed-clustered SE; the h = 12 ratio of means when the denominator's t is at least 3). R rows: exit 0.

The shadow-mode identity checks (I1, I2, I4) and the 32-seed default runs are run by shell scripts that call econsim-cli
directly; joint_gates.py gives G1-G4 for the trade link on.
"""
import argparse, gzip, json, os, subprocess, sys, time, urllib.request, threading
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
PROFILE = os.path.join(HERE, 'profiles', 'b8-world-s2.json')
AE5 = [0, 2, 3, 4, 9]          # EA, JP, UK, CA, AU  (bloc order EA CN JP UK CA MX IN KR BR AU OPEC+ RoW)
EME5 = [1, 5, 6, 7, 8]         # CN, MX, IN, KR, BR
CN = 1                         # bloc index of China
BLOC_IDX = {'EA': 0, 'CN': 1, 'JP': 2, 'UK': 3, 'CA': 4, 'MX': 5, 'IN': 6, 'KR': 7, 'BR': 8, 'AU': 9, 'OPEC+': 10, 'RoW': 11}
TROUGH_MONTHS = 48             # the W1-21/22 troughs are taken over months 0-47
SCENARIOS = {
    # code: ScheduledShock list (at_month 0 = first reported month). Scenario codes of spec sections 3.0 and stage 2.
    'pol100': [{'at_month': 0, 'shock': {'type': 'PolicyShift', 'bps': 100.0, 'months': 12}}],
    'pol100l': [{'at_month': 0, 'shock': {'type': 'PolicyShift', 'bps': 100.0, 'months': 24}}],
    'dollar10': [{'at_month': 0, 'shock': {'type': 'Dollar', 'pct': 10.0, 'half_life_months': 0}}],
    'dollarm10': [{'at_month': 0, 'shock': {'type': 'Dollar', 'pct': -10.0, 'half_life_months': 0}}],
    'tar10': [{'at_month': 0, 'shock': {'type': 'ImportTariff', 'pct': 10.0}}],
    'tar25': [{'at_month': 0, 'shock': {'type': 'ImportTariff', 'pct': 25.0}}],
    'tar50': [{'at_month': 0, 'shock': {'type': 'ImportTariff', 'pct': 50.0}}],
    'tarCN25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 25.0, 'retaliate': False}}],
    'tarCN15r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 15.0, 'retaliate': True}}],
    # Stage 3 (oil): the measurement cells schedule the oil shocks at reported month 12 (review ZLB note; spec 3 stage 3).
    'oil50': [{'at_month': 12, 'shock': {'type': 'OilPrice', 'pct': 50.0, 'half_life_months': 12.0}}],
    # Month-0 shock (the irf command's convention; W3-11 O1-O3 are stated for it).
    'oil50m0': [{'at_month': 0, 'shock': {'type': 'OilPrice', 'pct': 50.0, 'half_life_months': 12.0}}],
    'opec2': [{'at_month': 12, 'shock': {'type': 'OpecCut', 'mbd': 2.0, 'months': 1}}],
    'hormuz': [{'at_month': 12, 'shock': {'type': 'OilSupply', 'pct_of_world_supply': 12.0, 'ramp_months': 1, 'hold_months': 3, 'recover_months': 6}}],
    # Stage 4-1 (spec 3.0 and section 4): the risk-off, credibility, China risk-off, ECB and pin cells, at reported month 0.
    'riskoff14': [{'at_month': 0, 'shock': {'type': 'RiskOff', 'vix_points': 14.0}}],
    'uscred': [{'at_month': 0, 'shock': {'type': 'UsCredibility', 'tp_bp': 22.0, 'months': 3, 'dollar_pct': 3.3}}],
    'china3': [{'at_month': 0, 'shock': {'type': 'BlocDemand', 'bloc': 'CN', 'pct': -3.0, 'half_life_months': 12.0, 'risk_off': True}}],
    'ecb100': [{'at_month': 0, 'shock': {'type': 'BlocPolicy', 'bloc': 'EA', 'bps': 100.0, 'months': 12}}],
    'pin7': [{'at_month': 0, 'shock': {'type': 'PolicyRate', 'rate': 0.07, 'months': 24}}],
    'pin10': [{'at_month': 0, 'shock': {'type': 'PolicyRate', 'rate': 0.10, 'months': 24}}],
    # Stage 4-2 (spec 3.0 rev 8, section 6.2 of the brief): official flows and the term premium, QE, at reported month 0.
    'fo100': [{'at_month': 0, 'shock': {'type': 'OfficialTreasury', 'bloc': 'CN', 'bn': 100.0, 'months': 1}}],
    'fo400': [{'at_month': 0, 'shock': {'type': 'OfficialTreasury', 'bloc': 'CN', 'bn': -400.0, 'months': 6}}],
    'qe05': [{'at_month': 0, 'shock': {'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 0.5, 'months': 12}}],
    'qe1': [{'at_month': 0, 'shock': {'type': 'QE', 'tenor_idx': 3, 'monthly_face_pct_gdp': 1.0, 'months': 12}}],
    # Stage 4-3 (spec 2.5, section 5 "Tariff war with retaliation"): bilateral duties with and without retaliation, at reported month 0.
    'tarCN15': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 15.0, 'retaliate': False}}],
    'tarCN29': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 29.0, 'retaliate': False}}],
    'tarwar': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': None, 'pct': 10.0, 'retaliate': True}},
               {'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 15.0, 'retaliate': True}}],
    'tarEA25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'EA', 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    'tarCA25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CA', 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    'tarMX25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'MX', 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    'tarIN25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'IN', 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    'tarCN25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    # Stage 5 (spec 5 presets, D56 vii, task description 6.2; entries copied from the pre-flight event script): uniform and sector duties (sector 2 =
    # Durables, 4 = Tech), the Japan and China bloc policy presets, read at month 0.
    'tarU10': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': None, 'pct': 10.0, 'retaliate': False}}],
    'tarU25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': None, 'pct': 25.0, 'retaliate': False}}],
    'tarU25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': None, 'pct': 25.0, 'retaliate': True}}],
    'tarU50': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': None, 'pct': 50.0, 'retaliate': False}}],
    'tarD25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 2, 'pct': 25.0, 'retaliate': False}}],
    'tarT25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 4, 'pct': 25.0, 'retaliate': False}}],
    'tarDT25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 2, 'pct': 25.0, 'retaliate': False}},
                {'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 4, 'pct': 25.0, 'retaliate': False}}],
    'tarDT25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 2, 'pct': 25.0, 'retaliate': True}},
                 {'at_month': 0, 'shock': {'type': 'Tariff', 'partner': None, 'sector': 4, 'pct': 25.0, 'retaliate': True}}],
    'tarCNDT25': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': 2, 'pct': 25.0, 'retaliate': False}},
                  {'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': 4, 'pct': 25.0, 'retaliate': False}}],
    'tarCNDT25r': [{'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': 2, 'pct': 25.0, 'retaliate': True}},
                   {'at_month': 0, 'shock': {'type': 'Tariff', 'partner': 'CN', 'sector': 4, 'pct': 25.0, 'retaliate': True}}],
    'boj100': [{'at_month': 0, 'shock': {'type': 'BlocPolicy', 'bloc': 'JP', 'bps': 100.0, 'months': 12}}],
    'pboc-100': [{'at_month': 0, 'shock': {'type': 'BlocPolicy', 'bloc': 'CN', 'bps': -100.0, 'months': 12}}],
}
# Stage-2 gate bands (spec section 4, W2-*), by month (value = paired percent change of the mean paths). W2-01 and W2-02 are
# stated for the full 10 % step; they are scaled to the realised step at run time (S2-R6, `realised_scale`).
FULL_STEP_DOLLAR = 9.531017980432493   # c_t of a 10 % dollar move (100 ln 1.10)
W2_BANDS = {
    'W2-01': {12: (1.1, 3.1), 24: (2.4, 5.4), 36: (2.4, 5.4)},
    'W2-02': {12: (-4.4, -1.4), 24: (-8.2, -3.2), 36: (-9.6, -4.6)},
    'W2-03': (-5.1, -2.8),
    'W2-07': (-40.0, -20.0),
}
W2_LINK_BAND_PP = 0.3                  # W2-15 band on dX/GDP and dM/GDP (pp), tightened from 0.7 at S2-R2 (F8)


def seed_range(text):
    a, b = text.split('-')
    return list(range(int(a), int(b) + 1))


def post_compare(port, body, timeout=3600):
    req = urllib.request.Request('http://127.0.0.1:%d/api/compare' % port, data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read())


def cmd_run(a):
    shocks = SCENARIOS[a.scenario]
    if a.config:
        config = json.loads(a.config)
    else:
        with open(a.profile) as f:
            config = json.load(f)
        if a.overlay:
            config.update(json.loads(a.overlay))
    config.setdefault('n_households', a.households)     # --households reaches the request (F1)
    config.setdefault('warmup_months', 0)                # J4: the server default is 24 since joint calibration 4; this check pins 0
    os.makedirs(a.out, exist_ok=True)
    seeds = seed_range(a.seeds)
    procs = []
    env = dict(os.environ, RAYON_NUM_THREADS='1', ECONSIM_CACHE_MB='0', ECONSIM_WARMUP_MONTHS='0')
    server = os.path.abspath(a.server)
    ports = [a.base_port + i for i in range(a.servers)]
    try:
        for p in ports:
            proc = subprocess.Popen([server], env=dict(env, PORT=str(p)), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            procs.append(proc)
        time.sleep(2.0)
        jobs = list(seeds)
        lock = threading.Lock()
        errors = []

        def worker(port):
            while True:
                with lock:
                    if not jobs:
                        return
                    s = jobs.pop(0)
                body = {'config': config, 'shocks': shocks, 'months': a.months, 'seeds': [s]}
                try:
                    res = post_compare(port, body)
                    if 'warmup_months' in res:
                        raise RuntimeError('compare response carries warmup_months (the harness pins 0)')
                    with gzip.open(os.path.join(a.out, '%s_s%d.json.gz' % (a.scenario, s)), 'wt') as f:
                        json.dump(res, f)
                except Exception as e:  # keep going; report at the end
                    errors.append((s, str(e)))

        threads = [threading.Thread(target=worker, args=(p,)) for p in ports]
        for t in threads:
            t.start()
        for t in threads:
            t.join()
        if errors:
            print('errors:', errors)
            sys.exit(1)
        print('stored %d compares in %s (n_households %s, link %s)' % (len(seeds), a.out, config['n_households'], config.get('world_link_trade', False)))
    finally:
        for proc in procs:          # only the servers started above
            proc.terminate()
            proc.wait(timeout=30)


def load_pair(dirn, scenario, seed):
    with gzip.open(os.path.join(dirn, '%s_s%d.json.gz' % (scenario, seed)), 'rt') as f:
        d = json.load(f)
    return d['baseline'], d['shocked']


def paired_paths(dirn, scenario, seeds, kind):
    """Per seed, months 0-47: the foreign group mean of the shocked-minus-baseline world.y_gap (AE5 for W1-21, EME5 for W1-22)
    and the US GDP change 100 (gdp_shock - gdp_base) / gdp_base in percent. Returns (foreign, us), each seeds x months."""
    grp = AE5 if kind == 'w1_21' else EME5
    fy, us = [], []
    for s in seeds:
        base, shock = load_pair(dirn, scenario, s)
        n = TROUGH_MONTHS
        gb = np.array([m['gdp_real_bn_ar'] for m in base[:n]])
        gs = np.array([m['gdp_real_bn_ar'] for m in shock[:n]])
        us.append(100.0 * (gs - gb) / gb)
        yb = np.array([m['world']['y_gap'] for m in base[:n]])
        ys = np.array([m['world']['y_gap'] for m in shock[:n]])
        fy.append((ys - yb)[:, grp].mean(axis=1))
    return np.array(fy), np.array(us)


def trough_ratio(fy, us):
    """The spec 3.0 statistic on the mean paths: foreign group-mean trough over US trough."""
    return fy.mean(axis=0).min() / us.mean(axis=0).min()


def statistic(fy, us):
    """Ratio on the mean paths and its leave-one-seed-out jackknife SE (the mean path recomputed without each seed)."""
    n = len(fy)
    est = trough_ratio(fy, us)
    jk = np.array([trough_ratio(np.delete(fy, i, axis=0), np.delete(us, i, axis=0)) for i in range(n)])
    se = np.sqrt((n - 1) / n * np.sum((jk - jk.mean()) ** 2))
    return est, se


def describe(fy, us):
    fm, um = fy.mean(axis=0), us.mean(axis=0)
    return 'foreign trough %.3f @m%d, US trough %.3f @m%d' % (fm.min(), int(fm.argmin()), um.min(), int(um.argmin()))


def cmd_stats(a):
    seeds = seed_range(a.seeds)
    fy, us = paired_paths(a.dir, a.scenario, seeds, a.kind)
    est, se = statistic(fy, us)
    print('%s %s seeds %d: ratio %.4f (leave-one-seed-out jackknife SE %.4f); %s' % (a.kind, a.scenario, len(seeds), est, se, describe(fy, us)))


BANDS = {'w1_21': (0.25, 1.00), 'w1_22': (0.30, 1.20)}


def cmd_gates(a):
    if a.stage == 1:
        return gates_stage1(a)
    if a.stage == 2:
        return gates_stage2(a)
    sys.exit('stage must be 1 or 2')


def gates_stage1(a):
    ok = True
    for kind, scen in (('w1_21', 'pol100'), ('w1_22', 'pol100')):
        seeds = seeds_in(a.dir, scen)
        fy, us = paired_paths(a.dir, scen, seeds, kind)
        est, se = statistic(fy, us)
        lo, hi = BANDS[kind]
        verdict = 'PASS' if lo <= est <= hi else 'FAIL'
        ok = ok and verdict == 'PASS'
        print('%s  G  value %.3f (SE %.3f, %d seeds)  band [%.2f, %.2f]  %s' % (kind.upper().replace('_', '-'), est, se, len(seeds), lo, hi, verdict))
    sys.exit(0 if ok else 1)


def seeds_in(dirn, scen):
    return sorted(int(f.split('_s')[1].split('.')[0]) for f in os.listdir(dirn) if f.startswith(scen + '_s') and f.endswith('.json.gz'))


def arm_path(dirn, scen, seeds, arm, fn):
    """seeds x months array of fn(record) for the baseline ('b') or the shocked ('s') arm."""
    out = []
    for s in seeds:
        b, sh = load_pair(dirn, scen, s)
        rows = b if arm == 'b' else sh
        out.append([fn(m) for m in rows])
    return np.array(out)


def ratio_pct(B, S, idx):
    """100 (mean of the shocked path / mean of the baseline path - 1) over the window `idx` (the mean path, spec 3.0)."""
    return 100.0 * (S.mean(axis=0)[idx].mean() / B.mean(axis=0)[idx].mean() - 1.0)


def ratio_pct_jk(B, S, idx):
    est = ratio_pct(B, S, idx)
    n = B.shape[0]
    jk = np.array([ratio_pct(np.delete(B, i, axis=0), np.delete(S, i, axis=0), idx) for i in range(n)])
    se = np.sqrt((n - 1) / n * np.sum((jk - jk.mean()) ** 2))
    return est, se


def realised_scale(B, S, idx):
    """The paired real-dollar step over the window: the mean over seeds and months of c_t (shocked minus baseline), over the full
    10 % step (S2-R6). W2-01 and W2-02 bands are multiplied by it."""
    return float((S - B)[:, idx].mean()) / FULL_STEP_DOLLAR


def scaled_band(band, k):
    lo, hi = sorted((band[0] * k, band[1] * k))
    return (lo, hi)


def window(h, half=2):
    """The months h-half .. h+half (the single-month rows of spec section 4 use +/-2 months; the mean path is averaged over them)."""
    return list(range(max(0, h - half), min(60, h + half + 1)))


def gates_stage2(a):
    dol = a.dollar
    seeds = seeds_in(a.dir, dol)
    lines = []

    def row(id_, label, value, se, band, note=''):
        verdict = ''
        if band is not None:
            lo, hi = band
            verdict = 'PASS' if lo <= value <= hi else 'FAIL'
        lines.append('%-7s %s  value %.3f (SE %.3f)  band %s  %s %s' % (
            id_, label, value, se, '[%g, %g]' % (band[0], band[1]) if band is not None else 'reported', verdict, note))

    if seeds:
        vol = lambda m: m['world']['import_volume_idx']
        exps = lambda m: m['exports_bn_ar']
        price = lambda m: m['import_price_index']
        ctB, ctS = arm_path(a.dir, dol, seeds, 'b', lambda m: m['world']['c_t']), arm_path(a.dir, dol, seeds, 's', lambda m: m['world']['c_t'])
        for h in (12, 24, 36):
            k = realised_scale(ctB, ctS, window(h))
            B, S = arm_path(a.dir, dol, seeds, 'b', vol), arm_path(a.dir, dol, seeds, 's', vol)
            v, se = ratio_pct_jk(B, S, window(h))
            row('W2-01', 'G', v, se, scaled_band(W2_BANDS['W2-01'][h], k),
                '(real imports, import volume index, month %d +/-2; realised real-dollar step %.2f%%, band x %.3f)' % (h, k * FULL_STEP_DOLLAR, k))
            B, S = arm_path(a.dir, dol, seeds, 'b', exps), arm_path(a.dir, dol, seeds, 's', exps)
            v, se = ratio_pct_jk(B, S, window(h))
            row('W2-02', 'F', v, se, scaled_band(W2_BANDS['W2-02'][h], k),
                '(exports, nominal, month %d +/-2; realised real-dollar step %.2f%%, band x %.3f)' % (h, k * FULL_STEP_DOLLAR, k))
        B, S = arm_path(a.dir, dol, seeds, 'b', price), arm_path(a.dir, dol, seeds, 's', price)
        v, se = ratio_pct_jk(B, S, [24])
        row('W2-03', 'F', v, se, W2_BANDS['W2-03'], '(import price index, month 24)')
        worst = 0.0
        for s in seeds:
            for arm in load_pair(a.dir, dol, s):
                for m in arm:
                    w = m['world']
                    if m['exports_bn_ar'] > 0:
                        worst = max(worst, abs(sum(w['exports_bn']) - m['exports_bn_ar']) / m['exports_bn_ar'])
                    if m['imports_bn_ar'] > 0:
                        worst = max(worst, abs(sum(w['imports_bn']) - m['imports_bn_ar']) / m['imports_bn_ar'])
        lines.append('W2-11   E  max relative error of the bilateral sums %.3e (band <= 1e-9)  %s' % (worst, 'PASS' if worst <= 1e-9 else 'FAIL'))
    seeds_t = seeds_in(a.dir, 'tarCN25')
    if seeds_t:
        imp_b = np.array([[m['world']['imports_bn'] for m in load_pair(a.dir, 'tarCN25', s)[0]] for s in seeds_t])
        imp_s = np.array([[m['world']['imports_bn'] for m in load_pair(a.dir, 'tarCN25', s)[1]] for s in seeds_t])
        B = imp_b[:, :, CN]
        S = imp_s[:, :, CN]
        v, se = ratio_pct_jk(B, S, window(12))
        row('W2-07', 'G', v, se, W2_BANDS['W2-07'], '(US imports from CN, month 12 +/-2)')
        dB = imp_s.mean(axis=0)[12] - imp_b.mean(axis=0)[12]
        diversion = np.delete(dB, CN).sum() / (-dB[CN]) if dB[CN] != 0 else float('nan')
        lines.append('W2-08   R  diversion %.3f at month 12 (reported; reference band [0.3, 1.0])' % diversion)
    seeds_p = seeds_in(a.dir, 'pol100')
    if seeds_p:
        idx = list(range(12, 25))
        Bg = arm_path(a.dir, 'pol100', seeds_p, 'b', lambda m: m['gdp_real_bn_ar'])
        Sg = arm_path(a.dir, 'pol100', seeds_p, 's', lambda m: m['gdp_real_bn_ar'])
        Bi = arm_path(a.dir, 'pol100', seeds_p, 'b', lambda m: m['imports_bn_ar'])
        Si = arm_path(a.dir, 'pol100', seeds_p, 's', lambda m: m['imports_bn_ar'])
        dg = 100.0 * (Sg.mean(axis=0)[idx] - Bg.mean(axis=0)[idx]) / Bg.mean(axis=0)[idx]
        di = 100.0 * (Si.mean(axis=0)[idx] - Bi.mean(axis=0)[idx]) / Bi.mean(axis=0)[idx]
        lines.append('W2-10   R  import elasticity to US GDP, months 12-24: %.3f (reported; trade V11 reference [-2.3, -1.5])' % (di.mean() / dg.mean()))
    for ln in lines:
        print(ln)
    if not lines:
        sys.exit('no stage-2 compare files in %s' % a.dir)
    sys.exit(1 if any('FAIL' in ln for ln in lines) else 0)


# ---------------------------------------------------------------- full histories (CLI runs): W1-31, W2-15, W2-16

def load_hist(rundir, cell, seed):
    with gzip.open(os.path.join(rundir, '%s_s%d.json.gz' % (cell, seed)), 'rt') as f:
        return json.load(f)


def cmd_centring(a):
    """W1-31: the mean over months 12-239 of each bloc's y_gap and z, per seed; the mean over the holdout seeds; the band."""
    seeds = seed_range(a.seeds)
    ys, zs = [], []
    for s in seeds:
        h = load_hist(a.rundir, a.cell, s)[12:240]
        ys.append(np.array([m['world']['y_gap'] for m in h]).mean(axis=0))
        zs.append(np.array([m['world']['z'] for m in h]).mean(axis=0))
    ys, zs = np.array(ys), np.array(zs)
    my, mz = ys.mean(axis=0), zs.mean(axis=0)
    sy, sz = ys.std(axis=0, ddof=1) / np.sqrt(len(seeds)), zs.std(axis=0, ddof=1) / np.sqrt(len(seeds))
    ok = np.abs(my).max() <= 0.5 and np.abs(mz).max() <= 3.0
    print('W1-31 G holdout seeds %s: max |mean y| %.3f (band 0.5), max |mean z| %.3f (band 3.0): %s' % (
        a.seeds, np.abs(my).max(), np.abs(mz).max(), 'PASS' if ok else 'FAIL'))
    print('per bloc mean y (SE): ' + ' '.join('%.3f(%.3f)' % (v, e) for v, e in zip(my, sy)))
    print('per bloc mean z (SE): ' + ' '.join('%.3f(%.3f)' % (v, e) for v, e in zip(mz, sz)))
    sys.exit(0 if ok else 1)


def cmd_centring2(a):
    """W2-15 (G) and W2-16 (R) on the 32-seed default runs: paired X/GDP and M/GDP (pp) over months 12-239 against the block-off
    cell; the cross-section z~_j - c_t per bloc (mean and trend, percent and percent per year); c_t (mean and trend)."""
    seeds = seed_range(a.seeds)
    dx, dm, cs_mean, cs_trend, ct_mean, ct_trend = [], [], [], [], [], []
    t = np.arange(12, 240) / 12.0
    for s in seeds:
        on = load_hist(a.rundir, a.cell_on, s)[12:240]
        off = load_hist(a.rundir, a.cell_off, s)[12:240]
        xon = np.mean([100.0 * m['exports_bn_ar'] / m['gdp_nominal_bn_ar'] for m in on])
        xoff = np.mean([100.0 * m['exports_bn_ar'] / m['gdp_nominal_bn_ar'] for m in off])
        mon = np.mean([100.0 * m['imports_bn_ar'] / m['gdp_nominal_bn_ar'] for m in on])
        moff = np.mean([100.0 * m['imports_bn_ar'] / m['gdp_nominal_bn_ar'] for m in off])
        dx.append(xon - xoff)
        dm.append(mon - moff)
        cs = np.array([[m['world']['z_tilde'][j] - m['world']['c_t'] for j in range(12)] for m in on])
        ct = np.array([m['world']['c_t'] for m in on])
        cs_mean.append(cs.mean(axis=0))
        cs_trend.append(np.array([np.polyfit(t, cs[:, j], 1)[0] for j in range(12)]))
        ct_mean.append(ct.mean())
        ct_trend.append(np.polyfit(t, ct, 1)[0])
    dx, dm = np.array(dx), np.array(dm)
    ex_est, ex_se = jackknife(lambda x: x.mean(), [dx])
    dm_est, dm_se = jackknife(lambda x: x.mean(), [dm])
    cs_mean, cs_trend = np.array(cs_mean), np.array(cs_trend)
    ok_x = abs(ex_est) <= W2_LINK_BAND_PP
    ok_m = abs(dm_est) <= W2_LINK_BAND_PP
    print('W2-15 G dX/GDP %+.3f pp (SE %.3f; band |.| <= %.1f)  %s' % (ex_est, ex_se, W2_LINK_BAND_PP, 'PASS' if ok_x else 'FAIL'))
    print('W2-15 G dM/GDP %+.3f pp (SE %.3f; band |.| <= %.1f)  %s' % (dm_est, dm_se, W2_LINK_BAND_PP, 'PASS' if ok_m else 'FAIL'))
    cm = cs_mean.mean(axis=0)
    ctr = cs_trend.mean(axis=0)
    ok_c = np.abs(cm).max() < 3.0
    ok_t = np.abs(ctr).max() <= 0.5
    print('W2-15 G mean (z~_j - c_t), max |.| %.3f (band 3.0)  %s' % (np.abs(cm).max(), 'PASS' if ok_c else 'FAIL'))
    print('       per bloc: ' + ' '.join('%.3f' % v for v in cm))
    print('W2-15 G trend (z~_j - c_t), max |.| %.3f %%/yr (band 0.5)  %s' % (np.abs(ctr).max(), 'PASS' if ok_t else 'FAIL'))
    print('       per bloc: ' + ' '.join('%.3f' % v for v in ctr))
    ct_m, ct_t = np.array(ct_mean), np.array(ct_trend)
    print('W2-16 R c_t mean %.3f %% (sd %.3f across seeds, SE %.3f); trend %.3f %%/yr (sd %.3f)' % (
        ct_m.mean(), ct_m.std(ddof=1), ct_m.std(ddof=1) / np.sqrt(len(seeds)), ct_t.mean(), ct_t.std(ddof=1)))
    sys.exit(0 if (ok_x and ok_m and ok_c and ok_t) else 1)


def jackknife(stat_fn, arrays):
    """Leave-one-seed-out jackknife SE of stat_fn(*arrays) with the seed on axis 0."""
    est = stat_fn(*arrays)
    n = arrays[0].shape[0]
    reps = np.array([stat_fn(*[np.delete(x, i, axis=0) for x in arrays]) for i in range(n)])
    se = np.sqrt((n - 1) / n * np.sum((reps - reps.mean(axis=0)) ** 2, axis=0))
    return est, se


def cmd_continuity(a):
    """E row (S2-R2, F8): months 0-2 of the link on against the matched link-off cell, |dX| and |dM| as a share of nominal GDP.
    Per seed the mean over months 0-2 of the absolute monthly difference; band: mean over seeds <= 0.2 pp and every seed <= 0.5 pp."""
    seeds = seed_range(a.seeds)
    adx, adm, net = [], [], []
    for s in seeds:
        on = load_hist(a.rundir, a.cell_on, s)[0:3]
        off = load_hist(a.rundir, a.cell_off, s)[0:3]
        dx = [100.0 * (x['exports_bn_ar'] - y['exports_bn_ar']) / x['gdp_nominal_bn_ar'] for x, y in zip(on, off)]
        dm = [100.0 * (x['imports_bn_ar'] - y['imports_bn_ar']) / x['gdp_nominal_bn_ar'] for x, y in zip(on, off)]
        adx.append(np.mean(np.abs(dx)))
        adm.append(np.mean(np.abs(dm)))
        net.append(np.mean(np.array(dx) - np.array(dm)))
    adx, adm, net = np.array(adx), np.array(adm), np.array(net)
    ok = adx.mean() <= 0.2 and adm.mean() <= 0.2 and adx.max() <= 0.5 and adm.max() <= 0.5
    print('E continuity, months 0-2, %d seeds: mean |dX| %.3f pp (max seed %.3f), mean |dM| %.3f pp (max seed %.3f); '
          'net-export step (dX - dM) sd %.3f pp; band mean <= 0.2, every seed <= 0.5  %s' % (
              len(seeds), adx.mean(), adx.max(), adm.mean(), adm.max(), net.std(ddof=1) if len(net) > 1 else 0.0,
              'PASS' if ok else 'FAIL'))
    sys.exit(0 if ok else 1)


def fx_broad_path(rundir, cell, seed):
    """The broad index 100 fx_t / fx_0 of a 240-month default run (world.fx_broad), as a float array."""
    return np.array([m['world']['fx_broad'] for m in load_hist(rundir, cell, seed)], float)


def cmd_continuity_dollar(a):
    """W4-29 (E, S4-R2, D37): months 0-2 of the link-on cell, the monthly change of ln fx with the base fx as fx_{-1} (100 in index
    units). Per seed the mean over months 0-2 of the absolute change (%). Both fx clauses are enforced and both are printed (stage 4-1
    review, D5, S4-R13): clause 1, the mean over seeds <= 2.0 %; clause 2, every seed <= 4.0 %. With --cell-off (the matched link-off
    cell, stage 4-1b D49 [assumed, S4-R26]) a seed above 4.0 is excused when its matched cell is itself above 3.0 % and the link exceeds it
    by at most 0.5: the premium innovation is common to both arms. Both readings are printed. Exit status 1 when either clause fails (clause 2
    on the reading in force: the excused one when --cell-off is given). The trade clause (|dX|, |dM| against E) is `continuity`."""
    seeds = seed_range(a.seeds)

    def per_seed(cell):
        out = []
        for s in seeds:
            x = fx_broad_path(a.rundir, cell, s)[0:3]
            lx = np.log(np.concatenate([[100.0], x]) / 100.0)
            out.append(float(np.mean(np.abs(np.diff(lx))) * 100.0))
        return np.array(out)

    per = per_seed(a.cell_on)
    order = np.argsort(-per)
    top = ', '.join('seed %d %.3f' % (seeds[i], per[i]) for i in order[:3])
    mean_ok = per.mean() <= 2.0
    seed_ok = per.max() <= 4.0
    print('W4-29 dollar continuity, months 0-2, %d seeds (mean of per-seed |d ln fx| in %%)' % len(seeds))
    print('  clause 1, mean over seeds: %.3f (band <= 2.0)  %s' % (per.mean(), 'PASS' if mean_ok else 'FAIL'))
    print('  clause 2, every seed: max %.3f (band <= 4.0; %d seed(s) above 4.0)  %s' % (
        per.max(), int((per > 4.0).sum()), 'PASS' if seed_ok else 'FAIL'))
    print('  largest per-seed values: %s' % top)
    if a.cell_off:
        off = per_seed(a.cell_off)
        above = per > 4.0
        excused = above & (off > 3.0) & (per - off <= 0.5)
        left = above & ~excused
        # S4-R26(a): at most 2 excused seeds in 64 and at most 3 in the pooled 128; past the cap the clause fails.
        cap = 2 if len(seeds) <= 64 else 3
        over_cap = int(excused.sum()) > cap
        seed_ok = not left.any() and not over_cap
        print('  matched cell %s: mean %.3f, max %.3f' % (a.cell_off, off.mean(), off.max()))
        print('  clause 2, D49 reading: %d seed(s) above 4.0, %d excused (matched cell above 3.0 and link - matched <= 0.5; cap %d), %d left%s  %s' % (
            int(above.sum()), int(excused.sum()), cap, int(left.sum()),
            ' (seeds %s)' % ', '.join(str(seeds[i]) for i in np.where(left)[0]) if left.any() else '', 'PASS' if seed_ok else 'FAIL'))
        if excused.any():
            print('  excused seeds (listed by number): %s%s' % (', '.join('%d (link %.3f, matched %.3f)' % (seeds[i], per[i], off[i])
                                                             for i in np.where(excused)[0]), ' - OVER THE CAP' if over_cap else ''))
    sys.exit(0 if (mean_ok and seed_ok) else 1)


def cmd_dollar_path(a):
    """W4-31 (R, S4-R2, S4-i): cross-seed mean of ln D (link on, cell-on) against ln(fx/fx_0) of the matched cell (cell-off), in
    points, over months 12-59 and 60-239, and the trend of the difference over months 12-239 (per 100 months). Alarm when the
    months 12-59 means differ by more than 15 points (S4-i: report to the orchestrator, not a stop by itself)."""
    seeds = seed_range(a.seeds)
    on = np.array([np.log(fx_broad_path(a.rundir, a.cell_on, s) / 100.0) * 100.0 for s in seeds])
    off = np.array([np.log(fx_broad_path(a.rundir, a.cell_off, s) / 100.0) * 100.0 for s in seeds])
    mon_on, mon_off = on.mean(axis=0), off.mean(axis=0)
    d = mon_on - mon_off
    m1 = slice(12, 60)
    m2 = slice(60, 240)
    diff1 = float(d[m1].mean())
    diff2 = float(d[m2].mean())
    t = np.arange(12, 240)
    slope = float(np.polyfit(t, d[12:240], 1)[0] * 100.0)
    print('W4-31 baseline dollar path, %d seeds (ln D link on %s vs ln fx matched %s, points):' % (len(seeds), a.cell_on, a.cell_off))
    print('  months 12-59: ln D %.2f vs %.2f (difference %+.2f); months 60-239: ln D %.2f vs %.2f (difference %+.2f); trend of difference %+.2f per 100 months' % (
        float(mon_on[m1].mean()), float(mon_off[m1].mean()), diff1, float(mon_on[m2].mean()), float(mon_off[m2].mean()), diff2, slope))
    alarm = abs(diff1) > 15.0
    print('  S4-i alarm (|difference| months 12-59 > 15 points): %s' % ('ALARM, report to the orchestrator' if alarm else 'no'))
    sys.exit(0)


def cmd_identity(a):
    """Exact new-key sets (N5): the non-world keys are bit-identical; the top-level extra keys and the world sub-keys are exactly
    the expected sets. --ignore-world KEYS skips those world keys in the value comparison (stage 4-2, I5: the new keys differ in value)."""
    ignore = set(filter(None, (a.ignore_world or '').split(',')))
    def last(path):
        with open(path) as fh:
            lines = [ln for ln in fh.read().splitlines() if ln.strip()]
        return json.loads(lines[-1])
    base, new = last(a.base), last(a.new)
    if len(base) != len(new):
        print('record count differs: base %d new %d' % (len(base), len(new)))
        sys.exit(1)
    want_extra = set(filter(None, (a.expect_extra or '').split(',')))
    want_wextra = set(filter(None, (a.expect_world_extra or '').split(',')))
    extra, wextra = set(), set()
    bad = 0
    for i, (b, n) in enumerate(zip(base, new)):
        extra |= set(n) - set(b)
        if 'world' in n and 'world' in b:
            wextra |= set(n['world']) - set(b['world'])
            if any(b['world'][k] != n['world'][k] for k in b['world'] if k not in ignore):
                bad += 1
        elif 'world' in n:
            wextra |= set(n['world'])
        bb = {k: v for k, v in b.items() if k != 'world'}
        nn = {k: v for k, v in n.items() if k != 'world' and k not in want_extra}
        if list(bb.keys()) != list(nn.keys()) or any(bb[k] != nn[k] for k in bb):
            bad += 1
    ok = bad == 0 and extra - want_extra == set() and want_extra <= extra and wextra == want_wextra
    print('identity: top-level new keys %s (expected %s); world new keys %s (expected %s); differing records %d' % (
        sorted(extra), sorted(want_extra), sorted(wextra), sorted(want_wextra), bad))
    sys.exit(0 if ok else 1)


# ---------------------------------------------------------------- stage 4-2: official block rows (W4-04, W4-10..W4-14, W4-20)

RESEARCH_LAMBDA = os.path.join(HERE, '..', 'data', 'world', 'research', 'finance', 'params.json')
AE_LAMBDA_KEYS = {'EA': 'euro_area', 'JP': 'japan', 'UK': 'uk', 'CA': 'canada', 'AU': 'australia'}   # AE5 blocs in AE5 order


def pair_cell(dirn, cell, scen, seed):
    """DIR/<cell>_<scen>/<scen>_s<seed>.json.gz: the compare response (baseline and shocked arms, paired inside compare)."""
    path = os.path.join(dirn, '%s_%s' % (cell, scen), '%s_s%d.json.gz' % (scen, seed))
    with gzip.open(path, 'rt') as f:
        d = json.load(f)
    return d['baseline'], d['shocked']


def paired(dirn, cell, scen, seeds, fn):
    """seeds x months array of fn(shocked record) - fn(baseline record) of one compare cell."""
    out = []
    for s in seeds:
        b, sh = pair_cell(dirn, cell, scen, s)
        out.append([fn(x) - fn(y) for x, y in zip(sh, b)])
    return np.array(out, dtype=float)


def se_of(per_seed):
    """Mean over seeds and its standard error across seeds (per-seed statistics on axis 0)."""
    n = len(per_seed)
    return float(np.mean(per_seed)), float(np.std(per_seed, ddof=1) / np.sqrt(n)) if n > 1 else 0.0


def band_label(value, band):
    if band is None:
        return 'reported', ''
    lo, hi = band
    return ('PASS' if lo <= value <= hi else 'FAIL'), '[%g, %g]' % (lo, hi)


def fo_events_line(name, label, value, se, band, note=''):
    verdict, bnd = band_label(value, band)
    dist = ''
    if band is not None and se > 0:
        lo, hi = band
        d = min(abs(value - lo), abs(value - hi)) if verdict == 'PASS' else -min(abs(value - lo), abs(value - hi))
        dist = ' dist-to-bound %+.2f SE' % (d / se)
    print('%-22s %s value %.3f (SE %.3f) band %s %s%s %s' % (name, label, value, se, bnd or 'reported', verdict, dist, note))
    return verdict


def cmd_fo_events(a):
    """Stage 4-2 event rows (brief 6.3): W4-04 (uscred), W4-11 (fo100), W4-12 (dollar10), W4-14 (qe05 with E4b), and the fo400 preset."""
    d = a.dir
    seeds04 = seed_range(a.seeds_events) if a.seeds_events else seed_range('1001-1016')
    seeds32 = seed_range(a.seeds_qe)   # stage 5: the QE row takes its own set (--seeds-qe, default the stage-4 set 1001-1032)
    bp = lambda x: 100.0 * x['yields'][3]
    ok = []
    if os.path.isdir(os.path.join(d, 'L4b_uscred')):
        v = paired(d, 'L4b', 'uscred', seeds04, bp)[:, 2]
        m, se = se_of(v)
        ok.append(fo_events_line('W4-04 10y half', 'F', m, se, (10.0, 30.0), '(uscred, yields[3] h = 3, bp; expected 22.00)'))
    if os.path.isdir(os.path.join(d, 'L4b_fo100')):
        tp = lambda x: 100.0 * x['term_premium'][3]
        pf = lambda x: x['world']['tp_fo_bp']
        yv = paired(d, 'L4b', 'fo100', seeds04, bp)
        m1, se1 = se_of(yv[:, 0:12].mean(axis=1))
        m2, se2 = se_of(yv[:, 48:60].mean(axis=1))
        ok.append(fo_events_line('W4-11 yield h1-12', 'F', m1, se1, (-21.0, -3.0), '(expected -7.95)'))
        ok.append(fo_events_line('W4-11 yield h49-60', 'F', m2, se2, (-9.0, -2.0), '(expected -4.30)'))
        tv = paired(d, 'L4b', 'fo100', seeds04, tp)
        mt1, st1 = se_of(tv[:, 0:12].mean(axis=1))
        mt2, st2 = se_of(tv[:, 48:60].mean(axis=1))
        print('  mechanism: term_premium[3] h1-12 %.3f (SE %.3f), h49-60 %.3f (SE %.3f) bp' % (mt1, st1, mt2, st2))
        fv = paired(d, 'L4b', 'fo100', seeds04, pf)
        mf1, sf1 = se_of(fv[:, 0:12].mean(axis=1))
        mf2, sf2 = se_of(fv[:, 48:60].mean(axis=1))
        print('  mechanism: world.tp_fo_bp h1-12 %.3f (SE %.3f), h49-60 %.3f (SE %.3f) bp' % (mf1, sf1, mf2, sf2))
    if os.path.isdir(os.path.join(d, 'L4b_dollar10')):
        fo = lambda x: x['world']['fo_bn']
        fl = lambda x: x['world']['fo_flow_bn']
        v = paired(d, 'L4b', 'dollar10', seeds04, fo)
        m, se = se_of(v[:, 5])
        ok.append(fo_events_line('W4-12 fo_bn h6', 'F', m, se, (-120.0, -50.0), '(expected -72.98)'))
        impact = paired(d, 'L4b', 'dollar10', seeds04, fl)[:, 0]
        mi, si = se_of(impact)
        print('  impact flow (record 0): %.3f (SE %.3f) bn; expected -24.81' % (mi, si))
    if os.path.isdir(os.path.join(d, 'L4b_qe05')) and os.path.isdir(os.path.join(d, 'E4b_qe05')):
        rl = paired(d, 'L4b', 'qe05', seeds32, bp)[:, 11] / 100.0           # pp
        re_ = paired(d, 'E4b', 'qe05', seeds32, bp)[:, 11] / 100.0
        ok.append(fo_events_line('W4-14 (i) R(L) pp', 'G', float(rl.mean()), se_of(rl)[1], (-0.48, -0.18), '(expected -0.37)'))
        dd = 100.0 * (rl - re_)                                              # bp
        md, sd_ = se_of(dd)
        ok.append(fo_events_line('W4-14 (ii) R(L)-R(E) bp', 'G', md, sd_, (-2.0, 2.0), '(expected +0.3)'))
        mech = paired(d, 'L4b', 'qe05', seeds32, lambda x: x['world']['tp_fo_bp'])[:, 11]
        mm, sm = se_of(mech)
        ok.append(fo_events_line('W4-14 (iii) d tp_fo', 'G', mm, sm, (-0.5, 0.5), '(expected -0.13)'))
    for cell in ('L4b', 'L4b0'):
        if os.path.isdir(os.path.join(d, cell + '_fo400')):
            v = paired(d, cell, 'fo400', seeds04, bp)
            r5, r6 = se_of(v[:, 5]), se_of(v[:, 6])
            print('fo400 %s (reported): yield bp at record 5 %.2f (SE %.2f), record 6 %.2f (SE %.2f), mean 0-11 %.2f, mean 48-59 %.2f' % (
                cell, r5[0], r5[1], r6[0], r6[1], se_of(v[:, 0:12].mean(axis=1))[0], se_of(v[:, 48:60].mean(axis=1))[0]))
    sys.exit(1 if any(v == 'FAIL' for v in ok) else 0)


def cmd_fo_moments(a):
    """Stage 4-2 default runs (brief 6.3): W4-13 (the sd of the executed flow over months 12-239, reported), the cross-seed mean and sd
    of world.tp_fo_bp at the pre-registered records, and the W4-20 reported moments at records 0 and 239."""
    seeds = seed_range(a.seeds)
    def hist(s):
        with gzip.open(os.path.join(a.rundir, '%s_s%d.json.gz' % (a.cell, s)), 'rt') as f:
            return json.load(f)
    sd_flow, tp_rec, share, mv, face = [], {r: [] for r in (1, 11, 35, 59, 107, 119, 179, 239)}, {0: [], 239: []}, {0: [], 239: []}, {0: [], 239: []}
    for s in seeds:
        h = hist(s)
        sd_flow.append(float(np.std([m['world']['fo_flow_bn'] for m in h[12:240]], ddof=1)))
        for r in tp_rec:
            tp_rec[r].append(h[r]['world']['tp_fo_bp'])
        for r in (0, 239):
            share[r].append(h[r]['world']['fo_share_row_pct'])
            mv[r].append(h[r]['world']['row_mv_gdp_pct'])
            face[r].append(h[r]['world']['row_face_share_pct'])
    print('W4-13 (reported) sd of the executed flow over months 12-239, mean over %d seeds: %.2f bn (seed sd %.2f)' % (
        len(seeds), float(np.mean(sd_flow)), float(np.std(sd_flow, ddof=1))))
    pre = {1: 0.8, 11: 6.5, 35: 14.1, 59: 19.5, 107: 27.2, 119: 23.4, 179: 20.5, 239: 18.8}
    for r in sorted(tp_rec):
        v = np.array(tp_rec[r])
        print('tp_fo_bp record %3d: mean %7.3f sd %6.3f (pre-registered %s)' % (r, v.mean(), v.std(ddof=1), pre[r]))
    for r in (0, 239):
        print('W4-20 record %3d: fo_share_row_pct %.2f, row_mv_gdp_pct %.2f, row_face_share_pct %.2f (means over seeds)' % (
            r, np.mean(share[r]), np.mean(mv[r]), np.mean(face[r])))
    sys.exit(0)


# ---------------------------------------------------------------- stage 4-3: retaliation and positions rows (W4-23, W4-24, W4-24b, W4-25)

W424_BAND_X = (-5.0, -2.0)        # W4-24 exports: ln(goods exports) response per ln(1 + ret_tau), month 12
W424_BAND_M = (-1.7, -0.8)        # W4-24 imports: ln(imports from the tariffed origin) response per ln(1 + tau), month 12
W424B_BAND_X = (-30.0, -20.0)     # W4-24b exports (goods), percent, month 12 (reported)
TOL_SFC = 1e-9                    # W4-23: the largest identity residual relative to monthly GDP


def cmd_ret_events(a):
    """Stage 4-3 event rows from the L4c cells of DIR (cmd names of the brief, section 6.3). Horizon: month 12 is record 11 (T0 = 0)."""
    d = a.dir
    seeds = seed_range(a.seeds_events) if a.seeds_events else seed_range('1101-1116')
    j = BLOC_IDX[a.bloc]
    ok = []
    ex = lambda x: x['world']['exports_goods_bn'][j]
    im = lambda x: x['world']['imports_bn'][j]
    if os.path.isdir(os.path.join(d, 'L4c_tarCN15r')):
        sx, sm, dg, rt, to = [], [], [], [], []
        for sd in seeds:
            b, sh = pair_cell(d, 'L4c', 'tarCN15r', sd)
            rt.append(sh[11]['world']['ret_tau'][1])
            to.append(sh[11]['world']['tau_by_origin'][1])
            dgx = np.log(ex(sh[11]) / ex(b[11]))
            sx.append(dgx / np.log1p(rt[-1]))
            sm.append(np.log(im(sh[11]) / im(b[11])) / np.log1p(to[-1]))
            dg.append(100.0 * (ex(sh[11]) / ex(b[11]) - 1.0))
        m, se = se_of(sx)
        ok.append(fo_events_line('W4-24 exports', 'F', m, se, W424_BAND_X, '(tarCN15r, CN goods exports per ln(1 + ret_tau); hand -2.392, ret_tau %.4f)' % np.mean(rt)))
        m, se = se_of(sm)
        ok.append(fo_events_line('W4-24 imports', 'F', m, se, W424_BAND_M, '(tarCN15r, CN imports per ln(1 + tau); expected -1.2)'))
        m, se = se_of(dg)
        fo_events_line('W4-24b exports', 'R', m, se, W424B_BAND_X, '(tarCN15r, CN goods exports %, hand -28.58; data -26)')
    if os.path.isdir(os.path.join(d, 'L4c_tarCN29')):
        v = [100.0 * (im(pair_cell(d, 'L4c', 'tarCN29', sd)[1][11]) / im(pair_cell(d, 'L4c', 'tarCN29', sd)[0][11]) - 1.0) for sd in seeds]
        m, se = se_of(v)
        fo_events_line('W4-24b imports', 'R', m, se, None, '(tarCN29, CN imports %, hand -26.0, probe -25.2; data -30)')
    if os.path.isdir(os.path.join(d, 'L4c_tarwar')):
        fx = lambda x: x['world']['fx_broad']
        for rec, nm in ((2, 'month 3'), (11, 'month 12')):
            v = paired(d, 'L4c', 'tarwar', seeds, fx)[:, rec]       # 100 fx_t / fx_0: percentage points of the ABM's own dollar
            m, se = se_of(v)
            verdict = 'PASS' if abs(m) <= 8.0 else 'FAIL'
            print('W4-25 (a) dollar %-8s R value %.3f (SE %.3f) |d| <= 8 %s (tarwar, the ABM\'s own dollar; (b) with UsCredibility is deferred)' % (nm, m, se, verdict))
            ok.append(verdict)
    if os.path.isdir(os.path.join(d, 'E4c_tarCN15r')):
        # W4-37 matched cell (S4-s): the switch-off cell E4c (no retaliation) with the flagged duty must show no goods-export response.
        dgm = []
        for sd in seeds:
            b, sh = pair_cell(d, 'E4c', 'tarCN15r', sd)
            dgm.append(100.0 * (ex(sh[11]) / ex(b[11]) - 1.0))
        m, se = se_of(dgm)
        verdict = 'PASS' if abs(m) < 0.5 else 'FAIL'
        print('W4-37 matched cell   E4c_tarCN15r (switch off) goods exports %% at month 12: %.3f (SE %.3f) |.| < 0.5 %% %s' % (m, se, verdict))
        ok.append(verdict)
    sys.exit(1 if any(v == 'FAIL' for v in ok) else 0)


def cmd_pos_moments(a):
    """Stage 4-3 default runs (L4c cell): W4-23 on every month of every seed (largest residual), and the reported W4-21 moments: the
    model's niip_pct_gdp at records 0, 11, 59, 119, 239 and the mean current account over months 12-239."""
    seeds = seed_range(a.seeds)
    worst, niip, ca = 0.0, {r: [] for r in (0, 11, 59, 119, 239)}, []
    for s in seeds:
        with gzip.open(os.path.join(a.rundir, '%s_s%d.json.gz' % (a.cell, s)), 'rt') as f:
            h = json.load(f)
        worst = max(worst, max(m['world']['sfc_world_error'] for m in h))
        for r in niip:
            niip[r].append(h[r]['world']['niip_pct_gdp'])
        ca.append(float(np.mean([m['world']['ca_pct_gdp'] for m in h[12:240]])))
    print('W4-23 E largest identity residual over %d seeds x 240 months: %.3e of monthly GDP (bound %.0e): %s' % (
        len(seeds), worst, TOL_SFC, 'PASS' if worst <= TOL_SFC else 'FAIL'))
    for r in sorted(niip):
        v = np.array(niip[r])
        print('niip_pct_gdp record %3d: mean %8.2f sd %6.2f (reported; W4-21 research-scale -59.8, data -68.9)' % (r, v.mean(), v.std(ddof=1)))
    print('ca_pct_gdp mean over months 12-239: %.2f (seed sd %.2f) (reported)' % (float(np.mean(ca)), float(np.std(ca, ddof=1))))
    sys.exit(0 if worst <= TOL_SFC else 1)


def cmd_pos_events(a):
    """Stage 4-3 event rows of the positions (reported): the paired response of niip_pct_gdp to dollar10 (W4-22, dynamic) and the identity
    residual of every record of every event cell found in DIR (W4-23)."""
    d = a.dir
    seeds = seed_range(a.seeds_events) if a.seeds_events else seed_range('1101-1116')
    worst = 0.0
    for name in sorted(os.listdir(d)):
        if not name.startswith('L4c_'):
            continue
        scen = name[len('L4c_'):]
        for s in seeds:
            if not os.path.exists(os.path.join(d, name, '%s_s%d.json.gz' % (scen, s))):
                continue
            b, sh = pair_cell(d, 'L4c', scen, s)
            worst = max(worst, max(m['world']['sfc_world_error'] for m in b + sh))
    print('W4-23 E largest identity residual over all L4c event cells: %.3e of monthly GDP (bound %.0e): %s' % (worst, TOL_SFC, 'PASS' if worst <= TOL_SFC else 'FAIL'))
    if os.path.isdir(os.path.join(d, 'L4c_dollar10')):
        v = paired(d, 'L4c', 'dollar10', seeds, lambda x: x['world']['niip_pct_gdp'])
        for rec in (5, 11, 23):
            m, se = se_of(v[:, rec])
            print('dollar10 d niip_pct_gdp record %2d: %.3f (SE %.3f) pp of GDP (reported; uniform ln 1.10 on the memo stocks: -8.66)' % (rec, m, se))
    sys.exit(0 if worst <= TOL_SFC else 1)


def cmd_fo_pool(a):
    """Stage 4-2 W4-14 pooled decision (rule 4; the statistics of an earlier ad-hoc script, which this replaces). The qe05 cells
    L4b (block on, link on) and E4b (the stage 4-1 block, link off) on seeds 1001-1032 (--first) and pooled with the confirmatory seeds
    1033-1048 (--pooled). Seeds up to the last seed of --first are read from --events, the later seeds from --confirm. Clauses: (i) the
    12-month R(L) in [-0.48, -0.18] pp; (ii) |R(L) - R(E)| <= 2 bp; (iii) |d tp_fo| at h = 12 <= 0.5 bp. The exit status is the pooled
    decision (the first set is printed for the record)."""
    first, pooled = seed_range(a.first), seed_range(a.pooled)
    cut = first[-1]
    def load(seeds):
        RL, RE, mech = [], [], []
        for s in seeds:
            d = a.events if s <= cut else a.confirm
            bL, shL = pair_cell(d, 'L4b', 'qe05', s)
            bE, shE = pair_cell(d, 'E4b', 'qe05', s)
            RL.append(100.0 * (shL[11]['yields'][3] - bL[11]['yields'][3]))
            RE.append(100.0 * (shE[11]['yields'][3] - bE[11]['yields'][3]))
            mech.append(shL[11]['world']['tp_fo_bp'] - bL[11]['world']['tp_fo_bp'])
        return np.array(RL), np.array(RE), np.array(mech)
    pooled_ok = True
    for label, seeds in ((a.first, first), (a.pooled, pooled)):
        RL, RE, mech = load(seeds)
        n = len(RL)
        m_rl, se_rl = RL.mean(), RL.std(ddof=1) / np.sqrt(n)
        d = RL - RE
        m_d, se_d = d.mean(), d.std(ddof=1) / np.sqrt(n)
        m_mech, se_mech = mech.mean(), mech.std(ddof=1) / np.sqrt(n)
        print('%s (%d seeds): R(L) %.1f bp (SE %.1f) = %.3f pp; R(L)-R(E) %+.2f bp (SE %.2f); d tp_fo at h=12 %+.3f bp (SE %.3f)' % (
            label, n, m_rl, se_rl, m_rl / 100, m_d, se_d, m_mech, se_mech))
        c1, c2, c3 = -0.48 <= m_rl / 100 <= -0.18, abs(m_d) <= 2.0, abs(m_mech) <= 0.5
        print('   clause (i) R(L) in [-0.48,-0.18] pp: %s; clause (ii) |R(L)-R(E)| <= 2 bp: %s; clause (iii) |d tp_fo| <= 0.5 bp: %s' % (
            'PASS' if c1 else 'FAIL', 'PASS' if c2 else 'FAIL', 'PASS' if c3 else 'FAIL'))
        if seeds is pooled:
            pooled_ok = c1 and c2 and c3
    sys.exit(0 if pooled_ok else 1)


W410_AE = ['EA', 'JP', 'UK', 'CA', 'AU']
W410_IDX = {'EA': 0, 'JP': 2, 'UK': 3, 'CA': 4, 'AU': 9}


def w410_cells(dirn, seeds):
    """The pol100 cells (L4b, block on, link on), records 0-11 (h = 1..12). num[j, seed, h-1] = 0.2706 d policy_j + lambda_j d term_premium[3]
    (pp) for AE bloc j; den[seed, h-1] = d yields[3] (pp), the US response. Both are shocked minus baseline."""
    with open(RESEARCH_LAMBDA) as f:
        lam = json.load(f)['us_policy_spillovers']['foreign_term_premium_passthrough_lambda']['by_bloc']
    num = np.zeros((len(W410_AE), len(seeds), 12))
    den = np.zeros((len(seeds), 12))
    for i, s in enumerate(seeds):
        b, sh = pair_cell(dirn, 'L4b', 'pol100', s)
        for k in range(12):
            den[i, k] = sh[k]['yields'][3] - b[k]['yields'][3]
            dtp = sh[k]['term_premium'][3] - b[k]['term_premium'][3]
            for j, bloc in enumerate(W410_AE):
                dpol = sh[k]['world']['policy'][W410_IDX[bloc]] - b[k]['world']['policy'][W410_IDX[bloc]]
                num[j, i, k] = 0.2706 * dpol + lam[AE_LAMBDA_KEYS[bloc]]['value'] * dtp
    return num, den


def cmd_w410(a):
    """W4-10 (R), stage 4-2, in the two forms of S4-R21. (1) The pre-registered statistic, as measured and on record only: per seed the mean
    over the five AE blocs of d y10_j / d y10_US at record 11 (h = 12), then the mean over seeds with SE. It is ill-conditioned (a few seeds
    with a near-zero US response carry the mean), so it decides nothing. (2) The restated row, band [0.2, 1.0] unchanged: the window form, the
    pooled through-origin slope of the bloc responses on the US response over h = 1..12 with seeds as clusters; and the h = 12 ratio of
    means with a leave-one-seed-out jackknife SE, printed only where the denominator's t is at least 3. R rows do not gate: exit 0."""
    seeds = seed_range(a.seeds)
    num, den = w410_cells(a.dir, seeds)
    n = len(seeds)
    per_seed = (num[:, :, 11] / den[:, 11]).mean(axis=0)
    m, se = se_of(per_seed)
    print('W4-10 (R, pre-registered, on record) beta %.3f (SE %.3f, %d seeds) band %s %s; seeds with |US d y10| < 0.01 pp at h = 12: %d' % (
        m, se, n, *band_label(m, (0.2, 1.0)), int((np.abs(den[:, 11]) < 0.01).sum())))
    X = np.broadcast_to(den[None, :, :], num.shape)
    slope = float((X * num).sum() / (X * X).sum())
    g = (X * (num - slope * X)).sum(axis=(0, 2))
    se_slope = float(np.sqrt((g ** 2).sum()) / (X * X).sum())
    print('W4-10 (R, restated S4-R21) window slope, pooled through-origin over h = 1..12: %.3f (seed-clustered SE %.3f) band %s %s' % (
        slope, se_slope, *band_label(slope, (0.2, 1.0))))
    d12, nb12 = den[:, 11], num[:, :, 11]
    rom = lambda ix: float(np.mean([nb12[j, ix].mean() / d12[ix].mean() for j in range(len(W410_AE))]))
    point = rom(np.arange(n))
    reps = np.array([rom(np.delete(np.arange(n), i)) for i in range(n)])
    jk = float(np.sqrt((n - 1) / n * ((reps - reps.mean()) ** 2).sum()))
    t = float(d12.mean() / (d12.std(ddof=1) / np.sqrt(n)))
    if t >= 3.0:
        print('W4-10 (R, restated S4-R21) ratio of means at h = 12: %.3f (jackknife SE %.3f), denominator t %.2f: reported' % (point, jk, t))
    else:
        print('W4-10 (R, restated S4-R21) ratio of means at h = 12 not reported: denominator t %.2f < 3 (S4-R21)' % t)
    sys.exit(0)


def cmd_pins(a):
    """W4-28 (G, stage 4-1 definitions; 4-1b seeds 2201-2216): pin7 and pin10 with the link on against the same overlay with the block off.
    Cells are directories <prefix>_pin7 and <prefix>_pin10 of compare files pinN_s<seed>.json.gz. DOL12: paired dollar_index % at month 12
    (record 11); TR: per-seed minimum over months 1-36 of the paired real GDP %; UPK: per-seed maximum over months 1-36 of the paired
    unemployment change. Bands: DOL12 pin7 [5, 22], pin10 [8, 27]; |TR(on) - TR(off)| <= 1.5 pp and |UPK(on) - UPK(off)| <= 1.0 pp.
    Prints the SE of each statistic and the distance of DOL12 to its nearest bound in SE (rule 4 of the 4-1b protocol: confirmatory
    pins when within 1 SE). Exit status 1 on any FAIL."""
    seeds = seed_range(a.seeds)
    bands = {'pin7': (5.0, 22.0), 'pin10': (8.0, 27.0)}
    ok_all = True

    def stats(prefix, scen):
        d12, tr, upk = [], [], []
        for s in seeds:
            base, shock = load_pair('%s_%s' % (prefix, scen), scen, s)
            d12.append(100.0 * (shock[11]['dollar_index'] / base[11]['dollar_index'] - 1.0))
            tr.append(min(100.0 * (shock[t]['gdp_real_bn_ar'] / base[t]['gdp_real_bn_ar'] - 1.0) for t in range(36)))
            upk.append(max(shock[t]['unemployment_rate'] - base[t]['unemployment_rate'] for t in range(36)))
        return np.array(d12), np.array(tr), np.array(upk)

    def se(x):
        return float(x.std(ddof=1) / np.sqrt(len(x)))

    for scen in ('pin7', 'pin10'):
        d_on, t_on, u_on = stats(a.on_prefix, scen)
        d_off, t_off, u_off = stats(a.off_prefix, scen)
        lo, hi = bands[scen]
        ok_d = lo <= d_on.mean() <= hi
        dist = min(d_on.mean() - lo, hi - d_on.mean()) / se(d_on)
        dtr, dup = abs(t_on.mean() - t_off.mean()), abs(u_on.mean() - u_off.mean())
        ok = ok_d and dtr <= 1.5 and dup <= 1.0
        ok_all &= ok
        print('%s: DOL12 on %.2f (SE %.2f) band [%g, %g] %s, nearest bound %.1f SE%s; block off %.2f (SE %.2f)' % (
            scen, d_on.mean(), se(d_on), lo, hi, 'PASS' if ok_d else 'FAIL', dist, ' (within 1 SE: confirmatory pins)' if abs(dist) < 1.0 else '', d_off.mean(), se(d_off)))
        print('   TR on %.2f off %.2f |diff| %.2f (<= 1.5 %s); UPK on %.2f off %.2f |diff| %.2f (<= 1.0 %s)' % (
            t_on.mean(), t_off.mean(), dtr, 'PASS' if dtr <= 1.5 else 'FAIL', u_on.mean(), u_off.mean(), dup, 'PASS' if dup <= 1.0 else 'FAIL'))
    print('W4-28 G verdict: %s' % ('PASS' if ok_all else 'FAIL'))
    sys.exit(0 if ok_all else 1)


def cmd_g4_floor(a):
    """Stage 4-1b D49 [assumed, S4-R26]: the effect floor of G4 for the per-seed statistics (u_ac1, tot_lt25). From the runs of the link-on
    cell and the matched cell on the same seeds: the paired shift d, its SE, Z = sign d / SE (as joint_gates.py), the cross-seed sd of the
    statistic in the matched cell, the floor 0.2 sd, and the verdict (a failure needs Z < -2.0 and |d| >= floor). The unfloored Z is
    always printed. Exit status 1 on a floored failure."""
    sys.path.insert(0, HERE)      # python3 -I does not put the script directory on sys.path
    import paired as P
    seeds = seed_range(a.seeds)
    rows = {'u_ac1': (+1, [], []), 'tot_lt25': (-1, [], [])}
    for s in seeds:
        for cell, idx in ((a.cell_t, 1), (a.cell_b, 2)):
            st = P.seed_stats(P.load(a.rundir_t if idx == 1 else a.rundir_b, cell, s))[0]
            for k in rows:
                rows[k][idx].append(float(st[k]))
    bad = False
    for k, (sign, t, b) in rows.items():
        t, b = np.array(t), np.array(b)
        d = t - b
        se_d = float(d.std(ddof=1) / np.sqrt(len(d)))
        z = float(np.clip(sign * d.mean() / se_d, -3, 3)) if se_d > 0 else 0.0
        sd_b = float(b.std(ddof=1))
        floor = 0.2 * sd_b
        fail = z < -2.0 and abs(d.mean()) >= floor
        bad |= fail
        print('%-9s matched %.4f (sd across seeds %.4f) link %.4f ; d %+.4f (SE %.4f) Z %+.2f ; floor 0.2 sd = %.4f ; %s' % (
            k, b.mean(), sd_b, t.mean(), d.mean(), se_d, z,
            floor, 'FAIL (Z < -2 and |d| >= floor)' if fail else ('below the floor (not scored)' if z < -2.0 else 'pass')))
    sys.exit(1 if bad else 0)


def cmd_cost(a):
    """Interleaved cost protocol (S4-R20, D3). In each round the block-off profile (`{}`) and the block-on profile (--profile-json) run
    once each, and the order alternates from round to round, so a load change between two runs lands in both cells instead of in the
    overhead. Per cell the minimum over the rounds; the overhead is the ratio of the two minima. At least 12 rounds."""
    if a.rounds < 12:
        sys.exit('cost: --rounds must be at least 12 (S4-R20)')
    cfgs = (('off', '{}'), ('on', open(a.profile_json).read()))
    for threads in a.threads.split(','):
        load0 = os.getloadavg()[0]
        vals = {'off': [], 'on': []}
        for r in range(a.rounds):
            for label, cfg in (cfgs if r % 2 == 0 else cfgs[::-1]):
                env = dict(os.environ, RAYON_NUM_THREADS=threads, ECONSIM_CFG=cfg)
                out = subprocess.run([a.profile, str(a.households), str(a.months)], env=env, capture_output=True, text=True, check=True).stdout
                vals[label].append(float(out.split('total ')[1].split(' ms/month')[0]))
        off, on = min(vals['off']), min(vals['on'])
        print('RAYON %s: %d interleaved rounds, load avg (1 min) at start %.1f; minimum total off %.3f ms/month, on %.3f ms/month, overhead %.2f%%' % (
            threads, a.rounds, load0, off, on, 100.0 * (on - off) / off))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest='cmd', required=True)
    r = sub.add_parser('run')
    r.add_argument('--scenario', required=True, choices=sorted(SCENARIOS))
    r.add_argument('--seeds', required=True)
    r.add_argument('--households', type=int, default=10000)
    r.add_argument('--months', type=int, default=60)
    r.add_argument('--out', required=True)
    r.add_argument('--servers', type=int, default=2)
    r.add_argument('--base-port', type=int, default=8800, dest='base_port',
                   help='first port of the block of --servers ports the harness starts servers on (default 8800; the ports must be free)')
    r.add_argument('--config', default=None)
    r.add_argument('--profile', default=PROFILE)
    r.add_argument('--overlay', default=None, help='JSON object of config keys applied on top of the profile')
    r.add_argument('--server', default=os.path.join(HERE, '..', 'target', 'release', 'econsim-server'))
    s = sub.add_parser('stats')
    s.add_argument('--dir', required=True)
    s.add_argument('--scenario', required=True)
    s.add_argument('--seeds', required=True)
    s.add_argument('--kind', required=True, choices=['w1_21', 'w1_22'])
    g = sub.add_parser('gates')
    g.add_argument('--stage', type=int, required=True)
    g.add_argument('--dir', required=True)
    g.add_argument('--dollar', default='dollar10', help='the dollar scenario used for the W2-01..03 and W2-11 rows')
    cn = sub.add_parser('centring')
    cn.add_argument('--rundir', required=True)
    cn.add_argument('--cell', required=True)
    cn.add_argument('--seeds', default='13-32')
    c2 = sub.add_parser('centring2')
    c2.add_argument('--rundir', required=True)
    c2.add_argument('--cell-on', required=True, dest='cell_on')
    c2.add_argument('--cell-off', required=True, dest='cell_off')
    c2.add_argument('--seeds', default='1-32')
    ct = sub.add_parser('continuity')
    ct.add_argument('--rundir', required=True)
    ct.add_argument('--cell-on', required=True, dest='cell_on')
    ct.add_argument('--cell-off', required=True, dest='cell_off')
    ct.add_argument('--seeds', default='1-64')
    cd = sub.add_parser('continuity-dollar')
    cd.add_argument('--rundir', required=True)
    cd.add_argument('--cell-on', required=True, dest='cell_on')
    cd.add_argument('--seeds', default='1-64')
    cd.add_argument('--cell-off', dest='cell_off', default=None, help='the matched link-off cell: applies the D49 reading of clause 2')
    dp = sub.add_parser('dollar-path')
    dp.add_argument('--rundir', required=True)
    dp.add_argument('--cell-on', required=True, dest='cell_on')
    dp.add_argument('--cell-off', required=True, dest='cell_off')
    dp.add_argument('--seeds', default='1-64')
    idn = sub.add_parser('identity')
    idn.add_argument('--base', required=True)
    idn.add_argument('--new', required=True)
    idn.add_argument('--expect-extra', default='', dest='expect_extra')
    idn.add_argument('--expect-world-extra', default='', dest='expect_world_extra')
    idn.add_argument('--ignore-world', default='', dest='ignore_world', help='comma list of world keys skipped in the value comparison')
    c = sub.add_parser('cost')
    c.add_argument('--profile', required=True)
    c.add_argument('--profile-json', default=PROFILE, dest='profile_json')
    c.add_argument('--households', type=int, default=5000)
    c.add_argument('--months', type=int, default=48)
    c.add_argument('--threads', default='1,2', help='RAYON_NUM_THREADS values (the CPU cap of the stage allows at most 2)')
    c.add_argument('--rounds', type=int, default=12, help='interleaved rounds (off, on alternating); the minimum over them; at least 12')
    fe = sub.add_parser('fo-events', help='stage 4-2 event rows (W4-04, W4-11, W4-12, W4-14, fo400)')
    fe.add_argument('--dir', required=True)
    fe.add_argument('--seeds-events', default='', dest='seeds_events', help='event seeds for the single-cell rows (default 1001-1016)')
    fe.add_argument('--seeds-qe', default='1001-1032', dest='seeds_qe', help='event seeds of the QE row (default 1001-1032)')
    fm = sub.add_parser('fo-moments', help='stage 4-2 default-run moments: W4-13, tp_fo_bp path, W4-20')
    fm.add_argument('--rundir', required=True)
    fm.add_argument('--cell', required=True)
    fm.add_argument('--seeds', required=True)
    wd = sub.add_parser('w410', help='stage 4-2 W4-10 from the pol100 cells: the pre-registered statistic (on record) and the S4-R21 restatement')
    wd.add_argument('--dir', required=True, help='a runs directory holding L4b_pol100/ (e.g. runs/events)')
    wd.add_argument('--seeds', default='1001-1032')
    re_ = sub.add_parser('ret-events', help='stage 4-3 event rows (W4-24, W4-24b, W4-25 a) from DIR/L4c_<scenario>')
    re_.add_argument('--dir', required=True)
    re_.add_argument('--bloc', default='CN')
    re_.add_argument('--seeds-events', default='', dest='seeds_events', help='event seeds (default 1101-1116)')
    pm = sub.add_parser('pos-moments', help='stage 4-3 default runs: W4-23 residual, reported W4-21 moments')
    pm.add_argument('--rundir', required=True)
    pm.add_argument('--cell', required=True)
    pm.add_argument('--seeds', required=True)
    pe = sub.add_parser('pos-events', help='stage 4-3 event cells: W4-23 residual, dynamic W4-22 (dollar10)')
    pe.add_argument('--dir', required=True)
    pe.add_argument('--seeds-events', default='', dest='seeds_events')
    fp = sub.add_parser('fo-pool', help='stage 4-2 W4-14 pooled decision (1001-1032 first set, 1001-1048 pooled; replaces an earlier ad-hoc script)')
    fp.add_argument('--events', required=True, help='the runs directory of seeds 1001-1032 (L4b_qe05, E4b_qe05)')
    fp.add_argument('--confirm', required=True, help='the runs directory of the confirmatory seeds 1033-1048')
    fp.add_argument('--first', default='1001-1032')
    fp.add_argument('--pooled', default='1001-1048')
    pn = sub.add_parser('pins', help='stage 4-1b W4-28: pin7 and pin10 DOL12, TR, UPK against the block-off overlay')
    pn.add_argument('--on-prefix', required=True, dest='on_prefix', help='directories <prefix>_pin7, <prefix>_pin10 (link on)')
    pn.add_argument('--off-prefix', required=True, dest='off_prefix', help='directories <prefix>_pin7, <prefix>_pin10 (block off)')
    pn.add_argument('--seeds', default='2201-2216')
    gf = sub.add_parser('g4-floor', help='stage 4-1b D49: the effect floor of the per-seed G4 statistics')
    gf.add_argument('--rundir-t', required=True, dest='rundir_t')
    gf.add_argument('--cell-t', required=True, dest='cell_t')
    gf.add_argument('--rundir-b', required=True, dest='rundir_b')
    gf.add_argument('--cell-b', required=True, dest='cell_b')
    gf.add_argument('--seeds', default='3201-3264')
    a = ap.parse_args()
    {'run': cmd_run, 'stats': cmd_stats, 'gates': cmd_gates, 'centring': cmd_centring, 'centring2': cmd_centring2,
     'identity': cmd_identity, 'cost': cmd_cost, 'continuity': cmd_continuity,
     'continuity-dollar': cmd_continuity_dollar, 'dollar-path': cmd_dollar_path, 'fo-events': cmd_fo_events,
     'fo-moments': cmd_fo_moments, 'w410': cmd_w410, 'fo-pool': cmd_fo_pool, 'pins': cmd_pins, 'g4-floor': cmd_g4_floor,
     'ret-events': cmd_ret_events, 'pos-moments': cmd_pos_moments, 'pos-events': cmd_pos_events}[a.cmd](a)


if __name__ == '__main__':
    main()
