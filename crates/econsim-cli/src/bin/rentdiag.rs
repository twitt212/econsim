//! Rent-growth components by year: `rentdiag [households] [years]`.
use econsim_core::{sim, Config};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let years: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    println!("{:>3} {:>7} {:>7} {:>7} {:>7} {:>7} {:>8} {:>8}", "yr", "pi_e", "inc_g", "vac", "natvac", "arb", "rent_g", "hpi_g");
    let (mut r0, mut h0) = (w.prices.rent_index, w.prices.hpi);
    for y in 1..=years {
        let (mut pe, mut ig, mut vac, mut arb) = (0.0, 0.0, 0.0, 0.0);
        for _ in 0..12 {
            sim::step(&mut w);
            let m = w.history.last().unwrap();
            pe += m.expected_inflation / 100.0 / 12.0;
            ig += w.housing.income_growth / 12.0;
            vac += m.vacancy_rate_housing / 100.0 / 12.0;
            let rs = w.housing.rent_yield0 * w.prices.hpi / 12.0;
            arb += 0.08 * (rs / w.prices.rent_index).ln() / 12.0;
        }
        let rg = w.prices.rent_index / r0 - 1.0;
        let hg = w.prices.hpi / h0 - 1.0;
        r0 = w.prices.rent_index; h0 = w.prices.hpi;
        println!("{y:>3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>7.3} {:>8.3} {:>8.3}", pe, ig, vac, w.housing.natural_rental_vacancy, arb, rg, hg);
    }
}
