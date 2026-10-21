//! Today's-start tests (docs/CALIBRATION.md, section world-usnow): the dated reference of the trade block, its installation at the end of the steered window, and
//! its neutrality when absent. Expectations are computed here from the table and the formulas of trade.rs.

use super::params::WorldParams;
use super::trade::{TradeState, RET_GOODS};
use super::usnow;
use super::NB;
use crate::config::Config;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::World;

const CN: usize = 1;
const ID: &str = "2026-08-24";

fn ucfg(burn: u32, hh: usize) -> Config {
    Config { burn_in_months: burn, n_households: hh, world_block_enabled: true, world_link_trade: true, world_retaliation: true, world_positions: true, ..Config::default() }
}

fn tr(w: &World) -> &TradeState {
    w.world.as_ref().unwrap().trade.as_ref().unwrap()
}

fn world_with_level(seed: u64, level_pct: f64, months: u32) -> World {
    let mut w = sim::new_world(&ucfg(12, 300), seed);
    w.shocks.schedule(ScheduledShock { at_month: w.tick, shock: Shock::ImportTariff { pct: level_pct } });
    sim::run(&mut w, months);
    w
}

#[test]
fn the_table_is_complete_and_normalised() {
    assert_eq!(usnow::latest_id(), ID);
    let v = usnow::vintage(ID).expect("latest vintage is valid");
    assert!(usnow::known(ID) && !usnow::known("no-such-vintage"));
    for s in 0..RET_GOODS {
        let (m, x): (f64, f64) = ((0..NB).map(|j| v.omega[j][s]).sum(), (0..NB).map(|j| v.export_dest[j][s]).sum());
        assert!((m - 1.0).abs() < 1e-7 && (x - 1.0).abs() < 1e-7, "goods group {s}: import shares sum {m}, export shares sum {x}");
    }
    // China's duty is the highest in every goods group; Canada's and Mexico's are below the average; retaliation is China (and Canada, a little).
    for s in 0..RET_GOODS {
        assert!((0..NB).all(|j| j == CN || v.duty[CN][s] >= v.duty[j][s]), "group {s}");
    }
    assert!(v.ret[CN] > 0.3 && v.ret[4] > 0.0 && v.ret[4] < 0.1 && v.ret.iter().enumerate().all(|(j, r)| j == CN || j == 4 || *r == 0.0));
}

#[test]
fn the_reference_installs_shares_duty_and_retaliation() {
    let mut w = world_with_level(3, 8.22, 4);
    assert!(tr(&w).usnow.is_none());
    assert!(super::rebase_usnow(&mut w, ID));
    let v = usnow::vintage(ID).unwrap();
    let t = tr(&w);
    let u = t.usnow.as_ref().unwrap();
    assert!((u.lvl_ref - 0.0822).abs() < 1e-12);
    for j in 0..NB {
        for s in 0..RET_GOODS {
            assert!((t.tab.omega0[j][s] - v.omega[j][s]).abs() < 1e-9);
            assert_eq!(u.tau_ref[j][s], v.duty[j][s]);
        }
    }
    // each goods group keeps its export total, the destination split is today's
    let wp = WorldParams::load();
    for s in 0..RET_GOODS {
        let old: f64 = (0..NB).map(|j| wp.trade.exports_base[j][s] / wp.trade.gdp_2024_bn).sum();
        let new: f64 = (0..NB).map(|j| t.tab.x0[s][j]).sum();
        assert!((new - old).abs() < 1e-13, "group {s}: {new} vs {old}");
    }
    // the partners' duty on US goods: the goods-weighted mean (by the new export shares) is the measured one, exports start at R = 1
    for j in 0..NB {
        let den: f64 = (0..RET_GOODS).map(|s| t.tab.x0[s][j]).sum();
        let mean: f64 = (0..RET_GOODS).map(|s| t.tab.x0[s][j] * t.ret.tau_ref[j][s]).sum::<f64>() / den;
        assert!((mean - v.ret[j]).abs() < 1e-9, "bloc {j}: {mean} vs {}", v.ret[j]);
        for s in 0..RET_GOODS {
            assert_eq!(t.ret.tau_star[j][s], t.ret.tau_ref[j][s]);
            assert_eq!(t.ret.r[j][s], 1.0);
        }
    }
}

#[test]
fn with_no_change_the_block_holds_todays_state() {
    // The quarter-point bound on the memo below holds on the release ABM path of this 300-household world; `bank_loss_carry` moves
    // that path (the memo then moves by up to 0.26 points), so the test runs with its legacy value. `world_with_level` otherwise.
    let mut w = sim::new_world(&Config { bank_loss_carry: false, ..ucfg(12, 300) }, 3);
    w.shocks.schedule(ScheduledShock { at_month: w.tick, shock: Shock::ImportTariff { pct: 8.22 } });
    sim::run(&mut w, 4);
    super::rebase_usnow(&mut w, ID);
    let mut memo = Vec::new();
    for _ in 0..8 {
        sim::step(&mut w);
        let d = &w.world.as_ref().unwrap().diag;
        memo.push(d.tau_by_origin);
        assert!(d.ret_tau[CN] > 0.0, "today's retaliation stays");
        let t = tr(&w);
        for j in 0..NB {
            for s in 0..RET_GOODS {
                assert_eq!(t.ret.tau_star[j][s], t.ret.tau_ref[j][s], "retaliation holds");
                assert_eq!(t.ret.r[j][s], 1.0, "the export factor stays one");
            }
        }
    }
    // the memo is the reference duty by origin, goods-weighted: China's near 0.26 in every month, and it moves by less than a quarter point
    for m in &memo {
        assert!(m[CN] > 0.20 && m[CN] < 0.30, "{}", m[CN]);
        assert!((m[CN] - memo[0][CN]).abs() < 0.0025);
    }
    // the ABM's duty is the set-level duty, unchanged by the reference
    assert!((w.world.as_ref().unwrap().diag.tau_eff_hh - 0.0822).abs() < 1e-12);
}

#[test]
fn a_tariff_adds_points_to_todays_duty() {
    let run = |tariff: bool| {
        let mut w = world_with_level(5, 8.22, 4);
        super::rebase_usnow(&mut w, ID);
        if tariff {
            w.shocks.schedule(ScheduledShock { at_month: w.tick, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 25.0, retaliate: true } });
        }
        sim::run(&mut w, 3);
        w
    };
    let (a, b) = (run(false), run(true));
    let (da, db) = (&a.world.as_ref().unwrap().diag, &b.world.as_ref().unwrap().diag);
    // the shares move after the shock, so the weights of the reference part do too: the difference is 0.25 to within a fifth of a point
    assert!((db.tau_by_origin[CN] - da.tau_by_origin[CN] - 0.25).abs() < 2e-3, "{} vs {}", db.tau_by_origin[CN], da.tau_by_origin[CN]);
    // the partner's response is on top of the duty it already has: 0.319 plus the rule's 0.95 w_s x 0.25, spread over the goods groups
    let v = usnow::vintage(ID).unwrap();
    assert!(db.ret_tau[CN] > da.ret_tau[CN] + 0.2 && da.ret_tau[CN] > 0.3, "{} {}", db.ret_tau[CN], da.ret_tau[CN]);
    assert!((da.ret_tau[CN] - v.ret[CN]).abs() < 1e-9);
}

#[test]
fn the_share_response_is_measured_against_the_reference() {
    // +25 points on China from a duty of about 0.36 (durables) moves the log duty by ln(1.61 / 1.36) = 0.169 against ln(1.25) = 0.223 from zero:
    // the origin-share response is about three quarters of the one the typical start gives.
    let shock = |w: &mut World| w.shocks.schedule(ScheduledShock { at_month: w.tick, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 25.0, retaliate: false } });
    let change = |reference: bool| {
        let mut base = world_with_level(5, 8.22, 4);
        if reference {
            super::rebase_usnow(&mut base, ID);
        }
        let mut shocked = base.clone();
        shock(&mut shocked);
        sim::run(&mut base, 6);
        sim::run(&mut shocked, 6);
        let (b, s) = (tr(&base).omega[CN][2], tr(&shocked).omega[CN][2]);
        (s / b).ln()
    };
    let (with_ref, without) = (change(true), change(false));
    let ratio = with_ref / without;
    assert!(with_ref < 0.0 && without < 0.0, "{with_ref} {without}");
    assert!(ratio > 0.55 && ratio < 0.95, "ratio {ratio}: {with_ref} against {without}");
}

#[test]
fn without_a_reference_nothing_is_added() {
    let mut w = world_with_level(7, 8.22, 6);
    let t = tr(&w);
    assert!(t.usnow.is_none());
    assert!(t.ret.tau_ref.iter().flatten().all(|x| *x == 0.0) && t.ret.x_ref.iter().flatten().all(|x| *x == 0.0));
    sim::step(&mut w);
    let m = serde_json::to_value(w.history.last().unwrap()).unwrap();
    assert!(m["world"].get("imports_goods_bn").is_none(), "the new key appears only with the reference");
    // an unknown vintage installs nothing
    assert!(!super::rebase_usnow(&mut w, "no-such-vintage"));
    assert!(tr(&w).usnow.is_none());
    // with the trade link off the reference is not installed either
    let mut off = sim::new_world(&Config { world_link_trade: false, ..ucfg(12, 300) }, 7);
    assert!(!super::rebase_usnow(&mut off, ID));
}

#[test]
fn the_import_demand_term_is_continuous_at_the_reference() {
    // The blocs read ln(Q_j / Y), the real imports from bloc j over output, against a slow benchmark learned since the base. Installing today's
    // shares changes Q_j with no change in demand; the benchmark, its warm-up sum and last month's quantity move by the same log, so the
    // deviation the term is built from is unchanged at the reference, bloc by bloc.
    let w = world_with_level(5, 8.22, 18);
    let dev = |w: &World, j: usize| {
        let t = tr(w);
        assert!(t.bench_n[j] >= 12 && t.q_prev[j] > 0.0, "the benchmark is live after 18 reported months");
        t.q_prev[j].ln() - t.bench[j]
    };
    let before: Vec<f64> = (0..NB).map(|j| dev(&w, j)).collect();
    let mut b = w.clone();
    assert!(super::rebase_usnow(&mut b, ID));
    for j in 0..NB {
        assert!((dev(&b, j) - before[j]).abs() < 1e-9, "bloc {j}: {} against {}", dev(&b, j), before[j]);
    }
    // and the quantities did move: China's weight fell by about a third, Mexico's rose
    let (qa, qb) = (tr(&w).q_prev[CN], tr(&b).q_prev[CN]);
    assert!(qb < 0.8 * qa, "China's last-month quantity is restated at today's share: {qb} against {qa}");
}
