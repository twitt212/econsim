//! "Today's US economy" start state (docs/API.md "Start states"; design in docs/CALIBRATION.md, section us-now).
//!
//! `Config::start_state = us_now` makes `sim::compare` build each seed's common world by a *steered window*: after the usual burn-in,
//! the world is stepped through the months of `StartTargets::window` (24 by default) while the exogenous and policy drivers are
//! steered to the data, then released. Month 0 of the comparison is the first free month. Everything here is a pure function of
//! (config, seed): no clock, no network, no new random stream (the window re-draws re-key the seed, they do not add a stream).
//!
//! What is steered each window month (all through `SteerState`, read by four one-line hooks):
//! * the policy rate: a Taylor-rule intercept wedge solved so the shadow rate lands on the target (monetary::set_policy);
//! * the yield curve (3m, 2y, 5y, 10y, 30y): a persistent per-tenor offset solved so each yield lands on its target
//!   (monetary::update_curve), kept after release (default half-life 0 = permanent);
//! * the mortgage rate: a mortgage-spread offset (banking::set_rates), decaying with `mortgage_half_life` after release;
//! * oil: the AR(1) state `exog.oil_ar` is set so the oil price lands on the target (exog::step), so the model's own
//!   persistence carries it after release;
//! * the tariff: `ImportTariff` shocks following the quarterly path.
//! Unemployment is not steered; it is conditioned on: the window is re-drawn (the seed re-keyed) up to `max_attempts` times and,
//! if none lands within `accept_u` of the target at month 0, the world is rebuilt from a hopped seed up to `max_hops` times.
//! The best draw is kept when none is accepted.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::*;

/// Where month 0 of a comparison comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartState {
    /// The model's own state after the burn-in (and the warm-up, if any). The default; the code path is unchanged.
    #[default]
    Typical,
    /// Steered to `Config::start_targets`.
    UsNow,
}

impl StartState {
    pub fn is_typical(&self) -> bool {
        *self == StartState::Typical
    }
}

/// One entry per window month, oldest first. Rates are percent (4.25 = 4.25%), oil is $ per barrel, tariff is percent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartWindow {
    /// Labels such as "2024-11" (provenance only; the length is the window length).
    pub months: Vec<String>,
    /// Upper bound of the FOMC target range.
    pub policy_rate: Vec<f64>,
    pub y3m: Vec<f64>,
    pub y2: Vec<f64>,
    pub y5: Vec<f64>,
    pub y10: Vec<f64>,
    pub y30: Vec<f64>,
    pub mortgage_rate: Vec<f64>,
    pub oil_wti: Vec<f64>,
    pub tariff_pct: Vec<f64>,
}

/// Tuning of the steered window. These defaults are part of the design; tests may override them.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StartTuning {
    /// Accept a window draw when |unemployment at month 0 - target| <= this (percentage points).
    pub accept_u: f64,
    /// Window draws per world, and world rebuilds (hops) after that.
    pub max_attempts: u32,
    pub max_hops: u32,
    /// Half-life (months) of the mortgage-spread offset after release.
    pub mortgage_half_life: f64,
    /// Half-life (months) of the yield offsets after release; 0 = permanent.
    pub tp_half_life: f64,
    /// Monthly decay of the policy-rule wedge after release (0.97 = the curve's own PATH_DECAY).
    pub wedge_decay: f64,
}

impl Default for StartTuning {
    fn default() -> Self {
        StartTuning { accept_u: 0.5, max_attempts: 8, max_hops: 3, mortgage_half_life: 36.0, tp_half_life: 0.0, wedge_decay: 0.97 }
    }
}

/// The resolved data the steered window follows. Built by the web layer from FRED (or the committed fallback) and sent in the
/// request, so the simulation stays a pure function of its request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartTargets {
    /// Vintage of the data, e.g. "2026-10-12" (or "2026-10-12.2"). Provenance and cache-key only.
    pub as_of: String,
    pub window: StartWindow,
    /// Unemployment rate (percent) the world is conditioned on at month 0.
    pub unemployment_rate: f64,
    #[serde(default)]
    pub tuning: StartTuning,
    /// Today's start with the world block (docs/CALIBRATION.md, section world-usnow): the id of the dated trade reference the block starts from (today's duty by
    /// origin and goods group, trade shares, partners' retaliation), or "none". Absent: no reference (the merged behaviour); the server fills in the
    /// newest vintage for a request that has the world block and the trade link. Provenance and cache key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_ref: Option<String>,
}

impl StartTargets {
    pub fn window_months(&self) -> u32 {
        self.window.months.len() as u32
    }

    /// Range and length checks. The error text is returned to the API client as HTTP 400.
    pub fn validate(&self) -> Result<(), String> {
        let n = self.window.months.len();
        if ![12usize, 24, 36].contains(&n) {
            return Err("start_targets.window must have 12, 24 or 36 months".to_string());
        }
        if self.as_of.is_empty() || self.as_of.len() > 32 {
            return Err("start_targets.as_of must be 1-32 characters".to_string());
        }
        let w = &self.window;
        let cols: [(&str, &Vec<f64>, f64, f64); 9] = [
            ("policy_rate", &w.policy_rate, 0.0, 25.0),
            ("y3m", &w.y3m, -1.0, 25.0),
            ("y2", &w.y2, -1.0, 25.0),
            ("y5", &w.y5, -1.0, 25.0),
            ("y10", &w.y10, -1.0, 25.0),
            ("y30", &w.y30, -1.0, 25.0),
            ("mortgage_rate", &w.mortgage_rate, 0.0, 30.0),
            ("oil_wti", &w.oil_wti, 5.0, 400.0),
            ("tariff_pct", &w.tariff_pct, 0.0, 100.0),
        ];
        for (name, v, lo, hi) in cols {
            if v.len() != n {
                return Err(format!("start_targets.window.{name} must have {n} entries"));
            }
            if v.iter().any(|x| !x.is_finite() || *x < lo || *x > hi) {
                return Err(format!("start_targets.window.{name} values must be finite and within [{lo}, {hi}]"));
            }
        }
        let u = self.unemployment_rate;
        if !u.is_finite() || !(1.0..=25.0).contains(&u) {
            return Err("start_targets.unemployment_rate must be within [1, 25]".to_string());
        }
        let t = &self.tuning;
        if !(t.accept_u.is_finite() && t.accept_u > 0.0 && t.mortgage_half_life.is_finite() && t.mortgage_half_life >= 0.0
            && t.tp_half_life.is_finite() && t.tp_half_life >= 0.0 && t.wedge_decay.is_finite() && (0.0..=1.0).contains(&t.wedge_decay))
        {
            return Err("start_targets.tuning has an out-of-range value".to_string());
        }
        if t.max_attempts == 0 || t.max_attempts > 64 || t.max_hops > 8 {
            return Err("start_targets.tuning: max_attempts must be 1-64 and max_hops 0-8".to_string());
        }
        if let Some(id) = &self.trade_ref {
            if id != "none" && !crate::world::usnow::known(id) {
                return Err(format!("start_targets.trade_ref \"{id}\" is not a known vintage"));
            }
        }
        Ok(())
    }
}

/// Steering state carried by the world. All `None`/0 means no steering (the typical path).
#[derive(Clone, Debug, Default)]
pub struct SteerState {
    /// Shadow-rate target this month (decimal).
    pub policy: Option<f64>,
    /// Yield targets this month (decimal) by tenor: 3m, 2y, 5y, 10y, 30y.
    pub yields: [Option<f64>; N_TENORS],
    /// Mortgage-rate target this month (decimal).
    pub mortgage: Option<f64>,
    /// Oil price target this month ($ per barrel).
    pub oil: Option<f64>,
    /// Taylor-rule intercept wedge (decimal); persists after release and decays by `wedge_decay` per month.
    pub wedge: f64,
    pub wedge_decay: f64,
    /// Per-tenor yield offsets (decimal); persist after release and decay by `tp_decay` per month.
    pub tp_off: [f64; N_TENORS],
    pub tp_decay: f64,
    /// Mortgage-spread offset (decimal); decays by `mtg_decay` per month after release.
    pub mtg_off: f64,
    pub mtg_decay: f64,
}

/// Whether the steered window of a Today's start is being stepped (the policy-rate target is set in every window month and cleared when
/// the window ends). rz19 reads it to take the tariff reference after the window.
pub fn window_active(w: &World) -> bool {
    w.steer.policy.is_some()
}

fn decay_for(half_life: f64) -> f64 {
    if half_life <= 0.0 {
        1.0
    } else {
        0.5f64.powf(1.0 / half_life)
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

// ---- hooks called from the model (each does nothing unless steering is active or an offset is still decaying) ------------

/// monetary::set_policy: returns `i_star` plus the wedge. `rho` is `rate_smoothing`, `prev` the previous shadow rate, `innov` the
/// policy innovation of the month. While a policy target is set the wedge is solved so the shadow rate lands on it; afterwards the
/// wedge decays.
pub fn policy_wedge(w: &mut World, i_star: f64, rho: f64, prev: f64, innov: f64) -> f64 {
    if w.steer.policy.is_none() && w.steer.wedge == 0.0 {
        return i_star;
    }
    match w.steer.policy {
        Some(tgt) => {
            let s0 = rho * prev + (1.0 - rho) * i_star + innov;
            w.steer.wedge = (tgt - s0) / (1.0 - rho);
        }
        None => w.steer.wedge *= w.steer.wedge_decay,
    }
    i_star + w.steer.wedge
}

/// monetary::update_curve, after the curve is complete: put the yield offsets into the term premium and the yields.
pub fn curve_offsets(w: &mut World) {
    for j in 0..N_TENORS {
        if w.steer.yields[j].is_none() && w.steer.tp_off[j] == 0.0 {
            continue;
        }
        let natural = w.curve.yields[j];
        w.steer.tp_off[j] = match w.steer.yields[j] {
            Some(t) => t - natural,
            None => w.steer.tp_off[j] * w.steer.tp_decay,
        };
        w.curve.term_premium[j] += w.steer.tp_off[j];
        w.curve.yields[j] = natural + w.steer.tp_off[j];
    }
}

/// banking::set_rates: `mortgage_base` plus the mortgage-spread offset.
pub fn mortgage_offset(w: &mut World, mortgage_base: f64) -> f64 {
    if w.steer.mortgage.is_none() && w.steer.mtg_off == 0.0 {
        return mortgage_base;
    }
    match w.steer.mortgage {
        Some(t) => w.steer.mtg_off = t - mortgage_base,
        None => w.steer.mtg_off *= w.steer.mtg_decay,
    }
    mortgage_base + w.steer.mtg_off
}

/// exog::step, after the oil price index is known: put the whole log deviation into the AR state.
pub fn oil_state(w: &mut World, index: f64) {
    if let Some(t) = w.steer.oil {
        w.exog.oil_ar = (t / (w.exog.oil_base * index)).ln() - w.exog.oil_shock;
    }
}

// ---- the steered window ---------------------------------------------------------------------------------------------------------------

/// Step `w` through the window, steering, then release (the wedge, the yield offsets and the mortgage offset stay and decay).
pub fn run_window(w: &mut World, t: &StartTargets) {
    let win = &t.window;
    let tun = &t.tuning;
    w.steer.wedge_decay = tun.wedge_decay;
    w.steer.mtg_decay = decay_for(tun.mortgage_half_life);
    let mut prev = f64::NAN;
    for (m, &pct) in win.tariff_pct.iter().enumerate() {
        if pct != prev {
            w.shocks.schedule(ScheduledShock { at_month: w.tick + m as u32, shock: Shock::ImportTariff { pct } });
            prev = pct;
        }
    }
    for m in 0..win.months.len() {
        w.steer.policy = Some(win.policy_rate[m] / 100.0);
        w.steer.yields = [
            Some(win.y3m[m] / 100.0),
            Some(win.y2[m] / 100.0),
            Some(win.y5[m] / 100.0),
            Some(win.y10[m] / 100.0),
            Some(win.y30[m] / 100.0),
        ];
        w.steer.mortgage = Some(win.mortgage_rate[m] / 100.0);
        w.steer.oil = Some(win.oil_wti[m]);
        sim::step(w);
    }
    w.steer.policy = None;
    w.steer.yields = [None; N_TENORS];
    w.steer.mortgage = None;
    w.steer.oil = None;
    w.steer.tp_decay = decay_for(tun.tp_half_life);
}

/// How a seed's common world was found.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SeedStart {
    pub seed: u64,
    /// World rebuilds used (0 = the seed's own world).
    pub hop: u32,
    /// 1-based index of the window draw that was kept.
    pub attempts: u32,
    /// Whether the kept draw met `accept_u`.
    pub accepted: bool,
    /// Unemployment (percent) in the first free month (reported month 0).
    pub u_month0: f64,
}

/// Provenance of a us_now comparison, in the response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StartInfo {
    pub as_of: String,
    pub window_months: u32,
    pub per_seed: Vec<SeedStart>,
    /// Today's start with the world block: the dated trade reference the block started from (absent without it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trade_ref: Option<TradeRefInfo>,
}

/// The trade reference a Today's start with the world block was built on (id and the date of its data).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TradeRefInfo {
    pub id: String,
    pub as_of: String,
}

/// Build the common world of one seed for `start_state = us_now`: the world right after the window (before reported month 0).
/// `cfg.start_targets` must be present and valid (the server checks).
pub fn build_us_now(cfg: &Config, seed: u64) -> (World, SeedStart) {
    let t = cfg.start_targets.as_ref().expect("start_targets validated by the caller");
    let tun = &t.tuning;
    let mut best: Option<(f64, World, SeedStart)> = None;
    for hop in 0..=tun.max_hops {
        let s = if hop == 0 { seed } else { splitmix(seed ^ u64::from(hop).wrapping_mul(0xD6E8_FEB8_6659_FD93)) };
        let base = crate::init::build_world(cfg.clone(), s); // rz27: not `sim::new_world`, which anchors the oil-elastic Energy valuation; `sim::common_world` anchors after the window
        for k in 0..tun.max_attempts {
            let mut w = base.clone();
            if k > 0 {
                w.seed = splitmix(base.seed ^ u64::from(k).wrapping_mul(0xA24B_AED4_963E_E407));
            }
            run_window(&mut w, t);
            // Today's start with the world block: install the dated trade reference at the end of the window (no-op without the block).
            if let Some(id) = t.trade_ref.as_deref() {
                crate::world::rebase_usnow(&mut w, id);
            }
            let after = w.clone();
            sim::step(&mut w);
            let u = w.history.last().map_or(f64::NAN, |m| m.unemployment_rate);
            let score = (u - t.unemployment_rate).abs() / tun.accept_u;
            let ok = score <= 1.0;
            if !ok && best.as_ref().map_or(false, |b| b.0 <= score) {
                continue;
            }
            best = Some((score, after, SeedStart { seed, hop, attempts: k + 1, accepted: ok, u_month0: u }));
            if ok {
                let (_, w0, info) = best.unwrap();
                return (w0, info);
            }
        }
    }
    let (_, w0, info) = best.expect("at least one attempt");
    (w0, info)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn flat(n: usize, v: f64) -> Vec<f64> {
        vec![v; n]
    }

    /// A small valid target set: 12 months, constant data.
    pub(crate) fn targets12() -> StartTargets {
        let n = 12;
        StartTargets {
            as_of: "test".to_string(),
            window: StartWindow {
                months: (0..n).map(|i| format!("m{i}")).collect(),
                policy_rate: flat(n, 4.0),
                y3m: flat(n, 4.2),
                y2: flat(n, 4.7),
                y5: flat(n, 5.0),
                y10: flat(n, 5.2),
                y30: flat(n, 5.6),
                mortgage_rate: flat(n, 7.4),
                oil_wti: flat(n, 96.0),
                tariff_pct: flat(n, 8.0),
            },
            unemployment_rate: 4.2,
            tuning: StartTuning { max_attempts: 1, max_hops: 0, ..StartTuning::default() },
            trade_ref: None,
        }
    }

    fn cfg_us_now(n_hh: usize, burn: u32) -> Config {
        Config { n_households: n_hh, burn_in_months: burn, start_state: StartState::UsNow, start_targets: Some(targets12()), ..Default::default() }
    }

    #[test]
    fn validate_rejects_bad_targets() {
        assert!(targets12().validate().is_ok());
        let mut t = targets12();
        t.window.y10.pop();
        assert!(t.validate().unwrap_err().contains("y10"));
        let mut t = targets12();
        t.window.oil_wti[3] = f64::NAN;
        assert!(t.validate().is_err());
        let mut t = targets12();
        t.window.months.truncate(7);
        assert!(t.validate().unwrap_err().contains("12, 24 or 36"));
        let mut t = targets12();
        t.unemployment_rate = 0.2;
        assert!(t.validate().is_err());
        let mut t = targets12();
        t.tuning.max_attempts = 0;
        assert!(t.validate().is_err());
    }

    #[test]
    fn start_state_serde_names() {
        assert_eq!(serde_json::to_string(&StartState::Typical).unwrap(), "\"typical\"");
        assert_eq!(serde_json::to_string(&StartState::UsNow).unwrap(), "\"us_now\"");
        let c: Config = serde_json::from_str("{}").unwrap();
        assert_eq!(c.start_state, StartState::Typical);
        assert!(c.start_targets.is_none());
    }

    /// The window lands the steered variables on their targets in its last month, and the first free month stays close.
    #[test]
    fn window_hits_the_targets() {
        let cfg = cfg_us_now(1000, 12);
        let (w, info) = build_us_now(&cfg, 1);
        assert_eq!(info.hop, 0);
        assert_eq!(w.history.len(), 12);
        let m = w.history.last().unwrap();
        assert!((m.policy_rate - 4.0).abs() < 1e-9, "policy {}", m.policy_rate);
        for (j, want) in [4.2, 4.7, 5.0, 5.2, 5.6].iter().enumerate() {
            assert!((m.yields[j] - want).abs() < 1e-9, "yield {j}: {}", m.yields[j]);
        }
        assert!((m.mortgage_rate - 7.4).abs() < 0.2, "mortgage {}", m.mortgage_rate);
        assert!((m.oil_price - 96.0).abs() < 1e-6, "oil {}", m.oil_price);
        assert!((m.tariff_rate_pct - 8.0).abs() < 1e-9, "tariff {}", m.tariff_rate_pct);
    }

    /// Same inputs, same bytes; and the ledger holds in every window month (debug builds assert it inside `step`).
    #[test]
    fn deterministic_and_consistent() {
        let cfg = cfg_us_now(500, 12);
        let (a, ia) = build_us_now(&cfg, 3);
        let (b, ib) = build_us_now(&cfg, 3);
        assert_eq!(ia, ib);
        assert_eq!(serde_json::to_string(&a.history).unwrap(), serde_json::to_string(&b.history).unwrap());
        assert!(a.history.iter().all(|m| m.sfc_error == 0.0));
    }

    /// The typical path does not touch the steering hooks: a plain world and one built with the default start state agree.
    #[test]
    fn typical_world_is_unsteered() {
        let cfg = Config { n_households: 400, burn_in_months: 12, ..Default::default() };
        let mut w = sim::new_world(&cfg, 2);
        sim::run(&mut w, 6);
        assert_eq!(w.steer.wedge, 0.0);
        assert_eq!(w.steer.mtg_off, 0.0);
        assert!(w.steer.tp_off.iter().all(|x| *x == 0.0));
    }

    /// Rejection and hopping: an impossible tolerance returns the best draw, not accepted, after all attempts and hops.
    #[test]
    fn best_draw_kept_when_nothing_is_accepted() {
        let mut cfg = cfg_us_now(400, 12);
        {
            let t = cfg.start_targets.as_mut().unwrap();
            t.tuning.accept_u = 1e-9;
            t.tuning.max_attempts = 2;
            t.tuning.max_hops = 1;
        }
        let (w, info) = build_us_now(&cfg, 5);
        assert!(!info.accepted);
        assert!(info.hop <= 1 && info.attempts <= 2);
        assert_eq!(w.history.len(), 12);
    }

    /// A generous tolerance accepts the first draw.
    #[test]
    fn first_draw_accepted_with_a_wide_tolerance() {
        let mut cfg = cfg_us_now(400, 12);
        cfg.start_targets.as_mut().unwrap().tuning.accept_u = 50.0;
        let (_, info) = build_us_now(&cfg, 5);
        assert!(info.accepted && info.hop == 0 && info.attempts == 1);
    }

    /// `compare` with the us_now start: month 0 is tick 0, the window length is reported as the warm-up, provenance is present; a
    /// typical comparison has neither new key.
    #[test]
    fn compare_reports_the_start() {
        use crate::sim::{compare, Scenario};
        let sc = Scenario { config: cfg_us_now(400, 12), shocks: Vec::new(), months: 2, seeds: vec![1, 2] };
        let c = compare(&sc);
        assert_eq!(c.start_state, StartState::UsNow);
        assert_eq!(c.warmup_months, 12);
        assert_eq!(c.baseline[0].tick, 0);
        assert!((c.baseline[0].date - 2026.0).abs() < 1e-12);
        let info = c.start_info.as_ref().unwrap();
        assert_eq!((info.as_of.as_str(), info.window_months, info.per_seed.len()), ("test", 12, 2));
        let v = serde_json::to_value(&c).unwrap();
        assert_eq!(v["start_state"], "us_now");
        assert_eq!(v["warmup_months"], 12);
        let plain = compare(&Scenario {
            config: Config { n_households: 400, burn_in_months: 12, ..Default::default() },
            shocks: Vec::new(),
            months: 2,
            seeds: vec![1],
        });
        let pv = serde_json::to_value(&plain).unwrap();
        assert!(pv.get("start_state").is_none() && pv.get("start_info").is_none() && pv.get("warmup_months").is_none());
        // The same comparison twice: identical bytes.
        assert_eq!(serde_json::to_string(&compare(&sc)).unwrap(), serde_json::to_string(&c).unwrap());
    }

    /// Today's start with the world block: an unknown trade_ref is a 400, "none" and an absent key are fine.
    #[test]
    fn trade_ref_is_validated() {
        let mut t = targets12();
        assert!(t.validate().is_ok());
        t.trade_ref = Some("none".to_string());
        assert!(t.validate().is_ok());
        t.trade_ref = Some(crate::world::usnow::latest_id().to_string());
        assert!(t.validate().is_ok());
        t.trade_ref = Some("1999-01-01".to_string());
        assert!(t.validate().unwrap_err().contains("trade_ref"));
        // the absent key is skipped in the serialisation, so a request without it keeps its bytes (and its cache key)
        let plain = serde_json::to_value(targets12()).unwrap();
        assert!(plain.get("trade_ref").is_none());
    }

    /// `compare` with us_now, the world block and the trade link installs the reference at the end of the window and reports it; without the
    /// block, or with "none", the start is the one before (same bytes as a request that never named the key).
    #[test]
    fn compare_with_the_world_block_reports_the_trade_ref() {
        use crate::sim::{compare, Scenario};
        let world = |id: Option<&str>| {
            let mut c = cfg_us_now(300, 12);
            c.world_block_enabled = true;
            c.world_link_trade = true;
            c.world_retaliation = true;
            c.start_targets.as_mut().unwrap().trade_ref = id.map(str::to_string);
            Scenario { config: c, shocks: Vec::new(), months: 3, seeds: vec![1] }
        };
        let latest = crate::world::usnow::latest_id();
        let with = compare(&world(Some(latest)));
        let info = with.start_info.as_ref().unwrap();
        assert_eq!(info.trade_ref.as_ref().map(|r| r.id.as_str()), Some(latest));
        assert_eq!(info.trade_ref.as_ref().unwrap().as_of, "2026-08-24");
        let w = with.baseline[0].world.as_ref().unwrap();
        assert!(w.ret_tau[1] > 0.3 && w.imports_goods_bn.iter().sum::<f64>() > 0.0);
        let none = compare(&world(Some("none")));
        assert!(none.start_info.as_ref().unwrap().trade_ref.is_none());
        assert!(none.baseline[0].world.as_ref().unwrap().ret_tau[1] == 0.0);
        let absent = compare(&world(None));
        assert_eq!(serde_json::to_string(&absent).unwrap(), serde_json::to_string(&none).unwrap());
        // deterministic
        assert_eq!(serde_json::to_string(&compare(&world(Some(latest)))).unwrap(), serde_json::to_string(&with).unwrap());
    }
}
