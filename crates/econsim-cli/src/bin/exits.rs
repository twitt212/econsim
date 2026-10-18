//! Firm exits by age at failure and the firm's sales/expected demand: `exits [households] [years]`.
use econsim_core::{sim, Config};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let years: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    let buckets = [0u32, 3, 6, 12, 24, 60, u32::MAX];
    println!("{:>3} {:>6} {:>6} | exits by age at failure (months): <3 3-6 6-12 12-24 24-60 60+ | entrants  avg workers at death", "yr", "alive", "exits");
    for y in 1..=years {
        let mut cnt = [0usize; 6];
        let (mut exits, mut entries, mut wk) = (0usize, 0usize, 0usize);
        for _ in 0..12 {
            let before: Vec<(bool, u32, usize)> = w.firms.iter().map(|f| (f.alive, f.age_months, f.workers.len())).collect();
            sim::step(&mut w);
            for (i, f) in w.firms.iter().enumerate() {
                let Some(&(alive0, age0, wk0)) = before.get(i) else { entries += 1; continue };
                if alive0 && (!f.alive || f.age_months < age0) {
                    exits += 1;
                    wk += wk0;
                    let b = buckets.windows(2).position(|r| age0 >= r[0] && age0 < r[1]).unwrap_or(5);
                    cnt[b] += 1;
                }
                if f.alive && f.age_months == 0 { entries += 1; }
            }
        }
        let alive = w.firms.iter().filter(|f| f.alive).count();
        println!("{y:>3} {alive:>6} {exits:>6} | {:>4} {:>4} {:>4} {:>4} {:>4} {:>4} | {entries:>6} {:.1}", cnt[0], cnt[1], cnt[2], cnt[3], cnt[4], cnt[5], wk as f64 / exits.max(1) as f64);
    }
}
