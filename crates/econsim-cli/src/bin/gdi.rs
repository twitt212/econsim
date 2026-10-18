//! Income-side GDP decomposition after burn-in: `gdi [households] [months]`.
use econsim_core::types::*;
use econsim_core::{sim, Config};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(12);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    let scale = w.cfg.effective_scale() * 12.0 / 1e9 / months as f64;
    let mut acc = [0.0f64; 10];
    for _ in 0..months {
        sim::step(&mut w);
        let f = |g: &dyn Fn(&Firm) -> f64| w.firms.iter().filter(|f| f.alive).map(|x| g(x)).sum::<f64>();
        acc[0] += f(&|x| x.revenue);
        acc[1] += f(&|x| x.input_cost);
        acc[2] += f(&|x| x.wage_bill);
        acc[3] += f(&|x| x.depreciation_cost);
        acc[4] += f(&|x| x.interest_paid);
        acc[5] += f(&|x| x.taxes);
        acc[6] += f(&|x| x.profit);
        acc[7] += w.history.last().unwrap().gdp_nominal_bn_ar / scale / 12.0 * months as f64 / months as f64;
    }
    let names = ["revenue", "inputs", "wage_bill(incl payroll+benefits?)", "depreciation", "interest", "corp taxes", "profit"];
    for (i, nm) in names.iter().enumerate() {
        println!("{nm:36} {:10.0} bn/yr", acc[i] * scale);
    }
    println!("{:36} {:10.0} bn/yr", "GDP (metrics)", acc[7] * scale);
    println!("firm value added (rev - inputs)      {:10.0} bn/yr", (acc[0] - acc[1]) * scale);
}
