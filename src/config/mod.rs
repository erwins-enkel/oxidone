//! TOML config + platform paths (`directories`). The config dir holds the file
//! and the BYO `client_secret` path; the data dir holds the SQLite DB; the log
//! dir holds the rotating trace log.
//!
//! Loading is tolerant: a missing or malformed file falls back to defaults, so
//! the shell runs before the user has written any config (auth lands in a later
//! slice).

use std::path::{Path, PathBuf};

use directories::{BaseDirs, ProjectDirs};
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
    /// Startup default for the "hide distant tasks" view filter: when on, entries
    /// due more than `horizon_days` past today are hidden from the pane. A
    /// keybinding (`w`) toggles it live; this only seeds the initial state.
    pub hide_distant: bool,
    /// The horizon for `hide_distant`, in days from today. Entries due strictly
    /// more than this many days out are hidden while the filter is on. Undated
    /// entries are never distant. Preserved across toggles (see the two-field
    /// rationale in the design), so it holds even while `hide_distant` is off.
    pub horizon_days: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            client_secret_path: None,
            theme: "mocha".to_string(),
            ascii_fallback: false,
            hide_distant: false,
            horizon_days: 14,
        }
    }
}

impl Config {
    /// Load config from the platform config dir, falling back to defaults if the
    /// file is absent. A present-but-unreadable/malformed file also falls back —
    /// but logs a warning rather than silently resetting every setting.
    ///
    /// Tilde expansion happens here and only here: any other
    /// `toml::from_str::<Config>` call site receives paths verbatim and must not
    /// assume they are pre-expanded.
    pub fn load() -> Self {
        let Some(path) = config_file() else {
            return Self::default();
        };
        let config = match std::fs::read_to_string(&path) {
            Ok(contents) => toml::from_str(&contents).unwrap_or_else(|e| {
                tracing::warn!(path = %path.display(), error = %e, "malformed config; using defaults");
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "could not read config; using defaults");
                Self::default()
            }
        };
        match BaseDirs::new() {
            Some(dirs) => config.expand_paths(dirs.home_dir()),
            // No home resolvable: leave paths verbatim so the later file read
            // surfaces the real error rather than us inventing a path.
            None => config,
        }
    }

    /// Expand a leading `~`/`~/` in every path field against `home`. Applied once
    /// in [`Config::load`]; a future path field is one added `.map(...)` line.
    pub fn expand_paths(mut self, home: &Path) -> Self {
        self.client_secret_path = self.client_secret_path.map(|p| expand_tilde(p, home));
        self
    }
}

/// Expand a leading `~`/`~/` in `path` against `home`. `~user` and any non-tilde
/// path (absolute, relative, or literal) are returned unchanged.
///
/// `strip_prefix("~")` treats `~` as an ordinary path component, so it matches
/// only when the first component is exactly `~`: `~/x` → `home/x`, bare `~` →
/// `home`, and `~user`/`/abs`/`rel` fall through unchanged. Operates on the OS
/// string, so non-UTF-8 paths are handled without lossy conversion.
fn expand_tilde(path: PathBuf, home: &Path) -> PathBuf {
    match path.strip_prefix("~") {
        Ok(rest) => home.join(rest),
        Err(_) => path,
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
