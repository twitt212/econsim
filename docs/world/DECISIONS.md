# World spec review: orchestrator rulings (2026-10-12)

> **Note.** Rulings made during the development of the world block, kept as written. The review and verification notes they cite are not published. Roles named here (orchestrator, lead, implementer, reviewer) are the roles of an AI-assisted development process directed by the maintainer.

Review: the review note (not published) (Sonnet, adversarial). Verdict: ready after fixes. Every numbered problem is accepted and must be
fixed in the spec revision. The fixes the reviewer proposed are adopted unless a ruling below says otherwise.
The ten decisions the review left to the orchestrator are ruled as follows.

| # | Problem | Ruling |
|---|---|---|
| D-R1 | Baseline offset (2) | **Shift the input, keep the gate.** `ius_t = 100·(i_us_t − i_inf) − world_ius_offset`, where `world_ius_offset` is a fixed config constant (pp). Measure it once as the mean of `100·(i_us − i_inf)` over months 0–239 on the 12-seed master runs (expected ≈ −0.6), and record the script and value in the spec. It is not a per-run estimate. The system is linear, so this is equivalent to removing the steady state, at no cost. Keep W1-31 as G. Also add the `z~` centring gate from problem 12 (W2-15). |
| D-R2 | Forcing horizon (5) | **K = 240 for every forcing kernel**, not only the IS equation. Add the stage-1 test at persistence 0.99 with a permanent addend. Report the K=240 against K=480 error, with a stated tolerance (0.05 on any output). If it fails, add the geometric tail term `tail[e][out]·f_e[K−1]` and re-test. S1-d becomes "profile first; then reduce K only for the S and rule equations, re-running this test". |
| D-R3 | Oil variance (14) | **No extra activity-noise term in the integrated run**; bloc activity is the activity. Add `shocks.activity.{sigma,rho_y}` to `param_map` and `OilShocks` for the isolated `oil-check` only, drawn on `WORLD_OIL_S`. Split W3-05/W3-06 into isolated (data bands, G) and integrated rows. Integrated `oil_lvl` and VR12 keep the data bands as G. Pre-authorised levers: `rho_q` 0.995 and `sig_I` up to ×1.1. The integrated activity share is report-only. State the expected integrated `oil_lvl` (≈0.27, at the floor) before the run. |
| D-R4 | In-sample gates (22) | **Relabel as F** ("fit check": must pass, but is not evidence of validity). Use the reviewer's list as the starting point. The lead confirms each row against the calibration scripts. G is kept for the out-of-sample set. Gate reports show F and G in separate tables, and summary counts quote G only. |
| D-R5 | UsCredibility (17) | **Explicit field:** `UsCredibility{tp_bp, months, dollar_pct}`. `−dollar_pct/100` enters `q_t` as a premium term with the same time profile as the shock's term-premium component (held `months`, then the stated decay). Give a hand-value test. W4-04's band is unchanged. |
| D-R6 | Official flows (19) | **Fix the rule** (monthly change: `N_t = eta·100(ln D_t − ln D_{t−1})·nu_j·H`, test −83±3 at ln 1.10). **Demote W4-13 to report-only.** No noise term is added to hit a moment. |
| D-R7 | b7-transmission (20) | **M-F and M-X bind in world dollar mode.** Apply `bounded_rate_differential` to `ius/100` before it enters `fS` and `fR`. Write the exact `psi_step` call with every state update (`fx_psi_state`, `fx_psi_component`, `fx_psi_exog_prev`, `fx_mp_prev`, `burn_in`). Stage 4 starts only after b7-transmission has merged into joint3. Its first step re-reads the merged `fx.rs` and updates the call before coding. Add the M3 pin rows from `transmission_irf.py` to the stage-4 gate list. |
| D-R8 | ImportTariff (9) | **Set-level semantics**, matching `shocks.rs:250`: a separate `ImportTariff` level component, replaced and not added. Bilateral `Tariff{origin,…}` components add on top. Tau is a decimal everywhere. `cov` is defined per channel: household uses `cpi_tradable_weight`, firm uses `tradable_weight`. |
| D-R9 | W2-01 fallback (13) | **Profile, not defaults.** Import-elasticity changes ride in the world profile, so I1 is untouched. `legacy_b8off.json` resets them. |
| D-R10 | W4-02 band (18) | **Keep one AR(1)**, and add `world_fx_risk_rho` ∈ {0.90, 0.93, 0.95} to the levers. **The 6-month row becomes report-only**, as a stated limitation: the data path is hump-shaped (+2.9 / +3.6 / +1.0), and an AR(1) is monotone. Impact and 12-month rows stay G. S4-c cannot fire on the 6-month row. No band is lowered to fit. |

**Problems 1, 3, 4 and 8 (blockers or near-blockers)** are fixed as the reviewer proposed:
- the reference tooling (`gates`, `--inputs`, JSON-lines replay, defaults 0.93/0.95/0.98/0.97, P* passed through, stray file deleted, `exp_forcing_check.py` copied into `ref/`);
- `#[inline(never)]` on every new function called from an existing hot function, I1 after each hook with WIP commits, and a stop rule;
- `expected_rate_path` written out in all three branches, with a 360-length test. Prefer storing `update_curve`'s locals in a `Copy` struct on `w.monetary`, so the logic is not duplicated;
- H15 deleted, `import_rel` set under H6, and `fx_base` captured at the first world step.

**Problem 11:** recompute the trade hand values with the actual weights. Commit `scripts/world/trade_hand.py`, and have the unit tests call it. One set of numbers in the spec.

**Problem 12:** re-capture all bases at the first world step after the tick reset, and add W2-15.

**Problem 23:** every W5-04 row gets numeric [lo, hi] from the literature column before any run.

## Revision 2 (lead, 2026-10-12): accepted with these confirmations
- Isolated oil rows W3-01..W3-06 labelled F (they were fit on those bands): **confirmed**, consistent with D-R4.
- `ex[k]` carries the ius offset decaying at 0.97 a month (ex[0] = ius_t, ex[k] → 0): **accepted**; stage 1
  re-measures the offset (measured −0.40 pp, SE 0.09, on joint2-S6; not the ≈ −0.6 expected).
- Problem 12: the common component is the ABM's own real dollar relative to the re-captured base; the bloc price-level
  definition is dropped; W2-15 gates X/GDP and M/GDP within ±0.7 pp of master and `z~_j − c_t`: **accepted**.
- K = 240 alone failed the persistence test (JP 1.39 off); tail term with T = 720 (worst 0.022): **accepted**.
- Counts: G 29, F 40, R 15, E 16 (100 rows). New rows W1-32 (E), W2-15 (G), W3-19 (R), W4-28 (G).
- Next: a short verification pass of revision 2 (Sonnet) before stage 1; implementation starts after joint3.

## Verification of revision 2 (Sonnet, the revision-2 verification note (not published)): FIX FIRST, short list — rulings for revision 3
- Items 1–4, 6–8, 11 and the stage-1 guess list: fix as the verifier proposes.
- **Item 5 (M3 content):** M3 = master after joint calibration 3, which merges b7-transmission at its step 0 (keys off
  by default). So `pin_path_on`/`pinned_expected` exist at M3, and stage 1 implements all three branches of
  `expected_rate_path`. Keep a fallback sentence: if M3 lacks them, branch 3 moves to stage 4 step 0.
- **Item 9:** the W2-15 row and `targets.json` gate `z~_j − c_t`; `c_t` is reported (R). Correct open item 14 to the
  PT-based numbers (mean +2.87%, trend +0.15 %/yr on S6).
- **Item 10:** relabel W4-07, W4-08 and W4-15 as F; take W3-07 and W3-14 out of the Selection (keep G) or relabel them
  F if they stay as selection criteria (lead's choice; say which). W2-01 and W3-15 are reported F if their D13/D5 lever
  fires. **W1-31 is scored on holdout seeds 13–32 only** (the offset was measured on 1–12) and stays G.
- **Item 12 (Hormuz and US CPI m3 bands):** derive them from the literature column (Brent +45..65% etc.). If the
  prototype path sits outside, the row becomes R with that reason; do not centre bands on the model.
- **Item 13:** W4-02 impact and 12-month rows as F (D-R4 logic, gamma from the same regressions) are accepted. List this
  among the deviations. Fix "six lever groups" (five). W3-10: add a minimum-power note (report the SE; if
  2·SE > |effect|, the row is reported, not gated). State W3-15's expected first-run miss.

## Revision 3 (lead, 2026-10-12): accepted — READY FOR STAGE 1 (after joint calibration 3 lands as M3)
- All 13 VERIFY items and 18 guess-list entries resolved (section 1.9 schemas/types/accessors; placement table and
  `paths.py`/`make_layout.py`; new goldens irf_unit, pf_ar1_EA, kref, clean + stress replays). Reference checks in a
  scratch copy of the target layout: selftest OK, goldens --check max diff 0, kcheck identical, trade_hand byte-identical,
  gates 21 PASS (W1-28 REPORT).
- Lead's choice accepted: W3-07 and W3-14 stay G and are removed from the Selection and stop rules.
- Counts: G 26, F 43, R 16, E 16 (101 rows). Tests: S1 +37 (+1 ignored), S2 +28, S3 +25, S4 +32, S5 +6.
- Open by design: validation ranges for new shocks (lead's choices, in Notes); builder tools interface-only until
  stage 1; oil golden is stage 3; W3-09 and W4-03 borderline G.
- Stage 1 starts from M3 = master after joint3 (includes b7-transmission, keys off). Haiku implementer; Sonnet reviewer.

## Stage 1 implemented (Haiku), head 2a45faf on b8-world — all W1 rows pass; Sonnet review running
- world_ius_offset measured −0.431 (seeds 1–12; −0.452 at 32), used −0.43. I1–I4 hold; alt-codegen md5 equal; 332 core
  tests (+40 world, 1 ignored); overhead ≈2%.
- Rulings: (1) RAYON 8 checks as short one-off md5 runs are sufficient; long runs at RAYON ≤ 2 (CPU cap) accepted.
  (2) W1-21/22 statistic definitions to be confirmed by the reviewer against §4. (3) The W1-31 Rust vs reference gap
  (CA |y| 0.34 / |z| 1.58 vs 0.065 / 0.48) is carried into stage 2's centring work unless the reviewer finds a defect.
  (4) Interface deviations from §1.9 accepted for stage 1 if the reviewer finds no stage-2–4 blocker.
- Process note: one squashed WIP commit used `-c core.hooksPath=/dev/null` (no active hooks existed). Instructed: never
  bypass hooks again.

## Stage 1 review (Sonnet, the stage 1 review note (not published)): FIX (tooling and records; Rust/data/identity correct)
- F1 world_check.py ignored --households (ran 20k); F2 statistic was per-seed ratio, not the spec's mean-path ratio with
  leave-one-seed-out jackknife; F3 records misstate the W1-31 cause and the W1-21/22 settings; F4 DESIGN tick-order
  list lacks world steps, two docs point outside the repo.
- Under the spec statistic at 10k households, 16 seeds: W1-21 0.395 (SE 0.031), W1-22 0.433 (SE 0.034) — both PASS.
- W1-31 gap cause: the US output gap fed to the blocs (y_US = 200·(u* − u)) is permanently negative (≈ −0.91 mean on
  seeds 1–12) because mean u (4.87%) exceeds the capped real-time u* (≈4.34%).
- **Ruling N1:** centre y_US with a measured constant `world_yus_offset` (default = the seeds-1–12 mean of y_US over
  months 0–239, ≈ −0.91, measured the same way as `world_ius_offset`), applied where y_US enters the bloc forcing
  (net.rs:76). Done in the stage-1 fix round (world block only; block-off identity unaffected). Paired-difference rows
  (W1-21/22/27) are unaffected by a constant; W1-31 is re-measured (replay predicts 0.069 / 0.417).
- Recommended items taken into the fix round: W1-32 test at persistence 0.997; expected_rate_path tests with a zero
  floor and with rule_path_weight > 0; a Rust test scheduling a bloc shock through `w.shocks.schedule`; `world-dump`
  input validated before use.
