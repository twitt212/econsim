//! Exchange rate and the t=0 base of the exchange-rate channel (docs/DESIGN.md §11.4). Called from the monthly
//! step after the yield curve; `monetary` re-exports the public items.

use crate::common;
use crate::types::*;

/// t=0 reference values for the exchange-rate channel: the import price and the domestic-to-import terms ratio
/// (the ratio the import shares respond to), both captured in the first month of the world.
#[derive(Clone, Copy, Debug)]
pub struct FxBase {
    /// World price over the dollar index, p_world / fx, at t=0.
    pub import_price: f64,
    /// Domestic-to-import price ratio fx × P_s / p_world at t=0, by sector.
    pub terms: [f64; N_SECTORS],
}

impl FxBase {
    /// Base values for a world whose exchange rate is `fx` and whose world price level is `p_world`.
    fn capture(w: &World, fx: f64, p_world: f64) -> FxBase {
        FxBase {
            import_price: p_world / fx,
            terms: std::array::from_fn(|s| fx * common::sector_price(w, Sector::from_idx(s)) / p_world),
        }
    }
}

/// Base values, or the neutral base (t=0 prices of 1.0) for a world the exchange-rate channel has not stepped yet.
pub fn fx_base(w: &World) -> FxBase {
    w.monetary.fx_base.unwrap_or(FxBase { import_price: 1.0, terms: [1.0; N_SECTORS] })
}

/// Captures the FX base once, from the rate and the world price of the first month that reaches it (`exchange_rate`, or
/// `capture_fx_base` in dollar mode). Non-finite or non-positive inputs leave the base unset for a later month.
fn capture_base(w: &mut World, p_world: f64) {
    let fx_now = w.row.fx;
    if w.monetary.fx_base.is_none() && fx_now.is_finite() && fx_now > 0.0 && p_world > 0.0 {
        w.monetary.fx_base = Some(FxBase::capture(w, fx_now, p_world));
    }
}

/// Stage 4-1 (review item D9): the FX base captured in dollar mode, where hook H2 skips `exchange_rate` (`world::dollar_active`).
/// The same capture as `exchange_rate` makes, at the same point of the month, so a burn-in-zero world with the dollar link on
/// holds the base an otherwise identical link-off world holds (the neutral base when the initial prices are 1.0).
pub fn capture_fx_base(w: &mut World) {
    let p_world = common::world_price(w);
    capture_base(w, p_world);
}

/// Exchange rate (trade-weighted dollar index, `w.row.fx`), once per month after the yield curve.
///
/// Uncovered interest parity with a purchasing-power anchor: ln fx* = ln(p_world / p_us)
/// + κ × (r_us − r_world), where p_us is the CPI level relative to its base (100), r_us is the US real 2y rate
/// (2y yield less mean household expected inflation) and r_world is the initial 2y yield less the inflation
/// target. The log rate moves a fraction `fx_adjust` of the way to fx* each month. Dollar shocks add a decaying
/// log shift to the target. Non-finite results keep the previous rate.
///
/// p1-fx: the dollar risk premium enters the level through q_t = (1 − w)ψ_t + wψ_{t−1} (`fx_psi_weight`), where ψ is
/// the anchored premium of `anchored_premium` (zero through burn-in, so the reported run starts at the premium-free
/// level). The partial adjustment acts on the premium-free part of ln fx, ln fx − q_{t−1} (see `psi_step`).
pub fn exchange_rate(w: &mut World) {
    let p_world = common::world_price(w);
    capture_base(w, p_world);
    let fx_now = w.row.fx;
    let p_us = if w.cfg.fx_ppp_tradables {
        // US goods price relative to its t=0 level (the base the import-price terms use).
        let tw = &w.cfg.tradable_weight;
        let base = w.monetary.fx_base.map(|b| role_mean(tw, |s| b.terms[s]) * b.import_price).unwrap_or(0.0);
        let pg = role_mean(tw, |s| common::sector_price(w, Sector::from_idx(s)));
        if base > 0.0 { pg / base } else { w.prices.cpi / 100.0 }
    } else {
        w.prices.cpi / 100.0
    };
    if !(p_us.is_finite() && p_us > 0.0) || !(fx_now.is_finite() && fx_now > 0.0) {
        return;
    }
    let r_us = w.curve.yields[1] - w.monetary.pi_e;
    // The rest-of-world real rate equals the US real 2-year rate of the calibrated initial state (initial yield less
    // the initial expected inflation households start with), so the dollar starts at its fundamental value and moves
    // with the US real-rate differential from there. Fixed by configuration, so shocks cannot shift it.
    let r_world = w.cfg.initial_curve[1] - w.cfg.initial_inflation;
    // b7-transmission M-F: large rate differentials saturate (identity unless `fx_rate_sat_scale` > 0).
    let r_diff = bounded_rate_differential(r_us - r_world, w.cfg.fx_rate_knee, w.cfg.fx_rate_sat_scale);
    let ln_target = w.cfg.fx_ppp_weight * (p_world / p_us).ln()
        + w.cfg.fx_rate_sensitivity * r_diff
        + w.shocks.dollar_log();
    // p1-fx: the partial adjustment acts on the premium-free part of ln fx, then this month's premium-driven part q_t
    // (from the anchored premium ψ_t and ψ_{t−1}) is added. With ψ off every premium term is zero, so the step is
    // bit-identical to the rule without ψ.
    let psi = anchored_premium(
        w.fx_psi_state,
        w.fx_psi_exog_prev,
        w.exog.fx_psi(),
        w.cfg.fx_psi_rho,
        w.tick_offset == 0 && w.tick < w.cfg.burn_in_months,
    );
    // b7-transmission M-X: the monetary-surprise premium z_t joins the premium the step sees (ψ_t + z_t against
    // ψ_{t−1} + z_{t−1}). With κ = 0 the call is the legacy one, untouched.
    let mp_on = w.cfg.fx_mp_premium != 0.0;
    let z = if mp_on {
        monetary_surprise_premium(w.cfg.fx_mp_premium, w.cfg.fx_mp_cap, w.shocks.policy_gap(w.tick))
    } else {
        0.0
    };
    let (ln_fx, q) = if mp_on {
        psi_step(
            fx_now.ln(),
            w.fx_psi_component,
            w.fx_psi_state + w.monetary.fx_mp_prev,
            ln_target,
            psi + z,
            w.cfg.fx_adjust,
            w.cfg.fx_psi_weight,
        )
    } else {
        psi_step(
            fx_now.ln(),
            w.fx_psi_component,
            w.fx_psi_state,
            ln_target,
            psi,
            w.cfg.fx_adjust,
            w.cfg.fx_psi_weight,
        )
    };
    let fx = ln_fx.exp();
    if fx.is_finite() && fx > 0.0 {
        w.row.fx = fx;
        w.fx_psi_state = psi;
        w.fx_psi_component = q;
        if mp_on {
            w.monetary.fx_mp_prev = z;
        }
    }
    // The exogenous premium's last value is kept whether or not the rate was applied, so the next innovation is exact.
    w.fx_psi_exog_prev = w.exog.fx_psi();
}

/// p1-fx: the anchored dollar risk premium ψ'_t. The exogenous premium ψ_t (`Exogenous::fx_psi`, AR(1) with persistence
/// ρ = `fx_psi_rho`) is a near-unit-root process whose burn-in path would leave a random dollar offset at reported
/// month 0 (and so random trade shares and policy rates across seeds). Only its innovations are used:
/// ψ'_t = ρψ'_{t−1} + (ψ_t − ρψ_{t−1}), which is zero through burn-in (`burn_in`), so the reported run starts with no
/// premium and builds one from its own shocks. For the exogenous AR(1) the innovation is σ·z_t exactly.
pub fn anchored_premium(state_prev: f64, exog_prev: f64, exog: f64, rho: f64, burn_in: bool) -> f64 {
    if burn_in { 0.0 } else { rho * state_prev + (exog - rho * exog_prev) }
}

/// p1-fx: one step of ln fx with the dollar risk premium. Inputs: last month's ln fx, its premium-driven part `q_prev`
/// (q_{t−1}), last month's premium ψ_{t−1}; this month's target and ψ_t; the adjustment speed `a` and the weight `w` of
/// ψ_{t−1} in q. Returns the new ln fx and the new premium-driven part.
///
/// q_t = (1 − w)ψ_t + wψ_{t−1}, and ln fx_t = (ln fx_{t−1} − q_{t−1}) + a(ln target_t − (ln fx_{t−1} − q_{t−1})) + q_t.
/// Writing F_t = ln fx_t − q_t, the premium-free level, gives F_t = (1 − a)F_{t−1} + a ln target_t: the same path the
/// rule gives without any premium. So `q` is the part of ln fx that the premium explains, and the premium moves the
/// level in full (no pullback), with the two-month weighting w setting the monthly AR(1) of changes.
pub fn psi_step(ln_fx_prev: f64, q_prev: f64, psi_prev: f64, ln_target: f64, psi: f64, a: f64, w: f64) -> (f64, f64) {
    let q = (1.0 - w) * psi + w * psi_prev;
    let ln_fund = ln_fx_prev - q_prev;
    let ln_fx = ln_fund + a * (ln_target - ln_fund) + q;
    (ln_fx, q)
}

/// b7-transmission M-F: the rate differential d = r_us − r_world, bounded above the knee k: d ← k + L·tanh((d − k)/L) for
/// d > k when `scale` L is positive. The identity below k and when L is zero, so the legacy path is bit-identical.
/// Literature: Volcker-episode dollar (+18% in 12 months) against the pinned 10% case (the response is bounded).
pub fn bounded_rate_differential(d: f64, knee: f64, scale: f64) -> f64 {
    if !(scale > 0.0) || !d.is_finite() || d <= knee {
        return d;
    }
    knee + scale * ((d - knee) / scale).tanh()
}

/// b7-transmission M-X: monetary-surprise premium z_t = κ·L·tanh(gap_t / L) (log points), with κ = `kappa` (the
/// `fx_mp_premium` key), L = `cap` (`fx_mp_cap`) and gap_t the decimal policy gap. Bounded by |κ|·L; zero when κ = 0 or
/// L is not positive. A tightening (positive gap) gives a positive premium, so the dollar appreciates on impact.
pub fn monetary_surprise_premium(kappa: f64, cap: f64, gap: f64) -> f64 {
    if kappa == 0.0 || !(cap > 0.0) || !gap.is_finite() {
        return 0.0;
    }
    kappa * cap * (gap / cap).tanh()
}

#[cfg(test)]
mod tests {
    use super::{anchored_premium, monetary_surprise_premium, psi_step};

    /// κ = 0 gives no premium, whatever the gap.
    #[test]
    fn zero_kappa_gives_no_monetary_surprise_premium() {
        assert_eq!(monetary_surprise_premium(0.0, 0.03, 0.02), 0.0);
    }

    /// The premium has the sign of the gap (tightening appreciates) and is bounded by κ·L.
    #[test]
    fn monetary_surprise_premium_is_signed_and_bounded() {
        let (kappa, cap) = (1.25, 0.03);
        let up = monetary_surprise_premium(kappa, cap, 0.01);
        let down = monetary_surprise_premium(kappa, cap, -0.01);
        assert!(up > 0.0 && down < 0.0);
        assert!((up + down).abs() < 1e-15, "odd in the gap");
        for gap in [0.001, 0.01, 0.05, 0.5, 5.0] {
            let z = monetary_surprise_premium(kappa, cap, gap);
            assert!(z.abs() <= kappa * cap + 1e-15, "gap {gap}: {z}");
        }
        // Near-linear for small gaps: z ≈ κ·gap.
        assert!((monetary_surprise_premium(kappa, cap, 1e-6) - kappa * 1e-6).abs() < 1e-12);
    }

    /// The premium-free part F = ln fx − q follows the premium-free recursion when the total premium ψ + z drives the
    /// step, so the monetary premium moves the level without touching the partial adjustment.
    #[test]
    fn residual_recursion_holds_with_monetary_premium() {
        let (a, w) = (0.3, 0.28);
        let mut x = 0.2_f64.ln();
        let (mut q, mut psi_prev) = (0.0, 0.0);
        let mut free = x;
        let mut seed: u64 = 777;
        for t in 0..300 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let u = (seed >> 11) as f64 / (1u64 << 53) as f64;
            let target = (0.9 + 0.2 * u).ln();
            let psi = 0.03 * (u - 0.5);
            let z = monetary_surprise_premium(1.25, 0.03, 0.02 * (t as f64 / 40.0).sin());
            let (nx, nq) = psi_step(x, q, psi_prev, target, psi + z, a, w);
            free = (1.0 - a) * free + a * target;
            x = nx;
            q = nq;
            psi_prev = psi + z;
            assert!((x - q - free).abs() < 1e-12, "month {t}");
        }
    }

    /// Through burn-in the anchored premium is zero, whatever the exogenous path; from month 0 it is the AR(1) of the
    /// exogenous innovations, so it does not inherit the burn-in level.
    #[test]
    fn anchored_premium_starts_at_zero_after_burn_in() {
        let (rho, sigma) = (0.999, 0.0215);
        let mut exog = 1.7; // a large burn-in offset of the exogenous process
        let mut exog_prev;
        let mut state = 0.0;
        let mut seed: u64 = 99;
        for t in 0..144 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let z = (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            exog_prev = exog;
            exog = rho * exog + sigma * z;
            state = anchored_premium(state, exog_prev, exog, rho, true);
            assert_eq!(state, 0.0, "burn-in month {t}");
        }
        // Reported month 0: the state is the first innovation alone.
        let z0 = 0.3;
        let next = rho * exog + sigma * z0;
        let s0 = anchored_premium(0.0, exog, next, rho, false);
        assert!((s0 - sigma * z0).abs() < 1e-15);
    }

    /// The premium-driven part tracks ln fx exactly: ln fx − q follows the premium-free recursion.
    #[test]
    fn premium_component_is_exact_residual() {
        let (a, w) = (0.3, 0.28);
        let mut x = 0.2_f64.ln();
        let (mut q, mut psi_prev) = (0.0, 0.0);
        let mut free = x;
        let mut seed: u64 = 12345;
        for t in 0..500 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let u = (seed >> 11) as f64 / (1u64 << 53) as f64;
            let target = (0.9 + 0.2 * u).ln() + 0.001 * (t as f64 / 50.0).sin();
            let psi = 0.03 * (u - 0.5);
            let (nx, nq) = psi_step(x, q, psi_prev, target, psi, a, w);
            free = (1.0 - a) * free + a * target;
            x = nx;
            q = nq;
            psi_prev = psi;
            assert!((x - q - free).abs() < 1e-12, "month {t}");
        }
    }

    /// q_t is the two-month weighted premium (1 − w)ψ_t + wψ_{t−1}.
    #[test]
    fn premium_component_is_two_month_weighted() {
        let (_, q) = psi_step(0.0, 0.0, 0.5, 0.0, 0.2, 0.3, 0.28);
        assert!((q - (0.72 * 0.2 + 0.28 * 0.5)).abs() < 1e-15);
    }

    /// With ψ identically zero the step is the premium-free rule and q stays at zero.
    #[test]
    fn zero_premium_is_plain_partial_adjustment() {
        let (x, q) = psi_step(0.1, 0.0, 0.0, 0.2, 0.0, 0.3, 0.28);
        assert!((x - (0.7 * 0.1 + 0.3 * 0.2)).abs() < 1e-15);
        assert_eq!(q, 0.0);
    }
}

// b7-transmission M-F tests.
#[cfg(test)]
mod bounded_differential_tests {
    use super::bounded_rate_differential;

    #[test]
    fn identity_below_the_knee_and_when_off() {
        assert_eq!(bounded_rate_differential(0.01, 0.02, 0.015), 0.01);
        assert_eq!(bounded_rate_differential(0.05, 0.02, 0.0), 0.05);
    }

    #[test]
    fn bounded_above_and_monotone() {
        let (k, l) = (0.02, 0.015);
        let mut prev = f64::NEG_INFINITY;
        for i in 0..=200 {
            let d = 0.0 + 0.0005 * i as f64; // 0 to 10pp
            let b = bounded_rate_differential(d, k, l);
            assert!(b >= prev - 1e-15);
            assert!(b <= k + l + 1e-12, "above the bound at d {d}: {b}");
            prev = b;
        }
        // Continuous at the knee.
        assert!((bounded_rate_differential(k, k, l) - k).abs() < 1e-15);
    }
}
