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
    SelectNext,
    SelectPrev,
    ToggleComplete,
    AddTask,
    EditTitle,
    EditDue,
    EditNotes,
    DeleteTask,
    CycleSort,
    ToggleShowCompleted,
    ClearCompleted,
    // Sidebar List management. Bound to capitals so they never clash with the
    // task-pane verbs (`a`/`e`/`x`); the reducer additionally gates them on the
    // sidebar being focused.
    AddList,
    RenameList,
    DeleteList,
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
        Binding {
            key: KeyCode::Char('j'),
            action: Action::SelectNext,
            help: "select next",
        },
        Binding {
            key: KeyCode::Down,
            action: Action::SelectNext,
            help: "select next",
        },
        Binding {
            key: KeyCode::Char('k'),
            action: Action::SelectPrev,
            help: "select previous",
        },
        Binding {
            key: KeyCode::Up,
            action: Action::SelectPrev,
            help: "select previous",
        },
        Binding {
            key: KeyCode::Char(' '),
            action: Action::ToggleComplete,
            help: "toggle complete",
        },
        Binding {
            key: KeyCode::Char('a'),
            action: Action::AddTask,
            help: "add task",
        },
        Binding {
            key: KeyCode::Char('e'),
            action: Action::EditTitle,
            help: "edit title",
        },
        // `Enter` is the natural "open this row" affordance; for now it is an
        // alias of `e`. Overlay keys are routed before the keymap, so this never
        // shadows Enter-to-submit inside an overlay.
        Binding {
            key: KeyCode::Enter,
            action: Action::EditTitle,
            help: "edit title",
        },
        Binding {
            key: KeyCode::Char('d'),
            action: Action::EditDue,
            help: "edit due date",
        },
        Binding {
            key: KeyCode::Char('n'),
            action: Action::EditNotes,
            help: "edit notes ($EDITOR)",
        },
        Binding {
            key: KeyCode::Char('x'),
            action: Action::DeleteTask,
            help: "delete task",
        },
        Binding {
            key: KeyCode::Char('s'),
            action: Action::CycleSort,
            help: "cycle sort (manual/due/title)",
        },
        Binding {
            key: KeyCode::Char('c'),
            action: Action::ToggleShowCompleted,
            help: "show/hide completed",
        },
        Binding {
            key: KeyCode::Char('C'),
            action: Action::ClearCompleted,
            help: "clear completed",
        },
        Binding {
            key: KeyCode::Char('A'),
            action: Action::AddList,
            help: "add list",
        },
        Binding {
            key: KeyCode::Char('R'),
            action: Action::RenameList,
            help: "rename list",
        },
        Binding {
            key: KeyCode::Char('X'),
            action: Action::DeleteList,
            help: "delete list",
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
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        other => format!("{other:?}"),
    }
}
