//! Stage 4-1 (S4-R14, D7): the CLI refuses the bilateral dollar link without the trade link. The binary exits with status 2, prints
//! one line on stderr and nothing on stdout; the same overlay with the trade link on runs.

use std::process::{Command, Output};

fn econsim_with(cfg: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_econsim-cli"))
        .env("ECONSIM_CFG", cfg)
        .args(args)
        .output()
        .expect("econsim-cli starts")
}

#[test]
fn dollar_link_without_trade_link_exits_with_an_error() {
    let refused = r#"{"world_block_enabled":true,"world_link_dollar":true,"world_link_trade":false}"#;
    for args in [&["run", "300", "2", "1"][..], &["config"][..]] {
        let out = econsim_with(refused, args);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("world_link_trade"), "stderr: {err}");
        assert!(out.stdout.is_empty(), "args {args:?}: nothing on stdout");
    }
}

#[test]
fn dollar_link_with_trade_link_runs() {
    let ok = r#"{"world_block_enabled":true,"world_link_dollar":true,"world_link_trade":true}"#;
    let out = econsim_with(ok, &["run", "300", "2", "1"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn retaliation_without_trade_link_exits_with_an_error() {
    let refused = r#"{"world_block_enabled":true,"world_retaliation":true,"world_link_trade":false}"#;
    for args in [&["run", "300", "2", "1"][..], &["config"][..]] {
        let out = econsim_with(refused, args);
        assert_eq!(out.status.code(), Some(2), "args {args:?}");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(err.contains("world_link_trade"), "stderr: {err}");
        assert!(out.stdout.is_empty(), "args {args:?}: nothing on stdout");
    }
}

#[test]
fn retaliation_with_trade_link_runs() {
    let ok = r#"{"world_block_enabled":true,"world_retaliation":true,"world_positions":true,"world_link_trade":true}"#;
    let out = econsim_with(ok, &["run", "300", "2", "1"]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
}
