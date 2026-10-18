//! US inputs of the blocs (spec 2.1) and the cross-bloc terms of the network (spec 2.3, Peff of 2.2.2).

use super::bloc::{BlocState, UsHist};
use super::params::{Par, WorldParams};
use super::{K, NB};
use crate::monetary;
use crate::types::World;

/// What the blocs see of the US in one month (spec 2.1, `UsInputs`). `ius` and `ex` in pp with the offset applied;
/// `y_us` = y_US(t-1) in pp with `world_yus_offset` subtracted (N1); `oil` and `qty` in %; `fex` in VIX points (0 before stage 4); `dollar` = the level a_t of the
/// Dollar shocks in log points; `dollar_decay` = the monthly decay in effect.
#[derive(Clone, Debug)]
pub struct UsInputs {
    pub ius: f64,
    pub ex: [f64; K],
    pub y_us: f64,
    pub oil: f64,
    pub qty: f64,
    pub fex: f64,
    pub dollar: f64,
    pub dollar_decay: f64,
    /// Stage 4-1 (spec 2.6): delta_t, the Dollar shock level alone (the `dollar` field is the total addend a_t when `proj_on`).
    pub dollar_delta: f64,
    /// Stage 4-1: true only when the dollar link acts this month; then `step_bloc` projects the addend with `dollar_proj`.
    pub proj_on: bool,
    /// Stage 4-1: projected q + g + cred addend k months ahead, `dollar_proj[k-1]` for k = 1..=K (log points).
    pub dollar_proj: [f64; K],
}

impl UsInputs {
    /// No US signal at all: the isolation inputs of spec 1.9.
    pub fn zero() -> UsInputs {
        UsInputs {
            ius: 0.0,
            ex: [0.0; K],
            y_us: 0.0,
            oil: 0.0,
            qty: 0.0,
            fex: 0.0,
            dollar: 0.0,
            dollar_decay: 1.0,
            dollar_delta: 0.0,
            proj_on: false,
            dollar_proj: [0.0; K],
        }
    }
}

/// The cross-bloc terms for the month (spec 2.2.2 `Net`): f[i][m] = F_{i,t-1-m} (activity input of bloc i, with the US term),
/// wz[i][m] = sum over partners o of w_io z_{o,t-1-m}. Zero for months before creation.
#[derive(Clone, Debug)]
pub struct Net {
    pub f: [[f64; 3]; NB],
    pub wz: [[f64; 13]; NB],
}

impl Net {
    /// Built from the bloc histories (index 0 = month t-1) and the US history. `y_us_now` is y_US(t-1) (0 at the first month).
    /// `gap` (stage 2, coupled mode only) adds the bloc's US import-demand term g^{US->i} - y_US per month (spec 2.3): entry
    /// [i][m] is the term of month t-1-m. `None` gives the stage-1 network exactly.
    pub fn build(wp: &WorldParams, blocs: &[BlocState; NB], us: &UsHist, y_us_now: f64, gap: Option<&[[f64; 3]; NB]>) -> Net {
        let ys = [y_us_now, us.y_us[0], us.y_us[1]];
        let mut f = [[0.0; 3]; NB];
        let mut wz = [[0.0; 13]; NB];
        for i in 0..NB {
            for m in 0..3 {
                let g = match gap {
                    Some(gp) => ys[m] + gp[i][m],
                    None => ys[m],
                };
                let mut acc = wp.network.s_us[i] * g;
                for o in 0..NB {
                    acc += wp.network.s_bb[i][o] * blocs[o].hist.y[m];
                }
                f[i][m] = acc;
            }
            for m in 0..13 {
                let mut acc = 0.0;
                for o in 0..NB {
                    acc += wp.network.w_bb[i][o] * blocs[o].hist.z[m];
                }
                wz[i][m] = acc;
            }
        }
        Net { f, wz }
    }
}

/// The US inputs of month `w.tick` read from the ABM (spec 2.1 and the accessors of 1.9). `hist.u` is the unemployment ring:
/// it takes u_{t-1} now, and on the first month all three entries are u_{t-1}. Reads the policy rate, the curve state and
/// the expected path (`monetary::expected_rate_path`), so it must run after `update_curve` of the month.
pub fn us_inputs(w: &World, hist: &mut UsHist, first: bool, par: &Par) -> UsInputs {
    let u_prev = w.unemployment_rate();
    if first {
        hist.u = [u_prev; 3];
    } else {
        hist.u = [u_prev, hist.u[0], hist.u[1]];
    }
    let mean_u = (hist.u[0] + hist.u[1] + hist.u[2]) / 3.0;
    // N1: the raw gap has a permanent negative mean (u* is capped below the mean u); the constant is removed here, where y_US
    // enters the blocs, and the same centred value is stored in the history and the replay record.
    let y_us = 100.0 * par.okun_ratio * (w.natural_unemployment() - mean_u) - par.yus_offset;

    let pi_lr = w.monetary.pi_lr.unwrap_or(w.cfg.inflation_target);
    let i_inf = monetary::current_r_star(w) + pi_lr;
    let ius = 100.0 * (w.cb.policy_rate - i_inf) - par.ius_offset;

    let mut path = [0.0; K];
    monetary::expected_rate_path(w, &mut path);
    let mut ex = [0.0; K];
    let mut dk = 1.0;
    for k in 0..K {
        ex[k] = 100.0 * (path[k] - i_inf) - par.ius_offset * dk;
        dk *= par.ex_decay;
    }
    let dollar_decay = if w.shocks.dollar_decay == 0.0 { 1.0 } else { w.shocks.dollar_decay };
    UsInputs {
        ius,
        ex,
        y_us,
        oil: 0.0,
        qty: 0.0,
        fex: 0.0,
        dollar: w.shocks.dollar_log(),
        dollar_decay,
        dollar_delta: 0.0,
        proj_on: false,
        dollar_proj: [0.0; K],
    }
}

/// Stage-1 GDP weights under the selected OPEC+ composition (spec D7, `world_uae_in_opec`): (usd, ppp) per bloc.
pub fn gdp_weights(wp: &WorldParams, uae_in: bool) -> (Vec<f64>, Vec<f64>) {
    let s = if uae_in { &wp.opec.uae_in } else { &wp.opec.uae_out };
    (s.weight_usd.clone(), s.weight_ppp.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_inputs_have_unit_decay() {
        let z = UsInputs::zero();
        assert_eq!(z.dollar_decay, 1.0);
        assert!(z.ex.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn net_is_zero_before_creation() {
        let wp = WorldParams::load();
        let blocs: [BlocState; NB] = std::array::from_fn(|_| BlocState::default());
        let net = Net::build(&wp, &blocs, &UsHist::default(), 0.0, None);
        assert!(net.f.iter().flatten().all(|&x| x == 0.0));
        assert!(net.wz.iter().flatten().all(|&x| x == 0.0));
    }
}
