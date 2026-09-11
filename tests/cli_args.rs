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

// ---- `oxidone json` (ADR-0010) ----
//
// Only the paths that reach no network and open no browser are spawned here: the
// contract's *behaviour* is covered against the in-memory API in
// `cli_json_{args,reads,apply,errors}.rs`. What these add is that the real binary
// wires it up, and that the exit codes a plugin branches on are the ones the
// process actually produces.

#[test]
fn json_help_exits_zero_and_names_every_subcommand() {
    let out = oxidone().args(["json", "--help"]).output().unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).unwrap();
    for subcommand in ["today", "lists", "tasks --list", "due", "apply"] {
        assert!(stdout.contains(subcommand), "help omits {subcommand}");
    }
}

#[test]
fn the_top_level_help_points_at_the_json_entry_point() {
    let out = oxidone().arg("--help").output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("oxidone json"));
}

#[test]
fn json_without_a_subcommand_fails_closed_with_json_on_stderr() {
    let out = oxidone().arg("json").output().unwrap();
    // Exit 2 is "bad request" in the contract's table.
    assert_eq!(out.status.code(), Some(2));
    // stdout stays empty on failure, so a caller can pipe it to a parser
    // unconditionally; the error is machine-readable on stderr.
    assert!(out.stdout.is_empty());
    let error: serde_json::Value =
        serde_json::from_slice(&out.stderr).expect("machine-readable JSON on stderr");
    assert_eq!(error["error"]["kind"], "usage");
}

#[test]
fn json_due_resolves_a_phrase_without_needing_credentials() {
    // The preview path is pure: no config, no token, no network. A caller can
    // echo the resolved date before committing to it even on a machine that has
    // never been authorized.
    let out = oxidone()
        .args(["json", "due", "2026-12-25"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {:?}",
        String::from_utf8_lossy(&out.stderr)
    );
    let payload: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(payload["due"], "2026-12-25");
    assert_eq!(payload["input"], "2026-12-25");
}

#[test]
fn json_due_refuses_a_phrase_that_is_not_a_date() {
    let out = oxidone().args(["json", "due", "milk"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "invalid_due");
}

#[test]
fn json_reports_an_unconfigured_machine_instead_of_reaching_for_one() {
    // No config at all. It must say so and stop — not fall back to anything, and
    // not go looking for credentials it was never given.
    let config = tempfile::tempdir().unwrap();
    let out = oxidone()
        .args(["json", "today"])
        .env("XDG_CONFIG_HOME", config.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "not_configured");
}

#[test]
fn json_never_opens_a_browser_for_a_missing_grant() {
    // The property that makes this safe to run from a bar plugin every few
    // minutes (ADR-0010): credentials are configured but no grant is stored, and
    // `oxidone json` reports it rather than starting a consent flow. The TUI in
    // the same state opens a browser and waits three minutes for a redirect —
    // which, unattended, is a browser window per poll.
    //
    // If this ever regresses it fails by *hanging* (and launching a browser),
    // which is ugly but still a failure.
    let config = tempfile::tempdir().unwrap();
    let dir = config.path().join("oxidone");
    std::fs::create_dir_all(&dir).unwrap();

    let secret = dir.join("client_secret.json");
    std::fs::write(
        &secret,
        r#"{"installed":{"client_id":"test-client","client_secret":"test-secret",
            "auth_uri":"https://accounts.google.com/o/oauth2/auth",
            "token_uri":"https://oauth2.googleapis.com/token",
            "redirect_uris":["http://localhost"]}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("config.toml"),
        format!("client_secret_path = {:?}\n", secret.display().to_string()),
    )
    .unwrap();
    // Deliberately no token.json: there is nothing to refresh.

    let out = oxidone()
        .args(["json", "today"])
        .env("XDG_CONFIG_HOME", config.path())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(3));
    assert!(out.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "auth_expired");
    // The remedy a headless caller actually has.
    assert!(error["error"]["message"]
        .as_str()
        .unwrap()
        .contains("run `oxidone` once"));
}

#[test]
fn json_apply_refuses_a_malformed_command_before_it_authorizes() {
    use std::io::Write;
    use std::process::Stdio;

    // The property: a request the caller got wrong is reported on a machine with
    // no credentials at all. Anything else would make the contract undebuggable
    // until you had a Google account wired up.
    let mut child = oxidone()
        .args(["json", "apply"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"op":"nonsense"}"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();

    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "usage");
}
