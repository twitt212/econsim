"""US 100bp policy-shock spillover experiment on each bloc (GPM-style coefficients, quarterly RE reference).

US block: GPM6 US coefficients treated as a closed economy (no z term).  Bloc: GPM6 column assigned per bloc, with
exogenous US paths entering through (i) UIP (US real-rate gap rrus) and (ii) the foreign-activity term beta5*s_i,US*y_us(t-1)
(s from the bloc trade weights, import elasticity eps_M).  Other blocs are held at zero (partial equilibrium).
Optional global-financial-conditions channel:  y_i  -=  theta_f_i * gfc_{t-?}, with gfc = US real mid-term rate gap (placeholder 'fc').
Prints responses at 4, 8, 12 quarters and writes us_shock_spillover_gpm.csv under the blocs data folder
"""
import sys, os
import numpy as np
import pandas as pd
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bloc_model import GPM6, build
from lin_pf import LinModel

HERE = os.path.dirname(os.path.abspath(__file__))
import paths
DATA = paths.CSV_DIR
T = 120

ASSIGN = {'EA': 'EA', 'JP': 'JP', 'UK': 'RC6', 'CA': 'RC6', 'MX': 'LA6', 'BR': 'LA6', 'KR': 'EA6', 'IN': 'EA6', 'CN': 'EA6', 'AU': 'RC6'}


def us_path(scenario='four_quarters', size=1.0):
    q = dict(GPM6['US']); q['phi'] = 0.8; q['b4'] = 0.0; q['l3'] = 0.0
    if scenario == 'one_shot':
        M = build(q, k=1, uip='closed')
        e = np.zeros(T); e[0] = size
        s = M.solve(T, {'rule': e})
    elif scenario == 'four_quarters':
        # policy rate held +size for 4 quarters (exogenous), then the rule takes over (shock offsets the rule for 4 quarters)
        M = build(q, k=1, uip='closed')
        # find shock path such that i_t = size for t=0..3 : solve with exogenous-rate model for t<4 is awkward; use iteration
        e = np.zeros(T); e[:4] = size
        for _ in range(60):
            s = M.solve(T, {'rule': e})
            e[:4] += (size - s['i'][:4]) * 0.8
        s = M.solve(T, {'rule': e})
    return s


def bloc_response(bloc, us, eps_M=1.0, extra_theta=0.0):
    import pandas as pd
    S = pd.read_csv(os.path.join(DATA, 'bloc_spillover_coeffs_2024.csv'), index_col=0)
    sUS = S.loc[bloc, 'US'] * eps_M
    q = dict(GPM6[ASSIGN[bloc]])
    M = build(q, k=1, uip='closed')
    F = np.r_[0.0, us['y'][:-1]]                        # y_us(t-1)
    rhs_IS = q['b5'] * sUS * F
    rhs_ze = -us['rr']                                  # -(rrus)
    if extra_theta:                                     # financial-conditions channel: tightening via US mid-term real rate gap (lag 1)
        fc = np.r_[0.0, us['mrr'][:-1]]
        rhs_IS = rhs_IS - extra_theta * fc
    sol = M.solve(T, {'IS': rhs_IS, 'ze': rhs_ze})
    return sol, sUS


def main(eps_M=1.0, scenario='four_quarters', target_us_y=-0.7, extra_theta=0.0):
    us = us_path(scenario)
    # scale US shock so that the US output-gap trough = target (IN2019: -0.7% at ~2y)
    pk = np.min(us['y'][:16])
    k = target_us_y / pk
    us = {v: k * x for v, x in us.items()}
    rows = []
    for b in ASSIGN:
        sol, sUS = bloc_response(b, us, eps_M, extra_theta)
        for v, name in [('y', 'output_gap_pct'), ('pi', 'inflation_pp'), ('i', 'policy_rate_pp'), ('z', 'depreciation_vs_usd_pct')]:
            rows.append(dict(bloc=b, var=name, q4=sol[v][3], q8=sol[v][7], q12=sol[v][11], peak=float(sol[v][np.argmax(np.abs(sol[v][:24]))]), s_us=sUS))
    df = pd.DataFrame(rows)
    return us, df


if __name__ == '__main__':
    for eps in [1.0, 2.0]:
        us, df = main(eps_M=eps)
        print('eps_M', eps, ' US y@4,8,12:', np.round([us['y'][3], us['y'][7], us['y'][11]], 3), ' US i@0:', round(us['i'][0], 2), 'us pi@4,8,12', np.round([us['pi'][3], us['pi'][7], us['pi'][11]], 3))
        print(df.pivot(index='bloc', columns='var', values=['q4', 'q8', 'q12']).round(3).to_string())
