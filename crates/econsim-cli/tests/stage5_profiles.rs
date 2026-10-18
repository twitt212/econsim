//! Stage 5 (D58, D61): every stage-5 profile in scripts/profiles/ parses through the ECONSIM_CFG path and passes the world-link checks
//! (the `config` subcommand refuses an invalid overlay with status 2), and `legacy_b8off.json` merged into the defaults leaves
//! `Config::default()` unchanged field by field (identity I9: the same printed config).

use std::path::PathBuf;
use std::process::{Command, ExitStatus};

const PROFILES: &[&str] = &[
    "b8-world-s5a",
    "b8-world-s5b",
    "b8-world-s5c",
    "b8-world-s5a-nc",
    "b8-world-s5c-nd",
    "b8-world-s5a-lean",
    "b8-world-wl",
    "b8-world-wabm",
    "b8-world-wabm-mf",
    "legacy_b8off",
];

fn profile(name: &str) -> String {
    let p: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", "scripts", "profiles", &format!("{name}.json")].iter().collect();
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

/// Runs `econsim-cli config` with the given overlay (or none) and returns status, stdout and stderr.
fn config_of(cfg: Option<&str>) -> (ExitStatus, String, String) {
    let mut c = Command::new(env!("CARGO_BIN_EXE_econsim-cli"));
    c.env_remove("ECONSIM_CFG");
    if let Some(s) = cfg {
        c.env("ECONSIM_CFG", s);
    }
    let out = c.arg("config").output().expect("econsim-cli starts");
    (out.status, String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

#[test]
fn stage5_profiles_load_and_validate() {
    for name in PROFILES {
        let (st, _, err) = config_of(Some(&profile(name)));
        assert!(st.success(), "profile {name} refused: {err}");
    }
    let (st, defaults, err) = config_of(None);
    assert!(st.success(), "defaults refused: {err}");
    let (st, legacy, err) = config_of(Some(&profile("legacy_b8off")));
    assert!(st.success(), "legacy_b8off refused: {err}");
    assert_eq!(defaults, legacy, "legacy_b8off must leave Config::default() unchanged field by field");
}

/// Stage 5-T2 (D62, D63): the t2 profiles parse and pass the world-link checks, and the stage-5 `legacy_b8off` check above still holds.
const T2_PROFILES: &[&str] = &["b8-world-t2a", "b8-world-t2b", "b8-world-t2c", "b8-world-t2a-nc", "b8-world-t2c-nd"];

#[test]
fn t2_profiles_load_and_validate() {
    for name in T2_PROFILES {
        let (st, _, err) = config_of(Some(&profile(name)));
        assert!(st.success(), "profile {name} refused: {err}");
    }
    let (st, defaults, err) = config_of(None);
    assert!(st.success(), "defaults refused: {err}");
    assert!(defaults.contains("world_import_rel_drift"), "the key is in the printed config");
}
