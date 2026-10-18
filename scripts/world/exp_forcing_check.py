import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import numpy as np
import exp_forcing as X
from exp_forcing import *
# pure AR(1) forcing check: ffr_t = 0.95^t, no F, no G
X.ffr[:] = 0.95 ** np.arange(T); X.usy[:] = 0.0
b = 'EA'
q, m = setup(b, 0.3, 0.0, 0.0)   # theta=0
lam = 0.0
pf = pf_solution(b, q, m, lam)
A, C, idx, m2 = statespace(q | {'theta': 0.0}, T=300)
M = build(m, k=3, uip='closed')
IR = impulse_responses(M)
sq = sequential(b, q, m, lam, idx, C, IR, dict(F=0.95, G=0.928, S=0.95, RP=0.928, R=0.95))
for v in ['y', 'pi', 'i', 'z']:
    print(v, 'PF', np.round(pf[v][[0, 6, 12, 24, 36]], 4), 'seq', np.round(sq[v][[0, 6, 12, 24, 36]], 4), 'maxabs diff', float(np.max(np.abs(pf[v][:120] - sq[v][:120]))))
