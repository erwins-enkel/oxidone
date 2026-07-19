//! Keymap-as-data (ADR-0005 spirit): modeless single-key bindings expressed as
//! a (Focus, KeyCode) -> Action table, not a match sprawl. This makes the `?`
//! cheatsheet fall out for free and makes user rebinding a later config add.

use crate::app::Focus;

/// A user-facing verb. Named stably so a future `[keys]` config can reference it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    MoveUp,
    MoveDown,
    SwitchPane,
    AddTask,
    EditTitle,
    EditNotes, // spawns $EDITOR
    ToggleComplete,
    SetDue,
    DeleteTask,
    Indent,  // -> move(parent=prev)
    Outdent, // -> move(parent=None)
    ReorderUp,
    ReorderDown,
    CycleSort,
    ToggleShowCompleted,
    ClearCompleted,
    NewList,
    RenameList,
    DeleteList,
    Refresh,
    Help,
    Quit,
}

/// The default, hardcoded binding table. `(context, key) -> action`.
pub fn default_bindings() -> Vec<(Focus, char, Action)> {
    todo!("populate lazygit-style single-key verbs")
}
