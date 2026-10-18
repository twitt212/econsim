//! Response cache, single-flight and compute limiter for `POST /api/compare` (docs/API.md, "Compare caching").
//!
//! A compare result is a pure function of the validated [`Scenario`], so the cache key is that scenario's canonical
//! JSON (re-serialised from the parsed struct: key order, whitespace, number spelling and omitted defaults in the
//! request do not matter) and the value is the exact response body. Identical requests that arrive while a
//! computation is running share it. Every computation takes a permit from a semaphore, so at most `concurrency`
//! of them run at once; the rest wait without occupying a blocking thread.
//!
//! With `ECONSIM_CACHE_DIR` set, every computed body is also written to a [`DiskCache`], in a folder named for this
//! build, and a memory miss reads the disk before computing. A restarted server therefore answers from disk. A new
//! binary has a new folder, so it never serves an old binary's bytes.
//!
//! R13-S: a computation also reports each finished year as one NDJSON line, in a progress channel that a streamed request
//! replays and follows (joining the computation if it is running). The finished body is cached exactly as before. The warmed
//! common world of each seed is kept in a second LRU (`ECONSIM_WORLD_CACHE_MB`), keyed by the config and the seed, so a request
//! that differs only in its shocks or its months skips the warm-up.

use std::collections::{BTreeMap, HashMap};
use std::env;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, SystemTime};

use axum::body::Bytes;
use econsim_core::config::Config;
use econsim_core::metrics::Metrics;
use econsim_core::sim::{self, CommonWorld, Year};
use econsim_core::Scenario;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch, OwnedSemaphorePermit, Semaphore};

use crate::fields::{self, FieldSet};
use crate::COMPARE_RECORDS;

const DEFAULT_CACHE_MB: usize = 512;
/// The common-world budget when `ECONSIM_WORLD_CACHE_MB` is unset (R13-S).
const DEFAULT_WORLD_CACHE_MB: usize = 1024;
/// The disk budget when `ECONSIM_CACHE_DISK_MB` is unset.
const DEFAULT_DISK_MB: u64 = 4096;
const MB: u64 = 1024 * 1024;
/// Prefix of every build folder under `ECONSIM_CACHE_DIR`. Only folders with this prefix are ever removed.
const BUILD_PREFIX: &str = "build-";
/// A temporary file this old was left behind by an interrupted write, and a prune removes it.
const STALE_TMP: Duration = Duration::from_secs(600);

/// One compare response: the serialised body, and whether it was served from a cache (memory or disk).
pub struct Served {
    pub body: Bytes,
    pub hit: bool,
}

/// One streamed compare response (R13-S): the NDJSON lines as they are produced (each ends in a newline), and whether the
/// whole body was already cached (`hit`), in which case the only line is the `done` line. `task` is the task that writes the lines
/// (None for a cached body); it ends when the stream ends, or when the client has gone.
pub struct Streamed {
    pub hit: bool,
    pub lines: mpsc::Receiver<Bytes>,
    // Only the tests await it (that the writer ends after a disconnect); the server lets the task run detached.
    #[cfg_attr(not(test), allow(dead_code))]
    pub task: Option<tokio::task::JoinHandle<()>>,
}

/// What a streamed request finds for its key: a stored body, or a computation in flight (its progress receiver).
enum Found {
    Memory(Bytes),
    Flight(watch::Receiver<Progress>),
}

/// The common-world cache: the stored worlds (least recently used first), and a slot for each world being built. The first
/// request for a key fills its slot; a request for the same key meanwhile waits on the slot and then takes the same world.
struct WorldState {
    lru: Lru<Arc<CommonWorld>>,
    building: HashMap<String, Arc<OnceLock<Arc<CommonWorld>>>>,
}

/// What a computation or a disk read produced: the body, the body without its record arrays (`rest`, R13-S; known only for a
/// computation, not for a disk read), and whether it came from the disk store.
#[derive(Clone)]
struct Done {
    body: Bytes,
    rest: Option<Bytes>,
    from_disk: bool,
}

/// The two bytes a computation produces: the compare body, and its rest (see `Comparison::rest_json`).
struct Computed {
    body: Bytes,
    rest: Bytes,
}

type Outcome = Result<Done, String>;

/// What a computation has produced so far (R13-S): its year lines, unfiltered and each ending in a newline, and once it has ended
/// its outcome. A streamed request replays `lines` from the start and follows the rest.
#[derive(Clone, Default)]
struct Progress {
    lines: Vec<Bytes>,
    end: Option<Outcome>,
}

/// Least-recently-used map from a key to a value, bounded by total bytes: each entry is charged its key's length plus the
/// `bytes` its caller names. Used for response bodies (the bytes are the body) and for common worlds (an estimate).
pub struct Lru<V: Clone> {
    max_bytes: usize,
    used: usize,
    clock: u64,
    /// Last-use stamp, the value, and the bytes charged (without the key).
    map: HashMap<Arc<str>, (u64, V, usize)>,
    /// Last-use stamp to key; the first entry is the least recently used.
    order: BTreeMap<u64, Arc<str>>,
}

impl<V: Clone> Lru<V> {
    pub fn new(max_bytes: usize) -> Self {
        Lru {
            max_bytes,
            used: 0,
            clock: 0,
            map: HashMap::new(),
            order: BTreeMap::new(),
        }
    }

    /// Look up a value and mark it most recently used.
    pub fn get(&mut self, key: &str) -> Option<V> {
        let (stamp, value) = self.map.get(key).map(|(s, v, _)| (*s, v.clone()))?;
        self.clock += 1;
        let k = self.order.remove(&stamp)?;
        self.order.insert(self.clock, k);
        if let Some(entry) = self.map.get_mut(key) {
            entry.0 = self.clock;
        }
        Some(value)
    }

    /// Store a value charged `bytes` (plus the key's length), evicting least recently used entries until the total fits. A single
    /// entry larger than the whole budget (which includes the disabled cache, budget 0) is not stored.
    pub fn insert(&mut self, key: String, value: V, bytes: usize) {
        let size = key.len() + bytes;
        if size > self.max_bytes {
            return;
        }
        let key: Arc<str> = Arc::from(key);
        if let Some((stamp, _, old)) = self.map.remove(&*key) {
            self.order.remove(&stamp);
            self.used -= key.len() + old;
        }
        while self.used + size > self.max_bytes {
            let Some((_, victim)) = self.order.pop_first() else { break };
            if let Some((_, _, old)) = self.map.remove(&*victim) {
                self.used -= victim.len() + old;
            }
        }
        self.clock += 1;
        self.used += size;
        self.order.insert(self.clock, Arc::clone(&key));
        self.map.insert(key, (self.clock, value, bytes));
    }
}

struct State {
    lru: Lru<Bytes>,
    /// Computations in progress, by key. Each holds a receiver of its progress; joiners clone it.
    inflight: HashMap<String, watch::Receiver<Progress>>,
}

pub struct CompareCache {
    state: Mutex<State>,
    permits: Arc<Semaphore>,
    cache_bytes: usize,
    concurrency: usize,
    /// The disk store, when `ECONSIM_CACHE_DIR` is set.
    disk: Option<Arc<DiskCache>>,
    /// The warmed common worlds (R13-S), by config and seed; None when `ECONSIM_WORLD_CACHE_MB` is 0.
    worlds: Option<Mutex<WorldState>>,
    world_cache_bytes: usize,
    /// Test only: the year-1 gate. A computation waits at it after it has produced year 1, so a test can join the computation
    /// while it is running.
    #[cfg(test)]
    year_gate: Option<Arc<std::sync::Barrier>>,
    /// Test only: a common world's build waits at this gate before it starts, so a test can send a second request for it meanwhile.
    #[cfg(test)]
    world_gate: Option<Arc<std::sync::Barrier>>,
    /// Test only: a computation fails (panics) when it reaches this year, after the year's line.
    #[cfg(test)]
    fail_at_year: Option<u32>,
    /// Test only: a streamed request that has missed memory and the flight stops here (see `serve_stream`).
    #[cfg(test)]
    stream_gate: Option<Arc<StreamGate>>,
    /// Number of computations started; the single-flight test reads it.
    #[cfg(test)]
    pub computed: AtomicU64,
    /// Number of common worlds built (world cache misses); the world-cache tests read it.
    #[cfg(test)]
    pub worlds_built: AtomicU64,
    /// Number of requests for a common world (every call of `common`); the single-flight test reads it.
    #[cfg(test)]
    pub world_arrivals: AtomicU64,
}

/// Test only: a streamed request that has missed the memory copy and the flight, and has read the disk, sets `reached` and waits on
/// `release` until the test has finished a computation for its key.
#[cfg(test)]
pub struct StreamGate {
    pub reached: std::sync::atomic::AtomicBool,
    pub release: std::sync::Barrier,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Canonical cache key for a validated scenario.
pub fn key_of(sc: &Scenario) -> Result<String, serde_json::Error> {
    serde_json::to_string(sc)
}

/// The key of a common world (R13-S): the seed, and the whole config. The config has no shocks, so a request that differs only in
/// its shocks or its months has the same key, and a change to any config key (households, start, warm-up, ...) is a different one.
pub fn world_key(cfg: &Config, seed: u64) -> String {
    format!("{seed}|{}", serde_json::to_string(cfg).expect("config serialises"))
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .max(1)
}

impl CompareCache {
    /// `cache_bytes` bounds the stored responses (0 stores nothing); `concurrency` is the number of computations
    /// allowed at once (at least 1). No common-world cache (see [`CompareCache::with_worlds`]).
    pub fn new(cache_bytes: usize, concurrency: usize) -> Self {
        let concurrency = concurrency.max(1);
        CompareCache {
            state: Mutex::new(State {
                lru: Lru::new(cache_bytes),
                inflight: HashMap::new(),
            }),
            permits: Arc::new(Semaphore::new(concurrency)),
            cache_bytes,
            concurrency,
            disk: None,
            worlds: None,
            world_cache_bytes: 0,
            #[cfg(test)]
            year_gate: None,
            #[cfg(test)]
            world_gate: None,
            #[cfg(test)]
            fail_at_year: None,
            #[cfg(test)]
            stream_gate: None,
            #[cfg(test)]
            computed: AtomicU64::new(0),
            #[cfg(test)]
            worlds_built: AtomicU64::new(0),
            #[cfg(test)]
            world_arrivals: AtomicU64::new(0),
        }
    }

    /// Attaches the disk store: computed bodies are written to it, and a memory miss reads it first.
    pub fn with_disk(mut self, disk: DiskCache) -> Self {
        self.disk = Some(Arc::new(disk));
        self
    }

    /// Attaches the common-world cache with a budget of `bytes` (0: none).
    pub fn with_worlds(mut self, bytes: usize) -> Self {
        self.world_cache_bytes = bytes;
        self.worlds = (bytes > 0).then(|| {
            Mutex::new(WorldState {
                lru: Lru::new(bytes),
                building: HashMap::new(),
            })
        });
        self
    }

    /// Test only: a common world's build waits at `gate` (a barrier of two parties) before it starts.
    #[cfg(test)]
    pub fn with_world_gate(mut self, gate: Arc<std::sync::Barrier>) -> Self {
        self.world_gate = Some(gate);
        self
    }

    /// Test only: a computation panics after the line of year `year`.
    #[cfg(test)]
    pub fn with_failure_at_year(mut self, year: u32) -> Self {
        self.fail_at_year = Some(year);
        self
    }

    /// Test only: a streamed request stops at `gate` after it has read the disk (see `serve_stream`).
    #[cfg(test)]
    pub fn with_stream_gate(mut self, gate: Arc<StreamGate>) -> Self {
        self.stream_gate = Some(gate);
        self
    }

    /// Test only: the bytes charged to the stored worlds.
    #[cfg(test)]
    pub fn worlds_used(&self) -> usize {
        self.worlds.as_ref().map_or(0, |w| lock(w).lru.used)
    }

    /// Test only: the computations wait at year 1 on `gate` (a barrier of two parties) until the test joins them.
    #[cfg(test)]
    pub fn with_year_gate(mut self, gate: Arc<std::sync::Barrier>) -> Self {
        self.year_gate = Some(gate);
        self
    }

    /// The disk store's folder and budget, for the startup line; None without a disk store.
    pub fn disk_summary(&self) -> Option<String> {
        self.disk
            .as_ref()
            .map(|d| format!("{} (budget {} MB)", d.dir().display(), d.max_bytes / MB))
    }

    /// Whether anything is stored, in memory or on disk. A pre-warm pass does nothing when nothing is stored.
    pub fn stores_results(&self) -> bool {
        self.cache_bytes > 0 || self.disk.is_some()
    }

    /// Settings from `ECONSIM_CACHE_MB` (default 512, 0 disables stored responses),
    /// `ECONSIM_COMPARE_CONCURRENCY` (default max(1, available cores / 2)), `ECONSIM_WORLD_CACHE_MB` (default 1024, 0 disables the
    /// common-world cache), plus the disk store from `ECONSIM_CACHE_DIR` and `ECONSIM_CACHE_DISK_MB` (see [`disk_store_from_env`]).
    pub fn from_env() -> Self {
        let cache_mb: usize = env::var("ECONSIM_CACHE_MB")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_CACHE_MB);
        let concurrency: usize = env::var("ECONSIM_COMPARE_CONCURRENCY")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(default_concurrency);
        let world_mb: usize = env::var("ECONSIM_WORLD_CACHE_MB")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_WORLD_CACHE_MB);
        let cache = Self::new(cache_mb.saturating_mul(1024 * 1024), concurrency).with_worlds(world_mb.saturating_mul(1024 * 1024));
        match disk_store_from_env() {
            Some(disk) => cache.with_disk(disk),
            None => cache,
        }
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache_bytes
    }

    pub fn world_cache_bytes(&self) -> usize {
        self.world_cache_bytes
    }

    pub fn concurrency(&self) -> usize {
        self.concurrency
    }

    /// Serve the compare for `key` (the canonical form of `sc`): from the cache if stored, otherwise by joining an
    /// identical computation already in flight, otherwise by starting one. The computation runs in a detached task,
    /// so it finishes and is cached even if the HTTP request that started it is dropped.
    pub async fn serve(self: Arc<Self>, key: String, sc: Scenario) -> Result<Served, String> {
        let rx = {
            let mut guard = lock(&self.state);
            let st = &mut *guard;
            if let Some(body) = st.lru.get(&key) {
                return Ok(Served { body, hit: true });
            }
            match st.inflight.get(&key) {
                Some(rx) => rx.clone(),
                None => self.start(st, key, sc),
            }
        };
        let done = finished(rx).await?;
        Ok(Served {
            body: done.body,
            hit: done.from_disk,
        })
    }

    /// Serve the compare for `key` as NDJSON lines (R13-S): the body from the memory cache or the disk store answers at once with
    /// one `done` line; otherwise the request joins a computation in flight (replaying the lines so far, then following) or starts
    /// one. Every line goes through `keep` (the `fields` filter) on its way out. `sc` must have one seed (the server checks).
    ///
    /// The memory copy and the computation in flight are read under one lock, as `serve` reads them, so a request cannot find neither
    /// (a computation stores its copy and leaves the flight under the same lock). The disk copy is read outside the lock. A computation
    /// may finish during that read, so the memory copy and the flight are read again, under the lock, before a new computation starts:
    /// the request then takes the stored copy, not a duplicate computation.
    pub async fn serve_stream(self: Arc<Self>, key: String, sc: Scenario, keep: Option<FieldSet>) -> Result<Streamed, String> {
        if let Some(found) = self.peek(&key) {
            return Ok(found_stream(found, keep).await);
        }
        if let Some(body) = self.disk_get(&key).await {
            lock(&self.state).lru.insert(key.clone(), body.clone(), body.len());
            return Ok(Streamed {
                hit: true,
                lines: ready(end_line(Ok(Done { body, rest: None, from_disk: true }), keep, false).await),
                task: None,
            });
        }
        #[cfg(test)]
        if let Some(gate) = &self.stream_gate {
            gate.reached.store(true, Ordering::SeqCst);
            gate.release.wait();
        }
        Ok(found_stream(self.peek_or_start(key, sc), keep).await)
    }

    /// The memory copy of `key`, or else the computation in flight for it, read under the lock.
    fn peek(&self, key: &str) -> Option<Found> {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        if let Some(body) = st.lru.get(key) {
            return Some(Found::Memory(body));
        }
        st.inflight.get(key).map(|rx| Found::Flight(rx.clone()))
    }

    /// As `peek`, but a key that is neither stored nor in flight starts its computation with `sc`, under the same lock.
    fn peek_or_start(self: &Arc<Self>, key: String, sc: Scenario) -> Found {
        let mut guard = lock(&self.state);
        let st = &mut *guard;
        if let Some(body) = st.lru.get(&key) {
            return Found::Memory(body);
        }
        match st.inflight.get(&key) {
            Some(rx) => Found::Flight(rx.clone()),
            None => Found::Flight(self.start(st, key, sc)),
        }
    }

    /// Registers a new computation for `key` as in flight and starts it in a detached task. Returns its progress receiver.
    fn start(self: &Arc<Self>, st: &mut State, key: String, sc: Scenario) -> watch::Receiver<Progress> {
        let (tx, rx) = watch::channel(Progress::default());
        st.inflight.insert(key.clone(), rx.clone());
        tokio::spawn(Arc::clone(self).run(key, sc, tx));
        rx
    }

    async fn run(self: Arc<Self>, key: String, sc: Scenario, tx: watch::Sender<Progress>) {
        let outcome = match self.disk_get(&key).await {
            Some(body) => Ok(Done { body, rest: None, from_disk: true }),
            None => match Arc::clone(&self).compute(sc, tx.clone()).await {
                Ok(Computed { body, rest }) => {
                    // Written before the waiters are told, so a response that has returned is also on disk.
                    self.disk_put(&key, &body).await;
                    Ok(Done { body, rest: Some(rest), from_disk: false })
                }
                Err(e) => Err(e),
            },
        };
        {
            let mut guard = lock(&self.state);
            let st = &mut *guard;
            st.inflight.remove(&key);
            // Errors are never stored: only a successful body goes into the LRU.
            if let Ok(done) = &outcome {
                st.lru.insert(key, done.body.clone(), done.body.len());
            }
        }
        tx.send_modify(move |p| p.end = Some(outcome));
    }

    /// Runs the comparison (waiting for a permit first). Each year is pushed to `tx` as its line as it completes.
    async fn compute(self: Arc<Self>, sc: Scenario, tx: watch::Sender<Progress>) -> Result<Computed, String> {
        let permits = Arc::clone(&self.permits);
        let _permit: OwnedSemaphorePermit = permits.acquire_owned().await.map_err(|e| e.to_string())?;
        #[cfg(test)]
        self.computed.fetch_add(1, Ordering::SeqCst);
        tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            let gate = self.year_gate.clone();
            #[cfg(test)]
            let fail_at = self.fail_at_year;
            let common = |seed: u64| self.common(&sc.config, seed);
            let mut on_year = |year: &Year| {
                tx.send_modify(|p| p.lines.push(year_line(year)));
                #[cfg(test)]
                if year.year == 1 {
                    if let Some(gate) = &gate {
                        gate.wait();
                    }
                }
                #[cfg(test)]
                if Some(year.year) == fail_at {
                    panic!("injected failure in year {}", year.year);
                }
            };
            let comparison = sim::compare_with(&sc, &common, &mut on_year);
            // Serialised exactly as axum's Json would, so cached and fresh bodies are the same bytes.
            let body = serde_json::to_vec(&comparison).map_err(|e| format!("cannot serialise comparison: {e}"))?;
            Ok(Computed { body: Bytes::from(body), rest: Bytes::from(comparison.rest_json()) })
        })
        .await
        .map_err(|e| format!("worker task failed: {e}"))?
    }

    /// The common world of one seed for `cfg` (R13-S): from the world cache when it holds one, else built and kept. The kept copy is a
    /// clone, which has no spare vector capacity, so it takes about half the memory of the world the arms run on. Runs on the
    /// blocking pool.
    ///
    /// Single-flight: requests for one key share one build. The first request to find no stored world and no slot for the key creates
    /// the slot and builds the world; the others wait on the slot and then clone the same world. The stored copy and the slot's removal
    /// happen under one lock, so a request never misses both.
    pub fn common(&self, cfg: &Config, seed: u64) -> CommonWorld {
        let Some(worlds) = &self.worlds else { return sim::common_world(cfg, seed) };
        #[cfg(test)]
        self.world_arrivals.fetch_add(1, Ordering::SeqCst);
        let key = world_key(cfg, seed);
        let slot = {
            let mut guard = lock(worlds);
            let st = &mut *guard;
            if let Some(hit) = st.lru.get(&key) {
                return (hit.0.clone(), hit.1.clone());
            }
            Arc::clone(st.building.entry(key.clone()).or_insert_with(|| Arc::new(OnceLock::new())))
        };
        let mut built = None;
        let shared = slot.get_or_init(|| {
            #[cfg(test)]
            if let Some(gate) = &self.world_gate {
                gate.wait();
            }
            #[cfg(test)]
            self.worlds_built.fetch_add(1, Ordering::SeqCst);
            let world = sim::common_world(cfg, seed);
            let stored = Arc::new((world.0.clone(), world.1.clone()));
            let bytes = stored.0.approx_clone_bytes();
            {
                let mut guard = lock(worlds);
                let st = &mut *guard;
                st.building.remove(&key);
                st.lru.insert(key.clone(), Arc::clone(&stored), bytes);
            }
            built = Some(world);
            stored
        });
        match built {
            Some(world) => world,
            None => (shared.0.clone(), shared.1.clone()),
        }
    }

    /// The stored body for `key` on disk, if any. Disk I/O runs on the blocking pool.
    async fn disk_get(&self, key: &str) -> Option<Bytes> {
        let disk = Arc::clone(self.disk.as_ref()?);
        let key = key.to_owned();
        tokio::task::spawn_blocking(move || disk.get(&key)).await.ok().flatten()
    }

    /// Writes a computed body to disk. A failure is logged; the response is served either way.
    async fn disk_put(&self, key: &str, body: &Bytes) {
        let Some(disk) = self.disk.clone() else { return };
        let key = key.to_owned();
        let body = body.clone();
        match tokio::task::spawn_blocking(move || disk.put(&key, &body)).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => println!("cache: cannot store a result on disk: {e}"),
            Err(e) => println!("cache: disk write stopped: {e}"),
        }
    }
}

/// Waits until the computation behind `rx` has ended, and returns its body (or its error).
async fn finished(mut rx: watch::Receiver<Progress>) -> Result<Done, String> {
    let end = match rx.wait_for(|p| p.end.is_some()).await {
        Ok(p) => p.end.clone(),
        Err(_) => None,
    };
    match end {
        Some(Ok(done)) => Ok(done),
        Some(Err(e)) => Err(e),
        None => Err("compare worker stopped".to_string()),
    }
}

// ------------------------------------------------------------------------------------------------
// NDJSON lines (R13-S)
// ------------------------------------------------------------------------------------------------

/// The line of one year: `{"year","from","to","baseline","shocked"}` and a newline. Its records are the full comparison's records
/// for those months.
fn year_line(y: &Year) -> Bytes {
    #[derive(Serialize)]
    struct Line<'a> {
        year: u32,
        from: u32,
        to: u32,
        baseline: &'a [Metrics],
        shocked: &'a [Metrics],
    }
    let mut bytes = serde_json::to_vec(&Line {
        year: y.year,
        from: y.from,
        to: y.to,
        baseline: &y.baseline,
        shocked: &y.shocked,
    })
    .expect("a year line serialises");
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// An `error` line: `{"error": message}` and a newline. A stream has already sent 200, so a failure is reported in-band.
pub fn error_line(message: &str) -> Bytes {
    let mut bytes = serde_json::to_vec(&json!({ "error": message })).expect("an error line serialises");
    bytes.push(b'\n');
    Bytes::from(bytes)
}

/// The last line of a stream (R13-S). After year lines, a computed body is `{"done":true,"rest":<rest>}`: the body without its record
/// arrays, which the year lines carry. Otherwise (a cached body, which sent no year lines) it is `{"done":true,"body":<body>}`, the
/// body filtered by `keep` as the non-stream response is. An error outcome gives an `error` line.
async fn end_line(end: Outcome, keep: Option<FieldSet>, after_lines: bool) -> Bytes {
    tokio::task::spawn_blocking(move || match end {
        Ok(Done { rest: Some(rest), .. }) if after_lines => {
            let mut line = Vec::with_capacity(rest.len() + 32);
            line.extend_from_slice(br#"{"done":true,"rest":"#);
            line.extend_from_slice(&rest);
            line.extend_from_slice(b"}\n");
            Bytes::from(line)
        }
        Ok(done) => match done_bytes(&done.body, keep.as_ref()) {
            Ok(line) => Bytes::from(line),
            Err(e) => error_line(&format!("cannot filter response: {e}")),
        },
        Err(e) => error_line(&e),
    })
    .await
    .unwrap_or_else(|e| error_line(&format!("worker task failed: {e}")))
}

/// The bytes of a `done` line for `body`, filtered by `keep` when there is a filter.
fn done_bytes(body: &[u8], keep: Option<&FieldSet>) -> serde_json::Result<Vec<u8>> {
    let filtered = match keep {
        Some(k) => Some(fields::filter_bytes(body, COMPARE_RECORDS, k)?),
        None => None,
    };
    let body = filtered.as_deref().unwrap_or(body);
    let mut line = Vec::with_capacity(body.len() + 32);
    line.extend_from_slice(br#"{"done":true,"body":"#);
    line.extend_from_slice(body);
    line.extend_from_slice(b"}\n");
    Ok(line)
}

/// A year line through the `fields` filter (the records of `baseline` and `shocked`), in the blocking pool.
async fn filter_year(line: Bytes, keep: FieldSet) -> Bytes {
    tokio::task::spawn_blocking(move || {
        let text = &line[..line.len() - 1]; // without its newline
        match fields::filter_bytes(text, COMPARE_RECORDS, &keep) {
            Ok(mut v) => {
                v.push(b'\n');
                Bytes::from(v)
            }
            Err(e) => error_line(&format!("cannot filter response: {e}")),
        }
    })
    .await
    .unwrap_or_else(|e| error_line(&format!("worker task failed: {e}")))
}

/// A channel that already holds one line, for a body that was cached.
fn ready(line: Bytes) -> mpsc::Receiver<Bytes> {
    let (tx, rx) = mpsc::channel(1);
    let _ = tx.try_send(line);
    rx
}

/// Follows a computation's progress as a line channel: the lines produced so far, then each new one, then the end. The task that
/// writes the lines is returned with it.
fn follow(rx: watch::Receiver<Progress>, keep: Option<FieldSet>) -> (mpsc::Receiver<Bytes>, tokio::task::JoinHandle<()>) {
    let (tx, out) = mpsc::channel(16);
    (out, tokio::spawn(forward(rx, keep, tx)))
}

/// The response for what a streamed request found: a stored body (one `done` line), or a computation (its lines, then the end).
async fn found_stream(found: Found, keep: Option<FieldSet>) -> Streamed {
    match found {
        Found::Memory(body) => Streamed {
            hit: true,
            lines: ready(end_line(Ok(Done { body, rest: None, from_disk: false }), keep, false).await),
            task: None,
        },
        Found::Flight(rx) => {
            let (lines, task) = follow(rx, keep);
            Streamed { hit: false, lines, task: Some(task) }
        }
    }
}

async fn forward(mut rx: watch::Receiver<Progress>, keep: Option<FieldSet>, out: mpsc::Sender<Bytes>) {
    let mut sent = 0;
    loop {
        let (fresh, end) = {
            let p = rx.borrow_and_update();
            (p.lines[sent..].to_vec(), p.end.clone())
        };
        for line in fresh {
            sent += 1;
            let line = match &keep {
                Some(k) => filter_year(line, k.clone()).await,
                None => line,
            };
            if out.send(line).await.is_err() {
                return; // the client has gone
            }
        }
        if let Some(end) = end {
            let _ = out.send(end_line(end, keep, sent > 0).await).await;
            return;
        }
        if rx.changed().await.is_err() {
            let _ = out.send(error_line("compare worker stopped")).await;
            return;
        }
    }
}

// ------------------------------------------------------------------------------------------------
// disk store
// ------------------------------------------------------------------------------------------------

/// The disk store the environment asks for: `ECONSIM_CACHE_DIR` (unset or empty: none) with a budget of
/// `ECONSIM_CACHE_DISK_MB` megabytes (default 4096; 0: none). Startup removes the folders of other builds and prunes
/// to the budget. A failure is logged and leaves the server without a disk store, as it was before the store existed.
fn disk_store_from_env() -> Option<DiskCache> {
    let root = env::var("ECONSIM_CACHE_DIR").ok().filter(|d| !d.is_empty())?;
    let budget_mb = parse_disk_mb(env::var("ECONSIM_CACHE_DISK_MB").ok().as_deref());
    if budget_mb == 0 {
        return None;
    }
    let root = PathBuf::from(root);
    let opened = build_id().and_then(|build| {
        let disk = DiskCache::open(&root, &build, budget_mb.saturating_mul(MB))?;
        match sweep_other_builds(&root, &build) {
            Ok(0) => {}
            Ok(n) => println!("cache: removed {n} folder(s) of other builds from {}", root.display()),
            Err(e) => println!("cache: cannot remove other builds from {}: {e}", root.display()),
        }
        disk.prune()?;
        Ok(disk)
    });
    match opened {
        Ok(disk) => Some(disk),
        Err(e) => {
            println!("cache: disk store off, cannot use {}: {e}", root.display());
            None
        }
    }
}

/// `ECONSIM_CACHE_DISK_MB`: whole megabytes. Unset or not a number gives the default; 0 turns the disk store off.
fn parse_disk_mb(raw: Option<&str>) -> u64 {
    raw.and_then(|s| s.trim().parse().ok()).unwrap_or(DEFAULT_DISK_MB)
}

/// The folder name of this binary: the crate version and the first 16 hex digits of the SHA-256 of the running
/// executable. Any rebuild changes the executable, so a new model, a new serialisation or an uncommitted change gets a
/// new folder and never reads another binary's bodies.
pub fn build_id() -> io::Result<String> {
    let mut file = File::open(env::current_exe()?)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hex(&hasher.finalize());
    Ok(format!("{BUILD_PREFIX}{}-{}", env!("CARGO_PKG_VERSION"), &digest[..16]))
}

/// Removes the folders of builds other than `keep` under `root`. Only folders named as builds are removed, so anything
/// else in `root` is left alone. Returns how many were removed.
pub fn sweep_other_builds(root: &Path, keep: &str) -> io::Result<usize> {
    let mut removed = 0;
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(BUILD_PREFIX) && name != keep && entry.file_type()?.is_dir() {
            if fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

/// Bodies on disk: one file per stored body, `<root>/<build>/<sha256 of the key>.bin`. A body is written to a
/// temporary file, synced and renamed into place, so a reader never sees part of one. A hit touches the file's
/// mtime, which is its last use; a store over the budget removes the least recently used files first.
pub struct DiskCache {
    dir: PathBuf,
    max_bytes: u64,
    /// Makes the temporary names unique within this process.
    seq: AtomicU64,
}

impl DiskCache {
    /// Opens the folder of `build` under `root`, creating it if needed. Other builds' folders are left alone; see
    /// [`sweep_other_builds`].
    pub fn open(root: &Path, build: &str, max_bytes: u64) -> io::Result<Self> {
        let dir = root.join(build);
        fs::create_dir_all(&dir)?;
        Ok(DiskCache {
            dir,
            max_bytes,
            seq: AtomicU64::new(0),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The stored body for `key`, if there is one. A file that does not end like a JSON object is removed and is a miss.
    pub fn get(&self, key: &str) -> Option<Bytes> {
        let path = self.dir.join(entry_name(key));
        let body = fs::read(&path).ok()?;
        if !is_complete(&body) {
            let _ = fs::remove_file(&path);
            return None;
        }
        if let Ok(file) = File::open(&path) {
            let _ = file.set_modified(SystemTime::now());
        }
        Some(Bytes::from(body))
    }

    /// Stores `body` under `key`, then prunes the folder to its budget.
    pub fn put(&self, key: &str, body: &[u8]) -> io::Result<()> {
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let name = entry_name(key);
        let tmp = self.dir.join(format!("{name}.{}-{n}.tmp", process::id()));
        let written = write_synced(&tmp, body).and_then(|()| fs::rename(&tmp, self.dir.join(&name)));
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written?;
        self.prune()
    }

    /// Removes the least recently used bodies until the folder is within its budget, and any temporary file that an
    /// interrupted write left behind.
    pub fn prune(&self) -> io::Result<()> {
        let now = SystemTime::now();
        let mut bodies = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.dir)? {
            let entry = entry?;
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            match path.extension().and_then(|e| e.to_str()) {
                Some("tmp") => {
                    if now.duration_since(modified).is_ok_and(|age| age > STALE_TMP) {
                        let _ = fs::remove_file(&path);
                    }
                }
                Some("bin") => {
                    total += meta.len();
                    bodies.push((modified, meta.len(), path));
                }
                _ => {}
            }
        }
        if total <= self.max_bytes {
            return Ok(());
        }
        bodies.sort_by_key(|(modified, _, _)| *modified);
        for (_, len, path) in bodies {
            if total <= self.max_bytes {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
        Ok(())
    }
}

fn write_synced(path: &Path, body: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(body)?;
    file.sync_data()
}

/// A stored body is a JSON object, so its last non-blank byte is `}`. Anything else is an empty or torn file.
fn is_complete(body: &[u8]) -> bool {
    body.iter().rev().find(|b| !b.is_ascii_whitespace()) == Some(&b'}')
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The file name of a key's body: the SHA-256 of the key, so any key is a safe name.
fn entry_name(key: &str) -> String {
    format!("{}.bin", hex(&Sha256::digest(key.as_bytes())))
}

#[cfg(test)]
pub(crate) struct TempRoot(pub PathBuf);

#[cfg(test)]
impl TempRoot {
    /// A fresh folder under the system temp directory, removed when the guard drops.
    pub(crate) fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = env::temp_dir().join(format!("econsim-cache-test-{}-{tag}-{n}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        TempRoot(dir)
    }
}

#[cfg(test)]
impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Small enough to run in well under a second: 1,000 households, 3 months, 2 seeds, one shock.
    const SMALL: &str = r#"{"config":{"n_households":1000},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":3,"seeds":[1,2]}"#;

    fn body(n: usize) -> Bytes {
        Bytes::from(vec![b'x'; n])
    }

    /// A body of exactly 100 bytes that is a complete JSON object.
    fn object100(fill: u8) -> Vec<u8> {
        let mut v = vec![b'{'];
        v.extend(std::iter::repeat(fill).take(98));
        v.push(b'}');
        v
    }

    fn set_mtime(path: &Path, when: SystemTime) {
        fs::OpenOptions::new().write(true).open(path).unwrap().set_modified(when).unwrap();
    }

    fn small() -> Scenario {
        serde_json::from_str(SMALL).unwrap()
    }

    #[test]
    fn key_ignores_key_order_whitespace_and_number_spelling() {
        let a: Scenario = serde_json::from_str(
            r#"{"config":{"n_households":1000},"shocks":[{"at_month":1,"shock":{"type":"OilPrice","pct":20,"half_life_months":6}}],"months":3,"seeds":[1,2]}"#,
        )
        .unwrap();
        let b: Scenario = serde_json::from_str(
            r#"{
                "months" : 3,
                "shocks" : [ { "shock" : { "half_life_months" : 6.0, "pct" : 2e1, "type" : "OilPrice" }, "at_month" : 1 } ],
                "config" : { "n_households" : 1000 },
                "seeds" : [ 1 , 2 ]
            }"#,
        )
        .unwrap();
        assert_eq!(key_of(&a).unwrap(), key_of(&b).unwrap());
    }

    #[test]
    fn key_separates_requests_that_compute_differently() {
        let base: Scenario =
            serde_json::from_str(r#"{"config":{"n_households":1000},"months":3,"seeds":[1,2]}"#).unwrap();
        let seeds_reordered: Scenario =
            serde_json::from_str(r#"{"config":{"n_households":1000},"months":3,"seeds":[2,1]}"#).unwrap();
        let more_months: Scenario =
            serde_json::from_str(r#"{"config":{"n_households":1000},"months":4,"seeds":[1,2]}"#).unwrap();
        let k = key_of(&base).unwrap();
        assert_ne!(k, key_of(&seeds_reordered).unwrap());
        assert_ne!(k, key_of(&more_months).unwrap());
    }

    #[test]
    fn lru_evicts_least_recently_used_by_bytes() {
        let mut lru = Lru::new(100);
        lru.insert("a".into(), body(40), 40); // 41 bytes with the key
        lru.insert("b".into(), body(40), 40); // 82 total
        assert!(lru.get("a").is_some()); // a is now the most recently used
        lru.insert("c".into(), body(40), 40); // 123 > 100: evicts b
        assert!(lru.get("b").is_none());
        assert!(lru.get("a").is_some());
        assert!(lru.get("c").is_some());
        assert_eq!(lru.used, 82);
        assert!(lru.used <= 100);
    }

    #[test]
    fn lru_counts_each_key_once_and_skips_entries_over_budget() {
        let mut lru = Lru::new(100);
        lru.insert("a".into(), body(40), 40);
        lru.insert("a".into(), body(40), 40); // replacing the same key does not double count
        assert_eq!(lru.used, 41);
        lru.insert("big".into(), body(200), 200); // larger than the whole budget: not stored, evicts nothing
        assert!(lru.get("a").is_some());
        assert_eq!(lru.used, 41);
    }

    #[test]
    fn zero_budget_stores_nothing() {
        let mut lru = Lru::new(0);
        lru.insert("a".into(), body(1), 1);
        assert!(lru.get("a").is_none());
        assert_eq!(lru.used, 0);
    }

    #[test]
    fn disk_bodies_round_trip_and_a_different_build_id_misses() {
        let root = TempRoot::new("builds");
        let a = DiskCache::open(&root.0, "build-a", MB).unwrap();
        a.put("k", b"{\"x\":1}").unwrap();
        assert_eq!(a.get("k").unwrap(), Bytes::from_static(b"{\"x\":1}"));
        assert!(a.get("missing").is_none());
        // Another build has its own folder: it misses the body that build a stored, and keeps its own.
        let b = DiskCache::open(&root.0, "build-b", MB).unwrap();
        assert!(b.get("k").is_none(), "a different build id must miss");
        b.put("k", b"{\"x\":2}").unwrap();
        assert_eq!(b.get("k").unwrap(), Bytes::from_static(b"{\"x\":2}"));
        assert_eq!(a.get("k").unwrap(), Bytes::from_static(b"{\"x\":1}"));
    }

    #[test]
    fn disk_prune_removes_the_least_recently_used_bodies_first() {
        let root = TempRoot::new("prune");
        let mut d = DiskCache::open(&root.0, "build-p", 1000).unwrap();
        d.put("a", &object100(b'a')).unwrap();
        d.put("b", &object100(b'b')).unwrap();
        d.put("c", &object100(b'c')).unwrap();
        // Fix the order: b is oldest, then c, then a; the hit on a makes it the newest.
        let now = SystemTime::now();
        set_mtime(&d.dir.join(entry_name("b")), now - Duration::from_secs(7200));
        set_mtime(&d.dir.join(entry_name("c")), now - Duration::from_secs(3600));
        set_mtime(&d.dir.join(entry_name("a")), now - Duration::from_secs(10800));
        assert!(d.get("a").is_some());
        // Shrink the budget to 250 bytes, then store a fourth body: 400 bytes, so b and then c go.
        d.max_bytes = 250;
        d.put("d", &object100(b'd')).unwrap();
        assert!(d.get("b").is_none());
        assert!(d.get("c").is_none());
        assert!(d.get("a").is_some());
        assert!(d.get("d").is_some());
    }

    #[test]
    fn disk_drops_a_torn_body_and_reports_a_miss() {
        let root = TempRoot::new("torn");
        let d = DiskCache::open(&root.0, "build-t", MB).unwrap();
        let path = d.dir.join(entry_name("k"));
        fs::write(&path, b"{\"baseline\":[").unwrap();
        assert!(d.get("k").is_none());
        assert!(!path.exists());
    }

    #[test]
    fn sweep_removes_only_other_build_folders() {
        let root = TempRoot::new("sweep");
        fs::create_dir_all(root.0.join("build-old")).unwrap();
        fs::create_dir_all(root.0.join("build-cur")).unwrap();
        fs::create_dir_all(root.0.join("not-a-build")).unwrap();
        fs::write(root.0.join("notes.txt"), b"keep").unwrap();
        assert_eq!(sweep_other_builds(&root.0, "build-cur").unwrap(), 1);
        assert!(!root.0.join("build-old").exists());
        assert!(root.0.join("build-cur").is_dir());
        assert!(root.0.join("not-a-build").is_dir());
        assert!(root.0.join("notes.txt").is_file());
    }

    #[test]
    fn build_id_names_this_binary_and_is_stable() {
        let id = build_id().unwrap();
        let prefix = format!("{BUILD_PREFIX}{}-", env!("CARGO_PKG_VERSION"));
        assert!(id.starts_with(&prefix), "{id}");
        assert_eq!(id.len(), prefix.len() + 16);
        assert_eq!(build_id().unwrap(), id);
    }

    #[test]
    fn disk_budget_parses_whole_megabytes_with_a_default() {
        assert_eq!(parse_disk_mb(None), 4096);
        assert_eq!(parse_disk_mb(Some("abc")), 4096);
        assert_eq!(parse_disk_mb(Some("0")), 0);
        assert_eq!(parse_disk_mb(Some(" 512 ")), 512);
    }

    #[test]
    fn a_disk_store_counts_as_storing_results() {
        let root = TempRoot::new("stores");
        assert!(!CompareCache::new(0, 1).stores_results());
        let disk = DiskCache::open(&root.0, "build-s", MB).unwrap();
        assert!(CompareCache::new(0, 1).with_disk(disk).stores_results());
    }

    /// A restart is a new cache with empty memory and the same disk folder. The bytes are the same with and without the
    /// disk store, and another build's folder misses.
    #[tokio::test]
    async fn a_restarted_cache_serves_the_disk_copy_with_the_same_bytes() {
        let root = TempRoot::new("serve");
        let key = key_of(&small()).unwrap();
        let open = |build: &str| DiskCache::open(&root.0, build, 64 * MB).unwrap();

        let first = Arc::new(CompareCache::new(64 << 20, 2).with_disk(open("build-s")));
        let fresh = Arc::clone(&first).serve(key.clone(), small()).await.unwrap();
        assert!(!fresh.hit);
        assert_eq!(first.computed.load(Ordering::SeqCst), 1);

        let restarted = Arc::new(CompareCache::new(64 << 20, 2).with_disk(open("build-s")));
        let from_disk = Arc::clone(&restarted).serve(key.clone(), small()).await.unwrap();
        assert!(from_disk.hit);
        assert_eq!(restarted.computed.load(Ordering::SeqCst), 0);
        assert_eq!(from_disk.body, fresh.body);

        let other_build = Arc::new(CompareCache::new(0, 2).with_disk(open("build-t")));
        let recomputed = Arc::clone(&other_build).serve(key, small()).await.unwrap();
        assert!(!recomputed.hit);
        assert_eq!(other_build.computed.load(Ordering::SeqCst), 1);
        assert_eq!(recomputed.body, fresh.body);
    }

    /// R13-S: a streamed request for a body whose computation is running joins it. It replays the year lines produced so far, follows
    /// the rest, and ends with the same `done` line; the computation runs once. The year-1 gate holds the computation after year 1
    /// until both streams are open, so the join is certain.
    #[tokio::test(flavor = "multi_thread")]
    async fn joined_stream_replays_the_lines_so_far() {
        let gate = Arc::new(std::sync::Barrier::new(2));
        let st = Arc::new(CompareCache::new(64 << 20, 2).with_year_gate(Arc::clone(&gate)));
        let sc: Scenario = serde_json::from_str(r#"{"config":{"n_households":1000},"months":14,"seeds":[1]}"#).unwrap();
        let key = key_of(&sc).unwrap();

        let mut first = Arc::clone(&st).serve_stream(key.clone(), sc.clone(), None).await.unwrap();
        assert!(!first.hit);
        let year1 = first.lines.recv().await.unwrap();
        assert!(year1.starts_with(br#"{"year":1,"from":0,"to":12,"#), "{}", String::from_utf8_lossy(&year1));

        let mut second = Arc::clone(&st).serve_stream(key, sc, None).await.unwrap();
        assert!(!second.hit);
        gate.wait(); // release the computation: year 2 and the end follow

        let mut a = vec![year1];
        while let Some(line) = first.lines.recv().await {
            a.push(line);
        }
        let mut b = Vec::new();
        while let Some(line) = second.lines.recv().await {
            b.push(line);
        }
        assert_eq!(a.len(), 3, "two years and the done line");
        assert_eq!(a, b);
        assert!(a[1].starts_with(br#"{"year":2,"from":12,"to":14,"#));
        assert!(a[2].starts_with(br#"{"done":true,"rest":{"seeds":[1]}}"#), "{}", String::from_utf8_lossy(&a[2]));
        assert_eq!(st.computed.load(Ordering::SeqCst), 1);
    }

    /// The small scenario of the streaming tests: one seed, 14 months (two years).
    fn two_year_scenario() -> Scenario {
        serde_json::from_str(r#"{"config":{"n_households":1000},"months":14,"seeds":[1]}"#).unwrap()
    }

    /// R13-S: a streamed request that has missed the memory copy and the flight, and has read the disk, is not sent to a new
    /// computation when another request has computed its body meanwhile. It takes the stored copy, as one hit, and nothing is computed twice.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_stream_that_missed_memory_takes_a_copy_stored_meanwhile() {
        let gate = Arc::new(StreamGate {
            reached: std::sync::atomic::AtomicBool::new(false),
            release: std::sync::Barrier::new(2),
        });
        let st = Arc::new(CompareCache::new(64 << 20, 2).with_stream_gate(Arc::clone(&gate)));
        let sc = two_year_scenario();
        let key = key_of(&sc).unwrap();
        let streamer = tokio::spawn(Arc::clone(&st).serve_stream(key.clone(), sc.clone(), None));
        while !gate.reached.load(Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
        // Meanwhile the body is computed and stored by another request.
        Arc::clone(&st).serve(key, sc).await.unwrap();
        gate.release.wait();
        let mut streamed = streamer.await.unwrap().unwrap();
        assert!(streamed.hit, "a body stored meanwhile answers as a hit");
        let line = streamed.lines.recv().await.unwrap();
        assert!(line.starts_with(br#"{"done":true,"body":"#));
        assert!(streamed.lines.recv().await.is_none());
        assert_eq!(st.computed.load(Ordering::SeqCst), 1, "the body was computed once");
    }

    /// R13-S: a client that goes away ends the task that writes its lines, at the next line it cannot send. The computation carries on and
    /// is cached as usual.
    #[tokio::test(flavor = "multi_thread")]
    async fn the_forwarder_exits_when_the_client_disconnects() {
        let gate = Arc::new(std::sync::Barrier::new(2));
        let st = Arc::new(CompareCache::new(64 << 20, 2).with_year_gate(Arc::clone(&gate)));
        let sc = two_year_scenario();
        let key = key_of(&sc).unwrap();
        let mut streamed = Arc::clone(&st).serve_stream(key.clone(), sc.clone(), None).await.unwrap();
        let task = streamed.task.take().expect("a computation has a task");
        let year1 = streamed.lines.recv().await.unwrap();
        assert!(year1.starts_with(br#"{"year":1,"#));
        drop(streamed.lines); // the client goes away while year 2 is still to come
        gate.wait();
        tokio::time::timeout(std::time::Duration::from_secs(20), task)
            .await
            .expect("the forwarder exits after the disconnect")
            .unwrap();
        // The forwarder can exit before the computation has stored its body: a request then joins it, and once it is stored the next
        // request is a hit. Either way the computation ran once.
        Arc::clone(&st).serve(key.clone(), sc.clone()).await.unwrap();
        let later = Arc::clone(&st).serve(key, sc).await.unwrap();
        assert!(later.hit, "the computation finished and was stored");
        assert_eq!(st.computed.load(Ordering::SeqCst), 1);
    }

    /// R13-S: a failure after the 200 is one `error` line after the year lines written so far, and the stream then ends. Nothing is stored.
    /// The computation fails after the line of year 2, so the stream carries years 1 and 2, then the error.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failure_after_the_200_is_an_error_line() {
        let st = Arc::new(CompareCache::new(64 << 20, 2).with_failure_at_year(2));
        let sc = two_year_scenario();
        let key = key_of(&sc).unwrap();
        let mut streamed = Arc::clone(&st).serve_stream(key.clone(), sc, None).await.unwrap();
        assert!(!streamed.hit);
        let year1 = streamed.lines.recv().await.unwrap();
        assert!(year1.starts_with(br#"{"year":1,"#));
        let year2 = streamed.lines.recv().await.unwrap();
        assert!(year2.starts_with(br#"{"year":2,"#));
        let error = streamed.lines.recv().await.unwrap();
        assert!(error.starts_with(br#"{"error":"#), "{}", String::from_utf8_lossy(&error));
        assert!(error.ends_with(b"\n"));
        assert!(streamed.lines.recv().await.is_none(), "the stream ends after the error line");
        assert!(lock(&st.state).lru.get(&key).is_none(), "a failed computation is not stored");
    }

    /// R13-S: the common-world cache keeps the least recently used worlds within its budget. A hit refreshes a world, and a store past
    /// the budget removes the least recently used one. The bytes charged are each entry's key plus its estimated clone size, and the
    /// total never passes the budget.
    #[test]
    fn world_cache_evicts_least_recently_used_worlds_within_budget() {
        let cfg = Config { n_households: 300, burn_in_months: 12, ..Default::default() };
        let entry = world_key(&cfg, 1).len() + sim::common_world(&cfg, 1).0.approx_clone_bytes();
        let budget = 2 * entry + 1; // two worlds fit, three do not
        let st = CompareCache::new(0, 1).with_worlds(budget);
        st.common(&cfg, 1);
        st.common(&cfg, 2);
        assert_eq!(st.worlds_built.load(Ordering::SeqCst), 2);
        assert_eq!(st.worlds_used(), 2 * entry);
        st.common(&cfg, 1); // a hit: world 1 is now the most recently used
        assert_eq!(st.worlds_built.load(Ordering::SeqCst), 2);
        st.common(&cfg, 3); // over budget: world 2, the least recently used, goes
        assert_eq!(st.worlds_built.load(Ordering::SeqCst), 3);
        assert_eq!(st.worlds_used(), 2 * entry);
        assert!(st.worlds_used() <= budget);
        st.common(&cfg, 1);
        st.common(&cfg, 3);
        assert_eq!(st.worlds_built.load(Ordering::SeqCst), 3, "worlds 1 and 3 are still stored");
        st.common(&cfg, 2);
        assert_eq!(st.worlds_built.load(Ordering::SeqCst), 4, "world 2 was evicted and is built again");
        assert!(st.worlds_used() <= budget);
    }
}
