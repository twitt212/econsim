//! Real-side growth diagnostic: output per worker / per efficiency unit and labour productivity over time.
use econsim_core::{common, sim, Config};
fn main() {
    let mut w = sim::new_world(&Config { n_households: 5000, burn_in_months: 0, ..Default::default() }, 1);
    for t in 0..=156u32 {
        if t % 24 == 0 {
            let alive = || w.firms.iter().filter(|f| f.alive);
            let out: f64 = alive().map(|f| f.output).sum();
            let sales: f64 = alive().map(|f| f.sales).sum();
            let workers: usize = alive().map(|f| f.workers.len()).sum();
            let eff: f64 = alive().flat_map(|f| f.workers.iter()).map(|&h| common::human_capital(&w.cfg, &w.households[h as usize])).sum();
            let lp: f64 = alive().map(|f| f.labor_productivity * f.workers.len() as f64).sum::<f64>() / workers.max(1) as f64;
            let cap: f64 = alive().map(|f| f.capital * f.capital_productivity).sum();
            let hh_cons_real: f64 = w.hh_month.iter().map(|m| m.consumption.iter().zip(w.prices.sector.iter()).map(|(c, p)| c / p.max(1e-9)).sum::<f64>()).sum();
            println!("t{t:3}: output {out:.3e} sales {sales:.3e} workers {workers} eff/worker {:.3} lp {lp:.0} out/worker {:.0} out/eff {:.0} capacity {cap:.3e} util {:.3} hh_real_cons {hh_cons_real:.3e}",
                eff / workers.max(1) as f64, out / workers.max(1) as f64, out / eff.max(1e-9), out / cap.max(1e-9));
        }
        sim::step(&mut w);
    }
}
