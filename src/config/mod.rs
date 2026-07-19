//! TOML config + platform paths (`directories`). The config dir holds the file
//! and the BYO `client_secret` path; the data dir holds the SQLite DB; the log
//! dir holds the rotating trace log.
//!
//! Loading is tolerant: a missing or malformed file falls back to defaults, so
//! the shell runs before the user has written any config (auth lands in a later
//! slice).

use std::path::PathBuf;

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path to the user's BYO Google OAuth `client_secret.json` (ADR-0002).
    /// `None` until the user configures auth.
    pub client_secret_path: Option<PathBuf>,
    /// Catppuccin flavor: "latte" | "frappe" | "macchiato" | "mocha".
    pub theme: String,
    /// Render ASCII block bars where braille glyphs are unavailable.
    pub ascii_fallback: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_secret_path: None,
            theme: "mocha".to_string(),
            ascii_fallback: false,
        }
    }
}

impl Config {
    /// Load config from the platform config dir, falling back to defaults if the
    /// file is absent. A present-but-unreadable/malformed file also falls back —
    /// but logs a warning rather than silently resetting every setting.
    pub fn load() -> Self {
        let Some(path) = config_file() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "malformed config; using defaults");
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not read config; using defaults");
                Self::default()
            }
        }
    }
}

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("", "", "oxidone")
}

/// `<config dir>/config.toml`, e.g. `~/.config/oxidone/config.toml` on Linux.
pub fn config_file() -> Option<PathBuf> {
    project_dirs().map(|d| d.config_dir().join("config.toml"))
}

/// Where the rotating trace log lives. Prefers the platform state dir (Linux
/// `~/.local/state/oxidone`), falling back to `<data dir>/logs` on platforms
/// the `directories` crate gives no state dir for (macOS/Windows).
pub fn log_dir() -> Option<PathBuf> {
    project_dirs().map(|d| {
        d.state_dir()
            .map(|s| s.join("logs"))
            .unwrap_or_else(|| d.data_local_dir().join("logs"))
    })
}

/// `<data dir>/oxidone.db` — the local SQLite cache.
pub fn db_path() -> Option<PathBuf> {
    project_dirs().map(|d| d.data_local_dir().join("oxidone.db"))
}
