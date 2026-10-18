"""Offline solver for one monthly GPM bloc (reference for scripts/world/solve_blocs.py).

Outputs per bloc: C (5 x 33) mapping the state to period-0 values of (y, pi, i, z, mrr), and IR[eq][k] (K x 5), the period-0
response of those five variables to a unit right-hand-side impulse in equation `eq` at period k (eq in IS, PC, rule, S).
"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
import scipy.sparse as sp
import scipy.sparse.linalg as spla
from bloc_model import build, convert_to_monthly

OUT = ['y', 'pi', 'i', 'z', 'mrr']
STATE = [('y', 3), ('i', 1), ('z', 13), ('pi', 13), ('mrr', 3)]
EQ = {'IS': 'IS', 'PC': 'PC', 'rule': 'rule', 'S': 'ze'}      # name of the equation in bloc_model.build
T_SOLVE = 360


class Fact:
    def __init__(self, M, T=T_SOLVE):
        self.M, self.T = M, T
        n = len(M.vars)
        rows, cols, vals = [], [], []
        self.pre_terms = []     # (row, var, lag_index, coef) for out-of-sample lags
        for k, (name, terms) in enumerate(M.eqs):
            for t in range(T):
                row = k * T + t
                for c, v, l in terms:
                    tt = t + l
                    if 0 <= tt < T:
                        rows.append(row); cols.append(M.ix[v] * T + tt); vals.append(c)
                    elif tt < 0:
                        self.pre_terms.append((row, v, tt, c))
        self.lu = spla.splu(sp.csc_matrix((vals, (rows, cols)), shape=(n * T, n * T)))
        self.eqrow = {name: k for k, (name, _) in enumerate(M.eqs)}

    def solve_rhs(self, b):
        x = self.lu.solve(b)
        return {v: x[i * self.T:(i + 1) * self.T] for v, i in self.M.ix.items()}

    def impulse(self, eq, k):
        b = np.zeros(len(self.M.vars) * self.T); b[self.eqrow[eq] * self.T + k] = 1.0
        return self.solve_rhs(b)

    def state_response(self, v0, lag0):
        """Solution with x[v0, -lag0] = 1 (other pre-sample values zero) and no forcing."""
        b = np.zeros(len(self.M.vars) * self.T)
        for row, v, tt, c in self.pre_terms:
            if v == v0 and -tt == lag0:
                b[row] -= c
        return self.solve_rhs(b)


def solve_bloc(q, g_z=0.0, K=120):
    """q: quarterly coefficient dict (bloc_params.quarterly + overrides, incl. phi, uip_w, theta, k_us)."""
    m = convert_to_monthly(q)
    M = build(m, k=3, uip='closed')
    if g_z:
        for i, (name, terms) in enumerate(M.eqs):
            if name == 'rule':
                M.eqs[i] = (name, list(terms) + [(-(1 - m['g1']) * g_z, 'z', 0)])
    F = Fact(M)
    idx = [(v, lag) for v, n in STATE for lag in range(1, n + 1)]
    C = np.zeros((len(OUT), len(idx)))
    for j, (v0, l0) in enumerate(idx):
        sol = F.state_response(v0, l0)
        C[:, j] = [sol[v][0] for v in OUT]
    IR = {}
    for key, eq in EQ.items():
        arr = np.zeros((K, len(OUT)))
        for k in range(K):
            sol = F.impulse(eq, k)
            arr[k] = [sol[v][0] for v in OUT]
        IR[key] = arr
    return dict(m=m, C=C, idx=idx, IR=IR)


if __name__ == '__main__':
    import time
    from bloc_params import quarterly
    t = time.time()
    q = dict(quarterly('EA')); q.setdefault('phi', 0.8)
    r = solve_bloc(q)
    print('solved in %.1fs' % (time.time() - t), r['C'].shape, {k: v.shape for k, v in r['IR'].items()})
