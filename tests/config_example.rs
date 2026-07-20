//! Guards `config.example.toml` (the file `make config` scaffolds) against
//! drifting from `Config`. Two properties:
//!
//! 1. Its declared keys — counting commented-out assignments as declared —
//!    exactly match a fully-populated `Config`. A bare "it deserializes" check
//!    would be worthless: `Config` is `#[serde(default)]`, so a field added to
//!    `Config` and forgotten here would still deserialize and pass.
//! 2. Parsed as-is (comments as comments), `client_secret_path` is `None`, so a
//!    freshly scaffolded config never triggers the first-run auth path.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use oxidone::config::Config;

const EXAMPLE: &str = include_str!("../config.example.toml");

/// A line "declares a key" if it parses as a TOML assignment either as-is or
/// after stripping one leading `#`. Prose comments parse as neither and drop.
fn assignment(line: &str) -> Option<toml::Table> {
    line.parse::<toml::Table>().ok().filter(|t| !t.is_empty())
}

fn declared_keys(src: &str) -> BTreeSet<String> {
    let mut keys = BTreeSet::new();
    for line in src.lines() {
        let candidate = assignment(line)
            .or_else(|| assignment(line.trim_start().strip_prefix('#')?.trim_start()));
        if let Some(table) = candidate {
            keys.extend(table.keys().cloned());
        }
    }
    keys
}

#[test]
fn example_key_set_matches_config() {
    // Populate every field: `None` would be omitted by the serializer, which
    // would let the comparison pass with `client_secret_path` absent.
    let full = Config {
        client_secret_path: Some("/x".into()),
        ..Config::default()
    };
    let full: toml::Table = toml::to_string(&full).unwrap().parse().unwrap();
    let full_keys: BTreeSet<String> = full.keys().cloned().collect();

    assert_eq!(
        declared_keys(EXAMPLE),
        full_keys,
        "config.example.toml keys drifted from Config"
    );
}

#[test]
fn example_scaffolds_offline() {
    // As-is (client_secret_path commented out), it must deserialize with no
    // secret path — otherwise a scaffolded config would try to authenticate.
    let config: Config = toml::from_str(EXAMPLE).unwrap();
    assert!(config.client_secret_path.is_none());
}

const HOME: &str = "/home/tester";

/// Deserialize a config with the given `client_secret_path` literal, expand it
/// against a fixed fake home, and return the resulting path.
fn expanded_secret_path(literal: &str) -> Option<PathBuf> {
    let src = format!("client_secret_path = {literal:?}");
    let config: Config = toml::from_str(&src).unwrap();
    config.expand_paths(Path::new(HOME)).client_secret_path
}

#[test]
fn expand_paths_wires_client_secret_path() {
    // Wiring test: a `~/...` value in the deserialized config must expand *that*
    // field — catches "forgot to wire" / "wrong field" regressions.
    assert_eq!(
        expanded_secret_path("~/.config/oxidone/client_secret.json"),
        Some(PathBuf::from(
            "/home/tester/.config/oxidone/client_secret.json"
        )),
    );
}

#[test]
fn expand_paths_bare_tilde() {
    assert_eq!(expanded_secret_path("~"), Some(PathBuf::from(HOME)));
}

#[test]
fn expand_paths_leaves_tilde_user_verbatim() {
    assert_eq!(
        expanded_secret_path("~alice/secret.json"),
        Some(PathBuf::from("~alice/secret.json")),
    );
}

#[test]
fn expand_paths_leaves_absolute_verbatim() {
    assert_eq!(
        expanded_secret_path("/etc/oxidone/abs.json"),
        Some(PathBuf::from("/etc/oxidone/abs.json")),
    );
}

#[test]
fn expand_paths_leaves_relative_verbatim() {
    assert_eq!(
        expanded_secret_path("relative/secret.json"),
        Some(PathBuf::from("relative/secret.json")),
    );
}

#[test]
fn expand_paths_leaves_mid_path_tilde_verbatim() {
    // `~` only expands as the first component.
    assert_eq!(
        expanded_secret_path("a/~/b.json"),
        Some(PathBuf::from("a/~/b.json")),
    );
}

#[test]
fn expand_paths_leaves_none_none() {
    let config = Config::default();
    assert!(config.client_secret_path.is_none());
    assert!(config
        .expand_paths(Path::new(HOME))
        .client_secret_path
        .is_none());
}
