//! Reviewer's replacement tests for stage 4-2 (world_link_finance). Each test names the surviving mutants it kills (mutation list in
//! the reviewer's mutation script): F01, F23, F32, M01-M04, S01, R01, R03, P01, P02, P04, P06.

use super::finance::{self, OfficialProg};
use super::{bloc_index, world_tp_fo};
use crate::common;
use crate::config::Config;
use crate::monetary;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::{BondHolder, World};

fn fcfg(burn: u32, hh: usize) -> Config {
    Config {
        burn_in_months: burn,
        n_households: hh,
        world_block_enabled: true,
        world_link_trade: true,
        world_link_finance: true,
        fin_factor_rho: 0.83,
        fin_factor_sigma: 0.30,
        fin_jump_intensity: 0.06,
        fin_jump_mean: 1.3,
        fin_jump_sd: 0.5,
        erp_fin_loading: 0.0028,
        ..Config::default()
    }
}

fn quiet(mut c: Config) -> Config {
    c.world_fo_trend_cn = 0.0;
    c.world_fo_dollar_resp = 0.0;
    c.world_fo_vix_resp = 0.0;
    c
}

fn fin(w: &World) -> &finance::FinState {
    w.world.as_ref().unwrap().fin.as_ref().unwrap()
}

fn row_face(w: &World) -> f64 {
    w.bonds.iter().map(|b| b.held_by(BondHolder::RestOfWorld)).sum()
}

fn face_of(w: &World, h: BondHolder) -> f64 {
    w.bonds.iter().map(|b| b.held_by(h)).sum()
}

fn flows(w: &mut World) {
    let fx0 = Some(w.world.as_ref().unwrap().trade.as_ref().unwrap().base.as_ref().unwrap().fx0);
    let mut wb = w.world.take().unwrap();
    finance::step_flows(w, wb.fin.as_mut().unwrap(), fx0, true);
    w.world = Some(wb);
}

fn push_prog(w: &mut World, bloc: &str, per_month: f64) {
    let tick = w.tick;
    let mut wb = w.world.take().unwrap();
    wb.fin.as_mut().unwrap().progs.push(OfficialProg { bloc: Some(bloc_index(bloc)), per_month, t0: tick, months: 1 });
    w.world = Some(wb);
}

#[test]
fn rv_official_term_is_loaded_on_every_tenor_by_tp_slope() {
    // F01 (flat across tenors). After a +100 purchase the term in force at tenor j is TP_SLOPE[j] times the 10y term (the 3m none),
    // through `tp_finance` and through the hook `world_tp_fo` that `update_curve` calls.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    sim::step(&mut w);
    let t10 = finance::tp_finance(&w, 3);
    assert!(t10 < -9.0e-4 && t10 > -1.1e-3, "10y term {t10}");
    for j in 0..5 {
        let want = t10 * monetary::TP_SLOPE[j];
        assert!((finance::tp_finance(&w, j) - want).abs() < 1e-15, "tenor {j}: {} vs {want}", finance::tp_finance(&w, j));
        assert_eq!(world_tp_fo(&w, j), Some(finance::tp_finance(&w, j)), "hook tenor {j}");
    }
    assert!(finance::tp_finance(&w, 0).abs() < 1e-18, "the 3m bill carries nothing");
    assert!(finance::tp_finance(&w, 4) < finance::tp_finance(&w, 3), "30y loads more than the 10y");
}

#[test]
fn rv_official_sale_is_taken_by_the_banks_only() {
    // F23 (sales offered to the banks and the Fund): D45, sales go to the banks; the Fund's book is untouched.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    let (fund0, row0) = (face_of(&w, BondHolder::Fund), row_face(&w));
    let banks = |w: &World| (0..w.banks.len()).map(|b| face_of(w, BondHolder::Bank(b as crate::types::BankId))).sum::<f64>();
    let banks0 = banks(&w);
    push_prog(&mut w, "CN", -100.0);
    flows(&mut w);
    assert!(fin(&w).flow_rw < -99.0, "the sale executed: {}", fin(&w).flow_rw);
    let (fund1, row1, banks1) = (face_of(&w, BondHolder::Fund), row_face(&w), banks(&w));
    assert!((fund1 - fund0).abs() <= 1e-9 * fund0.max(1.0), "the Fund bought {}", fund1 - fund0);
    assert!(((banks1 - banks0) - (row0 - row1)).abs() <= 1e-9 * row0, "RoW's sale lands at the banks");
}

#[test]
fn rv_tp_fo_bp_metric_is_the_term_in_the_curve_of_the_month() {
    // F32: the recorded `tp_fo_bp` of month t is the term that month's curve carried (the state after month t-1), not the state after
    // t's own flow. The shock month records zero, the next month the full -10.14 bp.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    sim::step(&mut w);
    assert!(w.world.as_ref().unwrap().diag.tp_fo_bp.to_bits() == 0.0f64.to_bits(), "the shock month's curve is priced before the flow");
    let in_force = 1e4 * finance::tp_finance(&w, 3);
    assert!((in_force + 10.14).abs() < 0.5, "term after the flow {in_force}");
    sim::step(&mut w);
    let rec = w.world.as_ref().unwrap().diag.tp_fo_bp;
    assert!((rec - in_force).abs() < 1e-9, "recorded {rec} vs in force {in_force}");
}

#[test]
fn rv_rebalance_row_bond_adjustment_is_pinned() {
    // R01 (bonds not adjusted) and R03 (bonds adjusted with the wrong sign). After an official sale x > 0 the private book works on
    // deposits - x and bonds + x; the target is t' = (bonds + x)/12 + 0.5 GDP. Deposits x + t' - x/24 trade nothing; deposits x + t' + x/24 buy
    // exactly x/24 of bonds (RoW's cash falls by that amount); a target that ignores or mis-signs the bond adjustment trades more or less.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: -400.0, months: 1 } });
    sim::step(&mut w);
    let x = fin(&w).extra;
    assert!(x > 0.0);
    let gdp = common::gdp_month_model(&w);
    let tp = (w.row.bonds_value + x) / 12.0 + 0.5 * gdp;
    // (a) inside the band by x/24
    w.row.deposits = x + tp - x / 24.0;
    let (d0, f0) = (w.row.deposits, row_face(&w));
    monetary::rebalance_row(&mut w);
    assert!(w.row.deposits.to_bits() == d0.to_bits() && row_face(&w) == f0, "no trade inside the band");
    // (b) above the band by x/24: buys x/24 of bonds
    w.row.deposits = x + tp + x / 24.0;
    let d1 = w.row.deposits;
    monetary::rebalance_row(&mut w);
    let paid = d1 - w.row.deposits;
    assert!((paid - x / 24.0).abs() < 1e-6 * (x / 24.0), "bought {paid} vs {}", x / 24.0);
}

#[test]
fn rv_finance_defaults_are_the_spec_values() {
    // P01, P02, P04, P06 (a default that drifts and no test notices): spec 1.5, finance params.
    let c = Config::default();
    assert!(!c.world_link_finance, "ships off");
    assert_eq!(c.world_fo_theta_stock, 13.0);
    assert_eq!(c.world_fo_theta_trans, 20.0);
    assert_eq!(c.world_fo_trans_rho, 0.944);
    assert_eq!(c.world_fo_dollar_resp, -0.0023);
    assert_eq!(c.world_fo_vix_resp, 1.05);
    assert_eq!(c.world_fo_trend_cn, -80.0);
    assert!((0.944f64.powi(12) - 0.5).abs() < 0.01, "half-life of the transitory term is 12 months");
}

#[test]
fn rv_official_treasury_validation_bounds_are_two_sided() {
    // S01: |bn| <= 2000 on both sides.
    assert!(Shock::OfficialTreasury { bloc: None, bn: -2000.0, months: 1 }.validate().is_ok());
    assert!(Shock::OfficialTreasury { bloc: None, bn: -2000.5, months: 1 }.validate().is_err(), "-2000.5");
    assert!(Shock::OfficialTreasury { bloc: None, bn: f64::NAN, months: 1 }.validate().is_err(), "NaN");
    assert!(Shock::OfficialTreasury { bloc: None, bn: f64::INFINITY, months: 1 }.validate().is_err(), "inf");
}

#[test]
fn rv_finance_diagnostics_at_the_base() {
    // M01-M04: the reported W4-20 moments are right at the base (the last burn-in month's settle).
    let w = sim::new_world(&fcfg(12, 400), 3);
    let d = &w.world.as_ref().unwrap().diag;
    let b = fin(&w).base.as_ref().unwrap();
    assert!((d.fo_share_row_pct - 40.80).abs() < 0.01, "fo_share_row_pct {}", d.fo_share_row_pct);
    let gdp_ann = 12.0 * common::gdp_month_model(&w);
    let want_mv = 100.0 * b.row_mv_b / gdp_ann;
    assert!((d.row_mv_gdp_pct - want_mv).abs() < 0.01 * want_mv, "row_mv_gdp_pct {} vs {want_mv}", d.row_mv_gdp_pct);
    assert!(d.row_mv_gdp_pct > 20.0 && d.row_mv_gdp_pct < 90.0, "RoW market value is a share of annual GDP");
    let want_dep = 100.0 * b.row_dep_b / gdp_ann;
    assert!((d.row_dep_gdp_pct - want_dep).abs() < 0.01 * want_dep.abs().max(1e-9), "row_dep_gdp_pct {} vs {want_dep}", d.row_dep_gdp_pct);
    let ring_mean = fin(&w).ring.iter().sum::<f64>() / 6.0;
    assert!((d.f_ring_mean - ring_mean).abs() < 1e-12, "f_ring_mean {} vs {ring_mean}", d.f_ring_mean);
    assert!(fin(&w).ring.iter().any(|&x| x != 0.0));
    let want_face = 100.0 * row_face(&w) / w.gov.debt_face;
    assert!((d.row_face_share_pct - want_face).abs() < 0.01 * want_face, "row_face_share_pct {} vs {want_face}", d.row_face_share_pct);
    assert!((d.fo_bn - 3773.1).abs() < 1e-6, "fo_bn at the base {}", d.fo_bn);
}

#[test]
fn rv_burn_in_zero_world_captures_the_tranche_at_its_first_step() {
    // X12: a world without burn-in has no base-month `settle` branch; `capture_bases` (first step of the trade module) captures the
    // tranche before the first flow, so the link acts from month 0.
    let w0 = sim::new_world(&fcfg(0, 300), 3);
    assert!(fin(&w0).base.is_none(), "nothing is captured before the first step");
    let mut w = sim::new_world(&fcfg(0, 300), 3);
    sim::step(&mut w);
    let b = fin(&w).base.as_ref().expect("captured at the first step");
    assert!((b.h_b.iter().sum::<f64>() / b.k_fo - 3773.1).abs() < 1e-6);
    assert!(fin(&w).flow_rw != 0.0, "the default drivers (trend, dollar, VIX) trade in month 0: {}", fin(&w).flow_rw);
    assert!((fin(&w).fo_rw - 3773.1).abs() < 100.0, "FO^rw {}", fin(&w).fo_rw);
    assert!(w.world.as_ref().unwrap().diag.fo_bn > 3000.0);
}
