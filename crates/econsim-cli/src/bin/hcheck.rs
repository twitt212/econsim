//! Invariant probe (fix-housing): `hcheck [seeds=16] [households=20000] [months=240]`. For each scenario (baseline,
//! 10y term premium +100bp half-life 24m, policy pinned at 7% for 24m, policy +100bp for 12m) and each seed, runs the
//! world month by month and (rz21: a fifth scenario, a permanent 25% tariff; rz27: a sixth and a seventh, oil +50% for good and -50% fading) calls `ledger::check_consistency` and `housing::check_invariants` after every month.
//! Prints failure counts per scenario and the first failure seen. Exits non-zero if any check fails.

use econsim_core::housing::check_invariants;
use econsim_core::ledger::check_consistency;
use econsim_core::{sim, Config, ScheduledShock, Shock};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

struct Outcome {
    ledger_fail: usize,
    housing_fail: usize,
    first: Option<String>,
}

fn run_one(cfg: &Config, shocks: &[ScheduledShock], seed: u64, months: u32) -> Outcome {
    let mut w = sim::new_world(cfg, seed);
    let start = w.tick;
    for s in shocks {
        let mut s = s.clone();
        s.at_month += start;
        w.shocks.schedule(s);
    }
    let mut out = Outcome { ledger_fail: 0, housing_fail: 0, first: None };
    for _ in 0..months {
        sim::step(&mut w);
        if let Err(e) = check_consistency(&w) {
            out.ledger_fail += 1;
            out.first.get_or_insert(format!("seed {seed} t{}: ledger: {e}", w.tick));
        }
        if let Err(e) = check_invariants(&w) {
            out.housing_fail += 1;
            out.first.get_or_insert(format!("seed {seed} t{}: housing: {e}", w.tick));
        }
    }
    out
}

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let seeds: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(16);
    let hh: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let months: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(240);
    let cfg = Config { n_households: hh, ..Config::from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("baseline", vec![]),
        ("TP+100 hl24", at(Shock::TermPremium { tenor_idx: 3, bps: 100.0, half_life_months: 24.0 })),
        ("pinned 7% 24m", at(Shock::PolicyRate { rate: 0.07, months: 24 })),
        ("policy +100bp 12m", at(Shock::PolicyShift { bps: 100.0, months: 12 })),
        // rz21: a permanent 25% tariff, which enters the retail wedge, the import shares, the efficiency wedge and the rule's look-through when those keys are on
        ("tariff +25%", at(Shock::ImportTariff { pct: 25.0 })),
        // rz27: a permanent +50% and a fading -50% oil price, which move Energy's equity value through the oil-elastic valuation when `energy_equity_oil_on` is set
        ("oil +50%", at(Shock::OilPrice { pct: 50.0, half_life_months: 0.0 })),
        ("oil -50% hl12", at(Shock::OilPrice { pct: -50.0, half_life_months: 12.0 })),
    ];
    let mut total_fail = 0usize;
    for (name, shocks) in &scenarios {
        let next = AtomicUsize::new(1);
        let results: Mutex<Vec<Outcome>> = Mutex::new(Vec::new());
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(seeds as usize).max(1);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let seed = next.fetch_add(1, Ordering::SeqCst) as u64;
                    if seed > seeds {
                        break;
                    }
                    let o = run_one(&cfg, shocks, seed, months);
                    results.lock().unwrap().push(o);
                });
            }
        });
        let res = results.into_inner().unwrap();
        let lf: usize = res.iter().map(|o| o.ledger_fail).sum();
        let hf: usize = res.iter().map(|o| o.housing_fail).sum();
        let runs_failed = res.iter().filter(|o| o.first.is_some()).count();
        total_fail += lf + hf;
        println!("{name:<20} runs {:>3}  months {:>6}  ledger fails {lf}  housing fails {hf}  runs with a failure {runs_failed}", res.len(), res.len() as u64 * months as u64);
        if let Some(f) = res.iter().filter_map(|o| o.first.clone()).min() {
            println!("  first: {f}");
        }
    }
    println!("TOTAL FAILURES {total_fail}");
    if total_fail > 0 {
        std::process::exit(1);
    }
}
