//! Stage 5 sketch tests (D57): the centring constants of the coupled import-demand term.

use crate::config::Config;
use crate::sim;

fn cfg(offset: Vec<f64>) -> Config {
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
        world_import_gap_offset: offset,
        ..Config::default()
    }
}

#[test]
fn import_gap_offset_empty_is_identity() {
    // The key empty and twelve zeros give bit-identical records for 30 months (x - 0.0 == x), and the validation accepts both.
    let (a_cfg, b_cfg) = (cfg(Vec::new()), cfg(vec![0.0; 12]));
    assert!(a_cfg.validate_world_links().is_ok() && b_cfg.validate_world_links().is_ok());
    let (mut a, mut b) = (sim::new_world(&a_cfg, 7), sim::new_world(&b_cfg, 7));
    for _ in 0..30 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    for (ma, mb) in a.history.iter().zip(b.history.iter()) {
        assert_eq!(serde_json::to_value(ma).unwrap(), serde_json::to_value(mb).unwrap());
    }
}

#[test]
fn import_gap_offset_subtracts_after_the_clamp() {
    // The term of month t reaches the world metric `us_import_gap` as clamp(100 ewma, +-10) - offset_j once it is active (reported month 13
    // on). Two worlds with the same seed differ only in the key, so the first record in which they differ is the first active month with
    // an unchanged ewma: there `b[j] == a[j] - offset[j]` bit for bit. Before it, every record is identical (the offset is not applied
    // while the term is inactive).
    let off: Vec<f64> = (0..12).map(|j| 0.5 + 0.25 * j as f64).collect();
    let (mut a, mut b) = (sim::new_world(&cfg(Vec::new()), 11), sim::new_world(&cfg(off.clone()), 11));
    for _ in 0..40 {
        sim::step(&mut a);
        sim::step(&mut b);
    }
    let mut first = None;
    for (m, (ma, mb)) in a.history.iter().zip(b.history.iter()).enumerate() {
        let (ga, gb) = (&ma.world.as_ref().unwrap().us_import_gap, &mb.world.as_ref().unwrap().us_import_gap);
        if ga != gb {
            first = Some(m);
            for j in 0..12 {
                assert_eq!(gb[j].to_bits(), (ga[j] - off[j]).to_bits(), "bloc {j}: {} vs {} - {}", gb[j], ga[j], off[j]);
            }
            break;
        }
    }
    let m = first.expect("the offset must show in the metric once the term is active");
    assert!(m >= 12, "the term is inactive before reported month 13 (first difference at record {m})");
}

#[test]
fn config_validation_import_gap_offset() {
    let ok = |v: Vec<f64>| cfg(v).validate_world_links();
    assert!(ok(Vec::new()).is_ok() && ok(vec![2.0; 12]).is_ok() && ok(vec![-10.0; 12]).is_ok() && ok(vec![10.0; 12]).is_ok());
    for bad in [vec![2.0; 11], vec![2.0; 13], vec![10.5; 12], vec![-10.5; 12]] {
        match ok(bad.clone()) {
            Err(m) => assert!(m.contains("world_import_gap_offset"), "{m}"),
            Ok(()) => panic!("accepted {bad:?}"),
        }
    }
    let mut nan = vec![2.0; 12];
    nan[3] = f64::NAN;
    assert!(ok(nan).is_err());
    let mut inf = vec![2.0; 12];
    inf[0] = f64::INFINITY;
    assert!(ok(inf).is_err());
}
