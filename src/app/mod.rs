//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer, and `update`
//! emits `Command`s for them to run.

use std::collections::HashMap;

use crate::domain::{List, ListId, Status, Task, TaskId};
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
    /// Tasks of the active List (in Manual order).
    pub tasks: Vec<Task>,
    /// Index into `tasks` of the cursor, if any.
    pub selected_task: Option<usize>,
    pub focus: Focus,
    pub show_help: bool,
    pub should_quit: bool,
    /// Transient one-line message (load errors now; toasts later).
    pub status_line: Option<String>,
    /// Tasks with a completion write in flight, mapped to their pre-write
    /// snapshot for rollback. Acts as a single-flight guard: a Task already
    /// mid-write ignores further toggles, so no two writes race the same Task.
    pending_writes: HashMap<TaskId, Task>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            lists: Vec::new(),
            selected_list: None,
            tasks: Vec::new(),
            selected_task: None,
            focus: Focus::Sidebar,
            show_help: false,
            should_quit: false,
            status_line: None,
            pending_writes: HashMap::new(),
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

/// Everything that can happen. Keys plus worker results.
#[derive(Debug)]
pub enum Message {
    Key(crossterm::event::KeyEvent),
    /// The current set of Lists (from cache at startup, or a refresh).
    ListsLoaded(Vec<List>),
    /// The Tasks of a specific List. Ignored if that List is no longer active.
    TasksLoaded(ListId, Vec<Task>),
    /// A write succeeded; reconcile the Model with the server's Task.
    TaskUpdated(Task),
    /// A completion write failed; roll back to the pre-write snapshot.
    TaskWriteFailed {
        task: TaskId,
        reason: String,
    },
    /// A load failed; the reason is shown on the status line.
    LoadFailed(String),
}

/// Side-effect requests emitted by `update` for workers to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Load the Tasks of a List (from cache, and later a live refresh).
    LoadTasks(ListId),
    /// Write-through a completion toggle for a Task.
    SetCompleted {
        list: ListId,
        task: TaskId,
        completed: bool,
    },
}

/// The pure reducer. Applies a `Message` to the `Model` and returns any
/// side-effect `Command`s for workers to run.
pub fn update(model: &mut Model, msg: Message) -> Vec<Command> {
    match msg {
        Message::Key(key) => match keymap::resolve(key) {
            Some(action) => apply(model, action),
            None => Vec::new(),
        },
        Message::ListsLoaded(lists) => set_lists(model, lists),
        Message::TasksLoaded(list, tasks) => {
            set_tasks(model, &list, tasks);
            Vec::new()
        }
        Message::TaskUpdated(task) => {
            // The write completed; drop the snapshot and adopt the server Task.
            // Safe against races: while pending, the single-flight guard blocks
            // further toggles of this Task, so no newer local intent exists.
            model.pending_writes.remove(&task.id);
            if let Some(slot) = model.tasks.iter_mut().find(|t| t.id == task.id) {
                *slot = task;
            }
            Vec::new()
        }
        Message::TaskWriteFailed { task, reason } => {
            if let Some(previous) = model.pending_writes.remove(&task) {
                if let Some(slot) = model.tasks.iter_mut().find(|t| t.id == previous.id) {
                    *slot = previous; // exact pre-write state, incl. completed_at
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::LoadFailed(reason) => {
            model.status_line = Some(reason);
            Vec::new()
        }
    }
}

/// Set a Task's completed state (locally). Completing leaves `completed_at` for
/// the server response to fill; un-completing clears it.
fn set_completed(task: &mut Task, completed: bool) {
    task.status = if completed {
        Status::Completed
    } else {
        Status::NeedsAction
    };
    if !completed {
        task.completed_at = None;
    }
}

/// Replace the List set, keeping the active List selected by id where possible,
/// then request that List's Tasks.
fn set_lists(model: &mut Model, lists: Vec<List>) -> Vec<Command> {
    let previously_selected = model.selected_list_id().cloned();
    model.lists = lists;
    model.selected_list = if model.lists.is_empty() {
        None
    } else {
        previously_selected
            .as_ref()
            .and_then(|id| model.lists.iter().position(|l| l.id == *id))
            .or(Some(0))
    };
    model.status_line = None;
    // Only wipe the pane when the active List actually changed; a refresh that
    // keeps the same List reloads its Tasks in place, without a blank flash.
    let list_changed = previously_selected.as_ref() != model.selected_list_id();
    request_selected_tasks(model, list_changed)
}

/// Request the active List's Tasks. When `clear_pane`, the pane is emptied
/// first (a List change); otherwise the current Tasks stay until the new ones
/// arrive. With no List selected, the pane is always emptied.
fn request_selected_tasks(model: &mut Model, clear_pane: bool) -> Vec<Command> {
    match model.selected_list_id().cloned() {
        Some(id) => {
            if clear_pane {
                model.tasks.clear();
                model.selected_task = None;
            }
            vec![Command::LoadTasks(id)]
        }
        None => {
            model.tasks.clear();
            model.selected_task = None;
            Vec::new()
        }
    }
}

/// Fill the task pane, ignoring results for a List that is no longer active.
/// Keeps the task cursor on the same Task by id where possible.
fn set_tasks(model: &mut Model, list: &ListId, tasks: Vec<Task>) {
    if model.selected_list_id() != Some(list) {
        return;
    }
    let previously_selected = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .map(|t| t.id.clone());
    model.tasks = tasks;
    model.selected_task = if model.tasks.is_empty() {
        None
    } else {
        previously_selected
            .and_then(|id| model.tasks.iter().position(|t| t.id == id))
            .or(Some(0))
    };
}

fn apply(model: &mut Model, action: Action) -> Vec<Command> {
    match action {
        Action::Quit => model.should_quit = true,
        Action::ToggleHelp => model.show_help = !model.show_help,
        Action::CloseOverlay => model.show_help = false,
        Action::SwitchPane => model.focus = model.focus.toggled(),
        Action::SelectNext => return move_selection(model, 1),
        Action::SelectPrev => return move_selection(model, -1),
        Action::ToggleComplete => return toggle_complete(model),
    }
    Vec::new()
}

/// Toggle the selected Task's completion optimistically and request the
/// write-through. A no-op unless the task pane is focused with a Task selected.
fn toggle_complete(model: &mut Model) -> Vec<Command> {
    if model.focus != Focus::Tasks {
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(index) = model.selected_task else {
        return Vec::new();
    };
    let id = model.tasks[index].id.clone();
    // Single-flight: ignore a toggle while this Task's write is in flight.
    if model.pending_writes.contains_key(&id) {
        return Vec::new();
    }
    let snapshot = model.tasks[index].clone();
    let completed = model.tasks[index].status == Status::NeedsAction; // completing iff open
    set_completed(&mut model.tasks[index], completed);
    model.pending_writes.insert(id.clone(), snapshot);
    vec![Command::SetCompleted {
        list,
        task: id,
        completed,
    }]
}

/// Move the cursor in the focused pane. In the sidebar this changes the active
/// List and requests its Tasks; in the task pane it just moves the cursor.
fn move_selection(model: &mut Model, delta: isize) -> Vec<Command> {
    match model.focus {
        Focus::Sidebar => move_list_selection(model, delta),
        Focus::Tasks => {
            move_index(&mut model.selected_task, model.tasks.len(), delta);
            Vec::new()
        }
    }
}

fn move_list_selection(model: &mut Model, delta: isize) -> Vec<Command> {
    let before = model.selected_list;
    move_index(&mut model.selected_list, model.lists.len(), delta);
    if model.selected_list == before {
        return Vec::new();
    }
    request_selected_tasks(model, true)
}

/// Move a selection index by `delta`, clamped to `[0, len)`. No-op on empty.
fn move_index(selection: &mut Option<usize>, len: usize, delta: isize) {
    if len == 0 {
        return;
    }
    let Some(current) = *selection else {
        return;
    };
    let last = (len - 1) as isize;
    *selection = Some((current as isize + delta).clamp(0, last) as usize);
}
