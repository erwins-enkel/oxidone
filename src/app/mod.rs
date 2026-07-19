//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer.

use crate::domain::{List, ListId, SortView, Task, TaskId};

/// The whole application state. `view(&Model)` renders it; nothing else does.
pub struct Model {
    pub lists: Vec<List>,
    pub tasks: Vec<Task>, // for the active list (+ subtasks)
    pub focus: Focus,
    pub active_list: Option<ListId>,
    pub selected: Option<TaskId>,
    pub sort: SortView,
    pub show_completed: bool,
    pub input: Option<InputMode>, // add/edit/rename/confirm overlays
    pub status_line: Option<String>,
    // pending: in-flight optimistic ops keyed for rollback on failure (ADR-0001)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Tasks,
}

pub enum InputMode {
    AddTask,
    EditTitle(TaskId),
    RenameList(ListId),
    ConfirmDestructive(String),
    DueEntry(TaskId), // natural-language + ISO
}

/// Everything that can happen. Keys, API results, and ticks all become these.
pub enum Message {
    Key(crossterm::event::KeyEvent),
    Tick,
    // --- results from workers ---
    ListsLoaded(Vec<List>),
    TasksLoaded(ListId, Vec<Task>),
    WriteOk(TaskId),
    WriteFailed(TaskId, String), // triggers optimistic rollback
    AuthExpired,
}

/// The pure reducer. Returns side-effect requests (Commands) for workers to run.
pub fn update(_model: &mut Model, _msg: Message) -> Vec<Command> {
    todo!("the heart of the app; keep it pure and exhaustively tested")
}

/// A request for a worker to perform I/O and report back via a `Message`.
pub enum Command {
    LoadLists,
    LoadTasks(ListId),
    Insert(ListId, crate::api::NewTask),
    Patch(ListId, TaskId, crate::api::TaskPatch),
    Move(ListId, TaskId, Option<TaskId>, Option<TaskId>),
    ClearCompleted(ListId),
    SpawnEditor(TaskId), // suspend TUI, open $EDITOR for notes
}
