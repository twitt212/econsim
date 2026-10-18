//! Wage-offer growth vs average-wage growth vs unit labour cost: `wagegap [households] [years]`.
use econsim_core::types::*;
use econsim_core::{sim, Config};
fn snap(w: &World) -> (f64, f64, f64, f64, f64) {
    let alive: Vec<&Firm> = w.firms.iter().filter(|f| f.alive).collect();
    let emp: f64 = alive.iter().map(|f| f.workers.len() as f64).sum();
    // employment-weighted offer per efficiency unit
    let offer = alive.iter().map(|f| f.wage_offer * f.workers.len() as f64).sum::<f64>() / emp.max(1.0);
    let wages: Vec<f64> = w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed).map(|h| h.wage).collect();
    let avg = wages.iter().sum::<f64>() / wages.len().max(1) as f64;
    // stayers: same households employed at both dates are handled outside
    let lp = alive.iter().map(|f| f.labor_productivity * f.workers.len() as f64).sum::<f64>() / emp.max(1.0);
    let cpi = w.prices.cpi;
    (offer, avg, lp, cpi, emp)
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let years: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    let mut prev = snap(&w);
    let mut prev_wage: Vec<(bool, f64)> = w.households.iter().map(|h| (h.alive && h.status == LaborStatus::Employed, h.wage)).collect();
    println!("{:>3} {:>8} {:>8} {:>8} {:>8} {:>8}", "yr", "offer%", "avgw%", "stayer%", "lp%", "cpi%");
    for y in 1..=years {
        for _ in 0..12 { sim::step(&mut w); }
        let cur = snap(&w);
        let mut st = (0.0, 0.0);
        for (i, h) in w.households.iter().enumerate() {
            if let Some(&(was, w0)) = prev_wage.get(i) {
                if was && h.alive && h.status == LaborStatus::Employed && w0 > 0.0 { st.0 += h.wage; st.1 += w0; }
            }
        }
        let g = |a: f64, b: f64| 100.0 * (a / b - 1.0);
        let us: f64 = w.monetary.u_star;
        let u = w.unemployment_rate();
        let pie = w.monetary.pi_e;
        let va: f64 = w.firms.iter().filter(|f| f.alive).map(|f| f.revenue - f.input_cost).sum();
        let wb: f64 = w.firms.iter().filter(|f| f.alive).map(|f| f.wage_bill).sum();
        let hc: f64 = w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed).map(|h| econsim_core::common::human_capital(&w.cfg, h)).sum::<f64>() / w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Employed).count().max(1) as f64;
        eprintln!("   u {:.2}% u* {:.2}% pi_e {:.2}% labour share of VA {:.3} mean hc {:.3}", 100.0 * u, 100.0 * us, 100.0 * pie, wb / va.max(1.0), hc);
        println!("{:>3} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>8.2}", y, g(cur.0, prev.0), g(cur.1, prev.1), g(st.0, st.1), g(cur.2, prev.2), g(cur.3, prev.3));
        prev = cur;
        prev_wage = w.households.iter().map(|h| (h.alive && h.status == LaborStatus::Employed, h.wage)).collect();
    }
}
