import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
from bloc_ir import Fact, build, convert_to_monthly
from bloc_params import quarterly

def pin_eps(q, L, g_z=0.0, T=360):
    """rule-equation innovations eps[0..L) such that the observed rate i is held at 1.0 for months 0..L-1 (autonomous bloc, US inputs
    zero). Direct solve: G[t, s] = response of i_t to a unit rule impulse at period s; eps = G^-1 * 1."""
    m = convert_to_monthly(q)
    M = build(m, k=3, uip='closed')
    if g_z:
        for i_, (name, terms) in enumerate(M.eqs):
            if name == 'rule': M.eqs[i_] = (name, list(terms) + [(-(1 - m['g1']) * g_z, 'z', 0)])
    F = Fact(M, T)
    G = np.zeros((L, L))
    for s_ in range(L):
        sol = F.impulse('rule', s_)
        G[:, s_] = sol['i'][:L]
    e = np.linalg.solve(G, np.ones(L))
    b = np.zeros(len(M.vars) * T)
    b[F.eqrow['rule'] * T:F.eqrow['rule'] * T + L] = e
    sol = F.solve_rhs(b)
    return e, sol


if __name__ == '__main__':
    q = dict(quarterly('EA')); q.setdefault('phi', 0.8)
    for L in (3, 6, 12, 24):
        e, sol = pin_eps(q, L)
        print(L, 'eps[:4]', np.round(e[:4], 3), 'y trough', round(float(sol['y'][:48].min()), 3), '@', int(sol['y'][:48].argmin()), 'pi@12', round(float(sol['pi'][11]), 3), 'z@0', round(float(sol['z'][0]), 3), 'i[:L].min', round(float(sol['i'][:L].min()), 4))
