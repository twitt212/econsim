//! Housing impulse responses for the probe (fix-housing): `hirf [households=10000] [months=36] [seeds=6]`. Shock minus
//! baseline, averaged over common-random-number seeds, for the policy +100bp (12m), the oil +50% and the 10y term
//! premium +100bp (half-life 24m) shocks. Prints shelter, core CPI, total vacancy (not HVS), rent index, house prices,
//! mortgage rate and foreclosures at months 3, 6, 12, 24, 36.

use econsim_core::{sim, Config, Metrics, ScheduledShock, Scenario, Shock};

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let seeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(6);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("Policy +100bp 12m", at(Shock::PolicyShift { bps: 100.0, months: 12 })),
        ("Oil +50%", at(Shock::OilPrice { pct: 50.0, half_life_months: 12.0 })),
        ("TP +100bp hl24", at(Shock::TermPremium { tenor_idx: 3, bps: 100.0, half_life_months: 24.0 })),
    ];
    type G = fn(&Metrics) -> f64;
    let vars: Vec<(&str, G, bool)> = vec![
        ("shelter yoy pp", |m| m.shelter_inflation_yoy, false),
        ("core yoy pp", |m| m.core_inflation_yoy, false),
        ("total vacancy pp", |m| m.vacancy_rate_housing, false),
        ("rent index %", |m| m.rent_index, true),
        ("house px %", |m| m.median_house_price, true),
        ("mortgage pp", |m| m.mortgage_rate, false),
        ("foreclosures k", |m| m.foreclosures_thousands_ar, false),
        ("homeown pp", |m| m.homeownership_rate, false),
    ];
    let hs = [3usize, 6, 12, 24, 36];
    for (name, shocks) in scenarios {
        let sc = Scenario { config: cfg.clone(), shocks, months, seeds: (1..=seeds).collect() };
        let c = sim::compare(&sc);
        println!("\n=== {name}");
        print!("{:<16}", "");
        for h in hs {
            print!("{:>9}", format!("m{h}"));
        }
        println!();
        for (vn, g, pct) in &vars {
            print!("{:<16}", vn);
            for h in hs {
                if h > c.baseline.len() {
                    print!("{:>9}", "-");
                    continue;
                }
                let (b, s) = (g(&c.baseline[h - 1]), g(&c.shocked[h - 1]));
                let d = if *pct { if b.abs() > 1e-9 { (s / b - 1.0) * 100.0 } else { 0.0 } } else { s - b };
                print!("{:>9.3}", d);
            }
            println!();
        }
    }
}
