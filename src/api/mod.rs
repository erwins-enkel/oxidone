//! The seam between the TEA core and Google (ADR-0004). Logic and sync code
//! depend on the `TasksApi` trait, not on `reqwest`, so they test against
//! `fake::FakeTasksApi`. `rest::RestClient` is the real hand-rolled client.

mod fake;
mod rest;

pub use fake::FakeTasksApi;
pub use rest::RestClient;

use crate::domain::{List, ListId, Task, TaskId};
use async_trait::async_trait; // NOTE: add `async-trait` to Cargo.toml, or use RPITIT.

/// Every Google Tasks operation oxidone needs. Deliberately thin — this API is
/// only 2 resources and ~11 methods.
#[async_trait]
pub trait TasksApi: Send + Sync {
    // Lists
    async fn list_lists(&self) -> Result<Vec<List>, ApiError>;
    /// The user's default List (`@default`). Resolved once to its concrete
    /// `ListId`; the alias itself is never stored (ADR-0003) — storing it would
    /// put two keys for one List in the cache. Used to target the Today `a`
    /// capture so a new entry lands on the page it was created on.
    async fn default_list(&self) -> Result<List, ApiError>;
    async fn insert_list(&self, title: &str) -> Result<List, ApiError>;
    async fn patch_list(&self, id: &ListId, title: &str) -> Result<List, ApiError>;
    async fn delete_list(&self, id: &ListId) -> Result<(), ApiError>;

    // Tasks. `updated_min` enables cheap incremental Refresh (future poll).
    async fn list_tasks(
        &self,
        list: &ListId,
        show_completed: bool,
        show_hidden: bool,
        updated_min: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<Vec<Task>, ApiError>;
    async fn insert_task(&self, list: &ListId, task: NewTask) -> Result<Task, ApiError>;
    async fn patch_task(
        &self,
        list: &ListId,
        id: &TaskId,
        patch: TaskPatch,
    ) -> Result<Task, ApiError>;
    async fn delete_task(&self, list: &ListId, id: &TaskId) -> Result<(), ApiError>;
    /// Reposition / reparent — the only writer of Manual order.
    async fn move_task(
        &self,
        list: &ListId,
        id: &TaskId,
        parent: Option<&TaskId>,
        previous: Option<&TaskId>,
    ) -> Result<Task, ApiError>;
    /// Relocate a Task to another List (`move` with `destinationTasklist`).
    ///
    /// Separate from [`TasksApi::move_task`] rather than a fifth parameter on it:
    /// this takes no `parent`/`previous` and returns a Task whose `list` changed.
    /// The Task always lands **top-level at the head** of `destination` — the one
    /// position Google permits for every Task, including a Completed-and-Cleared
    /// one, so a single rule covers every case.
    ///
    /// Refusing a Task that has Subtasks is *oxidone's* policy, not Google's, so
    /// it lives in `sync`, not here — implementations relocate whatever they are
    /// given.
    async fn move_task_to_list(
        &self,
        list: &ListId,
        id: &TaskId,
        destination: &ListId,
    ) -> Result<Task, ApiError>;
    /// Sweep Completed Tasks out of view (`hidden=true`).
    async fn clear_completed(&self, list: &ListId) -> Result<(), ApiError>;
}

#[derive(Debug, Clone, Default)]
pub struct NewTask {
    pub title: String,
    pub notes: Option<String>,
    pub due: Option<chrono::NaiveDate>,
    pub parent: Option<TaskId>,
}

/// Partial update; `None` fields are left untouched.
#[derive(Debug, Clone, Default)]
pub struct TaskPatch {
    pub title: Option<String>,
    pub notes: Option<Option<String>>,
    pub due: Option<Option<chrono::NaiveDate>>,
    pub completed: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ApiError {
    #[error("network error: {0}")]
    Network(String),
    #[error("auth expired")]
    AuthExpired,
    #[error("not found")]
    NotFound,
    #[error("google rejected the request: {status} {message}")]
    Rejected { status: u16, message: String },
}
