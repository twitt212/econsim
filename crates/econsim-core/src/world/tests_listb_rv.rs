//! Reviewer's replacement tests for list B (stage 4-1b review). Each computes its expectation from the spec text (2.2.2 and 2.6, rev 9),
//! not from the code, and each kills at least one mutant that survived the 14 tests of `tests_listb.rs` (see section 6 of the reviewer's note, not published).

use std::sync::Arc;

use super::bloc::{self, OUT_Z};
use super::dollar::{self, DollarState, FC_MONTHS};
use super::net::UsInputs;
use super::params::{Par, WorldParams};
use super::{init_if_enabled, WorldBlock, K, NB};
use crate::config::Config;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::testkit;
use crate::types::World;

fn lcfg(burn: u32, households: usize) -> Config {
    Config {
        burn_in_months: burn,
        n_households: households,
        world_block_enabled: true,
        world_link_dollar: true,
        fin_factor_rho: 0.83,
        fin_factor_sigma: 0.30,
        fin_jump_intensity: 0.06,
        fin_jump_mean: 1.3,
        fin_jump_sd: 0.5,
        erp_fin_loading: 0.0028,
        ..Config::default()
    }
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tol {tol})");
}

fn dstate(w: &World) -> &DollarState {
    w.world.as_ref().unwrap().dollar.as_ref().unwrap()
}

fn forecast_of(w: &World) -> Arc<dollar::Forecast> {
    dstate(w).base.as_ref().unwrap().fc.clone().expect("forecast")
}

/// Spec 2.6 rev 9, the forecast law, written out from the table (not shared with the production code).
fn law(wb: &WorldBlock, us_b: &UsInputs, fex_b: f64, cfg: &Config, m: usize, skip: usize) -> UsInputs {
    let s = skip + m;
    let mut us = UsInputs::zero();
    for k in 0..K {
        us.ex[k] = if s + k < K { us_b.ex[s + k] } else { 0.0 };
    }
    us.ius = if s == 0 { us_b.ius } else { us.ex[0] };
    let e = s as i32;
    us.y_us = us_b.y_us * wb.par.rho_f.powi(e);
    us.oil = us_b.oil * wb.par.rho_oil.powi(e);
    us.qty = us_b.qty * wb.par.rho_qty.powi(e);
    us.fex = fex_b * cfg.fin_factor_rho.powi(e);
    us
}

fn fex_base(w: &World) -> f64 {
    let d = dstate(w);
    d.f_now - w.cfg.world_vix_per_unit * crate::exog::fin_factor_mean(&w.cfg) - d.fpol
}

/// z of bloc `i` after one step from a steady US state `ex = ius = G = c` with the premium state `u0`, only bloc `i` carrying a
/// non-zero `lambda_rp` (so a wrong bloc index shows), the IS and rule channels switched off (theta = k_us = 0).
fn one_step(i: usize, c: f64, lam: f64, u0: f64, rp: f64, mf: f64) -> f64 {
    let wp = WorldParams::load();
    let cfg = Config { world_link_dollar: true, world_rp_sat_scale: rp, fx_rate_sat_scale: mf, ..Config::default() };
    let mut par = Par::from_config(&cfg, &wp);
    par.theta[i] = 0.0;
    par.k_us[i] = 0.0;
    for j in 0..NB {
        par.lam_rp[j] = 0.0;
    }
    par.lam_rp[i] = lam;
    let mut bs = bloc::BlocState::default();
    bs.g = c;
    bs.u_rp = u0;
    let mut hist = bloc::UsHist::default();
    hist.ius = [c; 4];
    let mut us = UsInputs::zero();
    us.ius = c;
    us.ex = [c; K];
    let mut exb = [0.0; K];
    for k in 0..K {
        exb[k] = bloc::bound_pp(us.ex[k], &par);
    }
    bloc::step_bloc(i, &mut bs, &wp, &par, None, &us, &exb, &hist, &[], 0, None)[OUT_Z]
}

#[test]
fn rv_rp_bound_leaves_the_premium_innovation_outside() {
    // D48: fS[k] = -( M(ex[k] + lambda_rp gp[k]) + u_t rho_rp^k ): the premium innovation u_t is added after the bound. The block is linear
    // in fS, so the z response to u_t is the same whatever the (bounded) total is. A bound applied to the sum with u_t would compress it.
    for i in [0usize, 5] {
        let resp = |c: f64| one_step(i, c, 1.5, 0.5, 0.015, 0.015) - one_step(i, c, 1.5, 0.0, 0.015, 0.015);
        let low = resp(0.1); // total 0.25 pp, below the knee
        let high = resp(5.0); // total 12.5 pp, far above the knee
        assert!(low.abs() > 1e-6, "bloc {i}: the premium state moves z ({low})");
        close(high, low, 1e-12, &format!("bloc {i}: u_t response with the total above the knee"));
        // Below the knee the bound is the identity, so the new fS and the 4-1 fS (rp scale 0) agree exactly, premium innovation included
        // (this also pins the sign of the u_t term in the bounded branch).
        close(one_step(i, 0.1, 1.5, 0.5, 0.015, 0.015), one_step(i, 0.1, 1.5, 0.5, 0.0, 0.015), 1e-14, &format!("bloc {i}: identity below the knee with u_t"));
    }
}

#[test]
fn rv_rp_bound_uses_the_blocs_own_lambda() {
    // D48 per bloc: only bloc i has lambda_rp > 0; (c = 5, lambda 1.5) and (c = 12.5, lambda 0) have the same total 12.5, so the same z
    // after the bound, for every bloc index (a mutant reading lambda_rp[0] would give a total of 5 for i = 3).
    for i in [0usize, 3, 7, 11] {
        close(one_step(i, 5.0, 1.5, 0.0, 0.015, 0.015), one_step(i, 12.5, 0.0, 0.0, 0.015, 0.015), 1e-12, &format!("bloc {i}"));
    }
}

#[test]
fn rv_rp_bound_reads_the_raw_expected_path_not_the_mf_bounded_one() {
    // D48: M acts on the raw ex[k] + lambda gp[k]. With the M-F keys on as well (fx_rate_*), the bound must not be applied twice:
    // (c = 5, lambda 1.5) gives the same z whether or not fR/exb carry the M-F bound, since fS ignores exb when the bound is on.
    close(one_step(2, 5.0, 1.5, 0.0, 0.015, 0.015), one_step(2, 5.0, 1.5, 0.0, 0.015, 0.0), 1e-12, "fS ignores exb with the bound on");
}

#[test]
fn rv_forecast_horizon_and_defaults() {
    assert_eq!(FC_MONTHS, 480, "spec 2.6: 480 forecast months");
    let c = Config::default();
    assert_eq!((c.world_dollar_neutral, c.world_rp_sat_knee, c.world_rp_sat_scale), (true, 0.02, 0.015));
}

#[test]
fn rv_forecast_decays_oil_and_quantity_with_their_own_persistences() {
    // The oil and quantity rows of the forecast law (spec 2.6 table) are zero in a world whose oil link is off, so the 14 tests could not
    // see them. Call `forecast` with base-month inputs that carry an oil gap and an OPEC+ quantity gap, and replay the law.
    let cfg = lcfg(12, 300);
    let w = sim::new_world(&cfg, 5);
    let wb = w.world.as_ref().unwrap();
    let mut us_b = wb.last.as_ref().unwrap().us.clone();
    us_b.oil = 4.0;
    us_b.qty = -2.5;
    let fex_b = fex_base(&w);
    let fc = wb.forecast(&w, &us_b, 1);
    let mut tw = wb.clone();
    tw.dollar = None;
    tw.trade = None;
    tw.fin = None;
    tw.oil = None;
    let plain = forecast_of(&w);
    let mut moved = 0.0f64;
    for m in 0..FC_MONTHS {
        let us = law(&tw, &us_b, fex_b, &cfg, m, 1);
        let x = tw.step_inputs(&us, None, false);
        for j in 0..NB {
            close(x[j][OUT_Z], fc.z[m][j], 1e-12, &format!("z month {m} bloc {j}"));
            moved = moved.max((fc.z[m][j] - plain.z[m][j]).abs());
        }
    }
    assert!(moved > 1e-3, "the oil and quantity gaps must move the forecast ({moved})");
}

#[test]
fn rv_burn_in_zero_forecast_replays_the_first_month_rule() {
    // A world without burn-in forecasts from its first month's inputs with skip 0 (spec 2.6): index 0 of the forecast is month 0's own
    // expectation, ius_0 = ius_b, the clock starts at 0. Replay the law with skip 0 from the block as it stood before the first step.
    let cfg = lcfg(0, 300);
    let mut w = testkit::tiny_world(cfg.clone(), 4);
    init_if_enabled(&mut w);
    let wb0 = w.world.as_ref().unwrap().clone();
    sim::step(&mut w);
    let fc = forecast_of(&w);
    assert_eq!(fc.n0, 0);
    let us_first = w.world.as_ref().unwrap().last.as_ref().unwrap().us.clone();
    let fex_b = fex_base(&w);
    let mut tw = wb0;
    tw.dollar = None;
    tw.trade = None;
    tw.fin = None;
    tw.oil = None;
    for m in 0..80 {
        let us = law(&tw, &us_first, fex_b, &cfg, m, 0);
        let x = tw.step_inputs(&us, None, false);
        for j in 0..NB {
            close(x[j][OUT_Z], fc.z[m][j], 1e-12, &format!("burn-in-0 z month {m} bloc {j}"));
        }
    }
}

#[test]
fn rv_burn_in_zero_forecast_ignores_user_shocks_on_the_blocs() {
    // Spec 2.6: no user shock enters the forecast. In a world without burn-in the base is captured at the first step, after the month's
    // shocks were registered, so a bloc shock scheduled for month 0 is already in `users`; the forecast must still be the unshocked one.
    let make = |shock: bool| {
        let mut w = testkit::tiny_world(lcfg(0, 300), 4);
        init_if_enabled(&mut w);
        if shock {
            w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::BlocPolicy { bloc: "EA".to_string(), bps: 100.0, months: 12 } });
        }
        sim::step(&mut w);
        forecast_of(&w)
    };
    let (a, b) = (make(false), make(true));
    for m in 0..a.z.len() {
        for j in 0..NB {
            assert_eq!(a.z[m][j].to_bits(), b.z[m][j].to_bits(), "month {m} bloc {j}");
        }
    }
}

#[test]
fn rv_burn_in_zero_world_makes_no_forecast_with_the_key_off() {
    // `world_dollar_neutral` false is the 4-1 form in every path: a world without burn-in (base captured at its first step) must not
    // compute or store a forecast either, and z_ref stays at the base value.
    let mut w = testkit::tiny_world(Config { world_dollar_neutral: false, ..lcfg(0, 300) }, 4);
    init_if_enabled(&mut w);
    sim::step(&mut w);
    let d = dstate(&w);
    assert!(d.base.as_ref().unwrap().fc.is_none());
    assert_eq!(d.z_ref(7), d.base.as_ref().unwrap().z);
}
