# EconSim

EconSim is an agent-based simulator of the US economy, written in Rust. You change one thing (a Fed rate rise, an oil
shock, a tariff) and it runs the same economy twice from the same starting state with the same random numbers: once with
the change, once without. The difference between the two runs is the effect of the change, and averaging it over many
seeds averages out chance. **It is a simulation, not a forecast.** Where the model disagrees with the data, the documents
say so ([docs/STYLIZED_FACTS.md](docs/STYLIZED_FACTS.md), [docs/CALIBRATION.md](docs/CALIBRATION.md)).

The model runs monthly and every flow is a payment between two balance sheets, checked each month. It has households
(working, saving, renting or owning, ageing), firms in nine sectors, banks that create money by lending, a central bank
with a Taylor rule and a yield curve, and a government. An optional "world block" adds twelve foreign economies, trade,
oil and finance.

This repository holds the model (`crates/econsim-core`), the HTTP API (`crates/econsim-server`), a command-line runner
(`crates/econsim-cli`), the design and calibration documents (`docs/`), the research data (`data/`) and the check scripts
(`scripts/`). The EconSim website is a separate project that sends requests to this API; it is not part of this
repository.

## Build and test

You need a stable Rust toolchain (built and tested with 1.99.0). Python 3 with numpy (and pandas and scipy for `scripts/world/`) is needed only for `scripts/`.

```sh
cargo build --release
cargo test --release
```

## Run the API server

```sh
./target/release/econsim-server                  # listens on port 8787
PORT=9000 ./target/release/econsim-server        # another port
curl http://localhost:8787/api/health
```

Settings are environment variables; all are optional.

| Variable | Default | What it does |
|---|---|---|
| `PORT` | 8787 | Port to listen on (all interfaces) |
| `ECONSIM_WARMUP_MONTHS` | 24 | Months the common world is stepped before month 0 of a comparison that does not set `config.warmup_months`; 0 or a multiple of 12 up to 120; 0 gives the earlier behaviour |
| `ECONSIM_CACHE_MB` | 512 | Memory for stored comparison responses; 0 stores nothing |
| `ECONSIM_COMPARE_CONCURRENCY` | half the cores | Comparisons computed at once |
| `ECONSIM_WORLD_CACHE_MB` | 1024 | Memory for the cache of starting worlds; 0 disables it |
| `ECONSIM_CACHE_DIR`, `ECONSIM_CACHE_DISK_MB` | unset, 4096 | Disk copy of the response cache that survives restarts of the same binary |
| `ECONSIM_PREWARM_FILE` | unset | JSON file of requests to compute at start-up (`crates/econsim-server/prewarm_example.json`) |
| `ECONSIM_SIM_TTL_HOURS`, `ECONSIM_SIM_CAP` | 6, 200 | Idle time after which a live simulation (`/api/sims`) is removed, and how many are kept |
| `RAYON_NUM_THREADS` | all cores (on Apple silicon, the performance cores plus one) | Threads for the simulation; results do not depend on it |

The server has no authentication or rate limit, answers requests from any origin, and listens on all interfaces. Put it
behind a proxy of your own before you make it reachable by others. Every route, field and error is in
[docs/API.md](docs/API.md).

## Run the CLI

```sh
./target/release/econsim-cli run 5000 60 3        # households, months, seed: a table every 6 months
./target/release/econsim-cli irf 2000 24 2        # impulse responses to the standard shocks: households, months, seeds
./target/release/econsim-cli config               # the default configuration as JSON
```

`run` also takes `--json` to print the whole monthly history as a last line. A configuration profile is a JSON file of
config keys, applied with `ECONSIM_CFG`:

```sh
ECONSIM_CFG="$(cat scripts/profiles/b8-world-s2.json)" ./target/release/econsim-cli run 5000 24 1
```

The other binaries in `crates/econsim-cli/src/bin/` are diagnostics used by the scripts in `scripts/`.

## Reproduce a result from the website

Every result page on the website has a section "About this result". Its "API request for this result" is a `curl` command
with a JSON body for one seed (seed 1). The body holds the settings (`config`), the change (`shocks`), the length in
months and the seed. To reproduce it, start the server and send the same body to your own copy. This is the request for a
surprise Fed rate rise of 1 point, 5,000 households, 60 months, seed 1:

```sh
curl -s -X POST 'http://localhost:8787/api/compare?fields=date,unemployment_rate,cpi,policy_rate' \
  -H 'Content-Type: application/json' \
  -d '{"config":{"rate_perception_months":11,"rate_perception_firm_months":7,"rate_perception_stages":3,"rate_perception_deposit":true,"rate_perception_credit_bypass":0.7,"credit_tail_half_life_months":4,"fisim_constant_margin":true,"rate_smoothing":0.88,"speed_limit_weight":0.75,"energy_extraction_labor_share":0.11,"energy_extraction_elasticity":1,"user_cost_pi_e_mode":2,"user_cost_pi_e_offset":0.0035,"hh_pump_real_rate_weight":0,"phi_u":0.25,"n_households":5000},"shocks":[{"at_month":0,"shock":{"type":"PolicyShift","bps":100,"months":12}}],"months":60,"seeds":[1]}'
```

The answer has `baseline` (the economy without the change) and `shocked` (with it), one record per month. The effect is
shocked minus baseline. `?fields=` keeps only the listed keys of each record; leave it out to get every key, as the website
does. The website averages that difference over seeds 1 to N (N is 6, 12 or 32, depending on the
precision chosen on the page); to do the same, send the body once per seed, changing `seeds`.

**Seeds and determinism.** A request gives the same numbers every time on the same build, with any number of threads. A
seed fixes every random draw, and the baseline and the shocked run share them. Different builds can differ, so a result
is tied to a build and a model fingerprint.

**The fingerprint.** The default fingerprint is the md5 of the last line of a default run:

```sh
RAYON_NUM_THREADS=1 ./target/release/econsim-cli run 5000 60 3 --json | tail -n 1 | md5     # md5sum on Linux
```

On macOS it must be `6a56affe5fa25d7fcf1fa0ba7197234e` (see the accounting fixes in [docs/CALIBRATION.md](docs/CALIBRATION.md)),
and the same with `RAYON_NUM_THREADS=8`. If it is not, your build does not run the model of this release. The md5 can
differ between platforms, because the system maths library's `exp`, `ln` and `pow` are not bit-identical everywhere: on
Linux x86-64 (glibc) the same run gives `a06730ec789aa0742f2052eeb64e631b`. With
`ECONSIM_CFG="$(cat scripts/profiles/legacy_acctfix_off.json)"` the run reproduces the model before the accounting fixes:
`9f90c88a2c8b653857029d235cd6b0a6` on macOS, `1a76b12271eee590b51c6f247312627d` on Linux x86-64. Documents, profiles and
checks that name other fingerprints say which profile they belong to.

## Where to read next

| Document | What is in it |
|---|---|
| [docs/API.md](docs/API.md) | The HTTP API: routes, shocks, start states ("Today's US economy"), caching, fields filter |
| [docs/DESIGN.md](docs/DESIGN.md) | The model specification, module by module, with the literature behind each choice |
| [docs/CALIBRATION.md](docs/CALIBRATION.md) | How the parameters were chosen, the evidence for each step, and the gaps that remain |
| [docs/STYLIZED_FACTS.md](docs/STYLIZED_FACTS.md) | 208 empirical facts the model is scored against |
| [docs/SECTORS.md](docs/SECTORS.md) | The nine-sector calibration |
| [docs/SCENARIOS.md](docs/SCENARIOS.md) | The standard questions (oil, rates, QE, tariffs ...) with their measured answers |
| [docs/world/world_spec.md](docs/world/world_spec.md), [docs/world/DECISIONS.md](docs/world/DECISIONS.md) | The world block: specification and rulings |
| [data/README.md](data/README.md) | Every data file, its source and its terms |
| [scripts/](scripts/) | Calibration and check scripts; `scripts/profiles/` holds the configuration profiles |
| [NOTICE](NOTICE), [THIRD_PARTY.md](THIRD_PARTY.md) | Attributions and dependency licences |

## Cite

See [CITATION.cff](CITATION.cff). In short: Wittmann, T. (2026). *EconSim*, version 2026.10.23.2 (Apache-2.0). Describe the
results as a simulated economy, not a forecast.

## Licence

Apache-2.0, see [LICENSE](LICENSE). Copyright 2026 Tyson Wittmann. The data files keep the terms of their sources
([data/README.md](data/README.md)).

## Questions and challenges

Open an issue on this repository.
