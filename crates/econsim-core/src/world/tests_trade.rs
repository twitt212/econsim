//! Stage-2 tests of the trade block (spec 3, Stage 2, "Tests"). The hand values are the golden `trade_hand.json` (written by
//! `scripts/world/trade_hand.py`). The tests drive `TradeState` directly (with the blocs at zero unless stated) or step a tiny
//! world with the block on and the link on or off.

use super::bloc::BlocState;
use super::params::WorldParams;
use super::trade::{TariffComp, TradeState, ENERGY, GAP_CLAMP, S};
use super::NB;
use crate::common;
use crate::config::Config;
use crate::fiscal;
use crate::ledger;
use crate::shocks::{ScheduledShock, Shock, ShockState};
use crate::sim;
use crate::testkit;
use crate::types::{role_mean, World};
use serde_json::Value;

const GOLD_TRADE: &str = include_str!("data/golden/trade_hand.json");

/// Dollar +10% as a log step: z~ = 100 ln 1.10 (spec 2.5, `trade_hand.py` Z10).
const Z10: f64 = 9.531_017_980_432_49;

fn golden() -> Value {
    serde_json::from_str(GOLD_TRADE).expect("trade_hand.json parses")
}

fn gvec(v: &Value) -> Vec<f64> {
    v.as_array().expect("array").iter().map(|x| x.as_f64().expect("number")).collect()
}

fn zeros() -> [BlocState; NB] {
    std::array::from_fn(|_| BlocState::default())
}

/// A tiny world with the block on and the trade link as given (no burn-in: the first month is reported).
fn tiny_block(link: bool, households: usize, seed: u64) -> World {
    let cfg = Config { world_block_enabled: true, world_link_trade: link, n_households: households, ..Default::default() };
    testkit::tiny_world(cfg, seed)
}

/// A world with a real burn-in (`init::build_world`, D36): the block on and the link as given, `burn_in` months, and the world
/// goods inflation given (a perturbation of the burn-in prices, the same for link on and off).
fn burn_in_world(link: bool, households: usize, burn_in: u32, seed: u64, goods_infl: f64) -> World {
    let cfg = Config {
        world_block_enabled: true,
        world_link_trade: link,
        n_households: households,
        burn_in_months: burn_in,
        world_goods_inflation: goods_infl,
        ..Default::default()
    };
    crate::init::build_world(cfg, seed)
}

fn state() -> TradeState {
    TradeState::new(&Config::default(), &WorldParams::load())
}

/// A state with the origin substitution switched off (omega = omega0), as the dollar pass-through test requires.
fn state_no_substitution() -> TradeState {
    let mut tr = state();
    tr.par.sigma_sr = 0.0;
    tr.par.sigma_lr = 0.0;
    tr
}

/// Runs `tr.post_bloc` once with the world as given (the world is not stepped, so prices and the calendar stay put).
fn post(tr: &mut TradeState, w: &World, blocs: &[BlocState; NB]) -> super::trade::TradeOut {
    tr.post_bloc(w, blocs, None)
}

// ---------------------------------------------------------------- pass-through, shares and the origin price

#[test]
fn dcp_pass_through_hand_values() {
    // W2-04 / spec 2.5: with z~_j = 100 for every bloc (c_t = 100, blocs at zero), the non-energy origin price is
    // -r = theta-bar phi_d + (1 - theta-bar) phi_nd, with phi_d = 0.47 (1 - 0.92^n) and phi_nd = 0.95 (1 - 0.10^n), n = h + 1.
    // The sector-weighted mean of the block's origin prices equals the golden `pass_through_path[h]` (h = 0..60).
    let g = golden();
    let pass = gvec(&g["pass_through_path"]);
    let wp = WorldParams::load();
    let w = tiny_block(false, 300, 41);
    let mut tr = state_no_substitution();
    tr.capture(&w, &zeros());
    let mut w = w;
    w.row.fx *= 1.0f64.exp();
    let blocs = zeros();
    // Import-weighted origin weights of the non-energy sectors: the sector-weighted mean of the origin prices equals their mean.
    let imp = &wp.trade.imports_base;
    let col: Vec<f64> = (0..S).map(|s| (0..NB).map(|j| imp[j][s]).sum()).collect();
    let nonenergy: f64 = col[1..].iter().sum();
    for h in 0..61 {
        let out = post(&mut tr, &w, &blocs);
        let mean_rbar: f64 = (1..S).map(|s| col[s] / nonenergy * out.imp_rel[s].ln()).sum();
        let got = -mean_rbar;
        assert!((got - pass[h]).abs() < 1e-9, "h {h}: block {got} golden {}", pass[h]);
    }
    // The bands of W2-04 at 3, 12 and 24 months (the state after h+1 updates): 0.220, 0.379, 0.469.
    assert!((pass[3] - 0.220).abs() < 5e-4 && (pass[12] - 0.379).abs() < 5e-4 && (pass[24] - 0.469).abs() < 5e-4);
}

#[test]
fn origin_shares_sum_to_one_and_equal_base_at_t0() {
    // Shares sum to one by sector, and with zero shocks the shares equal the base shares of the 2024 imports (W2-12 input).
    let w = tiny_block(false, 300, 42);
    let mut tr = state();
    tr.capture(&w, &zeros());
    let _ = post(&mut tr, &w, &zeros());
    for s in 0..S {
        let sum: f64 = (0..NB).map(|j| tr.omega[j][s]).sum();
        assert!((sum - 1.0).abs() < 1e-12, "sector {s} sums to {sum}");
        for j in 0..NB {
            assert!((tr.omega[j][s] - tr.tab.omega0[j][s]).abs() < 1e-12, "origin {j} sector {s}");
        }
    }
}

#[test]
fn origin_substitution_profile() {
    // Effective origin elasticity after h months of a constant origin gap: sigma_SR + (sigma_LR - sigma_SR)(1 - (1 - speed)^h)
    // (the lagged state zw of spec 2.5). At 12 months 1.19, at 60 months 1.93 (spec 2.5, Boehm et al. profile).
    let p = state().par;
    let x = 1.0;
    let mut zw = 0.0;
    let mut profile = Vec::new();
    for _ in 1..=60u32 {
        zw += p.origin_speed * (x - zw);
        profile.push(p.sigma_sr + (p.sigma_lr - p.sigma_sr) * zw / x);
    }
    let at12 = profile[11];
    let at60 = profile[59];
    assert!((at12 - 1.19).abs() < 0.01, "12 months: {at12}");
    assert!((at60 - 1.93).abs() < 0.01, "60 months: {at60}");
}

#[test]
fn energy_origin_neutral() {
    // Energy carries no origin price (r = 0): with a common dollar move the energy import price is the PT ratio alone and its
    // origin shares do not move (spec 2.5).
    let mut w = tiny_block(false, 300, 43);
    let mut tr = state();
    tr.capture(&w, &zeros());
    w.row.fx *= 0.5f64.exp();
    let out = post(&mut tr, &w, &zeros());
    assert_eq!(out.imp_rel[ENERGY], 1.0, "energy import price is the PT ratio (one) at the base");
    for j in 0..NB {
        assert!((tr.omega[j][ENERGY] - tr.tab.omega0[j][ENERGY]).abs() < 1e-12, "energy origin {j}");
    }
    // The non-energy sectors do move.
    assert!(out.imp_rel[2] != 1.0);
}

// ---------------------------------------------------------------- tariffs

#[test]
fn tariff_eff_uniform_equals_b7_scalar() {
    // A uniform set-level tariff gives tau_eff_hh = tau_eff_firm = w.shocks.tariff exactly (identical to b7).
    let mut w = tiny_block(false, 300, 44);
    w.shocks.tariff = 0.1;
    let mut tr = state();
    tr.capture(&w, &zeros());
    let out = post(&mut tr, &w, &zeros());
    assert_eq!(out.tau_eff_hh, 0.1);
    assert_eq!(out.tau_eff_firm, 0.1);
    assert_eq!(out.tau_by_origin[0], 0.1);
}

#[test]
fn tariff_eff_partner_weighted() {
    // A bilateral 25% duty on China (all sectors) adds to the set level and enters the channel by origin weights.
    let w = tiny_block(false, 300, 45);
    let mut tr = state();
    tr.capture(&w, &zeros());
    tr.tariffs.push(TariffComp { partner: Some(1), sector: None, tau: 0.25, retaliate: false });
    let out = post(&mut tr, &w, &zeros());
    let cov_hh: Vec<bool> = (0..S).map(|s| w.cfg.cpi_tradable_weight[s] != 0.0).collect();
    let mut num = 0.0;
    let mut den = 0.0;
    for s in 0..S {
        if cov_hh[s] {
            den += tr.m_prev[s];
            num += tr.m_prev[s] * tr.omega[1][s] * 0.25;
        }
    }
    let expect = num / den;
    assert!((out.tau_eff_hh - expect).abs() < 1e-12, "{} vs {expect}", out.tau_eff_hh);
    assert!(out.tau_eff_hh > 0.0 && out.tau_eff_hh < 0.25);
    assert!((out.tau_by_origin[1] - 0.25).abs() < 1e-12, "China pays the full bilateral duty");
    assert!(out.tau_by_origin[0].abs() < 1e-12, "EA pays none");
    // The import price of China's own origin rises relative to the others (its duty-inclusive price is higher).
    assert!(tr.omega[1][2] < tr.tab.omega0[1][2], "origin substitution moves share away from the tariffed origin");
}

#[test]
fn tariff_eff_channels_use_own_cov() {
    // A duty on Food only: Food is outside the household channel (no CPI tradable weight) and inside the firm channel.
    let w = tiny_block(false, 300, 46);
    let mut tr = state();
    tr.capture(&w, &zeros());
    tr.tariffs.push(TariffComp { partner: None, sector: Some(1), tau: 0.25, retaliate: false });
    let out = post(&mut tr, &w, &zeros());
    assert_eq!(out.tau_eff_hh, 0.0, "household channel excludes Food");
    let firm_den: f64 = (0..S).filter(|&s| w.cfg.tradable_weight[s] != 0.0).map(|s| tr.m_prev[s]).sum();
    let expect = 0.25 * tr.m_prev[1] / firm_den;
    assert!((out.tau_eff_firm - expect).abs() < 1e-12, "firm channel includes Food: {} vs {expect}", out.tau_eff_firm);
}

#[test]
fn import_tariff_level_is_set_not_added() {
    // D34: ImportTariff replaces the set-level level; a bilateral Tariff is recorded as a world shock and adds on top in the block.
    let mut st = ShockState::default();
    st.schedule(ScheduledShock { at_month: 0, shock: Shock::ImportTariff { pct: 10.0 } });
    st.activate_due(0);
    st.schedule(ScheduledShock { at_month: 1, shock: Shock::ImportTariff { pct: 0.0 } });
    st.activate_due(1);
    assert_eq!(st.tariff, 0.0, "ImportTariff 10 then 0 leaves 0");
    st.schedule(ScheduledShock {
        at_month: 2,
        shock: Shock::Tariff { partner: Some("CN".to_string()), sector: None, pct: 25.0, retaliate: false },
    });
    st.activate_due(2);
    assert_eq!(st.tariff, 0.0, "a bilateral component does not touch the set-level level");
    assert_eq!(st.world_shocks.len(), 1, "the bilateral duty is pushed to the block");
}

#[test]
fn tariff_read_sites_use_tau_for() {
    // H8: with the link on, the read sites take the block's effective duty; with it off, the literal b7 scalar (zero here).
    let mut on = tiny_block(true, 300, 47);
    on.shocks.schedule(ScheduledShock {
        at_month: 0,
        shock: Shock::Tariff { partner: Some("CN".to_string()), sector: None, pct: 25.0, retaliate: false },
    });
    for _ in 0..2 {
        sim::step(&mut on);
    }
    let wb = on.world.as_deref().expect("block on");
    assert!(wb.out.tau_eff_hh > 0.0);
    assert_eq!(crate::world::tau_for(&on, crate::world::Channel::Hh), wb.out.tau_eff_hh);
    assert_eq!(crate::world::tau_for(&on, crate::world::Channel::Firm), wb.out.tau_eff_firm);
    let mut off = tiny_block(false, 300, 47);
    off.shocks.schedule(ScheduledShock {
        at_month: 0,
        shock: Shock::Tariff { partner: Some("CN".to_string()), sector: None, pct: 25.0, retaliate: false },
    });
    for _ in 0..2 {
        sim::step(&mut off);
    }
    assert_eq!(crate::world::tau_for(&off, crate::world::Channel::Hh), 0.0, "link off: the legacy scalar");
    assert_eq!(crate::world::tau_for(&off, crate::world::Channel::Firm), 0.0);
}

// ---------------------------------------------------------------- imports and exports

#[test]
fn imports_by_sector_sum_equals_customs_imports() {
    // H9 (both routings): the imports by sector sum to the month's customs imports (`row.imports_this_month`).
    for pool in [false, true] {
        let cfg = Config { firm_order_pool: pool, n_households: 400, ..Default::default() };
        let mut w = testkit::tiny_world(cfg, 48);
        for _ in 0..12 {
            sim::step(&mut w);
            let s: f64 = w.flows.imports_by_sector.iter().sum();
            let r = w.row.imports_this_month;
            assert!((s - r).abs() <= 1e-9 * r.max(1.0), "pool {pool}: sum {s} vs row {r}");
        }
    }
}

#[test]
fn exports_base_shares_equal_matrix() {
    // W2-12: at the base (blocs at zero, no shocks) the exports by sector and bloc equal x0 times trend GDP, and the shares
    // sum to 10.96% of GDP.
    let mut w = tiny_block(false, 300, 49);
    w.fiscal.trend_gdp = 1000.0;
    let mut tr = state();
    tr.capture(&w, &zeros());
    let _ = post(&mut tr, &w, &zeros());
    let wp = WorldParams::load();
    let mut total = 0.0;
    for s in 0..S {
        for j in 0..NB {
            let want = wp.trade.exports_base[j][s] / wp.trade.gdp_2024_bn;
            let got = tr.x_exp[s][j] / 1000.0;
            assert!((got - want).abs() < 1e-12, "sector {s} bloc {j}: {got} vs {want}");
            total += want;
        }
    }
    assert!((total * 100.0 - 10.96).abs() < 0.005, "share of GDP {}", total * 100.0);
}

#[test]
fn exports_income_elasticity_analytic() {
    // W2-09: a bloc output gap of -1% in every lag lowers exports by 1 - exp(1.4 x -0.01) of the base.
    let mut w = tiny_block(false, 300, 50);
    w.fiscal.trend_gdp = 1000.0;
    let mut tr = state();
    tr.capture(&w, &zeros());
    let base = post(&mut tr, &w, &zeros()).exports;
    let mut tr2 = state();
    tr2.capture(&w, &zeros());
    let mut blocs = zeros();
    for b in blocs.iter_mut() {
        b.hist.y = [0.0, -1.0, -1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
    }
    let shocked = post(&mut tr2, &w, &blocs).exports;
    for s in 0..S {
        let got = shocked[s] / base[s] - 1.0;
        let want = (1.4f64 * -0.01).exp() - 1.0;
        assert!((got - want).abs() < 1e-12, "sector {s}: {got} vs {want}");
    }
}

/// The hand path of spec 2.5 for Dollar +10%, recomputed in the test with the golden tables: the price and exchange-rate
/// channels with no income effect. `energy_rule` applies the Energy rule of spec 2.5 (the competitor term of Energy is the
/// world energy factor, zero here); off, every sector takes the competitor term, as `trade_hand.py` does.
fn hand_exports(energy_rule: bool, t_max: usize) -> Vec<f64> {
    let wp = WorldParams::load();
    let tr = &wp.trade;
    let theta = &tr.theta;
    let total: f64 = (0..NB).map(|j| (0..S).map(|s| tr.exports_base[j][s]).sum::<f64>()).sum();
    let (sd, kd, sn, kn) = (0.08, 0.47, 0.90, 0.95);
    let (ps, pl, fs, fl, speed) = (0.10, 0.90, 0.15, 0.30, 0.04);
    let mut phid = [0.0; NB];
    let mut phind = [0.0; NB];
    let mut uf = [0.0; NB];
    let mut up = [0.0; NB];
    let mut out = Vec::with_capacity(t_max);
    let z = Z10;
    for _ in 0..t_max {
        let mut r = [0.0; NB];
        for j in 0..NB {
            phid[j] += sd * (kd * z - phid[j]);
            phind[j] += sn * (kn * z - phind[j]);
            r[j] = -(theta[j] * phid[j] + (1.0 - theta[j]) * phind[j]) / 100.0;
        }
        let mut f = [0.0; NB];
        let mut rc = [0.0; NB];
        for j in 0..NB {
            uf[j] += speed * (z / 100.0 - uf[j]);
            let lnfx = -(fs * z / 100.0 + (fl - fs) * uf[j]);
            let (mut num, mut den) = (0.0, 0.0);
            for k in 0..NB {
                if k != j {
                    num += tr.import_shares[k][j] * r[k];
                    den += tr.import_shares[k][j];
                }
            }
            rc[j] = num / den;
            up[j] += speed * (rc[j] - up[j]);
            let lnrp = ps * rc[j] + (pl - ps) * up[j];
            f[j] = lnfx + lnrp;
        }
        // Energy: no competitor term under the Energy rule (the world factor is one at the base).
        let mut agg = 0.0;
        for j in 0..NB {
            let lnfx = -(fs * z / 100.0 + (fl - fs) * uf[j]);
            let e_fac = if energy_rule { lnfx } else { f[j] };
            let nonenergy: f64 = (1..S).map(|s| tr.exports_base[j][s]).sum();
            let energy = tr.exports_base[j][ENERGY];
            agg += nonenergy * f[j].exp() + energy * e_fac.exp();
        }
        out.push(100.0 * (agg / total - 1.0));
    }
    out
}

#[test]
fn exports_price_and_fx_hand_values() {
    // The hand path of trade_hand.py (no Energy rule) is reproduced by the recomputation below to 1e-6, and the block agrees
    // with the recomputation with the Energy rule of spec 2.5 to 1e-9. The Energy rule moves the 12/24/36-month values by
    // a few tenths of a point (reported in the stage 2 progress note, not published; W2-02 band -2.9 +/- 1.5, -5.7 +/- 2.5, -7.1 +/- 2.5).
    let g = golden();
    let golden_path = gvec(&g["exports_pct_dollar10_path"]);
    let no_rule = hand_exports(false, 61);
    for h in 0..61 {
        assert!((no_rule[h] - golden_path[h]).abs() < 1e-6, "h {h}: recomputed {} golden {}", no_rule[h], golden_path[h]);
    }
    let with_rule = hand_exports(true, 61);
    let mut w = tiny_block(false, 300, 51);
    w.fiscal.trend_gdp = 1000.0;
    let mut tr = state_no_substitution();
    tr.capture(&w, &zeros());
    // Dollar +10%: the log step of the real rate is Z10 / 100, and with the blocs at zero z~_j = c_t = Z10 for every bloc.
    w.row.fx *= (Z10 / 100.0).exp();
    let base_total: f64 = {
        let wp = WorldParams::load();
        (0..NB).map(|j| (0..S).map(|s| wp.trade.exports_base[j][s]).sum::<f64>()).sum::<f64>() / wp.trade.gdp_2024_bn * 1000.0
    };
    let blocs = zeros();
    for h in 0..37 {
        let out = post(&mut tr, &w, &blocs);
        let total: f64 = out.exports.iter().sum();
        let got = 100.0 * (total / base_total - 1.0);
        assert!((got - with_rule[h]).abs() < 1e-9, "h {h}: block {got} recomputed {}", with_rule[h]);
    }
    // Report the Energy-rule difference at 12, 24 and 36 months (state after h+1 updates).
    for h in [12usize, 24, 36] {
        eprintln!("exports month {h}: no Energy rule {:.3}, with Energy rule {:.3}", no_rule[h], with_rule[h]);
    }
}

#[test]
fn import_volume_hand_values() {
    // The ABM's import-share mechanism (goods.rs `elasticity_factor` on the terms ratio, lagged at speed 0.08) reproduces the
    // golden import-volume paths of trade_hand.py for the master pair and the two fallback pairs (D13), to 0.05 points.
    let g = golden();
    let ln_ratio: Vec<f64> = gvec(&g["pass_through_path"]).iter().map(|p| p * Z10 / 100.0).collect();
    for (key, sr, lr) in [("0.2/0.6", 0.2, 0.6), ("0.4/1.0", 0.4, 1.0), ("0.7/1.3", 0.7, 1.3)] {
        let want = gvec(&g["import_volume_pct_dollar10_path"][key]);
        let mut zl: Option<f64> = None;
        for t in 0..61 {
            let now = ln_ratio[t];
            let z = match zl {
                None => now,
                Some(p) => p + 0.08 * (now - p),
            };
            zl = Some(z);
            let factor = crate::goods::elasticity_factor(now.exp(), z, sr, lr);
            let got = 100.0 * (factor - 1.0);
            assert!((got - want[t]).abs() < 0.05, "{key} h {t}: {got} vs {}", want[t]);
        }
    }
}

#[test]
fn m_jk_orientation_and_column_sums() {
    // m_jk[j][k] is bloc j's import share from bloc k: the research matrix is stored [exporter][importer] (spec 2.5).
    let wp = WorldParams::load();
    let tab = state().tab;
    for j in 0..NB {
        for k in 0..NB {
            assert_eq!(tab.m_jk[j][k], wp.trade.import_shares[k][j]);
        }
    }
    // Importer columns over the twelve blocs are below one (the US is the missing exporter).
    for j in 0..NB {
        let col: f64 = (0..NB).map(|k| tab.m_jk[j][k]).sum();
        assert!(col > 0.4 && col < 1.0, "importer {j} column {col}");
    }
}

#[test]
fn exports_use_trend_gdp_not_last_month_gdp() {
    // Exports scale with trend nominal GDP (fiscal::trend_gdp_month), and nothing else in the export equation reads GDP.
    let mut w1 = tiny_block(false, 300, 52);
    w1.fiscal.trend_gdp = 1000.0;
    let mut t1 = state();
    t1.capture(&w1, &zeros());
    let e1 = post(&mut t1, &w1, &zeros()).exports;
    let mut w2 = tiny_block(false, 300, 52);
    w2.fiscal.trend_gdp = 2000.0;
    let mut t2 = state();
    t2.capture(&w2, &zeros());
    let e2 = post(&mut t2, &w2, &zeros()).exports;
    assert_eq!(fiscal::trend_gdp_month(&w2), 2000.0);
    for s in 0..S {
        assert!((e2[s] - 2.0 * e1[s]).abs() < 1e-9 * e1[s].max(1.0), "sector {s}");
    }
}

#[test]
fn import_price_numeraire_has_no_drift() {
    // D26: with no dollar move and no bloc real-rate gap the relative import price is one for 240 months.
    let w = tiny_block(false, 300, 53);
    let mut tr = state();
    tr.capture(&w, &zeros());
    for _ in 0..240 {
        let out = post(&mut tr, &w, &zeros());
        assert!(out.imp_rel.iter().all(|&x| (x - 1.0).abs() < 1e-12), "imp_rel {:?}", out.imp_rel);
        assert!((out.imp_rel_cpi - 1.0).abs() < 1e-12);
    }
}

#[test]
fn ztilde_common_component_is_abm_real_dollar() {
    // With the dollar link off, z~_j = c_t + (z_j - sum_k wH10_k z_k): a 10% stronger dollar with blocs at zero gives z~ = 100 ln 1.1.
    let mut w = tiny_block(false, 300, 54);
    let mut tr = state();
    tr.capture(&w, &zeros());
    w.row.fx *= 1.1;
    let _ = post(&mut tr, &w, &zeros());
    for j in 0..NB {
        assert!((tr.z_tilde[j] - Z10).abs() < 1e-9, "bloc {j}: {}", tr.z_tilde[j]);
    }
    // The cross-section: a bloc above the weighted mean has z~ above c_t by its deviation.
    let mut zb = zeros();
    zb[0].hist.z = [2.0; 14];
    let _ = post(&mut tr, &w, &zb);
    let zbar = tr.tab.dollar_w[0] * 2.0;
    for j in 0..NB {
        let dev = if j == 0 { 2.0 } else { 0.0 } - zbar;
        assert!((tr.z_tilde[j] - (Z10 + dev)).abs() < 1e-9, "bloc {j}");
    }
}

#[test]
fn bases_captured_at_last_burn_in_month() {
    // D36 (S2-R1): the bases are the ABM's values at the end of the last burn-in month (burn_in 5: captured in the fifth step),
    // so the first reported month is measured against them. The identity z~_j - (z_j - zbar) = c_t holds every reported month.
    let cfg = Config { world_block_enabled: true, world_link_trade: true, n_households: 300, ..Default::default() };
    let mut w = testkit::tiny_world(cfg, 55);
    w.cfg.burn_in_months = 5;
    for _ in 0..4 {
        sim::step(&mut w);
    }
    let wb = w.world.as_deref().expect("block on");
    assert!(wb.trade.as_ref().unwrap().base.is_none(), "no base before the last burn-in month ends");
    sim::step(&mut w);
    let wb = w.world.as_deref().expect("block on");
    let tr = wb.trade.as_ref().unwrap();
    let base = tr.base.as_ref().expect("captured at the end of the last burn-in month");
    assert!((base.p0 - common::world_price(&w)).abs() / base.p0 < 0.01, "world price at the base is the burn-in month's");
    sim::step(&mut w);
    let wb = w.world.as_deref().expect("block on");
    let tr = wb.trade.as_ref().unwrap();
    let zbar: f64 = (0..NB).map(|k| tr.tab.dollar_w[k] * wb.blocs[k].hist.z[0]).sum();
    for j in 0..NB {
        let c = tr.z_tilde[j] - (wb.blocs[j].hist.z[0] - zbar);
        assert!((c - tr.c_t).abs() < 1e-9, "bloc {j}: common component {c} against c_t {}", tr.c_t);
    }
}

#[test]
fn cpi_import_component_weights() {
    // imp_rel_cpi is the CPI-tradable mean of the sector import prices weighted by the import shares, over the same mean of the
    // shares (spec 2.5). With a bloc dollar move the sector prices differ and the index is not their plain mean.
    let mut w = tiny_block(false, 300, 56);
    let mut tr = state();
    tr.capture(&w, &zeros());
    let mut zb = zeros();
    zb[1].hist.z = [30.0; 14];
    w.row.fx *= 1.02;
    let out = post(&mut tr, &w, &zb);
    let cpi = w.cfg.cpi_tradable_weight;
    let share = w.cfg.import_share;
    let num = role_mean(&cpi, |s| share[s] * out.imp_rel[s]);
    let den = role_mean(&cpi, |s| share[s]);
    assert!((out.imp_rel_cpi - num / den).abs() < 1e-12);
    let plain = role_mean(&cpi, |s| out.imp_rel[s]);
    assert!((out.imp_rel_cpi - plain).abs() > 1e-9, "weights matter");
}

// ---------------------------------------------------------------- stepping: identities, hooks, shocks

#[test]
fn trade_off_goods_identical() {
    // Link off, block on: a bilateral duty and the set-level level leave every non-world key equal to the block-off run with
    // the set-level level only (the bilateral duty is ignored by the ABM without the link).
    let mut on = tiny_block(false, 300, 57);
    let mut off = testkit::tiny_world(Config { world_block_enabled: false, n_households: 300, ..Default::default() }, 57);
    for w in [&mut on, &mut off] {
        w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::ImportTariff { pct: 10.0 } });
    }
    on.shocks.schedule(ScheduledShock {
        at_month: 1,
        shock: Shock::Tariff { partner: Some("CN".to_string()), sector: None, pct: 25.0, retaliate: false },
    });
    for _ in 0..12 {
        sim::step(&mut on);
        sim::step(&mut off);
    }
    let strip = |v: Value| -> Value {
        Value::Array(
            v.as_array()
                .unwrap()
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r.as_object_mut().unwrap().remove("world");
                    r
                })
                .collect(),
        )
    };
    let a = serde_json::to_value(&on.history).unwrap();
    assert_eq!(strip(a), strip(serde_json::to_value(&off.history).unwrap()));
}

#[test]
fn trade_link_ledger_consistent_24_months() {
    // The link on: the stock-flow identities hold every month (the check of debug builds, run here explicitly in release).
    // A world with a real burn-in (D36).
    let mut w = burn_in_world(true, 300, 12, 58, 0.005);
    w.shocks.schedule(ScheduledShock { at_month: 2, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } });
    w.shocks.schedule(ScheduledShock {
        at_month: 3,
        shock: Shock::Tariff { partner: Some("CN".to_string()), sector: None, pct: 25.0, retaliate: false },
    });
    for m in 0..24 {
        sim::step(&mut w);
        ledger::check_consistency(&w).unwrap_or_else(|e| panic!("month {m}: {e}"));
    }
}

#[test]
fn us_import_gap_channel_clamped_and_lagged() {
    // Spec 2.3: the import-demand term is zero for the first twelve reported months, then 100 x the EWMA clamped to +/-10.
    let w = tiny_block(true, 300, 59);
    let mut tr = state();
    tr.capture(&w, &zeros());
    tr.gap_ewma = [1.0; NB];
    tr.since_base = 5;
    tr.pre_bloc(&w);
    assert!(tr.gap_ring.iter().all(|r| r[0] == 0.0), "warm-up: no term");
    tr.since_base = 20;
    tr.gap_ewma = [1.0; NB];
    tr.pre_bloc(&w);
    assert!(tr.gap_ring.iter().all(|r| r[0] == GAP_CLAMP), "clamped to +10");
    tr.gap_ewma = [-1.0; NB];
    tr.pre_bloc(&w);
    assert!(tr.gap_ring.iter().all(|r| r[0] == -GAP_CLAMP), "clamped to -10");
    tr.gap_ewma = [0.05; NB];
    tr.pre_bloc(&w);
    assert!(tr.gap_ring.iter().all(|r| (r[0] - 5.0).abs() < 1e-12), "unclamped 5");
    assert!(tr.gap_ring.iter().all(|r| r[1] == -GAP_CLAMP), "the ring shifts: month t-2 holds the previous term");
}

#[test]
fn bilateral_sums_equal_ledger() {
    // W2-11: the bilateral exports and imports by bloc sum to the ledger's exports and imports every month (a world with a
    // real burn-in, D36; the dollar shock at reported month 0 is measured against the base).
    let mut w = burn_in_world(true, 300, 12, 60, 0.005);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::Dollar { pct: 10.0, half_life_months: 0.0 } });
    for _ in 0..24 {
        sim::step(&mut w);
        let r = w.history.last().unwrap();
        let mm = r.world.as_ref().unwrap();
        let ex: f64 = mm.exports_bn.iter().sum();
        let im: f64 = mm.imports_bn.iter().sum();
        assert!((ex - r.exports_bn_ar).abs() <= 1e-9 * r.exports_bn_ar.max(1.0), "exports {ex} vs {}", r.exports_bn_ar);
        assert!((im - r.imports_bn_ar).abs() <= 1e-9 * r.imports_bn_ar.max(1.0), "imports {im} vs {}", r.imports_bn_ar);
    }
}

#[test]
fn terms_ratio_continuous_at_reported_month_0() {
    // D36 continuity (S2-R1, F1): with the link on, reported month 0 starts from the ABM's own burn-in state. The ABM's FX base
    // and its lagged logs are not overwritten, and the block's import price level is the legacy level of the base month
    // (`goods.import_rel` at the end of burn-in), so the terms ratio carries on from the burn-in. Checked on a world with
    // a real burn-in and perturbed goods prices (2 per cent a year over 24 months).
    let mut w = burn_in_world(true, 500, 24, 61, 0.02);
    let fxb_before = w.monetary.fx_base.expect("the ABM's FX base is captured in burn-in");
    let lag_before = w.trade_import_z.expect("the lagged import logs exist after burn-in");
    let imp_level = w.world.as_deref().unwrap().trade.as_ref().unwrap().base.as_ref().unwrap().imp_level;
    assert_eq!(Some(imp_level), w.goods.import_rel, "the base level is the legacy import level of the last burn-in month");
    sim::step(&mut w);
    let fxb_after = w.monetary.fx_base.expect("still captured");
    assert_eq!(fxb_before.import_price.to_bits(), fxb_after.import_price.to_bits(), "FX base not overwritten");
    for s in 0..S {
        assert_eq!(fxb_before.terms[s].to_bits(), fxb_after.terms[s].to_bits(), "FX base terms {s} not overwritten");
    }
    let lag_after = w.trade_import_z.expect("the lagged logs continue");
    for i in 0..=S {
        assert!((lag_after[i] - lag_before[i]).abs() < 0.01, "lagged terms log {i} continues from the burn-in");
    }
    let imp_now = w.world.as_deref().unwrap().out.imp_rel_cpi;
    assert!((imp_now / imp_level - 1.0).abs() < 3e-3, "import index at month 0 {imp_now} against the base level {imp_level}");
}

#[test]
fn import_rel_hook_feeds_firms_and_metrics() {
    // H6: the CPI-weighted import index is the goods' import_rel (firms and the metrics read it). A real burn-in (D36).
    let mut w = burn_in_world(true, 300, 12, 62, 0.005);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::Dollar { pct: 5.0, half_life_months: 0.0 } });
    for _ in 0..3 {
        sim::step(&mut w);
    }
    let wb = w.world.as_deref().unwrap();
    assert_eq!(w.goods.import_rel, Some(wb.out.imp_rel_cpi));
    let m = w.history.last().unwrap();
    assert!((m.import_price_index - 100.0 * wb.out.imp_rel_cpi).abs() < 1e-9);
}

#[test]
fn b7_uniform_tariff_gates_unchanged_with_link() {
    // W2-06 engineering part: a uniform set-level tariff with the link on gives the same effective duty as b7 (the scalar).
    // A real burn-in (D36).
    let mut w = burn_in_world(true, 300, 12, 63, 0.005);
    w.shocks.schedule(ScheduledShock { at_month: 0, shock: Shock::ImportTariff { pct: 10.0 } });
    for _ in 0..12 {
        sim::step(&mut w);
        let wb = w.world.as_deref().unwrap();
        assert_eq!(wb.out.tau_eff_hh, 0.1);
        assert_eq!(wb.out.tau_eff_firm, 0.1);
    }
    assert!((w.history.last().unwrap().tariff_rate_pct - 10.0).abs() < 1e-12);
}

#[test]
fn dollar_shock_trade_path_monotone() {
    // A permanent stronger dollar lowers the origin prices and the exports monotonically over 36 months (no sign reversal).
    let mut w = tiny_block(false, 300, 64);
    let mut tr = state();
    tr.capture(&w, &zeros());
    w.row.fx *= 1.10;
    let mut prev_exp = f64::INFINITY;
    let mut prev_imp = f64::INFINITY;
    for _ in 0..36 {
        let out = post(&mut tr, &w, &zeros());
        let total: f64 = out.exports.iter().sum();
        assert!(total < prev_exp, "exports fall: {total} after {prev_exp}");
        assert!(out.imp_rel_cpi < prev_imp, "import price falls");
        prev_exp = total;
        prev_imp = out.imp_rel_cpi;
    }
}

#[test]
fn link_first_month_is_continuous() {
    // S2-R1 / F6: a world with a real burn-in and perturbed prices (goods inflation 4 per cent a year over 24 burn-in months, so the
    // ABM's state at reported month 0 is far from the neutral base). The link on and the link off run the same burn-in and
    // differ only in the reported months: the trade flows of months 0-2 differ by less than 0.2 per cent of GDP each month.
    let mut on = burn_in_world(true, 1000, 24, 71, 0.04);
    let mut off = burn_in_world(false, 1000, 24, 71, 0.04);
    for m in 0..3 {
        sim::step(&mut on);
        sim::step(&mut off);
        let a = on.history.last().unwrap();
        let b = off.history.last().unwrap();
        let dx = (a.exports_bn_ar - b.exports_bn_ar) / a.gdp_nominal_bn_ar;
        let dm = (a.imports_bn_ar - b.imports_bn_ar) / a.gdp_nominal_bn_ar;
        assert!(dx.abs() < 0.002, "month {m}: |dX| {:.5} of GDP", dx.abs());
        assert!(dm.abs() < 0.002, "month {m}: |dM| {:.5} of GDP", dm.abs());
    }
}

#[test]
fn import_demand_benchmark_is_warm_up_mean_then_slow_ewma() {
    // S2-R5: the import-demand term is ln(Q/Y) against a benchmark that is the mean of the first twelve observations, then an
    // EWMA with a 60-month half-life. A constant level gives no deviation after the warm-up; a level shift of d moves the
    // benchmark by about d/2 after 60 months.
    let w = burn_in_world(true, 300, 12, 64, 0.005);
    let mut tr = state();
    tr.capture(&w, &zeros());
    let q = 2.0;
    for _ in 0..12 {
        tr.q_prev = [q; NB];
        tr.pre_bloc(&w);
    }
    let y = w.last().map(|m| m.gdp_real_bn_ar).filter(|y| y.is_finite() && *y > 0.0).unwrap_or(1.0);
    let x0 = (q / y).ln();
    assert!(tr.bench.iter().all(|b| (b - x0).abs() < 1e-12), "benchmark is the warm-up mean");
    for _ in 0..30 {
        tr.q_prev = [q; NB];
        tr.pre_bloc(&w);
    }
    assert!(tr.gap_ewma.iter().all(|g| g.abs() < 1e-12), "no deviation at a constant level");
    let shifted = q * 0.1f64.exp();
    for _ in 0..60 {
        tr.q_prev = [shifted; NB];
        tr.pre_bloc(&w);
    }
    let moved = tr.bench[0] - x0;
    assert!((moved - 0.05).abs() < 0.005, "benchmark moved {moved} after 60 months of +0.1 (half-life 60: about 0.05)");
}

#[test]
fn deflator_is_post_bloc_pt_ratio() {
    // F4: the real imports and exports are deflated by the PT ratio of post_bloc (stored), the same one that prices the imports.
    let mut w = burn_in_world(false, 300, 12, 65, 0.005);
    let mut tr = state();
    tr.capture(&w, &zeros());
    let pt0 = tr.base.as_ref().unwrap().pt0;
    for s in 0..S {
        w.prices.sector[s] *= 1.05;
    }
    let _ = post(&mut tr, &w, &zeros());
    let want = super::trade::pt_level(&w) / pt0;
    assert!((tr.pt_ratio - want).abs() < 1e-12, "pt_ratio {} against {want}", tr.pt_ratio);
}

