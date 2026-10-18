//! Stage 4-3 sketch tests (spec 2.5 retaliation, 2.8 positions; the stage 4-3 pre-flight sketch): a subset of the tests of
//! the stage's implementer task description, written to validate the hand values. Every expectation is computed here from the tables and the formulas.

use super::params::WorldParams;
use super::positions::PositionsState;
use super::trade::{RetState, TariffComp, RET_GOODS};
use super::NB;
use crate::config::Config;
use crate::ledger;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::{BondHolder, BondId, World};

const S: usize = 9;
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

fn ret_state(cfg: &Config) -> RetState {
    RetState::new(cfg, &WorldParams::load())
}

fn tr(w: &World) -> &super::trade::TradeState {
    w.world.as_ref().unwrap().trade.as_ref().unwrap()
}

fn pos(w: &World) -> &PositionsState {
    w.world.as_ref().unwrap().pos.as_ref().unwrap()
}

fn json(m: &crate::metrics::Metrics) -> serde_json::Value {
    serde_json::to_value(m).unwrap()
}

fn comp(partner: Option<usize>, sector: Option<usize>, tau: f64) -> TariffComp {
    TariffComp { partner, sector, tau, retaliate: true }
}

#[test]
fn retaliation_export_elasticity_12m() {
    // CN +15 points on all sectors, retaliate: tau*_s = 0.95 w_s 0.15, a step in x = ln(1 + tau*) at update 1. The exponent on x after n
    // updates is eps_sr + (eps_lr - eps_sr)(1 - (1 - speed)^n): 1.2 at n = 1, 2.43514 at n = 12, 3 in the limit.
    let c = rcfg(0, 300);
    let mut r = ret_state(&c);
    let tariffs = [comp(Some(CN), None, 0.15)];
    let w_s = [1.0, 1.5, 1.2, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0];
    let mut want_n = Vec::new();
    for n in 1..=300usize {
        r.step(&tariffs);
        want_n.push(1.0 + 2.0 * (1.0 - 0.9f64.powi(n as i32)));
        for s in 0..S {
            let tau = 0.95 * w_s[s] * 0.15;
            assert!((r.tau_star[CN][s] - tau).abs() < 1e-15, "tau* sector {s}: {}", r.tau_star[CN][s]);
            let x = (1.0 + tau).ln();
            if tau > 0.0 && (n == 1 || n == 12 || n == 300) {
                let got = -r.r[CN][s].ln() / x;
                assert!((got - want_n[n - 1]).abs() < 1e-9, "n {n} sector {s}: exponent {got} vs {}", want_n[n - 1]);
            }
            if tau == 0.0 {
                assert!(r.r[CN][s].to_bits() == 1.0f64.to_bits(), "w_s = 0 sectors keep R = 1 exactly");
            }
        }
    }
    assert!((want_n[11] - 2.43514).abs() < 1e-5 && (want_n[0] - 1.2).abs() < 1e-12 && (want_n[299] - 3.0).abs() < 1e-9);
    // Other blocs stay at zero: kappa_MX = 0, and EA has not met its lag.
    assert!(r.tau_star[5].iter().all(|&t| t == 0.0));
}

#[test]
fn retaliation_ratchet_up_one_down_tenth() {
    // Up with mu = 1, down with mu = 0.1: the average US duty on CN is 0.15 for 6 months and then 0.
    let c = rcfg(0, 300);
    let mut r = ret_state(&c);
    let up = [comp(Some(CN), None, 0.15)];
    for _ in 0..6 {
        r.step(&up);
    }
    let food = 0.95 * 1.5 * 0.15;
    assert!((r.tau_star[CN][1] - food).abs() < 1e-15);
    // the component ends: the state keeps the value and decays by 0.9 a month
    let mut want = food;
    for k in 1..=24 {
        r.step(&[]);
        want += 0.1 * (0.0 - want);
        assert!((r.tau_star[CN][1] - want).abs() < 1e-15, "month {k}: {} vs {want}", r.tau_star[CN][1]);
    }
    assert!((want - food * 0.9f64.powi(24)).abs() < 1e-15);
    // a higher target again: back up in one step
    r.step(&[comp(Some(CN), None, 0.30)]);
    assert!((r.tau_star[CN][1] - 0.95 * 1.5 * 0.30).abs() < 1e-15);
}

#[test]
fn retaliation_caps_lags_and_keys() {
    let c = rcfg(0, 300);
    let wp = WorldParams::load();
    let rt = &wp.trade.retaliation;
    // keys: the research names the euro area "EU"; bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW
    assert_eq!(rt.kappa, vec![0.2, 0.95, 0.0, 0.2, 0.5, 0.0, 0.1, 0.0, 0.0, 0.0, 0.0, 0.05]); // UK 0.2: owner choice (proxy for the euro area), 2026-10-15
    assert_eq!(rt.lag, vec![2, 0, 1, 2, 0, 1, 12, 1, 1, 1, 1, 1]);
    assert_eq!(rt.cap, vec![0.5, 1.25, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5, 0.5]);
    assert_eq!(rt.w_s, vec![1.0, 1.5, 1.2, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0]);
    // cap: a 500-point duty on CN: 0.95 x 1.5 x 5.0 = 7.1 is capped at 1.25 for Food, Durables 1.25, Nondurables 1.25, Tech 1.25 (0.5 x 4.75 = 2.4)
    let mut r = ret_state(&c);
    r.step(&[comp(Some(CN), None, 5.0)]);
    for s in 0..5 {
        assert_eq!(r.tau_star[CN][s], 1.25, "sector {s}");
    }
    // lag: EA (2 months) and IN (12 months) respond that many months after the US duty; CN and CA at once
    let mut r = ret_state(&c);
    let t = [comp(None, None, 0.25)];
    let mut first_ea = None;
    let mut first_in = None;
    let mut first_cn = None;
    for m in 0..20usize {
        r.step(&t);
        if first_ea.is_none() && r.tau_star[0][0] > 0.0 {
            first_ea = Some(m);
        }
        if first_in.is_none() && r.tau_star[6][0] > 0.0 {
            first_in = Some(m);
        }
        if first_cn.is_none() && r.tau_star[CN][0] > 0.0 {
            first_cn = Some(m);
        }
    }
    assert_eq!((first_cn, first_ea, first_in), (Some(0), Some(2), Some(12)));
    // EA after the lag: min(0.5, 0.2 w_s 0.25); MX never (kappa 0); RoW (kappa 0.05, lag 1): 0.05 w_s 0.25 from month 1
    assert!((r.tau_star[0][1] - 0.2 * 1.5 * 0.25).abs() < 1e-15);
    assert!(r.tau_star[5].iter().all(|&x| x == 0.0));
    assert!((r.tau_star[11][2] - 0.05 * 1.2 * 0.25).abs() < 1e-15);
}

#[test]
fn retaliation_taubar_weights_and_sector_spread() {
    // A sector-specific component: taubar_CN = 0.25 imports_base[CN][Durables] / sum_s imports_base[CN][s] (fixed 2024 import values);
    // the response then spreads over every sector by w_s (Tech 0.5, services 0).
    let c = rcfg(0, 300);
    let wp = WorldParams::load();
    let ib = &wp.trade.imports_base[CN];
    let tot: f64 = ib.iter().sum();
    let want = 0.25 * ib[2] / tot;
    let mut r = ret_state(&c);
    let t = [comp(Some(CN), Some(2), 0.25)];
    assert!((r.taubar(&t, CN) - want).abs() < 1e-15);
    r.step(&t);
    assert!((r.tau_star[CN][4] - 0.95 * 0.5 * want).abs() < 1e-15);
    assert!((want - 0.25 * 206.5 / 462.6).abs() < 2e-4, "hand 0.1116: {want}");
    // a component that is not flagged does nothing
    let mut r = ret_state(&c);
    r.step(&[TariffComp { partner: Some(CN), sector: None, tau: 0.25, retaliate: false }]);
    assert!(r.tau_star[CN].iter().all(|&x| x == 0.0) && r.r[CN].iter().all(|&x| x == 1.0));
}

#[test]
fn retaliation_off_or_unflagged_is_bit_identical() {
    // Switch on with no flagged shock changes nothing: exports, every ABM key and the world keys are bit-identical to the switch off
    // for 24 months, and the state is zero with R = 1.0 exactly.
    let on = rcfg(12, 300);
    let off = Config { world_retaliation: false, world_positions: false, ..on.clone() };
    let (mut a, mut b) = (sim::new_world(&on, 5), sim::new_world(&off, 5));
    assert!(tr(&a).ret.hist.iter().all(|h| h.iter().all(|&x| x == 0.0)) && tr(&a).ret.tau_star.iter().all(|r| r.iter().all(|&x| x == 0.0)));
    for _ in 0..24 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    for (ma, mb) in a.history.iter().zip(b.history.iter()).skip(12) {
        let (mut ja, mut jb) = (json(ma), json(mb));
        for j in [&mut ja, &mut jb] {
            let w = j.get_mut("world").unwrap().as_object_mut().unwrap();
            for k in ["niip_pct_gdp", "ca_pct_gdp", "ca_bilateral_bn", "sfc_world_error"] {
                w.remove(k);
            }
        }
        assert_eq!(ja, jb);
    }
    assert!(tr(&a).ret.r.iter().all(|r| r.iter().all(|&x| x.to_bits() == 1.0f64.to_bits())));
}

#[test]
fn retaliation_pair_identical_before_shock() {
    // Tariff{CN, all sectors, 15, retaliate} at reported month t0 in one of two worlds: everything is bit-identical before t0, and at t0
    // each export cell to CN differs by exactly exp(-1.2 ln(1 + tau*_s)) (the impact exponent).
    let t0 = 3u32;
    let c = rcfg(12, 300);
    let shock = Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 15.0, retaliate: true };
    let mut a = sim::new_world(&c, 4);
    let mut b = sim::new_world(&c, 4);
    b.shocks.schedule(ScheduledShock { at_month: t0, shock: shock.clone() });
    for m in 0..6u32 {
        sim::step(&mut a);
        sim::step(&mut b);
        if m < t0 {
            assert_eq!(json(&a.history[m as usize]), json(&b.history[m as usize]), "month {m}");
        }
        if m == t0 {
            let w_s = [1.0, 1.5, 1.2, 1.0, 0.5, 0.0, 0.0, 0.0, 0.0];
            for s in 0..S {
                let x = (1.0 + 0.95 * w_s[s] * 0.15f64).ln();
                let want = (-1.2 * x).exp();
                let got = tr(&b).x_exp[s][CN] / tr(&a).x_exp[s][CN];
                assert!((got - want).abs() < 1e-12, "sector {s}: {got} vs {want}");
            }
        }
    }
    let sc = sim::Scenario { config: Config { warmup_months: 12, ..c.clone() }, shocks: vec![ScheduledShock { at_month: t0, shock }], months: 6, seeds: vec![4] };
    let cmp = sim::compare(&sc);
    for m in 0..t0 as usize {
        assert_eq!(json(&cmp.baseline[m]), json(&cmp.shocked[m]), "compare: month {m}");
    }
}

#[test]
fn retaliation_goods_metrics_and_abm_exports() {
    // ret_tau is the goods-export-weighted mean of tau*; the ABM's orders equal the bilateral sum.
    let c = rcfg(12, 300);
    let mut w = sim::new_world(&c, 6);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 15.0, retaliate: true } });
    sim::step(&mut w);
    let d = &w.world.as_ref().unwrap().diag;
    let x0 = &tr(&w).tab.x0;
    let w_s = [1.0, 1.5, 1.2, 1.0, 0.5];
    let num: f64 = (0..RET_GOODS).map(|s| x0[s][CN] * 0.95 * w_s[s] * 0.15).sum();
    let den: f64 = (0..RET_GOODS).map(|s| x0[s][CN]).sum();
    assert!((d.ret_tau[CN] - num / den).abs() < 1e-12 && (d.ret_tau[CN] - 0.15121).abs() < 1e-4, "ret_tau {}", d.ret_tau[CN]);
    assert!(d.ret_tau.iter().enumerate().all(|(j, &x)| j == CN || x == 0.0));
    for s in 0..S {
        let sum: f64 = (0..NB).map(|j| tr(&w).x_exp[s][j]).sum();
        assert!((w.orders.exports[s] - sum).abs() <= 1e-12 * sum.abs().max(1.0), "sector {s}");
    }
    let goods: f64 = d.exports_goods_bn.iter().sum();
    let all: f64 = d.exports_bn.iter().sum();
    assert!(goods > 0.0 && goods < all);
}

#[test]
fn config_validation_retaliation() {
    let ok = Config { world_retaliation: true, world_link_trade: true, ..Config::default() };
    assert!(ok.validate_world_links().is_ok());
    assert!(Config { world_link_trade: false, ..ok.clone() }.validate_world_links().is_err());
    for (sr, lr, sp) in [(-0.1, 3.0, 0.1), (1.0, 10.5, 0.1), (3.5, 3.0, 0.1), (1.0, 3.0, -0.1), (1.0, 3.0, 1.5), (f64::NAN, 3.0, 0.1), (1.0, f64::INFINITY, 0.1), (1.0, 3.0, f64::NAN)] {
        let c = Config { world_ret_eps_sr: sr, world_ret_eps_lr: lr, world_ret_speed: sp, ..ok.clone() };
        assert!(c.validate_world_links().is_err(), "{sr} {lr} {sp}");
    }
    let d = Config::default();
    assert!(!d.world_retaliation && !d.world_positions && d.world_ret_eps_sr == 1.0 && d.world_ret_eps_lr == 3.0 && d.world_ret_speed == 0.10);
}

// ---------------------------------------------------------------- positions

fn research_sums(wp: &WorldParams) -> (f64, f64, f64, f64) {
    let t = &wp.finance.positions;
    let sum = |v: &Vec<f64>| v.iter().sum::<f64>();
    let a = sum(&t.a_eq) + sum(&t.a_debt) + sum(&t.a_fdi) + sum(&t.a_bank);
    let l = sum(&t.l_agcorp) + sum(&t.l_eq) + sum(&t.l_fdi);
    (a, l, sum(&wp.finance.tic_ust_bn), 0.0)
}

#[test]
fn positions_initial_ratios_and_memo_stocks_at_base() {
    // W4-21 (restated, D40): at the base month the memo assets are 115.38 % and the memo liabilities 129.33 % of the trend GDP 12 tau_b,
    // every memo stock is the research value times k = 12 tau_b / world_gdp_rw_bn (1e-9), and the research-scale nine-stock NIIP is -59.77 %.
    let c = rcfg(12, 400);
    let w = sim::new_world(&c, 3);
    let wp = WorldParams::load();
    let ps = pos(&w);
    let b = ps.base.as_ref().expect("captured at the end of the last burn-in month");
    let tau = crate::fiscal::trend_gdp_month(&w);
    assert!((b.tau_b - tau).abs() <= 1e-12 * tau, "tau_b is the trend after the base month's settlement");
    let k = 12.0 * tau / c.world_gdp_rw_bn;
    assert!((b.k - k).abs() <= 1e-12 * k);
    let t = &wp.finance.positions;
    for j in 0..NB {
        assert!((ps.a_eq[j] - t.a_eq[j] * k).abs() <= 1e-9 * (t.a_eq[j] * k).abs().max(1e-12));
        assert!((ps.l_fdi[j] - t.l_fdi[j] * k).abs() <= 1e-9 * (t.l_fdi[j] * k).abs().max(1e-12));
    }
    let y = 12.0 * tau;
    assert!((100.0 * ps.memo_assets() / y - 115.38).abs() < 0.05, "assets {}", 100.0 * ps.memo_assets() / y);
    assert!((100.0 * ps.memo_liabs() / y - 129.33).abs() < 0.05, "liabilities {}", 100.0 * ps.memo_liabs() / y);
    let (a, l, tic, _) = research_sums(&wp);
    let bis: f64 = (0..NB).map(|j| t.dep_share[j]).sum::<f64>();
    assert!((bis - 1.0).abs() < 1e-12 && (t.tic_share.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    let niip_rs = (a - l - tic - 5670.7) / 32560.0;
    assert!((100.0 * niip_rs + 59.77).abs() < 0.02, "research-scale NIIP {}", 100.0 * niip_rs);
    // the PI of the memo stocks and of the nine stocks at research scale
    let pi_memo = 0.038 * a / 32560.0 - 0.0255 * l / 32560.0;
    assert!((100.0 * pi_memo - 1.086).abs() < 0.01, "memo PI {}", 100.0 * pi_memo);
}

#[test]
fn niip_dollar_valuation_minus_8p7_gdp() {
    // W4-22: a uniform dollar step of ln 1.10 on the memo stocks (no return, no noise): -2.820 tn at research scale, -8.662 % of GDP.
    let c = rcfg(12, 400);
    let w = sim::new_world(&c, 3);
    let mut ps = pos(&w).clone();
    let y = 12.0 * ps.base.as_ref().unwrap().tau_b;
    let niip0 = ps.memo_assets() - ps.memo_liabs();
    let out = ps.memo_step(&[1.10f64.ln(); NB], 0.0, &[0.0; NB]);
    let income = out.pi_total;
    let d_niip = (ps.memo_assets() - ps.memo_liabs()) - niip0;
    assert!((d_niip - income - out.val).abs() <= 1e-9 * y, "valuation plus income is the change");
    assert!((100.0 * out.val / y + 8.662).abs() < 0.002, "uniform ln 1.10: {} % of GDP", 100.0 * out.val / y);
    let mut ps2 = pos(&w).clone();
    let out2 = ps2.memo_step(&[0.10; NB], 0.0, &[0.0; NB]);
    assert!((100.0 * out2.val / y + 9.089).abs() < 0.002, "10 % step: {}", 100.0 * out2.val / y);
}

#[test]
fn positions_memo_valuation_rules() {
    // Hand values per stock for one bloc (EA): ds = 0.05, r_us = 0.01, eta = 0.02: equity v = 1.15 x 0.01 + 0.02 - 0.9 x 0.05, debt -0.23 x 0.05,
    // FDI -0.05, bank claims -0.3 x 0.05; agency+corporate -0.03 x 0.05; US equity and FDI liabilities +0.01.
    let c = rcfg(12, 400);
    let w = sim::new_world(&c, 3);
    let mut ps = pos(&w).clone();
    let before = ps.clone();
    let mut eta = [0.0; NB];
    eta[0] = 0.02;
    ps.memo_step(&[0.05; NB], 0.01, &eta);
    let (ya, yl) = (0.038 / 12.0, 0.0255 / 12.0);
    let rel = |new: f64, old: f64| new / old;
    assert!((rel(ps.a_eq[0], before.a_eq[0]) - (1.0 + 1.15 * 0.01 + 0.02 - 0.9 * 0.05 + ya)).abs() < 1e-12);
    assert!((rel(ps.a_debt[0], before.a_debt[0]) - (1.0 - 0.23 * 0.05 + ya)).abs() < 1e-12);
    assert!((rel(ps.a_fdi[0], before.a_fdi[0]) - (1.0 - 0.05 + ya)).abs() < 1e-12);
    assert!((rel(ps.a_bank[0], before.a_bank[0]) - (1.0 - 0.3 * 0.05 + ya)).abs() < 1e-12);
    assert!((rel(ps.l_ag[0], before.l_ag[0]) - (1.0 - 0.03 * 0.05 + yl)).abs() < 1e-12);
    assert!((rel(ps.l_eq[0], before.l_eq[0]) - (1.0 + 0.01 + yl)).abs() < 1e-12);
    assert!((rel(ps.l_fdi[0], before.l_fdi[0]) - (1.0 + 0.01 + yl)).abs() < 1e-12);
    // CN: no noise in this call
    assert!((rel(ps.a_eq[1], before.a_eq[1]) - (1.0 + 0.55 * 0.01 - 0.9 * 0.05 + ya)).abs() < 1e-12);
}

fn run_months(w: &mut World, n: usize) {
    for _ in 0..n {
        sim::step(w);
    }
}

#[test]
fn positions_identity_exact_and_sums() {
    // 40 months with QE, a Dollar shock and a policy shift: |resid| <= 1e-9 of monthly GDP; the bilateral exports and imports sum to the
    // ledger totals and the bilateral current accounts to the total.
    let c = rcfg(12, 400);
    let mut w = sim::new_world(&c, 8);
    w.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 6 } });
    w.shocks.schedule(ScheduledShock { at_month: 4, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } });
    w.shocks.schedule(ScheduledShock { at_month: 8, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    for m in 0..40 {
        sim::step(&mut w);
        let ps = pos(&w);
        let gdp = crate::common::gdp_month_model(&w);
        assert!(ps.resid.abs() <= 1e-9 * gdp, "month {m}: residual {} of GDP {gdp}", ps.resid);
        let d = &w.world.as_ref().unwrap().diag;
        assert!(d.sfc_world_error <= 1e-9);
        let xs: f64 = tr(&w).exp_bloc.iter().sum();
        let ms: f64 = tr(&w).imp_bloc.iter().sum();
        assert!((xs - w.row.exports_this_month).abs() <= 1e-9 * xs.abs().max(1.0), "month {m}: sum of exports");
        assert!((ms - w.row.imports_this_month).abs() <= 1e-9 * ms.abs().max(1.0), "month {m}: sum of imports");
        let cas: f64 = ps.ca_j.iter().sum();
        assert!((cas - ps.ca_total).abs() <= 1e-9 * gdp, "month {m}: sum of bilateral CA");
    }
}

#[test]
fn positions_identity_holds_with_negative_yields_at_the_auction() {
    // The 3m and 2y yields below zero: new issues are priced at par but trade above it (coupon floored at 0). The auction term keeps the
    // identity exact.
    let c = rcfg(12, 400);
    let mut w = sim::new_world(&c, 9);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::TermPremium { tenor_idx: 0, bps: -400.0, half_life_months: 0.0 } });
    let mut saw_negative = false;
    for m in 0..12 {
        sim::step(&mut w);
        saw_negative |= w.curve.yields[0] < 0.0;
        let gdp = crate::common::gdp_month_model(&w);
        assert!(pos(&w).resid.abs() <= 1e-9 * gdp, "month {m}: residual {} (3m yield {})", pos(&w).resid, w.curve.yields[0]);
    }
    assert!(saw_negative, "the shock must make the 3m yield negative");
}

#[test]
fn ledger_interest_accumulators_match_the_payments() {
    // The coupon accumulator equals RoW's coupon income computed from the faces held when the service runs, and the principal of the
    // matured bonds is not in it.
    let c = rcfg(12, 400);
    let mut w = sim::new_world(&c, 10);
    run_months(&mut w, 3);
    let mut probe = w.clone();
    let want: f64 = probe.bonds.iter().map(|b| b.held_by(BondHolder::RestOfWorld) * b.coupon / 12.0).sum();
    let before = probe.world.as_ref().unwrap().pos.as_ref().unwrap().cpn_month;
    ledger::service_government_debt(&mut probe);
    let got = probe.world.as_ref().unwrap().pos.as_ref().unwrap().cpn_month - before;
    assert!((got - want).abs() <= 1e-12 * want.abs(), "coupon {got} vs {want}");
}

#[test]
fn positions_noise_stream_is_distinct_from_zeta() {
    use crate::rng::{self, normal, stream};
    // eta uses (seed, tick, WORLD_FX, 16 + j); zeta uses (.., j): different draws.
    let (seed, tick) = (11u64, 5u32);
    for j in 0..NB as u64 {
        let mut a = rng::agent_rng(seed, tick, stream::WORLD_FX, j);
        let mut b = rng::agent_rng(seed, tick, stream::WORLD_FX, super::positions::POS_NOISE_BASE + j);
        assert_ne!(normal(&mut a).to_bits(), normal(&mut b).to_bits());
    }
    let draws: Vec<f64> = (0..5000u32)
        .map(|t| {
            let mut r = rng::agent_rng(seed, t, stream::WORLD_FX, super::positions::POS_NOISE_BASE);
            super::positions::ETA_SD * normal(&mut r)
        })
        .collect();
    let m = draws.iter().sum::<f64>() / 5000.0;
    let sd = (draws.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / 4999.0).sqrt();
    assert!((sd - 0.02).abs() < 0.002, "sd {sd}");
}

#[test]
fn positions_pair_identical_before_shock() {
    // A Dollar shock at t0 in one world: positions outputs bit-identical before t0; at t0 the difference of the memo valuation is the
    // analytic -sum fc X (Dln fx_b - Dln fx_a).
    let t0 = 3u32;
    let c = rcfg(12, 300);
    let mut a = sim::new_world(&c, 4);
    let mut b = sim::new_world(&c, 4);
    b.shocks.schedule(ScheduledShock { at_month: t0, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } });
    for m in 0..6u32 {
        let (fa, fb) = (a.row.fx, b.row.fx);
        let (spa, spb) = (a.fund.share_price, b.fund.share_price);
        let stocks = pos(&a).clone();
        sim::step(&mut a);
        sim::step(&mut b);
        if m < t0 {
            assert_eq!(pos(&a).niip.to_bits(), pos(&b).niip.to_bits(), "month {m}");
            assert_eq!(pos(&a).val_memo.to_bits(), pos(&b).val_memo.to_bits());
        }
        if m == t0 {
            let ds = (b.row.fx / fb).ln() - (a.row.fx / fa).ln();
            let fc_sum: f64 = (0..NB)
                .map(|j| stocks.fc_eq * stocks.a_eq[j] + stocks.fc_debt * stocks.a_debt[j] + stocks.fc_fdi * stocks.a_fdi[j] + stocks.fc_bank * stocks.a_bank[j] - stocks.fc_liab * stocks.l_ag[j])
                .sum();
            // the US equity return of the month also differs between the worlds (the Fund's share price): net exposure of the memo stocks
            let dr = (b.fund.share_price / spb).ln() - (a.fund.share_price / spa).ln();
            let eq_exp: f64 = (0..NB).map(|j| stocks.load[j] * stocks.a_eq[j] - stocks.l_eq[j] - stocks.l_fdi[j]).sum();
            let got = pos(&b).val_memo - pos(&a).val_memo;
            let want = -fc_sum * ds + eq_exp * dr;
            assert!((got - want).abs() <= 1e-9 * fc_sum.abs() * ds.abs().max(1e-3), "valuation difference {got} vs {want}");
            assert!(fc_sum * ds.abs() > 0.0 && ds > 0.0, "the shock month moves the dollar");
        }
    }
    let sc = sim::Scenario { config: Config { warmup_months: 12, ..c.clone() }, shocks: vec![ScheduledShock { at_month: t0, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } }], months: 6, seeds: vec![4] };
    let cmp = sim::compare(&sc);
    for m in 0..t0 as usize {
        assert_eq!(json(&cmp.baseline[m]), json(&cmp.shocked[m]), "compare: month {m}");
    }
}

#[test]
fn positions_metrics_keys_and_off_state() {
    let on = rcfg(12, 300);
    let off = Config { world_positions: false, ..on.clone() };
    let (mut a, mut b) = (sim::new_world(&on, 5), sim::new_world(&off, 5));
    run_months(&mut a, 3);
    run_months(&mut b, 3);
    let da = &a.world.as_ref().unwrap().diag;
    let db = &b.world.as_ref().unwrap().diag;
    assert!(da.niip_pct_gdp.is_finite() && da.niip_pct_gdp < -20.0 && da.ca_pct_gdp.is_finite() && da.sfc_world_error < 1e-9);
    assert!(da.ca_bilateral_bn.iter().all(|x| x.is_finite()) && da.ca_bilateral_bn.iter().any(|&x| x != 0.0));
    assert!(db.niip_pct_gdp == 0.0 && db.ca_pct_gdp == 0.0 && db.sfc_world_error == 0.0 && db.ca_bilateral_bn.iter().all(|&x| x == 0.0));
    assert!(b.world.as_ref().unwrap().pos.is_none());
}

#[test]
fn positions_burn_in_zero_world_captures_at_its_first_step() {
    // A world without burn-in has no base-month settle: `capture_bases` captures the memo stocks at the first step, the first update has no
    // previous marks (its residual is set to zero) and the identity holds from the second month.
    let w0 = sim::new_world(&rcfg(0, 300), 3);
    assert!(pos(&w0).base.is_none(), "nothing is captured before the first step");
    let mut w = sim::new_world(&rcfg(0, 300), 3);
    sim::step(&mut w);
    assert!(pos(&w).base.is_some() && pos(&w).fallback && pos(&w).updates == 1 && pos(&w).resid == 0.0);
    for m in 1..12 {
        sim::step(&mut w);
        let gdp = crate::common::gdp_month_model(&w);
        assert!(pos(&w).resid.abs() <= 1e-9 * gdp, "month {m}: {}", pos(&w).resid);
    }
    assert!(super::check_identities(&w).is_ok());
}

#[test]
fn check_identities_flags_a_broken_identity() {
    let mut w = sim::new_world(&rcfg(12, 300), 3);
    run_months(&mut w, 3);
    assert!(super::check_identities(&w).is_ok());
    w.world.as_mut().unwrap().pos.as_mut().unwrap().resid = 1e-3 * crate::common::gdp_month_model(&w);
    assert!(super::check_identities(&w).unwrap_err().contains("NIIP identity"));
    let mut w2 = sim::new_world(&rcfg(12, 300), 3);
    run_months(&mut w2, 3);
    w2.world.as_mut().unwrap().pos.as_mut().unwrap().ca_total += 1e-3 * crate::common::gdp_month_model(&w2);
    assert!(super::check_identities(&w2).unwrap_err().contains("bilateral"));
}

#[test]
fn world_4_3_defaults_and_constants_are_the_spec_values() {
    // The unpinned defaults are what survives a mutation test (4-2 review): pin them.
    let d = Config::default();
    assert!(!d.world_retaliation && !d.world_positions);
    assert_eq!((d.world_ret_eps_sr, d.world_ret_eps_lr, d.world_ret_speed), (1.0, 3.0, 0.10));
    let wp = WorldParams::load();
    let t = &wp.finance.positions;
    assert_eq!((t.fc_eq, t.fc_fdi, t.fc_bank, t.fc_liab), (0.9, 1.0, 0.3, 0.03));
    assert!((t.fc_debt - 0.23).abs() < 1e-12 && t.yield_assets_pct == 3.8 && t.yield_liab_pct == 2.55);
    assert_eq!(super::positions::ETA_SD, 0.02);
    assert_eq!(super::positions::POS_NOISE_BASE, 16);
    assert_eq!((super::trade::RET_MU_UP, super::trade::RET_MU_DOWN, super::trade::RET_RING, super::trade::RET_GOODS), (1.0, 0.1, 13, 5));
    assert_eq!(t.equity_loading, vec![1.15, 0.55, 0.94, 0.89, 0.92, 1.01, 1.14, t.equity_loading[7], t.equity_loading[8], t.equity_loading[9], t.equity_loading[10], t.equity_loading[11]]);
    assert!((t.tic_share.iter().sum::<f64>() - 1.0).abs() < 1e-12);
}

#[test]
fn positions_work_without_the_trade_link_and_with_finance_on() {
    // The positions read the ledger and the shadow trade memo: the identity and the bilateral sums hold with the trade link off, and with
    // the finance link on (official sales and purchases are ledger trades).
    for (trade, fin) in [(false, false), (true, true), (false, true)] {
        let c = Config { world_link_trade: trade, world_retaliation: false, world_link_finance: fin, ..rcfg(12, 400) };
        let mut w = sim::new_world(&c, 12);
        if fin {
            w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: -400.0, months: 3 } });
            w.shocks.schedule(ScheduledShock { at_month: 6, shock: Shock::OfficialTreasury { bloc: None, bn: 300.0, months: 2 } });
        }
        for m in 0..24 {
            sim::step(&mut w);
            let gdp = crate::common::gdp_month_model(&w);
            let ps = pos(&w);
            assert!(ps.resid.abs() <= 1e-9 * gdp, "trade {trade} finance {fin} month {m}: {}", ps.resid);
            let xs: f64 = tr(&w).exp_bloc.iter().sum();
            assert!((xs - w.row.exports_this_month).abs() <= 1e-9 * xs.abs().max(1.0));
        }
    }
}

// ---------------------------------------------------------------- tests added after the first sketch round (tests 10, 19, 23-26 of the task description)

#[test]
fn retaliation_cn_goods_exports_month_12_match_the_hand_value() {
    // Integration: two tiny worlds of one seed, a flagged CN +15 duty at month 0 in one of them. Hand: statistic -2.390, goods ratio 0.714;
    // the probe at 10,000 households gave -2.428. Tiny worlds are noisy in the level but the factor is deterministic.
    let c = rcfg(12, 400);
    let mut a = sim::new_world(&c, 21);
    let mut b = sim::new_world(&c, 21);
    b.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 15.0, retaliate: true } });
    for _ in 0..12 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    let (da, db) = (&a.world.as_ref().unwrap().diag, &b.world.as_ref().unwrap().diag);
    let ratio = db.exports_goods_bn[CN] / da.exports_goods_bn[CN];
    let stat = ratio.ln() / (1.0 + db.ret_tau[CN]).ln();
    assert!(stat > -2.55 && stat < -2.25, "statistic {stat}");
    assert!(ratio > 0.69 && ratio < 0.74, "goods ratio {ratio}");
}

#[test]
fn ledger_interest_accumulators_match_the_payments_deposit_and_auction() {
    // Deposit interest: banking::service on a clone changes RoW's deposits by the accumulator only. Auction: collect_and_finance on a clone
    // records RoW's purchases of the new bonds at par.
    let c = rcfg(12, 400);
    let mut w = sim::new_world(&c, 10);
    run_months(&mut w, 3);
    let mut p2 = w.clone();
    for b in p2.banks.iter_mut() {
        b.deposit_rate = 0.03; // the tiny world's deposit rate can be zero: the test needs a payment
    }
    if p2.row.deposits <= 0.0 {
        crate::ledger::pay(&mut p2, crate::ledger::Account::Government, crate::ledger::Account::RestOfWorld, 1.0e6);
    }
    let d0 = p2.row.deposits;
    let before = p2.world.as_ref().unwrap().pos.as_ref().unwrap().dep_int_month;
    crate::banking::service(&mut p2);
    let got = p2.world.as_ref().unwrap().pos.as_ref().unwrap().dep_int_month - before;
    assert!(got > 0.0 && ((p2.row.deposits - d0) - got).abs() <= 1e-9 * got, "deposit interest {got} against the change {}", p2.row.deposits - d0);
    // Auction, unconditionally: the TGA is emptied on the clone, so the financing need is positive and the auction runs; RoW's share of the
    // auction is positive (auction_shares[2] = 0.25), so RoW buys and the purchase is recorded.
    let mut p3 = w.clone();
    p3.tick += 1;
    // The need is the target less the TGA after the month's flows (taxes and spending move it): a TGA 20 monthly GDPs in deficit keeps it positive.
    p3.cb.tga = -20.0 * crate::common::gdp_month_model(&p3).max(1.0);
    let row_before: Vec<(BondId, f64)> = p3.bonds.iter().map(|b| (b.id, b.held_by(BondHolder::RestOfWorld))).collect();
    crate::fiscal::collect_and_finance(&mut p3);
    let prim = p3.world.as_ref().unwrap().pos.as_ref().unwrap().prim_month.clone();
    assert!(!prim.is_empty(), "the forced auction must record RoW's purchase");
    for (id, face, paid) in &prim {
        let idx = p3.bonds.iter().rposition(|b| b.id == *id).expect("a recorded purchase belongs to an issued bond");
        let before = row_before.iter().find(|x| x.0 == *id).map_or(0.0, |x| x.1);
        let gain = p3.bonds[idx].held_by(BondHolder::RestOfWorld) - before;
        assert!((gain - face).abs() <= 1e-9 * face, "RoW's face increase of bond {id}: {gain} against {face}");
        assert!((paid - face * 1.0).abs() <= 1e-12 * face, "bond {id} paid at par: {paid} against {face}");
    }
}

#[test]
fn positions_hooks_inert_when_off() {
    // positions off with the block on, and the block off: the three notes change nothing and no state exists.
    let on = rcfg(12, 300);
    let off_pos = Config { world_positions: false, ..on.clone() };
    let mut w = sim::new_world(&off_pos, 5);
    super::note_row_coupon(&mut w, 5.0);
    super::note_row_dep_interest(&mut w, 5.0);
    super::note_row_primary(&mut w, 7, 1.0, 1.0);
    assert!(w.world.as_ref().unwrap().pos.is_none());
    let mut wb = sim::new_world(&Config { world_block_enabled: false, world_link_trade: false, world_retaliation: false, ..on.clone() }, 5);
    super::note_row_coupon(&mut wb, 5.0);
    assert!(wb.world.is_none());
    // positions on, retaliation off, 36 months: every key equals both off, after removing the four positions keys
    let a_cfg = Config { world_retaliation: false, ..on.clone() };
    let b_cfg = Config { world_retaliation: false, world_positions: false, ..on.clone() };
    let (mut a, mut b) = (sim::new_world(&a_cfg, 6), sim::new_world(&b_cfg, 6));
    run_months(&mut a, 36);
    run_months(&mut b, 36);
    for (ma, mb) in a.history.iter().zip(b.history.iter()).skip(12) {
        let (mut ja, mut jb) = (json(ma), json(mb));
        for j in [&mut ja, &mut jb] {
            let w = j.get_mut("world").unwrap().as_object_mut().unwrap();
            for k in ["niip_pct_gdp", "ca_pct_gdp", "ca_bilateral_bn", "sfc_world_error"] {
                w.remove(k);
            }
        }
        assert_eq!(ja, jb);
    }
}

#[test]
fn positions_ledger_split_by_bloc() {
    let c = rcfg(12, 300);
    let w = sim::new_world(&c, 3);
    let ps = pos(&w);
    assert!((ps.tic_share.iter().sum::<f64>() - 1.0).abs() < 1e-12 && (ps.dep_share.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    for (cpn, dep) in [(1.0, 0.0), (0.0, 1.0), (3.0, 2.0)] {
        let led = ps.ledger_interest_by_bloc(cpn, dep);
        assert!((led.iter().sum::<f64>() - (cpn + dep)).abs() <= 1e-12 * (cpn + dep));
        assert!((led[1] - (cpn * ps.tic_share[1] + dep * ps.dep_share[1])).abs() < 1e-15);
    }
}

#[test]
fn positions_dollar_link_on_uses_the_bilateral_rates() {
    // Both links on: ds_j is the change of the dollar module's ln_s_now[j], which is not the broad change; the identity stays exact.
    let c = Config { world_link_dollar: true, ..rcfg(12, 300) };
    let mut w = sim::new_world(&c, 14);
    let mut prev = [0.0f64; NB];
    let mut differs = false;
    for m in 0..12 {
        let fx_before = w.row.fx;
        sim::step(&mut w);
        let now = w.world.as_ref().unwrap().dollar.as_ref().unwrap().ln_s_now;
        let ps = pos(&w);
        for j in 0..NB {
            assert!((ps.ds[j] - (now[j] - prev[j])).abs() < 1e-12, "month {m} bloc {j}");
        }
        let broad = (w.row.fx / fx_before).ln(); // the broad change of the dollar: not what the module's positions use
        differs |= (0..NB).any(|j| (ps.ds[j] - broad).abs() > 1e-6);
        let gdp = crate::common::gdp_month_model(&w);
        assert!(ps.resid.abs() <= 1e-9 * gdp, "month {m}: {}", ps.resid);
        prev = now;
    }
    assert!(differs);
}

#[test]
fn bilateral_current_account_sums_with_a_retaliation_shock() {
    let c = rcfg(12, 300);
    let mut a = sim::new_world(&c, 15);
    let mut b = sim::new_world(&c, 15);
    b.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 25.0, retaliate: true } });
    for m in 0..15 {
        sim::step(&mut a);
        sim::step(&mut b);
        for w in [&a, &b] {
            let gdp = crate::common::gdp_month_model(w);
            let xs: f64 = tr(w).exp_bloc.iter().sum();
            let ms: f64 = tr(w).imp_bloc.iter().sum();
            assert!((xs - w.row.exports_this_month).abs() <= 1e-9 * xs.abs().max(1.0), "month {m}: exports");
            assert!((ms - w.row.imports_this_month).abs() <= 1e-9 * ms.abs().max(1.0), "month {m}: imports");
            assert!((pos(w).ca_j.iter().sum::<f64>() - pos(w).ca_total).abs() <= 1e-9 * gdp, "month {m}: current accounts");
            assert!(pos(w).resid.abs() <= 1e-9 * gdp);
        }
        if m >= 2 {
            assert!(tr(&b).exp_bloc[CN] < tr(&a).exp_bloc[CN], "month {m}: exports to China fall under retaliation");
        }
    }
}

#[test]
fn positions_valuation_uses_monthly_increments() {
    // ds is the change of ln fx in the month (not the change since the base), r_us the change of ln share price in the month.
    let c = rcfg(12, 300);
    let mut w = sim::new_world(&c, 17);
    let (mut fx_prev, mut sp_prev) = (w.row.fx, w.fund.share_price);
    for m in 0..18 {
        sim::step(&mut w);
        let ps = pos(&w);
        let want_ds = (w.row.fx / fx_prev).ln();
        let want_r = (w.fund.share_price / sp_prev).ln();
        for j in 0..NB {
            assert!((ps.ds[j] - want_ds).abs() < 1e-12, "month {m} bloc {j}: ds {} vs {want_ds}", ps.ds[j]);
        }
        assert!((ps.r_us - want_r).abs() < 1e-12, "month {m}: r_us {} vs {want_r}", ps.r_us);
        fx_prev = w.row.fx;
        sp_prev = w.fund.share_price;
    }
}
