//! TOML config + platform paths (`directories`). Config dir holds the file and
//! the BYO `client_secret` path; data dir holds the SQLite DB; log dir holds
//! the rotating trace log.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to the user's BYO Google OAuth `client_secret.json` (ADR-0002).
    pub client_secret_path: std::path::PathBuf,
    /// Catppuccin flavor: "latte" | "frappe" | "macchiato" | "mocha".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Fall back to ASCII block bars where braille glyphs are unavailable.
    #[serde(default)]
    pub ascii_fallback: bool,
    // [keys] rebinding table is intentionally NOT in v1 (keymap is data; add later).
}

fn default_theme() -> String {
    "mocha".to_string()
}
