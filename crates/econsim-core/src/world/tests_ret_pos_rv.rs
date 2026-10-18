//! Reviewer's replacement tests for sub-round 4-3 (section 6 of the stage 4-3 review note, not published). Each expectation is computed here from the
//! research numbers, the spec formulas or states the code under test does not compute in the same step. They kill the mutants that
//! survived the 28 tests of `tests_ret_pos.rs` (switch-off with a flagged duty, the stacked-duty cap, the goods allocation, the by-bloc
//! current account, the noise stream in `update`, the metric units, the shares, the boundaries of the validation, the dollar-on increments).

use super::positions::{ETA_SD, POS_NOISE_BASE};
use super::trade::{RetState, TariffComp, RET_GOODS, TAU_CAP};
use super::NB;
use crate::config::Config;
use crate::rng::{self, normal, stream};
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::World;

const CN: usize = 1;

fn rcfg(burn: u32, hh: usize) -> Config {
    Config {
        burn_in_months: burn,
        n_households: hh,
        world_block_enabled: true,
        world_link_trade: true,
        world_retaliation: true,
        world_positions: true,
        fin_factor_rho: 0.83,
        fin_factor_sigma: 0.30,
        fin_jump_intensity: 0.06,
        fin_jump_mean: 1.3,
        fin_jump_sd: 0.5,
        erp_fin_loading: 0.0028,
        ..Config::default()
    }
}

fn tr(w: &World) -> &super::trade::TradeState {
    w.world.as_ref().unwrap().trade.as_ref().unwrap()
}

fn pos(w: &World) -> &super::positions::PositionsState {
    w.world.as_ref().unwrap().pos.as_ref().unwrap()
}

fn json(m: &crate::metrics::Metrics) -> serde_json::Value {
    serde_json::to_value(m).unwrap()
}

fn cn_tariff(pct: f64, retaliate: bool) -> Shock {
    Shock::Tariff { partner: Some("CN".into()), sector: None, pct, retaliate }
}

#[test]
fn rv_switch_off_a_flagged_tariff_retaliates_nothing() {
    // W4-37: "with the switch off a flagged Tariff retaliates nothing". Flagged against unflagged, switch off: every key of every record
    // equal; with the switch on the same pair differs in the goods exports to China (so the comparison is not vacuous).
    let run = |ret: bool, flag: bool| {
        let c = Config { world_retaliation: ret, ..rcfg(12, 300) };
        let mut w = sim::new_world(&c, 7);
        w.shocks.schedule(ScheduledShock { at_month: 1, shock: cn_tariff(15.0, flag) });
        for _ in 0..8 {
            sim::step(&mut w);
        }
        w
    };
    let (a, b) = (run(false, true), run(false, false));
    assert!(!tr(&a).ret.on && tr(&a).ret.r.iter().all(|r| r.iter().all(|&x| x.to_bits() == 1.0f64.to_bits())));
    for (ma, mb) in a.history.iter().zip(b.history.iter()) {
        assert_eq!(json(ma), json(mb));
    }
    let (c, d) = (run(true, true), run(true, false));
    let (gc, gd) = (c.world.as_ref().unwrap().diag.exports_goods_bn[CN], d.world.as_ref().unwrap().diag.exports_goods_bn[CN]);
    assert!(gc < 0.9 * gd, "retaliation on: {gc} against {gd}");
}

#[test]
fn rv_taubar_caps_a_stack_of_components_at_tau_cap() {
    // Three 300-point components on all origins stack to 9.0 in a cell; the average duty the rule reads is capped at TAU_CAP (5.0) per
    // cell, as the b7 duty path caps it. A partner-specific stack and an all-origin stack agree for the named bloc.
    let c = rcfg(0, 300);
    let r = RetState::new(&c, &super::params::WorldParams::load());
    let stack = [TariffComp { partner: None, sector: None, tau: 3.0, retaliate: true }; 3];
    assert!((r.taubar(&stack, CN) - TAU_CAP).abs() < 1e-12, "{}", r.taubar(&stack, CN));
    assert!((r.taubar(&stack, 0) - TAU_CAP).abs() < 1e-12);
    let one = [TariffComp { partner: Some(CN), sector: None, tau: 4.0, retaliate: true }, TariffComp { partner: None, sector: None, tau: 0.5, retaliate: true }];
    assert!((r.taubar(&one, CN) - 4.5).abs() < 1e-12 && (r.taubar(&one, 0) - 0.5).abs() < 1e-12);
}

#[test]
fn rv_goods_exports_are_the_five_goods_sectors_of_the_settled_allocation() {
    // exports_goods_bn[j] = sum over s < 5 of the realised sector exports (w.flows.exports_by_sector) allocated pro rata to x_exp, times the
    // annualisation 12 scale / 1e9 (as exports_bn); recomputed here from the flows and the unsettled x_exp.
    let c = rcfg(12, 300);
    let mut w = sim::new_world(&c, 6);
    w.shocks.schedule(ScheduledShock { at_month: 1, shock: cn_tariff(25.0, true) });
    let ann = 12.0 * c.effective_scale() / 1e9;
    for m in 0..10 {
        sim::step(&mut w);
        let t = tr(&w);
        let d = &w.world.as_ref().unwrap().diag;
        for j in 0..NB {
            let mut goods = 0.0;
            let mut all = 0.0;
            for s in 0..9 {
                let tot: f64 = (0..NB).map(|k| t.x_exp[s][k]).sum();
                let part = if tot > 0.0 { w.flows.exports_by_sector[s] * t.x_exp[s][j] / tot } else { 0.0 };
                all += part;
                if s < RET_GOODS {
                    goods += part;
                }
            }
            assert!((t.exp_goods[j] - goods).abs() <= 1e-12 * goods.abs().max(1e-9), "month {m} bloc {j}");
            assert!((d.exports_goods_bn[j] - goods * ann).abs() <= 1e-9 * (goods * ann).abs().max(1e-9), "month {m} bloc {j}: metric");
            assert!((d.exports_bn[j] - all * ann).abs() <= 1e-9 * (all * ann).abs().max(1e-9), "month {m} bloc {j}: all exports");
        }
        assert!(d.exports_goods_bn[CN] < d.exports_bn[CN], "services exports to China are positive");
    }
}

#[test]
fn rv_bilateral_current_account_is_x_less_m_less_interest_plus_primary_income_by_bloc() {
    // ca_j = x_j - m_j - led_j + pi_j with pi_j from the stocks of t-1 and led_j = cpn tic_j + dep dep_j. The test solves the 12 equations
    // for (cpn, dep) by least squares: a shifted bloc index in x, m or pi, or a wrong share vector, leaves a residual.
    let c = rcfg(12, 400);
    let mut w = sim::new_world(&c, 11);
    for m in 0..14 {
        let st = pos(&w).clone();
        sim::step(&mut w);
        let (p, t) = (pos(&w), tr(&w));
        let (ya, yl) = (st.y_a / 12.0, st.y_l / 12.0);
        let led: Vec<f64> = (0..NB)
            .map(|j| {
                let pi = ya * (st.a_eq[j] + st.a_debt[j] + st.a_fdi[j] + st.a_bank[j]) - yl * (st.l_ag[j] + st.l_eq[j] + st.l_fdi[j]);
                t.exp_bloc[j] - t.imp_bloc[j] + pi - p.ca_j[j]
            })
            .collect();
        if p.updates == 0 {
            continue;
        }
        // normal equations of led = cpn tic + dep dep_share
        let (mut stt, mut std_, mut sdd, mut sl_t, mut sl_d) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for j in 0..NB {
            stt += st.tic_share[j] * st.tic_share[j];
            std_ += st.tic_share[j] * st.dep_share[j];
            sdd += st.dep_share[j] * st.dep_share[j];
            sl_t += st.tic_share[j] * led[j];
            sl_d += st.dep_share[j] * led[j];
        }
        let det = stt * sdd - std_ * std_;
        let cpn = (sl_t * sdd - sl_d * std_) / det;
        let dep = (sl_d * stt - sl_t * std_) / det;
        let tot: f64 = led.iter().sum();
        for j in 0..NB {
            let fit = cpn * st.tic_share[j] + dep * st.dep_share[j];
            assert!((fit - led[j]).abs() <= 1e-9 * tot.abs().max(1e-9), "month {m} bloc {j}: led {} fit {fit}", led[j]);
        }
        assert!(cpn > 0.0 && (cpn + dep - tot).abs() <= 1e-9 * tot.abs(), "month {m}: cpn {cpn} dep {dep} total {tot}");
    }
}

#[test]
fn rv_update_applies_the_spec_noise_stream() {
    // The month's valuation is memo_step(ds, r_us, eta) with eta_j = ETA_SD normal(agent_rng(seed, tick, WORLD_FX, 16 + j)): replayed here
    // on a clone of the stocks of t-1 (a missing eta, one common draw for all blocs or a shifted tick changes the valuation).
    let c = rcfg(12, 300);
    let mut w = sim::new_world(&c, 19);
    for _ in 0..2 {
        sim::step(&mut w);
    }
    for _ in 0..5 {
        let tick = w.tick;
        let mut st = pos(&w).clone();
        sim::step(&mut w);
        let after = pos(&w);
        let eta: [f64; NB] = std::array::from_fn(|j| {
            let mut r = rng::agent_rng(w.seed, tick, stream::WORLD_FX, POS_NOISE_BASE + j as u64);
            ETA_SD * normal(&mut r)
        });
        let out = st.memo_step(&after.ds, after.r_us, &eta);
        assert!((out.val - after.val_memo).abs() <= 1e-12 * out.val.abs().max(1e-6), "tick {tick}: {} vs {}", out.val, after.val_memo);
        for j in 0..NB {
            assert!((st.a_eq[j] - after.a_eq[j]).abs() <= 1e-12 * st.a_eq[j].abs().max(1e-9), "tick {tick} bloc {j}");
        }
    }
}

#[test]
fn rv_world_metrics_are_in_their_stated_units() {
    // niip_pct_gdp = 100 NIIP / (12 gdp), ca_pct_gdp = 100 CA / gdp, ca_bilateral_bn = ca_j 12 scale / 1e9 (gdp is the month `settle` reads:
    // `gdp_month_model` before the month's record is pushed); sfc_world_error = |resid| / gdp, probed by displacing NIIP_{t-1} by 1e-6 gdp
    // (release only: a debug build's check_identities would, rightly, panic).
    let c = rcfg(12, 300);
    let mut w = sim::new_world(&c, 5);
    for _ in 0..3 {
        sim::step(&mut w);
    }
    let ann = 12.0 * c.effective_scale() / 1e9;
    let gdp = crate::common::gdp_month_model(&w);
    sim::step(&mut w);
    let (p, d) = (pos(&w), &w.world.as_ref().unwrap().diag);
    assert!((d.niip_pct_gdp - 100.0 * p.niip / (12.0 * gdp)).abs() <= 1e-9 * d.niip_pct_gdp.abs(), "niip {}", d.niip_pct_gdp);
    assert!((d.ca_pct_gdp - 100.0 * p.ca_total / gdp).abs() <= 1e-9 * d.ca_pct_gdp.abs().max(1e-9), "ca {}", d.ca_pct_gdp);
    for j in 0..NB {
        assert!((d.ca_bilateral_bn[j] - p.ca_j[j] * ann).abs() <= 1e-9 * (p.ca_j[j] * ann).abs().max(1e-9), "bloc {j}");
    }
    assert!(d.niip_pct_gdp < -20.0 && d.niip_pct_gdp > -300.0 && d.ca_pct_gdp.abs() < 20.0, "plausible levels: {} {}", d.niip_pct_gdp, d.ca_pct_gdp);
    if !cfg!(debug_assertions) {
        let gdp = crate::common::gdp_month_model(&w);
        w.world.as_mut().unwrap().pos.as_mut().unwrap().niip_prev += 1.0e-6 * gdp;
        sim::step(&mut w);
        let e = w.world.as_ref().unwrap().diag.sfc_world_error;
        assert!((e - 1.0e-6).abs() < 1.0e-9, "displacement 1e-6 of gdp read back as {e}");
    }
}

#[test]
fn rv_shares_come_from_the_research_rows_and_the_validation_has_closed_boundaries() {
    // TIC holdings (EA 1950.5, CN 618.0, RoW 3115.7 of 9247.9) and BIS liabilities (EA 1119.0, CN 104.2, RoW 1738.1 of 5670.7) by bloc.
    let w = sim::new_world(&rcfg(12, 300), 3);
    let p = pos(&w);
    for (j, bn) in [(0usize, 1950.5), (1, 618.0), (11, 3115.7)] {
        assert!((p.tic_share[j] * 9247.9 - bn).abs() < 0.05, "tic_share[{j}]");
    }
    for (j, bn) in [(0usize, 1119.0), (1, 104.2), (11, 1738.1)] {
        assert!((p.dep_share[j] * 5670.7 - bn).abs() < 0.05, "dep_share[{j}]");
    }
    let ok = |sr: f64, lr: f64, sp: f64| Config { world_ret_eps_sr: sr, world_ret_eps_lr: lr, world_ret_speed: sp, ..Config::default() }.validate_world_links().is_ok();
    assert!(ok(2.0, 2.0, 0.1) && ok(0.0, 10.0, 0.0) && ok(10.0, 10.0, 1.0));
    assert!(!ok(10.1, 10.1, 0.1) && !ok(0.0, 10.1, 0.1) && !ok(0.0, 3.0, 1.01) && !ok(2.0, 1.999, 0.1));
}

#[test]
fn rv_dollar_link_on_increments_weight_to_the_broad_change() {
    // With world_link_dollar on, ln D = sum_j wH10_j ln S_j and fx = fx0 exp(ln D), so the H10-weighted increments the positions use equal
    // the change of ln fx of the month (an increment taken against the wrong reference, e.g. since the base, breaks this from month 2).
    let c = Config { world_link_dollar: true, ..rcfg(12, 300) };
    let mut w = sim::new_world(&c, 14);
    let mut fx_prev = w.row.fx;
    for m in 0..10 {
        sim::step(&mut w);
        let wh10 = w.world.as_ref().unwrap().dollar.as_ref().unwrap().wh10;
        let ds = pos(&w).ds;
        let weighted: f64 = (0..NB).map(|j| wh10[j] * ds[j]).sum();
        let broad = (w.row.fx / fx_prev).ln();
        assert!((weighted - broad).abs() < 1e-9, "month {m}: weighted {weighted} vs broad {broad}");
        fx_prev = w.row.fx;
    }
}

#[test]
fn rv_memo_step_uses_each_blocs_own_dollar_change_loading_and_noise() {
    // The memo tests of the implementer use one dollar change for every bloc, so a bloc index slipped in any term is invisible; here
    // ds, eta differ by bloc and every stock of every bloc is checked against spec 2.8 (fc_eq 0.9, fc_debt 0.23, fc_fdi 1.0, fc_bank 0.3,
    // fc_liab 0.03, yields 3.8 % and 2.55 %, equity loadings of the table).
    let w = sim::new_world(&rcfg(12, 300), 3);
    let before = pos(&w).clone();
    let mut ps = before.clone();
    let ds: [f64; NB] = std::array::from_fn(|j| 0.01 * (j as f64 + 1.0));
    let eta: [f64; NB] = std::array::from_fn(|j| 0.001 * (j as f64 + 1.0));
    ps.memo_step(&ds, 0.02, &eta);
    let (ya, yl) = (0.038 / 12.0, 0.0255 / 12.0);
    let rel = |new: f64, old: f64| if old != 0.0 { new / old } else { 1.0 };
    for j in 0..NB {
        let want_eq = 1.0 + before.load[j] * 0.02 + eta[j] - 0.9 * ds[j] + ya;
        assert!((rel(ps.a_eq[j], before.a_eq[j]) - want_eq).abs() < 1e-12, "a_eq bloc {j}");
        assert!((rel(ps.a_debt[j], before.a_debt[j]) - (1.0 - 0.23 * ds[j] + ya)).abs() < 1e-12, "a_debt bloc {j}");
        assert!((rel(ps.a_fdi[j], before.a_fdi[j]) - (1.0 - 1.0 * ds[j] + ya)).abs() < 1e-12, "a_fdi bloc {j}");
        assert!((rel(ps.a_bank[j], before.a_bank[j]) - (1.0 - 0.3 * ds[j] + ya)).abs() < 1e-12, "a_bank bloc {j}");
        assert!((rel(ps.l_ag[j], before.l_ag[j]) - (1.0 - 0.03 * ds[j] + yl)).abs() < 1e-12, "l_ag bloc {j}");
        assert!((rel(ps.l_eq[j], before.l_eq[j]) - (1.0 + 0.02 + yl)).abs() < 1e-12, "l_eq bloc {j}");
        assert!((rel(ps.l_fdi[j], before.l_fdi[j]) - (1.0 + 0.02 + yl)).abs() < 1e-12, "l_fdi bloc {j}");
    }
}

#[test]
fn rv_a_burn_in_worlds_first_update_is_checked_not_waived() {
    // Only a world without burn-in (`capture_bases`) waives the residual of its first update. A world with a burn-in captures at the base
    // month (fallback false) and its first reported month is checked: a displacement of NIIP_{t-1} by 1e-6 of GDP reads back as 1e-6.
    let mut w = sim::new_world(&rcfg(12, 300), 5);
    assert!(!pos(&w).fallback && pos(&w).updates == 0);
    if cfg!(debug_assertions) {
        return; // a debug build's check_identities would panic on the displaced residual, as it should
    }
    let gdp = crate::common::gdp_month_model(&w);
    w.world.as_mut().unwrap().pos.as_mut().unwrap().niip_prev += 1.0e-6 * gdp;
    sim::step(&mut w);
    let e = w.world.as_ref().unwrap().diag.sfc_world_error;
    assert!((e - 1.0e-6).abs() < 1.0e-9, "first update residual {e}");
}
