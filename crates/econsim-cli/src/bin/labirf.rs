//! Labour-market impulse responses (fix-labour). `labirf [households] [months] [seeds]` prints, for each scenario,
//! the difference from baseline (averaged over seeds, same seed in both runs) of unemployment, the employed-mean
//! wage growth (`wage_growth_yoy`), the composition-fixed stayer wage growth (`labor::StayerTrack`) and core
//! inflation, at 3, 6, 12, 24 and 36 months. The gap between the first two wage rows is the composition effect.

use econsim_core::labor::StayerTrack;
use econsim_core::{sim, Config, ScheduledShock, Shock};

/// Months simulated before the shock so the stayer tracker has a full year of history.
const WARM: u32 = 12;

/// Per-month series [unemployment %, employed-mean wage growth %, stayer wage growth %, core inflation %] for the
/// `months` after the warm-up. The shock starts at the first of those months.
fn run(cfg: &Config, seed: u64, months: u32, shock: Option<Shock>) -> Vec<[f64; 4]> {
    let mut w = sim::new_world(cfg, seed);
    if let Some(s) = shock {
        let at = w.tick + WARM;
        w.shocks.schedule(ScheduledShock { at_month: at, shock: s });
    }
    let mut tr = StayerTrack::default();
    let mut out = Vec::with_capacity(months as usize);
    for t in 0..months + WARM {
        sim::step(&mut w);
        let stay = tr.observe(&w.households).unwrap_or(f64::NAN);
        if t >= WARM {
            let m = w.history.last().expect("metrics recorded");
            out.push([m.unemployment_rate, m.wage_growth_yoy, stay, m.core_inflation_yoy]);
        }
    }
    out
}

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let nseeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let scenarios: Vec<(&str, Shock)> = vec![
        ("Policy rate pinned at 7% for 24m", Shock::PolicyRate { rate: 0.07, months: 24 }),
        ("Policy shock +100bp vs rule, 12m", Shock::PolicyShift { bps: 100.0, months: 12 }),
    ];
    let names = ["unemp pp", "wage gr pp (employed mean)", "stayer wage pp", "core yoy pp"];
    let horizons = [3usize, 6, 12, 24, 36];
    for (name, shock) in scenarios {
        let mut acc = vec![[0.0f64; 4]; months as usize];
        let mut cnt = vec![[0u32; 4]; months as usize];
        for seed in 1..=nseeds {
            let base = run(&cfg, seed, months, None);
            let shocked = run(&cfg, seed, months, Some(shock.clone()));
            for t in 0..months as usize {
                for k in 0..4 {
                    let d = shocked[t][k] - base[t][k];
                    if d.is_finite() {
                        acc[t][k] += d;
                        cnt[t][k] += 1;
                    }
                }
            }
        }
        println!("\n=== {name} ({nseeds} seeds, {n} households) ===");
        print!("{:<28}", "");
        for h in horizons {
            print!("{:>9}", format!("m{h}"));
        }
        println!();
        for (k, label) in names.iter().enumerate() {
            print!("{:<28}", label);
            for h in horizons {
                let t = h - 1;
                let v = if cnt[t][k] > 0 { acc[t][k] / f64::from(cnt[t][k]) } else { f64::NAN };
                print!("{:>9.2}", v);
            }
            println!();
        }
    }
}
