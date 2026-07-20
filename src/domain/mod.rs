//! The ubiquitous language, as Rust types. Mirrors Google's model exactly
//! (ADR-0003: pure mirror). See `CONTEXT.md` for definitions.

use chrono::{DateTime, NaiveDate, Utc};

/// A Google TaskList — a named container of Tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub id: ListId,
    pub title: String,
    pub etag: String,
    pub updated: DateTime<Utc>,
}

/// A single Task. A Subtask is simply a Task whose `parent` is `Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub list: ListId,
    /// `Some` => this is a Subtask. Capped at one level: a Task with a parent
    /// may never itself be a parent.
    pub parent: Option<TaskId>,
    pub title: String,
    pub notes: Option<String>,
    pub status: Status,
    /// Date only — the API discards any time component (see CONTEXT.md).
    pub due: Option<NaiveDate>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Opaque Manual-order key; changed only via a Move.
    pub position: String,
    pub etag: String,
    pub updated: DateTime<Utc>,
}

impl Task {
    /// A Subtask is any Task with a parent. Nesting is capped at one level, so
    /// `is_subtask()` also means "cannot itself be a parent".
    pub fn is_subtask(&self) -> bool {
        self.parent.is_some()
    }
}

/// The only two states a Task can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NeedsAction,
    Completed,
}

/// A local, read-only regrouping of the visible Tasks. Every view keeps Subtasks
/// under their parent and only reorders the groups; none of them writes Manual
/// order or a Task's `parent` — only a Move does. Attempting a Move from a Sort
/// view switches the pane back to `Manual` first (see `move_preconditions`), so
/// the reorder lands against the adjacency the user can actually see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortView {
    /// Google's `position` order ("My order").
    Manual,
    /// By due date; Tasks with no due date sink to the bottom deterministically.
    /// The home state — a daily driver opens on what is due.
    Due,
    /// Case-insensitive by title.
    Title,
}

impl SortView {
    /// The next view in the triage cycle, starting from the `Due` home state:
    /// Due → Title → Manual → Due.
    pub fn next(self) -> Self {
        match self {
            SortView::Manual => SortView::Due,
            SortView::Due => SortView::Title,
            SortView::Title => SortView::Manual,
        }
    }

    /// A short lower-case label for the pane title. Every view names itself, so
    /// the header always says which lens is active — with `Due` the home state,
    /// an unlabelled pane would make Manual the silent one.
    pub fn label(self) -> &'static str {
        match self {
            SortView::Manual => "my order",
            SortView::Due => "due",
            SortView::Title => "title",
        }
    }
}

// Newtypes keep List and Task ids from being swapped by accident.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListId(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);
