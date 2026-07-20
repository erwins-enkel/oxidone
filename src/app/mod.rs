//! The Elm Architecture (ADR-0005). `update` is the single, pure place state
//! changes; it is unit-testable with no terminal and no network. Async workers
//! (api/sync/auth) only ever emit `Message`s into this reducer, and `update`
//! emits `Command`s for them to run.

use std::collections::{HashMap, HashSet};

use chrono::NaiveDate;

use crate::dateparse;
use crate::domain::{EntryType, List, ListId, SortView, Status, Task, TaskId};
use crate::keymap::{self, Action};
use crate::links::{self, Link, OpenableUrl};

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
    /// Ids confirmed deleted or Cleared, per List, so a *stale* refresh cannot
    /// resurrect the row. A fetch issued before Google applied the delete still
    /// lists the id; if its `TasksLoaded` lands *after* the confirmation dropped
    /// the rollback snapshot, nothing else records the row is gone. `set_tasks`
    /// drops any tombstoned id from the fetch, and evicts a tombstone once a
    /// fetch of *that List* omits the id (Google has caught up). Keyed by List so
    /// a fetch of another List never evicts these — closing the switch-away race.
    ///
    /// Ephemeral reducer bookkeeping, the same category as `pending_deletes`, not
    /// a field on the Task cache: it augments nothing Google stores and
    /// round-trips nothing, so the pure-mirror rule (ADR-0003) holds.
    tombstones: HashMap<ListId, HashSet<TaskId>>,
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
    /// `(done, total)` per List, re-derived from the cache whenever it changes
    /// (see the recount at the runtime's event edge). The sidebar reads this for
    /// every List but the active one, which derives live from `tasks` so a
    /// completion moves its meter in the same frame.
    ///
    /// May hold entries for Lists no longer in `lists` — they are unreachable,
    /// since the sidebar only ever looks up Lists it is drawing, and the map is
    /// replaced wholesale on the next recount.
    ///
    /// Private like the other reducer-owned bookkeeping: the view reads it
    /// through [`Model::list_meter`], which decides live-versus-cached.
    list_counts: HashMap<ListId, (usize, usize)>,
    /// The one bit that tells [`set_lists`]'s two callers apart. A manual Refresh
    /// (`r`) is a **full** fan-out — every List's Tasks — so its meters all reflect
    /// Google; the startup cascade is **lazy**, fetching only Lists the cache
    /// aggregate did not already cover. Set in [`refresh`] (after the offline
    /// guard, so an offline `r` never latches a flag no `ListsLoaded` consumes) and
    /// consumed — read-and-cleared — by [`set_lists`], so it can never persist into
    /// a later lazy load. Private, like `list_counts`.
    pending_refresh: bool,
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
    /// Pick which of a Task's links to open — merged from its `links[]` and its
    /// notes URLs. Only raised for more than one; a single link opens without
    /// asking.
    OpenLink { links: Vec<Link>, selected: usize },
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
            Overlay::Confirm(_) | Overlay::OpenLink { .. } => None,
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
            tombstones: HashMap::new(),
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
            // Seeded from the cache aggregate at startup, then re-derived on
            // every cache change; empty until then means "no meters yet".
            list_counts: HashMap::new(),
            // No Refresh in flight; the startup cascade is lazy.
            pending_refresh: false,
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
    ///   lifts its parent and a Completed one never does — the position a group
    ///   takes is always explained by a date visible in it.
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
            // Sorts on the *display* title: U+2014 sorts above every ASCII
            // letter, so sorting raw would exile every typed entry to the tail
            // and separate it from the Tasks it reads alongside.
            SortView::Title => {
                for group in &mut groups {
                    group[1..].sort_by_cached_key(|t| t.display_title().to_lowercase());
                }
                groups.sort_by_cached_key(|g| g[0].display_title().to_lowercase());
            }
        }
        groups.into_iter().flatten().collect()
    }

    /// Split `tasks` into groups in Vec order: each top-level Task followed by its
    /// Subtasks, then each orphaned Subtask (no present top-level parent, e.g.
    /// after the parent was deleted) as a group of its own. Every Task lands in
    /// exactly one group, so nothing ever vanishes and flattening always
    /// reproduces the whole set.
    ///
    /// Linear in the Task count: every lens routes through here, and `Due` is the
    /// default, so this runs on each redraw (via `visible_tasks`) and on each
    /// cursor re-anchor. Bucketing children up front keeps it off the quadratic
    /// rescan the hierarchy used to cost when only `Manual` paid it.
    fn groups(&self) -> Vec<Vec<&Task>> {
        let top_level = self.top_level_ids();
        // One pass buckets every child under its parent, so building the groups
        // below is a hash lookup each rather than a rescan of `tasks`.
        let mut children: HashMap<&TaskId, Vec<&Task>> = HashMap::new();
        for task in &self.tasks {
            if let Some(parent) = task.parent.as_ref().filter(|p| top_level.contains(p)) {
                children.entry(parent).or_default().push(task);
            }
        }

        let mut out: Vec<Vec<&Task>> = Vec::with_capacity(top_level.len());
        for parent in self.tasks.iter().filter(|t| t.parent.is_none()) {
            let mut group = vec![parent];
            if let Some(kids) = children.get(&parent.id) {
                group.extend(kids.iter().copied());
            }
            out.push(group);
        }
        // Orphans are appended after every parent group. Under `Manual` that
        // puts them last (what `hierarchical` did); under the sorted lenses they
        // are ordinary single-Task groups, so they sort on their own key and
        // only fall back to this trailing position when that key ties. A parent
        // that is itself a Subtask counts as absent — `groups` nests only under
        // top-level Tasks, which is the rule `renders_as_subtask` draws.
        for task in &self.tasks {
            if task.parent.as_ref().is_some_and(|p| !top_level.contains(p)) {
                out.push(vec![task]);
            }
        }
        out
    }

    /// The ids of every top-level Task — the rows a Subtask can be drawn under.
    /// Built once per render so the per-row indent check is a hash lookup rather
    /// than a scan of `tasks`.
    pub fn top_level_ids(&self) -> HashSet<&TaskId> {
        self.tasks
            .iter()
            .filter(|t| t.parent.is_none())
            .map(|t| &t.id)
            .collect()
    }

    /// Whether any per-List sidebar meter has been seeded from the cache
    /// aggregate. False only before the first `recount`, or on a cache holding no
    /// Tasks. The runtime asserts this holds (against a non-empty cache) before the
    /// seed `ListsLoaded`, because [`set_lists`]'s lazy fan-out reads `list_counts`
    /// to decide coverage — a check the pure-reducer tests cannot observe.
    pub fn has_seeded_meters(&self) -> bool {
        !self.list_counts.is_empty()
    }

    /// `(done, total)` for a List's sidebar meter, or `None` when there is
    /// nothing honest to draw — the List is empty, or its Tasks have never been
    /// cached. Both render as *absent*, because neither is "0% done".
    ///
    /// The active List derives **live** from `tasks`, so completing a Task moves
    /// its meter in the same frame and a rolled-back write moves it back; the
    /// cache cannot do that, since write-through only mirrors after Google
    /// confirms. Every other List reads the recount map, which holds confirmed
    /// state — so a failed write can never leave a wrong count behind.
    ///
    /// An empty pane falls through to the map rather than deriving `(0, 0)`:
    /// between a List change and its `TasksLoaded` the pane is empty because
    /// nothing has arrived, not because the List is. The cost is that emptying
    /// the active List shows its pre-delete count until the recount lands.
    pub fn list_meter(&self, list: &ListId) -> Option<(usize, usize)> {
        let (done, total) = if self.selected_list_id() == Some(list) && !self.tasks.is_empty() {
            // Counted the same way the task-pane header counts — Subtasks
            // included, Events and Notes excluded — so the two meters for this
            // List always agree. The type filter is what keeps that true: the
            // header counts only actionable entries, and a sidebar row that
            // counted Events beside it would contradict the row it sits next to.
            let actionable = || {
                self.tasks
                    .iter()
                    .filter(|t| t.entry_type() == EntryType::Task)
            };
            let done = actionable()
                .filter(|t| t.status == Status::Completed)
                .count();
            (done, actionable().count())
        } else {
            *self.list_counts.get(list)?
        };
        (total > 0).then_some((done, total))
    }

    /// `(done, total)` of every row the pane draws **nested under** each parent.
    ///
    /// Keyed off `renders_as_subtask`, never a raw `parent`: a Task parented to a
    /// Subtask, or to a parent absent from the List, draws flush-left as its own
    /// group, so it is nobody's Subtask here either. Counting `parent` directly
    /// would give such a row's parent a meter and credit it a child drawn
    /// elsewhere.
    ///
    /// Takes the caller's `top_level` set so the meter and the indent decision
    /// read the same data and cannot disagree — and so this stays one pass over
    /// `tasks` rather than a scan per row.
    ///
    /// Counts only Task-typed Subtasks, as the header and sidebar meters do: a
    /// Note nested under a parent is a jotting about it, not a step toward it,
    /// and counting it would hold the parent's meter below full forever.
    pub fn subtask_counts<'a>(
        &'a self,
        top_level: &HashSet<&'a TaskId>,
    ) -> HashMap<&'a TaskId, (usize, usize)> {
        let mut counts: HashMap<&TaskId, (usize, usize)> = HashMap::new();
        for task in &self.tasks {
            if !renders_as_subtask(top_level, task) || task.entry_type() != EntryType::Task {
                continue;
            }
            let parent = task
                .parent
                .as_ref()
                .expect("renders_as_subtask implies a parent");
            let entry = counts.entry(parent).or_insert((0, 0));
            entry.1 += 1;
            if task.status == Status::Completed {
                entry.0 += 1;
            }
        }
        counts
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

/// The parent was deleted locally and a Refresh has not caught up.
const ORPHANED: &str = "its parent was deleted — refresh (r)";
/// The parent is itself a Subtask — deeper than Google's one-level cap allows.
const NESTED_TOO_DEEP: &str = "its parent is a subtask — refresh (r)";

/// Why the verbs must refuse to write to `task`, if they must.
///
/// A row is *detached* when it carries a `parent` that the pane does not draw it
/// under — which is exactly when [`renders_as_subtask`] is false while `parent`
/// is set, and exactly the rows [`Model::groups`] emits as their own group. Both
/// cases are transient or malformed, and writing to either is unsafe:
///
/// - the parent is **gone**: Google deletes Subtasks along with their parent, so
///   the row is very likely already deleted server-side and any Move or insert
///   would race that;
/// - the parent is **itself a Subtask**: depth-2 data Google's one-level cap
///   should make impossible, so there is nothing sound to nest or reorder under.
///
/// Keeping this on the same rule the display uses is the point: a row drawn
/// flush-left must never be told it is "already a subtask", and one drawn
/// indented must never be refused as detached.
fn detached_reason(model: &Model, task: &Task) -> Option<&'static str> {
    let parent = task.parent.as_ref()?;
    if !model.tasks.iter().any(|t| &t.id == parent) {
        return Some(ORPHANED);
    }
    (!renders_as_subtask(&model.top_level_ids(), task)).then_some(NESTED_TOO_DEEP)
}

/// Whether `task` should render indented, given `top_level` from the same Model
/// (see [`Model::top_level_ids`]). True only when its parent is a present,
/// top-level Task — which is exactly what [`Model::groups`] nests it under, so a
/// row this returns `false` for is one grouping already treats as top-level: an
/// orphan whose parent was deleted, or (malformed) one parented to a Subtask.
/// Either renders flush-left rather than claiming the row above it as a parent.
///
/// Independent of `show_completed` — a hidden parent is still a parent — so the
/// indent never flickers with the toggle.
pub fn renders_as_subtask(top_level: &HashSet<&TaskId>, task: &Task) -> bool {
    task.parent.as_ref().is_some_and(|p| top_level.contains(p))
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

/// Sort key for a due date under `Due`: dated ascending, undated last. The
/// leading flag does the sinking — `Option`'s own ordering would put `None`
/// first — and every undated value compares equal, so a stable sort leaves the
/// tail in stored order.
fn due_key(due: Option<NaiveDate>) -> (bool, Option<NaiveDate>) {
    (due.is_none(), due)
}

/// The `Due` key for a group: the earliest due date among its **incomplete**
/// Tasks, so an urgent Subtask lifts its parent out of the tail.
///
/// Completed Tasks never contribute, so a group is never placed by a date the
/// user cannot see in it — with the filter on, a group whose only dated member is
/// Completed would sit high in the pane with nothing on screen to explain why.
/// Note this is *not* what keeps the order stable across the filter: ordering
/// reads only `tasks`, never `show_completed` (the flag is applied afterwards, by
/// `visible_tasks`), so toggling adds and removes rows without moving any.
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
    /// Per-List `(done, total)` re-derived from the cache. Emitted at startup and
    /// after every cache change, so it carries the whole picture rather than a
    /// delta — the arm replaces `list_counts` outright.
    CountsLoaded(HashMap<ListId, (usize, usize)>),
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
    /// A List's Tasks failed to load. Attributed to its List (unlike the id-less
    /// `LoadFailed`) so a fan-out failure surfaces only when that List is active —
    /// a background List's failure must not read as the active pane failing, and a
    /// cold start would otherwise emit one per uncovered List.
    TasksLoadFailed {
        list: ListId,
        reason: String,
    },
    /// A load failed; the reason is shown on the status line.
    LoadFailed(String),
    /// Handing a URL to the browser failed. Nothing to roll back — opening a
    /// link mutates no state — but the attempt must not fail silently.
    LinkOpenFailed {
        reason: String,
    },
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
    /// Hand a URL to the platform browser. Carries an [`OpenableUrl`], so the
    /// scheme was checked before the value existed — the runtime opens it
    /// without re-deciding.
    OpenUrl(OpenableUrl),
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
        Message::CountsLoaded(counts) => {
            // Replaced wholesale, and deliberately *not* filtered against
            // `lists`: an entry for a List we no longer show is unreachable —
            // the sidebar only looks up Lists it draws — so filtering would buy
            // nothing while making this arm care whether it ran before or after
            // `ListsLoaded`. Emits no Commands; the runtime relies on that.
            model.list_counts = counts;
            Vec::new()
        }
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
            // Confirmed gone; drop the rollback snapshot. Two interleavings need
            // handling on top of the ordinary case, where the row is already off
            // screen and neither branch does anything:
            //
            // - A stale refresh may *still* land after this reply, its fetch
            //   issued before Google applied the delete. Remember the id as a
            //   tombstone under its List, so `set_tasks` drops it from that later
            //   fetch instead of resurrecting the row (#65).
            // - A refresh may *already* have landed inside the round-trip and
            //   re-added the row from a fetch Google had not yet applied the delete
            //   to — so re-remove it here too, mirroring the guard
            //   `TaskDeleteFailed` carries for the same interleaving (#51). Only
            //   the resurrected case does any work; with the row already absent
            //   this stays a no-op, so a delete reply never disturbs a cursor the
            //   user has moved on to.
            //
            // Only a delete we initiated does either: a spurious reply carries no
            // snapshot and must not suppress or drop a live row. No "list still
            // active" guard is needed (unlike `ClearedCompleted`): Task ids are
            // unique across Lists, so a reply arriving after a List switch matches
            // nothing.
            if let Some((_, previous)) = model.pending_deletes.remove(&task) {
                if model.tasks.iter().any(|t| t.id == task) {
                    // Only a cursor sitting on the resurrected row needs a new
                    // home; one elsewhere is left where the user put it. Either
                    // way the `retain` shifts indices, so the anchor is resolved
                    // by id after.
                    let anchor = if selected_id(model).as_ref() == Some(&task) {
                        // Taken before the row goes: afterwards it has no display
                        // position to anchor from.
                        display_successor(model, &task)
                    } else {
                        selected_id(model)
                    };
                    model.tasks.retain(|t| t.id != task);
                    model.selected_task =
                        anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                    reselect_visible(model);
                }
                model
                    .tombstones
                    .entry(previous.list)
                    .or_default()
                    .insert(task);
            }
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
                // The insert shifts every later index, so hold the cursor by id —
                // then re-anchor, since a refresh that emptied the pane leaves no
                // selection and this Task would otherwise render unhighlighted.
                let selected = selected_id(model);
                model.tasks.insert(0, task);
                model.selected_task =
                    selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                reselect_visible(model);
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
            // Confirmed swept; drop the rollback snapshot. Two interleavings, as
            // with a delete — by snapshot **id** throughout, never by
            // `Status::Completed`: a refresh may also carry Tasks completed after
            // the sweep, which this Clear never swept and which must stay.
            if let Some(removed) = model.pending_clears.remove(&list) {
                let swept: HashSet<TaskId> = removed.into_iter().map(|(_, task)| task.id).collect();
                // A refresh may *already* have landed inside the round-trip and
                // re-added the swept Tasks from a fetch Google had not yet cleared
                // — so re-remove them (#51). Only when that List is still active:
                // `model.tasks` holds just the active pane, and a List switch
                // during the Clear left a different one in place (cf. the failure
                // twin).
                if model.selected_list_id() == Some(&list)
                    && model.tasks.iter().any(|t| swept.contains(&t.id))
                {
                    // A cursor on a resurrected row steps to the nearest row that
                    // survives; one elsewhere is held by id, since the `retain`
                    // shifts every later index.
                    let anchor = selected_id(model).and_then(|id| {
                        if swept.contains(&id) {
                            display_neighbour(model, &id, |t| !swept.contains(&t.id))
                        } else {
                            Some(id)
                        }
                    });
                    model.tasks.retain(|t| !swept.contains(&t.id));
                    model.selected_task =
                        anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
                    reselect_visible(model);
                }
                // A stale refresh may *still* land later — even after switching
                // away and back — so tombstone the swept ids under their List,
                // regardless of the active pane, so `set_tasks` drops them instead
                // of resurrecting them (#65).
                model.tombstones.entry(list).or_default().extend(swept);
            }
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
        Message::LinkOpenFailed { reason } => {
            model.status_line = Some(format!("could not open link: {reason}"));
            Vec::new()
        }
        Message::TasksLoadFailed { list, reason } => {
            // Surface only the active List's failure. A background List's is
            // dropped, which is the fail-closed outcome: no `TasksLoaded` means no
            // counts, means no meter on that row — the honest state.
            if model.selected_list_id() == Some(&list) {
                model.status_line = Some(reason);
            }
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
/// then request Tasks — the active List's always, plus a fan-out to fill the
/// **sidebar meters** of Lists whose Tasks are not already loaded.
///
/// The fan-out policy turns on [`Model::pending_refresh`], read-and-cleared here:
///
/// - **full** (a manual Refresh): every List. `r` promises the latest from Google
///   for *all* meters, so a Refresh that touched only the active pane would lie.
/// - **lazy** (the startup cascade): only Lists the cache aggregate did not cover
///   (absent from `list_counts`), so a List never opened on this machine gets a
///   meter without a visit, while covered ones are left alone.
///
/// The active List is **never** part of the fan-out — [`request_selected_tasks`]
/// already emitted its `LoadTasks`, and re-emitting would fetch it twice on a cold
/// cache. So the commands come back **active List first**, then the fan-out.
///
/// A background List's fetch reports through [`Message::TasksLoadFailed`], not the
/// id-less `LoadFailed`, so one List's failure never splashes onto the active
/// pane's status line (see the reducer arm).
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
    let mut commands = request_selected_tasks(model, list_changed);

    // Consume the flag unconditionally, so a Refresh's `full` never carries into a
    // later lazy cascade.
    let full = std::mem::take(&mut model.pending_refresh);
    let active = model.selected_list_id().cloned();
    for list in &model.lists {
        // The active List's `LoadTasks` is already the head of `commands`.
        if active.as_ref() == Some(&list.id) {
            continue;
        }
        // Lazy: skip a List the aggregate already covers. Full: take every List.
        if full || !model.list_counts.contains_key(&list.id) {
            commands.push(Command::LoadTasks(list.id.clone()));
        }
    }
    commands
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

/// Drop tombstoned ids from a `list` fetch, and evict any tombstone the fetch
/// shows Google has caught up on. A tombstone marks an id whose delete/Clear was
/// confirmed but which a stale fetch may still list (its request predated the
/// delete); dropping it keeps the row gone (#65).
///
/// Reads the incoming id set *before* filtering, so a just-suppressed id is not
/// mistaken for "absent" and does not evict its own tombstone in the same pass.
/// Eviction is scoped to `list`, so a fetch of another List never spends these.
fn reconcile_tombstones(model: &mut Model, list: &ListId, tasks: Vec<Task>) -> Vec<Task> {
    let Some(set) = model.tombstones.get_mut(list) else {
        return tasks;
    };
    let incoming: HashSet<TaskId> = tasks.iter().map(|t| t.id.clone()).collect();
    // Evict tombstones Google has processed (id absent from this fetch)…
    set.retain(|id| incoming.contains(id));
    // …then drop the ids that are still stale.
    let tasks = tasks.into_iter().filter(|t| !set.contains(&t.id)).collect();
    if set.is_empty() {
        model.tombstones.remove(list);
    }
    tasks
}

/// Fill the task pane, ignoring results for a List that is no longer active.
/// Keeps the task cursor on the same Task by id where possible.
fn set_tasks(model: &mut Model, list: &ListId, tasks: Vec<Task>) {
    if model.selected_list_id() != Some(list) {
        return;
    }
    let tasks = reconcile_tombstones(model, list, tasks);
    let previously_selected = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .map(|t| t.id.clone());
    model.tasks = tasks;
    // Keep the cursor on the same Task. If it is gone, leave the selection empty
    // and let `reselect_visible` anchor it: with nothing to preserve it takes the
    // first *visible* row in display order, never stored index 0 — an arbitrary
    // row in every lens but Manual.
    model.selected_task =
        previously_selected.and_then(|id| model.tasks.iter().position(|t| t.id == id));
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
        Action::Migrate => return migrate(model),
        Action::CycleType => return cycle_type(model, EntryType::next),
        Action::CycleTypeBack => return cycle_type(model, EntryType::prev),
        Action::EditNotes => return edit_notes(model),
        Action::OpenLink => return open_link(model),
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

/// Open the title editor on the *display* title, so a typed entry's glyph never
/// enters a buffer the user types into. `finish_edit_title` re-applies the type.
fn open_edit_title(model: &mut Model) {
    if let Some(task) = focused_task(model) {
        model.overlay = Some(Overlay::EditTitle {
            task: task.id.clone(),
            buffer: task.display_title().to_string(),
        });
    }
}

/// Migrate the selected Task: push its due date one day past whichever is later,
/// today or its current due date. Bullet Journal's `>` disposition.
///
/// `max(today, due) + 1` rather than a flat "tomorrow" so the verb composes:
/// an overdue Task lands on tomorrow, a future one shifts a day, and repeated
/// presses defer repeatedly. An undated Task gets tomorrow — `max` has nothing
/// to compare against.
///
/// Refuses a Completed Task. Migration is for Tasks still `needsAction`;
/// re-dating a finished one is semantically empty, and `m` is pressed rapidly
/// down a list where a silent due-date rewrite would go unnoticed. This is not
/// the same as blocking a state Google permits — `d` still sets any date on a
/// Completed Task.
///
/// Rides `SetDue`, so single-flight guarding and rollback come from that path.
fn migrate(model: &mut Model) -> Vec<Command> {
    let Some(task) = focused_task(model) else {
        return Vec::new();
    };
    if task.status == Status::Completed {
        model.status_line = Some("completed tasks are not migrated".to_string());
        return Vec::new();
    }
    let (id, current) = (task.id.clone(), task.due);
    // Single-flight: don't lose the migration silently if a write is running.
    if model.pending_writes.contains_key(&id) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(index) = model.tasks.iter().position(|t| t.id == id) else {
        return Vec::new();
    };
    let today = model.now.date_naive();
    let due = Some(current.map_or(today, |d| d.max(today)) + chrono::Duration::days(1));
    model
        .pending_writes
        .insert(id.clone(), model.tasks[index].clone());
    model.tasks[index].due = due;
    vec![Command::SetDue {
        list,
        task: id,
        due,
    }]
}

/// Cycle the selected entry's Bullet Journal type, `step` choosing the direction
/// ([`EntryType::next`] for `t`, [`EntryType::prev`] for `T`).
///
/// The type lives in the title (ADR-0008), so this is a title write and rides
/// `SetTitle` — single-flight guarding and rollback come from that path.
///
/// Uses [`EntryType::retype`], never `apply`: retyping must repair a foreign
/// prefix first, or a title Google handed back as `"○Standup"` would stack into
/// `"○ ○Standup"` on the first press. `None` means the title strips to nothing,
/// which is not something a type can be attached to.
fn cycle_type(model: &mut Model, step: fn(EntryType) -> EntryType) -> Vec<Command> {
    let Some(task) = focused_task(model) else {
        return Vec::new();
    };
    let id = task.id.clone();
    let Some(title) = step(task.entry_type()).retype(task.display_title()) else {
        model.status_line = Some("an entry needs a title before it can be typed".to_string());
        return Vec::new();
    };
    // Single-flight: don't lose the type change silently if a write is running.
    // `T` exists partly so this is rarely hit — every type is one press away.
    if model.pending_writes.contains_key(&id) {
        model.status_line = Some("a write is already in progress for this task".to_string());
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(index) = model.tasks.iter().position(|t| t.id == id) else {
        return Vec::new();
    };
    model
        .pending_writes
        .insert(id.clone(), model.tasks[index].clone());
    model.tasks[index].title = title.clone();
    vec![Command::SetTitle {
        list,
        task: id,
        title,
    }]
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
    if let Some(reason) = detached_reason(model, task) {
        model.status_line = Some(reason.to_string());
        return;
    }
    // A Subtask adds a sibling under its own parent; a top-level Task parents it.
    let parent = match &task.parent {
        Some(p) => p.clone(),
        None => task.id.clone(),
    };
    model.overlay = Some(Overlay::AddSubtask {
        parent,
        buffer: String::new(),
    });
}

/// Optimistically insert a placeholder Subtask directly after `parent` in
/// **stored** order (its first child, matching Google's top-of-list insert) and
/// request the insert. Where it *renders* is up to the active lens, exactly as
/// for [`finish_add_task`]: the placeholder carries no due date, so under `Due`
/// it appears in the group's undated tail rather than as its first child — at
/// the head of that tail, since the sort is stable and it is the first child in
/// stored order. The cursor follows it either way.
fn finish_add_subtask(model: &mut Model, parent: TaskId, buffer: String) -> Vec<Command> {
    let title = buffer.trim().to_string();
    if title.is_empty() {
        return Vec::new();
    }
    let Some(list) = model.selected_list_id().cloned() else {
        return Vec::new();
    };
    let Some(pidx) = model.tasks.iter().position(|t| t.id == parent) else {
        // The parent went while the overlay was open (a refresh dropped it, or a
        // delete landed). Say so rather than swallowing the keystroke.
        model.status_line = Some("that task is gone — refresh (r)".to_string());
        return Vec::new();
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
            // A brand-new Task has no `links[]` (output-only; Google attaches
            // them only on the surface it was created from).
            links: Vec::new(),
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
/// off-screen. Ordering the switch last keeps the *in-flight* refusals — a Move
/// or a field write already running — from disturbing the lens: those report and
/// return with `sort` untouched.
///
/// It does **not** cover the verb-level refusals, which run after this function
/// returns and so always see the lens already switched: `detached_reason`,
/// "already a subtask", "not a subtask", "already first"/"already last". From a
/// Sort view those take two presses — the first switches, the second refuses with
/// the verb's own reason. That is intended: the refusal is about the row's
/// position among its neighbours, which the user can only judge once the pane is
/// showing the order the Move would act on.
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
    if let Some(reason) = detached_reason(model, &model.tasks[idx]) {
        model.status_line = Some(reason.to_string());
        return Vec::new();
    }
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
    if let Some(reason) = detached_reason(model, &model.tasks[idx]) {
        model.status_line = Some(reason.to_string());
        return Vec::new();
    }
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
    if let Some(reason) = detached_reason(model, &model.tasks[idx]) {
        model.status_line = Some(reason.to_string());
        return Vec::new();
    }
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

/// Open a link on the selected Task — merged from its `links[]` (Gmail/Chat/
/// Keep/Docs origin) and the URLs in its notes, deduped (#55).
///
/// Reports on every path. Nothing found and nothing *openable* are different
/// outcomes and say so differently: a `links[]` entry that is a `mailto:`, or a
/// notes blob full of `file:` URLs, does have links — they just aren't ones a
/// browser should be handed. Success reports too — the opener is detached and
/// silent, so without a status line a working `u` is indistinguishable from an
/// unbound key.
fn open_link(model: &mut Model) -> Vec<Command> {
    let Some(task) = focused_task(model) else {
        return Vec::new();
    };
    let notes = task.notes.as_deref().unwrap_or_default();
    let merged = links::openable_links(&task.links, notes);
    let mut openable = merged.openable;

    match openable.len() {
        0 => {
            model.status_line = Some(if merged.found == 0 {
                "no links on this task".to_string()
            } else {
                let plural = if merged.found == 1 { "link" } else { "links" };
                format!(
                    "{} {plural} found, none openable (http/https only)",
                    merged.found
                )
            });
            Vec::new()
        }
        1 => vec![open_url(model, openable.remove(0).url().clone())],
        _ => {
            model.overlay = Some(Overlay::OpenLink {
                links: openable,
                selected: 0,
            });
            Vec::new()
        }
    }
}

/// Announce the URL on the status line and emit the open. Shared by the
/// single-URL path and the picker so both report identically.
fn open_url(model: &mut Model, url: OpenableUrl) -> Command {
    model.status_line = Some(format!("opening {}", url.as_str()));
    Command::OpenUrl(url)
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
    // The display title: a destructive prompt is no place to leak the encoding.
    let (id, title) = (task.id.clone(), task.display_title().to_string());
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
    // Set *after* the offline guard: an offline `r` returns above without a
    // `RefreshLists`, so no `ListsLoaded` follows to consume the flag — setting it
    // unconditionally would latch it into the next (lazy) startup cascade.
    model.pending_refresh = true;
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

/// Route a key to the active overlay: cursor keys for the link picker, yes/no
/// for `Confirm`, text editing for the input overlays.
fn overlay_key(model: &mut Model, key: crossterm::event::KeyEvent) -> Vec<Command> {
    use crossterm::event::KeyCode;
    // Three-way on the overlay's kind. "Has a text buffer, else y/n" is not
    // enough: the picker has no buffer either, and falling through to the
    // confirm arm would let `y` silently dismiss it.
    match model.overlay {
        Some(Overlay::OpenLink { .. }) => return picker_key(model, key),
        Some(Overlay::Confirm(_)) => {
            return match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => execute_confirm(model),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    model.overlay = None;
                    Vec::new()
                }
                _ => Vec::new(),
            }
        }
        _ => {}
    }
    let Some(buffer) = model.overlay.as_mut().and_then(Overlay::input_buffer) else {
        return Vec::new();
    };
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
}

/// Keys for the link picker: move the cursor, open the selected link, or cancel.
fn picker_key(model: &mut Model, key: crossterm::event::KeyEvent) -> Vec<Command> {
    use crossterm::event::KeyCode;
    let Some(Overlay::OpenLink { links, selected }) = model.overlay.as_mut() else {
        return Vec::new();
    };
    match key.code {
        // Clamped rather than wrapping, matching pane selection.
        KeyCode::Char('j') | KeyCode::Down => {
            *selected = (*selected + 1).min(links.len().saturating_sub(1));
        }
        KeyCode::Char('k') | KeyCode::Up => *selected = selected.saturating_sub(1),
        KeyCode::Enter => {
            let url = links[*selected].url().clone();
            model.overlay = None;
            return vec![open_url(model, url)];
        }
        KeyCode::Esc => model.overlay = None,
        // Everything else is swallowed, as every other overlay already does —
        // `q` must not quit with a modal up.
        _ => {}
    }
    Vec::new()
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

/// Save an edited title, re-applying the entry's type.
///
/// Two orderings here are load-bearing.
///
/// **The empty-check runs before `apply`.** Applying first would turn a cleared
/// Note title into a bare `"— "` — a title that is pure encoding with no content.
///
/// **The type is read now, not when the overlay opened.** A type change landing
/// mid-edit (a refetch, a reconcile) is therefore preserved rather than reverted
/// by a stale capture.
///
/// Uses [`EntryType::apply`], never `retype`: this path must not strip. Opening
/// `e` on a title Google wrote as `"○Standup"` and pressing Enter unchanged has
/// to write it back byte-identical — a keystroke meaning "save what is already
/// there" must not silently delete a character.
fn finish_edit_title(model: &mut Model, task: TaskId, buffer: String) -> Vec<Command> {
    let display = buffer.trim();
    if display.is_empty() {
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
    let title = model.tasks[index].entry_type().apply(display);
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
            // The verb acts on the selection, so the cursor is normally on the
            // row about to go — but the overlay only gates keys, not the async
            // `TasksLoaded` of an in-flight Refresh, which can re-anchor the
            // cursor while the confirm is open. So re-home on the same terms as
            // the async paths: only a cursor actually on the doomed row follows
            // its successor; any other is re-resolved by id, since the `remove`
            // shifts later indices.
            let selected = selected_id(model);
            let anchor = if selected.as_ref() == Some(&task) {
                display_successor(model, &task)
            } else {
                selected
            };
            let removed = model.tasks.remove(index); // optimistic delete
            model.pending_deletes.insert(task.clone(), (index, removed));
            model.selected_task = anchor.and_then(|id| model.tasks.iter().position(|t| t.id == id));
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
/// ([`visible_tasks`](Model::visible_tasks)) — the grouped hierarchy in every
/// lens, ordered by the active one, hidden rows already excluded — then map back
/// to the Task's index in `tasks`. Clamps at the visible ends.
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
