# b4-rent: housing rental side (notes)

Branch `b4-rent`, from master 2e966a5. Target: re-fit rule-6 misses H5 (policy +100bp → HVS rental vacancy must not
rise at m3–24) and H2 (term-premium +100bp → median house price −5 to −10% at m24).

## Stage 0: plumbing, no behaviour change (recorded)

- All b4-rent fields are off by default (`config.rs`, `// b4-rent` block). θ stays 0.75.
- Run md5 (`econsim-cli run 5000 60 3 --json`, last line): `7ee60bad26a26c2494703d020b216ced` at RAYON_NUM_THREADS 1
  and 8, equal to master. Same md5 with `scripts/profiles/b4-rent-off.json` applied through ECONSIM_CFG.
- `hvs 10000 36 16` reproduces the archived `final_hvs16` run exactly (policy HVS +0.002 / +0.075 / +0.151 at m12/18/24).
- `hvs 10000 36 16 flows` (off), policy m24 row values: renter→owner −1.32%, VFR + tenant bought −3.50, VFR + expiry
  let +5.06, VFR put up for sale (outflow) −4.81, evictions +12.7. These match spec §1.1 (+5.1, −4.8, −3.5, +3.6).
  Note: `new leases` in the flow block counts all leases (+6.25 at m24), not only VFR→RO (spec table: −5.7).
- `hcheck 4 20000 240`: 0 failures. Debug run `run 2000 24 1` completes; `cargo clean --profile dev` run.
- Unit tests: 208 pass (`cargo test --release -p econsim-core`), 4 new b4-rent tests.
- irf_check report-only rows H5t, H7, H8, H2i added (status R, no gate changed). Off-state H5t +0.053, H7 −1.31,
  H8 −0.051, H2i −5.24.

## Mechanisms (implemented, all off by default)

- M-T `housing_buy_rate_semi` (β_b): first-time buyer hazard × exp(−β_b·d), d = pp of the 6-month mortgage-rate average
  above its reference (`buy_rate_mult`, bounded [0.5, 1.5]).
- M-V `housing_vacant_sell_reanchor` + `housing_vacant_sell_ms_elast` (η): vacant-rental sale hazard re-anchored at the
  120-month months-of-supply reference (`vacant_sell_prob`).
- M-G(a) `housing_rental_gap_sell_semi` (κ_s): sale hazard × exp(κ_s·100·(v₋₁ − v̄)).
- M-G(b) `housing_expiry_let_gap_semi` (κ_e): expiring unlet vacant listings are let with p_let = exp(−κ_e·100·(v₋₁−v̄))
  (floor 0.25, one-sided), else re-listed; draws from the new PH_EXPIRE = 11 phase of `stream::HOUSING`
  (not a new stream in `rng.rs`; see the spec deviation note below).
- M-θ: `housing_uc_theta` 0.75 → 1.0 is a profile-level change only (Stage C).

Spec deviation to report: the expiry routing draw uses the `PH_EXPIRE` tag on the existing housing stream (spec §0,
allowed). It is drawn only when κ_e > 0 and live, so the off-state draw sequence is unchanged.

## Measurement

(Recorded below as the stages complete.)

## Screen results (measured; all cells use `hvs 10000 36 16 flows`, `irf 10000 36 12`, theta 0.75 unless stated)

Gates: H5 at 16 seeds is max ≤ +0.05 and mean(m12/18/24) ≤ +0.02; at 48 seeds the same bound governs (spec §8).

| Cell | β_b | M-V η | κ_s | κ_e | H5 16s max / mean | H5 48s max / mean | H2 12s | Other gated rows at 12s |
|---|---|---|---|---|---|---|---|---|
| A_b00_e2 | 0 | 2 | 0 | 0 | +0.038 / +0.007 | +0.096 / +0.053 FAIL | −5.02 (48s −4.91 FAIL) | none |
| A_b00_e1 | 0 | 1 | 0 | 0 | +0.109 / +0.063 FAIL at 16 | – | −5.18 | none |
| A_b01_leg | 0.1 | – | 0 | 0 | +0.113 / +0.060 FAIL at 16 | – | −5.15 | none |
| A_b01_e2 | 0.1 | 2 | 0 | 0 | +0.036 / +0.004 | +0.081 / +0.046 FAIL | −5.13 | P7 0.66 (master 0.55) |
| A_b01_e1 | 0.1 | 1 | 0 | 0 | +0.037 / +0.011 | +0.125 / +0.050 FAIL | −5.38 | none |
| A_b02_leg | 0.2 | – | 0 | 0 | +0.147 / +0.101 FAIL | – | −5.10 | P7 1.08 |
| A_b02_e1 | 0.2 | 1 | 0 | 0 | +0.128 / +0.049 FAIL | – | −5.31 | none |
| A_b02_e2 | 0.2 | 2 | 0 | 0 | +0.109 / +0.055 FAIL | – | −5.22 | P7 0.71 |
| A_b03_leg | 0.3 | – | 0 | 0 | +0.078 / +0.034 FAIL | – | −5.27 | P7 0.82 |
| A_b03_e1 | 0.3 | 1 | 0 | 0 | +0.067 / −0.007 FAIL (max) | – | −5.70 | P7 0.65 |
| A_b03_e2 | 0.3 | 2 | 0 | 0 | +0.154 / +0.031 FAIL | – | −5.28 | none |
| B_ks01 | 0.1 | 2 | 0.1 | 0 | +0.045 / +0.028 FAIL (mean) | – | −5.15 | P7 0.70 |
| B_ks02 | 0.1 | 2 | 0.2 | 0 | +0.068 / +0.032 FAIL | – | −5.15 | none |
| B_ke025 | 0.1 | 2 | 0 | 0.25 | +0.085 / +0.042 FAIL | – | −5.36 | none |
| B_ke05 | 0.1 | 2 | 0 | 0.5 | +0.067 / +0.008 FAIL (max) | – | −5.42 | none |
| B_b04_e2 | 0.4 | 2 | 0 | 0 | +0.034 / −0.006 | **+0.059 / +0.017 FAIL (max by 0.009)** | −5.48 | none (P7 passes) |

Stage C (θ 1.0) was run on two cells, not one as first recorded:
- A_b00_e2 + θ 1.0: H2 12s −6.07, P7 0.38, H3 pass. 16 seeds H5 +0.040 / +0.020, 48 seeds +0.078 / +0.043 (fails at 48).
- B_b04_e2 + θ 1.0 (β_b 0.4, η 2; measured in review, `irf 10000 36 12`, `hvs 10000 36 48`, `hirf 20000 36 16`):
  H5 +0.044 / −0.002 at 48 seeds (16 seeds max +0.070), H2 −6.51, H2b −5.97, H3 −0.04 / −0.49, P7 −0.29.
  No failing gated IRF row. At 16 seeds × 20k × 240, paired vs the archived re-fit runs: hard bands pass (policy 2.267,
  u 4.81, core PCE 1.865, HP GDP sd 1.42); per-seed rows match master (one seed below 1.0 at 0.99; 9 of 16 seeds
  with ≤6 months u<2.5); hbust 4.62 / 4.35; hcheck 4 seeds 0 failures. FAILS: foreclosures 253.4k (+12.0 ± 6.4,
  above the 250k cap); worst real drawdown −25.4 vs −18.9 master (6.5 pp deeper, above the 5 pp guard; seeds 3 and 5
  are 8.1 and 9.5 pp deeper); hpa_sd +0.93.
So θ 1.0 fixes the IRFs for the B cell, but costs foreclosures and drawdowns. It is not adoptable as measured.
Saved as `scripts/profiles/b4-rent-theta.json` (β_b 0.4, η 2, θ 1.0) with this measured on-state.

Dias–Duarte signs at m24 for B_b04_e2 (48 seeds): homeownership −0.131 pp (<0, pass), renter→owner −3.35% (<0, pass),
put-up-for-sale outflow −9.60 (≤0, pass), expiry let +4.90 (≥0, pass). The signs are right; the level of H5 is not.

Reference, master (48 seeds, spec): H5 +0.116 / +0.051. The B_b04_e2 on-state reduces the mean from +0.051 to +0.017
and the max from +0.116 to +0.059, but it does not clear the +0.05 max bound.

## Decision

No candidate passes H5 at 48 seeds for the IRF gates alone (spec §8, "No H5 pass" branch). Ship OFF:
- Defaults stay off (all b4-rent fields off, θ 0.75). Off-state bit-identical to master (md5 `7ee60bad…`).
- `scripts/profiles/b4-rent.json` = B_b04_e2 (β_b 0.4, M-V η 2, θ 0.75). Its measured on-state FAILS H2b
  (hirf 20k × 16: −4.85 vs the −5 to −10 target; master −4.57). The on-state is recorded here as measured.
- `scripts/profiles/b4-rent-theta.json` = B_b04_e2 + θ 1.0. Passes every gated IRF row at the governing seed counts
  (H5 +0.044 / −0.002 at 48 seeds; H2 −6.51; H2b −5.97), but fails the foreclosure cap (253.4k vs 250k) and the
  real-drawdown guard (6.5 pp deeper vs 5 pp). Not adoptable as measured.
- `scripts/profiles/b4-rent-off.json` = all off, θ 0.75.

Corrections to the earlier record: H2 was not "not the binding gate", since the recorded on-state fails H2b. θ 1.0 was
not tried only on A_b00_e2; it was also tried on B_b04_e2 (above). θ 1.0 does fix the IRFs for the B cell, but its
long-run costs rule it out as measured. The general claim "θ did not rescue H5" held only for A_b00_e2.

Not run (no defaults changed, so Stage D/E/F and the full standard battery for default change are not required): the
16-seed scorecard, paired.py vs master on the recorded on-state, and the per-seed checks on the recorded on-state (the
theta cell's were run in review, see above). Stage E/F need a default change to ship.

Open issues for the orchestrator:
- The B_b04_e2 on-state fails H2b (−4.85) and H5 at 48 seeds by 0.009. The θ 1.0 variant fixes both IRFs but breaches
  the foreclosure cap and the drawdown guard. A follow-up needs a tenure/H2 mechanism that moves H2b without θ, or a
  smaller θ (e.g. 0.9, untested), or a larger β_b with a foreclosure check. Raising β_b alone was not tested above 0.4.
- P7 (GDP overshoot / |P1 trough|) already fails at master (0.55 at 12 seeds). Several on-states move it both ways
  (0.27–1.08). It is not caused by b4-rent alone and is not a gate this branch is meant to change.
- Spec deviation: `PH_EXPIRE` draws on the existing `stream::HOUSING` with a new phase tag, not a new stream in
  `rng.rs` (spec §0 allows this, and it draws only when κ_e > 0 and live).
- `new leases` flow row counts all leases (+6.25 at policy m24), not only VFR→RO (spec table −5.7). Report-only.
