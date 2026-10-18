//! Labour-market flow diagnostic (fix-labour). `labflows [households] [months] [seed]` prints one CSV row per month:
//! start-of-month stocks, raw flow counts (from the labour counters in `Flows` and from status snapshots), the
//! education / duration / UI breakdowns, and the realised cycle series. `scripts/labour_check.py` turns the rows into
//! rates and checks them against the CPS and JOLTS targets.

use econsim_core::types::*;
use econsim_core::{sim, Config};

/// Snapshot code: 1 employed, 2 unemployed, 3 out of the labour force (working age), 0 otherwise or dead.
fn code(h: &Household) -> u8 {
    if !h.alive {
        return 0;
    }
    match h.status {
        LaborStatus::Employed => 1,
        LaborStatus::Unemployed => 2,
        LaborStatus::OutOfLaborForce => 3,
        _ => 0,
    }
}

/// Grouped median of whole-month durations: each integer m is the bin [m, m+1) (the same convention as the mean's
/// m + 0.5). Gives a continuous median where the integer median only takes values 0.5, 1.5, ... months.
/// `durs` must be sorted.
fn grouped_median(durs: &[f64]) -> f64 {
    let n = durs.len();
    if n == 0 {
        return 0.0;
    }
    let half = n as f64 / 2.0;
    let mut cum = 0.0;
    let mut i = 0;
    while i < n {
        let m = durs[i];
        let mut j = i;
        while j < n && durs[j] == m {
            j += 1;
        }
        let f = (j - i) as f64;
        if cum + f >= half {
            return m + (half - cum) / f;
        }
        cum += f;
        i = j;
    }
    durs[n - 1] + 1.0
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(240);
    let seed: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, seed);
    println!(
        "tick,u_pct,lfpr_pct,wage_g_yoy,core_yoy,gdp_g_yoy,E,U,N,E0,U0,N0,U_PRE,N_PRE,UN,NU,EE,HU,HN,HIRES,LAY,SEPX,\
         VAC,NEEDED,emp_edu0,emp_edu1,emp_edu2,u_edu0,u_edu1,u_edu2,hu_edu0,hu_edu1,hu_edu2,sep_edu0,sep_edu1,sep_edu2,\
         u_dur0,u_dur1,u_dur2,u_dur3,hu_dur0,hu_dur1,hu_dur2,hu_dur3,u_ui0,u_ui1,u_ui2,hu_ui0,hu_ui1,hu_ui2,\
         dur_mean,dur_med,dur_27w,pe_edu0,pe_edu1,pe_edu2,pu_edu0,pu_edu1,pu_edu2,hu_snap,hn_snap,ee_cps,ue_snap,\
         ee_all_snap,wage_stay_yoy,dur_med_cont,\
         udur_rank_leftover,udur_rank_firms,udur_screened,udur_ui_ext_n,udur_resumed,\
         rewage_n0,rewage_n1,rewage_n2,rewage_n3,rewage_s0,rewage_s1,rewage_s2,rewage_s3,\
         uh_0,uh_1,uh_2,uh_3,uh_4,uh_5,uh_6,uh_7,uh_8,uh_9,uh_10,uh_11,uh_12,uh_13,uh_14,uh_15,uh_16,uh_17,uh_18,uh_19,\
         uh_20,uh_21,uh_22,uh_23,uh_24,udur_reentry_recent_n,udur_reentry_scar_n,\
         bls_dur_mean,bls_dur_med,bls_dur_27w"
    );
    let mut stayers = econsim_core::labor::StayerTrack::default();
    for _ in 0..months {
        let pre: Vec<u8> = w.households.iter().map(code).collect();
        let pre_emp: Vec<Option<u32>> = w.households.iter().map(|h| h.employer).collect();
        sim::step(&mut w);
        let stay = stayers.observe(&w.households);
        let post: Vec<u8> = w.households.iter().map(code).collect();
        let (mut u_pre, mut n_pre, mut un, mut nu) = (0u32, 0u32, 0u32, 0u32);
        // Snapshot flows between the start and end of the month (CPS-style: status in both months).
        let (mut hu_snap, mut hn_snap, mut ee_cps, mut ue_snap, mut ee_all) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for (i, (p, q)) in pre.iter().zip(post.iter()).enumerate() {
            match p {
                2 => u_pre += 1,
                3 => n_pre += 1,
                _ => {}
            }
            match (p, q) {
                (2, 3) => un += 1,
                (3, 2) => nu += 1,
                (2, 1) => hu_snap += 1,
                (3, 1) => hn_snap += 1,
                (1, 2) => ue_snap += 1,
                _ => {}
            }
            if *p == 1 && *q == 1 {
                let post_emp = w.households[i].employer;
                if pre_emp[i] != post_emp {
                    ee_cps += 1;
                }
            }
            if *p == 1 && *q == 1 && pre_emp[i] != w.households[i].employer {
                ee_all += 1;
            }
        }
        let (mut e, mut u, mut nn) = (0u32, 0u32, 0u32);
        for &q in &post {
            match q {
                1 => e += 1,
                2 => u += 1,
                3 => nn += 1,
                _ => {}
            }
        }
        let mut durs: Vec<f64> = w
            .households
            .iter()
            .filter(|h| h.alive && h.status == LaborStatus::Unemployed)
            .map(|h| h.months_unemployed as f64)
            .collect();
        durs.sort_by(|x, y| x.total_cmp(y));
        // b6-reentry: the BLS search-spell duration (a month out of the labour force restarts it).
        let mut bls: Vec<f64> = w
            .households
            .iter()
            .filter(|h| h.alive && h.status == LaborStatus::Unemployed)
            .map(|h| h.bls_unemployed as f64)
            .collect();
        bls.sort_by(|x, y| x.total_cmp(y));
        let bls_mean = bls.iter().sum::<f64>() / bls.len().max(1) as f64;
        let bls_med = if bls.is_empty() { 0.0 } else { bls[bls.len() / 2] };
        let bls_27w = bls.iter().filter(|&&d| d >= 6.0).count() as f64 / bls.len().max(1) as f64;
        let dur_mean = durs.iter().sum::<f64>() / durs.len().max(1) as f64;
        let dur_med = if durs.is_empty() { 0.0 } else { durs[durs.len() / 2] };
        let dur_med_cont = grouped_median(&durs);
        // 27+ weeks is months unemployed >= 6 (weeks = (m + 0.5) * 4.345 for a household in its m-th month).
        let dur_27w = durs.iter().filter(|&&d| d >= 6.0).count() as f64 / durs.len().max(1) as f64;
        let m = w.history.last().expect("metrics recorded");
        let f = &w.flows;
        let e0: u32 = f.emp_edu.iter().sum();
        let u0: u32 = f.u_stock_edu.iter().sum();
        let lfpr = 100.0 * (e + u) as f64 / (e + u + nn).max(1) as f64;
        let mut row: Vec<String> = vec![
            m.tick.to_string(),
            format!("{:.4}", m.unemployment_rate),
            format!("{:.3}", lfpr),
            format!("{:.4}", m.wage_growth_yoy),
            format!("{:.4}", m.core_inflation_yoy),
            format!("{:.4}", m.gdp_real_growth_yoy),
            e.to_string(),
            u.to_string(),
            nn.to_string(),
            e0.to_string(),
            u0.to_string(),
            f.n_stock.to_string(),
            u_pre.to_string(),
            n_pre.to_string(),
            un.to_string(),
            nu.to_string(),
            f.ee.to_string(),
            f.hires_from_u.to_string(),
            f.hires_from_n.to_string(),
            f.hires.to_string(),
            f.layoffs.to_string(),
            f.sep_exo.to_string(),
            f.vacancies.to_string(),
            w.labor.needed_last.to_string(),
        ];
        row.extend(f.emp_edu.iter().map(|x| x.to_string()));
        row.extend(f.u_stock_edu.iter().map(|x| x.to_string()));
        row.extend(f.hires_u_edu.iter().map(|x| x.to_string()));
        row.extend(f.sep_edu.iter().map(|x| x.to_string()));
        row.extend(f.u_stock_dur.iter().map(|x| x.to_string()));
        row.extend(f.hires_u_dur.iter().map(|x| x.to_string()));
        row.extend(f.u_stock_ui.iter().map(|x| x.to_string()));
        row.extend(f.hires_u_ui.iter().map(|x| x.to_string()));
        row.push(format!("{:.3}", dur_mean));
        row.push(format!("{:.3}", dur_med));
        row.push(format!("{:.4}", dur_27w));
        // Post-step stocks by education (the headline definition: start-of-step stocks include this month's entrants).
        let mut pe = [0u32; 3];
        let mut pu = [0u32; 3];
        for h in w.households.iter().filter(|h| h.alive) {
            let e = match h.education {
                Education::HighSchool => 0,
                Education::College => 1,
                Education::Graduate => 2,
            };
            match h.status {
                LaborStatus::Employed => pe[e] += 1,
                LaborStatus::Unemployed => pu[e] += 1,
                _ => {}
            }
        }
        row.extend(pe.iter().map(|x| x.to_string()));
        row.extend(pu.iter().map(|x| x.to_string()));
        row.push(hu_snap.to_string());
        row.push(hn_snap.to_string());
        row.push(ee_cps.to_string());
        row.push(ue_snap.to_string());
        row.push(ee_all.to_string());
        row.push(stay.map_or("nan".to_string(), |g| format!("{g:.4}")));
        row.push(format!("{:.4}", dur_med_cont));
        // b5-udur columns (appended; labour_check.py reads by header).
        row.push(f.udur_rank_leftover.to_string());
        row.push(f.udur_rank_firms.to_string());
        row.push(f.udur_screened.to_string());
        row.push(f.udur_ui_ext_n.to_string());
        row.push(f.udur_resumed.to_string());
        row.extend(f.udur_rewage_n.iter().map(|x| x.to_string()));
        row.extend(f.udur_rewage_sum.iter().map(|x| format!("{x:.6}")));
        // End-of-month unemployed by months unemployed (0..23, then 24+).
        let mut uh = [0u32; 25];
        for h in w.households.iter().filter(|h| h.alive && h.status == LaborStatus::Unemployed) {
            uh[(h.months_unemployed as usize).min(24)] += 1;
        }
        row.extend(uh.iter().map(|x| x.to_string()));
        // b6-reentry columns (appended).
        row.push(f.udur_reentry_recent_n.to_string());
        row.push(f.udur_reentry_scar_n.to_string());
        row.push(format!("{bls_mean:.3}"));
        row.push(format!("{bls_med:.3}"));
        row.push(format!("{bls_27w:.4}"));
        println!("{}", row.join(","));
    }
}
