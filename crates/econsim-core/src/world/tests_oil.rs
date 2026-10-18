//! Stage-3 tests of the world oil market (spec 3, Stage 3, "Tests"). The golden vectors are the Python reference outputs of
//! `scripts/world/oil_reference.py --golden` (the research prototype and its K6/K7 scenarios). The worlds that test the link
//! use a real burn-in (`sim::new_world`), as the stage-2 continuity rule requires.

use super::oil::{self, OilInputs, OilMarket, OilParams, OilShocks, OilState, OilTables};
use crate::config::Config;
use crate::rng::{self, stream};
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use serde_json::Value;

const GOLD_OIL: &str = include_str!("data/golden/oil_golden.json");

fn golden() -> Value {
    serde_json::from_str(GOLD_OIL).expect("oil golden parses")
}

fn vec_of(v: &Value) -> Vec<f64> {
    v.as_array().expect("array").iter().map(|x| x.as_f64().expect("number")).collect()
}

fn tables() -> OilTables {
    super::params::WorldParams::load().oil.clone()
}

/// The isolated aggregate market of the prototype: one region, no regional terms, zero shock scales unless given.
fn aggregate(cfg: &Config) -> OilParams {
    OilParams::new(&tables(), cfg, true)
}

/// The prototype's unit impulse paths: the innovation at month 0 and nothing after, 61 months.
fn impulse(p: &OilParams, which: &str, size: f64) -> Vec<super::oil::OilOut> {
    let mut s = OilState::new(p);
    let mut y_path = 0.0_f64;
    let mut out = Vec::new();
    for t in 0..61 {
        let mut sh = OilShocks::default();
        let mut y = 0.0;
        match which {
            "I" if t == 0 => sh.e_i = size,
            "D" if t == 0 => sh.e_d = size,
            "O" if t == 0 => sh.e_o = size,
            "news" if t == 0 => sh.nu = size,
            "y" => {
                // The prototype's activity AR(1): y_t = rho_y y_{t-1} + innovation at t = 0 (the gap enters the ylag update).
                y_path = if t == 0 { size } else { p.rho_y * y_path };
                y = y_path;
            }
            _ => {}
        }
        let inp = OilInputs { y: vec![y; p.n], rfx: vec![0.0; p.n], ..Default::default() };
        out.push(s.step(p, &inp, &sh));
    }
    out
}

fn close(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{what}: {a} vs {b}");
}

#[test]
fn oil_matches_python_prototype() {
    // Unit IRFs of the four shocks and of news, 61 months, against the prototype (`oil_reference.py`); tolerance 1e-9.
    let cfg = Config { stochastic_oil: false, ..Config::default() };
    let p = aggregate(&cfg);
    let g = golden();
    for (name, size, key) in [("I", 1.0, "I"), ("D", 1.25, "D"), ("O", -1.0, "O"), ("y", 1.0, "y")] {
        let out = impulse(&p, name, size);
        let want = &g["irf"][key];
        for (field, got) in [
            ("pbar", out.iter().map(|o| o.pbar).collect::<Vec<_>>()),
            ("supply", out.iter().map(|o| o.supply).collect()),
            ("demand", out.iter().map(|o| o.demand).collect()),
            ("i", out.iter().map(|o| o.i).collect()),
            ("o", out.iter().map(|o| o.o).collect()),
        ] {
            let w = vec_of(&want[field]);
            for t in 0..61 {
                close(got[t], w[t], 1e-9, &format!("IRF {name} {field} m{t}"));
            }
        }
    }
    // News: the expectation shock of the golden is normalised to +10 % on impact (K3).
    let size = g["irf"]["news"]["size"].as_f64().unwrap();
    let out = impulse(&p, "news", size);
    let w = vec_of(&g["irf"]["news"]["pbar"]);
    for t in 0..61 {
        close(out[t].pbar, w[t], 1e-9, &format!("news pbar m{t}"));
    }
}

#[test]
fn oil_scenarios_match_research_k7() {
    // Research K7 scenarios: Oil +50 % exact path, OPEC+ cut of 2 mb/d, the 2008-type collapse, Hormuz, SPR release.
    let cfg = Config { stochastic_oil: false, ..Config::default() };
    let p = aggregate(&cfg);
    let g = golden();
    let sc = &g["scenarios"];
    // Oil +50 % exact path: target = 100 ln(1.5) 0.5^(t/12) on the monthly average.
    let mut s = OilState::new(&p);
    let tgt: Vec<f64> = (0..48).map(|t| 100.0 * 1.5f64.ln() * 0.5f64.powf(t as f64 / 12.0)).collect();
    let mut exact = Vec::new();
    for t in 0..48 {
        let inp = OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], target: Some(tgt[t]), ..Default::default() };
        exact.push(s.step(&p, &inp, &OilShocks::default()).pbar);
    }
    for h in [0usize, 3, 6, 12, 24, 36] {
        close(exact[h], sc["oil50_exact_path"][h.to_string()]["price"].as_f64().unwrap(), 1e-9, "oil50 exact path");
    }
    // OPEC+ cut of 2 mb/d at month 0 and the price at 6 months (+5.8 %, band +/-1).
    let cut = -2.0 / 103.1 * 100.0;
    let mut s = OilState::new(&p);
    let mut pb6 = 0.0;
    let mut o6 = 0.0;
    for t in 0..60 {
        let sh = OilShocks { e_o: if t == 0 { cut } else { 0.0 }, ..Default::default() };
        let out = s.step(&p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &sh);
        if t == 6 {
            pb6 = out.pbar;
            o6 = out.o;
        }
    }
    assert!((4.8..=6.8).contains(&pb6), "OPEC+ cut price at 6 months {pb6}");
    close(pb6, sc["opec_cut_2mbd"]["6"]["price"].as_f64().unwrap(), 1e-9, "opec cut price m6");
    close(o6, sc["opec_cut_2mbd"]["6"]["o"].as_f64().unwrap(), 1e-9, "opec cut o m6");
    // 2008-type collapse: activity -0.7 for six months and a precautionary collapse for two.
    let mut s = OilState::new(&p);
    let mut path = Vec::new();
    let mut y = 0.0;
    for t in 0..48 {
        let (yin, ei) = if t < 6 { (-0.7, -2.2 * cfg_sig_i(&p)) } else { (0.0, 0.0) };
        let ei = if t < 2 { ei } else { 0.0 };
        y = p.rho_y * y + yin;
        let sh = OilShocks { e_i: ei, ..Default::default() };
        path.push(s.step(&p, &OilInputs { y: vec![y; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &sh).pbar);
    }
    let want = vec_of(&sc["collapse_2008_type"]["pbar"]);
    let min = path.iter().cloned().fold(f64::INFINITY, f64::min);
    assert!((-50.0..=-40.0).contains(&min), "collapse trough {min}");
    for t in 0..48 {
        close(path[t], want[t], 1e-9, &format!("collapse m{t}"));
    }
    // Hormuz: 12 % of Q lost for three months, restored over six. Peak +40 (band +/-5), back to baseline by month 13 (+/-1).
    let mut s = OilState::new(&p);
    let mut hz = Vec::new();
    for t in 0..36 {
        let ext = super::oil::supply_path(12.0, 1, 3, 6, t as u32);
        let inp = OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ext_supply: ext, ..Default::default() };
        hz.push(s.step(&p, &inp, &OilShocks::default()).pbar);
    }
    let peak = hz.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!((35.0..=45.0).contains(&peak), "Hormuz peak {peak}");
    assert!(hz[13].abs() < 1.0, "Hormuz back to baseline by month 13: {}", hz[13]);
    let want = vec_of(&sc["hormuz"]["pbar"]);
    for t in 0..36 {
        close(hz[t], want[t], 1e-9, &format!("hormuz m{t}"));
    }
}

fn cfg_sig_i(p: &OilParams) -> f64 {
    p.sig_i
}

#[test]
fn k6_replay_matches_golden_and_rmse_within_band() {
    // 2026 Gulf replay: the STEO production path with no precautionary shock (K6, W3-07 gate: RMSE <= 12).
    let cfg = Config { stochastic_oil: false, ..Config::default() };
    let p = aggregate(&cfg);
    let g = golden();
    let rows = g["k6"]["rows"].as_array().unwrap();
    let mut s = OilState::new(&p);
    let mut sq = 0.0;
    for (i, r) in rows.iter().enumerate() {
        let sup = r["supply_override"].as_f64().unwrap();
        let inp = OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], supply_override: Some(sup), ..Default::default() };
        let out = s.step(&p, &inp, &OilShocks::default());
        close(out.pbar, r["pbar"].as_f64().unwrap(), 1e-9, "k6 pbar");
        if i < 6 {
            let e = out.pbar - 100.0 * (r["brent_obs"].as_f64().unwrap() / g["k6"]["baseline_brent"].as_f64().unwrap()).ln();
            sq += e * e;
        }
    }
    let rmse = (sq / 6.0).sqrt();
    assert!(rmse <= 12.0, "K6 RMSE {rmse}");
    close(rmse, g["k6"]["rmse_logpts_mar_aug"].as_f64().unwrap(), 1e-6, "k6 rmse");
}

#[test]
fn oil_bloc_elasticities_aggregate() {
    // D12: the regional eta0_i and eta1_i aggregate to the aggregate elasticities exactly (0.100 and 0.2001).
    let cfg = Config::default();
    let p = OilParams::new(&tables(), &cfg, false);
    let e0: f64 = p.omega.iter().zip(&p.eta0_i).map(|(w, e)| w * e).sum();
    let e1: f64 = p.omega.iter().zip(&p.eta1_i).map(|(w, e)| w * e).sum();
    close(e0, 0.100, 1e-12, "eta0 aggregate");
    close(e1, 0.2001, 1e-12, "eta1 aggregate");
}

#[test]
fn oil_bloc_beta_uses_cyclical_income_elasticity() {
    // The consumption-weighted mean of the cyclical income elasticities is 0.85 (demand_aggregate.beta_y), and the crude price
    // elasticities of the blocs are not used (D12).
    let p = OilParams::new(&tables(), &Config::default(), false);
    let b: f64 = p.omega.iter().zip(&p.beta).map(|(w, b)| w * b).sum();
    close(b, 0.85, 1e-9, "consumption-weighted beta");
}

#[test]
fn consumption_shares_sum_to_one() {
    // Thirteen regions (the US and the twelve blocs), consumption shares sum to one.
    let p = OilParams::new(&tables(), &Config::default(), false);
    close(p.omega.iter().sum::<f64>(), 1.0, 1e-9, "omega sum");
    assert_eq!(p.n, 13);
}

#[test]
fn market_clearing_identity_k1() {
    // K1: supply - demand equals the inventory change of the month (identity to 1e-9), over a stochastic path.
    let cfg = Config::default();
    let p = OilParams::new(&tables(), &cfg, false);
    let mut s = OilState::new(&p);
    let mut r = rng::agent_rng(7, 0, stream::WORLD_OIL_I, 0);
    let mut worst: f64 = 0.0;
    let mut gar = oil::DrawState::default();
    for t in 0..120 {
        let e_i = oil::draw_innovation(&p, &mut gar, &mut r);
        let inp = OilInputs {
            y: (0..p.n).map(|i| 0.3 * ((t as f64 * 0.1 + i as f64).sin())).collect(),
            rfx: (0..p.n).map(|i| 0.2 * ((i + t) as f64 * 0.05).cos()).collect(),
            ..Default::default()
        };
        let out = s.step(&p, &inp, &OilShocks { e_i, ..Default::default() });
        worst = worst.max((out.supply - out.demand - out.di).abs());
    }
    assert!(worst < 1e-9, "clearing identity residual {worst}");
}

#[test]
fn zero_shock_stays_zero() {
    let p = aggregate(&Config { stochastic_oil: false, ..Config::default() });
    let mut s = OilState::new(&p);
    for _ in 0..200 {
        let out = s.step(&p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &OilShocks::default());
        assert!(out.pbar.abs() < 1e-12 && out.di.abs() < 1e-12);
    }
}

#[test]
fn exact_path_hits_target() {
    // The precautionary innovation is solved so that the monthly-average price equals the target in every month.
    let p = aggregate(&Config { stochastic_oil: false, ..Config::default() });
    let mut s = OilState::new(&p);
    for t in 0..60 {
        let target = 10.0 * (t as f64 * 0.2).sin();
        let inp = OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], target: Some(target), ..Default::default() };
        let out = s.step(&p, &inp, &OilShocks::default());
        close(out.pbar, target, 1e-10, "exact path target");
    }
}

#[test]
fn opec_rule_clip_decay_and_spare_cap() {
    // A deep cut is bounded by the deepest cumulative cut, and output never exceeds spare capacity.
    let p = aggregate(&Config { stochastic_oil: false, ..Config::default() });
    let mut s = OilState::new(&p);
    for _ in 0..80 {
        let sh = OilShocks { e_o: -5.0, ..Default::default() };
        let out = s.step(&p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &sh);
        assert!(out.tau >= p.tau_min - 1e-12 && out.tau <= p.spare_cap + 1e-12);
    }
    assert!((s.tau - p.tau_min).abs() < 1e-9, "cut clips at the deepest cut: {}", s.tau);
    let mut s = OilState::new(&p);
    for _ in 0..80 {
        let sh = OilShocks { e_o: 5.0, ..Default::default() };
        let out = s.step(&p, &OilInputs { y: vec![0.0; p.n], rfx: vec![0.0; p.n], ..Default::default() }, &sh);
        assert!(out.o <= p.spare_cap + 1e-12);
    }
    assert!(s.o <= p.spare_cap + 1e-12);
}

#[test]
fn us_shale_two_stage_profile() {
    // Cumulative US tight-oil output per 1 % of price, the two-stage lag (eps_LR 0.45; spec 3 Stage 3 test).
    let p = aggregate(&Config { stochastic_oil: false, ..Config::default() });
    // Research K8 indexing: the response at row h is after h + 1 monthly updates (the first update is the impact month).
    let mut u1 = 0.0;
    let mut u2 = 0.0;
    let mut got = Vec::new();
    for t in 0..=60 {
        u1 = p.rho_u1 * u1 + (1.0 - p.rho_u1) * p.eps_us_lr * 1.0;
        u2 = p.rho_u2 * u2 + (1.0 - p.rho_u2) * u1;
        if [6, 12, 18, 24, 36, 60].contains(&t) {
            got.push(u2);
        }
    }
    for (g, w) in got.iter().zip([0.06, 0.14, 0.22, 0.28, 0.37, 0.43]) {
        assert!((g - w).abs() <= 0.005 + 1e-9, "shale cumulative {g} vs {w}");
    }
}

#[test]
fn demand_elasticity_aggregates() {
    // Within the month 0.100, 12 months 0.152, long run 0.300 (W3-12): the demand response to a permanent 1 % price rise.
    let p = aggregate(&Config { stochastic_oil: false, ..Config::default() });
    let mut z = 0.0;
    let mut at12 = 0.0;
    for t in 1..=240 {
        z = p.rho_z * z + (1.0 - p.rho_z) * p.eta1 * 1.0;
        if t == 12 {
            at12 = p.eta0 + z;
        }
    }
    close(p.eta0, 0.100, 1e-12, "within month");
    assert!((at12 - 0.152).abs() < 0.005, "12-month demand {at12}");
    assert!((p.eta0 + p.eta1 - 0.300).abs() < 0.005, "long run demand");
}

#[test]
fn uae_flag_oil_runtime_switch() {
    // D7: with the UAE out, OPEC+ liquids fall by the UAE's production and the spare capacity by the spare share; both
    // composition sets conserve the group's total with the UAE's liquids.
    let t = tables();
    let inn = OilParams::new(&t, &Config { world_uae_in_opec: true, ..Config::default() }, false);
    let out = OilParams::new(&t, &Config { world_uae_in_opec: false, ..Config::default() }, false);
    let uae = t.opec.uae_liquids_mbd / t.q_mbd;
    close(inn.opec_q - out.opec_q, uae, 1e-9, "UAE liquids conserved");
    close(out.spare_cap, inn.spare_cap * 0.7, 1e-12, "UAE spare share removed");
}

#[test]
fn oil_shocks_keyed_and_thread_independent() {
    // Draws are keyed by (seed, tick, stream): the same key gives the same innovation, a different tick gives another.
    let p = OilParams::new(&tables(), &Config::default(), false);
    let mut g1 = oil::DrawState::default();
    let mut g2 = oil::DrawState::default();
    let mut ra = rng::agent_rng(3, 5, stream::WORLD_OIL_I, 0);
    let mut rb = rng::agent_rng(3, 5, stream::WORLD_OIL_I, 0);
    assert_eq!(oil::draw_innovation(&p, &mut g1, &mut ra), oil::draw_innovation(&p, &mut g2, &mut rb));
    let mut rc = rng::agent_rng(3, 6, stream::WORLD_OIL_I, 0);
    let mut g3 = oil::DrawState::default();
    assert_ne!(oil::draw_innovation(&p, &mut g3, &mut rc), 0.0);
}

#[test]
fn mean_offset_only_when_stochastic() {
    let t = tables();
    let on = OilParams::new(&t, &Config { stochastic_oil: true, ..Config::default() }, false);
    let off = OilParams::new(&t, &Config { stochastic_oil: false, ..Config::default() }, false);
    assert!((on.offset - 8.355).abs() < 1e-9 && on.shock_on == 1.0);
    assert_eq!(off.offset, 0.0);
    assert_eq!(off.shock_on, 0.0);
}

#[test]
fn qty_channel_opec_gdp() {
    // The OPEC+ quantity channel divides by OPEC+ liquids over Q (0.4626 with the UAE in; spec 2.2.2).
    let p = OilParams::new(&tables(), &Config::default(), false);
    close(p.opec_q, 0.4626, 1e-4, "OPEC+ liquids share of Q");
}

#[test]
fn oil_levers_wired() {
    // world_oil_rho_q and world_oil_sig_i_mult change only the named parameter.
    let t = tables();
    let base = OilParams::new(&t, &Config::default(), false);
    let lev = OilParams::new(&t, &Config { world_oil_rho_q: 0.995, world_oil_sig_i_mult: 1.1, ..Config::default() }, false);
    close(lev.rho_q, 0.995, 1e-12, "rho_q lever");
    close(lev.sig_i, base.sig_i * 1.1, 1e-12, "sig_I lever");
    assert_eq!(lev.psi, base.psi);
    assert_eq!(lev.eta0, base.eta0);
}

#[test]
fn supply_path_ramp_hold_recover() {
    // Ramp 2, hold 2, recover 4 for a 10 % loss: full level from month 1, held through month 2, then linear restoration.
    let v: Vec<f64> = (0..9).map(|k| oil::supply_path(10.0, 2, 2, 4, k)).collect();
    let want = [5.0, 10.0, 10.0, 7.5, 5.0, 2.5, 0.0, 0.0, 0.0];
    for (a, b) in v.iter().zip(want) {
        assert!((a - b).abs() < 1e-12, "{v:?}");
    }
}

#[test]
fn student_draw_has_unit_variance() {
    // The oil-specific Student-t base is unit variance (the winsorisation at +/-8 is inactive at this size).
    let p = OilParams::new(&tables(), &Config::default(), false);
    let mut r = rng::agent_rng(11, 0, stream::WORLD_OIL_S, 0);
    let mut sq = 0.0;
    let n = 20000;
    for _ in 0..n {
        let (_, eo) = oil::draw_supply(&OilParams { sig_o: 1.0, ..p.clone() }, &mut r);
        sq += eo * eo;
    }
    let var = sq / n as f64;
    assert!((var - 1.0).abs() < 0.08, "OPEC+ surprise variance {var}");
}

// -----------------------------------------------------------------------------------------------------------------------------
// The block: the splice (S3-R1), the interface to the ABM and the legacy identity. Worlds with a real burn-in.

fn linked_cfg(households: usize, burn: u32) -> Config {
    Config {
        n_households: households,
        burn_in_months: burn,
        world_block_enabled: true,
        world_link_oil: true,
        ..Config::default()
    }
}

#[test]
fn splice_kappa_cancels_in_matched_pair() {
    // S3-R1 (a): in a matched shocked and unshocked pair (same seed, same burn-in) A_shock - A_base equals p_shock - p_base to
    // machine precision, because kappa and the splice term are the same in both arms.
    let base_cfg = linked_cfg(300, 24);
    let mut wb = sim::new_world(&base_cfg, 5);
    let mut ws = sim::new_world(&base_cfg, 5);
    ws.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
    let mut worst: f64 = 0.0;
    for _ in 0..12 {
        sim::step(&mut wb);
        sim::step(&mut ws);
        let mb = wb.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        let ms = ws.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        let dev_diff = ms.dev - mb.dev;
        let p_diff = ms.p_bar - mb.p_bar;
        worst = worst.max((dev_diff - p_diff).abs());
    }
    assert!(worst < 1e-10, "pair difference residual {worst}");
}

#[test]
fn splice_rho_reads_oil_ar_and_sensitivities() {
    // S3-R1 (b, c): rho is the legacy oil_ar by default; rho = 1 keeps kappa for ever (the permanent anchor), rho = 0 drops it.
    // Stage-3 review R12: oil_ar is 0.9 here, so a hard-coded 0.985 would fail.
    let cfg = Config { oil_ar: 0.9, ..linked_cfg(200, 12) };
    assert_eq!(OilMarket::new(&cfg, &tables()).rho_splice, 0.9);
    let one = OilMarket::new(&Config { world_oil_splice_rho: 1.0, ..cfg.clone() }, &tables());
    assert_eq!(one.rho_splice, 1.0);
    let zero = OilMarket::new(&Config { world_oil_splice_rho: 0.0, ..cfg.clone() }, &tables());
    assert_eq!(zero.rho_splice, 0.0);
    let mut m = OilMarket::new(&cfg, &tables());
    m.splice = Some(oil::Splice { kappa: 40.0, rho: 1.0, kappa_q: 0.0 });
    m.t_rep = 17;
    assert!((oil::splice_term(&m) - 40.0).abs() < 1e-12);
    m.splice = Some(oil::Splice { kappa: 40.0, rho: 0.0, kappa_q: 0.0 });
    assert_eq!(oil::splice_term(&m), 0.0);
}

#[test]
fn splice_removes_month_zero_jump() {
    // Continuity (the S2-R1 rule as applied in S3-R1): with L_b the legacy level of the base month and p_b the market's price there,
    // kappa = L_b - (p_b - off), so the first reported deviation is A_0 = p_0 - off + kappa rho and
    // A_0 - L_b = (p_0 - p_b) - kappa (1 - rho): no jump beyond the market's own one-month move and the decay of kappa.
    let cfg = linked_cfg(300, 24);
    let mut w = sim::new_world(&cfg, 9);
    let l_b = 100.0 * (w.exog.oil_ar + w.exog.oil_shock);
    let kappa = w.world.as_ref().unwrap().oil.as_ref().unwrap().splice.unwrap().kappa;
    let rho = cfg.oil_ar;
    sim::step(&mut w);
    let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
    let off = m.par.offset;
    let p_b = l_b - kappa + off;
    let p_0 = m.diag.p_bar;
    close(m.diag.dev - l_b, (p_0 - p_b) - kappa * (1.0 - rho), 1e-9, "month-0 continuity identity");
    assert!((m.diag.dev - l_b).abs() <= (p_0 - p_b).abs() + kappa.abs() * (1.0 - rho) + 1e-9);
}

#[test]
fn splice_kappa_cancels_without_burn_in_month0_shock() {
    // Stage-3 review R4: no burn-in, OilPrice at the first reported month. The splice is captured after `exog::step` has already
    // added the month's legacy shock, so kappa must be taken without it: the pair difference of the deviation then equals the
    // market's pair difference at every month (before the fix it exceeded it by 100 oil_shock at month 0).
    let cfg = linked_cfg(300, 0);
    let mut wb = sim::new_world(&cfg, 5);
    let mut ws = sim::new_world(&cfg, 5);
    ws.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
    let mut worst: f64 = 0.0;
    for _ in 0..6 {
        sim::step(&mut wb);
        sim::step(&mut ws);
        let mb = wb.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        let ms = ws.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        worst = worst.max(((ms.dev - mb.dev) - (ms.p_bar - mb.p_bar)).abs());
    }
    assert!(worst < 1e-9, "pair difference of the deviation exceeds the market's by {worst}");
}

#[test]
fn exact_path_pair_difference_is_the_shock_without_feedback() {
    // Stage-3 review (S3-R2): with world_oil_feedback false the market is the same in both arms, so the exact path pins the
    // shocked deviation to the unshocked one plus the legacy log shock exactly: (A_s - A_b) - 100 oil_log_shock = 0 every month.
    let cfg = Config { world_oil_feedback: false, ..linked_cfg(300, 24) };
    let mut wb = sim::new_world(&cfg, 5);
    let mut ws = sim::new_world(&cfg, 5);
    ws.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
    let mut worst: f64 = 0.0;
    let mut live = 0;
    for _ in 0..12 {
        sim::step(&mut wb);
        sim::step(&mut ws);
        let mb = wb.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        let ms = ws.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        if ms.live_u {
            live += 1;
        }
        worst = worst.max(((ms.dev - mb.dev) - 100.0 * ws.shocks.oil_log_shock).abs());
    }
    assert!(live >= 8, "copy U live for {live} months");
    assert!(worst < 1e-9, "pair difference minus the shock {worst}");
}

#[test]
fn feedback_lowers_the_shocked_price() {
    // Stage-3 review (S3-R2, REVIEW 3.1): with the feedback on, the shocked arm's output gaps lower copy A's price, so the pair
    // difference minus the legacy shock is negative in the months after the shock (about -0.3 to -4 log points at months 3-12
    // for OilPrice +50 with 10,000 households; the sign is what this test checks).
    let cfg = linked_cfg(300, 24);
    let mut wb = sim::new_world(&cfg, 5);
    let mut ws = sim::new_world(&cfg, 5);
    ws.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
    let mut gaps = Vec::new();
    for _ in 0..12 {
        sim::step(&mut wb);
        sim::step(&mut ws);
        let mb = wb.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        let ms = ws.world.as_ref().unwrap().oil.as_ref().unwrap().diag;
        gaps.push((ms.dev - mb.dev) - 100.0 * ws.shocks.oil_log_shock);
    }
    for (k, g) in gaps.iter().enumerate().skip(4) {
        assert!(*g < 0.0, "month {k}: feedback gap {g} is not negative");
    }
}

#[test]
fn oil_interface_price_formula() {
    // exog.oil_price = oil_base x index x exp(dev/100) in reported months, and the blocs see the same deviation. The index is
    // read before the month (it advances with the tick at the end of the step).
    let cfg = linked_cfg(200, 12);
    let mut w = sim::new_world(&cfg, 4);
    let index = crate::common::oil_index(&w);
    sim::step(&mut w);
    let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
    let want = w.exog.oil_base * index * (m.diag.dev / 100.0).exp();
    close(w.exog.oil_price, want, 1e-12, "oil price interface");
    close(m.bloc_oil.0, m.diag.dev, 1e-12, "bloc oil gap equals the ABM deviation");
    close(w.exog.oil_ar, m.diag.dev / 100.0, 1e-12, "oil_ar is the copy-A deviation (no shock on the path)");
}

#[test]
fn oil_active_off_leaves_exog_untouched() {
    // Link off with the block on: the ABM oil price is the legacy expression, bit for bit, the same as with the block off.
    let on_link_off = Config { world_block_enabled: true, world_link_oil: false, ..linked_cfg(200, 12) };
    let block_off = Config { world_block_enabled: false, world_link_oil: false, ..linked_cfg(200, 12) };
    let mut a = sim::new_world(&on_link_off, 4);
    let mut b = sim::new_world(&block_off, 4);
    for _ in 0..6 {
        sim::step(&mut a);
        sim::step(&mut b);
        assert_eq!(a.exog.oil_price.to_bits(), b.exog.oil_price.to_bits());
        assert_eq!(a.exog.oil_ar.to_bits(), b.exog.oil_ar.to_bits());
    }
    assert!(a.world.as_ref().unwrap().oil.is_none());
}

#[test]
fn legacy_oil_draws_still_taken() {
    // The legacy oil draws (the EXOG base draw and the Student-t, jump and GARCH streams) are taken with the link on, so every
    // later EXOG draw (the risk-premium AR state) is unchanged.
    let on = linked_cfg(200, 12);
    let off = Config { world_link_oil: false, ..on.clone() };
    let mut a = sim::new_world(&on, 6);
    let mut b = sim::new_world(&off, 6);
    for _ in 0..10 {
        sim::step(&mut a);
        sim::step(&mut b);
        assert_eq!(a.exog.rp_ar.to_bits(), b.exog.rp_ar.to_bits());
        assert_eq!(a.exog.oil_cond_var.to_bits(), b.exog.oil_cond_var.to_bits());
    }
}

#[test]
fn stochastic_off_has_no_structural_shocks() {
    // stochastic_oil false: the structural draws are multiplied by zero (taken all the same) and the mean offset is zero.
    let mut cfg = linked_cfg(200, 12);
    cfg.stochastic_oil = false;
    let mut w = sim::new_world(&cfg, 8);
    for _ in 0..6 {
        sim::step(&mut w);
    }
    let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
    assert_eq!(m.par.offset, 0.0);
    assert_eq!(m.par.shock_on, 0.0);
}

#[test]
fn oil_exact_path_drops_copy_u_after_decay() {
    // The copy U of a live OilPrice shock is kept while |shock| > 1e-4 and dropped once the shock has decayed.
    let cfg = linked_cfg(200, 12);
    let mut w = sim::new_world(&cfg, 10);
    w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::OilPrice { pct: 50.0, half_life_months: 1.0 } });
    let mut live_seen = false;
    let mut dropped = false;
    for _ in 0..30 {
        sim::step(&mut w);
        let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
        if m.diag.live_u {
            live_seen = true;
        } else if live_seen {
            dropped = true;
        }
    }
    assert!(live_seen && dropped, "copy U must be live and then dropped");
}

#[test]
fn exact_path_identity_against_own_structural_price() {
    // The exact path pins the monthly-average price of the shocked copy to its own structural copy A plus the shock, every month
    // the shock is live (W3-10 allows for the feedback of the shock on copy A, which is measured separately).
    let cfg = linked_cfg(200, 12);
    let mut w = sim::new_world(&cfg, 12);
    w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
    let mut live_months = 0;
    let mut worst: f64 = 0.0;
    for _ in 0..20 {
        sim::step(&mut w);
        let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
        if m.diag.live_u {
            live_months += 1;
            let shock_pct = 100.0 * w.shocks.oil_log_shock;
            worst = worst.max((m.diag.p_bar - m.a.pbar - shock_pct).abs());
        }
    }
    assert!(live_months >= 8, "copy U live for {live_months} months");
    assert!(worst < 1e-9, "exact-path residual against copy A {worst}");
}

#[test]
fn oil_regime_rejects_unknown_string() {
    // Stage-3 review R10: unknown oil strings are refused by `validate_world_oil` (the server answers 400 and never runs them).
    assert!(Config::default().validate_world_oil().is_ok());
    assert!(Config { world_oil_profile: "no_such_profile".to_string(), ..Config::default() }.validate_world_oil().is_err());
    assert!(Config { world_opec_regime: "floor".to_string(), ..Config::default() }.validate_world_oil().is_err());
    assert!(Config { world_oil_shock_mask: "IX".to_string(), ..Config::default() }.validate_world_oil().is_err());
    assert!(Config { world_oil_profile: "shale_boom".to_string(), ..Config::default() }.validate_world_oil().is_ok());
    assert!(Config { world_opec_regime: "market_share".to_string(), ..Config::default() }.validate_world_oil().is_ok());
    assert!(Config { world_oil_shock_mask: String::new(), ..Config::default() }.validate_world_oil().is_ok());
}

#[test]
fn shock_mask_empty_matches_no_structural_draws() {
    // W3-19 cells: with feedback off the market does not read the blocs, so an empty mask (no structural source) gives the same
    // deviation path as stochastic_oil off, month by month and exactly (both zero structural innovations, same market).
    let base = Config { world_oil_feedback: false, ..linked_cfg(300, 24) };
    let mut wa = sim::new_world(&Config { world_oil_shock_mask: String::new(), ..base.clone() }, 7);
    let mut wb = sim::new_world(&Config { stochastic_oil: false, ..base.clone() }, 7);
    let mut wc = sim::new_world(&Config { world_oil_shock_mask: "I".to_string(), ..base.clone() }, 7);
    let mut moved = 0.0_f64;
    for _ in 0..12 {
        sim::step(&mut wa);
        sim::step(&mut wb);
        sim::step(&mut wc);
        let pa = wa.world.as_ref().unwrap().oil.as_ref().unwrap().diag.p_bar;
        let pb = wb.world.as_ref().unwrap().oil.as_ref().unwrap().diag.p_bar;
        let pc = wc.world.as_ref().unwrap().oil.as_ref().unwrap().diag.p_bar;
        assert!((pa - pb).abs() < 1e-12, "empty mask differs from no structural draws: {pa} vs {pb}");
        moved = moved.max((pc - pa).abs());
    }
    assert!(moved > 1e-3, "the oil-specific source moves the market when listed ({moved})");
}

#[test]
fn oil_us_demand_is_region_zero_demand() {
    // W3-18: the US-region demand deviation d_US = beta_0 ylag_0 + q - z_0 - eta0_0 (pbar + rfx_0), from the state after the step.
    let p = OilParams::new(&tables(), &Config::default(), false);
    let mut s = OilState::new(&p);
    let inp = OilInputs { y: vec![0.3; p.n], rfx: vec![0.1; p.n], ..Default::default() };
    let sh = OilShocks { e_i: 0.5, e_d: 0.2, e_o: -0.1, ..Default::default() };
    let out = s.step(&p, &inp, &sh);
    let want = p.beta[0] * s.ylag[0] + s.q - s.z[0] - p.eta0_i[0] * (s.pbar + 0.1);
    assert!((out.us_d - want).abs() < 1e-12, "US demand {} vs {want}", out.us_d);
}

#[test]
fn splice_qty_against_independent_base_month() {
    // Stage-4 step 0 (D1, S3-R13): the base-month quantity q_b is read from the last burn-in month's diagnostics, not derived from
    // kappa_q, so the test can see kappa_q. kappa_q is checked against it, and the decay of the splice uses oil_ar 0.9 (not the
    // default), so a hard-coded rho is caught. The matched-pair cancellation of the old test held for any kappa_q and is dropped.
    let cfg = Config { oil_ar: 0.9, ..linked_cfg(300, 24) };
    for seed in 1..=6u64 {
        let mut w = sim::new_world(&cfg, seed);
        let (q_b, s, off) = {
            let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
            (m.diag.qty_mkt, m.splice.unwrap(), m.par.qty_offset)
        };
        assert!((s.kappa_q - (oil::LEGACY_QTY_BASE - (q_b - off))).abs() < 1e-12, "seed {seed}: kappa_q {} vs base-month value", s.kappa_q);
        for t in 0..4 {
            sim::step(&mut w);
            let m = w.world.as_ref().unwrap().oil.as_ref().unwrap();
            let want = m.diag.qty_mkt - off + s.kappa_q * 0.9_f64.powi(t + 1);
            assert!((m.bloc_oil.1 - want).abs() < 1e-12, "seed {seed} month {t}: {} vs {want}", m.bloc_oil.1);
        }
    }
}

#[test]
fn qty_offset_only_when_stochastic() {
    // The offset of the quantity input follows the price offset (D6): it applies only when stochastic_oil is on.
    let on = OilParams::new(&tables(), &Config { stochastic_oil: true, world_oil_qty_offset: -6.0, ..Config::default() }, false);
    assert_eq!(on.qty_offset, -6.0);
    let off = OilParams::new(&tables(), &Config { stochastic_oil: false, world_oil_qty_offset: -6.0, ..Config::default() }, false);
    assert_eq!(off.qty_offset, 0.0);
}
