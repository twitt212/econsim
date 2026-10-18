"""Reference construction of the rational-expectations state-space of one monthly bloc (the object a Rust implementation would load).

State s_t = lagged y (3), i (1), z (13), pi (13), mrr (3)  = 33 numbers (exact 12-month moving windows).  Exogenous drivers (US variables,
foreign gaps, oil, G_t) enter through the right-hand sides; here they are zero (autonomous bloc).
Procedure: for each basis vector of the state, solve the perfect-foresight BVP (stable manifold) with zero shocks; the period-0 values give
x_0 = C s_0 and the new state s_1 = A s_0.  Shock columns: response at t=0 to a unit shock in equation e with s_0=0.
Returns A (33x33), C (8x33), B_e; verifies against the stacked IRF; reports eigenvalues."""
import sys, os
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bloc_model import GPM6, convert_to_monthly, build

STATE = [('y', 3), ('i', 1), ('z', 13), ('pi', 13), ('mrr', 3)]
VARS = ['y', 'pi', 'i', 'z', 'rr', 'rr12', 'mrr', 'ze']


def statespace(q, T=360):
    m = convert_to_monthly(q)
    M = build(m, k=3, uip='closed')
    idx = []
    for v, n in STATE:
        for lag in range(1, n + 1):
            idx.append((v, lag))
    N = len(idx)
    A = np.zeros((N, N)); C = np.zeros((len(VARS), N))
    for k, (v0, l0) in enumerate(idx):
        pre = {v: np.zeros(n) for v, n in STATE}
        pre[v0][len(pre[v0]) - l0] = 1.0           # x[-l0] = 1
        sol = M.solve(T, {}, pre=pre)
        x0 = np.array([sol[v][0] for v in VARS])
        C[:, k] = x0
        # new state: x_0 enters as lag 1, old lag j -> j+1
        s1 = np.zeros(N)
        for r, (v, lag) in enumerate(idx):
            if lag == 1:
                s1[r] = sol[v][0]
            else:
                s1[r] = pre[v][len(pre[v]) - (lag - 1)]
        A[:, k] = s1
    return A, C, idx, m


if __name__ == '__main__':
    q = GPM6['EA']
    A, C, idx, m = statespace(q, T=300)
    ev = np.linalg.eigvals(A)
    mod = np.sort(np.abs(ev))[::-1]
    print('state dimension', A.shape[0], ' spectral radius %.4f' % mod[0], ' top moduli', np.round(mod[:8], 4), ' #|ev|>0.01:', int((mod > 0.01).sum()))
    print('max |ev| < 1:', mod[0] < 1)
    np.save(os.path.join(__import__('paths').CSV_DIR, 'ea_monthly_A_matrix.npy'), A)


def verify(q, T=300, months=36):
    A, C, idx, m = statespace(q, T)
    M = build(m, k=3, uip='closed')
    e = np.zeros(T); e[0] = 1.0
    sol = M.solve(T, {'rule': e})
    x0 = np.array([sol[v][0] for v in VARS])
    s = np.zeros(A.shape[0])
    for r, (v, lag) in enumerate(idx):
        if lag == 1:
            s[r] = sol[v][0]
    ys = [sol['y'][0]]
    xs = [x0]
    for t in range(1, months):
        x = C @ s
        ys.append(x[0])
        s = A @ s
        # new lag-1 entries must be set from x_t: A already maps state->next state using the policy function
    err = np.max(np.abs(np.array(ys) - sol['y'][:months]))
    return err


def table():
    import csv
    from bloc_params import quarterly, BLOCS12
    rows = []
    for b in BLOCS12:
        q = dict(quarterly(b)); q.setdefault('phi', 0.8)
        A, C, idx, m = statespace(q, T=300)
        ev = np.linalg.eigvals(A)
        order = np.argsort(-np.abs(ev))
        lead = ev[order[0]]
        rho = abs(lead)
        per = 2 * np.pi / abs(np.angle(lead)) if abs(np.angle(lead)) > 1e-6 else float('inf')
        rows.append(dict(bloc=b, n_state=A.shape[0], spectral_radius_monthly=round(float(rho), 4), dominant_period_months=round(float(per), 1) if np.isfinite(per) else 'real',
                         half_life_months=round(float(np.log(0.5) / np.log(rho)), 1), n_eig_above_0p5=int((np.abs(ev) > 0.5).sum()), stable=bool(rho < 1)))
    out = os.path.join(__import__('paths').CSV_DIR, 'state_space_eigs.csv')
    with open(out, 'w', newline='') as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys())); w.writeheader(); w.writerows(rows)
    for r in rows: print(r)


if __name__ == '__main__' and len(sys.argv) > 1 and sys.argv[1] == 'table':
    print('EA reproduction error vs stacked solve (months 0..35, y):', verify(GPM6['EA']))
    table()
