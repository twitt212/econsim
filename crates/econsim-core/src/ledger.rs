//! Double-entry money ledger (docs/DESIGN.md §2). The ONLY place where money stocks change.
//!
//! Settlement rules:
//! - A depositor (household, firm, Fund, RestOfWorld) holds deposits at a commercial bank. Paying debits its
//!   deposits and its bank's deposit liability, and moves reserves out of that bank.
//! - Government holds money in the TGA at the central bank.
//! - A bank paying "on its own account" spends reserves (equity falls); receiving adds reserves (equity rises).
//! - The central bank paying creates reserves (its equity falls); receiving destroys them.
//! Money is therefore conserved except where banks lend (deposit creation) or the CB pays/receives.

use crate::types::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Account {
    Household(HhId),
    Firm(FirmId),
    Bank(BankId),
    Government,
    CentralBank,
    Fund,
    RestOfWorld,
}

impl From<Borrower> for Account {
    fn from(b: Borrower) -> Self {
        match b {
            Borrower::Household(h) => Account::Household(h),
            Borrower::Firm(f) => Account::Firm(f),
        }
    }
}

impl From<BondHolder> for Account {
    fn from(b: BondHolder) -> Self {
        match b {
            BondHolder::Bank(b) => Account::Bank(b),
            BondHolder::CentralBank => Account::CentralBank,
            BondHolder::Fund => Account::Fund,
            BondHolder::RestOfWorld => Account::RestOfWorld,
        }
    }
}

/// Money available to spend without overdraft (deposits; TGA; bank reserves; infinite for CB).
pub fn balance(w: &World, a: Account) -> f64 {
    match a {
        Account::Household(h) => w.households[h as usize].deposits,
        Account::Firm(f) => w.firms[f as usize].deposits,
        Account::Bank(b) => w.banks[b as usize].reserves,
        Account::Government => w.cb.tga,
        Account::CentralBank => f64::INFINITY,
        Account::Fund => w.fund.deposits,
        Account::RestOfWorld => w.row.deposits,
    }
}

fn debit(w: &mut World, a: Account, x: f64) {
    match a {
        Account::Household(h) => {
            let hh = &mut w.households[h as usize];
            hh.deposits -= x;
            let b = hh.bank as usize;
            w.banks[b].deposits -= x;
            w.banks[b].reserves -= x;
        }
        Account::Firm(f) => {
            let fm = &mut w.firms[f as usize];
            fm.deposits -= x;
            let b = fm.bank as usize;
            w.banks[b].deposits -= x;
            w.banks[b].reserves -= x;
        }
        Account::Fund => {
            w.fund.deposits -= x;
            let b = w.fund.bank as usize;
            w.banks[b].deposits -= x;
            w.banks[b].reserves -= x;
        }
        Account::RestOfWorld => {
            w.row.deposits -= x;
            let b = w.row.bank as usize;
            w.banks[b].deposits -= x;
            w.banks[b].reserves -= x;
        }
        Account::Bank(b) => w.banks[b as usize].reserves -= x,
        Account::Government => w.cb.tga -= x,
        Account::CentralBank => {}
    }
}

fn credit(w: &mut World, a: Account, x: f64) {
    match a {
        Account::Household(h) => {
            let hh = &mut w.households[h as usize];
            hh.deposits += x;
            let b = hh.bank as usize;
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
        Account::Firm(f) => {
            let fm = &mut w.firms[f as usize];
            fm.deposits += x;
            let b = fm.bank as usize;
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
        Account::Fund => {
            w.fund.deposits += x;
            let b = w.fund.bank as usize;
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
        Account::RestOfWorld => {
            w.row.deposits += x;
            let b = w.row.bank as usize;
            w.banks[b].deposits += x;
            w.banks[b].reserves += x;
        }
        Account::Bank(b) => w.banks[b as usize].reserves += x,
        Account::Government => w.cb.tga += x,
        Account::CentralBank => {}
    }
}

/// Transfer `amount` from `from` to `to`. Negative or non-finite amounts are ignored.
/// Deposits may go negative (overdraft); callers decide whether that is acceptable.
#[inline]
pub fn pay(w: &mut World, from: Account, to: Account, amount: f64) {
    if !(amount > 0.0) || !amount.is_finite() || from == to {
        return;
    }
    debit(w, from, amount);
    credit(w, to, amount);
}

/// Fast path equal to `pay(w, Household(h), Firm(f), amount)`: same guards, same float operations in the same
/// order, with no `Account` matching. Used by the per-household payment loops.
#[inline]
pub fn pay_hh_to_firm(w: &mut World, h: usize, f: usize, amount: f64) {
    if !(amount > 0.0) || !amount.is_finite() {
        return;
    }
    let hh = &mut w.households[h];
    hh.deposits -= amount;
    let bh = hh.bank as usize;
    let fm = &mut w.firms[f];
    fm.deposits += amount;
    let bf = fm.bank as usize;
    let b = &mut w.banks[bh];
    b.deposits -= amount;
    b.reserves -= amount;
    let b = &mut w.banks[bf];
    b.deposits += amount;
    b.reserves += amount;
}

/// Fast path equal to `pay(w, Household(h), RestOfWorld, amount)`.
#[inline]
pub fn pay_hh_to_row(w: &mut World, h: usize, amount: f64) {
    if !(amount > 0.0) || !amount.is_finite() {
        return;
    }
    let hh = &mut w.households[h];
    hh.deposits -= amount;
    let bh = hh.bank as usize;
    w.row.deposits += amount;
    let br = w.row.bank as usize;
    let b = &mut w.banks[bh];
    b.deposits -= amount;
    b.reserves -= amount;
    let b = &mut w.banks[br];
    b.deposits += amount;
    b.reserves += amount;
}

/// Fast path equal to `pay(w, Household(h), Government, amount)`.
#[inline]
pub fn pay_hh_to_gov(w: &mut World, h: usize, amount: f64) {
    if !(amount > 0.0) || !amount.is_finite() {
        return;
    }
    let hh = &mut w.households[h];
    hh.deposits -= amount;
    let bh = hh.bank as usize;
    let b = &mut w.banks[bh];
    b.deposits -= amount;
    b.reserves -= amount;
    w.cb.tga += amount;
}

/// Household side of `pay(Firm → Household)` for an owner-operated firm's payout. The caller has already debited the
/// firm and its bank (deposits and reserves); this credits the owner household and its bank. Non-positive or
/// non-finite amounts are ignored.
pub fn credit_owner_payout(w: &mut World, h: HhId, amount: f64) {
    if !(amount > 0.0) || !amount.is_finite() {
        return;
    }
    credit(w, Account::Household(h), amount);
}

/// Pay at most what `from` has (never overdraws a depositor). Returns the amount actually paid.
pub fn pay_up_to(w: &mut World, from: Account, to: Account, amount: f64) -> f64 {
    let avail = balance(w, from).max(0.0);
    let x = amount.min(avail);
    pay(w, from, to, x);
    x.max(0.0)
}

// ---------------------------------------------------------------- loans

/// Create a loan: bank books the asset and credits the borrower's deposits (endogenous money).
/// Also links it to `firm.loans` / `house.mortgage`. Returns the loan id.
pub fn create_loan(
    w: &mut World,
    bank: BankId,
    borrower: Borrower,
    kind: LoanKind,
    principal: f64,
    annual_rate: f64,
    term_months: u32,
) -> LoanId {
    let tick = w.tick;
    let payment = annuity_payment(principal, annual_rate, term_months);
    let id = w.loans.insert_with(|id| Loan {
        id,
        bank,
        borrower,
        kind,
        principal,
        annual_rate,
        term_months,
        months_remaining: term_months,
        payment,
        months_delinquent: 0,
        origination_tick: tick,
    });
    {
        let b = &mut w.banks[bank as usize];
        b.loans += principal;
        match kind {
            LoanKind::Mortgage { .. } => b.mortgage_loans += principal,
            _ => b.business_loans += principal,
        }
    }
    // Deposit creation: borrower deposit +P at its bank; if the lender differs, the lender settles
    // in reserves with the borrower's bank.
    let borrower_bank = match borrower {
        Borrower::Household(h) => w.households[h as usize].bank,
        Borrower::Firm(f) => w.firms[f as usize].bank,
    };
    match borrower {
        Borrower::Household(h) => w.households[h as usize].deposits += principal,
        Borrower::Firm(f) => w.firms[f as usize].deposits += principal,
    }
    w.banks[borrower_bank as usize].deposits += principal;
    if borrower_bank != bank {
        w.banks[bank as usize].reserves -= principal;
        w.banks[borrower_bank as usize].reserves += principal;
    }
    match (borrower, kind) {
        (Borrower::Firm(f), _) => w.firms[f as usize].loans.push(id),
        (_, LoanKind::Mortgage { house }) => w.houses[house as usize].mortgage = Some(id),
        _ => {}
    }
    id
}

fn loan_bucket(b: &mut Bank, kind: LoanKind, delta: f64) {
    b.loans += delta;
    match kind {
        LoanKind::Mortgage { .. } => b.mortgage_loans += delta,
        _ => b.business_loans += delta,
    }
}

/// Borrower repays `amount` of principal (capped at outstanding). Destroys deposits. Removes the loan when paid off.
/// Returns the amount repaid.
pub fn repay_principal(w: &mut World, loan: LoanId, amount: f64) -> f64 {
    let Some(l) = w.loans.get(loan) else { return 0.0 };
    let x = amount.min(l.principal).max(0.0);
    if x <= 0.0 {
        return 0.0;
    }
    let (bank, borrower, kind) = (l.bank, l.borrower, l.kind);
    // borrower deposits leave its bank and go to the lender as reserves; lender's loan asset falls.
    debit(w, borrower.into(), x);
    w.banks[bank as usize].reserves += x;
    loan_bucket(&mut w.banks[bank as usize], kind, -x);
    let l = w.loans.get_mut(loan).unwrap();
    l.principal -= x;
    if l.principal <= 1e-6 {
        remove_loan(w, loan);
    }
    x
}

/// Pay interest on a loan from the borrower to the lending bank (bank income).
pub fn pay_interest(w: &mut World, loan: LoanId, amount: f64) {
    let Some(l) = w.loans.get(loan) else { return };
    let (bank, borrower) = (l.bank, l.borrower);
    pay(w, borrower.into(), Account::Bank(bank), amount);
    w.banks[bank as usize].interest_income += amount.max(0.0);
}

/// Write off the remaining principal (after any recovery was paid via repay_principal). Returns the loss.
pub fn write_off(w: &mut World, loan: LoanId) -> f64 {
    let Some(l) = w.loans.get(loan) else { return 0.0 };
    let (bank, kind, p) = (l.bank, l.kind, l.principal);
    loan_bucket(&mut w.banks[bank as usize], kind, -p);
    w.banks[bank as usize].loan_losses += p;
    w.flows.loan_losses += p;
    remove_loan(w, loan);
    p
}

fn remove_loan(w: &mut World, loan: LoanId) {
    if let Some(l) = w.loans.remove(loan) {
        match (l.borrower, l.kind) {
            (Borrower::Firm(f), _) => w.firms[f as usize].loans.retain(|&x| x != loan),
            (_, LoanKind::Mortgage { house }) => {
                if w.houses[house as usize].mortgage == Some(loan) {
                    w.houses[house as usize].mortgage = None;
                }
            }
            _ => {}
        }
    }
}

/// Change the borrower of a loan (e.g. inheritance). No money moves.
pub fn reassign_loan(w: &mut World, loan: LoanId, new_borrower: Borrower) {
    if let Some(l) = w.loans.get_mut(loan) {
        l.borrower = new_borrower;
    }
}

// ---------------------------------------------------------------- houses

/// Change a house's owner, keeping `properties` lists consistent. No money moves (pay separately).
pub fn transfer_house(w: &mut World, house: HouseId, new_owner: Owner) {
    let old = w.houses[house as usize].owner;
    if let Owner::Household(h) = old {
        w.households[h as usize].properties.retain(|&x| x != house);
    }
    if let Owner::Household(h) = new_owner {
        let p = &mut w.households[h as usize].properties;
        if !p.contains(&house) {
            p.push(house);
        }
    }
    w.houses[house as usize].owner = new_owner;
}

// ---------------------------------------------------------------- Fund shares

/// Household buys Fund shares worth `amount` $ at the current share price.
pub fn fund_buy(w: &mut World, h: HhId, amount: f64) {
    if !(amount > 0.0) || w.fund.share_price <= 0.0 {
        return;
    }
    pay(w, Account::Household(h), Account::Fund, amount);
    let units = amount / w.fund.share_price;
    w.households[h as usize].shares += units;
    w.fund.total_shares += units;
}

/// Household sells Fund shares worth up to `amount` $ (capped at holdings). Returns $ received.
pub fn fund_sell(w: &mut World, h: HhId, amount: f64) -> f64 {
    if !(amount > 0.0) || w.fund.share_price <= 0.0 {
        return 0.0;
    }
    let held = w.households[h as usize].shares;
    let units = (amount / w.fund.share_price).min(held);
    let value = units * w.fund.share_price;
    w.households[h as usize].shares -= units;
    w.fund.total_shares -= units;
    pay(w, Account::Fund, Account::Household(h), value);
    value
}

/// Move all Fund shares from one household to another (inheritance).
pub fn transfer_shares(w: &mut World, from: HhId, to: HhId) {
    let s = w.households[from as usize].shares;
    w.households[from as usize].shares = 0.0;
    w.households[to as usize].shares += s;
}

// ---------------------------------------------------------------- bonds

fn add_holding(issue: &mut BondIssue, h: BondHolder, face: f64) {
    if let Some(e) = issue.holdings.iter_mut().find(|(k, _)| *k == h) {
        e.1 += face;
    } else {
        issue.holdings.push((h, face));
    }
    issue.holdings.retain(|(_, f)| *f > 1e-9);
}

/// Government issues a new bond: `buyer` pays `price_per_face × face` to the Government. Returns index into w.bonds.
pub fn issue_bond(
    w: &mut World,
    tenor_months: u32,
    coupon: f64,
    buyer: BondHolder,
    face: f64,
    price_per_face: f64,
) -> usize {
    let id = w.next_bond_id;
    w.next_bond_id += 1;
    let tick = w.tick;
    w.bonds.push(BondIssue {
        id,
        tenor_months,
        coupon,
        issue_tick: tick,
        maturity_tick: tick + tenor_months,
        holdings: vec![(buyer, face)],
    });
    pay(w, buyer.into(), Account::Government, face * price_per_face);
    w.bonds.len() - 1
}

/// Add more face to an existing issue (re-opening) sold to `buyer`.
pub fn reopen_bond(w: &mut World, idx: usize, buyer: BondHolder, face: f64, price_per_face: f64) {
    add_holding(&mut w.bonds[idx], buyer, face);
    pay(w, buyer.into(), Account::Government, face * price_per_face);
}

/// Secondary-market trade: `seller` delivers `face` of bond `idx` to `buyer`, who pays `price_per_face × face`.
/// Face is capped at the seller's holding. Returns face actually traded.
pub fn bond_transfer(
    w: &mut World,
    idx: usize,
    seller: BondHolder,
    buyer: BondHolder,
    face: f64,
    price_per_face: f64,
) -> f64 {
    let held = w.bonds[idx].held_by(seller);
    let f = face.min(held).max(0.0);
    if f <= 0.0 || seller == buyer {
        return 0.0;
    }
    add_holding(&mut w.bonds[idx], seller, -f);
    add_holding(&mut w.bonds[idx], buyer, f);
    pay(w, buyer.into(), seller.into(), f * price_per_face);
    f
}

/// Government pays coupons on all bonds (monthly) and redeems matured ones. Returns total interest paid.
pub fn service_government_debt(w: &mut World) -> f64 {
    let tick = w.tick;
    let mut payments: Vec<(BondHolder, f64)> = Vec::new();
    let mut interest = 0.0;
    let mut row_coupon = 0.0;
    for b in &w.bonds {
        for (h, face) in &b.holdings {
            let c = face * b.coupon / 12.0;
            interest += c;
            if *h == BondHolder::RestOfWorld {
                row_coupon += c;
            }
            let principal = if b.maturity_tick <= tick { *face } else { 0.0 };
            payments.push((*h, c + principal));
        }
    }
    for (h, x) in payments {
        pay(w, Account::Government, h.into(), x);
    }
    w.bonds.retain(|b| b.maturity_tick > tick);
    crate::world::note_row_coupon(w, row_coupon);
    interest
}

// ---------------------------------------------------------------- central bank lending

pub fn cb_lend(w: &mut World, bank: BankId, amount: f64) {
    if amount <= 0.0 {
        return;
    }
    w.banks[bank as usize].reserves += amount;
    w.banks[bank as usize].cb_borrowing += amount;
    w.cb.discount_loans += amount;
}

pub fn cb_repay(w: &mut World, bank: BankId, amount: f64) -> f64 {
    let b = &mut w.banks[bank as usize];
    let x = amount.min(b.cb_borrowing).max(0.0);
    b.reserves -= x;
    b.cb_borrowing -= x;
    w.cb.discount_loans -= x;
    x
}

// ---------------------------------------------------------------- consistency

/// Verify stock-flow consistency. Returns an error string describing the first violation.
pub fn check_consistency(w: &World) -> Result<(), String> {
    let nb = w.banks.len();
    let mut dep = vec![0.0f64; nb];
    for h in &w.households {
        dep[h.bank as usize] += h.deposits;
    }
    for f in &w.firms {
        dep[f.bank as usize] += f.deposits;
    }
    dep[w.fund.bank as usize] += w.fund.deposits;
    dep[w.row.bank as usize] += w.row.deposits;
    let mut loans = vec![0.0f64; nb];
    for (_, l) in w.loans.iter() {
        loans[l.bank as usize] += l.principal;
    }
    for (i, b) in w.banks.iter().enumerate() {
        let tol = 1e-6 * (1.0 + b.deposits.abs());
        if (dep[i] - b.deposits).abs() > tol {
            return Err(format!(
                "bank {i}: deposit liabilities {} != sum of customer deposits {}",
                b.deposits, dep[i]
            ));
        }
        let tol = 1e-6 * (1.0 + b.loans.abs());
        if (loans[i] - b.loans).abs() > tol {
            return Err(format!("bank {i}: loan book {} != sum of loans {}", b.loans, loans[i]));
        }
        if !b.reserves.is_finite() || !b.deposits.is_finite() {
            return Err(format!("bank {i}: non-finite balance"));
        }
    }
    for (i, h) in w.households.iter().enumerate() {
        if !h.deposits.is_finite() || !h.shares.is_finite() {
            return Err(format!("household {i}: non-finite deposits/shares"));
        }
    }
    let shares: f64 = w.households.iter().map(|h| h.shares).sum();
    if (shares - w.fund.total_shares).abs() > 1e-6 * (1.0 + shares.abs()) {
        return Err(format!(
            "fund shares {} != sum of household shares {}",
            w.fund.total_shares, shares
        ));
    }
    for (_, l) in w.loans.iter() {
        if let LoanKind::Mortgage { house } = l.kind {
            if w.houses[house as usize].mortgage != Some(l.id) {
                return Err(format!("mortgage {} not linked from house {}", l.id, house));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- batched household payments

/// Destination of a household payment in a [`PayBatch`].
#[derive(Clone, Copy, Debug)]
pub enum Payee {
    Firm(usize),
    RestOfWorld,
    Government,
}

/// Deterministic batch of household outflows, for parallel payment loops.
///
/// Each parallel worker owns a fixed chunk of households. It debits each household's own deposits directly
/// ([`PayBatch::hh_pay`]) and accumulates everything that other accounts receive into per-entity deltas. The
/// per-chunk batches are then merged in chunk order ([`PayBatch::merge`]) and applied once ([`PayBatch::apply`]).
///
/// Stock-flow consistency: every household payment debits the household's deposits and its bank's deposit
/// liability and reserves by the same amount, and credits the payee (firm / RoW deposits, bank deposit liability
/// and reserves of the payee's bank, or TGA). `apply` performs exactly these updates, with the same guards as
/// the single-payment functions (non-positive or non-finite amounts are ignored). Only the floating-point
/// summation order differs from the equivalent sequence of `pay_hh_to_*` calls.
#[derive(Clone, Debug)]
pub struct PayBatch {
    /// Net deposit receipts per firm.
    pub firm: Vec<f64>,
    /// Net deposit/reserve change per bank (both move identically for every household payment).
    pub bank: Vec<f64>,
    /// Total received by the Rest of World.
    pub row: f64,
    /// Total received by the government (TGA).
    pub gov: f64,
    row_bank: usize,
}

impl PayBatch {
    pub fn new(n_firms: usize, n_banks: usize, row_bank: usize) -> Self {
        PayBatch { firm: vec![0.0; n_firms], bank: vec![0.0; n_banks], row: 0.0, gov: 0.0, row_bank }
    }

    /// Pay `x` from household `hh` to `to`, debiting the household immediately. Returns the amount paid
    /// (0 if the amount was rejected by the guards).
    #[inline]
    pub fn hh_pay(&mut self, hh: &mut Household, to: Payee, x: f64) -> f64 {
        if !(x > 0.0) || !x.is_finite() {
            return 0.0;
        }
        hh.deposits -= x;
        self.bank[hh.bank as usize] -= x;
        match to {
            Payee::Firm(f) => self.firm[f] += x,
            Payee::RestOfWorld => {
                self.row += x;
                self.bank[self.row_bank] += x;
            }
            Payee::Government => self.gov += x,
        }
        x
    }

    /// Add another batch (same dimensions) into this one.
    pub fn merge(&mut self, o: &PayBatch) {
        for (a, b) in self.firm.iter_mut().zip(&o.firm) {
            *a += b;
        }
        for (a, b) in self.bank.iter_mut().zip(&o.bank) {
            *a += b;
        }
        self.row += o.row;
        self.gov += o.gov;
    }

    /// Apply the accumulated receipts to firms, banks, the RoW, and the TGA. Household debits were already applied
    /// by [`PayBatch::hh_pay`].
    pub fn apply(&self, w: &mut World) {
        for (f, &x) in self.firm.iter().enumerate() {
            if x != 0.0 {
                let fm = &mut w.firms[f];
                fm.deposits += x;
                let b = fm.bank as usize;
                w.banks[b].deposits += x;
                w.banks[b].reserves += x;
            }
        }
        for (b, &x) in self.bank.iter().enumerate() {
            // `bank` holds household debits and RoW credits; firm receipts were booked to their banks above.
            if x != 0.0 {
                w.banks[b].deposits += x;
                w.banks[b].reserves += x;
            }
        }
        if self.row != 0.0 {
            w.row.deposits += self.row;
        }
        if self.gov != 0.0 {
            w.cb.tga += self.gov;
        }
    }
}
