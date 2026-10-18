//! Monthly flow-of-funds: change in each sector's money holdings. `flows [households] [months]`.

use econsim_core::{sim, Config};

fn snap(w: &econsim_core::World) -> [f64; 10] {
    [
        w.households.iter().map(|h| h.deposits).sum(),
        w.firms.iter().map(|f| f.deposits).sum(),
        w.fund.deposits,
        w.row.deposits,
        w.cb.tga,
        w.banks.iter().map(|b| b.equity()).sum(),
        w.banks.iter().map(|b| b.loans).sum(),
        w.cb.bonds_value,
        w.banks.iter().map(|b| b.reserves).sum(),
        w.fund.total_shares * w.fund.share_price,
    ]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    println!("{:>4} {:>6} {:>6} | {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} {:>10} | {:>9} {:>9} {:>9}", "t", "u%", "π%", "Δhh", "Δfirm", "Δfund", "Δrow", "Δtga", "Δbank_eq", "Δloans", "Δcb_bonds", "Δreserves", "Δhh_fund$", "gdp", "deficit", "netexp");
    for _ in 0..months {
        let s0 = snap(&w);
        sim::step(&mut w);
        let s1 = snap(&w);
        let m = w.history.last().unwrap();
        let gdp = econsim_core::common::gdp_month_model(&w);
        print!("{:>4} {:>6.1} {:>6.1} |", m.tick, m.unemployment_rate, m.inflation_yoy);
        for i in 0..10 {
            print!(" {:>10.3e}", s1[i] - s0[i]);
        }
        println!(" | {:>9.3e} {:>9.3e} {:>9.3e}", gdp, w.gov.outlays - w.gov.revenue, w.flows.exports - w.flows.imports);
    }
}
