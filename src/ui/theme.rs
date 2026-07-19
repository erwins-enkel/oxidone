//! Catppuccin palette, four flavors, TOML-selectable (default Mocha). btop
//! supplies structure; Catppuccin supplies color — independent layers.

pub enum Flavor {
    Latte,
    Frappe,
    Macchiato,
    Mocha,
}

impl Flavor {
    pub fn from_config(name: &str) -> Self {
        match name {
            "latte" => Flavor::Latte,
            "frappe" => Flavor::Frappe,
            "macchiato" => Flavor::Macchiato,
            _ => Flavor::Mocha,
        }
    }
    // pub fn palette(&self) -> catppuccin::Flavor { ... }
    // Semantic role -> color: base, surface, overdue, due-soon, done, accent...
}
