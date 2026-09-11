//! The read subcommands: `today`, `lists`, `tasks`.
//!
//! All three are network reads that never touch `oxidone.db` — ADR-0010 keeps
//! the cache single-writer (ADR-0001) by keeping this process out of it entirely.

use chrono::{DateTime, TimeZone};
use serde_json::{json, Value};

use super::wire::{Entry, ListRow};
use super::CliError;
use crate::api::TasksApi;
use crate::domain::{due_on_or_before, within_completion_day, ListId, Task};

/// The **Today** set: every entry due on or before `today`, across every List,
/// minus the completions that belong to an earlier day.
///
/// Both halves of the rule come from `domain`, so this names exactly the set the
/// TUI's Today pane draws (#135): [`due_on_or_before`] decides membership — an
/// undated entry is never in it — and [`within_completion_day`] narrows the
/// Completed rows to those completed *today*, so the set answers "among what was
/// due, what got done" rather than accumulating every completion whose due date
/// is in the past.
///
/// Still carries its Completed rows, so a caller that wants a count filters to
/// `needsAction` itself — that count is unaffected by the recency rule, since
/// nothing it removes was `needsAction` to begin with.
///
/// Takes the caller's *instant*, not a date: `completed_at` is UTC while "today" is
/// the user's day, so the comparison needs a zone — and deriving both the day and
/// the zone from one `now` is what stops them being passed in disagreeing.
///
/// The fan-out is sequential. It is one request per List, which is what makes it
/// N+1 — acceptable for a caller that polls every few minutes, and the honest
/// trade against threading an `Arc<dyn TasksApi>` through this whole surface to
/// spawn with.
///
/// Fails closed: one List that will not load fails the call rather than returning
/// a short set, which is indistinguishable from a light day.
pub(super) async fn today<Tz: TimeZone>(
    api: &dyn TasksApi,
    now: &DateTime<Tz>,
) -> Result<Value, CliError> {
    let today = now.date_naive();
    let tz = now.timezone();
    let lists = api.list_lists().await?;
    let mut entries: Vec<Task> = Vec::new();
    for list in &lists {
        let tasks = api.list_tasks(&list.id, true, false, None).await?;
        entries.extend(tasks.into_iter().filter(|task| {
            due_on_or_before(task.due, today) && within_completion_day(task, today, &tz)
        }));
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
