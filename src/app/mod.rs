//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer, and `update`
//! emits `Command`s for them to run.

use std::collections::HashMap;

use chrono::NaiveDate;

use crate::dateparse;
use crate::domain::{List, ListId, SortView, Status, Task, TaskId};
use crate::keymap::{self, Action};

/// Shown when an operation needs Google but no API client was configured: a
/// write (ADR-0001: no offline editing in v1), or a Refresh, which has nothing
/// to pull without one.
pub const OFFLINE: &str = "not connected to Google";

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
    /// Local, read-only regrouping of the task pane. Never mutates `tasks`
    /// (Manual order) and never writes `position` to Google. Starts at
    /// [`SortView::Due`]; a Move press switches it back to `Manual` first.
    pub sort: SortView,
    /// Whether Completed Tasks are revealed in the pane. Off by default (they are
    /// hidden, the way the Google app does); a toggle reveals them. Purely a local
    /// view filter — it never re-fetches and never mutates `tasks`.
    pub show_completed: bool,
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
    /// Lists with a rename in flight, mapped to their pre-write snapshot for
    /// rollback. Single-flight guard, keyed by `ListId` (the Task analogue is
    /// `pending_writes`).
    pending_list_writes: HashMap<ListId, List>,
    /// Optimistically-removed Lists awaiting delete confirmation, mapped to their
    /// prior (index, List) for rollback on failure (e.g. Google's undeletable
    /// default List).
    pending_list_deletes: HashMap<ListId, (usize, List)>,
    /// Lists with a Clear in flight, mapped to the optimistically-removed
    /// Completed Tasks and their prior indices (ascending) for rollback. A
    /// per-item snapshot — like `pending_deletes` — so a failed Clear re-inserts
    /// only the swept Tasks, never clobbering a concurrent edit or a reload.
    pending_clears: HashMap<ListId, Vec<(usize, Task)>>,
    /// A Move (indent/outdent/reorder) in flight, with the List and the pre-Move
    /// `tasks` snapshot for rollback. A Move renumbers many positions, so it is
    /// single-flight (one at a time) and reconciled by a whole-pane refetch on
    /// success. Because that reconcile replaces `tasks` wholesale, a Move is also
    /// blocked while the moved Task has a field write in flight (see
    /// `move_preconditions`); a field edit to *another* Task during the window is
    /// transiently overwritten but re-converges via its own by-id reconciliation.
    pending_move: Option<(ListId, Vec<Task>)>,
    /// Counter for minting placeholder ids for optimistically-added Tasks, before
    /// the server assigns the real id.
    next_temp: u64,
    /// The current local time, stamped by the runtime before each event so the
    /// reducer can resolve relative due dates ("tomorrow") without reading the
    /// clock itself — keeping `update` pure/testable (ADR-0005).
    pub now: chrono::DateTime<chrono::Local>,
    /// Whether an external editor (`$VISUAL`/`$EDITOR`) is available, stamped by
    /// the runtime like `now`. Decides the notes-editing path purely: with an
    /// editor, `EditNotes` emits `SpawnEditor`; without, it opens the inline
    /// single-line fallback overlay.
    pub editor_available: bool,
    /// Whether an API client was configured at startup, stamped by the runtime
    /// like `editor_available`. Decides the Refresh path purely: with a client,
    /// `Refresh` emits the Command; without, it reports [`OFFLINE`] and emits
    /// nothing. Note this says credentials were constructed, *not* that Google
    /// is reachable — with the network down it still reads `true` and the
    /// Refresh fails at the worker, landing on the status line.
    pub api_available: bool,
}

/// A modal overlay drawn over the panes.
#[derive(Debug, Clone)]
pub enum Overlay {
    /// In-place title editor for a Task.
    EditTitle { task: TaskId, buffer: String },
    /// Capture a new Task's title.
    AddTask { buffer: String },
    /// Capture a new Subtask's title, to be inserted under `parent`.
    AddSubtask { parent: TaskId, buffer: String },
    /// Due-date entry for a Task: natural language or ISO, or empty to clear.
    EditDue { task: TaskId, buffer: String },
    /// Inline single-line notes editor — the fallback used when no external
    /// editor is configured. Empty clears the notes.
    EditNotes { task: TaskId, buffer: String },
    /// Capture a new List's title.
    AddList { buffer: String },
    /// In-place title editor for a List.
    RenameList { list: ListId, buffer: String },
    /// A reusable destructive-action confirmation (delete Task or List).
    Confirm(Confirm),
}

impl Overlay {
    /// The editable text buffer of a text-input overlay, if this is one.
    fn input_buffer(&mut self) -> Option<&mut String> {
        match self {
            Overlay::EditTitle { buffer, .. }
            | Overlay::AddTask { buffer }
            | Overlay::AddSubtask { buffer, .. }
            | Overlay::EditDue { buffer, .. }
            | Overlay::EditNotes { buffer, .. }
            | Overlay::AddList { buffer }
            | Overlay::RenameList { buffer, .. } => Some(buffer),
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
    DeleteList { list: ListId },
    ClearCompleted { list: ListId },
}

impl Default for Model {
    fn default() -> Self {
        Self {
            lists: Vec::new(),
            selected_list: None,
            tasks: Vec::new(),
            selected_task: None,
            sort: SortView::Due,
            show_completed: false,
            focus: Focus::Sidebar,
            show_help: false,
            should_quit: false,
            status_line: None,
            overlay: None,
            pending_writes: HashMap::new(),
            pending_deletes: HashMap::new(),
            pending_list_writes: HashMap::new(),
            pending_list_deletes: HashMap::new(),
            pending_clears: HashMap::new(),
            pending_move: None,
            next_temp: 0,
            // A fixed placeholder, deliberately not the real clock: `Default`
            // stays pure so tests construct a deterministic Model and set `now`
            // themselves. The runtime overwrites it before each draw and each
            // event, so the epoch is never what the view or reducer sees.
            now: chrono::DateTime::from_timestamp(0, 0)
                .expect("epoch is valid")
                .with_timezone(&chrono::Local),
            // Defaults to the inline fallback; the runtime stamps the real value.
            editor_available: false,
            // Defaults to offline; the runtime stamps the real value.
            api_available: false,
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

    /// The Tasks in the current `sort`'s **display order**.
    ///
    /// Every lens keeps the parent/Subtask hierarchy: a Task group is a top-level
    /// Task plus its Subtasks, and only the ordering *of* and *within* groups
    /// changes. An indented row therefore always sits under its real parent.
    ///
    /// - `Manual`: groups and children in Vec order. Derived from `parent` + Vec
    ///   order, so it is correct whether `position` strings are global (the fake)
    ///   or per-sibling-group (Google).
    /// - `Due`: children by due date ascending (undated last); a group sorts by
    ///   the earliest due among its **incomplete** Tasks, so an urgent Subtask
    ///   lifts its parent and a Completed one never does — group order is the
    ///   same whether or not `show_completed` reveals them.
    /// - `Title`: children and groups case-insensitive by title.
    ///
    /// Ordering is **stable over stored order** everywhere: groups are built in
    /// Vec order and sorted stably, so equal keys — including the key-less
    /// undated tail — keep Manual order. An orphaned Subtask (parent absent) is
    /// its own single-Task group, so it sorts on its own key rather than sinking
    /// to the bottom; `Manual` keeps appending it last.
    ///
    /// A pure, read-only lens: it borrows `tasks` and never mutates Manual order
    /// (`position`) nor emits any Command — the view renders this, the model is
    /// untouched. Navigation (`j`/`k`) follows this order, so the cursor never
    /// jumps between non-adjacent rows.
    pub fn sorted_tasks(&self) -> Vec<&Task> {
        let mut groups = self.groups();
        match self.sort {
            // Vec order already; `groups` built it.
            SortView::Manual => {}
            // Two rules hold for both lenses below. `sort_by_cached_key` is
            // stable, so equal keys — including the key-less tail — keep the
            // stored order they were collected in; it also computes each key
            // once rather than per comparison, which matters for the group key
            // (a scan) and the lowercased title (an allocation). And only
            // `group[1..]` is sorted: the parent stays the head of its group, or
            // a Subtask could render above the row it belongs to.
            SortView::Due => {
                for group in &mut groups {
                    group[1..].sort_by_cached_key(|t| due_key(t.due));
                }
                groups.sort_by_cached_key(|g| due_key(group_due_key(g)));
            }
            SortView::Title => {
                for group in &mut groups {
                    group[1..].sort_by_cached_key(|t| t.title.to_lowercase());
                }
                groups.sort_by_cached_key(|g| g[0].title.to_lowercase());
            }
        }
        groups.into_iter().flatten().collect()
    }

    /// Split `tasks` into groups in Vec order: each top-level Task followed by its
    /// Subtasks, then each orphaned Subtask (parent absent, e.g. after the parent
    /// was deleted) as a group of its own. Every Task lands in exactly one group,
    /// so nothing ever vanishes and flattening always reproduces the whole set.
    fn groups(&self) -> Vec<Vec<&Task>> {
        let mut out: Vec<Vec<&Task>> = Vec::new();
        for parent in self.tasks.iter().filter(|t| t.parent.is_none()) {
            let mut group = vec![parent];
            group.extend(
                self.tasks
                    .iter()
                    .filter(|c| c.parent.as_ref() == Some(&parent.id)),
            );
            out.push(group);
        }
        // Orphans are appended after every parent group. Under `Manual` that
        // puts them last (what `hierarchical` did); under the sorted lenses they
        // are ordinary single-Task groups, so they sort on their own key and
        // only fall back to this trailing position when that key ties.
        for task in &self.tasks {
            if !out.iter().flatten().any(|t| t.id == task.id) {
                out.push(vec![task]);
            }
        }
        out
    }

    /// Whether `task` should render indented: it has a parent **and** that parent
    /// is in the Task set. An orphaned Subtask renders flush-left rather than as a
    /// child of whatever precedes it. Independent of `show_completed` — a hidden
    /// parent is still a parent — so the indent never flickers with the toggle.
    pub fn renders_as_subtask(&self, task: &Task) -> bool {
        task.parent
            .as_ref()
            .is_some_and(|p| self.tasks.iter().any(|t| &t.id == p))
    }

    /// The Tasks actually shown in the pane: [`sorted_tasks`](Self::sorted_tasks)
    /// with Completed Tasks filtered out unless `show_completed` reveals them.
    /// The view renders this; the completion meter still counts over all `tasks`.
    pub fn visible_tasks(&self) -> Vec<&Task> {
        self.sorted_tasks()
            .into_iter()
            .filter(|t| self.is_visible(t))
            .collect()
    }

    /// Whether a Task is currently shown: always, unless it is Completed and
    /// completed Tasks are hidden.
    fn is_visible(&self, task: &Task) -> bool {
        self.show_completed || task.status != Status::Completed
    }
}

/// The id of the Task under the cursor, if any.
fn selected_id(model: &Model) -> Option<TaskId> {
    model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .map(|t| t.id.clone())
}

/// The Task that should take the cursor when `task` leaves the view: the next
/// **visible** Task after it in display order, else the nearest before it, else
/// `None`. Must be called *before* `task` is removed — afterwards it has no
/// display position to anchor from.
fn display_successor(model: &Model, task: &TaskId) -> Option<TaskId> {
    display_neighbour(model, task, |t| model.is_visible(t))
}

/// The nearest Task to `task` in display order that satisfies `keep` — forwards
/// first, then backwards. Callers pass a predicate describing which rows will
/// still be there once the mutation lands, so a cursor whose own row is going
/// moves one step rather than to the top of the pane.
fn display_neighbour(model: &Model, task: &TaskId, keep: impl Fn(&Task) -> bool) -> Option<TaskId> {
    let ordered = model.sorted_tasks();
    let pos = ordered.iter().position(|t| &t.id == task)?;
    ordered[pos + 1..]
        .iter()
        .find(|t| keep(t))
        .or_else(|| ordered[..pos].iter().rev().find(|t| keep(t)))
        .map(|t| t.id.clone())
}

/// Stored index of the first Task in display order, or `None` when there are no
/// Tasks. The cursor's home when there is no selection to preserve.
fn first_displayed_index(model: &Model) -> Option<usize> {
    let first = model.sorted_tasks().first().map(|t| t.id.clone())?;
    model.tasks.iter().position(|t| t.id == first)
}

/// Sort key for a due date under `Due`: dated ascending, undated last. The
/// leading flag does the sinking — `Option`'s own ordering would put `None`
/// first — and every undated value compares equal, so a stable sort leaves the
/// tail in stored order.
fn due_key(due: Option<NaiveDate>) -> (bool, Option<NaiveDate>) {
    (due.is_none(), due)
}

/// The `Due` key for a group: the earliest due date among its **incomplete**
/// Tasks, so an urgent Subtask lifts its parent out of the tail. Completed Tasks
/// never contribute — otherwise a row hidden by `show_completed` could set the
/// group's position, and toggling the filter would reorder the pane.
fn group_due_key(group: &[&Task]) -> Option<NaiveDate> {
    group
        .iter()
        .filter(|t| t.status != Status::Completed)
        .filter_map(|t| t.due)
        .min()
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
    /// A List add succeeded; replace the placeholder (by temp id) with the server List.
    ListInserted {
        temp: ListId,
        list: List,
    },
    /// A List add failed; drop the placeholder (by temp id).
    ListAddFailed {
        temp: ListId,
        reason: String,
    },
    /// A List rename succeeded; reconcile the Model with the server's List.
    ListUpdated(List),
    /// A List rename failed; roll back to the pre-write snapshot.
    ListWriteFailed {
        list: ListId,
        reason: String,
    },
    /// A List delete succeeded; drop the optimistic-delete bookkeeping.
    ListDeleted(ListId),
    /// A List delete failed; re-insert the List at its prior position.
    ListDeleteFailed {
        list: ListId,
        reason: String,
    },
    /// A Clear succeeded; drop the optimistic-Clear snapshot for the List.
    ClearedCompleted(ListId),
    /// A Clear failed; restore the List's pre-Clear Tasks.
    ClearCompletedFailed {
        list: ListId,
        reason: String,
    },
    /// A Move (indent/outdent/reorder) succeeded; drop the snapshot and reconcile
    /// the pane to the authoritative post-Move order.
    MoveSucceeded {
        list: ListId,
        tasks: Vec<Task>,
    },
    /// A Move failed; restore the pre-Move Tasks.
    MoveFailed {
        list: ListId,
        reason: String,
    },
    /// The external notes editor returned changed text; write it through. `notes`
    /// is `None` when the user emptied the buffer (clears the notes). The runtime
    /// emits this only on an actual change — an unchanged buffer emits nothing.
    NotesEdited {
        task: TaskId,
        notes: Option<String>,
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
    /// Write-through a due-date change for a Task. `None` clears the due date.
    SetDue {
        list: ListId,
        task: TaskId,
        due: Option<NaiveDate>,
    },
    /// Write-through a notes change for a Task. `None` clears the notes.
    SetNotes {
        list: ListId,
        task: TaskId,
        notes: Option<String>,
    },
    /// Suspend the TUI and open the Task's notes in the external editor. Handled
    /// synchronously by the runtime (it owns the terminal), which feeds the
    /// result back as `Message::NotesEdited`. Only emitted when an editor exists.
    SpawnEditor { task: TaskId, notes: Option<String> },
    /// Delete a Task.
    DeleteTask { list: ListId, task: TaskId },
    /// Insert a new Task into a List; `temp` echoes back so the placeholder can
    /// be reconciled with the server Task. `parent` is `Some` for a Subtask.
    AddTask {
        list: ListId,
        temp: TaskId,
        title: String,
        parent: Option<TaskId>,
    },
    /// Reposition / reparent a Task (indent, outdent, or reorder). `parent` sets
    /// the new parent (`None` = top-level); `previous` is the sibling to place it
    /// after (`None` = first among its new siblings).
    Move {
        list: ListId,
        task: TaskId,
        parent: Option<TaskId>,
        previous: Option<TaskId>,
    },
    /// Insert a new List; `temp` echoes back so the placeholder can be
    /// reconciled with the server List.
    AddList { temp: ListId, title: String },
    /// Write-through a title rename for a List.
    RenameList { list: ListId, title: String },
    /// Delete a List.
    DeleteList { list: ListId },
    /// Sweep a List's Completed Tasks to hidden (`clear_completed`).
    ClearCompleted { list: ListId },
    /// Re-pull the List set from Google. Named for what it does: the active
    /// List's Tasks are refreshed too, but as a *consequence* — the resulting
    /// `ListsLoaded` flows through `set_lists`, which re-requests them. So one
    /// Command refreshes both halves, at the cost of coupling their failures
    /// (a failed `list_lists` emits `LoadFailed` and the cascade never starts).
    RefreshLists,
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
                    // The insert shifts every later index, so re-resolve the
                    // cursor by id. It does *not* jump to the restored Task: the
                    // rollback arrives asynchronously, and by then the user may
                    // have moved on to an unrelated row.
                    let selected = selected_id(model);
                    model.tasks.insert(at, previous);
                    model.selected_task =
                        selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                    reselect_visible(model);
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
                // The insert shifts every later index, so hold the cursor by id.
                let selected = selected_id(model);
                model.tasks.insert(0, task);
                model.selected_task =
                    selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
            }
            Vec::new()
        }
        Message::TaskAddFailed { temp, reason } => {
            // Only a cursor sitting on the placeholder needs a new home; one that
            // has moved elsewhere is left where the user put it. Either way the
            // `retain` shifts indices, so the anchor is resolved by id afterwards.
            let anchor = if selected_id(model).as_ref() == Some(&temp) {
                // Taken before the placeholder goes: afterwards it has no display
                // position to anchor from.
                display_successor(model, &temp)
            } else {
                selected_id(model)
            };
            model.tasks.retain(|t| t.id != temp);
            model.selected_task = anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
            reselect_visible(model);
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::ListInserted { temp, list } => {
            // Reconcile the optimistic placeholder with the server's real List.
            if let Some(slot) = model.lists.iter_mut().find(|l| l.id == temp) {
                *slot = list.clone();
                // If the placeholder was the active List, load the server List's
                // Tasks now that it has a real id (a fresh List is empty, but this
                // keeps the pane and cache consistent).
                if model.selected_list_id() == Some(&list.id) {
                    return request_selected_tasks(model, true);
                }
            } else if !model.lists.iter().any(|l| l.id == list.id) {
                // A refresh wiped the placeholder before the reply; don't lose
                // the confirmed List (and don't duplicate one a refresh added).
                model.lists.push(list);
            }
            Vec::new()
        }
        Message::ListAddFailed { temp, reason } => {
            let was_selected = model.selected_list_id() == Some(&temp);
            model.lists.retain(|l| l.id != temp);
            clamp_list_selection(model);
            model.status_line = Some(reason);
            if was_selected {
                return request_selected_tasks(model, true);
            }
            Vec::new()
        }
        Message::ListUpdated(list) => {
            model.pending_list_writes.remove(&list.id);
            if let Some(slot) = model.lists.iter_mut().find(|l| l.id == list.id) {
                *slot = list;
            }
            Vec::new()
        }
        Message::ListWriteFailed { list, reason } => {
            if let Some(previous) = model.pending_list_writes.remove(&list) {
                if let Some(slot) = model.lists.iter_mut().find(|l| l.id == previous.id) {
                    *slot = previous;
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::ListDeleted(list) => {
            model.pending_list_deletes.remove(&list);
            Vec::new()
        }
        Message::ListDeleteFailed { list, reason } => {
            if let Some((index, previous)) = model.pending_list_deletes.remove(&list) {
                // Guard against a refresh having already re-added it: only
                // re-insert when it's genuinely absent, so we can't duplicate.
                if !model.lists.iter().any(|l| l.id == previous.id) {
                    let at = index.min(model.lists.len());
                    model.lists.insert(at, previous);
                    // Restore the selection to the recovered List and reload its
                    // Tasks — the pane currently shows the List we fell back to.
                    model.selected_list = Some(at);
                    model.status_line = Some(reason);
                    return request_selected_tasks(model, true);
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::ClearedCompleted(list) => {
            // Confirmed swept; drop the rollback snapshot (Tasks already removed).
            model.pending_clears.remove(&list);
            Vec::new()
        }
        Message::ClearCompletedFailed { list, reason } => {
            if let Some(removed) = model.pending_clears.remove(&list) {
                // Re-insert the swept Tasks only if that List is still active — a
                // List switch during the Clear left a different pane in place.
                // Ascending by prior index restores original order; skip any a
                // refresh already re-added so we can't duplicate (cf. delete).
                if model.selected_list_id() == Some(&list) {
                    let selected = selected_id(model);
                    for (index, task) in removed {
                        if !model.tasks.iter().any(|t| t.id == task.id) {
                            let at = index.min(model.tasks.len());
                            model.tasks.insert(at, task);
                        }
                    }
                    model.selected_task =
                        selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                    reselect_visible(model);
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::MoveSucceeded { list, tasks } => {
            // Drop the snapshot and reconcile to the authoritative post-Move order.
            model.pending_move = None;
            set_tasks(model, &list, tasks);
            Vec::new()
        }
        Message::MoveFailed { list, reason } => {
            if let Some((snap_list, snapshot)) = model.pending_move.take() {
                // Restore only if that List is still active — a switch during the
                // Move left a different pane in place.
                if snap_list == list && model.selected_list_id() == Some(&list) {
                    // The optimistic reorder parked the cursor on the moved
                    // Task's new index; restoring the prior order puts a
                    // different Task there, so re-resolve by id.
                    let selected = selected_id(model);
                    model.tasks = snapshot;
                    model.selected_task =
                        selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                    reselect_visible(model);
                }
            }
            model.status_line = Some(reason);
            Vec::new()
        }
        Message::NotesEdited { task, notes } => finish_edit_notes(model, task, notes),
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
    // Keep the cursor on the same Task; if it is gone, fall back to the first
    // *displayed* row rather than stored index 0, which is an arbitrary row in
    // every lens but Manual.
    model.selected_task = previously_selected
        .and_then(|id| model.tasks.iter().position(|t| t.id == id))
        .or_else(|| first_displayed_index(model));
    // Don't leave the cursor on a Task hidden by the completed filter.
    reselect_visible(model);
}

fn apply(model: &mut Model, action: Action) -> Vec<Command> {
    match action {
        Action::Quit => model.should_quit = true,
        Action::ToggleHelp => model.show_help = !model.show_help,
        Action::CloseOverlay => model.show_help = false,
        Action::SwitchPane => model.focus = model.focus.toggled(),
        // Directional, unlike `SwitchPane`: they name a pane rather than flip to
        // the other one, so they are idempotent at the layout's edges. The panes
        // are laid out sidebar-left, tasks-right (see `ui::view`).
        Action::FocusLeft => model.focus = Focus::Sidebar,
        Action::FocusRight => model.focus = Focus::Tasks,
        Action::SelectNext => return move_selection(model, 1),
        Action::SelectPrev => return move_selection(model, -1),
        Action::ToggleComplete => return toggle_complete(model),
        Action::AddTask => open_add_task(model),
        Action::EditTitle => open_edit_title(model),
        Action::EditDue => open_edit_due(model),
        Action::EditNotes => return edit_notes(model),
        Action::DeleteTask => open_delete_confirm(model),
        Action::AddList => open_add_list(model),
        Action::RenameList => open_rename_list(model),
        Action::DeleteList => open_delete_list_confirm(model),
        // View-only: cycle the local lens. `tasks` (Manual order) is untouched
        // and `selected_task` keeps indexing it, so the cursor stays on the
        // same Task by id across the re-sort. Never emits a Command.
        Action::CycleSort => model.sort = model.sort.next(),
        // View-only: reveal/hide Completed Tasks. Hiding may drop the selected
        // Task out of view, so re-anchor the cursor onto a visible one.
        Action::ToggleShowCompleted => {
            model.show_completed = !model.show_completed;
            reselect_visible(model);
        }
        Action::ClearCompleted => open_clear_completed_confirm(model),
        Action::Refresh => return refresh(model),
        Action::AddSubtask => open_add_subtask(model),
        Action::Indent => return indent(model),
        Action::Outdent => return outdent(model),
        Action::MoveDown => return reorder(model, 1),
        Action::MoveUp => return reorder(model, -1),
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

/// Optimistically insert a placeholder Task at the top of **stored** order
/// (Google adds new Tasks to the top of a List) and request the insert. Where it
/// *renders* is up to the active lens: the placeholder carries no due date, so
/// under `Due` it appears in the undated tail rather than the first row. The
/// cursor follows it either way. The placeholder carries a `temp-N` id; the
/// server Task replaces it via `TaskInserted`. Exact position reconciles on the
/// next refresh.
fn finish_add_task(model: &mut Model, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    add_task_placeholder(model, list, title, None, 0)
}

/// Open the capture overlay for a new Subtask under the selected Task. A Subtask
/// must hang off a top-level Task (one-level cap), so if a Subtask is selected the
/// new one is added under *its* parent (a sibling), not under the Subtask.
fn open_add_subtask(model: &mut Model) {
    let Some(task) = focused_task(model) else {
        return;
    };
    let parent = match &task.parent {
        Some(p) => p.clone(),
        None => task.id.clone(),
    };
    model.overlay = Some(Overlay::AddSubtask {
        parent,
        buffer: String::new(),
    });
}

/// Optimistically insert a placeholder Subtask under `parent` (as its first
/// child, matching Google's top-of-list insert) and request the insert.
fn finish_add_subtask(model: &mut Model, parent: TaskId, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(pidx) = model.tasks.iter().position(|t| t.id == parent) else {
        return Vec::new(); // parent vanished (e.g. a refresh dropped it)
    };
    if model.tasks[pidx].parent.is_some() {
        model.status_line = Some("subtasks can't have subtasks (one level max)".to_string());
        return Vec::new();
    }
    add_task_placeholder(model, list, title, Some(parent), pidx + 1)
}

/// Insert an optimistic placeholder Task at `index`, move the cursor onto it, and
/// emit its `AddTask`. Shared by the top-level and Subtask add paths.
fn add_task_placeholder(
    model: &mut Model,
    list: ListId,
    title: String,
    parent: Option<TaskId>,
    index: usize,
) -> Vec<Command> {
    let temp = TaskId(format!("temp-{}", model.next_temp));
    model.next_temp += 1;
    model.tasks.insert(
        index,
        Task {
            id: temp.clone(),
            list: list.clone(),
            parent: parent.clone(),
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
    model.selected_task = Some(index);
    vec![Command::AddTask {
        list,
        temp,
        title,
        parent,
    }]
}

/// Preconditions shared by every Move, checked in this order: the task pane
/// focused, an active List, a selection, no Move already in flight, no field
/// write in flight, and finally Manual sort.
///
/// The order is load-bearing. The sort check comes **last** because it is the
/// only one that mutates state: from a Sort view it switches the pane to
/// `Manual`, reports the switch, and returns `None` without moving anything —
/// the Move lands on the *next* press, against the adjacency the user can now
/// see. Every verb computes over stored order (`indent` picks the previous
/// top-level Task by Vec index, `reorder` swaps sibling slots), so applying a
/// Move while a Sort view is displayed would reorder against rows that were
/// off-screen. Putting the switch after the other guards keeps a Move refused
/// for any other reason from disturbing the lens.
///
/// Three outcomes:
/// - `Some((list, selected index))` — go ahead;
/// - `None` with a status line — refused (Move/write in flight), or the lens was
///   just switched to `Manual` and the user should press again;
/// - `None` silently — nothing to act on (unfocused, no List, no selection).
fn move_preconditions(model: &mut Model) -> Option<(ListId, usize)> {
    if model.focus != Focus::Tasks {
        return None;
    }
    let list = model.selected_list_id().cloned()?;
    let idx = model.selected_task?;
    if model.pending_move.is_some() {
        model.status_line = Some("a move is already in progress".to_string());
        return None;
    }
    // Don't move a Task whose field write is still in flight: the Move's whole-pane
    // reconcile would clobber that optimistic edit before it lands.
    if model.pending_writes.contains_key(&model.tasks[idx].id) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return None;
    }
    if model.sort != SortView::Manual {
        model.sort = SortView::Manual;
        model.status_line = Some("switched to \"my order\" — press again to move".to_string());
        return None;
    }
    Some((list, idx))
}

/// Indent the selected top-level Task into a Subtask of the top-level Task above
/// it (a Move). Rejected if it is already a Subtask (one-level cap) or has its own
/// Subtasks, or if there is no previous top-level Task to nest under.
fn indent(model: &mut Model) -> Vec<Command> {
    let Some((list, idx)) = move_preconditions(model) else {
        return Vec::new();
    };
    if model.tasks[idx].parent.is_some() {
        model.status_line = Some("already a subtask (one level max)".to_string());
        return Vec::new();
    }
    let task_id = model.tasks[idx].id.clone();
    if model
        .tasks
        .iter()
        .any(|t| t.parent.as_ref() == Some(&task_id))
    {
        model.status_line = Some("can't indent a task that has subtasks".to_string());
        return Vec::new();
    }
    let Some(parent_pos) = (0..idx).rev().find(|&j| model.tasks[j].parent.is_none()) else {
        model.status_line = Some("no previous task to indent under".to_string());
        return Vec::new();
    };
    let parent_id = model.tasks[parent_pos].id.clone();
    // Land after the parent's current last child (else as its first child).
    let previous = model
        .tasks
        .iter()
        .rev()
        .find(|t| t.parent.as_ref() == Some(&parent_id))
        .map(|t| t.id.clone());
    let snapshot = model.tasks.clone();
    model.tasks[idx].parent = Some(parent_id.clone());
    model.pending_move = Some((list.clone(), snapshot));
    vec![Command::Move {
        list,
        task: task_id,
        parent: Some(parent_id),
        previous,
    }]
}

/// Outdent the selected Subtask back to top-level (a Move), placed right after
/// its former parent. A no-op on a Task that is already top-level.
fn outdent(model: &mut Model) -> Vec<Command> {
    let Some((list, idx)) = move_preconditions(model) else {
        return Vec::new();
    };
    let Some(parent_id) = model.tasks[idx].parent.clone() else {
        model.status_line = Some("not a subtask".to_string());
        return Vec::new();
    };
    let task_id = model.tasks[idx].id.clone();
    let snapshot = model.tasks.clone();
    model.tasks[idx].parent = None;
    model.pending_move = Some((list.clone(), snapshot));
    vec![Command::Move {
        list,
        task: task_id,
        parent: None,
        previous: Some(parent_id),
    }]
}

/// Reorder the selected Task among its siblings (same parent) by `dir` (+1 down,
/// -1 up) — a Move. Swapping the Task's Vec slot with the adjacent sibling's
/// reorders their whole subtrees, since the display regroups children by parent.
fn reorder(model: &mut Model, dir: isize) -> Vec<Command> {
    // `dir` is a single step; the `previous`-sibling arithmetic below assumes it.
    debug_assert!(dir == 1 || dir == -1, "reorder steps one sibling at a time");
    let Some((list, idx)) = move_preconditions(model) else {
        return Vec::new();
    };
    let parent = model.tasks[idx].parent.clone();
    let sibs: Vec<usize> = model
        .tasks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.parent == parent)
        .map(|(i, _)| i)
        .collect();
    let pos = sibs
        .iter()
        .position(|&i| i == idx)
        .expect("selection is a sibling");
    let target = pos as isize + dir;
    if target < 0 || target as usize >= sibs.len() {
        model.status_line = Some(
            if dir > 0 {
                "already last"
            } else {
                "already first"
            }
            .to_string(),
        );
        return Vec::new();
    }
    let target = target as usize;
    let task_id = model.tasks[idx].id.clone();
    let sib_ids: Vec<TaskId> = sibs.iter().map(|&i| model.tasks[i].id.clone()).collect();
    // The sibling to land after: for a down move, the next sibling; for an up
    // move, the one before our previous sibling (`None` => first among siblings).
    let previous = if dir > 0 {
        Some(sib_ids[pos + 1].clone())
    } else if pos >= 2 {
        Some(sib_ids[pos - 2].clone())
    } else {
        None
    };
    let snapshot = model.tasks.clone();
    model.tasks.swap(idx, sibs[target]);
    model.selected_task = Some(sibs[target]); // follow the moved Task
    model.pending_move = Some((list.clone(), snapshot));
    vec![Command::Move {
        list,
        task: task_id,
        parent,
        previous,
    }]
}

/// Open the due-date editor, prefilled with the current due (ISO) or empty.
fn open_edit_due(model: &mut Model) {
    if let Some(task) = focused_task(model) {
        let buffer = task
            .due
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        model.overlay = Some(Overlay::EditDue {
            task: task.id.clone(),
            buffer,
        });
    }
}

/// Begin editing the selected Task's notes. With an external editor available
/// (`model.editor_available`), emit `SpawnEditor` for the runtime to suspend the
/// TUI and open it; otherwise open the inline single-line fallback overlay.
fn edit_notes(model: &mut Model) -> Vec<Command> {
    let Some(task) = focused_task(model) else {
        return Vec::new();
    };
    let (id, notes) = (task.id.clone(), task.notes.clone());
    // Don't open the editor over an in-flight write: the result would be rejected
    // by `finish_edit_notes`'s single-flight guard on submit, silently discarding
    // whatever the user typed. Refuse up front with an explanation instead.
    if model.pending_writes.contains_key(&id) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return Vec::new();
    }
    if model.editor_available {
        vec![Command::SpawnEditor { task: id, notes }]
    } else {
        model.overlay = Some(Overlay::EditNotes {
            task: id,
            buffer: notes.unwrap_or_default(),
        });
        Vec::new()
    }
}

/// Write-through a notes edit (from either the external editor or the inline
/// fallback): optimistically set `notes`, snapshot for rollback, and emit the
/// write — mirroring [`finish_edit_due`]. An empty/whitespace buffer clears the
/// notes. A no-op when the notes are unchanged.
fn finish_edit_notes(model: &mut Model, task: TaskId, notes: Option<String>) -> Vec<Command> {
    let notes = notes.filter(|n| !n.trim().is_empty()); // empty => cleared
    let Some(index) = model.tasks.iter().position(|t| t.id == task) else {
        return Vec::new();
    };
    if model.tasks[index].notes == notes {
        return Vec::new(); // nothing changed; don't write
    }
    // Single-flight: don't lose the edit silently if a write is already running.
    if model.pending_writes.contains_key(&task) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    model
        .pending_writes
        .insert(task.clone(), model.tasks[index].clone());
    model.tasks[index].notes = notes.clone();
    vec![Command::SetNotes { list, task, notes }]
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

/// Open the destructive-confirm for Clearing the active List's Completed Tasks.
/// A no-op with no active List or nothing to Clear (don't prompt for zero).
fn open_clear_completed_confirm(model: &mut Model) {
    let Some(list) = model.selected_list_id().cloned() else {
        return;
    };
    let done = model
        .tasks
        .iter()
        .filter(|t| t.status == Status::Completed)
        .count();
    if done == 0 {
        model.status_line = Some("no completed tasks to clear".to_string());
        return;
    }
    let plural = if done == 1 { "task" } else { "tasks" };
    model.overlay = Some(Overlay::Confirm(Confirm {
        prompt: format!("Clear {done} completed {plural}? (y/n)"),
        action: ConfirmAction::ClearCompleted { list },
    }));
}

/// Re-pull from Google on demand. Modeless: it is not gated on a pane, a
/// selection, or any in-flight operation. Emits a single `RefreshLists`, whose
/// `ListsLoaded` cascades through `set_lists` into the active List's Tasks.
///
/// The transient status is cleared by `set_lists` when the *Lists* half lands,
/// so it does not span the cascaded Tasks fetch — spanning both would need
/// in-flight state tracked across two Messages, which is more bookkeeping than
/// a spinner is worth.
fn refresh(model: &mut Model) -> Vec<Command> {
    if !model.api_available {
        model.status_line = Some(OFFLINE.to_string());
        return Vec::new();
    }
    model.status_line = Some("refreshing…".to_string());
    vec![Command::RefreshLists]
}

/// The active List, if the sidebar is focused with a selection. List management
/// is a sidebar action, so these verbs are inert while the task pane is focused
/// (their capital keys can't clash with the task-pane `a`/`e`/`x`).
fn focused_list(model: &Model) -> Option<&List> {
    if model.focus != Focus::Sidebar {
        return None;
    }
    model.selected_list.and_then(|i| model.lists.get(i))
}

/// Open the capture overlay for a new List (sidebar-focused only).
fn open_add_list(model: &mut Model) {
    if model.focus == Focus::Sidebar {
        model.overlay = Some(Overlay::AddList {
            buffer: String::new(),
        });
    }
}

/// Optimistically append a placeholder List (Google appends new Lists) and
/// request the insert. The placeholder carries a `temp-list-N` id; the server
/// List replaces it via `ListInserted`. Selecting it clears the task pane (a
/// fresh List is empty); the server round-trip fills it once reconciled.
fn finish_add_list(model: &mut Model, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new();
    }
    let temp = ListId(format!("temp-list-{}", model.next_temp));
    model.next_temp += 1;
    model.lists.push(List {
        id: temp.clone(),
        title: title.clone(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    });
    // Make the new List active. Don't emit LoadTasks for the placeholder id
    // (Google has no such List yet); the pane is empty until `ListInserted`.
    model.selected_list = Some(model.lists.len() - 1);
    model.tasks.clear();
    model.selected_task = None;
    vec![Command::AddList { temp, title }]
}

/// Open the rename overlay, prefilled with the active List's title.
fn open_rename_list(model: &mut Model) {
    if let Some(list) = focused_list(model) {
        model.overlay = Some(Overlay::RenameList {
            list: list.id.clone(),
            buffer: list.title.clone(),
        });
    }
}

fn finish_rename_list(model: &mut Model, list: ListId, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new(); // cancel silently on an empty title
    }
    // Single-flight: don't lose the edit silently if a write is already running.
    if model.pending_list_writes.contains_key(&list) {
        model.status_line = Some("a write is already in progress for this list".to_string());
        return Vec::new();
    }
    let Some(index) = model.lists.iter().position(|l| l.id == list) else {
        return Vec::new();
    };
    model
        .pending_list_writes
        .insert(list.clone(), model.lists[index].clone());
    model.lists[index].title = title.clone();
    vec![Command::RenameList { list, title }]
}

fn open_delete_list_confirm(model: &mut Model) {
    let Some(list) = focused_list(model) else {
        return;
    };
    let (id, title) = (list.id.clone(), list.title.clone());
    model.overlay = Some(Overlay::Confirm(Confirm {
        prompt: format!("Delete list \"{title}\"? (y/n)"),
        action: ConfirmAction::DeleteList { list: id },
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
        Some(Overlay::AddSubtask { parent, buffer }) => finish_add_subtask(model, parent, buffer),
        Some(Overlay::EditDue { task, buffer }) => finish_edit_due(model, task, buffer),
        Some(Overlay::EditNotes { task, buffer }) => finish_edit_notes(model, task, Some(buffer)),
        Some(Overlay::AddList { buffer }) => finish_add_list(model, buffer),
        Some(Overlay::RenameList { list, buffer }) => finish_rename_list(model, list, buffer),
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

/// Submit the due editor: empty clears the due date, otherwise parse it. On a
/// parse error, keep the overlay open with a status-line hint (don't drop the
/// edit). On success, optimistically set `due`, snapshot for rollback, and emit
/// the write-through — mirroring [`submit_edit_title`].
fn finish_edit_due(model: &mut Model, task: TaskId, buffer: String) -> Vec<Command> {
    let trimmed = buffer.trim();
    let due = if trimmed.is_empty() {
        None // clear the due date
    } else {
        match dateparse::parse_due_relative_to(trimmed, model.now) {
            Ok(date) => Some(date),
            Err(_) => {
                model.status_line = Some(format!("could not parse due date: {trimmed:?}"));
                // Keep the overlay open so the user can fix the input.
                model.overlay = Some(Overlay::EditDue { task, buffer });
                return Vec::new();
            }
        }
    };
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
    model.tasks[index].due = due;
    vec![Command::SetDue { list, task, due }]
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
            // Unconditional, unlike the async failure paths: the delete verb acts
            // on the selection and the confirm overlay swallows keys, so the
            // cursor is provably on the row about to disappear.
            let successor = display_successor(model, &task);
            let removed = model.tasks.remove(index); // optimistic delete
            model.pending_deletes.insert(task.clone(), (index, removed));
            model.selected_task =
                successor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
            reselect_visible(model);
            vec![Command::DeleteTask { list, task }]
        }
        ConfirmAction::DeleteList { list } => {
            let Some(index) = model.lists.iter().position(|l| l.id == list) else {
                return Vec::new();
            };
            let removed = model.lists.remove(index); // optimistic delete
            model
                .pending_list_deletes
                .insert(list.clone(), (index, removed));
            // Re-point selection to the List now occupying (or nearest to) the
            // freed slot and reload its Tasks. On failure (e.g. the undeletable
            // default List), `ListDeleteFailed` restores everything.
            let mut cmds = vec![Command::DeleteList { list }];
            clamp_list_selection(model);
            cmds.extend(request_selected_tasks(model, true));
            cmds
        }
        ConfirmAction::ClearCompleted { list } => {
            // Single-flight: one Clear per List at a time.
            if model.pending_clears.contains_key(&list) {
                model.status_line =
                    Some("a clear is already in progress for this list".to_string());
                return Vec::new();
            }
            // Snapshot the Completed Tasks with their indices (ascending), then
            // optimistically drop them. If they were hidden the change is
            // invisible, but Google is still swept and the log holds their history.
            let removed: Vec<(usize, Task)> = model
                .tasks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.status == Status::Completed)
                .map(|(i, t)| (i, t.clone()))
                .collect();
            // The sweep shifts every index after a cleared row, so the cursor is
            // held by id. If its own row is going — only reachable with Completed
            // Tasks revealed — it steps to the nearest row that survives, rather
            // than falling back to the top of the pane.
            let anchor = selected_id(model).and_then(|id| {
                let swept = model
                    .tasks
                    .iter()
                    .any(|t| t.id == id && t.status == Status::Completed);
                if swept {
                    display_neighbour(model, &id, |t| t.status != Status::Completed)
                } else {
                    Some(id)
                }
            });
            model.tasks.retain(|t| t.status != Status::Completed);
            model.pending_clears.insert(list.clone(), removed);
            model.selected_task = anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
            reselect_visible(model);
            vec![Command::ClearCompleted { list }]
        }
    }
}

/// Keep `selected_list` in range after the List set shrinks.
fn clamp_list_selection(model: &mut Model) {
    model.selected_list = if model.lists.is_empty() {
        None
    } else {
        Some(model.selected_list.unwrap_or(0).min(model.lists.len() - 1))
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
    // Completing iff open.
    let completed = model.tasks[index].status == Status::NeedsAction;
    // Completing a Task while completed are hidden drops it from view, so the
    // cursor moves onto the next visible Task (the Google-app behaviour). Take
    // that successor *before* the status changes: completing empties the Task's
    // group key, which sinks it to the undated tail, and scanning from there
    // would anchor on the row before its new position instead of the next one
    // by due date.
    let hides_it = completed && !model.show_completed;
    let successor = hides_it.then(|| display_successor(model, &id)).flatten();

    set_completed(&mut model.tasks[index], completed);
    model.pending_writes.insert(id.clone(), snapshot);
    if hides_it {
        if let Some(next) = successor {
            model.selected_task = model.tasks.iter().position(|t| t.id == next);
        }
        reselect_visible(model);
    }
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
            move_task_cursor(model, delta);
            Vec::new()
        }
    }
}

/// Move the task cursor by `delta` through the **displayed** order
/// ([`visible_tasks`](Model::visible_tasks)) — the hierarchy under Manual, the
/// sorted list otherwise, hidden rows already excluded — then map back to the
/// Task's index in `tasks`. Clamps at the visible ends.
fn move_task_cursor(model: &mut Model, delta: isize) {
    let visible = model.visible_tasks();
    if visible.is_empty() {
        return;
    }
    let current_id = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .map(|t| t.id.clone());
    let current_pos = current_id
        .as_ref()
        .and_then(|id| visible.iter().position(|t| &t.id == id));
    let new_pos = match current_pos {
        Some(p) => (p as isize + delta).clamp(0, visible.len() as isize - 1) as usize,
        None => 0,
    };
    let target = visible[new_pos].id.clone();
    model.selected_task = model.tasks.iter().position(|t| t.id == target);
}

/// Ensure `selected_task` points at a visible Task, re-anchoring in **display**
/// order so the cursor lands where the eye is — under a Sort view the stored
/// neighbour is an arbitrary row.
///
/// - Keeps the current selection when its Task is still visible.
/// - Otherwise takes the nearest visible Task at or after it in display order,
///   then the nearest before it.
/// - With no usable selection — `None`, or an index past the end after the Task
///   set shrank — anchors on the first visible Task in display order. (Stored
///   index 0, the old clamp, is an arbitrary row in every lens but Manual.)
/// - `None` when nothing is visible.
fn reselect_visible(model: &mut Model) {
    let ordered = model.sorted_tasks();
    // Display position of the current selection, if it still resolves to a Task.
    let start = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .and_then(|sel| ordered.iter().position(|t| t.id == sel.id));

    if let Some(pos) = start {
        if model.is_visible(ordered[pos]) {
            return;
        }
    }
    let from = start.unwrap_or(0);
    let anchor = ordered[from..]
        .iter()
        .find(|t| model.is_visible(t))
        .or_else(|| ordered[..from].iter().rev().find(|t| model.is_visible(t)))
        .map(|t| t.id.clone());

    model.selected_task = anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
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
