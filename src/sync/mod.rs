//! Reconciliation between the cache and Google.
//!
//! v1 (ADR-0001): the cache is the source of truth for reads. A refresh pulls
//! from Google via `TasksApi`, mirrors the result into the cache, and returns
//! the cached view. Write-through and the offline queue land in later slices.

use anyhow::Result;
use chrono::NaiveDate;

use crate::api::{ApiError, TaskPatch, TasksApi};
use crate::cache::Cache;
use crate::domain::{due_on_or_before, List, ListId, Task, TaskId};

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

/// The **Today** aggregate straight from the cache: every cached Task due on or
/// before `today`, across all Lists (each carrying its own `list`).
///
/// Borrow-based and **spawn-free** — the concurrent live fan-out lives in
/// `main.rs`'s `spawn_load_today`, keeping the "all `tokio::spawn` in `main.rs`"
/// convention. This serves the Today pane's *instant paint* (and the offline
/// path): the same cache read the fan-out worker finishes with, minus the network.
pub fn today_from_cache(cache: &Cache, today: NaiveDate) -> Result<Vec<Task>> {
    Ok(cache
        .all_tasks()?
        .into_iter()
        .filter(|t| due_on_or_before(t.due, today))
        .collect())
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

/// Insert a Task into a List on Google and mirror it into the cache. Returns
/// the server Task (with its real id/position).
pub async fn insert_task(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    title: &str,
) -> Result<Task> {
    let new = crate::api::NewTask {
        title: title.to_string(),
        ..Default::default()
    };
    let task = api.insert_task(list, new).await?;
    cache.upsert_task(&task)?;
    Ok(task)
}

/// Patch a Task's due date on Google and return the updated Task (no cache write
/// — see [`patch_completed`] for the split rationale). `due` is `Some(None)` to
/// clear it, `Some(Some(date))` to set it; the outer `Some` marks the field as
/// changed in the [`TaskPatch`].
pub async fn patch_due(
    api: &dyn TasksApi,
    list: &ListId,
    task: &TaskId,
    due: Option<NaiveDate>,
) -> std::result::Result<Task, ApiError> {
    let patch = TaskPatch {
        due: Some(due),
        ..Default::default()
    };
    api.patch_task(list, task, patch).await
}

/// Write-through a due-date change: patch on Google, mirror into the cache.
pub async fn write_due(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    task: &TaskId,
    due: Option<NaiveDate>,
) -> Result<Task> {
    let updated = patch_due(api, list, task, due).await?;
    cache.upsert_task(&updated)?;
    Ok(updated)
}

/// Patch a Task's notes on Google and return the updated Task (no cache write —
/// see [`patch_completed`] for the split rationale). `notes = None` clears them.
pub async fn patch_notes(
    api: &dyn TasksApi,
    list: &ListId,
    task: &TaskId,
    notes: Option<String>,
) -> std::result::Result<Task, ApiError> {
    let patch = TaskPatch {
        notes: Some(notes),
        ..Default::default()
    };
    api.patch_task(list, task, patch).await
}

/// Write-through a notes change: patch on Google, mirror into the cache.
pub async fn write_notes(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    task: &TaskId,
    notes: Option<String>,
) -> Result<Task> {
    let updated = patch_notes(api, list, task, notes).await?;
    cache.upsert_task(&updated)?;
    Ok(updated)
}

/// Insert a List on Google and mirror it into the cache. Returns the server
/// List (with its real id).
pub async fn insert_list(api: &dyn TasksApi, cache: &Cache, title: &str) -> Result<List> {
    let list = api.insert_list(title).await?;
    cache.upsert_list(&list)?;
    Ok(list)
}

/// Patch a List's title on Google and return the updated List (no cache write —
/// see [`patch_completed`] for the split rationale).
pub async fn patch_list_title(
    api: &dyn TasksApi,
    list: &ListId,
    title: &str,
) -> std::result::Result<List, ApiError> {
    api.patch_list(list, title).await
}

/// Write-through a List rename: patch on Google, mirror into the cache.
pub async fn write_list_title(
    api: &dyn TasksApi,
    cache: &Cache,
    list: &ListId,
    title: &str,
) -> Result<List> {
    let updated = patch_list_title(api, list, title).await?;
    cache.upsert_list(&updated)?;
    Ok(updated)
}

/// Delete a List on Google and mirror the removal into the cache.
pub async fn delete_list(api: &dyn TasksApi, cache: &Cache, list: &ListId) -> Result<()> {
    api.delete_list(list).await?;
    cache.delete_list(list)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Status;
    use chrono::{DateTime, NaiveDate};

    fn list(id: &str) -> List {
        List {
            id: ListId(id.into()),
            title: id.into(),
            etag: String::new(),
            updated: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }
    }

    fn dated(id: &str, list: &str, due: Option<NaiveDate>) -> Task {
        Task {
            id: TaskId(id.into()),
            list: ListId(list.into()),
            parent: None,
            title: id.into(),
            notes: None,
            status: Status::NeedsAction,
            due,
            completed_at: None,
            links: Vec::new(),
            position: "1".into(),
            etag: String::new(),
            updated: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
    }

    #[test]
    fn today_from_cache_keeps_due_on_or_before_today_across_lists_and_drops_the_rest() {
        let cache = Cache::open_in_memory().unwrap();
        cache.replace_lists(&[list("a"), list("b")]).unwrap();
        let today = ymd(2026, 7, 20);
        cache
            .replace_tasks(
                &ListId("a".into()),
                &[
                    dated("overdue", "a", Some(ymd(2026, 7, 19))),
                    dated("today", "a", Some(today)),
                    dated("undated", "a", None), // excluded: None is not <= today
                ],
            )
            .unwrap();
        cache
            .replace_tasks(
                &ListId("b".into()),
                &[dated("future", "b", Some(ymd(2026, 7, 21)))], // excluded
            )
            .unwrap();

        let got: Vec<String> = today_from_cache(&cache, today)
            .unwrap()
            .into_iter()
            .map(|t| t.id.0)
            .collect();
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.contains(&"overdue".to_string()));
        assert!(got.contains(&"today".to_string()));
    }
}
