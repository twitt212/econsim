# EconSim HTTP API (econsim-server, default port 8787)

All bodies are JSON. Field names are snake_case exactly as serialised by serde from the Rust types
(`Metrics`, `Snapshot`, `Config`, `Shock`, `Scenario`, `Comparison` in crates/econsim-core).

| Method & path | Body | Response |
|---|---|---|
| GET `/api/health` | — | `{ "ok": true, "version": "0.1.0", "prewarm": { "done": n, "total": m } }`; `prewarm` is `null` without `ECONSIM_PREWARM_FILE`, and `done >= total` means the pre-warm pass has finished |
| GET `/api/config/default` | — | `Config` plus `start_states` (see [Start states](#start-states)) |
| POST `/api/sims` | `{ "config"?: Partial<Config>, "seed"?: number }` | `SimInfo` |
| GET `/api/sims` | — | `SimInfo[]` |
| GET `/api/sims/:id` | — | `SimState` (optional `?fields=` filters `history`, see [Field filter](#field-filter-fields)) |
| POST `/api/sims/:id/step` | `{ "months": number }` (1–240) | `{ "tick": number, "metrics": Metrics[] }` (only the new months; optional `?fields=` filters `metrics`) |
| POST `/api/sims/:id/shocks` | `{ "shock": Shock, "at_month"?: number }` (default = current tick, i.e. next month simulated) | `SimInfo` |
| GET `/api/sims/:id/snapshot` | — | `Snapshot` |
| POST `/api/sims/:id/fork` | — | `SimInfo` (deep copy of the world; for counterfactuals from the current state) |
| DELETE `/api/sims/:id` | — | `{ "ok": true }`; an economy idle past the TTL is gone and answers 404 (see `ECONSIM_SIM_TTL_HOURS`) |
| POST `/api/compare` | `Scenario` = `{ config?, shocks: ScheduledShock[], months?, seeds? }` | `Comparison` = `{ baseline: Metrics[], shocked: Metrics[], seeds: number[], warmup_months?: number }` (`warmup_months` only when non-zero); header `x-econsim-cache: hit\|miss` (see [Compare caching](#post-apicompare-caching-concurrency-and-pre-warm)); optional `?fields=` filters the records (see [Field filter](#field-filter-fields)); `?stream=1` answers NDJSON lines year by year, one seed only (see [Streaming](#post-apicompare-streaming-stream1)) |

```ts
type SimInfo  = { id: string; tick: number; seed: number; n_households: number; applied_shocks: ScheduledShock[]; pending_shocks: ScheduledShock[]; stopped?: string }; // stopped: why a step panicked, only once one has (see Errors)
type SimState = SimInfo & { history: Metrics[] };
type ScheduledShock = { at_month: number; shock: Shock };
type Shock =
  | { type: "OilPrice"; pct: number; half_life_months: number }
  | { type: "PolicyRate"; rate: number; months: number }            // rate is a decimal: 0.05 = 5%
  | { type: "PolicyShift"; bps: number; months: number }            // innovation to the Taylor shadow rate: the rate jumps by bps, the gap then decays as an AR(1) with rho = 1 − 1/months (months = mean duration; no expiry, the rule keeps responding)
  | { type: "TaylorRule"; phi_pi: number; phi_u: number; r_star: number }
  | { type: "TermPremium"; tenor_idx: number; bps: number; half_life_months: number } // 0=3m 1=2y 2=5y 3=10y 4=30y
  | { type: "QE"; tenor_idx: number; monthly_face_pct_gdp: number; months: number }
  | { type: "Helicopter"; per_household: number; months: number }   // per_household in t=0 dollars (not wage-indexed; deferred)
  | { type: "MbsQE"; monthly_face_pct_gdp: number; months: number }   // agency-MBS purchases, % of annual GDP per month (negative = sales); moves the mortgage spread, no money moved
  | { type: "GovSpending"; pct: number; months: number }
  | { type: "IncomeTax"; pp: number; months: number }
  | { type: "MacroPrudential"; max_ltv: number; max_dti: number }
  | { type: "CreditSpread"; bps: number; months: number }
  | { type: "Productivity"; sector: number | null; pct: number }   // 0 Energy 1 Food 2 Goods 3 Services 4 Construction
  | { type: "ImportTariff"; pct: number }
  | { type: "Dollar"; pct: number; half_life_months: number };  // shifts the dollar target by ln(1 + pct/100); 0 = permanent
```

Errors: HTTP 4xx/5xx with `{ "error": string }`. CORS is open (`*`). Long computations run on a blocking thread pool.
A month that fails the stock-flow identities stops the run (`sfc_enforce`, default true). `/api/compare` caches nothing
and answers 500; with `?stream=1` the 200 NDJSON response has already started, so the failure arrives as an
`{"error": …}` line, after any year lines and in place of the done line (see [Streaming](#post-apicompare-streaming-stream1)).
A live economy whose step fails answers 500 to that step and to every later request except the list and DELETE. Its world is
dropped, and its `SimInfo` in the list gains `stopped`, the reason (the other fields are as of the failed step); `stopped` is
omitted while an economy runs. A refused request does not touch the economy (see `ECONSIM_SIM_TTL_HOURS`).
A config whose bounds are out of order where the run uses them (`u_star_min` above `u_star_max`, `ebp_min` above `ebp_max`,
`mortgage_spread_min_adj` above `mortgage_spread_max_adj` with a mortgage-spread gain, `r_star_est_min` above 0.03 with
`r_star_adapt` > 0) is refused with 400 on create and compare, the keys named (the CLI exits with status 2); it used to
panic in the run.
Creating a world with 20k households and burn-in takes a few seconds; `/api/compare` with 4 seeds × 60 months may take
10–30 s.

## POST /api/compare: caching, concurrency and pre-warm

- **Response header.** A 200 response carries `x-econsim-cache: hit` when it was served from the response cache, and
  `miss` when it was computed for this request or shared with an identical request already in flight. Errors (400, 500)
  are never cached. The body is `application/json` and is the same JSON as before this layer existed, byte for byte.
- **Cache key.** The validated `Scenario` re-serialised by serde_json. Key order, whitespace, number spelling
  (`2e1` and `20`) and omitted fields that take their default value do not change the key. Any change that reaches
  the simulation does (config, shock list and its order, `months`, the order of `seeds`). The `fields` query parameter
  is not part of the key: it is applied to the response after the cache (see [Field filter](#field-filter-fields)).
- **Determinism.** Results are deterministic for a given request on a given server build, so a hit returns exactly the
  bytes a fresh computation would return. The cache never changes a result. It is in process memory, and on disk when
  `ECONSIM_CACHE_DIR` is set (see the table below); without it, it starts empty after a restart.
- **Single-flight.** Identical requests that arrive while one is computing wait for it and share its result. The
  computation runs in its own task, so it still completes and is cached if the client disconnects. A streamed request for a
  computing body joins it too (see Streaming).
- **Common worlds.** The warmed common world of each seed (the state both arms start from) is kept in memory in its own
  LRU, budget `ECONSIM_WORLD_CACHE_MB`, keyed by the config (start state and targets included) and the seed. Shocks and
  months are not in the key, so a request that differs only in those skips the warm-up. A hit clones the world. Its size is
  an estimate (about 1.1 KB per household plus the history), so the budget is approximate.
- **Cache key and restarts.** The cache key is the validated request only, not the model version; restart the server after changing the binary or the defaults. The memory cache lives in process memory. The disk copy (`ECONSIM_CACHE_DIR`) is kept per build, so a changed binary never reads it.
- **Concurrency cap.** At most `ECONSIM_COMPARE_CONCURRENCY` computations run at once. Further compares queue without
  holding a blocking-pool thread, so `/api/health` and the other routes stay responsive while compares wait.
- **Warm-up (`config.warmup_months`).** The common world is stepped this many months in the reported regime, after the
  burn-in and before the shocks are scheduled; both arms start from that state. Reported months start after the warm-up,
  and their `tick` and `date` are rebased so month 0 is tick 0 at 2026.0. Must be 0 or a multiple of 12 up to 120
  (annual events are keyed to `tick % 12`); anything else is a 400 with `warmup_months must be 0 or a multiple of 12 up to 120`.
  A key that is absent takes the server default (24 since joint calibration 4, set by `ECONSIM_WARMUP_MONTHS`; the CLI and the
  harnesses keep the `Config` default 0 and pin their own value); an explicit value, `0` included, is kept. The response then
  carries `warmup_months` (omitted when 0), and the cache key contains the applied value. `POST /api/sims` validates the key but ignores
  it: live simulations are not warmed up.

Environment variables (read at startup):

| Variable | Default | Meaning |
|---|---|---|
| `ECONSIM_CACHE_MB` | `512` | Byte budget for stored responses (key plus body). Least recently used entries are evicted first, and a response larger than the whole budget is not stored. `0` stores nothing, but identical in-flight requests are still shared. |
| `ECONSIM_CACHE_DIR` | unset | Folder for a disk copy of every computed response, in `<dir>/build-<version>-<hash>/`, one file per key (SHA-256 of the key). A memory miss reads the disk before computing, so a restart keeps its warm cache. The build id hashes the running executable, so a new binary never reads an old one's bytes; the server removes the other builds' folders at startup. Unset or empty: no disk copy. Use one folder per server. Logged as `compare: disk cache DIR`. |
| `ECONSIM_CACHE_DISK_MB` | `4096` | Byte budget of the disk copy. The least recently used files (by mtime; a hit touches its file) are removed when it is exceeded. `0` turns the disk copy off. |
| `ECONSIM_COMPARE_CONCURRENCY` | `max(1, cores / 2)` | Compare computations allowed at once. `0` is treated as 1; a value that is not a number uses the default. |
| `ECONSIM_WARMUP_MONTHS` | `24` (code default since joint calibration 4; `Config::warmup_months` stays 0) | Default `config.warmup_months` for compare requests that omit the key (see Warm-up above). `0` restores the pre-joint-4 behaviour. Must be 0 or a multiple of 12 up to 120; any other value is replaced by the default 24 with one line of output. A line `compare: warm-up N months by default` is printed when N > 0. Harnesses that must not depend on it pin the value (`transmission_irf.py` and `shock_irf.py` set `ECONSIM_WARMUP_MONTHS=0` on the servers they start; `tariff_check.py` and `world_check.py` send `warmup_months: 0`). |
| `ECONSIM_WORLD_CACHE_MB` | `1024` | Byte budget (estimated, see Common worlds) of the warmed common worlds kept in memory, least recently used first. About 5.9 MB per 5,000-household world. `0` turns it off. |
| `ECONSIM_SIM_TTL_HOURS` | `6` | Explore economies (`/api/sims`) untouched for this many hours are removed. A touch is a create, a step, a read, a shock, a fork or a snapshot; a list or a delete is not, nor is a request refused because the economy stopped. Checked on each request and by a sweep every 60 s. A value that is not a positive number gives one line and the default. Each removal is one line: `sim ID removed: expired (untouched for 6 h)`. |
| `ECONSIM_SIM_CAP` | `200` | Most Explore economies kept. Creating one more removes the least recently touched, with one line `sim ID removed: evicted (least recently touched; cap 200 reached)`. A value that is not a positive integer gives one line and the default. |
| `ECONSIM_PREWARM_FILE` | unset | Path to a JSON array of compare request bodies (example: `crates/econsim-server/prewarm_example.json`). After startup, the server runs each entry through the same validated, cached path, one at a time and within the concurrency cap, and logs one line: `prewarm: N entries from PATH in T s: a computed, b already cached, c failed`. Invalid entries are logged and skipped. Skipped when nothing is stored (`ECONSIM_CACHE_MB=0` and no `ECONSIM_CACHE_DIR`). Progress is on `/api/health`. |

## POST /api/compare: streaming (`?stream=1`)

- **One seed.** `seeds` must have exactly one entry; any other count is a 400 that names the rule. `stream` other than `1`
  (or `0`, which is the usual body) is a 400.
- **Format.** `application/x-ndjson`, chunked: one JSON object per line, each followed by a newline.
- **Year lines.** One line per completed year of 12 reported months (the last year may be shorter):
  `{"year":k,"from":m0,"to":m1,"baseline":[...],"shocked":[...]}`. `k` counts from 1. The months are `from` up to but not
  including `to`, so `from` is the first month and `to` one past the last. Each array holds that year's records, the same
  records the full body holds for those months (each arm rebased exactly as in the full body). A year is sent when both arms
  have finished it.
- **Done line, after year lines.** A computed body (fresh, or joined while it runs) ends with
  `{"done":true,"rest":<rest>}`. The rest is the compare body without its `baseline` and `shocked` arrays: `seeds`,
  `warmup_months`, `start_state` and `start_info` where present, in the body's own key order. The records are sent once, in the
  year lines, so the stream is about the size of the plain body.
- **Done line, cache hit.** A body already in the memory cache or on disk sends no year lines and one line,
  `{"done":true,"body":<the compare body>}`, with the same bytes as the non-stream response (with `fields`, the filtered body).
- **Rebuilding the body.** Rule: rebuild from the year lines plus `rest`, or read `body` when the done line has one. To rebuild,
  start from the `rest` object and add `baseline` (the `baseline` arrays of the year lines, concatenated in year order) and
  `shocked` (likewise). As JSON values the result is the non-stream body (with `fields`, the filtered body).
- **Errors.** A failure after the response has started is one line, `{"error":"..."}`, in place of the done line.
- **Fields.** `?fields=` applies to every line, nested names included. Keys of a filtered year line come out sorted, as in
  a filtered body.
- **Cache.** A body already in the memory cache or on disk answers with one `done` line (with `body`) at once (`x-econsim-cache: hit`).
  A request for a body that is being computed joins it: it gets the year lines produced so far, then the rest. The finished
  body is cached exactly as before. `x-econsim-cache` is `miss` for any other streamed response.
- **Discovery.** `GET /api/health` reports `"stream": true` when the server streams.

## Field filter (`fields`)

`POST /api/compare`, `GET /api/sims/:id` (the `history` array) and `POST /api/sims/:id/step` (the `metrics` array) take an
optional `fields` query parameter. A single what-if result carries 80+ observation keys per month, so a client that plots
a handful of series can ask for only those.

- `fields=a,b,c` is a comma-separated list of top-level keys. Spaces around a name are ignored. Each record in the
  `Metrics` arrays keeps only the listed keys, plus the month key `tick`, which is always kept whether or not it is listed.
  `date` is kept only when it is listed.
- `parent.child` keeps one child of a nested object: `world.imports_bn` keeps only that key of `world`, and a whole `world` name wins over its children; one level only, a parent with no listed child that exists is dropped, and a name with more dots matches nothing.
- Unknown names are ignored. A name that is not a key of the record does not appear.
- `fields=` (empty, or only commas and spaces) and an absent parameter both mean no filter. Other query parameters are ignored.
- Everything else in the response is unchanged: `seeds`, `warmup_months`, the `SimInfo` fields and the `x-econsim-cache`
  header. Without a filter the response bytes are exactly as before.
- The filter runs after the compare cache. The cache key is the request body alone, so requests that differ only in `fields`
  share one cache entry, and a pre-warmed body is still `x-econsim-cache: hit` whatever `fields` says.
- A filtered body is compact JSON whose object keys are in sorted order. Each kept value has the same JSON text as in the
  unfiltered response, so the numbers are identical.

Example, one seed and 60 months, keeping three series:

```sh
curl -s -X POST 'http://localhost:8787/api/compare?fields=date,cpi,policy_rate' \
  -H 'content-type: application/json' \
  -d '{"config":{"n_households":5000},"shocks":[{"at_month":0,"shock":{"type":"PolicyShift","bps":100,"months":12}}],"months":60,"seeds":[1]}'
```

Excerpt of the real response: 60 records per array, two shown; the layout is added here.

```json
{
  "baseline": [
    {"cpi": 139.85868215066495, "date": 2026.0, "policy_rate": 0.75, "tick": 0},
    {"cpi": 140.05245838838323, "date": 2026.0833333333333, "policy_rate": 0.75, "tick": 1},
    ...
  ],
  "seeds": [1],
  "shocked": [
    {"cpi": 139.82684633552148, "date": 2026.0, "policy_rate": 1.7500000000000002, "tick": 0},
    ...
  ],
  "warmup_months": 24
}
```

## Start states

`config.start_state` chooses where month 0 of a comparison comes from. `GET /api/config/default` lists the choices in `start_states`
(`id`, `label`, `needs_targets`, `fallback_as_of`).

| `start_state` | Month 0 |
|---|---|
| `"typical"` (default) | The model's own state after the burn-in and the warm-up. Unchanged by this feature, byte for byte. |
| `"us_now"` | "Today's US economy": the common world is stepped through a window of real data (default 24 months) while the policy rate, the 3m/2y/5y/10y/30y yields, the mortgage rate, WTI oil and the import tariff are steered to it, then released; the window is re-drawn until unemployment at month 0 is within 0.5 pp of the data. With `world_block_enabled` and `world_link_trade` the trade link starts from today's tariffs and trade shares (`config.start_targets.trade_ref`). |

`us_now` takes `config.start_targets` (the EconSim website resolves it from FRED and sends it; fields in the table below). If it is omitted the
server uses the snapshot compiled in from `scripts/data/us_now.json`. `warmup_months` is ignored; the response reports the window length in
`warmup_months` and adds `start_state` and `start_info` (vintage, window length, and per seed the world rebuilds `hop`, the window draws
`attempts`, whether the draw was `accepted`, and `u_month0`). The cache key contains the targets. Errors: 400 for `start_targets` with
`typical`, for `us_now` with the world block and a dollar, finance or oil link, for a malformed window, for an unknown `trade_ref`, and for
`us_now` on `POST /api/sims`.

| Field | Type | Unit | Allowed range |
|---|---|---|---|
| `as_of` | string | vintage id, `YYYY-MM-DD` or `YYYY-MM-DD.2` | 1 to 32 characters |
| `window.months` | string[] | labels such as `"2024-11"`; only the length is used | 12, 24 or 36 entries |
| `window.policy_rate` | number[] | percent, upper bound of the FOMC range | 0 to 25 |
| `window.y3m, y2, y5, y10, y30` | number[] | percent | -1 to 25 |
| `window.mortgage_rate` | number[] | percent | 0 to 30 |
| `window.oil_wti` | number[] | $ per barrel | 5 to 400 |
| `window.tariff_pct` | number[] | percent | 0 to 100 |
| `unemployment_rate` | number | percent | 1 to 25 |
| `tuning` (optional) | object | `accept_u` 0.5, `max_attempts` 8, `max_hops` 3, `mortgage_half_life` 36, `tp_half_life` 0, `wedge_decay` 0.97 | see `StartTargets::validate` |
| `trade_ref` (optional) | string | vintage id, or `"none"` | `"2026-08-24"` (the only vintage so far); `"none"` starts without the trade reference; an unknown id is a 400; absent with the world block and the trade link on, the server fills in the newest (the Today's-start design note (not published; its content is summarised in CALIBRATION, `world-usnow`)) |

All arrays have the same length (12, 24 or 36 entries, oldest first; the last entry is the latest observation). Rules (400 `{ "error": ... }`):

- `start_state: "typical"` (default) with `start_targets` present: `start_targets is only used with start_state us_now`.
- `us_now` with `world_block_enabled: true` and `world_link_dollar`, `world_link_finance` or `world_link_oil` true: `start_state us_now takes the world block with the trade link only (world_link_dollar, world_link_finance and world_link_oil must be false)`.
- `start_targets.trade_ref` that is neither `"none"` nor a known vintage: `start_targets.trade_ref "<id>" is not a known vintage`.
- `us_now` with `start_targets` missing on `POST /api/compare`: not an error; the server uses its compiled-in `scripts/data/us_now.json`.
- `us_now` on `POST /api/sims`: `start_state us_now is available on /api/compare only`.
- Bad window or values: `start_targets.window.y10 must have 24 entries`, `... values must be finite and within [lo, hi]`, etc.
- `config.warmup_months` is ignored with `us_now` (the window is the warm-up); the response reports `warmup_months` as the window length.

With `us_now` the response adds (absent with `typical`):

```json
"warmup_months": 24, "start_state": "us_now",
"start_info": { "as_of": "2026-10-12", "window_months": 24,
                "per_seed": [ { "seed": 1, "hop": 0, "attempts": 2, "accepted": true, "u_month0": 4.38 } ] }
```

With the world block and the trade link on, the response also adds `start_info.trade_ref` (`{"id": "2026-08-24", "as_of": "2026-08-24"}`,
the reference the run started from) and `world.imports_goods_bn` (twelve annualised $bn, US goods imports by origin bloc in the order of
the blocs; absent without a reference). A `Tariff` on Today's start adds points to today's duty on the named partner and goods.

`baseline[0].tick` is 0 and `date` is 2026.0, as with the warm-up. `GET /api/config/default` returns the `Config` keys, now including
`"start_state": "typical"` and `"start_targets": null`, followed by `"start_states": [{"id":"typical",...},{"id":"us_now","label":"Today's
US economy","needs_targets":true,"fallback_as_of":"2026-10-12","trade_ref":{"latest":"2026-08-24","as_of":"2026-08-24"}}]`. `typical` has
no `trade_ref`; `us_now` has it (the newest vintage the server fills in).

Month 0 is the first free month after the window: policy rate, yields, mortgage rate, oil and tariff start within about 0.1 of the data on
average; unemployment starts near 4.2; inflation measures are outcomes of the model (see CALIBRATION.md, us-now).

**Web route.** `GET /api/us-now` and `GET /api/us-now?asof=ID` are served by the EconSim website (a separate project, not part of this repository), not by the Rust server. The
website fetches the FRED series (keyless CSV), keeps the last good copy, and publishes a vintage when the data move materially. Its id is
the UTC date (`.2`, `.3` for a second change that day). `asof` returns a stored vintage and 404 for an unknown one. Shared links carry
`asof`, so they reproduce the same targets.

## Observation fields (`Metrics`, metrics-out)

Every monthly record also carries 80 observation fields (crates/econsim-core/src/metrics.rs, struct `Observations`,
flattened into the record so each is a top-level key). They are computed from the model state and never feed back
into it; the existing fields are unchanged. Cadence: most are monthly; `(quarterly)` fields are refreshed every 3
months with the Gini block and carried forward; `(annual)` fields are updated each January and carried forward; `(annual refresh)` fields are recomputed every 12th month (and at the first distributional month) and carried forward between refreshes.
Older binaries omit them, so the TypeScript types mark them optional.

| Group | Fields (units) |
|---|---|
| Equity and Fund | `equity_return_1m` (%, monthly total return); `fund_dividends_paid_bn_ar` (dividends in that return: firms' declarations of last month plus bank dividends declared this month, $bn ar); `bank_dividends_bn_ar` (banks' dividends declared this month); `fund_dividends_regular_bn_ar`, `fund_dividends_special_bn_ar` (firms' regular and special dividends declared this month, paid to the Fund next month; bank dividends are not included); `equity_discount_rate` (%, y_eq); `sector_earnings_bn_ar`, `sector_dividends_bn_ar` (Sector order; bank dividends are not in the sector split); `sector_q` (relative to normal) |
| Policy and curve | `expected_short_rate` (%, by TENORS: the expectations component); `shadow_rate` (%); `r_star_est`, `obs_r_star_est`, `u_star` (% real-time estimates; `r_star_est` is the top-level `Metrics` field and `obs_r_star_est` its copy in this block, same value. Before joint calibration 4 both were written under the key `r_star_est` (a duplicate JSON key: parsers kept the last one, and `/api/compare` returned that copy as 0.0)) |
| Banks and credit | `lender_premium` (%, mean `Bank.ebp`); `expected_business_loss_rate` (%/yr, monthly); `biz_chargeoff_3m_ar` (% of business loans, annualised, this month and the two before); `firm_loan_rate`, `firm_loan_rate_p90` (%, principal-weighted 90th percentile, monthly); `bank_interest_expense_bn_ar`, `net_interest_income_bn_ar`, `bank_assets_bn`; `consumer_credit_bn`; `mortgage_delinquent_30_share`, `mortgage_delinquent_90_share` (% of mortgage balance); `overdraft_share` (% of alive households), `overdraft_balance_bn` |
| Firms | `firm_entries_ar`; `firms_by_age` (count, <1y, 1-4y, 5-9y, 10y+); `firm_exits_by_age_ar` (bankruptcies this month by age at exit, annualised count, classes <1y, 1-1.9y, 2-4.9y, 5-9.9y, 10y+); `firm_size_counts`, `firm_size_emp_share` (size in persons = heads × 1.75; classes 0-4, 5-9, 10-19, 20-49, 50-99, 100-249, 250-499, 500-999, 1000+ persons); `firm_share_500plus` (% of employment, firms with 500+ persons); `firm_size_tail_exponent` (Zipf in persons, firms with 10+ persons, annual refresh); `job_creation_by_age`, `job_destruction_by_age` (%/yr of average employment; classes under 12 months, 1-4 years, 5+ years at the start of the year; annual); `job_creation_entrants_pct` (%/yr, firms born since the last January only, BDS age 0; annual); `markup_p10/p50/p90` (100 × markup, annual refresh; P/MC = 1 + markup/100, so the dispersion ratio is (100 + p90)/(100 + p50)); `wage_offer_log_var` (variance of log wage OFFERS across alive firms, monthly; offer dispersion, not realised pay), `wage_offer_cv`; `wage_log_var_total`, `wage_log_var_between_firm`, `wage_log_var_within_firm` (realised log-wage variance of employed heads: total, between employers, within employers; total = between + within; annual); `wage_between_firm_share` (% of the realised variance that is between employers, raw with no worker fixed effects; annual); `investment_spike_share` (% of alive firms with a full 12-month window whose investment, the sum of monthly orders to January, exceeds 20% of capital), `investment_inaction_share` (|12-month investment| < 1% of capital); both annual, over firms born before the window started, and null until 12 months are recorded |
| Bankruptcy displacement | `bankruptcy_displaced_heads_ar` (employed heads displaced this month by firms that went bankrupt, annualised count; heads per exit = displaced ÷ exits); `flow_bankruptcy_displaced_pct` (% of employed heads at the previous record) |
| Worker flows | `flow_ee_pct` (EE moves, % of employed heads at the start of the labour step), `flow_eu_exo_pct` (exogenous E→U, % of employed heads at the start of the labour step), `flow_ue_pct` (U→E hires, % of unemployed heads at the start of the labour step); all three denominators are start-of-step stocks; `employed_heads_n` (raw count) |
| Stayers (annual) | `stayer_wage_change_p10/p50/p90` (% log change over 12 months), `stayer_wage_change_kelley` (Kelley skewness); stayers are employed 12+ months with the same employer a year earlier. Reported as null (not 0) until a year with a full year-end snapshot and at least 10 stayers |
| Distribution (annual refresh; existing distribution fields are quarterly) | `wealth_top1_share`, `wealth_bottom50_share` (%); `share_nonpositive_nw` (%); `median_nw_by_age` (model $, AGE_BUCKETS order); `htm_share` (deposits below half a month's gross income), `htm_wealthy_share` (of which homeowners); `disposable_income_gini` (gross income less taxes), `earnings_gini`, `consumption_gini`; `earnings_top1_share` (%) |
| Real activity, prices, housing | `real_residential_investment_bn_ar`; `trend_gdp_nominal_bn_ar` (nominal, the fiscal trend); `energy_price_level`; `core_pce_level`; `oil_ar_pct`, `oil_shock_pct` (log deviations, %); `transaction_price_index` (average sale price, 100 at the first sale month); `expected_hpa` (%/yr); `real_dollar_index`, `world_price_index` (100 at t=0); `tfp_level` (by sector) |
| Ownership | `owners_m`, `tenure_base_m` (millions, scaled) |

Definitions: gross job flows are net changes of firm employment between January snapshots of the same firm (matched on
slot and age; a reborn slot counts as exit plus entry). Investment rates are annual: each firm's orders summed over the
12 months to January, over its capital at the start of that window, i.e. at the previous January (metrics-only accumulators reset on death; the first window includes any
burn-in months recorded before month 0). Firm size is in persons (employed heads × 1.75), the unit of
`employment_persons_millions` and of the mean firm size. Entrant survival is not tracked by cohort: the early-exit rate
is exits under 2 years of age over entries (a steady-state proxy). Motor-fuel price is the existing `gasoline_price`.
Not reported (no state yet): hours worked, durable-goods stock, new vs existing home sales, the term-premium factors and
dollar risk premium of step 0a, cohort survival beyond two years, personal bankruptcy, and the owner's wage within
proprietors' income.

## Units and conventions

- `Metrics` money fields: `_bn_ar` = billions of dollars at annual rates; `_bn` = billions (stocks); scaled to the US by
  `Config.scale` (0 = auto: 132M / n_households). Rates, inflation, unemployment, shares and yields are **percent**
  (4.2 = 4.2%). Gini coefficients are 0–1. `employment_millions`, `population_households_m` are millions.
- `Snapshot.by_age_ownership`, `by_age_employment_rate`, `by_age_mortgage_rate`, `sector_markups` are percent;
  `yield_curve` is `[maturity_months, yield_percent]` for 3, 24, 60, 120, 360 months.
- Year-over-year fields (`*_yoy`) read 12 months back, into the burn-in history when fewer than 12 months have been reported, so they are valid from month 0 with the default burn-in; with `burn_in_months` 0 they are 0 until 12 months exist. `consumption_by_quintile` is 0 until 12 months of history exist.
- Shock parameters: `PolicyRate.rate` is a decimal (0.05 = 5%); `PolicyShift.bps`, `TermPremium.bps`,
  `CreditSpread.bps` are basis points; `tenor_idx` 0–4 = 3m, 2y, 5y, 10y, 30y; `sector` 0–8 = Energy, Food, Durable
  goods, Nondurable goods, Tech, Leisure & hospitality, Health & education, Other services, Construction (or null =
  all); `TechBoom.months` forces a technology boom for that many months; `MacroPrudential` limits are decimals. Out-of-range values return 400.
- The top income-tax bracket bound is the finite sentinel `1e15`.
- Measured GDP (BEA-style: housing services, indirect taxes, in-kind health and FISIM included) is ≈68% of BEA GDP at burn-in 0 and ≈88–104% at the first reported month, where the default burn-in has lifted the price level (see Known limitations of the earlier README, which is not part of this release; `scripts/facts_distribution.py` uses the factor 0.83)
  (no nonprofit sector or non-employer self-employment). Use growth rates, ratios and shocked-minus-baseline
  differences.

## World block: shocks and `world` metrics (stage 1)

Three shocks (validated always, ignored while `world_block_enabled` is false):

| Shock | Fields | Meaning |
|---|---|---|
| `BlocPolicy` | `bloc` (EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW), `bps` (|bps| <= 2000), `months` (3, 6, 12, 18, 24) | The bloc's policy rate held `bps` above its rule for `months` months (pin table) |
| `BlocDemand` | `bloc`, `pct` (|pct| <= 20), `half_life_months` (1-120), `risk_off` (default false) | IS demand shock whose isolated peak gap equals `pct` |
| `GlobalDemand` | `pct`, `half_life_months` | `BlocDemand` on all twelve blocs |

`Metrics.world` (absent from the JSON when the block is off) has, per bloc in the order above: `y_gap` (% output gap),
`pi_gap` (annualised pp), `i_gap` (pp), `z` (% real exchange rate, plus = depreciation), `infl` (annualised pp level),
`policy` (pp level, floored at the bloc's policy floor), `s_index` and `p_index` (1 at the first reported month), `headline`
(= `infl` in stage 1) and `u_gap` (pp; zero for OPEC+ and RoW).


## World block: trade shock and stage-2 `world` metrics (stage 2)

One more shock, validated always, used only while `world_link_trade` is on (ignored with the link off, and with the block off):

| Shock | Fields | Meaning |
|---|---|---|
| `Tariff` | `partner` (null or a bloc name), `sector` (null or 0-8), `pct` (0-500, percentage points), `retaliate` (default false) | A bilateral duty component on the origin `partner` and `sector` (null: all), added on top of the set-level `ImportTariff` level (which is replaced, not added). Decimal inside the block (25 % is 0.25). Permanent. `retaliate` (stage 4-3): with `world_retaliation` on, the partner's duty on US exports follows the retaliation rule and the export factor R multiplies the bilateral exports; with it off the flag is recorded and unused (see the Stage 4-3 paragraph below) |

Stage 5 (D60): a `Tariff` posted to `/api/compare` with `world_block_enabled` or `world_link_trade` off is refused with 400 (`Tariff needs world_block_enabled and world_link_trade (otherwise it is ignored)`). `ImportTariff` is unchanged and accepted with the block off.
| `OilSupply` | `pct_of_world_supply` (-30 to 50; a loss > 0, an SPR release < 0), `ramp_months`, `hold_months`, `recover_months` (each >= 1) | Oil supply disruption: ramp to the full loss, hold it, restore linearly (the Hormuz path of the research K7 is 12, 1, 3, 6). Acts through the oil market when `world_link_oil` is on; ignored otherwise. Stage 3 |
| `OpecCut` | `mbd` (|mbd| <= 10; a cut > 0), `months` (>= 1) | OPEC+ policy surprise of -(mbd/103.1) x 100 / months % of world consumption in each of `months` months. Acts through the oil market when `world_link_oil` is on. Stage 3 |

`Metrics.world` gains these keys when the block is on (all finite; zero before the trade base): `imports_bn[12]` and
`exports_bn[12]` (annualised $bn by bloc, their sums equal `imports_bn_ar` and `exports_bn_ar`), `r_import[12]` (% origin log price),
`tau_eff_hh` and `tau_eff_firm` (decimal effective duties of the household and firm channels), `tau_by_origin[12]` (decimal),
`import_volume_idx` and `export_volume_idx` (100 at the month before the base; nominal over the tradables price ratio),
`import_price_idx` (100 x the CPI-weighted import price index), `ret_tau[12]` (goods-weighted mean retaliatory duty on US exports to each bloc; zero unless `world_retaliation` is on and a flagged `Tariff` is active) and `us_import_gap[12]`
(% import-demand term of the bloc network). Diagnostics: `z_tilde[12]` (% common-free real exchange rate) and `c_t` (% common
component). Stage 2 adds 13 keys to the stage-1 `world` object; no top-level key is added.
Stage 4-3 adds five keys to the `world` object (zero with `world_positions` off; `exports_goods_bn` is always filled): `exports_goods_bn[12]` (US goods exports, Energy to Tech, by destination bloc, annualised $bn; the statistic of W4-24 is scored on it), `niip_pct_gdp` (memo net international investment position, % of annualised model GDP), `ca_pct_gdp` (current account, % of GDP), `ca_bilateral_bn[12]` (current account by bloc, annualised $bn; their sum is `ca_pct_gdp`/100 times annual GDP), `sfc_world_error` (the largest identity residual of the month relative to monthly GDP; a debug build panics above 1e-9). The three percentage keys (`niip_pct_gdp`, `ca_pct_gdp`, `sfc_world_error`) divide by the GDP of the month before (`gdp_month_model` reads the previous record at settle time), as 4-2's `row_mv_gdp_pct` does. Still no top-level key.

Stage-2 configuration keys (all in `Config`, defaults in CALIBRATION "b8-world S2"): `world_link_trade` (false), `world_us_import_channel`
(true), `world_origin_sigma_sr`, `world_origin_sigma_lr`, `world_origin_speed`, `world_dcp_kappa_usd`, `world_dcp_speed_usd`,
`world_dcp_kappa_nonusd`, `world_dcp_speed_nonusd`, `world_export_income_elast`, `world_export_price_sr`, `world_export_price_lr`,
`world_export_fx_sr`, `world_export_fx_lr`, `world_export_speed`.

Stage 3 (oil market, `world_link_oil`, off by default): `Metrics.world` gains `p_bar_pct` (market monthly-average price, % log deviation,
the stochastic offset not removed), `brent_usd` (80.53 exp(p/100), 2024 USD), `oil_supply_pct`, `oil_demand_pct` (% of world consumption),
`opec_out_mbd`, `us_prod_mbd` (mb/d), `stocks` (inventory state, mb), `spare_pct` (spare capacity left, % of Q), and the diagnostic `oil_dev_pct`
(the ABM oil log deviation this month, offset removed and the splice included). Keys are present when the block is on; zero when the link is off.
Config keys added in stage 3: `world_link_oil`, `world_oil_profile`, `world_opec_regime` (`price_defence` or `market_share`),
`world_oil_exact_path`, `world_oil_mean_offset`, `world_oil_sigma_mult`, `world_oil_rho_q`, `world_oil_sig_i_mult` (JSON key and field
spelled alike; it was `world_oil_sigI_mult` before the fix round; the server accepts the retired spelling as an alias, the CLI overlay rejects it), `world_oil_splice_rho`, `world_oilcheck_activity_sigma`,
`world_oilcheck_activity_rho`, `world_opec_uae_spare_share`.
Stage-3 fix round: `world_oil_feedback` (default true; false makes the market ignore the blocs' activity and real exchange rates, the
exact-path test), `world_oil_shock_mask` (default `IDO`; the structural sources I, D, O drawn into the market, the W3-19 cells),
`world_oil_qty_offset` (default -6.627, the stochastic mean of the OPEC+ quantity input, subtracted when `stochastic_oil` is on). The
server rejects an unknown `world_oil_profile`, `world_opec_regime` or mask character with 400. `Metrics.world` gains
`oil_us_demand_pct` (the market's US-region demand deviation, % of Q; W3-18).
Stage 4-1 (dollar link, `world_link_dollar`, off by default): config keys `world_link_dollar`, `world_vix_per_unit` (10), `world_vix_per_100bp` (13), `world_fx_risk_mult`, `world_fx_risk_rho` (0.9), `world_fx_risk_asym` (0.588), `world_fx_rp_sd_mult`, `world_fx_idio_sd_ae`/`_eme`/`_cn`/`_opec`, `world_fx_idio_rho`, `world_theta_f_ratio`, `world_china_risk_pts_per_pct`, `world_gdp_rw_bn`. Shocks: `RiskOff {vix_points}` (0 to 100 VIX points) and `UsCredibility {tp_bp, months, dollar_pct}` (tp_bp 0 to 200, months at least 1, dollar_pct 0 to 50). The dollar part is ignored when the dollar link is off; the term-premium part is applied from sub-round 4-2 when `world_link_finance` is on and ignored when it is off. `Metrics.world` gains `f_vix` (F_t, VIX points), `f_risk` (Frisk_t - Frisk_b), `fx_broad` (100 fx_t / fx_0, with the link on or off; 100 before the base), `a_dollar` (100 a_t) and `s_bil` (the bilateral rates S_j, bloc order); they are serialised when the block is on and are zero before the dollar state exists. The server accepts the same keys; the CLI overlay rejects an unknown key.
Stage 4-2 (official Treasury block and term premium, `world_link_finance`, off by default): config keys `world_link_finance`, `world_fo_theta_stock` (13), `world_fo_theta_trans` (20), `world_fo_trans_rho` (0.944), `world_fo_dollar_resp` (-0.0023), `world_fo_vix_resp` (1.05), `world_fo_trend_cn` (-80; the lever {-80, 0}). Ranges, checked whatever the link state (the server answers 400, the CLI exits with status 2): `world_fo_theta_stock` and `world_fo_theta_trans` 0 to 100; `world_fo_trans_rho` 0 up to below 1; `world_fo_dollar_resp` at most 0.05 in absolute value; `world_fo_vix_resp` 0 to 10; `world_fo_trend_cn` at most 500 in absolute value; `world_gdp_rw_bn` above 0 and at most 1e6. Shock `OfficialTreasury {bloc, bn, months}` (bloc null or a bloc name, |bn| at most 2000 in real-world $bn in total, months 1 to 60; ignored with the link off). `UsCredibility {tp_bp, months, dollar_pct}` now has two parts: the dollar term needs `world_link_dollar` (4-1), the term-premium part `tp_bp` needs `world_link_finance` (4-2; the 10y carries it, the other tenors load it by `TP_SLOPE[j]/TP_SLOPE[3]`). `RiskOff` needs `world_link_dollar`; the finance link does not inject into `f_t`. `Metrics.world` gains `fo_bn`, `fo_flow_bn`, `tp_fo_bp`, `tp_cred_bp`, `fo_extra`, `f_ring_mean`, `row_mv_gdp_pct`, `row_dep_gdp_pct`, `fo_share_row_pct` and `row_face_share_pct` (all zero with the link off). The server and the CLI accept the same keys.
Stage 4-1b (list B, dollar-link redesign, `world_link_dollar`, off by default): config keys `world_dollar_neutral` (bool, default true: the link reads each bloc's real exchange rate against its conditional forecast from the base month, D47; false gives the 4-1 form), `world_rp_sat_knee` (decimal a year, default 0.02) and `world_rp_sat_scale` (decimal a year, default 0.015; 0 turns the bound off): the bound on the total US-rate excess return `ex[k] + lambda_rp gp[k]` in the bloc dollar equation (D48). Ranges, checked whatever the link state (the server answers 400, the CLI exits with status 2): `world_rp_sat_knee` and `world_rp_sat_scale` finite and in 0 to 0.2. The bound and the forecast act only with `world_link_dollar` on; with the link off the three keys are inert. `legacy_s4a_dollar.json` (`world_dollar_neutral` false, `world_rp_sat_scale` 0) reproduces the 4-1 link. `Metrics.world` gains `z_fc` (per bloc: the z each bloc is read against in the month, the base `z` with `world_dollar_neutral` off; zero before the base and with the dollar link off).

Stage 4-3 (tariff retaliation and cross-border positions, `world_retaliation` and `world_positions`, both off by default): config keys `world_retaliation` (false; needs `world_link_trade`), `world_ret_eps_sr` (1.0), `world_ret_eps_lr` (3.0), `world_ret_speed` (0.10, [assumed]), `world_positions` (false). Ranges, checked whatever the switch state: each elasticity 0 to 10 with `world_ret_eps_lr` at least `world_ret_eps_sr`; `world_ret_speed` 0 to 1; NaN and infinity refused. Refused with status 400 (server) or exit 2 (CLI), the key named: `world_retaliation` true with `world_link_trade` false. The dollar and finance links are not part of any 4-3 profile (`b8-world-s4c.json` keeps them off).

## World block: stage 5 (centring constants, profiles, the `Tariff` check)

Stage 5 adds one config key and no default. `Config::default` is unchanged: a request without the key, or with the block off, gives the same numbers as before stage 5 (fingerprint `9f90c88a2c8b653857029d235cd6b0a6` at 5000 households, 60 months, seed 3).

- `world_import_gap_offset` (array of twelve numbers in percentage points, bloc order EA, CN, JP, UK, CA, MX, IN, KR, BR, AU, OPEC+, RoW; default empty). It is subtracted from the coupled import-demand term after the clamp, once the term is active (after reported month 12). Empty means zero for every bloc. Each value must be finite and within +-10; a wrong length, a value outside the range, NaN or infinity is refused with 400, and the message names the key.
- `Tariff` needs `world_block_enabled` and `world_link_trade` (400 otherwise, see above).
- Stage 5-T2 (D62): `world_import_rel_drift` (a decimal per year, default 0, finite and within +-0.05; any other value answers 400 naming the key). The block's import price is multiplied by `exp(-g n / 12)`, `n` the months since the base, so the import price drifts against the tradables price index; 0 is the stage-5 behaviour bit for bit. Stage 5-T2 also folds a uniform `Tariff{null, null, p}` into the set-level duty (D63): with it, `ImportTariff{p}` and `Tariff{null, null, p, false}` give the same run, every key of every record. No other request change.
- Profiles (`scripts/profiles/`, one JSON object each, to be passed as the request `config` or as `ECONSIM_CFG`; they set `warmup_months` 0 in the registered runs):

| File | Content | Class (see CALIBRATION `b8-world S5`) |
|---|---|---|
| `b8-world-s5a.json` | trade, retaliation and positions, with the twelve constants | trade off (S5-c); the file stays as the trade profile |
| `b8-world-s5a-lean.json` | `s5a` without the six financial-factor keys (reported arm) | as `s5a` |
| `b8-world-s5a-nc.json` | `s5a` with `world_us_import_channel` false (fallback S5-b, not run) | off |
| `b8-world-s5b.json` | `s4e` (dollar link and M-F keys) with the constants | dollar not run (needs trade) |
| `b8-world-s5c.json` | `s5b` with `world_link_finance` | dollar not run |
| `b8-world-s5c-nd.json` | `s5a` with `world_link_finance` (finance without dollar) | profile |
| `b8-world-t2a.json` | `s5a` with the drift `world_import_rel_drift` 0.0032 and the re-measured constants (2.381, 2.485, 2.419, 3.338, 1.838, 2.748, 3.678, 2.837, 2.568, 2.169, 2.264, 3.056) | trade (5-T2, see CALIBRATION `b8-world 5-T2`) |
| `b8-world-t2b.json` | `s5b` with the same two replacements (the dollar rung) | dollar: profile (D55) |
| `b8-world-t2c.json` | `s5c`, that is `t2b` with `world_link_finance` | finance: profile (D55) |
| `b8-world-t2a-nc.json` | `t2a` with `world_us_import_channel` false (the S5-b fallback; the constants are inert) | fallback, not run |
| `b8-world-t2c-nd.json` | `s5c-nd` with the same two replacements (finance without the dollar link) | fallback, run only if the dollar rung is off |
| `b8-world-wl.json`, `b8-world-wabm.json`, `b8-world-wabm-mf.json` | block-off reference cells (ABM keys only) | reference |
| `legacy_b8off.json` | the J4 defaults written out with the block off: `Config::default()` unchanged field by field | reference (I9) |

<!-- stage5-example: POST /api/compare -->
```json
{"config": {"n_households": 300, "warmup_months": 0, "world_block_enabled": true, "world_link_trade": false}, "shocks": [{"at_month": 0, "shock": {"type": "Tariff", "partner": "CN", "sector": null, "pct": 25.0, "retaliate": false}}], "months": 6, "seeds": [1], "expect_status": 400}
```

<!-- stage5-example: POST /api/compare -->
```json
{"config": {"n_households": 300, "warmup_months": 0, "world_block_enabled": true, "world_link_trade": true, "world_import_gap_offset": [1.983, 1.858, 1.802, 2.933, 1.163, 1.885, 3.007, 2.102, 2.125, 1.822, 1.814, 2.577]}, "shocks": [{"at_month": 0, "shock": {"type": "Tariff", "partner": "CN", "sector": null, "pct": 25.0, "retaliate": false}}], "months": 6, "seeds": [1], "expect_status": 200}
```

<!-- stage5-example: POST /api/compare -->
```json
{"config": {"n_households": 300, "warmup_months": 0, "world_block_enabled": false}, "shocks": [{"at_month": 0, "shock": {"type": "ImportTariff", "pct": 10.0}}], "months": 6, "seeds": [1], "expect_status": 200}
```
