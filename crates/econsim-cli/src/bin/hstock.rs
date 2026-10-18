//! Housing stock accounting by year: `hstock [households] [years]` — occupied/vacant by use, rental owners, renters.
use econsim_core::types::*;
use econsim_core::{sim, Config};
fn row(w: &World, y: u32) {
    let (mut own_occ, mut rented, mut vac_rent, mut vac_sale, mut vac_idle, mut hh_ll, mut firm_ll, mut bank_ll) = (0, 0, 0, 0, 0, 0, 0, 0);
    for h in w.houses.iter().filter(|h| h.completed) {
        match h.occupant {
            Some(o) => {
                let occ_owner = matches!(h.owner, Owner::Household(x) if x == o);
                if occ_owner { own_occ += 1 } else {
                    rented += 1;
                    match h.owner { Owner::Household(_) => hh_ll += 1, Owner::Firm(_) => firm_ll += 1, Owner::Bank(_) => bank_ll += 1 }
                }
            }
            None => if h.for_sale { vac_sale += 1 } else if h.for_rent { vac_rent += 1 } else { vac_idle += 1 },
        }
    }
    let renters = w.households.iter().filter(|h| h.alive && h.tenure == Tenure::Renter).count();
    let parents = w.households.iter().filter(|h| h.alive && h.tenure == Tenure::WithParents).count();
    let alive = w.households.iter().filter(|h| h.alive).count();
    let landlords = w.households.iter().filter(|h| h.alive && h.properties.len() > usize::from(h.tenure == Tenure::Owner)).count();
    let bands = [18.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0, 120.0];
    let mut prof = String::new();
    for b in bands.windows(2) {
        let g: Vec<&Household> = w.households.iter().filter(|h| h.alive && h.age_years() >= b[0] && h.age_years() < b[1]).collect();
        let o = g.iter().filter(|h| h.tenure == Tenure::Owner).count() as f64 / g.len().max(1) as f64;
        prof += &format!(" {:>2}:{:4.1}", b[0], 100.0 * o);
    }
    println!("{y:>3} {alive:>6} {own_occ:>7} {rented:>7} {renters:>7} {parents:>6} {vac_rent:>7} {vac_sale:>7} {vac_idle:>7} {hh_ll:>6} {firm_ll:>5} {bank_ll:>5} {landlords:>6} |{prof}");
}
/// Monthly renter→owner and owner→renter transitions by age band (realised annual rates), tallied from household
/// snapshots. A slot whose household was replaced (age fell, or dead) is skipped. A seller who sells and rents in the
/// same month counts as an owner→renter exit.
#[derive(Default)]
struct Flows { exp_r: [f64; 7], exp_o: [f64; 7], r2o: [f64; 7], o2r: [f64; 7] }
type Snap = Vec<(bool, Tenure, u16)>;
fn snap(w: &World) -> Snap { w.households.iter().map(|h| (h.alive, h.tenure, h.age_months)).collect() }
fn band7(age: f64) -> usize { [18.0, 25.0, 35.0, 45.0, 55.0, 65.0, 75.0].iter().rposition(|&b| age >= b).unwrap_or(0) }
fn tally(prev: &Snap, w: &World, f: &mut Flows) {
    for (i, h) in w.households.iter().enumerate() {
        let Some(&(alive, ten, am)) = prev.get(i) else { continue };
        if !alive || !h.alive || h.age_months < am { continue; }
        let b = band7(am as f64 / 12.0);
        match ten {
            Tenure::Renter => { f.exp_r[b] += 1.0; if h.tenure == Tenure::Owner { f.r2o[b] += 1.0; } }
            Tenure::Owner => { f.exp_o[b] += 1.0; if h.tenure == Tenure::Renter { f.o2r[b] += 1.0; } }
            Tenure::WithParents => {}
        }
    }
}
fn flows_row(f: &Flows) -> String {
    let fmt = |e: &[f64; 7], n: &[f64; 7]| -> String {
        (0..7).map(|b| format!("{:5.1}", 100.0 * n[b] / (e[b] / 12.0).max(1e-9))).collect::<Vec<_>>().join(" ")
    };
    format!("     r->o {}  | o->r {}", fmt(&f.exp_r, &f.r2o), fmt(&f.exp_o, &f.o2r))
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(20000);
    let years: u32 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(20);
    let mut w = sim::new_world(&Config { n_households: n, ..Config::from_env() }, 1);
    println!("{:>3} {:>6} {:>7} {:>7} {:>7} {:>6} {:>7} {:>7} {:>7} {:>6} {:>5} {:>5} {:>6}", "yr", "alive", "ownocc", "rented", "renters", "parent", "vacRent", "vacSale", "vacIdle", "hhLL", "firmL", "bankL", "lndlrd");
    row(&w, 0);
    let mut f = Flows::default();
    for y in 1..=years {
        for _ in 0..12 {
            let prev = snap(&w);
            sim::step(&mut w);
            tally(&prev, &w, &mut f);
        }
        if y % 2 == 0 {
            row(&w, y);
            println!("{}", flows_row(&f));
            f = Flows::default();
        }
    }
}
