//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer, and `update`
//! emits `Command`s for them to run.

use std::collections::HashMap;

use crate::domain::{List, ListId, SortView, Status, Task, TaskId};
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
    /// Local, read-only reordering of the task pane. Never mutates `tasks`
    /// (Manual order) and never writes `position` to Google.
    pub sort: SortView,
    pub focus: Focus,
    pub show_help: bool,
    pub should_quit: bool,
    /// Transient one-line message (load errors now; toasts later).
    pub status_line: Option<String>,
    /// The active modal overlay (text input or confirmation), if any. While set,
    /// keys route to the overlay instead of the normal keymap.
    pub overlay: Option<Overlay>,
    /// Tasks with a field write in flight, mapped to their pre-write snapshot for
    /// rollback. Acts as a single-flight guard: a Task already mid-write ignores
    /// further edits, so no two writes race the same Task.
    pending_writes: HashMap<TaskId, Task>,
    /// Optimistically-removed Tasks awaiting delete confirmation from the server,
    /// mapped to their prior (index, Task) for rollback on failure.
    pending_deletes: HashMap<TaskId, (usize, Task)>,
    /// Counter for minting placeholder ids for optimistically-added Tasks, before
    /// the server assigns the real id.
    next_temp: u64,
}

/// A modal overlay drawn over the panes.
#[derive(Debug, Clone)]
pub enum Overlay {
    /// In-place title editor for a Task.
    EditTitle { task: TaskId, buffer: String },
    /// Capture a new Task's title.
    AddTask { buffer: String },
    /// A reusable destructive-action confirmation (delete Task now; Clear and
    /// List delete later).
    Confirm(Confirm),
}

impl Overlay {
    /// The editable text buffer of a text-input overlay, if this is one.
    fn input_buffer(&mut self) -> Option<&mut String> {
        match self {
            Overlay::EditTitle { buffer, .. } | Overlay::AddTask { buffer } => Some(buffer),
            Overlay::Confirm(_) => None,
        }
    }
}

/// A yes/no confirmation of a destructive action.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub prompt: String,
    pub action: ConfirmAction,
}

/// The action a [`Confirm`] performs on "yes". Grows as destructive ops land.
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteTask { list: ListId, task: TaskId },
}

impl Default for Model {
    fn default() -> Self {
        Self {
            lists: Vec::new(),
            selected_list: None,
            tasks: Vec::new(),
            selected_task: None,
            sort: SortView::Manual,
            focus: Focus::Sidebar,
            show_help: false,
            should_quit: false,
            status_line: None,
            overlay: None,
            pending_writes: HashMap::new(),
            pending_deletes: HashMap::new(),
            next_temp: 0,
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

    /// The Tasks in the current `sort`'s **display order**. A pure, read-only
    /// lens: it borrows `tasks` and never mutates Manual order (`position`) nor
    /// emits any Command — the view renders this, the model is untouched.
    ///
    /// - `Manual`: stored order (by `position`, i.e. the current Vec order).
    /// - `Due`: due date ascending; Tasks with no due date sink to the bottom,
    ///   stable within that no-due group (a deterministic tail).
    /// - `Title`: case-insensitive by title, stable on ties.
    ///
    /// Known limitation: `j`/`k` still move `selected_task` in stored order, so
    /// under `Due`/`Title` the highlight can jump between non-adjacent rows.
    /// Making navigation follow the display order is a follow-up.
    pub fn sorted_tasks(&self) -> Vec<&Task> {
        let mut ordered: Vec<&Task> = self.tasks.iter().collect();
        match self.sort {
            SortView::Manual => {}
            // Stable sort keeps the stored order as the tie-breaker, so the
            // no-due tail (and same-day Tasks) stay in Manual order.
            SortView::Due => ordered.sort_by(|a, b| match (a.due, b.due) {
                (Some(x), Some(y)) => x.cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }),
            SortView::Title => ordered.sort_by_cached_key(|t| t.title.to_lowercase()),
        }
        ordered
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
    /// A field write failed; roll back to the pre-write snapshot.
    TaskWriteFailed {
        task: TaskId,
        reason: String,
    },
    /// A delete succeeded; drop the optimistic-delete bookkeeping.
    TaskDeleted(TaskId),
    /// A delete failed; re-insert the Task at its prior position.
    TaskDeleteFailed {
        task: TaskId,
        reason: String,
    },
    /// An add succeeded; replace the placeholder (by temp id) with the server Task.
    TaskInserted {
        temp: TaskId,
        task: Task,
    },
    /// An add failed; drop the placeholder (by temp id).
    TaskAddFailed {
        temp: TaskId,
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
    /// Write-through a title edit for a Task.
    SetTitle {
        list: ListId,
        task: TaskId,
        title: String,
    },
    /// Delete a Task.
    DeleteTask { list: ListId, task: TaskId },
    /// Insert a new Task into a List; `temp` echoes back so the placeholder can
    /// be reconciled with the server Task.
    AddTask {
        list: ListId,
        temp: TaskId,
        title: String,
    },
}

/// The pure reducer. Applies a `Message` to the `Model` and returns any
/// side-effect `Command`s for workers to run.
pub fn update(model: &mut Model, msg: Message) -> Vec<Command> {
    match msg {
        Message::Key(key) => {
            if model.overlay.is_some() {
                return overlay_key(model, key);
            }
            match keymap::resolve(key) {
                Some(action) => apply(model, action),
                None => Vec::new(),
            }
        }
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
        Message::TaskDeleted(task) => {
            // Confirmed gone; drop the rollback snapshot (Task already removed).
            model.pending_deletes.remove(&task);
            Vec::new()
        }
        Message::TaskDeleteFailed { task, reason } => {
            if let Some((index, previous)) = model.pending_deletes.remove(&task) {
                // Guard against a refresh having already re-added it: only
                // re-insert when it's genuinely absent, so we can't duplicate.
                if !model.tasks.iter().any(|t| t.id == previous.id) {
                    let at = index.min(model.tasks.len());
                    model.tasks.insert(at, previous);
                    clamp_task_selection(model);
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::TaskInserted { temp, task } => {
            // Reconcile the optimistic placeholder with the server's real Task.
            if let Some(slot) = model.tasks.iter_mut().find(|t| t.id == temp) {
                *slot = task;
            } else if !model.tasks.iter().any(|t| t.id == task.id) {
                // A refresh wiped the placeholder before the reply; don't lose
                // the confirmed Task (and don't duplicate one a refresh added).
                model.tasks.insert(0, task);
            }
            Vec::new()
        }
        Message::TaskAddFailed { temp, reason } => {
            model.tasks.retain(|t| t.id != temp);
            clamp_task_selection(model);
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
        Action::AddTask => open_add_task(model),
        Action::EditTitle => open_edit_title(model),
        Action::DeleteTask => open_delete_confirm(model),
        // View-only: cycle the local lens. `tasks` (Manual order) is untouched
        // and `selected_task` keeps indexing it, so the cursor stays on the
        // same Task by id across the re-sort. Never emits a Command.
        Action::CycleSort => model.sort = model.sort.next(),
    }
    Vec::new()
}

/// The selected Task, if the task pane is focused with a selection.
fn focused_task(model: &Model) -> Option<&Task> {
    if model.focus != Focus::Tasks {
        return None;
    }
    model.selected_task.and_then(|i| model.tasks.get(i))
}

fn open_edit_title(model: &mut Model) {
    if let Some(task) = focused_task(model) {
        model.overlay = Some(Overlay::EditTitle {
            task: task.id.clone(),
            buffer: task.title.clone(),
        });
    }
}

/// Open the capture overlay for a new Task (needs an active List to add into).
fn open_add_task(model: &mut Model) {
    if model.selected_list_id().is_some() {
        model.overlay = Some(Overlay::AddTask {
            buffer: String::new(),
        });
    }
}

/// Optimistically insert a placeholder Task at the top (Google adds new Tasks to
/// the top of a List) and request the insert. The placeholder carries a `temp-N`
/// id; the server Task replaces it via `TaskInserted`. Exact position reconciles
/// on the next refresh.
fn finish_add_task(model: &mut Model, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let temp = TaskId(format!("temp-{}", model.next_temp));
    model.next_temp += 1;
    model.tasks.insert(
        0,
        Task {
            id: temp.clone(),
            list: list.clone(),
            parent: None,
            title: title.clone(),
            notes: None,
            status: Status::NeedsAction,
            due: None,
            completed_at: None,
            position: String::new(),
            etag: String::new(),
            updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        },
    );
    model.selected_task = Some(0); // cursor moves to the new Task
    vec![Command::AddTask { list, temp, title }]
}

fn open_delete_confirm(model: &mut Model) {
    let Some(task) = focused_task(model) else {
        return;
    };
    let (id, title) = (task.id.clone(), task.title.clone());
    let Some(list) = model.selected_list_id().cloned() else {
        return;
    };
    model.overlay = Some(Overlay::Confirm(Confirm {
        prompt: format!("Delete \"{title}\"? (y/n)"),
        action: ConfirmAction::DeleteTask { list, task: id },
    }));
}

/// Route a key to the active overlay: text editing for input overlays, yes/no
/// for `Confirm`.
fn overlay_key(model: &mut Model, key: crossterm::event::KeyEvent) -> Vec<Command> {
    use crossterm::event::KeyCode;
    let input = model.overlay.as_mut().and_then(Overlay::input_buffer);
    if let Some(buffer) = input {
        match key.code {
            KeyCode::Char(c) => buffer.push(c),
            KeyCode::Backspace => {
                buffer.pop();
            }
            KeyCode::Enter => return submit_input(model),
            KeyCode::Esc => model.overlay = None,
            _ => {}
        }
        Vec::new()
    } else {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => execute_confirm(model),
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                model.overlay = None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }
}

/// Submit whichever text-input overlay is active.
fn submit_input(model: &mut Model) -> Vec<Command> {
    match model.overlay.take() {
        Some(Overlay::EditTitle { task, buffer }) => finish_edit_title(model, task, buffer),
        Some(Overlay::AddTask { buffer }) => finish_add_task(model, buffer),
        other => {
            model.overlay = other;
            Vec::new()
        }
    }
}

fn finish_edit_title(model: &mut Model, task: TaskId, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new(); // cancel silently on an empty title
    }
    // Single-flight: don't lose the edit silently if a write is already running.
    if model.pending_writes.contains_key(&task) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(index) = model.tasks.iter().position(|t| t.id == task) else {
        return Vec::new();
    };
    model
        .pending_writes
        .insert(task.clone(), model.tasks[index].clone());
    model.tasks[index].title = title.clone();
    vec![Command::SetTitle { list, task, title }]
}

fn execute_confirm(model: &mut Model) -> Vec<Command> {
    let Some(Overlay::Confirm(confirm)) = model.overlay.take() else {
        return Vec::new();
    };
    match confirm.action {
        ConfirmAction::DeleteTask { list, task } => {
            let Some(index) = model.tasks.iter().position(|t| t.id == task) else {
                return Vec::new();
            };
            let removed = model.tasks.remove(index); // optimistic delete
            model.pending_deletes.insert(task.clone(), (index, removed));
            clamp_task_selection(model);
            vec![Command::DeleteTask { list, task }]
        }
    }
}

/// Keep `selected_task` in range after the Task set shrinks.
fn clamp_task_selection(model: &mut Model) {
    model.selected_task = if model.tasks.is_empty() {
        None
    } else {
        Some(model.selected_task.unwrap_or(0).min(model.tasks.len() - 1))
    };
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
