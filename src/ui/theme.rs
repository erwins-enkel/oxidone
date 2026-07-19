//! Catppuccin palette, four flavors, selectable from config (default Mocha).
//! btop supplies structure; Catppuccin supplies color — independent layers
//! (ADR-0006). Only the roles the UI actually paints are surfaced here; more
//! (overdue, done, warning…) join as data widgets land.

use ratatui::style::Color;

pub struct Theme {
    /// Window background.
    pub base: Color,
    /// Primary text.
    pub text: Color,
    /// Dimmed / secondary text.
    pub subtext: Color,
    /// Border of an unfocused panel.
    pub surface: Color,
    /// Focused panel border + highlights.
    pub accent: Color,
}

impl Theme {
    pub fn from_flavor(name: &str) -> Self {
        let flavor = match name.to_ascii_lowercase().as_str() {
            "latte" => &catppuccin::PALETTE.latte,
            "frappe" | "frappé" => &catppuccin::PALETTE.frappe,
            "macchiato" => &catppuccin::PALETTE.macchiato,
            _ => &catppuccin::PALETTE.mocha,
        };
        let c = &flavor.colors;
        Self {
            base: conv(&c.base),
            text: conv(&c.text),
            subtext: conv(&c.subtext0),
            surface: conv(&c.surface1),
            accent: conv(&c.mauve),
        }
    }
}

fn conv(c: &catppuccin::Color) -> Color {
    Color::Rgb(c.rgb.r, c.rgb.g, c.rgb.b)
}
