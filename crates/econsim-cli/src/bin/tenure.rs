//! Tenure-transition accounting: `tenure [households] [months] [every]`. Counts monthly flows into and out of
//! owner-occupation by age band, to diagnose drift in the homeownership rate.

use econsim_core::types::*;
use econsim_core::{sim, Config};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(60);
    let every: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(12);
    let mut w = sim::new_world(&Config { n_households: n, burn_in_months: 0, ..Config::from_env() }, 1);
    let snap = |w: &World| w.households.iter().map(|h| (h.alive, h.tenure, h.age_months)).collect::<Vec<_>>();
    // flows: [became owner, owner->renter, owner->parents, owner died/recycled, entrant]
    let mut acc = [0usize; 6];
    let mut prev = snap(&w);
    let mut left: Vec<(usize, u32)> = Vec::new();
    let mut rebought = 0usize;
    println!("{:>4} {:>7} {:>8} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}", "t", "own%", "->own", "own->rent", "own->par", "own_died", "o->rent65+", "own_n", "recent_sel", "rebuy<=12m");
    for _ in 0..months {
        sim::step(&mut w);
        let cur = snap(&w);
        for (i, (p, c)) in prev.iter().zip(cur.iter()).enumerate() {
            let recycled = c.2 < p.2 || (p.0 && !c.0);
            if p.1 == Tenure::Owner && p.0 {
                if recycled {
                    acc[3] += 1;
                } else if c.1 == Tenure::Renter {
                    acc[1] += 1;
                    if w.housing.recent_seller.get(i).copied().unwrap_or(0) > 0 { acc[4] += 1; }
                    left.push((i, w.tick));
                    if w.households[i].age_years() >= 65.0 {
                        acc[5] += 1;
                    }
                } else if c.1 == Tenure::WithParents {
                    acc[2] += 1;
                }
            } else if c.1 == Tenure::Owner && c.0 && !recycled {
                acc[0] += 1;
                if let Some(pos) = left.iter().position(|&(j, _)| j == i) { let (_, t0) = left.swap_remove(pos); if w.tick - t0 <= 12 { rebought += 1; } }
            }
        }
        prev = cur;
        if w.tick % every == 0 {
            let alive = w.households.iter().filter(|h| h.alive).count();
            let own = w.households.iter().filter(|h| h.alive && h.tenure == Tenure::Owner).count();
            println!("{:>4} {:>7.1} {:>8} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9} {:>9}", w.tick, 100.0 * own as f64 / alive as f64, acc[0], acc[1], acc[2], acc[3], acc[5], own, acc[4], rebought);
            acc = [0; 6];
            rebought = 0;
        }
    }
}
