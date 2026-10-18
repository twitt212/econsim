//! Reviewer's replacement tests for stage 4-1 (the stage 4-1 review). Every expectation is computed in the test from the spec formulas
//! (2.6, 2.7) and from states the code does not compute in the same step; none of them reads `d.a_bil`, `us.dollar`,
//! `us.dollar_proj` or `ln_s_now` as an expectation.

use super::bloc::OUT_Y;
use super::dollar;
use super::finance::frisk_step;
use super::net::UsInputs;
use super::params::WorldParams;
use super::{init_if_enabled, WorldBlock, K, NB};
use crate::config::Config;
use crate::rng::{self, normal, stream};
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::testkit;
use crate::types::World;

/// The stage-4 profile's f-process keys are part of the configuration: without them `f_t` is identically zero and so are Frisk,
/// g and Fex (the implementer's `lcfg` leaves them out, so its integrated tests never see a moving factor).
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

fn dstate(w: &World) -> &dollar::DollarState {
    w.world.as_ref().unwrap().dollar.as_ref().unwrap()
}

/// Spec 2.6: p(age) = 1 for age < months, then 0.5^((age - months + 1)/3).
fn p_spec(age: i64, months: i64) -> f64 {
    if age < months {
        1.0
    } else {
        0.5f64.powf((age - months + 1) as f64 / 3.0)
    }
}

#[test]
fn rv_phase_a_b_against_an_independent_recomputation() {
    // World with a RiskOff (month 1), a Dollar shock (month 2) and a UsCredibility (month 3); risk multiplier 1.25 so that a
    // dropped multiplier shows. Every month the common and bilateral addends, the projection, Fex, zeta, ln S_j and the broad
    // rate are recomputed from the spec text.
    let cfg = Config { world_fx_risk_mult: 1.25, ..lcfg(12, 300) };
    let mut w = sim::new_world(&cfg, 6);
    w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::RiskOff { vix_points: 20.0 } });
    w.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::Dollar { pct: 5.0, half_life_months: 0.0 } });
    w.shocks.schedule(ScheduledShock { at_month: 3, shock: Shock::UsCredibility { tp_bp: 22.0, months: 3, dollar_pct: 3.3 } });
    let wp = WorldParams::load();
    let gamma = wp.finance.gamma_pct_per_vix.clone();
    let wh10 = w.world.as_ref().unwrap().trade.as_ref().unwrap().tab.dollar_w;
    // Spec 2.6: AE = EA, JP, UK, CA, AU; EME = MX, IN, KR, BR, RoW; bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW.
    let kind = ["ae", "cn", "ae", "ae", "ae", "eme", "eme", "eme", "eme", "ae", "opec", "eme"];
    let sd_of = |j: usize| match kind[j] {
        "ae" => cfg.world_fx_idio_sd_ae,
        "eme" => cfg.world_fx_idio_sd_eme,
        "cn" => cfg.world_fx_idio_sd_cn,
        _ => cfg.world_fx_idio_sd_opec,
    };
    let ln_cred = (1.0f64 - 0.033).ln();
    let fbar = 10.0 * (0.06 * 1.3 / (1.0 - 0.83));
    let mut zeta = [0.0f64; NB];
    let mut saw_delta = false;
    for t in 0..8i64 {
        let tick = w.tick;
        let ln_pt_before = dollar::ln_pt_of(&w);
        sim::step(&mut w);
        let d = dstate(&w);
        let wb = w.world.as_ref().unwrap();
        let us: UsInputs = wb.last.as_ref().unwrap().us.clone();
        let base = d.base.clone().expect("base");
        // zeta_j: AR(1) with the WORLD_FX draw of (seed, tick, j).
        for j in 0..NB {
            let mut r = rng::agent_rng(w.seed, tick, stream::WORLD_FX, j as u64);
            zeta[j] = cfg.world_fx_idio_rho * zeta[j] + sd_of(j) / 100.0 * normal(&mut r);
            close(d.zeta[j], zeta[j], 1e-15, &format!("zeta[{j}] month {t}"));
        }
        // Risk addend g_j = mult gamma_j/100 (Frisk_t - Frisk_b), gbar = sum wH10 g.
        let rel = d.frisk - base.frisk0;
        let g: Vec<f64> = (0..NB).map(|j| 1.25 * gamma[j] / 100.0 * rel).collect();
        let gbar: f64 = (0..NB).map(|j| wh10[j] * g[j]).sum();
        // Credibility term (fires at reported month 3), premium step q_psi from the world, Dollar shock level from the shock state.
        let cred = if t >= 3 { ln_cred * p_spec(t - 3, 3) } else { 0.0 };
        let qpsi = w.fx_psi_component;
        let delta = w.shocks.dollar_log();
        if t >= 2 {
            assert!(delta > 0.0, "Dollar shock level month {t}");
            saw_delta = true;
        } else {
            assert_eq!(delta, 0.0);
        }
        close(us.dollar_delta, delta, 1e-15, "dollar_delta");
        let q = qpsi + cred;
        close(us.dollar, delta + q + gbar, 1e-14, &format!("a_common month {t}"));
        assert!(us.proj_on);
        for j in 0..NB {
            close(d.a_bil[j], delta + q + g[j], 1e-14, &format!("a_bil[{j}] month {t}"));
        }
        // Projection k = 1..=K: qpsi rho_psi^k + gbar rho_risk^k + credibility profile advanced by k months.
        for k in [1usize, 2, 3, 5, 12, 40, 100, K] {
            let kf = k as i32;
            let c = if t >= 3 { ln_cred * p_spec(t - 3 + k as i64, 3) } else { 0.0 };
            let want = qpsi * cfg.fx_psi_rho.powi(kf) + gbar * cfg.world_fx_risk_rho.powi(kf) + c;
            close(us.dollar_proj[k - 1], want, 1e-14, &format!("dollar_proj[{}] month {t}", k - 1));
        }
        // Fex = F - Fbar - Fpol with F the factor of this month after its injections (10 f).
        let f_now = 10.0 * w.exog.fin_factor;
        close(d.f_now, f_now, 1e-12, "F");
        close(us.fex, f_now - fbar - d.fpol, 1e-12, &format!("fex month {t}"));
        // lnS_j and the broad rate.
        let mut ln_d = 0.0;
        for j in 0..NB {
            let z = wb.blocs[j].hist.z[0];
            // List B (rev 9): z is read against its conditional forecast, the array of the base (index t = the t-th reported month).
            let zfc = base.fc.as_ref().expect("forecast").z[t as usize][j];
            let ln_s = (z - zfc) / 100.0 + (wb.blocs[j].ln_p - base.ln_p[j]) - (ln_pt_before - base.ln_pt) + (delta + q + g[j]) + zeta[j];
            close(d.ln_s_now[j], ln_s, 1e-13, &format!("lnS[{j}] month {t}"));
            ln_d += wh10[j] * ln_s;
        }
        close(w.row.fx, base.fx0 * ln_d.exp(), 1e-12 * base.fx0, &format!("fx month {t}"));
    }
    assert!(saw_delta);
}

#[test]
fn rv_frisk_base_equals_the_shadow_filter_through_the_burn_in() {
    // The filter runs through the 12 burn-in months on the ABM's own f path; its state at the base is the independent recursion.
    let mut w = testkit::tiny_world(lcfg(0, 300), 9);
    w.cfg.burn_in_months = 12;
    init_if_enabled(&mut w);
    let (mut fr, mut f_prev) = (0.0, 0.0);
    let mut base_expect = 0.0;
    for t in 0..16 {
        sim::step(&mut w);
        let f = 10.0 * w.exog.fin_factor;
        fr = frisk_step(fr, f_prev, f, 0.83, 0.588, 0.9);
        f_prev = f;
        close(dstate(&w).frisk, fr, 1e-12, &format!("frisk month {t}"));
        if t == 11 {
            base_expect = fr;
        }
    }
    assert!(base_expect != 0.0, "the burn-in path must move the filter");
    close(dstate(&w).base.as_ref().unwrap().frisk0, base_expect, 1e-12, "Frisk_b is the filter state of the last burn-in month");
}

#[test]
fn rv_policy_surprise_feeds_frisk_and_china_risk_off_injects_absolute_points() {
    // PolicyShift 100 bp: the filter sees F + 13 in the shock month. BlocDemand CN -3 with risk_off: F + 6 x 3 = 18 points; the sign
    // of pct does not matter; without risk_off nothing is injected. A RiskOff and a BlocDemand do not enter Fpol.
    let mut b = sim::new_world(&lcfg(12, 300), 4);
    let mut a = sim::new_world(&lcfg(12, 300), 4);
    let mut c = sim::new_world(&lcfg(12, 300), 4);
    let mut e = sim::new_world(&lcfg(12, 300), 4);
    a.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    c.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::BlocDemand { bloc: "CN".to_string(), pct: -3.0, half_life_months: 12.0, risk_off: true } });
    e.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::BlocDemand { bloc: "CN".to_string(), pct: -3.0, half_life_months: 12.0, risk_off: false } });
    let (mut fr_prev_b, mut f_prev_b) = (0.0, 0.0);
    for t in 0..4 {
        for w in [&mut a, &mut b, &mut c, &mut e] {
            sim::step(w);
        }
        let (da, db, dc, de) = (dstate(&a), dstate(&b), dstate(&c), dstate(&e));
        if t == 2 {
            close(da.f_now - db.f_now, 13.0, 1e-9, "policy surprise 13 points");
            let want = frisk_step(fr_prev_b, f_prev_b, db.f_now + 13.0, 0.83, 0.588, 0.9) - db.frisk;
            close(da.frisk - db.frisk, want, 1e-9, "the filter sees the injected factor");
            close(dc.f_now - db.f_now, 18.0, 1e-9, "China risk-off 18 points");
            close(dc.fpol, db.fpol, 0.0, "risk-off is not policy");
            close(de.f_now - db.f_now, 0.0, 1e-12, "no injection without risk_off");
        }
        fr_prev_b = db.frisk;
        f_prev_b = db.f_now;
    }
}

#[test]
fn rv_fex_term_scales_with_ratio_per_100bp_and_persistence() {
    // fex enters the IS equation as theta_f fex rho_fin^k with theta_f = theta ratio / per_100bp.
    let resp = |cfg: Config, fex: f64| -> f64 {
        let mut zero = WorldBlock::new(&cfg);
        let mut pos = WorldBlock::new(&cfg);
        let us0 = UsInputs::zero();
        let mut us1 = UsInputs::zero();
        us1.fex = fex;
        let x0 = zero.step_inputs(&us0, None, false)[0][OUT_Y];
        let x1 = pos.step_inputs(&us1, None, false)[0][OUT_Y];
        x1 - x0
    };
    let base = lcfg(0, 300);
    let r = resp(base.clone(), 5.0);
    assert!(r < 0.0, "positive fex lowers output: {r}");
    close(resp(base.clone(), 10.0) / r, 2.0, 1e-9, "linear in fex");
    close(resp(Config { world_theta_f_ratio: 1.0, ..base.clone() }, 5.0) / r, 2.0, 1e-9, "proportional to the ratio");
    close(resp(Config { world_vix_per_100bp: 26.0, ..base.clone() }, 5.0) / r, 0.5, 1e-9, "inversely proportional to the VIX points per 100 bp");
    let (r0, r83, r95) = (resp(Config { fin_factor_rho: 0.0, ..base.clone() }, 5.0), resp(base.clone(), 5.0), resp(Config { fin_factor_rho: 0.95, ..base }, 5.0));
    assert!(r0.abs() < r83.abs() && r83.abs() < r95.abs(), "persistence of the factor channel: {r0} {r83} {r95}");
}

#[test]
fn rv_projected_addend_equals_the_merged_arithmetic() {
    // A decaying addend A d^k given as `dollar_proj` (delta 0) or as a delta part with the Dollar decay (proj 0) must equal the
    // merged arithmetic with `dollar = A` and `dollar_decay = d`, month by month.
    let cfg = lcfg(0, 300);
    let (a_lvl, dec) = (0.02, 0.9f64);
    let mut merged = WorldBlock::new(&cfg);
    let mut by_proj = WorldBlock::new(&cfg);
    let mut by_delta = WorldBlock::new(&cfg);
    let mut um = UsInputs::zero();
    um.dollar = a_lvl;
    um.dollar_decay = dec;
    let mut up = UsInputs::zero();
    up.dollar = a_lvl;
    up.proj_on = true;
    for k in 1..=K {
        up.dollar_proj[k - 1] = a_lvl * dec.powi(k as i32);
    }
    let mut ud = UsInputs::zero();
    ud.dollar = a_lvl;
    ud.dollar_decay = dec;
    ud.proj_on = true;
    ud.dollar_delta = a_lvl;
    for t in 0..4 {
        let xm = merged.step_inputs(&um, None, false);
        let xp = by_proj.step_inputs(&up, None, false);
        let xd = by_delta.step_inputs(&ud, None, false);
        for i in 0..NB {
            for k in 0..5 {
                close(xm[i][k], xp[i][k], 1e-13, &format!("proj month {t} bloc {i} out {k}"));
                close(xm[i][k], xd[i][k], 1e-13, &format!("delta month {t} bloc {i} out {k}"));
            }
        }
    }
}

#[test]
fn rv_trade_dollar_addend_is_in_percent() {
    // At the base with every bilateral addend 0.01 (1 %), z~_j = z_j + 1.0 and the dominant-currency state moves by
    // speed 0.08 x kappa 0.47 x 1.0 in its first update.
    let mut w = sim::new_world(&lcfg(12, 300), 3);
    let blocs = w.world.as_ref().unwrap().blocs.clone();
    let mut tr = w.world.as_mut().unwrap().trade.take().unwrap();
    let a = [0.01; NB];
    let _ = tr.post_bloc(&w, &blocs, Some(&a));
    for j in 0..NB {
        close(tr.z_tilde[j], blocs[j].hist.z[0] + 1.0, 1e-12, "z~ = z + 100 a");
        close(tr.phi_d[j], 0.08 * 0.47 * 1.0, 1e-12, "phi_d first update");
    }
}

#[test]
fn rv_step_timed_skips_exchange_rate_like_step() {
    // The profiling path has its own closure for hook H2: both paths must give the same broad rate and premium states.
    let mut a = sim::new_world(&lcfg(12, 300), 5);
    let mut b = sim::new_world(&lcfg(12, 300), 5);
    let mut t = [0.0f64; 21];
    for m in 0..6 {
        sim::step(&mut a);
        sim::step_timed(&mut b, &mut t);
        assert_eq!(a.row.fx.to_bits(), b.row.fx.to_bits(), "fx month {m}");
        assert_eq!(a.fx_psi_state.to_bits(), b.fx_psi_state.to_bits(), "fx_psi_state month {m}");
    }
}
