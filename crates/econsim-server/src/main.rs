//! EconSim HTTP server (docs/API.md). Axum 0.7 + tokio; every world build, step, snapshot and compare runs on
//! the blocking pool. Simulation entries are guarded by per-entry mutexes, so a long run on one sim does not
//! block the others. Compare results are cached, shared between identical in-flight requests and capped in
//! concurrency (cache.rs).
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod cache;
mod fields;

use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::{
    body::{Body, Bytes},
    extract::{Path, Query, Request, State},
    http::{header, HeaderName, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use cache::{CompareCache, Served, Streamed};
use fields::FieldSet;
use econsim_core::config::Config;
use econsim_core::metrics::{self, Metrics, Snapshot};
use econsim_core::shocks::{ScheduledShock, Shock};
use econsim_core::sim;
use econsim_core::start::{StartState, StartTargets};
use econsim_core::types::World;
use econsim_core::Scenario;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;

const MAX_HOUSEHOLDS: usize = 200_000;
const MAX_MONTHS_PER_STEP: u32 = 240;
const MAX_COMPARE_MONTHS: u32 = 240;
const MAX_COMPARE_SEEDS: usize = 8;
/// b9-warmup: `config.warmup_months` must be 0 or a multiple of 12 up to this (`ECONSIM_WARMUP_MONTHS` likewise).
const MAX_WARMUP_MONTHS: u32 = 120;
/// Joint calibration 4 (R-19): the warm-up the server applies to compare requests that omit `config.warmup_months` when
/// `ECONSIM_WARMUP_MONTHS` is unset (or invalid). `Config::warmup_months` itself stays 0, so the CLI, the harnesses and the tests are
/// unchanged; `ECONSIM_WARMUP_MONTHS=0` restores the pre-J4 server.
const DEFAULT_WARMUP_MONTHS: u32 = 24;
/// us-now: the committed fallback snapshot (scripts/data/us_now.json), compiled in. A compare request with `start_state: "us_now"`
/// and no `start_targets` is resolved to its `targets` before the cache key is formed, so a key always contains the data it ran on.
const US_NOW_FALLBACK: &str = include_str!("../../../scripts/data/us_now.json");

#[derive(Deserialize)]
struct UsNowFile {
    targets: StartTargets,
}

/// The compiled-in fallback targets (None if the file is malformed; a test guards that it is not).
fn us_now_fallback() -> Option<&'static StartTargets> {
    static CELL: std::sync::OnceLock<Option<StartTargets>> = std::sync::OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str::<UsNowFile>(US_NOW_FALLBACK).ok().map(|f| f.targets)).as_ref()
}

/// Response header on `POST /api/compare`: `hit` when served from the cache, `miss` otherwise.
const CACHE_HEADER: &str = "x-econsim-cache";
/// The arrays of `Metrics` records in a compare response (the `fields` filter applies to these).
const COMPARE_RECORDS: &[&str] = &["baseline", "shocked"];

// ------------------------------------------------------------------------------------------------
// state
// ------------------------------------------------------------------------------------------------

struct SimEntry {
    economy: Economy,
    seed: u64,
}

/// An economy in the `sims` map: running, or stopped by a panic.
enum Economy {
    Live(World),
    /// A step panicked on it (with `Config::sfc_enforce`, a stock-flow inconsistency). Its world was half-stepped, so it is
    /// dropped, which frees its memory; this is the economy's info at that point, `stopped` saying why. It is no longer read,
    /// forked, shocked or stepped (`lock_usable`), only listed and deleted.
    Stopped(SimInfo),
}

impl SimEntry {
    fn new(world: World, seed: u64) -> Self {
        SimEntry { economy: Economy::Live(world), seed }
    }

    /// The world of a running economy; `lock_usable` hands out no other.
    fn world(&self) -> &World {
        match &self.economy {
            Economy::Live(w) => w,
            Economy::Stopped(_) => unreachable!("a stopped economy has no world"),
        }
    }

    fn world_mut(&mut self) -> &mut World {
        match &mut self.economy {
            Economy::Live(w) => w,
            Economy::Stopped(_) => unreachable!("a stopped economy has no world"),
        }
    }
}

/// An economy shared between the `sims` map and the requests that hold it.
struct SimCell {
    entry: Mutex<SimEntry>,
    /// Set when the economy stops, and read without its lock: the map does not touch a stopped economy (`SimStore::touch`), and
    /// must not wait for a running step to find out.
    stopped: AtomicBool,
}

impl SimCell {
    /// Stops the economy `id` held in `g` (the first reason is kept): its info, with `why`, replaces its world.
    fn stop(&self, id: &str, g: &mut SimEntry, why: &str) {
        if matches!(g.economy, Economy::Live(_)) {
            let info = SimInfo { stopped: Some(why.to_string()), ..info_of(id, g) };
            g.economy = Economy::Stopped(info);
        }
        self.stopped.store(true, Ordering::Relaxed);
    }
}

/// Pre-warm progress for `/api/health`: the entries finished (computed, cached or failed) of the `total` in the pre-warm file.
struct PrewarmProgress {
    total: usize,
    done: AtomicUsize,
}

impl PrewarmProgress {
    fn new(total: usize) -> Self {
        PrewarmProgress {
            total,
            done: AtomicUsize::new(0),
        }
    }
}

/// One economy in the `sims` map: its world, and when and in what order it was last touched (R17 idle expiry).
struct Sim {
    cell: Arc<SimCell>,
    touched_at: Instant,
    /// Touch order: the least recently touched economy has the smallest `seq`.
    seq: u64,
}

/// R17: the idle policy of the `sims` map, from `ECONSIM_SIM_TTL_HOURS` (default 6) and `ECONSIM_SIM_CAP` (default 200).
#[derive(Clone, Copy, Debug, PartialEq)]
struct SimPolicy {
    ttl: Duration,
    cap: usize,
}

const DEFAULT_SIM_TTL_HOURS: f64 = 6.0;
const DEFAULT_SIM_CAP: usize = 200;
/// How often the background sweep removes idle economies between requests (every request also sweeps).
const SIM_SWEEP_EVERY: Duration = Duration::from_secs(60);

impl Default for SimPolicy {
    fn default() -> Self {
        SimPolicy {
            ttl: Duration::from_secs_f64(DEFAULT_SIM_TTL_HOURS * 3600.0),
            cap: DEFAULT_SIM_CAP,
        }
    }
}

fn hours(d: Duration) -> f64 {
    d.as_secs_f64() / 3600.0
}

/// The clock of the idle expiry. The server reads the monotonic clock; a test sets its own, so no test sleeps.
trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// An economy removed from the `sims` map, with the reason for its log line. The caller drops it after the lock is released.
struct Removed {
    id: String,
    reason: String,
    _sim: Sim,
}

/// The `sims` map with its idle policy (R17). A touch is a create, a step, a read, a shock, a fork or a snapshot; a list or a
/// delete is not, nor is a request refused because the economy stopped. An economy idle for the TTL is removed, and creating one
/// more than the cap removes the least recently touched.
struct SimStore {
    map: HashMap<String, Sim>,
    policy: SimPolicy,
    clock: Arc<dyn Clock>,
    /// The touch counter behind `Sim::seq`.
    touches: u64,
}

impl SimStore {
    fn new(policy: SimPolicy, clock: Arc<dyn Clock>) -> Self {
        SimStore {
            map: HashMap::new(),
            policy,
            clock,
            touches: 0,
        }
    }

    fn next_touch(&mut self) -> u64 {
        self.touches += 1;
        self.touches
    }

    /// Remove every economy idle for the TTL or longer (one touched exactly TTL ago is removed), in id order.
    fn expire(&mut self) -> Vec<Removed> {
        let now = self.clock.now();
        let ttl = self.policy.ttl;
        let mut idle: Vec<String> = self
            .map
            .iter()
            .filter(|(_, s)| now.saturating_duration_since(s.touched_at) >= ttl)
            .map(|(id, _)| id.clone())
            .collect();
        idle.sort();
        let reason = format!("expired (untouched for {} h)", hours(ttl));
        idle.into_iter()
            .filter_map(|id| {
                let sim = self.map.remove(&id)?;
                Some(Removed { id, reason: reason.clone(), _sim: sim })
            })
            .collect()
    }

    /// Remove least recently touched economies until there is room for one more under the cap.
    fn make_room(&mut self) -> Vec<Removed> {
        let mut out = Vec::new();
        while self.map.len() >= self.policy.cap {
            let Some(id) = self.map.iter().min_by_key(|(_, s)| s.seq).map(|(id, _)| id.clone()) else { break };
            let Some(sim) = self.map.remove(&id) else { break };
            let reason = format!("evicted (least recently touched; cap {} reached)", self.policy.cap);
            out.push(Removed { id, reason, _sim: sim });
        }
        out
    }

    /// The entry under `id`, touched now unless it has stopped; None when there is no such economy. A stopped economy refuses the
    /// request, so a client polling it must not keep it from expiring, nor ahead of running economies under the cap.
    fn touch(&mut self, id: &str) -> Option<Arc<SimCell>> {
        let touched_at = self.clock.now();
        let cell = Arc::clone(&self.map.get(id)?.cell);
        if !cell.stopped.load(Ordering::Relaxed) {
            let seq = self.next_touch();
            if let Some(sim) = self.map.get_mut(id) {
                sim.touched_at = touched_at;
                sim.seq = seq;
            }
        }
        Some(cell)
    }

    /// Store a new economy, touched now.
    fn insert(&mut self, id: String, entry: SimEntry) {
        let touched_at = self.clock.now();
        let seq = self.next_touch();
        let stopped = AtomicBool::new(matches!(entry.economy, Economy::Stopped(_)));
        self.map.insert(
            id,
            Sim {
                cell: Arc::new(SimCell { entry: Mutex::new(entry), stopped }),
                touched_at,
                seq,
            },
        );
    }
}

/// R17: one line per removed economy. Called with the store lock released, so the worlds are dropped outside it.
fn report_removed(removed: Vec<Removed>) {
    for r in removed {
        println!("sim {} removed: {}", r.id, r.reason);
    }
}

/// R17: the idle sweep. The background task runs it every `SIM_SWEEP_EVERY`; returns the ids it removed.
fn sweep_sims(st: &AppState) -> Vec<String> {
    let removed = lock(&st.sims).expire();
    let ids = removed.iter().map(|r| r.id.clone()).collect();
    report_removed(removed);
    ids
}

struct AppState {
    sims: Mutex<SimStore>,
    counter: AtomicU64,
    compare: Arc<CompareCache>,
    /// b9-warmup: the `config.warmup_months` applied to compare requests that omit it (`ECONSIM_WARMUP_MONTHS`).
    warmup_default: u32,
    /// Set when a pre-warm pass is running (or has run); None without a usable `ECONSIM_PREWARM_FILE`.
    prewarm: Option<Arc<PrewarmProgress>>,
}

type Shared = Arc<AppState>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicked simulation must not wedge the server; the data is still readable.
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The economy `id` locked for a request, refused (500, JSON error) once a step has panicked on it: that world was
/// half-stepped and is gone. A mutex poisoned by a panic elsewhere stops the economy the same way.
fn lock_usable<'a>(id: &str, cell: &'a SimCell) -> Result<MutexGuard<'a, SimEntry>, ApiError> {
    let poisoned = cell.entry.is_poisoned();
    let mut g = lock(&cell.entry);
    if poisoned {
        cell.stop(id, &mut g, "a request panicked while it held this simulation");
    }
    match &g.economy {
        Economy::Live(_) => Ok(g),
        Economy::Stopped(info) => Err(ApiError::Internal(format!(
            "simulation '{id}' stopped: {}; it can only be deleted",
            info.stopped.as_deref().unwrap_or_default()
        ))),
    }
}

/// The message of a caught panic.
fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    match p.downcast_ref::<String>() {
        Some(s) => s.clone(),
        None => p.downcast_ref::<&str>().map_or_else(|| "panic".to_string(), |s| s.to_string()),
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Insert a simulation under a fresh short hex id and return its info. R17: first the idle sweep, then the cap (the least
/// recently touched economies go until there is room), both logged.
fn register(st: &AppState, entry: SimEntry) -> SimInfo {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut removed = Vec::new();
    let info = {
        let mut store = lock(&st.sims);
        removed.extend(store.expire());
        removed.extend(store.make_room());
        loop {
            let c = st.counter.fetch_add(1, Ordering::Relaxed);
            let id = format!("{:08x}", splitmix(c ^ nanos.rotate_left(21)) as u32);
            if !store.map.contains_key(&id) {
                let info = info_of(&id, &entry);
                store.insert(id, entry);
                break info;
            }
        }
    };
    report_removed(removed);
    info
}

/// The entry under `id`, which this access touches (R17). An economy idle past the TTL is removed first and answers 404.
fn get_entry(st: &AppState, id: &str) -> Result<Arc<SimCell>, ApiError> {
    let (found, removed) = {
        let mut store = lock(&st.sims);
        let removed = store.expire();
        (store.touch(id), removed)
    };
    report_removed(removed);
    found.ok_or_else(|| ApiError::NotFound(format!("no simulation with id '{id}'")))
}

// ------------------------------------------------------------------------------------------------
// errors and helpers
// ------------------------------------------------------------------------------------------------

#[derive(Debug)]
enum ApiError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl ApiError {
    fn message(&self) -> &str {
        match self {
            ApiError::BadRequest(m) | ApiError::NotFound(m) | ApiError::Internal(m) => m,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// Parse a JSON body; an empty body is treated as `{}`.
fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, ApiError> {
    let src: &[u8] = if body.iter().all(|b| b.is_ascii_whitespace()) {
        &b"{}"[..]
    } else {
        body
    };
    serde_json::from_slice(src).map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))
}

/// Run CPU-bound work on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, ApiError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::Internal(format!("worker task failed: {e}")))
}

fn validate_config(cfg: &Config) -> Result<(), ApiError> {
    if cfg.n_households == 0 || cfg.n_households > MAX_HOUSEHOLDS {
        return Err(ApiError::BadRequest(format!(
            "n_households must be between 1 and {MAX_HOUSEHOLDS}"
        )));
    }
    // rz1: lag and credit-tail ranges.
    cfg.validate_rz1().map_err(ApiError::BadRequest)?;
    // Stage-3 review R10: unknown oil profile, OPEC+ regime or shock-mask strings are rejected (400), not run.
    cfg.validate_world_oil().map_err(ApiError::BadRequest)?;
    // us-now: start state and its targets.
    match cfg.start_state {
        StartState::Typical => {
            if cfg.start_targets.is_some() {
                return Err(ApiError::BadRequest("start_targets is only used with start_state us_now".to_string()));
            }
        }
        StartState::UsNow => {
            // Today's start with the world block (world-usnow in docs/CALIBRATION.md): the trade link only. The dollar, finance and oil links keep bases captured before
            // the window and are untested with it, so they stay refused.
            if cfg.world_block_enabled && (cfg.world_link_dollar || cfg.world_link_finance || cfg.world_link_oil) {
                return Err(ApiError::BadRequest(
                    "start_state us_now takes the world block with the trade link only (world_link_dollar, world_link_finance and world_link_oil must be false)".to_string(),
                ));
            }
            match &cfg.start_targets {
                None => return Err(ApiError::BadRequest("start_state us_now needs start_targets".to_string())),
                Some(t) => t.validate().map_err(ApiError::BadRequest)?,
            }
        }
    }
    // Stage 4-1 (S4-R14): the dollar link without the trade link is refused (400), not run.
    cfg.validate_world_links().map_err(ApiError::BadRequest)?;
    // rz2: range checks of the oil-fix keys (every off value passes).
    cfg.validate_rz2().map_err(ApiError::BadRequest)?;
    // Bound pairs the run passes to `f64::clamp` (a lower bound above the upper one panicked in the run, a 500).
    cfg.validate_clamp_bounds().map_err(ApiError::BadRequest)?;
    // b9-warmup: live simulations ignore the key, but it is validated here too (create_sim shares this check).
    if !warmup_ok(cfg.warmup_months) {
        return Err(ApiError::BadRequest(format!(
            "warmup_months must be 0 or a multiple of 12 up to {MAX_WARMUP_MONTHS}"
        )));
    }
    Ok(())
}

/// b9-warmup: 0 or a multiple of 12 up to `MAX_WARMUP_MONTHS`.
fn warmup_ok(months: u32) -> bool {
    months <= MAX_WARMUP_MONTHS && months % 12 == 0
}

/// b9-warmup: the server default from `ECONSIM_WARMUP_MONTHS` (unset = `DEFAULT_WARMUP_MONTHS`, 24 since joint calibration 4;
/// "0" is a valid explicit value and restores the pre-J4 server). Not a valid value: one line, then `DEFAULT_WARMUP_MONTHS`.
fn warmup_default_from_env() -> u32 {
    let n = parse_warmup_default(env::var("ECONSIM_WARMUP_MONTHS").ok().as_deref());
    if n > 0 {
        println!("compare: warm-up {n} months by default");
    }
    n
}

/// Parse an `ECONSIM_WARMUP_MONTHS` value (passed in, not read, so tests need no environment).
fn parse_warmup_default(raw: Option<&str>) -> u32 {
    let Some(raw) = raw else { return DEFAULT_WARMUP_MONTHS };
    match raw.parse::<u32>() {
        Ok(n) if warmup_ok(n) => n,
        _ => {
            println!(
                "ECONSIM_WARMUP_MONTHS={raw} is not 0 or a multiple of 12 up to {MAX_WARMUP_MONTHS}; using {DEFAULT_WARMUP_MONTHS}"
            );
            DEFAULT_WARMUP_MONTHS
        }
    }
}

/// R17: the sims idle policy from the environment, with one line saying what it is.
fn sim_policy_from_env() -> SimPolicy {
    let policy = SimPolicy {
        ttl: parse_sim_ttl(env::var("ECONSIM_SIM_TTL_HOURS").ok().as_deref()),
        cap: parse_sim_cap(env::var("ECONSIM_SIM_CAP").ok().as_deref()),
    };
    println!(
        "sims: an economy untouched for {} h is removed; at most {} are kept",
        hours(policy.ttl),
        policy.cap
    );
    policy
}

/// R17: `ECONSIM_SIM_TTL_HOURS` (unset = 6). A positive number of hours; anything else gives one line and the default.
fn parse_sim_ttl(raw: Option<&str>) -> Duration {
    let default = SimPolicy::default().ttl;
    let Some(raw) = raw else { return default };
    let parsed = raw
        .parse::<f64>()
        .ok()
        .and_then(|h| Duration::try_from_secs_f64(h * 3600.0).ok())
        .filter(|d| !d.is_zero());
    parsed.unwrap_or_else(|| {
        println!("ECONSIM_SIM_TTL_HOURS={raw} is not a positive number of hours; using {}", hours(default));
        default
    })
}

/// R17: `ECONSIM_SIM_CAP` (unset = 200). A positive integer; anything else gives one line and the default.
fn parse_sim_cap(raw: Option<&str>) -> usize {
    let Some(raw) = raw else { return DEFAULT_SIM_CAP };
    match raw.parse::<usize>() {
        Ok(n) if n >= 1 => n,
        _ => {
            println!("ECONSIM_SIM_CAP={raw} is not a positive integer; using {DEFAULT_SIM_CAP}");
            DEFAULT_SIM_CAP
        }
    }
}

// ------------------------------------------------------------------------------------------------
// response types
// ------------------------------------------------------------------------------------------------

#[derive(Serialize, Clone)]
struct SimInfo {
    id: String,
    tick: u32,
    seed: u64,
    n_households: usize,
    applied_shocks: Vec<ScheduledShock>,
    pending_shocks: Vec<ScheduledShock>,
    /// Why the economy stopped (a step panicked); omitted while it runs. The other fields are then as of that step.
    #[serde(skip_serializing_if = "Option::is_none")]
    stopped: Option<String>,
}

fn info_of(id: &str, e: &SimEntry) -> SimInfo {
    match &e.economy {
        Economy::Live(w) => SimInfo {
            id: id.to_string(),
            tick: w.tick,
            seed: e.seed,
            n_households: w.households.len(),
            applied_shocks: w.shocks.applied.clone(),
            pending_shocks: w.shocks.schedule.clone(),
            stopped: None,
        },
        Economy::Stopped(info) => info.clone(),
    }
}

#[derive(Serialize)]
struct SimState {
    #[serde(flatten)]
    info: SimInfo,
    history: Vec<Metrics>,
}

#[derive(Serialize)]
struct StepResponse {
    tick: u32,
    metrics: Vec<Metrics>,
}

// ------------------------------------------------------------------------------------------------
// request types
// ------------------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct CreateSimRequest {
    config: Option<Config>,
    seed: Option<u64>,
}

#[derive(Deserialize)]
struct StepRequest {
    months: u32,
}

#[derive(Deserialize)]
struct ShockRequest {
    shock: Shock,
    at_month: Option<u32>,
}

// ------------------------------------------------------------------------------------------------
// handlers
// ------------------------------------------------------------------------------------------------

/// The `fields` filter from a request's query string (see fields.rs); `None` when there is none.
fn fields_param(query: &HashMap<String, String>) -> Option<FieldSet> {
    FieldSet::parse(query.get("fields").map(String::as_str))
}

/// A JSON response with the `fields` filter applied to the named record arrays. Without a filter this is `Json(value)`,
/// the same bytes as before the filter existed.
async fn filtered_json<T>(value: T, records: &'static [&'static str], keep: Option<FieldSet>) -> Result<Response, ApiError>
where
    T: Serialize + Send + 'static,
{
    let Some(keep) = keep else { return Ok(Json(value).into_response()) };
    let body = blocking(move || -> Result<Vec<u8>, String> {
        let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        fields::filter_bytes(&bytes, records, &keep).map_err(|e| e.to_string())
    })
    .await?
    .map_err(|e| ApiError::Internal(format!("cannot filter response: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "application/json")], body).into_response())
}

/// `prewarm` is `{done, total}` while a pre-warm file is in use, and null without one. `done >= total` means the pass is finished.
async fn health(State(st): State<Shared>) -> Json<Value> {
    let prewarm = match &st.prewarm {
        Some(p) => json!({ "done": p.done.load(Ordering::Relaxed), "total": p.total }),
        None => Value::Null,
    };
    // `stream` (R13-S): the server answers `POST /api/compare?stream=1` with NDJSON lines.
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION"), "prewarm": prewarm, "stream": true }))
}

/// The default configuration. Joint calibration 4: `warmup_months` shows the warm-up the compare endpoint applies when a request omits
/// it (the server default), not the `Config` code default 0, so a client that posts this object back gets the same result as one that
/// omits the key.
async fn config_default(State(st): State<Shared>) -> Json<ConfigDefault> {
    Json(ConfigDefault {
        config: Config { warmup_months: st.warmup_default, ..Config::default() },
        start_states: vec![
            StartStateInfo { id: "typical", label: "A typical year in the model", needs_targets: false, fallback_as_of: None, trade_ref: None },
            StartStateInfo {
                id: "us_now",
                label: "Today's US economy",
                needs_targets: true,
                fallback_as_of: us_now_fallback().map(|t| t.as_of.clone()),
                // The world block's trade link can start from Today's tariffs and trade shares (docs/CALIBRATION.md, section world-usnow): the newest vintage.
                trade_ref: econsim_core::world::usnow::vintage(econsim_core::world::usnow::latest_id())
                    .map(|v| TradeRefInfo { latest: econsim_core::world::usnow::latest_id().to_string(), as_of: v.as_of.clone() }),
            },
        ],
    })
}

/// `GET /api/config/default`: the `Config` plus the capability list `start_states` (us-now). Flattening keeps the `Config` keys in
/// their usual order and appends `start_states` last.
#[derive(Serialize)]
struct ConfigDefault {
    #[serde(flatten)]
    config: Config,
    start_states: Vec<StartStateInfo>,
}

#[derive(Serialize)]
struct StartStateInfo {
    id: &'static str,
    label: &'static str,
    /// `us_now` takes `config.start_targets` (the web app resolves and sends them); without them the server uses its compiled-in fallback.
    needs_targets: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_as_of: Option<String>,
    /// `us_now` with the world block and the trade link starts from the newest dated trade reference (`config.start_targets.trade_ref`).
    #[serde(skip_serializing_if = "Option::is_none")]
    trade_ref: Option<TradeRefInfo>,
}

#[derive(Serialize)]
struct TradeRefInfo {
    latest: String,
    as_of: String,
}

async fn create_sim(State(st): State<Shared>, body: Bytes) -> Result<Json<SimInfo>, ApiError> {
    let req: CreateSimRequest = parse(&body)?;
    let cfg = req.config.unwrap_or_default();
    if cfg.start_state != StartState::Typical {
        return Err(ApiError::BadRequest("start_state us_now is available on /api/compare only".to_string()));
    }
    validate_config(&cfg)?;
    let seed = req.seed.unwrap_or(1);
    let world = blocking(move || sim::new_world(&cfg, seed)).await?;
    Ok(Json(register(&st, SimEntry::new(world, seed))))
}

/// `GET /api/sims`. Lists the economies after the idle sweep; a list does not touch them (R17).
async fn list_sims(State(st): State<Shared>) -> Result<Json<Vec<SimInfo>>, ApiError> {
    let (entries, removed) = {
        let mut store = lock(&st.sims);
        let removed = store.expire();
        let entries: Vec<(String, Arc<SimCell>)> = store
            .map
            .iter()
            .map(|(k, s)| (k.clone(), Arc::clone(&s.cell)))
            .collect();
        (entries, removed)
    };
    report_removed(removed);
    let mut infos = blocking(move || {
        entries
            .iter()
            .map(|(id, e)| info_of(id, &lock(&e.entry)))
            .collect::<Vec<SimInfo>>()
    })
    .await?;
    infos.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(infos))
}

/// `GET /api/sims/:id`. `?fields=` filters the `history` records (fields.rs).
async fn get_sim(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let keep = fields_param(&query);
    let e = get_entry(&st, &id)?;
    let state = blocking(move || -> Result<SimState, ApiError> {
        let g = lock_usable(&id, &e)?;
        Ok(SimState {
            info: info_of(&id, &g),
            history: g.world().history.clone(),
        })
    })
    .await??;
    filtered_json(state, &["history"], keep).await
}

/// `DELETE /api/sims/:id`. The idle sweep runs first, so an economy already past its TTL answers 404 (R17).
async fn delete_sim(State(st): State<Shared>, Path(id): Path<String>) -> Result<Json<Value>, ApiError> {
    let (removed, deleted) = {
        let mut store = lock(&st.sims);
        let removed = store.expire();
        (removed, store.map.remove(&id))
    };
    report_removed(removed);
    if deleted.is_none() {
        return Err(ApiError::NotFound(format!("no simulation with id '{id}'")));
    }
    Ok(Json(json!({ "ok": true })))
}

/// `POST /api/sims/:id/step`. `?fields=` filters the `metrics` records (fields.rs).
async fn step_sim(
    State(st): State<Shared>,
    Path(id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let keep = fields_param(&query);
    let req: StepRequest = parse(&body)?;
    if req.months == 0 || req.months > MAX_MONTHS_PER_STEP {
        return Err(ApiError::BadRequest(format!(
            "months must be between 1 and {MAX_MONTHS_PER_STEP}"
        )));
    }
    let e = get_entry(&st, &id)?;
    let resp = blocking(move || -> Result<StepResponse, ApiError> {
        let mut g = lock_usable(&id, &e)?;
        let w = g.world_mut();
        let before = w.history.len();
        // A panic in a month (with `Config::sfc_enforce`, a stock-flow inconsistency) leaves the world half-stepped. It is
        // caught here, while the lock is held, so the economy is stopped (and the mutex is not poisoned) and the client gets
        // the panic message in a JSON 500.
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sim::run(w, req.months)));
        if let Err(p) = run {
            let why = format!("a month panicked at tick {}: {}", g.world().tick, panic_text(&*p));
            e.stop(&id, &mut g, &why);
            return Err(ApiError::Internal(format!("simulation '{id}' stopped: {why}")));
        }
        let w = g.world();
        Ok(StepResponse {
            tick: w.tick,
            metrics: w.history[before..].to_vec(),
        })
    })
    .await??;
    filtered_json(resp, &["metrics"], keep).await
}

async fn add_shock(
    State(st): State<Shared>,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<SimInfo>, ApiError> {
    let req: ShockRequest = parse(&body)?;
    req.shock.validate().map_err(ApiError::BadRequest)?;
    let e = get_entry(&st, &id)?;
    let info = blocking(move || -> Result<SimInfo, ApiError> {
        let mut g = lock_usable(&id, &e)?;
        let w = g.world_mut();
        let at = req.at_month.unwrap_or(w.tick);
        if at < w.tick {
            return Err(ApiError::BadRequest(format!(
                "at_month {at} is in the past (current tick {})",
                w.tick
            )));
        }
        w.shocks.schedule(ScheduledShock {
            at_month: at,
            shock: req.shock,
        });
        Ok(info_of(&id, &g))
    })
    .await??;
    Ok(Json(info))
}

async fn snapshot_sim(State(st): State<Shared>, Path(id): Path<String>) -> Result<Json<Snapshot>, ApiError> {
    let e = get_entry(&st, &id)?;
    let snap = blocking(move || lock_usable(&id, &e).map(|g| metrics::snapshot(g.world()))).await??;
    Ok(Json(snap))
}

async fn fork_sim(State(st): State<Shared>, Path(id): Path<String>) -> Result<Json<SimInfo>, ApiError> {
    let e = get_entry(&st, &id)?;
    let copy = blocking(move || -> Result<SimEntry, ApiError> {
        let g = lock_usable(&id, &e)?;
        Ok(SimEntry::new(g.world().clone(), g.seed))
    })
    .await??;
    Ok(Json(register(&st, copy)))
}

/// Parse and validate a compare request body. HTTP requests and pre-warm entries take the same path.
/// b9-warmup: a body whose `config` has no `warmup_months` key (or has no `config`) takes `warmup_default`; an explicit
/// value, 0 included, is kept. At `warmup_default` 0 the body is parsed exactly as before.
fn parse_compare(body: &[u8], warmup_default: u32) -> Result<Scenario, ApiError> {
    let sc: Scenario = if warmup_default == 0 {
        parse(body)?
    } else {
        let mut v: Value = parse(body)?;
        if let Some(obj) = v.as_object_mut() {
            let cfg = obj.entry("config").or_insert(Value::Null);
            if cfg.is_null() {
                *cfg = json!({});
            }
            if let Some(c) = cfg.as_object_mut() {
                c.entry("warmup_months").or_insert(json!(warmup_default));
            }
        }
        serde_json::from_value(v).map_err(|e| ApiError::BadRequest(format!("invalid JSON body: {e}")))?
    };
    let mut sc = sc;
    // us-now: resolve a missing start_targets to the compiled-in snapshot, so the cache key holds the data.
    if sc.config.start_state == StartState::UsNow && sc.config.start_targets.is_none() {
        sc.config.start_targets = us_now_fallback().cloned();
    }
    // Today's start with the world block and the trade link: a request that names no trade reference gets the newest compiled-in one, before the
    // cache key is formed ("none" opts out and is kept).
    if sc.config.start_state == StartState::UsNow && sc.config.world_block_enabled && sc.config.world_link_trade {
        if let Some(t) = sc.config.start_targets.as_mut() {
            if t.trade_ref.is_none() {
                t.trade_ref = Some(econsim_core::world::usnow::latest_id().to_string());
            }
        }
    }
    validate_scenario(&sc)?;
    Ok(sc)
}

fn validate_scenario(sc: &Scenario) -> Result<(), ApiError> {
    validate_config(&sc.config)?;
    for s in &sc.shocks {
        s.shock.validate().map_err(ApiError::BadRequest)?;
        // Stage 5 (D60): a bilateral Tariff does nothing without the block and the trade link; refuse it instead of running a no-op.
        if matches!(s.shock, econsim_core::shocks::Shock::Tariff { .. }) && !(sc.config.world_block_enabled && sc.config.world_link_trade) {
            return Err(ApiError::BadRequest("Tariff needs world_block_enabled and world_link_trade (otherwise it is ignored)".to_string()));
        }
    }
    if sc.seeds.is_empty() || sc.seeds.len() > MAX_COMPARE_SEEDS {
        return Err(ApiError::BadRequest(format!(
            "seeds must contain between 1 and {MAX_COMPARE_SEEDS} entries"
        )));
    }
    if sc.months == 0 || sc.months > MAX_COMPARE_MONTHS {
        return Err(ApiError::BadRequest(format!(
            "months must be between 1 and {MAX_COMPARE_MONTHS}"
        )));
    }
    Ok(())
}

/// `?stream=1` asks for the NDJSON lines of the compare (R13-S, see `stream_response`). Absent or `stream=0` is the usual JSON
/// body; any other value is a 400.
fn stream_requested(query: &HashMap<String, String>) -> Result<bool, ApiError> {
    match query.get("stream").map(String::as_str) {
        None | Some("0") => Ok(false),
        Some("1") => Ok(true),
        Some(other) => Err(ApiError::BadRequest(format!("stream must be 1 or 0, not '{other}'"))),
    }
}

/// The NDJSON response of a streamed compare (R13-S): `application/x-ndjson`, one line per year as the year completes, then one
/// `done` line. The response is chunked, so each line is sent as it is produced, and `no-cache, no-transform` asks a proxy or a
/// compressing layer in front of the server not to buffer or transform it.
fn stream_response(s: Streamed) -> Response {
    let cache = if s.hit { "hit" } else { "miss" };
    let lines = futures_util::stream::unfold(s.lines, |mut rx| async move {
        rx.recv().await.map(|line| (Ok::<Bytes, std::io::Error>(line), rx))
    });
    (
        [
            (header::CONTENT_TYPE, "application/x-ndjson"),
            (header::CACHE_CONTROL, "no-cache, no-transform"),
            (HeaderName::from_static(CACHE_HEADER), cache),
        ],
        Body::from_stream(lines),
    )
        .into_response()
}

/// `POST /api/compare`. `?fields=` keeps only the listed keys of each `baseline` and `shocked` record (fields.rs). The filter
/// runs after the cache lookup, so the cache key is the body alone and a hit stays a hit whatever `fields` says.
/// `?stream=1` (R13-S) answers with NDJSON lines instead, for one seed only.
async fn compare(
    State(st): State<Shared>,
    Query(query): Query<HashMap<String, String>>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let sc = parse_compare(&body, st.warmup_default)?;
    let key = cache::key_of(&sc)
        .map_err(|e| ApiError::Internal(format!("cannot canonicalise request: {e}")))?;
    if stream_requested(&query)? {
        if sc.seeds.len() != 1 {
            return Err(ApiError::BadRequest(
                "stream=1 takes exactly one seed (seeds must have one entry)".to_string(),
            ));
        }
        let streamed = st
            .compare
            .clone()
            .serve_stream(key, sc, fields_param(&query))
            .await
            .map_err(ApiError::Internal)?;
        return Ok(stream_response(streamed));
    }
    let served = st
        .compare
        .clone()
        .serve(key, sc)
        .await
        .map_err(ApiError::Internal)?;
    let Some(keep) = fields_param(&query) else {
        return Ok(compare_response(served));
    };
    let hit = served.hit;
    let filtered = blocking(move || fields::filter_bytes(&served.body, COMPARE_RECORDS, &keep))
        .await?
        .map_err(|e| ApiError::Internal(format!("cannot filter response: {e}")))?;
    Ok(compare_response(Served {
        body: Bytes::from(filtered),
        hit,
    }))
}

/// The body is the JSON that `Json<Comparison>` would send, byte for byte.
fn compare_response(s: Served) -> Response {
    let cache = if s.hit { "hit" } else { "miss" };
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (HeaderName::from_static(CACHE_HEADER), cache),
        ],
        s.body,
    )
        .into_response()
}

/// One pre-warm entry through the same cached path as `POST /api/compare`. Returns whether it was already cached.
async fn warm_one(st: &Shared, item: &Value) -> Result<bool, String> {
    let body = serde_json::to_vec(item).map_err(|e| e.to_string())?;
    let sc = parse_compare(&body, st.warmup_default).map_err(|e| e.message().to_string())?;
    let key = cache::key_of(&sc).map_err(|e| e.to_string())?;
    let served = st.compare.clone().serve(key, sc).await?;
    Ok(served.hit)
}

/// The entries of an `ECONSIM_PREWARM_FILE`: a JSON array of compare bodies. An unreadable or malformed file is logged and gives None.
fn load_prewarm(path: &str) -> Option<Vec<Value>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            println!("prewarm: cannot read {path}: {e}");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(items) => Some(items),
        Err(e) => {
            println!("prewarm: {path} is not a JSON array of compare requests: {e}");
            None
        }
    }
}

/// Serve each entry of the pre-warm array through the compare cache, one at a time, in the background.
async fn prewarm(st: Shared, path: String, items: Vec<Value>) {
    let t0 = Instant::now();
    let (mut computed, mut cached, mut failed) = (0usize, 0usize, 0usize);
    for (i, item) in items.iter().enumerate() {
        match warm_one(&st, item).await {
            Ok(true) => cached += 1,
            Ok(false) => computed += 1,
            Err(e) => {
                failed += 1;
                println!("prewarm: entry {i} failed: {e}");
            }
        }
        if let Some(progress) = &st.prewarm {
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    println!(
        "prewarm: {} entries from {path} in {:.1} s: {computed} computed, {cached} already cached, {failed} failed",
        items.len(),
        t0.elapsed().as_secs_f64()
    );
}

// ------------------------------------------------------------------------------------------------
// middleware and server
// ------------------------------------------------------------------------------------------------

async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let t0 = Instant::now();
    let res = next.run(req).await;
    println!(
        "{method} {path} -> {} ({:.1} ms)",
        res.status().as_u16(),
        t0.elapsed().as_secs_f64() * 1000.0
    );
    res
}

fn app(state: Shared) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/config/default", get(config_default))
        .route("/api/sims", post(create_sim).get(list_sims))
        .route("/api/sims/:id", get(get_sim).delete(delete_sim))
        .route("/api/sims/:id/step", post(step_sim))
        .route("/api/sims/:id/shocks", post(add_shock))
        .route("/api/sims/:id/snapshot", get(snapshot_sim))
        .route("/api/sims/:id/fork", post(fork_sim))
        .route("/api/compare", post(compare))
        .layer(middleware::from_fn(log_request))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

#[tokio::main]
async fn main() {
    econsim_core::sim::configure_thread_pool();
    let port: u16 = env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);
    let compare = Arc::new(CompareCache::from_env());
    // The pre-warm file is read before the server listens, so health can report its size from the first request.
    let prewarm_job = env::var("ECONSIM_PREWARM_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .and_then(|path| {
            let items = load_prewarm(&path)?;
            if compare.stores_results() {
                Some((path, items))
            } else {
                println!("prewarm: skipped, nothing is stored (ECONSIM_CACHE_MB=0 and no ECONSIM_CACHE_DIR)");
                None
            }
        });
    let state = Arc::new(AppState {
        sims: Mutex::new(SimStore::new(sim_policy_from_env(), Arc::new(SystemClock))),
        counter: AtomicU64::new(1),
        compare: Arc::clone(&compare),
        warmup_default: warmup_default_from_env(),
        prewarm: prewarm_job
            .as_ref()
            .map(|(_, items)| Arc::new(PrewarmProgress::new(items.len()))),
    });
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| panic!("cannot bind {addr}: {e}"));
    println!("econsim-server {} listening on http://{addr}", env!("CARGO_PKG_VERSION"));
    println!(
        "compare: cache {} MB, {} concurrent computations",
        state.compare.cache_bytes() / (1024 * 1024),
        state.compare.concurrency()
    );
    if let Some(disk) = compare.disk_summary() {
        println!("compare: disk cache {disk}");
    }
    if state.compare.world_cache_bytes() > 0 {
        println!("compare: common-world cache {} MB", state.compare.world_cache_bytes() / (1024 * 1024));
    }
    if let Some((path, items)) = prewarm_job {
        tokio::spawn(prewarm(Arc::clone(&state), path, items));
    }
    // R17: idle economies are removed between requests too, so the memory of an unused economy is freed on time.
    let sweeper = Arc::clone(&state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SIM_SWEEP_EVERY);
        loop {
            tick.tick().await;
            sweep_sims(&sweeper);
        }
    });
    axum::serve(listener, app(state)).await.expect("server error");
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::Uri;
    use serde_json::value::RawValue;
    use std::collections::BTreeMap;

    /// Small enough to run in well under a second: 1,000 households, 3 months, 2 seeds, one shock.
    const REQ: &str = r#"{"config":{"n_households":1000},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":3,"seeds":[1,2]}"#;

    fn state(cache_bytes: usize) -> Shared {
        state_with(CompareCache::new(cache_bytes, 2), None)
    }

    /// A server with the given compare cache and pre-warm progress (None without a pre-warm file).
    fn state_with(compare: CompareCache, prewarm: Option<Arc<PrewarmProgress>>) -> Shared {
        Arc::new(AppState {
            sims: Mutex::new(SimStore::new(SimPolicy::default(), Arc::new(SystemClock))),
            counter: AtomicU64::new(1),
            compare: Arc::new(compare),
            warmup_default: 0,
            prewarm,
        })
    }

    /// A server whose compare cache keeps its bodies in the disk folder `root` (a restart opens the same folder again).
    fn state_on_disk(root: &cache::TempRoot) -> Shared {
        let disk = cache::DiskCache::open(&root.0, "build-main-test", 64 << 20).unwrap();
        state_with(CompareCache::new(64 << 20, 2).with_disk(disk), None)
    }

    /// Call the compare handler directly with a query string (`fields=...`). Returns the `x-econsim-cache` header and the body bytes.
    async fn call_q(st: &Shared, query: &str, body: &str) -> (String, Bytes) {
        let uri: Uri = format!("/api/compare?{query}").parse().unwrap();
        let q = Query::<HashMap<String, String>>::try_from_uri(&uri).unwrap();
        let resp = compare(State(st.clone()), q, Bytes::from(body.to_string()))
            .await
            .unwrap();
        let cache = resp.headers()[CACHE_HEADER].to_str().unwrap().to_string();
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (cache, bytes)
    }

    /// Call the compare handler with no query string. Returns the `x-econsim-cache` header and the body bytes.
    async fn call(st: &Shared, body: &str) -> (String, Bytes) {
        call_q(st, "", body).await
    }

    #[tokio::test]
    async fn cached_response_is_byte_identical_to_fresh() {
        let cached = state(64 << 20);
        let (first, fresh) = call(&cached, REQ).await;
        let (second, hit) = call(&cached, REQ).await;
        assert_eq!(first, "miss");
        assert_eq!(second, "hit");
        assert_eq!(fresh, hit);
        // A server with the cache disabled computes the same bytes again.
        let uncached = state(0);
        let (off, direct) = call(&uncached, REQ).await;
        assert_eq!(off, "miss");
        assert_eq!(direct, fresh);
        let v: Value = serde_json::from_slice(&fresh).unwrap();
        assert_eq!(v["baseline"].as_array().unwrap().len(), 3);
        assert_eq!(v["shocked"].as_array().unwrap().len(), 3);
    }

    #[tokio::test]
    async fn key_order_and_whitespace_hit_the_same_entry() {
        let st = state(64 << 20);
        let (_, fresh) = call(&st, REQ).await;
        let reordered = r#"{
            "seeds": [1, 2], "months": 3,
            "shocks": [ { "shock": { "half_life_months": 6.0, "pct": 2e1, "type": "OilPrice" }, "at_month": 1 } ],
            "config": { "n_households": 1000 }
        }"#;
        let (cache, same) = call(&st, reordered).await;
        assert_eq!(cache, "hit");
        assert_eq!(same, fresh);
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn identical_requests_in_flight_share_one_computation() {
        let st = state(64 << 20);
        let (a, b, c, d) = tokio::join!(call(&st, REQ), call(&st, REQ), call(&st, REQ), call(&st, REQ));
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 1);
        assert!(a.1 == b.1 && b.1 == c.1 && c.1 == d.1);
    }

    #[tokio::test]
    async fn invalid_requests_are_rejected_and_not_cached() {
        let st = state(64 << 20);
        let bad = r#"{"config":{"n_households":1000},"months":0}"#;
        let resp = compare(State(st.clone()), Query(HashMap::new()), Bytes::from(bad)).await;
        assert!(matches!(resp, Err(ApiError::BadRequest(_))));
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 0);
        // Nothing was stored, so the valid request is a miss and computes.
        let (cache, _) = call(&st, REQ).await;
        assert_eq!(cache, "miss");
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prewarm_entries_go_through_the_cache() {
        let st = state(64 << 20);
        let item: Value = serde_json::from_str(REQ).unwrap();
        assert_eq!(warm_one(&st, &item).await, Ok(false));
        assert_eq!(warm_one(&st, &item).await, Ok(true));
        let (cache, _) = call(&st, REQ).await;
        assert_eq!(cache, "hit");
    }

    /// `/api/health` keeps its fields and adds `prewarm`: null without a pre-warm file, else done of total, and it reaches
    /// done == total once every entry has been served.
    #[tokio::test]
    async fn health_reports_prewarm_progress() {
        let Json(none) = health(State(state(1 << 20))).await;
        assert_eq!(none["ok"], json!(true));
        assert_eq!(none["version"], json!(env!("CARGO_PKG_VERSION")));
        assert_eq!(none["prewarm"], Value::Null);
        assert_eq!(none["stream"], json!(true));

        let items: Vec<Value> = vec![serde_json::from_str(REQ).unwrap(), serde_json::from_str(REQ).unwrap()];
        let progress = Arc::new(PrewarmProgress::new(items.len()));
        let st = state_with(CompareCache::new(64 << 20, 2), Some(Arc::clone(&progress)));
        let Json(before) = health(State(st.clone())).await;
        assert_eq!(before["prewarm"], json!({ "done": 0, "total": 2 }));
        prewarm(st.clone(), "prewarm.json".to_string(), items).await;
        let Json(after) = health(State(st)).await;
        assert_eq!(after["prewarm"], json!({ "done": 2, "total": 2 }));
    }

    /// The disk store changes nothing a client sees: a restarted server answers from disk with the bytes a fresh compute
    /// gives, and so does a server without the store.
    #[tokio::test]
    async fn disk_store_serves_identical_bytes_across_a_restart() {
        let root = cache::TempRoot::new("main");
        let (first, fresh) = call(&state_on_disk(&root), REQ).await;
        assert_eq!(first, "miss");

        let restarted = state_on_disk(&root);
        let (second, from_disk) = call(&restarted, REQ).await;
        assert_eq!(second, "hit");
        assert_eq!(restarted.compare.computed.load(Ordering::SeqCst), 0);
        assert_eq!(from_disk, fresh);

        let (off, without_disk) = call(&state(0), REQ).await;
        assert_eq!(off, "miss");
        assert_eq!(without_disk, fresh);
    }

    /// The scenario key of a body parsed with the given server default.
    fn key_with_default(body: &str, default: u32) -> String {
        cache::key_of(&parse_compare(body.as_bytes(), default).unwrap()).unwrap()
    }

    /// b9-warmup: a body without `config.warmup_months` takes the server default; an explicit value (0 included) is kept.
    #[test]
    fn parse_compare_applies_the_default_when_the_key_is_absent() {
        let warm = |body: &str| parse_compare(body.as_bytes(), 24).unwrap().config.warmup_months;
        assert_eq!(warm(REQ), 24);
        assert_eq!(warm("{}"), 24);
        assert_eq!(warm(r#"{"config":null}"#), 24);
        assert_eq!(warm(r#"{"config":{"n_households":1000,"warmup_months":0}}"#), 0);
        assert_eq!(warm(r#"{"config":{"n_households":1000,"warmup_months":12}}"#), 12);
    }

    /// b9-warmup: with a default of 0 the parsed scenario is the same as the plain parse of the body.
    #[test]
    fn parse_compare_with_default_zero_changes_nothing() {
        for body in [REQ, "{}"] {
            let plain: Scenario = serde_json::from_slice(body.as_bytes()).unwrap();
            let expected = cache::key_of(&plain).unwrap();
            assert_eq!(key_with_default(body, 0), expected);
        }
    }

    /// b9-warmup: `warmup_months` must be 0 or a multiple of 12 up to 120, on the compare path.
    #[test]
    fn warmup_validation() {
        let body = |w: u32| format!(r#"{{"config":{{"n_households":1000,"warmup_months":{w}}}}}"#);
        for bad in [13, 121, 132] {
            match parse_compare(body(bad).as_bytes(), 0) {
                Err(e) => assert_eq!(e.message(), "warmup_months must be 0 or a multiple of 12 up to 120"),
                Ok(_) => panic!("warmup_months {bad} accepted"),
            }
        }
        for good in [0, 12, 120] {
            assert!(parse_compare(body(good).as_bytes(), 0).is_ok(), "warmup_months {good} rejected");
        }
    }

    /// rz2: `validate_config` answers 400 for an out-of-range oil-fix key (the call at `validate_config` is the only server-level
    /// guard; deleting it must fail this test), and accepts the off values and the RZ2 candidate values.
    #[test]
    fn rz2_keys_validation() {
        assert!(validate_config(&Config::default()).is_ok());
        let good = Config { hh_pump_real_rate_weight: 0.5, energy_extraction_labor_share: 0.11, energy_extraction_elasticity: 1.0, user_cost_pi_e_mode: 2, user_cost_pi_e_offset: 0.0035, ..Config::default() };
        assert!(validate_config(&good).is_ok());
        let bad = [
            Config { hh_pump_real_rate_weight: 1.5, ..Config::default() },
            Config { energy_extraction_labor_share: 2.0, ..Config::default() },
            Config { user_cost_pi_e_mode: 9, ..Config::default() },
        ];
        for cfg in &bad {
            match validate_config(cfg) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains("out of range"), "{m}"),
                _ => panic!("an out-of-range oil-fix key was accepted"),
            }
        }
    }

    /// Stage-4 step 0 (D4, S3-R13): `validate_config` answers 400 for an unknown oil profile, OPEC+ regime or shock-mask character
    /// (the call at `validate_config` is the only server-level guard; deleting it must fail this test).
    #[test]
    fn validate_config_rejects_unknown_world_oil_strings() {
        assert!(validate_config(&Config::default()).is_ok());
        let bad = [
            Config { world_oil_profile: "nope".to_string(), ..Config::default() },
            Config { world_opec_regime: "floor".to_string(), ..Config::default() },
            Config { world_oil_shock_mask: "IX".to_string(), ..Config::default() },
        ];
        for cfg in &bad {
            assert!(matches!(validate_config(cfg), Err(ApiError::BadRequest(_))), "unknown world oil string accepted");
        }
    }

    /// Stage 4-1 (S4-R14, D7): the dollar link without the trade link is refused with 400, on the create route (`validate_config`)
    /// and on `/api/compare` (`parse_compare` runs `validate_scenario`); with the trade link on the same request passes.
    #[test]
    fn validate_config_refuses_the_dollar_link_without_the_trade_link() {
        let dollar = |trade: bool| Config { world_block_enabled: true, world_link_dollar: true, world_link_trade: trade, ..Config::default() };
        assert!(validate_config(&dollar(true)).is_ok());
        match validate_config(&dollar(false)) {
            Err(ApiError::BadRequest(m)) => assert!(m.contains("world_link_trade"), "{m}"),
            _ => panic!("the dollar link without the trade link was accepted"),
        }
        let body = r#"{"config":{"n_households":1000,"world_block_enabled":true,"world_link_dollar":true,"world_link_trade":false},"months":3,"seeds":[1]}"#;
        assert!(matches!(parse_compare(body.as_bytes(), 0), Err(ApiError::BadRequest(_))), "compare accepted the refused link");
        let ok = body.replace(r#""world_link_trade":false"#, r#""world_link_trade":true"#);
        assert!(parse_compare(ok.as_bytes(), 0).is_ok(), "compare refused the link with the trade link on");
    }

    /// Stage 5 (D60): a bilateral `Tariff` posted with the block or the trade link off is refused with 400 on `/api/compare`; with both on it passes.
    #[test]
    fn tariff_without_trade_link_is_400() {
        let body = |block: bool, trade: bool| format!(r#"{{"config":{{"n_households":1000,"world_block_enabled":{block},"world_link_trade":{trade}}},"shocks":[{{"at_month":0,"shock":{{"type":"Tariff","partner":"CN","sector":null,"pct":25,"retaliate":true}}}}],"months":3,"seeds":[1]}}"#);
        assert!(parse_compare(body(true, true).as_bytes(), 0).is_ok(), "Tariff refused with the block and the trade link on");
        for (b, t) in [(false, false), (true, false), (false, true)] {
            match parse_compare(body(b, t).as_bytes(), 0) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains("world_link_trade"), "{m}"),
                _ => panic!("Tariff accepted with block {b} and trade link {t}"),
            }
        }
        let legacy = r#"{"config":{"n_households":1000},"shocks":[{"at_month":0,"shock":{"type":"ImportTariff","pct":10}}],"months":3,"seeds":[1]}"#;
        assert!(parse_compare(legacy.as_bytes(), 0).is_ok(), "the legacy ImportTariff must still pass");
    }

    /// R51-E: a `SharePrices` shock passes on `/api/compare` at its bounds, and past them is refused with 400 and a message that names the
    /// shock, the field and the value received (a sector given as a name, not a number, is the usual invalid-body 400).
    #[test]
    fn share_prices_shock_is_validated_on_compare() {
        let body = |shock: &str| {
            format!(r#"{{"config":{{"n_households":1000}},"shocks":[{{"at_month":0,"shock":{shock}}}],"months":3,"seeds":[1]}}"#)
        };
        for ok in [
            r#"{"type":"SharePrices","sector":4,"pct":-80,"half_life_months":24}"#,
            r#"{"type":"SharePrices","sector":null,"pct":-94.9,"half_life_months":0}"#,
            r#"{"type":"SharePrices","pct":300,"half_life_months":0}"#,
            r#"{"type":"SharePrices","sector":8,"pct":-30,"half_life_months":600}"#,
        ] {
            assert!(parse_compare(body(ok).as_bytes(), 0).is_ok(), "refused {ok}");
        }
        for (bad, says) in [
            (r#"{"type":"SharePrices","sector":4,"pct":-95,"half_life_months":0}"#, "SharePrices: pct (the change in share prices, in percent) must be above -95 and at most 300, got -95"),
            (r#"{"type":"SharePrices","sector":4,"pct":300.1,"half_life_months":0}"#, "must be above -95 and at most 300, got 300.1"),
            (r#"{"type":"SharePrices","sector":9,"pct":-30,"half_life_months":0}"#, "SharePrices: sector must be null (all sectors) or 0 to 8"),
            (r#"{"type":"SharePrices","sector":null,"pct":-30,"half_life_months":-0.5}"#, "SharePrices: half_life_months must be 0 (permanent) or more, got -0.5"),
        ] {
            match parse_compare(body(bad).as_bytes(), 0) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains(says), "{m}"),
                _ => panic!("accepted {bad}"),
            }
        }
        match parse_compare(body(r#"{"type":"SharePrices","sector":"tech","pct":-80,"half_life_months":0}"#).as_bytes(), 0) {
            Err(ApiError::BadRequest(m)) => assert!(m.starts_with("invalid JSON body:"), "{m}"),
            _ => panic!("a sector given as a name was accepted"),
        }
    }

    /// R51-E: `/api/compare` runs a `SharePrices` shock: on impact Tech's value is 20% of the baseline's at -80%, the other sectors are
    /// unchanged, and the answer is cached and identical on a second request.
    #[tokio::test]
    async fn compare_runs_a_share_price_shock() {
        let st = state(64 << 20);
        let req = r#"{"config":{"n_households":1000},"shocks":[{"at_month":0,"shock":{"type":"SharePrices","sector":4,"pct":-80,"half_life_months":0}}],"months":3,"seeds":[1]}"#;
        let (first, a) = call(&st, req).await;
        let (second, b) = call(&st, req).await;
        assert_eq!((first.as_str(), second.as_str()), ("miss", "hit"));
        assert_eq!(a, b);
        let v: Value = serde_json::from_slice(&a).unwrap();
        let (b0, s0) = (&v["baseline"][0]["equity_value_by_sector"], &v["shocked"][0]["equity_value_by_sector"]);
        let ratio = s0[4].as_f64().unwrap() / b0[4].as_f64().unwrap();
        assert!((ratio - 0.2).abs() < 1e-9, "tech value ratio {ratio}");
        assert_eq!(s0[0], b0[0], "another sector moved");
    }

    /// R51-E: a live economy takes a `SharePrices` shock on `/api/sims/:id/shocks`, validates it, and re-rates Tech in the month it fires.
    #[tokio::test]
    async fn live_economy_takes_a_share_price_shock() {
        let st = state(0);
        let id = live_sim(&st);
        let twin = fork_sim(State(st.clone()), Path(id.clone())).await.unwrap().0.id;
        let bad = add_shock(
            State(st.clone()),
            Path(id.clone()),
            Bytes::from_static(br#"{"shock":{"type":"SharePrices","sector":4,"pct":-100,"half_life_months":0}}"#),
        )
        .await;
        match bad {
            Err(ApiError::BadRequest(m)) => assert!(m.contains("SharePrices: pct") && m.contains("above -95 and at most 300, got -100"), "{m}"),
            _ => panic!("pct -100 was accepted"),
        }
        let info = add_shock(
            State(st.clone()),
            Path(id.clone()),
            Bytes::from_static(br#"{"shock":{"type":"SharePrices","sector":4,"pct":-80,"half_life_months":12}}"#),
        )
        .await
        .unwrap();
        assert_eq!(info.0.pending_shocks.len(), 1, "the refused one was not scheduled");
        let mut tech = Vec::new();
        for who in [&id, &twin] {
            let resp = step_sim(State(st.clone()), Path(who.clone()), Query(HashMap::new()), Bytes::from_static(br#"{"months":1}"#)).await.unwrap();
            let v: Value = serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
            tech.push(v["metrics"][0]["equity_value_by_sector"][4].as_f64().unwrap());
        }
        assert!((tech[0] / tech[1] - 0.2).abs() < 1e-9, "tech value ratio {}", tech[0] / tech[1]);
    }

    /// RZ23: a `Rebate` shock (the Treasury-financed one-off payment) passes on `/api/compare` at its bounds and past them is refused with 400
    /// and a message that names the shock and the bounds.
    #[test]
    fn rebate_shock_is_validated_on_compare() {
        let body = |shock: &str| {
            format!(r#"{{"config":{{"n_households":1000}},"shocks":[{{"at_month":0,"shock":{shock}}}],"months":3,"seeds":[1]}}"#)
        };
        for ok in [
            r#"{"type":"Rebate","per_household":2000,"months":1}"#,
            r#"{"type":"Rebate","per_household":0,"months":1}"#,
            r#"{"type":"Rebate","per_household":100000,"months":600}"#,
        ] {
            assert!(parse_compare(body(ok).as_bytes(), 0).is_ok(), "refused {ok}");
        }
        for bad in [
            r#"{"type":"Rebate","per_household":-1,"months":1}"#,
            r#"{"type":"Rebate","per_household":100001,"months":1}"#,
            r#"{"type":"Rebate","per_household":2000,"months":0}"#,
        ] {
            match parse_compare(body(bad).as_bytes(), 0) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains("Rebate: per_household in [0, 100000] and months >= 1"), "{m}"),
                _ => panic!("accepted {bad}"),
            }
        }
    }

    /// RZ23: `/api/compare` runs a `Rebate` shock. The payment shows in the one-off-payment series for the month it is paid and only then, the
    /// first month's spending follows the windfall rule, and the second request is a cache hit with the same answer.
    #[tokio::test]
    async fn compare_runs_a_rebate_shock() {
        let st = state(64 << 20);
        let req = r#"{"config":{"n_households":1000},"shocks":[{"at_month":0,"shock":{"type":"Rebate","per_household":2000,"months":1}}],"months":3,"seeds":[1]}"#;
        let (first, a) = call(&st, req).await;
        let (second, b) = call(&st, req).await;
        assert_eq!((first.as_str(), second.as_str()), ("miss", "hit"));
        assert_eq!(a, b);
        let v: Value = serde_json::from_slice(&a).unwrap();
        let h = |arm: &str, m: usize| v[arm][m]["helicopter_bn_ar"].as_f64().unwrap();
        assert!(h("shocked", 0) > 0.0 && h("shocked", 1) == 0.0 && h("baseline", 0) == 0.0);
        let c = |arm: &str| v[arm][0]["consumption_real_bn_ar"].as_f64().unwrap();
        assert!(c("shocked") > c("baseline"), "the payment is spent in the month it arrives");
    }

    /// Stage 4-2 (list A', S4-R22, review D2): an out-of-range finance value is refused with 400, on the create route (`validate_config`)
    /// and on `/api/compare`, and the message names the key. The defaults pass. NaN and infinity fail as well.
    #[test]
    fn validate_config_refuses_out_of_range_finance_values() {
        assert!(validate_config(&Config::default()).is_ok());
        let bad = [
            (Config { world_fo_trans_rho: 1.5, ..Config::default() }, "world_fo_trans_rho"),
            (Config { world_fo_theta_stock: 1e9, ..Config::default() }, "world_fo_theta_stock"),
            (Config { world_fo_theta_trans: 101.0, ..Config::default() }, "world_fo_theta_trans"),
            (Config { world_fo_dollar_resp: -5.0, ..Config::default() }, "world_fo_dollar_resp"),
            (Config { world_fo_vix_resp: -0.5, ..Config::default() }, "world_fo_vix_resp"),
            (Config { world_fo_trend_cn: -2000.0, ..Config::default() }, "world_fo_trend_cn"),
            (Config { world_gdp_rw_bn: 0.0, ..Config::default() }, "world_gdp_rw_bn"),
            (Config { world_gdp_rw_bn: f64::INFINITY, ..Config::default() }, "world_gdp_rw_bn"),
            (Config { world_fo_trans_rho: f64::NAN, ..Config::default() }, "world_fo_trans_rho"),
        ];
        for (cfg, key) in &bad {
            match validate_config(cfg) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains(key), "{m}"),
                _ => panic!("{key} out of range was accepted"),
            }
        }
        let body = r#"{"config":{"n_households":1000,"world_fo_trans_rho":1.5},"months":3,"seeds":[1]}"#;
        assert!(matches!(parse_compare(body.as_bytes(), 0), Err(ApiError::BadRequest(_))), "compare accepted rho 1.5");
        let ok = body.replace("1.5", "0.9");
        assert!(parse_compare(ok.as_bytes(), 0).is_ok(), "compare refused rho 0.9");
    }

    /// Stage 4-3: `world_retaliation` without the trade link and an out-of-range retaliation value are refused with 400, on the create
    /// route (`validate_config`) and on `/api/compare`; the message names the key. With the trade link and in-range values the same request passes.
    #[test]
    fn validate_config_refuses_retaliation_without_the_trade_link_and_bad_values() {
        let ret = |trade: bool| Config { world_block_enabled: true, world_retaliation: true, world_link_trade: trade, ..Config::default() };
        assert!(validate_config(&ret(true)).is_ok());
        match validate_config(&ret(false)) {
            Err(ApiError::BadRequest(m)) => assert!(m.contains("world_link_trade"), "{m}"),
            _ => panic!("world_retaliation without the trade link was accepted"),
        }
        for (cfg, key) in [
            (Config { world_ret_speed: 1.5, ..Config::default() }, "world_ret_speed"),
            (Config { world_ret_eps_lr: 11.0, ..Config::default() }, "world_ret_eps_lr"),
            (Config { world_ret_eps_sr: f64::NAN, ..Config::default() }, "world_ret_eps_sr"),
        ] {
            match validate_config(&cfg) {
                Err(ApiError::BadRequest(m)) => assert!(m.contains(key), "{m}"),
                _ => panic!("{key} out of range was accepted"),
            }
        }
        let body = r#"{"config":{"n_households":1000,"world_block_enabled":true,"world_retaliation":true,"world_link_trade":false},"months":3,"seeds":[1]}"#;
        assert!(matches!(parse_compare(body.as_bytes(), 0), Err(ApiError::BadRequest(_))), "compare accepted the refused combination");
        let ok = body.replace(r#""world_link_trade":false"#, r#""world_link_trade":true"#);
        assert!(parse_compare(ok.as_bytes(), 0).is_ok(), "compare refused retaliation with the trade link on");
    }

    /// rz1: the lag and the credit-tail half-life are range-checked on the compare path; the off values and the in-range
    /// values are accepted.
    #[test]
    fn rz1_keys_validation() {
        let body = |k: &str, v: &str| format!(r#"{{"config":{{"n_households":1000,"{k}":{v}}}}}"#);
        for (k, v) in [("rate_perception_months", "-1"), ("rate_perception_months", "40"), ("credit_tail_half_life_months", "-0.5"),
                       ("credit_tail_half_life_months", "61"), ("kstar_adapt", "1.5"), ("kstar_adapt", "-0.1"), ("rate_perception_firm_months", "-2"), ("rate_perception_firm_months", "37")] {
            match parse_compare(body(k, v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with(k), "{k}={v}: {}", e.message()),
                Ok(_) => panic!("{k}={v} accepted"),
            }
        }
        for (k, v) in [("rate_perception_months", "0"), ("rate_perception_months", "9"), ("rate_perception_months", "36"),
                       ("credit_tail_half_life_months", "0"), ("credit_tail_half_life_months", "9"), ("credit_tail_half_life_months", "60"),
                       ("rate_perception_deposit", "true"), ("kstar_adapt", "0.1"), ("kstar_adapt", "0"), ("rate_perception_firm_months", "-1"), ("rate_perception_firm_months", "4"), ("fisim_constant_margin", "true")] {
            assert!(parse_compare(body(k, v).as_bytes(), 0).is_ok(), "{k}={v} rejected");
        }
    }

    /// rz19: the tariff efficiency wedge's two keys are range-checked on the compare path; the off values and in-range values are accepted.
    #[test]
    fn rz19_keys_validation() {
        let body = |k: &str, v: &str| format!(r#"{{"config":{{"n_households":1000,"{k}":{v}}}}}"#);
        for (k, v) in [("tariff_tfp_loss", "-0.1"), ("tariff_tfp_loss", "2.5"), ("tariff_tfp_phase_in_months", "0.5"), ("tariff_tfp_phase_in_months", "121")] {
            match parse_compare(body(k, v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with(k), "{k}={v}: {}", e.message()),
                Ok(_) => panic!("{k}={v} accepted"),
            }
        }
        for (k, v) in [("tariff_tfp_loss", "0"), ("tariff_tfp_loss", "0.067"), ("tariff_tfp_loss", "2"), ("tariff_tfp_phase_in_months", "1"), ("tariff_tfp_phase_in_months", "60"), ("tariff_tfp_phase_in_months", "120")] {
            assert!(parse_compare(body(k, v).as_bytes(), 0).is_ok(), "{k}={v} rejected");
        }
    }

    /// rz21: the rule's look-through weight is range-checked on the compare path; the off value and in-range values are accepted.
    #[test]
    fn rz21_key_validation() {
        let body = |k: &str, v: &str| format!(r#"{{"config":{{"n_households":1000,"{k}":{v}}}}}"#);
        for v in ["-0.1", "1.5"] {
            match parse_compare(body("rule_lookthrough", v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with("rule_lookthrough"), "{v}: {}", e.message()),
                Ok(_) => panic!("rule_lookthrough={v} accepted"),
            }
        }
        for v in ["0", "0.5", "1"] {
            assert!(parse_compare(body("rule_lookthrough", v).as_bytes(), 0).is_ok(), "rule_lookthrough={v} rejected");
        }
    }

    /// rz20: the equity-channel keys are range-checked on the compare path; the off values and in-range values are accepted.
    #[test]
    fn rz20_keys_validation() {
        let body = |k: &str, v: &str| format!(r#"{{"config":{{"n_households":1000,"{k}":{v}}}}}"#);
        for (k, v) in [
            ("fund_wealth_tail_weight", "0.04"),
            ("fund_wealth_tail_weight", "1.1"),
            ("fund_wealth_ewma_span", "1.5"),
            ("fund_wealth_ewma_span", "61"),
            ("investment_q_sens_all", "-0.1"),
            ("investment_q_sens_all", "1.5"),
            ("ebp_equity_sens", "-0.001"),
            ("ebp_equity_sens", "0.06"),
            ("rerate_wealth_mpc_annual", "0.3"),
            ("rerate_wealth_mpc_annual", "-0.01"),
            ("rerate_q_sens", "1.2"),
            ("rerate_ebp_sens", "0.1"),
        ] {
            match parse_compare(body(k, v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with(k), "{k}={v}: {}", e.message()),
                Ok(_) => panic!("{k}={v} accepted"),
            }
        }
        for (k, v) in [
            ("fund_wealth_tail_weight", "0.35"),
            ("fund_wealth_tail_weight", "1"),
            ("fund_wealth_tail_weight", "0.05"),
            ("fund_wealth_ewma_span", "12"),
            ("fund_wealth_ewma_span", "24"),
            ("fund_wealth_ewma_span", "2"),
            ("investment_q_sens_all", "0"),
            ("investment_q_sens_all", "0.16"),
            ("investment_q_sens_all", "1"),
            ("ebp_equity_sens", "0"),
            ("ebp_equity_sens", "0.024"),
            ("ebp_equity_sens", "0.05"),
            ("rerate_wealth_mpc_annual", "0"),
            ("rerate_wealth_mpc_annual", "0.025"),
            ("rerate_q_sens", "0.16"),
            ("rerate_ebp_sens", "0.024"),
        ] {
            assert!(parse_compare(body(k, v).as_bytes(), 0).is_ok(), "{k}={v} rejected");
        }
    }

    /// rz24: the credit-spread equity loading is range-checked on the compare path; the off value and in-range values are accepted.
    #[test]
    fn rz24_key_validation() {
        let body = |v: &str| format!(r#"{{"config":{{"n_households":1000,"credit_shock_erp":{v}}}}}"#);
        for v in ["-0.1", "5.5"] {
            match parse_compare(body(v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with("credit_shock_erp"), "{v}: {}", e.message()),
                Ok(_) => panic!("credit_shock_erp={v} accepted"),
            }
        }
        for v in ["0", "0.56", "5"] {
            assert!(parse_compare(body(v).as_bytes(), 0).is_ok(), "credit_shock_erp={v} rejected");
        }
    }

    /// rz12: the perception stage counts are range-checked on the compare path; the off values and in-range values are accepted.
    #[test]
    fn rz12_keys_validation() {
        let body = |k: &str, v: &str| format!(r#"{{"config":{{"n_households":1000,"{k}":{v}}}}}"#);
        for (k, v) in [("rate_perception_stages", "0"), ("rate_perception_stages", "5"), ("rate_perception_firm_stages", "0"), ("rate_perception_firm_stages", "5"), ("rate_perception_firm_stages", "-2"), ("rate_perception_credit_bypass", "1.5"), ("rate_perception_credit_bypass", "-0.1")] {
            match parse_compare(body(k, v).as_bytes(), 0) {
                Err(e) => assert!(e.message().starts_with(k), "{k}={v}: {}", e.message()),
                Ok(_) => panic!("{k}={v} accepted"),
            }
        }
        for (k, v) in [("rate_perception_stages", "1"), ("rate_perception_stages", "2"), ("rate_perception_stages", "3"), ("rate_perception_stages", "4"), ("rate_perception_firm_stages", "-1"), ("rate_perception_firm_stages", "1"), ("rate_perception_firm_stages", "4"), ("rate_perception_credit_bypass", "0"), ("rate_perception_credit_bypass", "1")] {
            assert!(parse_compare(body(k, v).as_bytes(), 0).is_ok(), "{k}={v} rejected");
        }
    }

    /// b9-warmup: requests that differ only in `warmup_months` have different cache keys.
    #[test]
    fn cache_key_includes_the_warmup() {
        let zero = r#"{"config":{"n_households":1000,"warmup_months":0},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":3,"seeds":[1,2]}"#;
        let twelve = zero.replace(r#""warmup_months":0"#, r#""warmup_months":12"#);
        assert_ne!(key_with_default(zero, 0), key_with_default(&twelve, 0));
    }

    /// b9-warmup: `ECONSIM_WARMUP_MONTHS` parsing (the value is passed in; the environment is not touched).
    #[test]
    fn env_default_parsing() {
        // Joint calibration 4: unset and invalid values give the code default 24; an explicit "0" restores the pre-J4 server.
        assert_eq!(DEFAULT_WARMUP_MONTHS, 24);
        assert_eq!(parse_warmup_default(None), 24);
        assert_eq!(parse_warmup_default(Some("0")), 0);
        assert_eq!(parse_warmup_default(Some("12")), 12);
        assert_eq!(parse_warmup_default(Some("24")), 24);
        assert_eq!(parse_warmup_default(Some("120")), 120);
        assert_eq!(parse_warmup_default(Some("13")), 24);
        assert_eq!(parse_warmup_default(Some("132")), 24);
        assert_eq!(parse_warmup_default(Some("abc")), 24);
    }

    /// Joint calibration 4: `/api/config/default` shows the warm-up the server applies, and a request that posts it back is the same
    /// scenario (same cache key) as one that omits the key.
    #[tokio::test]
    async fn config_default_reports_the_server_warmup() {
        let st = Arc::new(AppState {
            sims: Mutex::new(SimStore::new(SimPolicy::default(), Arc::new(SystemClock))),
            counter: AtomicU64::new(1),
            compare: Arc::new(CompareCache::new(1 << 20, 2)),
            warmup_default: 24,
            prewarm: None,
        });
        let Json(cd) = config_default(State(st.clone())).await;
        let cfg = cd.config;
        assert_eq!(cfg.warmup_months, 24);
        assert_eq!(Config::default().warmup_months, 0);
        let posted = REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"warmup_months":24"#);
        assert_eq!(key_with_default(REQ, 24), key_with_default(&posted, 24));
        let st0 = state(1 << 20);
        let Json(cd0) = config_default(State(st0)).await;
        assert_eq!(cd0.config.warmup_months, 0);
    }

    // ---- us-now ------------------------------------------------------------------------------------------------------------------

    /// A 12-month window of constant data, 400 households and a 12-month burn-in so the tests run in about a second.
    fn us_body(tweak: impl FnOnce(&mut Value)) -> String {
        let n = 12;
        let col = |v: f64| vec![v; n];
        let mut v = json!({
            "config": {
                "n_households": 400, "burn_in_months": 12, "start_state": "us_now",
                "start_targets": {
                    "as_of": "test",
                    "window": {
                        "months": (0..n).map(|i| format!("m{i}")).collect::<Vec<_>>(),
                        "policy_rate": col(4.0), "y3m": col(4.2), "y2": col(4.7), "y5": col(5.0), "y10": col(5.2), "y30": col(5.6),
                        "mortgage_rate": col(7.4), "oil_wti": col(96.0), "tariff_pct": col(8.0)
                    },
                    "unemployment_rate": 4.2,
                    "tuning": { "max_attempts": 1, "max_hops": 0 }
                }
            },
            "shocks": [], "months": 2, "seeds": [1]
        });
        tweak(&mut v);
        v.to_string()
    }

    #[test]
    fn us_now_fallback_is_compiled_in_and_valid() {
        let t = us_now_fallback().expect("scripts/data/us_now.json parses");
        t.validate().unwrap();
        assert_eq!(t.window.months.len(), 24);
    }

    #[test]
    fn us_now_without_targets_takes_the_fallback() {
        let body = r#"{"config":{"n_households":400,"start_state":"us_now"},"months":2,"seeds":[1]}"#;
        let sc = parse_compare(body.as_bytes(), 0).unwrap();
        assert_eq!(sc.config.start_targets.as_ref().unwrap().as_of, us_now_fallback().unwrap().as_of);
    }

    #[test]
    fn us_now_validation() {
        let err = |b: String| match parse_compare(b.as_bytes(), 0) {
            Err(e) => e.message().to_string(),
            Ok(_) => panic!("accepted"),
        };
        assert!(parse_compare(us_body(|_| {}).as_bytes(), 0).is_ok());
        assert!(err(us_body(|v| v["config"]["start_state"] = json!("typical"))).contains("only used with start_state us_now"));
        // the world block with the trade link is accepted now; the dollar, finance and oil links stay refused with it
        assert!(parse_compare(us_body(|v| { v["config"]["world_block_enabled"] = json!(true); v["config"]["world_link_trade"] = json!(true); }).as_bytes(), 0).is_ok());
        for link in ["world_link_dollar", "world_link_finance", "world_link_oil"] {
            let e = err(us_body(|v| { v["config"]["world_block_enabled"] = json!(true); v["config"]["world_link_trade"] = json!(true); v["config"][link] = json!(true); }));
            assert!(e.contains("trade link only"), "{link}: {e}");
        }
        assert!(err(us_body(|v| { v["config"]["start_targets"]["trade_ref"] = json!("1999-01-01"); })).contains("trade_ref"));
        assert!(err(us_body(|v| { v["config"]["start_targets"]["window"]["y10"] = json!([5.2]); })).contains("y10"));
        assert!(err(us_body(|v| v["config"]["start_state"] = json!("elsewhere"))).contains("invalid JSON body"));
    }

    /// Today's start with the world block: the trade reference is filled in before the cache key, kept when named, and absent without the block.
    #[test]
    fn us_now_with_the_world_block_gets_a_trade_ref() {
        let world = |tweak: &dyn Fn(&mut Value)| {
            us_body(|v| {
                v["config"]["world_block_enabled"] = json!(true);
                v["config"]["world_link_trade"] = json!(true);
                tweak(v);
            })
        };
        let latest = econsim_core::world::usnow::latest_id();
        let tr = |b: String| parse_compare(b.as_bytes(), 0).unwrap().config.start_targets.unwrap().trade_ref;
        assert_eq!(tr(world(&|_| {})).as_deref(), Some(latest));
        assert_eq!(tr(world(&|v| v["config"]["start_targets"]["trade_ref"] = json!("none"))).as_deref(), Some("none"));
        assert_eq!(tr(us_body(|_| {})), None, "no world block: no reference, the request keeps its bytes");
        let k = |b: String| cache::key_of(&parse_compare(b.as_bytes(), 0).unwrap()).unwrap();
        assert_eq!(k(world(&|_| {})), k(world(&|v| v["config"]["start_targets"]["trade_ref"] = json!(latest))), "naming the newest vintage is the same request");
        assert_ne!(k(world(&|_| {})), k(world(&|v| v["config"]["start_targets"]["trade_ref"] = json!("none"))));
        // the trade link off: nothing to start from, nothing is filled in
        assert_eq!(tr(us_body(|v| v["config"]["world_block_enabled"] = json!(true))), None);
    }

    #[test]
    fn us_now_cache_key_follows_the_targets() {
        let k = |b: String| cache::key_of(&parse_compare(b.as_bytes(), 0).unwrap()).unwrap();
        let base = k(us_body(|_| {}));
        assert_eq!(base, k(us_body(|_| {})));
        assert_ne!(base, k(us_body(|v| v["config"]["start_targets"]["unemployment_rate"] = json!(4.3))));
        assert_ne!(base, k(us_body(|v| v["config"]["start_targets"]["window"]["oil_wti"][11] = json!(97.0))));
        assert_ne!(base, k(us_body(|v| v["config"]["start_targets"]["as_of"] = json!("test.2"))));
        // key order and number spelling do not matter
        assert_eq!(base, k(us_body(|v| v["config"]["start_targets"]["window"]["policy_rate"] = json!([4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4]))));
    }

    #[tokio::test]
    async fn us_now_compare_is_deterministic_and_reports_the_start() {
        let st = state(0);
        let (_, a) = call(&st, &us_body(|_| {})).await;
        let (_, b) = call(&st, &us_body(|_| {})).await;
        assert_eq!(a, b);
        let v: Value = serde_json::from_slice(&a).unwrap();
        assert_eq!(v["start_state"], "us_now");
        assert_eq!(v["warmup_months"], 12);
        assert_eq!(v["start_info"]["window_months"], 12);
        assert_eq!(v["start_info"]["per_seed"].as_array().unwrap().len(), 1);
        assert_eq!(v["baseline"][0]["tick"], 0);
        // with the cache on, the second identical request is a hit with the same bytes
        let cached = state(64 << 20);
        let (c1, x) = call(&cached, &us_body(|_| {})).await;
        let (c2, y) = call(&cached, &us_body(|_| {})).await;
        assert_eq!((c1.as_str(), c2.as_str()), ("miss", "hit"));
        assert_eq!(x, y);
        assert_eq!(x, a);
    }

    #[tokio::test]
    async fn create_sim_rejects_us_now() {
        let st = state(0);
        let r = create_sim(State(st), Bytes::from(r#"{"config":{"n_households":400,"start_state":"us_now"}}"#)).await;
        assert!(matches!(r, Err(ApiError::BadRequest(_))));
    }

    #[tokio::test]
    async fn config_default_lists_the_start_states() {
        let st = state(0);
        let Json(cd) = config_default(State(st)).await;
        let v = serde_json::to_value(&cd).unwrap();
        assert_eq!(v["start_state"], "typical");
        let ids: Vec<&str> = v["start_states"].as_array().unwrap().iter().map(|x| x["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["typical", "us_now"]);
        assert_eq!(v["start_states"][1]["needs_targets"], true);
        // the capability flag the web reads before it offers the trade model on Today's start; the typical start has none
        assert_eq!(v["start_states"][1]["trade_ref"]["latest"], econsim_core::world::usnow::latest_id());
        assert!(v["start_states"][0].get("trade_ref").is_none());
    }

    /// The sorted top-level keys of a JSON object.
    fn keys(v: &Value) -> Vec<&str> {
        let mut k: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        k.sort_unstable();
        k
    }

    /// `?fields=` on the compare endpoint: each record keeps the listed keys and the month key; `seeds` and the values of the
    /// kept keys are the same as in the unfiltered response.
    #[tokio::test]
    async fn fields_keep_only_the_listed_keys_and_the_month_key() {
        let st = state(64 << 20);
        let (_, full) = call(&st, REQ).await;
        let (_, lean) = call_q(&st, "fields=gdp_real_bn_ar,cpi", REQ).await;
        let full: Value = serde_json::from_slice(&full).unwrap();
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        assert_eq!(lean["seeds"], full["seeds"]);
        assert_eq!(keys(&lean), ["baseline", "seeds", "shocked"]);
        for side in ["baseline", "shocked"] {
            let (fr, lr) = (full[side].as_array().unwrap(), lean[side].as_array().unwrap());
            assert_eq!(lr.len(), 3);
            for (f, l) in fr.iter().zip(lr) {
                assert_eq!(keys(l), ["cpi", "gdp_real_bn_ar", "tick"]);
                for k in ["cpi", "gdp_real_bn_ar", "tick"] {
                    assert_eq!(l[k], f[k], "{side} {k}");
                }
            }
        }
    }

    /// Unknown names are ignored: they add no keys and do not fail the request.
    #[tokio::test]
    async fn fields_ignore_unknown_names() {
        let st = state(64 << 20);
        let (_, lean) = call_q(&st, "fields=nope,cpi,", REQ).await;
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        assert_eq!(keys(&lean["baseline"][0]), ["cpi", "tick"]);
        let (_, none) = call_q(&st, "fields=nope", REQ).await;
        let none: Value = serde_json::from_slice(&none).unwrap();
        assert_eq!(keys(&none["shocked"][2]), ["tick"]);
    }

    /// A nested object (the world block's `world`) is kept whole when its top-level key is listed.
    #[tokio::test]
    async fn fields_keep_nested_objects_whole() {
        let st = state(64 << 20);
        let world_req = REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#);
        let (_, full) = call(&st, &world_req).await;
        let (_, lean) = call_q(&st, "fields=world", &world_req).await;
        let full: Value = serde_json::from_slice(&full).unwrap();
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        assert!(full["baseline"][0]["world"].is_object());
        for side in ["baseline", "shocked"] {
            for i in 0..3 {
                assert_eq!(keys(&lean[side][i]), ["tick", "world"]);
                assert_eq!(lean[side][i]["world"], full[side][i]["world"]);
            }
        }
    }

    /// The JSON text of each child of `world` in record `i` of `side`, read from the body without re-printing it.
    fn world_children_text(body: &[u8], side: &str, i: usize) -> BTreeMap<String, String> {
        let top: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(body).unwrap();
        let records: Vec<BTreeMap<String, Box<RawValue>>> = serde_json::from_str(top[side].get()).unwrap();
        let world: BTreeMap<String, Box<RawValue>> = serde_json::from_str(records[i]["world"].get()).unwrap();
        world.into_iter().map(|(child, text)| (child, text.get().to_owned())).collect()
    }

    /// `?fields=world.<child>` on the compare endpoint with the world block on: each record keeps the listed children of
    /// `world` and the month key, and each kept child has the same JSON text as in the unfiltered response.
    #[tokio::test]
    async fn fields_keep_nested_children_bit_exact() {
        let st = state(64 << 20);
        let world_req = REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#);
        let (_, full) = call(&st, &world_req).await;
        let (_, lean) = call_q(&st, "fields=world.imports_bn,world.y_gap", &world_req).await;
        for side in ["baseline", "shocked"] {
            for i in 0..3 {
                let kept = world_children_text(&lean, side, i);
                assert_eq!(kept.keys().map(String::as_str).collect::<Vec<_>>(), ["imports_bn", "y_gap"]);
                let all = world_children_text(&full, side, i);
                for (child, text) in &kept {
                    assert_eq!(text, &all[child], "{side} {i} {child}");
                }
            }
        }
    }

    /// Without `fields`, and with an empty `fields=` (or one that names nothing), the body is the serialised comparison,
    /// byte for byte, as before the filter existed.
    #[tokio::test]
    async fn no_fields_gives_the_same_bytes_as_before() {
        let sc = parse_compare(REQ.as_bytes(), 0).unwrap();
        let expected = serde_json::to_vec(&sim::compare(&sc)).unwrap();
        let st = state(0);
        let (_, plain) = call(&st, REQ).await;
        assert_eq!(&plain[..], &expected[..]);
        for q in ["fields=", "fields=,", "fields=%20"] {
            let (_, empty) = call_q(&st, q, REQ).await;
            assert_eq!(&empty[..], &plain[..], "query {q}");
        }
    }

    /// The cache header is the same with and without `fields`. Requests with the same body share one entry: a filtered
    /// request hits the body an unfiltered one stored, and the cache keeps the unfiltered body.
    #[tokio::test]
    async fn cache_header_is_unchanged_by_fields() {
        let st = state(64 << 20);
        let (first, lean) = call_q(&st, "fields=cpi", REQ).await;
        let (second, full) = call(&st, REQ).await;
        let (third, lean_again) = call_q(&st, "fields=gdp_real_bn_ar", REQ).await;
        assert_eq!((first.as_str(), second.as_str(), third.as_str()), ("miss", "hit", "hit"));
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 1);
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        let lean_again: Value = serde_json::from_slice(&lean_again).unwrap();
        let full: Value = serde_json::from_slice(&full).unwrap();
        assert_eq!(keys(&lean["baseline"][0]), ["cpi", "tick"]);
        assert_eq!(keys(&lean_again["baseline"][0]), ["gdp_real_bn_ar", "tick"]);
        assert!(keys(&full["baseline"][0]).len() > 100, "the cached body is the unfiltered one");
    }

    /// Body bytes and content type of `GET /api/sims/:id` with the given query string.
    async fn get_sim_q(st: &Shared, id: &str, query: &str) -> (String, Bytes) {
        let uri: Uri = format!("/api/sims/{id}?{query}").parse().unwrap();
        let q = Query::<HashMap<String, String>>::try_from_uri(&uri).unwrap();
        let resp = get_sim(State(st.clone()), Path(id.to_string()), q).await.unwrap();
        let ctype = resp.headers()[header::CONTENT_TYPE].to_str().unwrap().to_string();
        (ctype, to_bytes(resp.into_body(), usize::MAX).await.unwrap())
    }

    /// A live simulation with 1,000 households, stepped three months so it has history, registered under a fresh id.
    fn live_sim(st: &Shared) -> String {
        let mut world = sim::new_world(&Config { n_households: 1000, ..Config::default() }, 7);
        sim::run(&mut world, 3);
        register(st, SimEntry::new(world, 7)).id
    }

    /// `GET /api/sims/:id` with `fields` filters `history` only. Without it the body is the serialised `SimState`, as before.
    #[tokio::test]
    async fn get_sim_filters_the_history_only() {
        let st = state(0);
        let id = live_sim(&st);
        let (ctype, full) = get_sim_q(&st, &id, "").await;
        assert_eq!(ctype, "application/json");
        let expected = {
            let e = get_entry(&st, &id).unwrap();
            let g = lock(&e.entry);
            serde_json::to_vec(&SimState { info: info_of(&id, &g), history: g.world().history.clone() }).unwrap()
        };
        assert_eq!(&full[..], &expected[..]);
        let (ctype, lean) = get_sim_q(&st, &id, "fields=cpi,policy_rate").await;
        assert_eq!(ctype, "application/json");
        let full: Value = serde_json::from_slice(&full).unwrap();
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        for k in ["id", "tick", "seed", "n_households", "applied_shocks", "pending_shocks"] {
            assert_eq!(lean[k], full[k], "{k}");
        }
        let (fh, lh) = (full["history"].as_array().unwrap(), lean["history"].as_array().unwrap());
        assert!(!fh.is_empty());
        assert_eq!(lh.len(), fh.len());
        for (f, l) in fh.iter().zip(lh) {
            assert_eq!(keys(l), ["cpi", "policy_rate", "tick"]);
            for k in ["cpi", "policy_rate", "tick"] {
                assert_eq!(l[k], f[k], "{k}");
            }
        }
    }

    /// `POST /api/sims/:id/step` with `fields` filters `metrics` only. The same months read back unfiltered from
    /// `GET /api/sims/:id` carry the same values for the kept keys.
    #[tokio::test]
    async fn step_filters_the_metrics_only() {
        let st = state(0);
        let id = live_sim(&st);
        let uri: Uri = "/api/sims/x/step?fields=cpi,policy_rate".parse().unwrap();
        let q = Query::<HashMap<String, String>>::try_from_uri(&uri).unwrap();
        let resp = step_sim(State(st.clone()), Path(id.clone()), q, Bytes::from_static(br#"{"months":2}"#))
            .await
            .unwrap();
        let lean = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let lean: Value = serde_json::from_slice(&lean).unwrap();
        let (_, full) = get_sim_q(&st, &id, "").await;
        let full: Value = serde_json::from_slice(&full).unwrap();
        assert_eq!(lean["tick"], full["tick"]);
        let (lm, fh) = (lean["metrics"].as_array().unwrap(), full["history"].as_array().unwrap());
        assert_eq!(lm.len(), 2);
        for (l, f) in lm.iter().zip(&fh[fh.len() - 2..]) {
            assert_eq!(keys(l), ["cpi", "policy_rate", "tick"]);
            for k in ["cpi", "policy_rate", "tick"] {
                assert_eq!(l[k], f[k], "{k}");
            }
        }
    }

    /// The status and the `error` message of a handler's error response.
    async fn error_of(e: ApiError) -> (StatusCode, String) {
        let resp = e.into_response();
        let status = resp.status();
        let body: Value = serde_json::from_slice(&to_bytes(resp.into_body(), usize::MAX).await.unwrap()).unwrap();
        (status, body["error"].as_str().unwrap().to_string())
    }

    /// Moves bank 0's deposit liability in economy `id` off the sum of its customers' deposits (no ledger entry), as a stock-flow
    /// bug would, so the economy's next month panics (`sfc_enforce`).
    fn corrupt(st: &Shared, id: &str) {
        let e = get_entry(st, id).unwrap();
        let mut g = lock(&e.entry);
        let b = &mut g.world_mut().banks[0];
        b.deposits += 0.01 * b.deposits.abs() + 1.0;
    }

    /// The step request of `months` months on economy `id`.
    async fn step_months(st: &Shared, id: &str, months: u32) -> Result<Response, ApiError> {
        let body = Bytes::from(format!(r#"{{"months":{months}}}"#));
        step_sim(State(st.clone()), Path(id.to_string()), Query(HashMap::new()), body).await
    }

    /// sfc_enforce: a month that panics (here a stock-flow inconsistency injected into a live economy) answers a JSON 500 with the
    /// panic message. The half-stepped economy is then refused on every route but the list and DELETE, its mutex is not poisoned,
    /// its world is dropped, the list says why it stopped (`stopped`, omitted for a running economy), and the other economies are
    /// unaffected.
    #[tokio::test]
    async fn a_panicking_step_is_a_500_and_stops_the_economy() {
        let st = state(0);
        let (id, other) = (live_sim(&st), live_sim(&st));
        let e = get_entry(&st, &id).unwrap();
        corrupt(&st, &id);
        let Err(err) = step_months(&st, &id, 2).await else { panic!("an inconsistent month must fail") };
        let (status, msg) = error_of(err).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(msg.contains("stock-flow inconsistency at tick 3"), "{msg}");
        assert!(!e.entry.is_poisoned());
        assert!(matches!(lock(&e.entry).economy, Economy::Stopped(_)), "the half-stepped world is dropped");
        let stopped = |r: Result<(), ApiError>| async move {
            let (status, msg) = error_of(r.expect_err("a stopped economy must be refused")).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(msg.contains("stopped") && msg.contains("stock-flow inconsistency"), "{msg}");
        };
        stopped(step_months(&st, &id, 2).await.map(|_| ())).await;
        stopped(get_sim(State(st.clone()), Path(id.clone()), Query(HashMap::new())).await.map(|_| ())).await;
        stopped(snapshot_sim(State(st.clone()), Path(id.clone())).await.map(|_| ())).await;
        stopped(fork_sim(State(st.clone()), Path(id.clone())).await.map(|_| ())).await;
        let shock = Bytes::from_static(br#"{"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}"#);
        stopped(add_shock(State(st.clone()), Path(id.clone()), shock).await.map(|_| ())).await;
        // The other economy steps on; the stopped one is still listed, as of the month that failed, and can be deleted.
        assert!(step_months(&st, &other, 2).await.is_ok());
        let listed = serde_json::to_value(list_sims(State(st.clone())).await.unwrap().0).unwrap();
        let listed = listed.as_array().unwrap();
        let info = |sim: &str| listed.iter().find(|s| s["id"] == sim).unwrap().clone();
        let (gone, running) = (info(&id), info(&other));
        assert_eq!(gone["tick"], 3, "the stopped economy was not stepped again");
        assert_eq!(gone["seed"], 7);
        let why = gone["stopped"].as_str().unwrap();
        assert!(why.starts_with("a month panicked at tick 3: stock-flow inconsistency"), "{why}");
        assert_eq!(running["tick"], 5);
        assert!(running.get("stopped").is_none(), "a running economy has no `stopped` key");
        assert!(delete_sim(State(st.clone()), Path(id.clone())).await.is_ok());
        assert!(matches!(get_sim(State(st.clone()), Path(id), Query(HashMap::new())).await, Err(ApiError::NotFound(_))));
    }

    /// R17: a request that a stopped economy refuses does not touch it, so a client polling it neither keeps it from expiring nor
    /// keeps it ahead of running economies under the cap: the next economy past the cap removes it, not the running one.
    #[tokio::test]
    async fn a_stopped_economy_is_not_touched_by_the_requests_it_refuses() {
        let clock = ManualClock::new();
        let st = state_with_sims(SimPolicy { ttl: Duration::from_secs(6 * 3600), cap: 2 }, Arc::clone(&clock));
        let (id, other) = (live_sim(&st), live_sim(&st));
        corrupt(&st, &id);
        assert!(step_months(&st, &id, 1).await.is_err());
        clock.advance(Duration::from_secs(1));
        get_sim(State(st.clone()), Path(other.clone()), Query(HashMap::new())).await.unwrap();
        let touched = |sim: &str| lock(&st.sims).map.get(sim).map(|s| (s.touched_at, s.seq)).unwrap();
        let last = touched(&id);
        for _ in 0..3 {
            clock.advance(Duration::from_secs(3600));
            assert!(get_sim(State(st.clone()), Path(id.clone()), Query(HashMap::new())).await.is_err());
            assert!(snapshot_sim(State(st.clone()), Path(id.clone())).await.is_err());
            assert!(step_months(&st, &id, 1).await.is_err());
        }
        assert_eq!(touched(&id), last, "a refused request touched the stopped economy");
        let third = tiny_sim(&st, 10);
        let mut expect = vec![other, third];
        expect.sort();
        assert_eq!(sim_ids(&st), expect);
    }

    /// A compare whose computation panics (the test hook `with_failure_at_year(2)` panics after the second year of reported
    /// months) answers a JSON 500. The flight is cleared, so the same request computes again instead of waiting on it, and the
    /// permits are released (two failures on a two-permit cache, then a valid request of one year still computes).
    #[tokio::test]
    async fn a_panicking_compare_is_a_500_and_frees_its_flight_and_permit() {
        let st = state_with(CompareCache::new(64 << 20, 2).with_failure_at_year(2), None);
        let bad = r#"{"config":{"n_households":300,"burn_in_months":12},"months":13,"seeds":[1]}"#;
        for n in 1..=2u64 {
            let Err(err) = compare(State(st.clone()), Query(HashMap::new()), Bytes::from(bad)).await else { panic!("must fail") };
            let (status, msg) = error_of(err).await;
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(msg.contains("panicked") && msg.contains("injected failure in year 2"), "{msg}");
            assert_eq!(st.compare.computed.load(Ordering::SeqCst), n, "a failed computation is neither stored nor left in flight");
        }
        let (cache, _) = call(&st, REQ).await;
        assert_eq!(cache, "miss");
    }

    /// A bound pair that the run would pass to `f64::clamp` with the lower bound above the upper one (a panic, so a 500) is a 400
    /// on create and on compare, before anything runs. A pair the run does not use passes: the mortgage-spread bounds without a
    /// spread gain, and `r_star_est_min` without `r_star_adapt`. Equal bounds pass.
    #[tokio::test]
    async fn clamp_bounds_out_of_order_are_a_400() {
        let st = state(64 << 20);
        let bad = [
            (r#""u_star_min":0.09,"u_star_max":0.01"#, "u_star_min = 0.09 is above u_star_max = 0.01"),
            (r#""ebp_min":0.05,"ebp_max":0.04"#, "ebp_min = 0.05 is above ebp_max = 0.04"),
            (
                r#""mortgage_spread_min_adj":0.02,"mortgage_spread_max_adj":0.01,"mortgage_spread_stress_gain":0.5"#,
                "mortgage_spread_min_adj = 0.02 is above mortgage_spread_max_adj = 0.01",
            ),
            (r#""r_star_est_min":0.05,"r_star_adapt":0.3"#, "r_star_est_min = 0.05 is above the r* estimate's upper bound = 0.03"),
        ];
        for (keys, want) in bad {
            let create = create_sim(State(st.clone()), Bytes::from(format!(r#"{{"config":{{"n_households":300,{keys}}}}}"#))).await;
            let body = format!(r#"{{"config":{{"n_households":300,{keys}}},"months":2,"seeds":[1]}}"#);
            let compared = compare(State(st.clone()), Query(HashMap::new()), Bytes::from(body)).await;
            for r in [create.map(|_| ()), compared.map(|_| ())] {
                let (status, msg) = error_of(r.expect_err(keys)).await;
                assert_eq!(status, StatusCode::BAD_REQUEST, "{keys}");
                assert!(msg.contains(want), "{msg}");
            }
        }
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 0, "nothing ran");
        assert!(lock(&st.sims).map.is_empty(), "nothing was created");
        let d = Config::default();
        for ok in [
            Config { mortgage_spread_min_adj: 0.02, mortgage_spread_max_adj: 0.01, ..d.clone() },
            Config { r_star_est_min: 0.05, ..d.clone() },
            Config { u_star_min: 0.05, u_star_max: 0.05, ebp_min: 0.01, ebp_max: 0.01, ..d.clone() },
        ] {
            assert!(validate_config(&ok).is_ok());
        }
    }

    // ---- R13-S: common-world cache, no-change fast path, streaming ------------------------------------------------------------

    /// A server whose compare cache holds the common worlds (64 MB), and the year-1 gate of `gate` when one is given.
    fn state_with_worlds() -> Shared {
        state_with(CompareCache::new(64 << 20, 2).with_worlds(64 << 20), None)
    }

    /// A one-seed request of 14 months (two years), 1,000 households, one oil shock.
    const REQ1: &str = r#"{"config":{"n_households":1000},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":14,"seeds":[1]}"#;

    /// Posts a streamed compare and returns the cache header and the NDJSON lines (each without its newline).
    async fn call_stream(st: &Shared, query: &str, body: &str) -> (String, Vec<Vec<u8>>) {
        let uri: Uri = format!("/api/compare?stream=1{query}").parse().unwrap();
        let q = Query::<HashMap<String, String>>::try_from_uri(&uri).unwrap();
        let resp = compare(State(st.clone()), q, Bytes::from(body.to_string())).await.unwrap();
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/x-ndjson");
        let cache = resp.headers()[CACHE_HEADER].to_str().unwrap().to_string();
        let all = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let lines = all.split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(<[u8]>::to_vec).collect();
        (cache, lines)
    }

    /// The bytes of a `done` line for `body` (a cached body).
    fn done_of(body: &[u8]) -> Vec<u8> {
        [br#"{"done":true,"body":"#.as_slice(), body, b"}"].concat()
    }

    /// The bytes of a `done` line for `rest` (a computed body after its year lines).
    fn rest_done_of(rest: &[u8]) -> Vec<u8> {
        [br#"{"done":true,"rest":"#.as_slice(), rest, b"}"].concat()
    }

    /// The rest of a plain compare body: `{` and the keys after its two record arrays, read from the body text (so the key order is
    /// the body's own), or `{}` when nothing follows them.
    fn rest_of(full: &[u8]) -> Vec<u8> {
        #[derive(Deserialize)]
        struct Records {
            baseline: Box<RawValue>,
            shocked: Box<RawValue>,
        }
        let records: Records = serde_json::from_slice(full).unwrap();
        let head = format!(r#"{{"baseline":{},"shocked":{}"#, records.baseline.get(), records.shocked.get());
        assert!(full.starts_with(head.as_bytes()));
        let tail = &full[head.len()..];
        if tail == b"}" {
            b"{}".to_vec()
        } else {
            [b"{".as_slice(), &tail[1..]].concat()
        }
    }

    /// The body a streamed compare rebuilds to, as a client reads it: the rest of the `done` line, then the records of the year lines
    /// in year order, as the `baseline` and `shocked` arrays.
    fn rebuild(lines: &[Vec<u8>]) -> Value {
        let (done, years) = lines.split_last().unwrap();
        let done: Value = serde_json::from_slice(done).unwrap();
        assert!(done.get("body").is_none() && done.get("rest").is_some(), "{done}");
        let mut body = done["rest"].as_object().unwrap().clone();
        assert!(!body.contains_key("baseline") && !body.contains_key("shocked"));
        let (mut baseline, mut shocked) = (Vec::new(), Vec::new());
        for line in years {
            let year: Value = serde_json::from_slice(line).unwrap();
            baseline.extend(year["baseline"].as_array().unwrap().iter().cloned());
            shocked.extend(year["shocked"].as_array().unwrap().iter().cloned());
        }
        body.insert("baseline".to_string(), Value::Array(baseline));
        body.insert("shocked".to_string(), Value::Array(shocked));
        Value::Object(body)
    }

    /// (a) A compare whose common worlds come from the world cache gives the bytes of a fresh compare, for typical, us_now and the
    /// world-profile config. The second request differs only in its shocks and its months, so it misses the body cache, hits the
    /// world cache and builds no world.
    #[tokio::test(flavor = "multi_thread")]
    async fn world_cache_gives_the_same_bytes() {
        let world = REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#);
        let us = us_body(|_| {});
        let cases = [
            (REQ.to_string(), REQ.replace(r#""months":3"#, r#""months":4"#).replace(r#""at_month":1"#, r#""at_month":2"#)),
            (us.clone(), us.replace(r#""months":2"#, r#""months":3"#)),
            (world.clone(), world.replace(r#""months":3"#, r#""months":4"#)),
        ];
        for (first_body, other_body) in cases {
            let (_, fresh) = call(&state(0), &first_body).await;
            let st = state_with_worlds();
            let (miss, first) = call(&st, &first_body).await;
            assert_eq!((miss.as_str(), &first[..]), ("miss", &fresh[..]));
            let built = st.compare.worlds_built.load(Ordering::SeqCst);
            assert!(built > 0);
            let (other_cache, other) = call(&st, &other_body).await;
            assert_eq!(other_cache, "miss");
            assert_eq!(st.compare.worlds_built.load(Ordering::SeqCst), built, "a world-cache hit builds nothing");
            let (_, other_fresh) = call(&state(0), &other_body).await;
            assert_eq!(&other[..], &other_fresh[..]);
        }
    }

    /// (b, c) The world key follows the config and the seed, and not the shocks or the months.
    #[test]
    fn world_key_follows_the_config_and_the_seed_not_the_shocks() {
        let k = |body: &str, seed: u64| cache::world_key(&parse_compare(body.as_bytes(), 0).unwrap().config, seed);
        let base = k(REQ, 1);
        assert_eq!(base, k(&REQ.replace(r#""at_month":1"#, r#""at_month":2"#), 1), "shocks do not enter the key");
        assert_eq!(base, k(&REQ.replace(r#""months":3"#, r#""months":9"#), 1), "months do not enter the key");
        assert_ne!(base, k(&REQ.replace(r#""n_households":1000"#, r#""n_households":1001"#), 1), "households");
        assert_ne!(base, k(&REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"burn_in_months":12"#), 1), "burn-in");
        assert_ne!(base, k(&REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"warmup_months":12"#), 1), "warm-up");
        assert_ne!(base, k(&REQ.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#), 1), "world block");
        assert_ne!(base, k(REQ, 2), "the seed");
        let us = k(&us_body(|_| {}), 1);
        let with_shock = us_body(|v| v["shocks"] = json!([{"at_month": 0, "shock": {"type": "PolicyShift", "bps": 100.0, "months": 12}}]));
        assert_eq!(us, k(&with_shock, 1), "a shock on the us_now request does not enter the key");
        assert_ne!(us, k(&us_body(|v| v["config"]["start_targets"]["unemployment_rate"] = json!(4.3)), 1), "the us_now targets");
    }

    /// (R13-S streaming) The year lines of a streamed compare hold the full body's records for their months, in order, per arm, and
    /// the `done` line holds the body's rest (its keys after the two arrays) byte for byte.
    #[tokio::test(flavor = "multi_thread")]
    async fn stream_lines_are_the_full_body_and_done_is_byte_identical() {
        let (_, full) = call(&state(0), REQ1).await;
        let (cache, lines) = call_stream(&state(0), "", REQ1).await;
        assert_eq!(cache, "miss");
        assert_eq!(lines.len(), 3, "two year lines and the done line");
        assert_eq!(lines[2], rest_done_of(&rest_of(&full)));

        let full_top: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(&full).unwrap();
        for side in ["baseline", "shocked"] {
            let full_records: Vec<Box<RawValue>> = serde_json::from_str(full_top[side].get()).unwrap();
            let mut year_records: Vec<Box<RawValue>> = Vec::new();
            for line in &lines[..2] {
                let year: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(line).unwrap();
                let records: Vec<Box<RawValue>> = serde_json::from_str(year[side].get()).unwrap();
                year_records.extend(records);
            }
            assert_eq!(year_records.len(), 14, "{side}");
            for (y, f) in year_records.iter().zip(&full_records) {
                assert_eq!(y.get(), f.get(), "{side}");
            }
        }
        let years: Vec<Value> = lines[..2].iter().map(|l| serde_json::from_slice(l).unwrap()).collect();
        assert_eq!((years[0]["year"].clone(), years[0]["from"].clone(), years[0]["to"].clone()), (json!(1), json!(0), json!(12)));
        assert_eq!((years[1]["year"].clone(), years[1]["from"].clone(), years[1]["to"].clone()), (json!(2), json!(12), json!(14)));
    }

    /// (R13-S) The client rebuilds a streamed compare from its year lines and the rest: typical, us_now and world-profile, with and
    /// without `fields`. The rebuilt body equals the plain body (of the same request) as JSON values.
    #[tokio::test(flavor = "multi_thread")]
    async fn stream_rebuilds_the_body_from_the_year_lines_and_the_rest() {
        let world = REQ1.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#);
        for body in [REQ1.to_string(), us_body(|_| {}), world] {
            let (_, lines) = call_stream(&state(0), "", &body).await;
            let (_, plain) = call(&state(0), &body).await;
            assert_eq!(rebuild(&lines), serde_json::from_slice::<Value>(&plain).unwrap());
            let (_, lean_lines) = call_stream(&state(0), "&fields=cpi,world.y_gap", &body).await;
            let (_, lean) = call_q(&state(0), "fields=cpi,world.y_gap", &body).await;
            assert_eq!(rebuild(&lean_lines), serde_json::from_slice::<Value>(&lean).unwrap());
        }
    }

    /// (R13-S) `fields` applies to every line: the records of each year keep the listed keys and the month key, nested names included,
    /// and the `done` body is the filtered non-stream body.
    #[tokio::test(flavor = "multi_thread")]
    async fn stream_applies_fields_to_every_line() {
        let world_req = REQ1.replace(r#""n_households":1000"#, r#""n_households":1000,"world_block_enabled":true"#);
        let (_, lines) = call_stream(&state(0), "&fields=world.y_gap,cpi", &world_req).await;
        for line in &lines[..lines.len() - 1] {
            let year: Value = serde_json::from_slice(line).unwrap();
            for side in ["baseline", "shocked"] {
                for rec in year[side].as_array().unwrap() {
                    assert_eq!(keys(rec), ["cpi", "tick", "world"]);
                    assert_eq!(keys(&rec["world"]), ["y_gap"]);
                }
            }
        }
        // The rest is not filtered (it holds no records), so it is the unfiltered body's rest, in the body's own key order.
        let (_, plain) = call(&state(0), &world_req).await;
        assert_eq!(lines[lines.len() - 1], rest_done_of(&rest_of(&plain)));
    }

    /// (R13-S) Streaming takes one seed: several seeds, or a `stream` value other than 0 or 1, is a 400 that names the rule.
    #[tokio::test]
    async fn stream_needs_one_seed_and_a_known_value() {
        let stream_q = |v: &str| Query(HashMap::from([("stream".to_string(), v.to_string())]));
        match compare(State(state(0)), stream_q("1"), Bytes::from(REQ)).await {
            Err(ApiError::BadRequest(m)) => assert!(m.contains("exactly one seed"), "{m}"),
            _ => panic!("several seeds were streamed"),
        }
        match compare(State(state(0)), stream_q("yes"), Bytes::from(REQ1)).await {
            Err(ApiError::BadRequest(m)) => assert!(m.contains("stream must be 1 or 0"), "{m}"),
            _ => panic!("stream=yes was accepted"),
        }
    }

    /// (R13-S) A streamed request for a body in the memory cache is one `done` line, marked a hit, with the cached bytes; the
    /// computation is not run again.
    #[tokio::test(flavor = "multi_thread")]
    async fn stream_of_a_cached_body_is_one_done_line() {
        let st = state(64 << 20);
        let (_, full) = call(&st, REQ1).await;
        let (cache, lines) = call_stream(&st, "", REQ1).await;
        assert_eq!(cache, "hit");
        assert_eq!(lines, vec![done_of(&full)]);
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 1);
    }

    /// (R13-S) The stream response is not to be buffered or transformed: `no-cache, no-transform`, and the NDJSON type.
    #[tokio::test]
    async fn stream_response_is_marked_as_not_to_be_buffered() {
        let uri: Uri = "/api/compare?stream=1".parse().unwrap();
        let q = Query::<HashMap<String, String>>::try_from_uri(&uri).unwrap();
        let resp = compare(State(state(0)), q, Bytes::from(REQ1)).await.unwrap();
        assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/x-ndjson");
        assert_eq!(resp.headers()[header::CACHE_CONTROL], "no-cache, no-transform");
    }

    /// (R13-S, single-flight) Two cold requests for one config and seed, with different shocks, build the world once: the second waits
    /// for the first build and takes the same world. Both bodies equal a fresh run. The first build is held until the second request
    /// has reached the world cache, so the two overlap for certain.
    #[tokio::test(flavor = "multi_thread")]
    async fn two_cold_requests_for_one_world_build_it_once() {
        let gate = Arc::new(std::sync::Barrier::new(2));
        let st = state_with(CompareCache::new(64 << 20, 2).with_worlds(64 << 20).with_world_gate(Arc::clone(&gate)), None);
        let fed = REQ1.replace(
            r#""shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}]"#,
            r#""shocks":[{"at_month":0,"shock":{"type":"PolicyShift","bps":100.0,"months":12}}]"#,
        );
        assert_ne!(fed, REQ1);
        let (s1, s2) = (Arc::clone(&st), Arc::clone(&st));
        let (a, b) = (REQ1.to_string(), fed.clone());
        let first = tokio::spawn(async move { call(&s1, &a).await });
        let second = tokio::spawn(async move { call(&s2, &b).await });
        while st.compare.world_arrivals.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        gate.wait();
        let (_, body_a) = first.await.unwrap();
        let (_, body_b) = second.await.unwrap();
        assert_eq!(st.compare.worlds_built.load(Ordering::SeqCst), 1, "one world for both requests");
        assert_eq!(st.compare.computed.load(Ordering::SeqCst), 2, "two different bodies were computed");
        let (_, fresh_a) = call(&state(0), REQ1).await;
        let (_, fresh_b) = call(&state(0), &fed).await;
        assert_eq!(&body_a[..], &fresh_a[..]);
        assert_eq!(&body_b[..], &fresh_b[..]);
    }

    /// (R13-S, a) Requests with different shocks run at once on one cached world: neither builds a world, and each body equals a fresh
    /// run of its own request.
    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_shocks_on_a_cached_world_match_fresh_runs() {
        let st = state_with_worlds();
        call(&st, REQ1).await; // builds the world
        let built = st.compare.worlds_built.load(Ordering::SeqCst);
        let fed = REQ1.replace(
            r#""shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}]"#,
            r#""shocks":[{"at_month":0,"shock":{"type":"PolicyShift","bps":100.0,"months":12}}]"#,
        );
        let shorter = REQ1.replace(r#""months":14"#, r#""months":13"#);
        assert_ne!(shorter, REQ1);
        let ((_, x), (_, y)) = tokio::join!(call(&st, &fed), call(&st, &shorter));
        assert_eq!(st.compare.worlds_built.load(Ordering::SeqCst), built, "both requests used the stored world");
        let (_, fresh_x) = call(&state(0), &fed).await;
        let (_, fresh_y) = call(&state(0), &shorter).await;
        assert_eq!(&x[..], &fresh_x[..]);
        assert_eq!(&y[..], &fresh_y[..]);
    }

    // ---- R17: idle expiry and the cap of the sims map ----------------------------------------------------------------------------

    /// A clock the test sets: the expiry tests move it forward instead of sleeping.
    struct ManualClock(Mutex<Instant>);

    impl ManualClock {
        fn new() -> Arc<Self> {
            Arc::new(ManualClock(Mutex::new(Instant::now())))
        }

        fn advance(&self, by: Duration) {
            *lock(&self.0) += by;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> Instant {
            *lock(&self.0)
        }
    }

    /// A server whose `sims` map has the given policy and clock.
    fn state_with_sims(policy: SimPolicy, clock: Arc<ManualClock>) -> Shared {
        Arc::new(AppState {
            sims: Mutex::new(SimStore::new(policy, clock)),
            counter: AtomicU64::new(1),
            compare: Arc::new(CompareCache::new(1 << 20, 2)),
            warmup_default: 0,
            prewarm: None,
        })
    }

    /// The ids in the `sims` map, sorted, without sweeping it first.
    fn sim_ids(st: &Shared) -> Vec<String> {
        let mut ids: Vec<String> = lock(&st.sims).map.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// A small economy of `n` households, not run, created through `register` as `create_sim` does.
    fn tiny_sim(st: &Shared, n: usize) -> String {
        let world = sim::new_world(&Config { n_households: n, ..Config::default() }, 3);
        register(st, SimEntry::new(world, 3)).id
    }

    /// R17: an economy untouched for the TTL is removed exactly at the TTL (not before); one created later is kept. The removal
    /// says why, and a removed economy answers 404.
    #[tokio::test]
    async fn idle_economy_is_removed_at_the_ttl() {
        let clock = ManualClock::new();
        let st = state_with_sims(SimPolicy::default(), Arc::clone(&clock));
        let old = live_sim(&st);
        clock.advance(Duration::from_secs(6 * 3600 - 1));
        let young = live_sim(&st);
        assert!(sweep_sims(&st).is_empty(), "removed one second before the TTL");
        clock.advance(Duration::from_secs(1));
        let removed = lock(&st.sims).expire();
        assert_eq!(removed.iter().map(|r| r.id.clone()).collect::<Vec<_>>(), vec![old.clone()]);
        assert_eq!(removed[0].reason, "expired (untouched for 6 h)");
        assert_eq!(sim_ids(&st), vec![young.clone()]);
        assert!(matches!(get_entry(&st, &old), Err(ApiError::NotFound(_))));
        assert!(get_entry(&st, &young).is_ok());
    }

    /// R17: every touch (a read, a step, a shock, a snapshot, a fork, a create) keeps an economy; a list does not touch it. At 6 h
    /// only the economy nobody touched since its creation is removed, and the fork's copy (made at 4 h) is kept.
    #[tokio::test]
    async fn every_touch_keeps_an_economy_and_a_list_does_not() {
        let clock = ManualClock::new();
        let st = state_with_sims(SimPolicy::default(), Arc::clone(&clock));
        let ids: Vec<String> = (0..6).map(|_| live_sim(&st)).collect();
        clock.advance(Duration::from_secs(4 * 3600));
        get_sim(State(st.clone()), Path(ids[4].clone()), Query(HashMap::new())).await.unwrap();
        step_sim(
            State(st.clone()),
            Path(ids[0].clone()),
            Query(HashMap::new()),
            Bytes::from_static(br#"{"months":1}"#),
        )
        .await
        .unwrap();
        let shocked = add_shock(
            State(st.clone()),
            Path(ids[1].clone()),
            Bytes::from_static(br#"{"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}"#),
        )
        .await
        .unwrap();
        assert_eq!(shocked.0.pending_shocks.len(), 1);
        let snap = snapshot_sim(State(st.clone()), Path(ids[2].clone())).await.unwrap();
        assert_eq!(snap.0.tick, 3);
        let fork = fork_sim(State(st.clone()), Path(ids[3].clone())).await.unwrap().0.id;
        assert_eq!(list_sims(State(st.clone())).await.unwrap().0.len(), 7);
        clock.advance(Duration::from_secs(2 * 3600));
        assert_eq!(sweep_sims(&st), vec![ids[5].clone()]);
        for kept in [&ids[0], &ids[1], &ids[2], &ids[3], &ids[4], &fork] {
            assert!(get_entry(&st, kept).is_ok(), "{kept} was removed");
        }
    }

    /// R17: the cap. Creating one more economy than the cap removes the least recently touched one and only that; a read
    /// changes which one that is. The removal names the cap.
    #[tokio::test]
    async fn creating_past_the_cap_removes_the_least_recently_touched() {
        let clock = ManualClock::new();
        let st = state_with_sims(SimPolicy { ttl: Duration::from_secs(6 * 3600), cap: 3 }, Arc::clone(&clock));
        let a = tiny_sim(&st, 10);
        clock.advance(Duration::from_secs(1));
        let b = tiny_sim(&st, 10);
        clock.advance(Duration::from_secs(1));
        let c = tiny_sim(&st, 10);
        clock.advance(Duration::from_secs(1));
        get_entry(&st, &a).unwrap(); // a is touched, so b is now the least recently touched
        clock.advance(Duration::from_secs(1));
        let d = tiny_sim(&st, 10);
        let mut expect = vec![a.clone(), c.clone(), d.clone()];
        expect.sort();
        assert_eq!(sim_ids(&st), expect);
        assert!(matches!(get_entry(&st, &b), Err(ApiError::NotFound(_))));
        let more = lock(&st.sims).make_room();
        assert_eq!(more.len(), 1);
        assert_eq!(more[0].reason, "evicted (least recently touched; cap 3 reached)");
    }

    /// R17: at the default cap of 200, the 201st economy removes the first one created (never touched since).
    #[test]
    fn the_201st_economy_removes_the_first_at_the_default_cap() {
        let st = state_with_sims(SimPolicy::default(), ManualClock::new());
        let ids: Vec<String> = (0..201).map(|_| tiny_sim(&st, 10)).collect();
        let kept = sim_ids(&st);
        assert_eq!(kept.len(), 200);
        assert!(!kept.contains(&ids[0]), "the first economy is still there");
        assert!(ids[1..].iter().all(|id| kept.contains(id)));
    }

    /// R17: `ECONSIM_SIM_TTL_HOURS` and `ECONSIM_SIM_CAP` take positive values; anything else gives the default, 6 h and 200.
    #[test]
    fn sim_policy_env_parsing() {
        assert_eq!(SimPolicy::default(), SimPolicy { ttl: Duration::from_secs(6 * 3600), cap: 200 });
        assert_eq!(parse_sim_ttl(None), Duration::from_secs(6 * 3600));
        assert_eq!(parse_sim_ttl(Some("2")), Duration::from_secs(2 * 3600));
        assert_eq!(parse_sim_ttl(Some("0.5")), Duration::from_secs(1800));
        for bad in ["0", "-1", "abc", "", "NaN", "inf"] {
            assert_eq!(parse_sim_ttl(Some(bad)), Duration::from_secs(6 * 3600), "TTL {bad:?}");
        }
        assert_eq!(parse_sim_cap(None), 200);
        assert_eq!(parse_sim_cap(Some("5")), 5);
        for bad in ["0", "-3", "1.5", "many", ""] {
            assert_eq!(parse_sim_cap(Some(bad)), 200, "cap {bad:?}");
        }
    }
}
