//! Keymap-as-data (ADR-0005 spirit): modeless single-key bindings expressed as
//! a table of `(key -> Action)`, not a match sprawl. The `?` cheatsheet is
//! rendered straight from this table, and user rebinding (a later ticket) is a
//! matter of loading a different table. Context-sensitivity (per-pane keys)
//! joins the table as slices need it.

use crossterm::event::{KeyCode, KeyEvent};

/// A user-facing verb. Grows as slices add behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleHelp,
    CloseOverlay,
    SwitchPane,
}

/// One row of the keymap: the key, the verb it triggers, and its cheatsheet text.
pub struct Binding {
    pub key: KeyCode,
    pub action: Action,
    pub help: &'static str,
}

/// The default, hardcoded binding table.
pub fn bindings() -> &'static [Binding] {
    const BINDINGS: &[Binding] = &[
        Binding {
            key: KeyCode::Char('q'),
            action: Action::Quit,
            help: "quit",
        },
        Binding {
            key: KeyCode::Char('?'),
            action: Action::ToggleHelp,
            help: "toggle this help",
        },
        Binding {
            key: KeyCode::Tab,
            action: Action::SwitchPane,
            help: "switch pane",
        },
        Binding {
            key: KeyCode::Esc,
            action: Action::CloseOverlay,
            help: "close overlay",
        },
    ];
    BINDINGS
}

/// Resolve a key press to its bound `Action`, if any. Modifiers are ignored for
/// now — the shell's verbs are all plain keys.
pub fn resolve(key: KeyEvent) -> Option<Action> {
    bindings()
        .iter()
        .find(|b| b.key == key.code)
        .map(|b| b.action)
}

/// A short label for a key, for the cheatsheet.
pub fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        other => format!("{other:?}"),
    }
}
