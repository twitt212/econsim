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
/// month 1 keeps the seeded total (weights from this month's sector profits, or equal weights).
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
            if !boom {
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
