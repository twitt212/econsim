//! A small, internally consistent world for unit tests (shared test helper).
//! `init::build_world` is the calibrated production initialiser; this one is deliberately simple.

use rand::Rng;

use crate::common::{self, human_capital};
use crate::config::Config;
use crate::ledger::{self, Account};
use crate::rng::{self, stream};
use crate::types::*;

/// The pre-re-fit defaults (a5296db `Config::default()` with p1 off): `Config::default()` merged with
/// `scripts/profiles/legacy_p1off.json`, exactly as `Config::from_env` merges an `ECONSIM_CFG` overlay.
/// Used only by tests that assert a property of the pre-re-fit defaults.
pub fn legacy_p1off_config() -> Config {
    let mut merged = serde_json::to_value(Config::default()).expect("Config serialises");
    let over: serde_json::Value =
        serde_json::from_str(include_str!("../../../scripts/profiles/legacy_p1off.json")).expect("legacy_p1off.json parses");
    let m = merged.as_object_mut().expect("Config is an object");
    for (k, v) in over.as_object().expect("legacy_p1off.json is an object") {
        assert!(m.contains_key(k), "unknown config key {k}");
        m.insert(k.clone(), v.clone());
    }
    serde_json::from_value(merged).expect("legacy_p1off does not fit Config")
}

/// Wage per efficiency unit per month at t=0.
pub const BASE_WAGE_OFFER: f64 = 4_700.0;

/// Capital/output ratio in months of gross output at full capacity (US private non-residential fixed capital ≈ 1×GDP
/// ≈ 7 months of gross output; BEA Fixed Assets).
pub const CAPITAL_OUTPUT_MONTHS: f64 = 8.0;

/// Share of revenue absorbed by depreciation and business property tax at target utilisation.
pub fn depreciation_share(cfg: &Config) -> f64 {
    (cfg.depreciation_annual + cfg.business_property_tax) / 12.0 * CAPITAL_OUTPUT_MONTHS / cfg.target_utilization
}

/// Re-fit Step 4c: capital/output months of sector `s`. For Energy with the upstream split on, the upstream capital
/// share κ of the domestic crude value (`crude_intensity × (1 − oil_import_share)` of Energy revenue at the base oil
/// price) is Energy's own capital cost: it adds κ·crude_intensity·(1 − s)·12·target_utilization / (depreciation +
/// business property tax) months to the capital stock. At the defaults that is 8 + 16.6 = 24.6 months, conservative
/// against BEA mining and utilities capital per unit of output. Otherwise `CAPITAL_OUTPUT_MONTHS`.
pub fn capital_output_months(cfg: &Config, s: Sector) -> f64 {
    match (s, cfg.upstream_split()) {
        (Sector::Energy, Some((_, _, _, kappa))) => {
            CAPITAL_OUTPUT_MONTHS
                + kappa * cfg.crude_intensity * (1.0 - cfg.oil_import_share) * 12.0 * cfg.target_utilization
                    / (cfg.depreciation_annual + cfg.business_property_tax)
        }
        _ => CAPITAL_OUTPUT_MONTHS,
    }
}

/// Re-fit Step 4c: `depreciation_share` for sector `s` (the Energy value with the upstream split on).
pub fn depreciation_share_for(cfg: &Config, s: Sector) -> f64 {
    (cfg.depreciation_annual + cfg.business_property_tax) / 12.0 * capital_output_months(cfg, s) / cfg.target_utilization
}

/// Labour productivity that makes price = 1 at the initial markup given the IO table,
/// after inputs, crude, depreciation and ~1% interest costs.
pub fn calibrated_labor_productivity(cfg: &Config, s: Sector) -> f64 {
    let j = s.idx();
    let inputs: f64 = (0..N_SECTORS).map(|i| cfg.io[i][j]).sum();
    let crude = if s == Sector::Energy { crate::common::energy_crude_unit_cost(cfg, 1.0) } else { 0.0 };
    let labor_share = 1.0 / (1.0 + cfg.initial_markup[j]) - inputs - crude - depreciation_share_for(cfg, s) - 0.01;
    BASE_WAGE_OFFER * (1.0 + cfg.payroll_tax + cfg.employer_benefits) / labor_share.max(0.05)
}

/// Build a small consistent world. Uses `cfg.n_households` (tests typically use 300–2000).
pub fn tiny_world(cfg: Config, seed: u64) -> World {
    let mut w = World::empty(cfg, seed);
    // Built directly, not through `init::build_world`: there is no burn-in, so demography runs from month 0.
    w.cfg.burn_in_months = 0;
    let mut r = rng::module_rng(seed, 0, stream::INIT);
    let n = w.cfg.n_households.max(50);
    let nb = w.cfg.n_banks.max(1);

    // banks
    for b in 0..nb {
        w.banks.push(Bank {
            id: b as BankId,
            alive: true,
            ..Default::default()
        });
    }
    w.fund.bank = 0;
    w.row.bank = 0;

    // prices, curve, exog
    w.prices.sector = [1.0; N_SECTORS];
    w.prices.cpi = 100.0;
    w.prices.core_cpi = 100.0;
    w.prices.wage_index = 100.0;
    w.prices.hpi = w.cfg.median_house_price;
    w.prices.rent_index = w.prices.hpi * w.cfg.gross_rental_yield / 12.0;
    w.curve.yields = w.cfg.initial_curve;
    w.curve.term_premium = w.cfg.tp_base;
    w.cb.policy_rate = w.cfg.initial_policy_rate;
    w.exog.oil_base = w.cfg.oil_base;
    w.exog.oil_price = w.cfg.oil_base;
    w.exog.tfp = [1.0; N_SECTORS];
    w.fund.share_price = 1.0;

    // firms by employment share
    let nf = ((n as f64 / w.cfg.households_per_firm).round() as usize).max(N_SECTORS * 2);
    let mut firm_sectors = Vec::with_capacity(nf);
    for s in Sector::ALL {
        let k = ((nf as f64 * w.cfg.employment_share[s.idx()]).round() as usize).max(2);
        for _ in 0..k {
            firm_sectors.push(s);
        }
    }
    for (i, &s) in firm_sectors.iter().enumerate() {
        let lp = calibrated_labor_productivity(&w.cfg, s);
        w.firms.push(Firm {
            id: i as FirmId,
            sector: s,
            alive: true,
            age_months: r.gen_range(12..240),
            months_dead: 0,
            bank: (i % nb) as BankId,
            deposits: 0.0,
            loans: Vec::new(),
            workers: Vec::new(),
            owner: None,
            labor_productivity: lp,
            capital: 0.0,
            capital_productivity: 0.0,
            wage_offer: BASE_WAGE_OFFER,
            vacancies: 0,
            openings: 0,
            planned_layoffs: 0,
            target_output: 0.0,
            price: 1.0,
            markup: w.cfg.initial_markup[s.idx()],
            months_since_reprice: 0,
            unit_cost: 1.0 / (1.0 + w.cfg.initial_markup[s.idx()]),
            inventory: 0.0,
            output: 0.0,
            sales: 0.0,
            demand: 0.0,
            expected_demand: 0.0,
            backlog: 0.0,
            revenue: 0.0,
            wage_bill: 0.0,
            input_cost: 0.0,
            interest_paid: 0.0,
            depreciation_cost: 0.0,
            taxes: 0.0,
            profit: 0.0,
            dividends: 0.0,
            profit_rate: 0.08,
            investment_order: 0.0,
            months_distressed: 0,
            k_star_smooth: 0.0,
        });
    }
    let nf = w.firms.len();
    let sector_firms: Vec<Vec<FirmId>> = Sector::ALL
        .iter()
        .map(|s| w.firms.iter().filter(|f| f.sector == *s).map(|f| f.id).collect())
        .collect();

    // households
    for i in 0..n {
        let age_years: f64 = 18.0 + r.gen::<f64>().powf(1.1) * 70.0;
        let u: f64 = r.gen();
        let education = if u < 0.55 {
            Education::HighSchool
        } else if u < 0.9 {
            Education::College
        } else {
            Education::Graduate
        };
        let mut pref = [0u32; N_CONSUMER];
        for s in 0..N_CONSUMER {
            let v = &sector_firms[s];
            pref[s] = v[r.gen_range(0..v.len())];
        }
        w.households.push(Household {
            id: i as HhId,
            alive: true,
            age_months: (age_years * 12.0) as u16,
            partnered: false,
            education,
            skill: rng::lognormal_mean1(&mut r, 0.35) as f32,
            status: LaborStatus::Unemployed,
            employer: None,
            wage: 0.0,
            last_wage: 0.0,
            months_unemployed: 0,
            bls_unemployed: 0,
            months_in_job: 0,
            months_employed: 0,
            reservation_wage: w.cfg.min_wage_monthly as f32,
            benefit_months_left: 0,
            career_earnings: 0.0,
            career_months: 0,
            pension: 0.0,
            bank: (i % nb) as BankId,
            deposits: 0.0,
            shares: 0.0,
            tenure: Tenure::Renter,
            home: None,
            properties: Vec::new(),
            rent: 0.0,
            lease_months_left: 0,
            housing_cost: 0.0,
            permanent_income: 0.0,
            expected_inflation: w.cfg.initial_inflation,
            consumption_budget: 0.0,
            preferred_firm: pref,
        });
        w.hh_month.push(HhMonth::default());
    }

    // labour status & employment (sector shares)
    let mut cum = [0.0; N_SECTORS];
    let mut acc = 0.0;
    for s in 0..N_SECTORS {
        acc += w.cfg.employment_share[s];
        cum[s] = acc;
    }
    let wage_index = w.prices.wage_index;
    for i in 0..n {
        let age = w.households[i].age_years();
        let edu = w.households[i].education;
        let status = if age >= 67.0 {
            LaborStatus::Retired
        } else if (edu == Education::College && age < 22.0) || (edu == Education::Graduate && age < 24.0) {
            LaborStatus::Student
        } else if r.gen::<f64>() < w.cfg.initial_unemployment {
            LaborStatus::Unemployed
        } else {
            LaborStatus::Employed
        };
        let hc = human_capital(&w.cfg, &w.households[i]);
        let hh = &mut w.households[i];
        let years_worked = (age - 20.0).clamp(0.0, 45.0);
        hh.career_months = (years_worked * 12.0) as u16;
        hh.career_earnings = BASE_WAGE_OFFER * hc * hh.career_months as f64 * 0.8;
        match status {
            LaborStatus::Employed => {
                let u: f64 = r.gen();
                let s = cum.iter().position(|&c| u <= c).unwrap_or(N_SECTORS - 1);
                let v = &sector_firms[s];
                let f = v[r.gen_range(0..v.len())];
                let wage = (BASE_WAGE_OFFER * hc).max(w.cfg.min_wage_monthly);
                common::hire_worker(&mut w, i as HhId, f, wage);
                let hh = &mut w.households[i];
                hh.months_in_job = r.gen_range(0..60);
                hh.months_employed = hh.months_in_job;
                hh.permanent_income = wage * 0.78;
            }
            LaborStatus::Retired => {
                let p = common::social_security_pia(hh.career_earnings, hh.career_months.into(), wage_index);
                hh.status = LaborStatus::Retired;
                hh.pension = p;
                hh.permanent_income = p;
            }
            other => {
                hh.status = other;
                hh.last_wage = BASE_WAGE_OFFER * hc;
                hh.permanent_income = 1_500.0;
            }
        }
    }

    // firm capital & expected demand consistent with workforce
    for f in 0..nf {
        let eff: f64 = w.firms[f]
            .workers
            .iter()
            .map(|&h| human_capital(&w.cfg, &w.households[h as usize]))
            .sum();
        let fm = &mut w.firms[f];
        let y = eff.max(0.5) * fm.labor_productivity;
        fm.expected_demand = y;
        fm.target_output = y;
        fm.capital_productivity = 1.0 / capital_output_months(&w.cfg, fm.sector);
        fm.capital = y / (fm.capital_productivity * w.cfg.target_utilization);
        fm.inventory = y * w.cfg.inventory_target[fm.sector.idx()];
    }

    // houses: owners & renters
    let nh_houses = (n as f64 * w.cfg.houses_per_household) as usize;
    for k in 0..nh_houses {
        let q = rng::lognormal_mean1(&mut r, 0.45);
        w.houses.push(House {
            id: k as HouseId,
            quality: q,
            owner: Owner::Bank(0),
            occupant: None,
            rent: 0.0,
            for_sale: false,
            ask_price: 0.0,
            months_listed: 0,
            for_rent: false,
            last_sale_price: 0.0,
            mortgage: None,
            completed: true,
        });
    }
    // median quality → hpi so that median value = median price
    let mut qs: Vec<f64> = w.houses.iter().map(|h| h.quality).collect();
    qs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med_q = qs[qs.len() / 2];
    w.prices.hpi = w.cfg.median_house_price / med_q;
    w.prices.rent_index = w.prices.hpi * w.cfg.gross_rental_yield / 12.0;

    let mut next_house = 0usize;
    let investors: Vec<HhId> = (0..n as HhId).filter(|_| r.gen::<f64>() < 0.08).collect();
    for i in 0..n {
        if next_house >= nh_houses {
            break;
        }
        let age = w.households[i].age_years();
        let own_p = match age as u32 {
            0..=24 => 0.25,
            25..=34 => 0.37,
            35..=44 => 0.62,
            45..=54 => 0.70,
            55..=64 => 0.75,
            _ => 0.79,
        };
        if age < 24.0 && r.gen::<f64>() < 0.5 {
            w.households[i].tenure = Tenure::WithParents;
            continue;
        }
        let house = next_house as HouseId;
        next_house += 1;
        if r.gen::<f64>() < own_p {
            ledger::transfer_house(&mut w, house, Owner::Household(i as HhId));
            w.houses[house as usize].occupant = Some(i as HhId);
            let hh = &mut w.households[i];
            hh.tenure = Tenure::Owner;
            hh.home = Some(house);
        } else {
            let mut landlord = if investors.is_empty() {
                0
            } else {
                investors[r.gen_range(0..investors.len())]
            };
            // A tenant is never their own landlord (the draw is kept; only the self-match is moved on).
            if landlord == i as HhId {
                landlord = (landlord + 1) % n as HhId;
            }
            ledger::transfer_house(&mut w, house, Owner::Household(landlord));
            let rent = w.prices.rent_index * w.houses[house as usize].quality;
            w.houses[house as usize].occupant = Some(i as HhId);
            w.houses[house as usize].rent = rent;
            let hh = &mut w.households[i];
            hh.tenure = Tenure::Renter;
            hh.home = Some(house);
            hh.rent = rent;
            hh.lease_months_left = r.gen_range(1..=12);
        }
    }
    // remaining houses vacant, owned by investors or listed by bank 0
    for k in next_house..nh_houses {
        if !investors.is_empty() {
            let landlord = investors[r.gen_range(0..investors.len())];
            ledger::transfer_house(&mut w, k as HouseId, Owner::Household(landlord));
            w.houses[k].for_rent = true;
        }
    }

    // money endowments (created by the CB as an initial-condition device)
    for i in 0..n {
        let pi = w.households[i].permanent_income.max(1_000.0);
        let age = w.households[i].age_years();
        let dep = pi * (2.0 + age / 10.0) * rng::lognormal_mean1(&mut r, 0.8);
        ledger::pay(&mut w, Account::CentralBank, Account::Household(i as HhId), dep);
        let fund = if r.gen::<f64>() < 0.6 {
            pi * (age - 18.0).max(0.0) * 0.5 * rng::lognormal_mean1(&mut r, 1.2)
        } else {
            0.0
        };
        if fund > 0.0 {
            ledger::pay(&mut w, Account::CentralBank, Account::Household(i as HhId), fund);
            ledger::fund_buy(&mut w, i as HhId, fund);
        }
    }
    for f in 0..nf {
        let costs = w.firms[f].workers.len() as f64 * BASE_WAGE_OFFER * 2.0 + 5_000.0;
        ledger::pay(&mut w, Account::CentralBank, Account::Firm(f as FirmId), costs * 3.0);
    }
    ledger::pay(&mut w, Account::CentralBank, Account::RestOfWorld, n as f64 * 2_000.0);
    ledger::pay(&mut w, Account::CentralBank, Account::Government, n as f64 * 300.0);
    for b in 0..nb {
        ledger::pay(&mut w, Account::CentralBank, Account::Bank(b as BankId), n as f64 * 3_000.0 / nb as f64);
    }

    // mortgages for 62% of owners
    for i in 0..n {
        let Some(house) = w.households[i].home else { continue };
        if w.households[i].tenure != Tenure::Owner || r.gen::<f64>() > 0.62 {
            continue;
        }
        let v = common::house_value(&w, house);
        let principal = v * r.gen_range(0.3..0.8);
        let rate = r.gen_range(0.03..0.07);
        let bank = w.households[i].bank;
        let term = w.cfg.mortgage_term_months;
        let id = ledger::create_loan(
            &mut w,
            bank,
            Borrower::Household(i as HhId),
            LoanKind::Mortgage { house },
            principal,
            rate,
            term,
        );
        // created loans credit deposits; the house was already "paid for": move the cash to the bank
        // as if spent on the house in the past (keeps deposits realistic).
        ledger::pay(&mut w, Account::Household(i as HhId), Account::Bank(bank), principal);
        let l = w.loans.get_mut(id).unwrap();
        l.months_remaining = r.gen_range(120..=360);
        l.payment = annuity_payment(l.principal, l.annual_rate, l.months_remaining);
    }

    // government bonds: issue across tenors to banks, Fund, CB, RoW
    let gdp_month: f64 = w
        .households
        .iter()
        .map(|h| h.wage)
        .sum::<f64>()
        * 1.6;
    let debt = gdp_month * 12.0 * 0.6;
    let holders = [
        (BondHolder::CentralBank, 0.2),
        (BondHolder::Fund, 0.3),
        (BondHolder::RestOfWorld, 0.2),
    ];
    for (ti, &tenor) in TENORS.iter().enumerate() {
        let coupon = w.curve.yields[ti];
        for &(h, sh) in &holders {
            let face = debt * sh * w.cfg.debt_mix()[ti];
            // endow buyer then buy
            if h != BondHolder::CentralBank {
                ledger::pay(&mut w, Account::CentralBank, h.into(), face);
            }
            let idx = ledger::issue_bond(&mut w, tenor, coupon, h, face, 1.0);
            w.bonds[idx].maturity_tick = (tenor / 2).max(1);
        }
        for b in 0..nb {
            let face = debt * 0.3 * w.cfg.debt_mix()[ti] / nb as f64;
            ledger::pay(&mut w, Account::CentralBank, Account::Bank(b as BankId), face);
            let idx = ledger::issue_bond(&mut w, tenor, coupon, BondHolder::Bank(b as BankId), face, 1.0);
            w.bonds[idx].maturity_tick = (tenor / 2).max(1);
        }
    }
    w.gov.debt_face = w.bonds.iter().map(|b| b.face_outstanding()).sum();
    for b in 0..nb {
        let v: f64 = w.bonds.iter().map(|x| x.held_by(BondHolder::Bank(b as BankId))).sum();
        w.banks[b].bonds_value = v;
    }
    w.cb.bonds_value = w.bonds.iter().map(|x| x.held_by(BondHolder::CentralBank)).sum();
    w.fund.bonds_value = w.bonds.iter().map(|x| x.held_by(BondHolder::Fund)).sum();
    w.row.bonds_value = w.bonds.iter().map(|x| x.held_by(BondHolder::RestOfWorld)).sum();
    w.fund.nav = w.fund.deposits + w.fund.bonds_value;
    w.fund.share_price = if w.fund.total_shares > 0.0 {
        w.fund.nav / w.fund.total_shares
    } else {
        1.0
    };

    w.prices.cpi_history = vec![100.0 / (1.0 + w.cfg.initial_inflation).powf(1.0 / 12.0), 100.0];
    w.prices.core_history = w.prices.cpi_history.clone();
    w.prices.core_pce_history = w.prices.cpi_history.clone();
    w.prices.hpi_history = vec![w.prices.hpi];
    // b8-world H12: after the last seeding step, as in build_world (the block is off unless configured).
    crate::world::init_if_enabled(&mut w);
    w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiny_world_is_consistent() {
        let cfg = Config {
            n_households: 400,
            ..Default::default()
        };
        let w = tiny_world(cfg, 7);
        ledger::check_consistency(&w).unwrap();
        assert!(w.households.iter().any(|h| h.status == LaborStatus::Employed));
        assert!(w.loans.len() > 10);
    }

    #[test]
    fn runs_with_stubs() {
        let cfg = Config {
            n_households: 300,
            ..Default::default()
        };
        let mut w = tiny_world(cfg, 1);
        crate::sim::run(&mut w, 3);
        assert_eq!(w.history.len(), 3);
    }
}
