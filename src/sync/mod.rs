//! Reconciliation between the cache and Google.
//!
//! v1 (ADR-0001): the cache is the source of truth for reads. A refresh pulls
//! from Google via `TasksApi`, mirrors the result into the cache, and returns
//! the cached view. Write-through and the offline queue land in later slices.

use anyhow::Result;

use crate::api::{ApiError, TaskPatch, TasksApi};
use crate::cache::Cache;
use crate::domain::{List, ListId, Task, TaskId};

/// Refresh Lists: fetch from Google, mirror into the cache (dropping Lists that
/// no longer exist), and return the cached Lists for the Model.
pub async fn load_lists(api: &dyn TasksApi, cache: &Cache) -> Result<Vec<List>> {
    let lists = api.list_lists().await?;
    mirror_lists(cache, &lists)
}

/// Refresh one List's Tasks: fetch the active view (completed shown, cleared
/// hidden), mirror into the cache, and return the cached Tasks for the Model.
pub async fn load_tasks(api: &dyn TasksApi, cache: &Cache, list: &ListId) -> Result<Vec<Task>> {
    let tasks = fetch_active_tasks(api, list).await?;
    mirror_tasks(cache, list, &tasks)
}

/// Fetch a List's active-view Tasks (`show_completed=true, show_hidden=false`).
/// Split out so a caller doing its own cache locking can fetch first (no lock
/// across the await) and mirror second — see the background workers in `main`.
pub async fn fetch_active_tasks(api: &dyn TasksApi, list: &ListId) -> Result<Vec<Task>> {
    Ok(api.list_tasks(list, true, false, None).await?)
}

/// Mirror fetched Lists into the cache and return the cached view (ADR-0001:
/// reads come from the cache, so callers render this, not the raw API response).
pub fn mirror_lists(cache: &Cache, lists: &[List]) -> Result<Vec<List>> {
    cache.replace_lists(lists)?;
    cache.lists()
}

/// Mirror fetched Tasks of one List and return the cached view.
pub fn mirror_tasks(cache: &Cache, list: &ListId, tasks: &[Task]) -> Result<Vec<Task>> {
    cache.replace_tasks(list, tasks)?;
    cache.tasks(list)
}

/// Set a Task's completed state on Google and return the updated Task from the
/// response. The cache is *not* touched — a caller doing its own locking mirrors
/// separately (see `main`'s worker); the combined [`write_completed`] is the
/// convenience used by tests. Auth-expiry retry (with a forced token refresh) is
/// handled uniformly inside `RestClient`, not here.
pub async fn patch_completed(
    api: &dyn TasksApi,
    list: &ListId,
    task: &TaskId,
    completed: bool,
) -> std::result::Result<Task, ApiError> {
    let patch = TaskPatch {
        completed: Some(completed),
        ..Default::default()
    };
    api.patch_task(list, task, patch).await
}

/// Write-through a completion toggle: patch on Google (retry-once), then mirror
/// the updated Task into the cache. Returns the updated Task.
pub async fn write_completed(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    task: &TaskId,
    completed: bool,
) -> Result<Task> {
    let updated = patch_completed(api, list, task, completed).await?;
    cache.upsert_task(&updated)?;
    Ok(updated)
}

/// Patch a Task's title on Google and return the updated Task (no cache write —
/// see [`patch_completed`] for the split rationale).
pub async fn patch_title(
    api: &dyn TasksApi,
    list: &ListId,
    task: &TaskId,
    title: &str,
) -> std::result::Result<Task, ApiError> {
    let patch = TaskPatch {
        title: Some(title.to_string()),
        ..Default::default()
    };
    api.patch_task(list, task, patch).await
}

/// Write-through a title edit: patch on Google, mirror into the cache.
pub async fn write_title(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    task: &TaskId,
    title: &str,
) -> Result<Task> {
    let updated = patch_title(api, list, task, title).await?;
    cache.upsert_task(&updated)?;
    Ok(updated)
}

/// Delete a Task on Google and mirror the removal into the cache.
pub async fn delete_task(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    task: &TaskId,
) -> Result<()> {
    api.delete_task(list, task).await?;
    cache.delete_task(task)?;
    Ok(())
}
