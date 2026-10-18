//! Household diagnostics (fix-households branch).
//!
//! `hhdiag mpc <households> <seeds> <months>`: $1,000 one-off transfer to every other household (even ids), with the
//! untreated halves of the same runs as the counterfactual. Prints the cumulative MPC by liquidity group and by
//! impatience type, and the durables share of the extra spending.
//! `hhdiag dist <households> <seeds> <months>`: post-burn-in liquidity distribution (share of households with
//! liquid wealth below half a month's income) and net worth by age band, medians and means.

use econsim_core::types::*;
use econsim_core::{banking, common, consumption, demography, exog, firms, fiscal, goods, housing, labor, ledger, metrics, monetary, sim, Config, World};
use econsim_core::ledger::Account;
use econsim_core::shocks::{Shock, ScheduledShock};

const AMOUNT: f64 = 1_000.0;
const HORIZONS: [usize; 4] = [1, 3, 6, 12];
/// Liquidity groups by deposits ÷ monthly income.
const GROUPS: [&str; 6] = ["HtM <0.5m", "0.5-2m", "2-6m", "6-24m", ">24m", "retired"];

/// One month of `sim::step` with an optional per-household one-off payment credited after the government's
/// transfers (the same position as helicopter money), so it is spent under the household's own rules.
fn step_inject(w: &mut World, inject: Option<&[f64]>, windfall: bool) {
    sim::begin_month(w);
    exog::step(w);
    monetary::set_policy(w);
    monetary::update_curve(w);
    monetary::exchange_rate(w);
    banking::set_rates(w);
    demography::step(w);
    firms::plan(w);
    labor::step(w);
    firms::produce(w);
    fiscal::pay_transfers(w);
    if let Some(r) = inject {
        for i in 0..w.households.len().min(r.len()) {
            if r[i] > 0.0 && w.households[i].alive {
                ledger::pay(w, Account::CentralBank, Account::Household(i as HhId), r[i]);
                if windfall {
                    w.hh_month[i].income.windfall += r[i];
                } else {
                    w.hh_month[i].income.transfers += r[i];
                }
            }
        }
    }
    housing::step(w);
    consumption::decide(w);
    fiscal::plan_spending(w);
    goods::clear(w);
    banking::service(w);
    fiscal::collect_and_finance(w);
    monetary::markets(w);
    firms::close_books(w);
    consumption::record_capital_income(w);
    metrics::record(w);
    w.tick += 1;
}

fn spent(m: &HhMonth) -> f64 {
    m.consumption.iter().sum()
}

fn group_of(liq: f64, income: f64, retired: bool) -> usize {
    if retired {
        return 5;
    }
    let r = liq / income.max(1.0);
    if r < 0.5 {
        0
    } else if r < 2.0 {
        1
    } else if r < 6.0 {
        2
    } else if r < 24.0 {
        3
    } else {
        4
    }
}

#[derive(Default, Clone)]
struct Acc {
    n: f64,
    cum: [f64; 4],
    dur: f64,
    tot: f64,
}

fn mpc_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(12);
    // `transfer` (5th argument): credit the payment as regular transfer income (transitory income), not a windfall
    let windfall = args.get(5).map_or(true, |s| s != "transfer");
    let cfg = Config { n_households: n, ..Config::from_env() };
    // (treated, untreated) accumulators per group, and overall
    let mut groups: Vec<(Acc, Acc)> = vec![(Acc::default(), Acc::default()); GROUPS.len()];
    let mut all: (Acc, Acc) = (Acc::default(), Acc::default());
    for seed in 1..=seeds {
        let w0 = sim::new_world(&cfg, seed);
        let mut base = w0.clone();
        let mut tr = w0.clone();
        let alive0: Vec<bool> = w0.households.iter().map(|h| h.alive).collect();
        let treated: Vec<bool> = w0.households.iter().map(|h| h.alive && h.id % 2 == 0).collect();
        let inject: Vec<f64> = treated.iter().map(|&t| if t { AMOUNT } else { 0.0 }).collect();
        // classify on the pre-transfer state (last month's income, current deposits)
        let grp: Vec<usize> = w0
            .households
            .iter()
            .zip(&w0.hh_month)
            .map(|(h, m)| {
                let inc = if m.income.total() > 0.0 { m.income.total() } else { h.permanent_income };
                group_of(h.deposits, inc, h.status == LaborStatus::Retired)
            })
            .collect();
        let nh = w0.households.len();
        let mut cum = vec![[0.0f64; 4]; nh];
        let mut dur = vec![0.0f64; nh];
        let mut tot = vec![0.0f64; nh];
        for t in 0..months {
            step_inject(&mut base, None, windfall);
            step_inject(&mut tr, if t == 0 { Some(&inject) } else { None }, windfall);
            for i in 0..nh.min(base.hh_month.len()).min(tr.hh_month.len()) {
                if !alive0[i] {
                    continue;
                }
                let d = spent(&tr.hh_month[i]) - spent(&base.hh_month[i]);
                let dd = tr.hh_month[i].consumption[Sector::DurableGoods.idx()] - base.hh_month[i].consumption[Sector::DurableGoods.idx()];
                dur[i] += dd;
                tot[i] += d;
                for (k, &h) in HORIZONS.iter().enumerate() {
                    if t < h {
                        cum[i][k] += d;
                    }
                }
            }
        }
        // difference-in-differences: treated minus untreated, within each liquidity group, so that general-equilibrium
        // effects common to both halves cancel
        for i in 0..nh {
            if !alive0[i] {
                continue;
            }
            let (g, a) = if treated[i] { (&mut groups[grp[i]].0, &mut all.0) } else { (&mut groups[grp[i]].1, &mut all.1) };
            g.n += 1.0;
            a.n += 1.0;
            for k in 0..4 {
                g.cum[k] += cum[i][k];
                a.cum[k] += cum[i][k];
            }
            g.dur += dur[i];
            a.dur += dur[i];
            g.tot += tot[i];
            a.tot += tot[i];
        }
    }
    let show = |name: &str, (t, u): &(Acc, Acc)| {
        if t.n == 0.0 || u.n == 0.0 {
            println!("{name:>12}: n=0");
            return;
        }
        let mpc: Vec<String> = (0..4)
            .map(|k| format!("m{} {:.3}", HORIZONS[k], (t.cum[k] / t.n - u.cum[k] / u.n) / AMOUNT))
            .collect();
        let dur = t.dur / t.n - u.dur / u.n;
        let tot = t.tot / t.n - u.tot / u.n;
        let share = if tot.abs() > 1e-9 { dur / tot } else { 0.0 };
        println!("{name:>12}: share {:.3} | {} | durables share of extra spend {:.3}", t.n / all.0.n, mpc.join("  "), share);
    };
    println!("cumulative MPC out of a ${AMOUNT:.0} one-off payment (DiD: treated minus untreated, {seeds} seeds, {n} households)");
    for (g, name) in GROUPS.iter().enumerate() {
        show(name, &groups[g]);
    }
    show("all", &all);
}

fn dist_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(120);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let bands = [(18.0, 35.0), (35.0, 45.0), (45.0, 55.0), (55.0, 65.0), (65.0, 75.0), (75.0, 80.0), (80.0, 90.0), (90.0, 120.0)];
    let mut htm = Vec::new();
    let mut ratios: Vec<Vec<f64>> = vec![Vec::new(); 2];
    let mut mean_ratio_75_55 = 0.0;
    for seed in 1..=seeds {
        let mut w = sim::new_world(&cfg, seed);
        sim::run(&mut w, months);
        // mean net worth 75+ over 55-64 (SCF 2022 about 1.0; the audit target is 0.9-1.1)
        {
            let oe = common::owner_equity(&w);
            let mean_of = |lo: f64, hi: f64| {
                let v: Vec<f64> = (0..w.households.len())
                    .filter(|&i| w.households[i].alive && w.households[i].age_years() >= lo && w.households[i].age_years() < hi)
                    .map(|i| common::net_worth(&w, i as HhId, &oe))
                    .collect();
                v.iter().sum::<f64>() / v.len().max(1) as f64
            };
            mean_ratio_75_55 += mean_of(75.0, 120.0) / mean_of(55.0, 65.0) / seeds as f64;
        }
        let oe = common::owner_equity(&w);
        let alive: Vec<usize> = (0..w.households.len()).filter(|&i| w.households[i].alive).collect();
        let mut r: Vec<f64> = alive
            .iter()
            .map(|&i| {
                let inc = w.hh_month[i].income.total().max(1.0);
                w.households[i].deposits / inc
            })
            .collect();
        let k = r.len() as f64;
        htm.push(r.iter().filter(|&&x| x < 0.5).count() as f64 / k);
        r.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |p: f64| r[((p * r.len() as f64) as usize).min(r.len() - 1)];
        println!("seed {seed}: HtM share {:.3} | deposits/monthly income p10 {:.2} p25 {:.2} p50 {:.2} p75 {:.2}", htm.last().unwrap(), q(0.1), q(0.25), q(0.5), q(0.75));
        println!("  band      n    retired  mean NW     median NW");
        for (lo, hi) in bands {
            let idx: Vec<usize> = alive.iter().copied().filter(|&i| {
                let a = w.households[i].age_years();
                a >= lo && a < hi
            }).collect();
            let mut nw: Vec<f64> = idx.iter().map(|&i| common::net_worth(&w, i as HhId, &oe)).collect();
            nw.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let retired = idx.iter().filter(|&&i| w.households[i].status == LaborStatus::Retired).count();
            let mean = nw.iter().sum::<f64>() / nw.len().max(1) as f64;
            let med = nw.get(nw.len() / 2).copied().unwrap_or(0.0);
            println!("  {:>3}-{:<3} {:6} {:7.2}  {:10.0}  {:10.0}", lo, hi, idx.len(), retired as f64 / idx.len().max(1) as f64, mean, med);
            if lo == 65.0 {
                ratios[0].push(med);
            }
            if lo == 80.0 {
                ratios[1].push(med);
            }
        }
    }
    let mean_htm = htm.iter().sum::<f64>() / htm.len() as f64;
    println!("mean HtM share {mean_htm:.3}");
    let r65: f64 = ratios[0].iter().sum::<f64>() / ratios[0].len().max(1) as f64;
    let r80: f64 = ratios[1].iter().sum::<f64>() / ratios[1].len().max(1) as f64;
    println!("median NW 80-90 / 65-75 = {:.3}", r80 / r65);
    println!("mean NW 75+ / 55-64 = {:.3}", mean_ratio_75_55);
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("mpc") => mpc_cmd(&a),
        Some("dist") => dist_cmd(&a),
        Some("jobloss") => jobloss_cmd(&a),
        Some("retire") => retire_cmd(&a),
        Some("energy") => energy_cmd(&a),
        Some("heligen") => heligen_cmd(&a),
        Some("food") => food_cmd(&a),
        _ => eprintln!("usage: hhdiag mpc|dist|jobloss|retire|energy|heligen|food <households> <seeds> <months>"),
    }
}

/// Job-loss spending path (fix-households, Ganong & Noel 2019): households employed for the three months before
/// separation and unemployed for at least ten months. Spending excludes HealthEducation (employer-benefit pass-through),
/// relative to the mean of the three pre-loss months. Prints the ratio at k = 0..=12 months after the loss, and the
/// step in spending at benefit exhaustion (the first month with no UI left).
fn jobloss_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(120);
    let cfg = Config { n_households: n, ..Config::from_env() };
    const K: usize = 13;
    let he = Sector::HealthEducation.idx();
    let mut sum = [0.0f64; K];
    let mut cnt = 0.0f64;
    let (mut step_sum, mut step_n) = (0.0f64, 0.0f64);
    let (mut pre_ratio, mut exh_n) = (0.0f64, 0.0f64);
    for seed in 1..=seeds {
        let mut w = sim::new_world(&cfg, seed);
        let nh = w.households.len();
        let mut spend: Vec<Vec<f64>> = Vec::with_capacity(months);
        let mut emp: Vec<Vec<bool>> = Vec::with_capacity(months);
        let mut une: Vec<Vec<bool>> = Vec::with_capacity(months);
        // unemployment benefit paid this month (the last paid month has benefit_months_left == 0 already)
        let mut ubn: Vec<Vec<f64>> = Vec::with_capacity(months);
        for _ in 0..months {
            step_inject(&mut w, None, true);
            spend.push(
                (0..nh)
                    .map(|i| {
                        let c = &w.hh_month[i].consumption;
                        c.iter().enumerate().filter(|(s, _)| *s != he).map(|(_, x)| *x).sum()
                    })
                    .collect(),
            );
            emp.push(w.households.iter().map(|h| h.status == LaborStatus::Employed).collect());
            une.push(w.households.iter().map(|h| h.status == LaborStatus::Unemployed).collect());
            ubn.push(w.hh_month.iter().map(|m| m.income.unemployment_benefit).collect());
        }
        for t in 3..months.saturating_sub(K + 9) {
            for h in 0..nh {
                if !(emp[t - 3..t].iter().all(|e| e[h])) || !(0..10).all(|j| une[t + j][h]) {
                    continue;
                }
                let base: f64 = (t - 3..t).map(|u| spend[u][h]).sum::<f64>() / 3.0;
                if base <= 1e-9 || !base.is_finite() {
                    continue;
                }
                for k in 0..K {
                    sum[k] += spend[t + k][h] / base;
                }
                cnt += 1.0;
                // exhaustion: first month after the loss with no unemployment benefit paid
                if let Some(j) = (1..10).find(|&j| ubn[t + j][h] <= 0.0) {
                    let r_prev = spend[t + j - 1][h] / base;
                    let r_here = spend[t + j][h] / base;
                    if r_prev > 0.0 {
                        step_sum += r_here / r_prev - 1.0;
                        step_n += 1.0;
                    }
                    pre_ratio += r_prev;
                    exh_n += 1.0;
                }
            }
        }
    }
    println!("job-loss spending path, {seeds} seeds, {n} households, {months} months: {cnt:.0} spells");
    for k in 0..K {
        println!("  k={k:2}  spending / pre-loss {:.3}", sum[k] / cnt.max(1.0));
    }
    println!("  step at benefit exhaustion (month of exhaustion vs previous month): {:+.3} over {step_n:.0} spells", step_sum / step_n.max(1.0));
    println!("  (mean ratio the month before exhaustion {:.3} over {exh_n:.0} spells)", pre_ratio / exh_n.max(1.0));
}

/// Retiree response to a +100bp policy shock for 12 months (fix-households, the audit's ratehet design): retirees'
/// spending (excluding HealthEducation) relative to the no-shock run with the same seed, in months 1-3, 4-6 and 10-12.
fn retire_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(24);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let he = Sector::HealthEducation.idx();
    let windows = [(0usize, 3usize), (3, 6), (9, 12)];
    let mut acc = [0.0f64; 3];
    let mut acc_all = [0.0f64; 3];
    for seed in 1..=seeds {
        let w0 = sim::new_world(&cfg, seed);
        let mut base = w0.clone();
        let mut shock = w0.clone();
        shock.shocks.schedule(ScheduledShock { at_month: shock.tick, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
        let mut tot_b = vec![0.0; months];
        let mut tot_s = vec![0.0; months];
        let mut ret_b = vec![0.0; months];
        let mut ret_s = vec![0.0; months];
        for t in 0..months {
            step_inject(&mut base, None, true);
            step_inject(&mut shock, None, true);
            // retirees by the no-shock run's status in this month (the same households in both runs)
            let retired: Vec<bool> = base.households.iter().map(|h| h.alive && h.status == LaborStatus::Retired).collect();
            for i in 0..base.households.len().min(shock.households.len()) {
                let sb: f64 = base.hh_month[i].consumption.iter().enumerate().filter(|(s, _)| *s != he).map(|(_, x)| *x).sum();
                let ss: f64 = shock.hh_month[i].consumption.iter().enumerate().filter(|(s, _)| *s != he).map(|(_, x)| *x).sum();
                tot_b[t] += sb;
                tot_s[t] += ss;
                if retired[i] {
                    ret_b[t] += sb;
                    ret_s[t] += ss;
                }
            }
        }
        for (k, &(a, b)) in windows.iter().enumerate() {
            let rb: f64 = ret_b[a..b].iter().sum();
            let rs: f64 = ret_s[a..b].iter().sum();
            acc[k] += 100.0 * (rs / rb - 1.0) / seeds as f64;
            let ab: f64 = tot_b[a..b].iter().sum();
            let as_: f64 = tot_s[a..b].iter().sum();
            acc_all[k] += 100.0 * (as_ / ab - 1.0) / seeds as f64;
        }
    }
    println!("+100bp for 12 months, retirees' spending vs no shock (%), {seeds} seeds, {n} households");
    println!("  retirees: m1-3 {:+.2}  m4-6 {:+.2}  m10-12 {:+.2}", acc[0], acc[1], acc[2]);
    println!("  all:      m1-3 {:+.2}  m4-6 {:+.2}  m10-12 {:+.2}", acc_all[0], acc_all[1], acc_all[2]);
}

/// `hhdiag energy <households> <seeds> <months>`: oil +50% (half-life 12 months) against the no-shock run. Prints, at
/// months 3, 6, 12 and 24, the energy price change, the change in household real energy demand (nominal orders
/// deflated by the energy price; orders, not realised purchases, so that supply rationing does not enter), and the
/// change in the energy share of household nominal orders (percentage points).
fn energy_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(24);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let e = Sector::Energy.idx();
    let shown = [0usize, 1, 2, 5, 11, 23];
    let mut price = vec![0.0f64; months];
    let mut real = vec![0.0f64; months];
    let mut share = vec![0.0f64; months];
    let f = seeds as f64;
    for seed in 1..=seeds {
        let w0 = sim::new_world(&cfg, seed);
        let mut base = w0.clone();
        let mut shock = w0.clone();
        shock.shocks.schedule(ScheduledShock { at_month: shock.tick, shock: Shock::OilPrice { pct: 50.0, half_life_months: 12.0 } });
        for t in 0..months {
            step_inject(&mut base, None, true);
            step_inject(&mut shock, None, true);
            let pb = common::sector_price(&base, Sector::Energy);
            let ps = common::sector_price(&shock, Sector::Energy);
            let (mut eb, mut es, mut tb, mut ts) = (0.0, 0.0, 0.0, 0.0);
            for (ob, os) in base.orders.household.iter().zip(&shock.orders.household) {
                eb += ob[e];
                es += os[e];
                tb += ob.iter().sum::<f64>();
                ts += os.iter().sum::<f64>();
            }
            price[t] += 100.0 * (ps / pb - 1.0) / f;
            real[t] += 100.0 * ((es / ps) / (eb / pb) - 1.0) / f;
            share[t] += 100.0 * (es / ts - eb / tb) / f;
        }
    }
    println!("oil +50% (half-life 12m) vs no shock, {seeds} seeds, {n} households");
    println!("              m1      m2      m3      m6     m12     m24");
    let row = |name: &str, v: &[f64]| {
        let cells: Vec<String> = shown.iter().map(|&t| format!("{:>7.2}", v[t])).collect();
        println!("{name:<13}{}", cells.join(""));
    };
    row("energy px %", &price);
    row("real energy %", &real);
    row("energy sh pp", &share);
}

/// `hhdiag heligen <households> <seeds> <months>`: general-equilibrium helicopter money, $2,000 per household a month for
/// 3 months (the IRF shock), against the no-shock run. Prints the cumulative nominal change in household consumption
/// (gross, after tax) at months 3, 6 and 12, as a share of the cumulative payments.
fn heligen_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(12);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let shown = [3usize, 6, 12];
    let mut acc = [0.0f64; 3];
    let f = seeds as f64;
    for seed in 1..=seeds {
        let w0 = sim::new_world(&cfg, seed);
        let mut base = w0.clone();
        let mut shock = w0.clone();
        shock.shocks.schedule(ScheduledShock { at_month: shock.tick, shock: Shock::Helicopter { per_household: 2000.0, months: 3 } });
        let (mut cum_d, mut cum_paid) = (0.0, 0.0);
        for t in 0..months {
            step_inject(&mut base, None, true);
            step_inject(&mut shock, None, true);
            let cb: f64 = base.hh_month.iter().map(spent).sum();
            let cs: f64 = shock.hh_month.iter().map(spent).sum();
            cum_d += cs - cb;
            if t < 3 {
                cum_paid += 2000.0 * shock.households.iter().filter(|h| h.alive).count() as f64;
            }
            for (k, &h) in shown.iter().enumerate() {
                if t + 1 == h {
                    acc[k] += cum_d / cum_paid / f;
                }
            }
        }
    }
    println!("helicopter $2000 x 3 months, GE: cumulative nominal consumption change / payments, {seeds} seeds, {n} households");
    println!("  m3 {:.3}   m6 {:.3}   m12 {:.3}", acc[0], acc[1], acc[2]);
}

/// `hhdiag food <households> <seeds> <months>`: nominal Food share of household orders by window (fix-households, the
/// necessity price response must be homogeneous of degree zero, so Engel's law holds over long runs: the share falls).
fn food_cmd(args: &[String]) {
    let n: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let seeds: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(2);
    let months: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(240);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let fd = Sector::Food.idx();
    let mut share = vec![0.0f64; months];
    let f = seeds as f64;
    for seed in 1..=seeds {
        let mut w = sim::new_world(&cfg, seed);
        for t in 0..months {
            step_inject(&mut w, None, true);
            let (mut fo, mut to) = (0.0, 0.0);
            for o in &w.orders.household {
                fo += o[fd];
                to += o.iter().sum::<f64>();
            }
            share[t] += 100.0 * fo / to.max(1e-9) / f;
        }
    }
    let window = |a: usize, b: usize| share[a..b.min(months)].iter().sum::<f64>() / (b.min(months) - a).max(1) as f64;
    println!("Food share of nominal household orders (%), {seeds} seeds, {n} households");
    println!("  m0-12 {:.2}   m108-120 {:.2}   m228-240 {:.2}", window(0, 12), window(108, 120), window(228, 240));
}
