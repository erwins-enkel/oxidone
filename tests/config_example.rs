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
