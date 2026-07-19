//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer — the shell has
//! none of those yet.

use crate::keymap::{self, Action};

/// Which pane currently has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Tasks,
}

impl Focus {
    fn toggled(self) -> Self {
        match self {
            Focus::Sidebar => Focus::Tasks,
            Focus::Tasks => Focus::Sidebar,
        }
    }
}

/// The whole application state. `view(&Model)` renders it; nothing else does.
#[derive(Debug, Clone)]
pub struct Model {
    pub focus: Focus,
    pub show_help: bool,
    pub should_quit: bool,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            focus: Focus::Sidebar,
            show_help: false,
            should_quit: false,
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Everything that can happen. Keys today; worker results (loads, write results,
/// ticks) join as later slices land.
#[derive(Debug)]
pub enum Message {
    Key(crossterm::event::KeyEvent),
}

/// Side-effect requests emitted by `update` for workers to run. The shell emits
/// none; variants arrive with the slices that touch I/O (load, write, editor…).
#[derive(Debug)]
pub enum Command {}

/// The pure reducer. Applies a `Message` to the `Model` and returns any
/// side-effect `Command`s for workers to run.
pub fn update(model: &mut Model, msg: Message) -> Vec<Command> {
    match msg {
        Message::Key(key) => {
            if let Some(action) = keymap::resolve(key) {
                apply(model, action);
            }
        }
    }
    Vec::new()
}

fn apply(model: &mut Model, action: Action) {
    match action {
        Action::Quit => model.should_quit = true,
        Action::ToggleHelp => model.show_help = !model.show_help,
        Action::CloseOverlay => model.show_help = false,
        Action::SwitchPane => model.focus = model.focus.toggled(),
    }
}
