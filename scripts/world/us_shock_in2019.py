"""US +100bp shock with the US path read from Iacoviello & Navarro (2019) Fig. 4 (benchmark specification), fed to GPM-style blocs.
Foreign channels: UIP (US real-rate gap), trade (foreign activity), plus an optional financial channel psi: foreign medium-term
real rate gap moves with the US one (psi * mrr_us), the 'imported term premium' of Hellerstein / Chen et al. type.
All numbers quarterly; blocs use GPM6 coefficients (rational expectations, stacked solve)."""
import sys, os
import numpy as np, pandas as pd
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bloc_model import GPM6, build, XI
from lin_pf import LinModel
from us_shock_spillover import ASSIGN, DATA, T

# IN2019 Fig. 4 read-off (approximate, quarters 0..20), US FFR (pp) and US GDP (%)
q_pts = np.array([0, 1, 2, 3, 4, 6, 8, 10, 12, 14, 16, 18, 20])
ffr_pts = np.array([1.0, 1.1, 1.0, 0.9, 0.85, 0.8, 0.4, 0.2, 0.0, -0.2, -0.4, -0.45, -0.5])
usy_pts = np.array([0, -0.15, -0.3, -0.45, -0.55, -0.70, -0.78, -0.85, -0.60, -0.40, -0.20, 0.0, 0.1])
t = np.arange(T)
ffr = np.interp(t, q_pts, ffr_pts, right=0.0)
usy = np.interp(t, q_pts, usy_pts, right=0.0)
# decay to zero after quarter 20
decay = np.where(t > 20, 0.85 ** (t - 20), 1.0)
ffr *= decay; usy *= decay
rr_us = ffr.copy()           # US real-rate gap ~ nominal path (inflation response ignored; flagged in the blocs research report, not published)


def mrr_from_rr(rr):
    n4 = 4
    rr12 = np.array([np.mean([rr[s + j] if s + j < T else 0 for j in range(4)]) for s in range(T)])
    def sh(x, k): return x[k:] if k == 0 else np.r_[x[k:], np.zeros(k)]
    mrr = XI['x1'] * rr + XI['x4'] * rr12
    mrr += XI['x12'] / 3 * sum(np.array([rr12[s + 4 * j] if s + 4 * j < T else 0 for s in range(T)]) for j in range(3))
    mrr += XI['x20'] / 5 * sum(np.array([rr12[s + 4 * j] if s + 4 * j < T else 0 for s in range(T)]) for j in range(5))
    return mrr
mrr_us = mrr_from_rr(rr_us)


def run(bloc, psi, eps_M=1.0, share=None):
    S = pd.read_csv(os.path.join(DATA, 'bloc_spillover_coeffs_2024.csv'), index_col=0)
    sUS = S.loc[bloc, 'US'] * eps_M
    q = dict(GPM6[ASSIGN[bloc]])
    M = build(q, k=1, uip='closed')
    F = np.r_[0.0, usy[:-1]]
    rhs_IS = q['b5'] * sUS * F - q['b3'] * psi * np.r_[0.0, mrr_us[:-1]]
    sol = M.solve(T, {'IS': rhs_IS, 'ze': -rr_us})
    return sol


AE = ['EA', 'JP', 'UK', 'CA', 'AU']
EME = ['CN', 'IN', 'KR', 'BR', 'MX']
if __name__ == '__main__':
    print('US path used: FFR@0,4,8,12', ffr[[0, 4, 8, 12]], ' US GDP@4,8,12', usy[[4, 8, 12]], ' mrr_us@0,4,8', np.round(mrr_us[[0, 4, 8]], 2))
    for psi in [0.0, 0.3, 0.5, 0.7, 1.0]:
        for eps_M in [1.0, 2.0]:
            res = {b: run(b, psi, eps_M) for b in AE + EME}
            ae = np.mean([[res[b]['y'][k] for k in (3, 7, 11, 15)] for b in AE], axis=0)
            em = np.mean([[res[b]['y'][k] for k in (3, 7, 11, 15)] for b in EME], axis=0)
            print('psi %.1f epsM %.0f | AE-avg y@4,8,12,16 %s | EME-avg %s' % (psi, eps_M, np.round(ae, 2), np.round(em, 2)))
