//! Share-price impulse responses (R51-E): `spirf [households=5000] [months=36] [seeds=4] [warmup=24]`. Shock minus baseline, averaged
//! over common-random-number seeds, for a `SharePrices` re-rating of Tech by -80% (permanent, and with a 24-month half-life) and of all
//! sectors by -30% (permanent). Prints the equity, Fund share price, risk premium, consumption, investment, GDP and unemployment gaps at
//! months 1, 3, 6, 12, 24 and 36, then the first-year consumption response per dollar of equity wealth lost (the marginal propensity to
//! consume out of stock-market wealth, a year's consumption change over the wealth change on impact). Report only: nothing is tuned.
//! `ECONSIM_CFG` overlays apply as for the other binaries.

use econsim_core::{sim, Config, Metrics, ScheduledShock, Scenario, Shock};

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let seeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(4);
    let warm: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(24);
    let cfg = Config { n_households: n, warmup_months: warm, ..Config::from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("Tech shares -80%, permanent", at(Shock::SharePrices { sector: Some(4), pct: -80.0, half_life_months: 0.0 })),
        ("Tech shares -80%, half-life 24m", at(Shock::SharePrices { sector: Some(4), pct: -80.0, half_life_months: 24.0 })),
        ("All shares -30%, permanent", at(Shock::SharePrices { sector: None, pct: -30.0, half_life_months: 0.0 })),
        ("All shares -30%, half-life 24m", at(Shock::SharePrices { sector: None, pct: -30.0, half_life_months: 24.0 })),
        ("All shares -50%, permanent", at(Shock::SharePrices { sector: None, pct: -50.0, half_life_months: 0.0 })),
    ];
    type G = fn(&Metrics) -> f64;
    let vars: Vec<(&str, G, bool)> = vec![
        ("equity value %", |m| m.equity_value_bn, true),
        ("tech equity %", |m| m.equity_value_by_sector[4], true),
        ("fund share px %", |m| m.fund_share_price, true),
        ("equity prem pp", |m| m.equity_risk_premium, false),
        ("12m eq return pp", |m| m.equity_return_12m, false),
        ("tech q (rel) ", |m| m.obs.sector_q[4], false),
        ("real cons %", |m| m.consumption_real_bn_ar, true),
        ("real inv %", |m| m.investment_real_bn_ar, true),
        ("tech inv %", |m| m.tech_investment_bn_ar, true),
        ("real GDP %", |m| m.gdp_real_bn_ar, true),
        ("unemp pp", |m| m.unemployment_rate, false),
        ("deposits %", |m| m.deposits_bn, true),
        ("10y pp", |m| m.yields[3], false),
    ];
    let hs = [1usize, 3, 6, 12, 24, 36];
    for (name, shocks) in scenarios {
        let sc = Scenario { config: cfg.clone(), shocks, months, seeds: (1..=seeds).collect() };
        let c = sim::compare(&sc);
        println!("\n=== {name}  ({n} households, {seeds} seeds, warm-up {warm})");
        print!("{:<18}", "");
        for h in hs {
            print!("{:>9}", format!("m{h}"));
        }
        println!();
        for (vn, g, pct) in &vars {
            print!("{:<18}", vn);
            for h in hs {
                if h > c.baseline.len() {
                    print!("{:>9}", "-");
                    continue;
                }
                let (b, s) = (g(&c.baseline[h - 1]), g(&c.shocked[h - 1]));
                let d = if *pct { if b.abs() > 1e-9 { (s / b - 1.0) * 100.0 } else { 0.0 } } else { s - b };
                print!("{:>9.3}", d);
            }
            println!();
        }
        // First-year consumption response per dollar of equity wealth lost on impact (month 1 = the impact month).
        let k = 12.min(c.baseline.len());
        let d_cons: f64 = (0..k).map(|t| c.shocked[t].consumption_real_bn_ar - c.baseline[t].consumption_real_bn_ar).sum::<f64>() / 12.0;
        let base_cons: f64 = (0..k).map(|t| c.baseline[t].consumption_real_bn_ar).sum::<f64>() / 12.0;
        let d_eq = c.shocked[0].equity_value_bn - c.baseline[0].equity_value_bn;
        let d_inv: f64 = (0..k).map(|t| c.shocked[t].investment_real_bn_ar - c.baseline[t].investment_real_bn_ar).sum::<f64>() / 12.0;
        let base_inv: f64 = (0..k).map(|t| c.baseline[t].investment_real_bn_ar).sum::<f64>() / 12.0;
        // The annual-rate gap in months 7-12 (the build-up has happened by then), per dollar of the wealth lost on impact.
        let late: f64 = (6..k).map(|t| c.shocked[t].consumption_real_bn_ar - c.baseline[t].consumption_real_bn_ar).sum::<f64>() / (k.saturating_sub(6)).max(1) as f64;
        println!(
            "first-year: equity wealth change on impact {:.0} $bn; consumption change {:.1} $bn ({:.3}% of baseline); investment change {:.1} $bn ({:.3}%); cents of consumption per $ of equity wealth {:.2} (first-year sum), {:.2} (annual rate, months 7-12)",
            d_eq,
            d_cons,
            100.0 * d_cons / base_cons,
            d_inv,
            100.0 * d_inv / base_inv,
            if d_eq.abs() > 1e-9 { 100.0 * d_cons / d_eq } else { f64::NAN },
            if d_eq.abs() > 1e-9 { 100.0 * late / d_eq } else { f64::NAN }
        );
    }
}
