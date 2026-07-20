//! Argument dispatch for the `oxidone` binary. The `match` lives in
//! `src/main.rs`, so it can only be exercised by spawning the built binary via
//! `CARGO_BIN_EXE_oxidone`. Never spawn with zero arguments — that launches the
//! TUI and would hang the suite.

use std::process::Command;

use oxidone::config;

fn oxidone() -> Command {
    Command::new(env!("CARGO_BIN_EXE_oxidone"))
}

#[test]
fn version_prints_crate_version_and_exits_zero() {
    let out = oxidone().arg("--version").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(
        stdout.trim(),
        format!("oxidone {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_exits_zero_and_mentions_usage() {
    let out = oxidone().arg("--help").output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("Usage:"));
}

#[test]
fn print_config_path_matches_the_library() {
    let out = oxidone().arg("--print-config-path").output().unwrap();
    // With a home dir present (the test environment has one) this succeeds and
    // agrees with the library's own resolution — the single source of truth the
    // Makefile's `config` target relies on.
    match config::config_file() {
        Some(expected) => {
            assert!(out.status.success());
            let stdout = String::from_utf8(out.stdout).unwrap();
            assert_eq!(stdout.trim(), expected.display().to_string());
        }
        None => assert!(!out.status.success()),
    }
}

#[test]
fn unknown_argument_fails_closed() {
    let out = oxidone().arg("--notaflag").output().unwrap();
    // Fail closed: non-zero exit with usage on stderr, never a silent launch.
    assert!(!out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("unrecognized argument"));
    assert!(stderr.contains("Usage:"));
}
