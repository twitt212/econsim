#!/usr/bin/env python3
"""Business-cycle shape and macro dynamics: model vs US stylised facts.

Recession frequency, duration and depth (Harding-Pagan BBQ turning points on quarterly real GDP, the two-negative-
quarters rule, the Sahm rule), asymmetry (the Dupraz-Nakamura-Steinsson 2019 "plucking" property, speeds of
unemployment rises and falls, skewness of GDP growth and unemployment), persistence (GDP growth, unemployment, core
inflation), volatility and cyclicality of the expenditure components, employment, productivity and real wages
(HP-filtered quarterly, lambda 1600; Stock & Watson 1999, King & Rebelo 1999), Okun's law and the Phillips curve.

Usage:
  python3 scripts/facts_cycle.py [path/to/econsim-cli] [--seeds 8] [--months 240] [--hh 20000] [--workers 8]
                                 [--cache DIR] [--json OUT] [--targets-json OUT] [--fred DIR]
  The CLI path defaults to $ECONSIM_CLI, then target/release/econsim-cli. ECONSIM_CFG is passed through to the model
  (e.g. ECONSIM_CFG='{"risk_premium_sigma":0,"tfp_shock_sigma":0,"stochastic_oil":false,"tech_boom_enabled":false}'
  for a run without aggregate shocks). Seeds run in parallel with RAYON_NUM_THREADS=1 each; the default
  8 x 240 x 20,000 takes ~25 s of wall time on a loaded 10-core M-series machine.
  --cache DIR (or $FACTS_CACHE) keeps each run's JSON (keyed on CLI path/size/mtime, ECONSIM_CFG, size, seed) so the
  analysis can be re-run without re-simulating.
  --fred DIR adds empirical columns computed from FRED CSV downloads (https://fred.stlouisfed.org/graph/fredgraph.csv
  ?id=SERIES, one SERIES.csv each) with exactly the code applied to the model: GDPC1, PCECC96, DDURRA3Q086SBEA,
  A008RA3Q086SBEA, A011RA3Q086SBEA, CBIC1, UNRATE, PAYEMS, HOANBS, COMPRNFB, CPILFESL, PCEPILFE, NROU.
  Only numpy and the standard library are used.

Each model run starts after the CLI's 72-month burn-in; the first 12 reported months are dropped and the rest is
averaged to quarters (76 quarters per seed). Episode statistics (recessions, unemployment cycles, plucking
regressions) are pooled over seeds; other moments are seed means (seed sd shown). Gap flags: ok inside the target
range, otherwise the distance outside it in units of half the range (in logs for ratios/counts/durations, or a
stated scale): minor <= 0.5, material <= 2, SEVERE beyond.
Model-side definitions: residential investment is deflated by the Construction price; durables are
consumption_by_sector_real[DurableGoods]; employment is employment_millions (no hours margin); the real wage is
avg_wage_annual / cpi; the Phillips-curve gap is u minus a lambda=1e5 HP trend (data: CBO NROU).
"""
import argparse, hashlib, json, math, os, subprocess, sys, time
from concurrent.futures import ThreadPoolExecutor
import numpy as np

# ---------------------------------------------------------------------------------------------------------------
# Targets: (key, label, lo, hi, point, sample, source). Values marked "FRED, this script" were computed with
# `--fred` on the listed series using exactly the transformations applied to the model.
TARGETS = []  # filled below


def T(key, label, lo, hi, point, sample, source, log=False, scale=None):
    TARGETS.append(dict(key=key, label=label, lo=lo, hi=hi, point=point, sample=sample, source=source, log=log,
                        scale=scale))


# ---------------------------------------------------------------------------------------------------------------
# helpers
def q_avg(v):
    v = np.asarray(v, float)
    n = len(v) // 3 * 3
    return v[:n].reshape(-1, 3).mean(axis=1)


def hp(y, lam=1600.0):
    y = np.asarray(y, float)
    n = len(y)
    I = np.eye(n)
    D = np.diff(I, 2, axis=0)
    return y - np.linalg.solve(I + lam * D.T @ D, y)


def hp_trend(y, lam):
    return np.asarray(y, float) - hp(y, lam)


def skew(x):
    x = np.asarray(x, float)
    x = x[np.isfinite(x)]
    d = x - x.mean()
    m2 = (d ** 2).mean()
    return float((d ** 3).mean() / m2 ** 1.5) if m2 > 0 else float('nan')


def ac1(x):
    x = np.asarray(x, float)
    return float(np.corrcoef(x[1:], x[:-1])[0, 1])


def corr(a, b):
    return float(np.corrcoef(a, b)[0, 1])


def ols(y, X):
    """OLS of y on [1, X...]; returns coefficients (const first) and R^2."""
    y = np.asarray(y, float)
    X = np.column_stack([np.ones(len(y))] + [np.asarray(x, float) for x in X])
    b, *_ = np.linalg.lstsq(X, y, rcond=None)
    e = y - X @ b
    r2 = 1 - (e @ e) / ((y - y.mean()) @ (y - y.mean())) if len(y) > 2 else float('nan')
    return b, float(r2)


def ar_sum(x, p=4):
    x = np.asarray(x, float)
    Y = x[p:]
    X = [x[p - k:len(x) - k] for k in range(1, p + 1)]
    b, _ = ols(Y, X)
    return float(b[1:].sum())


# ---------------------------------------------------------------------------------------------------------------
# turning points
def bbq(logy, k=2, min_phase=2, min_cycle=5):
    """Harding & Pagan (2002) quarterly turning points on log level. Returns sorted [(t, 'P'|'T')]."""
    y = np.asarray(logy, float)
    n = len(y)
    tp = []
    for t in range(k, n - k):
        w = y[t - k:t + k + 1]
        if y[t] == w.max() and y[t] > y[t - 1] and y[t] >= y[t + 1]:
            tp.append([t, 'P'])
        elif y[t] == w.min() and y[t] < y[t - 1] and y[t] <= y[t + 1]:
            tp.append([t, 'T'])

    def alternate(tp):
        out = []
        for t, s in tp:
            if out and out[-1][1] == s:
                keep_new = (y[t] > y[out[-1][0]]) if s == 'P' else (y[t] < y[out[-1][0]])
                if keep_new:
                    out[-1] = [t, s]
            else:
                out.append([t, s])
        return out

    tp = alternate(tp)
    changed = True
    while changed:
        changed = False
        # minimum phase length: drop the weaker pair of a too-short phase
        for i in range(len(tp) - 1):
            if tp[i + 1][0] - tp[i][0] < min_phase:
                del tp[i:i + 2]
                tp = alternate(tp)
                changed = True
                break
        if changed:
            continue
        for i in range(len(tp) - 2):
            if tp[i + 2][0] - tp[i][0] < min_cycle:
                # drop the pair (i+1, i+2) or (i, i+1) with the smaller amplitude
                a1 = abs(y[tp[i + 1][0]] - y[tp[i][0]])
                a2 = abs(y[tp[i + 2][0]] - y[tp[i + 1][0]])
                j = i if a1 < a2 else i + 1
                del tp[j:j + 2]
                tp = alternate(tp)
                changed = True
                break
    return [(int(t), s) for t, s in tp]


def phases(tp, logy):
    """Complete recessions (P->T) and expansions (T->P): lists of (duration_quarters, amplitude_log_pct, start)."""
    rec, exp = [], []
    for (t0, s0), (t1, s1) in zip(tp, tp[1:]):
        d = t1 - t0
        a = 100 * (logy[t1] - logy[t0])
        (rec if s0 == 'P' else exp).append((d, a, t0))
    return rec, exp


def two_q_rule(g):
    """Maximal runs of >= 2 consecutive negative quarterly growth rates: list of (start, length)."""
    runs, i = [], 0
    while i < len(g):
        if g[i] < 0:
            j = i
            while j < len(g) and g[j] < 0:
                j += 1
            if j - i >= 2:
                runs.append((i, j - i))
            i = j
        else:
            i += 1
    return runs


def sahm_triggers(u_m, thr=0.5, refractory=12):
    """Onsets of the Sahm rule (Sahm 2019): months where the 3-month average unemployment rate first reaches `thr`
    above its low over the previous 12 months, at least `refractory` months after the previous onset."""
    u = np.asarray(u_m, float)
    ma = np.convolve(u, np.ones(3) / 3, mode='valid')  # ma[i] averages u[i..i+2]
    trig, last, above = [], -10 ** 9, False
    for i in range(12, len(ma)):
        on = ma[i] - ma[i - 12:i].min() >= thr - 1e-9
        if on and not above and i - last >= refractory:
            trig.append(i)
            last = i
        above = on
    return trig


def dns_cycles(u_m, X=1.5):
    """Dupraz-Nakamura-Steinsson (2019, App. A) unemployment peaks (u lows) and troughs (u highs), threshold X pp.
    Returns lists of contractions (u rises) and expansions (u falls) as (months, amplitude_pp, start_index)."""
    u = np.asarray(u_m, float)
    n = len(u)
    peaks, troughs = [], []
    t = 0
    cp = 0
    state = 'peak'
    while t < n:
        if state == 'peak':
            if u[t] < u[cp]:
                cp = t
            elif u[t] >= u[cp] + X - 1e-9:  # weak inequality: X = 1.5 keeps DNS's 1970-73 expansion
                peaks.append(cp)
                state, ct = 'trough', t
        else:
            if u[t] > u[ct]:
                ct = t
            elif u[t] <= u[ct] - X + 1e-9:
                troughs.append(ct)
                state, cp = 'peak', t
        t += 1
    ev = sorted([(p, 'P') for p in peaks] + [(q, 'T') for q in troughs])
    contr, expn = [], []
    for (t0, s0), (t1, _) in zip(ev, ev[1:]):
        amp = abs(u[t1] - u[t0])
        (contr if s0 == 'P' else expn).append((t1 - t0, amp, t0))
    return contr, expn


# ---------------------------------------------------------------------------------------------------------------
# moments from one sample (model run or data). Inputs: dict of quarterly arrays and monthly unemployment / core
# price index. Quarterly keys: y, c, cd, i, ri, dinv (level, same units as y), emp, hours (optional), rw, pcore
# (core price level), pce_yoy (core PCE yoy %, quarterly average), u (quarterly average u), ustar (optional).
def u_updown(d12):
    """Mean of rising 12-month changes in u over the mean size of falling ones (US 1948-2019: 1.85)."""
    d12 = np.asarray(d12, float)
    up, dn = d12[d12 > 0], -d12[d12 < 0]
    return float(up.mean() / dn.mean()) if len(up) and len(dn) else float('nan')


def sample_moments(Q, um):
    out = {}
    ly = np.log(Q['y'])
    g = 400 * np.diff(ly)
    y = hp(ly)
    out['g_mean'] = g.mean()
    out['g_sd'] = g.std()
    out['g_ac1'] = ac1(g)
    out['g_skew'] = skew(g)
    out['neg_q_share'] = 100 * (g < 0).mean()
    g4 = 100 * (ly[4:] - ly[:-4])
    out['g4_sd'] = g4.std()
    out['y_sd'] = 100 * y.std()
    out['y_ac1'] = ac1(y)

    def cyc(key, log=True):
        v = Q.get(key)
        if v is None:
            return None
        return hp(np.log(v)) if log else hp(v)

    for key in ['c', 'cd', 'i', 'ri', 'emp', 'hours', 'rw']:
        x = cyc(key)
        if x is None:
            continue
        out[f'rel_{key}'] = x.std() / y.std()
        out[f'corr_{key}'] = corr(x, y)
    # labour productivity: output per worker (model has no hours margin)
    lp = hp(ly - np.log(Q['emp']))
    out['rel_lp'] = lp.std() / y.std()
    out['corr_lp'] = corr(lp, y)
    out['corr_lp_emp'] = corr(lp, hp(np.log(Q['emp'])))
    # residential investment lead: lag k (quarters) maximising corr(RI_{t-k}, Y_t), k in -4..4
    if 'ri' in Q:
        ri = hp(np.log(Q['ri']))
        best = max(range(-4, 5), key=lambda k: corr(ri[max(0, -k):len(ri) - max(0, k)], y[max(0, k):len(y) - max(0, -k)]))
        out['ri_lead'] = best
    c = hp(np.log(Q['c']))
    out['c_lead'] = max(range(-4, 5), key=lambda k: corr(c[max(0, -k):len(c) - max(0, k)], y[max(0, k):len(y) - max(0, -k)]))
    # inventories: change in inventories as % of GDP, HP cycle
    di = hp(100 * Q['dinv'] / Q['y'])
    out['corr_dinv'] = corr(di, y)
    # unemployment
    uq = Q['u']
    out['u_ac1'] = ac1(uq)
    uc = hp(uq)
    out['ucyc_sd'] = uc.std()
    out['u_skew'] = skew(um)
    du = np.diff(uq)
    out['du_skew'] = skew(du)
    dum = np.diff(um)
    out['dum_sd'] = dum.std()
    out['dum_ac1'] = ac1(dum)
    # 12-month changes in u (monthly): asymmetry of rises and falls (per-seed here; paired.py pools them)
    d12 = um[12:] - um[:-12]
    out['du12_skew'] = skew(d12)
    out['u_updown'] = u_updown(d12)
    # per-worker real wage (income-side metric wage_per_eff, CPI-deflated): corr with output, as corr_rw
    if Q.get('rwe') is not None:
        out['corr_rw_eff'] = corr(hp(np.log(Q['rwe'])), y)
    if Q.get('emp_growth') is not None:
        out['emp_growth'] = Q['emp_growth']
    # Okun: gap version (HP) and difference version (4-quarter changes)
    out['okun_gap'] = np.polyfit(100 * y, uc, 1)[0]
    du4 = uq[4:] - uq[:-4]
    out['okun_diff'] = np.polyfit(g4, du4, 1)[0]
    # inflation persistence: core price, quarterly annualised log change
    if 'pcore' in Q:
        pi = 400 * np.diff(np.log(Q['pcore']))
        out['pi_ac1'] = ac1(pi)
        out['pi_arsum'] = ar_sum(pi, 4)
        out['pi_sd'] = pi.std()
    # Phillips curve: 4q core PCE inflation on its own 4-quarter lag and the unemployment gap (data: CBO NROU;
    # model: a slow HP trend, lambda 1e5, standing in for its real-time u* estimate)
    if 'pce_yoy' in Q:
        p4 = Q['pce_yoy']
        ustar = Q.get('ustar')
        gap = uq - (ustar if ustar is not None else hp_trend(uq, 1e5))
        b, _ = ols(p4[4:], [p4[:-4], gap[4:]])
        out['pc_slope'] = b[2]
        out['pc_corr'] = corr(hp(p4), uc)
    return out


def episode_stats(samples):
    """Pooled turning-point statistics. samples: list of (Q, um)."""
    rec, exp, two_q, sahm, years, qtrs_rec, depth_u, dns_c, dns_e, inv_share, nq = [], [], [], [], 0.0, 0, [], [], [], [], 0
    hk = []
    for Q, um in samples:
        ly = np.log(Q['y'])
        tp = bbq(ly)
        r, e = phases(tp, ly)
        rec += r
        exp += e
        nq += len(ly)
        qtrs_rec += sum(d for d, _, _ in r)
        g = np.diff(ly)
        two_q += two_q_rule(g)
        sahm += sahm_triggers(um)
        years += len(um) / 12
        # unemployment rise over each BBQ recession window (peak quarter to 4 quarters after the trough, max u)
        for d, _, t0 in r:
            seg = Q['u'][t0:min(len(ly), t0 + d + 5)]
            depth_u.append(seg.max() - Q['u'][t0])
            # inventory contribution to the peak-to-trough fall in GDP (Blinder & Maccini 1991)
            dy = Q['y'][t0 + d] - Q['y'][t0]
            if dy < 0:
                inv_share.append((Q['dinv'][t0 + d] - Q['dinv'][t0]) / dy)
        c, x = dns_cycles(um)
        dns_c.append(c)
        dns_e.append(x)
        # Hall & Kudlyak (2022): proportional rate of decline of u in recoveries, log points per year
        hk += [12 * (math.log(um[t0]) - math.log(um[t0 + d])) / d for d, _, t0 in x if um[t0 + d] > 0]
    st = {}
    st['hk_rate'] = float(np.mean(hk)) if hk else float('nan')
    st['n_rec'] = len(rec)
    st['rec_per_century'] = 100 * len(rec) / years
    st['two_q_per_century'] = 100 * len(two_q) / years
    st['sahm_per_century'] = 100 * len(sahm) / years
    st['rec_share'] = 100 * qtrs_rec / nq
    st['rec_dur_m'] = 3 * np.mean([d for d, _, _ in rec]) if rec else float('nan')
    st['exp_dur_m'] = 3 * np.mean([d for d, _, _ in exp]) if exp else float('nan')
    st['rec_depth'] = np.mean([a for _, a, _ in rec]) if rec else float('nan')
    st['rec_depth_med'] = np.median([a for _, a, _ in rec]) if rec else float('nan')
    st['rec_du'] = np.mean(depth_u) if depth_u else float('nan')
    st['inv_share'] = 100 * np.median(inv_share) if inv_share else float('nan')
    # DNS plucking regressions, pooled within samples (pairs never span samples)
    cx, ey, ex, cy = [], [], [], []
    speeds_c, speeds_e, dur_c, dur_e, amp_c = [], [], [], [], []
    for c, x in zip(dns_c, dns_e):
        for d, a, t0 in c:
            speeds_c.append((a, d))
            dur_c.append(d)
            amp_c.append(a)
            nxt = [e for e in x if e[2] == t0 + d]
            if nxt:
                cx.append(a)
                ey.append(nxt[0][1])
        for d, a, t0 in x:
            speeds_e.append((a, d))
            dur_e.append(d)
            nxt = [e for e in c if e[2] == t0 + d]
            if nxt:
                ex.append(a)
                cy.append(nxt[0][1])
    st['dns_n_contr'] = len(dur_c)
    st['dns_contr_per_century'] = 100 * len(dur_c) / years
    if len(cx) >= 3:
        b, r2 = ols(ey, [cx])
        st['pluck_slope'], st['pluck_r2'] = b[1], r2
    if len(ex) >= 3:
        b, r2 = ols(cy, [ex])
        st['antipluck_slope'], st['antipluck_r2'] = b[1], r2
    if speeds_c and speeds_e:
        # DNS Table 2: mean over cycles of amplitude / duration, pp per year
        sc = float(np.mean([12 * a / d for a, d in speeds_c]))
        se = float(np.mean([12 * a / d for a, d in speeds_e]))
        st['speed_contr'], st['speed_exp'], st['speed_ratio'] = sc, se, sc / se
        st['dns_dur_contr'], st['dns_dur_exp'] = np.mean(dur_c), np.mean(dur_e)
        st['dns_amp_contr'] = np.mean(amp_c)
    # growth cycles: BBQ on log GDP net of the sample's own linear trend (independent of trend growth)
    gc_n, gc_d = 0, []
    for Q, um in samples:
        ly = np.log(Q['y'])
        t = np.arange(len(ly))
        r, _ = phases(bbq(ly - np.polyval(np.polyfit(t, ly, 1), t)), ly)
        gc_n += len(r)
        gc_d += [d for d, _, _ in r]
    st['gc_per_century'] = 100 * gc_n / years
    return st


# ---------------------------------------------------------------------------------------------------------------
# model runs
def cache_key(cli, hh, months, seed):
    st = os.stat(cli)
    cfg = os.environ.get('ECONSIM_CFG', '')
    s = f"{os.path.realpath(cli)}|{st.st_size}|{int(st.st_mtime)}|{cfg}|{hh}|{months}|{seed}"
    return hashlib.sha1(s.encode()).hexdigest()[:16]


def run_model(cli, hh, months, seed, cache):
    path = None
    if cache:
        os.makedirs(cache, exist_ok=True)
        path = os.path.join(cache, f"run_{hh}_{months}_{seed}_{cache_key(cli, hh, months, seed)}.json")
        if os.path.exists(path):
            with open(path) as f:
                return json.load(f)
    env = dict(os.environ, RAYON_NUM_THREADS='1')
    out = subprocess.run([cli, 'run', str(hh), str(months), str(seed), '--json'], capture_output=True, text=True,
                         env=env)
    line = out.stdout.strip().splitlines()[-1] if out.stdout.strip() else ''
    if not line.startswith('['):
        raise RuntimeError(f"seed {seed}: no JSON from CLI\n{out.stderr[-2000:]}")
    if path:
        with open(path + '.tmp', 'w') as f:
            f.write(line)
        os.replace(path + '.tmp', path)
    return json.loads(line)


CONSTRUCTION, DURABLES = 8, 2


def model_sample(h, skip=12):
    h = h[skip:]
    g = lambda k: np.array([m[k] for m in h], float)
    p_con = np.array([m['sector_prices'][CONSTRUCTION] for m in h], float)
    Q = dict(
        y=q_avg(g('gdp_real_bn_ar')),
        c=q_avg(g('consumption_real_bn_ar')),
        cd=q_avg([m['consumption_by_sector_real'][DURABLES] for m in h]),
        i=q_avg(g('investment_real_bn_ar')),
        ri=q_avg(g('residential_investment_bn_ar') / (p_con / p_con[0])),
        dinv=q_avg(g('inventory_change_real_bn_ar')),
        emp=q_avg(g('employment_millions')),
        rw=q_avg(g('avg_wage_annual') / g('cpi')),
        pcore=q_avg(g('core_cpi')),
        pce_yoy=q_avg(g('core_pce_inflation_yoy')),
        u=q_avg(g('unemployment_rate')),
        rwe=q_avg(g('wage_per_eff') / g('cpi')) if 'wage_per_eff' in h[0] else None,
        emp_growth=100 * ((g('employment_millions')[-1] / g('employment_millions')[0]) ** (12 / (len(h) - 1)) - 1),
    )
    return Q, g('unemployment_rate')


# ---------------------------------------------------------------------------------------------------------------
# FRED data (for --fred): quarterly samples built exactly like the model's
def read_fred(d, sid):
    p = os.path.join(d, sid + '.csv')
    dates, vals = [], []
    with open(p) as f:
        next(f)
        for line in f:
            a, b = line.strip().split(',')[:2]
            if b in ('', '.'):
                continue
            dates.append(a)
            vals.append(float(b))
    return dates, np.array(vals)


def fred_sample(d, y0, y1):
    """Quarterly data y0Q1..y1Q4 and monthly unemployment y0-01..y1-12."""
    def qser(sid, monthly=False):
        dates, v = read_fred(d, sid)
        keys = [(int(s[:4]), (int(s[5:7]) - 1) // 3) for s in dates]
        acc = {}
        for k, x in zip(keys, v):
            acc.setdefault(k, []).append(x)
        want = [(yr, qq) for yr in range(y0, y1 + 1) for qq in range(4)]
        need = 3 if monthly else 1
        if any(len(acc.get(k, [])) < need for k in want):
            return None
        return np.array([np.mean(acc[k]) for k in want])

    def mser(sid):
        dates, v = read_fred(d, sid)
        m = {s[:7]: x for s, x in zip(dates, v)}
        return np.array([m[f"{yr}-{mo:02d}"] for yr in range(y0, y1 + 1) for mo in range(1, 13)])

    Q = dict(y=qser('GDPC1'), c=qser('PCECC96'), cd=qser('DDURRA3Q086SBEA'), i=qser('A008RA3Q086SBEA'),
             ri=qser('A011RA3Q086SBEA'), emp=qser('PAYEMS', True), hours=qser('HOANBS'), rw=qser('COMPRNFB'),
             pcore=qser('CPILFESL', True), u=qser('UNRATE', True), ustar=qser('NROU'))
    dates, cbi = read_fred(d, 'CBIC1')
    cb = {(int(s[:4]), (int(s[5:7]) - 1) // 3): x for s, x in zip(dates, cbi)}
    Q['dinv'] = np.array([cb[(yr, qq)] for yr in range(y0, y1 + 1) for qq in range(4)])
    # core PCE yoy (needs the year before y0)
    dates, p = read_fred(d, 'PCEPILFE')
    pm = {s[:7]: x for s, x in zip(dates, p)}
    yoy = []
    for yr in range(y0, y1 + 1):
        for mo in range(1, 13):
            a, b = pm.get(f"{yr}-{mo:02d}"), pm.get(f"{yr - 1}-{mo:02d}")
            yoy.append(100 * (a / b - 1) if a and b else np.nan)
    Q['pce_yoy'] = q_avg(yoy)
    Q = {k: v for k, v in Q.items() if v is not None and np.all(np.isfinite(v))}
    return Q, mser('UNRATE')


# ---------------------------------------------------------------------------------------------------------------
# Targets. "FRED" values were computed with --fred using exactly the code applied to the model: "postwar" =
# 1949Q1-2019Q4 (inflation 1960-2019), "GM" = Great Moderation 1984Q1-2019Q4, "19y" = mean over 19-year windows
# (the length of one model sample), which removes most of the small-sample bias in persistence, skewness and
# episode statistics. COVID (2020-) is excluded. Ranges span postwar and GM values and the literature; `log` flags
# compare ratios, `scale` overrides the tolerance unit.
NBER = "NBER Business Cycle Dating Committee chronology (nber.org/research/business-cycle-dating)"
DNS = "Dupraz, Nakamura & Steinsson (2019 NBER WP 26351; JME 2025) Tables 1, 2, A.1, US Jan 1948-Feb 2020"
FRED = "FRED, this script (--fred)"
SW = "Stock & Watson (1999, Handbook of Macroeconomics ch. 1)"
KR = "King & Rebelo (1999, Handbook of Macroeconomics ch. 14, Table 1)"
T('rec_per_century', 'recessions per century (BBQ, quarterly GDP)', 6, 15, 12.7,
  'postwar 12.7, GM 5.6 (NBER: 14.8 in 1945-2019, 8.3 in 1984-2019)',
  f"{NBER}; Harding & Pagan (2002 JME) BBQ rule; {FRED} GDPC1", log=True)
T('two_q_per_century', 'two-negative-quarter episodes per century', 5.5, 12, 11.3, 'postwar 11.3, GM 5.6',
  f"{FRED} GDPC1 (the rule misses mild NBER recessions such as 1960 and 2001)", log=True)
T('sahm_per_century', 'Sahm-rule onsets per century', 9, 18, 16.9, 'postwar 16.9, GM 11.1',
  f"Sahm (2019, Hamilton Project 'Recession Ready'): the 0.5pp rule caught every recession since 1970 without a "
  f"false positive; {FRED} UNRATE (the 1959 steel strike is the postwar false positive)", log=True)
T('rec_share', 'share of quarters in recession (BBQ), %', 4, 13, 9.9,
  'postwar 9.9, GM 4.2 (NBER months: 13% 1945-2019, 7% 1984-2019)', f"{NBER}; {FRED} GDPC1", log=True)
T('exp_dur_m', 'mean expansion length (BBQ), months', 60, 120, 73.5,
  'postwar 73.5 (NBER 1945-2020: 64.2; 1982-2020: 103)', f"{NBER}; {FRED} GDPC1", log=True)
T('rec_dur_m', 'mean recession length (BBQ), months', 8, 14, 10.3, 'postwar 9.3 (NBER 1945-2020: 10.3)',
  f"{NBER}; {FRED} GDPC1", log=True)
T('rec_depth', 'mean peak-to-trough real GDP fall, %', -3.5, -1.8, -2.4,
  'postwar -2.4 (median -2.6; 1980 -2.2, 1990-91 -1.4, 2007-09 -4.0)', f"{FRED} GDPC1 BBQ; {NBER}")
T('rec_du', 'u rise over a BBQ recession (+4q), pp', 2.2, 4.0, 2.9, 'postwar 2.9, GM 3.1',
  f"{FRED} UNRATE over GDPC1 BBQ windows")
T('inv_share', 'inventory share of recession GDP fall, %', 30, 90, 47,
  'postwar median 47, GM 33 (Blinder & Maccini: 87% of the fall in 1948-82 recessions)',
  f"Blinder & Maccini (1991 JEP 5:1); {FRED} CBIC1, GDPC1", log=True)
T('pluck_slope', 'plucking: next u fall on u rise (slope)', 0.7, 1.4, 1.12, '1948-2020: 1.12 (R2 0.59)',
  f"{DNS} (this script reproduces their dates exactly)")
T('antipluck_slope', 'plucking: next u rise on u fall (slope)', -0.7, 0.2, -0.38,
  '1948-2020: -0.38 (R2 0.22, insignificant)', DNS)
T('speed_ratio', 'speed of u rises / u falls (DNS cycles)', 1.7, 2.8, 2.17, '1948-2020: 1.89 vs 0.87 pp/yr; GM 2.2',
  f"{DNS}; Neftci (1984 JPE); McKay & Reis (2008 JME)", log=True)
T('dns_dur_exp', 'u-falling phase length (DNS), months', 45, 75, 59.1, '1948-2020: 59.1 (rising phases 26.9)', DNS,
  log=True)
T('hk_rate', 'u decline in recoveries, log points/yr', 0.07, 0.17, 0.1, 'H&K 1949-2019 ~0.1; postwar 0.16, GM 0.10',
  f"Hall & Kudlyak (2022 NBER Macro Annual 'The inexorable recoveries of unemployment'); {FRED} UNRATE on DNS phases",
  log=True)
T('u_skew', 'skewness of u (monthly level)', 0.3, 1.0, 0.64, '1948-2020: 0.64; GM 0.74',
  f"{DNS}; Sichel (1993 Econ. Inquiry) 'deepness'")
T('du_skew', 'skewness of quarterly changes in u', 0.8, 2.2, 1.27, 'postwar 1.27, GM 2.1 (19y windows 1.2-1.5)',
  f"Sichel (1993) 'steepness'; McKay & Reis (2008); {FRED} UNRATE")
T('g_skew', 'skewness of q/q GDP growth', -1.3, -0.05, -0.3, 'postwar -0.07 (19y windows -0.6), GM -1.3',
  f"Jensen, Petrella, Ravn & Santoro (2020 AEJ:Macro): negative and deepening after 1984; {FRED} GDPC1")
T('y_sd', 'sd of HP GDP cycle, %', 0.9, 1.3, 1.0, 'GM 0.99, 1990-2019 1.03 (postwar 1.53)',
  f"McConnell & Perez-Quiros (2000 AER); Stock & Watson (2002 NBER Macro Annual): volatility halved after 1984; "
  f"{SW}; {FRED} GDPC1", log=True)
T('g_sd', 'sd of q/q annualised GDP growth, %', 1.9, 2.7, 2.25, 'GM 2.25, 1990-2019 2.32 (postwar 3.7)',
  f"McConnell & Perez-Quiros (2000); Stock & Watson (2002); {FRED} GDPC1", log=True)
T('neg_q_share', 'share of quarters with falling GDP, %', 7, 14, 10, 'postwar 13.4, GM 8.4 (1990-2019 10.1)',
  f"{FRED} GDPC1", log=True)
T('g_ac1', 'AR(1) of q/q GDP growth', 0.25, 0.45, 0.35, 'postwar 0.35, GM 0.37',
  f"Cogley & Nason (1995 AER): positive autocorrelation at lags 1-2; {FRED} GDPC1")
T('y_ac1', 'AR(1) of HP GDP cycle', 0.82, 0.92, 0.86, 'postwar 0.84, GM 0.88', f"{SW}; {KR} 0.84; {FRED}")
T('u_ac1', 'AR(1) of quarterly u', 0.95, 0.99, 0.97, 'postwar 0.97, GM 0.98 (19y windows 0.96-0.98)',
  f"{FRED} UNRATE", scale=0.02)
T('dum_sd', 'sd of monthly change in u, pp', 0.13, 0.22, 0.17, 'postwar 0.21, GM 0.15',
  f"{FRED} UNRATE (BLS: CPS sampling error of a monthly change ~0.1pp)")
T('pi_ac1', 'AR(1) of q/q core CPI inflation', 0.6, 0.9, 0.75, '1960-2019 0.88, GM 0.83 (19y windows 0.65-0.73)',
  f"Fuhrer (2010 Handbook of Monetary Econ. ch. 9); Stock & Watson (2007 JMCB); {FRED} CPILFESL")
T('pi_arsum', 'sum of AR(4) coefficients, core CPI inflation', 0.65, 0.95, 0.9, '1960-2019 0.91, GM 0.90',
  f"Pivetta & Reis (2007 JEDC); {FRED} CPILFESL")
T('rel_c', 'sd(C)/sd(Y)', 0.72, 0.9, 0.8, 'postwar 0.80, GM 0.83', f"{SW}; {KR} 0.74; {FRED} PCECC96", log=True)
T('corr_c', 'corr(C, Y)', 0.78, 0.92, 0.86, 'postwar 0.79, GM 0.87', f"{SW}; {KR} 0.88; {FRED} PCECC96")
T('c_lead', 'lead of C over Y at peak corr, quarters', -0.5, 0.5, 0, 'postwar 0, GM 0 (coincident)', f"{SW}; {FRED}",
  scale=1.0)
T('rel_cd', 'sd(durables C)/sd(Y)', 2.6, 3.6, 3.0, 'postwar 3.1, GM 3.0', f"{SW}; {FRED} DDURRA3Q086SBEA", log=True)
T('rel_i', 'sd(nonresidential fixed I)/sd(Y)', 2.8, 4.3, 3.1, 'postwar 2.9, GM 4.2', f"{SW}; {FRED} A008RA3Q086SBEA",
  log=True)
T('rel_ri', 'sd(residential I)/sd(Y)', 5.0, 7.5, 6.3, 'postwar 6.2, GM 6.9',
  f"Davis & Heathcote (2005 IER): residential ~2x as volatile as nonresidential; {FRED} A011RA3Q086SBEA", log=True)
T('corr_ri', 'corr(residential I, Y)', 0.45, 0.75, 0.6, 'postwar 0.56, GM 0.57', f"Davis & Heathcote (2005); {FRED}")
T('ri_lead', 'lead of residential I over Y, quarters', 1, 3, 2, 'postwar 2, GM 2',
  f"Leamer (2007 Jackson Hole) 'Housing IS the business cycle'; Fisher (2007 JPE); {FRED}", scale=1.0)
T('corr_dinv', 'corr(inventory change/Y, Y)', 0.45, 0.7, 0.6, 'postwar 0.62, GM 0.50',
  f"Blinder & Maccini (1991); {FRED} CBIC1")
T('rel_emp', 'sd(employment)/sd(Y)', 0.8, 1.1, 0.9, 'postwar 0.89, GM 1.06 (hours HOANBS ~1.0)',
  f"{KR} hours 0.99; {FRED} PAYEMS", log=True)
T('corr_emp', 'corr(employment, Y)', 0.72, 0.9, 0.8, 'postwar 0.81, GM 0.80', f"{KR}; {FRED} PAYEMS")
T('corr_lp', 'corr(output per worker, Y)', 0.0, 0.5, 0.25, 'postwar 0.48, GM 0.23',
  f"Gali & van Rens (2021 EJ) 'vanishing procyclicality of labour productivity'; {FRED} GDPC1/PAYEMS")
T('corr_lp_emp', 'corr(output per worker, employment)', -0.5, -0.05, -0.25, 'postwar -0.13, GM -0.40',
  f"Gali & van Rens (2021); Stiroh (2009 JEDC); {FRED}")
T('rel_rw', 'sd(real wage)/sd(Y)', 0.35, 1.1, 0.6, 'postwar 0.62, GM 1.07 (real comp./hour)',
  f"{KR} 0.38 (avg hourly earnings); {FRED} COMPRNFB", log=True)
T('u_updown', 'mean rise / mean fall of 12-month change in u (pooled)', 1.2, 2.7, 1.85,
  'US 1948-2019 1.85; 19-year windows 1.16-2.74', f"{DNS}; {FRED} UNRATE. Pooled over seeds in paired.py")
T('du12_skew', 'skewness of 12-month change in u', 0.4, 1.5, 0.97, 'US 1948-2019 0.97', f"{FRED} UNRATE")
T('corr_rw_eff', 'corr(CPI-deflated wage per efficiency unit, Y), HP', 0.0, 0.5, 0.25,
  'Solon, Barsky & Parker (1994 QJE): composition-adjusted real wage, mildly procyclical', f"{SW}; {FRED}")
T('emp_growth', 'employment growth, %/yr (months 12-239)', 0.3, 0.6, 0.45,
  'BLS Employment Projections 2024-34; CBO Demographic Outlook', "BLS Employment Projections 2024-34; CBO")
T('corr_rw', 'corr(real wage, Y)', -0.1, 0.4, 0.15, 'postwar 0.18, GM -0.02',
  f"{KR} 0.12; Solon, Barsky & Parker (1994 QJE): procyclical net of composition; {FRED} COMPRNFB")
T('okun_gap', 'Okun: u cycle per 1% GDP cycle (HP)', -0.55, -0.4, -0.45, 'postwar -0.44, GM -0.54',
  f"Ball, Leigh & Loungani (2017 JMCB) US levels -0.4 to -0.5; {FRED}")
T('okun_diff', 'Okun: 4q change in u per 1pp 4q growth', -0.45, -0.3, -0.38, 'postwar -0.37, GM -0.41',
  f"Ball, Leigh & Loungani (2017); Knotek (2007 KC Fed Econ. Review); {FRED}")
T('pc_slope', 'Phillips: core PCE on u gap given lagged π', -0.3, -0.03, -0.1, '1960-2019 -0.19, GM -0.07',
  f"Blanchard (2016 AER P&P); Del Negro, Lenza, Primiceri & Tambalotti (2020 BPEA); Hazell, Herreno, Nakamura & "
  f"Steinsson (2022 QJE): flat since the 1990s; {FRED} PCEPILFE, NROU")
T('pc_corr', 'corr(core PCE infl. cycle, u cycle)', -0.6, -0.25, -0.45, 'GM -0.46 (1960-2019 -0.04: 1970s)',
  f"{SW} (inflation procyclical with a lag); {FRED} PCEPILFE, UNRATE")


# ---------------------------------------------------------------------------------------------------------------
def flag(t, v):
    """ok inside the range; otherwise distance outside in units of `scale` (default: half the range or 10% of the
    point, in logs for `log` targets): minor <= 0.5, material <= 2, severe beyond."""
    if v is None or not np.isfinite(v):
        return 'n/a'
    lo, hi = t['lo'], t['hi']
    if lo <= v <= hi:
        return 'ok'
    if t.get('log') and v > 0 and lo > 0:
        d = abs(math.log(v / (lo if v < lo else hi))) / max(math.log(hi / lo) / 2, 0.1)
    else:
        sc = t.get('scale') or max((hi - lo) / 2, 0.1 * abs(t['point']), 1e-9)
        d = (lo - v if v < lo else v - hi) / sc
    return 'minor' if d <= 0.5 else 'material' if d <= 2 else 'SEVERE'


def summarise(samples):
    per = [sample_moments(Q, um) for Q, um in samples]
    keys = sorted(set().union(*[p.keys() for p in per]))
    mean = {k: float(np.nanmean([p[k] for p in per if k in p])) for k in keys}
    sd = {k: float(np.nanstd([p[k] for p in per if k in p])) for k in keys}
    ep = episode_stats(samples)
    mean.update({k: float(v) for k, v in ep.items()})
    return mean, sd


def fred_columns(d):
    """Empirical values from the same code: postwar (1949-2019, inflation 1960-2019), GM (1984-2019), 19-year
    windows (1949-2019 every 3 years; inflation from 1960)."""
    post, _ = summarise([fred_sample(d, 1949, 2019)])
    p60, _ = summarise([fred_sample(d, 1960, 2019)])
    for k in ('pi_ac1', 'pi_arsum', 'pi_sd', 'pc_slope', 'pc_corr'):
        if k in p60:
            post[k] = p60[k]
    gm, _ = summarise([fred_sample(d, 1984, 2019)])
    win, _ = summarise([fred_sample(d, y, y + 18) for y in range(1949, 2002, 3)])
    w60, _ = summarise([fred_sample(d, y, y + 18) for y in range(1960, 2002, 3)])
    for k in ('pi_ac1', 'pi_arsum', 'pi_sd', 'pc_slope', 'pc_corr'):
        if k in w60:
            win[k] = w60[k]
    return {'US postwar': post, 'US GM': gm, 'US 19y': win}


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..')
    ap.add_argument('cli', nargs='?', default=os.environ.get('ECONSIM_CLI',
                                                             os.path.join(root, 'target', 'release', 'econsim-cli')))
    ap.add_argument('--seeds', type=int, default=8)
    ap.add_argument('--months', type=int, default=240)
    ap.add_argument('--hh', type=int, default=20000)
    ap.add_argument('--workers', type=int, default=8)
    ap.add_argument('--cache', default=os.environ.get('FACTS_CACHE'))
    ap.add_argument('--json', help='write targets, model values, data values and flags to this file')
    ap.add_argument('--targets-json', help='write only the targets with sources to this file')
    ap.add_argument('--fred', help='directory of FRED CSVs: add the empirical columns computed by this code')
    a = ap.parse_args()

    data_cols = fred_columns(a.fred) if a.fred else {}
    if a.targets_json:
        with open(a.targets_json, 'w') as f:
            json.dump([dict(t, **({'us_' + c.split()[-1]: data_cols[c].get(t['key']) for c in data_cols}))
                       for t in TARGETS], f, indent=1, default=float)

    t0 = time.time()
    with ThreadPoolExecutor(a.workers) as ex:
        runs = list(ex.map(lambda s: run_model(a.cli, a.hh, a.months, s, a.cache), range(1, a.seeds + 1)))
    wall = time.time() - t0
    samples = [model_sample(h) for h in runs]
    model, msd = summarise(samples)

    cols = list(data_cols)
    hdr = (f"{'fact':46s} {'target':>13s} {'model':>7s} {'seed sd':>7s} {'gap':>8s}" +
           ''.join(f" {c:>10s}" for c in cols))
    print(hdr)
    print('-' * len(hdr))
    rows = []
    for t in TARGETS:
        v = model.get(t['key'])
        f = flag(t, v)
        s = msd.get(t['key'])
        sd_s = f"{s:7.2f}" if s is not None and np.isfinite(s) and s > 0 else ' ' * 7
        vs = f"{v:7.2f}" if v is not None and np.isfinite(v) else '    n/a'
        extra = ''.join(f" {data_cols[c].get(t['key'], float('nan')):10.2f}" for c in cols)
        print(f"{t['label'][:46]:46s} {t['lo']:>6g}..{t['hi']:<5g} {vs} {sd_s} {f:>8s}{extra}")
        rows.append(dict(t, model=v, model_seed_sd=s, gap=f, **{c: data_cols[c].get(t['key']) for c in cols}))
    info = ['g_mean', 'n_rec', 'rec_depth_med', 'gc_per_century', 'dns_n_contr', 'dns_contr_per_century',
            'speed_contr', 'speed_exp', 'dns_dur_contr', 'dns_amp_contr', 'pluck_r2', 'antipluck_r2', 'y_sd', 'g4_sd',
            'ucyc_sd', 'pi_sd', 'dum_ac1', 'corr_i', 'rel_lp', 'corr_cd', 'rel_hours', 'corr_hours']
    print('\nsupplementary (model [US postwar / GM]):')
    for k in info:
        if k in model or any(k in data_cols[c] for c in cols):
            dv = ' / '.join(f"{data_cols[c][k]:.2f}" for c in cols[:2] if k in data_cols[c])
            mv = f"{model[k]:.2f}" if k in model else 'n/a'
            print(f"  {k:22s} {mv:>8s}  [{dv}]")
    counts = {g: sum(r['gap'] == g for r in rows) for g in ('ok', 'minor', 'material', 'SEVERE')}
    print(f"\nflags: {counts}")
    print(f"{a.seeds} seeds x {a.months} months x {a.hh} households (first 12 months dropped); "
          f"simulation wall time {wall:.1f}s (0 if cached); CLI {a.cli}")
    if a.json:
        with open(a.json, 'w') as f:
            json.dump(dict(rows=rows, model=model, model_seed_sd=msd, data=data_cols,
                           run=dict(seeds=a.seeds, months=a.months, hh=a.hh, wall_s=wall, cli=a.cli,
                                    cfg=os.environ.get('ECONSIM_CFG', ''))), f, indent=1, default=float)


if __name__ == '__main__':
    main()
