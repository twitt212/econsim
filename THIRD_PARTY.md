# Third-party software

EconSim is licensed under Apache-2.0. Its Rust dependencies (the full resolved tree in `Cargo.lock`, 95 packages other than EconSim's own three crates, all platforms) carry the licences below, as published on crates.io (read with `cargo metadata --format-version 1 --locked`; eight of the more unusual entries were also checked against their crates.io pages on 2026-10-18).

**Result.** Every dependency is under a permissive licence compatible with Apache-2.0: MIT, Apache-2.0, BSD-2-Clause, BSD-3-Clause, Unlicense, BSL-1.0, Unicode-3.0, or a choice among them. There is no GPL, LGPL, MPL, CDDL or other copyleft licence, and no package without a licence field. Where a package offers a choice (for example `MIT OR Apache-2.0`), EconSim uses it under either; where a package carries two licences joined by AND (`matchit`: MIT and BSD-3-Clause; `unicode-ident`: MIT or Apache-2.0, and Unicode-3.0 for its tables), both apply.

A binary built from this repository contains compiled code from these packages. When you distribute a binary, include the licence texts of the packages it contains (the MIT and BSD licences require their notices to be kept). `cargo about` or `cargo deny` can generate the notices from `Cargo.lock`.

The direct dependencies are: `serde`, `serde_json`, `rand`, `rand_pcg`, `rand_distr`, `rayon` (core); `axum`, `tokio`, `tower-http`, `sha2`, `futures-util` (server); `mimalloc` (server and CLI; it bundles the mimalloc allocator's C source, MIT).

## Licences by count

| Licence expression | Packages |
|---|---|
| MIT OR Apache-2.0 | 59 |
| MIT | 23 |
| Apache-2.0 OR MIT | 3 |
| MIT/Apache-2.0 | 2 |
| BSD-2-Clause OR Apache-2.0 OR MIT | 2 |
| MIT AND BSD-3-Clause | 1 |
| Unlicense OR MIT | 1 |
| Apache-2.0 OR BSL-1.0 | 1 |
| Apache-2.0 | 1 |
| (MIT OR Apache-2.0) AND Unicode-3.0 | 1 |
| Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | 1 |

## Packages

| Package | Version | Licence |
|---|---|---|
| async-trait | 0.1.92 | MIT OR Apache-2.0 |
| atomic-waker | 1.1.2 | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Apache-2.0 OR MIT |
| axum | 0.7.9 | MIT |
| axum-core | 0.4.5 | MIT |
| bitflags | 2.13.2 | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | MIT OR Apache-2.0 |
| bytes | 1.12.1 | MIT |
| cc | 1.6.0 | MIT OR Apache-2.0 |
| cfg-if | 1.0.5 | MIT OR Apache-2.0 |
| cpufeatures | 0.2.17 | MIT OR Apache-2.0 |
| crossbeam-deque | 0.8.8 | MIT OR Apache-2.0 |
| crossbeam-epoch | 0.9.21 | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.23 | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | MIT OR Apache-2.0 |
| digest | 0.10.7 | MIT OR Apache-2.0 |
| either | 1.19.0 | MIT OR Apache-2.0 |
| errno | 0.3.14 | MIT OR Apache-2.0 |
| find-msvc-tools | 0.1.14 | MIT OR Apache-2.0 |
| form_urlencoded | 1.2.2 | MIT OR Apache-2.0 |
| futures-core | 0.3.34 | MIT OR Apache-2.0 |
| futures-task | 0.3.34 | MIT OR Apache-2.0 |
| futures-util | 0.3.34 | MIT OR Apache-2.0 |
| generic-array | 0.14.7 | MIT |
| getrandom | 0.2.17 | MIT OR Apache-2.0 |
| http | 1.5.0 | MIT OR Apache-2.0 |
| http-body | 1.1.0 | MIT |
| http-body-util | 0.1.5 | MIT |
| httparse | 1.10.1 | MIT OR Apache-2.0 |
| httpdate | 1.0.3 | MIT OR Apache-2.0 |
| hyper | 1.12.0 | MIT |
| hyper-util | 0.1.21 | MIT |
| itoa | 1.0.18 | MIT OR Apache-2.0 |
| libc | 0.2.190 | MIT OR Apache-2.0 |
| libm | 0.2.16 | MIT |
| libmimalloc-sys | 0.1.49 | MIT |
| lock_api | 0.4.14 | MIT OR Apache-2.0 |
| log | 0.4.34 | MIT OR Apache-2.0 |
| matchit | 0.7.3 | MIT AND BSD-3-Clause |
| memchr | 2.8.3 | Unlicense OR MIT |
| mimalloc | 0.1.43 | MIT |
| mime | 0.3.17 | MIT OR Apache-2.0 |
| mio | 1.2.4 | MIT |
| num-traits | 0.2.19 | MIT OR Apache-2.0 |
| once_cell | 1.21.4 | MIT OR Apache-2.0 |
| parking_lot | 0.12.5 | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | MIT OR Apache-2.0 |
| percent-encoding | 2.3.2 | MIT OR Apache-2.0 |
| pin-project-lite | 0.2.17 | Apache-2.0 OR MIT |
| ppv-lite86 | 0.2.21 | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | MIT OR Apache-2.0 |
| quote | 1.0.47 | MIT OR Apache-2.0 |
| rand | 0.8.8 | MIT OR Apache-2.0 |
| rand_chacha | 0.3.1 | MIT OR Apache-2.0 |
| rand_core | 0.6.4 | MIT OR Apache-2.0 |
| rand_distr | 0.4.3 | MIT OR Apache-2.0 |
| rand_pcg | 0.3.1 | MIT OR Apache-2.0 |
| rayon | 1.12.0 | MIT OR Apache-2.0 |
| rayon-core | 1.13.0 | MIT OR Apache-2.0 |
| redox_syscall | 0.5.18 | MIT |
| rustversion | 1.0.23 | MIT OR Apache-2.0 |
| ryu | 1.0.23 | Apache-2.0 OR BSL-1.0 |
| scopeguard | 1.2.0 | MIT OR Apache-2.0 |
| serde | 1.0.229 | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | MIT OR Apache-2.0 |
| serde_path_to_error | 0.1.20 | MIT OR Apache-2.0 |
| serde_urlencoded | 0.7.1 | MIT/Apache-2.0 |
| sha2 | 0.10.9 | MIT OR Apache-2.0 |
| shlex | 2.0.1 | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | MIT OR Apache-2.0 |
| slab | 0.4.12 | MIT |
| smallvec | 1.16.2 | MIT OR Apache-2.0 |
| socket2 | 0.6.5 | MIT OR Apache-2.0 |
| syn | 2.0.119 | MIT OR Apache-2.0 |
| syn | 3.0.6 | MIT OR Apache-2.0 |
| sync_wrapper | 1.0.2 | Apache-2.0 |
| tokio | 1.53.2 | MIT |
| tokio-macros | 2.7.2 | MIT |
| tower | 0.5.3 | MIT |
| tower-http | 0.5.2 | MIT |
| tower-layer | 0.3.3 | MIT |
| tower-service | 0.3.3 | MIT |
| tracing | 0.1.44 | MIT |
| tracing-core | 0.1.36 | MIT |
| typenum | 1.20.1 | MIT OR Apache-2.0 |
| unicode-ident | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 |
| version_check | 0.9.5 | MIT/Apache-2.0 |
| wasi | 0.11.1+wasi-snapshot-preview1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| windows-link | 0.2.1 | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | MIT OR Apache-2.0 |
| zerocopy | 0.8.62 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zerocopy-derive | 0.8.62 | BSD-2-Clause OR Apache-2.0 OR MIT |
| zmij | 1.0.23 | MIT |
