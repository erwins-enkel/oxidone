//! Reconciliation between the cache and Google.
//!
//! v1 (ADR-0001): the cache is the source of truth for reads. A refresh pulls
//! from Google via `TasksApi`, mirrors the result into the cache, and returns
//! the cached view. Write-through and the offline queue land in later slices.

use anyhow::Result;

use crate::api::TasksApi;
use crate::cache::Cache;
use crate::domain::{List, ListId, Task};

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
