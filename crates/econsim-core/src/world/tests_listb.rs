//! List B tests (spec 2.2.2 rev 9 D48, 2.5 and 2.6 rev 9 D47, 1.6b): the conditional forecast of the blocs' real exchange rate and the
//! bound on the total US-rate excess return. Every expectation is computed in the test from the spec text.

use std::sync::Arc;

use super::bloc::{self, OUT_Z};
use super::dollar::{self, DollarState, FC_MONTHS};
use super::net::UsInputs;
use super::params::{Par, WorldParams};
use super::trade::TradeState;
use super::{init_if_enabled, WorldBlock, K, NB};
use crate::config::Config;
use crate::sim;
use crate::testkit;
use crate::types::World;

/// The stage-4 profile's f-process keys with the link on (as the 4-1 tests: without them `f_t` is identically zero).
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

/// The inputs the forecast rule gives month `m` of the forecast (spec 2.6, rev 9): the base month's expected policy path moved forward,
/// the activity, oil, quantity and non-policy-risk terms decaying at the persistences the blocs project them with. `skip` = 1 for a
/// base month that ran.
fn forecast_inputs(wb: &WorldBlock, us_b: &UsInputs, fex_b: f64, cfg: &Config, m: usize, skip: usize) -> UsInputs {
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

/// Fex of the base month: F - Fbar - Fpol (spec 2.7).
fn fex_base(w: &World) -> f64 {
    let d = dstate(w);
    d.f_now - w.cfg.world_vix_per_unit * crate::exog::fin_factor_mean(&w.cfg) - d.fpol
}

#[test]
fn bound_rp_hand_values() {
    // Spec 2.2.2 rev 9 (D48): identity up to the knee 2.0 pp, knee + 1.5 tanh((x - knee)/1.5) above it, ceiling 3.5 pp.
    let cfg = Config { world_link_dollar: true, ..Config::default() };
    let par = Par::from_config(&cfg, &WorldParams::load());
    assert_eq!((par.rp_knee, par.rp_sat), (0.02, 0.015));
    for x in [-9.0, -2.0, 0.0, 1.0, 2.0] {
        assert_eq!(bloc::bound_rp(x, &par), x, "identity at or below the knee: {x}");
    }
    for (x, want) in [(2.5, 2.482269), (3.0, 2.874174), (3.5, 3.142391), (5.0, 3.446041), (7.3, 3.497443)] {
        close(bloc::bound_rp(x, &par), want, 2e-6, &format!("bound({x})"));
    }
    assert!(bloc::bound_rp(17.0, &par) <= 3.5 && bloc::bound_rp(17.0, &par) > 3.499);
    // Strictly increasing, continuous and with slope one at the knee.
    let mut prev = bloc::bound_rp(2.0, &par);
    for i in 1..200 {
        let v = bloc::bound_rp(2.0 + 0.05 * i as f64, &par);
        assert!(v > prev, "increasing at {i}");
        prev = v;
    }
    close(bloc::bound_rp(2.0 + 1e-7, &par) - 2.0, 1e-7, 1e-12, "slope one at the knee");
    // Off: the link off, or the scale 0, gives the identity everywhere.
    let off = Par::from_config(&Config { world_link_dollar: false, ..Config::default() }, &WorldParams::load());
    assert_eq!(off.rp_sat, 0.0);
    assert_eq!(bloc::bound_rp(17.0, &off), 17.0);
    let zero = Par::from_config(&Config { world_link_dollar: true, world_rp_sat_scale: 0.0, ..Config::default() }, &WorldParams::load());
    assert_eq!(bloc::bound_rp(17.0, &zero), 17.0);
}

#[test]
fn rp_bound_acts_on_the_total_return() {
    // D48: the bound acts on ex[k] + lambda_rp gp[k] together. One bloc, the IS and rule channels switched off (theta = k_us = 0), a
    // steady state of the US inputs (ex = ius = G = c, so gp[k] = c for every k): (c = 5, lambda 1.5) and (c = 12.5, lambda 0) have the
    // same total 12.5, so with the bound they give the same z; the 4-1 form, which bounds the rate differential only, gives different ones.
    let wp = WorldParams::load();
    let run = |c: f64, lam: f64, rp: f64, mf: f64| -> f64 {
        let cfg = Config { world_link_dollar: true, world_rp_sat_scale: rp, fx_rate_sat_scale: mf, ..Config::default() };
        let mut par = Par::from_config(&cfg, &wp);
        let i = 0;
        par.theta[i] = 0.0;
        par.k_us[i] = 0.0;
        par.lam_rp[i] = lam;
        let mut bs = bloc::BlocState::default();
        bs.g = c;
        let mut hist = bloc::UsHist::default();
        hist.ius = [c; 4];
        let mut us = UsInputs::zero();
        us.ius = c;
        us.ex = [c; K];
        let mut exb = [0.0; K];
        for k in 0..K {
            exb[k] = bloc::bound_pp(us.ex[k], &par);
        }
        let x = bloc::step_bloc(i, &mut bs, &wp, &par, None, &us, &exb, &hist, &[], 0, None);
        x[OUT_Z]
    };
    // With the bound on the total (rp scale 0.015) and the M-F bound on the differential also on (mf 0.015): equal totals, equal z.
    close(run(5.0, 1.5, 0.015, 0.015), run(12.5, 0.0, 0.015, 0.015), 1e-12, "bound on the total");
    // The 4-1 form (rp off): the differential is bounded alone, so the split matters.
    assert!((run(5.0, 1.5, 0.0, 0.015) - run(12.5, 0.0, 0.0, 0.015)).abs() > 1e-3, "4-1 form differs");
    // Below the knee the new form is the merged arithmetic: total 1.5 (c = 0.6, lambda 1.5) gives the same z with the bound on or off.
    close(run(0.6, 1.5, 0.015, 0.015), run(0.6, 1.5, 0.0, 0.015), 1e-14, "identity below the knee");
}

#[test]
fn rp_keys_are_validated() {
    assert!(Config { world_link_dollar: true, world_link_trade: true, ..Config::default() }.validate_world_links().is_ok());
    for (k, l, ok) in [(0.02, 0.015, true), (0.0, 0.0, true), (0.2, 0.2, true), (-0.01, 0.015, false), (0.02, -0.001, false), (0.3, 0.015, false), (0.02, 0.3, false), (f64::NAN, 0.015, false), (0.02, f64::INFINITY, false)] {
        let cfg = Config { world_rp_sat_knee: k, world_rp_sat_scale: l, ..Config::default() };
        assert_eq!(cfg.validate_world_links().is_ok(), ok, "knee {k} scale {l}");
    }
}

#[test]
fn forecast_exists_only_with_the_link_and_neutral_on() {
    let w = sim::new_world(&lcfg(12, 300), 5);
    let fc = forecast_of(&w);
    assert_eq!(fc.z.len(), FC_MONTHS);
    assert_eq!(fc.n0, w.world.as_ref().unwrap().n, "index 0 is the first reported month");
    // neutral off: no forecast, z_ref is the base value (the 4-1 form)
    let off = sim::new_world(&Config { world_dollar_neutral: false, ..lcfg(12, 300) }, 5);
    let d = dstate(&off);
    assert!(d.base.as_ref().unwrap().fc.is_none());
    let b = d.base.as_ref().unwrap().z;
    for n in [0u32, 5, 400] {
        assert_eq!(d.z_ref(n), b, "z_ref is the base value with the key off (n {n})");
    }
    // link off: no dollar state at all, so no forecast and no cost
    let none = sim::new_world(&Config { world_link_dollar: false, ..lcfg(12, 300) }, 5);
    assert!(none.world.as_ref().unwrap().dollar.is_none());
}

#[test]
fn forecast_reproduces_the_block() {
    // D47: fed the inputs it expects at the base, the block reproduces its forecast month by month. The replay path (`step_inputs`) is
    // the same `core` the forecast loop calls, started from a copy of the block as it stands at the base.
    let cfg = lcfg(12, 300);
    let w = sim::new_world(&cfg, 5);
    let fc = forecast_of(&w);
    let wb = w.world.as_ref().unwrap();
    let us_b = wb.last.as_ref().unwrap().us.clone();
    assert!(!us_b.proj_on, "the base month ran with the link not acting");
    let fex_b = fex_base(&w);
    assert!(fex_b != 0.0, "the factor moves in this world, so the fex term is exercised");
    let mut tw = wb.clone();
    tw.dollar = None;
    tw.trade = None;
    tw.fin = None;
    tw.oil = None;
    for m in 0..FC_MONTHS {
        let us = forecast_inputs(&tw, &us_b, fex_b, &cfg, m, 1);
        let x = tw.step_inputs(&us, None, false);
        for j in 0..NB {
            close(x[j][OUT_Z], fc.z[m][j], 1e-12, &format!("z month {m} bloc {j}"));
        }
    }
}

#[test]
fn forecast_error_is_the_response_to_the_news_alone() {
    // D47: z - z_fc is what the block does with the news. Linearity (no bound binds: rp scale 0, M-F off): a copy of the block at the
    // base stepped with the forecast inputs plus an extra policy impulse (ius +1 at forecast month 3, expected to decay at 0.97) gives
    // z - z_fc equal to the z of a fresh zero-state block stepped with the impulse alone.
    let cfg = Config { world_rp_sat_scale: 0.0, ..lcfg(12, 300) };
    let w = sim::new_world(&cfg, 7);
    let fc = forecast_of(&w);
    let wb = w.world.as_ref().unwrap();
    let us_b = wb.last.as_ref().unwrap().us.clone();
    let fex_b = fex_base(&w);
    let mut tw = wb.clone();
    tw.dollar = None;
    tw.trade = None;
    tw.fin = None;
    tw.oil = None;
    let mut fresh = WorldBlock::new(&Config { world_link_dollar: false, ..cfg.clone() });
    // The fresh block must start where the copy's clocks do not matter: its n only indexes the bloc clock, and no innovation is drawn.
    let impulse = |m: usize| -> UsInputs {
        let mut us = UsInputs::zero();
        if m >= 3 {
            let age = (m - 3) as i32;
            us.ius = 0.97f64.powi(age);
            for k in 0..K {
                us.ex[k] = 0.97f64.powi(age + k as i32);
            }
        } else {
            // expected from month 0 on? No: the news arrives at month 3, earlier months carry none.
        }
        us
    };
    for m in 0..40 {
        let mut us = forecast_inputs(&tw, &us_b, fex_b, &cfg, m, 1);
        let imp = impulse(m);
        us.ius += imp.ius;
        for k in 0..K {
            us.ex[k] += imp.ex[k];
        }
        let x = tw.step_inputs(&us, None, false);
        let y = fresh.step_inputs(&imp, None, false);
        for j in 0..NB {
            close(x[j][OUT_Z] - fc.z[m][j], y[j][OUT_Z], 1e-9, &format!("news response month {m} bloc {j}"));
        }
    }
}

#[test]
fn forecast_is_shock_independent_and_shared() {
    // The forecast is a function of the base state: a shock scheduled for any month leaves it unchanged (bit for bit), and a clone of the
    // world, the pair of a comparison, shares the one array.
    let a = sim::new_world(&lcfg(12, 300), 3);
    let mut b = sim::new_world(&lcfg(12, 300), 3);
    b.shocks.schedule(crate::shocks::ScheduledShock { at_month: 0, shock: crate::shocks::Shock::PolicyShift { bps: 100.0, months: 12 } });
    let (fa, fb) = (forecast_of(&a), forecast_of(&b));
    assert_eq!(fa.z.len(), fb.z.len());
    for m in 0..fa.z.len() {
        for j in 0..NB {
            assert_eq!(fa.z[m][j].to_bits(), fb.z[m][j].to_bits(), "month {m} bloc {j}");
        }
    }
    let c = a.clone();
    assert!(Arc::ptr_eq(&fa, &forecast_of(&c)), "clones share the forecast");
}

#[test]
fn forecast_holds_its_last_value_and_decays() {
    let w = sim::new_world(&lcfg(12, 300), 4);
    let d = dstate(&w);
    let fc = forecast_of(&w);
    let n0 = fc.n0;
    let last = fc.z[FC_MONTHS - 1];
    assert_eq!(d.z_ref(n0 + FC_MONTHS as u32 + 25), last, "held beyond the horizon");
    assert_eq!(d.z_ref(n0), fc.z[0]);
    // The expected US path is zero beyond K = 240 months and every state decays: the last value is small.
    for j in 0..NB {
        assert!(last[j].abs() < 0.05, "bloc {j} forecast at the horizon {}", last[j]);
    }
}

#[test]
fn z_fc_metric_and_phase_b_read_the_forecast() {
    // Each reported month the world's `z_fc` metric, the state `z_ref_now` and the forecast array agree, and ln S_j uses them.
    let mut w = sim::new_world(&lcfg(12, 300), 6);
    let fc = forecast_of(&w);
    for m in 0..6usize {
        sim::step(&mut w);
        let wb = w.world.as_ref().unwrap();
        let d = dstate(&w);
        for j in 0..NB {
            assert_eq!(d.z_ref_now[j].to_bits(), fc.z[m][j].to_bits(), "z_ref_now month {m} bloc {j}");
            assert_eq!(wb.diag.z_fc[j].to_bits(), fc.z[m][j].to_bits(), "diag.z_fc month {m} bloc {j}");
        }
    }
}

#[test]
fn first_month_forecast_error_is_innovation_sized() {
    // D47 / W4-29: the weighted z - z_fc of the first reported month is a forecast error (mean about 0, sd about 0.3 points over 64
    // seeds in the prototype); the zero-input reference of the stage-4-1 review gave mean +1.5, sd 2.4, max 7.9. Eight seeds, 500 hh.
    let mut worst: f64 = 0.0;
    let mut sum = 0.0;
    for seed in 1..=8u64 {
        let mut w = sim::new_world(&lcfg(24, 500), seed);
        let wh10 = w.world.as_ref().unwrap().trade.as_ref().unwrap().tab.dollar_w;
        sim::step(&mut w);
        let wb = w.world.as_ref().unwrap();
        let fc = forecast_of(&w);
        let e: f64 = (0..NB).map(|j| wh10[j] * (wb.blocs[j].hist.z[0] - fc.z[0][j])).sum();
        worst = worst.max(e.abs());
        sum += e;
    }
    assert!(worst < 4.0, "worst weighted first-month forecast error {worst}");
    assert!((sum / 8.0).abs() < 1.5, "mean {}", sum / 8.0);
}

#[test]
fn trade_zrel_reads_the_forecast() {
    // Spec 2.5 rev 9: in dollar mode with the forecast, zrel_j = z_j - zref_j + 100 a_j. One month of `post_bloc_ref` on a tiny world.
    let mut w = testkit::tiny_world(Config { world_block_enabled: true, world_link_dollar: true, n_households: 300, ..Config::default() }, 2);
    init_if_enabled(&mut w);
    let wp = WorldParams::load();
    let cfg = w.cfg.clone();
    let mut tr = TradeState::new(&cfg, &wp);
    let mut blocs: [bloc::BlocState; NB] = std::array::from_fn(|_| bloc::BlocState::default());
    for j in 0..NB {
        blocs[j].hist.z[0] = 2.0 + j as f64;
    }
    tr.capture(&w, &blocs);
    let a = [0.01; NB];
    let zref: [f64; NB] = std::array::from_fn(|j| 1.5 + 0.5 * j as f64);
    let _ = tr.post_bloc_ref(&w, &blocs, Some(&a), Some(&zref));
    for j in 0..NB {
        let zrel = (2.0 + j as f64) - zref[j] + 100.0 * 0.01;
        // Phi^d after one month from zero: speed x kappa x zrel.
        close(tr.phi_d[j], cfg.world_dcp_speed_usd * cfg.world_dcp_kappa_usd * zrel, 1e-12, &format!("phi_d bloc {j}"));
    }
    // None: the 4-1 form, z~ - z_level (z_level = the captured z, so zrel = 100 a).
    let mut tr2 = TradeState::new(&cfg, &wp);
    tr2.capture(&w, &blocs);
    let _ = tr2.post_bloc_ref(&w, &blocs, Some(&a), None);
    for j in 0..NB {
        close(tr2.phi_d[j], cfg.world_dcp_speed_usd * cfg.world_dcp_kappa_usd * 1.0, 1e-12, &format!("4-1 form bloc {j}"));
    }
}

#[test]
fn burn_in_zero_world_forecast_starts_at_the_first_month() {
    // A world without burn-in has no base month that ran: its forecast starts at the first step with that month's inputs as the
    // expectation (index 0 at tick 0). The base is captured at the first step.
    let mut w = testkit::tiny_world(lcfg(0, 300), 4);
    init_if_enabled(&mut w);
    assert!(dstate(&w).base.is_none());
    sim::step(&mut w);
    let fc = forecast_of(&w);
    assert_eq!(fc.n0, 0);
    // The first month's forecast error is the month's innovation: small against the level of the forecast itself.
    let wb = w.world.as_ref().unwrap();
    let wh10 = wb.trade.as_ref().unwrap().tab.dollar_w;
    let e: f64 = (0..NB).map(|j| wh10[j] * (wb.blocs[j].hist.z[0] - fc.z[0][j])).sum();
    assert!(e.abs() < 3.0, "first-month forecast error {e}");
}

#[test]
fn step_month_passes_the_forecast_to_the_trade_module() {
    // D47: in a world step the trade pass-through reads z against the forecast. After the first reported month the dominant-currency state
    // is one update from zero: phi_d = speed x kappa x zrel, with zrel_j = z_j - z_fc,j(0) + 100 a_j (2.5). With the key off it is the
    // rev-7 zrel_j = (z_j + 100 a_j) - z_j,b.
    let base = Config { world_link_trade: true, ..lcfg(12, 300) };
    for neutral in [true, false] {
        let mut w = sim::new_world(&Config { world_dollar_neutral: neutral, ..base.clone() }, 5);
        let fc = forecast_or_none(&w);
        let zb = dstate(&w).base.as_ref().unwrap().z;
        sim::step(&mut w);
        let wb = w.world.as_ref().unwrap();
        let d = dstate(&w);
        let tr = wb.trade.as_ref().unwrap();
        for j in 0..NB {
            let z = wb.blocs[j].hist.z[0];
            let zref = match (&fc, neutral) {
                (Some(f), true) => f.z[0][j],
                _ => zb[j],
            };
            let zrel = (z - zref) + 100.0 * d.a_bil[j];
            close(tr.phi_d[j], w.cfg.world_dcp_speed_usd * w.cfg.world_dcp_kappa_usd * zrel, 1e-12, &format!("phi_d bloc {j} neutral {neutral}"));
        }
    }
}

fn forecast_or_none(w: &World) -> Option<Arc<dollar::Forecast>> {
    dstate(w).base.as_ref().unwrap().fc.clone()
}

#[test]
fn legacy_keys_reproduce_the_4_1_link() {
    // I6 at unit scale: world_dollar_neutral false and world_rp_sat_scale 0 give the cc33f15 (stage 4-1) broad rate bit for bit. The
    // fixture is `fx_broad` of months 0-3 of `econsim-cli run 300 4 5 12` on cc33f15 with the same keys (trade link on). The fixture
    // predates `bank_loss_carry`, which moves the ABM path, so the test runs with its legacy value.
    let legacy = [102.54312580070962f64, 99.5785773023966, 99.38103646232622, 96.89806140231458];
    let cfg = Config {
        world_link_trade: true,
        world_dollar_neutral: false,
        world_rp_sat_scale: 0.0,
        bank_loss_carry: false,
        ..lcfg(12, 300)
    };
    let mut w = sim::new_world(&cfg, 5);
    for (t, want) in legacy.iter().enumerate() {
        sim::step(&mut w);
        let got = w.world.as_ref().unwrap().diag.fx_broad;
        assert_eq!(got.to_bits(), want.to_bits(), "fx_broad month {t}: {got} vs {want}");
    }
    // The redesigned link is a different path from month 0 (the forecast reference and the bound act).
    let mut n = sim::new_world(&Config { world_link_trade: true, ..lcfg(12, 300) }, 5);
    sim::step(&mut n);
    assert!((n.world.as_ref().unwrap().diag.fx_broad - legacy[0]).abs() > 1e-6);
}
