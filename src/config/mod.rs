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

/// A Catppuccin flavor, parsed from [`Config::theme`].
///
/// Lives here rather than in `ui::theme` because `app::Model` holds one and
/// `app` depends on nothing in `ui` — the arrow runs `ui → app`, and the reducer
/// is the terminal-free core (ADR-0005). `config` is where the setting comes
/// from, imports neither, and so may be depended on by both.
///
/// `Config::theme` stays a `String`: making it a `Flavor` would move parsing into
/// serde, where one bad value fails the whole `toml::from_str` and — per
/// [`Config::load`]'s tolerant fallback — silently resets *every* setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Flavor {
    Latte,
    Frappe,
    Macchiato,
    #[default]
    Mocha,
}

impl Flavor {
    /// Every flavor, in the order the `:flavor` row's refusal lists them —
    /// that message is built from this, so a fifth variant cannot leave it
    /// naming four.
    pub const ALL: [Flavor; 4] = [
        Flavor::Latte,
        Flavor::Frappe,
        Flavor::Macchiato,
        Flavor::Mocha,
    ];

    /// The canonical name, as `ui::theme::Theme::from_flavor` expects it.
    pub fn as_str(self) -> &'static str {
        match self {
            Flavor::Latte => "latte",
            Flavor::Frappe => "frappe",
            Flavor::Macchiato => "macchiato",
            Flavor::Mocha => "mocha",
        }
    }

    /// Parse a flavor name, or `None` if it names no flavor.
    ///
    /// **Fails closed, unlike `Theme::from_flavor`**, whose `_ => mocha` arm
    /// accepts anything — so an unknown name there silently paints Mocha and
    /// reports success. The Omnibox's `:flavor` command needs the refusal.
    ///
    /// Exactly as tolerant as `from_flavor` otherwise, because `main.rs` now
    /// seeds the model *through* this: case-insensitive, and `frappé` accepted
    /// alongside `frappe`. A config value that works today must keep working.
    pub fn from_name(name: &str) -> Option<Flavor> {
        match name.to_ascii_lowercase().as_str() {
            "latte" => Some(Flavor::Latte),
            "frappe" | "frappé" => Some(Flavor::Frappe),
            "macchiato" => Some(Flavor::Macchiato),
            "mocha" => Some(Flavor::Mocha),
            _ => None,
        }
    }
}

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

#[cfg(test)]
mod tests {
    use super::Flavor;

    /// `main.rs` now seeds `Model::flavor` through `from_name`, so every spelling
    /// `Theme::from_flavor` accepts today has to survive the new hop — otherwise a
    /// working `config.toml` silently regresses to Mocha.
    #[test]
    fn from_name_is_as_tolerant_as_from_flavor() {
        for (name, want) in [
            ("latte", Flavor::Latte),
            ("Latte", Flavor::Latte),
            ("MOCHA", Flavor::Mocha),
            ("frappe", Flavor::Frappe),
            ("frappé", Flavor::Frappe),
            ("Macchiato", Flavor::Macchiato),
        ] {
            assert_eq!(Flavor::from_name(name), Some(want), "input {name:?}");
        }
    }

    /// The half `Theme::from_flavor` cannot do: an unknown name is a refusal, not
    /// a silent fall back to Mocha.
    #[test]
    fn from_name_refuses_an_unknown_flavor() {
        assert_eq!(Flavor::from_name("purple"), None);
        assert_eq!(Flavor::from_name(""), None);
    }

    #[test]
    fn every_flavor_round_trips_through_its_own_name() {
        for f in Flavor::ALL {
            assert_eq!(Flavor::from_name(f.as_str()), Some(f), "flavor {f:?}");
        }
    }
}
