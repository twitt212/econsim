//! Central bank, yield curve and financial markets (docs/DESIGN.md §11.4).

use rayon::prelude::*;

use crate::common;
use crate::ledger;
use crate::types::*;

// Moved out of this module (zero behaviour change): the equity valuation lives in `equity`, the exchange rate in `fx`.
// The re-exports keep the paths `fx::exchange_rate`, `monetary::fx_base` and `monetary::trailing_equity_return`.
pub use crate::equity::trailing_equity_return;
pub use crate::fx::{capture_fx_base, exchange_rate, fx_base, FxBase};

/// Policy-rate rounding step (0.25%), §11.4.
const RATE_STEP: f64 = 0.0025;
/// Monthly decay of the expected short-rate gap to the long-run rate (0.97^k), §11.4.
const PATH_DECAY: f64 = 0.97;
/// Term-premium slope by tenor (3m, 2y, 5y, 10y, 30y), §11.4.
pub(crate) const TP_SLOPE: [f64; N_TENORS] = [0.0, 0.2, 0.5, 1.0, 1.3];
/// fix-monetary: saturation of the policy gap's term-premium response (decimal; the response is tanh-bounded at ±3pp).
const TP_GAP_CAP: f64 = 0.03;
/// Floor on any tenor's yield (annual decimal), §11.4.
const YIELD_FLOOR: f64 = -0.005;
/// Fund deposit band as a share of NAV: refill below MIN, sweep above MAX, aim at TARGET (§11.4).
const FUND_DEP_MIN: f64 = 0.02;
const FUND_DEP_TARGET: f64 = 0.05;
const FUND_DEP_MAX: f64 = 0.08;
/// RoW deposit buffer above 1/12 of its bond holdings, in months of GDP (calibration choice).
const ROW_BUFFER_GDP_MONTHS: f64 = 0.5;
/// Tolerance for face and money amounts.
pub(crate) const EPS: f64 = 1e-9;
/// Households per parallel chunk in the Fund income distribution (fixed, so results are thread-count independent).
const FUND_CHUNK: usize = 1024;

/// b8-world H5: the locals of `update_curve` that `expected_rate_path` needs (spec 2.1). Stored every month.
#[derive(Clone, Copy, Debug, Default)]
pub struct CurveInputs {
    /// True on the rule path (no override in force): `floored_expected` applies.
    pub gap_on: bool,
    /// The rule level, rounded as the policy rate, less the gap in force (`base` of update_curve).
    pub base: f64,
    /// The priced target `i_star` (the blend of base and the smoothed rule target).
    pub i_star: f64,
    /// Long-run anchor r* + pi^LR.
    pub i_inf: f64,
    /// Months of an active override (zero when none).
    pub flat: f64,
}

/// Monetary-module state (§11.4). Shared structs must not gain fields; extra state lives here.
#[derive(Clone, Debug, Default)]
pub struct MonetaryState {
    /// Unrounded smoothed shadow policy rate. None until set_policy has run.
    pub shadow_rate: Option<f64>,
    /// First observed private-duration ratio D0 (term-premium reference). None until observed.
    pub d0: Option<f64>,
    /// Real-time natural unemployment estimate (decimal; 0 until first set).
    pub u_star: f64,
    /// Real-time natural real rate estimate (None until the rule first adapts it).
    pub r_star_est: Option<f64>,
    /// Mean household expected inflation used by the last set_policy call.
    pub pi_e: f64,
    /// Central bank's target holdings (face). Set to its holdings at the first markets() call, grown with
    /// trend nominal GDP, moved by QE/QT shocks, and reduced by runoff during QT.
    pub cb_target_face: f64,
    /// True once `cb_target_face` has been initialised.
    pub cb_target_init: bool,
    /// Face bought this month to restore holdings to target (reinvestment, not shock QE). Informational.
    pub reinvestment_this_month: f64,
    /// True once the equity smoothing state has been seeded (first markets() call).
    pub equity_init: bool,
    /// t=0 reference values for the exchange-rate channel (set on the first `exchange_rate` call).
    pub fx_base: Option<FxBase>,
    /// Monthly total returns on the Fund's equity (capital gain plus equity dividends distributed), most recent last,
    /// capped at 12 entries. Feeds the extrapolative term of the equity risk premium.
    pub equity_returns: Vec<f64>,
    /// Equity risk premium used for the current month's valuation (decimal; 0 before the first valuation).
    pub erp_current: f64,
    /// Long rate equity investors discount at: the 10-year yield smoothed over `equity_rate_months` (None until set).
    pub equity_long_rate: Option<f64>,
    // fix-monetary
    /// Central-bank agency-MBS holdings from MbsQE shocks, as a share of annual GDP (cumulative; no runoff).
    pub mbs_stock_gdp: f64,
    /// Mortgage-spread change from central-bank MBS holdings (decimal, ≤ 0 when the bank holds MBS). Set in `markets`
    /// from `Config::mbs_spread_per_gdp`; for the banking module to add to the mortgage rate (not yet wired in).
    pub mbs_spread_adj: f64,
    /// Normal Tobin's q per sector (raw, sector equity ÷ replacement value): a 12-month-half-life average of non-boom
    /// valuations, frozen during booms. The reference that init::build_world rebases sector_q_rel to at the end of
    /// burn-in, so a boom's valuation is not rebased into "normal".
    pub normal_q: [f64; N_SECTORS],
    // end fix-monetary
    // p1-curve
    /// Unsmoothed target rate i*_t of the rule this month (decimal; None until set_policy has run). Reported in the
    /// metrics; the curve prices the smoothed target `i_star_smooth` instead.
    pub i_star: Option<f64>,
    /// EWMA of the rule target i*, with gain `Config::i_star_gain` (decimal). The rule's projected path the curve prices
    /// (update_curve) moves toward this, so the month-to-month noise of u does not reach the long end.
    pub i_star_smooth: Option<f64>,
    /// Long-run expected inflation π^LR (decimal): the curve's long-run anchor is r*_est + π^LR. Starts at the target and
    /// moves on core-inflation surprises (`Config::lr_infl_gain`).
    pub pi_lr: Option<f64>,
    /// Unemployment rate over the last seven months, current month last (at most 7 entries), for the speed-limit term.
    pub u_hist: Vec<f64>,
    // end p1-curve
    // b7-transmission
    /// QE lots still held by the central bank from bought purchases: (maturity tick, face). Aged implicitly by the
    /// maturity tick (matured lots are dropped at the start of `markets`), and scaled down by QT sales. Feeds the
    /// scarcity term of the priced duration (`Config::qe_duration_weight`); it does not affect anything at weight 1.
    pub qe_lots: Vec<(u32, f64)>,
    /// b7-transmission M-X: last month's monetary-surprise premium z_{t−1} (log points), added to the dollar's premium
    /// state by `fx::exchange_rate`. Zero whenever `Config::fx_mp_premium` is zero.
    pub fx_mp_prev: f64,
    /// b8-world H5: the curve locals of this month (`record_curve_inputs`), read by `expected_rate_path`.
    pub curve_inputs: CurveInputs,
    /// b7-transmission M-E: reference level of the 2y real rate (EWMA with gain 1/`firm_stance_months`), None until the
    /// first curve update. Computed always; read only when `Config::firm_stance_weight` is non-zero.
    pub stance_ref: Option<f64>,
    /// b7-transmission M-E: policy stance S_t (decimal): the 2y real rate less its reference, before this month's
    /// update. Feeds firms' expectations through `firm_expected_inflation`.
    pub firm_stance: f64,
}

/// Price per 1 face of a bond with annual `coupon` (paid monthly at coupon/12), discounted at annual
/// `yield_` (monthly rate yield/12) over `months` remaining months. Par when coupon == yield (§11.4).
/// Returns par (1.0) for zero remaining months or invalid inputs.
pub fn bond_price(coupon: f64, yield_: f64, months: u32) -> f64 {
    if months == 0 {
        return 1.0;
    }
    let r = yield_ / 12.0;
    if !(1.0 + r > 0.0) || !coupon.is_finite() {
        return 1.0;
    }
    let n = months as f64;
    let v = (1.0 + r).powf(-n);
    let annuity = if r.abs() < 1e-12 { n } else { (1.0 - v) / r };
    coupon / 12.0 * annuity + v
}

/// Months until bond issue `b` matures (0 once due). §11.4.
pub fn remaining_months(w: &World, b: &BondIssue) -> u32 {
    b.maturity_tick.saturating_sub(w.tick)
}

/// Σ face × remaining years over government bonds held outside the central bank (face-years).
/// Divide by 12 × `common::gdp_month_model` to get the scale-free duration ratio D of §11.4.
pub fn private_duration(w: &World) -> f64 {
    let mut d = 0.0;
    for b in &w.bonds {
        let years = remaining_months(w, b) as f64 / 12.0;
        for (h, face) in &b.holdings {
            if *h != BondHolder::CentralBank {
                d += face * years;
            }
        }
    }
    d
}

/// Scale-free private duration D = private_duration / (12 × monthly GDP), guarded against zero GDP.
fn duration_ratio_raw(w: &World) -> f64 {
    let denom = 12.0 * common::gdp_month_model(w);
    if !(denom > 0.0) || !denom.is_finite() {
        return 0.0;
    }
    let d = private_duration(w) / denom;
    if d.is_finite() {
        d
    } else {
        0.0
    }
}

/// b7-transmission M-Q: H, the scale-free duration of the QE lots the central bank holds (face × remaining years, over
/// 12 × monthly GDP, the same scaling as `duration_ratio_raw`). Zero when there are no lots or GDP is not positive.
fn lot_duration_ratio(w: &World) -> f64 {
    let denom = 12.0 * common::gdp_month_model(w);
    if !(denom > 0.0) || !denom.is_finite() {
        return 0.0;
    }
    let tick = w.tick;
    let h: f64 = w
        .monetary
        .qe_lots
        .iter()
        .map(|&(mat, face)| face * mat.saturating_sub(tick) as f64 / 12.0)
        .sum();
    let d = h / denom;
    if d.is_finite() {
        d
    } else {
        0.0
    }
}

/// Duration ratio priced by the term premium: the raw ratio less the QE purchases announced but not yet executed
/// (weighted by `qe_announcement_weight`), so part of a programme's stock effect is priced when it is announced
/// (Gagnon et al. 2011) rather than as each purchase is made. Equals the raw ratio when no programme is announced.
/// fix-monetary.
fn duration_ratio(w: &World) -> f64 {
    let raw = duration_ratio_raw(w);
    let announced = w.cfg.qe_announcement_weight * w.shocks.qe_announced_duration(w.tick);
    // b7-transmission M-Q: λ = 1 keeps the legacy expression exactly. Otherwise the central bank's held lots count
    // λ times: d = raw − w_a·A − (λ − 1)·(H + w_a·A), with H the duration ratio of the held lots.
    let lambda = w.cfg.qe_duration_weight;
    let mut d = raw - announced;
    if lambda != 1.0 {
        d -= (lambda - 1.0) * (lot_duration_ratio(w) + announced);
    }
    if d.is_finite() {
        d.max(1e-6)
    } else {
        0.0
    }
}

/// Mean expected inflation over alive households (§11.4 π_e). Falls back to the target if none alive.
fn mean_expected_inflation(w: &World) -> f64 {
    // Fixed-size chunks reduced in chunk order, so the sum does not depend on the thread count.
    let (sum, n) = w
        .households
        .par_chunks(8192)
        .map(|c| {
            c.iter().filter(|h| h.alive).fold((0.0, 0usize), |(s, n), h| (s + h.expected_inflation, n + 1))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .fold((0.0, 0usize), |(s, n), (a, b)| (s + a, n + b));
    if n == 0 {
        return w.cfg.inflation_target;
    }
    let m = sum / n as f64;
    if m.is_finite() {
        m
    } else {
        w.cfg.inflation_target
    }
}

/// Price per face of issue `idx` at the current curve, at its remaining maturity.
pub(crate) fn issue_price(w: &World, idx: usize) -> f64 {
    let b = &w.bonds[idx];
    let rem = remaining_months(w, b);
    bond_price(b.coupon, w.curve.yield_at(rem as f64), rem)
}

/// Holders other than the central bank: Fund, RestOfWorld, then each bank in order.
fn private_holders(nb: usize) -> Vec<BondHolder> {
    let mut v = vec![BondHolder::Fund, BondHolder::RestOfWorld];
    v.extend((0..nb).map(|b| BondHolder::Bank(b as BankId)));
    v
}

/// Taylor rule or override; set helicopter amount from shocks (§11.4, §16).
///
/// i* = r* + π_e + φπ(π − π*) + φu·2·(u* − u); the shadow rate is smoothed with `rate_smoothing` and the
/// policy rate is that shadow rounded to 0.25% and floored at zero. The unrounded shadow is kept in
/// `MonetaryState` so rounding does not stall convergence. A policy-rate override pins both.
/// Monthly adaptation of the natural-unemployment estimate (≈11-year half-life; Laubach–Williams-style estimates
/// move slowly).
const U_STAR_ADAPT: f64 = 0.005;

/// Monthly re-anchoring of the duration-supply reference (≈6-year half-life).
const DURATION_ANCHOR_ADJ: f64 = 0.0096;
/// Term premium per unit of |expected inflation − target| (inflation-risk premium), scaled by tenor slope.
const INFLATION_RISK_PREMIUM: f64 = 0.1;

/// Bounds on the real-time natural-unemployment estimate: `Config::u_star_min` (re-fit Step 11b; was the constant 0.038) and
/// `Config::u_star_max`. CBO's NAIRU is ≈4.3% and real-time estimates have stayed within ≈4–5%; without bounds the estimate
/// follows unemployment and the rule loses its anchor against persistent slack.
/// Upper bound on the natural-unemployment estimate: `Config::u_star_max` (round 7 step 10a; 0.044 is the CBO noncyclical
/// rate for 2026, which the equilibrium u sits about 0.2pp above).

/// Upper bound on the adaptive r* estimate. The lower bound is `Config::r_star_est_min` (re-fit Step 3), which
/// `Config::validate_clamp_bounds` keeps at or below this.
pub const R_STAR_EST_MAX: f64 = 0.03;
/// Policy rate (decimal) at or below which the r* anti-windup switch (`Config::r_star_antiwindup`) skips negative-gap
/// integration.
const R_STAR_ZLB_RATE: f64 = 0.0025;

/// The natural real rate the rule and the curve use: a Taylor-rule shock's value, else the adaptive estimate, else
/// the configured value.
pub fn current_r_star(w: &World) -> f64 {
    match w.shocks.taylor {
        Some(t) => t.2,
        None => w.monetary.r_star_est.unwrap_or(w.cfg.r_star),
    }
}

/// Re-fit Step 3: one month of the adaptive r* integrator. Returns the new estimate, or `None` when the estimate is left
/// where it is: the anti-windup switch skips a negative gap at the zero bound (below-target inflation there is not
/// evidence of a lower neutral rate, since the rule cannot cut further), or the result is not finite.
pub fn r_star_integrate(cfg: &crate::config::Config, prev: f64, gap: f64, policy_rate: f64) -> Option<f64> {
    if cfg.r_star_antiwindup && gap < 0.0 && policy_rate <= R_STAR_ZLB_RATE {
        return None;
    }
    let next = prev + cfg.r_star_adapt / 12.0 * gap;
    next.is_finite().then(|| next.clamp(cfg.r_star_est_min, R_STAR_EST_MAX))
}

/// rz21: the inflation the rule reads once the first round of a tariff is looked through. `pi` is the index's 12-month inflation (a decimal);
/// `d` the log change over the last 12 months of the index over its constant-wedge twin (`Prices::rule_wedge_ln`), so the index without the
/// wedge's rise grows at `(1 + pi) e^(-d) - 1`. The rule reads `pi - lt (pi - pi_xt)`. Identity (the same bits) with `lt` = 0, with fewer than
/// 13 months of series, or with `d` = 0 (no tariff wedge moved in the year); a non-finite `d` leaves `pi` where it was.
pub fn lookthrough_inflation(lt: f64, pi: f64, d: f64) -> f64 {
    if !(lt > 0.0) || d == 0.0 || !d.is_finite() {
        return pi;
    }
    let pi_xt = (1.0 + pi) * (-d).exp() - 1.0;
    let out = pi - lt.min(1.0) * (pi - pi_xt);
    if out.is_finite() {
        out
    } else {
        pi
    }
}

/// rz21: `lookthrough_inflation` for the world's own series.
fn rule_inflation(w: &World, pi: f64) -> f64 {
    let h = &w.prices.rule_wedge_ln;
    if !(w.cfg.rule_lookthrough > 0.0) || h.len() < 13 {
        return pi;
    }
    lookthrough_inflation(w.cfg.rule_lookthrough, pi, h[h.len() - 1] - h[h.len() - 13])
}

pub fn set_policy(w: &mut World) {
    let target = w.cfg.inflation_target;
    let pi_e = mean_expected_inflation(w);
    w.monetary.pi_e = pi_e;
    let (phi_pi, phi_u) = w.shocks.taylor.map_or((w.cfg.phi_pi, w.cfg.phi_u), |t| (t.0, t.1));

    // First month of a new world only (not the first reported month after burn-in, when the tick is reset).
    if w.tick < 1 && w.monetary.shadow_rate.is_none() {
        // fix-monetary: a policy innovation scheduled at the first month still moves the rate (its gap is then known
        // to update_curve through policy_gap, so the two must agree).
        let innov = w.shocks.policy_innovation(w.tick, w.cfg.rate_smoothing);
        let s = w.cfg.initial_policy_rate + innov;
        w.monetary.shadow_rate = Some(s);
        w.cb.policy_rate = ((s / RATE_STEP).round() * RATE_STEP).max(0.0);
        // p1-curve: the rule's target excludes the policy innovation (a shock, priced separately by update_curve).
        w.monetary.i_star = Some(w.cfg.initial_policy_rate);
    } else {
        let pi = if w.cfg.rule_uses_core {
            if w.cfg.rule_uses_pce {
                w.core_pce_inflation_yoy()
            } else {
                w.core_inflation_yoy()
            }
        } else {
            w.inflation_yoy()
        };
        // rz21: the inflation the rule reads, with the first round of a tariff looked through (identity with `rule_lookthrough` = 0).
        let pi_rule = rule_inflation(w, pi);
        let u = w.unemployment_rate();
        // Real-time natural-rate estimate adapts slowly to observed unemployment (≈3-year half-life), as policymakers'
        // NAIRU estimates do; wage setters use the same estimate (firms::plan).
        let prev_u_star = if w.monetary.u_star > 0.0 { w.monetary.u_star } else { w.cfg.natural_unemployment };
        w.monetary.u_star = (prev_u_star + U_STAR_ADAPT * (u - prev_u_star)).clamp(w.cfg.u_star_min, w.cfg.u_star_max);
        // p1-curve: long-run inflation expectations move slowly on core-inflation surprises (the curve's long-run anchor).
        // Non-finite inputs leave the state where it was (a NaN here would stick in the EWMA and reach every yield).
        let prev_pi_lr = w.monetary.pi_lr.unwrap_or(target);
        let pi_lr = prev_pi_lr + w.cfg.lr_infl_gain * (pi - prev_pi_lr);
        if pi_lr.is_finite() {
            w.monetary.pi_lr = Some(pi_lr.clamp(target - 0.01, target + 0.02));
        }
        // p1-curve: seven months of unemployment (the current month and six before it) for the speed-limit term.
        if u.is_finite() {
            w.monetary.u_hist.push(u);
            if w.monetary.u_hist.len() > 7 {
                w.monetary.u_hist.remove(0);
            }
        }
        // Taylor (1993): i = r* + π + φπ(π − π*) + φu·Okun(u* − u), with π the realised (core) inflation, so the
        // nominal rate moves more than one-for-one with inflation (the Taylor principle).
        let _ = pi_e;
        // The estimate adapts only while the rule is in charge (not under a rate override or a rule change).
        let rule_active = w.shocks.taylor.is_none() && w.shocks.policy_rate_override(w.tick).is_none();
        let gap = pi_rule - target;
        if w.cfg.r_star_adapt > 0.0 && rule_active {
            let prev = w.monetary.r_star_est.unwrap_or(w.cfg.r_star);
            if let Some(next) = r_star_integrate(&w.cfg, prev, gap, w.cb.policy_rate) {
                w.monetary.r_star_est = Some(next);
            }
        }
        let r_star = current_r_star(w);
        // p1-curve: speed-limit term (Orphanides & Williams 2002) on a smoothed gap: the three-month average of u against
        // its level six months earlier. A rise in unemployment over that window lowers the target by
        // `speed_limit_weight` per unit of change. The single-month noise in u does not enter it.
        let speed_limit = if w.monetary.u_hist.len() == 7 {
            let u_lag = w.monetary.u_hist[0];
            let u3 = w.monetary.u_hist[4..].iter().sum::<f64>() / 3.0;
            -w.cfg.speed_limit_weight * (u3 - u_lag)
        } else {
            0.0
        };
        let i_star = r_star + pi_rule + phi_pi * (pi_rule - target) + phi_u * 2.0 * (w.monetary.u_star - u) + speed_limit;
        // us-now: the intercept wedge of the steered window (identity when steering is off)
        let (rho, prev_s) = (w.cfg.rate_smoothing, w.monetary.shadow_rate.unwrap_or(w.cfg.initial_policy_rate));
        let innov_s = w.shocks.policy_innovation(w.tick, rho);
        let i_star = crate::start::policy_wedge(w, i_star, rho, prev_s, innov_s);
        if i_star.is_finite() {
            w.monetary.i_star = Some(i_star);
            // p1-curve: the curve prices an EWMA of the target (gain `i_star_gain`; 1 = the raw target).
            let g = w.cfg.i_star_gain.clamp(0.0, 1.0);
            let smooth = match w.monetary.i_star_smooth {
                Some(prev_s) if g < 1.0 => prev_s + g * (i_star - prev_s),
                _ => i_star,
            };
            if smooth.is_finite() {
                w.monetary.i_star_smooth = Some(smooth);
            }
        }
        let prev = w.monetary.shadow_rate.unwrap_or(w.cfg.initial_policy_rate);
        // fix-monetary: the monetary-policy innovation enters the shadow rate directly, so the rate moves by the full
        // shock on impact and the rule's inertia carries it (see ShockState::policy_innovation).
        let innov = w.shocks.policy_innovation(w.tick, w.cfg.rate_smoothing);
        let s = w.cfg.rate_smoothing * prev + (1.0 - w.cfg.rate_smoothing) * i_star + innov;
        if s.is_finite() {
            w.monetary.shadow_rate = Some(s);
            w.cb.policy_rate = ((s / RATE_STEP).round() * RATE_STEP).max(0.0);
        }
    }

    if let Some(r) = w.shocks.policy_rate_override(w.tick) {
        w.cb.policy_rate = r;
        w.monetary.shadow_rate = Some(r);
        // fix-monetary: the override pins the shadow rate, so any monetary-policy gap is no longer in it. The gap is
        // dropped with it, so update_curve never subtracts a gap the rate does not contain.
        w.shocks.policy_shift = None;
    }
    w.cb.helicopter_per_household = w.shocks.helicopter_per_household(w.tick);
}

/// Average of the expected short-rate path over the first `tenor` months (§11.4).
///
/// The first `flat` months sit at `base` (an active override, forward guidance); afterwards the rate
/// decays from `base` toward `i_inf` at PATH_DECAY per month. Uses the closed form of the geometric sum.
/// fix-monetary: expected average short rate over `tenor` months with a monetary-policy gap in force. Month k's rule
/// path is i_inf + (base − i_inf)·PATH_DECAY^k (base = the unfloored rule level without the gap); the known gap is added
/// and the sum is floored at zero each month, because the policy rate cannot fall below zero.
/// p1-curve: with `rule_path_weight` > 0 the path is the rule's projection: the rate closes the gap to the smoothed target
/// `i_star` at ρ per month, and the target decays to i_inf at `rule_path_decay` per month (0.97 = PATH_DECAY). The two terms are summed, which is
/// an approximation: a rate chasing a decaying target does not follow that sum exactly (docs/CALIBRATION.md, p1-curve).
fn floored_expected(w: &World, base: f64, i_star: f64, i_inf: f64, tenor: u32) -> f64 {
    let t = tenor.max(1);
    let mut sum = 0.0;
    // p1-curve: the rule's own projection. The rate closes the gap to the current target i* at the rule's speed (1 − ρ a
    // month), and the target itself decays to i_inf at `rule_path_decay` a month. At k = 0 the path is the base rate.
    let rho = w.cfg.rate_smoothing;
    // Only the rule path reads `rule_path_decay`. The default (and every base path) keeps the PATH_DECAY constant on
    // its original arithmetic: a runtime decay with the same value gave last-bit differences in the output.
    let custom = w.cfg.rule_path_weight > 0.0 && w.cfg.rule_path_decay.is_finite() && w.cfg.rule_path_decay != PATH_DECAY;
    let decay = w.cfg.rule_path_decay.clamp(0.0, 0.999);
    for k in 0..t {
        let dk = if custom { decay.powi(k as i32) } else if t <= 24 { PATH_DECAY_POWERS[k as usize] } else { path_decay_powi(k as i32) };
        let path = i_inf + (i_star - i_inf) * dk + (base - i_star) * rho.powi(k as i32);
        sum += (path + w.shocks.policy_gap(w.tick + k)).max(0.0);
    }
    sum / t as f64
}

fn expected_average(base: f64, i_inf: f64, flat: f64, tenor: u32) -> f64 {
    let t = tenor as f64;
    if t <= 0.0 {
        return base;
    }
    let n_flat = flat.clamp(0.0, t);
    let m = t - n_flat;
    let geo = if m > 0.0 {
        (1.0 - PATH_DECAY.powf(m)) / (1.0 - PATH_DECAY)
    } else {
        0.0
    };
    (n_flat * base + m * i_inf + (base - i_inf) * geo) / t
}

/// b7-transmission M-P: expected average of the short rate over a `tenor` (months) under a pin that holds the rate at
/// `base` for `flat` months and then releases it toward `i_inf`. `h` is the monthly hazard that an open-ended pin exits;
/// `rho` the monthly release speed after exit. Per month k the pinned weight a is updated as
/// a ← p' + (p − p') + ρ(a − p), with p the still-pinned probability (p' = p(1 − h) while k + 1 < flat, else 0).
/// With h = 0 and ρ = 0.97 this equals `expected_average` for flat ≥ 1 (see tests); larger h moves the average toward
/// the rule path, larger ρ releases the pin more slowly.
fn pinned_expected(base: f64, i_inf: f64, flat: f64, tenor: u32, h: f64, rho: f64) -> f64 {
    let t = tenor as f64;
    if t <= 0.0 {
        return base;
    }
    let mut p = 1.0_f64;
    let mut a = 1.0_f64;
    let mut sum = 0.0;
    for k in 0..tenor {
        sum += i_inf + (base - i_inf) * a;
        let pn = if ((k + 1) as f64) < flat { p * (1.0 - h) } else { 0.0 };
        a = pn + (p - pn) + rho * (a - p);
        p = pn;
    }
    sum / t
}

/// p1-curve: loading of the random term-premium level factor by tenor (3m, 2y, 5y, 10y, 30y). Flatter than TP_SLOPE, so
/// the level factor's own shape adds less curvature to the yield changes (pca_curv); the 3m loading is zero.
const TP_LEVEL_LOAD: [f64; N_TENORS] = [0.0, 0.3, 0.6, 1.0, 1.2];
/// p1-curve: decay rate λ of the Nelson–Siegel curvature loading (Diebold & Li 2006; per month, tenors in months).
const TP_CURV_LAMBDA: f64 = 0.0609;

/// p1-curve: Nelson–Siegel curvature loading at a tenor in months, (1 − e^{−λτ})/(λτ) − e^{−λτ}. It is a hump, zero at
/// the short end and at long tenors, peaking near two years.
fn curv_loading(tenor_months: f64) -> f64 {
    let x = TP_CURV_LAMBDA * tenor_months;
    let e = (-x).exp();
    (1.0 - e) / x - e
}

/// Compute the yield curve (expected short path + term premia + shocks) (§11.4).
///
/// yields[j] = expected[j] + TP[j], floored at −0.5%, with
/// TP[j] = tp_base[j] + κ·(D/D0 − 1)·slope[j] + shock[j] + policy-gap premium + 0.5·|π_e − π*|·slope[j]
///         + level_factor·TP_LEVEL_LOAD[j] + curv_factor·curv_loading(τ)   (p1-curve; both factors 0 by default).
/// The long-run anchor is r*_est + π^LR (π^LR = target by default). The expected path follows the rule's smoothed
/// target when `rule_path_weight` > 0 (`floored_expected`).
pub fn update_curve(w: &mut World) {
    let target = w.cfg.inflation_target;
    // p1-curve: the long-run anchor uses the long-run expected inflation π^LR, which equals the target until it moves.
    let pi_lr = w.monetary.pi_lr.unwrap_or(target);
    let i_inf = current_r_star(w) + pi_lr;
    let flat = match w.shocks.policy_override {
        Some((_, until)) if w.tick < until => (until - w.tick) as f64,
        _ => 0.0,
    };
    // fix-monetary: one curve model for every world. The policy gap of a monetary shock is known and decays on its own
    // path, so the expected short rate is the rule's path (without the gap) plus the gap, floored at zero month by
    // month (floored_expected). With no gap in force the floored path equals the unfloored closed form whenever the
    // rule path is non-negative, so a 0bp shift or no shift at all gives the same curve.
    // Under a rate override the policy rate is pinned, so the gap is not in it and nothing is subtracted.
    let gap_on = flat == 0.0;
    let gap_now = if gap_on { w.shocks.policy_gap(w.tick) } else { 0.0 };
    // The rule level is the shadow rate rounded exactly as the policy rate is, but unfloored, so base = policy_rate
    // whenever the rate is not at the zero floor, and base − gap is the rule path the floored rate embodies.
    let base = if gap_on {
        let rule = match w.monetary.shadow_rate {
            Some(s) => (s / RATE_STEP).round() * RATE_STEP,
            None => w.cb.policy_rate,
        };
        rule - gap_now
    } else {
        w.cb.policy_rate
    };

    // The reference (d0) and its anchor track the raw private duration; only the priced ratio nets off announced QE.
    let d_raw = duration_ratio_raw(w);
    let d = duration_ratio(w);
    if w.monetary.d0.is_none() && d_raw > 0.0 {
        w.monetary.d0 = Some(d_raw);
    }
    // Duration-supply effect (preferred habitat; Vayanos & Vila 2021) in log terms, bounded at ±1pp at 10y. The
    // anchor re-equilibrates slowly (≈10-year half-life) as arbitrageurs absorb supply, so QE/issuance effects are
    // persistent but do not accumulate without bound.
    let ratio = match w.monetary.d0 {
        // Smoothly bounded at ±0.4 log points × κ (2.5%) ≈ ±1pp on the 10y (tanh keeps QE effective near the bound).
        Some(d0) if d0 > 0.0 && d > 0.0 => 0.4 * ((d / d0).ln() / 0.4).tanh(),
        _ => 0.0,
    };
    if let Some(d0) = w.monetary.d0.as_mut() {
        if d_raw > 0.0 {
            *d0 += DURATION_ANCHOR_ADJ * (d_raw - *d0);
        }
    }

    let shock = w.shocks.term_premium_shock();
    let infl_unc = (w.monetary.pi_e - target).abs();
    let kappa = w.cfg.tp_duration_kappa;
    // p1-curve: random term-premium factors from the exogenous state (level on TP_SLOPE, curvature on the hump).
    // A non-finite factor state contributes nothing (the exogenous AR(1) has no guard of its own).
    let tp_level = Some(w.exog.tp_level()).filter(|x| x.is_finite()).unwrap_or(0.0);
    let tp_curv = Some(w.exog.tp_curv()).filter(|x| x.is_finite()).unwrap_or(0.0);
    // Target the rule is moving toward (the smoothed rule target; unrounded), the base rate when none has been set.
    // `rule_path_weight` = 1 prices the rule's projected target; 0 keeps the old path (the rate decays from its base).
    let i_star = match w.monetary.i_star_smooth {
        Some(t) if t.is_finite() => (1.0 - w.cfg.rule_path_weight) * base + w.cfg.rule_path_weight * t,
        _ => base,
    };
    record_curve_inputs(w, gap_on, base, i_star, i_inf, flat);
    for j in 0..N_TENORS {
        let exp = if gap_on {
            floored_expected(w, base, i_star, i_inf, TENORS[j])
        } else if w.cfg.pin_path_on {
            // b7-transmission M-P: the rule-consistent release of the pin (legacy when the switch is off).
            pinned_expected(base, i_inf, flat, TENORS[j], w.cfg.pin_exit_hazard, w.cfg.pin_exit_decay)
        } else {
            expected_average(base, i_inf, flat, TENORS[j])
        };
        // fix-monetary: term premium responds to the policy innovation itself (Hanson & Stein 2015), decaying with it.
        // The gap enters through tanh, so a very large easing saturates at TP_GAP_CAP rather than growing without bound.
        let tp_mp = w.cfg.policy_tp_response * TP_GAP_CAP * (gap_now / TP_GAP_CAP).tanh() * TP_SLOPE[j];
        let tp = w.cfg.tp_base[j]
            + kappa * ratio * TP_SLOPE[j]
            + shock[j]
            + tp_mp
            + INFLATION_RISK_PREMIUM * infl_unc * TP_SLOPE[j];
        // b8-world H5 (stage 4-2, spec 2.7 official part): the official-demand and credibility term premium of the finance link.
        let tp = if let Some(fo) = crate::world::world_tp_fo(w, j) { tp + fo } else { tp };
        w.curve.expected[j] = exp;
        w.curve.term_premium[j] = tp;
        w.curve.yields[j] = (exp + tp).max(YIELD_FLOOR);
    }
    // p1-curve: the random factor terms, in a separate pass that runs only when a factor is on. Kept out of the loop above
    // so that with both factors at 0 the arithmetic is exactly the base arithmetic (the model is sensitive to the last
    // bit: a 1-ulp change in a rounded rate can cascade through the run).
    if tp_level != 0.0 || tp_curv != 0.0 {
        for j in 0..N_TENORS {
            let tp = w.curve.term_premium[j] + tp_level * TP_LEVEL_LOAD[j] + tp_curv * curv_loading(TENORS[j] as f64);
            w.curve.term_premium[j] = tp;
            w.curve.yields[j] = (w.curve.expected[j] + tp).max(YIELD_FLOOR);
        }
    }
    // us-now: persistent yield offsets of the steered window (nothing when steering is off)
    crate::start::curve_offsets(w);
    update_policy_stance(w);
}

/// b8-world H5: stores the locals of `update_curve` in `w.monetary.curve_inputs`. Arithmetic-free, so the curve is unchanged.
#[inline(never)]
pub fn record_curve_inputs(w: &mut World, gap_on: bool, base: f64, i_star: f64, i_inf: f64, flat: f64) {
    w.monetary.curve_inputs = CurveInputs { gap_on, base, i_star, i_inf, flat };
}

/// b8-world H5 (spec 2.1): fills `out[k]` for every k < out.len() with the monthly short rate that `update_curve` averages
/// into `curve.expected[j]` (the mean of the first TENORS[j] entries), in the branch that `update_curve` used this month:
/// 1. the rule path (`gap_on`, `floored_expected`); 2. the override with `pin_path_on` off (`expected_average`); 3. the
/// override with `pin_path_on` on (`pinned_expected`). The blocs read `out[0..240]`.
pub fn expected_rate_path(w: &World, out: &mut [f64]) {
    let ci = w.monetary.curve_inputs;
    let rho = w.cfg.rate_smoothing;
    if ci.gap_on {
        let custom = w.cfg.rule_path_weight > 0.0 && w.cfg.rule_path_decay.is_finite() && w.cfg.rule_path_decay != PATH_DECAY;
        let decay = w.cfg.rule_path_decay.clamp(0.0, 0.999);
        for (k, o) in out.iter_mut().enumerate() {
            let dk = if custom { decay.powi(k as i32) } else { path_decay_powi(k as i32) };
            let path = ci.i_inf + (ci.i_star - ci.i_inf) * dk + (ci.base - ci.i_star) * rho.powi(k as i32);
            *o = (path + w.shocks.policy_gap(w.tick + k as u32)).max(0.0);
        }
    } else if w.cfg.pin_path_on {
        let (h, rho_pin) = (w.cfg.pin_exit_hazard, w.cfg.pin_exit_decay);
        let (mut p, mut a) = (1.0_f64, 1.0_f64);
        for (k, o) in out.iter_mut().enumerate() {
            *o = ci.i_inf + (ci.base - ci.i_inf) * a;
            let pn = if ((k + 1) as f64) < ci.flat { p * (1.0 - h) } else { 0.0 };
            a = pn + (p - pn) + rho_pin * (a - p);
            p = pn;
        }
    } else {
        for (k, o) in out.iter_mut().enumerate() {
            let kf = k as f64;
            *o = if kf < ci.flat { ci.base } else { ci.i_inf + (ci.base - ci.i_inf) * PATH_DECAY.powf(kf - ci.flat) };
        }
    }
}

/// b7-transmission M-E: the policy stance S_t = r2 − ref_{t−1}, with r2 the 2y real rate (2y yield less π_e) and ref an
/// EWMA of r2 with gain 1/`firm_stance_months` (ref starts at r2). Always computed; non-finite inputs leave the state
/// unchanged. Firms read S_t only when `firm_stance_weight` is non-zero.
fn update_policy_stance(w: &mut World) {
    let r2 = w.curve.yields[1] - w.monetary.pi_e;
    if !r2.is_finite() {
        return;
    }
    match w.monetary.stance_ref {
        None => {
            w.monetary.stance_ref = Some(r2);
            w.monetary.firm_stance = 0.0;
        }
        Some(prev) => {
            let months = w.cfg.firm_stance_months;
            let gain = if months > 0.0 && months.is_finite() { 1.0 / months } else { 1.0 };
            w.monetary.firm_stance = r2 - prev;
            w.monetary.stance_ref = Some(prev + (r2 - prev) * gain);
        }
    }
}

/// Recompute bonds_value for banks, CB, Fund and RoW, and gov.debt_face, at current curve prices (§11.4).
fn mark_to_market(w: &mut World) {
    for b in w.banks.iter_mut() {
        b.bonds_value = 0.0;
    }
    w.cb.bonds_value = 0.0;
    w.fund.bonds_value = 0.0;
    w.row.bonds_value = 0.0;
    let mut debt = 0.0;
    for idx in 0..w.bonds.len() {
        let price = issue_price(w, idx);
        for (h, face) in w.bonds[idx].holdings.iter() {
            let mv = face * price;
            debt += face;
            match *h {
                BondHolder::Bank(b) => w.banks[b as usize].bonds_value += mv,
                BondHolder::CentralBank => w.cb.bonds_value += mv,
                BondHolder::Fund => w.fund.bonds_value += mv,
                BondHolder::RestOfWorld => w.row.bonds_value += mv,
            }
        }
    }
    w.gov.debt_face = debt;
}

/// Pay Fund income to households pro rata to shares, in one parallel pass over fixed household chunks (§16).
/// Each household's dividend is credited in place; the Fund is never constrained, so its debit and the bank
/// deposit and reserve changes are reduced per chunk in chunk order.
fn distribute_fund_income(w: &mut World, amount: f64) {
    let total = w.fund.total_shares;
    if !(amount > EPS) || !amount.is_finite() || !(total > EPS) {
        return;
    }
    let nb = w.banks.len();
    let parts: Vec<(Vec<f64>, f64)> = w
        .households
        .par_chunks_mut(FUND_CHUNK)
        .zip(w.hh_month.par_chunks_mut(FUND_CHUNK))
        .map(|(hhs, mons)| {
            let mut bank = vec![0.0; nb];
            let mut out = 0.0;
            for (hh, mon) in hhs.iter_mut().zip(mons.iter_mut()) {
                let s = hh.shares;
                if !hh.alive || s <= 0.0 {
                    continue;
                }
                let x = amount * s / total;
                // Fund → household (`ledger::pay`).
                if x > 0.0 && x.is_finite() {
                    hh.deposits += x;
                    bank[hh.bank as usize] += x;
                    out += x;
                }
                mon.income.dividends += x;
            }
            (bank, out)
        })
        .collect();
    let mut net = vec![0.0; nb];
    let mut out_total = 0.0;
    for (b, o) in &parts {
        for (a, x) in net.iter_mut().zip(b) {
            *a += x;
        }
        out_total += o;
    }
    let fund_bank = w.fund.bank as usize;
    net[fund_bank] -= out_total;
    w.fund.deposits -= out_total;
    crate::fiscal::apply_bank_net(w, &net);
}

/// Total face held by the central bank across all issues.
fn cb_face(w: &World) -> f64 {
    w.bonds.iter().map(|b| b.held_by(BondHolder::CentralBank)).sum()
}

/// Face of issue `idx` held by any of `holders`.
fn private_face(w: &World, idx: usize, holders: &[BondHolder]) -> f64 {
    holders.iter().map(|&h| w.bonds[idx].held_by(h)).sum()
}

/// Buy up to `face` of issue `idx` from `holders` in order, at market price (§11.4). Returns face bought.
fn buy_private_face(w: &mut World, idx: usize, face: f64, holders: &[BondHolder]) -> f64 {
    if !(face > EPS) || !face.is_finite() {
        return 0.0;
    }
    let price = issue_price(w, idx);
    let mut left = face;
    for &h in holders {
        if left <= EPS {
            break;
        }
        let x = w.bonds[idx].held_by(h).min(left);
        if x <= EPS {
            continue;
        }
        left -= ledger::bond_transfer(w, idx, h, BondHolder::CentralBank, x, price);
    }
    (face - left).max(0.0)
}

/// Central-bank open-market operation in tenor `tenor_idx` (§11.4). `share` is monthly face as a share of
/// annual GDP. Positive buys from Fund, RoW, then banks, taking closest-maturity issues first; negative
/// sells to Fund, RoW and banks in proportion to their holdings of each issue. Money moves via
/// `ledger::bond_transfer`, so purchases create reserves and deposits and sales destroy them.
/// Returns the net face purchased (negative for QT).
fn apply_qe(w: &mut World, tenor_idx: usize, share: f64) -> f64 {
    let target = share.abs() * 12.0 * common::gdp_month_model(w);
    if !(target > EPS) || !target.is_finite() {
        return 0.0;
    }
    let tenor = TENORS[tenor_idx.min(N_TENORS - 1)] as f64;
    let mut order: Vec<(f64, usize)> = w
        .bonds
        .iter()
        .enumerate()
        .map(|(i, b)| ((remaining_months(w, b) as f64 - tenor).abs(), i))
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));

    let holders = private_holders(w.banks.len());
    let mut left = target;
    let mut net = 0.0;
    for &(_, idx) in &order {
        if left <= EPS {
            break;
        }
        if share > 0.0 {
            let t = buy_private_face(w, idx, left, &holders);
            left -= t;
            net += t;
            // b7-transmission M-Q: remember the purchased lot by its maturity (it ages and drops out at maturity).
            if t > EPS {
                let mat = w.bonds[idx].maturity_tick;
                w.monetary.qe_lots.push((mat, t));
            }
        } else {
            let price = issue_price(w, idx);
            let x = w.bonds[idx].held_by(BondHolder::CentralBank).min(left);
            if x <= EPS {
                continue;
            }
            // Sell in proportion to each holder's holding of this issue. If no private holder has any of it,
            // fall back to their total government-bond book so the sale still executes.
            let mut buyers: Vec<(BondHolder, f64)> = holders
                .iter()
                .map(|&h| (h, w.bonds[idx].held_by(h)))
                .filter(|(_, held)| *held > 0.0)
                .collect();
            if buyers.is_empty() {
                buyers = holders
                    .iter()
                    .map(|&h| (h, w.bonds.iter().map(|b| b.held_by(h)).sum::<f64>()))
                    .filter(|(_, held)| *held > 0.0)
                    .collect();
            }
            let total: f64 = buyers.iter().map(|(_, held)| held).sum();
            if !(total > EPS) {
                continue;
            }
            for (h, held) in buyers {
                let t = ledger::bond_transfer(w, idx, BondHolder::CentralBank, h, x * held / total, price);
                left -= t;
                net -= t;
            }
        }
    }
    // b7-transmission M-Q: QT sales shrink every held lot pro rata (floored at zero).
    if share < 0.0 && net < 0.0 {
        let sold = -net;
        let total: f64 = w.monetary.qe_lots.iter().map(|lot| lot.1).sum();
        if total > EPS {
            let keep = (1.0 - sold / total).max(0.0);
            for lot in w.monetary.qe_lots.iter_mut() {
                lot.1 *= keep;
            }
        }
    }
    w.cb.qe_this_month += net;
    w.flows.qe_purchases += net;
    net
}

/// Restore central-bank holdings by `gap` face (reinvestment of maturing bonds and trend growth, §11.4).
/// The gap is spread over issues in proportion to outstanding face; each slice is bought from the Fund, then
/// RoW, then banks. Any shortfall on an issue is filled pro rata to the remaining private supply.
/// Returns the face bought.
fn reinvest(w: &mut World, gap: f64) -> f64 {
    let total: f64 = w.bonds.iter().map(|b| b.face_outstanding()).sum();
    if !(gap > EPS) || !gap.is_finite() || !(total > EPS) {
        return 0.0;
    }
    let holders = private_holders(w.banks.len());
    let shares: Vec<f64> = w.bonds.iter().map(|b| b.face_outstanding() / total).collect();
    let mut bought = 0.0;
    for (idx, share) in shares.iter().enumerate() {
        bought += buy_private_face(w, idx, gap * share, &holders);
    }
    let left = gap - bought;
    if left > EPS {
        let supply: Vec<f64> = (0..w.bonds.len()).map(|i| private_face(w, i, &holders)).collect();
        let tot: f64 = supply.iter().sum();
        if tot > EPS {
            let take = left.min(tot);
            for (idx, s) in supply.iter().enumerate() {
                bought += buy_private_face(w, idx, take * s / tot, &holders);
            }
        }
    }
    bought
}

/// Trade bonds worth `value` (market) between `holder` and the `counterparties` (§11.4 liquidity).
/// `buy` = holder buys from the counterparties; otherwise holder sells to them. Issues are filled in order and
/// each issue's amount is split equally across the remaining counterparties. Returns the market value traded.
pub(crate) fn trade_with(w: &mut World, holder: BondHolder, counterparties: &[BondHolder], value: f64, buy: bool) -> f64 {
    let n = counterparties.len();
    if n == 0 || !(value > EPS) || !value.is_finite() {
        return 0.0;
    }
    let mut left = value;
    for idx in 0..w.bonds.len() {
        if left <= EPS {
            break;
        }
        let price = issue_price(w, idx);
        if !(price > 0.0) {
            continue;
        }
        for (k, &cp) in counterparties.iter().enumerate() {
            if left <= EPS {
                break;
            }
            let face = left / (n - k) as f64 / price;
            let t = if buy {
                ledger::bond_transfer(w, idx, cp, holder, face, price)
            } else {
                ledger::bond_transfer(w, idx, holder, cp, face, price)
            };
            left -= t * price;
        }
    }
    (value - left).max(0.0)
}

/// Trade with the banks; banks always accept (see `trade_with`).
pub(crate) fn trade_with_banks(w: &mut World, holder: BondHolder, value: f64, buy: bool) -> f64 {
    let banks: Vec<BondHolder> = (0..w.banks.len()).map(|b| BondHolder::Bank(b as BankId)).collect();
    trade_with(w, holder, &banks, value, buy)
}

/// Fund cash above the deploy threshold: deposits in excess of FUND_DEP_TARGET × NAV once deposits exceed
/// FUND_DEP_MAX × NAV, otherwise 0. NAV = deposits + bond market value + equity value (§11.4).
pub fn fund_excess_cash(w: &World) -> f64 {
    let dep = w.fund.deposits;
    let nav = dep + w.fund.bonds_value + w.fund.equity_value;
    if !(nav > 0.0) || !nav.is_finite() {
        return 0.0;
    }
    if dep > FUND_DEP_MAX * nav {
        dep - FUND_DEP_TARGET * nav
    } else {
        0.0
    }
}

/// Fund keeps deposits between 2% and 8% of NAV, targeting 5% (§11.4 item 6). Excess cash (see
/// `fund_excess_cash`) buys bonds from banks, then from RoW; any remainder stays on deposit. Short cash is
/// raised by selling bonds to banks.
fn rebalance_fund(w: &mut World) {
    let dep = w.fund.deposits;
    let nav = dep + w.fund.bonds_value + w.fund.equity_value;
    if !(nav > 0.0) || !nav.is_finite() {
        return;
    }
    if dep > FUND_DEP_MAX * nav {
        let mut left = fund_excess_cash(w);
        left -= trade_with_banks(w, BondHolder::Fund, left, true);
        if left > EPS {
            left -= trade_with(w, BondHolder::Fund, &[BondHolder::RestOfWorld], left, true);
        }
        // Remaining cash stays on deposit: Treasury issuance is driven only by fiscal financing needs.
        let _ = left;
    } else if dep < FUND_DEP_MIN * nav {
        trade_with_banks(w, BondHolder::Fund, FUND_DEP_TARGET * nav - dep, false);
    }
}

/// RoW holds deposits near 1/12 of its bond holdings plus a GDP buffer: buys bonds from banks when above
/// that target, sells bonds to banks when deposits are negative (§11.4 item 6).
pub(crate) fn rebalance_row(w: &mut World) {
    let gdp = common::gdp_month_model(w).max(0.0);
    // b8-world H5 (stage 4-2, D45): the official block's executed flows are invisible to the private RoW book. `x` is the cumulative net
    // official sales at market value: the cash they brought is taken out of the deposits and the sold bonds are put back (without the
    // bonds, the `bonds_value / 12` term of the target would undo 1/13 of every sale).
    let (bonds, dep) = match crate::world::world_row_extra(w) {
        Some(x) => (w.row.bonds_value + x, w.row.deposits - x),
        None => (w.row.bonds_value, w.row.deposits),
    };
    let target = bonds / 12.0 + ROW_BUFFER_GDP_MONTHS * gdp;
    if dep > target {
        trade_with_banks(w, BondHolder::RestOfWorld, dep - target, true);
    } else if dep < 0.0 {
        trade_with_banks(w, BondHolder::RestOfWorld, target - dep, false);
    }
}

/// Coupons & maturities, central-bank target, QE/QT and reinvestment, Fund and RoW portfolio decisions, bond
/// & equity valuation, Fund NAV (§11.4, §16).
pub fn markets(w: &mut World) {
    // b7-transmission M-Q: lots that have matured leave the central bank's book (matched by maturity, not index).
    let tick = w.tick;
    w.monetary.qe_lots.retain(|&(mat, _)| mat > tick);
    let qe_now = w.shocks.qe_active(w.tick);
    let qt = qe_now.iter().any(|&(_, share)| share < 0.0);

    // 0. Central-bank target face: initialised to current holdings at the first call, then grown with trend
    //    nominal GDP. The trend pauses during QT, when the target moves only with shock sales and runoff.
    if !w.monetary.cb_target_init {
        w.monetary.cb_target_face = cb_face(w);
        w.monetary.cb_target_init = true;
    } else if !qt {
        // Trend growth of the central bank's balance-sheet target: nominal GDP trend from configuration (§11.4).
        w.monetary.cb_target_face *= (1.0 + w.cfg.nominal_trend_growth()).powf(1.0 / 12.0);
    }
    w.monetary.reinvestment_this_month = 0.0;
    // Face the central bank holds that matures this month; under QT it runs off and the target falls with it.
    let matured_cb: f64 = w
        .bonds
        .iter()
        .filter(|b| b.maturity_tick <= w.tick)
        .map(|b| b.held_by(BondHolder::CentralBank))
        .sum();

    // 1. Coupons and maturities. Fund's coupon income is measured before the payment so it can be
    //    distributed to households (principal repayments are not outlays).
    let fund_coupon: f64 = w
        .bonds
        .iter()
        .map(|b| b.held_by(BondHolder::Fund) * b.coupon / 12.0)
        .sum();
    let interest = ledger::service_government_debt(w);
    w.gov.interest_paid += interest;
    w.gov.outlays += interest;

    // 2. QE / QT for every active shock. The target moves by the face that actually executed, so shortfalls
    //    in private supply do not turn into later reinvestment.
    for &(tenor_idx, share) in &qe_now {
        let net = apply_qe(w, tenor_idx, share);
        w.monetary.cb_target_face += net;
    }
    if qt {
        w.monetary.cb_target_face -= matured_cb;
    }
    // fix-monetary: agency-MBS purchases accumulate and set the mortgage-spread change for the banking module.
    w.monetary.mbs_stock_gdp += w.shocks.mbs_share(w.tick);
    w.monetary.mbs_spread_adj = -w.cfg.mbs_spread_per_gdp * w.monetary.mbs_stock_gdp;

    // 2b. Reinvestment: unless QT is active, buy bonds to restore holdings to target (Fed reinvestment policy).
    if !qt {
        let gap = w.monetary.cb_target_face - cb_face(w);
        w.monetary.reinvestment_this_month = reinvest(w, gap);
    }

    // 3. Mark to market at current curve prices.
    mark_to_market(w);

    // 4. Fund dividends (firms & banks) plus Fund coupon income go to households pro rata to shares.
    let div_received = if w.fund.dividends_received.is_finite() {
        w.fund.dividends_received.max(0.0)
    } else {
        0.0
    };
    let special = if w.fund.special_dividends.is_finite() { w.fund.special_dividends.max(0.0) } else { 0.0 };
    w.fund.special_dividends = 0.0;
    distribute_fund_income(w, div_received + special + fund_coupon);
    // 5. Equity valuation by sector (stage 4) and the Fund equity return: see `equity::revalue_fund_equity`.
    let equity = crate::equity::revalue_fund_equity(w, div_received, special);

    // 6. Fund and RoW liquidity.
    rebalance_fund(w);
    rebalance_row(w);

    // 7. Final mark to market (trades above changed bank and Fund holdings), then NAV and share price.
    //    The share price is kept when total_shares is ~0.
    mark_to_market(w);
    let nav = w.fund.deposits + w.fund.bonds_value + equity;
    w.fund.nav = nav;
    if w.fund.total_shares > EPS && nav.is_finite() {
        w.fund.share_price = nav / w.fund.total_shares;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::equity::DIVIDEND_SMOOTHING;
    use crate::ledger::Account;
    use crate::shocks::{ScheduledShock, Shock, ShockState};
    use crate::testkit::tiny_world;

    /// Re-fit Step 3: the r* integrator skips a negative gap at the zero bound only when the anti-windup switch is on,
    /// and the estimate is clamped to `r_star_est_min`.
    #[test]
    fn r_star_integrator_has_antiwindup_and_a_configurable_floor() {
        let legacy = Config { r_star_adapt: 0.3, ..Default::default() };
        let anti = Config { r_star_antiwindup: true, ..legacy.clone() };
        // Off the zero bound, or with a positive gap, the integrator runs as before.
        let below = r_star_integrate(&legacy, 0.0075, -0.01, 0.0).expect("integrates when the switch is off");
        assert!((below - (0.0075 - 0.3 / 12.0 * 0.01)).abs() < 1e-12, "legacy step {below}");
        assert!(r_star_integrate(&anti, 0.0075, -0.01, 0.0).is_none(), "anti-windup did not skip at the zero bound");
        assert!(r_star_integrate(&anti, 0.0075, -0.01, 0.02).is_some(), "anti-windup skipped above the zero bound");
        assert!(r_star_integrate(&anti, 0.0075, 0.01, 0.0).is_some(), "anti-windup skipped a positive gap");
        // The floor is a config field: −1% by default, 0 in the Step 3 fallback.
        let floor_m1 = r_star_integrate(&legacy, -0.009, -1.0, 0.0).expect("finite");
        assert!((floor_m1 - (-0.01)).abs() < 1e-12, "default floor {floor_m1}");
        let floor_0 = r_star_integrate(&Config { r_star_est_min: 0.0, ..legacy.clone() }, -0.009, -1.0, 0.0).expect("finite");
        assert!(floor_0.abs() < 1e-12, "fallback floor {floor_0}");
        assert!(r_star_integrate(&legacy, 0.0075, f64::NAN, 0.0).is_none(), "non-finite gap was accepted");
    }

    fn world(seed: u64) -> World {
        tiny_world(
            Config {
                n_households: 500,
                ..Default::default()
            },
            seed,
        )
    }

    fn total_reserves(w: &World) -> f64 {
        w.banks.iter().map(|b| b.reserves).sum()
    }

    fn total_deposits(w: &World) -> f64 {
        w.banks.iter().map(|b| b.deposits).sum()
    }

    /// One month of the monetary block (policy, curve, markets) without the rest of the tick.
    fn month(w: &mut World) {
        set_policy(w);
        update_curve(w);
        markets(w);
    }

    #[test]
    fn bond_price_is_par_at_par_yield_and_discounts_otherwise() {
        assert!((bond_price(0.04, 0.04, 120) - 1.0).abs() < 1e-12);
        assert!((bond_price(0.04, 0.04, 0) - 1.0).abs() < 1e-12);
        assert!((bond_price(0.0, 0.0, 60) - 1.0).abs() < 1e-12);
        assert!(bond_price(0.02, 0.04, 120) < 1.0);
        assert!(bond_price(0.04, 0.08, 120) < bond_price(0.04, 0.04, 120));
    }

    #[test]
    fn policy_override_moves_two_year_yield_more_than_thirty_year() {
        let mut w = world(1);
        set_policy(&mut w);
        update_curve(&mut w);
        let base = w.curve.yields;
        let r = w.cb.policy_rate;

        w.shocks.policy_override = Some((r + 0.01, 24));
        set_policy(&mut w);
        update_curve(&mut w);
        assert!((w.cb.policy_rate - (r + 0.01)).abs() < 1e-12);
        assert_eq!(w.monetary.shadow_rate, Some(r + 0.01));

        let d2 = w.curve.yields[1] - base[1];
        let d30 = w.curve.yields[4] - base[4];
        assert!(d2 > 0.005, "2y should rise by most of 100bp, got {d2}");
        assert!(d2 > 3.0 * d30, "2y move {d2} should dominate 30y move {d30}");
    }

    #[test]
    fn positive_term_premium_shock_raises_ten_year_by_shock() {
        let mut w = world(2);
        set_policy(&mut w);
        update_curve(&mut w);
        let y10 = w.curve.yields[3];
        w.shocks.term_premium[3] += 0.01;
        update_curve(&mut w);
        assert!((w.curve.yields[3] - y10 - 0.01).abs() < 1e-12);
    }

    #[test]
    fn qe_lowers_private_duration_and_term_premium_and_adds_reserves_and_deposits() {
        let mut base = world(3);
        set_policy(&mut base);
        update_curve(&mut base);
        let mut qe = base.clone();
        let (res0, dep0) = (total_reserves(&base), total_deposits(&base));
        let dur0 = private_duration(&base);

        let bought = apply_qe(&mut qe, 3, 0.05);
        assert!(bought > 0.0, "CB should buy bonds, got {bought}");
        assert!(private_duration(&qe) < dur0);
        assert!(total_reserves(&qe) > res0, "reserves should rise");
        assert!(total_deposits(&qe) > dep0, "deposits should rise (Fund/RoW sold)");
        assert!((qe.cb.qe_this_month - bought).abs() < 1e-6);
        ledger::check_consistency(&qe).unwrap();

        update_curve(&mut qe);
        assert!(
            qe.curve.term_premium[3] < base.curve.term_premium[3],
            "10y term premium should fall after QE"
        );
    }

    #[test]
    fn monthly_block_is_consistent_with_qe_and_pays_coupons() {
        let mut w = world(4);
        w.shocks.qe.push((3, 0.02, 10));
        month(&mut w);
        ledger::check_consistency(&w).unwrap();
        assert!(w.cb.qe_this_month > 0.0);
        assert!(w.gov.interest_paid > 0.0);
        assert_eq!(w.gov.interest_paid, w.gov.outlays);
        assert!(w.gov.debt_face > 0.0);
        let mtm: f64 = w.banks.iter().map(|b| b.bonds_value).sum::<f64>()
            + w.cb.bonds_value
            + w.fund.bonds_value
            + w.row.bonds_value;
        assert!(mtm > 0.0 && mtm.is_finite());
    }

    #[test]
    fn fund_share_price_falls_when_yields_jump() {
        let mut w = world(5);
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        let p0 = w.fund.share_price;
        assert!(p0 > 0.0 && p0.is_finite());

        w.shocks.term_premium = [0.02; N_TENORS];
        update_curve(&mut w);
        markets(&mut w);
        assert!(
            w.fund.share_price < p0,
            "share price should fall: {} -> {}",
            p0,
            w.fund.share_price
        );
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn fund_dividends_and_coupons_reach_households_pro_rata() {
        let mut w = world(6);
        set_policy(&mut w);
        update_curve(&mut w);
        w.fund.dividends_received = 100.0;
        let coupon: f64 = w
            .bonds
            .iter()
            .map(|b| b.held_by(BondHolder::Fund) * b.coupon / 12.0)
            .sum();
        markets(&mut w);
        let paid: f64 = w.hh_month.iter().map(|m| m.income.dividends).sum();
        assert!((paid - (100.0 + coupon)).abs() < 1e-6 * (1.0 + paid));
        assert_eq!(w.fund.dividends_received, 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn zero_fund_shares_keep_previous_share_price() {
        let mut w = world(7);
        for h in w.households.iter_mut() {
            h.shares = 0.0;
        }
        w.fund.total_shares = 0.0;
        w.fund.share_price = 1.234;
        month(&mut w);
        assert_eq!(w.fund.share_price, 1.234);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn taylor_rule_is_rounded_and_floored_at_zero() {
        let mut w = world(8);
        w.shocks.taylor = Some((0.5, 1.0, -0.10));
        for t in 1..60 {
            w.tick = t;
            set_policy(&mut w);
            assert!(w.cb.policy_rate >= 0.0);
            let steps = w.cb.policy_rate / RATE_STEP;
            assert!((steps - steps.round()).abs() < 1e-9);
        }
        assert_eq!(w.cb.policy_rate, 0.0);
        assert!(w.monetary.shadow_rate.unwrap() < 0.0, "shadow stays unrounded");
    }

    // rz13: `phi_u` is the weight on the Okun-scaled unemployment gap in the rule's target (i* = r* + pi + phi_pi (pi - pi*) + phi_u 2 (u* - u) + speed
    // limit). RZ13's candidate c3 lowers this existing value (0.5 -> 0.25), with two other rule/credit values, and changes no code. Two identical worlds that differ only in `phi_u` must give
    // targets that differ by exactly the weight difference times 2 (u* - u) at the first rule month (nothing else has had time to diverge).
    #[test]
    fn phi_u_scales_the_rules_response_to_the_unemployment_gap() {
        let at_first_rule_month = |phi_u: f64| -> (f64, f64, f64) {
            let mut w = tiny_world(Config { n_households: 500, phi_u, ..Default::default() }, 21);
            w.tick = 1;
            set_policy(&mut w);
            (w.monetary.i_star.expect("a finite rule target"), w.monetary.u_star, w.unemployment_rate())
        };
        let (half, u_star, u) = at_first_rule_month(0.5);
        let (quarter, u_star_q, u_q) = at_first_rule_month(0.25);
        let (zero, _, _) = at_first_rule_month(0.0);
        assert_eq!((u_star.to_bits(), u.to_bits()), (u_star_q.to_bits(), u_q.to_bits()), "the two worlds differ only in phi_u");
        let gap = 2.0 * (u_star - u);
        assert!(gap.abs() > 1e-4, "the test needs a non-zero unemployment gap to see the weight (gap {gap})");
        assert!(((half - quarter) - 0.25 * gap).abs() < 1e-12, "0.5 -> 0.25 moves the target by 0.25 x 2 (u* - u): {} vs {}", half - quarter, 0.25 * gap);
        assert!(((half - zero) - 0.5 * gap).abs() < 1e-12, "0.5 -> 0 moves the target by 0.5 x 2 (u* - u)");
    }

    // rz13: the default weight is Taylor's 0.5, so every run without an overlay is untouched by c3 (the default fingerprint is the other half of this proof).
    #[test]
    fn phi_u_default_is_taylors_half_and_the_rule_defaults_are_untouched() {
        assert_eq!(Config::default().phi_u, 0.5);
        assert_eq!(Config::default().speed_limit_weight, 0.0);
        assert_eq!(Config::default().rate_smoothing, 0.9);
    }

    #[test]
    fn helicopter_amount_follows_shock_schedule() {
        let mut w = world(9);
        w.shocks.helicopter = Some((250.0, 5));
        set_policy(&mut w);
        assert_eq!(w.cb.helicopter_per_household, 250.0);
        w.tick = 5;
        set_policy(&mut w);
        assert_eq!(w.cb.helicopter_per_household, 0.0);
    }

    /// Treasury-style supply for tests: a new 60-month issue sold at par to the Fund (cash endowed by the CB).
    fn treasury_issue(w: &mut World, face: f64) {
        ledger::pay(w, Account::CentralBank, Account::Fund, face);
        let coupon = w.curve.yields[2].max(0.0);
        ledger::issue_bond(w, 60, coupon, BondHolder::Fund, face, 1.0);
    }

    fn cb_holdings(w: &World) -> f64 {
        w.bonds.iter().map(|b| b.held_by(BondHolder::CentralBank)).sum()
    }

    #[test]
    fn reinvestment_keeps_cb_holdings_within_two_percent_of_target_for_60_months() {
        let mut w = world(10);
        set_policy(&mut w);
        update_curve(&mut w);
        let gdp = common::gdp_month_model(&w);
        let mut max_dev = 0.0f64;
        let mut reinvested = 0.0;
        let mut init_target = None;
        for t in 0..60 {
            w.tick = t;
            treasury_issue(&mut w, 0.02 * gdp);
            set_policy(&mut w);
            update_curve(&mut w);
            markets(&mut w);
            ledger::check_consistency(&w).unwrap();
            let target = w.monetary.cb_target_face;
            assert!(target > 0.0);
            init_target.get_or_insert(target);
            max_dev = max_dev.max((cb_holdings(&w) - target).abs() / target);
            reinvested += w.monetary.reinvestment_this_month;
        }
        assert!(max_dev <= 0.02, "CB holdings deviate {max_dev:.4} from target");
        assert!(reinvested > 0.0, "bonds matured, so reinvestment should have bought bonds");
        // Trend growth of 4%/yr applies to the 59 months after initialisation.
        let expected = init_target.unwrap() * (1.0 + w.cfg.nominal_trend_growth()).powf(59.0 / 12.0);
        assert!((w.monetary.cb_target_face - expected).abs() < 1e-9 * expected);
    }

    #[test]
    fn qe_executes_in_full_raises_target_permanently_and_lowers_ten_year_term_premium() {
        let mut w = world(11);
        set_policy(&mut w);
        update_curve(&mut w);
        // Counterfactual with identical months and no QE shock.
        let mut base = w.clone();
        let gdp = common::gdp_month_model(&w);
        let flow0 = w.flows.qe_purchases;
        // 1% of annual GDP per month of the 10y tenor for 6 months.
        w.shocks.qe.push((3, 0.01, 6));
        for t in 0..6 {
            w.tick = t;
            base.tick = t;
            for x in [&mut w, &mut base] {
                set_policy(x);
                update_curve(x);
                markets(x);
                ledger::check_consistency(x).unwrap();
            }
        }
        let executed = w.flows.qe_purchases - flow0;
        let want = 0.06 * 12.0 * gdp;
        assert!(
            (executed - want).abs() < 1e-6 * want,
            "QE executed {executed}, wanted {want} (6% of annual GDP)"
        );
        let target_after_qe = w.monetary.cb_target_face;
        assert!(cb_holdings(&w) > cb_holdings(&base) + 0.9 * want, "CB holdings should rise by the QE amount");

        // The target stays raised after QE ends: holdings do not run off.
        for t in 6..18 {
            w.tick = t;
            base.tick = t;
            for x in [&mut w, &mut base] {
                set_policy(x);
                update_curve(x);
                markets(x);
                ledger::check_consistency(x).unwrap();
            }
            assert!(w.monetary.cb_target_face >= target_after_qe);
            let target = w.monetary.cb_target_face;
            assert!(cb_holdings(&w) >= 0.98 * target, "holdings ran off after QE ended");
        }
        assert!(
            w.curve.term_premium[3] < base.curve.term_premium[3],
            "10y term premium should fall relative to no QE: {} vs {}",
            w.curve.term_premium[3],
            base.curve.term_premium[3]
        );
    }

    #[test]
    fn qt_lowers_target_by_sales_and_runoff_and_pauses_reinvestment() {
        let mut w = world(12);
        set_policy(&mut w);
        update_curve(&mut w);
        w.tick = 0;
        markets(&mut w);
        let t0 = w.monetary.cb_target_face;
        let flow0 = w.flows.qe_purchases;

        // 0.5% of annual GDP per month of sales for ticks 1..=6.
        w.shocks.qe.push((3, -0.005, 7));
        let mut runoff = 0.0;
        for t in 1..=6 {
            w.tick = t;
            runoff += w
                .bonds
                .iter()
                .filter(|b| b.maturity_tick <= t)
                .map(|b| b.held_by(BondHolder::CentralBank))
                .sum::<f64>();
            set_policy(&mut w);
            update_curve(&mut w);
            markets(&mut w);
            assert_eq!(w.monetary.reinvestment_this_month, 0.0, "no reinvestment during QT");
            ledger::check_consistency(&w).unwrap();
        }
        let sold = w.flows.qe_purchases - flow0;
        assert!(sold < 0.0, "QT should sell bonds, got {sold}");
        let t_qt = w.monetary.cb_target_face;
        assert!(t_qt < t0, "QT should lower the target");
        assert!(
            (t_qt - (t0 + sold - runoff)).abs() < 1e-6 * t0,
            "target {t_qt} should equal {t0} + sales {sold} - runoff {runoff}"
        );

        // After QT, reinvestment resumes and holdings return to target.
        w.tick = 7;
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        assert!((cb_holdings(&w) - w.monetary.cb_target_face).abs() <= 0.02 * w.monetary.cb_target_face);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn fund_excess_cash_is_deployed_to_banks_and_rest_of_world() {
        let mut w = world(13);
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        let gdp = common::gdp_month_model(&w);
        ledger::pay(&mut w, Account::CentralBank, Account::Fund, 2.0 * gdp);
        let nav = w.fund.deposits + w.fund.bonds_value + w.fund.equity_value;
        assert!(w.fund.deposits > FUND_DEP_MAX * nav, "setup: Fund should be over-deposited");
        let excess = fund_excess_cash(&w);
        assert!(excess > 0.0);
        let private_others = |w: &World| -> f64 {
            w.bonds
                .iter()
                .flat_map(|b| b.holdings.iter())
                .filter(|(h, _)| matches!(h, BondHolder::Bank(_) | BondHolder::RestOfWorld))
                .map(|(_, f)| *f)
                .sum()
        };
        // Make sure banks hold bonds the Fund can buy.
        for i in 0..w.bonds.len() {
            let f = w.bonds[i].held_by(BondHolder::Fund);
            if f > 0.0 {
                ledger::bond_transfer(&mut w, i, BondHolder::Fund, BondHolder::Bank(0), 0.5 * f, 1.0);
            }
        }
        let others_before = private_others(&w);
        let bonds_n = w.bonds.len();
        markets(&mut w);
        ledger::check_consistency(&w).unwrap();
        // Excess cash buys existing bonds from banks/RoW (no new issuance outside fiscal financing).
        assert!(private_others(&w) < others_before, "Fund should have bought bonds from banks/RoW");
        assert!(w.bonds.len() <= bonds_n, "no new issues created by the Fund");
    }

    #[test]
    fn fund_short_of_cash_sells_bonds_to_banks() {
        let mut w = world(15);
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        let dep = w.fund.deposits;
        ledger::pay(&mut w, Account::Fund, Account::Government, 0.99 * dep);
        assert!(w.fund.deposits < FUND_DEP_MIN * w.fund.nav);
        markets(&mut w);
        ledger::check_consistency(&w).unwrap();
        let nav = w.fund.nav;
        assert!(
            (w.fund.deposits - FUND_DEP_TARGET * nav).abs() < 0.01 * nav,
            "deposits {} should be refilled to 5% of NAV {}",
            w.fund.deposits,
            nav
        );
    }

    #[test]
    fn first_month_equity_is_seeded_from_existing_equity_value() {
        let mut w = world(16);
        w.fund.equity_value = 1000.0;
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        assert!(
            (w.fund.equity_value - 1000.0).abs() < 1e-6,
            "month 1 equity should keep the seeded value, got {}",
            w.fund.equity_value
        );
        // With no dividends, the smoothed dividends then decay at DIVIDEND_SMOOTHING per month; the growth signal
        // moves the premium a little, so allow for that.
        markets(&mut w);
        assert!((w.fund.equity_value - DIVIDEND_SMOOTHING * 1000.0).abs() < 0.02 * 1000.0);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn tighter_policy_appreciates_dollar_and_cuts_exports() {
        use crate::shocks::{Shock, ScheduledShock};
        let cfg = Config { n_households: 2000, burn_in_months: 0, ..Default::default() };
        let w0 = crate::sim::new_world(&cfg, 7);
        assert_eq!(w0.row.fx, 1.0);
        let mut base = w0.clone();
        let mut tight = w0;
        tight.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyRate { rate: 0.07, months: 24 } });
        for _ in 0..6 {
            crate::sim::step(&mut base);
            crate::sim::step(&mut tight);
        }
        assert!(base.row.fx.is_finite() && tight.row.fx.is_finite());
        assert!(tight.row.fx > base.row.fx * 1.01, "fx {} vs baseline {}", tight.row.fx, base.row.fx);
        let exports = |w: &World| w.orders.exports.iter().sum::<f64>();
        assert!(exports(&tight) < exports(&base), "exports {} vs baseline {}", exports(&tight), exports(&base));
    }

    #[test]
    fn sector_equity_sums_to_fund_equity() {
        let mut w = world(21);
        w.fund.equity_value = 1000.0;
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        assert!((w.fund.sector_equity.iter().sum::<f64>() - w.fund.equity_value).abs() < 1e-6 * (1.0 + w.fund.equity_value));
        // Sector dividends (and bank dividends in the residual) move sector values; the total must stay the sum.
        for m in 0..36 {
            let s = m % N_SECTORS;
            w.fund.sector_dividends_received[s] += 5.0 + s as f64;
            w.fund.dividends_received += 5.0 + s as f64 + if m % 3 == 0 { 2.0 } else { 0.0 };
            markets(&mut w);
            let sum: f64 = w.fund.sector_equity.iter().sum();
            assert!((sum - w.fund.equity_value).abs() < 1e-6 * (1.0 + sum), "month {m}: {sum} vs {}", w.fund.equity_value);
            assert!(w.fund.sector_equity.iter().all(|e| e.is_finite() && *e >= 0.0));
        }
    }

    #[test]
    fn tech_boom_raises_tech_valuation_relative_to_others() {
        let mut w = world(22);
        w.fund.equity_value = 1000.0;
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        let tech0 = w.fund.sector_equity[Sector::Tech.idx()] / w.fund.sector_equity[Sector::Energy.idx()];
        w.exog.tech_boom = true;
        for _ in 0..24 {
            markets(&mut w);
        }
        let tech1 = w.fund.sector_equity[Sector::Tech.idx()] / w.fund.sector_equity[Sector::Energy.idx()];
        assert!(tech1 > 1.1 * tech0, "tech/energy equity {tech0:.3} -> {tech1:.3}");
    }

    /// p1-curve: the Nelson–Siegel curvature loading is zero at the short end and at long tenors and peaks at 2 years.
    #[test]
    fn curvature_loading_is_a_hump_peaking_at_two_years() {
        let l: Vec<f64> = TENORS.iter().map(|&t| curv_loading(t as f64)).collect();
        assert!(l[0] < l[1] && l[1] > l[2] && l[2] > l[3] && l[3] > l[4], "loadings {l:?}");
        assert!(l[4] > 0.0 && l[0] > 0.0);
    }

    /// fix-monetary: a policy innovation at month 0 (burn-in 0) moves the first policy rate, as it does at later months.
    #[test]
    fn policy_shift_at_month_zero_moves_the_first_rate() {
        let mut base = world(5);
        let mut shocked = world(5);
        shocked.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 100.0, months: 12 } });
        shocked.shocks.activate_due(0);
        base.shocks.activate_due(0);
        set_policy(&mut base);
        set_policy(&mut shocked);
        update_curve(&mut shocked);
        update_curve(&mut base);
        let dr = shocked.cb.policy_rate - base.cb.policy_rate;
        assert!((dr - 0.01).abs() < 1e-9, "policy rate moved {dr}");
        assert!(shocked.curve.yields[1] > base.curve.yields[1], "2y did not rise");
    }

    /// fix-monetary: a large easing that drives the policy rate to the zero floor must still lower the short yields, and
    /// its term-premium response must stay bounded (the gap enters through tanh).
    #[test]
    fn large_easing_at_the_zero_floor_lowers_the_two_year() {
        let mut base = world(7);
        let mut eased = world(7);
        eased.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: -1500.0, months: 12 } });
        eased.shocks.activate_due(0);
        base.shocks.activate_due(0);
        set_policy(&mut base);
        set_policy(&mut eased);
        update_curve(&mut base);
        update_curve(&mut eased);
        assert_eq!(eased.cb.policy_rate, 0.0, "policy rate should sit at the floor");
        let d2 = (eased.curve.yields[1] - base.curve.yields[1]) * 1e4;
        assert!(d2 < -100.0, "2y change under a -1500bp easing {d2:.1}bp");
        let tp_bound = eased.cfg.policy_tp_response * TP_GAP_CAP * TP_SLOPE[3];
        let tp_change = (eased.curve.term_premium[3] - base.curve.term_premium[3]).abs();
        assert!(tp_change <= tp_bound + 1e-12, "10y term premium moved {tp_change} > bound {tp_bound}");
    }

    /// fix-monetary: a 0bp PolicyShift is an exact no-op on the curve, also with the rule at r* = −5% (the floor binds
    /// and the rule path is negative): one curve model serves every world, so a scheduled zero shift changes nothing.
    #[test]
    fn zero_bp_policy_shift_leaves_the_curve_unchanged_at_the_floor() {
        let mut base = world(9);
        let mut shifted = world(9);
        let rule = |w: &mut World| {
            w.shocks.schedule(ScheduledShock {
                at_month: 0,
                shock: Shock::TaylorRule { phi_pi: 1.5, phi_u: 0.5, r_star: -0.05 },
            });
        };
        rule(&mut base);
        rule(&mut shifted);
        shifted.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::PolicyShift { bps: 0.0, months: 12 } });
        base.shocks.activate_due(0);
        shifted.shocks.activate_due(0);
        for t in 0..24 {
            base.tick = t;
            shifted.tick = t;
            set_policy(&mut base);
            set_policy(&mut shifted);
            update_curve(&mut base);
            update_curve(&mut shifted);
            assert_eq!(base.cb.policy_rate, shifted.cb.policy_rate, "month {t}: policy rate");
            for j in 0..N_TENORS {
                assert!(
                    (base.curve.yields[j] - shifted.curve.yields[j]).abs() < 1e-12,
                    "month {t} tenor {j}: {} vs {}",
                    base.curve.yields[j],
                    shifted.curve.yields[j]
                );
            }
        }
        assert!(base.monetary.shadow_rate.unwrap() < 0.0, "the rule should reach the floor");
        assert_eq!(base.cb.policy_rate, 0.0);
    }

    /// fix-monetary: an announced programme is priced at the first month, not only as its purchases execute.
    #[test]
    fn qe_announcement_is_priced_on_the_first_month() {
        let mut base = world(6);
        let mut qe = world(6);
        let qe_shock = Shock::QE { tenor_idx: 3, monthly_face_pct_gdp: 0.5, months: 12 };
        qe.shocks.schedule(ScheduledShock { at_month: 0, shock: qe_shock });
        qe.shocks.activate_due(0);
        base.shocks.activate_due(0);
        set_policy(&mut base);
        set_policy(&mut qe);
        update_curve(&mut base);
        update_curve(&mut qe);
        let d10 = (qe.curve.yields[3] - base.curve.yields[3]) * 1e4;
        assert!(d10 < -5.0, "10y change at announcement {d10:.1}bp");
    }

    // b7-transmission M-E: the stance starts at zero and turns positive when the 2y real rate rises above its reference.
    #[test]
    fn policy_stance_starts_at_zero_and_turns_positive_when_the_two_year_rises() {
        let mut w = world(8);
        set_policy(&mut w);
        update_curve(&mut w);
        assert_eq!(w.monetary.firm_stance, 0.0, "first month: reference is the level");
        assert!(w.monetary.stance_ref.is_some());
        w.shocks.policy_shift = Some((0.01, w.tick, 0.9));
        set_policy(&mut w);
        update_curve(&mut w);
        assert!(w.monetary.firm_stance > 0.0, "stance {} should rise with the 2y", w.monetary.firm_stance);
    }

    // joint3 F3 (b7-transmission review): call-site tests.
    #[test]
    fn apply_qe_positive_share_books_lots_for_the_face_bought() {
        let mut w = world(3);
        set_policy(&mut w);
        update_curve(&mut w);
        let net = apply_qe(&mut w, 2, 0.001);
        assert!(net > 0.0, "a positive share buys face, got {net}");
        let lots: f64 = w.monetary.qe_lots.iter().map(|l| l.1).sum();
        assert!((lots - net).abs() <= 1e-9 * net, "lots {lots} vs face bought {net}");
    }

    #[test]
    fn pin_path_on_gives_a_lower_ten_year_expected_path_under_an_override() {
        fn ten_year_expected(pin_on: bool) -> f64 {
            let mut w = world(1);
            w.cfg.pin_path_on = pin_on;
            w.cfg.pin_exit_hazard = 0.0417;
            w.cfg.pin_exit_decay = 0.9;
            set_policy(&mut w);
            update_curve(&mut w);
            let r = w.cb.policy_rate;
            w.shocks.policy_override = Some((r + 0.01, 24));
            set_policy(&mut w);
            update_curve(&mut w);
            let j = TENORS.iter().position(|&t| t == 120).expect("10-year tenor");
            w.curve.expected[j]
        }
        let (on, off) = (ten_year_expected(true), ten_year_expected(false));
        assert!(on < off, "pinned 10y expected path {on} should lie below the flat-pin path {off}");
    }

    // b7-transmission M-Q tests.
    #[test]
    fn qe_duration_weight_one_keeps_the_legacy_priced_duration() {
        let mut w = world(3);
        // joint3: the default λ is 1.5; this test is about λ = 1, so set it explicitly.
        w.cfg.qe_duration_weight = 1.0;
        set_policy(&mut w);
        update_curve(&mut w);
        w.shocks.qe.push((3, 0.01, 6));
        w.monetary.qe_lots.push((w.tick + 60, 1.0e6));
        let legacy = (duration_ratio_raw(&w) - w.cfg.qe_announcement_weight * w.shocks.qe_announced_duration(w.tick))
            .max(1e-6);
        assert_eq!(duration_ratio(&w), legacy);
    }

    #[test]
    fn qe_duration_weight_above_one_lowers_priced_duration_by_the_stated_amount() {
        let mut w = world(4);
        set_policy(&mut w);
        update_curve(&mut w);
        w.shocks.qe.push((3, 0.01, 6));
        w.monetary.qe_lots.push((w.tick + 84, 5.0e5));
        let raw = duration_ratio_raw(&w);
        let announced = w.cfg.qe_announcement_weight * w.shocks.qe_announced_duration(w.tick);
        let h = lot_duration_ratio(&w);
        assert!(h > 0.0, "lots should carry duration");
        // joint3: the default λ is 1.5; the baseline of this comparison is λ = 1.
        w.cfg.qe_duration_weight = 1.0;
        let legacy = duration_ratio(&w);
        let lambda = 1.75;
        w.cfg.qe_duration_weight = lambda;
        let d = duration_ratio(&w);
        let want = (raw - announced - (lambda - 1.0) * (h + announced)).max(1e-6);
        assert!((d - want).abs() < 1e-12, "priced duration {d}, wanted {want}");
        assert!(((legacy - d) - (lambda - 1.0) * (h + announced)).abs() < 1e-12);
    }

    #[test]
    fn qe_lots_drop_at_maturity_and_shrink_pro_rata_under_qt() {
        let mut w = world(5);
        set_policy(&mut w);
        update_curve(&mut w);
        let big = 1.0e15;
        w.monetary.qe_lots.push((w.tick + 3, big));
        w.monetary.qe_lots.push((w.tick + 30, big));
        // The lot that matures at tick+3 leaves the book at the start of that month.
        w.tick += 3;
        set_policy(&mut w);
        update_curve(&mut w);
        markets(&mut w);
        assert_eq!(w.monetary.qe_lots.len(), 1, "matured lot should be dropped");
        assert!((w.monetary.qe_lots[0].1 - big).abs() < 1e-6);
        // QT sales shrink every remaining lot pro rata, so the total falls by exactly the amount sold.
        let before: f64 = w.monetary.qe_lots.iter().map(|l| l.1).sum();
        let net = apply_qe(&mut w, 3, -0.0001);
        let after: f64 = w.monetary.qe_lots.iter().map(|l| l.1).sum();
        assert!(net <= 0.0);
        assert!((after - (before + net)).abs() <= 1e-6 * before, "after {after}, before {before}, net {net}");
    }

    #[test]
    fn mbs_qe_validates_and_round_trips_through_json() {
        let shock = Shock::MbsQE { monthly_face_pct_gdp: 0.25, months: 12 };
        assert!(shock.validate().is_ok());
        let json = serde_json::to_string(&shock).unwrap();
        let back: Shock = serde_json::from_str(&json).unwrap();
        assert_eq!(back, shock);
        let mut st = ShockState::default();
        st.activate_due(0);
        st.schedule(ScheduledShock { at_month: 0, shock });
        st.activate_due(0);
        assert!((st.mbs_share(0) - 0.0025).abs() < 1e-12);
        let s2: ShockState = serde_json::from_str(&serde_json::to_string(&st).unwrap()).unwrap();
        assert!((s2.mbs_share(0) - 0.0025).abs() < 1e-12);
    }
}

// b7-transmission M-P tests.
#[cfg(test)]
mod pinned_expected_tests {
    use super::{expected_average, pinned_expected};

    /// With h = 0 and ρ = 0.97 the pinned average equals the legacy expected average. Tested for pins of 1, 5 and 24
    /// months and several tenors. (flat = 0 never reaches this branch: `update_curve` uses the floored path then.)
    #[test]
    fn zero_hazard_and_legacy_decay_match_expected_average() {
        for &flat in &[1.0, 5.0, 24.0] {
            for &tenor in &[3u32, 12, 24, 60, 120] {
                let (base, i_inf) = (0.07, 0.025);
                let a = pinned_expected(base, i_inf, flat, tenor, 0.0, 0.97);
                let b = expected_average(base, i_inf, flat, tenor);
                assert!((a - b).abs() <= 1e-12, "flat {flat} tenor {tenor}: {a} vs {b}");
            }
        }
    }

    #[test]
    fn decreases_in_hazard_and_increases_in_release_speed_when_base_is_above_the_anchor() {
        let (base, i_inf, flat, tenor) = (0.07, 0.025, 24.0, 24u32);
        let h0 = pinned_expected(base, i_inf, flat, tenor, 0.0, 0.9);
        let h1 = pinned_expected(base, i_inf, flat, tenor, 1.0 / 24.0, 0.9);
        assert!(h1 < h0, "a finite pin prices lower: {h1} vs {h0}");
        let slow = pinned_expected(base, i_inf, 5.0, 24, 0.0, 0.97);
        let fast = pinned_expected(base, i_inf, 5.0, 24, 0.0, 0.9);
        assert!(fast < slow, "faster release prices lower: {fast} vs {slow}");
    }
}

// T-R2 (joint3): floored_expected against a plain recurrence.
#[cfg(test)]
mod floored_expected_tests {
    use super::floored_expected;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    /// `floored_expected` equals a plain multiplicative recurrence to 1e-12 for every tenor, so a different lowering of
    /// `powi` (libm table or runtime) can move the model by rounding only.
    #[test]
    fn floored_expected_matches_the_plain_recurrence() {
        let w = tiny_world(Config { n_households: 300, ..Default::default() }, 1);
        let rho = w.cfg.rate_smoothing;
        for &tenor in &[1u32, 3, 12, 24, 25, 60, 120, 360] {
            for &(base, i_star, i_inf) in &[(0.07, 0.03, 0.025), (0.0, 0.02, 0.025), (0.0025, 0.0075, 0.02)] {
                let got = floored_expected(&w, base, i_star, i_inf, tenor);
                let (mut d, mut r, mut sum) = (1.0f64, 1.0f64, 0.0f64);
                for _ in 0..tenor {
                    sum += (i_inf + (i_star - i_inf) * d + (base - i_star) * r).max(0.0);
                    d *= 0.97;
                    r *= rho;
                }
                let want = sum / tenor as f64;
                assert!((got - want).abs() <= 1e-12, "tenor {tenor}: {got} vs {want}");
            }
        }
    }
}

// joint3 (T-R2): why a table. `PATH_DECAY.powi(k)` has a constant base. When LLVM fully unrolls the 24-month tenor loop of
// `update_curve` it folds these powers with libm `pow`; in longer loops it calls `__powidf2` at run time. The two differ in
// the last bits for 18 of the 24 values, and the model amplifies that into another (equally valid) trajectory, so any growth
// of `update_curve` could change the default output. The table holds the libm values for tenors up to 24 months and
// `path_decay_powi` is an opaque runtime `powi` for longer tenors, which is what master computes. The table must stop at
// 24: tables to 3 or to 60 months change the md5 (b7-transmission review F1; docs/CALIBRATION.md, Joint calibration 3).
const PATH_DECAY_POWERS: [f64; 25] = [
    1.0,
    0.97,
    0.9409,
    0.912673,
    0.8852928099999999,
    0.8587340256999999,
    0.8329720049289998,
    0.8079828447811298,
    0.7837433594376959,
    0.7602310586545651,
    0.7374241268949281,
    0.7153014030880802,
    0.6938423609954377,
    0.6730270901655746,
    0.6528362774606074,
    0.6332511891367891,
    0.6142536534626855,
    0.5958260438588049,
    0.5779512625430407,
    0.5606127246667495,
    0.543794342926747,
    0.5274805126389446,
    0.5116560972597762,
    0.49630641434198286,
    0.48141722191172337
];
#[inline(never)]
fn path_decay_powi(k: i32) -> f64 { PATH_DECAY.powi(k) }

// rz21: the rule's look-through of a tariff's first round.
#[cfg(test)]
mod rz21_tests {
    use super::lookthrough_inflation;

    #[test]
    fn lookthrough_is_the_identity_when_off_or_when_no_wedge_moved() {
        for pi in [-0.01, 0.0, 0.02, 0.0537, 0.31] {
            for lt in [0.0, -0.5, f64::NAN] {
                assert_eq!(lookthrough_inflation(lt, pi, 0.0123).to_bits(), pi.to_bits(), "key off");
            }
            for d in [0.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(lookthrough_inflation(0.7, pi, d).to_bits(), pi.to_bits(), "no wedge, or a non-finite one");
            }
        }
    }

    #[test]
    fn lookthrough_removes_the_wedges_share_of_twelve_month_inflation() {
        // the index grew 3.0% in the year, of which the wedge made ln(1.01): the rest grew (1.03 / 1.01) - 1
        let (pi, d) = (0.03, 1.01f64.ln());
        let pi_xt = (1.0 + pi) / 1.01 - 1.0;
        assert!((lookthrough_inflation(1.0, pi, d) - pi_xt).abs() < 1e-15);
        // half a look-through is halfway; the weight is capped at 1
        assert!((lookthrough_inflation(0.5, pi, d) - 0.5 * (pi + pi_xt)).abs() < 1e-15);
        assert_eq!(lookthrough_inflation(3.0, pi, d).to_bits(), lookthrough_inflation(1.0, pi, d).to_bits());
        // a falling wedge (a duty cut) is looked through the other way: the rule reads more than the index shows
        assert!(lookthrough_inflation(1.0, pi, -d) > pi);
    }
}
