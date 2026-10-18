"""E3: single monthly bloc (EA) driven by US-originated forcing. Compare (a) perfect foresight about the forcing path with
(b) sequential solution with AR(1)-extrapolated forcing (the Rust algorithm): x_t = C s_t + sum_c sum_j D_c[j] e_c[t-j], state shift."""
import sys, os, copy
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bloc_model import GPM6, convert_to_monthly, build
from bloc_params import quarterly, RHO_G, THETA, LAMBDA_RP, K_US
from re_statespace import statespace, STATE, VARS
from us_shock_in2019 import q_pts, ffr_pts, usy_pts
import world_net as wn
S, W = wn.load_mats(1.0)

T = 360
months = np.arange(T)
ffr = np.interp(months / 3.0, q_pts, ffr_pts, right=0.0); usy = np.interp(months / 3.0, q_pts, usy_pts, right=0.0)
decay = np.where(months / 3.0 > 20, 0.85 ** (months / 3.0 - 20), 1.0); ffr *= decay; usy *= decay

def setup(b='EA', kus=None, lam=None, theta=None):
    q = dict(quarterly(b)); q.setdefault('phi', 0.8)
    if kus is not None: q['k_us'] = kus
    if theta is not None: q['theta'] = theta
    m = convert_to_monthly(q)
    return q, m

def exog_paths(b, m, lam):
    rhoG = RHO_G ** (1 / 3.0)
    G = np.zeros(T)
    x_ = np.r_[np.zeros(3), ffr[:T - 3]]
    for t in range(T):
        G[t] = (1 - rhoG) * x_[t] + (rhoG * G[t - 1] if t > 0 else 0.0)
    sUS = S.loc[b, 'US']
    F = usy.copy()
    return G, F, sUS, rhoG

def pf_solution(b, q, m, lam):
    M = build(m, k=3, uip='closed')
    G, F, sUS, rhoG = exog_paths(b, m, lam)
    Fl = np.r_[np.zeros(3), F[:-3]]
    rhs_IS = m['b5'] * sUS * Fl - m['theta'] * G
    rhs_S = -(ffr + lam * G)          # rrus ~ ffr (nominal path), plus risk premium loading on G
    rhs_rule = (1 - m['g1']) * q['k_us'] * ffr
    return M.solve(T, {'IS': rhs_IS, 'ze': rhs_S, 'rule': rhs_rule})

def impulse_responses(M, K=150, eqs=('IS', 'PC', 'rule', 'ze')):
    """IR[eq][k] = vector of x_0 (VARS) response to a unit rhs impulse in equation eq at period k."""
    IR = {}
    for eq in eqs:
        arr = np.zeros((K, len(VARS)))
        for k in range(K):
            e = np.zeros(T); e[k] = 1.0
            sol = M.solve(T, {eq: e})
            arr[k] = [sol[v][0] for v in VARS]
        IR[eq] = arr
    return IR

def channel_D(IR, eq, delay, rho, K=150):
    """D[j], j=0..delay: response of x_0 to e_{t-j}; forcing at t+k uses e_{t+k-delay}."""
    D = np.zeros((delay + 1, len(VARS)))
    for j in range(1, delay + 1):
        D[j] = IR[eq][delay - j]
    D[0] = sum(IR[eq][k] * rho ** (k - delay) for k in range(delay, K))
    return D

def sequential(b, q, m, lam, A_idx, C, IR, rhos):
    G, F, sUS, rhoG = exog_paths(b, m, lam)
    # channels: IS_F (delay 3), IS_G (delay 0), S_rr (delay 0), S_rp (delay 0), rule (delay 0)
    chans = {
        'F': (channel_D(IR, 'IS', 3, rhos['F']), lambda t: m['b5'] * sUS * F[t]),
        'G': (channel_D(IR, 'IS', 0, rhos['G']), lambda t: -m['theta'] * G[t]),
        'S': (channel_D(IR, 'ze', 0, rhos['S']), lambda t: -(ffr[t])),
        'RP': (channel_D(IR, 'ze', 0, rhos['RP']), lambda t: -(lam * G[t])),
        'R': (channel_D(IR, 'rule', 0, rhos['R']), lambda t: (1 - m['g1']) * q['k_us'] * ffr[t]),
    }
    idx = A_idx
    s = np.zeros(len(idx))
    out = {v: np.zeros(T) for v in VARS}
    hist = {c: [] for c in chans}
    for t in range(T):
        x = C @ s
        for c, (D, f) in chans.items():
            hist[c].append(f(t))
            for j in range(D.shape[0]):
                if t - j >= 0:
                    x = x + D[j] * hist[c][t - j]
        for v, val in zip(VARS, x): out[v][t] = val
        xd = dict(zip(VARS, x))
        s_new = np.zeros(len(idx))
        pos = {k_: r for r, k_ in enumerate(idx)}
        for r, (v, lag) in enumerate(idx):
            s_new[r] = xd[v] if lag == 1 else s[pos[(v, lag - 1)]]
        s = s_new
    return out

if __name__ == '__main__':
    b = 'EA'
    for (kus, lam, th) in [(0.0, 1.0, None), (0.3, 2.0, None)]:
        q, m = setup(b, kus, lam, th)
        pf = pf_solution(b, q, m, lam)
        A, C, idx, m2 = statespace(q, T=300)
        M = build(m, k=3, uip='closed')
        IR = impulse_responses(M)
        for rhos in [dict(F=0.95, G=0.928, S=0.97, RP=0.928, R=0.95), dict(F=0.9, G=0.928, S=0.95, RP=0.928, R=0.9), dict(F=0.97, G=0.928, S=0.98, RP=0.928, R=0.97)]:
            sq = sequential(b, q, m, lam, idx, C, IR, rhos)
            print(f'{b} kus {kus} lam {lam} rhos {list(rhos.values())}')
            for v in ['y', 'pi', 'i', 'z']:
                hs = (12, 24, 36)
                print(f'   {v}: PF {[round(float(pf[v][h]),2) for h in hs]} | seq {[round(float(sq[v][h]),2) for h in hs]}')
