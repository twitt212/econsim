//! Demography (docs/DESIGN.md §5, §16): aging, school completion, labour-force exits and re-entry, partnership,
//! retirement, mortality with inheritance, and entry: native households at ages drawn from the headship schedule
//! (weighted by the births of the cohort that reaches each age) and net immigrants at ages from the CBO net-migrant
//! profile.
//!
//! Partnered households survive the death of one partner (they continue as single households); only a single
//! household dissolves on its head's death. During burn-in (`init::build_world`) demography is frozen except for
//! labour-force flows.
//!
//! Money moves only through `ledger`. A dead household keeps its slot (ids are stable), holding no money, housing or
//! job, and stays dead until an entrant takes the lowest dead slot; new slots are appended only when none is free.

use rayon::prelude::*;
use rand::Rng;

use crate::common;
use crate::config::Config;
use crate::ledger::{self, Account};
use crate::rng::{self, stream, SimRng};
use crate::testkit::BASE_WAGE_OFFER;
use crate::types::*;

/// Households per parallel chunk in the monthly pass. Fixed, so chunk outputs merge identically at any thread count.
const CHUNK: usize = 1024;

/// Entry wage base used for reservation wages (wage per efficiency unit, t=0 level).
const ENTRY_WAGE_BASE: f64 = BASE_WAGE_OFFER;
/// Ages in months.
const ENTRANT_AGE: u32 = 18 * 12;
const COLLEGE_END: u32 = 22 * 12;
const GRAD_END: u32 = 24 * 12;
/// Prime-age window for labour-force exits (25–61 inclusive).
const PRIME_LO: u32 = 25 * 12;
const PRIME_HI: u32 = 62 * 12;
/// Heir window (25–60 inclusive); `HEIR_BUCKETS` one bucket per year of age.
const HEIR_LO: u32 = 25 * 12;
const HEIR_HI: u32 = 61 * 12;
const HEIR_BUCKETS: usize = 36;
/// Parents of new entrants: aged 40–64 inclusive.
const PARENT_LO: u32 = 40 * 12;
const PARENT_HI: u32 = 65 * 12;
const PARENT_MIN_DEPOSITS: f64 = 20_000.0;
const MAX_AGE: u32 = 100 * 12;
/// Monthly retirement hazards at 68–74 and from 75 (fix-demog; see `retirement_hazard`). The 75+ hazard is set so that
/// employment at 75+ is ≈8% (BLS); 0.6%/m gives 7–8% over a decade, 2%/m gave ≈4%.
const RETIRE_68_74_MONTHLY: f64 = 0.010;
const RETIRE_75_PLUS_MONTHLY: f64 = 0.006;
/// Labour-force flows (DESIGN §5): 0.2%/m out of the labour force, ~6%/m back in.
const OLF_INFLOW: f64 = 0.002;
const OLF_OUTFLOW: f64 = 0.06;
/// US births (millions) by calendar year, 1990–2025 (CDC NVSS). Native entrants aged 18 scale with the birth cohort
/// 18 years earlier; years outside the table take the nearest entry.
const US_BIRTHS_M: [f64; 36] = [
    4.16, 4.11, 4.07, 4.00, 3.95, 3.90, 3.89, 3.88, 3.94, 3.96, // 1990–1999
    4.06, 4.03, 4.02, 4.09, 4.11, 4.14, 4.27, 4.32, 4.25, 4.13, // 2000–2009
    4.00, 3.95, 3.95, 3.93, 3.99, 3.98, 3.95, 3.86, 3.79, 3.75, // 2010–2019
    3.61, 3.66, 3.67, 3.60, 3.62, 3.60, // 2020–2025
];
/// Reference birth year for the births index (the base entry rate is rescaled to the first post-burn-in cohort).
const BASE_COHORT_YEAR: i32 = 2008;
/// Starting money for a new entrant, paid by a parent (LogNormal, mean $3k).
const ENTRANT_MONEY_MEAN: f64 = 3_000.0;
const ENTRANT_MONEY_SIGMA: f64 = 0.8;
const ENTRANT_PERMANENT_INCOME: f64 = 1_500.0;
/// Native entrants (new households) take an age from the headship schedule (fix-demog): cumulative share of a birth
/// cohort that has formed a household by each age, CPS-based. Entry ages are whole years 18–39 (plus the fraction
/// within the year), weighted by the headship mass in each year and by the size of the birth cohort that reaches it:
/// an entrant of age a in calendar year y comes from the cohort born in y − a.
const HEADSHIP_KNOTS: [(f64, f64); 7] = [(18.0, 0.03), (20.0, 0.10), (22.0, 0.20), (25.0, 0.35), (30.0, 0.50), (35.0, 0.58), (40.0, 0.62)];
const ENTRY_AGE_LO: u32 = 18;
const N_ENTRY_AGES: usize = 22;
/// Years over which the native base entry rate averages HH_FORMATION_ANNUAL (fix-demog).
const FORMATION_HORIZON_YEARS: i32 = 20;
/// Net immigrant households' head ages (fix-demog): CBO net-migrant age profile, mode 25–34 with ≈15% over 45.
/// Cumulative share, lo, hi of each band; each band is uniform in age.
const IMMIG_AGE_BANDS: [(f64, f64, f64); 5] = [(0.12, 22.0, 25.0), (0.60, 25.0, 35.0), (0.85, 35.0, 45.0), (0.95, 45.0, 55.0), (1.0, 55.0, 65.0)];
/// Monthly share of postponed household formation released (≈3-year catch-up).
const PENT_UP_RELEASE: f64 = 0.03;
/// Share of new entrants who arrive partnered.
const ENTRANT_PARTNERED_SHARE: f64 = 0.35;
/// Partnership: single heads aged 20–59 partner at 5%/yr; partnered heads under 65 separate at 1.5%/yr.
const PARTNER_FORM_ANNUAL: f64 = 0.05;
const PARTNER_FORM_AGE_LO: u32 = 20 * 12;
const PARTNER_FORM_AGE_HI: u32 = 60 * 12;
const PARTNER_SEP_ANNUAL: f64 = 0.015;
const PARTNER_SEP_AGE_HI: u32 = 65 * 12;
/// A partnered household loses a partner with probability `PARTNER_LOSS_MULT` × the head's monthly mortality hazard:
/// the head's own hazard plus the partner's, which is ≈0.6× (wives are ≈2 years younger on average and female
/// mortality is ≈40% lower at 65–85; SSA period life tables).
const PARTNER_LOSS_MULT: f64 = 1.6;

/// Per-agent demography memory and per-month indices (rebuilt each step; no money stored here).
#[derive(Clone, Debug, Default)]
pub struct DemographyState {
    /// Alive households aged 25–60 bucketed by age in years − 25 (possible heirs; lazily cleaned).
    heirs: Vec<Vec<HhId>>,
    /// Alive households aged 40–64 with deposits above the parent threshold (possible givers).
    parents: Vec<HhId>,
    /// Alive firms by consumer sector (preferred-firm draws for entrants).
    firms_by_sector: Vec<Vec<FirmId>>,
    /// Mean expected inflation over alive households (new entrants start from it).
    infl_mean: f64,
    /// Alive households at the start of the step.
    pub n_alive: usize,
    /// Native entrants per month divided by the mean `native_formation_index` over `FORMATION_HORIZON_YEARS`, fixed at
    /// the first post-burn-in month so that entry equals expected dissolutions of single households plus native
    /// formation, averaged over the horizon.
    base_entrants: Option<f64>,
    /// Re-fit Step 6: expected dissolutions of single households at the calibration month (`diss0`, set with
    /// `base_entrants`) and in the current month (`diss_now`, recomputed each month while not frozen).
    diss0: f64,
    diss_now: f64,
    /// Fractional native entrants and immigrant entrants carried between months.
    native_acc: f64,
    /// Native household formation postponed by weak labour markets (negative after booms pull formation forward).
    pent_up: f64,
    immigrant_acc: f64,
    /// Per-chunk outputs of the monthly pass, reused across months and merged in chunk order.
    chunks: Vec<ChunkOut>,
}

/// One chunk's output from the monthly household pass: index entries plus rare events needing sequential handling.
#[derive(Clone, Debug, Default)]
struct ChunkOut {
    /// Alive households aged 25–60: (heir bucket, id), in household order.
    heirs: Vec<(u8, HhId)>,
    /// Alive households aged 40–64 with deposits above the parent threshold, in household order.
    parents: Vec<HhId>,
    /// Households with a labour-force inflow, retirement or death this month, in household order.
    events: Vec<Event>,
    alive: usize,
    infl: f64,
    /// b6-reentry: re-entries with spell resumed, and re-entries by hazard class.
    resumed: u32,
    re_recent: u32,
    re_scar: u32,
}

impl ChunkOut {
    fn clear(&mut self) {
        self.heirs.clear();
        self.parents.clear();
        self.events.clear();
        self.alive = 0;
        self.infl = 0.0;
        self.resumed = 0;
        self.re_recent = 0;
        self.re_scar = 0;
    }
}

/// A household's rare event this month. `rng` is the household's own generator, positioned after its draws,
/// and is present only when the household dies.
#[derive(Clone, Debug)]
struct Event {
    id: HhId,
    olf_in: bool,
    retire: bool,
    rng: Option<rng::SimRng>,
}

/// Monthly hazard of retirement for a non-retired household whose age becomes `age_months` this month
/// (DESIGN §5): 0.4%/m at 55–61, a one-off 4% at the 62nd birthday, 1.5%/m at 63–66, a one-off 8% at the
/// 67th birthday, 1.0%/m at 68–74 and 0.6%/m from 75. The hazards from 68 are calibrated to BLS employment-to-population
/// ratios (≈18% at 70–74, ≈8% at 75+); the former 3%/m at 68–74 and forced retirement at 75 emptied the labour force
/// of over-70s (fix-demog).
pub fn retirement_hazard(age_months: u32) -> f64 {
    let years = age_months / 12;
    let birthday = age_months % 12 == 0;
    match years {
        55..=61 => 0.004,
        62 if birthday => 0.04,
        63..=66 => 0.015,
        67 if birthday => 0.08,
        68..=74 => RETIRE_68_74_MONTHLY,
        y if y >= 75 => RETIRE_75_PLUS_MONTHLY,
        _ => 0.0,
    }
}

/// Cumulative share of a cohort that has retired by each age in months (0..=1200), starting from the
/// §5 hazards at age 55. Used by `init` so the initial retired stock is consistent with the dynamics.
pub fn retired_share_table() -> Vec<f64> {
    let mut out = vec![0.0; (MAX_AGE + 1) as usize];
    let mut surv = 1.0;
    for a in 0..=MAX_AGE {
        out[a as usize] = 1.0 - surv;
        surv *= 1.0 - retirement_hazard(a);
    }
    out
}

/// A household record with all fields at neutral values; the caller fills it in with `fill_entrant`.
fn blank_household(id: HhId, bank: BankId) -> Household {
    Household {
        id,
        alive: true,
        age_months: ENTRANT_AGE as u16,
        education: Education::HighSchool,
        skill: 1.0,
        status: LaborStatus::Unemployed,
        employer: None,
        wage: 0.0,
        last_wage: 0.0,
        months_unemployed: 0,
        bls_unemployed: 0,
        months_in_job: 0,
        months_employed: 0,
        reservation_wage: 0.0,
        benefit_months_left: 0,
        career_earnings: 0.0,
        career_months: 0,
        pension: 0.0,
        bank,
        deposits: 0.0,
        shares: 0.0,
        tenure: Tenure::WithParents,
        home: None,
        properties: Vec::new(),
        rent: 0.0,
        lease_months_left: 0,
        housing_cost: 0.0,
        permanent_income: ENTRANT_PERMANENT_INCOME,
        expected_inflation: 0.0,
        consumption_budget: 0.0,
        preferred_firm: [0; N_CONSUMER],
        partnered: false,
    }
}

/// Cumulative headship by age: piecewise linear through `HEADSHIP_KNOTS`, 0 below 18 and flat above 40.
fn cum_headship(age: f64) -> f64 {
    let k = &HEADSHIP_KNOTS;
    if age < k[0].0 {
        return 0.0;
    }
    for w in k.windows(2) {
        let ((x0, y0), (x1, y1)) = (w[0], w[1]);
        if age <= x1 {
            return y0 + (y1 - y0) * (age - x0) / (x1 - x0);
        }
    }
    k[k.len() - 1].1
}

/// Headship mass of each entry age (whole years from `ENTRY_AGE_LO`). The 3% headed by 18 enter at 18.
fn headship_masses() -> [f64; N_ENTRY_AGES] {
    std::array::from_fn(|j| {
        let lo = f64::from(ENTRY_AGE_LO) + j as f64;
        let m = cum_headship(lo + 1.0) - cum_headship(lo);
        if j == 0 { m + cum_headship(lo) } else { m }
    })
}

/// Mean of `native_formation_index` over the `FORMATION_HORIZON_YEARS` from `y0` (fix-demog). The base rate is set so
/// that formation averages HH_FORMATION_ANNUAL over the horizon; calibrating to the first year alone let the falling
/// births index pull household growth below the target.
fn mean_formation_index(y0: i32) -> f64 {
    (0..FORMATION_HORIZON_YEARS).map(|k| native_formation_index(y0 + k)).sum::<f64>() / f64::from(FORMATION_HORIZON_YEARS)
}

/// Headship-weighted births index of native entrants in calendar year `y`: Σ_a mass(a) × births index of y − a. It is
/// the entry flow per unit of the base rate, so the base is calibrated against it (`step`).
fn native_formation_index(y: i32) -> f64 {
    let m = headship_masses();
    (0..N_ENTRY_AGES).map(|j| m[j] * birth_cohort_index(y - (ENTRY_AGE_LO as i32 + j as i32))).sum()
}

/// Cumulative entry-age distribution of native entrants in calendar year `y` (birth-weighted headship masses).
fn entry_age_cdf(y: i32) -> [f64; N_ENTRY_AGES] {
    let m = headship_masses();
    let mut w = [0.0; N_ENTRY_AGES];
    let mut acc = 0.0;
    for j in 0..N_ENTRY_AGES {
        acc += m[j] * birth_cohort_index(y - (ENTRY_AGE_LO as i32 + j as i32));
        w[j] = acc;
    }
    if acc > 0.0 {
        for x in w.iter_mut() {
            *x /= acc;
        }
    }
    w
}

/// Native entrant's age in years: an entry age bucket from `cdf` and a uniform fraction of its year.
fn draw_native_entry_age(rng: &mut SimRng, cdf: &[f64; N_ENTRY_AGES]) -> f64 {
    let u: f64 = rng.gen();
    let j = cdf.iter().position(|&c| u < c).unwrap_or(N_ENTRY_AGES - 1);
    f64::from(ENTRY_AGE_LO) + j as f64 + rng.gen::<f64>()
}

/// Net immigrant head's age in years, from `IMMIG_AGE_BANDS`.
fn draw_immigrant_age(rng: &mut SimRng) -> f64 {
    let u: f64 = rng.gen();
    let (_, lo, hi) = IMMIG_AGE_BANDS.iter().copied().find(|b| u < b.0).unwrap_or(IMMIG_AGE_BANDS[IMMIG_AGE_BANDS.len() - 1]);
    lo + (hi - lo) * rng.gen::<f64>()
}

/// Reset an agent's personal fields as a new household head of `age_years` (DESIGN §5, §16). Does not touch
/// `deposits`, `shares`, `bank` or `id`, which are money/ledger fields owned elsewhere.
fn fill_entrant(
    hh: &mut Household,
    cfg: &Config,
    rng: &mut SimRng,
    infl: f64,
    firms_by_sector: &[Vec<FirmId>],
    age_years: f64,
    nom: &crate::fiscal::Nominal,
) {
    let u: f64 = rng.gen();
    hh.education = if u < 0.55 {
        Education::HighSchool
    } else if u < 0.90 {
        Education::College
    } else {
        Education::Graduate
    };
    hh.skill = rng::lognormal_mean1(rng, cfg.distquick_entrant_skill_sigma) as f32;
    hh.partnered = rng.gen::<f64>() < ENTRANT_PARTNERED_SHARE;
    hh.alive = true;
    hh.age_months = (age_years * 12.0) as u16;
    // Still at school until the schooling end of their education; otherwise finished, looking for work.
    let schooling_end = match hh.education {
        Education::HighSchool => 0,
        Education::College => COLLEGE_END,
        Education::Graduate => GRAD_END,
    };
    hh.status = if u32::from(hh.age_months) < schooling_end { LaborStatus::Student } else { LaborStatus::Unemployed };
    hh.employer = None;
    hh.wage = 0.0;
    hh.last_wage = 0.0;
    hh.months_unemployed = 0;
    hh.bls_unemployed = 0;
    hh.months_in_job = 0;
    hh.months_employed = 0;
    // Reservation wage ≈ 0.8 × typical entry wage at the entrant's experience (Mincer x from schooling end).
    let hc = common::human_capital(cfg, hh);
    hh.reservation_wage = (0.8 * nom.min_wage.max(ENTRY_WAGE_BASE * hc * nom.entry_scale)) as f32;
    hh.benefit_months_left = 0;
    // Career history before arrival is imputed as in `init` (earnings from age 20 at the entry wage).
    let career_months = ((age_years - 20.0).clamp(0.0, 45.0) * 12.0) as u32;
    hh.career_months = career_months as u16;
    hh.career_earnings = if hh.status == LaborStatus::Student { 0.0 } else { ENTRY_WAGE_BASE * hc * 0.8 * career_months as f64 };
    hh.pension = 0.0;
    hh.tenure = Tenure::WithParents;
    hh.home = None;
    hh.properties.clear();
    hh.rent = 0.0;
    hh.lease_months_left = 0;
    hh.housing_cost = 0.0;
    hh.permanent_income = ENTRANT_PERMANENT_INCOME;
    hh.expected_inflation = infl;
    hh.consumption_budget = 0.0;
    for s in 0..N_CONSUMER {
        let list = &firms_by_sector[s];
        if !list.is_empty() {
            hh.preferred_firm[s] = list[rng.gen_range(0..list.len())];
        }
    }
}

/// Starting money for a new entrant: a random alive household aged 40–64 with deposits > $20k pays
/// LogNormal(mean $3k) (capped at half its deposits). Starts with nothing if no parent is found.
fn endow_from_parent(w: &mut World, st: &DemographyState, child: HhId, rng: &mut SimRng) {
    if st.parents.is_empty() {
        return;
    }
    let amount = ENTRANT_MONEY_MEAN * rng::lognormal_mean1(rng, ENTRANT_MONEY_SIGMA);
    for _ in 0..16 {
        let p = st.parents[rng.gen_range(0..st.parents.len())];
        let ph = &w.households[p as usize];
        if ph.alive && p != child && (PARENT_LO..PARENT_HI).contains(&u32::from(ph.age_months)) && ph.deposits > PARENT_MIN_DEPOSITS {
            let x = amount.min(0.5 * ph.deposits);
            ledger::pay(w, Account::Household(p), Account::Household(child), x);
            return;
        }
    }
}

/// Calendar year of the current month: the reported run starts in `BASE_YEAR`, and burn-in months precede it.
fn calendar_year(w: &World) -> i32 {
    let months = i64::from(w.tick) + i64::from(w.tick_offset) - i64::from(w.cfg.burn_in_months);
    crate::init::BASE_YEAR + months.div_euclid(12) as i32
}

/// Births in the year `y`, relative to the birth cohort of `BASE_COHORT_YEAR`. Years outside the table are clamped.
fn birth_cohort_index(y: i32) -> f64 {
    let at = |y: i32| US_BIRTHS_M[(y.clamp(1990, 2025) - 1990) as usize];
    at(y) / at(BASE_COHORT_YEAR)
}

/// Rebuild the firms-by-consumer-sector index used for preferred-firm draws of new entrants.
fn index_firms(w: &World, st: &mut DemographyState) {
    st.firms_by_sector.resize_with(N_CONSUMER, Vec::new);
    for v in st.firms_by_sector.iter_mut() {
        v.clear();
    }
    for f in &w.firms {
        if f.alive && f.sector.idx() < N_CONSUMER {
            st.firms_by_sector[f.sector.idx()].push(f.id);
        }
    }
}

/// Merge the chunk indices in chunk order (heir buckets, parent pool, alive count, mean inflation).
fn merge_index(w: &World, st: &mut DemographyState) {
    st.heirs.resize_with(HEIR_BUCKETS, Vec::new);
    for b in st.heirs.iter_mut() {
        b.clear();
    }
    st.parents.clear();
    let mut infl_sum = 0.0;
    let mut n_alive = 0usize;
    for c in &st.chunks {
        for &(b, id) in &c.heirs {
            st.heirs[b as usize].push(id);
        }
        st.parents.extend_from_slice(&c.parents);
        n_alive += c.alive;
        infl_sum += c.infl;
    }
    st.n_alive = n_alive;
    st.infl_mean = if n_alive > 0 && infl_sum.is_finite() {
        infl_sum / n_alive as f64
    } else {
        w.cfg.initial_inflation
    };
}

/// Heir draws per age bucket for the wealth-matched choice (`distquick_heir_match`, plan item 14).
const HEIR_DRAWS: usize = 8;
/// Penalty, in log-wealth units, for an heir with a different education level than the decedent (matched choice).
/// Education is a weaker wealth predictor than wealth itself, so it only breaks near-ties in wealth.
const HEIR_EDU_PENALTY: f64 = 0.25;

/// Choose an heir: alive, aged 25–60, nearest to the deceased's age − 30; else any alive household.
/// Stale heir-index entries are dropped lazily. Returns None if nobody suitable is found.
fn pick_heir(w: &World, st: &mut DemographyState, dead: HhId, age_months: u32, rng: &mut SimRng) -> Option<HhId> {
    let target = ((age_months / 12) as i64 - 30).clamp(25, 60) - 25;
    for d in 0..HEIR_BUCKETS as i64 {
        for b in [target + d, target - d] {
            if b < 0 || b >= HEIR_BUCKETS as i64 {
                continue;
            }
            let bucket = &mut st.heirs[b as usize];
            let mut tries = 0;
            while !bucket.is_empty() && tries < 8 {
                let k = rng.gen_range(0..bucket.len());
                let h = bucket[k];
                let hh = &w.households[h as usize];
                if hh.alive && h != dead && (HEIR_LO..HEIR_HI).contains(&u32::from(hh.age_months)) {
                    return Some(h);
                }
                bucket.swap_remove(k);
                tries += 1;
            }
        }
    }
    random_alive_household(w, dead, rng)
}

/// Wealth-matched heir (`distquick_heir_match`, plan item 14): within the nearest age bucket that has one, up to
/// `HEIR_DRAWS` random valid candidates are drawn and the one closest in log net worth to the deceased is chosen (rich
/// estates pass to rich heirs, poor to poor), with `HEIR_EDU_PENALTY` for a different education level. Net worth
/// excludes owned-firm equity. Bucket order and the fallback are as in `pick_heir`.
fn pick_heir_matched(
    w: &World,
    st: &mut DemographyState,
    dead: HhId,
    age_months: u32,
    rng: &mut SimRng,
) -> Option<HhId> {
    let target = ((age_months / 12) as i64 - 30).clamp(25, 60) - 25;
    let dead_hh = &w.households[dead as usize];
    let edu = dead_hh.education;
    let dead_lw = (common::net_worth(w, dead, &[]).max(0.0) + 1.0).ln();
    for d in 0..HEIR_BUCKETS as i64 {
        for b in [target + d, target - d] {
            if b < 0 || b >= HEIR_BUCKETS as i64 {
                continue;
            }
            let bucket = &mut st.heirs[b as usize];
            let mut tries = 0;
            let mut best: Option<(f64, HhId)> = None;
            while !bucket.is_empty() && tries < HEIR_DRAWS {
                let k = rng.gen_range(0..bucket.len());
                let h = bucket[k];
                let hh = &w.households[h as usize];
                if hh.alive && h != dead && (HEIR_LO..HEIR_HI).contains(&u32::from(hh.age_months)) {
                    let lw = (common::net_worth(w, h, &[]).max(0.0) + 1.0).ln();
                    let score = (lw - dead_lw).abs() + if hh.education == edu { 0.0 } else { HEIR_EDU_PENALTY };
                    if best.is_none_or(|(s, _)| score < s) {
                        best = Some((score, h));
                    }
                    tries += 1;
                    continue;
                }
                bucket.swap_remove(k);
                tries += 1;
            }
            if let Some((_, h)) = best {
                return Some(h);
            }
        }
    }
    random_alive_household(w, dead, rng)
}

/// Fallback heir: a random alive household other than `dead` (32 attempts), or None.
fn random_alive_household(w: &World, dead: HhId, rng: &mut SimRng) -> Option<HhId> {
    let n = w.households.len();
    for _ in 0..32 {
        let h = rng.gen_range(0..n) as HhId;
        if h != dead && w.households[h as usize].alive {
            return Some(h);
        }
    }
    None
}

/// One heir draw for `process_death`: the wealth-matched rule when `matched`, else the base random rule.
fn draw_heir(w: &World, st: &mut DemographyState, dead: HhId, age_months: u32, matched: bool, rng: &mut SimRng) -> Option<HhId> {
    if matched {
        pick_heir_matched(w, st, dead, age_months, rng)
    } else {
        pick_heir(w, st, dead, age_months, rng)
    }
}

/// Settle a death: separate from employer, pass deposits, shares, houses and mortgages to the heir, and vacate the
/// home. The slot is then left empty (no money, housing or job) until an entrant reuses it. Returns false (agent stays
/// alive this month) if no heir can be found.
///
/// With `distquick_estate_heirs` = k > 1 (plan item 14) the estate is split among up to k distinct heirs: deposits
/// and Fund shares in equal parts (the last heir takes the remainder, so the decedent's balance ends at exactly zero),
/// houses and their mortgages round-robin in ownership order (a single house passes whole to one heir). Fewer heirs
/// are used if fewer distinct ones are found in a few attempts.
fn process_death(w: &mut World, st: &mut DemographyState, id: HhId, rng: &mut SimRng) -> bool {
    let age = u32::from(w.households[id as usize].age_months);
    let k = w.cfg.distquick_estate_heirs.max(1);
    let matched = w.cfg.distquick_heir_match;
    let Some(first) = draw_heir(w, st, id, age, matched, rng) else {
        return false;
    };
    let mut heirs = vec![first];
    let mut attempts = 0;
    while heirs.len() < k && attempts < 4 * k {
        attempts += 1;
        if let Some(h) = draw_heir(w, st, id, age, matched, rng) {
            if !heirs.contains(&h) {
                heirs.push(h);
            }
        }
    }
    let n = heirs.len();
    common::separate_worker(w, id, LaborStatus::Unemployed);
    w.households[id as usize].alive = false;

    // Deposits: positive balances inherited; overdrafts settled by the heirs.
    let dep = w.households[id as usize].deposits;
    for (i, &heir) in heirs.iter().enumerate() {
        let amount = if i + 1 == n { w.households[id as usize].deposits } else { dep / n as f64 };
        if amount > 0.0 {
            ledger::pay(w, Account::Household(id), Account::Household(heir), amount);
        } else if amount < 0.0 {
            ledger::pay(w, Account::Household(heir), Account::Household(id), -amount);
        }
    }
    if n == 1 {
        ledger::transfer_shares(w, id, heirs[0]);
    } else {
        let shares = w.households[id as usize].shares;
        for (i, &heir) in heirs.iter().enumerate() {
            let amount = if i + 1 == n { w.households[id as usize].shares } else { shares / n as f64 };
            w.households[id as usize].shares -= amount;
            w.households[heir as usize].shares += amount;
        }
    }
    let props = w.households[id as usize].properties.clone();
    for (i, p) in props.into_iter().enumerate() {
        let heir = heirs[i % n];
        ledger::transfer_house(w, p, Owner::Household(heir));
        if let Some(l) = w.houses[p as usize].mortgage {
            ledger::reassign_loan(w, l, Borrower::Household(heir));
        }
    }
    // Vacate the dead agent's home (owned houses already passed to the heirs; rentals are simply let go).
    if let Some(home) = w.households[id as usize].home {
        let hs = &mut w.houses[home as usize];
        if hs.occupant == Some(id) {
            hs.occupant = None;
            hs.rent = 0.0;
        }
    }
    let hh = &mut w.households[id as usize];
    hh.alive = false;
    hh.home = None;
    hh.tenure = Tenure::WithParents;
    hh.properties.clear();
    hh.rent = 0.0;
    hh.lease_months_left = 0;
    hh.housing_cost = 0.0;
    hh.employer = None;
    hh.wage = 0.0;
    hh.status = LaborStatus::OutOfLaborForce;
    hh.pension = 0.0;
    hh.consumption_budget = 0.0;
    true
}

/// Settings of one month's household pass, shared by all chunks.
struct PassCtx<'a> {
    cfg: &'a Config,
    /// Round 7 step 1: nominal dollar parameters (entrant reservation wage).
    nom: crate::fiscal::Nominal,
    seed: u64,
    tick: u32,
    /// Burn-in: ages, school completion, partnership, retirement, death and entry are frozen. Labour-force flows run.
    frozen: bool,
    /// Monthly probabilities of partnership formation and of separation.
    p_form: f64,
    p_sep: f64,
    /// b6-reentry M4b: spell memory per household (see `LaborState::reentry_prev`). Ids past the end mean none.
    reentry_prev: &'a [u16],
    reentry_n: &'a [u16],
    /// b6-reentry M4d: tightness factor for attachment (1 when off), from `labor::theta_factor`.
    att_theta: f64,
}

/// Re-entry hazard class of an out-of-labour-force household (b6-reentry M4b/M4c).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ReentryClass {
    Base,
    Recent,
    Scar,
}

/// Monthly re-entry hazard into unemployment for an out-of-labour-force household with spell memory `prev` (months
/// unemployed before its last exit, 0 = none) and `n` months out since that exit (b6-reentry M4b/M4c):
/// - `udur_reentry_recent` while n ≤ the window, for a household with a memory and a positive recent hazard;
/// - OLF_OUTFLOW × `udur_reentry_scar` once a long spell (≥ `udur_reentry_scar_spell`) has been out past the window;
/// - OLF_OUTFLOW otherwise. At the defaults this is exactly the legacy hazard.
fn reentry_hazard(cfg: &Config, prev: u16, n: u16) -> (f64, ReentryClass) {
    if prev > 0 && n <= cfg.udur_reentry_window && cfg.udur_reentry_recent > 0.0 {
        return (cfg.udur_reentry_recent, ReentryClass::Recent);
    }
    if prev > 0 && prev >= cfg.udur_reentry_scar_spell && n > cfg.udur_reentry_window {
        return (OLF_OUTFLOW * cfg.udur_reentry_scar, ReentryClass::Scar);
    }
    (OLF_OUTFLOW, ReentryClass::Base)
}

/// Spell memory of household `id` from a per-household slice (no entry means none).
#[inline]
fn memory_of(s: &[u16], id: HhId) -> u16 {
    s.get(id as usize).copied().unwrap_or(0)
}

/// Monthly probability equivalent to an annual rate.
fn monthly_rate(annual: f64) -> f64 {
    1.0 - (1.0 - annual).powf(1.0 / 12.0)
}

/// One household's monthly pass (DESIGN §5), run in parallel. Ages the household, records its index entries, and
/// performs the transitions that touch only its own fields (school completion, partnership, loss of a partner,
/// labour-force re-entry). The labour-force inflow, retirement and death draws are made from the household's own
/// generator in the original order; their effects (firm worker lists, ledger, heirs) are applied later in
/// `apply_events`.
fn monthly_pass(hh: &mut Household, id: HhId, ctx: &PassCtx, out: &mut ChunkOut) {
    if !hh.alive {
        return;
    }
    let cfg = ctx.cfg;
    if !ctx.frozen {
        hh.age_months += 1;
    }
    let age = u32::from(hh.age_months);
    out.alive += 1;
    out.infl += hh.expected_inflation;
    if (HEIR_LO..HEIR_HI).contains(&age) {
        out.heirs.push((((age / 12) - 25) as u8, id));
    }
    if (PARENT_LO..PARENT_HI).contains(&age) && hh.deposits > PARENT_MIN_DEPOSITS {
        out.parents.push(id);
    }

    let mut rng = rng::agent_rng(ctx.seed, ctx.tick, stream::DEMOGRAPHY, id as u64);

    if !ctx.frozen {
        // a. School completion (Student -> job seeker).
        if hh.status == LaborStatus::Student {
            let done = match hh.education {
                Education::College => age >= COLLEGE_END,
                Education::Graduate => age >= GRAD_END,
                Education::HighSchool => true,
            };
            if done {
                let hc = common::human_capital(cfg, hh);
                let reservation = 0.8 * ctx.nom.min_wage.max(ENTRY_WAGE_BASE * hc * ctx.nom.entry_scale);
                hh.status = LaborStatus::Unemployed;
                hh.months_unemployed = 0;
                hh.bls_unemployed = 0;
                hh.benefit_months_left = 0;
                hh.reservation_wage = reservation as f32;
            }
        }
        // a2. Partnership: single heads aged 20–59 form one; partnered heads under 65 separate.
        if hh.partnered {
            if age < PARTNER_SEP_AGE_HI && rng.gen::<f64>() < ctx.p_sep {
                hh.partnered = false;
            }
        } else if (PARTNER_FORM_AGE_LO..PARTNER_FORM_AGE_HI).contains(&age) && rng.gen::<f64>() < ctx.p_form {
            hh.partnered = true;
        }
    }

    // b. Labour-force exits and re-entry. Exits are applied later (they touch the employer's worker list).
    let mut olf_in = false;
    match hh.status {
        LaborStatus::Employed | LaborStatus::Unemployed if (PRIME_LO..PRIME_HI).contains(&age) => {
            olf_in = rng.gen::<f64>() < OLF_INFLOW;
        }
        LaborStatus::OutOfLaborForce => {
            // b6-reentry M4b/M4c: the hazard depends on spell memory; the single draw is unchanged.
            let prev = memory_of(ctx.reentry_prev, id);
            let n = memory_of(ctx.reentry_n, id);
            let (hz, class) = if cfg.udur_att_on {
                // M4d: attachment replaces the M4c classes; the same single draw decides the exit.
                let a = crate::labor::attachment(cfg, prev, n, ctx.att_theta);
                let class = if a < 1.0 { ReentryClass::Scar } else { ReentryClass::Base };
                (OLF_OUTFLOW * cfg.udur_att_base_mult * a, class)
            } else {
                reentry_hazard(cfg, prev, n)
            };
            if rng.gen::<f64>() < hz {
                hh.status = LaborStatus::Unemployed;
                hh.months_unemployed = 0;
                hh.bls_unemployed = 0;
                hh.benefit_months_left = 0;
                match class {
                    ReentryClass::Recent => out.re_recent += 1,
                    ReentryClass::Scar => out.re_scar += 1,
                    ReentryClass::Base => {}
                }
                if cfg.udur_resume_spell && prev > 0 {
                    hh.months_unemployed = prev;
                    out.resumed += 1;
                }
            }
        }
        _ => {}
    }

    // c. Retirement (not for students or those already retired). Students and retirees are excluded by status,
    // which the labour-force exit above does not change.
    let mut retire = false;
    if !ctx.frozen && !matches!(hh.status, LaborStatus::Student | LaborStatus::Retired) {
        let hz = retirement_hazard(age);
        retire = hz > 0.0 && rng.gen::<f64>() < hz;
    }

    // d. Mortality (Gompertz–Makeham, forced at 100). A partnered household cannot dissolve: the death of a partner
    // leaves a single household, which carries on with the same money, home and job.
    let mut dies = false;
    if !ctx.frozen {
        let mort = common::mortality_monthly(age as f64 / 12.0);
        if hh.partnered {
            if age >= MAX_AGE || rng.gen::<f64>() < (PARTNER_LOSS_MULT * mort).min(1.0) {
                hh.partnered = false;
            }
        } else {
            dies = age >= MAX_AGE || rng.gen::<f64>() < mort;
        }
    }

    if olf_in || retire || dies {
        out.events.push(Event {
            id,
            olf_in,
            retire,
            rng: dies.then_some(rng),
        });
    }
}

/// Apply this month's rare events in household order: labour-force exits, retirements, then deaths. Deaths draw
/// from the generator carried in the event; the dead slot is refilled later by `add_entrants`.
fn apply_events(w: &mut World, st: &mut DemographyState) -> Vec<HhId> {
    let mut chunks = std::mem::take(&mut st.chunks);
    let mut deaths: Vec<HhId> = Vec::new();
    for c in chunks.iter_mut() {
        for ev in c.events.drain(..) {
            let id = ev.id;
            let i = id as usize;
            if ev.olf_in {
                common::separate_worker(w, id, LaborStatus::OutOfLaborForce);
            }
            if ev.retire {
                common::separate_worker(w, id, LaborStatus::Retired);
                // Integration (fix-fiscal): no benefit is set here. Claiming (62+, age-based hazard) lives in fiscal.rs.
                w.flows.retirements += 1;
            }
            if let Some(mut rng) = ev.rng {
                if process_death(w, st, id, &mut rng) {
                    deaths.push(id);
                    w.flows.deaths += 1;
                    w.hh_month[i] = HhMonth::default();
                }
            }
        }
    }
    st.chunks = chunks;
    deaths
}

/// Create this month's entrants (DESIGN §5). Native households arrive at the base rate scaled by
/// `native_formation_index` (headship-weighted births), at an age drawn from the headship schedule for that year.
/// Net immigrants (`Config::immigration_annual`) arrive at CBO net-migrant ages, finished with schooling and looking
/// for work. Each takes the lowest dead slot, and a new slot is appended only when none is free.
fn add_entrants(w: &mut World, st: &mut DemographyState) {
    let base = st.base_entrants.unwrap_or(0.0);
    // Household formation is procyclical (young adults form households when they find jobs; US formation fell from
    // ≈1.3M to ≈0.5M a year in 2008–11 and then caught up): formation this month is the cohort-driven flow plus a
    // share of the postponed backlog, scaled by exp(β × (u* − u)). The backlog keeps long-run formation unchanged.
    let mut due = base * native_formation_index(calendar_year(w));
    // Re-fit Step 6: native entry also replaces the change in the dissolutions of single households since calibration
    // (floored at zero). Legacy: the base rate alone.
    if w.cfg.native_entry_tracks_dissolutions {
        due = (due + (st.diss_now - st.diss0)).max(0.0);
    }
    let beta = w.cfg.household_formation_cyclicality;
    let formed = if beta > 0.0 {
        let gap_pp = 100.0 * (w.natural_unemployment() - w.unemployment_rate());
        let m = (beta * gap_pp).exp().clamp(0.3, 1.6);
        let f = ((due + PENT_UP_RELEASE * st.pent_up) * m).max(0.0);
        st.pent_up = (st.pent_up + due - f).clamp(-24.0 * base, 24.0 * base);
        f
    } else {
        due
    };
    st.native_acc += formed;
    let n_native = st.native_acc.floor().max(0.0);
    st.native_acc -= n_native;
    st.immigrant_acc += st.n_alive as f64 * w.cfg.immigration_annual / 12.0;
    let n_immigrant = st.immigrant_acc.floor().max(0.0);
    st.immigrant_acc -= n_immigrant;
    let (n_native, n_immigrant) = (n_native as usize, n_immigrant as usize);
    if n_native + n_immigrant == 0 {
        return;
    }
    // Dead slots in index order: entrant j takes free[j] while any remain.
    let free: Vec<usize> = (0..w.households.len()).filter(|&i| !w.households[i].alive).collect();
    let mut mrng = rng::module_rng(w.seed, w.tick, stream::DEMOGRAPHY);
    let cdf = entry_age_cdf(calendar_year(w));
    let nom = crate::fiscal::nominal(w);
    for j in 0..n_native + n_immigrant {
        let native = j < n_native;
        let i = match free.get(j) {
            Some(&slot) => slot,
            None => {
                let len = w.households.len();
                let mut bank: BankId = 0;
                for _ in 0..32 {
                    let k = mrng.gen_range(0..len);
                    if w.households[k].alive {
                        bank = w.households[k].bank;
                        break;
                    }
                }
                w.households.push(blank_household(len as HhId, bank));
                w.hh_month.push(HhMonth::default());
                len
            }
        };
        let id = i as HhId;
        crate::housing::reset_household(&mut w.housing, id);
        w.hh_month[i] = HhMonth::default();
        if w.cfg.entrant_slot_reset {
            reset_slot_memory(w, id);
        }
        let age = if native {
            draw_native_entry_age(&mut mrng, &cdf)
        } else {
            draw_immigrant_age(&mut mrng)
        };
        let hh = &mut w.households[i];
        fill_entrant(hh, &w.cfg, &mut mrng, st.infl_mean, &st.firms_by_sector, age, &nom);
        if !native && hh.status == LaborStatus::Student {
            hh.status = LaborStatus::Unemployed;
        }
        endow_from_parent(w, st, id, &mut mrng);
        w.flows.births += 1;
    }
}

/// `Config::entrant_slot_reset`: clear the per-slot memory the other modules keep for slot `h`, which an entrant is
/// taking (housing bookkeeping and `hh_month` are cleared by `add_entrants` in any case). Without it, an entrant in a slot
/// freed this month is taxed on the decedent's last-month distributions, spends at the capital-income MPC out of the
/// decedent's capital income, and keeps the decedent's windfall stock, UI-spell flag and impatience type when it is not
/// younger, plus the job-search sector and pay multiplier. The fiscal state is also cleared here, so the birth-month test
/// in `fiscal::occupant_changes` is not needed for it (it misses an entrant born in the decedent's month). Slots appended
/// this month have no entries yet, and the vectors grow with zeros. The estate itself passed to the heirs at death.
fn reset_slot_memory(w: &mut World, h: HhId) {
    let i = h as usize;
    crate::consumption::reset_household(&mut w.consumption, h);
    crate::labor::reset_household(&mut w.labor, h);
    let f = &mut w.fiscal;
    for v in [&mut f.hh_arrears, &mut f.ytd_wages, &mut f.fund_shares_prev, &mut f.fund_basis, &mut f.cg_loss_cf] {
        if let Some(x) = v.get_mut(i) {
            *x = 0.0;
        }
    }
}

/// True while `init::build_world` is settling the economy, before the reported run's tick reset. A world built with
/// `burn_in_months = 0` is never in burn-in.
fn in_burn_in(w: &World) -> bool {
    w.tick_offset == 0 && w.tick < w.cfg.burn_in_months
}

/// Advance demographics by one month (DESIGN §5). During burn-in only labour-force flows run.
pub fn step(w: &mut World) {
    let mut st = std::mem::take(&mut w.demography);
    let frozen = in_burn_in(w);
    let ctx = PassCtx {
        cfg: &w.cfg,
        nom: crate::fiscal::nominal(w),
        seed: w.seed,
        tick: w.tick,
        frozen,
        p_form: monthly_rate(PARTNER_FORM_ANNUAL),
        p_sep: monthly_rate(PARTNER_SEP_ANNUAL),
        reentry_prev: &w.labor.reentry_prev,
        reentry_n: &w.labor.reentry_n,
        att_theta: crate::labor::theta_factor(&w.cfg, &w.labor),
    };

    // 1. One parallel pass over fixed-size chunks: age everyone alive, record index entries, and draw the
    //    per-household transitions. The index is merged in chunk order, so it equals a sequential build.
    let n0 = w.households.len();
    st.chunks.resize_with(n0.div_ceil(CHUNK), ChunkOut::default);
    w.households
        .par_chunks_mut(CHUNK)
        .zip(st.chunks.par_iter_mut())
        .enumerate()
        .for_each(|(c, (chunk, out))| {
            out.clear();
            for (j, hh) in chunk.iter_mut().enumerate() {
                monthly_pass(hh, (c * CHUNK + j) as HhId, &ctx, out);
            }
        });
    for c in &st.chunks {
        w.flows.udur_resumed += c.resumed;
        w.flows.udur_reentry_recent_n += c.re_recent;
        w.flows.udur_reentry_scar_n += c.re_scar;
    }
    merge_index(w, &mut st);
    index_firms(w, &mut st);
    if !frozen {
        // Re-fit Step 6: expected dissolutions of single households (their heads' mortality, as partnered households do
        // not dissolve), recomputed each month. The calibration month's value is `diss0`.
        let dissolutions: f64 = w
            .households
            .iter()
            .filter(|h| h.alive && !h.partnered)
            .map(|h| common::mortality_monthly(h.age_years()))
            .sum();
        st.diss_now = dissolutions;
        if st.base_entrants.is_none() {
            // Native entry at the first post-burn-in month: the expected dissolutions plus native formation of
            // `hh_formation_annual` a year.
            st.diss0 = dissolutions;
            let target = dissolutions + st.n_alive as f64 * w.cfg.hh_formation_annual / 12.0;
            st.base_entrants = Some(target / mean_formation_index(calendar_year(w)));
        }
    }

    // 2. Rare events (labour-force exits, retirements, deaths), in household order.
    let mut deaths = apply_events(w, &mut st);
    // A dead household's firms are no longer owner-operated by it.
    if !deaths.is_empty() {
        deaths.sort_unstable();
        for fm in w.firms.iter_mut() {
            if fm.owner.is_some_and(|o| deaths.binary_search(&o).is_ok()) {
                fm.owner = None;
            }
        }
    }

    // 3. Entry: native households and immigrants, into dead slots first. Frozen during burn-in.
    if !frozen {
        add_entrants(w, &mut st);
    }

    w.demography = st;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::tiny_world;

    fn world(seed: u64) -> World {
        tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            seed,
        )
    }

    #[test]
    fn runs_consistently_and_records_flows() {
        let mut w = world(11);
        let n0 = w.households.len();
        for _ in 0..240 {
            step(&mut w);
            w.tick += 1;
            ledger::check_consistency(&w).unwrap();
        }
        assert!(w.flows.deaths > 0, "expected deaths over 20 years");
        assert!(w.flows.births >= w.flows.deaths, "births must replace deaths");
        assert!(w.flows.retirements > 0, "expected retirements");
        // Every death empties one alive slot and every entrant fills one, so the alive stock follows the flows.
        let alive = w.households.iter().filter(|h| h.alive).count();
        let expected_alive = n0 + w.flows.births as usize - w.flows.deaths as usize;
        assert_eq!(alive, expected_alive, "alive stock must follow entry and deaths");
        // Entrants include the net immigration of about immigration_annual × households per year.
        let immigrants_expected = n0 as f64 * w.cfg.immigration_annual * 20.0;
        assert!(w.flows.births as f64 > immigrants_expected, "entrants {} below immigration", w.flows.births);
        assert!(w.households.iter().all(|h| h.age_months >= 216 && u32::from(h.age_months) <= MAX_AGE + 1));
    }

    /// Plan item 14 switches on: estates split among 2–3 heirs with the wealth-matched rule. The ledger must stay exact
    /// every month, the decedents must be left with no deposits, shares or houses, and no heir may be dead.
    #[test]
    fn estate_split_keeps_ledger_and_leaves_decedents_empty() {
        for k in [2usize, 3] {
            let mut w = tiny_world(
                Config {
                    n_households: 500,
                    distquick_estate_heirs: k,
                    distquick_heir_match: true,
                    distquick_kink_wage_indexed: true,
                    distquick_entrant_skill_sigma: 0.45,
                    ..Default::default()
                },
                23,
            );
            let mut dead_seen = 0usize;
            for _ in 0..240 {
                let before: Vec<bool> = w.households.iter().map(|h| h.alive).collect();
                step(&mut w);
                w.tick += 1;
                ledger::check_consistency(&w).unwrap();
                // Entrants may append slots, so only the households alive before this month are checked.
                for (i, h) in w.households.iter().enumerate().take(before.len()) {
                    if before[i] && !h.alive {
                        dead_seen += 1;
                        assert_eq!(h.deposits, 0.0, "decedent {i} keeps deposits");
                        assert_eq!(h.shares, 0.0, "decedent {i} keeps shares");
                        assert!(h.properties.is_empty(), "decedent {i} keeps houses");
                    }
                }
                assert!(w.houses.iter().all(|h| match h.owner {
                    Owner::Household(o) => w.households[o as usize].alive,
                    _ => true,
                }));
            }
            assert!(dead_seen > 0, "expected deaths over 20 years (k = {k})");
        }
    }

    #[test]
    fn houses_and_employment_follow_living_owners() {
        let mut w = world(5);
        for _ in 0..180 {
            step(&mut w);
            w.tick += 1;
        }
        for h in &w.houses {
            if let Owner::Household(o) = h.owner {
                let hh = &w.households[o as usize];
                assert!(hh.alive, "house {} owned by dead household {}", h.id, o);
                assert!(hh.properties.contains(&h.id));
            }
        }
        for hh in w.households.iter().filter(|h| h.alive) {
            if let Some(f) = hh.employer {
                assert!(w.firms[f as usize].workers.contains(&hh.id));
            }
            assert!(hh.deposits.is_finite() && hh.permanent_income.is_finite());
        }
        for (_, l) in w.loans.iter() {
            if let Borrower::Household(b) = l.borrower {
                assert!(w.households[b as usize].alive, "loan {} held by dead household", l.id);
            }
        }
    }

    #[test]
    fn out_of_labour_force_stock_near_three_percent() {
        let mut w = world(9);
        for _ in 0..300 {
            step(&mut w);
            w.tick += 1;
        }
        let prime: Vec<&Household> = w
            .households
            .iter()
            .filter(|h| h.alive && (PRIME_LO..PRIME_HI).contains(&u32::from(h.age_months)))
            .collect();
        let olf = prime.iter().filter(|h| h.status == LaborStatus::OutOfLaborForce).count() as f64;
        let share = olf / prime.len() as f64;
        assert!((0.01..0.05).contains(&share), "OLF share {share}");
    }

    #[test]
    fn hazards_and_retired_table_are_sane() {
        assert_eq!(retirement_hazard(62 * 12), 0.04);
        assert_eq!(retirement_hazard(62 * 12 + 1), 0.0);
        assert_eq!(retirement_hazard(68 * 12), RETIRE_68_74_MONTHLY);
        assert_eq!(retirement_hazard(75 * 12), RETIRE_75_PLUS_MONTHLY);
        assert_eq!(retirement_hazard(90 * 12 + 5), RETIRE_75_PLUS_MONTHLY);
        let t = retired_share_table();
        assert!(t.windows(2).all(|p| p[1] >= p[0]));
        assert!(t.iter().all(|x| (0.0..=1.0).contains(x)));
        let r70 = t[70 * 12];
        assert!((0.75..0.95).contains(&r70), "retired by 70: {r70}");
    }

    #[test]
    fn burn_in_freezes_demography_but_not_labour_force_flows() {
        let mut w = world(4);
        w.cfg.burn_in_months = 24;
        let n0 = w.households.len();
        let ages0: Vec<u16> = w.households.iter().map(|h| h.age_months).collect();
        let partnered0: Vec<bool> = w.households.iter().map(|h| h.partnered).collect();
        for _ in 0..12 {
            // Each month's status changes must involve the labour-force state (entry to or exit from it).
            let before: Vec<LaborStatus> = w.households.iter().map(|h| h.status).collect();
            step(&mut w);
            w.tick += 1;
            for (h, &status) in w.households.iter().zip(&before) {
                if h.status != status {
                    assert!(
                        h.status == LaborStatus::OutOfLaborForce || status == LaborStatus::OutOfLaborForce,
                        "only labour-force flows may change status during burn-in: {status:?} -> {:?}",
                        h.status
                    );
                }
            }
        }
        assert_eq!(w.households.len(), n0);
        assert_eq!((w.flows.births, w.flows.deaths, w.flows.retirements), (0, 0, 0));
        for (h, (&age, &partnered)) in w.households.iter().zip(ages0.iter().zip(&partnered0)) {
            assert_eq!(h.age_months, age, "no ageing during burn-in");
            assert_eq!(h.partnered, partnered, "no partnership transitions during burn-in");
        }
    }

    #[test]
    fn partnered_households_survive_the_death_of_a_partner() {
        let mut w = world(6);
        for h in w.households.iter_mut() {
            h.partnered = true;
            h.age_months = 90 * 12;
        }
        step(&mut w);
        assert_eq!(w.flows.deaths, 0, "a partnered household never dissolves on death");
        let lost = w.households.iter().filter(|h| h.alive && !h.partnered).count();
        assert!(lost > 0, "some partners should be lost at age 90");
    }

    #[test]
    fn native_entry_ages_follow_headship_schedule() {
        let m = headship_masses();
        let total: f64 = m.iter().sum();
        assert!((total - HEADSHIP_KNOTS[HEADSHIP_KNOTS.len() - 1].1).abs() < 1e-9, "headship mass by 40: {total}");
        let mut cdf = [0.0; N_ENTRY_AGES];
        let mut acc = 0.0;
        for j in 0..N_ENTRY_AGES {
            acc += m[j] / total;
            cdf[j] = acc;
        }
        let mut r = rng::module_rng(7, 0, stream::DEMOGRAPHY);
        let ages: Vec<f64> = (0..20_000).map(|_| draw_native_entry_age(&mut r, &cdf)).collect();
        assert!(ages.iter().all(|&a| (18.0..40.0).contains(&a)));
        // Headship by 25 is 35% of a cohort, so 0.35 / 0.62 of new heads are under 25; mean entry age ≈ 25.4.
        let under25 = ages.iter().filter(|&&a| a < 25.0).count() as f64 / ages.len() as f64;
        assert!((0.53..0.60).contains(&under25), "native entrants under 25: {under25}");
        let mean = ages.iter().sum::<f64>() / ages.len() as f64;
        assert!((25.0..25.8).contains(&mean), "mean native entry age {mean}");
    }

    #[test]
    fn immigrant_ages_follow_cbo_profile() {
        let mut r = rng::module_rng(8, 0, stream::DEMOGRAPHY);
        let ages: Vec<f64> = (0..20_000).map(|_| draw_immigrant_age(&mut r)).collect();
        assert!(ages.iter().all(|&a| (22.0..65.0).contains(&a)));
        let over45 = ages.iter().filter(|&&a| a >= 45.0).count() as f64 / ages.len() as f64;
        assert!((0.13..0.17).contains(&over45), "immigrants over 45: {over45}");
        let mode = ages.iter().filter(|&&a| (25.0..35.0).contains(&a)).count() as f64 / ages.len() as f64;
        assert!((0.45..0.51).contains(&mode), "immigrants aged 25–34: {mode}");
    }

    #[test]
    fn household_stays_small() {
        // 176 bytes with four consumer sectors; the nine-sector model has eight consumer slots per household
        // (consumption and preferred firm), which adds 16 bytes with alignment. Round 7 step 6 adds the u16
        // continuous-employment count, which pads the record to 200 bytes.
        assert!(std::mem::size_of::<Household>() <= 200, "Household is {} bytes", std::mem::size_of::<Household>());
    }

    #[test]
    fn step_is_deterministic() {
        let mut a = world(3);
        let mut b = world(3);
        for _ in 0..30 {
            step(&mut a);
            step(&mut b);
            a.tick += 1;
            b.tick += 1;
        }
        assert_eq!(a.flows.deaths, b.flows.deaths);
        assert_eq!(a.flows.retirements, b.flows.retirements);
        for (x, y) in a.households.iter().zip(&b.households) {
            assert_eq!(x.age_months, y.age_months);
            assert_eq!(x.status, y.status);
        }
    }
}

#[cfg(test)]
mod entry_tracking_tests {
    use super::*;
    use crate::testkit::tiny_world;

    fn alive_count(w: &World) -> usize {
        w.households.iter().filter(|h| h.alive).count()
    }

    /// Runs one `add_entrants` with the given entry state on a world that has taken one demography step (so the
    /// household index exists), and returns the number of alive households it added.
    fn entrants_for(cfg: Config, base: f64, diss0: f64, diss_now: f64) -> usize {
        let mut w = tiny_world(cfg, 9);
        step(&mut w);
        let mut st = std::mem::take(&mut w.demography);
        st.base_entrants = Some(base);
        st.diss0 = diss0;
        st.diss_now = diss_now;
        let before = alive_count(&w);
        add_entrants(&mut w, &mut st);
        let added = alive_count(&w) - before;
        w.demography = st;
        added
    }

    /// Re-fit Step 6: with native entry tracking dissolutions, a fall in expected dissolutions since calibration cannot
    /// make due negative (no native entrants at a zero base), and a rise adds entrants. With the switch off the change
    /// is ignored, so the legacy path is unchanged.
    #[test]
    fn native_due_follows_dissolutions_and_is_floored_at_zero() {
        let cfg = Config { n_households: 500, immigration_annual: 0.0, ..Default::default() };
        assert_eq!(entrants_for(cfg.clone(), 0.0, 5.0, 0.0), 0, "a fall in dissolutions produced entrants");
        // A surplus of 30 expected dissolutions: fractional native entrants accumulate, and at least one arrives in
        // the month, since the due is 30 at the base of zero.
        assert!(entrants_for(cfg.clone(), 0.0, 0.0, 30.0) >= 1, "a rise in dissolutions produced no entrants");
        let off = Config { native_entry_tracks_dissolutions: false, ..cfg };
        assert_eq!(entrants_for(off, 0.0, 0.0, 30.0), 0, "the switch-off path changed entry");
    }

    /// `entrant_slot_reset`: an entrant placed in the slot of a household that died this month starts with none of the
    /// decedent's per-slot memory, so it pays no tax on the decedent's last-month dividends; its impatience type is
    /// redrawn although it is older than the decedent was. Legacy (off): the memory carries over and the entrant is taxed
    /// on the decedent's distributions.
    #[test]
    fn entrant_in_reused_slot_starts_clean() {
        for reset in [true, false] {
            let cfg = Config { n_households: 500, immigration_annual: 0.0, entrant_slot_reset: reset, ..Default::default() };
            let mut w = tiny_world(cfg, 9);
            step(&mut w);
            let n = w.households.len();
            // The lowest dead slot, or a household without home, property, job or partner that dies now.
            let i = match (0..n).find(|&i| !w.households[i].alive) {
                Some(i) => i,
                None => {
                    let i = (0..n)
                        .find(|&i| {
                            let hh = &w.households[i];
                            hh.home.is_none() && hh.properties.is_empty() && hh.employer.is_none() && !hh.partnered
                        })
                        .expect("a household with no home or job");
                    // As `process_death` leaves it: the estate (deposits and Fund shares) is gone to the heirs.
                    ledger::fund_sell(&mut w, i as HhId, f64::MAX);
                    let dep = w.households[i].deposits;
                    ledger::pay(&mut w, Account::Household(i as HhId), Account::Government, dep);
                    w.households[i].alive = false;
                    i
                }
            };
            // The decedent's memory, as the modules leave it at the end of its last month.
            let cs = &mut w.consumption;
            cs.distributions_prev.resize(n, 0.0);
            cs.capital_income_prev.resize(n, 0.0);
            cs.windfall_stock.resize(n, 0.0);
            cs.ui_spell.resize(n, false);
            cs.type_age.resize(n, 0);
            cs.distributions_prev[i] = 50_000.0;
            cs.capital_income_prev[i] = 80_000.0;
            cs.windfall_stock[i] = 9_000.0;
            cs.ui_spell[i] = true;
            cs.type_age[i] = 0;
            w.labor.last_sector.resize(n, None);
            w.labor.last_sector[i] = Some(Sector::Energy);
            w.fiscal.hh_arrears.resize(n, 0.0);
            w.fiscal.hh_arrears[i] = 1_234.0;
            // Exactly one native entrant this month, into the lowest dead slot.
            let mut st = std::mem::take(&mut w.demography);
            st.base_entrants = Some(0.0);
            st.diss0 = 0.0;
            st.diss_now = 0.0;
            st.pent_up = 0.0;
            st.native_acc = 1.0;
            st.immigrant_acc = 0.0;
            add_entrants(&mut w, &mut st);
            w.demography = st;
            assert!(w.households[i].alive, "the entrant did not take slot {i}");
            let cs = &w.consumption;
            if reset {
                assert_eq!(cs.distributions_prev[i], 0.0);
                assert_eq!(cs.capital_income_prev[i], 0.0);
                assert_eq!(cs.windfall_stock[i], 0.0);
                assert!(!cs.ui_spell[i]);
                assert_eq!(cs.type_age[i], u32::MAX, "the impatience type is not redrawn for an older entrant");
                assert_eq!(w.labor.last_sector[i], None);
                assert_eq!(w.fiscal.hh_arrears[i], 0.0);
            } else {
                assert_eq!(cs.distributions_prev[i], 50_000.0, "legacy: the decedent's distributions carry over");
                assert_eq!(w.labor.last_sector[i], Some(Sector::Energy));
            }
            // Tax this month: the entrant has no income of its own (its month record is fresh) and enough cash to pay.
            ledger::pay(&mut w, Account::CentralBank, Account::Household(i as HhId), 1.0e6);
            crate::fiscal::collect_and_finance(&mut w);
            let tax = w.hh_month[i].taxes;
            if reset {
                assert_eq!(tax, 0.0, "the entrant paid tax on the decedent's dividends");
            } else {
                assert!(tax > 0.0, "legacy: the entrant is taxed on the decedent's distributions");
            }
        }
    }

    /// b6-reentry M4b/M4c: the hazard is the legacy OLF_OUTFLOW at the defaults, and the recent and scar classes apply
    /// by memory and window.
    #[test]
    fn reentry_hazard_classes_follow_memory_and_window() {
        let mut cfg = Config::default();
        assert_eq!(reentry_hazard(&cfg, 9, 2), (OLF_OUTFLOW, ReentryClass::Base), "defaults are not legacy");
        assert_eq!(reentry_hazard(&cfg, 20, 5), (OLF_OUTFLOW, ReentryClass::Scar), "scar class, value unchanged at 1");
        cfg.udur_reentry_recent = 0.15;
        cfg.udur_reentry_scar = 0.67;
        assert_eq!(reentry_hazard(&cfg, 5, 2), (0.15, ReentryClass::Recent));
        assert_eq!(reentry_hazard(&cfg, 5, 4), (OLF_OUTFLOW, ReentryClass::Base), "short spell past window");
        assert_eq!(reentry_hazard(&cfg, 20, 2), (0.15, ReentryClass::Recent));
        assert_eq!(reentry_hazard(&cfg, 20, 4), (OLF_OUTFLOW * 0.67, ReentryClass::Scar));
        assert_eq!(reentry_hazard(&cfg, 0, 0), (OLF_OUTFLOW, ReentryClass::Base), "no memory");
    }

    /// b6-reentry M4b: a re-entrant with memory resumes its spell length when `udur_resume_spell` is on, and not when
    /// it is off. The single draw is the same in both cases (hazard 1 here, so the draw always re-enters).
    #[test]
    fn reentry_resumes_spell_only_when_on() {
        for resume in [true, false] {
            let mut cfg = Config { n_households: 500, ..Default::default() };
            cfg.udur_reentry_recent = 1.0;
            cfg.udur_resume_spell = resume;
            let mut w = tiny_world(cfg, 71);
            let h = 0usize;
            w.labor.reentry_prev = vec![0; w.households.len()];
            w.labor.reentry_n = vec![0; w.households.len()];
            w.labor.reentry_prev[h] = 7;
            {
                let hh = &mut w.households[h];
                hh.alive = true;
                hh.status = LaborStatus::OutOfLaborForce;
                hh.age_months = 480;
                hh.months_unemployed = 0;
                hh.bls_unemployed = 0;
            }
            let nom = crate::fiscal::nominal(&w);
            let ctx = PassCtx {
                cfg: &w.cfg,
                nom,
                seed: w.seed,
                tick: w.tick,
                frozen: false,
                p_form: 0.0,
                p_sep: 0.0,
                reentry_prev: &w.labor.reentry_prev,
                reentry_n: &w.labor.reentry_n,
                att_theta: 1.0,
            };
            let mut out = ChunkOut::default();
            monthly_pass(&mut w.households[h], h as HhId, &ctx, &mut out);
            assert_eq!(w.households[h].status, LaborStatus::Unemployed);
            assert_eq!(out.re_recent, 1, "re-entry not classed as recent");
            let want = if resume { 7 } else { 0 };
            assert_eq!(w.households[h].months_unemployed, want, "resume {resume}");
            // The BLS search-spell clock restarts at the re-entry whether or not the ranking clock resumes.
            assert_eq!(w.households[h].bls_unemployed, 0, "bls clock after re-entry, resume {resume}");
            assert_eq!(out.resumed, u32::from(resume));
        }
    }
}
