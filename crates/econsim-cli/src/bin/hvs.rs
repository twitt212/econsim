//! Paired IRF probe for rental vacancy (fix-housing, review 8): `hvs [households=10000] [months=36] [seeds=16]`.
//! Shock minus baseline, averaged over common-random-number seeds, for the policy +100bp (12m), the oil +50% and the 10y
//! term premium +100bp (half-life 24m) shocks. Prints the Census HVS rental vacancy (vacant for-rent units divided by
//! renter-occupied plus vacant for-rent units, computed from the houses as `update_indices` does), total vacancy, and
//! shelter yoy, at months 3, 6, 12, 18, 24 and 36.
//!
//! With a fourth argument `flows` (b4-rent), extra rows follow at the same horizons: cumulative renter→owner and
//! owner→renter household transitions (% change of the cumulative count against the baseline), the vacant-for-rent
//! stock flows (renters who bought, tenants not re-housed, listing expiries let, vacant rentals put up for sale, new
//! leases; shock minus baseline in units, one unit per house per 10k households at 10k), evictions, homeownership pp,
//! the house price index % and the median house price %. The existing rows are printed unchanged.

use econsim_core::types::{HhId, Owner, Tenure};
use econsim_core::{sim, Config, ScheduledShock, Shock, World};

fn hvs_rental_vacancy(w: &World) -> f64 {
    let (mut vac, mut rented) = (0usize, 0usize);
    for h in w.houses.iter().filter(|h| h.completed) {
        match h.occupant {
            None if h.for_rent => vac += 1,
            Some(o) if !matches!(h.owner, econsim_core::types::Owner::Household(x) if x == o) => rented += 1,
            _ => {}
        }
    }
    100.0 * vac as f64 / (rented + vac).max(1) as f64
}

/// Household tenure state for the flow rows (as in the b4-rent probe): dead, parents or seeking, renter, owner.
#[derive(Clone, Copy, PartialEq)]
enum HS {
    Dead,
    Parents,
    Seeker,
    Renter,
    Owner,
}

const C_OTHER: u8 = 0;
const C_RENTER_OCC: u8 = 1;
const C_VFR: u8 = 2;
const C_VFS: u8 = 3;

/// One month's state of the houses and households, for differencing two consecutive months.
struct Snap {
    cat: Vec<u8>,
    occ: Vec<Option<HhId>>,
    owner: Vec<Owner>,
    hs: Vec<HS>,
    age: Vec<u16>,
}

fn snap(w: &World) -> Snap {
    Snap {
        cat: w
            .houses
            .iter()
            .map(|h| match h.occupant {
                _ if !h.completed => C_OTHER,
                Some(o) if !matches!(h.owner, Owner::Household(x) if x == o) => C_RENTER_OCC,
                Some(_) => C_OTHER,
                None if h.for_sale => C_VFS,
                None if h.for_rent => C_VFR,
                None => C_OTHER,
            })
            .collect(),
        occ: w.houses.iter().map(|h| h.occupant).collect(),
        owner: w.houses.iter().map(|h| h.owner).collect(),
        hs: (0..w.households.len())
            .map(|i| {
                let h = &w.households[i];
                if !h.alive {
                    return HS::Dead;
                }
                let seeking = w.housing.seeking.get(i).copied().unwrap_or(false);
                match h.tenure {
                    Tenure::Owner => HS::Owner,
                    _ if h.home.is_some() && h.tenure == Tenure::Renter => HS::Renter,
                    _ if seeking && h.home.is_none() => HS::Seeker,
                    _ => HS::Parents,
                }
            })
            .collect(),
        age: w.households.iter().map(|h| h.age_months).collect(),
    }
}

/// Flow rows, in print order: renter→owner, owner→renter (cumulative household transitions); renters who bought, tenants
/// not re-housed, expiries let, vacant rentals put up for sale, new leases (cumulative stock flows into or out of the
/// vacant-for-rent stock); evictions (cumulative); then homeownership (level), house price index and median house price
/// (levels).
const NF: usize = 11;

/// Per-month rows of the flow block for one world (cumulative where the row says so).
fn flow_trace(cfg: &Config, shocks: &[ScheduledShock], seed: u64, months: u32) -> Vec<[f64; NF]> {
    let mut w = sim::new_world(cfg, seed);
    let start = w.tick;
    for s in shocks {
        let mut s = s.clone();
        s.at_month += start;
        w.shocks.schedule(s);
    }
    let mut out = Vec::new();
    let mut acc = [0.0f64; NF];
    let mut p = snap(&w);
    for _ in 0..months {
        sim::step(&mut w);
        let q = snap(&w);
        for i in 0..q.hs.len() {
            // A household whose age fell was replaced by a newborn in its slot: it counts as dead last month.
            let s0 = if i < p.hs.len() && !(q.age[i] < p.age[i] && p.hs[i] != HS::Dead) { p.hs[i] } else { HS::Dead };
            if s0 == HS::Renter && q.hs[i] == HS::Owner {
                acc[0] += 1.0;
            }
            if s0 == HS::Owner && q.hs[i] == HS::Renter {
                acc[1] += 1.0;
            }
        }
        for k in 0..p.cat.len().min(q.cat.len()) {
            if p.cat[k] == C_RENTER_OCC && q.cat[k] == C_VFR {
                // The previous tenant's new state decides (bought, or not re-housed; a tenant who moved to another rental
                // is neither).
                let o = p.occ[k].map_or(usize::MAX, |x| x as usize);
                let st = if o < q.hs.len() && !(q.age[o] < p.age[o] && p.hs[o] != HS::Dead) { q.hs[o] } else { HS::Dead };
                match st {
                    HS::Owner => acc[2] += 1.0,
                    HS::Renter => {}
                    _ => acc[3] += 1.0,
                }
            }
            if p.cat[k] == C_VFS && q.cat[k] == C_VFR && q.owner[k] == p.owner[k] {
                acc[4] += 1.0;
            }
            if p.cat[k] == C_VFR && q.cat[k] == C_VFS {
                acc[5] += 1.0;
            }
        }
        acc[6] += w.flows.new_leases as f64;
        acc[7] += w.flows.evictions as f64;
        let m = w.last().expect("metrics after a step").clone();
        let mut row = acc;
        row[8] = m.homeownership_rate;
        row[9] = w.prices.hpi;
        row[10] = m.median_house_price;
        out.push(row);
        p = q;
    }
    out
}

fn trace(cfg: &Config, shocks: &[ScheduledShock], seed: u64, months: u32) -> Vec<(f64, f64, f64)> {
    let mut w = sim::new_world(cfg, seed);
    let start = w.tick;
    for s in shocks {
        let mut s = s.clone();
        s.at_month += start;
        w.shocks.schedule(s);
    }
    let mut out = Vec::new();
    for _ in 0..months {
        sim::step(&mut w);
        let m = w.last().expect("metrics after a step").clone();
        out.push((hvs_rental_vacancy(&w), m.vacancy_rate_housing, m.shelter_inflation_yoy));
    }
    out
}

fn main() {
    sim::configure_thread_pool();
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(10_000);
    let months: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(36);
    let seeds: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(16);
    let cfg = Config { n_households: n, ..Config::from_env() };
    let at = |shock: Shock| vec![ScheduledShock { at_month: 0, shock }];
    let scenarios: Vec<(&str, Vec<ScheduledShock>)> = vec![
        ("Policy +100bp 12m", at(Shock::PolicyShift { bps: 100.0, months: 12 })),
        ("Oil +50%", at(Shock::OilPrice { pct: 50.0, half_life_months: 12.0 })),
        ("TP +100bp hl24", at(Shock::TermPremium { tenor_idx: 3, bps: 100.0, half_life_months: 24.0 })),
    ];
    let flows = a.get(4).is_some_and(|s| s == "flows");
    let hs = [3usize, 6, 12, 18, 24, 36];
    // HVS_PERSEED_OUT (b6-joint): also write the rental-vacancy shock-minus-base difference per seed to a TSV file.
    let perseed_out = std::env::var("HVS_PERSEED_OUT").ok();
    let mut tsv = String::from("scenario\tseed\tm3\tm6\tm12\tm18\tm24\tm36\n");
    let base: Vec<Vec<(f64, f64, f64)>> = (1..=seeds).map(|s| trace(&cfg, &[], s, months)).collect();
    let base_flow: Vec<Vec<[f64; NF]>> =
        if flows { (1..=seeds).map(|s| flow_trace(&cfg, &[], s, months)).collect() } else { Vec::new() };
    for (name, shocks) in scenarios {
        let shk: Vec<Vec<(f64, f64, f64)>> = (1..=seeds).map(|s| trace(&cfg, &shocks, s, months)).collect();
        println!("\n=== {name}");
        print!("{:<22}", "");
        for h in hs {
            print!("{:>9}", format!("m{h}"));
        }
        println!();
        if perseed_out.is_some() {
            for i in 0..seeds as usize {
                tsv.push_str(name);
                tsv.push('\t');
                tsv.push_str(&(i + 1).to_string());
                for h in hs {
                    if h > months as usize {
                        tsv.push_str("\t-");
                    } else {
                        tsv.push_str(&format!("\t{:.6}", shk[i][h - 1].0 - base[i][h - 1].0));
                    }
                }
                tsv.push('\n');
            }
        }
        for (label, idx) in [("HVS rental vac pp", 0usize), ("total vacancy pp", 1), ("shelter yoy pp", 2)] {
            print!("{label:<22}");
            for h in hs {
                if h > months as usize {
                    print!("{:>9}", "-");
                    continue;
                }
                let pick = |v: &(f64, f64, f64)| [v.0, v.1, v.2][idx];
                let d: Vec<f64> = (0..seeds as usize).map(|i| pick(&shk[i][h - 1]) - pick(&base[i][h - 1])).collect();
                print!("{:>9.3}", d.iter().sum::<f64>() / d.len() as f64);
            }
            println!();
        }
        if flows {
            let shk_flow: Vec<Vec<[f64; NF]>> = (1..=seeds).map(|s| flow_trace(&cfg, &shocks, s, months)).collect();
            print_flow_rows(&base_flow, &shk_flow, &hs, months);
        }
    }
    if let Some(path) = perseed_out {
        std::fs::write(&path, tsv).expect("HVS_PERSEED_OUT write");
    }
}

/// The flow rows of `hvs ... flows` (see the module docs): per-seed shock and baseline rows, averaged over seeds.
fn print_flow_rows(base: &[Vec<[f64; NF]>], shk: &[Vec<[f64; NF]>], hs: &[usize], months: u32) {
    let n = base.len() as f64;
    let labels = [
        "renter->owner %",
        "owner->renter %",
        "VFR + tenant bought",
        "VFR + not re-housed",
        "VFR + expiry let",
        "VFR - put up for sale",
        "new leases",
        "evictions",
        "homeownership pp",
        "hpi %",
        "median house price %",
    ];
    for (r, label) in labels.iter().enumerate() {
        print!("{label:<22}");
        for &h in hs {
            if h > months as usize {
                print!("{:>9}", "-");
                continue;
            }
            let t = h - 1;
            let v = match r {
                // Cumulative household transitions: percent change of the mean cumulative count.
                0 | 1 => {
                    let k: f64 = shk.iter().map(|x| x[t][r]).sum::<f64>() / n;
                    let b: f64 = base.iter().map(|x| x[t][r]).sum::<f64>() / n;
                    100.0 * (k / b.max(1e-9) - 1.0)
                }
                // Levels in percent: mean over seeds of the per-seed percent difference.
                9 | 10 => {
                    shk.iter().zip(base).map(|(x, b)| 100.0 * (x[t][r] / b[t][r] - 1.0)).sum::<f64>() / n
                }
                // Stock flows, pp of homeownership, and cumulative counts: shock minus baseline, averaged.
                8 => shk.iter().zip(base).map(|(x, b)| x[t][r] - b[t][r]).sum::<f64>() / n,
                _ => shk.iter().zip(base).map(|(x, b)| x[t][r] - b[t][r]).sum::<f64>() / n,
            };
            print!("{v:>9.3}");
        }
        println!();
    }
}
