//! Labour market (docs/DESIGN.md §6, §16): separations, layoffs, search & matching, wages.
//!
//! Monthly order inside `step`:
//! 1. bookkeeping (tenure, career totals, unemployment counters, reservation wages, human capital);
//! 2. planned layoffs executed by firms;
//! 3. exogenous quits / match destruction;
//! 4. search & matching (applications, ranking by human capital, hiring, job-to-job moves);
//! 5. stayers' annual wage reset (§6 downward rigidity);
//! 6. end-of-month summaries (unfilled vacancies, unemployed headcount).
//!
//! Labour moves no money: wages are paid by `firms::produce`, benefits by `fiscal`.

use rayon::prelude::*;

use crate::common;
use crate::config::Config;
use crate::rng::{agent_rng, lognormal_mean1, module_rng, stream, Rng};
use crate::types::*;

/// Per-agent RNG id bases. Firm draws and search draws are keyed apart from household separation draws
/// (keyed by the raw household index) so no two decisions share a generator.
const FIRM_ID_BASE: u64 = 1 << 40;
const SEARCH_ID_BASE: u64 = 1 << 41;
/// fix-labour: RNG stream for match-specific pay shocks (a new stream id; distinct from `rng::stream` values).
const MATCH_PAY_STREAM: u64 = crate::rng::stream::LABOUR_MATCH_PAY;
/// Applications sent by an employed worker searching on the job (§6).
const ON_THE_JOB_APPLICATIONS: usize = 2;
/// Share of unemployed applications aimed at the sector of the previous employer; the rest go anywhere (§6).
const SECTOR_TARGETING: f64 = 0.7;
/// Random jitter, in months of tenure, when ranking layoff candidates ("LIFO with noise", §6).
const LAYOFF_NOISE_MONTHS: f64 = 3.0;
/// fix-labour: RNG stream for unemployed households' exits to the labour force (new stream id).
const UE_EXIT_STREAM: u64 = crate::rng::stream::LABOUR_UE_EXIT;
/// fix-labour: smoothing of the slow job-finding reference used by the cyclical exit scale.
const F_REF_SMOOTH: f64 = 0.98;
/// b6-reentry M4d: smoothing of the tightness reference θ̄ (about a 100-month average).
const THETA_REF_SMOOTH: f64 = 0.99;
/// Stayers' annual wage reset may cut the wage by at most this factor (§6).
const STAYER_MAX_CUT: f64 = 0.98;
/// fix-labour: multiplicative ranking bonus of non-employed applicants over employed ones (soft, not strict).
const UNEMPLOYED_RANK_BONUS: f64 = 0.05;
/// Households per parallel chunk when generating applications (fixed, so pair order is thread-count independent).
const APP_CHUNK: usize = 4096;
/// Ages covered by the Mincer table (months); older ages fall back to `common::human_capital`.
const HC_TAB_MONTHS: usize = 1_300;
/// Schooling-end ages (years) by education index, as used by `common::human_capital`.
const SCHOOL_END: [f64; 3] = [18.0, 22.0, 24.0];

/// Fill the Mincer exp-term table. Each entry is computed with the same expression as `common::human_capital`.
fn fill_exp_table(tab: &mut Vec<f64>) {
    tab.clear();
    for &school_end in &SCHOOL_END {
        for a in 0..HC_TAB_MONTHS {
            let x = (a as f64 / 12.0 - school_end).max(0.0);
            tab.push((0.035 * x - 0.0006 * x * x).exp());
        }
    }
}

/// Human capital of `hh`, bit-identical to `common::human_capital` (uses the table when it covers the age).
#[inline]
fn hc_of(cfg: &Config, tab: &[f64], hh: &Household) -> f64 {
    let edu = match hh.education {
        Education::HighSchool => 0,
        Education::College => 1,
        Education::Graduate => 2,
    };
    let age = hh.age_months as usize;
    if age < HC_TAB_MONTHS && !tab.is_empty() {
        let prem = cfg.edu_premium[edu];
        prem * hh.skill as f64 * tab[edu * HC_TAB_MONTHS + age]
    } else {
        common::human_capital(cfg, hh)
    }
}

/// Labour-market memory and scratch buffers (module-owned). Per-household and per-firm vectors are
/// resized lazily at the start of each step, because the household Vec can grow.
#[derive(Clone, Debug, Default)]
pub struct LaborState {
    /// Sector of the most recent employer, used to target applications (§6). None if never employed.
    pub last_sector: Vec<Option<Sector>>,
    /// Vacancies per firm still open after this month's matching (firms use it to raise wage offers).
    pub unfilled: Vec<u32>,
    /// Human capital per household, computed once per month.
    hc: Vec<f64>,
    /// Application pairs (firm, household) this month, in household order (built in parallel chunks).
    pairs: Vec<(FirmId, HhId)>,
    /// Applicants per firm as a flat list: firm f's applicants are `app_list[app_start[f]..app_start[f + 1]]`,
    /// in household order (then ranked per firm before matching). Buffers are reused across months.
    app_start: Vec<usize>,
    app_cursor: Vec<usize>,
    app_list: Vec<HhId>,
    /// Epoch in which each household was last hired; a hire this epoch blocks further hires.
    hired_epoch: Vec<u32>,
    /// Exogenous-separation flags for this month.
    sep_flag: Vec<bool>,
    /// fix-labour: unemployed households drawn to leave the labour force this month.
    exit_flag: Vec<bool>,
    /// fix-labour: job-finding rate last month (hires from unemployment ÷ unemployed) and its slow average.
    f_last: f64,
    f_ref: f64,
    /// Match-specific pay multiplier of each household's current job (1 for initial matches and when disabled).
    match_mult: Vec<f64>,
    /// Open-vacancy sampling table: firms grouped by sector, with cumulative opening weights.
    vac_firms: Vec<FirmId>,
    vac_cum: Vec<f64>,
    /// Range of `vac_firms` holding each sector's open vacancies.
    sector_range: [(usize, usize); N_SECTORS],
    /// Firms with applicants, in random processing order for this month.
    order: Vec<FirmId>,
    /// exp(0.035x − 0.0006x²) by education and age in months, for the Mincer term (built once; see `hc_of`).
    exp_tab: Vec<f64>,
    /// Incremented once per step.
    epoch: u32,
    /// Unemployed households alive at the end of the last step.
    unemployed: u32,
    /// Hires and hires needed (sum of firms' `vacancies`) in the last step; firms::plan turns them into the
    /// expected fill rate for next month (the share of needed hires that the market delivers).
    pub hires_last: u32,
    pub needed_last: u32,
    /// fix-labour: vacancies opened at each firm by poaching its employees this month (match_firm). They count
    /// in `needed_last` and are removed from the firm's `vacancies` after matching, so firms::plan's unfilled
    /// signal sees only the firm's own postings.
    poach_open: Vec<u32>,
    /// Hires into each firm this month (match_firm). Splits the firm's remaining openings between planned and poach slots.
    hires_at: Vec<u32>,
    /// b5-udur M1: scratch (score, household) pairs for duration ranking in `match_firm`.
    rank_buf: Vec<(f64, HhId)>,
    /// b5-udur M3: human-capital scar per household (1 = none). Used only when `udur_hc_loss` > 0.
    pub scar: Vec<f32>,
    /// b6-reentry M4b: months unemployed before the household's last U→N exit (0 = no memory), and months spent out
    /// of the labour force since that exit. Read by the demography re-entry hazard; set in `unemployed_exits`,
    /// cleared by `bookkeeping` once the household is no longer out of the labour force.
    pub reentry_prev: Vec<u16>,
    pub reentry_n: Vec<u16>,
    /// b6-reentry M4d: tightness at the end of the last step (θ_{t−1}), and its slow average θ̄ (0 until set).
    pub theta_last: f64,
    pub theta_ref: f64,
}

/// Reservation wage (§6): max of the minimum wage, the last wage decayed by 0.97 per unemployed month,
/// and — while benefits last — the benefit level min(replacement × last wage, cap).
pub fn reservation_wage(cfg: &Config, nom: &crate::fiscal::Nominal, hh: &Household) -> f64 {
    let decay = cfg.udur_resv_decay.powi(hh.months_unemployed.min(10_000) as i32);
    let mut r = nom.min_wage.max(hh.last_wage * decay);
    if hh.benefit_months_left > 0 {
        r = r.max((cfg.benefit_replacement * hh.last_wage).min(nom.ui_cap));
    }
    if r.is_finite() {
        r
    } else {
        nom.min_wage
    }
}

/// Wage offered for a worker of human capital `hc` by a firm with `wage_offer` per efficiency unit (§6).
#[inline]
fn offered_wage(min_wage: f64, wage_offer: f64, hc: f64) -> f64 {
    (wage_offer * hc).max(min_wage)
}

/// Applications each household may send this month (slots reserved in `targets`).
#[inline]
fn slots_per_household(cfg: &Config) -> usize {
    cfg.applications.max(ON_THE_JOB_APPLICATIONS)
}

/// Run this month's labour market. Firms' `vacancies` and `planned_layoffs` were set by firms::plan.
pub fn step(w: &mut World) {
    let mut st = std::mem::take(&mut w.labor);
    ensure_sizes(w, &mut st);
    st.epoch = st.epoch.wrapping_add(1);

    // Unemployed households after this month's separations: counted at bookkeeping (status unchanged there),
    // plus every separation to unemployment made by layoffs and exogenous separations.
    let exit_scale = cyclical_exit_scale(w.cfg.unemp_exit_cyc, st.f_last, st.f_ref);
    let unemployed_book = bookkeeping(w, &mut st, exit_scale);
    tally_stocks(w);
    let layoffs_n = layoffs(w);
    let exo_n = exogenous_separations(w, &st);
    let exits_n = unemployed_exits(w, &mut st);
    let unemployed = (unemployed_book + layoffs_n + exo_n).saturating_sub(exits_n);
    // Non-participants of working age who may search directly: the stock at the start of the step plus new exits.
    let nonpart_pool = w.flows.n_stock + exits_n;
    let (hires_u_before, u_count) = (w.flows.hires_from_u, unemployed.max(1));
    search_and_match(w, &mut st, unemployed, nonpart_pool);
    let f_t = f64::from(w.flows.hires_from_u - hires_u_before) / f64::from(u_count);
    st.f_last = f_t;
    st.f_ref = if st.f_ref > 0.0 { F_REF_SMOOTH * st.f_ref + (1.0 - F_REF_SMOOTH) * f_t } else { f_t };
    st.unemployed = stayer_wage_reset(w, &st);
    // b6-reentry M4d: θ_{t} at the end of the step (vacancies over unemployed, as `tightness`), read next month.
    let theta = f64::from(w.flows.vacancies) / f64::from(st.unemployed.max(1));
    st.theta_last = theta;
    st.theta_ref = if st.theta_ref > 0.0 { THETA_REF_SMOOTH * st.theta_ref + (1.0 - THETA_REF_SMOOTH) * theta } else { theta };
    w.labor = st;
}

/// b6-reentry M4d: attachment of a non-participant with previous unemployment spell `prev` (months) who has been out
/// for `n` months: 1 unless `prev` reached `udur_att_spell`; otherwise [scar + (1 − scar)(1 − e^{−n/τ})] × θ-factor.
/// `theta_fac` is `theta_factor` (1 when tightness is unchanged). Pure, so the hazard and the contact weight agree.
pub fn attachment(cfg: &Config, prev: u16, n: u16, theta_fac: f64) -> f64 {
    if !cfg.udur_att_on || prev == 0 || prev < cfg.udur_att_spell {
        return 1.0;
    }
    let scar = cfg.udur_att_scar;
    let tau = cfg.udur_att_tau.max(1e-9);
    let recover = 1.0 - (-f64::from(n) / tau).exp();
    (scar + (1.0 - scar) * recover) * theta_fac
}

/// b6-reentry M4d: the tightness factor (θ_{t−1}/θ̄)^η clamped to [0.5, 2]; 1 before the reference is set.
pub fn theta_factor(cfg: &Config, st: &LaborState) -> f64 {
    if !cfg.udur_att_on || cfg.udur_att_eta == 0.0 || st.theta_last <= 0.0 || st.theta_ref <= 0.0 {
        return 1.0;
    }
    (st.theta_last / st.theta_ref).powf(cfg.udur_att_eta).clamp(0.5, 2.0)
}

/// fix-labour: scale on unemployed exits to the labour force. Job finding below its slow average lowers exits
/// (U->N falls in recessions, as in CPS gross flows; the sign is set by `unemp_exit_cyc`). Clamped to [0.5, 2].
fn cyclical_exit_scale(cyc: f64, f_last: f64, f_ref: f64) -> f64 {
    if f_last > 0.0 && f_ref > 0.0 && cyc != 0.0 {
        (f_ref / f_last).powf(cyc).clamp(0.5, 2.0)
    } else {
        1.0
    }
}

/// Vacancies at `f` left unfilled after this month's matching (0 if unknown or dead).
pub fn unfilled_vacancies(w: &World, f: FirmId) -> u32 {
    w.labor.unfilled.get(f as usize).copied().unwrap_or(0)
}

/// Labour-market tightness: unfilled vacancies over unemployed households at the end of this month.
pub fn tightness(w: &World) -> f64 {
    w.flows.vacancies as f64 / w.labor.unemployed.max(1) as f64
}

fn ensure_sizes(w: &World, st: &mut LaborState) {
    let nh = w.households.len();
    let nf = w.firms.len();
    st.last_sector.resize(nh, None);
    st.hc.resize(nh, 0.0);
    st.hired_epoch.resize(nh, 0);
    st.sep_flag.resize(nh, false);
    st.exit_flag.resize(nh, false);
    st.match_mult.resize(nh, 1.0);
    st.unfilled.resize(nf, 0);
    st.poach_open.resize(nf, 0);
    st.hires_at.resize(nf, 0);
    st.scar.resize(nh, 1.0);
    st.reentry_prev.resize(nh, 0);
    st.reentry_n.resize(nh, 0);
}

/// Step 1 (§16): tenure and career counters, unemployment counters, reservation wages. Also records
/// each employed household's sector (for targeting) and its human capital for this month. The exogenous
/// separation draw (§6) is taken here from each employed household's own generator and applied later by
/// `exogenous_separations`, after layoffs. Returns the number of unemployed households.
fn bookkeeping(w: &mut World, st: &mut LaborState, exit_scale: f64) -> u32 {
    if st.exp_tab.is_empty() {
        fill_exp_table(&mut st.exp_tab);
    }
    // b6-reentry M4b: spell memory bookkeeping (sequential; reads only status and alive). Out-of-labour-force
    // households with a memory count the months since their exit; everyone else loses the memory.
    for (hh, (p, n)) in w.households.iter().zip(st.reentry_prev.iter_mut().zip(st.reentry_n.iter_mut())) {
        if hh.alive && hh.status == LaborStatus::OutOfLaborForce {
            if *p > 0 {
                *n = n.saturating_add(1);
            }
        } else {
            *p = 0;
            *n = 0;
        }
    }
    let nom = crate::fiscal::nominal(w);
    let cfg = &w.cfg;
    let firms = &w.firms;
    let (seed, tick, rate) = (w.seed, w.tick, cfg.separation_rate);
    let mult = cfg.separation_mult_by_edu;
    // Career earnings are kept in base-year dollars: wages divided by the average wage index (100 at t=0).
    let wage_scale = 100.0 / w.prices.wage_index.max(1e-9);
    let tab = &st.exp_tab;
    let ecfg = (cfg.unemp_exit_base, cfg.unemp_exit_duration, cfg.unemp_exit_exhaust);
    let udur_exit = (cfg.udur_exit_base_shift, cfg.udur_exit_dur, cfg.udur_exit_dur_months);
    let (loss, loss_max, recover) = (cfg.udur_hc_loss, cfg.udur_hc_loss_max, cfg.udur_hc_recover);
    w.households
        .par_iter_mut()
        .enumerate()
        .zip(st.hc.par_iter_mut())
        .zip(st.last_sector.par_iter_mut())
        .zip(st.sep_flag.par_iter_mut())
        .zip(st.exit_flag.par_iter_mut())
        .zip(st.scar.par_iter_mut())
        .map(|((((((h, hh), hc), last), flag), exit), scar)| {
            *flag = false;
            *exit = false;
            if !hh.alive {
                *scar = 1.0;
                return 0u32;
            }
            *hc = hc_of(cfg, tab, hh);
            if loss > 0.0 {
                // b5-udur M3: scarring by status, then human capital × scar. A household that has never worked
                // (an entrant placed in a reused slot) carries no scar from a previous occupant. Test on last_wage:
                // months_employed is reset at every separation, so it cannot tell a displaced worker from an entrant.
                if hh.last_wage <= 0.0 {
                    *scar = 1.0;
                } else {
                    match hh.status {
                        LaborStatus::Unemployed => *scar = (1.0 - loss_max as f32).max(*scar * (1.0 - loss as f32)),
                        LaborStatus::Employed => *scar += recover as f32 * (1.0 - *scar),
                        _ => {}
                    }
                }
                *hc *= f64::from(*scar);
            }
            match hh.status {
                LaborStatus::Employed => {
                    hh.months_in_job += 1;
                    hh.months_employed = hh.months_employed.saturating_add(1);
                    hh.career_earnings += hh.wage * wage_scale;
                    hh.career_months += 1;
                    if let Some(f) = hh.employer {
                        *last = Some(firms[f as usize].sector);
                        let mut rng = agent_rng(seed, tick, stream::LABOR, h as u64);
                        // fix-labour: separation incidence by education (same draw, education-scaled threshold)
                        *flag = rng.gen::<f64>() < rate * mult[edu_idx(hh.education)];
                    }
                    0
                }
                LaborStatus::Unemployed => {
                    hh.months_unemployed += 1;
                    hh.bls_unemployed += 1;
                    hh.reservation_wage = reservation_wage(cfg, &nom, hh) as f32;
                    // fix-labour: exit to out of the labour force, own generator per household
                    let hz = unemp_exit_hazard(ecfg, udur_exit, hh, exit_scale);
                    if hz > 0.0 {
                        let mut rng = agent_rng(seed, tick, UE_EXIT_STREAM, h as u64);
                        *exit = rng.gen::<f64>() < hz;
                    }
                    1
                }
                _ => 0,
            }
        })
        .sum()
}

/// Education index used by the labour counters (HighSchool, College, Graduate).
#[inline]
fn edu_idx(e: Education) -> usize {
    match e {
        Education::HighSchool => 0,
        Education::College => 1,
        Education::Graduate => 2,
    }
}

/// Duration bucket of an unemployed household (months unemployed 1-3, 4-6, 7-12, 13+); None before the first month.
#[inline]
fn dur_bucket(months: u16) -> Option<usize> {
    match months {
        0 => None,
        1..=3 => Some(0),
        4..=6 => Some(1),
        7..=12 => Some(2),
        _ => Some(3),
    }
}

/// UI bucket (benefits with more than one month left, last benefit month, exhausted).
#[inline]
fn ui_bucket(benefit_months_left: u16) -> usize {
    match benefit_months_left {
        0 => 2,
        1 => 1,
        _ => 0,
    }
}

/// Fix-labour counters (observation only): employed, unemployed and out-of-labour-force stocks at the start of the
/// labour step, by education, duration and UI status. Draws no random numbers and changes no state but `Flows`.
fn tally_stocks(w: &mut World) {
    let mut emp_edu = [0u32; 3];
    let mut u_edu = [0u32; 3];
    let mut u_dur = [0u32; 4];
    let mut u_ui = [0u32; 3];
    let mut n_stock = 0u32;
    for hh in w.households.iter().filter(|h| h.alive) {
        let e = edu_idx(hh.education);
        match hh.status {
            LaborStatus::Employed => emp_edu[e] += 1,
            LaborStatus::Unemployed => {
                u_edu[e] += 1;
                if let Some(b) = dur_bucket(hh.months_unemployed) {
                    u_dur[b] += 1;
                }
                u_ui[ui_bucket(hh.benefit_months_left)] += 1;
            }
            LaborStatus::OutOfLaborForce => n_stock += 1,
            _ => {}
        }
    }
    w.flows.emp_edu = emp_edu;
    w.flows.u_stock_edu = u_edu;
    w.flows.u_stock_dur = u_dur;
    w.flows.u_stock_ui = u_ui;
    w.flows.n_stock = n_stock;
}

/// fix-labour: monthly probability that an unemployed household leaves the labour force (see `Config`).
#[inline]
fn unemp_exit_hazard(ecfg: (f64, f64, f64), udur_exit: (f64, f64, f64), hh: &Household, exit_scale: f64) -> f64 {
    let (mut base, dur, exhaust) = ecfg;
    // b5-udur M4a: duration-rising exits (skipped when both terms are zero, so the legacy hazard is unchanged).
    let (shift, du, dm) = udur_exit;
    if (shift != 0.0 || du != 0.0) && dm > 0.0 {
        base += shift + du * f64::from(hh.months_unemployed).min(dm) / dm;
    }
    let m = f64::from(hh.months_unemployed.min(12)) / 12.0;
    let exhausted = if hh.benefit_months_left == 0 { exhaust } else { 0.0 };
    ((base + dur * m + exhausted) * exit_scale).clamp(0.0, 1.0)
}

/// fix-labour: apply this month's unemployed exits to the labour force (out of labour force, no benefits).
/// Applied after layoffs and exogenous separations, so only households still unemployed leave. Returns the count.
fn unemployed_exits(w: &mut World, st: &mut LaborState) -> u32 {
    let mut n = 0u32;
    for h in 0..w.households.len() {
        if !st.exit_flag[h] {
            continue;
        }
        if w.households[h].alive && w.households[h].status == LaborStatus::Unemployed {
            // b6-reentry M4b: spell memory is the months unemployed before this exit.
            st.reentry_prev[h] = w.households[h].months_unemployed;
            st.reentry_n[h] = 0;
            common::separate_worker(w, h as HhId, LaborStatus::OutOfLaborForce);
            let hh = &mut w.households[h];
            hh.benefit_months_left = 0;
            // Duration is not reset: the spell's count carries into the non-participant reservation wage, and a return
            // to unemployment (demography) starts a new spell, so exits do not shorten measured durations.
            w.flows.u_to_n += 1;
            n += 1;
        }
    }
    n
}

/// Separate a worker to unemployment (counted as a separation) and refresh their reservation wage,
/// since benefit eligibility and `last_wage` change at separation.
fn separate_to_unemployment(w: &mut World, h: HhId) {
    let edu = edu_idx(w.households[h as usize].education);
    common::separate_worker(w, h, LaborStatus::Unemployed);
    w.flows.separations += 1;
    w.flows.sep_edu[edu] += 1;
    let nom = crate::fiscal::nominal(w);
    let r = reservation_wage(&w.cfg, &nom, &w.households[h as usize]);
    w.households[h as usize].reservation_wage = r as f32;
}

/// Step 2 (§6): each firm with planned layoffs lets go of that many workers, shortest tenure first with
/// random jitter, so the most recent hires go first.
fn layoffs(w: &mut World) -> u32 {
    let (seed, tick) = (w.seed, w.tick);
    // A worker belongs to one firm, and candidate selection reads only the worker's `alive` flag and tenure, so each
    // firm's choice is independent of the others: choices are made in parallel, then applied in firm order.
    let picks: Vec<Vec<HhId>> = {
        let households = &w.households;
        w.firms
            .par_iter()
            .enumerate()
            .map(|(f, firm)| {
                let planned = firm.planned_layoffs as usize;
                if !firm.alive || planned == 0 || firm.workers.is_empty() {
                    return Vec::new();
                }
                let mut rng = agent_rng(seed, tick, stream::LABOR, FIRM_ID_BASE + f as u64);
                let mut cand: Vec<(f64, HhId)> = Vec::with_capacity(firm.workers.len());
                for &h in &firm.workers {
                    let hh = &households[h as usize];
                    if !hh.alive {
                        continue;
                    }
                    let jitter = rng.gen::<f64>() * LAYOFF_NOISE_MONTHS;
                    cand.push((hh.months_in_job as f64 + jitter, h));
                }
                let k = planned.min(cand.len());
                if k == 0 {
                    return Vec::new();
                }
                if k < cand.len() {
                    cand.select_nth_unstable_by(k, |a, b| a.0.total_cmp(&b.0));
                }
                cand[..k].iter().map(|&(_, h)| h).collect()
            })
            .collect()
    };
    let mut total = 0u32;
    for list in picks {
        for &h in &list {
            separate_to_unemployment(w, h);
            w.flows.layoffs += 1;
        }
        total += list.len() as u32;
    }
    total
}

/// Step 3 (§6): each employed household separates with probability `cfg.separation_rate` (drawn in
/// `bookkeeping`). Applied in household order to households still employed with an employer. Returns the count.
fn exogenous_separations(w: &mut World, st: &LaborState) -> u32 {
    let mut n = 0u32;
    for h in 0..w.households.len() {
        if !st.sep_flag[h] {
            continue;
        }
        let hh = &w.households[h];
        if hh.alive && hh.status == LaborStatus::Employed && hh.employer.is_some() {
            separate_to_unemployment(w, h as HhId);
            w.flows.sep_exo += 1;
            n += 1;
        }
    }
    n
}

/// Step 4 (§6, Lengnick 2013 style): build the open-vacancy table, let every searching household send
/// applications, then let each firm (in random order) hire its best-ranked acceptable applicants.
fn search_and_match(w: &mut World, st: &mut LaborState, unemployed: u32, nonpart_pool: u32) {
    let nf = w.firms.len();
    let needed: u64 = w.firms.iter().filter(|f| f.alive).map(|f| f.vacancies as u64).sum();
    st.poach_open.iter_mut().for_each(|p| *p = 0);
    st.hires_at.iter_mut().for_each(|h| *h = 0);
    build_vacancy_table(w, st);
    generate_applications(w, st, unemployed, nonpart_pool);
    build_applicant_lists(st, nf);

    st.order.clear();
    for (f, firm) in w.firms.iter().enumerate() {
        if firm.alive && firm.vacancies > 0 && st.app_start[f + 1] > st.app_start[f] {
            st.order.push(f as FirmId);
        }
    }
    let mut rng = module_rng(w.seed, w.tick, stream::LABOR);
    for i in (1..st.order.len()).rev() {
        let j = rng.gen_range(0..=i);
        st.order.swap(i, j);
    }

    let mut list = std::mem::take(&mut st.app_list);
    let order = std::mem::take(&mut st.order);
    for &f in &order {
        let (a, b) = (st.app_start[f as usize], st.app_start[f as usize + 1]);
        match_firm(w, st, f, &mut list[a..b]);
    }
    st.order = order;
    st.app_list = list;

    let mut total: u32 = 0;
    let mut poached: u64 = 0;
    for (f, firm) in w.firms.iter_mut().enumerate() {
        let open = if firm.alive { firm.vacancies } else { 0 };
        st.unfilled[f] = open;
        total = total.saturating_add(open);
        // Residual poach vacancies leave the firm's postings (they are not its plan's vacancies). Integration fix: only
        // the unfilled share of this month's poach slots is removed. Matching fills planned and poach openings alike, so
        // the unfilled poach slots are the poach openings times the firm's remaining share of its openings (remaining
        // over remaining plus hires). Removing every poach slot made a firm that refilled its poach slot look staffed.
        let poach = st.poach_open[f];
        let p = if poach > 0 && firm.vacancies > 0 {
            let total = f64::from(firm.vacancies) + f64::from(st.hires_at[f]);
            let share = f64::from(firm.vacancies) / total.max(1.0);
            ((f64::from(poach) * share).round() as u32).min(poach).min(firm.vacancies)
        } else {
            0
        };
        firm.vacancies -= p;
        poached += u64::from(st.poach_open[f]);
    }
    w.flows.vacancies = total;
    st.hires_last = w.flows.hires;
    st.needed_last = (needed + poached).min(u64::from(u32::MAX)) as u32;
}

/// Group the application pairs by firm (counting sort, stable: each firm's applicants stay in household order).
fn build_applicant_lists(st: &mut LaborState, nf: usize) {
    st.app_start.clear();
    st.app_start.resize(nf + 1, 0);
    for &(f, _) in &st.pairs {
        st.app_start[f as usize + 1] += 1;
    }
    for f in 0..nf {
        st.app_start[f + 1] += st.app_start[f];
    }
    st.app_cursor.clear();
    st.app_cursor.extend_from_slice(&st.app_start[..nf]);
    st.app_list.clear();
    st.app_list.resize(st.pairs.len(), 0);
    for &(f, h) in &st.pairs {
        let c = &mut st.app_cursor[f as usize];
        st.app_list[*c] = h;
        *c += 1;
    }
}

/// Flat cumulative-weight table of open vacancies, grouped by sector (built in `build_vacancy_table`).
struct VacTable<'a> {
    firms: &'a [FirmId],
    cum: &'a [f64],
    range: &'a [(usize, usize); N_SECTORS],
}

impl VacTable<'_> {
    /// Draw one posted opening with probability proportional to its firm's openings. If `sector` is given and
    /// has vacancies, the draw is restricted to that sector with probability `SECTOR_TARGETING`.
    fn draw<R: Rng + ?Sized>(&self, rng: &mut R, sector: Option<Sector>) -> Option<FirmId> {
        let local = sector
            .map(|s| self.range[s.idx()])
            .filter(|&(lo, hi)| hi > lo);
        let (lo, hi) = match local {
            Some(r) if rng.gen::<f64>() < SECTOR_TARGETING => r,
            _ => (0, self.firms.len()),
        };
        if hi <= lo {
            return None;
        }
        let base = if lo == 0 { 0.0 } else { self.cum[lo - 1] };
        let total = self.cum[hi - 1] - base;
        if !(total > 0.0) {
            return None;
        }
        let u = base + rng.gen::<f64>() * total;
        let idx = lo + self.cum[lo..hi].partition_point(|&c| c <= u);
        Some(self.firms[idx.min(hi - 1)])
    }
}

/// Group firms with posted openings by sector, with cumulative opening counts. O(F).
fn build_vacancy_table(w: &World, st: &mut LaborState) {
    st.vac_firms.clear();
    st.vac_cum.clear();
    let mut acc = 0.0;
    for s in Sector::ALL {
        let lo = st.vac_firms.len();
        for (i, f) in w.firms.iter().enumerate() {
            if f.alive && f.sector == s && f.openings > 0 {
                acc += f.openings as f64;
                st.vac_firms.push(i as FirmId);
                st.vac_cum.push(acc);
            }
        }
        st.sector_range[s.idx()] = (lo, st.vac_firms.len());
    }
}

/// fix-labour: relative search effort of an unemployed household. Benefits lower it by `ui_search_cut` times the
/// replacement ratio while they last, the last benefit month raises it (exhaustion spike), and contact with the
/// unemployed decays with duration (negative duration dependence). Contact is divided by the constant
/// `search_effort_norm` (not the monthly mean), so these effects change aggregate job finding.
/// b5-udur M2a: duration contact profile c(m) (see `Config::udur_contact_on`). Only used when that is on.
#[inline]
fn contact_profile(cfg: &Config, months: f64) -> f64 {
    let a = months.min(cfg.udur_contact_m1).max(0.0);
    let seg = (months - cfg.udur_contact_m1).max(0.0).min((cfg.udur_contact_m2 - cfg.udur_contact_m1).max(0.0));
    cfg.udur_contact_d1.powf(a) * cfg.udur_contact_d2.powf(seg)
}

#[inline]
fn search_effort(cfg: &Config, nom: &crate::fiscal::Nominal, hh: &Household) -> f64 {
    let mut e = 1.0;
    if hh.benefit_months_left > 0 && hh.last_wage > 0.0 {
        let ratio = ((cfg.benefit_replacement * hh.last_wage).min(nom.ui_cap) / hh.last_wage).min(1.0);
        // b5-udur M5: recipients past the standard entitlement (extended months) get the scaled UI search cut.
        let scale = if u32::from(hh.months_unemployed) >= cfg.benefit_months { cfg.udur_ui_ext_search_scale } else { 1.0 };
        e *= 1.0 - cfg.ui_search_cut * scale * ratio;
        if hh.benefit_months_left == 1 {
            e *= 1.0 + cfg.ui_exhaustion_boost;
        }
    }
    let c = if cfg.udur_contact_on {
        contact_profile(cfg, f64::from(hh.months_unemployed))
    } else {
        let months = f64::from(hh.months_unemployed).min(cfg.duration_contact_cap);
        cfg.duration_contact_decay.powf(months)
    };
    (e * c).max(0.0)
}

/// Each searching household picks its target firms. Unemployed households send `cfg.applications`
/// applications; employed households search on the job with probability `cfg.on_the_job_search`, sending
/// `ON_THE_JOB_APPLICATIONS`, targeting their current sector. Runs in parallel over fixed household chunks with
/// per-household RNGs; the (firm, household) pairs are kept in household order.
fn generate_applications(w: &World, st: &mut LaborState, unemployed: u32, nonpart_pool: u32) {
    let k = slots_per_household(&w.cfg);
    let nom = crate::fiscal::nominal(w);

    let cfg = &w.cfg;
    let firms = &w.firms;
    let (seed, tick) = (w.seed, w.tick);
    // b6-reentry M4d: with attachment on, the non-participant pool is scaled by the mean attachment weight of the
    // out-of-labour-force households (weights from last month's tightness), and each N→E contact by its weight.
    let att_on = cfg.udur_att_on;
    let theta_fac = theta_factor(cfg, st);
    let (rp, rn) = (&st.reentry_prev, &st.reentry_n);
    let weight = |h: usize| -> f64 {
        if !att_on {
            return 1.0;
        }
        attachment(cfg, rp.get(h).copied().unwrap_or(0), rn.get(h).copied().unwrap_or(0), theta_fac)
    };
    // The legacy pool is scaled by the mean weight of the out-of-labour-force households (1 when all weights are 1).
    let nonpart_pool = if att_on {
        let (sum, count) = w
            .households
            .iter()
            .enumerate()
            .filter(|(_, hh)| hh.alive && hh.status == LaborStatus::OutOfLaborForce)
            .fold((0.0, 0u64), |(s, c), (h, _)| (s + weight(h), c + 1));
        if count > 0 { f64::from(nonpart_pool) * sum / count as f64 } else { 0.0 }
    } else {
        f64::from(nonpart_pool)
    };
    let vac = VacTable {
        firms: &st.vac_firms,
        cum: &st.vac_cum,
        range: &st.sector_range,
    };
    let last_sector = &st.last_sector;

    // Matching friction (Petrongolo & Pissarides 2001): aggregate meetings M = μ·U^½·V^½, so each unemployed
    // household is in contact with the market this month with probability f = min(1, μ·√(V/U)). Out-of-labour-force
    // households who search directly (fix-labour) join the searchers in the denominator.
    let searchers = (f64::from(unemployed) + cfg.nonparticipant_contact * nonpart_pool).max(1.0);
    let vacancies: f64 = w.firms.iter().filter(|f| f.alive).map(|f| f.openings as f64).sum();
    let p_meet = (cfg.matching_efficiency * (vacancies / searchers).sqrt()).min(1.0);
    let norm = if cfg.udur_contact_on { cfg.udur_effort_norm } else { cfg.search_effort_norm }.max(1e-9);
    let ot_search = (cfg.on_the_job_search * cfg.on_the_job_search_mult).min(1.0);

    let chunks: Vec<Vec<(FirmId, HhId)>> = w
        .households
        .par_chunks(APP_CHUNK)
        .enumerate()
        .map(|(c, chunk)| {
            let mut out: Vec<(FirmId, HhId)> = Vec::new();
            for (j, hh) in chunk.iter().enumerate() {
                let h = c * APP_CHUNK + j;
                if !hh.alive {
                    continue;
                }
                // Generators are created only for households that can search (the draws are per-agent, so this
                // leaves every other household's stream unchanged).
                let (n, sector, mut rng) = match hh.status {
                    LaborStatus::Unemployed => {
                        let mut rng = agent_rng(seed, tick, stream::LABOR, SEARCH_ID_BASE + h as u64);
                        let p_h = (p_meet * search_effort(cfg, &nom, hh) / norm).min(1.0);
                        if !(rng.gen::<f64>() < p_h) {
                            continue;
                        }
                        (cfg.applications.min(k), last_sector[h], rng)
                    }
                    // fix-labour: out-of-labour-force households of working age search directly (N->E hires)
                    LaborStatus::OutOfLaborForce if cfg.nonparticipant_contact > 0.0 => {
                        let mut rng = agent_rng(seed, tick, stream::LABOR, SEARCH_ID_BASE + h as u64);
                        let p_h = (cfg.nonparticipant_contact * p_meet * weight(h)).min(1.0);
                        if !(rng.gen::<f64>() < p_h) {
                            continue;
                        }
                        (cfg.applications_nonparticipant.min(k), last_sector[h], rng)
                    }
                    LaborStatus::Employed => {
                        let mut rng = agent_rng(seed, tick, stream::LABOR, SEARCH_ID_BASE + h as u64);
                        if !(rng.gen::<f64>() < ot_search) {
                            continue;
                        }
                        let s = hh
                            .employer
                            .filter(|&f| firms[f as usize].alive)
                            .map(|f| firms[f as usize].sector);
                        (ON_THE_JOB_APPLICATIONS.min(k), s, rng)
                    }
                    _ => continue,
                };
                for _ in 0..n {
                    if let Some(f) = vac.draw(&mut rng, sector) {
                        out.push((f, h as HhId));
                    }
                }
            }
            out
        })
        .collect();
    st.pairs.clear();
    for c in chunks {
        st.pairs.extend(c);
    }
}

/// One firm's hiring round: rank applicants by human capital (descending), then hire each applicant
/// whose offer meets their bar (reservation wage if unemployed, `job_to_job_premium` × wage if employed) until vacancies
/// run out. Each household is hired at most once per epoch.
fn match_firm(w: &mut World, st: &mut LaborState, f: FirmId, list: &mut [HhId]) {
    let fi = f as usize;
    let nom = crate::fiscal::nominal(w);
    let hc = &st.hc;
    // Rank applicants by human capital (descending), with a soft preference for the non-employed (a multiplicative
    // bonus, not strict priority: employed applicants can still win openings), then id. Callback evidence favours
    // employed searchers (Kroft, Lange & Notowidigdo 2013), so the bonus is small.
    let score = |h: HhId| {
        let bonus = if w.households[h as usize].status == LaborStatus::Employed { 1.0 } else { 1.0 + UNEMPLOYED_RANK_BONUS };
        hc[h as usize] * bonus
    };
    let kappa = w.cfg.udur_rank_kappa;
    if kappa > 0.0 {
        // b5-udur M1: unemployed applicants are discounted by their duration; other applicants keep the legacy score.
        let cap = w.cfg.udur_rank_cap;
        let mut buf = std::mem::take(&mut st.rank_buf);
        buf.clear();
        buf.extend(list.iter().map(|&h| {
            let hh = &w.households[h as usize];
            let s = score(h);
            let s = if hh.status == LaborStatus::Unemployed {
                s * (-kappa * f64::from(hh.months_unemployed).min(cap) / 12.0).exp()
            } else {
                s
            };
            (s, h)
        }));
        buf.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        for (slot, &(_, h)) in list.iter_mut().zip(buf.iter()) {
            *slot = h;
        }
        st.rank_buf = buf;
    } else {
        list.sort_unstable_by(|&a, &b| score(b).total_cmp(&score(a)).then(a.cmp(&b)));
    }

    let wage_offer = w.firms[fi].wage_offer;
    let mut vac = w.firms[fi].vacancies;
    // fix-labour: match-specific pay shocks, drawn per firm in applicant order (deterministic at any thread count)
    let sigma = w.cfg.match_wage_dispersion;
    let mut pay_rng = agent_rng(w.seed, w.tick, MATCH_PAY_STREAM, FIRM_ID_BASE + f as u64);
    let n_app = list.len();
    for (i, &h) in list.iter().enumerate() {
        if vac == 0 {
            // b5-udur diagnostics: applicants this firm did not reach because its openings were filled.
            w.flows.udur_rank_leftover += (n_app - i) as u32;
            w.flows.udur_rank_firms += 1;
            break;
        }
        let hi = h as usize;
        if st.hired_epoch[hi] == st.epoch {
            continue;
        }
        let hh = &w.households[hi];
        if !hh.alive || hh.employer == Some(f) {
            continue;
        }
        let eps = if sigma > 0.0 { lognormal_mean1(&mut pay_rng, sigma) } else { 1.0 };
        let offer = offered_wage(nom.min_wage, wage_offer * eps, hc[hi]);
        let accept = match hh.status {
            LaborStatus::Unemployed => offer >= f64::from(hh.reservation_wage),
            // fix-labour: non-participants accept direct offers above their reservation wage (N->E hires)
            LaborStatus::OutOfLaborForce => offer >= nonparticipant_reservation(nom.min_wage, w.cfg.udur_resv_decay, hh),
            LaborStatus::Employed => offer >= w.cfg.job_to_job_premium * hh.wage,
            _ => false,
        };
        if !accept {
            continue;
        }
        let was_employed = hh.status == LaborStatus::Employed;
        // Fix-labour counters, read before the hire resets the household's spell.
        let (from, edu, dur, ui) = (hh.status, edu_idx(hh.education), hh.months_unemployed, hh.benefit_months_left);
        let (last_wage, loser) = (hh.last_wage, hh.employer);
        common::hire_worker(w, h, f, offer);
        if let Some(l) = loser {
            // fix-labour: a poach opens a vacancy at the loser at once, so it can refill it this month if it has not
            // yet been processed in the random firm order (its applicant list is already fixed). Integration: a loser
            // that plans layoffs does not post a replacement.
            if w.firms[l as usize].alive && w.firms[l as usize].planned_layoffs == 0 {
                w.firms[l as usize].vacancies += 1;
                st.poach_open[l as usize] += 1;
            }
        }
        st.match_mult[hi] = eps;
        st.hired_epoch[hi] = st.epoch;
        vac -= 1;
        st.hires_at[fi] += 1;
        w.flows.hires += 1;
        match from {
            LaborStatus::Employed => w.flows.ee += 1,
            // Unemployed at the start of the step: bookkeeping has counted at least one month. A worker separated
            // earlier this step (months_unemployed reset to 0) is rehired outside the start-of-step stock, so it is
            // not a job-finding hire from unemployment and is left out of the UI, duration and education buckets.
            LaborStatus::Unemployed if dur >= 1 => {
                w.flows.hires_from_u += 1;
                w.flows.hires_u_edu[edu] += 1;
                w.flows.hires_u_ui[ui_bucket(ui)] += 1;
                if let Some(b) = dur_bucket(dur) {
                    w.flows.hires_u_dur[b] += 1;
                    if last_wage > 0.0 && offer > 0.0 {
                        w.flows.udur_rewage_sum[b] += (offer / last_wage).ln();
                        w.flows.udur_rewage_n[b] += 1;
                    }
                }
            }
            LaborStatus::Unemployed => {}
            _ => w.flows.hires_from_n += 1,
        }
        if was_employed {
            w.flows.separations += 1;
        }
    }
    w.firms[fi].vacancies = vac;
}

/// fix-labour: reservation offer of a non-participant of working age, from the last wage decayed by the
/// unemployment decay (no benefits), and never below the minimum wage.
fn nonparticipant_reservation(min_wage: f64, resv_decay: f64, hh: &Household) -> f64 {
    let decay = resv_decay.powi(hh.months_unemployed.min(10_000) as i32);
    let r = min_wage.max(hh.last_wage * decay);
    if r.is_finite() {
        r
    } else {
        min_wage
    }
}

/// Step 5 (§6): on each job anniversary a stayer's wage resets to the firm's offer × human capital,
/// but never falls by more than 2% and never below the minimum wage. Also returns the number of unemployed
/// households at month end (status is final once matching is done).
fn stayer_wage_reset(w: &mut World, st: &LaborState) -> u32 {
    let nom = crate::fiscal::nominal(w);
    let firms = &w.firms;
    let hc = &st.hc;
    w.households
        .par_iter_mut()
        .enumerate()
        .map(|(h, hh)| {
            if hh.alive
                && hh.status == LaborStatus::Employed
                && hh.months_in_job != 0
                && hh.months_in_job % 12 == 0
            {
                if let Some(f) = hh.employer {
                    let fm = &firms[f as usize];
                    if fm.alive {
                        let target = offered_wage(nom.min_wage, fm.wage_offer * st.match_mult[h], hc[h]);
                        let new_wage = target.max(STAYER_MAX_CUT * hh.wage).max(nom.min_wage);
                        if new_wage.is_finite() {
                            hh.wage = new_wage;
                        }
                    }
                }
            }
            (hh.alive && hh.status == LaborStatus::Unemployed) as u32
        })
        .sum()
}

/// fix-labour (diagnostics, observation only): composition-fixed wage growth. Tracks each household's wage and
/// employer over the last 12 months and reports the mean 12-month wage change of job-stayers (employed now, with
/// 12+ months in the job, and at the same employer 12 months ago). Unlike the employed-mean wage, it does not move
/// when the mix of who is employed changes. Draws no random numbers and changes no state of the world.
#[derive(Clone, Debug, Default)]
pub struct StayerTrack {
    /// Snapshots (wage, employer or u32::MAX if not employed) of the last 12 month-ends, oldest first.
    hist: std::collections::VecDeque<Vec<(f64, u32)>>,
}

impl StayerTrack {
    /// Record the households after this month's step. Returns the mean % wage change of stayers over 12 months,
    /// or None while the history is short or no stayers exist.
    pub fn observe(&mut self, hh: &[Household]) -> Option<f64> {
        let cur: Vec<(f64, u32)> = hh
            .iter()
            .map(|h| match (h.alive, h.status, h.employer) {
                (true, LaborStatus::Employed, Some(f)) => (h.wage, f),
                _ => (0.0, u32::MAX),
            })
            .collect();
        let mut out = None;
        if self.hist.len() == 12 {
            let old = &self.hist[0];
            let (mut sum, mut n) = (0.0f64, 0u32);
            for (i, h) in hh.iter().enumerate() {
                // Households born since the snapshot have no entry (the household list grows): not stayers.
                let (w0, f0) = old.get(i).copied().unwrap_or((0.0, u32::MAX));
                let f1 = cur[i].1;
                let stayer = h.alive && h.status == LaborStatus::Employed && h.months_in_job >= 12;
                if stayer && f0 != u32::MAX && f0 == f1 && w0 > 0.0 {
                    sum += 100.0 * (h.wage / w0 - 1.0);
                    n += 1;
                }
            }
            if n > 0 {
                out = Some(sum / f64::from(n));
            }
            self.hist.pop_front();
        }
        self.hist.push_back(cur);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::fiscal::Nominal;
    use crate::ledger;

    /// b6-reentry M4d: attachment scars at exit, recovers with months out, and scales with tightness; off is 1.
    #[test]
    fn attachment_scars_recovers_and_tracks_tightness() {
        let mut cfg = Config { udur_att_on: true, udur_att_scar: 0.5, udur_att_spell: 6, udur_att_tau: 12.0, ..Default::default() };
        cfg.udur_att_eta = 0.5;
        assert_eq!(attachment(&cfg, 3, 0, 1.0), 1.0, "short spell is unscarred");
        assert_eq!(attachment(&cfg, 0, 0, 1.0), 1.0, "no memory");
        assert!((attachment(&cfg, 8, 0, 1.0) - 0.5).abs() < 1e-12, "scar at exit");
        let rec = 0.5 + 0.5 * (1.0 - (-1.0f64).exp());
        assert!((attachment(&cfg, 8, 12, 1.0) - rec).abs() < 1e-12, "recovery at tau");
        assert!(attachment(&cfg, 8, 120, 1.0) > 0.99, "near full attachment after long out");
        assert!((attachment(&cfg, 8, 0, 2.0) - 1.0).abs() < 1e-12, "tight market offsets scar");
        cfg.udur_att_on = false;
        assert_eq!(attachment(&cfg, 8, 0, 0.5), 1.0, "off is legacy");
    }

    /// b6-reentry M4d: the tightness factor is (θ_{t−1}/θ̄)^η clamped to [0.5, 2], and 1 when off or unset.
    #[test]
    fn theta_factor_is_clamped_and_off_by_default() {
        let mut cfg = Config { udur_att_on: true, udur_att_eta: 0.5, ..Default::default() };
        let mut st = LaborState { theta_last: 2.0, theta_ref: 1.0, ..Default::default() };
        assert!((theta_factor(&cfg, &st) - 2f64.sqrt()).abs() < 1e-12);
        st.theta_last = 1000.0;
        assert_eq!(theta_factor(&cfg, &st), 2.0, "upper clamp");
        st.theta_last = 1e-6;
        assert_eq!(theta_factor(&cfg, &st), 0.5, "lower clamp");
        st.theta_ref = 0.0;
        assert_eq!(theta_factor(&cfg, &st), 1.0, "reference not set");
        cfg.udur_att_on = false;
        st.theta_ref = 1.0;
        st.theta_last = 2.0;
        assert_eq!(theta_factor(&cfg, &st), 1.0, "off");
    }
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

    /// Emulates firms::plan: every firm posts `rate` × its headcount as vacancies, no layoffs.
    fn post_vacancies(w: &mut World, rate: f64) -> u32 {
        let mut posted = 0;
        for f in w.firms.iter_mut() {
            f.vacancies = (rate * f.workers.len() as f64).round() as u32;
            f.openings = f.vacancies;
            f.planned_layoffs = 0;
            posted += f.vacancies;
        }
        posted
    }

    /// Workers and employers agree in both directions, nobody is listed twice, and the ledger balances.
    fn assert_labor_invariants(w: &World) {
        let mut listed = 0usize;
        for (f, firm) in w.firms.iter().enumerate() {
            let mut seen = std::collections::HashSet::new();
            for &h in &firm.workers {
                let hh = &w.households[h as usize];
                assert_eq!(hh.employer, Some(f as FirmId), "worker {h} listed by firm {f}");
                assert_eq!(hh.status, LaborStatus::Employed, "listed worker {h} not Employed");
                assert!(seen.insert(h), "worker {h} duplicated in firm {f}");
            }
            listed += firm.workers.len();
        }
        let employed = w
            .households
            .iter()
            .filter(|h| h.employer.is_some())
            .inspect(|h| {
                let f = h.employer.unwrap() as usize;
                assert!(w.firms[f].workers.contains(&h.id), "household {} missing from firm {f}", h.id);
                assert_eq!(h.status, LaborStatus::Employed);
            })
            .count();
        assert_eq!(listed, employed, "workers lists and employer links disagree");
        ledger::check_consistency(w).expect("ledger consistency");
    }

    #[test]
    fn unemployed_exits_fall_in_recessions() {
        // fix-labour: with a negative cycle exponent, job finding below its slow average lowers U->N exits
        // (procyclical participation), and above it raises them.
        assert!(cyclical_exit_scale(-0.5, 0.10, 0.20) < 1.0);
        assert!(cyclical_exit_scale(-0.5, 0.30, 0.20) > 1.0);
        assert_eq!(cyclical_exit_scale(0.0, 0.10, 0.20), 1.0);
    }

    #[test]
    fn hires_and_separations_happen_and_stay_consistent() {
        let mut w = world(7);
        assert_labor_invariants(&w);
        let (mut hires, mut seps) = (0u32, 0u32);
        for _ in 0..24 {
            let posted = post_vacancies(&mut w, 0.04);
            w.flows = Flows::default();
            step(&mut w);
            // Each poach opens one vacancy at the loser (fix-labour), which may be filled the same month, so the bound is
            // posted vacancies plus job-to-job moves.
            assert!(w.flows.hires <= posted + w.flows.ee, "more hires than vacancies posted plus poaches");
            hires += w.flows.hires;
            seps += w.flows.separations;
            assert_labor_invariants(&w);
            w.tick += 1;
        }
        assert!(hires > 0, "no hires over 24 months");
        assert!(seps > 0, "no separations over 24 months");
    }

    #[test]
    fn planned_layoffs_remove_workers_and_go_to_unemployment() {
        let mut cfg = Config {
            n_households: 500,
            ..Default::default()
        };
        cfg.separation_rate = 0.0;
        cfg.on_the_job_search = 0.0;
        let mut w = tiny_world(cfg, 3);
        let f = (0..w.firms.len())
            .max_by_key(|&f| w.firms[f].workers.len())
            .unwrap();
        let before = w.firms[f].workers.clone();
        let k = 3.min(before.len());
        for fm in w.firms.iter_mut() {
            fm.vacancies = 0;
            fm.planned_layoffs = 0;
        }
        w.firms[f].planned_layoffs = 3;
        w.flows = Flows::default();
        step(&mut w);

        assert_eq!(w.firms[f].workers.len(), before.len() - k);
        assert_eq!(w.flows.layoffs as usize, k);
        assert_eq!(w.flows.separations as usize, k);
        assert_eq!(w.flows.hires, 0);
        let laid_off: Vec<HhId> = before
            .iter()
            .copied()
            .filter(|h| !w.firms[f].workers.contains(h))
            .collect();
        assert_eq!(laid_off.len(), k);
        for h in laid_off {
            let hh = &w.households[h as usize];
            assert_eq!(hh.status, LaborStatus::Unemployed);
            assert_eq!(hh.employer, None);
            assert_eq!(u32::from(hh.benefit_months_left), w.cfg.benefit_months);
        }
        assert_labor_invariants(&w);
    }

    #[test]
    fn dead_households_and_firms_are_skipped() {
        let cfg = Config {
            n_households: 500,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 5);
        w.cfg.separation_rate = 0.0;
        let dead_hh: Vec<usize> = (0..w.households.len())
            .filter(|&h| w.households[h].status == LaborStatus::Unemployed)
            .take(20)
            .collect();
        for &h in &dead_hh {
            w.households[h].alive = false;
        }
        let dead_firm = 0usize;
        w.firms[dead_firm].alive = false;
        let before_workers = w.firms[dead_firm].workers.len();
        post_vacancies(&mut w, 0.5);
        w.firms[dead_firm].vacancies = 50;
        step(&mut w);

        for &h in &dead_hh {
            let hh = &w.households[h];
            assert_eq!(hh.employer, None, "dead household {h} was hired");
            assert_eq!(hh.months_unemployed, 0, "dead household {h} aged");
        }
        assert!(w.firms[dead_firm].workers.len() <= before_workers, "dead firm hired");
        assert_eq!(w.labor.unfilled[dead_firm], 0);
    }

    #[test]
    fn non_participants_are_hired_directly_when_contact_is_on() {
        let mut cfg = Config {
            n_households: 2_000,
            ..Default::default()
        };
        cfg.separation_rate = 0.0;
        cfg.nonparticipant_contact = 1.0;
        cfg.applications_nonparticipant = 3;
        cfg.on_the_job_search = 0.0;
        cfg.match_wage_dispersion = 0.0;
        let mut w = tiny_world(cfg, 9);
        // Hiring firms and non-participants of working age; no unemployed households.
        for h in w.households.iter_mut() {
            if h.status == LaborStatus::Unemployed {
                h.status = LaborStatus::OutOfLaborForce;
            }
        }
        let mut hires_n = 0u32;
        for _ in 0..12 {
            post_vacancies(&mut w, 0.3);
            w.flows = Flows::default();
            step(&mut w);
            hires_n += w.flows.hires_from_n;
            assert_eq!(w.flows.hires_from_u, 0, "no unemployed households exist to hire");
            assert_labor_invariants(&w);
            w.tick += 1;
        }
        assert!(hires_n > 0, "non-participants with contact on were never hired");
    }

    #[test]
    fn unemployed_exits_leave_the_labour_force_without_benefits() {
        let mut cfg = Config {
            n_households: 1_000,
            ..Default::default()
        };
        cfg.separation_rate = 0.0;
        cfg.unemp_exit_base = 1.0; // every unemployed household exits this month
        cfg.unemp_exit_duration = 0.0;
        cfg.unemp_exit_exhaust = 0.0;
        cfg.on_the_job_search = 0.0;
        let mut w = tiny_world(cfg, 13);
        let unemployed_before = w.households.iter().filter(|h| h.status == LaborStatus::Unemployed).count();
        assert!(unemployed_before > 0);
        for fm in w.firms.iter_mut() {
            fm.vacancies = 0;
            fm.openings = 0;
            fm.planned_layoffs = 0;
        }
        w.flows = Flows::default();
        step(&mut w);
        assert_eq!(w.flows.u_to_n as usize, unemployed_before);
        assert!(w.households.iter().all(|h| h.status != LaborStatus::Unemployed || h.months_unemployed > 0));
        assert_labor_invariants(&w);
    }

    #[test]
    fn reservation_wage_follows_decay_and_benefits() {
        let cfg = Config::default();
        let mut hh = world(1).households[0].clone();
        hh.last_wage = 10_000.0;
        hh.months_unemployed = 0;
        hh.bls_unemployed = 0;
        hh.benefit_months_left = 0;
        let nom = Nominal::base(&cfg);
        assert!((reservation_wage(&cfg, &nom, &hh) - 10_000.0).abs() < 1e-9);
        hh.months_unemployed = 12;
        assert!((reservation_wage(&cfg, &nom, &hh) - 10_000.0 * 0.97f64.powi(12)).abs() < 1e-6);
        hh.months_unemployed = 100;
        hh.benefit_months_left = 6;
        assert!((reservation_wage(&cfg, &nom, &hh) - cfg.benefit_cap).abs() < 1e-9);
        hh.last_wage = 0.0;
        hh.benefit_months_left = 0;
        assert!((reservation_wage(&cfg, &nom, &hh) - cfg.min_wage_monthly).abs() < 1e-9);
    }

    #[test]
    fn unemployment_stays_bounded_with_steady_vacancies() {
        let mut w = world(11);
        let mut rates = Vec::new();
        let mut total_hires = 0u64;
        for t in 0..240 {
            post_vacancies(&mut w, 0.04);
            w.flows = Flows::default();
            step(&mut w);
            total_hires += w.flows.hires as u64;
            let unemp = w
                .households
                .iter()
                .filter(|h| h.alive && h.status == LaborStatus::Unemployed)
                .count();
            let lf = w
                .households
                .iter()
                .filter(|h| h.alive && matches!(h.status, LaborStatus::Employed | LaborStatus::Unemployed))
                .count()
                .max(1);
            let u = unemp as f64 / lf as f64;
            rates.push(u);
            if t % 40 == 0 || t == 239 {
                eprintln!(
                    "month {t:3}: unemployment {:.2}%  vacancies(unfilled) {}  tightness {:.3}  hires {}",
                    100.0 * u,
                    w.flows.vacancies,
                    tightness(&w),
                    w.flows.hires
                );
            }
            w.tick += 1;
        }
        assert!(total_hires > 0);
        let tail = &rates[rates.len() - 60..];
        let mean = tail.iter().sum::<f64>() / tail.len() as f64;
        assert!(mean.is_finite() && mean < 0.15, "unemployment drifted to {mean:.3}");
        assert_labor_invariants(&w);
    }

    /// b5-udur M3: scarring decays by δ each unemployed month, floors at 1 − L, and recovers while employed.
    #[test]
    fn scarring_floors_and_recovers() {
        let mut cfg = Config { n_households: 500, ..Default::default() };
        cfg.udur_hc_loss = 0.05;
        cfg.udur_hc_loss_max = 0.2;
        cfg.udur_hc_recover = 0.1;
        let mut w = tiny_world(cfg, 41);
        let mut st = LaborState::default();
        ensure_sizes(&w, &mut st);
        let h = 0usize;
        w.households[h].alive = true;
        w.households[h].status = LaborStatus::Unemployed;
        w.households[h].months_employed = 0;
        w.households[h].last_wage = 2_000.0;
        for _ in 0..60 {
            w.households[h].months_unemployed += 1;
            w.households[h].bls_unemployed += 1;
            bookkeeping(&mut w, &mut st, 1.0);
        }
        assert!((f64::from(st.scar[h]) - 0.8).abs() < 1e-6, "scar {}", st.scar[h]);
        w.households[h].status = LaborStatus::Employed;
        bookkeeping(&mut w, &mut st, 1.0);
        assert!((f64::from(st.scar[h]) - 0.82).abs() < 1e-6, "recovery {}", st.scar[h]);
    }

    /// b5-udur M3: a replacement entrant in a reused slot starts with scar 1, not the previous occupant's scar.
    #[test]
    fn scar_does_not_leak_into_reused_slot() {
        let mut cfg = Config { n_households: 500, ..Default::default() };
        cfg.udur_hc_loss = 0.05;
        cfg.udur_hc_loss_max = 0.2;
        let mut w = tiny_world(cfg, 43);
        let mut st = LaborState::default();
        ensure_sizes(&w, &mut st);
        let h = 0usize;
        st.scar[h] = 0.8;
        w.households[h].alive = true;
        w.households[h].status = LaborStatus::Unemployed;
        w.households[h].months_unemployed = 0;
        w.households[h].bls_unemployed = 0;
        w.households[h].months_employed = 0;
        w.households[h].last_wage = 0.0;
        bookkeeping(&mut w, &mut st, 1.0);
        assert!((f64::from(st.scar[h]) - 1.0).abs() < 1e-6, "entrant inherited scar {}", st.scar[h]);
    }

    /// b6-reentry M4b: an exit records the unemployment spell; the memory counts months out, and is cleared once the
    /// household is unemployed again (or dead).
    #[test]
    fn reentry_memory_records_exit_counts_months_out_and_clears() {
        let cfg = Config { n_households: 500, ..Default::default() };
        let mut w = tiny_world(cfg, 47);
        let mut st = LaborState::default();
        ensure_sizes(&w, &mut st);
        let h = 0usize;
        w.households[h].alive = true;
        w.households[h].status = LaborStatus::Unemployed;
        w.households[h].months_unemployed = 9;
        w.households[h].months_employed = 0;
        w.households[h].last_wage = 2_000.0;
        st.exit_flag[h] = true;
        unemployed_exits(&mut w, &mut st);
        assert_eq!(w.households[h].status, LaborStatus::OutOfLaborForce);
        assert_eq!((st.reentry_prev[h], st.reentry_n[h]), (9, 0), "exit did not record the spell");
        bookkeeping(&mut w, &mut st, 1.0);
        bookkeeping(&mut w, &mut st, 1.0);
        assert_eq!((st.reentry_prev[h], st.reentry_n[h]), (9, 2), "months out not counted");
        w.households[h].status = LaborStatus::Unemployed;
        bookkeeping(&mut w, &mut st, 1.0);
        assert_eq!((st.reentry_prev[h], st.reentry_n[h]), (0, 0), "memory survived re-entry");
    }

    /// b5-udur M1: with equal human capital and one opening, the applicant with the shorter spell is hired.
    #[test]
    fn ranking_by_duration_hires_shorter_spell_first() {
        let mut cfg = Config { n_households: 500, ..Default::default() };
        cfg.udur_rank_kappa = 2.0;
        cfg.match_wage_dispersion = 0.0;
        let mut w = tiny_world(cfg, 21);
        let mut st = LaborState::default();
        ensure_sizes(&w, &mut st);
        st.epoch = 1;
        let (a, b) = (0usize, 1usize);
        for (h, months) in [(a, 12u16), (b, 1u16)] {
            let hh = &mut w.households[h];
            hh.alive = true;
            hh.status = LaborStatus::Unemployed;
            hh.employer = None;
            hh.months_unemployed = months;
            hh.benefit_months_left = 0;
            hh.reservation_wage = 0.0;
            st.hc[h] = 1.0;
        }
        let f = w.firms.iter().position(|fm| fm.alive).expect("a live firm");
        w.firms[f].vacancies = 1;
        w.firms[f].wage_offer = 1.0e6;
        let mut list = vec![a as HhId, b as HhId];
        match_firm(&mut w, &mut st, f as FirmId, &mut list);
        assert_eq!(w.households[b].employer, Some(f as FirmId), "shorter spell not hired first");
        assert_eq!(w.households[a].employer, None, "longer spell hired over the shorter one");
    }
}
