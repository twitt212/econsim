"""Prototype of the recommended monthly world oil market (section 8 of the oil research report, not published). Log-linear around the 2024 steady state, flows in % of baseline
world liquids consumption Q (103.1 mb/d), the price p in % log deviation of the real Brent price from its anchor. A test bench for the lead
modeller (parameter values, IRFs, moments), not production code. Pure numpy, no external data.

Timing inside month t. p_t is the end-of-month price; the reported/used price is the monthly average
    pbar_t = W*p_t + (1-W)*p_{t-1},  W = 0.789,
the time average of a random walk (Working 1960): it gives ac1(d pbar) = 0.25 and var(d pbar) = (2/3) var(d p), as in the FRED monthly-average data.
  1. shocks: the oil-specific (Kilian) shock eps_I, which enters twice: as a precautionary/inventory-demand flow u_I (AR(1), short lived) and, with weight
     theta_q, as a persistent shift q of oil consumption demand (AR(1), rho_q = 0.99; the 'oil consumption demand' shock of Baumeister & Hamilton). Its fat tails,
     GARCH and jumps apply to eps_I. Also: supply disruption s_D (Poisson jumps, AR(1) decay), global activity y (AR(1)), OPEC+ policy surprise e_O, news nu
     (Kanzig-type announcement: expectation shock + phased-in future OPEC+ cut);
  2. predetermined supply: OPEC+ target tau (integral rule on the 3-month average price, dead band, capacity cap), realised OPEC+ output o,
     US shale u2 and rest of non-OPEC r (distributed lags of past prices);
  3. market clearing for p_t   S_t - D_t = dI_t   (closed form, linear in p_t)
        S_t  = o + sh_us_resp*u2 + sh_rest*r - s_D + eps_S * pbar_t
        D_t  = beta_y * ylag_t + q_t - z_t - eta0 * pbar_t,    z_t = rho_z z_{t-1} + (1-rho_z) eta1 pbar_t
        dI_t = -psi (1-rho_E) p_t - [xi*max(i,0) + xi_d*min(i,0)]_{t-1} + psi * lam_n * n_t + u_I,t,        E_t p_{t+1} = rho_E p_t + lam_n n_t
        i_t  = i_{t-1} + dI_t
"""
import sys, os
from dataclasses import dataclass, replace
import numpy as np
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import paths

W_AVG = 0.789


@dataclass
class Params:
    w: float = W_AVG
    # --- demand (aggregate of the 13 blocs)
    eta0: float = 0.1        # within-month elasticity of world demand to the crude price (absolute value)
    eta1: float = 0.2001        # additional slow elasticity (long run total eta0 + eta1)
    rho_z: float = 0.975      # monthly persistence of the slow demand response (half-life 27 months)
    beta_y: float = 0.85      # cyclical income elasticity
    rho_ylag: float = 0.6328    # oil demand follows activity with a lag
    # --- supply
    eps_S: float = 0.025       # fast supply elasticity (within month): spare-capacity use and quick responders
    sh_us_resp: float = 0.128  # price-responsive US crude output as share of Q (13.2/103.1)
    sh_rest: float = 0.28      # price-responsive rest of non-OPEC conventional supply as share of Q
    spare_cap: float = 3.4    # OPEC+ spare capacity, % of Q (EIA STEO 2024: 3.56 mb/d surplus capacity)
    theta_O: float = 0.35     # monthly speed at which realised OPEC+ output follows the target
    pi_L: float = -12.0       # OPEC+ price floor, % below anchor (3-month average)
    pi_H: float = 25.0        # ceiling, % above anchor
    kap_L: float = 0.02      # % of Q cut per month per 1 % below floor
    kap_H: float = 0.008      # % of Q added per month per 1 % above ceiling
    tau_floor: float = -12.0  # deepest cumulative cut, % of Q
    stock_kappa: float = 0.0  # response of the target to the stock gap
    tau_decay: float = 0.02   # monthly erosion of past cuts/increases (compliance erosion, unwinding) while the price is above the floor
    eps_US_LR: float = 0.45   # long-run elasticity of US responsive crude output
    rho_U1: float = 0.92
    rho_U2: float = 0.92
    eps_R_LR: float = 0.15
    rho_R: float = 0.96
    # --- inventories and price formation
    psi: float = 1.8507
    rho_E: float = 0.9
    xi: float = 0.15         # stock-level feedback when stocks are above normal (builds)
    xi_d: float = 0.08        # stock-level feedback when stocks are below normal (draws): scarcity premium
    lam_n: float = 0.4882
    # --- oil-specific / precautionary shock
    rho_I: float = 0.95      # persistence of the precautionary/inventory-demand flow u_I
    rho_q: float = 0.99       # persistence of the oil consumption-demand shift q
    theta_q: float = 0.8405      # size of the q innovation per unit of the u_I innovation (both % of Q)
    sig_I: float = 1.774        # % of Q per month
    nu_I: float = 6.0
    garch_a: float = 0.1
    garch_b: float = 0.8
    jump_lam_I: float = 0.013
    jump_mu_I: float = -2.3   # sd units
    jump_sd_I: float = 3.3972
    # --- supply disruptions and OPEC+ policy surprises
    rho_D: float = 0.9794
    sig_O: float = 0.55
    nu_O: float = 5.0
    jump_lam_D: float = 0.045
    jump_mean_D: float = 2.1
    # --- activity
    rho_y: float = 0.9943
    sig_y: float = 1.6657
    # --- news
    news_cut_share: float = 0.3915
    news_phase: float = 0.9427
    rho_n: float = 0.99


class State:
    __slots__ = ('p', 'pbar', 'z', 'ylag', 'y', 'tau', 'o', 'u1', 'u2', 'r', 'sD', 'uI', 'q', 'i', 'n', 'cut_pend', 'hist')

    def __init__(self):
        for k in self.__slots__:
            setattr(self, k, 0.0)
        self.hist = [0.0, 0.0, 0.0]


def step(P: Params, S: State, eps: dict, ext_supply: float = 0.0, fixed_demand: float = 0.0, supply_override=None, target_pbar=None) -> dict:
    """One month. eps: innovations 'I' (to u_I, % of Q), 'D' (new disruption, % of Q, >0 = loss), 'O' (OPEC+ policy surprise on the target,
    % of Q, >0 = more supply), 'y' (activity innovation, %), 'news' (expectation shock, % of Q). ext_supply: extra supply loss (% of Q) for
    historical-episode tests. Returns the month's flows."""
    S.uI = P.rho_I * S.uI + eps.get('I', 0.0)
    S.q = P.rho_q * S.q + P.theta_q * eps.get('I', 0.0) + eps.get('q', 0.0)
    S.sD = P.rho_D * S.sD + eps.get('D', 0.0)
    S.y = eps['y_set'] if 'y_set' in eps else P.rho_y * S.y + eps.get('y', 0.0)   # 'y_set' overrides the activity gap (used to feed a given path)
    S.ylag = P.rho_ylag * S.ylag + (1 - P.rho_ylag) * S.y
    nu = eps.get('news', 0.0)
    S.n = P.rho_n * S.n + nu
    S.cut_pend += P.news_cut_share * nu
    step_cut = (1 - P.news_phase) * S.cut_pend
    S.cut_pend -= step_cut
    ptil = sum(S.hist) / 3.0
    dtau = -P.kap_L * max(0.0, P.pi_L - ptil) + P.kap_H * max(0.0, ptil - P.pi_H) - P.stock_kappa * S.i + eps.get('O', 0.0) - step_cut
    S.tau = float(np.clip(S.tau * (1.0 - (P.tau_decay if ptil >= P.pi_L else 0.0)) + dtau, P.tau_floor, P.spare_cap))
    S.o = min(S.o + P.theta_O * (S.tau - S.o), P.spare_cap)
    pl = S.hist[0]
    S.u1 = P.rho_U1 * S.u1 + (1 - P.rho_U1) * P.eps_US_LR * pl
    S.u2 = P.rho_U2 * S.u2 + (1 - P.rho_U2) * S.u1
    S.r = P.rho_R * S.r + (1 - P.rho_R) * P.eps_R_LR * pl
    s_pre = S.o + P.sh_us_resp * S.u2 + P.sh_rest * S.r - S.sD - ext_supply
    eps_s = P.eps_S
    if supply_override is not None:     # historical-episode test: total supply (% of Q, deviation from baseline) is given
        s_pre = supply_override
        eps_s = 0.0
    k = eps_s + P.eta0 + (1 - P.rho_z) * P.eta1
    a = s_pre - P.beta_y * S.ylag - S.q + P.rho_z * S.z - fixed_demand
    psiE = P.psi * (1 - P.rho_E)
    pm1 = S.p
    stock_fb = P.xi * max(S.i, 0.0) + P.xi_d * min(S.i, 0.0)
    if target_pbar is not None:   # 'exact path' mode: the precautionary innovation is solved so that the monthly-average price equals the target
        p_t = (target_pbar - (1 - P.w) * pm1) / P.w
        S.uI = p_t * (k * P.w + psiE) + stock_fb + a + k * (1 - P.w) * pm1 - P.psi * P.lam_n * S.n
    rhs = S.uI + P.psi * P.lam_n * S.n - stock_fb - a - k * (1 - P.w) * pm1
    S.p = rhs / (k * P.w + psiE)
    S.pbar = P.w * S.p + (1 - P.w) * pm1
    dI = -psiE * S.p - stock_fb + P.psi * P.lam_n * S.n + S.uI
    S.i += dI
    S.z = P.rho_z * S.z + (1 - P.rho_z) * P.eta1 * S.pbar
    S.hist = [S.pbar, S.hist[0], S.hist[1]]
    supply = s_pre + eps_s * S.pbar
    demand = P.beta_y * S.ylag + S.q - S.z - P.eta0 * S.pbar
    return dict(p=S.p, pbar=S.pbar, supply=supply, demand=demand, dI=dI, i=S.i, o=S.o, us=S.u2, sD=S.sD, y=S.y, q=S.q)


def irf(P: Params, shock: str, size: float, H: int = 61) -> dict:
    """Deterministic impulse response (differences from the zero-shock path, which is the steady state)."""
    S = State(); out = {k: [] for k in ('p', 'pbar', 'supply', 'demand', 'dI', 'i', 'o', 'us', 'sD', 'y', 'q')}
    for t in range(H):
        e = {shock: size} if t == 0 else {}
        r = step(P, S, e)
        for k in out: out[k].append(r[k])
    return {k: np.array(v) for k, v in out.items()}


# ------------------------------------------------------------------------------------------------------------------------------
# stochastic simulation with fat tails, GARCH and jumps
def _t_unit(rng, nu, size=None):
    if nu is None or nu <= 0:
        return rng.standard_normal(size)
    return rng.standard_t(nu, size=size) * np.sqrt((nu - 2) / nu)


def simulate(P: Params, T: int, rng: np.random.Generator, shocks=('I', 'D', 'O', 'y'), burn: int = 120, hist_ext=None):
    """Simulate T months after a burn-in. Returns dict of arrays including the monthly-average price pbar (% log deviation)."""
    S = State(); n = T + burn; out = {k: np.empty(n) for k in ('p', 'pbar', 'dI', 'i', 'o', 'supply', 'demand')}
    h = 1.0   # GARCH conditional variance multiplier
    last_eI = 0.0
    for t in range(n):
        e = {}
        if 'I' in shocks:
            h = (1 - P.garch_a - P.garch_b) + P.garch_a * last_eI ** 2 + P.garch_b * h
            h = min(max(h, 0.04), 25.0)
            z = _t_unit(rng, P.nu_I)
            base = z * np.sqrt(h)
            nj = rng.poisson(P.jump_lam_I)
            jump = rng.normal(P.jump_mu_I * nj, P.jump_sd_I * np.sqrt(nj)) if nj else 0.0
            var_j = P.jump_lam_I * (P.jump_mu_I ** 2 + P.jump_sd_I ** 2)
            ei = np.sqrt(max(1 - var_j, 0.05)) * base + jump - P.jump_lam_I * P.jump_mu_I
            last_eI = base
            e['I'] = P.sig_I * ei
        if 'D' in shocks:
            nd = rng.poisson(P.jump_lam_D)
            e['D'] = (rng.exponential(P.jump_mean_D, size=nd).sum() if nd else 0.0) - P.jump_lam_D * P.jump_mean_D   # compensated (mean zero)
        if 'O' in shocks:
            e['O'] = P.sig_O * _t_unit(rng, P.nu_O)
        if 'y' in shocks:
            e['y'] = P.sig_y * rng.standard_normal()
        r = step(P, S, e)
        for k in out: out[k][t] = r[k]
    return {k: v[burn:] for k, v in out.items()}


def moments(pbar: np.ndarray) -> dict:
    """Moments of the monthly-average log price (pbar in %) comparable with the FRED real WTI statistics of 1986-2024."""
    d = np.diff(pbar)
    z = (d - d.mean()) / d.std()
    sk = float((z ** 3).mean()); ku = float((z ** 4).mean() - 3)
    ac1 = float(np.corrcoef(d[1:], d[:-1])[0, 1])
    d12 = pbar[12:] - pbar[:-12]
    vr12 = float(d12.var() / (12 * d.var()))
    ar1 = float(np.corrcoef(pbar[1:], pbar[:-1])[0, 1])
    return dict(sd=float(d.std()), skew=sk, exkurt=ku, ac1=ac1, vr12=vr12, ar1_level=ar1, sd_level=float(pbar.std()) / 100,
                share_big12=float((np.abs(d12) > 50 * 1.0).mean()), share_gt3sd=float((np.abs(z) > 3).mean()))


# ------------------------------------------------------------------------------------------------------------------------------
# Stage 3 golden (spec 1.1, 2.4, 3 Stage 3): `python3 -B -I oil_reference.py --golden [--check]` writes (or checks) the golden
# vectors of the Rust tests: the unit IRFs of the four shocks and of news (61 months), the research's K7 scenarios and the 2026
# STEO replay (K6). The functions above are the research prototype, unchanged (cmp against research/oil/scripts).
def _golden_scenarios(P):
    import numpy as np
    out = {}
    Q = 103.1
    def run(T, shocks_at=None, target=None, ext=None):
        S = State(); rows = []
        for tt in range(T):
            e = shocks_at.get(tt, {}) if shocks_at else {}
            rr = step(P, S, e, target_pbar=(target[tt] if target is not None else None), ext_supply=(ext(tt) if ext else 0.0))
            rows.append(rr)
        return rows
    tgt = 100 * np.log(1.5) * 0.5 ** (np.arange(48) / 12.0)
    a = run(48, target=tgt)
    out['oil50_exact_path'] = {str(h): dict(price=float(a[h]['pbar'])) for h in [0, 3, 6, 12, 24, 36]}
    c = run(60, {0: {'O': -2.0 / Q * 100}})
    out['opec_cut_2mbd'] = {str(h): dict(price=float(c[h]['pbar']), o=float(c[h]['o'])) for h in [0, 3, 6, 12, 24, 36]}
    dsh = {tt: {'y': -0.7, 'I': (-2.2 * P.sig_I if tt < 2 else 0.0)} for tt in range(6)}
    d = run(48, dsh)
    out['collapse_2008_type'] = {'pbar': [float(r['pbar']) for r in d]}
    path = lambda t: 12.0 if t < 3 else max(0.0, 12.0 * (1 - (t - 2) / 6.0))
    hz = run(36, ext=path)
    out['hormuz'] = {'pbar': [float(r['pbar']) for r in hz]}
    sp = run(36, ext=lambda t: -1.0 / Q * 100 if t < 6 else 0.0)
    out['spr_release_1mbd_6m'] = {'pbar': [float(r['pbar']) for r in sp]}
    return out


def _golden_irf(P):
    import numpy as np
    res = {}
    for name, shock, size in [('I', 'I', 1.0), ('D', 'D', 1.25), ('O', 'O', -1.0), ('y', 'y', 1.0)]:
        S = State(); rows = []
        for t in range(61):
            rows.append(step(P, S, {shock: size} if t == 0 else {}))
        res[name] = {k: [float(r[k]) for r in rows] for k in ('p', 'pbar', 'supply', 'demand', 'dI', 'i', 'o', 'us', 'sD', 'y', 'q')}
    # news: normalised to +10 % on impact (K3)
    r0 = irf(P, 'news', 1.0)
    sn = 10.0 / r0['pbar'][0]
    S = State(); rows = []
    for t in range(61):
        rows.append(step(P, S, {'news': sn} if t == 0 else {}))
    res['news'] = {k: [float(r[k]) for r in rows] for k in ('p', 'pbar', 'supply', 'demand', 'dI', 'i', 'o', 'us', 'sD', 'y', 'q')}
    res['news']['size'] = float(sn)
    return res


def _golden_k6(P):
    """2026 Gulf replay (K6): STEO world production path relative to the Dec-Feb mean, no precautionary shock (research check K6)."""
    import numpy as np, csv
    Q = 103.1
    path = os.path.join(paths.OIL_DATA_DIR, 'steo_monthly_tidy.csv')
    rows = list(csv.DictReader(open(path)))
    def g(tab, code):
        d = {}
        for r in rows:
            if r['table'] == tab and r['code'] == code:
                d.setdefault(r['month'], float(r['value']))
        return d
    prod, cons, brent = g('3atab', 'papr_world'), g('3atab', 'patc_world'), g('2tab', 'BREPUUS')
    base_p = np.mean([prod[m] for m in ['2025-12', '2026-01', '2026-02']])
    base_c = np.mean([cons[m] for m in ['2025-12', '2026-01', '2026-02']])
    b0 = np.mean([brent[m] for m in ['2026-01', '2026-02']])
    months = [f'2026-{m:02d}' for m in range(3, 10)]
    S = State(); out = []
    for m in months:
        sup = (prod[m] - base_p) / Q * 100
        rr = step(P, S, {}, supply_override=sup)
        out.append(dict(month=m, supply_override=float(sup), pbar=float(rr['pbar']), brent_model=float(b0 * np.exp(rr['pbar'] / 100)),
                        brent_obs=float(brent[m]), err_logpts=float(rr['pbar'] - 100 * np.log(brent[m] / b0))))
    e = np.array([o['err_logpts'] for o in out[:6]])
    return dict(baseline_brent=float(b0), rmse_logpts_mar_aug=float(np.sqrt(np.mean(e ** 2))), rows=out)


def _golden_bands():
    """K2/K3 bands and centres of the research check table (`data/world/research/oil/data/oil_model_irf_check.csv`)."""
    import csv
    out = {}
    for r in csv.DictReader(open(paths.oil_csv('oil_model_irf_check.csv'))):
        out.setdefault(r['check'], []).append(dict(h=int(r['h']), model=float(r['model']), center=float(r['center']),
                                                   lo=float(r['lo']), hi=float(r['hi'])))
    return out


def _golden_balance():
    """2024 production minus consumption over the thirteen blocs, mb/d (research K1: 0.023)."""
    import json
    p = json.load(open(paths.research_json('oil')))
    b = p['blocs']
    prod = sum(float(b[k]['liquids_production_2024_mbd']['value']) for k in b)
    cons = sum(float(b[k]['consumption_2024_mbd']['value']) for k in b)
    return dict(production_mbd=prod, consumption_mbd=cons, gap_mbd=prod - cons)


def _golden_doc():
    P = Params()
    return dict(
        meta=dict(description='Stage 3 oil golden: unit IRFs (61 months), the K7 scenarios, the K6 replay and the K2/K3 bands of the research prototype',
                  params='Params() defaults of oil_reference.py (the research oil params.json values)', q_mbd=103.1, months=61),
        irf=_golden_irf(P), scenarios=_golden_scenarios(P), k6=_golden_k6(P), bands=_golden_bands(), balance_2024=_golden_balance())


if __name__ == '__main__' and ('--golden' in sys.argv or '--check' in sys.argv):
    import json
    doc = _golden_doc()
    target = paths.golden('oil_golden')
    if '--check' in sys.argv:
        committed = json.load(open(target))
        def cmp(a, b, path=''):
            if isinstance(a, dict):
                assert a.keys() == b.keys(), path
                return max([cmp(a[k], b[k], path + '/' + k) for k in a] + [0.0])
            if isinstance(a, list):
                assert len(a) == len(b), path
                return max([cmp(x, y, path) for x, y in zip(a, b)] + [0.0])
            if isinstance(a, float):
                return abs(a - b)
            assert a == b, path
            return 0.0
        dmax = cmp(json.loads(json.dumps(doc)), committed)
        print('oil_reference --check OK: max abs difference %.3e' % dmax)
    else:
        with open(target, 'w') as f:
            json.dump(doc, f, indent=1)
            f.write('\n')
        print('wrote', target)
