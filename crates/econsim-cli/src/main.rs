//! Headless runner.
//!   econsim-cli run [households] [months] [seed] [--json]   monthly summary table
//!   econsim-cli irf [households] [months] [seeds]           validation impulse responses (DESIGN §14)
//!   econsim-cli config                                      print the resolved Config (ECONSIM_CFG applied) as JSON
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use econsim_core::metrics::Metrics;
use econsim_core::{sim, Config, ScheduledShock, Scenario, Shock};
use std::time::Instant;

fn arg<T: std::str::FromStr>(args: &[String], i: usize, d: T) -> T {
    args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d)
}

/// The ECONSIM_CFG overlay, refused with exit status 2 when the combination is not supported (stage 4-1, S4-R14): the dollar link
/// without the trade link. The same checks the server makes (`Config::validate_world_links`, `validate_rz2`, and
/// `validate_clamp_bounds` for bound pairs that would panic in the run).
fn config_from_env() -> Config {
    let cfg = Config::from_env();
    if let Err(e) = cfg.validate_world_links() {
        eprintln!("econsim-cli: {e}");
        std::process::exit(2);
    }
    if let Err(e) = cfg.validate_rz2() {
        eprintln!("econsim-cli: {e}");
        std::process::exit(2);
    }
    if let Err(e) = cfg.validate_clamp_bounds() {
        eprintln!("econsim-cli: {e}");
        std::process::exit(2);
    }
    cfg
}

fn main() {
    sim::configure_thread_pool();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("irf") => irf(&args),
        Some("run") => run(&args[1..]),
        Some("config") => println!("{}", serde_json::to_string_pretty(&config_from_env()).unwrap()),
        Some("world-dump") => world_dump(&args),
        Some("oil-check") => oil_check(&args),
        _ => run(&args),
    }
}

fn run(args: &[String]) {
    let n: usize = arg(args, 1, 20_000);
    let months: u32 = arg(args, 2, 120);
    let seed: u64 = arg(args, 3, 1);
    let burn: u32 = arg(args, 4, Config::default().burn_in_months);
    let cfg = Config { n_households: n, burn_in_months: burn, ..config_from_env() };
    let t0 = Instant::now();
    let mut w = sim::new_world(&cfg, seed);
    let t_init = t0.elapsed();
    let t1 = Instant::now();
    println!(
        "{:>5} {:>8} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>9} {:>6} {:>7} {:>6} {:>6}",
        "tick", "GDPr", "g%", "u%", "π%", "core%", "i%", "10y%", "mtg%", "HPI$", "own%", "oil$", "firms", "debt%"
    );
    for _ in 0..months {
        sim::step(&mut w);
        let m = w.history.last().unwrap();
        if m.tick % 6 == 0 {
            println!(
                "{:>5} {:>8.0} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>6.2} {:>9.0} {:>6.1} {:>7.1} {:>6.0} {:>6.1}",
                m.tick,
                m.gdp_real_bn_ar,
                m.gdp_real_growth_yoy,
                m.unemployment_rate,
                m.inflation_yoy,
                m.core_inflation_yoy,
                m.policy_rate,
                m.yields[3],
                m.mortgage_rate,
                m.median_house_price,
                m.homeownership_rate,
                m.oil_price,
                m.firms_alive,
                m.gov_debt_gdp
            );
        }
    }
    let el = t1.elapsed();
    eprintln!(
        "init {:.2?}; {} months in {:.2?} ({:.1} months/s) with {} households",
        t_init,
        months,
        el,
        months as f64 / el.as_secs_f64(),
        n
    );
    if args.iter().any(|a| a == "--json") {
        println!("{}", serde_json::to_string(&w.history).unwrap());
    }
}

/// Prints a usage error to stderr and exits with status 2.
fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(2)
}

/// b8-world H18: the replay file of spec 2.1 for the reference parity test (W1-23). `world-dump HOUSEHOLDS MONTHS SEED
/// [SHOCK_JSON]`: a JSON array of ScheduledShock, written to stdout as a header line and one line per month. The block is on,
/// the burn-in is zero (so the bloc clock is the replay month), and the ECONSIM_CFG overlay is applied.
fn world_dump(args: &[String]) {
    use econsim_core::shocks::Shock;
    use serde_json::json;
    let households: usize = arg(args, 2, 20_000);
    let months: u32 = arg(args, 3, 60);
    let seed: u64 = arg(args, 4, 1);
    // The shock file is validated before use (bloc names, ranges): a bad entry is a usage error, not a panic in the block.
    let shocks: Vec<ScheduledShock> = match args.get(5) {
        Some(p) => {
            let text = std::fs::read_to_string(p).unwrap_or_else(|e| fail(&format!("world-dump: cannot read {p}: {e}")));
            econsim_core::shocks::parse_schedule(&text).unwrap_or_else(|e| fail(&format!("world-dump: {p}: {e}")))
        }
        None => Vec::new(),
    };
    let cfg = Config { n_households: households, burn_in_months: 0, world_block_enabled: true, ..config_from_env() };
    let mut w = sim::new_world(&cfg, seed);
    for s in &shocks {
        w.shocks.schedule(s.clone());
    }
    let wb = w.world.as_ref().expect("the world block is on");
    let listed: Vec<serde_json::Value> = shocks
        .iter()
        .filter_map(|s| match &s.shock {
            Shock::BlocPolicy { bloc, bps, months } => Some(json!({"type": "BlocPolicy", "bloc": bloc, "bps": bps, "months": months, "t0": s.at_month})),
            Shock::BlocDemand { bloc, pct, half_life_months, .. } => {
                Some(json!({"type": "BlocDemand", "bloc": bloc, "pct": pct, "half_life_months": half_life_months, "t0": s.at_month}))
            }
            Shock::GlobalDemand { pct, half_life_months } => {
                Some(json!({"type": "GlobalDemand", "pct": pct, "half_life_months": half_life_months, "t0": s.at_month}))
            }
            _ => None,
        })
        .collect();
    let header = json!({"header": {
        "k": 240, "rho_f": wb.par.rho_f, "rho_p": wb.par.rho_p, "rho_oil": wb.par.rho_oil, "rho_qty": wb.par.rho_qty,
        "ius_offset": wb.par.ius_offset, "yus_offset": wb.par.yus_offset, "fx_rate_knee": wb.par.knee, "fx_rate_sat_scale": wb.par.sat,
        "pstar_sha": wb.params.meta.pstar_sha, "shocks": listed,
    }});
    println!("{header}");
    for _ in 0..months {
        sim::step(&mut w);
        let rec = w.world.as_ref().and_then(|b| b.last.clone()).expect("a month record");
        let t = w.world.as_ref().unwrap().n - 1;
        let eps: Vec<Vec<f64>> = rec.eps.iter().map(|e| e.to_vec()).collect();
        let col = |o: usize| -> Vec<f64> { rec.out.iter().map(|x| x[o]).collect() };
        let line = json!({
            "t": t, "ius": rec.us.ius, "ex": rec.us.ex.to_vec(), "y_us": rec.us.y_us, "oil": rec.us.oil, "qty": rec.us.qty,
            "fex": rec.us.fex, "dollar": rec.us.dollar, "dollar_decay": rec.us.dollar_decay, "eps": eps,
            "out": {"y": col(0), "pi": col(1), "i": col(2), "z": col(3)},
        });
        println!("{line}");
    }
}

type Getter = fn(&Metrics) -> f64;

fn irf(args: &[String]) {
    let n: usize = arg(args, 2, 10_000);
    let months: u32 = arg(args, 3, 36);
    let nseeds: u64 = arg(args, 4, 4);
    let cfg = Config { n_households: n, ..config_from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("Policy shock +100bp vs rule, 12m", at(Shock::PolicyShift { bps: 100.0, months: 12 })),
        ("Policy rate pinned at 7% for 24m", at(Shock::PolicyRate { rate: 0.07, months: 24 })),
        ("Oil +50% (half-life 12m)", at(Shock::OilPrice { pct: 50.0, half_life_months: 12.0 })),
        ("10y term premium +100bp", at(Shock::TermPremium { tenor_idx: 3, bps: 100.0, half_life_months: 24.0 })),
        ("QE 0.5% GDP/month x12 @10y", at(Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 })),
        ("Helicopter $2,000 x 3m", at(Shock::Helicopter { per_household: 2_000.0, months: 3 })),
        ("Tech boom 72m (forced regime)", at(Shock::TechBoom { months: 72 })),
        ("Gov purchases +5% (≈1% of GDP) x 24m", at(Shock::GovSpending { pct: 5.0, months: 24 })),
    ];
    let vars: Vec<(&str, Getter, bool)> = vec![
        ("real GDP %", |m| m.gdp_real_bn_ar, true),
        ("real cons %", |m| m.consumption_real_bn_ar, true),
        ("durables %", |m| m.consumption_by_sector_real[2], true),
        ("energy cons %", |m| m.consumption_by_sector_real[0], true),
        ("real inv %", |m| m.investment_real_bn_ar, true),
        ("gov real %", |m| m.obs.government_real_bn_ar, true),
        ("unemp pp", |m| m.unemployment_rate, false),
        ("CPI yoy pp", |m| m.inflation_yoy, false),
        ("core yoy pp", |m| m.core_inflation_yoy, false),
        ("policy pp", |m| m.policy_rate, false),
        ("10y pp", |m| m.yields[3], false),
        ("dollar %", |m| m.dollar_index, true),
        ("mortgage pp", |m| m.mortgage_rate, false),
        ("shelter yoy pp", |m| m.shelter_inflation_yoy, false),
        ("house px %", |m| m.median_house_price, true),
        ("sales %", |m| m.house_sales_millions_ar, true),
        ("starts %", |m| m.housing_starts_millions_ar, true),
        ("fund px %", |m| m.fund_share_price, true),
        ("tech eq %", |m| m.equity_value_by_sector[4], true),
        ("tech inv %", |m| m.tech_investment_bn_ar, true),
        ("deposits %", |m| m.deposits_bn, true),
        ("wage gr pp", |m| m.wage_growth_yoy, false),
        ("spread pp", |m| m.credit_spread, false),
    ];
    let horizons = [3usize, 6, 12, 24, 36];
    for (name, shocks) in scenarios {
        let t = Instant::now();
        let sc = Scenario { config: cfg.clone(), shocks, months, seeds: (1..=nseeds).collect() };
        let c = sim::compare(&sc);
        println!("\n=== {name}  ({:.1?}) ===", t.elapsed());
        print!("{:<14}", "");
        for h in horizons {
            print!("{:>9}", format!("m{h}"));
        }
        println!();
        for (vn, g, pct) in &vars {
            print!("{:<14}", vn);
            for h in horizons {
                if h > c.baseline.len() {
                    print!("{:>9}", "-");
                    continue;
                }
                let (b, s) = (g(&c.baseline[h - 1]), g(&c.shocked[h - 1]));
                let d = if *pct { if b.abs() > 1e-9 { (s / b - 1.0) * 100.0 } else { 0.0 } } else { s - b };
                print!("{:>9.2}", d);
            }
            println!();
        }
        if name.starts_with("Gov purchases") {
            // Cumulative multiplier: sum over t <= H of the GDP level gap, over the same sum of the government gap.
            for hz in [12usize, 24] {
                let h = hz.min(c.baseline.len()).min(c.shocked.len());
                let dy: f64 = (0..h).map(|t| c.shocked[t].gdp_real_bn_ar - c.baseline[t].gdp_real_bn_ar).sum();
                let dg: f64 = (0..h)
                    .map(|t| c.shocked[t].obs.government_real_bn_ar - c.baseline[t].obs.government_real_bn_ar)
                    .sum();
                println!("cumulative multiplier H={hz}: {:.2}", dy / dg);
            }
            // Impact multiplier at m3 (spec 8G F1): level gap in GDP over level gap in government spending, month 3.
            let t3 = 2usize.min(c.baseline.len() - 1).min(c.shocked.len() - 1);
            let dy3 = c.shocked[t3].gdp_real_bn_ar - c.baseline[t3].gdp_real_bn_ar;
            let dg3 = c.shocked[t3].obs.government_real_bn_ar - c.baseline[t3].obs.government_real_bn_ar;
            println!("impact multiplier m3: {:.2}", dy3 / dg3);
        }
        if name.starts_with("Policy shock") {
            // Dollar-to-policy ratios (spec 8G P6): same-month ratio at m3, and the maximum monthly ratio over m3-m6.
            let ratio = |t: usize| {
                let dollar = c.shocked[t].dollar_index / c.baseline[t].dollar_index.max(1e-9) * 100.0 - 100.0;
                let pol = c.shocked[t].policy_rate - c.baseline[t].policy_rate;
                if pol.abs() > 1e-9 { dollar / pol } else { f64::NAN }
            };
            let last = 5usize.min(c.baseline.len() - 1).min(c.shocked.len() - 1);
            let maxr = (2..=last).map(ratio).filter(|r| r.is_finite()).fold(f64::NEG_INFINITY, f64::max);
            println!("dollar/policy ratio same-month m3: {:.2}", ratio(2));
            println!("dollar/policy ratio max monthly m3-m6: {:.2}", maxr);
        }
    }
}

/// Stage 3 (spec 3, measurement (a)): the isolated oil market checks K1-K8 with the Rust market, the research bands from the golden
/// file. `oil-check [SAMPLES]` (default 1000, K4 and K5 medians; K2 and K3 are single paths). Config from ECONSIM_CFG (the
/// profile and levers). Prints one table per check and the verdicts for the W3 rows (F rows W3-01..W3-06, W3-12, W3-13, W3-16).
fn oil_check(args: &[String]) {
    use econsim_core::world::oil_check as oc;
    let n: usize = arg(args, 2, 1000);
    let cfg = config_from_env();
    let p = oc::aggregate_params(&cfg);
    let g = oc::golden();
    println!("oil-check: profile {} regime {} samples {} (activity sd {} rho {})", cfg.world_oil_profile, cfg.world_opec_regime, n,
        cfg.world_oilcheck_activity_sigma, cfg.world_oilcheck_activity_rho);
    println!("levers: rho_q {} sig_I x{} (profile sig_I {})", p.rho_q, cfg.world_oil_sig_i_mult, p.sig_i);
    let (worst, drift, gap) = oc::identity_k1(&p, &g);
    println!("K1 identity: clearing residual {:.2e} (<= 1e-9), zero-shock drift {:.2e}, 2024 balance gap {:.3} mb/d (<= 0.1)", worst, drift, gap);
    let (s_i, s_y, s_n) = oc::irf_scales(&p, &g);
    println!("K2 scales: oil-specific {:.3} % of Q (1 sd), aggregate demand {:.3}, news {:.3} (normalised to +10 % on impact)", s_i, s_y, s_n);
    let rows = oc::irf_checks(&p, &g);
    let mut checks: Vec<&str> = Vec::new();
    for r in &rows {
        if !checks.contains(&r.check.as_str()) {
            checks.push(r.check.as_str());
        }
    }
    for c in &checks {
        let rs: Vec<&oc::BandRow> = rows.iter().filter(|r| r.check == *c).collect();
        let inside = rs.iter().filter(|r| r.inside).count();
        let line: Vec<String> = rs.iter().map(|r| format!("h{}={:.2}[{:.2},{:.2}]{}", r.h, r.model, r.lo, r.hi, if r.inside { "" } else { "*" })).collect();
        println!("  {:<28} {}/{} inside: {}", c, inside, rs.len(), line.join(" "));
    }
    let (errs, rmse) = oc::replay_k6(&p, &g);
    println!("K6 replay (Mar-Aug 2026): RMSE {:.2} log points (band <= 12), errors {:?}", rmse, errs.iter().map(|e| (e * 10.0).round() / 10.0).collect::<Vec<f64>>());
    let (within, at12, lr) = oc::elasticities(&p);
    println!("K8 demand elasticity: within month {:.3}, 12 months {:.3}, long run {:.3}", within, at12, lr);
    let m4 = oc::median_moments(&p, &cfg, n, 468, 120);
    println!("K4a moments (median of {} samples x 468 months): sd {:.2} skew {:.2} exkurt {:.2} ac1 {:.3} vr12 {:.3} big12 {:.3} sd level 20y {:.3} AR1 level {:.3}",
        n, m4.sd, m4.skew, m4.exkurt, m4.ac1, m4.vr12, m4.share_big12, m4.sd_level_20y, m4.ar1_level);
    let mut shares = Vec::new();
    for sh in ["I", "y", "D", "O"] {
        let v12 = oc::variance_of_changes(&p, &cfg, n.min(300), 588, 120, 12, oc::Mask::only(sh));
        let v24 = oc::variance_of_changes(&p, &cfg, n.min(300), 588, 120, 24, oc::Mask::only(sh));
        shares.push((sh, v12, v24));
    }
    let tot12: f64 = shares.iter().map(|x| x.1).sum();
    let tot24: f64 = shares.iter().map(|x| x.2).sum();
    let sup24 = shares.iter().filter(|x| x.0 == "D" || x.0 == "O").map(|x| x.2).sum::<f64>() / tot24;
    println!("K5 variance shares of 24-month changes: oil-specific {:.3}, activity {:.3}, supply {:.3} (12 months: {:.3}, {:.3}, {:.3})",
        shares[0].2 / tot24, shares[1].2 / tot24, sup24, shares[0].1 / tot12, shares[1].1 / tot12,
        shares[2].1 / tot12 + shares[3].1 / tot12);
    let scen = &g["scenarios"];
    println!("K7 scenarios (golden): Hormuz peak {:.1} (band 35-45), 2008 trough {:.1} (band -50..-40), OPEC+ cut m6 {:.2} (band 4.8-6.8)",
        scen["hormuz"]["pbar"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).fold(f64::NEG_INFINITY, f64::max),
        scen["collapse_2008_type"]["pbar"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).fold(f64::INFINITY, f64::min),
        scen["opec_cut_2mbd"]["6"]["price"].as_f64().unwrap());
}
