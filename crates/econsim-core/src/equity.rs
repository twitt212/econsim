//! Fund equity: the equity risk premium, sector valuation by capitalised dividends, and the Fund's equity return
//! (docs/DESIGN.md §11.4, docs/SECTORS.md §Stage 4). Called from `monetary::markets`.

use crate::common;
use crate::monetary::EPS;
use crate::types::*;

/// Sectors that adopt a tech boom's productivity (Durable goods and Other services; see exog.rs).
const ADOPTER_SECTORS: [usize; 2] = [2, 7];
/// Bounds on the extrapolative term of expected sector earnings growth (decimal per year).
const EXT_MIN: f64 = -0.015;
const EXT_MAX: f64 = 0.015;
/// fix-monetary: monthly weight on a new non-boom valuation in the normal Tobin's q (12-month half-life).
const NORMAL_Q_RATE: f64 = 0.0561;
/// Weight on the previous smoothed Fund dividend (monthly), §11.4.
pub(crate) const DIVIDEND_SMOOTHING: f64 = 0.9;
/// Floor on the equity capitalisation rate denominator, §11.4.
const MIN_CAP_RATE: f64 = 0.02;
/// p1-equity: lower bound on the raw capitalisation rate in the log-linear valuation (equity_valuation_log). This is a
/// deliberate bound on valuations, not a numerical guard: exp((c − r) ÷ c) is finite for every finite r, so the bound is
/// not needed for finiteness. While a sector's raw rate is below 0.005 the valuation uses 0.005, so the sector does not
/// respond to rates or premia, and its value is capped at exp((c − 0.005) ÷ c), about 2.4 × the reference for c ≈ 0.044.
/// On the profile (8 seeds × 20,000 × 240) it binds in 1.8% of sector-months, and in about 61% of Tech boom months
/// (docs/CALIBRATION.md "p1-equity", "Log-form guard"). Lowering it, or replacing it by a smooth bound, is untested here.
const LOG_CAP_GUARD: f64 = 0.005;
/// p1-equity: monthly decay of the 12-month EWMA of the Fund share price (span 12: alpha = 2/13 per month).
const SHARE_PRICE_EWMA_DECAY: f64 = 11.0 / 13.0;

/// p1-equity: the smoothed Fund share price that households' wealth effect reads when `fund_wealth_ewma` is on (the
/// 12-month EWMA, or the current price before the EWMA is set; the current price when the switch is off). The EWMA is
/// updated at the start of `revalue_fund_equity` with the close of the previous month, and households decide before
/// `markets` runs. So in month t the term reads closes up to t−2, one month more lag than the current price (mean lag
/// about 6.5 months).
pub fn smoothed_share_price(w: &World) -> f64 {
    let e = w.fund.share_price_ewma;
    if w.cfg.fund_wealth_ewma && e > 0.0 && e.is_finite() {
        e
    } else {
        w.fund.share_price
    }
}

/// p1-equity: bound on the equity capitalisation rate, applied to the raw denominator `raw` = y_eq + premium − g. With
/// `softness` = 0 this is the hard floor max(raw, floor) (base behaviour). With `softness` s > 0 it is the smooth
/// bound floor + s × softplus((raw − floor) ÷ s), which is never below the floor, tends to raw well above the floor
/// (the excess over raw is s × e^{−(raw−floor)/s}), and keeps a non-zero slope in the premium and the rates below the
/// floor (the hard floor makes the slope zero there). NaN `raw` is read as the floor, as f64::max does.
pub fn bounded_cap(raw: f64, floor: f64, softness: f64) -> f64 {
    let raw = if raw.is_nan() { floor } else { raw };
    if softness > 0.0 && softness.is_finite() {
        let x = (raw - floor) / softness;
        let softplus = if x > 30.0 { x } else { x.exp().ln_1p() };
        floor + softness * softplus
    } else {
        raw.max(floor)
    }
}

/// Trailing 12-month total return on the Fund's equity (decimal, compounded), or None before 12 monthly observations.
pub fn trailing_equity_return(w: &World) -> Option<f64> {
    let r = &w.monetary.equity_returns;
    if r.len() < 12 {
        return None;
    }
    Some(r[r.len() - 12..].iter().map(|x| 1.0 + x).product::<f64>() - 1.0)
}

/// Time-varying equity risk premium (decimal) for this month (§11.4):
///   ERP_t = erp + κ_u × (u_t − u*) − θ × (R12_t − R̄)
///           + b_f × f_t + b_rp × (1 + κ_rp × max(u_t − u*, 0)) × rp_t + b_s × S_t,
/// bounded to [erp_min, erp_max]. The p1-equity terms (b_f = `erp_fin_loading`, b_rp = `erp_rp_loading`, κ_rp =
/// `erp_rp_gap_scale`, b_s = `erp_slow_loading`) are all 0 by default; f_t is the financial factor, rp_t the demand
/// risk-premium shock and S_t its slow EWMA (`Fund::erp_slow`). The countercyclical term (Campbell & Shiller 1988;
/// Cochrane 2011) raises the premium in slumps. The extrapolative term (Greenwood & Shleifer 2014; Barberis et al. 2015)
/// lowers it after strong past returns. The extrapolative term is zero until 12 monthly returns exist.
fn equity_risk_premium(w: &World) -> f64 {
    let cfg = &w.cfg;
    let gap = w.unemployment_rate() - w.natural_unemployment();
    let momentum = trailing_equity_return(w).map_or(0.0, |r| {
        (r - cfg.erp_return_anchor).clamp(-cfg.erp_momentum_cap, cfg.erp_momentum_cap)
    });
    // p1-equity: the financial factor (cfg.erp_fin_loading) and the demand risk-premium shock (cfg.erp_rp_loading) enter
    // unsmoothed, so a risk-off move or a demand-driven slump reprices equities in the month it arrives.
    // The demand loading is scaled up in a slump (1 + erp_rp_gap_scale × max(gap, 0)): uncertainty about the premium is
    // largest when unemployment is high, which makes volatility countercyclical. The scale is 1 by default.
    let rp_scale = 1.0 + cfg.erp_rp_gap_scale * gap.max(0.0);
    // Re-fit Step 10a: the financial term is the build-up of f_t − f̄ when erp_fin_build_months > 0, else f_t (legacy).
    // The demand term loads on the AR state rp_ar, so the financial-factor jumps reach the ERP only through the build-up.
    let fin_term = if cfg.erp_fin_build_months > 0.0 { w.fund.fin_erp_build } else { w.exog.fin_factor() };
    let erp = cfg.erp + cfg.erp_unemp_sens * gap - cfg.erp_extrap * momentum
        + cfg.erp_fin_loading * fin_term
        + cfg.erp_rp_loading * rp_scale * w.exog.rp_ar
        + cfg.erp_slow_loading * w.fund.erp_slow;
    if erp.is_finite() {
        erp.clamp(cfg.erp_min, cfg.erp_max)
    } else {
        cfg.erp
    }
}

/// Sector equity valuation (stage 4, docs/SECTORS.md §Stage 4). For each sector s, with the raw capitalisation rate
///   r_s = y_eq + ERP_t + erp_sector_s [− tech boom discount] − g_s^e,
/// the value is E_s = 12 × smoothed dividends_s ÷ cap_s in the base 1/cap form, where cap_s = max(cap_floor, r_s),
/// cap_floor = max(`equity_cap_floor`, MIN_CAP_RATE) (0.02 by default). The p1-equity alternatives are:
/// `equity_cap_softness` > 0 replaces the hard floor by the smooth bound of `bounded_cap`, and `equity_valuation_log`
/// uses the log-linear form E_s = 12 × dividends_s ÷ c_s × exp((c_s − r_s) ÷ c_s), where c_s is the sector's reference
/// rate fixed at seeding (ln E_s is linear in r_s, so the log return is linear in the change in r_s and has no 1/cap
/// convexity). The log form has no capitalisation-rate floor, but r_s is bounded below at LOG_CAP_GUARD (0.005), which
/// binds for Tech in many boom months. The profile uses the log form.
///   g_s^e = clamp(long_growth + κ × (x_s − long_growth), [g_min, g_max]),
/// where x_s is the slow EWMA of the sector's annualised dividend growth (the extrapolative signal) and κ is
/// `sector_growth_extrapolation`. Dividends are the sector's regular payouts from firms, with bank dividends in
/// OtherServices. Updates the per-sector state in `w.fund` (smoothed dividends, growth signal, q) and returns the
/// total, Σ_s E_s. `seed_equity` = Some(E₀) on the first valuation: the sector dividends are backed out of E₀ so that
/// month 1 keeps the seeded total (weights from this month's sector profits, or equal weights). R51-E: an active
/// `Shock::SharePrices` multiplies each sector's value E_s by its wedge `ShockState::share_price_wedge` after E_s is formed
/// (docs/DESIGN.md §12), so the value that reaches `sector_equity`, the Tobin's q and the Fund's NAV is the re-rated one while
/// the dividends, the growth signal and the capitalisation rate stay the model's own.
fn value_sectors(w: &mut World, y_eq: f64, erp: f64, div_received: f64, seed_equity: f64) -> f64 {
    let cfg = &w.cfg;
    let lg = cfg.long_growth;
    let decay = cfg.sector_growth_decay;
    let ext_w = cfg.sector_growth_extrapolation;
    let (g_min, g_max) = (cfg.sector_growth_min, cfg.sector_growth_max);
    let erp_sector = cfg.erp_sector;
    // p1-equity: floor on the capitalisation rate (never below MIN_CAP_RATE), which bounds the 1/cap convexity, and its
    // smoothing (0 = hard floor).
    let cap_floor = cfg.equity_cap_floor.max(MIN_CAP_RATE);
    let cap_soft = cfg.equity_cap_softness;
    // p1-equity: log-linear valuation (Campbell & Shiller 1988): ln E_s is linear in the capitalisation rate, with the
    // sector's reference rate c_s (set at seeding) as the point of tangency with the 1/cap valuation.
    let cap_log = cfg.equity_valuation_log;
    let mut cap_ref_new = [0.0; N_SECTORS];
    let boom = w.exog.tech_boom;
    // p1-equity: the Tech discount (Pástor & Veronesi 2009) is phased in through Tech's productivity-news EWMA rather
    // than switched on with the boom flag, so a boom's onset is gradual and it unwinds with the news (bust decay).
    let news_full = cfg.tech_boom_extra_growth;
    let tech_disc = |news: f64| -> f64 {
        if cfg.tech_disc_news_phase && news_full > EPS {
            cfg.tech_boom_erp_discount * (news / news_full).clamp(0.0, 1.0)
        } else if boom {
            cfg.tech_boom_erp_discount
        } else {
            0.0
        }
    };
    let news_weight = cfg.sector_productivity_news;
    let news_decay = cfg.sector_news_decay;
    let seeding = seed_equity >= 0.0;
    // R51-E: the `SharePrices` shock's wedge on each sector's valuation (None, and never read below, unless one is active).
    let wedge = w.shocks.share_price_wedge();
    let fund = &w.fund;

    // Dividends by sector: firms' regular payouts, plus the bank residual to OtherServices.
    let mut div = fund.sector_dividends_received;
    let firm_div: f64 = div.iter().sum();
    div[Sector::OtherServices.idx()] += (div_received - firm_div).max(0.0);
    // Seeding (first valuation): the seeded equity is split by this month's sector dividends (or by sector capital if
    // there are none), and each sector's smoothed dividend is the one that its own capitalisation rate would value at
    // trend growth. Starting from the actual sector split avoids a spurious growth transient as the series converges.
    let mut cap_units = [0.0; N_SECTORS];
    for f in w.firms.iter().filter(|f| f.alive) {
        cap_units[f.sector.idx()] += f.capital.max(0.0);
    }
    let cap_sum: f64 = cap_units.iter().sum();
    let div_sum: f64 = div.iter().sum();
    let weights: [f64; N_SECTORS] = if div_sum > EPS {
        div.map(|d| d.max(0.0) / div_sum)
    } else if cap_sum > EPS {
        cap_units.map(|k| k / cap_sum)
    } else {
        [1.0 / N_SECTORS as f64; N_SECTORS]
    };
    let p_cap = common::capital_price_world(w);

    let mut sd_new = [0.0; N_SECTORS];
    let mut eq_new = [0.0; N_SECTORS];
    let mut growth_new = [0.0; N_SECTORS];
    let mut slow_new = [0.0; N_SECTORS];
    let mut news_new = [0.0; N_SECTORS];
    let lag_fast = DIVIDEND_SMOOTHING / (1.0 - DIVIDEND_SMOOTHING);
    let lag_slow = decay / (1.0 - decay);
    // Economy-wide growth signal (same construction): sector growth is measured relative to it, so the common
    // cycle in dividends cancels and only sector-specific growth is extrapolated.
    let agg_sd: f64 = (0..N_SECTORS).map(|s| if seeding { 0.0 } else { DIVIDEND_SMOOTHING * fund.sector_smoothed_dividends[s] + (1.0 - DIVIDEND_SMOOTHING) * div[s] }).sum();
    let agg_slow_old: f64 = fund.sector_slow_dividends.iter().sum();
    let x_agg = if seeding {
        lg
    } else {
        let slow = decay * agg_slow_old + (1.0 - decay) * agg_sd;
        if agg_sd > EPS && slow > EPS {
            (12.0 * (agg_sd / slow).ln() / (lag_slow - lag_fast)).clamp(-0.5, 0.5)
        } else {
            lg
        }
    };
    for s in 0..N_SECTORS {
        let sd_old = fund.sector_smoothed_dividends[s];
        let x_old = if fund.sector_growth[s] == 0.0 { lg } else { fund.sector_growth[s] };
        let sd = if seeding {
            let raw0 = y_eq + erp + erp_sector[s] - if s == Sector::Tech.idx() { tech_disc(fund.sector_news[s]) } else { 0.0 } - lg;
            let cap0 = if cap_log {
                cap_ref_new[s] = raw0.max(LOG_CAP_GUARD);
                cap_ref_new[s]
            } else {
                bounded_cap(raw0, cap_floor, cap_soft)
            };
            weights[s] * seed_equity * cap0 / 12.0
        } else {
            DIVIDEND_SMOOTHING * sd_old + (1.0 - DIVIDEND_SMOOTHING) * div[s]
        };
        // Growth signal: for a series growing at rate g, ln(fast EWMA ÷ slow EWMA) = g × (lag_slow − lag_fast) / 12,
        // with EWMA lags d/(1−d) months. The slow leg averages out the monthly noise in sector payouts. On seeding it
        // is set so that the signal starts at trend growth (no spurious jump in the premium).
        let (x, slow) = if seeding {
            (lg, sd * (-lg * (lag_slow - lag_fast) / 12.0).exp())
        } else {
            let slow = decay * fund.sector_slow_dividends[s] + (1.0 - decay) * sd;
            let x = if sd > EPS && slow > EPS {
                (12.0 * (sd / slow).ln() / (lag_slow - lag_fast)).clamp(-0.5, 0.5)
            } else {
                x_old
            };
            (x, slow)
        };
        slow_new[s] = slow;
        growth_new[s] = x;
        // Productivity news: excess productivity growth in the sector during a boom, smoothed.
        let excess = if !boom {
            0.0
        } else if s == Sector::Tech.idx() {
            cfg.tech_boom_extra_growth
        } else if ADOPTER_SECTORS.contains(&s) {
            cfg.tech_boom_spillover
        } else {
            0.0
        };
        // Optimism builds slowly during a boom but unwinds quickly once it ends (bubbles deflate within quarters).
        let decay = if boom { news_decay } else { cfg.sector_news_decay_bust };
        let news = decay * fund.sector_news[s] + (1.0 - decay) * excess;
        news_new[s] = news;
        // The extrapolative term is capped too: a small sector's noisy payouts must not swing its premium by ±4pp.
        let ext = (ext_w * (x - x_agg)).clamp(EXT_MIN, EXT_MAX);
        let g_e = (lg + ext + news_weight * news).clamp(g_min, g_max);
        let premium = erp + erp_sector[s] - if s == Sector::Tech.idx() { tech_disc(news) } else { 0.0 };
        let raw = y_eq + premium - g_e;
        let e = if cap_log {
            // Log-linear form: E = 12·sd ÷ c × exp((c − r) ÷ c), so d ln E ÷ d r = −1/c, the same slope as the 1/cap
            // valuation at the reference c, with no convexity in ln E. c is fixed at seeding (or at the first valuation
            // after it), so each sector keeps its own steady-state level. The bound LOG_CAP_GUARD on r is a deliberate
            // lower bound on the valuation: where it binds, the sector does not respond to rates or premia.
            let r = raw.max(LOG_CAP_GUARD);
            let c = if fund.sector_cap_ref[s] > EPS { fund.sector_cap_ref[s] } else { r };
            cap_ref_new[s] = c;
            12.0 * sd / c * ((c - r) / c).exp()
        } else {
            12.0 * sd / bounded_cap(raw, cap_floor, cap_soft)
        };
        // R51-E: a user share-price re-rating multiplies the model value after it is formed. Dividends (`sd`), the growth signal and
        // the capitalisation rate above are untouched, so the shock is a level wedge on the price, not a change in earnings or rates
        // (and it works the same in the 1/cap and the log-linear form, and where LOG_CAP_GUARD binds).
        let e = match &wedge {
            Some(wd) => e * wd[s],
            None => e,
        };
        sd_new[s] = if sd.is_finite() { sd } else { 0.0 };
        eq_new[s] = if e.is_finite() { e } else { 0.0 };
    }

    // Tobin's q by sector: equity ÷ replacement value of the sector's capital, relative to its normal level.
    // fix-monetary: the q that investment reads is an EWMA of the raw ratio (cfg.q_signal_decay), so a boom's rise in
    // Tech's q reaches investment over months rather than in one step. Non-boom valuations also set the normal q that
    // init::build_world rebases to when the burn-in ends inside a boom.
    let mut q0 = w.fund.sector_q0;
    let mut q_rel = [1.0; N_SECTORS];
    let q_decay = cfg.q_signal_decay.clamp(0.0, 0.999);
    for s in 0..N_SECTORS {
        let repl = cap_units[s] * p_cap;
        if repl > EPS && eq_new[s] > EPS {
            let q = eq_new[s] / repl;
            if q0[s] <= 0.0 {
                q0[s] = q;
            }
            let raw = q / q0[s];
            // The normal q is a slow average of non-boom valuations (12-month half-life), frozen while a boom runs. It
            // is the reference that init::build_world rebases to, so one month's (or the unconverged early burn-in's)
            // valuation does not set it.
            // R51-E: also frozen while a share-price re-rating is in force in the sector (the normal q is the model's own level).
            let rerated = wedge.as_ref().map_or(false, |wd| wd[s] != 1.0);
            if !boom && !rerated {
                let nq = w.monetary.normal_q[s];
                w.monetary.normal_q[s] = if nq > 0.0 { nq + NORMAL_Q_RATE * (q - nq) } else { q };
            }
            let prev = fund.sector_q_rel[s];
            q_rel[s] = if prev > 0.0 && q_decay > 0.0 { q_decay * prev + (1.0 - q_decay) * raw } else { raw };
        }
    }

    let total: f64 = eq_new.iter().sum();
    w.fund.sector_smoothed_dividends = sd_new;
    w.fund.sector_equity = eq_new;
    w.fund.sector_growth = growth_new;
    w.fund.sector_slow_dividends = slow_new;
    w.fund.sector_news = news_new;
    w.fund.sector_q0 = q0;
    w.fund.sector_q_rel = q_rel;
    if cap_log {
        w.fund.sector_cap_ref = cap_ref_new;
    }
    w.fund.smoothed_dividends = sd_new.iter().sum();
    total
}

/// Equity revaluation at the valuation step of `markets` (§11.4): sets the month's equity risk premium and long rate,
/// revalues each sector, records the Fund's monthly equity return and stores the new total as `fund.equity_value`.
/// `div_received` and `special` are the Fund's firm and special dividends this month, which are paid out to households
/// and count toward the return. Returns the new equity value.
pub fn revalue_fund_equity(w: &mut World, div_received: f64, special: f64) -> f64 {
    // p1-equity: the share-price EWMA moves with the price that closed the previous month.
    let p = w.fund.share_price;
    if p.is_finite() && p > 0.0 {
        let prev = w.fund.share_price_ewma;
        w.fund.share_price_ewma = if prev > 0.0 && prev.is_finite() {
            SHARE_PRICE_EWMA_DECAY * prev + (1.0 - SHARE_PRICE_EWMA_DECAY) * p
        } else {
            p
        };
    }
    // p1-equity: the slow demand-premium state moves with this month's demand premium (EWMA, weight 1 − erp_slow_decay).
    let slow_decay = w.cfg.erp_slow_decay.clamp(0.0, 1.0);
    let rp = w.exog.risk_premium;
    if rp.is_finite() {
        w.fund.erp_slow = slow_decay * w.fund.erp_slow + (1.0 - slow_decay) * rp;
    }
    // Re-fit Step 10a: the financial-factor term of the ERP builds up over erp_fin_build_months (f_t − f̄ EWMA).
    let build = w.cfg.erp_fin_build_months;
    if build > 0.0 {
        let target = w.exog.fin_factor() - crate::exog::fin_factor_mean(&w.cfg);
        if target.is_finite() {
            w.fund.fin_erp_build += (target - w.fund.fin_erp_build) / build;
        }
    }
    // Time-varying equity risk premium (§11.4) from the unemployment gap and past equity returns (before this month's
    // return enters the history, so the valuation uses only information available at the start of the month).
    let erp = equity_risk_premium(w);
    w.monetary.erp_current = erp;
    // Equity investors discount at a smoothed long rate: they look partly through transitory moves in Treasury yields
    // (stocks fell ≈8% per 100bp of 10-year yield in 2022, and QE announcements moved them by a few percent).
    let y10 = w.curve.yields[3];
    let lag = w.cfg.equity_rate_months.max(0.0);
    let y_eq = match w.monetary.equity_long_rate {
        Some(p) if lag > 0.0 && p.is_finite() => p + (y10 - p) / (1.0 + lag),
        _ => y10,
    };
    w.monetary.equity_long_rate = Some(y_eq);
    // First call: if the Fund already carries an equity value (init seeds it) but no smoothed dividends, back
    // the smoothed dividends out of that value so month 1 keeps E unchanged instead of resetting it to 0.
    let seed = !w.monetary.equity_init && w.fund.equity_value.is_finite() && w.fund.equity_value > 0.0;
    w.monetary.equity_init = true;
    let equity_prev = w.fund.equity_value;
    let equity = value_sectors(w, y_eq, erp, div_received, if seed { equity_prev } else { -1.0 });
    w.fund.dividends_received = 0.0;
    w.fund.sector_dividends_received = [0.0; N_SECTORS];
    // Monthly total return on the Fund's equity: (E_t + equity dividends paid this month) / E_{t−1} − 1.
    if equity_prev > EPS && equity.is_finite() {
        let paid = div_received + special;
        let r = (equity + paid) / equity_prev - 1.0;
        if r.is_finite() {
            w.monetary.equity_returns.push(r);
            let excess = w.monetary.equity_returns.len().saturating_sub(12);
            w.monetary.equity_returns.drain(..excess);
        }
    }
    w.fund.equity_value = equity;
    equity
}

#[cfg(test)]
mod share_price_tests {
    //! R51-E: the `SharePrices` shock, at the valuation step (tiny worlds) and through the whole month (paired worlds).
    use super::*;
    use crate::config::Config;
    use crate::metrics::Metrics;
    use crate::shocks::{ScheduledShock, Shock};
    use crate::sim::{self, Comparison, Scenario};
    use std::sync::OnceLock;

    // ---- the valuation step: one month from a common world, with the re-rating put into the shock state ----

    const N: usize = 1500;
    const SEEDS: [u64; 3] = [1, 2, 3];

    fn cfg() -> Config {
        Config { n_households: N, ..Config::default() }
    }

    static WORLDS: OnceLock<Vec<World>> = OnceLock::new();

    /// The common world of each seed, built once for all the tests below (they clone it). Called from the test thread before any
    /// parallel run: building a world uses rayon, so building it from inside a rayon worker (the seeds of `compare_with` run on
    /// workers) while other workers wait for it would deadlock.
    fn worlds() -> &'static Vec<World> {
        WORLDS.get_or_init(|| SEEDS.iter().map(|&s| sim::common_world(&cfg(), s).0).collect())
    }

    fn common(seed: u64) -> sim::CommonWorld {
        let worlds = WORLDS.get().expect("worlds() ran first");
        (worlds[SEEDS.iter().position(|&s| s == seed).expect("a test seed")].clone(), None)
    }

    fn world(seed: u64) -> World {
        worlds();
        common(seed).0
    }

    /// A re-rating already in force, as `activate_due` leaves it (sector, log size now, monthly decay factor).
    fn rerate(w: &mut World, sector: Option<usize>, log: f64, decay: f64) {
        w.shocks.share_price_log.push((sector, log, decay));
    }

    #[test]
    fn a_rerating_multiplies_one_sectors_value_and_leaves_dividends_growth_and_rates_alone() {
        let t = Sector::Tech.idx();
        for seed in SEEDS {
            let mut base = world(seed);
            let mut hit = base.clone();
            rerate(&mut hit, Some(t), 0.2f64.ln(), 1.0);
            sim::step(&mut base);
            sim::step(&mut hit);
            assert!(base.fund.sector_equity[t] > 0.0, "seed {seed}: Tech has a value");
            // Everything before the valuation is the same in both worlds, so the gap is the wedge, to rounding.
            let ratio = hit.fund.sector_equity[t] / base.fund.sector_equity[t];
            assert!((ratio - 0.2).abs() < 1e-12, "seed {seed}: tech value ratio {ratio}");
            for s in (0..N_SECTORS).filter(|&s| s != t) {
                assert_eq!(hit.fund.sector_equity[s].to_bits(), base.fund.sector_equity[s].to_bits(), "seed {seed}: sector {s} must not move");
            }
            // Not touched: the smoothed dividends, the growth signals, the news, the premium and the discount rate.
            assert_eq!(hit.fund.sector_smoothed_dividends, base.fund.sector_smoothed_dividends);
            assert_eq!(hit.fund.sector_growth, base.fund.sector_growth);
            assert_eq!(hit.fund.sector_news, base.fund.sector_news);
            assert_eq!(hit.monetary.erp_current.to_bits(), base.monetary.erp_current.to_bits());
            assert_eq!(hit.monetary.equity_long_rate, base.monetary.equity_long_rate);
            // The total, the Fund's NAV and the share price follow the re-rated value.
            let total: f64 = hit.fund.sector_equity.iter().sum();
            assert!((hit.fund.equity_value - total).abs() <= 1e-9 * total);
            assert!(hit.fund.equity_value < base.fund.equity_value && hit.fund.nav < base.fund.nav);
            assert!(hit.fund.share_price < base.fund.share_price, "price {} vs {}", hit.fund.share_price, base.fund.share_price);
            // The month's return carries the loss on the re-rated sector.
            let (r, rb) = (*hit.monetary.equity_returns.last().unwrap(), *base.monetary.equity_returns.last().unwrap());
            assert!(r < rb - 0.02, "return {r} vs {rb}");
            crate::ledger::check_consistency(&hit).unwrap();
        }
    }

    #[test]
    fn a_rerating_of_all_sectors_scales_every_sector_and_tobins_q_follows() {
        let mut base = world(1);
        let mut hit = base.clone();
        rerate(&mut hit, None, 0.7f64.ln(), 1.0);
        sim::step(&mut base);
        sim::step(&mut hit);
        for s in 0..N_SECTORS {
            let (a, b) = (hit.fund.sector_equity[s], base.fund.sector_equity[s]);
            assert!(b > 0.0 && (a / b - 0.7).abs() < 1e-12, "sector {s}: {a} vs {b}");
        }
        // q is the equity over the replacement value, smoothed (q_signal_decay): it falls, by less than the equity does on impact.
        let mut checked = 0;
        for s in 0..N_SECTORS {
            if base.fund.sector_q0[s] > 0.0 {
                let (a, b) = (hit.fund.sector_q_rel[s], base.fund.sector_q_rel[s]);
                assert!(a < b && a > 0.7 * b, "sector {s} q {a} vs {b}");
                checked += 1;
            }
        }
        assert!(checked >= 5, "q is set in {checked} sectors");
    }

    #[test]
    fn a_world_without_a_rerating_is_bit_identical_and_a_zero_one_changes_nothing() {
        let mut a = world(2);
        let mut zero = a.clone();
        // Scheduled and fired, but 0% and permanent: the wedge is exp(0) = 1 exactly in every month, so the worlds stay the same.
        zero.shocks.schedule(ScheduledShock { at_month: zero.tick, shock: Shock::SharePrices { sector: None, pct: 0.0, half_life_months: 0.0 } });
        for _ in 0..3 {
            sim::step(&mut a);
            sim::step(&mut zero);
        }
        assert!(a.shocks.share_price_wedge().is_none());
        assert!(zero.shocks.share_price_wedge().is_some(), "the zero shock was active");
        assert_eq!(serde_json::to_string(&a.history).unwrap(), serde_json::to_string(&zero.history).unwrap());
        for s in 0..N_SECTORS {
            assert_eq!(zero.fund.sector_equity[s].to_bits(), a.fund.sector_equity[s].to_bits(), "sector {s}");
        }
        assert_eq!(zero.fund.share_price.to_bits(), a.fund.share_price.to_bits());
        assert_eq!(zero.fund.sector_q_rel, a.fund.sector_q_rel);
    }

    #[test]
    fn the_normal_q_is_frozen_while_a_rerating_is_in_force_in_the_sector() {
        let t = Sector::Tech.idx();
        let mut tested = 0;
        for seed in SEEDS {
            let mut base = world(seed);
            let before = base.monetary.normal_q;
            let mut hit = base.clone();
            rerate(&mut hit, Some(t), 0.2f64.ln(), 1.0);
            let mut boom = false;
            for _ in 0..6 {
                sim::step(&mut base);
                sim::step(&mut hit);
                boom |= base.exog.tech_boom || hit.exog.tech_boom;
            }
            if boom {
                continue; // a boom freezes the normal q in every sector, so there is nothing to tell apart
            }
            tested += 1;
            // The re-rated sector's normal q is where it was; the others, not re-rated, are still moving with the valuations.
            assert_eq!(hit.monetary.normal_q[t].to_bits(), before[t].to_bits(), "seed {seed}: frozen in the re-rated sector");
            assert!((0..N_SECTORS).filter(|&s| s != t).any(|s| hit.monetary.normal_q[s] != before[s]), "seed {seed}: the other sectors move");
            assert!(base.monetary.normal_q[t] != before[t], "seed {seed}: without the re-rating it moves");
        }
        assert!(tested > 0, "every test seed was in a boom: pick other seeds");
    }

    // ---- paired worlds: baseline and shocked arms from one common world ----

    fn paired(shocks: Vec<ScheduledShock>, seeds: &[u64], months: u32) -> Comparison {
        worlds();
        let sc = Scenario { config: cfg(), shocks, months, seeds: seeds.to_vec() };
        assert!(sc.config.sfc_enforce, "the stock-flow checks are on, so a bad month would stop the run");
        sim::compare_with(&sc, &|seed| common(seed), &mut |_| {})
    }

    fn at(month: u32, shock: Shock) -> Vec<ScheduledShock> {
        vec![ScheduledShock { at_month: month, shock }]
    }

    fn tech(pct: f64, hl: f64) -> Shock {
        Shock::SharePrices { sector: Some(Sector::Tech.idx()), pct, half_life_months: hl }
    }

    fn all(pct: f64, hl: f64) -> Shock {
        Shock::SharePrices { sector: None, pct, half_life_months: hl }
    }

    fn tech_equity(m: &Metrics) -> f64 {
        m.equity_value_by_sector[Sector::Tech.idx()]
    }

    #[test]
    fn tech_minus_80_permanent_takes_80_percent_off_tech_on_impact_against_the_paired_baseline() {
        for seed in SEEDS {
            let c = paired(at(0, tech(-80.0, 0.0)), &[seed], 6);
            // Impact month. Nothing before the valuation step of that month differs between the arms (households decide
            // on last month's price), so the gap is the wedge alone: 1 + pct/100 = 0.2, and exp(ln 0.2) differs from 0.2 by
            // a rounding error. 1e-9 is a bound on rounding, not on economics.
            let ratio = tech_equity(&c.shocked[0]) / tech_equity(&c.baseline[0]);
            assert!((ratio - 0.2).abs() < 1e-9, "seed {seed}: tech value ratio on impact {ratio}");
            // Every other sector is the same on impact, bit for bit.
            for s in (0..N_SECTORS).filter(|&s| s != Sector::Tech.idx()) {
                assert_eq!(c.shocked[0].equity_value_by_sector[s].to_bits(), c.baseline[0].equity_value_by_sector[s].to_bits(), "sector {s}");
            }
            // The Fund and its price fall by Tech's share of the Fund, so by less than 80%.
            let drop = 1.0 - c.shocked[0].fund_share_price / c.baseline[0].fund_share_price;
            assert!(drop > 0.02 && drop < 0.8, "seed {seed}: the Fund's price fell {drop}");
        }
    }

    #[test]
    fn the_gap_follows_the_half_life_and_the_permanent_gap_holds() {
        // A 3-month half-life: half of the jump is left after three months, a quarter after six. After the impact month the
        // economy answers (dividends, the premium), so the world's gap is the wedge plus a feedback term that grows with
        // the horizon. Measured, in log points: at most 0.02 through month 6 on the seeds below (0.04 on seeds 1-4 at 5,000
        // households), and larger later (0.05 by month 8 at 1,500 households, up to 0.25 at 5,000 households by month 14),
        // which is why the horizon is six months and the bound 0.06 (the half-life separates the cases by 0.8). The decay
        // rate itself is exact in `shocks::tests::share_price_gap_decays_with_its_half_life`.
        let l = 0.2f64.ln();
        for seed in SEEDS {
            let decaying = paired(at(0, tech(-80.0, 3.0)), &[seed], 8);
            let permanent = paired(at(0, tech(-80.0, 0.0)), &[seed], 8);
            let gap = |c: &Comparison, k: usize| (tech_equity(&c.shocked[k]) / tech_equity(&c.baseline[k])).ln();
            assert!((gap(&decaying, 0) - l).abs() < 1e-9, "impact");
            assert!((gap(&permanent, 0) - l).abs() < 1e-9, "impact");
            for k in 0..=6usize {
                let want = l * 0.5f64.powf(k as f64 / 3.0);
                assert!((gap(&decaying, k) - want).abs() < 0.06, "seed {seed} k {k}: gap {} want {want}", gap(&decaying, k));
                assert!((gap(&permanent, k) - l).abs() < 0.06, "seed {seed} k {k}: permanent gap {} want {l}", gap(&permanent, k));
            }
            // At one half-life the decaying gap is about half the permanent one: far from both the full jump and zero.
            let (d3, p3) = (gap(&decaying, 3), gap(&permanent, 3));
            assert!(d3 / p3 > 0.35 && d3 / p3 < 0.65, "seed {seed}: {d3} / {p3}");
            for k in 1..=6usize {
                assert!(gap(&decaying, k) > gap(&decaying, k - 1) - 0.03, "seed {seed} k {k}: the gap closes");
            }
        }
    }

    #[test]
    fn a_shock_leaves_every_month_before_it_unchanged() {
        let c = paired(at(5, all(-50.0, 0.0)), &[1], 9);
        for k in 0..5 {
            let (a, b) = (serde_json::to_string(&c.baseline[k]).unwrap(), serde_json::to_string(&c.shocked[k]).unwrap());
            assert_eq!(a, b, "month {k} is before the shock");
        }
        assert!(tech_equity(&c.shocked[5]) < 0.6 * tech_equity(&c.baseline[5]));
    }

    #[test]
    fn an_unused_share_price_input_moves_no_draw() {
        // 0% (a wedge of exp(0) = 1) and a shock that is scheduled after the run both leave every record of the shocked arm
        // equal to the baseline's, byte for byte (the arms share every random draw).
        for shocks in [at(0, all(0.0, 12.0)), at(0, tech(0.0, 0.0)), at(500, all(-80.0, 0.0))] {
            let c = paired(shocks.clone(), &[1, 2], 6);
            assert_eq!(serde_json::to_string(&c.baseline).unwrap(), serde_json::to_string(&c.shocked).unwrap(), "{shocks:?}");
        }
        // And a run with no shocks at all is the baseline of one with a share-price shock: the baseline arm is the same either way.
        let with = paired(at(0, all(-30.0, 0.0)), &[1], 6);
        let without = paired(Vec::new(), &[1], 6);
        assert_eq!(serde_json::to_string(&with.baseline).unwrap(), serde_json::to_string(&without.baseline).unwrap());
    }

    #[test]
    fn share_price_runs_are_deterministic_and_do_not_depend_on_the_thread_count() {
        worlds();
        let sc = at(2, all(-40.0, 12.0));
        let a = serde_json::to_string(&paired(sc.clone(), &[1, 2], 8)).unwrap();
        let b = serde_json::to_string(&paired(sc.clone(), &[1, 2], 8)).unwrap();
        assert_eq!(a, b, "the same request twice");
        for threads in [1usize, 4] {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let c = pool.install(|| serde_json::to_string(&paired(sc.clone(), &[1, 2], 8)).unwrap());
            assert_eq!(a, c, "{threads} threads");
        }
    }

    /// The first-year sum of a monthly series over seeds' mean, shocked minus baseline.
    fn year_gap(c: &Comparison, f: impl Fn(&Metrics) -> f64) -> f64 {
        c.shocked.iter().zip(&c.baseline).take(12).map(|(s, b)| f(s) - f(b)).sum()
    }

    #[test]
    fn a_fall_lowers_wealth_spending_and_tech_investment_and_raises_the_premium() {
        let c = paired(at(0, all(-30.0, 0.0)), &SEEDS, 14);
        // Consumption: nothing on impact (households decided on last month's price), then lower over the year.
        assert_eq!(c.shocked[0].consumption_real_bn_ar.to_bits(), c.baseline[0].consumption_real_bn_ar.to_bits());
        let cons = year_gap(&c, |m| m.consumption_real_bn_ar);
        assert!(cons < 0.0, "first-year real consumption gap {cons} (annual-rate $bn summed over months)");
        // The Fund's price is down on impact and through the year; the 12-month return is deeply negative.
        assert!(c.shocked[0].fund_share_price < 0.9 * c.baseline[0].fund_share_price);
        assert!((1..12).all(|k| c.shocked[k].fund_share_price < c.baseline[k].fund_share_price));
        assert!(c.shocked[3].equity_return_12m < c.baseline[3].equity_return_12m - 20.0);
        // The risk premium rises through the extrapolative term (returns below the anchor), after the impact month.
        assert_eq!(c.shocked[0].equity_risk_premium.to_bits(), c.baseline[0].equity_risk_premium.to_bits());
        assert!((2..9).all(|k| c.shocked[k].equity_risk_premium > c.baseline[k].equity_risk_premium), "premium");
        // Tobin's q of Tech falls, and with it Tech's investment (the sector with a q sensitivity, `investment_q_sens`).
        assert!(c.shocked[6].obs.sector_q[Sector::Tech.idx()] < c.baseline[6].obs.sector_q[Sector::Tech.idx()]);
        let tinv = year_gap(&c, |m| m.tech_investment_bn_ar);
        assert!(tinv < 0.0, "first-year Tech investment gap {tinv}");
        assert!(c.shocked[11].tech_investment_bn_ar < c.baseline[11].tech_investment_bn_ar);
        // Total real investment is lower over the year (Tech's cut and the weaker demand).
        let inv = year_gap(&c, |m| m.investment_real_bn_ar);
        assert!(inv < 0.0, "first-year real investment gap {inv}");
    }

    #[test]
    fn a_rise_moves_the_same_channels_the_other_way() {
        let c = paired(at(0, all(100.0, 0.0)), &SEEDS, 14);
        assert!(c.shocked[0].fund_share_price > 1.3 * c.baseline[0].fund_share_price);
        let cons = year_gap(&c, |m| m.consumption_real_bn_ar);
        assert!(cons > 0.0, "first-year real consumption gap {cons}");
        let tinv = year_gap(&c, |m| m.tech_investment_bn_ar);
        assert!(tinv > 0.0, "first-year Tech investment gap {tinv}");
        assert!((2..9).all(|k| c.shocked[k].equity_risk_premium < c.baseline[k].equity_risk_premium), "premium");
    }

    #[test]
    fn the_stock_flow_checks_hold_and_no_month_is_stopped_at_tech_minus_80_or_all_minus_50() {
        // `paired` asserts sfc_enforce is on: a month that broke an identity would panic and stop the run. Both cases also run
        // through a manual world, with the full ledger check after every month, and every record carries sfc_error 0.
        for (label, shock) in [("tech -80%", tech(-80.0, 0.0)), ("all -50%", all(-50.0, 0.0)), ("tech -80%, hl 24", tech(-80.0, 24.0))] {
            let c = paired(at(0, shock.clone()), &SEEDS, 24);
            assert_eq!(c.shocked.len(), 24, "{label}: the run was cut short");
            assert!(c.shocked.iter().chain(&c.baseline).all(|m| m.sfc_error == 0.0), "{label}: sfc_error");
            assert!(c.shocked.iter().all(|m| m.fund_share_price.is_finite() && m.fund_share_price > 0.0), "{label}: Fund price");
            worlds();
            let (mut w, _) = common(1);
            w.shocks.schedule(ScheduledShock { at_month: w.tick, shock });
            for k in 0..24 {
                sim::step(&mut w);
                crate::ledger::check_consistency(&w).unwrap_or_else(|e| panic!("{label}, month {k}: {e}"));
            }
            assert!(w.fund.share_price.is_finite() && w.fund.share_price > 0.0);
        }
    }
}
