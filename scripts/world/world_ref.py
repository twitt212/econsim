#!/usr/bin/env python3
"""Reference implementation of the b8-world bloc network (spec sections 2.1-2.3), in the form the Rust code uses:
sequential monthly rational expectations,   x_t = C s_t + sum_e sum_k IR_e[k] f_e[t+k] + sum_e tail_e f_e[K-1]   with K = 240 forcing kernels.

Everything is driven by committed files (locations: paths.py); there are no hidden overrides:
  * P* (pre-registered calibration)      paths.PSTAR   (load_pstar(); uip_w 'research' = the quarterly table value)
  * research coefficients                 paths.research_json('blocs') (via bloc_params.py; `selftest` checks monthly_coefficients)
  * projection persistences (spec D17)    rho_F 0.93, rho_P 0.95, rho_oil 0.98, rho_qty 0.97 (Opt defaults)
  * forcing horizon K = 240 (+ tail from kernels solved to 480), stacked solve T = 720 for kernels, T = 480 for the pin tables

Commands (python3 -B world_ref.py <cmd>):
  selftest            monthly coefficients equal research monthly_coefficients; bands of this file equal targets.json (bracket pairs); layout check
  gates [--json F]    W1-01..W1-05, W1-08, W1-10..W1-20, W1-24..W1-28 (reference simulation, P*); R rows print REPORT
  w109                W1-09: sequential algorithm = perfect foresight (single bloc, and 12-bloc network with oracle cross terms)
  kcheck              forcing-horizon test (D29): K = 240 plus tail against K = 480 and K = 960; persistence 0.99 and permanent addend
  goldens [--write|--check]   regenerate (write) or compare (check) the golden files: stage1, irf_unit, pf_ar1_EA, kref, inputs_clean, inputs_stress
  gen-inputs {clean|stress} [F]   write one JSON-lines replay file with the reference outputs embedded
  --inputs F          replay a JSON-lines file (written by `econsim-cli world-dump` or by gen-inputs); prints max abs difference
  centring            W1-31 cross-check with real ABM policy-rate series (set --offset; runs from WORLD_RUNS)

Replay file format (JSON lines, UTF-8).  Line 0:  {"header": {"k": 240, "rho_f": .., "rho_p": .., "rho_oil": .., "rho_qty": .., "ius_offset": ..,
 "fx_rate_knee": 0.02, "fx_rate_sat_scale": 0.0, "pstar_sha": "..", "shocks": [{"type": "BlocPolicy", "bloc": "EA", "bps": 100, "months": 12, "t0": 0},
 {"type": "BlocDemand", "bloc": "CN", "pct": -3, "half_life_months": 12, "t0": 0}, ...]}}   (shocks: BlocPolicy, BlocDemand, GlobalDemand; each is
 active, and known to all agents, from month t0 on).
Lines 1..N (one per month t, in order):  {"t": t, "ius": pp (offset applied, NOT M-F bounded), "ex": [K floats, pp, offset applied, not bounded],
 "y_us": y_US(t-1) in %, "oil": p_bar gap %, "qty": OPEC+ output deviation %, "fex": VIX points,
 "dollar": log points (level a_t of the Dollar shocks, current month, after firing), "dollar_decay": monthly decay in effect (1.0 if unset),
 "eps": null | [[e_y, e_pi, e_i, eta] x 12], "out": {"y": [12], "pi": [12], "i": [12], "z": [12]}}.
Bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW.  "out" is optional; if present the replay prints the max abs difference.
The reference applies the M-F bound (header knee/scale) to ius and ex in the rule and UIP forcing only (never in G).
"""
import os, sys, json, math, hashlib, argparse, re
HERE = os.path.dirname(os.path.abspath(__file__)); sys.path.insert(0, HERE)
import numpy as np
import pandas as pd
import paths
from bloc_ir import OUT
from bloc_params import quarterly, BLOCS12, RHO_G
from bloc_model import convert_to_monthly
from pin_tables import pin_eps

DATA = paths.CSV_DIR
PSTAR_FILE = paths.PSTAR
TARGETS_FILE = paths.TARGETS
RESEARCH_BLOCS = paths.research_json('blocs')
CACHE = paths.CACHE_DIR
K = 240
T_SOLVE = K + 240              # pin tables
K_TAIL = 2 * K                 # kernels are solved to 2K; IR[e][K:2K] is summed into tail[e] (D29)
T_KERNEL = K_TAIL + 240        # 720
NB = 12
BI = {b: i for i, b in enumerate(BLOCS12)}
AE = ['EA', 'JP', 'UK', 'CA', 'AU']; EME = ['CN', 'IN', 'KR', 'BR', 'MX']; FLO = ['IN', 'KR', 'BR', 'MX']
RHO_G_M = RHO_G ** (1 / 3.0)       # 0.9283
RHO_RP_M = 0.8 ** (1 / 3.0)        # premium state; blocs FX.risk_premium_shock_AR_quarterly = 0.8 per quarter
UNIT_SHOCKS = ('demand', 'cost_push', 'policy', 'risk_premium')      # index of eps[i][j]


class Opt:
    """Run options. Defaults are the spec values (D17, D33); nothing else in this file overrides them."""
    def __init__(self, rho_F=0.93, rho_P=0.95, rho_oil=0.98, rho_qty=0.97, rho_fex=0.83, theta_f_ratio=0.5, vix_per_100bp=13.0, opec_va_share=0.20,
                 fx_rate_knee=0.02, fx_rate_sat_scale=0.0):
        self.rho_F, self.rho_P, self.rho_oil, self.rho_qty, self.rho_fex = rho_F, rho_P, rho_oil, rho_qty, rho_fex
        self.theta_f_ratio, self.vix_per_100bp, self.opec_va_share = theta_f_ratio, vix_per_100bp, opec_va_share
        self.knee, self.sat = fx_rate_knee, fx_rate_sat_scale

    def bound(self, x):
        """b7-transmission M-F on a pp series: 100 * bounded_rate_differential(x/100, knee, scale); identity when scale == 0."""
        x = np.asarray(x, float)
        if not (self.sat > 0): return x
        d = x / 100.0
        out = np.where(d > self.knee, self.knee + self.sat * np.tanh((d - self.knee) / self.sat), d)
        return 100.0 * out


# ------------------------------------------------------------------ data
S_MAT = pd.read_csv(paths.csv('bloc_spillover_coeffs_2024.csv'), index_col=0)     # [receiving i][partner o]
W_MAT = pd.read_csv(paths.csv('bloc_trade_weights_2024.csv'), index_col=0)        # [own i][partner o], rows sum to 1 over US + 11 blocs
S_US = np.array([S_MAT.loc[b, 'US'] for b in BLOCS12]); W_US = np.array([W_MAT.loc[b, 'US'] for b in BLOCS12])
S_BB = np.array([[(S_MAT.loc[b, o] if o != b else 0.0) for o in BLOCS12] for b in BLOCS12])
W_BB = np.array([[(W_MAT.loc[b, o] if o != b else 0.0) for o in BLOCS12] for b in BLOCS12])
W_SUM = W_BB.sum(axis=1)           # sum over the 11 non-US partners (= 1 - w_iUS)


def pstar_sha():
    return hashlib.sha256(open(PSTAR_FILE, 'rb').read()).hexdigest()[:16]


def load_pstar(path=None):
    """P* table from the committed JSON (the same file solve_blocs.py reads). uip_w 'research' -> None (quarterly table value)."""
    raw = json.load(open(path or PSTAR_FILE)); cfg = {}
    for b in BLOCS12:
        r = raw[b]
        cfg[b] = dict(k_us=float(r['k_us']), lam_rp=float(r['lam_rp']), theta_scale=float(r['theta_scale']), g_z=float(r['g_z']),
                      uip_w=None if r['uip_w'] == 'research' else float(r['uip_w']), oil_is_scale=float(r.get('oil_is_scale', 1.0)))
    return cfg

pstar = load_pstar      # name used by the older experiment scripts

_RES = None
def research():
    global _RES
    if _RES is None: _RES = json.load(open(RESEARCH_BLOCS))
    return _RES


def bloc_q(name, c):
    q = dict(quarterly(name)); q.setdefault('phi', 0.8)
    q['k_us'] = c['k_us']; q['theta'] = q['theta'] * c['theta_scale']
    if c.get('uip_w') is not None: q['uip_w'] = c['uip_w']
    return q


def solve_horizon(q, g_z, Kk=K, T=None):
    """Stacked perfect-foresight solve (bloc_ir.Fact) with horizon T = Kk + 240: C (5 x 33) and IR[e] (Kk x 5), e in IS, PC, rule, S."""
    from bloc_ir import Fact, build, STATE, EQ
    T = T or (Kk + 240)
    m = convert_to_monthly(q)
    M = build(m, k=3, uip='closed')
    if g_z:
        for i, (name, terms) in enumerate(M.eqs):
            if name == 'rule': M.eqs[i] = (name, list(terms) + [(-(1 - m['g1']) * g_z, 'z', 0)])
    F = Fact(M, T)
    idx = [(v, lag) for v, n in STATE for lag in range(1, n + 1)]
    C = np.zeros((len(OUT), len(idx)))
    for j, (v0, l0) in enumerate(idx):
        sol = F.state_response(v0, l0); C[:, j] = [sol[v][0] for v in OUT]
    IR = {}
    for key, eq in EQ.items():
        arr = np.zeros((Kk, len(OUT)))
        for k in range(Kk):
            sol = F.impulse(eq, k); arr[k] = [sol[v][0] for v in OUT]
        IR[key] = arr
    return dict(m=m, C=C, idx=idx, IR=IR)


def _qkey(q, g_z, *extra):
    return hashlib.sha1(json.dumps([sorted((k, float(v)) for k, v in q.items() if v is not None), float(g_z), *extra]).encode()).hexdigest()[:20]


def _cached_solve(q, g_z):
    os.makedirs(CACHE, exist_ok=True)
    f = os.path.join(CACHE, _qkey(q, g_z, K, K_TAIL) + '.npz')
    if os.path.exists(f):
        z = np.load(f, allow_pickle=True)
        return dict(m=z['m'].item(), C=z['C'], idx=[tuple(x) for x in z['idx'].tolist()], IR={e: z['IR_' + e] for e in ('IS', 'PC', 'rule', 'S')},
                    tail={e: z['TAIL_' + e] for e in ('IS', 'PC', 'rule', 'S')})
    full = solve_horizon(q, g_z, Kk=K_TAIL, T=T_KERNEL)
    sol = dict(m=full['m'], C=full['C'], idx=full['idx'], IR={e: full['IR'][e][:K] for e in full['IR']}, tail={e: full['IR'][e][K:].sum(0) for e in full['IR']})
    np.savez(f, m=np.array(sol['m'], dtype=object), C=sol['C'], idx=np.array(sol['idx'], dtype=object),
             **{'IR_' + e: sol['IR'][e] for e in sol['IR']}, **{'TAIL_' + e: sol['tail'][e] for e in sol['tail']})
    return sol


def _cached_big(q, g_z, Kk):
    """IR kernels solved to Kk (T = Kk + 240) for the K = 480 / 960 reference sums."""
    os.makedirs(CACHE, exist_ok=True)
    f = os.path.join(CACHE, _qkey(q, g_z, 'big', Kk) + '.npz')
    if os.path.exists(f):
        z = np.load(f); return {e: z['IR_' + e] for e in ('IS', 'PC', 'rule', 'S')}
    IR = solve_horizon(q, g_z, Kk=Kk)['IR']
    np.savez(f, **{'IR_' + e: IR[e] for e in IR})
    return IR


class Bloc:
    def __init__(self, name, c, opt):
        self.name = name; self.c = c; self.opt = opt
        self.q = bloc_q(name, c)
        sol = _cached_solve(self.q, c['g_z'])
        self.m, self.C, self.idx, self.IR, self.tail = sol['m'], sol['C'], sol['idx'], sol['IR'], sol['tail']
        self.pos = {k_: r for r, k_ in enumerate(self.idx)}
        self.s = np.zeros(len(self.idx)); self.G = 0.0; self.u = 0.0
        self.hist = {v: [] for v in OUT}
        rb = research()['blocs'][name]
        self.beta_oil = rb['oil']['IS_oil_coefficient_monthly']['value'] * c['oil_is_scale']
        self.beta_q = opt.opec_va_share * (1 - self.m['b1'] - self.m['b2']) if name == 'OPEC+' else 0.0
        self.theta_f = self.m['theta'] * opt.theta_f_ratio / opt.vix_per_100bp
        self._pin = {}

    def pin(self, L):
        if L not in self._pin:
            e, _ = pin_eps(self.q, L, self.c['g_z'], T=T_SOLVE); self._pin[L] = e
        return self._pin[L]


def make_blocs(cfg, opt):
    return {b: Bloc(b, cfg[b], opt) for b in BLOCS12}


# ------------------------------------------------------------------ US inputs
def dollar_state_path(shocks, n):
    """Level a_t (log points) and effective decay d_t of the Dollar shocks, as ShockState does it (shocks.rs activate_due, run once per month in exog):
    a_t = a_{t-1} * d_{t-1} + sum of the shocks firing at t;  d is replaced by the latest firing shock's decay (1.0 for half-life <= 0; unset = 1.0)."""
    x = 0.0; dec = 0.0; lev = []; dd = []
    for t in range(n):
        x *= (1.0 if dec == 0.0 else dec)
        for sh in shocks:
            if sh['type'] == 'Dollar' and int(sh.get('t0', 0)) == t:
                x += math.log(max(1 + sh['pct'] / 100.0, 0.01))
                hl = float(sh.get('half_life_months', 0))
                dec = 1.0 if hl <= 0 else 0.5 ** (1.0 / hl)
        lev.append(x); dd.append(1.0 if dec == 0.0 else dec)
    return np.array(lev), np.array(dd)


class UsPath:
    """Per-month inputs of section 2.1.  ius[j]: realised US policy deviation (pp, ius_offset already applied); ex(t): the expected path seen at t
    (K values, pp); usy[j]: y_US of month j (known from month j+1 on); oil/qty/fex/dollar as in the replay format."""
    def __init__(self, ius, usy, mode='path', rho_proj=0.97, oil=None, qty=None, fex=None, dollar=None, ddecay=None, eps=None, ex_rows=None):
        self.ius = np.asarray(ius, float); self.usy = np.asarray(usy, float); self.mode, self.rho_proj = mode, rho_proj
        n = len(self.ius); self.n = n
        z = lambda a: np.zeros(n) if a is None else np.asarray(a, float)
        self._oil, self._qty, self._fex, self._dollar = z(oil), z(qty), z(fex), z(dollar)
        self._dd = np.ones(n) if ddecay is None else np.asarray(ddecay, float)
        self._eps, self._ex_rows = eps, ex_rows
    def I(self, j): return self.ius[j] if 0 <= j < self.n else 0.0
    def Y(self, j): return self.usy[j] if 0 <= j < len(self.usy) else 0.0
    def oil(self, j): return self._oil[j] if 0 <= j < self.n else 0.0
    def qty(self, t): return self._qty[t] if 0 <= t < self.n else 0.0
    def fex(self, t): return self._fex[t] if 0 <= t < self.n else 0.0
    def a_at(self, t, j):
        """Dollar-shock level seen at month t for calendar month j: realised for j <= t, a_t * d_t^(j-t) beyond (shocks scheduled but not yet fired are not anticipated)."""
        if j < 0 or t < 0: return 0.0
        if j <= t: return self._dollar[j] if j < self.n else 0.0
        return (self._dollar[t] if t < self.n else 0.0) * (self._dd[t] if t < self.n else 1.0) ** (j - t)
    def eps(self, t): return None if self._eps is None or t >= len(self._eps) else self._eps[t]
    def ex(self, t):
        if self._ex_rows is not None: return np.asarray(self._ex_rows[t], float)
        if self.mode == 'path': return np.array([self.I(t + k) for k in range(K)])
        return self.I(t) * self.rho_proj ** np.arange(K)      # 'ar1'


def in2019_path(T=120):
    """US policy-deviation and GDP path read from Iacoviello-Navarro (2019) Fig. 4 (monthly interpolation of the quarterly read-off)."""
    from us_shock_in2019 import q_pts, ffr_pts, usy_pts
    months = np.arange(T)
    ffr = np.interp(months / 3.0, q_pts, ffr_pts, right=0.0); usy = np.interp(months / 3.0, q_pts, usy_pts, right=0.0)
    decay = np.where(months / 3.0 > 20, 0.85 ** (months / 3.0 - 20), 1.0)
    return ffr * decay, usy * decay


# ------------------------------------------------------------------ shocks
def _shift(B, s, x):
    xd = dict(zip(OUT, x)); s_new = np.zeros(len(B.idx))
    for r, (v, lag) in enumerate(B.idx):
        s_new[r] = xd[v] if lag == 1 else s[B.pos[(v, lag - 1)]]
    return s_new


def demand_scale(B, hl, pct):
    """Scale of the unit IS path 0.5^(k/hl) so that the isolated bloc's peak gap equals pct (unit path gives y > 0; pct < 0 -> negative scale).
    Isolated run (zero US inputs, no network) over max(36, ceil(4 hl)) months, at most 240, with the tail term (spec 2.2.6)."""
    n = int(min(240, max(36, math.ceil(4 * hl))))
    uis = 0.5 ** (np.arange(n + K) / hl)
    s = np.zeros(len(B.idx)); ys = []
    for t in range(n):
        f = uis[t:t + K]
        x = B.C @ s + (B.IR['IS'] * f[:, None]).sum(0) + B.tail['IS'] * f[K - 1]
        ys.append(x[0]); s = _shift(B, s, x)
    return pct / max(ys)


def shock_paths(bl, shocks, n):
    """Per bloc, lists of (t0, array) for the rule innovation (pin) and the IS forcing (uIS); an entry is seen by the blocs from month t0 on."""
    pin = {b: [] for b in BLOCS12}; uis = {b: [] for b in BLOCS12}
    for sh in shocks:
        t0 = int(sh.get('t0', 0)); typ = sh['type']
        if typ == 'BlocPolicy':
            L = int(sh['months']); e = bl[sh['bloc']].pin(L)
            arr = np.zeros(n + K)
            for s in range(L):
                if t0 + s < n + K: arr[t0 + s] += sh['bps'] / 100.0 * e[s]
            pin[sh['bloc']].append((t0, arr))
        elif typ in ('BlocDemand', 'GlobalDemand'):
            blist = [sh['bloc']] if typ == 'BlocDemand' else BLOCS12
            hl = float(sh['half_life_months'])
            for b in blist:
                sc = demand_scale(bl[b], hl, sh['pct'])
                arr = np.zeros(n + K)
                for k in range(n + K - t0): arr[t0 + k] = sc * 0.5 ** (k / hl)
                uis[b].append((t0, arr))
    return pin, uis


# ------------------------------------------------------------------ the simulation
def run(cfg, us, T, opt=None, shocks=(), network=True, oracle=None, blocs=None, record=None, use_tail=True):
    """Sequential monthly simulation of the 12 blocs for months 0..T-1.
    network=False is the isolated mode (spec 1.9: Net = None): F = Peff = 0, US inputs as given (pass zeros for a bloc on its own).
    oracle: dict of perfect-foresight paths '<b>_y' / '<b>_z' used for the cross-bloc terms at ALL horizons (W1-09 network test).
    record: optional list; receives one dict {y,pi,i,z: [12]} per month (for replay files)."""
    opt = opt or Opt()
    bl = blocs or make_blocs(cfg, opt)
    n = max(T, us.n) + 1
    pin, uis = shock_paths(bl, shocks, n)
    Y = np.zeros((NB, T + 1)); Zm = np.zeros((NB, T + 1))
    if oracle is not None:
        OY = np.array([np.asarray(oracle[f'{b}_y'], float) for b in BLOCS12]); OZ = np.array([np.asarray(oracle[f'{b}_z'], float) for b in BLOCS12])
    ks = np.arange(K)
    for t in range(T):
        ex = us.ex(t); eps = us.eps(t)
        exb = opt.bound(ex); iusb = float(opt.bound(us.I(t)))          # M-F: rule and UIP forcing only
        zlast = Zm[:, t - 1] if t >= 1 else np.zeros(NB)
        newx = {}
        for i, b in enumerate(BLOCS12):
            B = bl[b]; m = B.m; c = B.c
            B.G = RHO_G_M * B.G + (1 - RHO_G_M) * (us.I(t - 3) if t >= 3 else 0.0)
            B.u = RHO_RP_M * B.u + (eps[i][3] if eps is not None else 0.0)
            Gp = np.zeros(K); Gp[0] = B.G
            for k in range(1, K):
                j = t + k - 3
                Gp[k] = RHO_G_M * Gp[k - 1] + (1 - RHO_G_M) * (us.I(j) if j < t else ex[j - t])
            if network:
                def F_at(j):          # foreign-activity input of bloc i at calendar month j
                    if j < 0: return 0.0
                    if oracle is not None: return S_US[i] * us.Y(j) + float(S_BB[i] @ OY[:, j])
                    return S_US[i] * us.Y(j) + float(S_BB[i] @ Y[:, j])
                Flast = F_at(t - 1) if t >= 1 else 0.0
                Fv = np.zeros(K)
                for k in range(K):
                    j = t + k - 3
                    Fv[k] = F_at(j) if (oracle is not None or j <= t - 1) else Flast * opt.rho_F ** (j - (t - 1))
                Pj = np.zeros(K + 14)                       # Peff_{i,j}, j = t - 13 + n
                wz_last = float(W_BB[i] @ zlast)
                for n_ in range(K + 14):
                    j = t - 13 + n_
                    if j < 0: continue
                    if oracle is not None: zp = float(W_BB[i] @ OZ[:, j])
                    elif j <= t - 1: zp = float(W_BB[i] @ Zm[:, j])
                    else: zp = wz_last * opt.rho_P ** (j - (t - 1))
                    Pj[n_] = zp + 100.0 * us.a_at(t, j) * (W_SUM[i] - 1.0)
                cs = np.concatenate([[0.0], np.cumsum(Pj)])
                win = cs[12:12 + K] - cs[:K]                  # sum_{l=2..13} Peff_{t+k-l}  = Pj[k .. k+11]
                dP = Pj[13:13 + K] - Pj[1:1 + K]              # Peff_{t+k} - Peff_{t+k-12}
            else:
                Fv = np.zeros(K); win = np.zeros(K); dP = np.zeros(K)
            ov = np.zeros(K)
            if B.beta_oil != 0.0:
                for k in range(K):
                    j = t + k - 3
                    ov[k] = us.oil(j) if j <= t else us.oil(t) * opt.rho_oil ** (j - t)
            qv = us.qty(t) * opt.rho_qty ** ks if B.beta_q != 0.0 else np.zeros(K)
            fexv = us.fex(t) * opt.rho_fex ** ks
            uis_t = sum((arr[t:t + K] for (t0, arr) in uis[b] if t0 <= t), np.zeros(K))
            pin_t = sum((arr[t:t + K] for (t0, arr) in pin[b] if t0 <= t), np.zeros(K))
            fIS = m['b5'] * Fv - m['theta'] * Gp - B.theta_f * fexv + B.beta_oil * ov + B.beta_q * qv - (m['b4'] / 12.0) * win + uis_t
            fPC = -(m['l3'] / 4.0) * dP
            fR = (1 - m['g1']) * c['k_us'] * exb + pin_t
            fS = -(exb + B.u * RHO_RP_M ** ks + c['lam_rp'] * Gp)
            if eps is not None:
                fIS = fIS.copy(); fPC = fPC.copy(); fR = fR.copy()
                fIS[0] += eps[i][0]; fPC[0] += eps[i][1]; fR[0] += eps[i][2]
            x = B.C @ B.s + (B.IR['IS'] * fIS[:, None]).sum(0) + (B.IR['PC'] * fPC[:, None]).sum(0) + (B.IR['rule'] * fR[:, None]).sum(0) + (B.IR['S'] * fS[:, None]).sum(0)
            if use_tail:       # constant continuation of each forcing path beyond k = K-1 (exact for permanent / stored paths)
                x = x + B.tail['IS'] * fIS[K - 1] + B.tail['PC'] * fPC[K - 1] + B.tail['rule'] * fR[K - 1] + B.tail['S'] * fS[K - 1]
            newx[b] = x
        for i, b in enumerate(BLOCS12):
            B = bl[b]; x = newx[b]
            for v, val in zip(OUT, x): B.hist[v].append(float(val))
            B.s = _shift(B, B.s, x)
            Y[i, t] = x[0]; Zm[i, t] = x[3]
        if record is not None:
            record.append({v: [bl[b].hist[v][-1] for b in BLOCS12] for v in ('y', 'pi', 'i', 'z')})
    return bl


def run_net(cfg, ffr, usy, T=60, **kw):
    """Backwards-compatible helper: I&N-style run with the true future US path as the expected path."""
    return run(cfg, UsPath(ffr, usy, mode='path'), T, Opt(**kw))


# ------------------------------------------------------------------ statistics used by the gates
def grp(bl, lst, v):
    return np.array([bl[b].hist[v] for b in lst]).mean(0)


def group_metrics(bl, H=60):
    out = {}
    for g, lst in [('AE', AE), ('EME', EME), ('FLO', FLO)]:
        y, i, z = grp(bl, lst, 'y')[:H], grp(bl, lst, 'i')[:H], grp(bl, lst, 'z')[:H]
        out[g] = dict(ytr=float(y[12:49].min()), ytr_m=12 + int(np.argmin(y[12:49])), y12=float(y[12]), i3=float(i[3]), i24=float(i[24]), i36=float(i[36]),
                      imax=float(i[3:25].max()), zpk=float(z[:25].max()), z24=float(z[24]), z36=float(z[36]))
    y = np.array(bl['CN'].hist['y'])[:H]
    out['CN'] = dict(ytr=float(y[12:49].min()), i24=float(bl['CN'].hist['i'][24]))
    return out

metrics = group_metrics


def group_metrics_from_out(out_rows, H=60):
    """The same statistics from the `out` fields of a replay file (list of {y,pi,i,z: [12]}); used by the Rust test network_in_path_matches_golden."""
    class _B:  pass
    bl = {}
    for i, b in enumerate(BLOCS12):
        o = _B(); o.hist = {v: [r[v][i] for r in out_rows] for v in ('y', 'pi', 'i', 'z')}; bl[b] = o
    return group_metrics(bl, H)


def summarize(bl, hs=(12, 24, 36)):
    def g(lst, v): return [round(float(np.mean([bl[b].hist[v][h] for b in lst])), 2) for h in hs]
    for name, lst in [('AE5', AE), ('EME5', EME), ('FLO4', FLO)]:
        print(f'{name}: y {g(lst,"y")} pi {g(lst,"pi")} i {g(lst,"i")} z {g(lst,"z")}')


def pf_network(cfg, ffr, usy, T):
    from pf_monthly_net import pf_monthly
    return pf_monthly(cfg, ffr, usy, T)


# ------------------------------------------------------------------ gates
BANDS = {   # id -> (lo, hi); the same numbers as the spec table (selftest compares them with targets.json)
    'W1-01': (-0.75, -0.40), 'W1-02': (-0.70, -0.35), 'W1-03': (-0.50, -0.30), 'W1-04': (-1.00, -0.60),
    'W1-10': (-0.60, -0.30), 'W1-11': (-0.70, -0.30), 'W1-12': (-0.30, 0.10), 'W1-13': (-0.20, 0.50), 'W1-14': (-1.20, -0.20),
    'W1-15': (0.20, 1.20), 'W1-16': (4.0, 12.0), 'W1-17': (2.0, 10.0), 'W1-18': (4.0, 16.0), 'W1-19': (2.0, 16.0), 'W1-20': (-0.90, -0.10),
    'W1-24': (-0.80, -0.35), 'W1-25a': (-0.95, -0.50), 'W1-25b': (-1.00, -0.55), 'W1-26': (-0.70, -0.20),
    'W1-27a': (0.05, 0.20), 'W1-27b': (0.02, 0.15), 'W1-28': (0.0, 0.05),
}


def compute_gates(cfg=None, opt=None):
    cfg = cfg or load_pstar(); opt = opt or Opt()
    bl0 = make_blocs(cfg, opt)
    res = {}
    hold = {}
    for b in BLOCS12:
        e, sol = pin_eps(bl0[b].q, 12, bl0[b].c['g_z'], T=T_SOLVE)
        hold[b] = dict(eps=e, **{v: np.asarray(sol[v][:49]) for v in ('y', 'pi', 'i', 'z')})
    res['W1-01'] = (float(hold['EA']['y'].min()), int(hold['EA']['y'].argmin()))
    res['W1-02'] = (float(hold['UK']['y'].min()), int(hold['UK']['y'].argmin()))
    res['W1-03'] = (float(hold['CA']['y'][:24].mean()), None)
    res['W1-04'] = (float(hold['AU']['y'].min()), int(hold['AU']['y'].argmin()))
    res['W1-05'] = ({b: round(float(hold[b]['y'].min()), 2) for b in ['JP', 'CN', 'KR', 'IN', 'MX', 'BR', 'RoW']}, None)
    us0 = UsPath(np.zeros(60), np.zeros(60))
    bl_iso = run(cfg, us0, 49, opt, shocks=[dict(type='BlocPolicy', bloc='EA', bps=100, months=12, t0=0)], network=False)
    res['seq_vs_direct_EA_maxabs'] = (float(np.max(np.abs(np.array(bl_iso['EA'].hist['y'][:49]) - hold['EA']['y']))), None)
    ffr, usy = in2019_path(400)
    bl = run(cfg, UsPath(ffr, usy, mode='path'), 60, opt)
    gm = group_metrics(bl)
    res['W1-10'] = (gm['AE']['ytr'], gm['AE']['ytr_m']); res['W1-11'] = (gm['EME']['ytr'], gm['EME']['ytr_m'])
    res['W1-12'] = (gm['AE']['y12'], None); res['W1-13'] = (gm['AE']['i3'], None); res['W1-14'] = (gm['AE']['i24'], None)
    res['W1-15'] = (gm['FLO']['imax'], None); res['W1-16'] = (gm['AE']['zpk'], None); res['W1-17'] = (gm['AE']['z24'], None)
    res['W1-18'] = (gm['FLO']['zpk'], None); res['W1-19'] = (gm['FLO']['z24'], None); res['W1-20'] = (gm['CN']['ytr'], None)
    res['network_groups'] = (gm, None)
    n = 400
    blo = run(cfg, UsPath(np.zeros(n), np.zeros(n), oil=np.full(n, 10.0)), 85, opt)
    tr = lambda b: float(np.array(blo[b].hist['y'])[:85].min())
    res['W1-24'] = (tr('EA'), None); res['W1-25a'] = (tr('JP'), None); res['W1-25b'] = (tr('CN'), None)
    res['W1-26'] = (float(np.mean([tr(b) for b in ['IN', 'KR', 'BR', 'MX', 'RoW']])), None)
    us_z = UsPath(np.zeros(n), np.zeros(n))
    blc = run(cfg, us_z, 60, opt, shocks=[dict(type='BlocDemand', bloc='CN', pct=-3.0, half_life_months=12, t0=0)])
    pk = lambda bb, b: float(np.abs(np.array(bb[b].hist['y'])).max())
    res['W1-27a'] = (pk(blc, 'EA') / pk(blc, 'CN'), None); res['W1-27b'] = (pk(blc, 'UK') / pk(blc, 'CN'), None)
    ble = run(cfg, us_z, 60, opt, shocks=[dict(type='BlocPolicy', bloc='EA', bps=100, months=12, t0=0)])
    res['W1-28'] = (max(pk(ble, b) for b in BLOCS12 if b != 'EA'), None)
    rad = {}
    for b in BLOCS12:
        B = bl0[b]; n_ = len(B.idx); A = np.zeros((n_, n_))
        for r, (v, lag) in enumerate(B.idx):
            if lag == 1: A[r, :] = B.C[OUT.index(v), :]
            else: A[r, B.pos[(v, lag - 1)]] = 1
        rad[b] = float(np.max(np.abs(np.linalg.eigvals(A))))
    res['W1-08_radius'] = (rad, None)
    res['_hold'] = (hold, None)
    return res


def kind_of(i):
    if not os.path.exists(TARGETS_FILE): return '?'
    base = i[:5]
    for r in json.load(open(TARGETS_FILE)):
        if r['id'] == base: return r['kind']
    return '?'


def print_gates(res):
    print('%-8s %-2s %-10s %-18s %s' % ('id', 'K', 'value', 'band', 'status'))
    for i in sorted(BANDS):
        v = res[i][0]; lo, hi = BANDS[i]; k = kind_of(i)
        st = 'REPORT' if k == 'R' else ('PASS' if lo <= v <= hi else 'FAIL')
        print('%-8s %-2s %-10.4f [%6.2f, %6.2f]  %s %s' % (i, k, v, lo, hi, st, ('@m%d' % res[i][1]) if res[i][1] is not None else ''))
    print('W1-05 (report):', res['W1-05'][0])
    print('sequential isolated run vs direct stacked solution, EA hold-12, max abs diff: %.2e' % res['seq_vs_direct_EA_maxabs'][0])
    print('W1-08 spectral radii:', {b: round(v, 4) for b, v in res['W1-08_radius'][0].items()}, 'max %.4f' % max(res['W1-08_radius'][0].values()))
    print('groups:', json.dumps({g: {k: round(v, 4) for k, v in d.items()} for g, d in res['network_groups'][0].items()}))


# ------------------------------------------------------------------ W1-09: sequential equals perfect foresight
def pf_ar1_ea(T=60, rho=0.95, opt=None):
    """EA with theta = 0 (world_theta_mult 0), isolated (no network), US policy deviation ius_t = rho^t realised and expected (ex[k] = rho^(t+k)).
    Returns (sequential bloc, stacked perfect-foresight solution dict, ius)."""
    cfg = load_pstar(); opt = opt or Opt()
    n = T + K + 280
    ius = rho ** np.arange(n)
    cfg1 = {b: dict(cfg[b]) for b in BLOCS12}; cfg1['EA']['theta_scale'] = 0.0
    bl = run(cfg1, UsPath(ius, np.zeros(n), mode='ar1', rho_proj=rho), T, opt, network=False)
    from bloc_ir import Fact, build
    B = bl['EA']; m = B.m; c = B.c
    M = build(m, k=3, uip='closed')
    Tn = T_SOLVE
    F = Fact(M, Tn)
    G = np.zeros(Tn); xs = np.r_[np.zeros(3), ius[:Tn - 3]]
    for t in range(Tn): G[t] = (1 - RHO_G_M) * xs[t] + (RHO_G_M * G[t - 1] if t else 0.0)
    b_ = np.zeros(len(M.vars) * Tn)
    b_[F.eqrow['rule'] * Tn:(F.eqrow['rule'] + 1) * Tn] = (1 - m['g1']) * c['k_us'] * ius[:Tn]
    b_[F.eqrow['ze'] * Tn:(F.eqrow['ze'] + 1) * Tn] = -(ius[:Tn] + c['lam_rp'] * G)
    return bl, F.solve_rhs(b_), ius


def w109(T=60):
    """(a) single bloc (EA, theta = 0), AR(1) 0.95 policy path = exactly the projected forcing: sequential = stacked perfect foresight.
    (b) 12-bloc network with the cross-bloc terms taken from the perfect-foresight solution at all horizons (oracle): sequential = PF (Python only)."""
    cfg = load_pstar(); opt = Opt(); out = {}
    bl, sol, ius = pf_ar1_ea(T)
    out['single_bloc'] = {v: float(np.max(np.abs(np.array(bl['EA'].hist[v][:T]) - sol[v][:T]))) for v in ('y', 'pi', 'i', 'z')}
    ffr, usy = in2019_path(T + K + 200)
    pf = pf_network(cfg, ffr, usy, T + K + 100)
    bl2 = run(cfg, UsPath(ffr, usy, mode='path'), T, opt, oracle=pf)
    out['network_oracle'] = {f'{b}_{v}': float(np.max(np.abs(np.array(bl2[b].hist[v][:T]) - pf[f'{b}_{v}'][:T]))) for b in BLOCS12 for v in ('y', 'pi', 'i', 'z')}
    out['network_oracle_max'] = max(out['network_oracle'].values())
    return out


# ------------------------------------------------------------------ K test (D29)
def kref(blocs=None):
    """Period-0 (y, pi, i, z) response of every bloc to a forcing path of persistence rho in each equation, from kernels solved to K = 480 and K = 960
    (no tail): the reference for `forcing_horizon_truncation`."""
    cfg = load_pstar(); out = {}
    for b in (blocs or BLOCS12):
        q = bloc_q(b, cfg[b]); out[b] = {}
        irs = {Kk: _cached_big(q, cfg[b]['g_z'], Kk) for Kk in (480, 960)}
        for e in ('IS', 'PC', 'rule', 'S'):
            out[b][e] = {}
            # 0.997 (half-life about 231 months) was added in the stage-1 fix round, where the persistence sweep was worst.
            for rho in (0.99, 0.997, 1.0):
                out[b][e][str(rho)] = {f'K{Kk}': [float(v) for v in (irs[Kk][e] * (rho ** np.arange(Kk))[:, None]).sum(0)[:4]] for Kk in (480, 960)}
    return out


def kcheck(blocs=None):
    """K = 240 alone, and K = 240 plus tail[e] f[K-1], against K = 480 and K = 960, for forcing persistence 0.99 and a permanent addend, in each equation.
    For a permanent addend the comparison with K = 480 is an identity (tail = sum of IR[240:480]); the meaningful reference is K = 960."""
    cfg = load_pstar(); ref = kref(blocs); res = {}
    mx = {'no_tail_vs480_0.99': 0, 'no_tail_vs480_1.0': 0, 'tail_vs480_0.99': 0, 'tail_vs960_0.99': 0, 'tail_vs960_1.0': 0, 'K480_vs960_0.99': 0, 'K480_vs960_1.0': 0}
    worst = {}
    for b in (blocs or BLOCS12):
        small = _cached_solve(bloc_q(b, cfg[b]), cfg[b]['g_z']); res[b] = {}
        for e in ('IS', 'PC', 'rule', 'S'):
            for rho in (0.99, 1.0):
                f = rho ** np.arange(K)
                x240 = (small['IR'][e] * f[:, None]).sum(0)[:4]; xt = x240 + small['tail'][e][:4] * f[K - 1]
                r480 = np.array(ref[b][e][str(rho)]['K480']); r960 = np.array(ref[b][e][str(rho)]['K960'])
                vals = {f'no_tail_vs480_{rho}': np.max(np.abs(r480 - x240)), f'tail_vs960_{rho}': np.max(np.abs(r960 - xt)), f'K480_vs960_{rho}': np.max(np.abs(r960 - r480))}
                if rho == 0.99: vals['tail_vs480_0.99'] = np.max(np.abs(r480 - xt))
                for k, v in vals.items():
                    if v > mx[k]: mx[k] = float(v); worst[k] = f'{b} {e}'
                res[b][f'{e}@{rho}'] = {k: round(float(v), 5) for k, v in vals.items()}
    res['_max'] = {k: round(v, 5) for k, v in mx.items()}; res['_worst'] = worst
    return res


# ------------------------------------------------------------------ unit-shock IRFs (W1-06)
def irf_unit(blocs=('EA', 'JP', 'UK', 'MX'), months=61):
    """Sequential isolated run (network off, zero US inputs) with a unit innovation at t = 0 in demand / cost_push / policy / risk_premium."""
    cfg = load_pstar(); opt = Opt(); out = {}
    for b in blocs:
        out[b] = {}
        for j, name in enumerate(UNIT_SHOCKS):
            eps = [[[0.0] * 4 for _ in BLOCS12] for _ in range(months)]
            eps[0][BI[b]][j] = 1.0
            bl = run(cfg, UsPath(np.zeros(months), np.zeros(months), eps=eps), months, opt, network=False)
            out[b][name] = {v: [float(x) for x in bl[b].hist[v][:months]] for v in ('y', 'pi', 'i', 'z')}
    return out


# ------------------------------------------------------------------ replay files
def _write_replay(path, hdr, rows):
    with open(path, 'w') as f:
        f.write(json.dumps({'header': hdr}) + '\n')
        for r in rows: f.write(json.dumps(r) + '\n')


def gen_inputs(kind, path=None, T=60):
    """kind 'clean': the Iacoviello-Navarro path, no extra shocks (W1-10..20 statistics, per-bloc paths).
    kind 'stress': extra shocks (BlocPolicy EA at t0 = 6, BlocDemand CN -3, permanent Dollar +5% at month 24, oil gap +5 from month 12) and the I&N path
    scaled by 3 so that the M-F bound (knee 0.02, scale 0.015) bites; exercises every forcing channel (W1-23, E)."""
    cfg = load_pstar()
    ffr, usy = in2019_path(T + K + 10); n = len(ffr)
    if kind == 'clean':
        opt = Opt(); shocks = []; scale = 1.0; oil = np.zeros(n)
    else:
        opt = Opt(fx_rate_sat_scale=0.015); scale = 3.0
        shocks = [dict(type='BlocPolicy', bloc='EA', bps=100, months=12, t0=6), dict(type='BlocDemand', bloc='CN', pct=-3.0, half_life_months=12, t0=0),
                  dict(type='Dollar', pct=5.0, half_life_months=0, t0=24)]
        oil = np.where(np.arange(n) >= 12, 5.0, 0.0)
    ffr = ffr * scale
    lev, dd = dollar_state_path(shocks, n)
    us = UsPath(ffr, usy, mode='path', oil=oil, dollar=lev, ddecay=dd)
    rec = []
    run(cfg, us, T, opt, shocks=[s for s in shocks if s['type'] != 'Dollar'], record=rec)
    hdr = dict(k=K, rho_f=opt.rho_F, rho_p=opt.rho_P, rho_oil=opt.rho_oil, rho_qty=opt.rho_qty, ius_offset=0.0, fx_rate_knee=opt.knee, fx_rate_sat_scale=opt.sat,
               pstar_sha=pstar_sha(), shocks=shocks)
    rows = [dict(t=t, ius=float(ffr[t]), ex=[round(float(x), 9) for x in us.ex(t)], y_us=float(usy[t - 1]) if t >= 1 else 0.0, oil=float(us.oil(t)), qty=0.0, fex=0.0,
                 dollar=float(lev[t]), dollar_decay=float(dd[t]), eps=None, out={v: [round(x, 10) for x in rec[t][v]] for v in rec[t]}) for t in range(T)]
    path = path or paths.golden('inputs_clean' if kind == 'clean' else 'inputs_stress')
    _write_replay(path, hdr, rows)
    return path


def replay(path):
    lines = [json.loads(l) for l in open(path) if l.strip()]
    hdr = lines[0]['header']; rows = lines[1:]
    assert hdr['k'] == K, 'replay file has K=%d, reference K=%d' % (hdr['k'], K)
    opt = Opt(rho_F=hdr['rho_f'], rho_P=hdr['rho_p'], rho_oil=hdr['rho_oil'], rho_qty=hdr['rho_qty'],
              fx_rate_knee=hdr.get('fx_rate_knee', 0.02), fx_rate_sat_scale=hdr.get('fx_rate_sat_scale', 0.0))
    cfg = load_pstar()
    if hdr.get('pstar_sha') and hdr['pstar_sha'] != pstar_sha(): print('WARNING: pstar_sha differs (file %s, reference %s)' % (hdr['pstar_sha'], pstar_sha()))
    T = len(rows)
    usy = [rows[t + 1]['y_us'] if t + 1 < T else 0.0 for t in range(T)]        # y_US(t) is the value reported at line t+1
    us = UsPath([r['ius'] for r in rows], usy, oil=[r.get('oil', 0.0) for r in rows], qty=[r.get('qty', 0.0) for r in rows], fex=[r.get('fex', 0.0) for r in rows],
                dollar=[r.get('dollar', 0.0) for r in rows], ddecay=[r.get('dollar_decay', 1.0) for r in rows], eps=[r.get('eps') for r in rows],
                ex_rows=[r['ex'] for r in rows])
    rec = []
    run(cfg, us, T, opt, shocks=hdr.get('shocks', []), record=rec)
    worst = {}
    for t, r in enumerate(rows):
        if 'out' not in r: continue
        for v in ('y', 'pi', 'i', 'z'):
            worst[v] = max(worst.get(v, 0.0), float(np.max(np.abs(np.array(rec[t][v]) - np.array(r['out'][v])))))
    print('replay', os.path.basename(path), T, 'months; max abs difference per variable:', {v: '%.2e' % d for v, d in worst.items()})
    return worst


# ------------------------------------------------------------------ W1-31 centring with real ABM series
def abm_ius(path_gz, offset):
    import gzip
    d = json.load(gzip.open(path_gz))
    pr = np.array([r['policy_rate'] for r in d]); rs = np.array([r['r_star_est'] for r in d]); pl = np.array([r['pi_lr'] for r in d])
    return pr - (rs + pl) - offset


def _centring_one(args):
    path, offset, rho_proj = args
    ius = abm_ius(path, offset); n = len(ius)
    bl = run(load_pstar(), UsPath(ius, np.zeros(n), mode='ar1', rho_proj=rho_proj), n, Opt())
    return {b: (float(np.mean(bl[b].hist['y'][12:])), float(np.mean(bl[b].hist['z'][12:]))) for b in BLOCS12}


def centring(runs_dir, tag, seeds, offset, rho_proj=0.97, procs=3):
    """Reference cross-check of W1-31. NOTE: the expected path is approximated by ius_t * rho_proj^k (the ABM's own expected_rate_path is not in the run files)."""
    from multiprocessing import Pool
    jobs = [(os.path.join(runs_dir, f'{tag}_s{s}.json.gz'), offset, rho_proj) for s in seeds]
    with Pool(procs) as p: res = p.map(_centring_one, jobs)
    return {b: (float(np.mean([r[b][0] for r in res])), float(np.mean([r[b][1] for r in res]))) for b in BLOCS12}


# ------------------------------------------------------------------ self test
def _pairs(band):
    return [(float(a), float(b)) for a, b in re.findall(r'\[\s*([+-]?\d+\.?\d*)\s*,\s*([+-]?\d+\.?\d*)\s*\]', band)]


def selftest():
    ok = True
    miss = paths.check()
    if miss: ok = False; print('MISSING files for layout', paths.LAYOUT, miss)
    res = research()['blocs']
    for b in BLOCS12:
        q = dict(quarterly(b)); q.setdefault('phi', 0.8); m = convert_to_monthly(q)
        r = res[b]['monthly_coefficients']
        for k in ('b1', 'b2', 'b3', 'b4', 'b5', 'l1', 'l2', 'l3', 'g1', 'g2', 'g4', 'phi', 'theta'):
            rv = r.get(k); rv = rv['value'] if isinstance(rv, dict) else rv
            if rv is None: continue
            if abs(m[k] - rv) > 5e-5 + 1e-4 * abs(rv):
                ok = False; print('MISMATCH monthly coefficient', b, k, 'reference', m[k], 'research', rv)
    T = {r['id']: r for r in json.load(open(TARGETS_FILE))}
    for i, (lo, hi) in BANDS.items():
        row = T[i[:5]]
        if row['kind'] == 'R': continue                                   # reported rows have no band
        if (lo, hi) not in [(round(a, 6), round(b, 6)) for a, b in _pairs(row['band'])]:
            ok = False; print('band mismatch: %s file "%s" vs code (%s, %s)' % (i, row['band'], lo, hi))
    # golden P* hash and K
    gp = paths.golden('stage1')
    if os.path.exists(gp):
        meta = json.load(open(gp))['meta']
        if meta.get('pstar_sha') != pstar_sha() or meta.get('K') != K: ok = False; print('golden stage1 meta differs from the committed P* / K')
    # M-F helper: identity at scale 0, bounded above the knee
    o0, o1 = Opt(), Opt(fx_rate_sat_scale=0.015)
    if not np.allclose(o0.bound([1.0, 5.0]), [1.0, 5.0]) or not (o1.bound(10.0) < 2.0 + 1.5 + 1e-9): ok = False; print('M-F bound check failed')
    print('selftest', 'OK' if ok else 'FAILED', '; layout', paths.LAYOUT, '; pstar_sha', pstar_sha())
    return ok


# ------------------------------------------------------------------ goldens
def golden_docs():
    """All golden objects in memory: dict name -> (path, obj or list-of-lines)."""
    cfg = load_pstar(); opt = Opt()
    res = compute_gates(cfg, opt)
    g = dict(meta=dict(K=K, K_tail=K_TAIL, T_kernels=T_KERNEL, T_pin=T_SOLVE, rho_F=opt.rho_F, rho_P=opt.rho_P, rho_oil=opt.rho_oil, rho_qty=opt.rho_qty,
                       pstar_sha=pstar_sha(), command='python3 -B ref/world_ref.py goldens --write   (P* is read from the committed P* file; no overrides)'))
    months = [0, 1, 3, 6, 9, 12, 15, 18, 24, 36, 48]
    for b in BLOCS12:
        h = res['_hold'][0][b]
        g[f'hold12_{b}'] = dict(eps=[float(x) for x in h['eps']], months=months, **{v: [float(h[v][mm]) for mm in months] for v in ('y', 'pi', 'i', 'z')})
    ffr, usy = in2019_path(400)
    bl = run(cfg, UsPath(ffr, usy, mode='path'), 60, opt)
    hm = [12, 24, 36, 48]
    g['network_IN_path'] = dict(months=hm, blocs={b: {v: [round(bl[b].hist[v][h], 6) for h in hm] for v in ('y', 'pi', 'i', 'z')} for b in BLOCS12}, groups=res['network_groups'][0])
    g['gates'] = {i: res[i][0] for i in res if not i.startswith('_') and i != 'network_groups'}
    g['gate_months'] = {i: res[i][1] for i in res if not i.startswith('_') and i != 'network_groups' and res[i][1] is not None}
    bl1, sol, ius = pf_ar1_ea(60)
    pf = dict(meta=dict(bloc='EA', theta='0 (world_theta_mult 0)', isolated=True, ius='0.95^t realised, ex[k] = 0.95^(t+k)', months=60, tolerance=1e-4, pstar_sha=pstar_sha()),
              pf={v: [float(x) for x in sol[v][:60]] for v in ('y', 'pi', 'i', 'z')}, ius=[float(x) for x in ius[:60]])
    iu = dict(meta=dict(shocks=list(UNIT_SHOCKS), unit='1.0 at t = 0 in eps[i][j]; isolated bloc (network off, zero US inputs)', months=61, pstar_sha=pstar_sha()), irf=irf_unit())
    kr = dict(meta=dict(note='period-0 (y, pi, i, z) response to a forcing path rho^k in one equation, kernels summed to K = 480 and K = 960, no tail', pstar_sha=pstar_sha()), kref=kref())
    return {'stage1': g, 'pf_ar1_EA': pf, 'irf_unit': iu, 'kref': kr}


def _flat(a, b, path=''):
    """max abs difference between two JSON-like objects (raises on structure mismatch)"""
    if isinstance(a, dict):
        assert a.keys() == b.keys(), (path, set(a) ^ set(b)); return max([_flat(a[k], b[k], path + '/' + k) for k in a] + [0.0])
    if isinstance(a, list):
        assert len(a) == len(b), path; return max([_flat(x, y, path) for x, y in zip(a, b)] + [0.0])
    if isinstance(a, (int, float)) and not isinstance(a, bool): return abs(a - b)
    assert a == b, (path, a, b); return 0.0


def goldens(write=False, check=False):
    docs = golden_docs()
    diffs = {}
    for name, obj in docs.items():
        p = paths.golden(name)
        if write:
            os.makedirs(os.path.dirname(p), exist_ok=True); json.dump(obj, open(p, 'w'), indent=1, default=float); print('wrote', p)
        elif check:
            diffs[name] = _flat(json.loads(json.dumps(obj, default=float)), json.load(open(p)))
    if write:
        for kind in ('clean', 'stress'): print('wrote', gen_inputs(kind))
    if check:
        for kind in ('clean', 'stress'):
            tmp = os.path.join(paths.CACHE_DIR, f'_chk_{kind}.jsonl'); os.makedirs(paths.CACHE_DIR, exist_ok=True)
            gen_inputs(kind, tmp)
            diffs['inputs_' + kind] = 0.0 if open(tmp).read() == open(paths.golden('inputs_' + kind)).read() else float('nan')
            os.remove(tmp)
        print('max abs difference regenerated vs committed:', diffs)
    return docs


if __name__ == '__main__':
    ap = argparse.ArgumentParser()
    ap.add_argument('cmd', nargs='?', default='gates'); ap.add_argument('arg', nargs='?')
    ap.add_argument('--inputs'); ap.add_argument('--json'); ap.add_argument('--write', action='store_true'); ap.add_argument('--check', action='store_true')
    ap.add_argument('--runs', default=paths.RUNS_DIR); ap.add_argument('--tag', default='s1_S6')
    ap.add_argument('--seeds', default='1-12'); ap.add_argument('--offset', type=float, default=0.0)
    a = ap.parse_args()
    if a.inputs: replay(a.inputs)
    elif a.cmd == 'selftest': sys.exit(0 if selftest() else 1)
    elif a.cmd == 'gates':
        r = compute_gates(); print_gates(r)
        if a.json: json.dump({k: v[0] for k, v in r.items() if not k.startswith('_')}, open(a.json, 'w'), indent=1, default=float)
    elif a.cmd == 'goldens': goldens(a.write, a.check)
    elif a.cmd == 'gen-inputs': print('wrote', gen_inputs(a.arg or 'clean', None))
    elif a.cmd == 'w109': print(json.dumps(w109(), indent=1))
    elif a.cmd == 'kcheck': print(json.dumps(kcheck(), indent=1))
    elif a.cmd == 'centring':
        assert a.runs, 'set WORLD_RUNS or --runs'
        lo, hi = [int(x) for x in a.seeds.split('-')]
        out = centring(a.runs, a.tag, range(lo, hi + 1), a.offset)
        print('mean y, z over months 12-239 (%s seeds %s, ius_offset %.3f):' % (a.tag, a.seeds, a.offset))
        for b in BLOCS12: print('  %-6s y %6.3f  z %6.3f' % (b, out[b][0], out[b][1]))
    else:
        ap.error('unknown command')
