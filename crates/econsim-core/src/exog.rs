//! Exogenous block (docs/DESIGN.md §4): shock activation, oil price, productivity trend.

use rand::Rng;

use crate::config::Config;
use crate::rng::{self, stream};
use crate::types::*;

/// Sectors that adopt a tech boom's productivity (Durable goods and Other services; Jorgenson, Ho & Stiroh 2008).
const ADOPTER_SECTORS: [usize; 2] = [2, 7];
/// Index of the producing sector (Tech) in Sector::ALL order.
const TECH: usize = 4;

/// Monthly update of the tech-boom Markov regime (normal ↔ boom). Draws one uniform from `r` (only called when the
/// regime is enabled, so a disabled regime leaves the EXOG stream untouched).
pub fn next_tech_regime(prev_boom: bool, entry: f64, exit: f64, r: &mut rng::SimRng) -> bool {
    let u: f64 = r.gen();
    if prev_boom {
        u >= exit
    } else {
        u < entry
    }
}

/// Financial risk factor: volatility multiplier exp(γ·f) is clamped to this range (p0-exog).
pub const FIN_VOL_MULT_RANGE: (f64, f64) = (0.2, 5.0);
/// Smallest Student-t degrees of freedom used: unit variance needs ν > 2, and ν/2 ≥ 1 keeps the gamma sampler simple.
/// Below ν = 4 the untruncated t has infinite kurtosis, so the unit-variance draw is winsorised at `OIL_SHOCK_CAP` sd
/// (see `oil_month`). The winsorised variance is then below 1 (0.73 at ν = 2.5, 0.98 at ν = 4), but it stays finite.
pub const T_NU_MIN: f64 = 2.5;
/// Oil conditional sd is clamped to `OIL_VOL_MULT_RANGE` × the long-run sd `oil_sigma` (p0-exog), the same range as the
/// financial factor. The GARCH variance is stored after the clamp, so the recursion cannot wind up past the bound. The
/// floor binds only when α + β is close to 1 (the GARCH intercept is σ²(1−α−β)). The cap is 5σ: a 3σ cap distorts the
/// Bollerslev ε² autocorrelation at α = 0.15, β = 0.80 (0.26 instead of 0.30), while 5σ leaves it unchanged.
pub const OIL_VOL_MULT_RANGE: (f64, f64) = (FIN_VOL_MULT_RANGE.0, FIN_VOL_MULT_RANGE.1);
/// Cap on the oil log deviation |oil_ar| (p0-exog), so the level stays within a factor of e^{1.5} ≈ 4.5 of the anchor.
/// It applies only when an extension is on (Student-t, GARCH or jumps); the plain Gaussian path is never capped, so it
/// is the pre-p0 recursion for any σ and ρ. At the base parameters (σ = 0.06, ρ = 0.95, stationary sd 0.19) the cap is
/// about 8 sd and never binds in practice.
pub const OIL_LOG_DEV_CAP: f64 = 1.5;
/// Winsorisation of the unit-variance Student-t oil draw, in sd units (p0-exog). Only Student-t draws are capped; the
/// Gaussian draw is untouched, so the pre-p0 path is unchanged. The cut is 0.035% of draws under t(4) and 0.007% under t(6).
pub const OIL_SHOCK_CAP: f64 = 8.0;

/// Parameters of the financial risk factor (`Config::fin_*`).
#[derive(Clone, Copy, Debug, Default)]
pub struct FinSpec {
    pub rho: f64,
    pub sigma: f64,
    pub gamma: f64,
    pub jump_intensity: f64,
    pub jump_mean: f64,
    pub jump_sd: f64,
}

/// One month of the financial risk factor (p0-exog). f_t = ρ·f_{t−1} + σ·exp(γ·f_{t−1})·clamp·ε_t + J_t, where J_t is
/// a Poisson number of N(μ_J, σ_J) jumps. `eps` is the month's N(0,1) draw, and the jump draws come from `r` after it.
/// Returns (f_t, number of jumps, J_t = sum of this month's jump sizes). With σ = 0 and no jumps the factor decays to 0;
/// γ = 0 gives constant volatility.
pub fn fin_factor_step<R: Rng + ?Sized>(prev: f64, eps: f64, s: &FinSpec, r: &mut R) -> (f64, u32, f64) {
    let mult = (s.gamma * prev).exp().clamp(FIN_VOL_MULT_RANGE.0, FIN_VOL_MULT_RANGE.1);
    let mut f = s.rho * prev + s.sigma * mult * eps;
    let n = poisson(r, s.jump_intensity);
    let mut jump_sum = 0.0;
    for _ in 0..n {
        let j = s.jump_mean + s.jump_sd * rng::normal(r);
        f += j;
        jump_sum += j;
    }
    (f, n, jump_sum)
}

/// Unconditional mean of the financial factor, f̄ = λμ/(1 − ρ) (re-fit Step 10a). Exact while fin_factor_vol_gamma = 0.
pub fn fin_factor_mean(c: &crate::config::Config) -> f64 {
    c.fin_jump_intensity * c.fin_jump_mean / (1.0 - c.fin_factor_rho)
}

/// One month of a term-premium factor or the dollar risk premium (p0-exog): x_t = ρ·x_{t−1} + σ·z_t, with `z` the
/// standardised innovation.
pub fn ar1_step(prev: f64, rho: f64, sigma: f64, z: f64) -> f64 {
    rho * prev + sigma * z
}

/// Dollar risk-premium innovation correlated with the financial factor's innovation (p0-exog): the standardised shock
/// is corr·ε_f + √(1 − corr²)·u, so corr(ψ innovation, f innovation) = corr.
pub fn psi_innovation(corr: f64, eps_fin: f64, u: f64) -> f64 {
    let c = corr.clamp(-1.0, 1.0);
    c * eps_fin + (1.0 - c * c).sqrt() * u
}

/// Parameters of the oil innovation (p0-exog). `nu = 0` is Gaussian, and `alpha = beta = 0` is the constant `sigma`.
#[derive(Clone, Copy, Debug, Default)]
pub struct OilSpec {
    pub sigma: f64,
    pub nu: f64,
    pub alpha: f64,
    pub beta: f64,
    pub jump_intensity: f64,
    pub jump_sd: f64,
}

/// GARCH state carried between months (p0-exog). `cond_var` is last month's conditional variance (0 = not yet set,
/// which starts the recursion at the long-run variance). `prev_shock_sq` is last month's squared innovation.
#[derive(Clone, Copy, Debug, Default)]
pub struct OilVolState {
    pub cond_var: f64,
    pub prev_shock_sq: f64,
}

/// One month of the oil log-price AR(1) (p0-exog).
#[derive(Clone, Copy, Debug, Default)]
pub struct OilMonth {
    /// New oil AR state (log deviation).
    pub ar: f64,
    /// The month's innovation before the AR and jumps (sd × unit-variance draw).
    pub shock: f64,
    /// Conditional sd of the innovation.
    pub sd: f64,
    /// Number of jumps this month.
    pub jumps: u32,
}

/// Oil spec from the configuration (p0-exog).
///
/// Panics unless α, β ≥ 0 and α + β < 1 (a GARCH with α + β ≥ 1 has no finite long-run variance).
pub fn oil_spec(c: &Config) -> OilSpec {
    assert!(
        c.oil_garch_alpha >= 0.0 && c.oil_garch_beta >= 0.0 && c.oil_garch_alpha + c.oil_garch_beta < 1.0,
        "oil GARCH needs alpha, beta >= 0 and alpha + beta < 1 (got {} and {})",
        c.oil_garch_alpha,
        c.oil_garch_beta
    );
    OilSpec {
        sigma: c.oil_sigma,
        nu: if c.oil_t_nu > 0.0 { c.oil_t_nu.max(T_NU_MIN) } else { 0.0 },
        alpha: c.oil_garch_alpha,
        beta: c.oil_garch_beta,
        jump_intensity: c.oil_jump_intensity,
        jump_sd: c.oil_jump_sd,
    }
}

/// One month of the oil AR(1) in logs (p0-exog). `z` is the base N(0,1) draw. The Student-t scale draws come from `r_t`
/// (`stream::OIL_EXT`) and the jump draws from `r_j` (`stream::OIL_JUMP`), so changing ν never redraws the jumps. With the
/// defaults (ν = 0, GARCH off, no jumps) the result is exactly `rho·prev + sigma·z`, the pre-p0 recursion, for any σ and ρ.
///
/// Student-t: z·√((ν−2)/V) with V ~ χ²(ν), which is t(ν) rescaled to unit variance, winsorised at ±`OIL_SHOCK_CAP`.
/// GARCH(1,1): h_t = σ²(1−α−β) + α ε²_{t−1} + β h_{t−1}, so the long-run variance is σ². The conditional variance is
/// clamped to `OIL_VOL_MULT_RANGE`² × σ² before it is stored. The new log deviation is capped at ±`OIL_LOG_DEV_CAP` when
/// an extension (t, GARCH or jumps) is on.
pub fn oil_month<R: Rng + ?Sized>(
    s: &OilSpec,
    rho: f64,
    prev_ar: f64,
    vs: &mut OilVolState,
    z: f64,
    r_t: &mut R,
    r_j: &mut R,
) -> OilMonth {
    let garch = s.alpha + s.beta > 0.0;
    let lr = s.sigma * s.sigma;
    let sd = if garch {
        let h_prev = if vs.cond_var > 0.0 { vs.cond_var } else { lr };
        let h_raw = (lr * (1.0 - s.alpha - s.beta) + s.alpha * vs.prev_shock_sq + s.beta * h_prev).max(0.0);
        let (lo, hi) = (OIL_VOL_MULT_RANGE.0 * OIL_VOL_MULT_RANGE.0, OIL_VOL_MULT_RANGE.1 * OIL_VOL_MULT_RANGE.1);
        let h = (h_raw.clamp(lo * lr, hi * lr)).max(0.0);
        vs.cond_var = h;
        h.sqrt()
    } else {
        s.sigma
    };
    let zz = if s.nu > 0.0 {
        (z * ((s.nu - 2.0) / chi_sq(s.nu, r_t)).sqrt()).clamp(-OIL_SHOCK_CAP, OIL_SHOCK_CAP)
    } else {
        z
    };
    let shock = sd * zz;
    if garch {
        vs.prev_shock_sq = shock * shock;
    }
    let jumps = poisson(r_j, s.jump_intensity);
    let mut ar = rho * prev_ar + shock;
    if jumps > 0 {
        let jump: f64 = (0..jumps).map(|_| s.jump_sd * rng::normal(r_j)).sum();
        ar += jump;
    }
    // The level cap is part of the extensions only, so the plain Gaussian path is the pre-p0 recursion for any σ, ρ.
    let extended = s.nu > 0.0 || garch || s.jump_intensity > 0.0;
    let ar = if extended { ar.clamp(-OIL_LOG_DEV_CAP, OIL_LOG_DEV_CAP) } else { ar };
    OilMonth { ar, shock, sd, jumps }
}

/// Poisson draw by Knuth's multiplication method (p0-exog). Returns 0 without drawing when λ ≤ 0.
pub fn poisson<R: Rng + ?Sized>(r: &mut R, lambda: f64) -> u32 {
    if !(lambda > 0.0) {
        return 0;
    }
    let limit = (-lambda).exp();
    let mut k = 0u32;
    let mut p = 1.0;
    loop {
        p *= r.gen::<f64>();
        if p <= limit || k >= 1000 {
            return k;
        }
        k += 1;
    }
}

/// χ²(ν) draw for ν ≥ 2.5 (p0-exog): 2·Gamma(ν/2), with Marsaglia & Tsang (2000) for shape ≥ 1.
fn chi_sq<R: Rng + ?Sized>(nu: f64, r: &mut R) -> f64 {
    2.0 * gamma_ge1(r, 0.5 * nu)
}

fn gamma_ge1<R: Rng + ?Sized>(r: &mut R, k: f64) -> f64 {
    let d = k - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x = rng::normal(r);
        let v0 = 1.0 + c * x;
        if v0 <= 0.0 {
            continue;
        }
        let v = v0 * v0 * v0;
        let u: f64 = r.gen();
        if u < 1.0 - 0.0331 * x * x * x * x || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
            return d * v;
        }
    }
}

/// Monthly update of the p0-exog factors: financial risk factor f_t, term-premium level and curvature factors, and the
/// dollar risk premium ψ_t. Each process draws from its own registered stream (rng::stream), never from EXOG, so a
/// process that is off leaves every other path unchanged. With all sigmas at 0 the factors stay at 0.
fn factors_step(w: &mut World, tick: u32) {
    let seed = w.seed;
    let c = &w.cfg;

    let fin = FinSpec {
        rho: c.fin_factor_rho,
        sigma: c.fin_factor_sigma,
        gamma: c.fin_factor_vol_gamma,
        jump_intensity: c.fin_jump_intensity,
        jump_mean: c.fin_jump_mean,
        jump_sd: c.fin_jump_sd,
    };
    let mut rf = rng::module_rng(seed, tick, stream::FIN_FACTOR);
    // The diffusion draw is always taken, so the innovation exists for the ψ correlation even if f is off.
    let eps = rng::normal(&mut rf);
    let (f, n, jump_sum) = fin_factor_step(w.exog.fin_factor, eps, &fin, &mut rf);
    w.exog.fin_factor = f;
    w.exog.fin_factor_eps = eps;
    w.exog.fin_jump_n = n;

    let mut rl = rng::module_rng(seed, tick, stream::TERM_LEVEL);
    let zl = rng::normal(&mut rl);
    // Flight to quality: the level factor also loads on the current financial factor (default loading 0).
    // The AR(1) state is kept apart from the loading, so the loading is not compounded by ρ (it is a loading on f_t).
    w.exog.tp_level_ar = ar1_step(w.exog.tp_level_ar, c.tp_level_rho, c.tp_level_sigma, zl);
    w.exog.tp_level = w.exog.tp_level_ar + c.tp_level_fin_loading * f;

    let mut rc = rng::module_rng(seed, tick, stream::TERM_CURV);
    let zc = rng::normal(&mut rc);
    w.exog.tp_curv = ar1_step(w.exog.tp_curv, c.tp_curv_rho, c.tp_curv_sigma, zc);

    let mut rp = rng::module_rng(seed, tick, stream::FX_PSI);
    let u = rng::normal(&mut rp);
    let shock = psi_innovation(c.fx_psi_fin_corr, eps, u);
    let mut psi = ar1_step(w.exog.fx_psi, c.fx_psi_rho, c.fx_psi_sigma, shock);
    // Optional response to f's jumps (default loading 0, so the default path is untouched).
    if c.fx_psi_fin_jump_loading != 0.0 {
        psi += c.fx_psi_fin_jump_loading * jump_sum;
    }
    w.exog.fx_psi = psi;
}

/// rz19: the effective duty on imported inputs and equipment as the wedge reads it (a decimal, finite, never below 0).
fn tariff_tfp_duty(w: &World) -> f64 {
    let tau = crate::world::tau_for(w, crate::world::Channel::Firm);
    if tau.is_finite() {
        tau.max(0.0)
    } else {
        0.0
    }
}

/// rz19: takes the reference duty the wedge is measured against, before this month's shocks are activated: the duty in force in the first
/// month that is not part of a steered window (in a Today's start the window follows the data, so today's duty is the reference and
/// carries no wedge; in a typical start nothing precedes the run and the reference is the duty of the first month, which is 0). A no-op
/// with the key off.
fn tariff_tfp_reference(w: &mut World) {
    if !(w.cfg.tariff_tfp_loss > 0.0) {
        return;
    }
    if crate::start::window_active(w) {
        w.exog.tariff_tfp_ref_set = false;
        return;
    }
    if !w.exog.tariff_tfp_ref_set {
        w.exog.tariff_tfp_ref = tariff_tfp_duty(w);
        w.exog.tariff_tfp_ref_set = true;
    }
}

/// rz19: one month of the tariff efficiency wedge. The target is `-tariff_tfp_loss * tau` in logs, with `tau` the effective duty on
/// imported inputs and equipment (`world::tau_for(w, Channel::Firm)`) above the reference duty of the run, as a decimal and never below
/// 0. The state moves toward the target by at most `peak / tariff_tfp_phase_in_months` a month, `peak` being the largest |target| seen
/// so far: a linear ramp that takes the same number of months for a tariff of any size, and phases out at the speed it phased in.
/// Returns the factor to apply to every sector's productivity this month, or `None` when the key is off, the month belongs to a
/// steered window, or the state did not move (so a run without a tariff multiplies nothing and stays bit-identical).
fn tariff_tfp_step(w: &mut World) -> Option<f64> {
    let kappa = w.cfg.tariff_tfp_loss;
    if !(kappa > 0.0) || crate::start::window_active(w) || !w.exog.tariff_tfp_ref_set {
        return None;
    }
    let tau = (tariff_tfp_duty(w) - w.exog.tariff_tfp_ref).max(0.0);
    let target = -kappa * tau;
    let state = w.exog.tariff_tfp_log;
    let peak = w.exog.tariff_tfp_peak.max(target.abs());
    w.exog.tariff_tfp_peak = peak;
    let step = peak / w.cfg.tariff_tfp_phase_in_months.max(1.0);
    // the new state is never above zero (a duty cut gives no gain), and the factor applied is exactly the change of the stored state
    let next = (state + (target - state).clamp(-step, step)).min(0.0);
    let ds = next - state;
    if ds == 0.0 {
        return None;
    }
    w.exog.tariff_tfp_log = next;
    Some(ds.exp())
}

pub fn step(w: &mut World) {
    let tick = w.tick;
    // rz19: the wedge's reference duty is read before this month's shocks apply (a no-op with the key off)
    tariff_tfp_reference(w);
    w.shocks.activate_due(tick);

    // p0-exog: financial, term-premium and dollar factors (their own streams; before oil so ψ sees this month's ε).
    factors_step(w, tick);

    // Oil: AR(1) in logs + decaying user shock. The base Gaussian draw stays on EXOG (first draw of the month), so
    // the risk-premium and TFP draws below are unchanged by the p0-exog oil options.
    let mut r = rng::module_rng(w.seed, tick, stream::EXOG);
    if w.cfg.stochastic_oil {
        let z = rng::normal(&mut r);
        let spec = oil_spec(&w.cfg);
        let mut vs = OilVolState { cond_var: w.exog.oil_cond_var, prev_shock_sq: w.exog.oil_prev_shock_sq };
        let mut ext_t = rng::module_rng(w.seed, tick, stream::OIL_EXT);
        let mut ext_j = rng::module_rng(w.seed, tick, stream::OIL_JUMP);
        let m = oil_month(&spec, w.cfg.oil_ar, w.exog.oil_ar, &mut vs, z, &mut ext_t, &mut ext_j);
        w.exog.oil_ar = m.ar;
        w.exog.oil_vol = m.sd;
        w.exog.oil_jump_n = m.jumps;
        w.exog.oil_cond_var = vs.cond_var;
        w.exog.oil_prev_shock_sq = vs.prev_shock_sq;
    }
    w.exog.oil_shock = w.shocks.oil_log_shock;
    // p0-exog: the price-level index sits in the oil price, while oil_base stays the anchor that oil_ratio divides by,
    // so the index reaches firms' crude cost, the Energy oil leg, extraction labour and the world energy price.
    let index = crate::common::oil_index(w);
    crate::start::oil_state(w, index); // us-now: steered window only
    // b8-world H3 (stage 3): with the oil link acting, the world oil market sets the price (world::after_exog); the legacy
    // draws above are still taken, so every later EXOG draw is unchanged.
    if !crate::world::oil_active(w) {
        w.exog.oil_price = w.exog.oil_base * index * (w.exog.oil_ar + w.exog.oil_shock).exp();
    }
    // Re-fit Step 10a: the demand shock is an AR(1) state (rp_ar) plus, when rp_fin_loading is on, a loading on the
    // centred financial factor. With the loading at 0 the total equals the AR state, which is the legacy path.
    if w.cfg.risk_premium_sigma > 0.0 {
        let rp = w.cfg.risk_premium_ar * w.exog.rp_ar + w.cfg.risk_premium_sigma * rng::normal(&mut r);
        w.exog.rp_ar = rp.clamp(-0.05, 0.05);
    }
    w.exog.risk_premium = if w.cfg.rp_fin_loading != 0.0 {
        let f_bar = fin_factor_mean(&w.cfg);
        (w.exog.rp_ar + w.cfg.rp_fin_loading * (w.exog.fin_factor - f_bar)).clamp(-0.05, 0.05)
    } else {
        w.exog.rp_ar
    };
    let tfp_shock = if w.cfg.tfp_shock_sigma > 0.0 { (w.cfg.tfp_shock_sigma * rng::normal(&mut r)).exp() } else { 1.0 };

    // Technology-boom regime (stage 4): a two-state Markov chain, drawn only when enabled.
    if w.cfg.tech_boom_enabled {
        w.exog.tech_regime = next_tech_regime(
            w.exog.tech_regime,
            w.cfg.tech_boom_entry_monthly,
            w.cfg.tech_boom_exit_monthly,
            &mut r,
        );
    }
    w.exog.tech_boom = w.exog.tech_regime || w.shocks.tech_boom_forced(tick);
    w.exog.tech_boom_months = if w.exog.tech_boom { w.exog.tech_boom_months.saturating_add(1) } else { 0 };

    // Productivity: trend growth plus pending level shocks.
    let mut mult: [f64; N_SECTORS] = std::array::from_fn(|s| {
        (1.0 + w.cfg.tfp_growth_annual + w.cfg.tfp_sector_offset[s]).powf(1.0 / 12.0) * tfp_shock
    });
    if w.exog.tech_boom {
        mult[TECH] *= (1.0 + w.cfg.tech_boom_extra_growth).powf(1.0 / 12.0);
        for s in ADOPTER_SECTORS {
            mult[s] *= (1.0 + w.cfg.tech_boom_spillover).powf(1.0 / 12.0);
        }
    }
    // Deterministic trend-plus-boom growth (the common TFP shock is excluded so the metric is not monthly noise).
    w.exog.tfp_growth_m = std::array::from_fn(|s| {
        let trend = (1.0 + w.cfg.tfp_growth_annual + w.cfg.tfp_sector_offset[s]).powf(1.0 / 12.0);
        let boom = if !w.exog.tech_boom {
            1.0
        } else if s == TECH {
            (1.0 + w.cfg.tech_boom_extra_growth).powf(1.0 / 12.0)
        } else if ADOPTER_SECTORS.contains(&s) {
            (1.0 + w.cfg.tech_boom_spillover).powf(1.0 / 12.0)
        } else {
            1.0
        };
        trend * boom
    });
    for (sector, m) in std::mem::take(&mut w.shocks.productivity_pending) {
        match sector {
            Some(s) if s < N_SECTORS => mult[s] *= m,
            Some(_) => {}
            None => mult.iter_mut().for_each(|x| *x *= m),
        }
    }
    // rz19: the tariff efficiency wedge (off with `tariff_tfp_loss` = 0: nothing is read, nothing is multiplied).
    if let Some(m) = tariff_tfp_step(w) {
        mult.iter_mut().for_each(|x| *x *= m);
    }
    for s in 0..N_SECTORS {
        w.exog.tfp[s] *= mult[s];
    }
    for f in w.firms.iter_mut() {
        f.labor_productivity *= mult[f.sector.idx()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common;
    use crate::sim;

    #[test]
    fn tech_regime_draws_are_deterministic_and_persistent() {
        let run = |seed: u64| -> Vec<bool> {
            let mut boom = false;
            (0..4000u32)
                .map(|t| {
                    let mut r = rng::module_rng(seed, t, stream::EXOG);
                    boom = next_tech_regime(boom, 1.0 / 240.0, 1.0 / 96.0, &mut r);
                    boom
                })
                .collect()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        let path = run(7);
        let share = path.iter().filter(|&&b| b).count() as f64 / path.len() as f64;
        // Stationary boom share is (1/240)/(1/240 + 1/96) ≈ 0.29.
        assert!(share > 0.1 && share < 0.5, "boom share {share}");
    }

    // ---- p0-exog: analytic checks. Each process is run alone on its own stream for 200k months. ----

    /// OLS slope of x_t on x_{t-1} without intercept (the series is mean zero in these tests).
    fn ar_slope(x: &[f64]) -> f64 {
        let num: f64 = x.windows(2).map(|p| p[1] * p[0]).sum();
        let den: f64 = x[..x.len() - 1].iter().map(|v| v * v).sum();
        num / den
    }

    fn mean(x: &[f64]) -> f64 {
        x.iter().sum::<f64>() / x.len() as f64
    }

    fn variance(x: &[f64]) -> f64 {
        let m = mean(x);
        x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / x.len() as f64
    }

    fn excess_kurtosis(x: &[f64]) -> f64 {
        let m = mean(x);
        let m2 = variance(x);
        let m4 = x.iter().map(|v| (v - m).powi(4)).sum::<f64>() / x.len() as f64;
        m4 / (m2 * m2) - 3.0
    }

    fn lag1_autocorr(x: &[f64]) -> f64 {
        let m = mean(x);
        let num: f64 = x.windows(2).map(|p| (p[1] - m) * (p[0] - m)).sum();
        num / x.iter().map(|v| (v - m) * (v - m)).sum::<f64>()
    }

    const N_SIM: usize = 200_000;

    #[test]
    fn fin_factor_ar_and_unconditional_sd() {
        // Analytic: AR coefficient rho; unconditional sd sigma / sqrt(1 - rho^2) when gamma = 0 and there are no jumps.
        let (rho, sigma) = (0.98, 0.1);
        let s = FinSpec { rho, sigma, ..Default::default() };
        let mut r = rng::module_rng(11, 0, stream::FIN_FACTOR);
        let mut f = 0.0;
        let mut path = Vec::with_capacity(N_SIM);
        for _ in 0..N_SIM {
            let eps = rng::normal(&mut r);
            let (next, n, _) = fin_factor_step(f, eps, &s, &mut r);
            assert_eq!(n, 0);
            f = next;
            path.push(f);
        }
        let slope = ar_slope(&path);
        assert!((slope - rho).abs() < 0.004, "AR slope {slope}");
        let sd = variance(&path).sqrt();
        let target = sigma / (1.0 - rho * rho).sqrt();
        assert!((sd / target - 1.0).abs() < 0.10, "sd {sd} vs {target}");
        assert!(excess_kurtosis(&path).abs() < 0.5, "Gaussian innovations give little excess kurtosis");
    }

    #[test]
    fn fin_factor_jump_frequency_and_mean() {
        // Analytic (sigma = 0): P(at least one jump in a month) = 1 - e^{-lambda}; E[f] = lambda * mu_J / (1 - rho).
        let (rho, lambda, mu, sd) = (0.9, 0.05, 0.5, 0.2);
        let s = FinSpec { rho, sigma: 0.0, jump_intensity: lambda, jump_mean: mu, jump_sd: sd, ..Default::default() };
        let mut r = rng::module_rng(12, 0, stream::FIN_FACTOR);
        let (mut f, mut months_with_jump, mut total_jumps) = (0.0, 0usize, 0u64);
        let mut path = Vec::with_capacity(N_SIM);
        for _ in 0..N_SIM {
            let (next, n, _) = fin_factor_step(f, 0.0, &s, &mut r);
            if n > 0 {
                months_with_jump += 1;
            }
            total_jumps += u64::from(n);
            f = next;
            path.push(f);
        }
        let freq = months_with_jump as f64 / N_SIM as f64;
        assert!((freq - (1.0 - (-lambda).exp())).abs() < 0.004, "jump frequency {freq}");
        let rate = total_jumps as f64 / N_SIM as f64;
        assert!((rate - lambda).abs() < 0.004, "jumps per month {rate}");
        let target_mean = lambda * mu / (1.0 - rho);
        assert!((mean(&path) - target_mean).abs() < 0.02, "mean {} vs {target_mean}", mean(&path));
    }

    #[test]
    fn fin_factor_volatility_rises_with_level() {
        // With gamma > 0 the innovation sd is sigma * exp(gamma * f_{t-1}) (inside the clamp). At f = 0 that is sigma;
        // at f = 0.5 with gamma = 2 it is sigma * e (e^1 is inside the [0.2, 5] range).
        let s = FinSpec { rho: 0.0, sigma: 0.1, gamma: 2.0, ..Default::default() };
        let mut r = rng::module_rng(13, 0, stream::FIN_FACTOR);
        let calm: Vec<f64> = (0..N_SIM).map(|_| fin_factor_step(0.0, rng::normal(&mut r), &s, &mut r).0).collect();
        let stressed: Vec<f64> = (0..N_SIM).map(|_| fin_factor_step(0.5, rng::normal(&mut r), &s, &mut r).0).collect();
        let (sd_calm, sd_stress) = (variance(&calm).sqrt(), variance(&stressed).sqrt());
        assert!((sd_calm / 0.1 - 1.0).abs() < 0.02, "calm sd {sd_calm}");
        assert!((sd_stress / (0.1 * 1f64.exp()) - 1.0).abs() < 0.02, "stressed sd {sd_stress}");
        // The clamp: exp(2 * 5) is capped at the top of the range.
        let (_, hi) = FIN_VOL_MULT_RANGE;
        let (f_big, _, _) = fin_factor_step(5.0, 1.0, &FinSpec { rho: 0.0, sigma: 1.0, gamma: 2.0, ..Default::default() }, &mut r);
        assert!((f_big - hi).abs() < 1e-12, "clamped multiplier gives {f_big}");
    }

    #[test]
    fn term_premium_factors_ar_and_sd() {
        for (rho, sigma, stream_id) in [(0.98, 0.09, stream::TERM_LEVEL), (0.95, 0.03, stream::TERM_CURV)] {
            let mut r = rng::module_rng(14, 0, stream_id);
            let mut x = 0.0;
            let path: Vec<f64> = (0..N_SIM)
                .map(|_| {
                    x = ar1_step(x, rho, sigma, rng::normal(&mut r));
                    x
                })
                .collect();
            let slope = ar_slope(&path);
            assert!((slope - rho).abs() < 0.004, "rho {rho}: slope {slope}");
            let target = sigma / (1.0 - rho * rho).sqrt();
            let sd = variance(&path).sqrt();
            assert!((sd / target - 1.0).abs() < 0.10, "rho {rho}: sd {sd} vs {target}");
        }
    }

    #[test]
    fn dollar_psi_ar_sd_and_fin_correlation() {
        // AR(1) with rho = 0.99 and sd sigma / sqrt(1 - rho^2); innovation correlated with f_t's innovation at 0.6.
        let (rho, sigma, corr) = (0.99, 0.013, 0.6);
        let mut rf = rng::module_rng(15, 0, stream::FIN_FACTOR);
        let mut ru = rng::module_rng(15, 0, stream::FX_PSI);
        let mut psi = 0.0;
        let (mut psi_path, mut shocks, mut eps_path) = (Vec::with_capacity(N_SIM), Vec::new(), Vec::new());
        for _ in 0..N_SIM {
            let eps = rng::normal(&mut rf);
            let u = rng::normal(&mut ru);
            let shock = psi_innovation(corr, eps, u);
            psi = ar1_step(psi, rho, sigma, shock);
            psi_path.push(psi);
            shocks.push(shock);
            eps_path.push(eps);
        }
        let slope = ar_slope(&psi_path);
        assert!((slope - rho).abs() < 0.004, "psi AR slope {slope}");
        let target = sigma / (1.0 - rho * rho).sqrt();
        assert!((variance(&psi_path).sqrt() / target - 1.0).abs() < 0.10);
        let (ms, me) = (mean(&shocks), mean(&eps_path));
        let cov: f64 = shocks.iter().zip(&eps_path).map(|(a, b)| (a - ms) * (b - me)).sum::<f64>() / N_SIM as f64;
        let corr_hat = cov / (variance(&shocks).sqrt() * variance(&eps_path).sqrt());
        assert!((corr_hat - corr).abs() < 0.01, "corr {corr_hat}");
    }

    #[test]
    fn oil_student_t_has_unit_variance_and_excess_kurtosis() {
        // Analytic: unit-variance t(nu) has excess kurtosis 6 / (nu - 4); nu = 10 gives 1.0.
        let nu = 10.0;
        let spec = OilSpec { sigma: 0.06, nu, ..Default::default() };
        let mut r = rng::module_rng(16, 0, stream::OIL_EXT);
        let mut rj = rng::module_rng(16, 0, stream::OIL_JUMP);
        let mut zr = rng::module_rng(16, 0, stream::EXOG);
        let mut vs = OilVolState::default();
        let mut ar = 0.0;
        let (mut shocks, mut ars) = (Vec::with_capacity(N_SIM), Vec::with_capacity(N_SIM));
        for _ in 0..N_SIM {
            let z = rng::normal(&mut zr);
            let m = oil_month(&spec, 0.95, ar, &mut vs, z, &mut r, &mut rj);
            ar = m.ar;
            shocks.push(m.shock);
            ars.push(ar);
        }
        let sd_shock = variance(&shocks).sqrt();
        assert!((sd_shock / 0.06 - 1.0).abs() < 0.02, "innovation sd {sd_shock}");
        let kurt = excess_kurtosis(&shocks);
        assert!((kurt - 6.0 / (nu - 4.0)).abs() < 0.35, "excess kurtosis {kurt}");
        // AR(1) of the log state: rho = 0.95, sd sigma / sqrt(1 - rho^2).
        assert!((ar_slope(&ars) - 0.95).abs() < 0.01, "oil AR slope {}", ar_slope(&ars));
        let target = 0.06 / (1.0 - 0.95f64 * 0.95).sqrt();
        assert!((variance(&ars).sqrt() / target - 1.0).abs() < 0.06);
    }

    #[test]
    fn oil_garch_persistence_matches_bollerslev() {
        // Gaussian GARCH(1,1) with alpha = 0.15, beta = 0.80: long-run variance sigma^2, and lag-1 autocorrelation of
        // eps^2 equal to alpha (1 - alpha beta - beta^2) / (1 - 2 alpha beta - beta^2) = 0.30 (Bollerslev 1986).
        let (alpha, beta, sigma) = (0.15, 0.80, 0.06);
        let spec = OilSpec { sigma, alpha, beta, ..Default::default() };
        let mut r = rng::module_rng(17, 0, stream::OIL_EXT);
        let mut rj = rng::module_rng(17, 0, stream::OIL_JUMP);
        let mut zr = rng::module_rng(17, 0, stream::EXOG);
        let mut vs = OilVolState::default();
        let mut ar = 0.0;
        let mut sq = Vec::with_capacity(N_SIM);
        for _ in 0..N_SIM {
            let m = oil_month(&spec, 0.95, ar, &mut vs, rng::normal(&mut zr), &mut r, &mut rj);
            ar = m.ar;
            sq.push(m.shock * m.shock);
        }
        let mean_sq = mean(&sq);
        assert!((mean_sq / (sigma * sigma) - 1.0).abs() < 0.08, "long-run variance {mean_sq}");
        let rho1 = lag1_autocorr(&sq);
        let analytic = alpha * (1.0 - alpha * beta - beta * beta) / (1.0 - 2.0 * alpha * beta - beta * beta);
        assert!((analytic - 0.30).abs() < 1e-3);
        assert!((rho1 - analytic).abs() < 0.03, "lag-1 autocorr of eps^2 {rho1} vs {analytic}");
    }

    #[test]
    fn oil_jump_frequency_matches_poisson() {
        // Analytic: P(at least one jump) = 1 - e^{-lambda}; lambda = 0.1 gives 0.0952.
        let lambda = 0.1;
        let spec = OilSpec { sigma: 0.0, jump_intensity: lambda, jump_sd: 0.2, ..Default::default() };
        let mut r = rng::module_rng(18, 0, stream::OIL_EXT);
        let mut rj = rng::module_rng(18, 0, stream::OIL_JUMP);
        let mut zr = rng::module_rng(18, 0, stream::EXOG);
        let mut vs = OilVolState::default();
        let (mut ar, mut months, mut total) = (0.0, 0usize, 0u64);
        for _ in 0..N_SIM {
            let m = oil_month(&spec, 0.95, ar, &mut vs, rng::normal(&mut zr), &mut r, &mut rj);
            ar = m.ar;
            total += u64::from(m.jumps);
            if m.jumps > 0 {
                months += 1;
            }
        }
        let freq = months as f64 / N_SIM as f64;
        assert!((freq - (1.0 - (-lambda).exp())).abs() < 0.004, "jump frequency {freq}");
        assert!((total as f64 / N_SIM as f64 - lambda).abs() < 0.004);
    }

    /// Replica of the pre-p0 oil recursion, drawn from the same EXOG stream.
    fn base_oil_path(seed: u64, cfg: &Config, months: u32) -> Vec<f64> {
        let mut ar = 0.0;
        (0..months)
            .map(|t| {
                let mut r = rng::module_rng(seed, t, stream::EXOG);
                ar = cfg.oil_ar * ar + cfg.oil_sigma * rng::normal(&mut r);
                ar
            })
            .collect()
    }

    #[test]
    fn all_defaults_reproduce_base_exog_path() {
        // With the p0-exog defaults the oil state equals the pre-p0 recursion bit for bit, and every new state is 0.
        let mut cfg = crate::testkit::legacy_p1off_config();
        cfg.n_households = 60;
        let seed = 5;
        let mut w = crate::testkit::tiny_world(cfg.clone(), seed);
        let base = base_oil_path(seed, &cfg, 120);
        for t in 0..120u32 {
            w.tick = t;
            step(&mut w);
            assert_eq!(w.exog.oil_ar.to_bits(), base[t as usize].to_bits(), "oil AR differs at month {t}");
            assert_eq!(w.exog.fin_factor(), 0.0);
            assert_eq!(w.exog.tp_level(), 0.0);
            assert_eq!(w.exog.tp_curv(), 0.0);
            assert_eq!(w.exog.fx_psi(), 0.0);
            assert_eq!(w.exog.fin_jump_n + w.exog.oil_jump_n, 0);
            assert_eq!(w.exog.oil_vol_state().to_bits(), cfg.oil_sigma.to_bits());
        }
        assert_eq!(w.exog.oil_base.to_bits(), cfg.oil_base.to_bits());
    }

    #[test]
    fn all_processes_on_run_through_the_world() {
        let mut cfg = Config::default();
        cfg.n_households = 60;
        cfg.fin_factor_sigma = 0.1;
        cfg.fin_factor_vol_gamma = 1.0;
        cfg.fin_jump_intensity = 0.05;
        cfg.fin_jump_mean = 0.5;
        cfg.fin_jump_sd = 0.2;
        cfg.tp_level_sigma = 0.09;
        cfg.tp_level_fin_loading = -0.5;
        cfg.tp_curv_sigma = 0.03;
        cfg.fx_psi_sigma = 0.013;
        cfg.fx_psi_fin_corr = 0.6;
        cfg.oil_t_nu = 6.0;
        cfg.oil_garch_alpha = 0.15;
        cfg.oil_garch_beta = 0.80;
        cfg.oil_jump_intensity = 0.02;
        cfg.oil_jump_sd = 0.2;
        cfg.oil_index_to_world = true;
        let mut w = crate::testkit::tiny_world(cfg.clone(), 9);
        let mut max_fin: f64 = 0.0;
        for t in 0..300u32 {
            w.tick = t;
            step(&mut w);
            assert!(w.exog.oil_price.is_finite() && w.exog.oil_price > 0.0);
            assert!(w.exog.fin_factor().is_finite() && w.exog.tp_level().is_finite());
            max_fin = max_fin.max(w.exog.fin_factor().abs());
        }
        assert!(max_fin > 0.0);
        // The anchor stays at cfg.oil_base; the domestic index is carried by the oil price.
        assert_eq!(w.exog.oil_base.to_bits(), cfg.oil_base.to_bits());
    }

    #[test]
    fn term_level_loading_is_on_current_fin_factor_not_accumulated() {
        // With no AR innovation the level factor is exactly the loading times f_t (no compounding by rho).
        let mut cfg = crate::testkit::legacy_p1off_config();
        cfg.n_households = 60;
        cfg.fin_factor_sigma = 0.1;
        cfg.fin_factor_rho = 0.98;
        cfg.tp_level_fin_loading = -0.5;
        let mut w = crate::testkit::tiny_world(cfg.clone(), 21);
        for t in 0..500u32 {
            factors_step(&mut w, t);
            assert_eq!(w.exog.tp_level_ar, 0.0);
            assert_eq!(w.exog.tp_level().to_bits(), (cfg.tp_level_fin_loading * w.exog.fin_factor()).to_bits());
        }
    }

    #[test]
    fn term_level_slope_on_fin_factor_equals_loading() {
        // The AR state is independent of f_t, so the OLS slope of tp_level on f_t estimates the loading L. The state
        // is a separate stream and is not fed back, so the slope is L (no ρ-compounding, which would give about 25 L).
        let mut cfg = Config::default();
        cfg.n_households = 60;
        cfg.fin_factor_sigma = 0.1;
        cfg.tp_level_sigma = 0.09;
        cfg.tp_level_fin_loading = -0.5;
        let mut w = crate::testkit::tiny_world(cfg.clone(), 22);
        let (mut sxy, mut sxx) = (0.0, 0.0);
        for t in 0..N_SIM as u32 {
            factors_step(&mut w, t);
            let f = w.exog.fin_factor();
            sxy += w.exog.tp_level() * f;
            sxx += f * f;
        }
        let slope = sxy / sxx;
        assert!((slope - cfg.tp_level_fin_loading).abs() < 0.1, "slope of tp_level on f_t {slope}");
    }

    #[test]
    fn risk_premium_is_the_ar_state_when_fin_loading_is_off() {
        // Re-fit Step 10a: with rp_fin_loading = 0 the total demand premium equals the AR state every month (legacy).
        let mut cfg = crate::testkit::legacy_p1off_config();
        cfg.n_households = 60;
        cfg.risk_premium_sigma = 0.002;
        cfg.fin_jump_intensity = 1.0 / 96.0;
        cfg.fin_jump_mean = 1.0;
        cfg.fin_jump_sd = 0.25;
        cfg.fin_factor_rho = 0.95;
        let mut w = crate::testkit::tiny_world(cfg, 31);
        let mut max_rp: f64 = 0.0;
        for t in 0..2000u32 {
            w.tick = t;
            step(&mut w);
            assert_eq!(w.exog.risk_premium.to_bits(), w.exog.rp_ar.to_bits());
            max_rp = max_rp.max(w.exog.rp_ar.abs());
        }
        assert!(max_rp > 0.0);
    }

    #[test]
    fn risk_premium_adds_the_centred_fin_factor_and_fin_mean_is_stationary() {
        // Re-fit Step 10a: risk_premium = clamp(rp_ar + L (f - fbar)), with fbar = lambda mu / (1 - rho); the sample mean
        // of the jump-AR factor converges to fbar (about 0.208 for these settings).
        let mut cfg = crate::testkit::legacy_p1off_config();
        cfg.fin_jump_intensity = 1.0 / 96.0;
        cfg.fin_jump_mean = 1.0;
        cfg.fin_jump_sd = 0.25;
        cfg.fin_factor_rho = 0.95;
        cfg.rp_fin_loading = 0.01;
        let f_bar = fin_factor_mean(&cfg);
        assert!((f_bar - (1.0 / 96.0) / 0.05).abs() < 1e-12);
        let mut w = crate::testkit::tiny_world(cfg.clone(), 32);
        let mut sum_f = 0.0;
        for t in 0..N_SIM as u32 {
            factors_step(&mut w, t);
            sum_f += w.exog.fin_factor();
        }
        let mean_f = sum_f / N_SIM as f64;
        assert!((mean_f - f_bar).abs() < 0.03, "sample mean of f {mean_f} vs fbar {f_bar}");
        // The composition, on one month of the full step.
        w.tick = 0;
        step(&mut w);
        let expect = (w.exog.rp_ar + cfg.rp_fin_loading * (w.exog.fin_factor() - f_bar)).clamp(-0.05, 0.05);
        assert_eq!(w.exog.risk_premium.to_bits(), expect.to_bits());
    }

    #[test]
    fn oil_index_grows_at_domestic_inflation_so_real_oil_has_no_drift() {
        // No oil shocks, so oil_ratio (the quantity every consumer uses) is exactly the index: it grows at the domestic
        // target, and deflated by the price level at the target it is flat. The import-price trend must not enter.
        let cfg = Config { n_households: 60, oil_index_to_world: true, stochastic_oil: false, ..Default::default() };
        let mut w = crate::testkit::tiny_world(cfg.clone(), 41);
        let (t0, t1) = (0u32, 240u32);
        w.tick = t0;
        step(&mut w);
        let r0 = common::oil_ratio(&w);
        w.tick = t1;
        step(&mut w);
        let r1 = common::oil_ratio(&w);
        let annual = (r1 / r0).powf(12.0 / f64::from(t1 - t0)) - 1.0;
        assert!((annual - cfg.inflation_target).abs() < 1e-9, "oil ratio growth {annual}");
        assert!((annual - cfg.world_goods_inflation).abs() > 0.01);
        // Month 0 is the anchor: oil_base is unchanged.
        assert_eq!(w.exog.oil_base.to_bits(), cfg.oil_base.to_bits());
    }

    #[test]
    fn real_oil_cost_seen_by_firms_has_no_trend_with_index_on() {
        // The real oil cost is the oil ratio that firms use (crude cost, Energy oil leg, extraction labour) deflated by
        // the price level at the target. With the stochastic oil AR on, its OLS trend must be zero with the index on
        // and -pi* a year with it off, which shows the test has power.
        let months = 3000u32;
        let trend_per_year = |index_on: bool| -> f64 {
            let cfg = Config { n_households: 60, oil_index_to_world: index_on, ..Default::default() };
            let mut w = crate::testkit::tiny_world(cfg.clone(), 51);
            let per_month = (1.0 + cfg.inflation_target).ln() / 12.0;
            let (mut sx, mut sxx, mut sy, mut sxy) = (0.0, 0.0, 0.0, 0.0);
            let n = f64::from(months);
            for t in 0..months {
                w.tick = t;
                step(&mut w);
                // Real oil cost: log of the firms' oil ratio, deflated by the price level at the target.
                let y = common::oil_ratio(&w).ln() - per_month * f64::from(t);
                let x = f64::from(t);
                sx += x;
                sxx += x * x;
                sy += y;
                sxy += x * y;
            }
            let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
            slope * 12.0
        };
        let on = trend_per_year(true);
        let off = trend_per_year(false);
        assert!(on.abs() < 0.005, "real oil trend with the index on {on} a year");
        assert!((off + 0.02).abs() < 0.003, "control: real oil trend with the index off {off} a year");
    }

    #[test]
    fn oil_jump_path_does_not_depend_on_t_nu() {
        // Jumps draw from their own stream, so turning the Student-t on or changing ν leaves the jump months unchanged.
        let jumps_for = |nu: f64| -> Vec<u32> {
            let spec = OilSpec { sigma: 0.06, nu, jump_intensity: 0.1, jump_sd: 0.2, ..Default::default() };
            let mut rt = rng::module_rng(19, 0, stream::OIL_EXT);
            let mut rj = rng::module_rng(19, 0, stream::OIL_JUMP);
            let mut zr = rng::module_rng(19, 0, stream::EXOG);
            let mut vs = OilVolState::default();
            let mut ar = 0.0;
            (0..20_000)
                .map(|_| {
                    let m = oil_month(&spec, 0.95, ar, &mut vs, rng::normal(&mut zr), &mut rt, &mut rj);
                    ar = m.ar;
                    m.jumps
                })
                .collect()
        };
        let gaussian = jumps_for(0.0);
        assert!(gaussian.iter().sum::<u32>() > 1000, "enough jumps to compare");
        assert_eq!(gaussian, jumps_for(6.0));
        assert_eq!(gaussian, jumps_for(10.0));
    }

    #[test]
    fn gaussian_oil_path_is_the_pre_p0_recursion_for_any_sigma_and_rho() {
        // With no extension on, the level cap does not apply, so the recursion is exact even when the stationary sd is
        // large enough for |ar| to exceed the cap.
        let mut beyond_cap = false;
        for (sigma, rho) in [(0.2, 0.99), (0.085, 0.985), (0.06, 0.95)] {
            let spec = OilSpec { sigma, ..Default::default() };
            let mut rt = rng::module_rng(20, 0, stream::OIL_EXT);
            let mut rj = rng::module_rng(20, 0, stream::OIL_JUMP);
            let mut zr = rng::module_rng(20, 0, stream::EXOG);
            let mut vs = OilVolState::default();
            let (mut ar, mut reference) = (0.0, 0.0);
            for _ in 0..50_000 {
                let z = rng::normal(&mut zr);
                let m = oil_month(&spec, rho, ar, &mut vs, z, &mut rt, &mut rj);
                reference = rho * reference + sigma * z;
                assert_eq!(m.ar.to_bits(), reference.to_bits(), "sigma {sigma} rho {rho}");
                ar = m.ar;
                beyond_cap |= ar.abs() > OIL_LOG_DEV_CAP;
            }
        }
        assert!(beyond_cap, "the test should reach the level that the cap would clip");
    }

    #[test]
    fn dollar_psi_responds_to_fin_jumps_only_through_the_loading() {
        // f has no diffusion and ψ has no AR or noise, so ψ_t = loading · J_t, with J_t = f_t − ρ f_{t−1}.
        let mut cfg = crate::testkit::legacy_p1off_config();
        cfg.n_households = 60;
        cfg.fin_factor_rho = 0.98;
        cfg.fin_jump_intensity = 0.5;
        cfg.fin_jump_mean = 0.5;
        cfg.fin_jump_sd = 0.2;
        cfg.fx_psi_rho = 0.0;
        cfg.fx_psi_fin_jump_loading = 2.0;
        let mut w = crate::testkit::tiny_world(cfg.clone(), 52);
        let mut prev_f = 0.0;
        let mut nonzero = 0;
        for t in 0..200u32 {
            factors_step(&mut w, t);
            let f = w.exog.fin_factor();
            let jump = f - cfg.fin_factor_rho * prev_f;
            assert!((w.exog.fx_psi() - cfg.fx_psi_fin_jump_loading * jump).abs() < 1e-12, "month {t}");
            nonzero += usize::from(w.exog.fx_psi() != 0.0);
            prev_f = f;
        }
        assert!(nonzero > 20, "jumps should occur in 200 months ({nonzero})");
    }

    #[test]
    fn oil_conditional_sd_is_clamped_to_the_range() {
        let lr_sd = 0.06;
        let (lo, hi) = OIL_VOL_MULT_RANGE;
        // Explosive shock history: h_raw far above the top of the range, so the stored sd is hi × σ.
        let spec = OilSpec { sigma: lr_sd, alpha: 0.15, beta: 0.80, ..Default::default() };
        let mut vs = OilVolState { cond_var: lr_sd * lr_sd, prev_shock_sq: 1e6 };
        let m = oil_month(&spec, 0.95, 0.0, &mut vs, 0.0, &mut rng::module_rng(1, 0, stream::OIL_EXT), &mut rng::module_rng(1, 0, stream::OIL_JUMP));
        assert!((m.sd - hi * lr_sd).abs() < 1e-12, "top clamp sd {}", m.sd);
        assert!((vs.cond_var.sqrt() - hi * lr_sd).abs() < 1e-12, "stored variance is clamped too");
        // Collapsing variance (α + β ≈ 1 leaves almost nothing): the stored sd is lo × σ.
        let spec = OilSpec { sigma: lr_sd, alpha: 0.001, beta: 0.998, ..Default::default() };
        let mut vs = OilVolState { cond_var: 1e-8, prev_shock_sq: 0.0 };
        let m = oil_month(&spec, 0.95, 0.0, &mut vs, 0.0, &mut rng::module_rng(2, 0, stream::OIL_EXT), &mut rng::module_rng(2, 0, stream::OIL_JUMP));
        assert!((m.sd - lo * lr_sd).abs() < 1e-12, "bottom clamp sd {}", m.sd);
    }

    #[test]
    fn oil_student_t_draws_are_winsorised() {
        let spec = OilSpec { sigma: 0.06, nu: 4.0, ..Default::default() };
        let mut vs = OilVolState::default();
        let mut r = rng::module_rng(3, 0, stream::OIL_EXT);
        let mut rj = rng::module_rng(3, 0, stream::OIL_JUMP);
        let mut zr = rng::module_rng(3, 0, stream::EXOG);
        // A huge base draw is capped at +/- OIL_SHOCK_CAP sd, in either direction.
        let up = oil_month(&spec, 0.0, 0.0, &mut vs, 1e6, &mut r, &mut rj);
        let dn = oil_month(&spec, 0.0, 0.0, &mut vs, -1e6, &mut r, &mut rj);
        assert!((up.shock / up.sd - OIL_SHOCK_CAP).abs() < 1e-12);
        assert!((dn.shock / dn.sd + OIL_SHOCK_CAP).abs() < 1e-12);
        // Over a long t(4) run no draw exceeds the cap.
        let mut max_abs: f64 = 0.0;
        for _ in 0..N_SIM {
            let m = oil_month(&spec, 0.0, 0.0, &mut vs, rng::normal(&mut zr), &mut r, &mut rj);
            max_abs = max_abs.max((m.shock / m.sd).abs());
        }
        assert!(max_abs <= OIL_SHOCK_CAP + 1e-12, "max |z| {max_abs}");
    }

    #[test]
    #[should_panic(expected = "alpha + beta < 1")]
    fn oil_garch_requires_stationary_variance() {
        let cfg = Config { oil_garch_alpha: 0.2, oil_garch_beta: 0.85, ..Default::default() };
        let _ = oil_spec(&cfg);
    }

    #[test]
    fn oil_plan9_parameters_stay_in_bounds() {
        // Tail check at the plan-9 oil parameters (t(4), GARCH 0.15/0.80, rho 0.985, sigma 0.085) over 552-month
        // windows: the conditional sd never leaves the clamp range and the winsorised shocks stay within the cap.
        let spec = OilSpec { sigma: 0.085, nu: 4.0, alpha: 0.15, beta: 0.80, ..Default::default() };
        let (rho, windows, months) = (0.985, 200usize, 552u32);
        let (lo, hi) = (OIL_VOL_MULT_RANGE.0 * spec.sigma, OIL_VOL_MULT_RANGE.1 * spec.sigma);
        let mut worst_ar: f64 = 0.0;
        for w in 0..windows {
            let mut r = rng::module_rng(100 + w as u64, 0, stream::OIL_EXT);
        let mut rj = rng::module_rng(100 + w as u64, 0, stream::OIL_JUMP);
            let mut zr = rng::module_rng(100 + w as u64, 0, stream::EXOG);
            let mut vs = OilVolState::default();
            let mut ar = 0.0;
            for _ in 0..months {
                let m = oil_month(&spec, rho, ar, &mut vs, rng::normal(&mut zr), &mut r, &mut rj);
                ar = m.ar;
                assert!(m.sd >= lo - 1e-12 && m.sd <= hi + 1e-12, "sd {} outside [{lo}, {hi}]", m.sd);
                assert!((m.shock / m.sd).abs() <= OIL_SHOCK_CAP + 1e-12);
                worst_ar = worst_ar.max(ar.abs());
            }
        }
        assert!(worst_ar <= OIL_LOG_DEV_CAP + 1e-12, "worst |ar| {worst_ar}");
    }

    #[test]
    fn oil_index_is_continuous_across_burn_in_reset() {
        // The index is 1 at reported month 0 and continuous with the last burn-in month, so the oil price moves by one
        // month of inflation (a factor (1+π)^(1/12), about 0.16%) across the reset, not by the burn-in length. The AR is
        // off here so that the price is the index times the anchor.
        let cfg = Config {
            burn_in_months: 12,
            n_households: 2_000,
            oil_index_to_world: true,
            stochastic_oil: false,
            ..Default::default()
        };
        let base = cfg.oil_base;
        let pi = cfg.inflation_target;
        let mut w = crate::init::build_world(cfg, 31);
        assert_eq!(w.tick_offset, 12);
        let last_burn_in = w.exog.oil_price;
        assert!((last_burn_in / (base * (1.0 + pi).powf(-1.0 / 12.0)) - 1.0).abs() < 1e-9, "last burn-in {last_burn_in}");
        assert_eq!(w.exog.oil_base.to_bits(), base.to_bits(), "the anchor is not indexed");
        sim::step(&mut w);
        let first = w.exog.oil_price;
        assert!((first / base - 1.0).abs() < 1e-9, "month 0 oil price {first} vs {base}");
        let one_month = (1.0 + pi).powf(1.0 / 12.0);
        assert!((first / last_burn_in / one_month - 1.0).abs() < 1e-9, "jump across reset {}", first / last_burn_in);
    }


    // ---- rz19: the tariff efficiency wedge ----

    /// A small world whose month 0 (tick 0, no tariff) has run: the wedge's reference duty is taken there. Later months start at tick 1.
    fn wedge_world(loss: f64, phase_in: f64, seed: u64) -> World {
        let mut cfg = Config::default();
        cfg.n_households = 60;
        cfg.tariff_tfp_loss = loss;
        cfg.tariff_tfp_phase_in_months = phase_in;
        let mut w = crate::testkit::tiny_world(cfg, seed);
        w.shocks.tariff = 0.0;
        w.tick = 0;
        step(&mut w);
        w
    }

    /// `months` months of `step` from tick `from`, with the set-level duty `tau`.
    fn run_exog(w: &mut World, from: u32, months: u32, tau: f64) {
        w.shocks.tariff = tau;
        for t in from..from + months {
            w.tick = t;
            step(w);
        }
    }

    #[test]
    fn tariff_wedge_off_multiplies_nothing_and_reads_no_state() {
        // key off (the default 0): a tariff changes neither the indices nor the firms' productivity, bit for bit; the phase-in is inert
        let mut a = wedge_world(0.0, 60.0, 5);
        let mut b = wedge_world(0.0, 7.0, 5);
        let mut c = wedge_world(0.0, 60.0, 5);
        run_exog(&mut a, 1, 36, 0.25);
        run_exog(&mut b, 1, 36, 0.25);
        run_exog(&mut c, 1, 36, 0.0);
        for s in 0..N_SECTORS {
            assert_eq!(a.exog.tfp[s].to_bits(), b.exog.tfp[s].to_bits());
            assert_eq!(a.exog.tfp[s].to_bits(), c.exog.tfp[s].to_bits(), "a tariff moves no index with the key off");
        }
        for (fa, fc) in a.firms.iter().zip(c.firms.iter()) {
            assert_eq!(fa.labor_productivity.to_bits(), fc.labor_productivity.to_bits());
        }
        assert_eq!((a.exog.tariff_tfp_log, a.exog.tariff_tfp_peak, a.exog.tariff_tfp_ref, a.exog.tariff_tfp_ref_set), (0.0, 0.0, 0.0, false));
    }

    #[test]
    fn tariff_wedge_is_inert_without_a_tariff() {
        // key on, no tariff above the reference: the state stays 0 and every productivity is the legacy one, bit for bit
        let mut on = wedge_world(0.3, 24.0, 8);
        let mut off = wedge_world(0.0, 60.0, 8);
        run_exog(&mut on, 1, 48, 0.0);
        run_exog(&mut off, 1, 48, 0.0);
        assert_eq!(on.exog.tariff_tfp_log, 0.0);
        for s in 0..N_SECTORS {
            assert_eq!(on.exog.tfp[s].to_bits(), off.exog.tfp[s].to_bits());
        }
        for (fa, fb) in on.firms.iter().zip(off.firms.iter()) {
            assert_eq!(fa.labor_productivity.to_bits(), fb.labor_productivity.to_bits());
        }
    }

    #[test]
    fn tariff_wedge_is_measured_above_the_duty_the_run_starts_from() {
        // a world that starts with a duty of 8 points in force (Today's start): that duty carries no wedge; only the rise above it does
        let (kappa, n) = (0.067, 60u32);
        let mut cfg = Config::default();
        cfg.n_households = 60;
        cfg.tariff_tfp_loss = kappa;
        cfg.tariff_tfp_phase_in_months = f64::from(n);
        let mut w = crate::testkit::tiny_world(cfg, 5);
        w.shocks.tariff = 0.08;
        w.tick = 0;
        step(&mut w);
        assert!((w.exog.tariff_tfp_ref - 0.08).abs() < 1e-15 && w.exog.tariff_tfp_ref_set);
        run_exog(&mut w, 1, 24, 0.08);
        assert_eq!(w.exog.tariff_tfp_log, 0.0, "today's duty carries no wedge");
        run_exog(&mut w, 25, n, 0.08 + 0.25);
        assert!((w.exog.tariff_tfp_log + kappa * 0.25).abs() < 1e-12, "{}", w.exog.tariff_tfp_log);
        // a duty below the reference gives no gain
        let mut low = crate::testkit::tiny_world(Config { n_households: 60, tariff_tfp_loss: kappa, ..Config::default() }, 5);
        low.shocks.tariff = 0.08;
        low.tick = 0;
        step(&mut low);
        run_exog(&mut low, 1, 24, 0.02);
        assert_eq!(low.exog.tariff_tfp_log, 0.0);
    }

    #[test]
    fn tariff_wedge_waits_for_the_steered_window_to_end() {
        // while a steered window is stepped the wedge does nothing and the reference is retaken; the duty in force at the first month
        // after the window is the reference
        let (kappa, n) = (0.067, 12u32);
        let mut cfg = Config::default();
        cfg.n_households = 60;
        cfg.tariff_tfp_loss = kappa;
        cfg.tariff_tfp_phase_in_months = f64::from(n);
        let mut w = crate::testkit::tiny_world(cfg, 6);
        w.shocks.tariff = 0.0;
        w.tick = 0;
        step(&mut w);
        assert!(w.exog.tariff_tfp_ref_set);
        // the window: the duty follows the data from 0 to 8 points; the policy-rate target marks the window months
        for t in 1..=24u32 {
            w.steer.policy = Some(0.04);
            run_exog(&mut w, t, 1, 0.08 * f64::from(t) / 24.0);
            assert!(!w.exog.tariff_tfp_ref_set, "reference retaken in window month {t}");
            assert_eq!(w.exog.tariff_tfp_log, 0.0, "no wedge in the window");
        }
        w.steer.policy = None;
        run_exog(&mut w, 25, 3, 0.08);
        assert!(w.exog.tariff_tfp_ref_set && (w.exog.tariff_tfp_ref - 0.08).abs() < 1e-15);
        assert_eq!(w.exog.tariff_tfp_log, 0.0, "the duty at the end of the window is the reference");
        run_exog(&mut w, 28, n, 0.08 + 0.10);
        assert!((w.exog.tariff_tfp_log + kappa * 0.10).abs() < 1e-12);
    }

    #[test]
    fn tariff_wedge_ramps_linearly_to_loss_times_duty_and_back() {
        let (kappa, n) = (0.067, 60u32);
        let mut on = wedge_world(kappa, f64::from(n), 5);
        let mut off = wedge_world(0.0, 60.0, 5);
        let target = -kappa * 0.25;
        // the ramp, month by month, and the ratio to the world without the wedge
        for m in 1..=n {
            run_exog(&mut on, m, 1, 0.25);
            run_exog(&mut off, m, 1, 0.25);
            let want = target * f64::from(m) / f64::from(n);
            assert!((on.exog.tariff_tfp_log - want).abs() < 1e-12, "month {m}: state {} want {want}", on.exog.tariff_tfp_log);
            for s in 0..N_SECTORS {
                assert!((on.exog.tfp[s] / off.exog.tfp[s] - want.exp()).abs() < 1e-12, "index of sector {s} at month {m}");
            }
            for (a, b) in on.firms.iter().zip(off.firms.iter()) {
                assert!((a.labor_productivity / b.labor_productivity - want.exp()).abs() < 1e-12);
            }
        }
        // it holds while the duty holds ...
        run_exog(&mut on, n + 1, 24, 0.25);
        assert!((on.exog.tariff_tfp_log - target).abs() < 1e-12);
        // ... and ramps back to zero over the same number of months when the duty is removed
        run_exog(&mut on, n + 25, n / 2, 0.0);
        assert!((on.exog.tariff_tfp_log - target / 2.0).abs() < 1e-12, "half way back: {}", on.exog.tariff_tfp_log);
        run_exog(&mut on, n + 25 + n / 2, n / 2, 0.0);
        assert!(on.exog.tariff_tfp_log.abs() < 1e-12 && on.exog.tariff_tfp_log <= 0.0);
        run_exog(&mut on, 2 * n + 25, 6, 0.0);
        assert_eq!(on.exog.tariff_tfp_log, 0.0, "no drift once back");
    }

    #[test]
    fn tariff_wedge_takes_the_same_time_for_any_size_and_scales_with_it() {
        let (kappa, n) = (0.05, 40u32);
        for &tau in &[0.02, 0.10, 0.60] {
            let mut w = wedge_world(kappa, f64::from(n), 3);
            run_exog(&mut w, 1, n, tau);
            assert!((w.exog.tariff_tfp_log + kappa * tau).abs() < 1e-12, "tau {tau}: {}", w.exog.tariff_tfp_log);
            run_exog(&mut w, n + 1, 5, tau);
            assert!((w.exog.tariff_tfp_log + kappa * tau).abs() < 1e-12, "holds at tau {tau}");
        }
        // half way up, the duty goes: the state moves back toward 0
        let mut w = wedge_world(kappa, f64::from(n), 3);
        run_exog(&mut w, 1, 20, 0.10);
        let mid = w.exog.tariff_tfp_log;
        assert!((mid + kappa * 0.10 * 0.5).abs() < 1e-12);
        run_exog(&mut w, 21, 1, 0.0);
        assert!(w.exog.tariff_tfp_log > mid, "the duty is gone: the state moves back toward 0");
    }

    #[test]
    fn tariff_wedge_never_gives_a_gain() {
        // a negative or non-finite duty gives no wedge and no productivity gain
        let mut w = wedge_world(0.2, 12.0, 4);
        run_exog(&mut w, 1, 24, -0.10);
        assert_eq!(w.exog.tariff_tfp_log, 0.0);
        let mut nan = wedge_world(0.2, 12.0, 4);
        run_exog(&mut nan, 1, 6, f64::NAN);
        assert_eq!(nan.exog.tariff_tfp_log, 0.0);
    }

    #[test]
    fn tariff_wedge_lowers_output_through_the_whole_economy() {
        // end to end on a small world with an ImportTariff of 25 points: the key lowers real GDP, and the state reaches loss x duty
        let run = |loss: f64| -> (f64, f64) {
            let mut cfg = Config::default();
            cfg.n_households = 600;
            cfg.burn_in_months = 0;
            cfg.tariff_tfp_loss = loss;
            cfg.tariff_tfp_phase_in_months = 12.0;
            let mut w = crate::init::build_world(cfg, 21);
            sim::step(&mut w); // month 0 without a tariff: the reference
            w.shocks.schedule(crate::shocks::ScheduledShock { at_month: 1, shock: crate::shocks::Shock::ImportTariff { pct: 25.0 } });
            let mut gdp = 0.0;
            for _ in 0..36 {
                sim::step(&mut w);
                gdp = w.last().map(|m| m.gdp_real_bn_ar).unwrap_or(0.0);
            }
            (gdp, w.exog.tariff_tfp_log)
        };
        let (g_off, s_off) = run(0.0);
        let (g_on, s_on) = run(0.4);
        assert_eq!(s_off, 0.0);
        assert!((s_on + 0.4 * 0.25).abs() < 1e-9, "the wedge reached loss x duty: {s_on}");
        assert!(g_off.is_finite() && g_on.is_finite() && g_on < g_off, "real GDP {g_on} with the wedge, {g_off} without");
    }

}
