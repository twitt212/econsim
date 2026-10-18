"""Single-bloc GPM-style gap model (closed block + UIP vs exogenous US/foreign drivers).

Quarterly form follows Carabenciov et al. (2013), IMF WP/13/87, eqs (9),(20),(21),(23)-(32), in deviations from
steady state.  The same builder produces a MONTHLY version (k=3 periods per quarter) from coefficients converted by
convert_to_monthly().  Everything is linear, solved by stacked perfect foresight (lin_pf.LinModel).

Variable conventions (deviation from steady state; rates are annualised percent):
  y  output gap (%)                       pi  annualised inflation of the period (pp)
  i  nominal policy rate (pp)             z   real exchange rate gap vs USD (%; + = depreciation of bloc currency)
  rr ex-ante real rate gap (pp)           mrr medium-term real rate gap (pp)
  u  unemployment gap (pp, actual minus NAIRU; Okun)
Exogenous drivers entering through the right-hand sides: F (foreign-activity input) and rrus (US real rate gap).
"""
import numpy as np
from math import sqrt
from lin_pf import LinModel

XI = dict(x1=0.10, x4=0.35, x12=0.35, x20=0.20)     # GPM6 Table 1 medium-term-rate weights (all regions)

# ---------------------------------------------------------------- GPM6 Table 1 (quarterly) -----------------------
# keys: b1 lag, b2 lead, b3 medium-term real rate gap, b4 REER gap, b5 foreign activity, th financial (BLT) spillover,
#       l1 inflation lead weight, l2 output gap, l3 REER-gap change, g1 smoothing, g2 inflation gap, g4 output gap,
#       phi UIP forward weight, a1/a2 Okun lag/gap (G3 only), g_ss potential growth, rr_ss neutral real rate,
#       pi_tar inflation target, tau potential-growth adjustment, rho neutral-rate adjustment.
GPM6 = {
 'US':  dict(b1=.569, b2=.231, b3=.187, b4=.051, b5=.891, th=1.071, l1=.750, l2=.180, l3=.100, g1=.711, g2=.910, g4=.205,
             phi=None, a1=.824, a2=.182, g_ss=2.273, rr_ss=1.728, pi_tar=2.5, tau=.027, rho=.290),
 'EA':  dict(b1=.756, b2=.044, b3=.201, b4=.067, b5=.891, th=.300, l1=.700, l2=.222, l3=.246, g1=.686, g2=1.306, g4=.201,
             phi=.834, a1=.717, a2=.140, g_ss=2.261, rr_ss=1.984, pi_tar=1.9, tau=.029, rho=.467),
 'JP':  dict(b1=.779, b2=.021, b3=.148, b4=.036, b5=.891, th=.300, l1=.750, l2=.184, l3=.152, g1=.750, g2=1.058, g4=.169,
             phi=.856, a1=.759, a2=.060, g_ss=1.444, rr_ss=1.379, pi_tar=1.0, tau=.037, rho=.030),
 'EA6': dict(b1=.471, b2=.215, b3=.200, b4=.171, b5=.891, th=.996, l1=.720, l2=.197, l3=.081, g1=.667, g2=1.114, g4=.169,
             phi=.800, a1=None, a2=None, g_ss=7.900, rr_ss=2.000, pi_tar=4.0, tau=.030, rho=.200),
 'LA6': dict(b1=.544, b2=.178, b3=.200, b4=.148, b5=.891, th=.992, l1=.594, l2=.228, l3=.161, g1=.645, g2=.911, g4=.202,
             phi=.800, a1=None, a2=None, g_ss=4.000, rr_ss=2.000, pi_tar=3.5, tau=.030, rho=.200),
 'RC6': dict(b1=.441, b2=.408, b3=.200, b4=.070, b5=.891, th=.996, l1=.545, l2=.149, l3=.098, g1=.725, g2=.898, g4=.162,
             phi=.800, a1=None, a2=None, g_ss=4.000, rr_ss=2.000, pi_tar=4.5, tau=.030, rho=.200),
}


def stable_unstable_roots(b1, b2):
    """roots of b2*mu^2 - mu + b1 = 0 for y_t = b1 y_{t-1} + b2 y_{t+1} + forcing (b2>=0)."""
    if b2 <= 1e-12:
        return b1, np.inf
    s = sqrt(1 - 4 * b1 * b2)
    return (1 - s) / (2 * b2), (1 + s) / (2 * b2)


def convert_to_monthly(q, k=3):
    """Quarterly GPM coefficients -> monthly (k periods per quarter).  See section 4 of the blocs research report (not published) for the rules."""
    m = dict(q)
    mu_s, mu_u = stable_unstable_roots(q['b1'], q['b2'])
    mus_m = mu_s ** (1.0 / k)
    if np.isinf(mu_u):
        b2m, b1m = 0.0, mus_m
    else:
        muu_m = mu_u ** (1.0 / k)
        b2m = 1.0 / (mus_m + muu_m)
        b1m = b2m * mus_m * muu_m
    scale = (1 - b1m - b2m) / (1 - q['b1'] - q['b2'])       # preserves long-run multiplier of any persistent forcing
    m.update(b1=b1m, b2=b2m, b3=q['b3'] * scale, b4=q['b4'] * scale, b5=q['b5'] * scale)
    if 'theta' in q:
        m['theta'] = q['theta'] * scale          # global financial channel is an IS forcing term
    m['forcing_scale'] = scale
    m['g1'] = q['g1'] ** (1.0 / k)                              # policy-rate smoothing = partial-adjustment gain
    if q.get('a1') is not None:
        m['a1'] = q['a1'] ** (1.0 / k)
        m['a2'] = q['a2'] * (1 - m['a1']) / (1 - q['a1'])
    return m


def build(p, k=1, with_okun=False, rule=True, uip='classic', filt=None):
    """LinModel for one bloc.  p: coefficients in the units of the chosen frequency.
    k = periods per quarter (1 quarterly, 3 monthly).  rule=False makes the policy rate exogenous (path via rhs 'rule')."""
    V = ['y', 'pi', 'i', 'z', 'rr', 'rr12', 'mrr', 'ze']
    if with_okun:
        V.append('u')
    M = LinModel(V)
    n4 = 4 * k
    # real rate: i minus next-quarter inflation window (periods t+1..t+k)
    M.add('rr', [(1, 'rr', 0), (-1, 'i', 0)] + [(1.0 / k, 'pi', j) for j in range(1, k + 1)])
    # 4-quarter forward average real rate gap
    M.add('rr12', [(1, 'rr12', 0)] + [(-1.0 / n4, 'rr', j) for j in range(0, n4)])
    # medium-term real rate gap (GPM6 eq. 27, in gap form)
    x = XI
    t = [(1, 'mrr', 0), (-x['x1'], 'rr', 0), (-x['x4'], 'rr12', 0)]
    t += [(-x['x12'] / 3.0, 'rr12', n4 * j) for j in range(0, 3)]
    t += [(-x['x20'] / 5.0, 'rr12', n4 * j) for j in range(0, 5)]
    M.add('mrr', t)
    # IS curve.  z enters as a moving average of 4 quarters of lags; monthly: contiguous window, same mean lag (7.5m)
    zlags = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    t = [(1, 'y', 0), (-p['b1'], 'y', -1), (-p['b2'], 'y', 1), (p['b3'], 'mrr', -k)]
    if filt and k == 3:
        # n-stage geometric (Erlang) filter chain with total mean lag 7.5 months (2 months pure delay + n stages)
        nst = int(filt)
        lam = (5.5 / nst) / (1 + 5.5 / nst)
        for v in range(1, nst + 1):
            M.vars.append(f'zf{v}'); M.ix[f'zf{v}'] = len(M.vars) - 1
        M.add('zf1', [(1, 'zf1', 0), (-lam, 'zf1', -1), (-(1 - lam), 'z', -2)])
        for v in range(2, nst + 1):
            M.add(f'zf{v}', [(1, f'zf{v}', 0), (-lam, f'zf{v}', -1), (-(1 - lam), f'zf{v-1}', 0)])
        t += [(-p['b4'], f'zf{nst}', 0)]
    else:
        t += [(-p['b4'] / len(zlags), 'z', -l) for l in zlags]
    M.add('IS', t)          # rhs = b5*F_{t-k} + eps_y
    # Phillips curve (GPM6 eq. 20): lead / lag y/y windows with the same mean lead / lag as the quarterly version
    lead = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    lag = list(range(1, 5)) if k == 1 else list(range(2, 2 + n4))
    t = [(1, 'pi', 0)]
    t += [(-p['l1'] / len(lead), 'pi', l) for l in lead]
    if filt and k == 3:
        nst = int(filt)
        lam = (5.5 / nst) / (1 + 5.5 / nst)
        for v in range(1, nst + 1):
            M.vars.append(f'pf{v}'); M.ix[f'pf{v}'] = len(M.vars) - 1
        M.add('pf1', [(1, 'pf1', 0), (-lam, 'pf1', -1), (-(1 - lam), 'pi', -2)])
        for v in range(2, nst + 1):
            M.add(f'pf{v}', [(1, f'pf{v}', 0), (-lam, f'pf{v}', -1), (-(1 - lam), f'pf{v-1}', 0)])
        t += [(-(1 - p['l1']), f'pf{nst}', 0)]
    else:
        t += [(-(1 - p['l1']) / len(lag), 'pi', -l) for l in lag]
    t += [(-p['l2'], 'y', -k)]
    t += [(-p['l3'] / 4.0, 'z', 0), (p['l3'] / 4.0, 'z', -n4)]
    M.add('PC', t)          # rhs = eps_pi
    # policy rule (GPM6 eq. 21 in deviation form)
    if rule:
        win = list(range(0, 4)) if k == 1 else list(range(-1, 11))
        t = [(1, 'i', 0), (-p['g1'], 'i', -1)]
        t += [(-(1 - p['g1']) * (1 + p['g2']) / len(win), 'pi', l) for l in win]
        t += [(-(1 - p['g1']) * p['g4'], 'y', 0)]
        M.add('rule', t)    # rhs = eps_i
    else:
        M.add('rule', [(1, 'i', 0)])
    # UIP with partially backward expectations (GPM6 eqs. 30-31); ppy = periods per year
    ppy = 4 * k
    if uip == 'classic':
        M.add('ze', [(1, 'ze', 0), (-p['phi'], 'z', 1), (-(1 - p['phi']), 'z', -1)])
        M.add('UIP', [(1, 'rr', 0), (-ppy, 'ze', 0), (ppy, 'z', 0)])     # rr - rrus = ppy*(z^e_{t+1}-z_t); rrus on rhs
    else:
        # closed form of the same UIP (exact in the quarterly model): z_t = rho_z z_{t-1} - kappa * S_t,
        # S_t = sum_{j>=t} d_j (d = rr - rrus - eps_rp).  rho_z=(1-phi)/phi, kappa=(1-rho_z)/(ppy*(2phi-1)) (k=1: 1/(4 phi)).
        phi = p['phi']
        rz = ((1 - phi) / phi) ** (1.0 / k)
        kap = p.get('uip_w', 1.0) * (1 - rz) / (ppy * (2 * phi - 1))
        M.add('ze', [(1, 'ze', 0), (1.0, 'z', -1)])     # placeholder equation: ze := -z_{t-1} (unused), keeps square system
        M.add('UIP', [(1, 'z', 0), (-rz, 'z', -1), (kap, 'ze', 0)])    # z_t - rz z_{t-1} + kap*S_t = 0 ; S stored in 'ze' variable
        # replace placeholder: define S in ze via S_t - S_{t+1} = rr_t - rrus - eps  -> rewrite equations below
        M.eqs = [e for e in M.eqs if e[0] != 'ze']
        M.add('ze', [(1, 'ze', 0), (-1, 'ze', 1), (-1, 'rr', 0)])      # S_t - S_{t+1} - rr_t = -(rrus+eps)  (rhs)
    if with_okun:
        M.add('okun', [(1, 'u', 0), (-p['a1'], 'u', -1), (p['a2'], 'y', 0)])
    return M
