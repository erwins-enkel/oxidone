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

/// The only two states a Task can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NeedsAction,
    Completed,
}

/// A local, read-only reordering of the visible Tasks. Never writes Manual order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortView {
    /// Google's `position` order ("My order"). The home state.
    Manual,
    /// By due date; Tasks with no due date sink to the bottom deterministically.
    Due,
    Title,
}

// Newtypes keep List and Task ids from being swapped by accident.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListId(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);
