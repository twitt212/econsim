//! Sector-level diagnostics: `debug [households] [months] [every]`.

use econsim_core::types::*;
use econsim_core::{sim, Config};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(5000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let every: u32 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
    let burn: u32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut w = sim::new_world(&Config { n_households: n, burn_in_months: burn, ..Default::default() }, 1);
    {
        let cp = &w.consumption.capital_income_prev;
        println!("BEFORE t0: cap_prev mean {:.0} n={} hh={}", cp.iter().sum::<f64>() / cp.len().max(1) as f64, cp.len(), w.households.len());
    }
    for _ in 0..months {
        sim::step(&mut w);
        let t = w.tick - 1;
        if t % every != 0 && t > 3 {
            continue;
        }
        let m = w.history.last().unwrap();
        println!(
            "\n== t={t} u={:.1}% π={:.1}% i={:.2}% wage={:.0} hh_budget={:.3e} unspent={:.3e} C={:?} I={:.3e} G={:.3e} X={:.3e} M={:.3e}",
            m.unemployment_rate, m.inflation_yoy, m.policy_rate, m.avg_wage_annual,
            w.households.iter().map(|h| h.consumption_budget).sum::<f64>(),
            w.fills.household_unspent.iter().sum::<f64>(),
            w.flows.consumption.map(|x| (x / 1e6).round()), w.flows.investment, w.flows.government, w.flows.exports, w.flows.imports
        );
        {
            let alive = || w.households.iter().zip(w.hh_month.iter()).filter(|(h, _)| h.alive);
            let n = alive().count() as f64;
            let inc = |g: &dyn Fn(&IncomeMonth) -> f64| alive().map(|(_, m)| g(&m.income)).sum::<f64>() / n;
            let tax = alive().map(|(_, m)| m.taxes).sum::<f64>() / n;
            let hc = alive().map(|(h, _)| h.housing_cost).sum::<f64>() / n;
            let cons = alive().map(|(_, m)| m.consumption.iter().sum::<f64>()).sum::<f64>() / n;
            let budget = alive().map(|(h, _)| h.consumption_budget).sum::<f64>() / n;
            let dep = alive().map(|(h, _)| h.deposits).sum::<f64>() / n;
            let yp = alive().map(|(h, _)| h.permanent_income).sum::<f64>() / n;
            println!(
                "per-hh: wage {:.0} ben {:.0} pens {:.0} transf {:.0} rent {:.0} int {:.0} div {:.0} | tax {:.0} housing {:.0} budget {:.0} cons {:.0} yp {:.0} deposits {:.0} fundval {:.0}",
                inc(&|i| i.wage), inc(&|i| i.unemployment_benefit), inc(&|i| i.pension), inc(&|i| i.transfers),
                inc(&|i| i.rent_received), inc(&|i| i.interest), inc(&|i| i.dividends), tax, hc, budget, cons, yp, dep,
                alive().map(|(h, _)| h.shares).sum::<f64>() / n * w.fund.share_price
            );
            let hh_dep: f64 = w.households.iter().filter(|h| h.alive).map(|h| h.deposits).sum();
            let firm_dep: f64 = w.firms.iter().map(|f| f.deposits).sum();
            let bank_eq: f64 = w.banks.iter().map(|b| b.equity()).sum();
            let bank_res: f64 = w.banks.iter().map(|b| b.reserves).sum();
            let loans: f64 = w.banks.iter().map(|b| b.loans).sum();
            let bonds_banks: f64 = w.banks.iter().map(|b| b.bonds_value).sum();
            println!("balances: hh_dep {:.3e} firm_dep {:.3e} fund_dep {:.3e} row_dep {:.3e} tga {:.3e} bank_eq {:.3e} reserves {:.3e} loans {:.3e} bank_bonds {:.3e} cb_bonds {:.3e} fund_bonds {:.3e} row_bonds {:.3e} | fund_div_pending {:.3e} bank_profit {:.3e} bank_div {:.3e}",
                hh_dep, firm_dep, w.fund.deposits, w.row.deposits, w.cb.tga, bank_eq, bank_res, loans, bonds_banks, w.cb.bonds_value, w.fund.bonds_value, w.row.bonds_value,
                w.fund.dividends_received, w.banks.iter().map(|b| b.profit).sum::<f64>(), w.banks.iter().map(|b| b.dividends).sum::<f64>());
            println!("housing: (listings, bids, med ask, med bid, max bid, med ask/q, movers) = {:?}; hpi {:.0}; seeking {}",
                econsim_core::housing::market_diagnostics(&w), w.prices.hpi, w.housing.seeking.iter().filter(|&&x| x).count());
            println!("unspent by sector (E,F,G,S): {:?}", w.goods.unspent_by_sector.map(|x| (x / 1e3).round()));
            let cp = &w.consumption.capital_income_prev;
            println!("cap_prev mean {:.0} (n={}), expected_infl mean {:.4}, real-rate inputs: dep {:.4} y2 {:.4}",
                cp.iter().sum::<f64>() / cp.len().max(1) as f64, cp.len(),
                w.households.iter().map(|h| h.expected_inflation).sum::<f64>() / w.households.len() as f64,
                w.banks[0].deposit_rate, w.curve.yield_at(24.0));
            {
                use econsim_core::types::LaborStatus;
                let grp = |f: &dyn Fn(&Household) -> bool| {
                    let v: Vec<&Household> = w.households.iter().filter(|h| h.alive && f(h)).collect();
                    let n = v.len().max(1) as f64;
                    (v.len(), v.iter().map(|h| h.consumption_budget).sum::<f64>() / n, v.iter().map(|h| h.housing_cost).sum::<f64>() / n,
                     v.iter().map(|h| h.permanent_income).sum::<f64>() / n, v.iter().map(|h| h.deposits).sum::<f64>() / n)
                };
                println!("retired {:?}", grp(&|h| h.status == LaborStatus::Retired));
                println!("employed {:?}", grp(&|h| h.status == LaborStatus::Employed));
                println!("other {:?}", grp(&|h| h.status != LaborStatus::Employed && h.status != LaborStatus::Retired));
            }
            println!("tenure: {}", econsim_core::housing::tenure_diagnostics(&w));
            println!("gov: revenue {:.3e} outlays {:.3e} purchases {:.3e} transfers {:.3e} interest {:.3e}; fund nav {:.3e} eq {:.3e}",
                w.gov.revenue, w.gov.outlays, w.gov.purchases, w.gov.transfers, w.gov.interest_paid, w.fund.nav, w.fund.equity_value);
        }
        println!("{:<13}{:>6}{:>7}{:>9}{:>9}{:>9}{:>9}{:>9}{:>9}{:>7}{:>7}{:>10}{:>9}", "sector", "firms", "wkrs", "vac", "exp_dem", "demand", "output", "sales", "inv", "price", "mkup", "profit", "deposits");
        println!("{:<13}{:>8}{:>8}{:>8}{:>8}{:>8}", "sector", "wmkup", "util", "u_p10", "u_p90", "top5sh");
        for s in Sector::ALL {
            let fs: Vec<&Firm> = w.firms.iter().filter(|f| f.alive && f.sector == s).collect();
            let sum = |g: &dyn Fn(&Firm) -> f64| fs.iter().map(|f| g(f)).sum::<f64>();
            let nf = fs.len().max(1) as f64;
            println!(
                "{:<13}{:>6}{:>7}{:>9}{:>9.3e}{:>9.3e}{:>9.3e}{:>9.3e}{:>9.3e}{:>7.3}{:>7.3}{:>10.3e}{:>9.3e}",
                s.name(), fs.len(), sum(&|f| f.workers.len() as f64), sum(&|f| f.vacancies as f64),
                sum(&|f| f.expected_demand), sum(&|f| f.demand), sum(&|f| f.output), sum(&|f| f.sales), sum(&|f| f.inventory),
                sum(&|f| f.price) / nf, sum(&|f| f.markup) / nf, sum(&|f| f.profit), sum(&|f| f.deposits)
            );
            let out = sum(&|f| f.output).max(1e-9);
            let mut u: Vec<f64> = fs.iter().map(|f| f.output / (f.capital * f.capital_productivity).max(1e-9)).collect();
            u.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mut o: Vec<f64> = fs.iter().map(|f| f.output).collect();
            o.sort_by(|a, b| b.partial_cmp(a).unwrap());
            let q = |p: f64| if u.is_empty() { 0.0 } else { u[((u.len() - 1) as f64 * p) as usize] };
            println!("{:<13}{:>8.3}{:>8.3}{:>8.3}{:>8.3}{:>8.3}", s.name(), sum(&|f| f.markup * f.output) / out,
                sum(&|f| f.output) / sum(&|f| f.capital * f.capital_productivity).max(1e-9), q(0.1), q(0.9),
                o.iter().take(5).sum::<f64>() / out);
        }
    }
}
