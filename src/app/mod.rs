//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer.

use crate::domain::{List, ListId};
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
    pub lists: Vec<List>,
    /// Index into `lists` of the active List, if any.
    pub selected_list: Option<usize>,
    pub focus: Focus,
    pub show_help: bool,
    pub should_quit: bool,
    /// Transient one-line message (load errors now; toasts later).
    pub status_line: Option<String>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            lists: Vec::new(),
            selected_list: None,
            focus: Focus::Sidebar,
            show_help: false,
            should_quit: false,
            status_line: None,
        }
    }
}

impl Model {
    pub fn new() -> Self {
        Self::default()
    }

    /// The `ListId` of the active List, if one is selected.
    pub fn selected_list_id(&self) -> Option<&ListId> {
        self.selected_list
            .and_then(|i| self.lists.get(i))
            .map(|l| &l.id)
    }
}

/// Everything that can happen. Keys plus worker results; more join as slices land.
#[derive(Debug)]
pub enum Message {
    Key(crossterm::event::KeyEvent),
    /// The current set of Lists (from cache at startup, or a refresh).
    ListsLoaded(Vec<List>),
    /// A load failed; the reason is shown on the status line.
    LoadFailed(String),
}

/// Side-effect requests emitted by `update` for workers to run. Variants arrive
/// with the slices that touch I/O (refresh, write, editor…).
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
        Message::ListsLoaded(lists) => set_lists(model, lists),
        Message::LoadFailed(reason) => model.status_line = Some(reason),
    }
    Vec::new()
}

/// Replace the List set, keeping the active List selected by id where possible.
fn set_lists(model: &mut Model, lists: Vec<List>) {
    let previously_selected = model.selected_list_id().cloned();
    model.lists = lists;
    model.selected_list = if model.lists.is_empty() {
        None
    } else {
        previously_selected
            .and_then(|id| model.lists.iter().position(|l| l.id == id))
            .or(Some(0))
    };
    model.status_line = None;
}

fn apply(model: &mut Model, action: Action) {
    match action {
        Action::Quit => model.should_quit = true,
        Action::ToggleHelp => model.show_help = !model.show_help,
        Action::CloseOverlay => model.show_help = false,
        Action::SwitchPane => model.focus = model.focus.toggled(),
        Action::SelectNext => move_selection(model, 1),
        Action::SelectPrev => move_selection(model, -1),
    }
}

/// Move the sidebar selection by `delta`, clamped to the List bounds. Only the
/// sidebar has a selection today, so this is a no-op when the task pane is focused.
fn move_selection(model: &mut Model, delta: isize) {
    if model.focus != Focus::Sidebar || model.lists.is_empty() {
        return;
    }
    let Some(current) = model.selected_list else {
        return;
    };
    let last = model.lists.len().saturating_sub(1);
    let next = (current as isize + delta).clamp(0, last as isize) as usize;
    model.selected_list = Some(next);
}
