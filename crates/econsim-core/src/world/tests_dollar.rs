//! Stage 4-1 tests (spec 2.6, 2.7, 1.6b, 3.0; the 25 of section 5 of the stage's implementer task description). The hand values are the ones of spec
//! 2.6 and 2.7. Worlds that test the burn-in use `sim::new_world` (the burn-in runs there and the tick is rebased) or a tiny world
//! with `burn_in_months` set by hand (`testkit::tiny_world` sets it to zero).

use super::bloc::OUT_Z;
use super::dollar::{cred_profile, DollarState};
use super::finance::frisk_step;
use super::net::UsInputs;
use super::{dollar_active, init_if_enabled, WorldBlock, K, NB};
use crate::config::Config;
use crate::exog::{self, FinSpec};
use crate::rng::{self, normal, stream};
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::testkit;
use crate::types::World;

/// The stage-4 switches on with the profile's f-process keys (stage 4-1 review, D3: without them `f_t` is identically zero, and so
/// are Frisk, g and Fex); `households` and the burn-in are the test's.
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

/// The dollar state of a world whose block exists.
fn dstate(w: &World) -> &DollarState {
    w.world.as_ref().unwrap().dollar.as_ref().unwrap()
}

/// A tiny world with the block (and the dollar link when `link`), burn-in set by hand.
fn tiny(burn: u32, link: bool, seed: u64) -> World {
    let mut w = testkit::tiny_world(Config { world_block_enabled: true, world_link_dollar: link, n_households: 300, fin_factor_rho: 0.83, ..Config::default() }, seed);
    w.cfg.burn_in_months = burn;
    init_if_enabled(&mut w);
    w
}

#[test]
fn dollar_premium_matches_exchange_rate() {
    // Spec 2.6: phase A makes the calls and state updates of `fx::exchange_rate`. With the same exogenous path the premium states
    // of the link-on world equal those of the link-off world every month (the link acts on the broad rate only). Hand values with
    // psi 0 and M-X 1.25: z_0 = 0.012057 (`1.25 x 0.03 tanh(0.01/0.03)`), qpsi_0 = 0.72 z_0 = 0.008681.
    for (mp, sigma) in [(0.0, 0.0215), (1.25, 0.0215), (1.25, 0.0)] {
        let base = Config { burn_in_months: 0, n_households: 300, world_block_enabled: true, fx_psi_sigma: sigma, fx_mp_premium: mp, ..Config::default() };
        let mut a = sim::new_world(&base, 7);
        let mut b = sim::new_world(&Config { world_link_dollar: true, ..base.clone() }, 7);
        let shock = ScheduledShock { at_month: 2, shock: Shock::PolicyShift { bps: 100.0, months: 12 } };
        a.shocks.schedule(shock.clone());
        b.shocks.schedule(shock);
        for t in 0..24 {
            sim::step(&mut a);
            sim::step(&mut b);
            close(a.fx_psi_component, b.fx_psi_component, 1e-15, "fx_psi_component");
            close(a.fx_psi_state, b.fx_psi_state, 1e-15, "fx_psi_state");
            close(a.monetary.fx_mp_prev, b.monetary.fx_mp_prev, 1e-15, "fx_mp_prev");
            if t == 2 && sigma == 0.0 && mp > 0.0 {
                close(b.fx_psi_component, 0.72 * 0.012057, 2e-6, "qpsi_0");
                close(b.monetary.fx_mp_prev, 0.012057, 2e-6, "z_0");
            }
        }
    }
}

#[test]
fn dollar_active_flag_and_exchange_rate_skip() {
    // Off in burn-in, on from the first reported month; never on with the link or the block off.
    let mut w = tiny(5, true, 3);
    for t in 0..7u32 {
        assert_eq!(dollar_active(&w), t >= 5, "month {t}");
        sim::step(&mut w);
    }
    let off = tiny(0, false, 3);
    assert!(!dollar_active(&off));
    let no_block = testkit::tiny_world(Config { world_block_enabled: false, world_link_dollar: true, n_households: 300, ..Config::default() }, 3);
    assert!(no_block.world.is_none() && !dollar_active(&no_block), "block off: never active");
}

#[test]
fn dollar_base_captured_at_last_burn_in_month() {
    // Burn-in 12: the base exists after the burn-in and equals the values of that month; later months never recapture it.
    let w = sim::new_world(&lcfg(12, 300), 3);
    let d = dstate(&w);
    let b = d.base.clone().expect("base after burn-in");
    close(b.fx0, w.row.fx, 1e-12, "fx0 is the broad rate of the last burn-in month");
    let mut w = w;
    for _ in 0..3 {
        sim::step(&mut w);
    }
    let b2 = dstate(&w).base.clone().unwrap();
    close(b2.fx0, b.fx0, 0.0, "fx0 unchanged after reported months");
    close(b2.frisk0, b.frisk0, 0.0, "frisk0 unchanged");
    for j in 0..NB {
        close(b2.z[j], b.z[j], 0.0, "z_b unchanged");
    }
}

#[test]
fn burn_in_zero_world_dollar_base_at_first_step() {
    let mut w = tiny(0, true, 4);
    assert!(dstate(&w).base.is_none(), "no base before the first step");
    sim::step(&mut w);
    assert!(dstate(&w).base.is_some(), "base at the first step");
}

#[test]
fn dollar_first_month_is_continuous() {
    // Spec 1.6b / W4-29: the first reported month moves the broad rate by at most a few percent from the base (8 seeds, burn-in 24).
    let mut sum = 0.0;
    let mut worst: f64 = 0.0;
    for seed in 1..=8u64 {
        let mut w = sim::new_world(&lcfg(24, 500), seed);
        let fx0 = dstate(&w).base.as_ref().unwrap().fx0;
        sim::step(&mut w);
        let d = (w.row.fx / fx0).ln().abs();
        sum += d;
        worst = worst.max(d);
    }
    assert!(sum / 8.0 < 0.025, "mean |ln(fx_0/fx_b)| {}", sum / 8.0);
    assert!(worst < 0.07, "worst |ln(fx_0/fx_b)| {worst}");
}

#[test]
fn dollar_pair_identical_before_shock() {
    // Two worlds on the same seed: a shock at reported month 4 leaves the broad rate identical before it (1.6b).
    let mut a = sim::new_world(&lcfg(12, 300), 5);
    let mut b = sim::new_world(&lcfg(12, 300), 5);
    b.shocks.schedule(ScheduledShock { at_month: 4, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    for t in 0..8 {
        sim::step(&mut a);
        sim::step(&mut b);
        if t < 4 {
            assert_eq!(a.row.fx.to_bits(), b.row.fx.to_bits(), "month {t} differs before the shock");
        }
    }
    assert!((a.row.fx - b.row.fx).abs() > 0.0, "shock moves the rate after month 4");
}

#[test]
fn dollar_pair_identical_before_shock_through_compare() {
    // Stage 4-1 review (S4-R14): the pair the server compares. `sim::compare` with the server's default warm-up (24 months): the
    // shocked and the unshocked arms are one world until the shock month, so every metric of every month before it is identical,
    // with the link on. The trade link is on as in the measured profile (the server refuses the dollar link without it).
    let sc = crate::sim::Scenario {
        config: Config { warmup_months: 24, world_link_trade: true, ..lcfg(144, 300) },
        shocks: vec![ScheduledShock { at_month: 6, shock: Shock::PolicyShift { bps: 100.0, months: 12 } }],
        months: 12,
        seeds: vec![1, 2],
    };
    let c = sim::compare(&sc);
    assert_eq!(c.warmup_months, 24);
    assert_eq!(c.baseline.len(), 12);
    assert_eq!(c.shocked.len(), 12);
    for t in 0..6 {
        let a = serde_json::to_string(&c.baseline[t]).unwrap();
        let b = serde_json::to_string(&c.shocked[t]).unwrap();
        assert_eq!(a, b, "month {t}: the arms differ before the shock");
    }
    assert!((6..12).any(|t| c.shocked[t].policy_rate != c.baseline[t].policy_rate), "the shock moves the policy rate from month 6");
}

#[test]
fn burn_in_zero_fx_base_captured_in_dollar_mode() {
    // Stage 4-1 review (D9): hook H2 skips `exchange_rate` in reported months with the link on, so the FX base is captured there
    // (`fx::capture_fx_base`). With no burn-in, the link-on world holds the base the link-off world captures in `exchange_rate` at the
    // same point of the first month; with the initial prices at 1.0 that base is the neutral one.
    let mut on = sim::new_world(&lcfg(0, 300), 3);
    let mut off = sim::new_world(&Config { world_link_dollar: false, ..lcfg(0, 300) }, 3);
    assert!(on.monetary.fx_base.is_none() && off.monetary.fx_base.is_none(), "no base before the first step");
    sim::step(&mut on);
    sim::step(&mut off);
    let a = on.monetary.fx_base.expect("base captured in dollar mode");
    let b = off.monetary.fx_base.expect("base captured by exchange_rate");
    assert_eq!(a.import_price.to_bits(), b.import_price.to_bits(), "import price base");
    for s in 0..a.terms.len() {
        assert_eq!(a.terms[s].to_bits(), b.terms[s].to_bits(), "terms base, sector {s}");
    }
    assert_eq!(a.import_price, 1.0, "neutral import price");
    assert!(a.terms.iter().all(|&t| t == 1.0), "neutral terms");
}

#[test]
fn world_link_checks_refuse_the_unsupported_values() {
    // Stage 4-1 review (D7, S4-R14): the dollar link needs the trade link. Stage 4-2 (list A', S4-R22, D2): the finance ranges, with
    // the defaults inside them and each bound two-sided. The server and the CLI apply these before they run a config.
    assert!(Config { world_link_dollar: true, world_link_trade: true, ..Config::default() }.validate_world_links().is_ok());
    assert!(Config { world_link_dollar: false, world_link_trade: false, ..Config::default() }.validate_world_links().is_ok());
    let e = Config { world_link_dollar: true, world_link_trade: false, ..Config::default() }.validate_world_links().unwrap_err();
    assert!(e.contains("world_link_trade"), "{e}");
    assert!(Config::default().validate_world_finance().is_ok(), "the defaults are in range");
    let bad = [
        (Config { world_fo_trans_rho: 1.0, ..Config::default() }, "world_fo_trans_rho"),
        (Config { world_fo_trans_rho: -0.1, ..Config::default() }, "world_fo_trans_rho"),
        (Config { world_fo_theta_stock: 1e9, ..Config::default() }, "world_fo_theta_stock"),
        (Config { world_fo_theta_trans: -1.0, ..Config::default() }, "world_fo_theta_trans"),
        (Config { world_fo_dollar_resp: -5.0, ..Config::default() }, "world_fo_dollar_resp"),
        (Config { world_fo_vix_resp: 11.0, ..Config::default() }, "world_fo_vix_resp"),
        (Config { world_fo_trend_cn: -501.0, ..Config::default() }, "world_fo_trend_cn"),
        (Config { world_gdp_rw_bn: 0.0, ..Config::default() }, "world_gdp_rw_bn"),
        (Config { world_gdp_rw_bn: 1.0e9, ..Config::default() }, "world_gdp_rw_bn"),
        (Config { world_gdp_rw_bn: f64::NAN, ..Config::default() }, "world_gdp_rw_bn"),
    ];
    for (cfg, key) in &bad {
        let e = cfg.validate_world_links().unwrap_err();
        assert!(e.contains(key), "{key}: {e}");
    }
}

#[test]
fn dollar_psi_zero_through_burn_in() {
    // Spec 2.6: the premium states are exactly zero at the end of the burn-in (anchored premium and M-X surprise are zero there).
    let w = sim::new_world(&lcfg(12, 300), 6);
    assert_eq!(w.fx_psi_state, 0.0);
    assert_eq!(w.fx_psi_component, 0.0);
    assert_eq!(w.monetary.fx_mp_prev, 0.0);
}

#[test]
fn frisk_runs_in_shadow_and_is_based() {
    // Spec 2.6 / 2.7: the risk factor filter run from zero through 144 burn-in months and 124 reported months, 300 seeds. The mean of
    // Frisk_t - Frisk_b over reported months 0-24 is within 1.5 VIX points of its mean over months 100-124.
    let spec = FinSpec { rho: 0.83, sigma: 0.30, gamma: 0.0, jump_intensity: 0.06, jump_mean: 1.3, jump_sd: 0.5 };
    let (mut early, mut late) = (0.0, 0.0);
    let n = 300u64;
    for seed in 1..=n {
        let mut f = 0.0;
        let mut f_prev = 0.0;
        let mut fr = 0.0;
        let mut path = Vec::with_capacity(269);
        for tick in 0..269u32 {
            let mut rf = rng::module_rng(seed, tick, stream::FIN_FACTOR);
            let eps = normal(&mut rf);
            let (next, _, _) = exog::fin_factor_step(f, eps, &spec, &mut rf);
            f = next;
            let big_f = 10.0 * f;
            fr = frisk_step(fr, f_prev, big_f, 0.83, 0.588, 0.9);
            f_prev = big_f;
            path.push(fr);
        }
        let base = path[143];
        early += (0..=24).map(|m| path[144 + m] - base).sum::<f64>() / 25.0;
        late += (100..=124).map(|m| path[144 + m] - base).sum::<f64>() / 25.0;
    }
    early /= n as f64;
    late /= n as f64;
    assert!((early - late).abs() < 1.5, "early {early:.3} late {late:.3}");
}

#[test]
fn frisk_asymmetry_and_decay() {
    // Falls are scaled by 0.588, rises are not; with the factor decaying at rho the risk factor decays at risk_rho.
    let (rho, asym, rr) = (0.83, 0.588, 0.9);
    close(frisk_step(0.0, 10.0, 0.0, rho, asym, rr), asym * (-8.3), 1e-12, "fall");
    close(frisk_step(0.0, 0.0, 10.0, rho, asym, rr), 10.0, 1e-12, "rise");
    let mut fr = 10.0;
    let mut f = 10.0;
    for k in 1..=5 {
        let next = rho * f;
        fr = frisk_step(fr, f, next, rho, asym, rr);
        f = next;
        close(fr, 10.0 * rr.powi(k), 1e-12, "decay");
    }
}

#[test]
fn fin_factor_moments_with_world_profile() {
    // Spec 2.7 (W4-05 inputs): AR slope 0.83; P(F rise >= 10 points in a month) 0.039 +/- 0.003 over 300,000 months; mean F 4.6.
    let spec = FinSpec { rho: 0.83, sigma: 0.30, gamma: 0.0, jump_intensity: 0.06, jump_mean: 1.3, jump_sd: 0.5 };
    let (mut rises, mut months) = (0u64, 0u64);
    let (mut sx, mut sy, mut sxy, mut sxx, mut sum) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for seed in 1..=300u64 {
        let mut f = 0.0;
        for tick in 0..1144u32 {
            let mut rf = rng::module_rng(seed, tick, stream::FIN_FACTOR);
            let eps = normal(&mut rf);
            let (next, _, _) = exog::fin_factor_step(f, eps, &spec, &mut rf);
            if tick >= 144 {
                // F = 10 f in VIX points; a rise of 10 points is a change of 1.0 in f.
                if 10.0 * (next - f) >= 10.0 {
                    rises += 1;
                }
                months += 1;
                sx += f;
                sy += next;
                sxy += f * next;
                sxx += f * f;
                sum += next;
            }
            f = next;
        }
    }
    let p = rises as f64 / months as f64;
    close(p, 0.039, 0.003, "P(rise >= 10 points)");
    let nm = months as f64;
    let slope = (nm * sxy - sx * sy) / (nm * sxx - sx * sx);
    close(slope, 0.83, 0.01, "AR slope");
    close(10.0 * sum / nm, 4.6, 0.3, "mean F in VIX points");
}

#[test]
fn policy_surprise_adds_13_points() {
    // PolicyShift 100 bp at month 2: F rises by 13 VIX points (f by 1.3) in that month only (the factor then decays at 0.83); Fpol
    // rises by 13 and decays at 0.83 a month. A second PolicyShift overwrites the slot and injects its own bps.
    let mut a = sim::new_world(&lcfg(12, 300), 4);
    let mut b = sim::new_world(&lcfg(12, 300), 4);
    a.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    a.shocks.schedule(ScheduledShock { at_month: 5, shock: Shock::PolicyShift { bps: 50.0, months: 12 } });
    for t in 0..7u32 {
        sim::step(&mut a);
        sim::step(&mut b);
        let (da, db) = (dstate(&a), dstate(&b));
        match t {
            2 => {
                close(da.fpol, 13.0, 1e-9, "fpol at the shock month");
                close(da.f_now - db.f_now, 13.0, 1e-9, "F rises 13 points");
            }
            3 => {
                close(da.fpol, 13.0 * 0.83, 1e-9, "fpol decays at 0.83");
                close(da.f_now - db.f_now, 13.0 * 0.83, 1e-9, "F decays with the factor");
            }
            // Month 5: the decayed first surprise (13 x 0.83^3 after the decay) plus the second shift's 6.5 points.
            5 => close(da.fpol, 13.0 * 0.83f64.powi(3) + 6.5, 1e-9, "second shift overwrites the slot"),
            _ => {}
        }
    }
    // The second shift injects 50 bp = 6.5 points in month 5 alone.
    let mut c = sim::new_world(&lcfg(12, 300), 4);
    c.shocks.schedule(ScheduledShock { at_month: 5, shock: Shock::PolicyShift { bps: 50.0, months: 12 } });
    for _ in 0..6 {
        sim::step(&mut c);
    }
    close(dstate(&c).fpol, 6.5, 1e-9, "second shift injects its own bps");
}

#[test]
fn fpol_excluded_from_bloc_demand() {
    // The policy surprise is not in Fex: the non-policy part of F is the same with and without the surprise (2.7).
    let mut a = sim::new_world(&lcfg(12, 300), 4);
    let mut b = sim::new_world(&lcfg(12, 300), 4);
    a.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    for t in 0..4u32 {
        sim::step(&mut a);
        sim::step(&mut b);
        let fa = a.world.as_ref().unwrap().last.as_ref().unwrap().us.fex;
        let fb = b.world.as_ref().unwrap().last.as_ref().unwrap().us.fex;
        if t == 2 {
            close(fa, fb, 1e-9, "fex with and without the surprise");
        }
    }
}

#[test]
fn fex_term_in_is_equation() {
    // With fex zero the block is bit-identical whether or not the link is on (the term is skipped); with fex positive the IS
    // demand falls, so the impact output of the bloc falls.
    let on = WorldBlock::new(&lcfg(0, 300));
    let off = WorldBlock::new(&Config { world_link_dollar: false, ..lcfg(0, 300) });
    let mut a = on.clone();
    let mut b = off.clone();
    let us = UsInputs::zero();
    let xa = a.step_inputs(&us, None, false);
    let xb = b.step_inputs(&us, None, false);
    for i in 0..NB {
        for k in 0..5 {
            assert_eq!(xa[i][k].to_bits(), xb[i][k].to_bits(), "bloc {i} output {k}");
        }
    }
    let mut c = WorldBlock::new(&lcfg(0, 300));
    let mut us_pos = UsInputs::zero();
    us_pos.fex = 5.0;
    let xc = c.step_inputs(&us_pos, None, false);
    assert!(xc[0][0] < xa[0][0], "positive fex lowers EA output: {} vs {}", xc[0][0], xa[0][0]);
}

#[test]
fn riskoff_event_path() {
    // RiskOff 14 at month 2: the F difference is 14 points in that month (the risk factor moves by the innovation difference), and
    // from month 3 the risk-factor difference decays at world_fx_risk_rho.
    let mut a = sim::new_world(&lcfg(12, 300), 4);
    let mut b = sim::new_world(&lcfg(12, 300), 4);
    a.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::RiskOff { vix_points: 14.0 } });
    let (mut d_prev, mut f_prev_b, mut fr_prev_b) = (0.0, 0.0, 0.0);
    for t in 0..5u32 {
        sim::step(&mut a);
        sim::step(&mut b);
        let (da, db) = (dstate(&a), dstate(&b));
        if t == 2 {
            close(da.f_now - db.f_now, 14.0, 1e-9, "F +14");
            let e_b = db.f_now - 0.83 * f_prev_b;
            let expect = frisk_step(fr_prev_b, f_prev_b, db.f_now + 14.0, 0.83, 0.588, 0.9) - db.frisk;
            close(da.frisk - db.frisk, expect, 1e-9, "risk-factor step with the innovation +14");
            let _ = e_b;
        }
        if t == 3 {
            close(da.frisk - db.frisk, 0.9 * d_prev, 1e-9, "decays at risk_rho");
        }
        d_prev = da.frisk - db.frisk;
        f_prev_b = db.f_now;
        fr_prev_b = db.frisk;
    }
}

#[test]
fn riskoff_injects_exactly_vix_points() {
    // The injection into the financial factor is 14 VIX points = 1.4 units, exactly, in the month it fires.
    let mut a = sim::new_world(&lcfg(12, 300), 4);
    let mut b = sim::new_world(&lcfg(12, 300), 4);
    a.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::RiskOff { vix_points: 14.0 } });
    for t in 0..3u32 {
        sim::step(&mut a);
        sim::step(&mut b);
        if t == 2 {
            close(a.exog.fin_factor - b.exog.fin_factor, 1.4, 1e-12, "fin_factor +1.4 units");
            close(dstate(&a).fpol, dstate(&b).fpol, 0.0, "a risk-off is not policy");
        }
    }
}

#[test]
fn dollar_bilateral_sum_to_broad() {
    // ln D = sum_j wH10_j lnS_j, fx = fx_0 exp(ln D), the weights sum to one, and s_bil = exp(lnS).
    let mut w = sim::new_world(&lcfg(12, 300), 8);
    for _ in 0..6 {
        sim::step(&mut w);
    }
    let d = dstate(&w);
    let wh10 = d.wh10;
    close(wh10.iter().sum::<f64>(), 1.0, 1e-12, "weights sum to one");
    let ln_d: f64 = (0..NB).map(|j| wh10[j] * d.ln_s_now[j]).sum();
    let fx0 = d.base.as_ref().unwrap().fx0;
    close(w.row.fx, fx0 * ln_d.exp(), 1e-12 * fx0, "fx = fx_0 exp(ln D)");
    for j in 0..NB {
        close(w.world.as_ref().unwrap().diag.s_bil[j], d.ln_s_now[j].exp(), 1e-15, "s_bil");
    }
}

#[test]
fn dollar_addend_seen_by_blocs() {
    // With proj_on, a dollar_delta equal to the shock level and a zero projection, the blocs see the merged arithmetic to 1e-15.
    let cfg = lcfg(0, 300);
    let mut a = WorldBlock::new(&cfg);
    let mut b = WorldBlock::new(&cfg);
    let mut us_a = UsInputs::zero();
    us_a.dollar = 0.02;
    let mut us_b = us_a.clone();
    us_b.proj_on = true;
    us_b.dollar_delta = 0.02;
    for t in 0..4 {
        let xa = a.step_inputs(&us_a, None, false);
        let xb = b.step_inputs(&us_b, None, false);
        for i in 0..NB {
            for k in 0..5 {
                close(xa[i][k], xb[i][k], 1e-15, &format!("month {t} bloc {i} output {k}"));
            }
        }
    }
}

#[test]
fn credibility_dollar_hand_value() {
    // Spec 2.6: 100 cred_0 = 100 ln(0.967) = -3.3557 (exact); through the blocs the broad index is -3.327 +/- 0.01 (the weighted bloc
    // z moves +0.0285 in the impact month, with the exact projection of the credibility addend).
    let mut d = DollarState::new(&lcfg(0, 300), &crate::world::params::WorldParams::load(), [0.0; NB]);
    d.add_cred(3.3, 3, 0);
    let cred0 = d.cred.iter().map(|c| c.ln_dollar).sum::<f64>() * cred_profile(0, 3);
    close(100.0 * cred0, 100.0 * (0.967f64).ln(), 1e-9, "100 cred_0");
    close(100.0 * cred0, -3.3557, 1e-4, "100 cred_0 rounded");
    let cfg = Config { world_link_dollar: true, fx_psi_sigma: 0.0, fx_mp_premium: 0.0, world_shock_scale: 0.0, world_fx_idio_sd_ae: 0.0, world_fx_idio_sd_eme: 0.0, world_fx_idio_sd_cn: 0.0, world_fx_idio_sd_opec: 0.0, ..lcfg(0, 300) };
    let mut wb = WorldBlock::new(&cfg);
    let wh10 = wb.trade.as_ref().unwrap().tab.dollar_w;
    let mut us = UsInputs::zero();
    us.dollar = cred0;
    us.proj_on = true;
    for k in 1..=K {
        us.dollar_proj[k - 1] = (0.967f64).ln() * cred_profile(k as u32, 3);
    }
    let x = wb.step_inputs(&us, None, false);
    let zw: f64 = (0..NB).map(|j| wh10[j] * x[j][OUT_Z]).sum();
    let ln_d_100 = 100.0 * cred0 + zw;
    assert!((ln_d_100 - (-3.327)).abs() < 0.01, "100 ln D_0 = {ln_d_100}");
}

#[test]
fn credibility_profile_hold_then_halflife() {
    // p = 1 for `months` months (ages 0..months-1), then 0.5^((age - months + 1)/3).
    close(cred_profile(0, 3), 1.0, 0.0, "age 0");
    close(cred_profile(2, 3), 1.0, 0.0, "age 2");
    close(cred_profile(3, 3), 0.5f64.powf(1.0 / 3.0), 1e-12, "age 3");
    close(cred_profile(5, 3), 0.5, 1e-12, "age 5");
    close(cred_profile(8, 3), 0.25, 1e-12, "age 8");
}

#[test]
fn credibility_shock_sign() {
    // A US credibility shock is a dollar fall: the dollar term is negative; a zero shock adds nothing. Through the schedule the
    // component is registered at its firing month on the bloc clock (burn-in 12 plus the reported month 3).
    let mut d = DollarState::new(&lcfg(0, 300), &crate::world::params::WorldParams::load(), [0.0; NB]);
    d.add_cred(3.3, 3, 0);
    d.add_cred(0.0, 3, 0);
    assert!(d.cred_at(0, 0) < 0.0, "dollar term negative");
    close(d.cred[1].ln_dollar, 0.0, 0.0, "zero shock");
    let mut w = sim::new_world(&lcfg(12, 300), 2);
    w.shocks.schedule(ScheduledShock { at_month: 3, shock: Shock::UsCredibility { tp_bp: 22.0, months: 3, dollar_pct: 3.3 } });
    for _ in 0..4 {
        sim::step(&mut w);
    }
    let cr = &dstate(&w).cred;
    assert_eq!(cr.len(), 1, "one component");
    assert!(cr[0].ln_dollar < 0.0);
    assert_eq!(cr[0].t0, 12 + 3, "firing month on the bloc clock");
}

#[test]
fn yen_gamma_small_no_haven() {
    // Spec W4-03: the yen's loading is small and negative, so a 14-point VIX jump moves the yen by about -0.08 %, inside [-3, 1] %.
    let wp = crate::world::params::WorldParams::load();
    let gamma_jp = wp.finance.gamma_pct_per_vix[2];
    close(gamma_jp, -0.006, 1e-12, "gamma JP");
    let move_pct = gamma_jp / 100.0 * 14.0 * 100.0;
    assert!(move_pct > -3.0 && move_pct < 1.0, "yen move {move_pct} %");
}

#[test]
fn zeta_noise_ar1_sd() {
    // Spec 2.6: zeta is an AR(1) with persistence world_fx_idio_rho and innovation sd sd_j (% per month): its unconditional sd is
    // sd / sqrt(1 - rho^2): 10.6 % for an AE innovation of 1.5 and 14.2 % for an EME innovation of 2.0. Pooled over 20 seeds of
    // 5000 months (one path of 5000 months at rho 0.99 has about 25 effective observations, too few for a 0.8 tolerance).
    let rho = 0.99;
    for (j, sd, want) in [(0usize, 1.5f64, 10.6f64), (5usize, 2.0f64, 14.2f64)] {
        let (mut s1, mut s2, mut count) = (0.0, 0.0, 0.0);
        for seed in 1..=20u64 {
            let mut z = 0.0;
            for tick in 0..5000u32 {
                let mut r = rng::agent_rng(seed, tick, stream::WORLD_FX, j as u64);
                let e = normal(&mut r);
                z = rho * z + sd / 100.0 * e;
                s1 += z * 100.0;
                s2 += (z * 100.0) * (z * 100.0);
                count += 1.0;
            }
        }
        let m = s1 / count;
        let sdv = (s2 / count - m * m).sqrt();
        assert!((sdv - want).abs() < 0.8, "bloc {j}: sd {sdv} vs {want}");
    }
}

#[test]
fn trade_ztilde_dollar_mode_zero_at_base() {
    // Dollar mode with zero addends at the base: z~_j = z_j and the change since the base is zero, so the trade prices do not move.
    let mut w = sim::new_world(&lcfg(12, 300), 3);
    let blocs = w.world.as_ref().unwrap().blocs.clone();
    let mut tr = w.world.as_mut().unwrap().trade.take().unwrap();
    let zero = [0.0; NB];
    let _ = tr.post_bloc(&w, &blocs, Some(&zero));
    for j in 0..NB {
        close(tr.z_tilde[j], blocs[j].hist.z[0], 1e-12, "z~ = z at the base");
        close(tr.phi_d[j], 0.0, 1e-12, "phi_d stays at zero");
    }
}

#[test]
fn fx_base_unchanged_by_dollar_link() {
    // The ABM's FX base (`monetary.fx_base`, captured in burn-in) and the terms it holds are not touched by the link.
    let mut w = sim::new_world(&lcfg(12, 300), 2);
    let before = w.monetary.fx_base.expect("fx base after burn-in");
    for _ in 0..6 {
        sim::step(&mut w);
    }
    let after = w.monetary.fx_base.expect("fx base kept");
    close(after.import_price, before.import_price, 0.0, "import price base");
    for s in 0..before.terms.len() {
        close(after.terms[s], before.terms[s], 0.0, "terms base");
    }
}

#[test]
fn world_off_and_links_off_identical() {
    // Block off and block on with every stage-4 switch off: every non-world key of each month is identical (24 months).
    let off = Config { world_block_enabled: false, n_households: 300, burn_in_months: 0, ..Config::default() };
    let on = Config { world_block_enabled: true, world_link_dollar: false, ..off.clone() };
    let mut a = testkit::tiny_world(off, 11);
    let mut b = testkit::tiny_world(on, 11);
    for t in 0..24 {
        sim::step(&mut a);
        sim::step(&mut b);
        let mut va = serde_json::to_value(a.history.last().unwrap()).unwrap();
        let mut vb = serde_json::to_value(b.history.last().unwrap()).unwrap();
        va.as_object_mut().unwrap().remove("world");
        vb.as_object_mut().unwrap().remove("world");
        assert_eq!(va, vb, "month {t}: non-world keys differ");
    }
}
