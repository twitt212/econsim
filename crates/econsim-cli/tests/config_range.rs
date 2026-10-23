//! Stage 4-2 (list A', S4-R22, review D2): the CLI refuses an out-of-range finance value with status 2, one line on stderr and nothing
//! on stdout, before it runs anything. The defaults and in-range values run.

use std::process::{Command, Output};

fn econsim_with(cfg: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_econsim-cli"))
        .env("ECONSIM_CFG", cfg)
        .args(args)
        .output()
        .expect("econsim-cli starts")
}

#[test]
fn out_of_range_finance_values_exit_with_an_error() {
    let refused = [
        (r#"{"world_fo_trans_rho":1.5}"#, "world_fo_trans_rho"),
        (r#"{"world_fo_theta_stock":1e9}"#, "world_fo_theta_stock"),
        (r#"{"world_fo_dollar_resp":-5}"#, "world_fo_dollar_resp"),
        (r#"{"world_gdp_rw_bn":0}"#, "world_gdp_rw_bn"),
    ];
    for (cfg, key) in refused {
        for args in [&["run", "300", "2", "1"][..], &["config"][..]] {
            let out = econsim_with(cfg, args);
            assert_eq!(out.status.code(), Some(2), "{cfg} {args:?}");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains(key), "{cfg} {args:?}: stderr: {err}");
            assert!(out.stdout.is_empty(), "{cfg} {args:?}: nothing on stdout");
        }
    }
}

#[test]
fn finance_values_inside_the_ranges_run() {
    let ok = r#"{"world_block_enabled":true,"world_link_finance":true,"world_fo_trend_cn":0,"world_fo_trans_rho":0.9}"#;
    let out = econsim_with(ok, &["config"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let run = econsim_with(ok, &["run", "300", "2", "1"]);
    assert!(run.status.success(), "stderr: {}", String::from_utf8_lossy(&run.stderr));
}

#[test]
fn out_of_range_retaliation_values_exit_with_an_error() {
    let refused = [
        (r#"{"world_ret_speed":1.5}"#, "world_ret_speed"),
        (r#"{"world_ret_eps_lr":11}"#, "world_ret_eps_lr"),
        (r#"{"world_ret_eps_sr":-1}"#, "world_ret_eps_sr"),
    ];
    for (cfg, key) in refused {
        for args in [&["run", "300", "2", "1"][..], &["config"][..]] {
            let out = econsim_with(cfg, args);
            assert_eq!(out.status.code(), Some(2), "{cfg} {args:?}");
            let err = String::from_utf8_lossy(&out.stderr);
            assert!(err.contains(key), "{cfg} {args:?}: stderr: {err}");
            assert!(out.stdout.is_empty(), "{cfg} {args:?}: nothing on stdout");
        }
    }
}

/// A bound pair out of order (`Config::validate_clamp_bounds`) exits with status 2 before anything runs, instead of panicking in
/// `f64::clamp` during the run.
#[test]
fn clamp_bounds_out_of_order_exit_with_an_error() {
    let out = econsim_with(r#"{"u_star_min":0.09,"u_star_max":0.01}"#, &["run", "300", "2", "1"]);
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("u_star_min = 0.09 is above u_star_max = 0.01"), "stderr: {err}");
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

/// R51-E: the one place the CLI takes shocks from the user is the `world-dump` shock file, which goes through `parse_schedule` and
/// `Shock::validate` before anything runs. A `SharePrices` entry outside its bounds (pct in (-95, 300], half_life_months >= 0,
/// sector null or 0-8) exits with status 2, one line on stderr that names the shock and the entry, and nothing on stdout; the
/// bounds themselves run. The same bounds are tested on `Shock::validate` in `econsim-core` (shocks::tests).
fn world_dump_with_shock(tag: &str, shock: &str) -> Output {
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("share_prices_{tag}.json"));
    std::fs::write(&path, format!(r#"[{{"at_month":1,"shock":{shock}}}]"#)).expect("the shock file is written");
    let out = econsim_with("{}", &["world-dump", "300", "3", "1", path.to_str().expect("utf-8 path")]);
    std::fs::remove_file(&path).expect("the shock file is removed");
    out
}

#[test]
fn out_of_range_share_price_shocks_exit_with_an_error() {
    let refused = [
        ("pct_low", r#"{"type":"SharePrices","sector":4,"pct":-95,"half_life_months":0}"#),
        ("pct_below", r#"{"type":"SharePrices","sector":null,"pct":-100,"half_life_months":0}"#),
        ("pct_high", r#"{"type":"SharePrices","sector":null,"pct":300.5,"half_life_months":0}"#),
        ("half_life", r#"{"type":"SharePrices","sector":null,"pct":-30,"half_life_months":-1}"#),
        ("sector", r#"{"type":"SharePrices","sector":9,"pct":-30,"half_life_months":0}"#),
    ];
    for (tag, shock) in refused {
        let out = world_dump_with_shock(tag, shock);
        assert_eq!(out.status.code(), Some(2), "{shock}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("entry 0: SharePrices"), "{shock}: stderr: {err}");
        assert!(out.stdout.is_empty(), "{shock}: nothing on stdout");
    }
}

#[test]
fn share_price_shocks_at_their_bounds_run() {
    let ok = [
        ("ok_low", r#"{"type":"SharePrices","sector":4,"pct":-94.9,"half_life_months":0}"#),
        ("ok_high", r#"{"type":"SharePrices","sector":8,"pct":300,"half_life_months":24}"#),
        ("ok_all", r#"{"type":"SharePrices","sector":null,"pct":-30,"half_life_months":12}"#),
        ("ok_missing_sector", r#"{"type":"SharePrices","pct":-30,"half_life_months":12}"#),
    ];
    for (tag, shock) in ok {
        let out = world_dump_with_shock(tag, shock);
        assert!(out.status.success(), "{shock}: stderr: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 4, "{shock}: a header and three months");
    }
}

/// RZ23: the same bounds for the Treasury-financed `Rebate` shock (per_household in [0, 100000], months >= 1): a shock file with an entry
/// outside them exits with status 2 and one line that names the shock and the entry; the bounds themselves run.
#[test]
fn out_of_range_rebate_shocks_exit_with_an_error() {
    for (tag, shock) in [
        ("rebate_neg", r#"{"type":"Rebate","per_household":-1,"months":1}"#),
        ("rebate_big", r#"{"type":"Rebate","per_household":100000.5,"months":1}"#),
        ("rebate_zero_months", r#"{"type":"Rebate","per_household":2000,"months":0}"#),
    ] {
        let out = world_dump_with_shock(tag, shock);
        assert_eq!(out.status.code(), Some(2), "{shock}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("entry 0: Rebate"), "{shock}: stderr: {err}");
        assert!(out.stdout.is_empty(), "{shock}: nothing on stdout");
    }
}

#[test]
fn rebate_shocks_at_their_bounds_run() {
    for (tag, shock) in [
        ("rebate_ok_zero", r#"{"type":"Rebate","per_household":0,"months":1}"#),
        ("rebate_ok_2000", r#"{"type":"Rebate","per_household":2000,"months":1}"#),
        ("rebate_ok_max", r#"{"type":"Rebate","per_household":100000,"months":3}"#),
    ] {
        let out = world_dump_with_shock(tag, shock);
        assert!(out.status.success(), "{shock}: stderr: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 4, "{shock}: a header and three months");
    }
}
