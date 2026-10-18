//! Per-phase timing: `profile [households] [months]`.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
use econsim_core::{sim, Config};
fn main() {
    econsim_core::sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(24);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    let mut t = [0.0; 21];
    for _ in 0..months {
        sim::step_timed(&mut w, &mut t);
    }
    let total: f64 = t.iter().sum();
    let mut idx: Vec<usize> = (0..20).collect();
    idx.sort_by(|&a, &b| t[b].partial_cmp(&t[a]).unwrap());
    for i in idx {
        println!("{:<15} {:>8.2} ms/month {:>5.1}%", sim::PHASES[i], 1e3 * t[i] / months as f64, 100.0 * t[i] / total);
    }
    println!("total {:.2} ms/month ({:.1} months/s)", 1e3 * total / months as f64, months as f64 / total);
}
