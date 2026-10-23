//! Commercial banks (docs/DESIGN.md §11.1–11.3): rates, credit supply, loan servicing, liquidity, dividends.
//! Cross-module split per §16: banking owns loan payments, delinquency counters, `firm.interest_paid`,
//! deposit and overdraft interest, IORB, discount window, bank P&L and dividends.

use rand::Rng;
use rayon::prelude::*;

use crate::common;
use crate::config::Config;
use crate::ledger::{self, Account};
use crate::rng;
use crate::types::*;

/// Key for the income-shock miss stream (`stream::BANKING`); no other banking draw uses this stream.
const PH_SHOCK_MISS: u64 = 1;

/// Risk weight on mortgages (§11.2).
const RW_MORTGAGE: f64 = 0.5;
/// Risk weight on business loans (§11.2).
const RW_BUSINESS: f64 = 1.0;
/// Capital buffer above the requirement needed before a bank pays dividends (§11.3).
const DIVIDEND_BUFFER: f64 = 0.02;
/// Capital buffer above the requirement needed before a bank lends to firms (§11.2).
const BUSINESS_LOAN_BUFFER: f64 = 0.01;
/// Business loan term (months).
const BUSINESS_TERM_MONTHS: u32 = 60;
/// Maximum leverage (debt / assets) at which a maturing business loan is rolled over.
const MAX_ROLLOVER_LEVERAGE: f64 = 0.85;
/// Maximum post-loan firm leverage (debt / assets) for a business loan (§11.2).
const MAX_FIRM_LEVERAGE: f64 = 0.75;
/// Minimum interest coverage (earnings / total interest) for a business loan (§11.2).
const MIN_INTEREST_COVERAGE: f64 = 1.5;
/// Investor mortgages pay this much more than owner-occupier mortgages.
const INVESTOR_RATE_ADD: f64 = 0.005;
/// Share of current rent income counted as qualifying income for investors.
const INVESTOR_RENT_SHARE: f64 = 0.7;
/// Discount-window draw when reserves are negative: shortfall plus this share of deposits (§11.3).
const LIQUIDITY_BUFFER_SHARE: f64 = 0.01;
/// Target reserve floor (share of deposits) above which discount-window borrowing is repaid (§11.3).
const RESERVE_TARGET_SHARE: f64 = 0.05;
/// Leverage cap used when a firm has debt but no assets (keeps the rate premium finite).
const MAX_LEVERAGE: f64 = 10.0;
/// Interest coverage used for a firm with no interest-bearing debt (its default risk is then minimal).
const MAX_COVERAGE: f64 = 50.0;
/// Reference firm for default-risk pricing: leverage 0.5 and interest coverage 3 carry the average spread, so the
/// premium is zero for it (docs/DESIGN.md §11.1).
const PD_REF_LEVERAGE: f64 = 0.5;
const PD_REF_COVERAGE: f64 = 3.0;
/// Bounds on the firm default-risk premium over the bank base rate.
const PD_PREMIUM_MIN: f64 = -0.01;
const PD_PREMIUM_MAX: f64 = 0.06;

#[derive(Clone, Debug, Default)]
pub struct BankingState {
    // b9-rates M-M: dynamic mortgage spread. All zero/false until `update_mortgage_spread` first runs (and unused when both gains are 0).
    pub mtg_init: bool,
    /// 10y yield seen at the last update (annual decimal).
    pub mtg_y10_prev: f64,
    /// EWMA of |monthly change of the 10y yield| (decimal): realised rate volatility, and its slow norm.
    pub mtg_vol: f64,
    pub mtg_vol_norm: f64,
    /// Slow norm of the mean lender premium (decimal).
    pub mtg_stress_norm: f64,
    /// Current spread adjustment added to `Config::mortgage_spread` (decimal).
    pub mtg_adj: f64,
}

/// Terms a bank would offer for a mortgage.
#[derive(Clone, Copy, Debug)]
pub struct MortgageQuote {
    pub bank: BankId,
    pub rate: f64,
    /// Maximum principal the household qualifies for on a house of the given price.
    pub max_loan: f64,
}

#[inline]
fn finite_or_zero(x: f64) -> f64 {
    if x.is_finite() {
        x
    } else {
        0.0
    }
}

/// Risk-weighted assets from a bank's mortgage and business loan books (§11.2).
#[inline]
fn rwa(mortgages: f64, business: f64) -> f64 {
    RW_MORTGAGE * mortgages.max(0.0) + RW_BUSINESS * business.max(0.0)
}

/// Capital ratio = equity / risk-weighted assets (§11.2). With zero RWA the ratio is reported as 1.0
/// (fully capitalised) when equity is positive and 0.0 otherwise, so the result is always finite.
pub fn capital_ratio(w: &World, bank: BankId) -> f64 {
    let b = &w.banks[bank as usize];
    let risk = rwa(b.mortgage_loans, b.business_loans);
    let eq = b.equity();
    if risk <= 1e-9 {
        return if eq > 0.0 { 1.0 } else { 0.0 };
    }
    finite_or_zero(eq / risk)
}

/// Bank with the highest capital ratio among alive banks (None if no bank is alive).
fn best_capitalised_bank(w: &World) -> Option<BankId> {
    let mut best: Option<(BankId, f64)> = None;
    for (i, b) in w.banks.iter().enumerate() {
        if !b.alive {
            continue;
        }
        let cr = capital_ratio(w, i as BankId);
        if best.map_or(true, |(_, c)| cr > c) {
            best = Some((i as BankId, cr));
        }
    }
    best.map(|(i, _)| i)
}

/// Set deposit, mortgage and business-loan base rates on every bank from the yield curve (§11.1).
pub fn set_rates(w: &mut World) {
    let policy = finite_or_zero(w.cb.policy_rate);
    let deposit_rate = finite_or_zero((w.cfg.deposit_beta * policy).max(0.0));
    let req = w.cfg.capital_requirement;
    // rz1 F2: a decaying tail after the CreditSpread window (half-life 0 = the legacy rectangle).
    let credit = w.shocks.credit_spread_add_tail(w.tick, w.cfg.credit_tail_half_life_months);
    // Integration (fix-monetary): agency-MBS purchases compress the mortgage spread (mbs_spread_adj <= 0).
    // b9-rates M-M: with a volatility or stress gain the spread moves (update_mortgage_spread); otherwise the legacy statement.
    let mtg_dynamic = w.cfg.mortgage_spread_vol_gain != 0.0 || w.cfg.mortgage_spread_stress_gain != 0.0;
    if mtg_dynamic {
        update_mortgage_spread(w);
    }
    let mortgage_base = if mtg_dynamic {
        w.curve.yield_at(120.0) + w.cfg.mortgage_spread + w.banking.mtg_adj + credit + w.monetary.mbs_spread_adj
    } else {
        w.curve.yield_at(120.0) + w.cfg.mortgage_spread + credit + w.monetary.mbs_spread_adj
    };
    // b9-rates R-B: the policy rate enters the business base rate with weight w (0 = legacy, verbatim).
    // us-now: the mortgage-spread offset of the steered window (identity when steering is off)
    let mortgage_base = crate::start::mortgage_offset(w, mortgage_base);
    let business_base = if w.cfg.business_policy_weight > 0.0 {
        let wt = w.cfg.business_policy_weight.min(1.0);
        let term = 0.5 * w.curve.yield_at(24.0) + 0.5 * w.curve.yield_at(60.0);
        wt * policy + (1.0 - wt) * term + w.cfg.business_spread + credit + w.cfg.business_policy_offset
    } else {
        0.5 * w.curve.yield_at(24.0) + 0.5 * w.curve.yield_at(60.0) + w.cfg.business_spread + credit
    };
    update_lender_premium(w);
    for b in 0..w.banks.len() {
        // Capital-shortfall premium: banks below requirement + 2% pay more for mortgage funding.
        let shortfall = (req + DIVIDEND_BUFFER - capital_ratio(w, b as BankId)).max(0.0);
        let bank = &mut w.banks[b];
        bank.deposit_rate = deposit_rate;
        bank.mortgage_rate = finite_or_zero(mortgage_base + 0.5 * shortfall);
        // Business lending rate = curve + spread + the bank's lender balance-sheet premium (excess bond premium).
        bank.business_rate_base = finite_or_zero(business_base + bank.ebp);
    }
}

/// b9-rates M-M: spread adjustment = k_v (vol - vol_norm) + k_c (stress - stress_norm), bounded.
/// `vol` is an EWMA of |monthly change of the 10y yield|; the norms adapt slowly, after the adjustment is formed.
/// Reads last month's `bank.ebp` (`update_lender_premium` runs later in `set_rates`). Deterministic and sequential.
fn update_mortgage_spread(w: &mut World) {
    let y10 = finite_or_zero(w.curve.yield_at(120.0));
    let (kv, kc) = (w.cfg.mortgage_spread_vol_gain, w.cfg.mortgage_spread_stress_gain);
    let g_vol = 1.0 / (1.0 + w.cfg.mortgage_spread_vol_months.max(0.0));
    let g_norm = 1.0 / (1.0 + w.cfg.mortgage_spread_norm_months.max(0.0));
    let (lo, hi) = (w.cfg.mortgage_spread_min_adj, w.cfg.mortgage_spread_max_adj);
    let (vol_ref, stress_ref) = (w.cfg.mortgage_spread_vol_ref.max(0.0), w.cfg.mortgage_spread_stress_ref);
    let (mut sum, mut n) = (0.0f64, 0usize);
    for b in w.banks.iter() {
        if b.alive && b.ebp.is_finite() {
            sum += b.ebp;
            n += 1;
        }
    }
    let stress = if n > 0 { sum / n as f64 } else { stress_ref };
    let st = &mut w.banking;
    if !st.mtg_init {
        st.mtg_init = true;
        st.mtg_y10_prev = y10;
        st.mtg_vol = vol_ref;
        st.mtg_vol_norm = vol_ref;
        st.mtg_stress_norm = stress_ref;
        st.mtg_adj = 0.0;
        return;
    }
    let d = (y10 - st.mtg_y10_prev).abs();
    st.mtg_y10_prev = y10;
    st.mtg_vol += g_vol * (d - st.mtg_vol);
    let adj = kv * (st.mtg_vol - st.mtg_vol_norm) + kc * (stress - st.mtg_stress_norm);
    st.mtg_adj = if adj.is_finite() { adj.clamp(lo, hi) } else { 0.0 };
    // norms adapt after the adjustment is formed
    st.mtg_vol_norm += g_norm * (st.mtg_vol - st.mtg_vol_norm);
    st.mtg_stress_norm += g_norm * (stress - st.mtg_stress_norm);
}

/// Monthly lender balance-sheet premium (excess bond premium, §11.1). Each bank rolls last month's business
/// write-offs into its trailing 12-month history, compares the annualised loss rate (trailing write-offs ÷ business
/// loans) with a slow-moving norm, and targets a premium that rises with that gap and with any capital shortfall
/// below `ebp_capital_buffer` above the requirement. The premium adjusts by a partial step each month and is bounded.
/// Banks are processed in index order from state set in the previous month, so the result does not depend on threads.
fn update_lender_premium(w: &mut World) {
    let req = w.cfg.capital_requirement;
    let (loss_sens, cap_sens, buffer) = (w.cfg.ebp_loss_sens, w.cfg.ebp_capital_sens, w.cfg.ebp_capital_buffer);
    let (adjust, pmin, pmax) = (w.cfg.ebp_adjust, w.cfg.ebp_min, w.cfg.ebp_max);
    let (norm_adapt, norm_init) = (w.cfg.loss_norm_adapt, w.cfg.loss_norm_initial);
    let expected = expected_loss_rates(w);
    // rz20: the equity term, the same for every bank (0 and never computed with the key off).
    let mut eq_term = if w.cfg.ebp_equity_sens > 0.0 { w.cfg.ebp_equity_sens * (-w.fund.q_raw_log).max(0.0) } else { 0.0 };
    // rz20 (re-rating form): the same term from the log of a `SharePrices` re-rating only (0 with no re-rating in force).
    if w.cfg.rerate_ebp_sens > 0.0 && w.fund.rerate_log < 0.0 {
        eq_term += w.cfg.rerate_ebp_sens * (-w.fund.rerate_log);
    }
    for b in 0..w.banks.len() {
        let cr = capital_ratio(w, b as BankId);
        let bank = &mut w.banks[b];
        // Roll the month's business write-offs into the trailing 12-month window.
        let pos = bank.biz_hist_pos as usize % 12;
        bank.biz_writeoff_hist[pos] = bank.biz_writeoffs;
        bank.biz_writeoffs = 0.0;
        bank.biz_hist_pos = ((pos + 1) % 12) as u8;
        if !bank.alive {
            continue;
        }
        let rate = expected.get(b).copied().unwrap_or(0.0);
        let shortfall = (req + buffer - cr).max(0.0);
        step_premium(bank, rate, shortfall, &PremiumParams { eq_term, loss_sens, cap_sens, adjust, pmin, pmax, norm_adapt, norm_init });
    }
}

/// rz20: ln of the Fund's sector q relative to normal, averaged over the sectors with a q and weighted by their equity value (0 when no sector
/// has one yet). Negative when equity is valued below its normal level. Finite whatever the inputs.
pub fn fund_q_log(w: &World) -> f64 {
    let f = &w.fund;
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for s in 0..N_SECTORS {
        let (e, q) = (f.sector_equity[s], f.sector_q_rel[s]);
        if f.sector_q0[s] > 0.0 && e > 0.0 && e.is_finite() && q.is_finite() {
            num += e * q.max(1e-6).ln();
            den += e;
        }
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

struct PremiumParams {
    /// rz20: the equity term of the target (`ebp_equity_sens` x the fall of the Fund's q; 0 with the key off).
    eq_term: f64,
    loss_sens: f64,
    cap_sens: f64,
    adjust: f64,
    pmin: f64,
    pmax: f64,
    norm_adapt: f64,
    norm_init: f64,
}

/// One month of a bank's lender premium given its expected business loss rate and capital shortfall: the target is
/// sensitivity × (expected loss − its slow norm) + sensitivity × shortfall, bounded, closed partially each month.
fn step_premium(bank: &mut Bank, rate: f64, shortfall: f64, p: &PremiumParams) {
    let loans = bank.business_loans.max(0.0);
    if !(bank.biz_loss_norm > 0.0) {
        bank.biz_loss_norm = if rate > 0.0 { rate } else { p.norm_init };
    }
    let loss_gap = if loans > 1e-6 { rate - bank.biz_loss_norm } else { 0.0 };
    bank.biz_loss_norm += p.norm_adapt * (rate - bank.biz_loss_norm);
    let mut target = p.loss_sens * loss_gap + p.cap_sens * shortfall;
    if p.eq_term != 0.0 {
        target += p.eq_term;
    }
    let target = target.clamp(p.pmin, p.pmax);
    bank.ebp = finite_or_zero(bank.ebp + p.adjust * (target - bank.ebp)).clamp(p.pmin, p.pmax);
}

// ---------------------------------------------------------------- business lending

fn firm_debt(w: &World, f: FirmId) -> f64 {
    w.firms[f as usize]
        .loans
        .iter()
        .filter_map(|&l| w.loans.get(l))
        .map(|l| l.principal.max(0.0))
        .sum()
}

/// Annual interest bill on a firm's existing loans.
fn firm_interest(w: &World, f: FirmId) -> f64 {
    w.firms[f as usize]
        .loans
        .iter()
        .filter_map(|&l| w.loans.get(l))
        .map(|l| l.principal.max(0.0) * finite_or_zero(l.annual_rate).max(0.0))
        .sum()
}

/// Balance-sheet assets a bank sees for a firm (§11.1): capital at the capital price, deposits, inventory at price.
fn firm_assets(w: &World, f: FirmId) -> f64 {
    let fm = &w.firms[f as usize];
    let p_goods = common::capital_price_world(w);
    fm.capital.max(0.0) * p_goods + fm.deposits.max(0.0) + fm.inventory.max(0.0) * fm.price.max(0.0)
}

fn leverage(debt: f64, assets: f64) -> f64 {
    let lev = if assets > 1e-9 {
        debt / assets
    } else if debt > 1e-9 {
        MAX_LEVERAGE
    } else {
        0.0
    };
    finite_or_zero(lev).clamp(0.0, MAX_LEVERAGE)
}

/// Earnings a firm can use to service debt (after interest): profit rate × capital for a mature firm, 50% of
/// annualised revenue for a firm under 12 months old.
fn firm_earnings(w: &World, f: FirmId) -> f64 {
    let fm = &w.firms[f as usize];
    if fm.age_months < 12 {
        let revenue = fm.revenue.max(fm.expected_demand.max(0.0) * fm.price.max(0.0));
        0.5 * 12.0 * revenue
    } else {
        fm.profit_rate * fm.capital.max(0.0) * common::capital_price_world(w)
    }
}

/// Interest coverage on existing debt: (earnings + interest) / interest, capped at MAX_COVERAGE.
fn firm_coverage(w: &World, f: FirmId) -> f64 {
    let interest = firm_interest(w, f);
    if interest <= 1e-9 {
        return MAX_COVERAGE;
    }
    finite_or_zero((firm_earnings(w, f) + interest) / interest).clamp(0.1, MAX_COVERAGE)
}

/// Annual default probability: logistic(a + b·leverage + c·ln(1/coverage)).
pub fn firm_pd(cfg: &Config, lev: f64, cov: f64) -> f64 {
    let z = cfg.pd_intercept + cfg.pd_leverage * lev - cfg.pd_coverage * cov.max(1e-6).ln();
    finite_or_zero(1.0 / (1.0 + (-z).exp()))
}

/// Expected annual loss rate on each bank's business loans: loan-weighted LGD × PD of its current borrowers.
/// Forward-looking (it moves as borrowers' earnings and leverage deteriorate), like the default expectations priced
/// in corporate spreads (Gilchrist & Zakrajšek 2012), rather than trailing realised write-offs.
pub fn expected_loss_rates(w: &World) -> Vec<f64> {
    let nb = w.banks.len();
    let (mut el, mut debt) = (vec![0.0; nb], vec![0.0; nb]);
    for (f, fm) in w.firms.iter().enumerate() {
        if !fm.alive || fm.loans.is_empty() {
            continue;
        }
        let d = firm_debt(w, f as FirmId);
        if !(d > 0.0) {
            continue;
        }
        let lev = leverage(d, firm_assets(w, f as FirmId));
        let pd = firm_pd(&w.cfg, lev, firm_coverage(w, f as FirmId));
        let b = fm.bank as usize;
        if b < nb {
            el[b] += d * w.cfg.pd_lgd * pd;
            debt[b] += d;
        }
    }
    el.iter().zip(&debt).map(|(&e, &d)| if d > 1e-9 { finite_or_zero(e / d) } else { 0.0 }).collect()
}

/// Default-risk premium for a firm (§11.1): loss-given-default × (annual PD − PD of the reference firm), with
/// PD = logistic(a + b·leverage + c·ln(1/coverage)). Convex in leverage and in weak coverage.
pub fn default_premium(cfg: &Config, lev: f64, cov: f64) -> f64 {
    let p = firm_pd(cfg, lev, cov);
    let p_ref = firm_pd(cfg, PD_REF_LEVERAGE, PD_REF_COVERAGE);
    finite_or_zero(cfg.pd_lgd * (p - p_ref)).clamp(PD_PREMIUM_MIN, PD_PREMIUM_MAX)
}

/// Bank-side tightening of credit standards (SLOOS, §11.2): (maximum post-loan leverage, minimum interest coverage,
/// maximum rollover leverage) for a bank whose premium is `ebp`. Positive premia above zero tighten standards.
fn standards(w: &World, bank: BankId) -> (f64, f64, f64) {
    let b = &w.banks[bank as usize];
    let pp = 100.0 * b.ebp.max(0.0);
    let lev = MAX_FIRM_LEVERAGE - w.cfg.standards_leverage_per_pp * pp;
    let cov = MIN_INTEREST_COVERAGE * (1.0 + w.cfg.standards_coverage_per_pp * pp);
    let roll = MAX_ROLLOVER_LEVERAGE - w.cfg.standards_leverage_per_pp * pp;
    (lev, cov, roll)
}

/// Interest rate a firm would pay on a new business loan: bank base + default-risk premium (§11.1).
pub fn business_rate(w: &World, firm: FirmId) -> f64 {
    let base = w.banks[w.firms[firm as usize].bank as usize].business_rate_base;
    base + firm_risk_premium(w, firm)
}

/// Firm-specific default-risk premium over the bank base rate, and the firm's leverage.
fn firm_risk_terms(w: &World, firm: FirmId) -> (f64, f64) {
    let lev = leverage(firm_debt(w, firm), firm_assets(w, firm));
    (default_premium(&w.cfg, lev, firm_coverage(w, firm)), lev)
}

fn firm_risk_premium(w: &World, firm: FirmId) -> f64 {
    firm_risk_terms(w, firm).0
}

/// Ask the firm's bank for a business loan. Returns the loan id if approved (deposits are credited).
/// Approval requires post-loan leverage ≤ 0.75, interest coverage ≥ 1.5 and a post-loan bank capital
/// ratio ≥ requirement + 1% (§11.2). Coverage uses (profit + current interest) / (total interest after the
/// loan); firms under 12 months old use 50% of annualised revenue as the earnings proxy.
pub fn request_business_loan(w: &mut World, firm: FirmId, amount: f64) -> Option<LoanId> {
    if !(amount > 0.0) || !amount.is_finite() {
        return None;
    }
    let fm = &w.firms[firm as usize];
    if !fm.alive {
        return None;
    }
    let bank_id = fm.bank;
    let bank = &w.banks[bank_id as usize];
    if !bank.alive {
        return None;
    }
    let rate = business_rate(w, firm);

    // Credit standards tighten with the bank's premium (SLOOS): lower maximum leverage, higher minimum coverage.
    let (max_lev, min_cov, _) = standards(w, bank_id);

    // Leverage after the loan (the loan adds equal debt and deposits).
    let debt = firm_debt(w, firm);
    let assets = firm_assets(w, firm);
    let post_assets = assets + amount;
    if post_assets <= 1e-9 || (debt + amount) / post_assets > max_lev {
        return None;
    }

    // Interest coverage.
    let existing_interest = firm_interest(w, firm);
    let total_interest = existing_interest + amount * rate;
    let earnings = firm_earnings(w, firm);
    if total_interest > 1e-9 {
        let coverage = (earnings + existing_interest) / total_interest;
        if !(coverage >= min_cov) {
            return None;
        }
    }

    // Bank capital after the loan: equity unchanged, RWA rises by the loan.
    let bank = &w.banks[bank_id as usize];
    let post_rwa = rwa(bank.mortgage_loans, bank.business_loans + amount);
    if !(bank.equity() / post_rwa >= w.cfg.capital_requirement + BUSINESS_LOAN_BUFFER) {
        return None;
    }

    let id = ledger::create_loan(
        w,
        bank_id,
        Borrower::Firm(firm),
        LoanKind::Business,
        amount,
        rate,
        BUSINESS_TERM_MONTHS,
    );
    w.flows.new_business_loans += amount;
    Some(id)
}

// ---------------------------------------------------------------- mortgages

/// Sum of monthly scheduled mortgage payments on the houses household `h` owns (§11.3, used by §16 housing).
pub fn household_mortgage_payments(w: &World, h: HhId) -> f64 {
    let Some(hh) = w.households.get(h as usize) else {
        return 0.0;
    };
    hh.properties
        .iter()
        .filter_map(|&p| w.houses.get(p as usize).and_then(|hs| hs.mortgage))
        .filter_map(|l| w.loans.get(l))
        .filter(|l| l.borrower == Borrower::Household(h))
        .map(|l| finite_or_zero(l.payment).max(0.0))
        .sum()
}

/// Scheduled monthly mortgage payments of a household, given its record (round 7 step 4: consumption reads it
/// without indexing the household list). Sums the payments of household-borrowed mortgages on its properties.
pub fn mortgage_payments_of(w: &World, hh: &Household) -> f64 {
    let h = hh.id;
    hh.properties
        .iter()
        .filter_map(|&p| w.houses.get(p as usize).and_then(|hs| hs.mortgage))
        .filter_map(|l| w.loans.get(l))
        .filter(|l| l.borrower == Borrower::Household(h))
        .map(|l| finite_or_zero(l.payment).max(0.0))
        .sum()
}

/// Monthly rent a household receives from tenants (current house rents, or this month's rent income if larger).
fn rental_income(w: &World, h: HhId) -> f64 {
    let hh = &w.households[h as usize];
    let let_out: f64 = hh
        .properties
        .iter()
        .filter_map(|&p| w.houses.get(p as usize))
        .filter(|hs| matches!(hs.occupant, Some(o) if o != h))
        .map(|hs| finite_or_zero(hs.rent).max(0.0))
        .sum();
    let_out.max(w.hh_month[h as usize].income.rent_received.max(0.0))
}

/// Quote from one specific bank, or None if the bank is closed or below the capital requirement (§11.2).
///
/// `replaced` is an existing mortgage being refinanced: its payment is left out of the household's DTI test.
fn quote_at(
    w: &World,
    h: HhId,
    price: f64,
    investor: bool,
    bank_id: BankId,
    replaced: Option<LoanId>,
) -> Option<MortgageQuote> {
    let bank = w.banks.get(bank_id as usize)?;
    if !bank.alive || capital_ratio(w, bank_id) < w.cfg.capital_requirement {
        return None;
    }
    let mut rate = bank.mortgage_rate;
    if investor {
        rate += INVESTOR_RATE_ADD;
    }
    if !rate.is_finite() {
        return None;
    }

    // Borrowing limits: macroprudential shock overrides config; investors face a tighter LTV cap (§11.2).
    let (mut max_ltv, max_dti) = w.shocks.mortgage_limits(w.cfg.max_ltv, w.cfg.max_dti);
    if investor {
        max_ltv = max_ltv.min(w.cfg.max_ltv_investor);
    }

    let hh = &w.households[h as usize];
    // Integration (fix-fiscal): pension income counts whatever the labour status (early claimants are not yet Retired).
    let wage_income = match hh.status {
        LaborStatus::Employed => hh.wage + hh.pension.max(0.0),
        _ => hh.pension.max(0.0),
    };
    let rent = if investor {
        INVESTOR_RENT_SHARE * rental_income(w, h)
    } else {
        0.0
    };
    let gross = finite_or_zero(wage_income.max(0.0) + rent);
    let property_tax = w.cfg.property_tax / 12.0 * price;
    // fix-housing: a refinance replaces its own loan, so that payment does not count against the new one.
    let replaced_payment = replaced
        .and_then(|l| w.loans.get(l))
        .filter(|l| l.borrower == Borrower::Household(h))
        .map_or(0.0, |l| finite_or_zero(l.payment).max(0.0));
    let max_payment =
        max_dti * gross - (household_mortgage_payments(w, h) - replaced_payment).max(0.0) - property_tax;
    if !(max_payment > 0.0) {
        return None;
    }
    let by_income = annuity_principal(max_payment, rate, w.cfg.mortgage_term_months);
    let max_loan = (max_ltv * price).min(by_income);
    if !(max_loan > 0.0) || !max_loan.is_finite() {
        return None;
    }
    Some(MortgageQuote {
        bank: bank_id,
        rate,
        max_loan,
    })
}

/// Quote the best mortgage for household `h` buying at `price` (investor = buy-to-let rules).
/// Tries the household's own bank first, then the best-capitalised bank.
pub fn mortgage_quote(w: &World, h: HhId, price: f64, investor: bool) -> Option<MortgageQuote> {
    mortgage_quote_replacing(w, h, price, investor, None)
}

/// Quote for household `h` refinancing its loan `replaced` on a house worth `price` (owner rules; the old payment is
/// excluded from the DTI test). Same bank order as `mortgage_quote`.
// fix-housing
pub fn mortgage_quote_refi(w: &World, h: HhId, price: f64, replaced: LoanId) -> Option<MortgageQuote> {
    mortgage_quote_replacing(w, h, price, false, Some(replaced))
}

fn mortgage_quote_replacing(
    w: &World,
    h: HhId,
    price: f64,
    investor: bool,
    replaced: Option<LoanId>,
) -> Option<MortgageQuote> {
    let hh = w.households.get(h as usize)?;
    if !hh.alive || !(price > 0.0) || !price.is_finite() {
        return None;
    }
    if let Some(q) = quote_at(w, h, price, investor, hh.bank, replaced) {
        return Some(q);
    }
    let best = best_capitalised_bank(w)?;
    if best == hh.bank {
        return None;
    }
    quote_at(w, h, price, investor, best, replaced)
}

/// Originate a mortgage on `house` for household `h` from `quote.bank`. Returns the loan id.
/// Re-checks the principal against the quote and the bank's post-loan capital ratio (≥ requirement).
pub fn originate_mortgage(
    w: &mut World,
    h: HhId,
    house: HouseId,
    principal: f64,
    quote: MortgageQuote,
) -> Option<LoanId> {
    if !(principal > 0.0) || !principal.is_finite() || principal > quote.max_loan + 1e-6 {
        return None;
    }
    if !w.households.get(h as usize)?.alive {
        return None;
    }
    if w.houses.get(house as usize)?.mortgage.is_some() {
        return None;
    }
    let bank = w.banks.get(quote.bank as usize)?;
    if !bank.alive {
        return None;
    }
    let post_rwa = rwa(bank.mortgage_loans + principal, bank.business_loans);
    if !(bank.equity() / post_rwa >= w.cfg.capital_requirement) {
        return None;
    }
    let term = w.cfg.mortgage_term_months;
    Some(ledger::create_loan(
        w,
        quote.bank,
        Borrower::Household(h),
        LoanKind::Mortgage { house },
        principal,
        quote.rate,
        term,
    ))
}

// ---------------------------------------------------------------- servicing

#[inline]
fn borrower_deposits(w: &World, b: Borrower) -> f64 {
    match b {
        Borrower::Household(h) => w.households[h as usize].deposits,
        Borrower::Firm(f) => w.firms[f as usize].deposits,
    }
}

#[inline]
fn borrower_alive(w: &World, b: Borrower) -> bool {
    match b {
        Borrower::Household(h) => w.households[h as usize].alive,
        Borrower::Firm(f) => w.firms[f as usize].alive,
    }
}

/// Read-only facts gathered in parallel before the loan loop.
#[derive(Clone, Copy)]
struct LoanGather {
    /// Scheduled payment (non-business loans; 0 otherwise).
    payment: f64,
    /// Household borrower's (alive, deposits) before this phase.
    hh: Option<(bool, f64)>,
}

/// The fast-path settlement of one loan, if it qualifies (see `service_loans`): (borrower, interest, principal,
/// scheduled payment), all paid in full.
/// Post-anchor income-shock miss (fix-housing review 7, fix 2; graded in review 8, deposits only since review 12). An
/// unemployed owner on a high-LTV mortgage may miss a payment. The monthly probability is `housing_income_miss_prob` when
/// liquidity (deposits only, in months of payments) is at or below `housing_shock_full_miss_months`, falls linearly, and
/// is zero at or above `housing_shock_liquidity_months`. With the shipped default (grading off) the probability is the
/// full value for every eligible owner, so this is an LTV-gated payment shock, not an illiquidity gradient. The
/// delinquency then runs through the post-anchor paths in housing.rs. Before the anchor nothing changes.
fn income_shock_miss_prob(w: &World, id: LoanId) -> f64 {
    if !w.housing.anchored || w.cfg.housing_income_miss_prob <= 0.0 {
        return 0.0;
    }
    let Some(l) = w.loans.get(id) else { return 0.0 };
    let (LoanKind::Mortgage { house }, Borrower::Household(h)) = (l.kind, l.borrower) else { return 0.0 };
    let hh = &w.households[h as usize];
    if !hh.alive || w.houses[house as usize].owner != Owner::Household(h) {
        return 0.0;
    }
    // An unemployed owner, or an owner in an income-shock episode (housing.rs `update_income_shocks`), may miss.
    let episode = crate::housing::income_shock_active(w, house, h);
    if !episode && hh.status != LaborStatus::Unemployed {
        return 0.0;
    }
    let v = crate::housing::house_value_i(w, house);
    if !(v > 0.0) || !(l.principal / v > w.cfg.housing_shock_ltv) {
        return 0.0;
    }
    // An employed owner in an episode misses with the episode probability. It is graded by liquidity like the unemployed
    // miss only when grading is on (config `housing_shock_liquidity_months`); by default it is not graded.
    let base = if episode && hh.status != LaborStatus::Unemployed {
        w.cfg.housing_shock_episode_miss_prob
    } else {
        w.cfg.housing_income_miss_prob
    };
    let payment = if l.payment.is_finite() { l.payment.max(0.0) } else { 0.0 };
    // Liquidity is deposits only: Fund shares are a long-run savings vehicle, not a payment buffer a job loss draws on.
    let weight = shock_miss_liquidity_weight(hh.deposits, payment, &w.cfg);
    base * weight
}

/// Liquidity weight of the income-shock miss, in [0, 1]: 1 below `housing_shock_full_miss_months` of payments, 0 at
/// or above `housing_shock_liquidity_months`, linear in between. With no payment due, any liquidity is ample.
fn shock_miss_liquidity_weight(liquid: f64, payment: f64, cfg: &Config) -> f64 {
    let (full, zero) = (cfg.housing_shock_full_miss_months, cfg.housing_shock_liquidity_months);
    if !(payment > 0.0) {
        return 0.0;
    }
    let months = liquid.max(0.0) / payment;
    if zero <= full {
        return if months < zero { 1.0 } else { 0.0 };
    }
    ((zero - months) / (zero - full)).clamp(0.0, 1.0)
}

fn fast_mortgage_payment(w: &World, id: LoanId, g: &LoanGather, n_loans: &[u8]) -> Option<(HhId, f64, f64, f64)> {
    let l = w.loans.get(id)?;
    let Borrower::Household(h) = l.borrower else { return None };
    if !matches!(l.kind, LoanKind::Mortgage { .. }) || n_loans[h as usize] != 1 || l.months_remaining <= 1 {
        return None;
    }
    // Loans that may take an income-shock miss go through the general path, which draws the miss.
    if income_shock_miss_prob(w, id) > 0.0 {
        return None;
    }
    let (alive, deposits) = g.hh?;
    let principal = l.principal;
    if !alive || !(principal > 0.0) {
        return None;
    }
    let rate = finite_or_zero(l.annual_rate).max(0.0);
    let payment = g.payment;
    let interest = principal * rate / 12.0;
    let sched = (payment - interest).clamp(0.0, principal);
    let due = interest + sched;
    if !(interest >= 0.0) || !due.is_finite() || principal - sched <= 1e-6 || !(deposits.max(0.0) >= due) {
        return None;
    }
    Some((h, interest, sched, payment))
}

fn gather_loan(w: &World, id: LoanId) -> LoanGather {
    let Some(l) = w.loans.get(id) else {
        return LoanGather { payment: 0.0, hh: None };
    };
    let payment = if l.kind != LoanKind::Business {
        let rate = finite_or_zero(l.annual_rate).max(0.0);
        finite_or_zero(annuity_payment(l.principal, rate, l.months_remaining))
    } else {
        0.0
    };
    let hh = match l.borrower {
        Borrower::Household(h) => {
            let hh = &w.households[h as usize];
            Some((hh.alive, hh.deposits))
        }
        Borrower::Firm(_) => None,
    };
    LoanGather { payment, hh }
}

/// Scheduled P&I for every loan (§11.3). Household borrowers short of cash sell Fund shares first; any
/// remaining shortfall is paid interest-first and counted as delinquency. Unpaid interest is not capitalised. With
/// `housing_reo_nonaccrual` a repossession's loan (`housing::is_reo_loan`) is left untouched until the house sells.
///
/// The read-only facts (scheduled payment, household borrower's alive flag and deposits) are gathered in parallel
/// up front. A household's gathered values are used only until one of its loans has been processed: no other loan
/// or payment changes another household's deposits, alive flag or shares within this phase, so those values equal
/// what a live read would return. After a household's first loan, all reads are live.
fn service_loans(w: &mut World) {
    // Collect ids first: repayments can remove loans.
    let ids = w.loans.keys();
    let gathered: Vec<LoanGather> = {
        let wr: &World = w;
        ids.par_iter().map(|&id| gather_loan(wr, id)).collect()
    };
    // `housing_reo_nonaccrual`: a repossession's loan (the bank owns the house until it sells) is a non-accruing asset. It
    // is skipped by every path below: no payment, shortfall, miss or Fund sale, and it is not counted as the household's
    // loan. Off: no loan is skipped (the release).
    let reo: Vec<bool> = if w.cfg.housing_reo_nonaccrual {
        let wr: &World = w;
        ids.par_iter()
            .map(|&id| wr.loans.get(id).is_some_and(|l| crate::housing::is_reo_loan(&wr.housing, id, l.kind)))
            .collect()
    } else {
        vec![false; ids.len()]
    };
    // Fast path: a household's only loan, an amortising mortgage that is not in its final month and that the
    // borrower can pay in full from deposits. Such a payment cannot trigger a Fund sale, a shortfall or a payoff,
    // and no other loan touches the household, so it is settled with the balance updates `ledger::pay_interest` and
    // `ledger::repay_principal` would make: loans and lender banks in loan order, borrower debits in parallel.
    let nh = w.households.len();
    let mut n_loans = vec![0u8; nh];
    for (k, &id) in ids.iter().enumerate() {
        if reo[k] {
            continue;
        }
        if let Some(Loan { borrower: Borrower::Household(h), .. }) = w.loans.get(id) {
            n_loans[*h as usize] = n_loans[*h as usize].saturating_add(1);
        }
    }
    let fast: Vec<Option<(HhId, f64, f64, f64)>> = {
        let wr: &World = w;
        ids.par_iter()
            .zip(gathered.par_iter())
            .zip(reo.par_iter())
            .map(|((&id, g), &skip)| if skip { None } else { fast_mortgage_payment(wr, id, g, &n_loans) })
            .collect()
    };
    let mut debit = vec![0.0; nh];
    for (k, &id) in ids.iter().enumerate() {
        let Some((h, interest, principal_paid, payment)) = fast[k] else { continue };
        let Some(l) = w.loans.get_mut(id) else { continue };
        l.payment = payment;
        l.principal -= principal_paid;
        l.months_delinquent = 0;
        l.months_remaining = l.months_remaining.saturating_sub(1);
        let bank = &mut w.banks[l.bank as usize];
        bank.reserves += interest;
        bank.interest_income += interest;
        bank.reserves += principal_paid;
        bank.loans -= principal_paid;
        bank.mortgage_loans -= principal_paid;
        debit[h as usize] = interest + principal_paid;
    }
    let nb = w.banks.len();
    let parts: Vec<Vec<f64>> = w
        .households
        .par_chunks_mut(BANK_CHUNK)
        .zip(debit.par_chunks(BANK_CHUNK))
        .map(|(hhs, ds)| {
            let mut bank = vec![0.0; nb];
            for (h, &d) in hhs.iter_mut().zip(ds) {
                if d > 0.0 {
                    h.deposits -= d;
                    bank[h.bank as usize] -= d;
                }
            }
            bank
        })
        .collect();
    for bank in &parts {
        for (b, &x) in bank.iter().enumerate() {
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
    }

    // Each firm's loan rate and leverage, once per month at the start of servicing (computing them per loan walks
    // all of the firm's loans, which is quadratic in loans per firm). Within the month a firm's debt changes only
    // when one of its loans matures unrolled.
    let firm_terms: Vec<(f64, f64)> = {
        let wr: &World = w;
        (0..wr.firms.len())
            .into_par_iter()
            .map(|f| {
                let f = f as FirmId;
                if wr.firms[f as usize].loans.is_empty() {
                    return (0.0, 0.0);
                }
                let base = wr.banks[wr.firms[f as usize].bank as usize].business_rate_base;
                let (prem, lev) = firm_risk_terms(wr, f);
                (base + prem, lev)
            })
            .collect()
    };
    // Fast path for business credit: a live firm whose deposits cover the interest on all its non-maturing loans
    // pays that interest in full (interest-only, so no principal moves and nothing is removed). Those loans are
    // settled with the updates `ledger::pay_interest` would make; maturing loans and cash-short firms take the
    // general path below.
    let firm_ok: Vec<bool> = {
        let wr: &World = w;
        (0..wr.firms.len())
            .into_par_iter()
            .map(|f| {
                let fm = &wr.firms[f];
                if !fm.alive || fm.loans.is_empty() {
                    return false;
                }
                let rate = finite_or_zero(firm_terms[f].0).max(0.0);
                let due: f64 = fm
                    .loans
                    .iter()
                    .filter_map(|&l| wr.loans.get(l))
                    .filter(|l| l.kind == LoanKind::Business && l.months_remaining > 1 && l.principal > 0.0)
                    .map(|l| l.principal * rate / 12.0)
                    .sum();
                due.is_finite() && due <= fm.deposits.max(0.0)
            })
            .collect()
    };
    let mut biz_fast = vec![false; ids.len()];
    let mut firm_debit = vec![0.0; w.firms.len()];
    for (k, &id) in ids.iter().enumerate() {
        if fast[k].is_some() {
            continue;
        }
        let Some(l) = w.loans.get_mut(id) else { continue };
        let Borrower::Firm(f) = l.borrower else { continue };
        let fu = f as usize;
        if l.kind != LoanKind::Business || !firm_ok[fu] || l.months_remaining <= 1 || !(l.principal > 0.0) {
            continue;
        }
        let rate = finite_or_zero(firm_terms[fu].0).max(0.0);
        let interest = l.principal * rate / 12.0;
        l.annual_rate = firm_terms[fu].0;
        l.payment = interest;
        l.months_delinquent = 0;
        l.months_remaining -= 1;
        if interest > 0.0 {
            let bank = &mut w.banks[l.bank as usize];
            bank.reserves += interest;
            bank.interest_income += interest;
            firm_debit[fu] += interest;
        }
        biz_fast[k] = true;
    }
    for (f, &d) in firm_debit.iter().enumerate() {
        if d > 0.0 {
            let fm = &mut w.firms[f];
            fm.deposits -= d;
            fm.interest_paid += d;
            let b = &mut w.banks[fm.bank as usize];
            b.deposits -= d;
            b.reserves -= d;
        }
    }

    let mut touched = vec![false; w.households.len()];
    for (k, &id) in ids.iter().enumerate() {
        if fast[k].is_some() || biz_fast[k] || reo[k] {
            continue;
        }
        let g = gathered[k];
        let Some(l) = w.loans.get(id) else { continue };
        let (borrower, principal, mut months_rem, kind, delinquent) =
            (l.borrower, l.principal, l.months_remaining, l.kind, l.months_delinquent);
        // Gathered household state, if this household has not been touched yet in this phase.
        let fresh = match borrower {
            Borrower::Household(h) if !touched[h as usize] => g.hh,
            _ => None,
        };
        let alive = match fresh {
            Some((alive, _)) => alive,
            None => borrower_alive(w, borrower),
        };
        if !(principal > 0.0) || !alive {
            continue;
        }
        // Business credit (US C&I loans and corporate bonds) is floating-rate and rolled over rather than
        // amortised: interest-only, repriced monthly off the curve, and refinanced at maturity while the firm is
        // in good standing — otherwise steady amortisation would destroy money every month.
        let business = kind == LoanKind::Business;
        if let (true, Borrower::Firm(f)) = (business, borrower) {
            let (r, lev) = firm_terms[f as usize];
            // Rollover is refused when the lender's standards have tightened below the firm's leverage.
            let lbank = w.loans.get(id).map_or(0, |l| l.bank);
            let (_, _, roll_max) = standards(w, lbank);
            if months_rem <= 1 && delinquent == 0 && lev <= roll_max {
                months_rem = BUSINESS_TERM_MONTHS;
                if let Some(l) = w.loans.get_mut(id) {
                    l.months_remaining = months_rem;
                }
            }
            if let Some(l) = w.loans.get_mut(id) {
                l.annual_rate = r;
            }
        }
        let rate = finite_or_zero(w.loans.get(id).map_or(0.0, |l| l.annual_rate)).max(0.0);
        // Scheduled payment on the current balance; the final month clears the balance.
        let payment = if business && months_rem > 1 {
            principal * rate / 12.0
        } else if !business {
            g.payment
        } else {
            finite_or_zero(annuity_payment(principal, rate, months_rem))
        };
        let interest = principal * rate / 12.0;
        let sched_principal = if months_rem <= 1 {
            principal
        } else {
            (payment - interest).clamp(0.0, principal)
        };
        let due = interest + sched_principal;
        if let Some(l) = w.loans.get_mut(id) {
            l.payment = payment;
        }

        // Income-shock miss (post-anchor, see `income_shock_miss_prob`): the payment is not made, no Fund shares are
        // sold and no cash moves, so the loan is delinquent for the month.
        let miss_prob = match borrower {
            Borrower::Household(_) => income_shock_miss_prob(w, id),
            _ => 0.0,
        };
        let missed = miss_prob > 0.0 && {
            let mut r = rng::agent_rng(w.seed, w.tick, rng::stream::BANKING, (PH_SHOCK_MISS << 40) | id as u64);
            r.gen::<f64>() < miss_prob
        };

        // Households raise cash from Fund shares before missing a payment (§16).
        if let (Borrower::Household(h), false) = (borrower, missed) {
            let deposits = match fresh {
                Some((_, d)) => d,
                None => borrower_deposits(w, borrower),
            };
            let short = due - deposits.max(0.0);
            if short > 0.0 {
                ledger::fund_sell(w, h, short);
            }
        }

        let (paid_int, paid_prin) = if missed {
            (0.0, 0.0)
        } else {
            let mut cash = borrower_deposits(w, borrower).max(0.0);
            let paid_int = interest.min(cash).max(0.0);
            ledger::pay_interest(w, id, paid_int);
            cash -= paid_int;
            if let Borrower::Firm(f) = borrower {
                w.firms[f as usize].interest_paid += paid_int;
            }
            let paid_prin = ledger::repay_principal(w, id, sched_principal.min(cash).max(0.0));
            (paid_int, paid_prin)
        };
        if let Borrower::Household(h) = borrower {
            touched[h as usize] = true;
        }

        let tol = 1e-9 * (1.0 + due);
        let short = missed || paid_int < interest - tol || paid_prin < sched_principal - tol;
        // A fully repaid loan has been removed by the ledger; nothing more to update.
        if let Some(l) = w.loans.get_mut(id) {
            if short {
                l.months_delinquent += 1;
            } else {
                l.months_delinquent = 0;
                l.months_remaining = months_rem.saturating_sub(1);
            }
        }
    }
}

/// Overdraft charge on a depositor with balance `dep` (None when there is no charge).
#[inline]
fn overdraft_charge(dep: f64, rate: f64) -> Option<f64> {
    let od = -dep * rate;
    if od > 0.0 && od.is_finite() {
        Some(od)
    } else {
        None
    }
}

/// Deposit interest amount `deposits × bank.deposit_rate / 12` (None when not payable).
#[inline]
fn deposit_interest_amount(deposits: f64, deposit_rate: f64) -> Option<f64> {
    let amt = deposits * deposit_rate / 12.0;
    if amt > 0.0 && amt.is_finite() {
        Some(amt)
    } else {
        None
    }
}

/// Pay `deposits × bank.deposit_rate / 12` from the depositor's bank to the depositor. Returns the amount paid.
fn pay_deposit_interest(w: &mut World, depositor: Account, bank: BankId, deposits: f64) -> f64 {
    let Some(amt) = deposit_interest_amount(deposits, w.banks[bank as usize].deposit_rate) else {
        return 0.0;
    };
    ledger::pay(w, Account::Bank(bank), depositor, amt);
    w.banks[bank as usize].interest_expense += amt;
    amt
}

/// Overdrawn depositors (households, firms) pay (policy + credit-card spread) on the overdraft (§11.3), and
/// positive deposits earn monthly interest (households, firms, the Fund and RoW; §11.3).
///
/// One sequential pass over households and one over firms, each account touched only through the ledger. Every
/// household (firm) is either overdrawn or in credit, so the two charges never meet on one account; the bank
/// totals they touch are order-independent sums. Returns the wage bill per bank (alive, employed households),
/// which is unchanged by this phase and is used by the operating-expense charge.
fn service_deposits(w: &mut World) -> Vec<f64> {
    let od_rate = (finite_or_zero(w.cb.policy_rate) + w.cfg.credit_card_spread).max(0.0) / 12.0;
    let nb = w.banks.len();
    // One parallel pass over fixed household chunks. Each household is charged or paid at most once and only its
    // own record changes, so the amounts are applied in place. The bank side of each payment (`ledger::pay`
    // between a household and its own bank) nets to a change in the bank's deposit liability with no net reserve
    // movement; those, the interest income/expense and the wage bill are reduced per chunk in chunk order.
    let rates: Vec<f64> = w.banks.iter().map(|b| b.deposit_rate).collect();
    let parts: Vec<[Vec<f64>; 4]> = w
        .households
        .par_chunks_mut(BANK_CHUNK)
        .zip(w.hh_month.par_chunks_mut(BANK_CHUNK))
        .map(|(chunk, mchunk)| {
            let mut dep = vec![0.0; nb];
            let mut inc = vec![0.0; nb];
            let mut exp = vec![0.0; nb];
            let mut ws = vec![0.0; nb];
            for (h, m) in chunk.iter_mut().zip(mchunk.iter_mut()) {
                let b = h.bank as usize;
                if h.alive && h.status == LaborStatus::Employed {
                    ws[b] += h.wage.max(0.0);
                }
                if !h.alive {
                    continue;
                }
                if h.deposits < 0.0 {
                    if let Some(od) = overdraft_charge(h.deposits, od_rate) {
                        h.deposits -= od;
                        dep[b] -= od;
                        inc[b] += od;
                    }
                } else if h.deposits > 0.0 {
                    if let Some(amt) = deposit_interest_amount(h.deposits, rates[b]) {
                        h.deposits += amt;
                        m.income.interest += amt;
                        dep[b] += amt;
                        exp[b] += amt;
                    }
                }
            }
            [dep, inc, exp, ws]
        })
        .collect();
    let mut wage_sum = vec![0.0; nb];
    for [dep, inc, exp, ws] in &parts {
        for b in 0..nb {
            let bank = &mut w.banks[b];
            bank.deposits += dep[b];
            bank.interest_income += inc[b];
            bank.interest_expense += exp[b];
            wage_sum[b] += ws[b];
        }
    }
    for i in 0..w.firms.len() {
        let (alive, dep, b) = {
            let f = &w.firms[i];
            (f.alive, f.deposits, f.bank)
        };
        if !alive {
            continue;
        }
        if dep < 0.0 {
            if let Some(od) = overdraft_charge(dep, od_rate) {
                ledger::pay(w, Account::Firm(i as FirmId), Account::Bank(b), od);
                w.banks[b as usize].interest_income += od;
                w.firms[i].interest_paid += od;
            }
        } else if dep > 0.0 {
            pay_deposit_interest(w, Account::Firm(i as FirmId), b, dep);
        }
    }
    let (dep, b) = (w.fund.deposits, w.fund.bank);
    if dep > 0.0 {
        pay_deposit_interest(w, Account::Fund, b, dep);
    }
    let (dep, b) = (w.row.deposits, w.row.bank);
    if dep > 0.0 {
        let paid = pay_deposit_interest(w, Account::RestOfWorld, b, dep);
        crate::world::note_row_dep_interest(w, paid);
    }
    wage_sum
}

/// Households per chunk in the parallel read passes (fixed, so results do not depend on the thread count).
const BANK_CHUNK: usize = 4096;

/// Interest on reserves at the policy rate (paid by the CB) and discount-window interest at policy + spread.
fn service_reserves_and_discount(w: &mut World) {
    let policy = finite_or_zero(w.cb.policy_rate).max(0.0);
    let iorb = policy / 12.0;
    let discount = (policy + w.cfg.discount_spread).max(0.0) / 12.0;
    for b in 0..w.banks.len() {
        if !w.banks[b].alive {
            continue;
        }
        let (res, cbb) = (w.banks[b].reserves, w.banks[b].cb_borrowing);
        if res > 0.0 {
            let amt = res * iorb;
            if amt > 0.0 && amt.is_finite() {
                ledger::pay(w, Account::CentralBank, Account::Bank(b as BankId), amt);
                w.banks[b].interest_income += amt;
            }
        }
        if cbb > 0.0 {
            let amt = cbb * discount;
            if amt > 0.0 && amt.is_finite() {
                ledger::pay(w, Account::Bank(b as BankId), Account::CentralBank, amt);
                w.banks[b].interest_expense += amt;
            }
        }
    }
}

/// Discount window (§11.3): draw when reserves are negative (shortfall + 1% of deposits); repay down to
/// the 5%-of-deposits reserve level when reserves are above it.
fn service_liquidity(w: &mut World) {
    for b in 0..w.banks.len() {
        if !w.banks[b].alive {
            continue;
        }
        let dep = w.banks[b].deposits.max(0.0);
        let (res, cbb) = (w.banks[b].reserves, w.banks[b].cb_borrowing);
        if res < 0.0 {
            ledger::cb_lend(w, b as BankId, -res + LIQUIDITY_BUFFER_SHARE * dep);
        } else if cbb > 0.0 && res > RESERVE_TARGET_SHARE * dep {
            let excess = res - RESERVE_TARGET_SHARE * dep;
            ledger::cb_repay(w, b as BankId, excess.min(cbb));
        }
    }
}

/// Bank P&L and dividends (§11.3): profit = interest income − interest expense − loan losses; if profitable and
/// the capital ratio exceeds requirement + 2%, pay `bank_payout_ratio × profit` to the Fund (capped at reserves).
/// Management capital-ratio buffer above the requirement, and the monthly payout share of capital above it.
const CAPITAL_TARGET_BUFFER: f64 = 0.05;
const EXCESS_CAPITAL_PAYOUT: f64 = 0.01;

/// Operating expenses (efficiency ratio ≈ 60% of net interest income; mostly compensation): paid as compensation to
/// the bank's employed depositors in proportion to their wages (a stand-in for financial-sector employees).
/// `wage_sum` is the bank's wage bill over alive, employed households (see `service_deposits`).
fn pay_operating_expenses(w: &mut World, wage_sum: &[f64]) -> Vec<f64> {
    let nb = w.banks.len();
    let mut opex = vec![0.0; nb];
    for (b, bank) in w.banks.iter().enumerate() {
        if bank.alive {
            opex[b] = BANK_OPEX_SHARE * finite_or_zero(bank.interest_income - bank.interest_expense).max(0.0);
        }
    }
    // Bank-to-household compensation, applied in place per fixed chunk (each household paid at most once); the
    // bank side nets to a rise in its deposit liability (`ledger::pay` from a bank to its own depositor).
    let parts: Vec<Vec<f64>> = w
        .households
        .par_chunks_mut(BANK_CHUNK)
        .zip(w.hh_month.par_chunks_mut(BANK_CHUNK))
        .map(|(chunk, mchunk)| {
            let mut dep = vec![0.0; nb];
            for (h, m) in chunk.iter_mut().zip(mchunk.iter_mut()) {
                let bank = h.bank as usize;
                if !h.alive || h.status != LaborStatus::Employed || wage_sum[bank] <= 0.0 || opex[bank] <= 0.0 {
                    continue;
                }
                let x = opex[bank] * h.wage.max(0.0) / wage_sum[bank];
                if x > 0.0 && x.is_finite() {
                    h.deposits += x;
                    m.income.wage += x;
                    dep[bank] += x;
                }
            }
            dep
        })
        .collect();
    for dep in &parts {
        for (b, x) in dep.iter().enumerate() {
            w.banks[b].deposits += x;
        }
    }
    opex
}

/// Bank non-interest expenses as a share of net interest income (US efficiency ratio ≈ 55–65%).
const BANK_OPEX_SHARE: f64 = 0.6;

fn service_bank_pnl(w: &mut World, wage_sum: &[f64]) {
    let req = w.cfg.capital_requirement;
    let payout = w.cfg.bank_payout_ratio.max(0.0);
    let opex = pay_operating_expenses(w, wage_sum);
    for b in 0..w.banks.len() {
        if !w.banks[b].alive {
            continue;
        }
        let profit = finite_or_zero(
            w.banks[b].interest_income - w.banks[b].interest_expense - w.banks[b].loan_losses - opex[b],
        );
        w.banks[b].profit = profit;
        // Losses booked after this point are left for next month's P&L (`bank_loss_carry`, see `sim::begin_month`).
        w.banks[b].loan_losses_recognised = w.banks[b].loan_losses;
        // Payout: a share of profit when adequately capitalised, plus a gradual distribution of capital above the
        // management target (buybacks/special dividends), so bank equity does not accumulate without bound.
        let cr = capital_ratio(w, b as BankId);
        let target = req + CAPITAL_TARGET_BUFFER;
        let excess = if cr > target { (cr - target) * rwa(w.banks[b].mortgage_loans, w.banks[b].business_loans) } else { 0.0 };
        if (profit > 0.0 && cr > req + DIVIDEND_BUFFER) || excess > 0.0 {
            // Above the management target, all profit is paid out (capital is not needed for growth).
            let regular = if cr > target { profit.max(0.0) } else { payout * profit.max(0.0) };
            let div = (regular + EXCESS_CAPITAL_PAYOUT * excess).min(w.banks[b].reserves.max(0.0));
            if div > 0.0 {
                ledger::pay(w, Account::Bank(b as BankId), Account::Fund, div);
                let reg = div.min(regular);
                w.fund.dividends_received += reg;
                w.fund.special_dividends += div - reg;
                w.banks[b].dividends = div;
            }
        }
    }
}

/// Service all loans, pay deposit interest, interest on reserves, liquidity management, bank dividends.
/// Cost is O(loans + depositors): one pass over the loan book, one over households and firms.
pub fn service(w: &mut World) {
    service_loans(w);
    let wage_sum = service_deposits(w);
    service_reserves_and_discount(w);
    service_liquidity(w);
    service_bank_pnl(w, &wage_sum);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::testkit::tiny_world;

    #[test]
    fn unemployed_illiquid_owner_misses_a_payment_after_the_anchor() {
        let mut w = world(4);
        set_rates(&mut w);
        // An unemployed owner-occupier on a 95% LTV mortgage.
        let (h, house) = (0..w.houses.len())
            .find_map(|i| match w.houses[i].owner {
                Owner::Household(h) if w.houses[i].occupant == Some(h) => Some((h, i as HouseId)),
                _ => None,
            })
            .expect("tiny world has an owner-occupier");
        w.households[h as usize].status = LaborStatus::Unemployed;
        let bank = w.households[h as usize].bank;
        let value = common::house_value(&w, house);
        let id = ledger::create_loan(
            &mut w,
            bank,
            Borrower::Household(h),
            LoanKind::Mortgage { house },
            0.95 * value,
            0.06,
            360,
        );
        // Before the anchor the household is rich enough to pay (this sets the loan's payment).
        w.households[h as usize].deposits += 1.0e7;
        ledger::fund_sell(&mut w, h, f64::MAX);
        assert_eq!(income_shock_miss_prob(&w, id), 0.0);
        service(&mut w);
        assert_eq!(w.loans.get(id).unwrap().months_delinquent, 0);

        // After the anchor, with 1.5 months of payments in deposits, the payment is missed although it could be paid. The
        // miss is made certain here (probability 1 at 1.5 months, under a six-month grading).
        w.cfg.housing_income_miss_prob = 1.0;
        w.cfg.housing_shock_full_miss_months = 2.0;
        w.cfg.housing_shock_liquidity_months = 6.0;
        let payment = w.loans.get(id).unwrap().payment;
        let dep = w.households[h as usize].deposits;
        ledger::pay(&mut w, Account::Household(h), Account::Government, dep - 1.5 * payment);
        w.housing.anchored = true;
        w.housing.anchor_tick = w.tick;
        assert!((income_shock_miss_prob(&w, id) - w.cfg.housing_income_miss_prob).abs() < 1e-12);
        let before = w.households[h as usize].deposits;
        service(&mut w);
        assert_eq!(w.loans.get(id).unwrap().months_delinquent, 1);
        assert!(w.households[h as usize].deposits >= before - 1e-6, "a missed payment takes no cash out");

        // An employed borrower with the same loan never takes the income-shock miss.
        w.households[h as usize].status = LaborStatus::Employed;
        assert_eq!(income_shock_miss_prob(&w, id), 0.0);
    }

    #[test]
    fn income_shock_miss_is_graded_by_liquidity() {
        // The grading is off by default (p1-housing); this test pins the graded mechanism itself at 1 and 3 months.
        let cfg = Config { housing_shock_full_miss_months: 1.0, housing_shock_liquidity_months: 3.0, ..Config::default() };
        let payment = 1_000.0;
        // Full probability at or below one month of payments, zero at or above three (a job loss with under three months
        // of liquidity), linear in between.
        assert_eq!(shock_miss_liquidity_weight(0.0, payment, &cfg), 1.0);
        assert_eq!(shock_miss_liquidity_weight(1.0 * payment, payment, &cfg), 1.0);
        assert!((shock_miss_liquidity_weight(2.0 * payment, payment, &cfg) - 0.5).abs() < 1e-12);
        assert_eq!(shock_miss_liquidity_weight(3.0 * payment, payment, &cfg), 0.0);
        assert_eq!(shock_miss_liquidity_weight(60.0 * payment, payment, &cfg), 0.0);
        // Negative deposits count as no liquidity; a loan with no payment due never misses.
        assert_eq!(shock_miss_liquidity_weight(-5.0 * payment, payment, &cfg), 1.0);
        assert_eq!(shock_miss_liquidity_weight(0.0, 0.0, &cfg), 0.0);
    }

    #[test]
    fn income_shock_episode_belongs_to_the_owner_who_started_it() {
        let mut w = world(3);
        let (house, h) = (0..w.houses.len())
            .find_map(|i| match w.houses[i].owner {
                Owner::Household(h) if w.houses[i].completed => Some((i as HouseId, h)),
                _ => None,
            })
            .expect("tiny world has an owner-occupier");
        let other = (h + 1) % w.households.len() as HhId;
        w.housing.income_shock_until = vec![0; w.houses.len()];
        w.housing.income_shock_owner = vec![crate::housing::HHID_NONE; w.houses.len()];
        w.housing.income_shock_until[house as usize] = w.tick + 12;
        w.housing.income_shock_owner[house as usize] = h;
        assert!(crate::housing::income_shock_active(&w, house, h));
        // A buyer or heir of the house does not inherit the episode.
        assert!(!crate::housing::income_shock_active(&w, house, other));
        // Once the episode lapses it is inactive for everyone.
        w.tick += 12;
        assert!(!crate::housing::income_shock_active(&w, house, h));
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

    fn has_household_loan(w: &World, h: HhId) -> bool {
        w.loans.iter().any(|(_, l)| l.borrower == Borrower::Household(h))
    }

    #[test]
    fn rates_follow_curve_and_policy() {
        let mut w = world(1);
        set_rates(&mut w);
        let spread = w.cfg.mortgage_spread;
        for b in 0..w.banks.len() {
            let bank = &w.banks[b];
            assert!((bank.deposit_rate - w.cfg.deposit_beta * w.cb.policy_rate).abs() < 1e-12);
            let shortfall = (w.cfg.capital_requirement + 0.02 - capital_ratio(&w, b as BankId)).max(0.0);
            let expected = w.curve.yield_at(120.0) + spread + 0.5 * shortfall;
            assert!((bank.mortgage_rate - expected).abs() < 1e-12);
            let biz = 0.5 * w.curve.yield_at(24.0) + 0.5 * w.curve.yield_at(60.0) + w.cfg.business_spread;
            assert!((bank.business_rate_base - biz).abs() < 1e-12);
        }

        // Moving the 10y yield moves mortgage rates one for one; a credit-spread shock adds to both loan rates.
        let before = w.banks[0].mortgage_rate;
        w.curve.yields[3] += 0.01;
        set_rates(&mut w);
        assert!((w.banks[0].mortgage_rate - before - 0.01).abs() < 1e-12);
        w.shocks.credit_spread = Some((0.005, u32::MAX));
        let before = w.banks[0].business_rate_base;
        set_rates(&mut w);
        assert!((w.banks[0].business_rate_base - before - 0.005).abs() < 1e-12);

        // Deposit rate follows policy and is floored at zero.
        w.cb.policy_rate = 0.05;
        set_rates(&mut w);
        assert!((w.banks[0].deposit_rate - w.cfg.deposit_beta * 0.05).abs() < 1e-12);
        w.cb.policy_rate = -0.01;
        set_rates(&mut w);
        assert_eq!(w.banks[0].deposit_rate, 0.0);
    }

    #[test]
    fn capital_ratio_guards_zero_rwa() {
        let mut w = World::empty(Config::default(), 1);
        w.banks.push(Bank {
            alive: true,
            reserves: 10.0,
            ..Default::default()
        });
        assert_eq!(capital_ratio(&w, 0), 1.0);
        w.banks[0].reserves = 0.0;
        w.banks[0].deposits = 5.0;
        assert_eq!(capital_ratio(&w, 0), 0.0);
    }

    #[test]
    fn mortgage_quotes_respect_ltv_and_dti() {
        let mut w = world(2);
        set_rates(&mut w);
        let price = w.cfg.median_house_price;
        let h = (0..w.households.len())
            .find(|&i| {
                let hh = &w.households[i];
                hh.alive && hh.status == LaborStatus::Employed && hh.wage > 0.0
            })
            .unwrap() as HhId;
        let wage = w.households[h as usize].wage;
        let existing = household_mortgage_payments(&w, h);
        let q = mortgage_quote(&w, h, price, false).expect("employed household should qualify");
        assert!(q.max_loan > 0.0);
        assert!(q.max_loan <= w.cfg.max_ltv * price + 1e-6);
        let payment = annuity_payment(q.max_loan, q.rate, w.cfg.mortgage_term_months);
        let proptax = w.cfg.property_tax / 12.0 * price;
        assert!(payment + proptax + existing <= w.cfg.max_dti * wage + 1e-6);
        assert_eq!(q.rate, w.banks[q.bank as usize].mortgage_rate);

        // Investor rules: higher rate and tighter LTV.
        if let Some(qi) = mortgage_quote(&w, h, price, true) {
            assert!(qi.max_loan <= w.cfg.max_ltv_investor * price + 1e-6);
            if qi.bank == q.bank {
                assert!((qi.rate - q.rate - INVESTOR_RATE_ADD).abs() < 1e-12);
            }
        }

        // Unemployed household with no rent income: no qualifying income, no quote.
        let u = (0..w.households.len())
            .find(|&i| w.households[i].alive && w.households[i].status == LaborStatus::Unemployed)
            .unwrap() as HhId;
        assert!(mortgage_quote(&w, u, price, false).is_none());
    }

    #[test]
    fn loans_amortise_to_zero_over_term() {
        let mut w = world(3);
        set_rates(&mut w);
        let h: HhId = 0;
        let bank = w.households[h as usize].bank;
        // Enough cash to service this loan alongside any mortgage the household already has.
        ledger::pay(&mut w, Account::CentralBank, Account::Household(h), 200_000.0);
        let id = ledger::create_loan(
            &mut w,
            bank,
            Borrower::Household(h),
            LoanKind::Consumer,
            12_000.0,
            0.06,
            12,
        );
        let expected_payment = annuity_payment(12_000.0, 0.06, 12);
        assert!((w.loans.get(id).unwrap().payment - expected_payment).abs() < 1e-9);
        let mut last_principal = 12_000.0;
        for _ in 0..12 {
            service(&mut w);
            ledger::check_consistency(&w).unwrap();
            if let Some(l) = w.loans.get(id) {
                assert!(l.principal < last_principal);
                assert_eq!(l.months_delinquent, 0);
                last_principal = l.principal;
            }
        }
        assert!(w.loans.get(id).is_none(), "loan should be fully repaid after its term");
    }

    #[test]
    fn delinquency_increments_when_borrower_cannot_pay() {
        let mut w = world(4);
        set_rates(&mut w);
        let h: HhId = 5;
        let bank = w.households[h as usize].bank;
        let id = ledger::create_loan(
            &mut w,
            bank,
            Borrower::Household(h),
            LoanKind::Consumer,
            6_000.0,
            0.06,
            24,
        );
        // Strip cash and Fund shares through the ledger.
        ledger::fund_sell(&mut w, h, f64::MAX);
        let dep = w.households[h as usize].deposits;
        ledger::pay(&mut w, Account::Household(h), Account::Government, dep);
        assert!(w.households[h as usize].deposits.abs() < 1e-9);

        service(&mut w);
        ledger::check_consistency(&w).unwrap();
        let l = w.loans.get(id).unwrap();
        assert_eq!(l.months_delinquent, 1);
        assert_eq!(l.months_remaining, 24);
        assert_eq!(l.principal, 6_000.0);
    }

    #[test]
    fn deposit_interest_is_paid_to_depositor() {
        let mut w = world(5);
        set_rates(&mut w);
        let h = (0..w.households.len())
            .find(|&i| w.households[i].alive && w.households[i].deposits > 100.0 && !has_household_loan(&w, i as HhId))
            .unwrap();
        let bank = w.households[h].bank as usize;
        let dep = w.households[h].deposits;
        let rate = w.banks[bank].deposit_rate;
        let income_before = w.hh_month[h].income.interest;
        let expense_before = w.banks[bank].interest_expense;
        service(&mut w);
        let paid = w.hh_month[h].income.interest - income_before;
        assert!(paid > 0.0);
        assert!((paid - dep * rate / 12.0).abs() < 1e-9);
        assert!(w.banks[bank].interest_expense - expense_before >= paid - 1e-9);
        ledger::check_consistency(&w).unwrap();
    }

    #[test]
    fn discount_window_covers_negative_reserves_and_is_repaid() {
        let mut w = world(6);
        set_rates(&mut w);
        let before = w.banks[0].reserves;
        let amt = before + 1_000.0;
        ledger::pay(&mut w, Account::Bank(0), Account::CentralBank, amt);
        assert!((w.banks[0].reserves + 1_000.0).abs() < 1e-6);

        service(&mut w);
        ledger::check_consistency(&w).unwrap();
        assert!(w.banks[0].cb_borrowing > 0.0, "negative reserves should draw on the discount window");
        assert!(w.banks[0].reserves >= -1e-9);
        assert!(w.banks[0].interest_expense > 0.0);

        // Flood reserves well above the 5%-of-deposits target; the discount window is repaid in full.
        let flood = 0.05 * w.banks[0].deposits + 10_000.0;
        ledger::pay(&mut w, Account::CentralBank, Account::Bank(0), flood);
        service(&mut w);
        ledger::check_consistency(&w).unwrap();
        assert!(w.banks[0].cb_borrowing.abs() < 1e-6);
    }

    #[test]
    fn credit_refused_when_capital_is_short() {
        let mut w = world(7);
        set_rates(&mut w);
        let f = w.firms.iter().find(|f| f.alive && f.bank == 0).unwrap().id;
        let n_before = w.flows.new_business_loans;
        assert!(request_business_loan(&mut w, f, 10.0).is_some());
        assert!((w.flows.new_business_loans - n_before - 10.0).abs() < 1e-9);
        ledger::check_consistency(&w).unwrap();

        // Make bank 0 insolvent through the ledger (reserves paid to the CB).
        let x = w.banks[0].equity() + 1.0e6;
        ledger::pay(&mut w, Account::Bank(0), Account::CentralBank, x);
        assert!(capital_ratio(&w, 0) < w.cfg.capital_requirement);
        assert!(request_business_loan(&mut w, f, 10.0).is_none());

        // Households of bank 0 fall back to the best-capitalised bank for mortgages.
        set_rates(&mut w);
        let h = (0..w.households.len())
            .find(|&i| w.households[i].alive && w.households[i].bank == 0 && w.households[i].status == LaborStatus::Employed)
            .unwrap() as HhId;
        if let Some(q) = mortgage_quote(&w, h, w.cfg.median_house_price, false) {
            assert_ne!(q.bank, 0);
        }
    }

    #[test]
    fn business_rate_has_default_risk_premium() {
        let mut w = world(8);
        set_rates(&mut w);
        let f: FirmId = 0;
        let base = w.banks[w.firms[0].bank as usize].business_rate_base;
        // No business debt: leverage is zero and coverage capped, so the premium is the (small, negative) PD term.
        assert!(w.firms[0].loans.is_empty());
        let no_debt = default_premium(&w.cfg, 0.0, MAX_COVERAGE);
        assert!((business_rate(&w, f) - (base + no_debt)).abs() < 1e-12);
        // Debt far above assets lifts the rate above the base, and matches the PD formula at that leverage.
        ledger::create_loan(&mut w, 0, Borrower::Firm(f), LoanKind::Business, 1.0e9, 0.05, 60);
        let lev = 1.0e9 / (w.firms[0].capital.max(0.0) * common::capital_price_world(&w)
            + w.firms[0].deposits.max(0.0)
            + w.firms[0].inventory.max(0.0) * w.firms[0].price.max(0.0));
        let cov = firm_coverage(&w, f);
        let expected = base + default_premium(&w.cfg, lev.min(MAX_LEVERAGE), cov);
        assert!((business_rate(&w, f) - expected).abs() < 1e-9);
        assert!(business_rate(&w, f) > base);
    }

    #[test]
    fn default_premium_is_convex_in_leverage_and_falls_with_coverage() {
        let cfg = Config::default();
        let p = |l: f64, c: f64| default_premium(&cfg, l, c);
        // Reference firm carries no premium.
        assert!(p(PD_REF_LEVERAGE, PD_REF_COVERAGE).abs() < 1e-12);
        // Increasing and convex in leverage.
        let (a, b, c) = (p(0.5, 3.0), p(0.65, 3.0), p(0.8, 3.0));
        assert!(a < b && b < c);
        assert!(c - b > b - a);
        // Weaker coverage raises the premium.
        assert!(p(0.5, 1.5) > p(0.5, 3.0) && p(0.5, 3.0) > p(0.5, 10.0));
    }

    #[test]
    fn lender_premium_rises_with_losses_and_is_bounded_and_gradual() {
        let w = world(8);
        let cfg = &w.cfg;
        let p = PremiumParams {
            eq_term: 0.0,
            loss_sens: cfg.ebp_loss_sens,
            cap_sens: cfg.ebp_capital_sens,
            adjust: cfg.ebp_adjust,
            pmin: cfg.ebp_min,
            pmax: cfg.ebp_max,
            norm_adapt: cfg.loss_norm_adapt,
            norm_init: cfg.loss_norm_initial,
        };
        let mut bank = w.banks[0].clone();
        bank.business_loans = 100.0;
        bank.ebp = 0.0;
        // Normal expected losses (1%/yr): the norm is set from them and the premium stays at zero.
        step_premium(&mut bank, 0.01, 0.0, &p);
        assert!(bank.ebp.abs() < 1e-12);
        // Expected losses jump to 50%/yr: the target is at the cap, closed only partly in one month.
        step_premium(&mut bank, 0.5, 0.0, &p);
        let p1 = bank.ebp;
        assert!(p1 > 0.0 && p1 < p.pmax, "one partial step, got {p1}");
        for _ in 0..24 {
            step_premium(&mut bank, 0.5, 0.0, &p);
        }
        assert!(bank.ebp <= p.pmax + 1e-12 && bank.ebp > 0.5 * p.pmax);
        // Expected losses fall back below the (now raised) norm: the premium turns negative, bounded below.
        for _ in 0..200 {
            step_premium(&mut bank, 0.0, 0.0, &p);
        }
        assert!(bank.ebp >= p.pmin - 1e-12 && bank.ebp < 0.0);
    }

    #[test]
    fn service_books_bank_pnl_and_dividends() {
        let mut w = world(9);
        set_rates(&mut w);
        service(&mut w);
        ledger::check_consistency(&w).unwrap();
        for b in &w.banks {
            let opex = BANK_OPEX_SHARE * (b.interest_income - b.interest_expense).max(0.0);
            let expect = b.interest_income - b.interest_expense - b.loan_losses - opex;
            assert!((b.profit - expect).abs() < 1e-6 * (1.0 + expect.abs()));
        }
        let paid: f64 = w.banks.iter().map(|b| b.dividends).sum();
        assert!((w.fund.dividends_received + w.fund.special_dividends - paid).abs() < 1e-6 * (1.0 + paid));
    }

    /// `bank_loss_carry`: a business write-off booked after the month's P&L (as `firms::close_books` books a failed
    /// firm's loans) lowers the next month's bank profit, dividends and bank tax by exactly that loss, once. Legacy
    /// (`false`) drops it from the P&L.
    #[test]
    fn late_write_off_reaches_next_months_bank_pnl_once() {
        let month = |x: &mut World| {
            crate::sim::begin_month(x);
            set_rates(x);
            service(x);
        };
        let mut w = world(9);
        assert!(w.cfg.bank_loss_carry, "the fix is the default");
        month(&mut w);
        // Size the loss well inside next month's tax bases (after and before operating expenses), from a dry run.
        let mut dry = w.clone();
        month(&mut dry);
        let base = |b: &Bank| b.profit.min(b.interest_income - b.interest_expense - b.loan_losses);
        let bank = (0..dry.banks.len()).max_by(|&a, &b| base(&dry.banks[a]).total_cmp(&base(&dry.banks[b]))).unwrap();
        assert!(base(&dry.banks[bank]) > 0.0, "a bank with a positive tax base");
        let loss = 0.25 * base(&dry.banks[bank]);
        // A business loan of that bank written off after the P&L, in both worlds.
        let f = (0..w.firms.len()).find(|&f| w.firms[f].alive).unwrap();
        let lid = ledger::create_loan(&mut w, bank as BankId, Borrower::Firm(f as FirmId), LoanKind::Business, loss, 0.06, 60);
        let mut legacy = w.clone();
        legacy.cfg.bank_loss_carry = false;
        for x in [&mut w, &mut legacy] {
            assert_eq!(x.banks[bank].loan_losses_recognised, x.banks[bank].loan_losses);
            let pre = x.banks[bank].loan_losses;
            assert_eq!(ledger::write_off(x, lid), loss);
            assert_eq!(x.banks[bank].loan_losses, pre + loss);
        }
        // A dead bank carries nothing.
        let mut dead = w.clone();
        dead.banks[bank].alive = false;
        crate::sim::begin_month(&mut dead);
        assert_eq!(dead.banks[bank].loan_losses, 0.0);

        // Next month: only the fixed P&L recognises the loss; interest flows and operating expenses are identical.
        let carried = w.banks[bank].loan_losses - w.banks[bank].loan_losses_recognised;
        month(&mut w);
        month(&mut legacy);
        let (on, off) = (&w.banks[bank], &legacy.banks[bank]);
        assert_eq!(on.loan_losses.to_bits(), carried.to_bits());
        assert!((carried - loss).abs() <= 1e-12 * loss, "carried {carried} vs written off {loss}");
        assert_eq!(off.loan_losses, 0.0);
        assert_eq!(on.interest_income.to_bits(), off.interest_income.to_bits());
        assert_eq!(on.interest_expense.to_bits(), off.interest_expense.to_bits());
        assert!((off.profit - on.profit - loss).abs() <= 1e-9 * loss, "profit falls by the loss: {}", off.profit - on.profit);
        assert!(on.dividends <= off.dividends && off.dividends - on.dividends <= loss * (1.0 + 1e-9));
        assert!(on.dividends < off.dividends, "the loss is not paid out as a dividend");
        // The bank tax base is lower by the loss on both bases (after operating expenses, and the legacy one before).
        let cit = w.cfg.corporate_tax;
        for after_opex in [true, false] {
            let tax = |x: &World| {
                let mut x = x.clone();
                x.cfg.bank_tax_after_opex = after_opex;
                crate::fiscal::collect_and_finance(&mut x);
                x.fiscal.bank_tax_month
            };
            let d = tax(&legacy) - tax(&w);
            assert!((d - cit * loss).abs() <= 1e-9 * cit * loss, "after_opex {after_opex}: tax falls by {d}, want {}", cit * loss);
        }
        // Recognised once: the month after, nothing is left to carry.
        month(&mut w);
        assert_eq!(w.banks[bank].loan_losses, 0.0);
        ledger::check_consistency(&w).unwrap();
    }

    // rz1 F2: the credit-spread tail reaches both loan rates through `set_rates`.
    #[test]
    fn credit_tail_reaches_mortgage_and_business_rates() {
        let rates = |tick: u32, half_life: f64, with_shock: bool| -> (f64, f64) {
            let mut w = world(11);
            w.cfg.credit_tail_half_life_months = half_life;
            if with_shock {
                w.shocks.credit_spread = Some((0.02, 12));
            }
            w.tick = tick;
            set_rates(&mut w);
            (w.banks[0].mortgage_rate - 0.5 * (req_shortfall(&w, 0)), w.banks[0].business_rate_base - w.banks[0].ebp)
        };
        let (m0, b0) = rates(14, 0.0, false);
        // Inside the window: the full 2 pp on both, with or without the tail key.
        for hl in [0.0, 9.0] {
            let (m, b) = rates(5, hl, true);
            let (m_ref, b_ref) = rates(5, 0.0, false);
            assert!((m - m_ref - 0.02).abs() < 1e-12 && (b - b_ref - 0.02).abs() < 1e-12, "hl {hl}");
        }
        // After the window, tail off: nothing (the rectangle). Tail on: p x 0.5^(3 / 9) at tick 14.
        let (m, b) = rates(14, 0.0, true);
        assert!((m - m0).abs() < 1e-12 && (b - b0).abs() < 1e-12);
        let want = 0.02 * 0.5f64.powf(3.0 / 9.0);
        let (m, b) = rates(14, 9.0, true);
        assert!((m - m0 - want).abs() < 1e-12 && (b - b0 - want).abs() < 1e-12, "{} {}", m - m0, b - b0);
    }

    /// The bank's capital-shortfall premium on the mortgage rate (added in `set_rates`), so tests can read the base alone.
    fn req_shortfall(w: &World, b: usize) -> f64 {
        (w.cfg.capital_requirement + DIVIDEND_BUFFER - capital_ratio(w, b as BankId)).max(0.0)
    }

    // b9-rates tests: R-B (business policy weight) and M-M (dynamic mortgage spread).
    /// Business base rate without the bank's lender premium (`ebp` is added last in `set_rates`).
    fn biz_base_ex_ebp(w: &World, b: usize) -> f64 {
        w.banks[b].business_rate_base - w.banks[b].ebp
    }

    #[test]
    fn business_weight_zero_is_legacy() {
        let mut w = world(1);
        set_rates(&mut w);
        assert!(!w.banking.mtg_init);
        for b in 0..w.banks.len() {
            let legacy =
                0.5 * w.curve.yield_at(24.0) + 0.5 * w.curve.yield_at(60.0) + w.cfg.business_spread + w.banks[b].ebp;
            assert_eq!(w.banks[b].business_rate_base.to_bits(), legacy.to_bits());
        }
    }

    #[test]
    fn business_weight_blends_policy() {
        let mut w = world(2);
        w.cfg.business_policy_weight = 0.5;
        w.curve.yields[1] = 0.03;
        w.curve.yields[2] = 0.05;
        w.cb.policy_rate = 0.04;
        set_rates(&mut w);
        // 0.5 x policy + 0.5 x (0.5 y2 + 0.5 y5) + spread: with policy 0.04 and T 0.04 this is 0.04 + spread.
        for b in 0..w.banks.len() {
            let want = 0.5 * 0.04 + 0.5 * 0.04 + w.cfg.business_spread;
            assert!((biz_base_ex_ebp(&w, b) - want).abs() < 1e-12);
        }
        // A 1 pp rise in the policy rate moves the base by 0.5 pp; the curve is unchanged.
        let before = biz_base_ex_ebp(&w, 0);
        w.cb.policy_rate = 0.05;
        set_rates(&mut w);
        assert!((biz_base_ex_ebp(&w, 0) - before - 0.005).abs() < 1e-12);
    }

    #[test]
    fn business_weight_one_follows_policy_only() {
        for weight in [1.0, 1.7] {
            let mut w = world(3);
            w.cfg.business_policy_weight = weight;
            w.cfg.business_policy_offset = 0.001;
            w.cb.policy_rate = 0.04;
            set_rates(&mut w);
            // Weights above 1 act as 1: the base is policy + spread + offset.
            for b in 0..w.banks.len() {
                let want = 0.04 + w.cfg.business_spread + 0.001;
                assert!((biz_base_ex_ebp(&w, b) - want).abs() < 1e-12, "weight {weight}");
            }
        }
    }

    #[test]
    fn business_offset_is_neutral_by_design() {
        for policy in [0.02, 0.06] {
            let mut w = world(4);
            w.curve.yields[1] = 0.03;
            w.curve.yields[2] = 0.05;
            let term = 0.5 * w.curve.yield_at(24.0) + 0.5 * w.curve.yield_at(60.0);
            w.cfg.business_policy_weight = 0.5;
            w.cfg.business_policy_offset = 0.5 * (term - policy);
            w.cb.policy_rate = policy;
            set_rates(&mut w);
            // The offset makes the level equal to the legacy base (term + spread) for any policy rate.
            let legacy = term + w.cfg.business_spread;
            for b in 0..w.banks.len() {
                assert!((biz_base_ex_ebp(&w, b) - legacy).abs() < 1e-12, "policy {policy}");
            }
        }
    }

    /// Test world with 500 households and the given config overrides (as `world`).
    fn world_cfg(cfg: Config, seed: u64) -> World {
        tiny_world(Config { n_households: 500, ..cfg }, seed)
    }

    /// Deterministic uniform draw in [0, 1) from a 64-bit LCG (Knuth MMIX constants).
    fn lcg_unit(state: &mut u64) -> f64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (*state >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Capital-shortfall premium on a bank's mortgage rate, as in `set_rates`.
    fn mtg_shortfall(w: &World, b: usize) -> f64 {
        (w.cfg.capital_requirement + DIVIDEND_BUFFER - capital_ratio(w, b as BankId)).max(0.0)
    }

    #[test]
    fn mortgage_spread_off_is_legacy() {
        let mut w = world(5);
        for _ in 0..12 {
            set_rates(&mut w);
        }
        assert!(!w.banking.mtg_init);
        for b in 0..w.banks.len() {
            let expected = w.curve.yield_at(120.0) + w.cfg.mortgage_spread + 0.5 * mtg_shortfall(&w, b);
            assert!((w.banks[b].mortgage_rate - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn mortgage_spread_first_call_is_neutral() {
        let cfg = Config { mortgage_spread_vol_gain: 4.0, ..Default::default() };
        let mut dynamic = world_cfg(cfg, 6);
        let mut legacy = world(6);
        set_rates(&mut dynamic);
        set_rates(&mut legacy);
        assert!(dynamic.banking.mtg_init);
        assert_eq!(dynamic.banking.mtg_adj, 0.0);
        for b in 0..dynamic.banks.len() {
            assert!((dynamic.banks[b].mortgage_rate - legacy.banks[b].mortgage_rate).abs() < 1e-12);
        }
    }

    #[test]
    fn mortgage_spread_widens_with_volatility() {
        let cfg = || Config { mortgage_spread_vol_gain: 4.0, ..Default::default() };
        let mut volatile = world_cfg(cfg(), 7);
        let mut flat = world_cfg(cfg(), 7);
        // One initialising call, then six updating calls.
        set_rates(&mut volatile);
        set_rates(&mut flat);
        for k in 0..6 {
            volatile.curve.yields[3] += if k % 2 == 0 { 0.005 } else { -0.005 };
            set_rates(&mut volatile);
            set_rates(&mut flat);
        }
        // Volatility EWMA after six updates: d - (d - 0.0012)(6/7)^6 = 0.00349 for d = 0.005 and 0.00048 for d = 0.
        // The adjustment is about +0.0089..0.0092 on the volatile path and about -0.0026..-0.0029 on the flat path.
        let up = volatile.banking.mtg_adj;
        let down = flat.banking.mtg_adj;
        assert!((0.0085..=0.0100).contains(&up), "volatile adjustment {up}");
        assert!((-0.0032..=-0.0026).contains(&down), "flat adjustment {down}");
        for b in 0..volatile.banks.len() {
            let expected =
                volatile.curve.yield_at(120.0) + volatile.cfg.mortgage_spread + up + 0.5 * mtg_shortfall(&volatile, b);
            assert!((volatile.banks[b].mortgage_rate - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn mortgage_spread_responds_to_lender_premium() {
        let cfg = Config { mortgage_spread_stress_gain: 0.5, ..Default::default() };
        let mut w = world_cfg(cfg, 8);
        update_mortgage_spread(&mut w); // initialising call
        for bank in w.banks.iter_mut() {
            if bank.alive {
                bank.ebp = 0.02;
            }
        }
        update_mortgage_spread(&mut w);
        assert!((w.banking.mtg_adj - 0.5 * (0.02 - 0.0031)).abs() < 1e-12);
    }

    #[test]
    fn mortgage_spread_is_bounded_and_finite() {
        // Huge volatility: the adjustment is clamped to max_adj exactly.
        let cfg = Config { mortgage_spread_vol_gain: 1000.0, ..Default::default() };
        let mut w = world_cfg(cfg, 9);
        update_mortgage_spread(&mut w);
        for k in 0..6 {
            w.curve.yields[3] += if k % 2 == 0 { 0.05 } else { -0.05 };
            update_mortgage_spread(&mut w);
        }
        assert_eq!(w.banking.mtg_adj, w.cfg.mortgage_spread_max_adj);
        // A NaN 10y yield is read as 0, so the adjustment stays finite.
        w.curve.yields[3] = f64::NAN;
        update_mortgage_spread(&mut w);
        assert!(w.banking.mtg_adj.is_finite());
        // Random-walk yields for 240 months: all state stays finite and the adjustment stays within its bounds.
        let cfg = Config { mortgage_spread_vol_gain: 4.0, mortgage_spread_stress_gain: 0.5, ..Default::default() };
        let mut w = world_cfg(cfg, 19);
        let mut st = 0x2545_F491_4F6C_DD1D_u64;
        let mut y = 0.04;
        for _ in 0..240 {
            y += (lcg_unit(&mut st) - 0.5) * 0.004;
            w.curve.yields[3] = y;
            update_mortgage_spread(&mut w);
            let s = &w.banking;
            assert!(s.mtg_vol.is_finite() && s.mtg_vol_norm.is_finite() && s.mtg_stress_norm.is_finite());
            assert!(s.mtg_adj >= w.cfg.mortgage_spread_min_adj && s.mtg_adj <= w.cfg.mortgage_spread_max_adj);
        }
    }

    #[test]
    fn mortgage_spread_norm_makes_mean_neutral() {
        let cfg = Config { mortgage_spread_vol_gain: 4.0, ..Default::default() };
        let mut w = world_cfg(cfg, 10);
        let mut st = 0x9E37_79B9_7F4A_7C15_u64;
        update_mortgage_spread(&mut w); // initialising call
        let (mut sum, mut n) = (0.0f64, 0usize);
        for t in 0..1200 {
            // Stationary path: i.i.d. uniform around 4% with mean |monthly change| about 0.0012 (half-width 0.0018).
            w.curve.yields[3] = 0.04 + 0.0018 * (2.0 * lcg_unit(&mut st) - 1.0);
            update_mortgage_spread(&mut w);
            if t >= 600 {
                sum += w.banking.mtg_adj;
                n += 1;
            }
        }
        let mean = sum / n as f64;
        assert!(mean.abs() < 0.0005, "mean adjustment {mean}");
    }

    #[test]
    fn mtg_state_is_deterministic() {
        let cfg = || Config { mortgage_spread_vol_gain: 4.0, mortgage_spread_stress_gain: 0.5, ..Default::default() };
        let mut a = world_cfg(cfg(), 11);
        let mut b = world_cfg(cfg(), 11);
        for t in 0..24 {
            let y = 0.04 + 0.002 * ((t % 5) as f64 - 2.0);
            a.curve.yields[3] = y;
            b.curve.yields[3] = y;
            set_rates(&mut a);
            set_rates(&mut b);
        }
        assert_eq!(a.banking.mtg_adj.to_bits(), b.banking.mtg_adj.to_bits());
        for k in 0..a.banks.len() {
            assert_eq!(a.banks[k].mortgage_rate.to_bits(), b.banks[k].mortgage_rate.to_bits());
        }
    }

    /// A mortgaged owner-occupier's home repossessed as `housing::foreclose` leaves it: the household moves out, the lender
    /// owns the house (listed for sale) and holds the loan as the repossession's loan, 4 months delinquent. The household
    /// has no other loan. Returns (household, loan).
    fn repossess_for_test(w: &mut World) -> (HhId, LoanId) {
        let (h, house) = (0..w.houses.len())
            .find_map(|i| match w.houses[i].owner {
                Owner::Household(h)
                    if w.houses[i].occupant == Some(h)
                        && w.houses[i].mortgage.is_some()
                        && w.households[h as usize].properties.len() == 1 =>
                {
                    Some((h, i as HouseId))
                }
                _ => None,
            })
            .expect("tiny world has a mortgaged owner-occupier with one property");
        let loan = w.houses[house as usize].mortgage.unwrap();
        let bank = w.loans.get(loan).unwrap().bank;
        let hs = &mut w.houses[house as usize];
        hs.occupant = None;
        hs.for_sale = true;
        hs.for_rent = false;
        let hh = &mut w.households[h as usize];
        hh.home = None;
        hh.tenure = Tenure::WithParents;
        ledger::transfer_house(w, house, Owner::Bank(bank));
        w.housing.reo_loan.resize(w.houses.len(), None);
        w.housing.reo_loan[house as usize] = Some(loan);
        w.loans.get_mut(loan).unwrap().months_delinquent = 4;
        assert!(crate::housing::is_reo_loan(&w.housing, loan, w.loans.get(loan).unwrap().kind));
        assert!(!w.loans.iter().any(|(id, l)| id != loan && l.borrower == Borrower::Household(h)));
        (h, loan)
    }

    /// `housing_reo_nonaccrual`: the evicted borrower does not service the repossession's loan. With no cash and Fund
    /// shares to sell, no shares are sold, no interest or principal is paid, and the loan's balance and delinquency are
    /// left as they were until the house sells. Legacy (off): the former borrower sells shares and pays in full on a house
    /// the bank owns, which also clears the delinquency.
    #[test]
    fn repossession_loan_is_not_serviced_by_the_former_borrower() {
        for on in [true, false] {
            let mut w = world(6);
            w.cfg.housing_reo_nonaccrual = on;
            set_rates(&mut w);
            let (h, loan) = repossess_for_test(&mut w);
            let hu = h as usize;
            // Deposits to zero, then Fund shares worth far more than a payment.
            let dep = w.households[hu].deposits;
            ledger::pay(&mut w, Account::Household(h), Account::Government, dep);
            ledger::pay(&mut w, Account::CentralBank, Account::Household(h), 1.0e6);
            ledger::fund_buy(&mut w, h, 1.0e6);
            let (dep0, shares0) = (w.households[hu].deposits, w.households[hu].shares);
            let p0 = w.loans.get(loan).unwrap().principal;
            service_loans(&mut w);
            ledger::check_consistency(&w).unwrap();
            let l = w.loans.get(loan).expect("the loan stays on the books until the house sells");
            if on {
                assert_eq!(l.principal.to_bits(), p0.to_bits(), "principal was paid on a repossession's loan");
                assert_eq!(l.months_delinquent, 4, "delinquency moved on a non-accruing loan");
                assert_eq!(w.households[hu].shares.to_bits(), shares0.to_bits(), "Fund shares were sold to pay it");
                assert_eq!(w.households[hu].deposits.to_bits(), dep0.to_bits(), "the former borrower paid");
            } else {
                assert!(l.principal < p0, "legacy: the former borrower repays principal");
                assert_eq!(l.months_delinquent, 0, "legacy: paid in full from Fund shares");
                assert!(w.households[hu].shares < shares0, "legacy: Fund shares are sold to pay");
            }
        }
    }

    /// `housing_reo_nonaccrual`: whoever holds the slot later (the former borrower after buying again, or an entrant who
    /// took the dead borrower's slot) pays only the slot's own mortgage; the repossession's loan is not charged and does
    /// not count as a second loan of the household.
    #[test]
    fn repossession_loan_is_not_charged_to_the_slots_next_mortgage() {
        let mut w = world(7);
        set_rates(&mut w);
        let (h, loan) = repossess_for_test(&mut w);
        let hu = h as usize;
        // The slot's household has cash and a new mortgage on another house.
        ledger::pay(&mut w, Account::CentralBank, Account::Household(h), 1.0e6);
        let other = (0..w.houses.len())
            .find(|&i| matches!(w.houses[i].owner, Owner::Household(o) if o != h) && w.houses[i].mortgage.is_none())
            .expect("an unmortgaged house") as HouseId;
        ledger::transfer_house(&mut w, other, Owner::Household(h));
        let bank = w.households[hu].bank;
        let own = ledger::create_loan(&mut w, bank, Borrower::Household(h), LoanKind::Mortgage { house: other }, 50_000.0, 0.06, 360);
        let (p_reo, p_own) = (w.loans.get(loan).unwrap().principal, w.loans.get(own).unwrap().principal);
        let dep0 = w.households[hu].deposits;
        service_loans(&mut w);
        ledger::check_consistency(&w).unwrap();
        assert_eq!(w.loans.get(loan).unwrap().principal.to_bits(), p_reo.to_bits());
        let l = w.loans.get(own).unwrap();
        assert!(l.principal < p_own && l.months_delinquent == 0, "the slot's own mortgage was not serviced");
        let paid = dep0 - w.households[hu].deposits;
        let due = p_own * 0.06 / 12.0 + (p_own - l.principal);
        assert!((paid - due).abs() < 1e-6, "paid {paid}, own mortgage due {due}");
    }
}
