"""Full multi-bloc (12 blocs + exogenous US) GPM-style network, quarterly or monthly, solved by stacked perfect foresight.

Per bloc i (deviations from steady state):
  rr, rr12, mrr as in bloc_model; IS with foreign activity  b5 * sum_j s_ij y_j(t-1)  (+ optional extra channel);
  REER gap  zeff_i = sum_j w_ij (z_i - z_j)  with z_US = 0, entering IS (4q MA) and Phillips (change over 4q, /4);
  UIP closed form vs the exogenous US real-rate gap (rrus).
Exogenous US inputs: usy (output gap), rrus (real rate gap), optional mrr_us for a financial channel.
"""
import os, sys
import numpy as np
import pandas as pd
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bloc_model import GPM6, XI, convert_to_monthly
from lin_pf import LinModel

HERE = os.path.dirname(os.path.abspath(__file__))
import paths
DATA = paths.CSV_DIR

BLOCS12 = ['EA', 'CN', 'JP', 'UK', 'CA', 'MX', 'IN', 'KR', 'BR', 'AU', 'OPEC+', 'RoW']
# GPM6 column assignment for coefficients (see section 3 of the blocs research report, not published); China/India/OPEC+/RoW refined in params.json
ASSIGN = {'EA': 'EA', 'CN': 'EA6', 'JP': 'JP', 'UK': 'RC6', 'CA': 'RC6', 'MX': 'LA6', 'IN': 'EA6', 'KR': 'EA6', 'BR': 'LA6',
          'AU': 'RC6', 'OPEC+': 'RC6', 'RoW': 'RC6'}


def load_mats(eps_M=1.0):
    S = pd.read_csv(os.path.join(DATA, 'bloc_spillover_coeffs_2024.csv'), index_col=0) * eps_M
    W = pd.read_csv(os.path.join(DATA, 'bloc_trade_weights_2024.csv'), index_col=0)
    return S, W


def build_world(params, S, W, k=1, extra=None):
    """params: dict bloc -> coefficient dict in units of frequency k.  extra: optional dict bloc-> theta for IS financial channel
    (term:  -theta * mrr_us(t-k) is passed through rhs by the caller).  Returns LinModel with names '<bloc>_<var>'."""
    names = []
    for b in BLOCS12:
        names += [f'{b}_{v}' for v in ['y', 'pi', 'i', 'z', 'rr', 'rr12', 'mrr', 'S']]
    M = LinModel(names)
    n4 = 4 * k
    ppy = 4 * k
    zlags = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    lead = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    lag = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    win = list(range(0, 4)) if k == 1 else list(range(-1, 11))
    for b in BLOCS12:
        p = params[b]
        v = lambda s: f'{b}_{s}'
        M.add(f'{b}:rr', [(1, v('rr'), 0), (-1, v('i'), 0)] + [(1.0 / k, v('pi'), j) for j in range(1, k + 1)])
        M.add(f'{b}:rr12', [(1, v('rr12'), 0)] + [(-1.0 / n4, v('rr'), j) for j in range(0, n4)])
        t = [(1, v('mrr'), 0), (-XI['x1'], v('rr'), 0), (-XI['x4'], v('rr12'), 0)]
        t += [(-XI['x12'] / 3.0, v('rr12'), n4 * j) for j in range(0, 3)]
        t += [(-XI['x20'] / 5.0, v('rr12'), n4 * j) for j in range(0, 5)]
        M.add(f'{b}:mrr', t)
        # effective REER gap  zeff = z_b - sum_j w_bj z_j  (z_US = 0, weights sum to one over all 13 partners)
        wsum_nonUS = 0.0
        zterms = lambda lagk, scale: [(scale, v('z'), -lagk)] + [(-scale * W.loc[b, o], f'{o}_z', -lagk) for o in BLOCS12 if o != b and o in W.columns]
        # IS
        t = [(1, v('y'), 0), (-p['b1'], v('y'), -1), (-p['b2'], v('y'), 1), (p['b3'], v('mrr'), -k)]
        for l in zlags:
            t += zterms(l, -p['b4'] / len(zlags))
        # foreign activity from other blocs (US enters through rhs)
        for o in BLOCS12:
            if o != b:
                t += [(-p['b5'] * S.loc[b, o], f'{o}_y', -k)]
        M.add(f'{b}:IS', t)
        # Phillips
        t = [(1, v('pi'), 0)]
        t += [(-p['l1'] / len(lead), v('pi'), l) for l in lead]
        t += [(-(1 - p['l1']) / len(lag), v('pi'), -l) for l in lag]
        t += [(-p['l2'], v('y'), -k)]
        t += zterms(0, -p['l3'] / 4.0) + zterms(n4, p['l3'] / 4.0)
        M.add(f'{b}:PC', t)
        # rule
        t = [(1, v('i'), 0), (-p['g1'], v('i'), -1)]
        t += [(-(1 - p['g1']) * (1 + p['g2']) / len(win), v('pi'), l) for l in win]
        t += [(-(1 - p['g1']) * p['g4'], v('y'), 0)]
        M.add(f'{b}:rule', t)       # rhs = (1-g1)*k_us*i_us(t) + eps_i   (k_us: pass-through of the US policy-rate deviation into the bloc rule)
        # UIP closed form vs US: z_t = rz z_{t-1} - kap * S_t ; S_t - S_{t+1} = rr_t - rrus - eps
        phi = p['phi']
        rz = ((1 - phi) / phi) ** (1.0 / k)
        kap = p.get('uip_w', 1.0) * (1 - rz) / (ppy * (2 * phi - 1))
        M.add(f'{b}:z', [(1, v('z'), 0), (-rz, v('z'), -1), (kap, v('S'), 0)])
        M.add(f'{b}:S', [(1, v('S'), 0), (-1, v('S'), 1), (-1, v('rr'), 0)])    # rhs = -(rrus + eps_rp)
    return M


def make_params(k=1, overrides=None):
    from bloc_params import quarterly
    params = {}
    for b in BLOCS12:
        q = quarterly(b)
        q = dict(q); q.setdefault('phi', 0.8)
        params[b] = convert_to_monthly(q, k) if k == 3 else q
        if overrides and b in overrides:
            params[b].update(overrides[b])
    return params


def us_rhs(S, usy, rrus, b, p, k, mrr_us=None, theta=0.0):
    """right-hand sides contributed by exogenous US variables for bloc b."""
    T = len(usy)
    Fy = np.r_[np.zeros(k), usy[:-k]]
    rhs_IS = p['b5'] * S.loc[b, 'US'] * Fy
    if theta and mrr_us is not None:
        rhs_IS = rhs_IS - theta * np.r_[np.zeros(k), mrr_us[:-k]]
    rhs_S = -rrus
    return rhs_IS, rhs_S


def rule_rhs(p, ius):
    """US policy-rate pass-through term of the bloc rule (k_us default 0)."""
    return (1 - p['g1']) * p.get('k_us', 0.0) * np.asarray(ius)
