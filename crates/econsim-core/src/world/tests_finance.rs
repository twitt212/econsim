//! Stage 4-2 tests (spec 2.7 official part, D19, D32, D37, D39, D43-D46; the 22 of section 5 of the stage's implementer task description). Every
//! expectation is computed here from the spec formulas and from states the code does not compute in the same step (the recursion of
//! the trend-only path, the rule's value from the realised `ln D` and the ring). Worlds: `sim::new_world` (burn-in 12, 300-400
//! households, the profile's f keys) or `sim::step` on it; `quiet` sets the three drivers to zero.

use super::finance::{self, FinState, OfficialProg};
use super::{bloc_index, world_row_extra, world_tp_fo, NB};
use crate::common;
use crate::config::Config;
use crate::exog;
use crate::fiscal;
use crate::ledger;
use crate::monetary;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::{BondHolder, World};

/// The official holdings of the base month, real-world $bn (finance `foreign_official_ust_bn`).
const FO_RW: f64 = 3773.1;

/// Finance link on (the trade link on, the dollar link off), the profile's f-process keys; the drivers at their defaults.
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

/// Every driver of the flow rule at zero: only shocks move the tranche.
fn quiet(mut c: Config) -> Config {
    c.world_fo_trend_cn = 0.0;
    c.world_fo_dollar_resp = 0.0;
    c.world_fo_vix_resp = 0.0;
    c
}

fn fin(w: &World) -> &FinState {
    w.world.as_ref().unwrap().fin.as_ref().unwrap()
}

fn fx0_of(w: &World) -> f64 {
    w.world.as_ref().unwrap().trade.as_ref().unwrap().base.as_ref().unwrap().fx0
}

/// RoW's total face holdings (the market value is refreshed only by `mark_to_market`, so the tests read faces).
fn row_face(w: &World) -> f64 {
    w.bonds.iter().map(|b| b.held_by(BondHolder::RestOfWorld)).sum()
}

/// One call of the official block for the month, as `world::step` makes it (the block is taken out of `w`).
fn flows(w: &mut World, rep: bool) {
    let fx0 = Some(fx0_of(w));
    let mut wb = w.world.take().unwrap();
    finance::step_flows(w, wb.fin.as_mut().unwrap(), fx0, rep);
    w.world = Some(wb);
}

/// H_j of the state (real-world $bn): the memo path at the last trend read plus the deviation.
fn h_rw(w: &World, j: usize) -> f64 {
    let f = fin(w);
    let b = f.base.as_ref().unwrap();
    (b.h_b[j] * f.tau_prev / b.tau_b + f.dh[j]) / b.k_fo
}

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

fn json(m: &crate::metrics::Metrics) -> serde_json::Value {
    serde_json::to_value(m).unwrap()
}

#[test]
fn official_flow_monthly_rule_hand_values() {
    // D32: N_j = eta 100 (ln D_t - ln D_{t-1}) H_j,t-1 / k_FO, eta = -0.0023, trend and VIX off. A one-month step of ln D by ln 1.10
    // (9.531 log points) gives -0.0023 x 9.531 x 3773.1 = -82.71 bn. A gradual path to the same level over six months gives the six-month
    // sum eta x 100 ln D_6 x FO^rw (the telescoped monthly rule; the task description writes an extra factor 6, see the stage result note (not published)).
    let mut c = fcfg(12, 400);
    c.world_fo_trend_cn = 0.0;
    c.world_fo_vix_resp = 0.0;
    let eta = c.world_fo_dollar_resp;
    // The step is applied to the base state (ln D_b = 0, FO^rw = 3773.1, tau = tau_b): no month has run yet.
    let mut w = sim::new_world(&c, 3);
    let fx0 = fx0_of(&w);
    let ln0 = fin(&w).ln_d_prev;
    let fo_prev = fin(&w).fo_rw;
    assert!((fo_prev - FO_RW).abs() < 1e-6, "FO^rw at the base {fo_prev}");
    w.row.fx = fx0 * (ln0 + 0.10_f64.ln_1p()).exp();
    flows(&mut w, true);
    let want = eta * 100.0 * 0.10_f64.ln_1p() * fo_prev;
    assert!(rel(fin(&w).flow_rw, want) < 1e-9, "one-month step {} vs {want}", fin(&w).flow_rw);
    assert!((fin(&w).flow_rw + 82.71).abs() < 0.01, "hand value -82.71: {}", fin(&w).flow_rw);

    // Gradual path: ln D rises linearly to ln 1.10 over six months, from the base.
    let mut w = sim::new_world(&c, 3);
    let fx0 = fx0_of(&w);
    let ln0 = fin(&w).ln_d_prev;
    let fo_b = fin(&w).fo_rw;
    let mut sum = 0.0;
    for k in 1..=6 {
        w.row.fx = fx0 * (ln0 + 0.10_f64.ln_1p() * f64::from(k) / 6.0).exp();
        flows(&mut w, true);
        sum += fin(&w).flow_rw;
    }
    let want6 = eta * 100.0 * 0.10_f64.ln_1p() * fo_b;
    assert!((sum - want6).abs() <= 0.02 * want6.abs(), "six-month sum {sum} vs {want6}");
}

#[test]
fn fo_rule_flows_at_realised_step() {
    // The ABM's own dollar (dollar link off): a permanent Dollar +10 % at reported month 0 moves ln D_h = 9.531 (1 - 0.7^h) before
    // feedback. Paired against the unshocked world of the same seed: the impact flow is -24.81 bn and the six-month sum -73 bn.
    let mut c = quiet(fcfg(12, 400));
    c.world_fo_dollar_resp = -0.0023;
    let mut a = sim::new_world(&c, 3);
    let mut b = sim::new_world(&c, 3);
    b.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } });
    let mut d = Vec::new();
    for _ in 0..6 {
        sim::step(&mut a);
        sim::step(&mut b);
        d.push(fin(&b).flow_rw - fin(&a).flow_rw);
    }
    assert!((d[0] + 24.81).abs() < 2.0, "impact flow {}", d[0]);
    let cum: f64 = d.iter().sum();
    assert!((cum + 73.0).abs() < 5.0, "cumulative through h = 6: {cum}");
}

#[test]
fn official_execute_is_ledger_consistent() {
    // Default drivers (the CN trend, the dollar and the VIX terms) with a +100 purchase and a -400 sale over six months, 24 months.
    // The ledger is checked every month; `extra` equals minus the executed flows at market value (model units).
    let mut w = sim::new_world(&fcfg(12, 400), 5);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    w.shocks.schedule(ScheduledShock { at_month: 6, shock: Shock::OfficialTreasury { bloc: None, bn: -400.0, months: 6 } });
    let k = fin(&w).base.as_ref().unwrap().k_fo;
    let mut executed = 0.0;
    for _ in 0..24 {
        sim::step(&mut w);
        ledger::check_consistency(&w).expect("stock-flow consistency");
        executed += fin(&w).flow_rw * k;
    }
    let extra = fin(&w).extra;
    assert!(rel(extra, -executed) <= 1e-9, "extra {extra} vs minus executed {}", -executed);
    assert!(fin(&w).extra > 0.0, "net sales recorded");
}

#[test]
fn tp_official_100bn_hand_values() {
    // Quiet world, OfficialTreasury{CN, +100, 1} at month 0. The term in force after the flow falls 10.14 bp (stock 3.99 + transitory
    // 6.14), 7.25 bp thirteen months later, and tends to about -4.3 bp. Two worlds whose RoW/GDP differ by more than 10 % give the
    // same bp: the units are real-world (D39).
    let mut firsts = Vec::new();
    let mut ratios = Vec::new();
    for big_row in [false, true] {
        let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
        if big_row {
            // RoW's market value 1.4 times larger when the base is captured (the same seed; the tranche follows RoW's holdings)
            w.row.bonds_value *= 1.4;
            let mut wb = w.world.take().unwrap();
            wb.fin.as_mut().unwrap().base = None;
            wb.fin.as_mut().unwrap().capture(&w);
            w.world = Some(wb);
        }
        let gdp = 12.0 * common::gdp_month_model(&w);
        ratios.push(fin(&w).base.as_ref().unwrap().row_mv_b / gdp);
        w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
        let mut path = Vec::new();
        for _ in 0..40 {
            sim::step(&mut w);
            path.push(1e4 * finance::tp_finance(&w, 3)); // the term in force in the next month's curve, bp
        }
        assert!(fin(&w).flow_rw.abs() < 1e-9, "no flow after the shock month");
        assert!((path[0] + 10.14).abs() < 0.5, "first month after the flow {}", path[0]);
        assert!((path[12] + 7.25).abs() < 0.5, "13 months after the flow {}", path[12]);
        assert!(path[39] < -4.0 && path[39] > -4.9, "month 40 {}", path[39]);
        firsts.push(path[0]);
    }
    assert!((ratios[1] / ratios[0] - 1.0).abs() > 0.1, "RoW/GDP differ by more than 10 %: {ratios:?}");
    assert!((firsts[0] - firsts[1]).abs() < 1e-6, "same bp for both worlds: {firsts:?}");
}

#[test]
fn official_tp_uses_real_world_units() {
    // Two worlds of one seed; in the second RoW's market value is 1.5 times larger when the base is captured (only k_FO changes; the
    // tranche is real-world). The term premium and the executed flow are the same to 1e-9.
    let c = quiet(fcfg(12, 400));
    let mut a = sim::new_world(&c, 3);
    let mut b = sim::new_world(&c, 3);
    b.row.bonds_value *= 1.5;
    let mut wb = b.world.take().unwrap();
    {
        let fs = wb.fin.as_mut().unwrap();
        fs.base = None;
        fs.capture(&b);
    }
    b.world = Some(wb);
    let k_a = fin(&a).base.as_ref().unwrap().k_fo;
    let k_b = fin(&b).base.as_ref().unwrap().k_fo;
    assert!(rel(k_b, 1.5 * k_a) < 1e-9, "k_FO scales with RoW's market value");
    for w in [&mut a, &mut b] {
        w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    }
    for _ in 0..12 {
        sim::step(&mut a);
        sim::step(&mut b);
        let (ta, tb) = (1e4 * finance::tp_finance(&a, 3), 1e4 * finance::tp_finance(&b, 3));
        assert!((ta - tb).abs() < 1e-9, "tp {ta} vs {tb}");
        assert!((fin(&a).flow_rw - fin(&b).flow_rw).abs() < 1e-9, "flow");
    }
}

#[test]
fn fo_base_captured_and_normalised() {
    let mut w = sim::new_world(&fcfg(12, 400), 3);
    let fs = fin(&w);
    let b = fs.base.as_ref().expect("tranche captured at the end of the last burn-in month").clone();
    // the base exists right after `new_world` returns: tau_b is the trend of the last burn-in month (the trend does not change after it)
    assert!(rel(b.tau_b, fiscal::trend_gdp_month(&w)) < 1e-12, "tau_b equals the trend after the burn-in");
    let fo_model: f64 = b.h_b.iter().sum();
    let share = fo_model / b.row_mv_b;
    assert!((100.0 * share - 40.80).abs() < 0.01, "official share of RoW holdings {}", 100.0 * share);
    assert!((fo_model / b.k_fo - FO_RW).abs() < 1e-6, "FO^rw_b {}", fo_model / b.k_fo);
    let expect = [194.4, 585.1, 660.0, 149.2, 42.5, 75.7, 191.8, 111.4, 159.0, 20.1, 279.8, 1304.0];
    for j in 0..NB {
        let hj = b.h_b[j] / b.k_fo;
        assert!((hj - expect[j]).abs() < 0.06, "H_{j} {hj} vs {}", expect[j]);
    }
    // W4-20 (i): at creation (a burn-in-0 world, before any step) RoW holds 30.0 % of the face debt (28 to 31 %, init ROW_BOND_SHARE).
    let w0 = sim::new_world(&fcfg(0, 400), 3);
    let face_share = 100.0 * row_face(&w0) / w0.gov.debt_face;
    assert!((28.0..=31.0).contains(&face_share), "RoW face share at creation {face_share}");
    // nu from the sourced tables: 3773.1 over sum_j official_share_j tic_j.
    let p = super::params::WorldParams::load();
    let s: f64 = p.finance.tic_ust_bn.iter().zip(&p.finance.official_share).map(|(t, o)| t * o).sum();
    let nu = FO_RW / s;
    assert!((nu - 0.99652).abs() < 5e-6, "nu {nu}");
    // not re-captured after three reported months, and not by a scheduled shock
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    for _ in 0..3 {
        sim::step(&mut w);
    }
    let b2 = fin(&w).base.as_ref().unwrap();
    assert!(b2.tau_b.to_bits() == b.tau_b.to_bits(), "tau_b unchanged");
    assert!(b2.h_b.iter().zip(&b.h_b).all(|(x, y)| x.to_bits() == y.to_bits()), "h_b unchanged");
}

#[test]
fn fo_first_month_is_continuous() {
    // (a) quiet drivers: at reported month 0 the term in force is 0.0 bit for bit, the states are zero, and FO^rw is 3773.1 tau/tau_b.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    {
        let f = fin(&w);
        assert!(f.stock_pp == 0.0 && f.t_trans == 0.0 && f.dh.iter().all(|&x| x == 0.0), "zero states at the base");
    }
    let tau = fiscal::trend_gdp_month(&w);
    let tau_b = fin(&w).base.as_ref().unwrap().tau_b;
    sim::step(&mut w);
    assert!(fin(&w).tp10_fo_bp.to_bits() == 0.0f64.to_bits(), "tp_fo at month 0: {}", fin(&w).tp10_fo_bp);
    assert!(fin(&w).stock_pp == 0.0 && fin(&w).t_trans == 0.0, "no flow in the quiet month");
    assert!(rel(fin(&w).fo_rw, FO_RW * tau / tau_b) < 1e-9, "FO^rw {} vs {}", fin(&w).fo_rw, FO_RW * tau / tau_b);

    // (b) the dollar term acts at month 0: the flow is the rule's value from the realised ln D_0 and the ring.
    let c = fcfg(12, 400);
    let mut w = sim::new_world(&c, 3);
    let fx0 = fx0_of(&w);
    let k = fin(&w).base.as_ref().unwrap().k_fo;
    let hrw: [f64; NB] = std::array::from_fn(|j| fin(&w).base.as_ref().unwrap().h_b[j] / k);
    sim::step(&mut w);
    let dln = 100.0 * (w.row.fx / fx0).ln();
    let ring_mean: f64 = fin(&w).ring.iter().sum::<f64>() / 6.0;
    let mut want = 0.0;
    for j in 0..NB {
        let mut n = c.world_fo_dollar_resp * dln * hrw[j] + c.world_fo_vix_resp * ring_mean * hrw[j] / FO_RW;
        if j == bloc_index("CN") {
            n += c.world_fo_trend_cn / 12.0;
        }
        want += n.max(-hrw[j]);
    }
    assert!((fin(&w).flow_rw - want).abs() < 1e-6 * want.abs().max(1.0), "month-0 flow {} vs {want}", fin(&w).flow_rw);
    assert!(dln.abs() > 0.0, "the ABM's own month-0 step is not zero");
}

#[test]
fn fo_stock_ratio_constant_in_baseline() {
    // (a) quiet, 120 months: the term is 0.0 every month and FO^rw = 3773.1 tau_t/tau_b to 1e-9.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    let tau_b = fin(&w).base.as_ref().unwrap().tau_b;
    for _ in 0..120 {
        let tau = fiscal::trend_gdp_month(&w);
        sim::step(&mut w);
        assert!(finance::tp_finance(&w, 3) == 0.0, "tp_fo is zero in the baseline");
        assert!(rel(fin(&w).fo_rw, FO_RW * tau / tau_b) < 1e-9, "FO^rw ratio");
    }
    // (b) trend only (-80 a year): the 10y term is the test's own recursion on the realised trend path, with the per-bloc cap.
    let mut c = quiet(fcfg(12, 400));
    c.world_fo_trend_cn = -80.0;
    let mut w = sim::new_world(&c, 3);
    let (tau_b, h0, _k) = {
        let b = fin(&w).base.as_ref().unwrap();
        (b.tau_b, b.h_b[bloc_index("CN")] / b.k_fo, b.k_fo)
    };
    let (mut d, mut t, mut tau_prev) = (0.0_f64, 0.0_f64, tau_b);
    for m in 0..120 {
        let tau = fiscal::trend_gdp_month(&w);
        sim::step(&mut w);
        let hprev = h0 * tau_prev / tau_b + d;
        let flow = (c.world_fo_trend_cn / 12.0).max(-hprev);
        d = d * (tau / tau_prev) + flow;
        let gdp = c.world_gdp_rw_bn * tau / tau_b;
        t = c.world_fo_trans_rho * t + 100.0 * flow / gdp;
        let stock = 100.0 * d / gdp;
        let want_bp = -(c.world_fo_theta_stock * stock + c.world_fo_theta_trans * t);
        let got_bp = 1e4 * finance::tp_finance(&w, 3);
        assert!((got_bp - want_bp).abs() < 1e-9, "month {m}: {got_bp} vs {want_bp}");
        tau_prev = tau;
    }
}

#[test]
fn vix_ring_filled_in_shadow() {
    // (a) burn-in 0 (every month reported): the ring holds the last six values of VIX per unit x f_t minus Fbar, this month included.
    let c = fcfg(0, 300);
    let mut w = sim::new_world(&c, 9);
    let unit = c.world_vix_per_unit;
    let fbar = unit * exog::fin_factor_mean(&c);
    let mut vals = Vec::new();
    for _ in 0..20 {
        sim::step(&mut w);
        vals.push(unit * w.exog.fin_factor - fbar);
        if vals.len() >= 6 {
            let ring = fin(&w).ring;
            for (k, v) in ring.iter().enumerate() {
                assert!((v - vals[vals.len() - 6 + k]).abs() < 1e-12, "ring entry {k}");
            }
        }
    }
    // (b) burn-in 12: after the burn-in the ring holds the last burn-in month's value; a reported step moves the window by one.
    let c = fcfg(12, 400);
    let mut w = sim::new_world(&c, 3);
    let fbar = c.world_vix_per_unit * exog::fin_factor_mean(&c);
    assert!((fin(&w).ring[5] - (c.world_vix_per_unit * w.exog.fin_factor - fbar)).abs() < 1e-12, "ring after burn-in");
    assert!(fin(&w).ring.iter().any(|&x| x != 0.0), "the ring is filled in shadow");
    let before = fin(&w).ring;
    sim::step(&mut w);
    assert!((fin(&w).ring[5] - (c.world_vix_per_unit * w.exog.fin_factor - fbar)).abs() < 1e-12, "current month in the ring");
    assert!((fin(&w).ring[4] - before[5]).abs() < 1e-12, "window moved by one");
}

#[test]
fn fo_vix_term_hand_values() {
    // F - Fbar = 14 VIX points for six months: the flow is 1.05 x 14 k/6 in month k (pro rata to H_j), and the first month falls out of
    // the window after the sixth (month 7 at 0 points: 1.05 x 70/6).
    let mut c = quiet(fcfg(12, 400));
    c.world_fo_vix_resp = 1.05;
    let mut w = sim::new_world(&c, 3);
    let ffm = exog::fin_factor_mean(&w.cfg);
    {
        let mut wb = w.world.take().unwrap();
        wb.fin.as_mut().unwrap().ring = [0.0; 6];
        w.world = Some(wb);
    }
    let dh0: Vec<f64> = fin(&w).dh.to_vec();
    assert!(dh0.iter().all(|&x| x == 0.0));
    for k in 1..=6 {
        w.exog.fin_factor = ffm + 1.4;
        flows(&mut w, true);
        let want = 1.05 * 14.0 * f64::from(k) / 6.0;
        assert!((fin(&w).flow_rw - want).abs() < 1e-9 * want, "month {k}: {} vs {want}", fin(&w).flow_rw);
    }
    // pro rata to H_j: the deviations of the six months are proportional to the tranche of each bloc
    let b = fin(&w).base.as_ref().unwrap().clone();
    let r0 = fin(&w).dh[0] / b.h_b[0];
    for j in 0..NB {
        assert!(rel(fin(&w).dh[j] / b.h_b[j], r0) < 1e-9, "pro rata to H_j for bloc {j}");
    }
    // month 7: the first month (14 points) falls out of the window
    w.exog.fin_factor = ffm;
    flows(&mut w, true);
    let want7 = 1.05 * 70.0 / 6.0;
    assert!((fin(&w).flow_rw - want7).abs() < 1e-9 * want7, "month 7: {} vs {want7}", fin(&w).flow_rw);
}

#[test]
fn qe_and_official_terms_add() {
    // (a) QE 0.5 % of GDP a month for 12 months with the default drivers: the explicit term's own response (the paired difference of
    // the term in force with QE and without) stays within 0.5 bp. (b) QE alone changes no tranche deviation: dh is zero throughout with
    // the drivers at zero. (c) an official trade leaves the private duration unchanged (1e-12 relative).
    let c = fcfg(12, 400);
    let mut on = sim::new_world(&c, 3);
    let mut onq = sim::new_world(&c, 3);
    onq.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 } });
    let mut worst = 0.0_f64;
    for _ in 0..12 {
        sim::step(&mut on);
        sim::step(&mut onq);
        worst = worst.max((fin(&onq).tp10_fo_bp - fin(&on).tp10_fo_bp).abs());
    }
    assert!(worst <= 0.5, "explicit term's response to QE: {worst} bp");

    let mut q = sim::new_world(&quiet(fcfg(12, 400)), 3);
    q.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 } });
    for _ in 0..12 {
        sim::step(&mut q);
        assert!(fin(&q).dh.iter().all(|&x| x == 0.0), "QE changes no tranche deviation");
    }

    let mut t = sim::new_world(&quiet(fcfg(12, 400)), 3);
    let d0 = monetary::private_duration(&t);
    let tick = t.tick;
    let mut wb = t.world.take().unwrap();
    wb.fin.as_mut().unwrap().progs.push(OfficialProg { bloc: Some(bloc_index("CN")), per_month: 100.0, t0: tick, months: 1 });
    t.world = Some(wb);
    flows(&mut t, true);
    assert!((fin(&t).flow_rw - 100.0).abs() < 1e-6, "the purchase executed");
    assert!(rel(monetary::private_duration(&t), d0) < 1e-12, "private duration unchanged by an official trade");
}

#[test]
fn rebalance_row_hook_keeps_extra_deposits() {
    // After an executed official SALE the private RoW book is untouched by rebalance_row inside the band, and below zero (the adjusted
    // deposits, not the raw ones) it sells target' - dep_adj, target' = (bonds + x) / 12 + 0.5 GDP (D45, S4-R16 item 3).
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: -400.0, months: 1 } });
    sim::step(&mut w);
    assert!(fin(&w).extra > 0.0, "net sales recorded");
    let gdp = common::gdp_month_model(&w);
    let x = fin(&w).extra;
    // inside the band: adjusted deposits between zero and target'
    w.row.deposits = x + (w.row.bonds_value + x) / 12.0 + 0.25 * gdp;
    let face0 = row_face(&w);
    monetary::rebalance_row(&mut w);
    assert_eq!(row_face(&w), face0, "no bond trade inside the band");
    // below zero for the adjusted deposits while the raw deposits are positive
    w.row.deposits = x - 0.01 * gdp;
    assert!(w.row.deposits > 0.0, "raw deposits positive");
    let want = (w.row.bonds_value + x) / 12.0 + 0.5 * gdp + 0.01 * gdp;
    let face1 = row_face(&w);
    monetary::rebalance_row(&mut w);
    let sold = face1 - row_face(&w);
    assert!(sold > 0.0 && (sold - want).abs() < 0.05 * want, "sold face {sold} vs market value {want}");
}

#[test]
fn tranche_clipped_to_row_holdings() {
    // (a) a request of -2000 $bn against the CN tranche of 585: 585 executes, H_CN ends at 0 and is never negative.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    let h_cn = fin(&w).base.as_ref().unwrap().h_b[1] / fin(&w).base.as_ref().unwrap().k_fo;
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: -2000.0, months: 1 } });
    sim::step(&mut w);
    assert!((fin(&w).flow_rw + h_cn).abs() < 1e-6 * h_cn, "585 executes: {}", fin(&w).flow_rw);
    let left = h_rw(&w, 1);
    assert!(left.abs() <= 1e-9 * h_cn && left >= -1e-9, "H_CN ends at 0: {left}");
    // (b) RoW holds 2 % of its bonds: the sale executes what RoW can deliver (all of its holdings), not the request.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    let row_before = row_face(&w);
    for idx in 0..w.bonds.len() {
        let f = w.bonds[idx].held_by(BondHolder::RestOfWorld) * 0.98;
        if f > 0.0 {
            ledger::bond_transfer(&mut w, idx, BondHolder::RestOfWorld, BondHolder::Fund, f, 0.0);
        }
    }
    assert!(row_face(&w) < 0.05 * row_before, "RoW cut to 2 %");
    // the request of -2000 $bn for the month, through the programme list (the flow of the month, read before the ABM's RoW rebalance)
    let tick = w.tick;
    let mut wb = w.world.take().unwrap();
    wb.fin.as_mut().unwrap().progs.push(OfficialProg { bloc: Some(bloc_index("CN")), per_month: -2000.0, t0: tick, months: 1 });
    w.world = Some(wb);
    flows(&mut w, true);
    assert!(fin(&w).flow_rw < 0.0 && -fin(&w).flow_rw < 0.5 * h_cn, "a partial sale: {}", fin(&w).flow_rw);
    assert!(row_face(&w) < 1e-6 * row_before, "RoW delivered all of its holdings: {} of {row_before}", row_face(&w));
    assert!(h_rw(&w, 1) > 0.0 && h_rw(&w, 1) >= -1e-9, "H_CN is not negative: {}", h_rw(&w, 1));
}

#[test]
fn credibility_tp_part() {
    // UsCredibility{tp 22, months 3, dollar 3.3} with the finance link only: the 10y term premium is 22.0 bp at ages 0 to 2, then
    // 22 x 0.5^((age-2)/3) (17.46, 13.86); the 2y 4.4 bp, the 3m zero. The curve of the shock month carries it (against the link off).
    let c = quiet(fcfg(12, 400));
    let mut on = sim::new_world(&c, 3);
    let mut off = sim::new_world(&Config { world_link_finance: false, ..c.clone() }, 3);
    for w in [&mut on, &mut off] {
        w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::UsCredibility { tp_bp: 22.0, months: 3, dollar_pct: 3.3 } });
    }
    sim::step(&mut on);
    sim::step(&mut off);
    assert_eq!(finance::tp_finance(&on, 3), 0.0, "no credibility before the shock");
    sim::step(&mut on);
    sim::step(&mut off);
    let d10 = 1e4 * (on.curve.term_premium[3] - off.curve.term_premium[3]);
    let d2 = 1e4 * (on.curve.term_premium[1] - off.curve.term_premium[1]);
    let d3m = on.curve.term_premium[0] - off.curve.term_premium[0];
    assert!((d10 - 22.0).abs() < 1e-9, "the shock month's curve: 10y {d10}");
    assert!((d2 - 4.4).abs() < 1e-9, "2y {d2}");
    assert!(d3m.abs() < 1e-15, "3m zero");
    for (age, want) in [(0, 22.0), (1, 22.0), (2, 22.0), (3, 22.0 * 0.5f64.powf(1.0 / 3.0)), (4, 22.0 * 0.5f64.powf(2.0 / 3.0))] {
        let got = 1e4 * fin(&on).tp_cred(3, 1 + age);
        assert!((got - want).abs() < 1e-9, "age {age}: {got} vs {want}");
    }
    // identical with the dollar link also on
    let mut dl = sim::new_world(&Config { world_link_dollar: true, ..c }, 3);
    dl.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::UsCredibility { tp_bp: 22.0, months: 3, dollar_pct: 3.3 } });
    for _ in 0..5 {
        sim::step(&mut dl);
    }
    for age in 0..5u32 {
        let tick = 1 + age;
        let want = 22.0 * super::dollar::cred_profile(age, 3);
        assert!((1e4 * fin(&dl).tp_cred(3, tick) - want).abs() < 1e-9 * want.max(1.0), "dollar link on, age {age}");
    }
}

#[test]
fn official_treasury_shock_path() {
    // {CN, +100, 1}: executes 100.0 in the shock month and nothing after. {null, -400, 6}: -66.67 a month; dh_j proportional to H_b,j.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    sim::step(&mut w);
    assert!((fin(&w).flow_rw - 100.0).abs() < 1e-6, "executed {}", fin(&w).flow_rw);
    sim::step(&mut w);
    assert!(fin(&w).flow_rw == 0.0, "nothing after the shock month");

    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: None, bn: -400.0, months: 6 } });
    for _ in 0..6 {
        sim::step(&mut w);
        assert!((fin(&w).flow_rw + 400.0 / 6.0).abs() < 1e-6, "-66.67 a month: {}", fin(&w).flow_rw);
    }
    let b = fin(&w).base.as_ref().unwrap().clone();
    let f = fin(&w);
    let r0 = f.dh[0] / b.h_b[0];
    for j in 0..NB {
        assert!(rel(f.dh[j] / b.h_b[j], r0) < 1e-9, "dh proportional to H_b, bloc {j}");
    }
    // validation
    let ok = |s: Shock| s.validate().is_ok();
    assert!(ok(Shock::OfficialTreasury { bloc: None, bn: 100.0, months: 1 }));
    assert!(ok(Shock::OfficialTreasury { bloc: Some("CN".into()), bn: -2000.0, months: 60 }));
    assert!(!ok(Shock::OfficialTreasury { bloc: Some("XX".into()), bn: 1.0, months: 1 }), "bloc XX");
    assert!(!ok(Shock::OfficialTreasury { bloc: None, bn: 2001.0, months: 1 }), "|bn| > 2000");
    assert!(!ok(Shock::OfficialTreasury { bloc: None, bn: 1.0, months: 0 }), "months 0");
    assert!(!ok(Shock::OfficialTreasury { bloc: None, bn: 1.0, months: 61 }), "months 61");
    // ignored with the link off: the same ABM as without the shock
    let off = Config { world_link_finance: false, ..quiet(fcfg(12, 400)) };
    let mut a = sim::new_world(&off, 3);
    let mut b = sim::new_world(&off, 3);
    b.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    for _ in 0..6 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    for m in 0..6 {
        assert_eq!(json(&a.history[m]), json(&b.history[m]), "month {m} identical with the link off");
    }
}

#[test]
fn official_purchase_executes_in_full_when_banks_hold_no_bonds() {
    // Every bank holding moves to the Fund (faces only: price 0). A +100 purchase then executes in full; a bank-only list would not.
    let mut w = sim::new_world(&quiet(fcfg(12, 400)), 3);
    for idx in 0..w.bonds.len() {
        for b in 0..w.banks.len() {
            let h = BondHolder::Bank(b as crate::types::BankId);
            let face = w.bonds[idx].held_by(h);
            if face > 0.0 {
                ledger::bond_transfer(&mut w, idx, h, BondHolder::Fund, face, 0.0);
            }
        }
    }
    assert!(w.bonds.iter().all(|b| (0..w.banks.len()).all(|k| b.held_by(BondHolder::Bank(k as crate::types::BankId)) < 1e-9)));
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 } });
    sim::step(&mut w);
    assert!((fin(&w).flow_rw - 100.0).abs() < 1e-6, "executed {}", fin(&w).flow_rw);
}

#[test]
fn finance_pair_identical_before_shock() {
    // A shock at reported month t0 in one of two worlds of one seed (the OfficialTreasury flow and the UsCredibility term): every
    // output and state and every ABM key is bit-identical before t0; at t0 the difference is 100.0 (flow) or 22.0 bp (term), and the
    // same holds through `sim::compare` with warm-up 12.
    let t0 = 3u32;
    for shock in [
        Shock::OfficialTreasury { bloc: Some("CN".into()), bn: 100.0, months: 1 },
        Shock::UsCredibility { tp_bp: 22.0, months: 3, dollar_pct: 3.3 },
    ] {
        let c = fcfg(12, 400);
        let mut a = sim::new_world(&c, 4);
        let mut b = sim::new_world(&c, 4);
        b.shocks.schedule(ScheduledShock { at_month: t0, shock: shock.clone() });
        for m in 0..8u32 {
            sim::step(&mut a);
            sim::step(&mut b);
            if m < t0 {
                assert_eq!(json(&a.history[m as usize]), json(&b.history[m as usize]), "month {m}");
                assert!(fin(&a).dh.iter().zip(fin(&b).dh.iter()).all(|(x, y)| x.to_bits() == y.to_bits()));
                assert!(fin(&a).ring == fin(&b).ring && fin(&a).extra.to_bits() == fin(&b).extra.to_bits());
            }
            if m == t0 {
                match &shock {
                    Shock::OfficialTreasury { .. } => assert!((fin(&b).flow_rw - fin(&a).flow_rw - 100.0).abs() < 1e-6, "flow difference"),
                    _ => assert!((b.world.as_ref().unwrap().diag.tp_cred_bp - 22.0).abs() < 1e-9, "term difference"),
                }
            }
        }
        let cmp_cfg = Config { warmup_months: 12, ..c.clone() };
        let sc = sim::Scenario { config: cmp_cfg, shocks: vec![ScheduledShock { at_month: t0, shock: shock.clone() }], months: 8, seeds: vec![4] };
        let cmp = sim::compare(&sc);
        for m in 0..t0 as usize {
            assert_eq!(json(&cmp.baseline[m]), json(&cmp.shocked[m]), "compare: month {m} identical before the shock");
        }
    }
}

#[test]
fn finance_on_zero_drivers_equals_off() {
    // I5 on a tiny scale: the finance link with every driver at zero changes no non-world key of the first 24 months; the term and the
    // extra are zero.
    let on = quiet(fcfg(12, 300));
    let off = Config { world_link_finance: false, ..on.clone() };
    let (mut a, mut b) = (sim::new_world(&on, 5), sim::new_world(&off, 5));
    for _ in 0..24 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    for (ma, mb) in a.history.iter().zip(b.history.iter()).skip(12) {
        let (mut ja, mut jb) = (json(ma), json(mb));
        ja.as_object_mut().unwrap().remove("world");
        jb.as_object_mut().unwrap().remove("world");
        assert_eq!(ja, jb);
    }
    assert!(finance::tp_finance(&a, 3) == 0.0 && fin(&a).extra == 0.0);
}

#[test]
fn cn_tranche_exhaustion_cap() {
    // The default trend (-80 a year) with the default drivers, three seeds of tiny worlds, 140 months: the CN tranche never goes negative,
    // its deviation stops moving after exhaustion, and exhaustion falls between months 60 and 140.
    for seed in [11u64, 12, 13] {
        let mut w = sim::new_world(&fcfg(12, 300), seed);
        let mut exhausted_at: Option<usize> = None;
        for m in 0..140usize {
            sim::step(&mut w);
            let h = h_rw(&w, 1);
            assert!(h >= -1e-9, "seed {seed} month {m}: H_CN {h}");
            if exhausted_at.is_none() && h < 1e-6 {
                exhausted_at = Some(m);
            } else if let Some(e) = exhausted_at {
                // the trade's EPS guard (monetary::EPS = 1e-9 model units) leaves a residual of at most EPS / k_FO (about 2e-9 rw)
                assert!(h.abs() <= 1e-8, "seed {seed}: H_CN stays at 0 after month {e} ({h} at {m})");
            }
        }
        let e = exhausted_at.expect("the CN tranche is exhausted within 140 months");
        assert!((60..=140).contains(&e), "seed {seed}: exhaustion at month {e}");
        assert!(ledger::check_consistency(&w).is_ok(), "ledger after 140 months");
    }
}

#[test]
fn rule_reads_the_dollar_link_index_when_on() {
    // Both links on (the dollar link needs the trade link): the ln D the rule uses is ln(w.row.fx / fx_0) of the link's own index.
    let mut c = fcfg(12, 400);
    c.world_link_dollar = true;
    let mut w = sim::new_world(&c, 3);
    let fx0_dollar = w.world.as_ref().unwrap().dollar.as_ref().unwrap().base.as_ref().unwrap().fx0;
    assert!(rel(fx0_dollar, fx0_of(&w)) < 1e-12, "the two bases coincide");
    for _ in 0..12 {
        sim::step(&mut w);
        let want = (w.row.fx / fx0_dollar).ln();
        assert!((fin(&w).ln_d_prev - want).abs() < 1e-12, "ln D of the link's index");
    }
}

#[test]
fn finance_hooks_inert_when_off() {
    // The hooks return None with the block off and with the link off; a link-off world keeps the new world keys at zero and has no state.
    let no_block = Config { world_block_enabled: false, world_link_finance: true, n_households: 300, burn_in_months: 0, ..Config::default() };
    let w = sim::new_world(&no_block, 3);
    assert!(world_tp_fo(&w, 3).is_none() && world_row_extra(&w).is_none(), "block off");
    let mut lo = sim::new_world(&Config { world_link_finance: false, ..fcfg(12, 300) }, 3);
    assert!(world_tp_fo(&lo, 3).is_none() && world_row_extra(&lo).is_none(), "link off");
    assert!(lo.world.as_ref().unwrap().fin.is_none(), "no finance state with the link off");
    for _ in 0..24 {
        sim::step(&mut lo);
    }
    let wm = lo.history.last().unwrap().world.as_ref().unwrap();
    assert!(wm.fo_bn == 0.0 && wm.fo_flow_bn == 0.0 && wm.tp_fo_bp == 0.0 && wm.tp_cred_bp == 0.0 && wm.fo_extra == 0.0);
    let on = sim::new_world(&fcfg(12, 300), 3);
    assert!(world_tp_fo(&on, 3).is_some() && world_row_extra(&on).is_some(), "link on and acting");
}

#[test]
fn finance_metrics_keys() {
    // The ten new world keys are present and finite; zero with the link off.
    const KEYS: [&str; 10] = [
        "fo_bn", "fo_flow_bn", "tp_fo_bp", "tp_cred_bp", "fo_extra", "f_ring_mean", "row_mv_gdp_pct", "row_dep_gdp_pct",
        "fo_share_row_pct", "row_face_share_pct",
    ];
    let mut on = sim::new_world(&fcfg(12, 300), 3);
    for _ in 0..3 {
        sim::step(&mut on);
    }
    let v = serde_json::to_value(on.history.last().unwrap().world.as_ref().unwrap()).unwrap();
    for k in KEYS {
        let x = v[k].as_f64().unwrap_or_else(|| panic!("key {k} missing or not a number"));
        assert!(x.is_finite(), "key {k} finite");
    }
    assert!(v["fo_bn"].as_f64().unwrap() > 3000.0, "the tranche is reported");
    let mut lo = sim::new_world(&Config { world_link_finance: false, ..fcfg(12, 300) }, 3);
    for _ in 0..3 {
        sim::step(&mut lo);
    }
    let v = serde_json::to_value(lo.history.last().unwrap().world.as_ref().unwrap()).unwrap();
    for k in KEYS {
        assert_eq!(v[k].as_f64(), Some(0.0), "key {k} zero with the link off");
    }
}
