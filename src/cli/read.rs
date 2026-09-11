//! The read subcommands: `today`, `lists`, `tasks`.
//!
//! All three are network reads that never touch `oxidone.db` — ADR-0010 keeps
//! the cache single-writer (ADR-0001) by keeping this process out of it entirely.

use chrono::NaiveDate;
use serde_json::{json, Value};

use super::wire::{Entry, ListRow};
use super::CliError;
use crate::api::TasksApi;
use crate::domain::{due_on_or_before, ListId, Task};

/// The **Today** set: every entry due on or before `today`, across every List.
///
/// Membership is [`due_on_or_before`] and nothing else, so an undated entry is
/// never in it and the bar can never name a different set than the TUI's. It is
/// **status-blind**: a caller filters to `needsAction` for a count, and gets the
/// Completed rows for free if it wants to show what was done. (The TUI's Today
/// pane additionally hides a Completed row that was not completed today — a
/// display rule of that pane, not a second definition of Today.)
///
/// The fan-out is sequential. It is one request per List, which is what makes it
/// N+1 — acceptable for a caller that polls every few minutes, and the honest
/// trade against threading an `Arc<dyn TasksApi>` through this whole surface to
/// spawn with.
///
/// Fails closed: one List that will not load fails the call rather than returning
/// a short set, which is indistinguishable from a light day.
pub(super) async fn today(api: &dyn TasksApi, today: NaiveDate) -> Result<Value, CliError> {
    let lists = api.list_lists().await?;
    let mut entries: Vec<Task> = Vec::new();
    for list in &lists {
        let tasks = api.list_tasks(&list.id, true, false, None).await?;
        entries.extend(
            tasks
                .into_iter()
                .filter(|task| due_on_or_before(task.due, today)),
        );
    }
    // A specified order, because an unspecified one in a contract is a promise
    // nobody can rely on and everybody will. Overdue first, then the day, then
    // something total so two entries on one day never swap between polls.
    entries.sort_by(|a, b| {
        (a.due, a.display_title(), &a.id.0).cmp(&(b.due, b.display_title(), &b.id.0))
    });
    let entries: Vec<Entry> = entries.iter().map(Entry::new).collect();
    Ok(json!({ "today": today, "entries": entries }))
}

/// The Lists, and which one is the default.
///
/// `default_list` is the concrete id `@default` resolves to, never the alias
/// (ADR-0003) — so a caller can compare it against the ids in the same payload.
pub(super) async fn lists(api: &dyn TasksApi) -> Result<Value, CliError> {
    let lists = api.list_lists().await?;
    let default = api.default_list().await?;
    let rows: Vec<ListRow> = lists.iter().map(ListRow::new).collect();
    Ok(json!({ "lists": rows, "default_list": default.id.0 }))
}

/// One List's entries in **Manual order**.
///
/// Sorted by `position`, which is literally what the cache's own
/// `ORDER BY position` calls Manual order — asserted here rather than inherited
/// from Google's response order, because the order is part of the contract.
///
/// Subtasks are identified by `parent` and not nested: nesting is capped at one
/// level, so a caller groups by that field without recursing, and the payload
/// stays a flat array it can diff against its own copy.
pub(super) async fn tasks(api: &dyn TasksApi, list: &ListId) -> Result<Value, CliError> {
    let mut tasks = api.list_tasks(list, true, false, None).await?;
    tasks.sort_by(|a, b| a.position.cmp(&b.position));
    let entries: Vec<Entry> = tasks.iter().map(Entry::new).collect();
    Ok(json!({ "list": list.0, "entries": entries }))
}
