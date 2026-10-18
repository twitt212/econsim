//! Foreclosure response to house-price busts (fix-housing): `hbust [households=20000] [months=36] [seeds=16]`. Shock
//! minus baseline, averaged over common-random-number seeds, for the policy pinned at 7% for 24 months and the 10y term
//! premium +300bp (half-life 60m). Prints the mean annualised foreclosures (thousands) over months 13-24 and 25-36, the
//! house price and unemployment change at month 24, and the baseline foreclosure level for comparison.

use econsim_core::{sim, Config, Metrics, Scenario, ScheduledShock, Shock};

fn mean(v: &[Metrics], f: impl Fn(&Metrics) -> f64, lo: usize, hi: usize) -> f64 {
    let s: Vec<f64> = v[lo..hi.min(v.len())].iter().map(&f).collect();
    s.iter().sum::<f64>() / s.len().max(1) as f64
}

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let seeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(16);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("pinned 7% 24m", at(Shock::PolicyRate { rate: 0.07, months: 24 })),
        ("TP +300bp hl60", at(Shock::TermPremium { tenor_idx: 3, bps: 300.0, half_life_months: 60.0 })),
    ];
    for (name, shocks) in scenarios {
        let sc = Scenario { config: cfg.clone(), shocks, months, seeds: (1..=seeds).collect() };
        let c = sim::compare(&sc);
        let (b, s) = (&c.baseline, &c.shocked);
        let fc = |v: &[Metrics], lo, hi| mean(v, |m| m.foreclosures_thousands_ar, lo, hi);
        let hpi = (mean(s, |m| m.median_house_price, 23, 24) / mean(b, |m| m.median_house_price, 23, 24) - 1.0) * 100.0;
        println!(
            "{name:<16} base fc k/yr m13-24 {:>7.1}  m25-36 {:>7.1} | shock fc k/yr m13-24 {:>7.1}  m25-36 {:>7.1} | ratio {:>4.2} / {:>4.2} | house px m24 {:+.1}% | unemp m24 {:+.2}pp",
            fc(b, 12, 24),
            fc(b, 24, 36),
            fc(s, 12, 24),
            fc(s, 24, 36),
            fc(s, 12, 24) / fc(b, 12, 24).max(1e-9),
            fc(s, 24, 36) / fc(b, 24, 36).max(1e-9),
            hpi,
            mean(s, |m| m.unemployment_rate, 23, 24) - mean(b, |m| m.unemployment_rate, 23, 24),
        );
    }
}
