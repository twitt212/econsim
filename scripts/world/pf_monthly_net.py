import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import copy, numpy as np
import world_net as wn
from world_ref import pstar, in2019_path, summarize, AE, EME, FLO, S_MAT, W_MAT
from bloc_params import quarterly, BLOCS12, RHO_G
from bloc_model import convert_to_monthly

def pf_monthly(cfg, ffr, usy, T=240):
    S, W = S_MAT, W_MAT
    params = {}
    for b in BLOCS12:
        q = dict(quarterly(b)); q.setdefault('phi', 0.8)
        c = cfg[b]
        q['k_us'] = c['k_us']; q['theta'] = q['theta'] * c['theta_scale']
        if c.get('uip_w') is not None: q['uip_w'] = c['uip_w']
        params[b] = convert_to_monthly(q)
    for b in BLOCS12: params[b]['phi'] = params[b].get('phi', 0.8)
    M = wn.build_world(params, S, W, k=3)
    for i, (name, terms) in enumerate(M.eqs):
        b = name.split(':')[0]
        if name.endswith(':rule') and cfg[b]['g_z']:
            M.eqs[i] = (name, list(terms) + [(-(1 - params[b]['g1']) * cfg[b]['g_z'], f'{b}_z', 0)])
    rhoG = RHO_G ** (1 / 3.0)
    x_ = np.r_[np.zeros(3), ffr[:T - 3]]
    G = np.zeros(T)
    for t in range(T): G[t] = (1 - rhoG) * x_[t] + (rhoG * G[t - 1] if t else 0.0)
    rhs = {}
    for b in BLOCS12:
        p = params[b]
        Fl = np.r_[np.zeros(3), usy[:T - 3]]
        rhs[f'{b}:IS'] = p['b5'] * S.loc[b, 'US'] * Fl - p['theta'] * G
        rhs[f'{b}:S'] = -(ffr[:T] + cfg[b]['lam_rp'] * G)
        rhs[f'{b}:rule'] = (1 - p['g1']) * cfg[b]['k_us'] * ffr[:T]
    return M.solve(T, rhs)

if __name__ == '__main__':
    ffr, usy = in2019_path(240)
    cfg = pstar()
    sol = pf_monthly(cfg, ffr, usy, 240)
    class Obj: pass
    bl = {}
    for b in BLOCS12:
        o = Obj(); o.hist = {v: sol[f'{b}_{v}'] for v in ['y', 'pi', 'i', 'z']}; bl[b] = o
    summarize(bl)
