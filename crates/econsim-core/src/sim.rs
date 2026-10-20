//! Simulation driver: monthly tick (docs/DESIGN.md §3), scenarios and shock comparison.

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::metrics::Metrics;
use crate::shocks::ScheduledShock;
use crate::types::*;
use crate::world;
use crate::*;

/// Size the global rayon pool for hybrid CPUs. A month is a sequence of short fork-join passes, so a pool that
/// includes slower efficiency cores waits on them at every join: on Apple silicon, the performance cores plus one
/// is ≈10% faster than all cores. Does nothing when `RAYON_NUM_THREADS` is set, on uniform CPUs, or if the pool
/// was already built.
pub fn configure_thread_pool() {
    if std::env::var_os("RAYON_NUM_THREADS").is_some() {
        return;
    }
    let all = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let perf = performance_cores().filter(|&p| p > 0 && p < all);
    if let Some(p) = perf {
        let _ = rayon::ThreadPoolBuilder::new().num_threads((p + 1).min(all)).build_global();
    }
}

/// Number of performance cores on a hybrid CPU (macOS `hw.perflevel0.physicalcpu`), if known.
fn performance_cores() -> Option<usize> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.perflevel0.physicalcpu"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

/// Reset per-month flow accumulators and size order buffers.
pub fn begin_month(w: &mut World) {
    let nh = w.households.len();
    let nf = w.firms.len();
    // Independent per-element resets (no reductions), so they run in parallel.
    w.households.par_iter_mut().for_each(|h| {
        h.consumption_budget = 0.0;
    });
    w.hh_month.par_iter_mut().for_each(|m| *m = HhMonth::default());
    w.firms.par_iter_mut().for_each(|f| {
        f.revenue = 0.0;
        f.wage_bill = 0.0;
        f.input_cost = 0.0;
        f.interest_paid = 0.0;
        f.depreciation_cost = 0.0;
        f.taxes = 0.0;
        f.profit = 0.0;
        f.dividends = 0.0;
        f.output = 0.0;
        f.sales = 0.0;
        f.demand = 0.0;
        f.investment_order = 0.0;
    });
    // `bank_loss_carry`: losses booked after last month's P&L (business write-offs in `firms::close_books`) open this
    // month's loan losses, so the P&L recognises them once; legacy zeroes them. A dead bank has no P&L to carry into.
    let carry = w.cfg.bank_loss_carry;
    for b in w.banks.iter_mut() {
        b.interest_income = 0.0;
        b.interest_expense = 0.0;
        let unrecognised = b.loan_losses - b.loan_losses_recognised;
        b.loan_losses = if carry && b.alive && unrecognised.is_finite() { unrecognised } else { 0.0 };
        b.loan_losses_recognised = 0.0;
        b.profit = 0.0;
        b.dividends = 0.0;
    }
    w.gov.revenue = 0.0;
    w.gov.outlays = 0.0;
    w.gov.interest_paid = 0.0;
    w.gov.purchases = 0.0;
    w.gov.transfers = 0.0;
    w.gov.deficit = 0.0;
    w.row.imports_this_month = 0.0;
    w.row.exports_this_month = 0.0;
    w.cb.qe_this_month = 0.0;
    w.cb.helicopter_total = 0.0;
    w.flows = Flows::default();

    let o = &mut w.orders;
    o.household.clear();
    o.household.resize(nh, [0.0; N_CONSUMER]);
    o.firm_inputs.clear();
    o.firm_inputs.resize(nf, [0.0; N_SECTORS]);
    o.firm_investment.clear();
    o.firm_investment.resize(nf, [0.0; N_SECTORS]);
    o.government = [0.0; N_SECTORS];
    o.exports = [0.0; N_SECTORS];
    w.fills.firm_capital_received.clear();
    w.fills.firm_capital_received.resize(nf, 0.0);
    w.fills.household_unspent.clear();
    w.fills.household_unspent.resize(nh, 0.0);
}

/// Advance the world by one month.
pub fn step(w: &mut World) {
    debug_assert_eq!(w.hh_month.len(), w.households.len());
    begin_month(w);
    exog::step(w);
    world::after_exog(w);
    monetary::set_policy(w);
    monetary::update_curve(w);
    // Hook H2 (spec 2.6): with the bilateral dollar link acting, the blocs' index replaces the ABM's exchange rate in `world::step`;
    // the ABM's FX base is still captured there (stage 4-1, D9).
    if !world::dollar_active(w) {
        monetary::exchange_rate(w);
    } else {
        monetary::capture_fx_base(w);
    }
    world::step(w);
    banking::set_rates(w);
    demography::step(w);
    debug_assert_eq!(w.hh_month.len(), w.households.len());
    firms::plan(w);
    labor::step(w);
    firms::produce(w);
    fiscal::pay_transfers(w);
    housing::step(w);
    consumption::decide(w);
    fiscal::plan_spending(w);
    goods::clear(w);
    banking::service(w);
    fiscal::collect_and_finance(w);
    monetary::markets(w);
    firms::close_books(w);
    consumption::record_capital_income(w);
    world::settle(w);
    metrics::record(w);
    if cfg!(debug_assertions) {
        if let Err(e) = ledger::check_consistency(w) {
            panic!("stock-flow inconsistency at tick {}: {e}", w.tick);
        }
        if let Err(e) = world::check_identities(w) {
            panic!("world identity violated at tick {}: {e}", w.tick);
        }
    }
    if w.cfg.sfc_enforce {
        enforce_identities(w);
    }
    w.tick += 1;
}

/// `Config::sfc_enforce`: panics, naming the tick and the identity, when this month's record carries `sfc_error` (the
/// checks of `ledger::check_consistency`, fused into the passes of `metrics::record`) or when the world block's identities
/// fail. Both are already computed (the world check is O(blocs)), so a consistent month costs nothing. The full
/// `ledger::check_consistency` runs only after the fused check has failed, to name the identity.
fn enforce_identities(w: &World) {
    if w.history.last().is_some_and(|m| m.sfc_error != 0.0) {
        let what = match ledger::check_consistency(w) {
            Err(e) => e,
            // Only the summation order differs between the two checks, so this is a residual at the tolerance.
            Ok(()) => "sfc_error is set: a bank's deposit liabilities or loan book, or the Fund's shares, differ from the sum \
                       over their holders by more than 1e-6 relative (ledger::check_consistency, summing in another order, \
                       passes)"
                .to_string(),
        };
        panic!("stock-flow inconsistency at tick {}: {what} (sfc_enforce)", w.tick);
    }
    if let Err(e) = world::check_identities(w) {
        panic!("world identity violated at tick {}: {e} (sfc_enforce)", w.tick);
    }
}

/// Names of the tick phases timed by `step_timed` (21 with the world block's phase after "fx").
pub const PHASES: [&str; 21] = [
    "begin", "exog", "policy", "curve", "fx", "world", "rates", "demography", "firms.plan", "labor", "firms.produce", "transfers",
    "housing", "consumption", "gov.spend", "goods", "banking", "fiscal", "markets", "firms.close", "metrics",
];

/// `step` with wall-clock seconds per phase accumulated into `t` (profiling).
pub fn step_timed(w: &mut World, t: &mut [f64; 21]) {
    use std::time::Instant;
    let fns: [fn(&mut World); 21] = [
        begin_month,
        |w| {
            exog::step(w);
            world::after_exog(w);
        },
        monetary::set_policy,
        monetary::update_curve,
        |w| {
            if !world::dollar_active(w) {
                monetary::exchange_rate(w);
            } else {
                monetary::capture_fx_base(w);
            }
        },
        world::step,
        banking::set_rates,
        demography::step,
        firms::plan,
        labor::step,
        firms::produce,
        fiscal::pay_transfers,
        housing::step,
        consumption::decide,
        fiscal::plan_spending,
        goods::clear,
        banking::service,
        fiscal::collect_and_finance,
        monetary::markets,
        |w| {
            firms::close_books(w);
            consumption::record_capital_income(w);
            world::settle(w);
        },
        metrics::record,
    ];
    for (i, f) in fns.iter().enumerate() {
        let t0 = Instant::now();
        f(w);
        t[i] += t0.elapsed().as_secs_f64();
    }
    w.tick += 1;
}

pub fn run(w: &mut World, months: u32) {
    for _ in 0..months {
        step(w);
    }
}

/// A scenario: config + scheduled shocks + horizon.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Scenario {
    #[serde(default)]
    pub config: Config,
    #[serde(default)]
    pub shocks: Vec<ScheduledShock>,
    #[serde(default = "default_months")]
    pub months: u32,
    #[serde(default = "default_seeds")]
    pub seeds: Vec<u64>,
}

fn default_months() -> u32 {
    60
}
fn default_seeds() -> Vec<u64> {
    vec![1, 2, 3, 4]
}

/// Result of comparing a shocked scenario to a common-random-numbers baseline.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comparison {
    /// Mean over seeds of baseline metrics per month.
    pub baseline: Vec<Metrics>,
    /// Mean over seeds of shocked metrics per month.
    pub shocked: Vec<Metrics>,
    pub seeds: Vec<u64>,
    /// b9-warmup: months the common world was stepped before the reported months (`Config::warmup_months`). Omitted
    /// when 0, so the legacy response is unchanged.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub warmup_months: u32,
    /// us-now: the start state of the comparison. Omitted for `typical`, so the legacy response is unchanged.
    #[serde(default, skip_serializing_if = "crate::start::StartState::is_typical")]
    pub start_state: crate::start::StartState,
    /// us-now: how each seed's common world was found (vintage, window length, draws used). Omitted for the typical start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_info: Option<crate::start::StartInfo>,
}

impl Comparison {
    /// The comparison's JSON object without `baseline` and `shocked`, its other keys in the body's own order. A streamed compare
    /// sends these once, after the year lines (R13-S). It is cut from the serialisation with the two arrays empty, so the order and
    /// the skipped keys are the struct's by construction.
    pub fn rest_json(&self) -> Vec<u8> {
        const HEAD: &[u8] = br#"{"baseline":[],"shocked":[],"#;
        let bare = Comparison {
            baseline: Vec::new(),
            shocked: Vec::new(),
            seeds: self.seeds.clone(),
            warmup_months: self.warmup_months,
            start_state: self.start_state,
            start_info: self.start_info.clone(),
        };
        let text = serde_json::to_vec(&bare).expect("comparison serialises");
        assert!(text.starts_with(HEAD), "a comparison starts with baseline and shocked, then the other keys");
        let mut rest = Vec::with_capacity(text.len());
        rest.push(b'{');
        rest.extend_from_slice(&text[HEAD.len()..]);
        rest
    }
}

fn is_zero_u32(x: &u32) -> bool {
    *x == 0
}

/// Build a world and run burn-in so it starts near its steady state.
pub fn new_world(cfg: &Config, seed: u64) -> World {
    init::build_world(cfg.clone(), seed)
}

/// b9-warmup: advance a freshly built world `months` more in the reported regime (every burn-in switch is already off).
/// The tick is not rebased: absolute-tick state keeps its meaning, and `compare` shifts the reported `tick`/`date` instead.
pub fn warm_up(w: &mut World, months: u32) {
    for _ in 0..months {
        step(w);
    }
}

/// The common world of one seed (the `w0` of `compare`): the world before reported month 0, and for `us_now` how it was found.
pub type CommonWorld = (World, Option<crate::start::SeedStart>);

/// The months stepped before reported month 0: the window length for `us_now` (where `Config::warmup_months` is ignored),
/// else `Config::warmup_months`. The reported ticks and dates are rebased by this many months.
pub fn warm_months(cfg: &Config) -> u32 {
    if cfg.start_state == crate::start::StartState::UsNow {
        cfg.start_targets.as_ref().map_or(0, |t| t.window_months())
    } else {
        cfg.warmup_months
    }
}

/// The common world of one seed, built as `compare` has always built it: `start::build_us_now` for `us_now`, else
/// `new_world` plus `warm_up` for `Config::warmup_months`. A function of the config and the seed alone (no shocks).
pub fn common_world(cfg: &Config, seed: u64) -> CommonWorld {
    // rz1 F1b: the common world is built and warmed up with the legacy real-FISIM measure (the key only changes recorded real
    // series, so the economy is the same) and the key applies from reported month 0 (`begin_reported_window`, called by `run_seed`),
    // so the recorded history is on one definition.
    let mut start_cfg = cfg.clone();
    start_cfg.fisim_constant_margin = false;
    let cfg = &start_cfg;
    if cfg.start_state == crate::start::StartState::UsNow {
        let (w, s) = crate::start::build_us_now(cfg, seed);
        (w, Some(s))
    } else {
        let mut w0 = new_world(cfg, seed);
        warm_up(&mut w0, cfg.warmup_months); // nothing when 0
        (w0, None)
    }
}

/// One year of a comparison, for streaming (R13-S): the reported months `from..to` (half-open; 12 months, the last year may be
/// shorter), the 1-based `year`, and each arm's records for those months. With one seed these are that seed's records, each
/// averaged with `Metrics::mean` over one seed, which is exactly what the full comparison holds for the same months.
#[derive(Clone, Debug)]
pub struct Year {
    pub year: u32,
    pub from: u32,
    pub to: u32,
    pub baseline: Vec<Metrics>,
    pub shocked: Vec<Metrics>,
}

/// One seed's records of both arms (rebased) and its us-now start.
type SeedRun = (Vec<Metrics>, Vec<Metrics>, Option<crate::start::SeedStart>);

/// Both arms of one seed, stepped a year at a time: the baseline arm is `w0` itself, the shocked arm a clone with the scenario's
/// shocks scheduled at `at_month + tick` (both as before). Stepping in 12-month pieces runs the same `step` calls in the same
/// order, so each arm's records are the same as one long `run`. With no shocks (R13-S fast path) there is one arm, and the
/// shocked records are a copy of the baseline records. `on_year(from, to, baseline, shocked)` runs after both arms have finished
/// each year. The reported ticks and dates are rebased by `warm`.
fn run_seed(sc: &Scenario, mut w0: World, warm: u32, on_year: &mut dyn FnMut(u32, u32, &[Metrics], &[Metrics])) -> SeedRun {
    begin_reported_window(&mut w0, sc.config.fisim_constant_margin); // rz1 F1b (a no-op write of the off value when the key is off)
    let start = w0.tick; // 0 when warm == 0; the shocks are scheduled at at_month + start as before
    let fast = sc.shocks.is_empty();
    let (mut base, mut shocked) = if fast {
        (w0, None)
    } else {
        let base = w0.clone();
        (base, Some(w0))
    };
    if let Some(shocked) = shocked.as_mut() {
        for s in &sc.shocks {
            let mut s = s.clone();
            s.at_month += start;
            shocked.shocks.schedule(s);
        }
    }
    let mut cb = base.history.len();
    let mut cs = shocked.as_ref().map_or(0, |s| s.history.len());
    let mut hist_b = Vec::with_capacity(sc.months as usize);
    let mut hist_s = Vec::with_capacity(sc.months as usize);
    let mut from = 0u32;
    while from < sc.months {
        let to = (from + 12).min(sc.months);
        let n = to - from;
        if from == 0 && sc.config.fisim_constant_margin && n > 0 {
            // rz1 F1b: the baseline arm's first reported month captures the base margins (before any shock acts); the shocked arm
            // uses the same base, so the paired gap holds the margins fixed. Identical stepping for the baseline.
            run(&mut base, 1);
            if let Some(s) = shocked.as_mut() {
                s.metrics_state.fisim_base = base.metrics_state.fisim_base;
            }
            run(&mut base, n - 1);
        } else {
            run(&mut base, n);
        }
        let mut year_b = base.history[cb..].to_vec();
        cb = base.history.len();
        let mut year_s = match shocked.as_mut() {
            Some(s) => {
                run(s, n);
                let v = s.history[cs..].to_vec();
                cs = s.history.len();
                v
            }
            None => year_b.clone(),
        };
        if warm > 0 {
            for m in year_b.iter_mut().chain(year_s.iter_mut()) {
                m.tick = m.tick.saturating_sub(warm); // reported month 0 stays tick 0
                m.date -= f64::from(warm) / 12.0; // and date 2026.0
            }
        }
        on_year(from, to, &year_b, &year_s);
        hist_b.extend(year_b);
        hist_s.extend(year_s);
        from = to;
    }
    (hist_b, hist_s, None)
}

/// The mean over seeds of the per-seed records, as the comparison holds them.
fn assemble(sc: &Scenario, runs: Vec<SeedRun>, warm: u32) -> Comparison {
    let us_now = sc.config.start_state == crate::start::StartState::UsNow;
    let n = runs.len().max(1) as f64;
    let len = runs.iter().map(|r| r.0.len().min(r.1.len())).min().unwrap_or(0);
    let mut baseline = Vec::with_capacity(len);
    let mut shocked = Vec::with_capacity(len);
    for t in 0..len {
        baseline.push(Metrics::mean(runs.iter().map(|r| &r.0[t]), n));
        shocked.push(Metrics::mean(runs.iter().map(|r| &r.1[t]), n));
    }
    let start_info = if us_now {
        sc.config.start_targets.as_ref().map(|t| crate::start::StartInfo {
            as_of: t.as_of.clone(),
            window_months: warm,
            per_seed: runs.iter().filter_map(|r| r.2.clone()).collect(),
            trade_ref: t
                .trade_ref
                .as_deref()
                .filter(|id| *id != "none" && sc.config.world_block_enabled && sc.config.world_link_trade)
                .and_then(|id| world::usnow::vintage(id).map(|v| crate::start::TradeRefInfo { id: id.to_string(), as_of: v.as_of.clone() })),
        })
    } else {
        None
    };
    Comparison {
        baseline,
        shocked,
        seeds: sc.seeds.clone(),
        warmup_months: warm,
        start_state: sc.config.start_state,
        start_info,
    }
}

/// rz1 F1b: start the reported window of a common world that was built and warmed up with `fisim_constant_margin` off: apply the
/// key from here on and clear the base, so the first recorded month captures the margins of reported month 0.
pub fn begin_reported_window(w: &mut World, fisim_constant_margin: bool) {
    w.cfg.fisim_constant_margin = fisim_constant_margin;
    w.metrics_state.fisim_base = Default::default();
}

/// Run baseline and shocked worlds for every seed in parallel and average the metrics.
///
/// us-now: with `start_state = us_now` the common world of each seed comes from `start::build_us_now` (a steered window of
/// `start_targets.window` months, replacing the warm-up), and `Config::warmup_months` is ignored. The typical path is unchanged.
pub fn compare(sc: &Scenario) -> Comparison {
    compare_with(sc, &|seed| common_world(&sc.config, seed), &mut |_| {})
}

/// `compare` with the common world of each seed supplied by `common` (the server caches it), and each year reported to `on_year`
/// as it completes. `on_year` is called only when the scenario has exactly one seed (the year means are then that seed's records);
/// with more seeds the years need every seed first, so it is not called. The result is the same for any `common` that returns
/// the common world of `common_world`, and the same for any `on_year`.
pub fn compare_with(sc: &Scenario, common: &(dyn Fn(u64) -> CommonWorld + Sync), on_year: &mut dyn FnMut(&Year)) -> Comparison {
    let warm = warm_months(&sc.config);
    let runs: Vec<SeedRun> = if sc.seeds.len() == 1 {
        sc.seeds
            .iter()
            .map(|&seed| {
                let (w0, seed_start) = common(seed);
                let mut emit = |from: u32, to: u32, b: &[Metrics], s: &[Metrics]| {
                    let mean = |rs: &[Metrics]| rs.iter().map(|m| Metrics::mean(std::iter::once(m), 1.0)).collect::<Vec<_>>();
                    on_year(&Year { year: from / 12 + 1, from, to, baseline: mean(b), shocked: mean(s) });
                };
                let mut seed_run = run_seed(sc, w0, warm, &mut emit);
                seed_run.2 = seed_start;
                seed_run
            })
            .collect()
    } else {
        sc.seeds
            .par_iter()
            .map(|&seed| {
                let (w0, seed_start) = common(seed);
                let mut seed_run = run_seed(sc, w0, warm, &mut |_, _, _, _| {});
                seed_run.2 = seed_start;
                seed_run
            })
            .collect()
    };
    assemble(sc, runs, warm)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small consistent world stepped once, then with bank 0's deposit liability moved off the sum of its customers'
    /// deposits by 1% (no ledger entry), as a stock-flow bug would.
    fn corrupted_world(sfc_enforce: bool) -> World {
        let mut w = crate::testkit::tiny_world(Config { n_households: 200, sfc_enforce, ..Default::default() }, 1);
        step(&mut w);
        assert_eq!(w.history.last().unwrap().sfc_error, 0.0);
        let b = &mut w.banks[0];
        b.deposits += 0.01 * b.deposits.abs() + 1.0;
        w
    }

    /// sfc_enforce: the next month stops with a panic that names the tick and the identity, in release builds too (the
    /// full debug check reports the same identity first in debug builds).
    #[test]
    fn sfc_enforce_panics_on_an_injected_inconsistency() {
        let mut w = corrupted_world(true);
        let tick = w.tick;
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step(&mut w))).expect_err("an inconsistent month must panic");
        let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.starts_with(&format!("stock-flow inconsistency at tick {tick}: ")), "{msg}");
        assert!(msg.contains("bank 0: deposit liabilities"), "{msg}");
    }

    /// sfc_enforce off: a release build records the inconsistency in `sfc_error` and goes on, as the release did. Debug builds
    /// always panic on the full check, so this runs in release builds only.
    #[cfg(not(debug_assertions))]
    #[test]
    fn sfc_enforce_off_records_the_inconsistency_and_goes_on() {
        let mut w = corrupted_world(false);
        step(&mut w);
        step(&mut w);
        assert!(w.history.iter().rev().take(2).all(|m| m.sfc_error == 1.0));
    }

    /// Smoke test for the CLI `irf` / server compare path: a 1-seed, 12-month compare crosses the first January,
    /// where the stayer fields are null, and must average and deserialise without panicking.
    #[test]
    fn compare_survives_null_observations() {
        let sc = Scenario {
            config: crate::config::Config { n_households: 400, ..Default::default() },
            shocks: Vec::new(),
            months: 12,
            seeds: vec![1],
        };
        let c = compare(&sc);
        assert_eq!(c.baseline.len(), 12);
        assert_eq!(c.shocked.len(), 12);
        assert!(c.baseline[0].obs.stayer_wage_change_p50.is_nan());
    }

    #[test]
    fn compare_survives_months_without_hires() {
        // Tiny worlds have months with no new hires (newhire_wage_per_eff is NaN); the averaged paths must still
        // deserialise, so the small-world irf and hirf sizes do not panic.
        let sc = Scenario {
            config: crate::config::Config { n_households: 100, ..Default::default() },
            shocks: Vec::new(),
            months: 36,
            seeds: vec![1, 2, 3, 4],
        };
        let c = compare(&sc);
        assert_eq!(c.baseline.len(), 36);
        assert_eq!(c.shocked.len(), 36);
    }

    /// J4 (R-19): the compare mean keeps both r* keys. Before the rename the observation copy came back as 0.0.
    #[test]
    fn compare_mean_keeps_r_star_est_in_both_keys() {
        let c = compare(&small(0, 12, 3, Vec::new()));
        for m in c.baseline.iter().chain(c.shocked.iter()) {
            assert!(m.r_star_est > 0.0, "r_star_est {}", m.r_star_est);
            assert_eq!(m.r_star_est.to_bits(), m.obs.obs_r_star_est.to_bits());
        }
        let text = serde_json::to_string(&c).unwrap();
        assert_eq!(text.matches("\"r_star_est\":").count(), c.baseline.len() + c.shocked.len());
    }

    fn small(warmup_months: u32, burn_in_months: u32, months: u32, shocks: Vec<ScheduledShock>) -> Scenario {
        Scenario {
            config: Config { n_households: 400, burn_in_months, warmup_months, ..Default::default() },
            shocks,
            months,
            seeds: vec![1],
        }
    }

    /// `bank_loss_carry`: through the real month order, every loan loss booked (`flows.loan_losses`, which includes the
    /// write-offs of firms failing in `firms::close_books`, after the bank P&L) reaches the bank P&L exactly once:
    /// opening carry + losses booked = losses recognised + closing carry. Legacy drops the late write-offs.
    #[test]
    fn every_loan_loss_reaches_the_bank_pnl_once() {
        let run_months = |carry: bool| -> (f64, f64, f64) {
            let cfg = Config { n_households: 400, burn_in_months: 12, bank_loss_carry: carry, ..Default::default() };
            let mut w = new_world(&cfg, 2);
            let pending =
                |w: &World| w.banks.iter().filter(|b| b.alive).map(|b| b.loan_losses - b.loan_losses_recognised).sum::<f64>();
            let open = pending(&w);
            // A firm with a business loan is left insolvent, so it fails in the first month's close_books.
            let f = (0..w.firms.len()).find(|&f| w.firms[f].alive).unwrap();
            let bank = w.firms[f].bank;
            ledger::create_loan(&mut w, bank, Borrower::Firm(f as FirmId), LoanKind::Business, 1.0e6, 0.06, 60);
            let d = w.firms[f].deposits;
            ledger::pay(&mut w, ledger::Account::Firm(f as FirmId), ledger::Account::Government, d + 1.0e9);
            let (mut booked, mut recognised) = (0.0, 0.0);
            for t in 0..4 {
                step(&mut w);
                if t == 0 {
                    assert!(!w.firms[f].alive, "the insolvent firm fails in close_books");
                }
                booked += w.flows.loan_losses;
                recognised += w.banks.iter().filter(|b| b.alive).map(|b| b.loan_losses_recognised).sum::<f64>();
            }
            (open + booked, recognised, pending(&w))
        };
        let (inflow, recognised, close) = run_months(true);
        assert!(inflow >= 1.0e6, "the failed firm's loan is written off: {inflow}");
        assert!(
            (inflow - recognised - close).abs() <= 1e-9 * inflow,
            "opening carry + booked {inflow} != recognised {recognised} + closing carry {close}"
        );
        let (inflow, recognised, close) = run_months(false);
        assert!(recognised + close <= inflow - 1.0e6 * (1.0 - 1e-9), "legacy: booked {inflow}, recognised {recognised} + {close}");
    }

    /// b9-warmup: with 0 the compare is the legacy one: reported month 0 is tick 0 at 2026.0, and the serialised
    /// response has no `warmup_months` key.
    #[test]
    fn warmup_zero_is_the_legacy_compare() {
        let c = compare(&small(0, Config::default().burn_in_months, 12, Vec::new()));
        assert_eq!(c.baseline.len(), 12);
        assert_eq!(c.baseline[0].tick, 0);
        assert_eq!(c.baseline[0].date, 2026.0);
        assert_eq!(c.warmup_months, 0);
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("warmup_months").is_none());
    }

    /// b9-warmup: reported months start after the warm-up, and their tick and date are rebased to month 0.
    #[test]
    fn warmup_rebases_tick_and_date() {
        let c = compare(&small(12, 12, 6, Vec::new()));
        assert_eq!(c.baseline.len(), 6);
        assert_eq!(c.shocked.len(), 6);
        assert_eq!(c.baseline[0].tick, 0);
        assert_eq!(c.baseline[5].tick, 5);
        assert!((c.baseline[0].date - 2026.0).abs() < 1e-12);
        assert_eq!(c.warmup_months, 12);
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["warmup_months"], 12);
    }

    /// b9-warmup: the baseline arm is the same world stepped 12 + 6 months: its reported months equal the plain run's
    /// months 12-17.
    #[test]
    fn warmup_baseline_equals_the_stepped_world() {
        let cfg = Config { n_households: 400, burn_in_months: 12, ..Default::default() };
        let mut w = new_world(&cfg, 1);
        run(&mut w, 12);
        let hb = w.history.len();
        run(&mut w, 6);
        let plain: Vec<(f64, f64, f64)> = w.history[hb..]
            .iter()
            .map(|m| (m.gdp_real_bn_ar, m.unemployment_rate, m.policy_rate))
            .collect();
        let c = compare(&small(12, 12, 6, Vec::new()));
        for (t, (gdp, u, pol)) in plain.iter().enumerate() {
            let b = &c.baseline[t];
            for (got, want) in [(b.gdp_real_bn_ar, *gdp), (b.unemployment_rate, *u), (b.policy_rate, *pol)] {
                assert!((got - want).abs() <= 1e-9 * want.abs().max(1.0), "month {t}: {got} vs {want}");
            }
        }
    }

    /// b9-warmup: a shock at `at_month` 0 is scheduled at the first reported month (absolute tick 12), not at the first
    /// warm-up month; the sign of the policy-rate response matches the legacy compare.
    #[test]
    fn warmup_shock_starts_after_the_warmup() {
        // 1,000 households: the 400-household world sits on the zero policy floor after a 12-month warm-up, so the
        // shock would not show there.
        let shock = vec![ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 100.0, months: 12 } }];
        let scenario = |warmup_months: u32| Scenario {
            config: Config { n_households: 1000, burn_in_months: 12, warmup_months, ..Default::default() },
            shocks: shock.clone(),
            months: 6,
            seeds: vec![1],
        };
        let warm = compare(&scenario(12));
        let gap_warm = warm.shocked[0].policy_rate - warm.baseline[0].policy_rate;
        assert!(gap_warm > 0.5, "warm-up shock gap {gap_warm}");
        let legacy = compare(&scenario(0));
        let gap_legacy = legacy.shocked[0].policy_rate - legacy.baseline[0].policy_rate;
        assert!(gap_legacy > 0.0, "legacy shock gap {gap_legacy}");
    }

    /// The comparison as it was before R13-S, kept verbatim: the year-stepped `compare` must give the same bytes as this.
    fn legacy_compare(sc: &Scenario) -> Comparison {
        let us_now = sc.config.start_state == crate::start::StartState::UsNow;
        let warm = if us_now {
            sc.config.start_targets.as_ref().map_or(0, |t| t.window_months())
        } else {
            sc.config.warmup_months
        };
        let runs: Vec<(Vec<Metrics>, Vec<Metrics>, Option<crate::start::SeedStart>)> = sc
            .seeds
            .par_iter()
            .map(|&seed| {
                let (w0, seed_start) = if us_now {
                    let (w, s) = crate::start::build_us_now(&sc.config, seed);
                    (w, Some(s))
                } else {
                    let mut w0 = new_world(&sc.config, seed);
                    warm_up(&mut w0, warm);
                    (w0, None)
                };
                let start = w0.tick;
                let mut base = w0.clone();
                let mut shocked = w0;
                for s in &sc.shocks {
                    let mut s = s.clone();
                    s.at_month += start;
                    shocked.shocks.schedule(s);
                }
                let hb = base.history.len();
                let hs = shocked.history.len();
                run(&mut base, sc.months);
                run(&mut shocked, sc.months);
                let mut hist_b = base.history[hb..].to_vec();
                let mut hist_s = shocked.history[hs..].to_vec();
                if warm > 0 {
                    for m in hist_b.iter_mut().chain(hist_s.iter_mut()) {
                        m.tick = m.tick.saturating_sub(warm);
                        m.date -= f64::from(warm) / 12.0;
                    }
                }
                (hist_b, hist_s, seed_start)
            })
            .collect();
        let n = runs.len().max(1) as f64;
        let len = runs.iter().map(|r| r.0.len().min(r.1.len())).min().unwrap_or(0);
        let mut baseline = Vec::with_capacity(len);
        let mut shocked = Vec::with_capacity(len);
        for t in 0..len {
            baseline.push(Metrics::mean(runs.iter().map(|r| &r.0[t]), n));
            shocked.push(Metrics::mean(runs.iter().map(|r| &r.1[t]), n));
        }
        let start_info = if us_now {
            sc.config.start_targets.as_ref().map(|t| crate::start::StartInfo {
                as_of: t.as_of.clone(),
                window_months: warm,
                per_seed: runs.iter().filter_map(|r| r.2.clone()).collect(),
                trade_ref: None,
            })
        } else {
            None
        };
        Comparison { baseline, shocked, seeds: sc.seeds.clone(), warmup_months: warm, start_state: sc.config.start_state, start_info }
    }

    fn json(c: &Comparison) -> String {
        serde_json::to_string(c).unwrap()
    }

    /// A typical scenario: 400 households, a 12-month burn-in and warm-up, a Fed-style shock, 30 months (the last year is partial).
    fn typical_sc(seeds: Vec<u64>, shocks: Vec<ScheduledShock>) -> Scenario {
        Scenario {
            config: Config { n_households: 400, burn_in_months: 12, warmup_months: 12, ..Default::default() },
            shocks,
            months: 30,
            seeds,
        }
    }

    fn fed_shock() -> Vec<ScheduledShock> {
        vec![ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 100.0, months: 12 } }]
    }

    /// A us_now scenario with the 12-month test targets, 400 households and 14 months.
    fn us_sc(seeds: Vec<u64>, shocks: Vec<ScheduledShock>) -> Scenario {
        let config = Config {
            n_households: 400,
            burn_in_months: 12,
            start_state: crate::start::StartState::UsNow,
            start_targets: Some(crate::start::tests::targets12()),
            ..Default::default()
        };
        Scenario { config, shocks, months: 14, seeds }
    }

    /// A world-profile scenario: the world block on (typical start), 400 households, 13 months, one shock.
    fn world_sc() -> Scenario {
        let config = Config { n_households: 400, burn_in_months: 12, warmup_months: 12, world_block_enabled: true, ..Default::default() };
        Scenario { config, shocks: fed_shock(), months: 13, seeds: vec![1, 2] }
    }

    /// R13-S: the year-stepped compare gives the same bytes as the legacy two-arm compare, typical, us_now and world-profile.
    #[test]
    fn year_stepped_compare_matches_the_legacy_compare() {
        for sc in [typical_sc(vec![1, 2], fed_shock()), us_sc(vec![1], fed_shock()), world_sc()] {
            assert_eq!(json(&compare(&sc)), json(&legacy_compare(&sc)));
        }
    }

    /// R13-S fast path: with no shocks the one-arm run gives the same bytes as the two-arm run, for several seeds, typical and us_now.
    #[test]
    fn no_change_fast_path_matches_the_two_arm_run() {
        for sc in [typical_sc(vec![1, 2, 3], Vec::new()), us_sc(vec![1, 2], Vec::new())] {
            let fast = compare(&sc);
            assert_eq!(json(&fast), json(&legacy_compare(&sc)));
            assert_eq!(serde_json::to_string(&fast.shocked).unwrap(), serde_json::to_string(&fast.baseline).unwrap());
        }
    }

    /// R13-S common-world cache: a common world built once and cloned gives the same bytes as a fresh build, typical, us_now and
    /// world-profile. The cached world is a clone, so this is the state a cache hit hands the arms.
    #[test]
    fn cached_common_world_gives_the_same_bytes() {
        for sc in [typical_sc(vec![1], fed_shock()), us_sc(vec![1], fed_shock()), world_sc()] {
            let stored: Vec<CommonWorld> = sc.seeds.iter().map(|&s| common_world(&sc.config, s)).collect();
            let cached = |seed: u64| {
                let i = sc.seeds.iter().position(|&s| s == seed).unwrap();
                (stored[i].0.clone(), stored[i].1.clone())
            };
            let mut ignore = |_: &Year| {};
            assert_eq!(json(&compare_with(&sc, &cached, &mut ignore)), json(&compare(&sc)));
        }
    }

    /// R13-S: the clone-size model (`World::approx_clone_bytes`, which the common-world cache charges) against a clone measured at 5,000
    /// households, with the warm-up the model was fitted on (burn-in 144, warm-up 24). The clone is made on this thread, so the counting
    /// allocator (alloc_count.rs) sees its bytes and nothing else. The model must be within 10% of the measured clone.
    #[test]
    fn clone_size_model_matches_a_measured_clone_at_5000_households() {
        let cfg = Config { n_households: 5000, warmup_months: 24, ..Default::default() };
        let (w, _) = common_world(&cfg, 1);
        let before = crate::alloc_count::live_bytes();
        let copy = w.clone();
        let measured = (crate::alloc_count::live_bytes() - before) as f64;
        let model = w.approx_clone_bytes() as f64;
        assert!((model - measured).abs() <= 0.10 * measured, "model {model:.0} bytes, measured clone {measured:.0} bytes");
        drop(copy);
    }

    /// R13-S: `rest_json` is the body without its two arrays. Its keys are the body's, in the body's order (the tail of the body
    /// text is the rest), and the rest with the arrays rebuilds the body as JSON values.
    #[test]
    fn rest_json_is_the_body_without_the_record_arrays() {
        for sc in [typical_sc(vec![1, 2], fed_shock()), us_sc(vec![1], fed_shock()), world_sc()] {
            let c = compare(&sc);
            let text = json(&c);
            let rest = String::from_utf8(c.rest_json()).unwrap();
            assert!(rest.starts_with('{') && rest.ends_with('}'), "{rest}");
            assert!(!rest.contains("\"baseline\"") && !rest.contains("\"shocked\""), "{rest}");
            assert!(text.ends_with(&format!(",{}", &rest[1..])), "same key order and tail:\n{text}\n{rest}");
            // Both sides are parsed from text, so both go through the same float parser.
            let full: serde_json::Value = serde_json::from_str(&text).unwrap();
            let mut rebuilt = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&rest).unwrap();
            let arrays = format!(r#"{{"baseline":{},"shocked":{}}}"#, serde_json::to_string(&c.baseline).unwrap(), serde_json::to_string(&c.shocked).unwrap());
            let arrays: serde_json::Value = serde_json::from_str(&arrays).unwrap();
            rebuilt.insert("baseline".to_string(), arrays["baseline"].clone());
            rebuilt.insert("shocked".to_string(), arrays["shocked"].clone());
            assert_eq!(serde_json::Value::Object(rebuilt), full);
        }
    }

    /// R13-S streaming: each year's records are the full comparison's records for those months (the same bytes, one by one), the
    /// years cover 1..=months with the right bounds, and the callback is not called when there is more than one seed.
    #[test]
    fn year_lines_are_the_full_comparison_months() {
        let sc = typical_sc(vec![1], fed_shock());
        let full = compare(&sc);
        let mut years: Vec<Year> = Vec::new();
        let c = compare_with(&sc, &|seed| common_world(&sc.config, seed), &mut |y: &Year| years.push(y.clone()));
        assert_eq!(json(&c), json(&full));
        assert_eq!(years.iter().map(|y| (y.year, y.from, y.to)).collect::<Vec<_>>(), [(1, 0, 12), (2, 12, 24), (3, 24, 30)]);
        for y in &years {
            assert_eq!(y.baseline.len() as u32, y.to - y.from);
            for (i, m) in y.baseline.iter().enumerate() {
                assert_eq!(serde_json::to_string(m).unwrap(), serde_json::to_string(&full.baseline[y.from as usize + i]).unwrap());
            }
            for (i, m) in y.shocked.iter().enumerate() {
                assert_eq!(serde_json::to_string(m).unwrap(), serde_json::to_string(&full.shocked[y.from as usize + i]).unwrap());
            }
        }
        let two_seeds = typical_sc(vec![1, 2], fed_shock());
        let mut calls = 0;
        compare_with(&two_seeds, &|seed| common_world(&two_seeds.config, seed), &mut |_| calls += 1);
        assert_eq!(calls, 0);
    }

    /// rz1 F1b (RZ-R8): with `fisim_constant_margin` and a 24-month warm-up the baseline arm's reported month 0 is identical to the
    /// key-off run; the base is captured at reported month 0 (not at the first warm-up tick) and shared with the shocked arm, so the
    /// only fields that differ anywhere, in either arm, are the four recorded real series.
    #[test]
    fn fisim_constant_margin_compare_with_warmup_24() {
        let shock = vec![ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 100.0, months: 12 } }];
        let scenario = |on: bool| Scenario {
            config: Config { n_households: 1000, burn_in_months: 12, warmup_months: 24, fisim_constant_margin: on, ..Default::default() },
            shocks: shock.clone(),
            months: 8,
            seeds: vec![1],
        };
        let (off, on) = (compare(&scenario(false)), compare(&scenario(true)));
        let allowed = ["consumption_real_bn_ar", "gdp_real_bn_ar", "gdp_real_growth_yoy", "real_consumption_per_hh"];
        for (arm, a, b) in [("baseline", &off.baseline, &on.baseline), ("shocked", &off.shocked, &on.shocked)] {
            let mut differing = std::collections::BTreeSet::new();
            for (t, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                let (xv, yv) = (serde_json::to_value(x).unwrap(), serde_json::to_value(y).unwrap());
                for (k, vx) in xv.as_object().unwrap() {
                    if vx != &yv[k] {
                        differing.insert(k.clone());
                        if arm == "baseline" {
                            assert!(t >= 1, "baseline month 0 must be identical, key {k}");
                        }
                    }
                }
            }
            assert!(differing.iter().all(|k| allowed.contains(&k.as_str())), "{arm}: unexpected fields differ: {differing:?}");
        }
        // The baseline month 0 is the legacy value. The shocked arm does not capture a base of its own (that would put the shock's
        // first-month margin move into the base): it uses the baseline's, so its month-0 real consumption is the legacy value less the
        // margin effect of the shock, and differs from the key-off run.
        assert_eq!(off.baseline[0].consumption_real_bn_ar.to_bits(), on.baseline[0].consumption_real_bn_ar.to_bits());
        assert_ne!(off.shocked[0].consumption_real_bn_ar.to_bits(), on.shocked[0].consumption_real_bn_ar.to_bits());
    }
}
