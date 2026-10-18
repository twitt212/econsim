//! One bloc, one month (spec 2.2): forcing paths over the K = 240 horizon, the kernel sum with the tail term, the state
//! shift and the histories. This is the reference `run()` of `scripts/world/world_ref.py` restricted to one bloc.
//!
//! Timing convention (matches the reference): `t` is the bloc's own month counter, `hist.ius[m]` is the realised US
//! deviation of month t-1-m (ring pushed at the end of each month), the bloc histories `hist.{y,pi,i,z,mrr}[m]` are the
//! outputs of month t-1-m, and `UsHist` holds the realised US inputs of earlier months.

use super::net::{Net, UsInputs};
use super::params::{BlocSolution, Par, WorldParams, PIN_LENGTHS};
use super::{K, NS};
use crate::fx::bounded_rate_differential;

/// Output order of the solution: y, pi, i, z, mrr. Equation order of the kernels: IS, PC, rule, UIP (solver key S).
pub const OUT_Y: usize = 0;
pub const OUT_PI: usize = 1;
pub const OUT_I: usize = 2;
pub const OUT_Z: usize = 3;
pub const OUT_MRR: usize = 4;

/// Histories of one bloc, index m = month t-1-m (zero before creation). Pushed at the end of each month.
#[derive(Clone, Debug, Default)]
pub struct BlocHist {
    pub y: [f64; 14],
    pub pi: [f64; 14],
    pub i: [f64; 14],
    pub z: [f64; 14],
    pub mrr: [f64; 14],
}

fn push14(h: &mut [f64; 14], v: f64) {
    for m in (1..14).rev() {
        h[m] = h[m - 1];
    }
    h[0] = v;
}

/// The state of one bloc (spec 1.2). `s` is the 33-state vector in the order of the solution file.
#[derive(Clone, Debug)]
pub struct BlocState {
    pub s: [f64; NS],
    pub hist: BlocHist,
    /// Global financial-conditions state G_t (spec 2.2.2).
    pub g: f64,
    /// Risk-premium state u_t.
    pub u_rp: f64,
    /// Okun unemployment gap state (reporting only).
    pub u_okun: f64,
    /// ln P_{j,t} accumulated from creation (spec 2.2.5), natural log.
    pub ln_p: f64,
}

impl Default for BlocState {
    fn default() -> Self {
        BlocState { s: [0.0; NS], hist: BlocHist::default(), g: 0.0, u_rp: 0.0, u_okun: 0.0, ln_p: 0.0 }
    }
}

/// Realised US history (spec 1.2 `UsHist`), with the conventions of 2.1 and the reference: `ius[m]` is the policy deviation
/// of month t-1-m; `y_us[m]` is y_US(t-2-m) (the value reported at step t is y_US(t-1) and enters `y_us` of the next step);
/// `u[m]` is the unemployment of month t-1-m; `a[m]` the Dollar level of month t-1-m; `oil[m]` the oil gap of month t-1-m.
#[derive(Clone, Debug, Default)]
pub struct UsHist {
    pub ius: [f64; 4],
    pub y_us: [f64; 4],
    pub u: [f64; 3],
    pub a: [f64; 14],
    pub oil: [f64; 4],
}

/// A user shock acting on the blocs, with its activation month on the bloc clock.
#[derive(Clone, Debug)]
pub struct UserShock {
    pub bloc: usize,
    pub t0: u32,
    pub kind: UserKind,
}

#[derive(Clone, Debug)]
pub enum UserKind {
    /// Rate held `coef` (decimal) x `pin[k]` above the rule for `pin.len()` months (`BlocPolicy`).
    Policy { coef: f64, pin: Vec<f64> },
    /// Unit path 0.5^(k/hl) in the IS equation, scaled by `scale` (`BlocDemand`, `GlobalDemand`).
    Demand { scale: f64, hl: f64 },
}

/// Pin innovations of a `BlocPolicy` path: the solution's `pin[L]` vector.
pub fn pin_vector(sol: &BlocSolution, months: u32) -> Vec<f64> {
    let l = months as usize;
    let j = PIN_LENGTHS.iter().position(|&x| x == l).unwrap_or_else(|| panic!("BlocPolicy months {months} not in {:?}", PIN_LENGTHS));
    sol.pin[j].clone()
}

/// The kernel sum for one bloc from state `s` and four forcing paths (spec 2.2.3 step 2).
fn solve_bloc(sol: &BlocSolution, s: &[f64; NS], f: [&[f64; K]; 4]) -> [f64; 5] {
    let mut x = [0.0; 5];
    for (o, xo) in x.iter_mut().enumerate() {
        let row = &sol.c[o];
        let mut acc = 0.0;
        for j in 0..NS {
            acc += row[j] * s[j];
        }
        *xo = acc;
    }
    for (e, fe) in f.iter().enumerate() {
        let ir = &sol.ir[e];
        for k in 0..K {
            let fk = fe[k];
            let row = &ir[k];
            for o in 0..5 {
                x[o] += row[o] * fk;
            }
        }
        let tl = &sol.tail[e];
        for o in 0..5 {
            x[o] += tl[o] * fe[K - 1];
        }
    }
    x
}

/// Shift the 33-state by one month with the new outputs x (spec 2.2.3 step 3): lag 1 <- x, lag j <- lag j-1.
fn shift_state(s: &mut [f64; NS], x: &[f64; 5]) {
    // y lags 1..3 at indices 0..2
    s[2] = s[1];
    s[1] = s[0];
    s[0] = x[OUT_Y];
    // i lag 1 at index 3
    s[3] = x[OUT_I];
    // z lags 1..13 at indices 4..16
    for idx in (5..=16).rev() {
        s[idx] = s[idx - 1];
    }
    s[4] = x[OUT_Z];
    // pi lags 1..13 at indices 17..29
    for idx in (18..=29).rev() {
        s[idx] = s[idx - 1];
    }
    s[17] = x[OUT_PI];
    // mrr lags 1..3 at indices 30..32
    s[32] = s[31];
    s[31] = s[30];
    s[30] = x[OUT_MRR];
}

/// Scale of a demand shock: the isolated bloc (zero US inputs, no network) run for max(36, ceil(4 hl)) months, at most 240,
/// under the unit path 0.5^(k/hl) starting at month 0; the scale is pct / max_t y_t (spec 1.9, 2.2.6).
pub fn demand_scale(sol: &BlocSolution, hl: f64, pct: f64) -> f64 {
    let n = ((4.0 * hl).ceil().max(36.0)).min(240.0) as usize;
    let mut s = [0.0; NS];
    let mut ymax = f64::NEG_INFINITY;
    for t in 0..n {
        let mut f = [0.0; K];
        for k in 0..K {
            f[k] = 0.5f64.powf((t + k) as f64 / hl);
        }
        let x = solve_bloc(sol, &s, [&f, &[0.0; K], &[0.0; K], &[0.0; K]]);
        // Only the IS equation carries the unit path; the other three forcings are zero (solve_bloc adds their tails too,
        // which are zero for zero paths).
        ymax = ymax.max(x[OUT_Y]);
        shift_state(&mut s, &x);
    }
    pct / ymax
}

/// The user paths of one bloc at bloc month `t`: (pin innovations path, IS demand path), both K long (spec 2.2.2).
pub fn user_paths(users: &[UserShock], bloc: usize, t: u32) -> ([f64; K], [f64; K]) {
    let mut pin = [0.0; K];
    let mut uis = [0.0; K];
    for u in users.iter().filter(|u| u.bloc == bloc && u.t0 <= t) {
        let age = (t - u.t0) as usize;
        match &u.kind {
            UserKind::Policy { coef, pin: path } => {
                for k in 0..K {
                    let s = age + k;
                    if s < path.len() {
                        pin[k] += coef * path[s];
                    }
                }
            }
            UserKind::Demand { scale, hl } => {
                for k in 0..K {
                    uis[k] += scale * 0.5f64.powf((age + k) as f64 / hl);
                }
            }
        }
    }
    (pin, uis)
}

/// The expected path of the US policy deviation as the bloc sees it (`exb`: the M-F bound applied, identity at scale 0).
pub fn bound_pp(x: f64, par: &Par) -> f64 {
    if !(par.sat > 0.0) {
        return x;
    }
    100.0 * bounded_rate_differential(x / 100.0, par.knee, par.sat)
}

/// List B (rev 9, D48): the bound on the total US-rate excess return of the bloc dollar equation, `x = ex[k] + lambda_rp gp[k]` in pp
/// a year: the b7-transmission M-F function (identity up to the knee, `knee + L tanh((x - knee)/L)` above it) with the knee and scale
/// of `world_rp_sat_knee`, `world_rp_sat_scale`. Identity when `rp_sat` is zero (link off or the key 0).
#[inline(never)]
pub fn bound_rp(x: f64, par: &Par) -> f64 {
    if !(par.rp_sat > 0.0) {
        return x;
    }
    100.0 * bounded_rate_differential(x / 100.0, par.rp_knee, par.rp_sat)
}

/// One month of bloc `i`. `net` is `None` in isolated mode (F = Peff = 0). `eps` is the month's innovations
/// [e_y, e_pi, e_i, eta] (`None` = zero). `exb` is the M-F-bounded expected path (used in the rule and UIP forcing).
/// Updates G, the risk-premium state, the Okun state, ln P, the 33-state and the histories; returns (y, pi, i, z, mrr).
#[allow(clippy::too_many_arguments)]
pub fn step_bloc(
    i: usize,
    bs: &mut BlocState,
    wp: &WorldParams,
    par: &Par,
    net: Option<&Net>,
    us: &UsInputs,
    exb: &[f64; K],
    hist: &UsHist,
    users: &[UserShock],
    t: u32,
    eps: Option<[f64; 4]>,
) -> [f64; 5] {
    let mo = &wp.blocs[i].monthly;
    let sol = &wp.sol[i];
    let rho_g = wp.financial.rho_g;
    let rho_rp = wp.financial.rho_rp;
    let ti = t as i64;

    // G_t = rho_G G_{t-1} + (1 - rho_G) ius_{t-3} (realised, unbounded); u_t = rho_rp u_{t-1} + eta_t.
    bs.g = rho_g * bs.g + (1.0 - rho_g) * hist.ius[2];
    bs.u_rp = rho_rp * bs.u_rp + eps.map_or(0.0, |e| e[3]);

    // Projection of G over the horizon: Gp[k] with the realised ius for j < t and the expected path for j >= t (j = t+k-3).
    let mut gp = [0.0; K];
    gp[0] = bs.g;
    for k in 1..K {
        let j = ti + k as i64 - 3;
        let x = if j < ti {
            let m = (ti - 1 - j) as usize;
            hist.ius[m]
        } else {
            us.ex[(j - ti) as usize]
        };
        gp[k] = rho_g * gp[k - 1] + (1.0 - rho_g) * x;
    }

    // Foreign activity F and partner terms Peff (spec 2.2.2b), zero in isolated mode.
    let mut fv = [0.0; K];
    let mut win = [0.0; K];
    let mut dp = [0.0; K];
    if let Some(net) = net {
        for k in 0..3 {
            fv[k] = net.f[i][2 - k];
        }
        for k in 3..K {
            fv[k] = net.f[i][0] * par.rho_f.powi((k - 2) as i32);
        }
        // Peff over j = t-13 .. t+K (index n = 0 .. K+13 means j = t-13+n).
        let mut pj = [0.0; K + 14];
        let addend = 100.0 * (wp.wsum[i] - 1.0);
        for (n, p) in pj.iter_mut().enumerate() {
            let zp = if n <= 12 { net.wz[i][12 - n] } else { net.wz[i][0] * par.rho_p.powi((n - 12) as i32) };
            let jrel = n as i64 - 13; // j - t
            let a = if jrel < 0 {
                hist.a[(-jrel - 1) as usize]
            } else if jrel == 0 {
                us.dollar
            } else if us.proj_on {
                // Stage 4-1 (spec 2.6): the delta part decays, the q, g and credibility addend is projected (`dollar_proj`).
                us.dollar_delta * us.dollar_decay.powi(jrel as i32) + us.dollar_proj[(jrel - 1) as usize]
            } else {
                us.dollar * us.dollar_decay.powi(jrel as i32)
            };
            *p = zp + addend * a;
        }
        for k in 0..K {
            let mut acc = 0.0;
            for n in k..k + 12 {
                acc += pj[n];
            }
            win[k] = acc;
            dp[k] = pj[k + 13] - pj[k + 1];
        }
    }

    // Oil gap channel: ov[k] = oil_j, realised for j <= t (j = t+k-3), then oil_t rho_oil^(j-t).
    let mut ov = [0.0; K];
    if par.beta_oil[i] != 0.0 {
        for k in 0..K {
            ov[k] = if k <= 2 {
                hist.oil[2 - k]
            } else if k == 3 {
                us.oil
            } else {
                us.oil * par.rho_oil.powi((k - 3) as i32)
            };
        }
    }
    // OPEC+ production channel.
    let mut qv = [0.0; K];
    if par.beta_q[i] != 0.0 {
        for k in 0..K {
            qv[k] = us.qty * par.rho_qty.powi(k as i32);
        }
    }
    let (pin_t, uis_t) = user_paths(users, i, t);

    let theta = par.theta[i];
    let beta_oil = par.beta_oil[i];
    let beta_q = par.beta_q[i];
    let b4 = mo.b4;
    let mut fis = [0.0; K];
    let mut fpc = [0.0; K];
    let mut fr = [0.0; K];
    let mut fs = [0.0; K];
    for k in 0..K {
        fis[k] = mo.b5 * fv[k] - theta * gp[k] + beta_oil * ov[k] + beta_q * qv[k] - (b4 / 12.0) * win[k] + uis_t[k];
        fpc[k] = -(mo.l3 / 4.0) * dp[k];
        fr[k] = (1.0 - mo.g1) * par.k_us[i] * exb[k] + pin_t[k];
        fs[k] = if par.rp_sat > 0.0 {
            // List B (D48): the bound acts on the total expected excess return, the rate differential and the US-rate-driven premium
            // together; `us.ex` is the raw expected path (the M-F bound of `exb` is not applied twice).
            -(bound_rp(us.ex[k] + par.lam_rp[i] * gp[k], par) + bs.u_rp * rho_rp.powi(k as i32))
        } else {
            -(exb[k] + bs.u_rp * rho_rp.powi(k as i32) + par.lam_rp[i] * gp[k])
        };
    }
    // Stage 4-1 (spec 2.7): the non-policy-risk channel of the financial factor, theta_f fex rho_fin^k. Skipped when fex is zero,
    // so the merged arithmetic is bit-identical with the dollar link off.
    if us.fex != 0.0 {
        let theta_f = theta * par.theta_f_ratio / par.vix_per_100bp;
        for k in 0..K {
            fis[k] -= theta_f * us.fex * par.fin_rho.powi(k as i32);
        }
    }
    if let Some(e) = eps {
        fis[0] += e[0];
        fpc[0] += e[1];
        fr[0] += e[2];
    }
    let x = solve_bloc(sol, &bs.s, [&fis, &fpc, &fr, &fs]);
    shift_state(&mut bs.s, &x);

    push14(&mut bs.hist.y, x[OUT_Y]);
    push14(&mut bs.hist.pi, x[OUT_PI]);
    push14(&mut bs.hist.i, x[OUT_I]);
    push14(&mut bs.hist.z, x[OUT_Z]);
    push14(&mut bs.hist.mrr, x[OUT_MRR]);

    let okun = wp.blocs[i].okun.unwrap_or(super::params::Okun { a1: 0.0, a2: 0.0 });
    bs.u_okun = okun.a1 * bs.u_okun - okun.a2 * x[OUT_Y];
    bs.ln_p += (wp.blocs[i].inflation_target + x[OUT_PI]) / 1200.0;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shift_state_moves_lags() {
        let mut s = [0.0; NS];
        for (j, v) in s.iter_mut().enumerate() {
            *v = j as f64 + 1.0;
        }
        let x = [100.0, 200.0, 300.0, 400.0, 500.0];
        shift_state(&mut s, &x);
        assert_eq!(s[0], 100.0);
        assert_eq!(s[1], 1.0);
        assert_eq!(s[2], 2.0);
        assert_eq!(s[3], 300.0);
        assert_eq!(s[4], 400.0);
        assert_eq!(s[5], 5.0);
        assert_eq!(s[16], 16.0);
        assert_eq!(s[17], 200.0);
        assert_eq!(s[18], 18.0);
        assert_eq!(s[30], 500.0);
        assert_eq!(s[31], 31.0);
        assert_eq!(s[32], 32.0);
    }
}
