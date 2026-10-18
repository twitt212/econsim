//! Stage-1 tests of the world block (spec 3, Stage 1, "Tests"). The golden vectors are the Python reference outputs
//! (`scripts/world/world_ref.py goldens --check` reproduces them); the tests replay the same inputs through the Rust code.

use super::bloc;
use super::net::{self, UsInputs};
use super::params::{Par, WorldParams};
use super::{Bloc, WorldBlock, K, NB};
use crate::config::Config;
use crate::metrics::Metrics;
use crate::rng::{self, stream};
use crate::shocks::{ScheduledShock, Shock, BLOC_NAMES};
use crate::sim;
use crate::testkit;
use crate::types::{World, N_TENORS, TENORS};
use crate::monetary;
use serde_json::Value;

const GOLD_STAGE1: &str = include_str!("data/golden/stage1.json");
const GOLD_IRF: &str = include_str!("data/golden/irf_unit.json");
const GOLD_PF: &str = include_str!("data/golden/pf_ar1_EA.json");
const GOLD_KREF: &str = include_str!("data/golden/kref.json");
const GOLD_CLEAN: &str = include_str!("data/golden/inputs_in2019_clean.jsonl");
const GOLD_STRESS: &str = include_str!("data/golden/inputs_stress.jsonl");
const RESEARCH_BLOCS: &str = include_str!("../../../../data/world/research/blocs/params.json");

/// Output indices of the per-bloc rows: y, pi, i, z (and mrr).
const OY: usize = 0;
const OPI: usize = 1;
const OI: usize = 2;
const OZ: usize = 3;

fn golden(s: &str) -> Value {
    serde_json::from_str(s).expect("golden parses")
}

fn f64_of(v: &Value) -> f64 {
    v.as_f64().expect("number")
}

fn vec_of(v: &Value) -> Vec<f64> {
    v.as_array().expect("array").iter().map(f64_of).collect()
}

fn us_from_row(r: &Value) -> UsInputs {
    let ex_v = r["ex"].as_array().expect("ex");
    let mut ex = [0.0; K];
    for k in 0..K {
        ex[k] = f64_of(&ex_v[k]);
    }
    UsInputs {
        ius: f64_of(&r["ius"]),
        ex,
        y_us: f64_of(&r["y_us"]),
        oil: r.get("oil").map_or(0.0, f64_of),
        qty: r.get("qty").map_or(0.0, f64_of),
        fex: r.get("fex").map_or(0.0, f64_of),
        dollar: r.get("dollar").map_or(0.0, f64_of),
        dollar_decay: r.get("dollar_decay").map_or(1.0, f64_of),
        ..UsInputs::zero()
    }
}

fn eps_from_row(r: &Value) -> Option<[[f64; 4]; NB]> {
    if r["eps"].is_null() {
        return None;
    }
    let rows = r["eps"].as_array().expect("eps");
    let mut e = [[0.0; 4]; NB];
    for i in 0..NB {
        for j in 0..4 {
            e[i][j] = f64_of(&rows[i][j]);
        }
    }
    Some(e)
}

/// The block configured as a replay file's header says (knee, saturation, persistences).
fn replay_config(h: &Value) -> Config {
    Config {
        world_block_enabled: true,
        fx_rate_knee: f64_of(&h["fx_rate_knee"]),
        fx_rate_sat_scale: f64_of(&h["fx_rate_sat_scale"]),
        world_rho_f: f64_of(&h["rho_f"]),
        world_rho_p: f64_of(&h["rho_p"]),
        world_rho_oil: f64_of(&h["rho_oil"]),
        world_rho_qty: f64_of(&h["rho_qty"]),
        world_ius_offset: 0.0,
        world_yus_offset: 0.0,
        ..Default::default()
    }
}

/// Replays a golden JSON-lines file through the Rust bloc code (network on). Returns the outputs per month and the largest
/// difference from the stored `out` fields (y, pi, i, z).
fn replay(text: &str) -> (Vec<[[f64; 5]; NB]>, f64) {
    let mut lines = text.lines().filter(|l| !l.trim().is_empty()).map(|l| serde_json::from_str::<Value>(l).expect("json line"));
    let header = lines.next().expect("header");
    let h = &header["header"];
    let mut wb = WorldBlock::new(&replay_config(h));
    for sh in h["shocks"].as_array().expect("shocks") {
        let t0 = sh["t0"].as_u64().unwrap_or(0) as u32;
        let shock = match sh["type"].as_str().unwrap() {
            "BlocPolicy" => Shock::BlocPolicy {
                bloc: sh["bloc"].as_str().unwrap().to_string(),
                bps: f64_of(&sh["bps"]),
                months: sh["months"].as_u64().unwrap() as u32,
            },
            "BlocDemand" => Shock::BlocDemand {
                bloc: sh["bloc"].as_str().unwrap().to_string(),
                pct: f64_of(&sh["pct"]),
                half_life_months: f64_of(&sh["half_life_months"]),
                risk_off: false,
            },
            "GlobalDemand" => Shock::GlobalDemand { pct: f64_of(&sh["pct"]), half_life_months: f64_of(&sh["half_life_months"]) },
            _ => continue, // Dollar and the other stage-4 shocks are not bloc shocks
        };
        wb.add_user(&shock, t0);
    }
    let mut outs = Vec::new();
    let mut worst: f64 = 0.0;
    for r in lines {
        let us = us_from_row(&r);
        let eps = eps_from_row(&r);
        let x = wb.step_inputs(&us, eps.as_ref(), false);
        if let Some(o) = r.get("out") {
            for i in 0..NB {
                for (v, idx) in [("y", OY), ("pi", OPI), ("i", OI), ("z", OZ)] {
                    worst = worst.max((x[i][idx] - f64_of(&o[v][i])).abs());
                }
            }
        }
        outs.push(x);
    }
    (outs, worst)
}

/// Stage-1 group statistics of the reference (`world_ref.group_metrics`), from per-month outputs.
struct Groups {
    ae_ytr: f64,
    ae_ytr_m: usize,
    eme_ytr: f64,
    ae_y12: f64,
    ae_i3: f64,
    ae_i24: f64,
    flo_imax: f64,
    ae_zpk: f64,
    ae_z24: f64,
    flo_zpk: f64,
    flo_z24: f64,
    cn_ytr: f64,
}

fn groups(outs: &[[[f64; 5]; NB]]) -> Groups {
    let ae = [0usize, 2, 3, 4, 9];
    let eme = [1usize, 6, 7, 8, 5];
    let flo = [6usize, 7, 8, 5];
    let mean = |idx: &[usize], t: usize, o: usize| idx.iter().map(|&i| outs[t][i][o]).sum::<f64>() / idx.len() as f64;
    let series = |idx: &[usize], o: usize, n: usize| (0..n).map(|t| mean(idx, t, o)).collect::<Vec<f64>>();
    let ae_y = series(&ae, OY, 49);
    let (ae_ytr_m, ae_ytr) = (12..49).map(|t| (t, ae_y[t])).fold((12, f64::INFINITY), |a, b| if b.1 < a.1 { b } else { a });
    let eme_y = series(&eme, OY, 49);
    let eme_ytr = eme_y[12..49].iter().cloned().fold(f64::INFINITY, f64::min);
    let ae_i = series(&ae, OI, 25);
    let flo_i = series(&flo, OI, 25);
    let ae_z = series(&ae, OZ, 25);
    let flo_z = series(&flo, OZ, 25);
    let cn_y: Vec<f64> = (0..49).map(|t| outs[t][1][OY]).collect();
    Groups {
        ae_ytr,
        ae_ytr_m,
        eme_ytr,
        ae_y12: ae_y[12],
        ae_i3: ae_i[3],
        ae_i24: ae_i[24],
        flo_imax: flo_i[3..25].iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ae_zpk: ae_z.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        ae_z24: ae_z[24],
        flo_zpk: flo_z.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        flo_z24: flo_z[24],
        cn_ytr: cn_y[12..49].iter().cloned().fold(f64::INFINITY, f64::min),
    }
}

/// A block with one BlocPolicy of `bps` on each listed bloc from month 0.
fn policy_block(bps: f64, months: u32, blocs: &[&str]) -> WorldBlock {
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    for b in blocs {
        wb.add_user(&Shock::BlocPolicy { bloc: (*b).to_string(), bps, months }, 0);
    }
    wb
}

/// `months` steps with zero US inputs, isolated (F = Peff = 0) or with the network.
fn run_zero(wb: &mut WorldBlock, months: usize, isolated: bool) -> Vec<[[f64; 5]; NB]> {
    let zero = UsInputs::zero();
    (0..months).map(|_| wb.step_inputs(&zero, None, isolated)).collect()
}

fn tiny_on(seed: u64, households: usize) -> World {
    let cfg = Config { world_block_enabled: true, n_households: households, ..Default::default() };
    testkit::tiny_world(cfg, seed)
}

fn tiny_off(seed: u64, households: usize) -> World {
    let cfg = Config { world_block_enabled: false, n_households: households, ..Default::default() };
    testkit::tiny_world(cfg, seed)
}

// ---------------------------------------------------------------- tables and solution

#[test]
fn bloc_solution_shapes_and_finite() {
    let p = WorldParams::load();
    assert_eq!(p.sol.len(), NB);
    for s in &p.sol {
        assert!(s.c.iter().flatten().all(|x| x.is_finite()));
        for e in 0..4 {
            assert_eq!(s.ir[e].len(), K);
            assert!(s.ir[e].iter().flatten().all(|x| x.is_finite()));
            assert!(s.tail[e].iter().all(|x| x.is_finite()));
        }
        assert!(s.pin.iter().all(|p| p.iter().all(|x| x.is_finite())));
        assert_eq!(s.pin[2].len(), 12);
    }
}

#[test]
fn coefficients_match_params_monthly() {
    let p = WorldParams::load();
    let rb: Value = serde_json::from_str(RESEARCH_BLOCS).expect("research params");
    for (i, b) in p.blocs.iter().enumerate() {
        let r = &rb["blocs"][Bloc::ALL[i].name()]["monthly_coefficients"];
        let m = &b.monthly;
        for (k, x) in [("b1", m.b1), ("b2", m.b2), ("b3", m.b3), ("b4", m.b4), ("b5", m.b5), ("l1", m.l1), ("l2", m.l2), ("l3", m.l3), ("g1", m.g1), ("g2", m.g2), ("g4", m.g4), ("phi", m.phi), ("theta", m.theta)] {
            if r[k].is_null() {
                continue;
            }
            let rv = match &r[k] {
                Value::Object(o) => f64_of(&o["value"]),
                v => f64_of(v),
            };
            assert!((x - rv).abs() <= 5e-5 + 1e-4 * rv.abs(), "{} {k}: embedded {x} research {rv}", b.name);
        }
    }
}

#[test]
fn autonomous_bloc_decays() {
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    wb.blocs[0].s[0] = 1.0;
    run_zero(&mut wb, 600, true);
    let norm: f64 = wb.blocs[0].s.iter().map(|x| x * x).sum::<f64>().sqrt();
    assert!(norm < 1e-6, "norm after 600 months {norm}");
}

#[test]
fn sequential_matches_perfect_foresight_ar1() {
    // W1-09: EA with theta 0, isolated, ius_t = 0.95^t realised and expected (ex[k] = 0.95^(t+k)) against the stacked solution.
    let g = golden(GOLD_PF);
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, world_theta_mult: 0.0, ..Default::default() });
    let rho: f64 = 0.95;
    let mut worst: f64 = 0.0;
    for t in 0..60 {
        let mut ex = [0.0; K];
        for k in 0..K {
            ex[k] = rho.powi((t + k) as i32);
        }
        let us = UsInputs { ius: rho.powi(t as i32), ex, ..UsInputs::zero() };
        let x = wb.step_inputs(&us, None, true);
        for (v, idx) in [("y", OY), ("pi", OPI), ("i", OI), ("z", OZ)] {
            worst = worst.max((x[0][idx] - f64_of(&g["pf"][v][t])).abs());
        }
    }
    eprintln!("W1-09 single-bloc max difference {worst:.3e}");
    assert!(worst <= 1e-4, "max difference against the stacked solution {worst}");
}

#[test]
fn forcing_horizon_truncation() {
    // W1-32 (D29): period-0 responses to forcing of persistence rho, K = 240 plus the tail, against K = 480 and K = 960.
    let p = WorldParams::load();
    let kr = golden(GOLD_KREF);
    let mut worst: f64 = 0.0;
    for (i, b) in Bloc::ALL.iter().enumerate() {
        for (e, key) in ["IS", "PC", "rule", "S"].iter().enumerate() {
            let sol = &p.sol[i];
            for (rk, rho) in [("0.99", 0.99_f64), ("0.997", 0.997_f64), ("1.0", 1.0_f64)] {
                let mut x240 = [0.0; 5];
                for k in 0..K {
                    let f = rho.powi(k as i32);
                    for o in 0..5 {
                        x240[o] += sol.ir[e][k][o] * f;
                    }
                }
                let fk = rho.powi((K - 1) as i32);
                let xt: Vec<f64> = (0..4).map(|o| x240[o] + sol.tail[e][o] * fk).collect();
                let r960 = &kr["kref"][b.name()][*key][rk]["K960"];
                for o in 0..4 {
                    worst = worst.max((xt[o] - f64_of(&r960[o])).abs());
                }
                if rk == "0.99" {
                    let r480 = &kr["kref"][b.name()][*key][rk]["K480"];
                    for o in 0..4 {
                        worst = worst.max((xt[o] - f64_of(&r480[o])).abs());
                    }
                }
            }
        }
    }
    eprintln!("W1-32 forcing horizon max difference {worst:.3e}");
    assert!(worst <= 0.05, "forcing horizon error {worst}");
}

// ---------------------------------------------------------------- isolated bloc, goldens and bands

#[test]
fn own_hold12_matches_golden() {
    let g = golden(GOLD_STAGE1);
    let p = WorldParams::load();
    let mut wb = policy_block(100.0, 12, &BLOC_NAMES);
    let outs = run_zero(&mut wb, 49, true);
    for (i, b) in Bloc::ALL.iter().enumerate() {
        let h = &g[format!("hold12_{}", b.name())];
        let e = vec_of(&h["eps"]);
        for (j, x) in e.iter().enumerate() {
            assert!((p.sol[i].pin[2][j] - x).abs() < 1e-9, "{} pin {j}", b.name());
        }
        let mut worst_b: f64 = 0.0;
        for (m_idx, &m) in vec_of(&h["months"]).iter().enumerate() {
            let m = m as usize;
            for (v, idx) in [("y", OY), ("pi", OPI), ("i", OI), ("z", OZ)] {
                let want = f64_of(&h[v][m_idx]);
                let got = outs[m][i][idx];
                worst_b = worst_b.max((got - want).abs());
                assert!((got - want).abs() <= 1e-6, "{} {v} month {m}: {got} vs {want}", b.name());
            }
        }
        eprintln!("W1-06/hold-12 {} max difference against the golden {worst_b:.3e}", b.name());
    }
}

#[test]
fn own_hold12_bands() {
    // W1-01..W1-04 (F): own-policy GDP troughs of the reference hold-12 experiment, against the bands of targets.json.
    let mut wb = policy_block(100.0, 12, &BLOC_NAMES);
    let outs = run_zero(&mut wb, 49, true);
    let ea: Vec<f64> = (0..49).map(|t| outs[t][0][OY]).collect();
    let ea_min = ea.iter().cloned().fold(f64::INFINITY, f64::min);
    let ea_m = ea.iter().position(|&x| x == ea_min).unwrap();
    assert!((-0.75..=-0.40).contains(&ea_min), "W1-01 EA trough {ea_min}");
    assert!((12..=18).contains(&ea_m), "W1-01 trough month {ea_m}");
    let uk_min = (0..49).map(|t| outs[t][3][OY]).fold(f64::INFINITY, f64::min);
    assert!((-0.70..=-0.35).contains(&uk_min), "W1-02 UK trough {uk_min}");
    let ca_mean: f64 = (0..24).map(|t| outs[t][4][OY]).sum::<f64>() / 24.0;
    assert!((-0.50..=-0.30).contains(&ca_mean), "W1-03 CA mean {ca_mean}");
    let au_min = (0..49).map(|t| outs[t][9][OY]).fold(f64::INFINITY, f64::min);
    assert!((-1.00..=-0.60).contains(&au_min), "W1-04 AU trough {au_min}");
}

#[test]
fn monthly_irfs_match_golden() {
    // W1-06: unit innovations at t = 0 in the four equations, isolated blocs EA, JP, UK, MX, against irf_unit.json.
    let g = golden(GOLD_IRF);
    let names = ["demand", "cost_push", "policy", "risk_premium"];
    let mut worst: f64 = 0.0;
    for bname in ["EA", "JP", "UK", "MX"] {
        let bi = BLOC_NAMES.iter().position(|&n| n == bname).unwrap();
        for (j, sh) in names.iter().enumerate() {
            let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
            let zero = UsInputs::zero();
            for t in 0..61 {
                let mut e = [[0.0; 4]; NB];
                if t == 0 {
                    e[bi][j] = 1.0;
                }
                let x = wb.step_inputs(&zero, Some(&e), true);
                for (v, idx) in [("y", OY), ("pi", OPI), ("i", OI), ("z", OZ)] {
                    let want = f64_of(&g["irf"][bname][*sh][v][t]);
                    worst = worst.max((x[bi][idx] - want).abs());
                }
            }
        }
    }
    eprintln!("W1-06 unit IRF max difference {worst:.3e}");
    assert!(worst <= 1e-6, "unit IRF difference {worst}");
}

#[test]
fn pin_tables_hold_rate_one() {
    // Each bloc, each pin length: a 100bp BlocPolicy for L months holds the rate 1.0 above the rule for those months.
    for &l in &[3u32, 6, 12, 18, 24] {
        let mut wb = policy_block(100.0, l, &BLOC_NAMES);
        let outs = run_zero(&mut wb, l as usize, true);
        for (t, out) in outs.iter().enumerate() {
            for (b, o) in out.iter().enumerate() {
                assert!((o[OI] - 1.0).abs() < 1e-8, "L {l} bloc {b} month {t}: i {}", o[OI]);
            }
        }
    }
}

#[test]
fn pin_scales_linearly() {
    let mut a = policy_block(100.0, 12, &["EA"]);
    let mut b = policy_block(200.0, 12, &["EA"]);
    let xa = run_zero(&mut a, 36, true);
    let xb = run_zero(&mut b, 36, true);
    for t in 0..36 {
        for o in 0..5 {
            assert!((xb[t][0][o] - 2.0 * xa[t][0][o]).abs() < 1e-9, "month {t} output {o}");
        }
    }
}

#[test]
fn forcing_g_and_premium_paths() {
    // G_t = rho_G G_{t-1} + (1 - rho_G) ius_{t-3}; u_t = rho_rp u_{t-1} + eta_t (spec 2.2.2).
    let wp = WorldParams::load();
    let (rho_g, rho_rp) = (wp.financial.rho_g, wp.financial.rho_rp);
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    let mut ius = Vec::new();
    let mut eta = Vec::new();
    for t in 0..10 {
        ius.push((t as f64 + 1.0) * 0.1);
        let mut e = [[0.0; 4]; NB];
        e[0][3] = if t < 3 { 1.0 } else { 0.0 };
        eta.push(e[0][3]);
        let us = UsInputs { ius: ius[t], ..UsInputs::zero() };
        wb.step_inputs(&us, Some(&e), true);
    }
    let (mut g, mut u) = (0.0, 0.0);
    for t in 0..10 {
        let lagged = if t >= 3 { ius[t - 3] } else { 0.0 };
        g = rho_g * g + (1.0 - rho_g) * lagged;
        u = rho_rp * u + eta[t];
    }
    assert!((wb.blocs[0].g - g).abs() < 1e-12, "G {} vs {g}", wb.blocs[0].g);
    assert!((wb.blocs[0].u_rp - u).abs() < 1e-12, "u {} vs {u}", wb.blocs[0].u_rp);
}

#[test]
fn forcing_partner_term_and_dollar_addend() {
    // A dollar addend of 10 log points lowers the partner real exchange rates Peff by 100 a w_us: a positive demand effect.
    let mut base = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    let mut dollar = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    let us0 = UsInputs::zero();
    let us1 = UsInputs { dollar: 10.0, ..UsInputs::zero() };
    let (mut y0, mut y1) = (0.0, 0.0);
    for t in 0..3 {
        let x0 = base.step_inputs(&us0, None, false);
        let x1 = dollar.step_inputs(if t == 0 { &us1 } else { &us0 }, None, false);
        y0 = x0[0][OY];
        y1 = x1[0][OY];
    }
    assert!(y1 > y0, "dollar addend should raise EA demand: {y1} vs {y0}");
    assert!(y1 - y0 > 1e-6, "effect too small: {}", y1 - y0);
}

#[test]
fn nominal_rate_depreciates_with_higher_foreign_inflation() {
    // D11: ln S = z/100 + ln P_j - ln PT; higher foreign inflation at constant z raises ln S (foreign currency depreciates).
    let base = super::ln_s_level(1.5, 0.1, 0.2);
    let higher = super::ln_s_level(1.5, 0.1 + 0.02 / 12.0, 0.2);
    assert!(higher > base);
}

#[test]
fn price_level_and_floor_reporting() {
    let mut w = tiny_on(3, 400);
    for _ in 0..12 {
        sim::step(&mut w);
    }
    let wp = WorldParams::load();
    let last = w.history.last().unwrap().world.clone().expect("world metrics");
    for i in 0..NB {
        assert!(last.policy[i] >= wp.blocs[i].policy_rate_floor - 1e-12, "policy below floor for bloc {i}");
        assert!(last.s_index[i].is_finite() && last.p_index[i].is_finite());
    }
    let first = w.history[0].world.clone().expect("world metrics");
    for i in 0..NB {
        assert!((first.s_index[i] - 1.0).abs() < 1e-12, "S index at the first reported month");
        assert!((first.p_index[i] - 1.0).abs() < 1e-12, "P index at the first reported month");
    }
}

#[test]
fn us_inputs_okun_gap_and_deviation() {
    // Month 0 of a fresh world: the unemployment ring holds u_{t-1} (the natural rate, no history) three times, and
    // y_US = 100 okun (u* - u_{t-1}) with u* read after the month's set_policy.
    let mut w = tiny_on(5, 400);
    sim::step(&mut w);
    let wb = w.world.as_ref().unwrap();
    let nat = w.cfg.natural_unemployment;
    assert!(wb.us.u.iter().all(|&u| u == nat), "first-month ring is filled with u_{{t-1}}");
    let last = wb.last.clone().expect("month record");
    // N1: the centring constant is removed from the raw gap.
    let expect = 100.0 * 2.0 * (w.natural_unemployment() - nat) - wb.par.yus_offset;
    assert!((last.us.y_us - expect).abs() < 1e-9, "y_US {} vs {expect}", last.us.y_us);
}

#[test]
fn yus_offset_centres_the_us_gap_and_only_that() {
    // N1: the offset moves the block's y_US input by its constant (the history follows), and the ABM is untouched (shadow).
    let mut a = tiny_on(16, 400);
    let mut b = tiny_on(16, 400);
    a.world.as_mut().unwrap().par.yus_offset = 0.0;
    b.world.as_mut().unwrap().par.yus_offset = -0.91;
    for _ in 0..3 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    let (wa, wb) = (a.world.as_ref().unwrap(), b.world.as_ref().unwrap());
    let (ya, yb) = (wa.last.clone().unwrap().us.y_us, wb.last.clone().unwrap().us.y_us);
    assert!((yb - ya - 0.91).abs() < 1e-12, "y_US centred by +0.91 pp: {ya} vs {yb}");
    assert!((wb.us.y_us[0] - wa.us.y_us[0] - 0.91).abs() < 1e-12, "history follows the centred input");
    assert_eq!(a.history.last().unwrap().gdp_real_bn_ar, b.history.last().unwrap().gdp_real_bn_ar);
}

#[test]
fn paired_differences_do_not_depend_on_the_yus_offset() {
    // N1 leaves the paired rows (W1-21, W1-22, W1-27) unchanged: the block is linear in its US inputs (the bound acts on ius
    // and ex only), so a constant removed from both arms cancels in the difference of the two arms' outputs.
    let raw_a = |t: usize| 0.8 * (0.2 * t as f64).sin() - 0.5;
    let raw_b = |t: usize| raw_a(t) + 0.6 * (-0.05 * t as f64).exp();
    let run = |raw: &dyn Fn(usize) -> f64, c: f64| -> Vec<[[f64; 5]; NB]> {
        let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
        (0..60).map(|t| wb.step_inputs(&UsInputs { ius: 0.5, y_us: raw(t) - c, ..UsInputs::zero() }, None, false)).collect()
    };
    let (a0, b0) = (run(&raw_a, 0.0), run(&raw_b, 0.0));
    let (a1, b1) = (run(&raw_a, -0.91), run(&raw_b, -0.91));
    for t in 0..60 {
        for i in 0..NB {
            for o in 0..5 {
                let (d0, d1) = (a0[t][i][o] - b0[t][i][o], a1[t][i][o] - b1[t][i][o]);
                assert!((d0 - d1).abs() < 1e-9, "t {t} bloc {i} out {o}: difference {d0} vs {d1}");
            }
        }
    }
    assert!((a1[10][0][OY] - a0[10][0][OY]).abs() > 1e-3, "the level does move with the offset");
}

#[test]
fn y_us_ring_initialisation() {
    let mut w = tiny_on(6, 400);
    sim::step(&mut w);
    let u0 = w.world.as_ref().unwrap().us.u;
    assert!(u0[0] == u0[1] && u0[1] == u0[2], "ring filled on the first month");
    sim::step(&mut w);
    let u1 = w.world.as_ref().unwrap().us.u;
    assert!(u1[1] == u0[0], "second month shifts the ring");
}

#[test]
fn reported_flag_and_base_capture() {
    let mut w = tiny_on(7, 400);
    w.cfg.burn_in_months = 10;
    w.tick = 3;
    assert!(!super::reported(&w), "burn-in month is not reported");
    w.tick_offset = 5;
    w.tick = 0;
    assert!(super::reported(&w), "after the tick reset the run is reported");
    // Bases are captured exactly once, at the first reported step.
    let mut w = tiny_on(8, 400);
    sim::step(&mut w);
    let b0 = w.world.as_ref().unwrap().bases.clone().expect("bases at the first reported month");
    sim::step(&mut w);
    let b1 = w.world.as_ref().unwrap().bases.clone().unwrap();
    for i in 0..NB {
        assert_eq!(b0.ln_p[i], b1.ln_p[i]);
        assert_eq!(b0.ln_s[i], b1.ln_s[i]);
    }
    assert_eq!(b0.ln_pt, b1.ln_pt);
}

#[test]
fn ius_offset_shifts_inputs_and_expected_path() {
    // The offset shifts ius and every ex[k] by the decaying amount, and leaves the ABM untouched (shadow mode).
    let mut a = tiny_on(9, 400);
    let mut b = tiny_on(9, 400);
    a.world.as_mut().unwrap().par.ius_offset = 0.0;
    b.world.as_mut().unwrap().par.ius_offset = -0.43;
    sim::step(&mut a);
    sim::step(&mut b);
    let ua = a.world.as_ref().unwrap().last.clone().unwrap().us;
    let ub = b.world.as_ref().unwrap().last.clone().unwrap().us;
    // ius = 100 (policy - i_inf) - offset: a -0.43 offset adds +0.43 pp to ius and to ex[k] (decaying along the path).
    assert!((ub.ius - ua.ius - 0.43).abs() < 1e-12, "ius shifted by +0.43 pp");
    assert!((ub.ex[0] - ua.ex[0] - 0.43).abs() < 1e-12, "ex[0] shifted");
    for k in [1usize, 10, 50] {
        let want = 0.43 * 0.97f64.powi(k as i32);
        assert!(((ub.ex[k] - ua.ex[k]) - want).abs() < 1e-10, "ex[{k}] offset decays at 0.97");
    }
    assert_eq!(a.history.last().unwrap().gdp_real_bn_ar, b.history.last().unwrap().gdp_real_bn_ar);
}

#[test]
fn expected_path_means_equal_curve_expected() {
    // H5: the mean of the first TENORS[j] entries of expected_rate_path equals curve.expected[j] on the rule path with and
    // without a policy gap, on an override, and on a pinned override (three branches).
    let check = |w: &mut World| {
        monetary::update_curve(w);
        let mut path = vec![0.0; 360];
        monetary::expected_rate_path(w, &mut path);
        for j in 0..N_TENORS {
            let n = TENORS[j] as usize;
            let mean: f64 = path[..n].iter().sum::<f64>() / n as f64;
            assert!((mean - w.curve.expected[j]).abs() < 1e-10, "tenor {} mean {mean} vs {}", TENORS[j], w.curve.expected[j]);
        }
    };
    let mut w = tiny_off(11, 400);
    for _ in 0..3 {
        sim::step(&mut w);
    }
    check(&mut w);
    let mut w = tiny_off(12, 400);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
    sim::step(&mut w);
    sim::step(&mut w);
    check(&mut w);
    let mut w = tiny_off(13, 400);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyRate { rate: 0.03, months: 24 } });
    sim::step(&mut w);
    check(&mut w);
    let mut w = tiny_off(14, 400);
    w.cfg.pin_path_on = true;
    w.cfg.pin_exit_hazard = 0.02;
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyRate { rate: 0.03, months: 24 } });
    sim::step(&mut w);
    check(&mut w);
}

/// Mean over the tenors of the expected path against the curve's expected yields, with the maximum difference.
fn expected_path_gap(w: &World) -> f64 {
    let mut path = vec![0.0; 360];
    monetary::expected_rate_path(w, &mut path);
    (0..N_TENORS)
        .map(|j| {
            let n = TENORS[j] as usize;
            let mean: f64 = path[..n].iter().sum::<f64>() / n as f64;
            (mean - w.curve.expected[j]).abs()
        })
        .fold(0.0, f64::max)
}

#[test]
fn expected_path_with_rule_path_weight_and_gap() {
    // H5 with rule_path_weight > 0: the rule branch with a custom decay and i_star != base, with and without a policy gap.
    for (wgt, dec) in [(0.6_f64, 0.95_f64), (1.0, 0.97), (0.4, 0.99)] {
        for gap in [false, true] {
            let cfg = Config { n_households: 400, rule_path_weight: wgt, rule_path_decay: dec, world_block_enabled: false, ..Default::default() };
            let mut w = testkit::tiny_world(cfg, 31);
            if gap {
                w.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::PolicyShift { bps: 150.0, months: 12 } });
            }
            for _ in 0..5 {
                sim::step(&mut w);
            }
            monetary::update_curve(&mut w);
            let d = expected_path_gap(&w);
            assert!(d < 1e-10, "weight {wgt} decay {dec} gap {gap}: {d}");
        }
    }
}

#[test]
fn expected_path_at_zero_floor() {
    // H5 at the zero floor: a large easing holds the policy rate at its floor, and the path still means to the curve.
    let cfg = Config { n_households: 400, world_block_enabled: false, ..Default::default() };
    let mut w = testkit::tiny_world(cfg, 33);
    w.shocks.schedule(ScheduledShock { at_month: 1, shock: Shock::PolicyShift { bps: -900.0, months: 24 } });
    for _ in 0..6 {
        sim::step(&mut w);
    }
    monetary::update_curve(&mut w);
    assert!(w.cb.policy_rate == 0.0 || w.monetary.curve_inputs.base < 0.005, "the easing reaches the floor");
    let d = expected_path_gap(&w);
    assert!(d < 1e-10, "zero floor: {d}");
}

#[test]
fn bloc_shocks_scheduled_through_shock_state_reach_the_block() {
    // A bloc shock scheduled on the ABM's ShockState fires at its month (`activate_due`), and the block takes each one once,
    // with its activation month on the bloc clock.
    let mut w = tiny_on(17, 400);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::BlocPolicy { bloc: "EA".to_string(), bps: 100.0, months: 12 } });
    w.shocks.schedule(ScheduledShock {
        at_month: 2,
        shock: Shock::BlocDemand { bloc: "CN".to_string(), pct: -3.0, half_life_months: 12.0, risk_off: false },
    });
    sim::step(&mut w);
    {
        let wb = w.world.as_ref().unwrap();
        assert_eq!(w.shocks.world_shocks.len(), 1, "the policy shock fired in month 0");
        assert_eq!(wb.users.len(), 1);
        assert_eq!((wb.users[0].bloc, wb.users[0].t0), (super::bloc_index("EA"), 0));
        assert!(wb.out.i_gap[0] > 0.0, "the EA policy deviation rises in the shock month: {}", wb.out.i_gap[0]);
    }
    sim::step(&mut w);
    sim::step(&mut w);
    let wb = w.world.as_ref().unwrap();
    assert_eq!(w.shocks.world_shocks.len(), 2, "both shocks fired");
    assert_eq!(wb.users.len(), 2);
    assert_eq!((wb.users[1].bloc, wb.users[1].t0), (super::bloc_index("CN"), 2));
    assert_eq!(wb.consumed, 2, "each fired shock is taken once");
}

#[test]
fn shock_file_with_unknown_bloc_is_rejected_before_use() {
    // world-dump reads its shock file through `parse_schedule`: a bad entry is an error, never a panic in `bloc_index`.
    let good = r#"[{"at_month":3,"shock":{"type":"BlocPolicy","bloc":"EA","bps":100.0,"months":12}}]"#;
    assert_eq!(crate::shocks::parse_schedule(good).unwrap().len(), 1);
    let bad = r#"[{"at_month":3,"shock":{"type":"BlocPolicy","bloc":"XX","bps":100.0,"months":12}}]"#;
    let err = crate::shocks::parse_schedule(bad).unwrap_err();
    assert!(err.contains("entry 0") && err.contains("bloc"), "{err}");
    assert!(crate::shocks::parse_schedule("{\"not\": \"an array\"}").is_err());
}

#[test]
fn ius_bounded_by_rate_sat() {
    // M-F (D33): the bound applies above the knee when the scale is positive; identity otherwise. G uses the unbounded input.
    let off = Config { world_block_enabled: true, ..Default::default() };
    let on = Config { world_block_enabled: true, fx_rate_knee: 0.02, fx_rate_sat_scale: 0.015, ..Default::default() };
    let p_off = Par::from_config(&off, &WorldParams::load());
    let p_on = Par::from_config(&on, &WorldParams::load());
    assert_eq!(bloc::bound_pp(5.0, &p_off), 5.0);
    let b = bloc::bound_pp(5.0, &p_on);
    assert!(b < 5.0 && b > 2.0, "bounded {b}");
    assert_eq!(bloc::bound_pp(1.0, &p_on), 1.0, "identity below the knee");
    let mut a = WorldBlock::new(&off);
    let mut c = WorldBlock::new(&on);
    let us = UsInputs { ius: 5.0, ex: [5.0; K], ..UsInputs::zero() };
    let x_off = a.step_inputs(&us, None, true);
    let x_on = c.step_inputs(&us, None, true);
    assert!((a.blocs[0].g - c.blocs[0].g).abs() < 1e-15, "G does not see the bound");
    assert!((x_off[0][OY] - x_on[0][OY]).abs() > 0.0, "the rule and UIP forcing see the bound");
}

#[test]
fn shock_draws_keyed_and_thread_independent() {
    let run = |threads: usize| -> Value {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        pool.install(|| {
            let mut w = tiny_on(15, 400);
            for _ in 0..12 {
                sim::step(&mut w);
            }
            serde_json::to_value(&w.history).unwrap()
        })
    };
    assert_eq!(run(1), run(4));
}

#[test]
fn zero_scale_shocks_still_consume_draws() {
    let mut w = tiny_on(16, 400);
    let raw1 = super::draws_raw(&w, 0);
    w.world.as_mut().unwrap().par.shock_scale = 0.0;
    let raw0 = super::draws_raw(&w, 0);
    assert_eq!(raw0, raw1, "the normals are drawn whatever the scale");
    let wb = w.world.as_ref().unwrap();
    let e = super::draws(&w, &wb.params, &wb.par);
    assert!(e.iter().flatten().all(|&x| x == 0.0), "zero scale gives zero innovations");
}

#[test]
fn world_off_has_no_world_key_in_json() {
    let mut w = tiny_off(17, 400);
    for _ in 0..6 {
        sim::step(&mut w);
    }
    let v = serde_json::to_value(&w.history).unwrap();
    for rec in v.as_array().unwrap() {
        assert!(rec.get("world").is_none(), "no world key when the block is off");
    }
}

#[test]
fn shadow_mode_leaves_abm_bit_identical() {
    // W1-29 (engineering, tiny world): with the block on and no link, every non-world key equals the block-off run.
    let mut on = tiny_on(18, 400);
    let mut off = tiny_off(18, 400);
    for _ in 0..24 {
        sim::step(&mut on);
        sim::step(&mut off);
    }
    let strip = |v: Value| -> Value {
        Value::Array(
            v.as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r.as_object_mut().unwrap().remove("world");
                    r
                })
                .collect(),
        )
    };
    let a = serde_json::to_value(&on.history).unwrap();
    assert!(a[23].get("world").is_some());
    assert_eq!(strip(a), strip(serde_json::to_value(&off.history).unwrap()));
}

#[test]
fn metrics_mean_includes_world() {
    let mut w = tiny_on(19, 400);
    for _ in 0..3 {
        sim::step(&mut w);
    }
    let h = &w.history;
    let mean = Metrics::mean(h[1..3].iter(), 2.0);
    let a = h[1].world.as_ref().unwrap();
    let b = h[2].world.as_ref().unwrap();
    let m = mean.world.as_ref().expect("mean keeps the world block");
    for i in 0..NB {
        assert!((m.y_gap[i] - 0.5 * (a.y_gap[i] + b.y_gap[i])).abs() < 1e-12);
    }
}

#[test]
fn world_stream_ids_registered() {
    for id in [stream::WORLD_BLOC, stream::WORLD_OIL_I, stream::WORLD_OIL_S, stream::WORLD_FX] {
        assert!(rng::ALL_STREAMS.contains(&id), "stream {id:#x} registered");
    }
    assert_eq!(stream::WORLD_BLOC, 0x574F_524C_0000_0009);
    assert_eq!(stream::WORLD_FX, 0x574F_524C_0000_000C);
}

#[test]
fn bloc_demand_peak_equals_pct() {
    // BlocDemand scales the isolated bloc so that its extreme over the scaling horizon equals pct (spec 2.2.6): the maximum
    // for a positive shock, the minimum for a negative one.
    for &(hl, pct) in &[(12.0_f64, 3.0_f64), (4.0, -2.0), (30.0, 5.0)] {
        let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
        wb.add_user(&Shock::BlocDemand { bloc: "EA".to_string(), pct, half_life_months: hl, risk_off: false }, 0);
        let n = ((4.0 * hl).ceil().max(36.0)).min(240.0) as usize;
        let ys: Vec<f64> = run_zero(&mut wb, n, true).iter().map(|x| x[0][OY]).collect();
        let extreme = if pct > 0.0 { ys.iter().cloned().fold(f64::NEG_INFINITY, f64::max) } else { ys.iter().cloned().fold(f64::INFINITY, f64::min) };
        assert!((extreme - pct).abs() < 1e-6, "hl {hl} pct {pct}: extreme {extreme}");
    }
}

#[test]
fn shock_validation_rejects_bad_bloc_and_months() {
    assert!(Shock::BlocPolicy { bloc: "EA".into(), bps: 100.0, months: 12 }.validate().is_ok());
    assert!(Shock::BlocPolicy { bloc: "XX".into(), bps: 100.0, months: 12 }.validate().is_err());
    assert!(Shock::BlocPolicy { bloc: "EA".into(), bps: 100.0, months: 7 }.validate().is_err());
    assert!(Shock::BlocPolicy { bloc: "OPEC+".into(), bps: 2500.0, months: 12 }.validate().is_err());
    assert!(Shock::BlocDemand { bloc: "CN".into(), pct: -3.0, half_life_months: 12.0, risk_off: true }.validate().is_ok());
    assert!(Shock::BlocDemand { bloc: "CN".into(), pct: -3.0, half_life_months: 0.5, risk_off: false }.validate().is_err());
    assert!(Shock::BlocDemand { bloc: "RoW".into(), pct: 25.0, half_life_months: 12.0, risk_off: false }.validate().is_err());
    assert!(Shock::GlobalDemand { pct: 2.0, half_life_months: 12.0 }.validate().is_ok());
    assert!(Shock::GlobalDemand { pct: 2.0, half_life_months: 200.0 }.validate().is_err());
    let j = serde_json::to_value(Shock::BlocPolicy { bloc: "OPEC+".into(), bps: 100.0, months: 3 }).unwrap();
    assert_eq!(j["type"], "BlocPolicy");
    assert_eq!(j["bloc"], "OPEC+");
}

#[test]
fn world_clone_does_not_alias() {
    let mut w = tiny_on(20, 400);
    sim::step(&mut w);
    let copy = w.world.as_ref().unwrap().clone();
    sim::step(&mut w);
    let now = w.world.as_ref().unwrap();
    assert_eq!(copy.n + 1, now.n);
    assert_ne!(copy.blocs[0].ln_p, now.blocs[0].ln_p);
}

#[test]
fn uae_flag_conserves_world_gdp() {
    let wp = WorldParams::load();
    let (ui, pi) = net::gdp_weights(&wp, true);
    let (uo, po) = net::gdp_weights(&wp, false);
    let s = |v: &[f64]| v.iter().sum::<f64>();
    assert!((s(&ui) - s(&uo)).abs() < 1e-9, "usd weights conserved: {} vs {}", s(&ui), s(&uo));
    assert!((s(&pi) - s(&po)).abs() < 1e-9, "ppp weights conserved");
    let (o, r) = (Bloc::OpecPlus.idx(), Bloc::RoW.idx());
    assert!(uo[o] < ui[o] && uo[r] > ui[r], "the UAE moves from OPEC+ to RoW");
    assert!(((ui[o] - uo[o]) - (uo[r] - ui[r])).abs() < 1e-12, "the transfer is equal and opposite");
}

// ---------------------------------------------------------------- network and golden replays

#[test]
fn network_in_path_matches_golden() {
    // The clean replay of the Iacoviello-Navarro path (no extra shock), 60 months: per-bloc outputs against the stored `out`
    // (1e-6), and the group statistics W1-10..W1-20 against the reference and their bands.
    let (outs, worst) = replay(GOLD_CLEAN);
    eprintln!("W1-23 clean golden replay max difference {worst:.3e}");
    assert!(worst <= 1e-6, "replay difference {worst}");
    let g = groups(&outs);
    let gate = golden(GOLD_STAGE1);
    let want = &gate["gates"];
    let checks = [
        ("W1-10", g.ae_ytr, -0.60, -0.30),
        ("W1-11", g.eme_ytr, -0.70, -0.30),
        ("W1-12", g.ae_y12, -0.30, 0.10),
        ("W1-13", g.ae_i3, -0.20, 0.50),
        ("W1-14", g.ae_i24, -1.20, -0.20),
        ("W1-15", g.flo_imax, 0.20, 1.20),
        ("W1-16", g.ae_zpk, 4.0, 12.0),
        ("W1-17", g.ae_z24, 2.0, 10.0),
        ("W1-18", g.flo_zpk, 4.0, 16.0),
        ("W1-19", g.flo_z24, 2.0, 16.0),
        ("W1-20", g.cn_ytr, -0.90, -0.10),
    ];
    for (id, v, lo, hi) in checks {
        let r = f64_of(&want[id]);
        assert!((v - r).abs() < 1e-6, "{id} rust {v} vs golden {r}");
        assert!((lo..=hi).contains(&v), "{id} {v} outside [{lo}, {hi}]");
    }
    assert_eq!(g.ae_ytr_m, 26, "AE trough month (W1-10 expected at m26)");
}

#[test]
fn replay_stress_parity() {
    // W1-23 (E): every forcing channel with the M-F bound on (knee 0.02, scale 0.015), against inputs_stress.jsonl (1e-6).
    let (_outs, worst) = replay(GOLD_STRESS);
    eprintln!("W1-23 stress golden replay max difference {worst:.3e}");
    assert!(worst <= 1e-6, "stress replay difference {worst}");
}

#[test]
fn oil_gap_channel_fsgm_troughs() {
    // W1-24..W1-26 (F): a permanent +10 oil gap held for 85 months, zero US policy inputs, network on.
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    let us = UsInputs { oil: 10.0, ..UsInputs::zero() };
    let outs: Vec<_> = (0..85).map(|_| wb.step_inputs(&us, None, false)).collect();
    let tr = |b: usize| (0..85).map(|t| outs[t][b][OY]).fold(f64::INFINITY, f64::min);
    let ea = tr(0);
    let jp = tr(2);
    let cn = tr(1);
    let other: f64 = [6usize, 7, 8, 5, 11].iter().map(|&b| tr(b)).sum::<f64>() / 5.0;
    assert!((-0.80..=-0.35).contains(&ea), "W1-24 EA trough {ea}");
    assert!((-0.95..=-0.50).contains(&jp), "W1-25 JP trough {jp}");
    assert!((-1.00..=-0.55).contains(&cn), "W1-25 CN trough {cn}");
    assert!((-0.70..=-0.20).contains(&other), "W1-26 other EM trough {other}");
}

#[test]
fn china_demand_spillover_ratio() {
    // W1-27 (G): peak output response of EA and UK relative to CN under BlocDemand CN -3, half-life 12, network on.
    let mut wb = WorldBlock::new(&Config { world_block_enabled: true, ..Default::default() });
    wb.add_user(&Shock::BlocDemand { bloc: "CN".into(), pct: -3.0, half_life_months: 12.0, risk_off: true }, 0);
    let outs = run_zero(&mut wb, 60, false);
    let pk = |b: usize| (0..60).map(|t| outs[t][b][OY].abs()).fold(0.0, f64::max);
    let cn = pk(1);
    let ea = pk(0) / cn;
    let uk = pk(3) / cn;
    assert!((0.05..=0.20).contains(&ea), "W1-27a EA {ea}");
    assert!((0.02..=0.15).contains(&uk), "W1-27b UK {uk}");
}

#[test]
fn gvar_lower_bound_report() {
    // W1-28 (R): peak output of any other bloc after a 100bp EA policy shock held 12 months (reported, no band).
    let mut wb = policy_block(100.0, 12, &["EA"]);
    let outs = run_zero(&mut wb, 60, false);
    let best = (1..NB).map(|b| (0..60).map(|t| outs[t][b][OY].abs()).fold(0.0, f64::max)).fold(0.0, f64::max);
    assert!(best.is_finite() && best > 0.0, "reported value {best}");
}

#[test]
fn baseline_centring_smoke() {
    // W1-31 smoke (loose): the default block in a small ABM world over 240 months stays bounded.
    let mut w = tiny_on(21, 400);
    for _ in 0..240 {
        sim::step(&mut w);
    }
    for rec in &w.history {
        let m = rec.world.as_ref().unwrap();
        assert!(m.y_gap.iter().chain(m.z.iter()).all(|x| x.is_finite()));
        assert!(m.y_gap.iter().all(|y| y.abs() < 50.0), "output gap bounded");
    }
}

#[test]
#[ignore = "measures world_yus_offset (N1); run: YUS_SEEDS=1-12 RAYON_NUM_THREADS=1 cargo test --release -p econsim-core yus_offset_measure -- --ignored --nocapture"]
fn yus_offset_measure() {
    // The mean of the block's own y_US over the 240 reported months of the default run (`run 20000 240 seed`: burn-in 144,
    // block on). The offset is zero here, so the raw gap is read. Shadow mode: the same ABM as the base binary.
    let spec = std::env::var("YUS_SEEDS").unwrap_or_else(|_| "1-12".to_string());
    let (a, b) = spec.split_once('-').expect("YUS_SEEDS as a-b");
    let (a, b): (u64, u64) = (a.parse().expect("first seed"), b.parse().expect("last seed"));
    let mut means = Vec::new();
    for seed in a..=b {
        let cfg = Config { n_households: 20_000, world_block_enabled: true, world_yus_offset: 0.0, ..Config::default() };
        let mut w = sim::new_world(&cfg, seed);
        let mut sum = 0.0;
        for _ in 0..240 {
            sim::step(&mut w);
            sum += w.world.as_ref().unwrap().last.as_ref().unwrap().us.y_us;
        }
        eprintln!("YUS seed {seed}: mean y_US over months 0-239 {:.6}", sum / 240.0);
        means.push(sum / 240.0);
    }
    let n = means.len() as f64;
    let mean = means.iter().sum::<f64>() / n;
    let sd = (means.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / (n - 1.0)).sqrt();
    eprintln!("YUS seeds {a}-{b}: mean {mean:.6} pp, sd {sd:.4}, SE {:.4}", sd / n.sqrt());
}

#[test]
#[ignore = "cost measurement (W1-30) is done by scripts/world/world_check.py cost"]
fn cost_smoke() {
    let mut on = tiny_on(22, 2000);
    let mut off = tiny_off(22, 2000);
    let t0 = std::time::Instant::now();
    for _ in 0..24 {
        sim::step(&mut off);
    }
    let t_off = t0.elapsed();
    let t1 = std::time::Instant::now();
    for _ in 0..24 {
        sim::step(&mut on);
    }
    eprintln!("off {t_off:?} on {:?}", t1.elapsed());
}
