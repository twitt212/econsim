"""Tiny linear perfect-foresight (stacked boundary-value) solver used for IRF checks.

Equations are lists of terms (coef, var, lag) with lag<0 = past, lag>0 = lead.
Each equation k at time t reads  sum_terms coef * x[var, t+lag] = rhs_k[t].
Values outside [0, T-1] are zero (economy starts at steady state, returns to it at T).
Pure numpy/scipy; no external code.
"""
import numpy as np
import scipy.sparse as sp
import scipy.sparse.linalg as spla


class LinModel:
    def __init__(self, variables):
        self.vars = list(variables)
        self.ix = {v: i for i, v in enumerate(self.vars)}
        self.eqs = []          # list of (name, terms)

    def add(self, name, terms):
        for c, v, l in terms:
            assert v in self.ix, f"unknown var {v} in {name}"
        self.eqs.append((name, list(terms)))

    def solve(self, T, rhs, pre=None):
        """rhs: dict eq_name -> array(T) of right-hand-side (shock) values. Returns dict var->array(T).
        pre: optional dict var -> array of pre-sample values x[-P..-1] (index P-1 is t=-1); others are zero."""
        n = len(self.vars)
        assert len(self.eqs) == n, f"{len(self.eqs)} equations vs {n} variables"
        rows, cols, vals = [], [], []
        b = np.zeros(n * T)
        for k, (name, terms) in enumerate(self.eqs):
            r = rhs.get(name)
            for t in range(T):
                row = k * T + t
                if r is not None:
                    b[row] = r[t]
                for c, v, l in terms:
                    tt = t + l
                    if 0 <= tt < T:
                        rows.append(row)
                        cols.append(self.ix[v] * T + tt)
                        vals.append(c)
                    elif tt < 0 and pre is not None and v in pre:
                        P = len(pre[v])
                        if -tt <= P:
                            b[row] -= c * pre[v][P + tt]
        A = sp.csc_matrix((vals, (rows, cols)), shape=(n * T, n * T))
        x = spla.spsolve(A, b)
        return {v: x[i * T:(i + 1) * T] for v, i in self.ix.items()}


def half_life(path, peak_idx=None, frac=0.5, dt=1.0):
    """Time (in periods*dt) after the peak for |x| to fall to frac*|peak|."""
    a = np.abs(np.asarray(path))
    if peak_idx is None:
        peak_idx = int(np.argmax(a))
    pk = a[peak_idx]
    for j in range(peak_idx, len(a)):
        if a[j] <= frac * pk:
            # linear interpolation
            if j == peak_idx:
                return 0.0
            x0, x1 = a[j - 1], a[j]
            f = (a[j - 1] - frac * pk) / (a[j - 1] - a[j]) if a[j - 1] != a[j] else 0
            return (j - 1 - peak_idx + f) * dt
    return float('nan')
