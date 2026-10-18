//! Age structure, partnership and labour status by age band over time: `demog [households] [years] [seed]`.
//! Also prints the head-age summary (under-25 share, mean age) and the labour-force statistics per year.
use econsim_core::types::*;
use econsim_core::{sim, Config};

/// Prints the table and returns the alive household count.
fn table(w: &World, label: &str) -> usize {
    let bands = [18u32, 25, 35, 45, 55, 62, 65, 70, 75, 85, 120];
    let alive: Vec<&Household> = w.households.iter().filter(|h| h.alive).collect();
    let n = alive.len() as f64;
    let mean_age = alive.iter().map(|h| h.age_years()).sum::<f64>() / n;
    let partnered = 100.0 * alive.iter().filter(|h| h.partnered).count() as f64 / n;
    println!("{label}: n {} mean age {:.1} partnered {:.1}%", alive.len(), mean_age, partnered);
    println!("  band     share  partnered  retired  employed  unemp  olf  student");
    for b in bands.windows(2) {
        let g: Vec<&&Household> = alive.iter().filter(|h| { let a = h.age_years(); a >= b[0] as f64 && a < b[1] as f64 }).collect();
        let k = g.len().max(1) as f64;
        let c = |s: LaborStatus| 100.0 * g.iter().filter(|h| h.status == s).count() as f64 / k;
        let p = 100.0 * g.iter().filter(|h| h.partnered).count() as f64 / k;
        println!("  {:>3}-{:<3} {:6.1} {:10.1} {:7.1} {:9.1} {:6.1} {:4.1} {:7.1}", b[0], b[1], 100.0 * g.len() as f64 / n, p,
            c(LaborStatus::Retired), c(LaborStatus::Employed), c(LaborStatus::Unemployed), c(LaborStatus::OutOfLaborForce), c(LaborStatus::Student));
    }
    alive.len()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let years: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let seed: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, seed);
    let n0 = table(&w, "year 0");
    for y in 1..=years {
        for _ in 0..12 { sim::step(&mut w); }
        if y % 10 == 0 {
            let ny = table(&w, &format!("year {y}"));
            if y == 10 {
                let growth = 100.0 * ((ny as f64 / n0 as f64).powf(0.1) - 1.0);
                println!("alive household growth over the first decade: {growth:.2}%/yr");
            }
        }
    }
}
