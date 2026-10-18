//! Sub-round 5-T2 sketch tests (spec rev 12, D62, D63): the drift of the block's import price against PT, and the uniform-duty fold.

use crate::config::Config;
use crate::shocks::{ScheduledShock, Shock};
use crate::sim;
use crate::types::World;

fn cfg(drift: f64) -> Config {
    Config {
        burn_in_months: 12,
        n_households: 300,
        world_block_enabled: true,
        world_link_trade: true,
        fin_factor_rho: 0.83,
        fin_factor_sigma: 0.30,
        fin_jump_intensity: 0.06,
        fin_jump_mean: 1.3,
        fin_jump_sd: 0.5,
        erp_fin_loading: 0.0028,
        world_import_rel_drift: drift,
        ..Config::default()
    }
}

fn json(m: &crate::metrics::Metrics) -> serde_json::Value {
    serde_json::to_value(m).unwrap()
}

fn run_with(c: &Config, seed: u64, shocks: &[ScheduledShock], months: usize) -> World {
    let mut w = sim::new_world(c, seed);
    for s in shocks {
        w.shocks.schedule(s.clone());
    }
    for _ in 0..months {
        sim::step(&mut w);
    }
    w
}

#[test]
fn import_rel_drift_is_one_at_the_base_and_scales_the_import_price_after_it() {
    // Same seed, drift 0 and drift 0.0032 a year. Record 0 (the base month, since_base 0) is identical in every key: the drift is a ramp from the
    // base, so the continuity of W2-17 is untouched. In record 1 the block's CPI-weighted import price index (100 imp_rel_cpi, computed before the
    // month's clear from the same ABM state in both worlds) differs by exactly the factor exp(-g / 12), and in record 6 the two worlds have diverged
    // through the ABM but the ratio of the *block* prices of the same month is not asserted (the ABM state differs).
    let g = 0.0032;
    let (a, b) = (run_with(&cfg(0.0), 5, &[], 3), run_with(&cfg(g), 5, &[], 3));
    assert_eq!(json(&a.history[0]), json(&b.history[0]), "record 0 must be identical (drift factor 1 at the base)");
    let (pa, pb) = (a.history[1].world.as_ref().unwrap().import_price_idx, b.history[1].world.as_ref().unwrap().import_price_idx);
    let ratio = pb / pa;
    assert!((ratio - (-g / 12.0).exp()).abs() <= 1e-12, "ratio {ratio} against exp(-g/12) {}", (-g / 12.0).exp());
    assert!(pb < pa, "a positive drift makes imports cheaper against PT");
}

#[test]
fn import_rel_drift_zero_and_default_are_the_same_world() {
    let (a, b) = (run_with(&Config { world_import_rel_drift: 0.0, ..cfg(0.0) }, 9, &[], 18), run_with(&cfg(0.0), 9, &[], 18));
    for (ma, mb) in a.history.iter().zip(b.history.iter()) {
        assert_eq!(json(ma), json(mb));
    }
    assert_eq!(Config::default().world_import_rel_drift, 0.0);
}

#[test]
fn config_validation_import_rel_drift() {
    let ok = |g: f64| cfg(g).validate_world_links();
    assert!(ok(0.0).is_ok() && ok(0.0032).is_ok() && ok(-0.05).is_ok() && ok(0.05).is_ok());
    for bad in [0.0501, -0.0501, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        match ok(bad) {
            Err(m) => assert!(m.contains("world_import_rel_drift"), "{m}"),
            Ok(()) => panic!("accepted {bad}"),
        }
    }
}

#[test]
fn uniform_tariff_is_bit_identical_to_the_import_tariff() {
    // D63: a component with no partner and no sector is folded into the set-level duty, so `Tariff{null, null, p}` (retaliation flag or not) and
    // `ImportTariff{p}` give the same floating-point duty and the same run, every key of every record (the world block included), for 14 months.
    let c = cfg(0.0);
    let imp = [ScheduledShock { at_month: 1, shock: Shock::ImportTariff { pct: 25.0 } }];
    let uni = [ScheduledShock { at_month: 1, shock: Shock::Tariff { partner: None, sector: None, pct: 25.0, retaliate: false } }];
    let unr = [ScheduledShock { at_month: 1, shock: Shock::Tariff { partner: None, sector: None, pct: 25.0, retaliate: true } }];
    let (a, b, d) = (run_with(&c, 3, &imp, 14), run_with(&c, 3, &uni, 14), run_with(&c, 3, &unr, 14));
    for m in 0..14 {
        assert_eq!(json(&a.history[m]), json(&b.history[m]), "month {m}: ImportTariff against the uniform Tariff");
        assert_eq!(json(&a.history[m]), json(&d.history[m]), "month {m}: ImportTariff against the uniform flagged Tariff (retaliation off)");
    }
    assert!(a.history[3].tariff_rate_pct > 24.9, "the duty is in force");
}

#[test]
fn uniform_and_bilateral_duties_add_by_origin() {
    // Tariff{CN, all, 10} plus Tariff{all, all, 5}: the duty by origin is 15 points on China and 5 on every other bloc.
    let c = cfg(0.0);
    let sh = [
        ScheduledShock { at_month: 0, shock: Shock::Tariff { partner: Some("CN".into()), sector: None, pct: 10.0, retaliate: false } },
        ScheduledShock { at_month: 0, shock: Shock::Tariff { partner: None, sector: None, pct: 5.0, retaliate: false } },
    ];
    let w = run_with(&c, 4, &sh, 3);
    let t = &w.history[2].world.as_ref().unwrap().tau_by_origin;
    assert!((t[1] - 0.15).abs() <= 1e-9, "CN {}", t[1]);
    for j in [0usize, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11] {
        assert!((t[j] - 0.05).abs() <= 1e-9, "bloc {j}: {}", t[j]);
    }
}
