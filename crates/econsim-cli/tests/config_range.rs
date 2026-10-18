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
