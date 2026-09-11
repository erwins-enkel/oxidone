//! `oxidone json apply` — one write, read as JSON from **stdin**.
//!
//! Stdin and not flags, because argv is not private: `/proc/<pid>/cmdline` is
//! readable by every process running as the same user, so a `--title` flag would
//! publish task titles and notes to anything else on the machine (ADR-0010).
//!
//! One command per invocation. A batch is cheap to add later — accept an array —
//! and impossible to remove, and a one-shot caller has no use for the
//! partial-failure story it would need.

use chrono::NaiveDate;
use serde::Deserialize;
use serde_json::{json, Value};

use super::wire::Entry;
use super::{CliError, ErrorKind};
use crate::api::{NewTask, TaskPatch, TasksApi};
use crate::domain::{migrated_due, ListId, Status, Task, TaskId};

/// The eight operations the bar plugin's v1 needs.
///
/// `deny_unknown_fields` is not decoration: a field name the caller got wrong
/// would otherwise be dropped in silence, and "the write succeeded, differently
/// from what you asked" is exactly the swallowed failure this codebase refuses.
///
/// `set_due` and `clear_due` are two ops rather than one with a nullable `due`
/// for the same reason — serde reads a *missing* `Option` field as `None`, so a
/// single op would turn a typo into a silent due-date wipe.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum ApplyCommand {
    Complete {
        list: String,
        task: String,
    },
    Uncomplete {
        list: String,
        task: String,
    },
    /// Create an entry from a title alone. The title is written **verbatim**, so
    /// its **Entry type** is whatever `EntryType::parse` makes of it — the same
    /// thing that happens to a title typed into Google's own web client. The
    /// answer carries the resulting `type`, so nothing is left to guess.
    Create {
        list: String,
        title: String,
    },
    /// Retitle, preserving the entry's type.
    ///
    /// `title` is a **Display title** — no glyph. The current type is read first
    /// and re-applied, which is precisely what the TUI's `e` does, so the caller
    /// never has to know that the type lives in the title (ADR-0008).
    Retitle {
        list: String,
        task: String,
        title: String,
    },
    /// Set the due date. ISO `YYYY-MM-DD` only — resolve a phrase with
    /// `oxidone json due` first, so the user sees the date before it is written.
    SetDue {
        list: String,
        task: String,
        due: NaiveDate,
    },
    ClearDue {
        list: String,
        task: String,
    },
    Delete {
        list: String,
        task: String,
    },
    /// **Migrate**: defer to [`migrated_due`]. Refused on a Completed entry,
    /// exactly as the `m` key behaves — re-dating a finished entry means nothing.
    Migrate {
        list: String,
        task: String,
    },
}

/// Read a command off stdin.
///
/// Every malformed input is [`ErrorKind::Usage`] — an unknown `op`, a field that
/// is not one, a missing field, a `due` that is not a date, or bytes that are not
/// JSON. They are all the same thing to a caller: the request was not one.
pub(super) fn parse(stdin: &str) -> Result<ApplyCommand, CliError> {
    if stdin.trim().is_empty() {
        return Err(CliError::usage(
            "oxidone json apply: expected a JSON command on stdin",
        ));
    }
    serde_json::from_str(stdin).map_err(|e| CliError::usage(format!("oxidone json apply: {e}")))
}

/// Run one already-parsed command.
pub(super) async fn run(
    api: &dyn TasksApi,
    command: ApplyCommand,
    today: NaiveDate,
) -> Result<Value, CliError> {
    match command {
        ApplyCommand::Complete { list, task } => {
            let (list, task) = ids(&list, &task)?;
            patch(api, &list, &task, completion(true)).await
        }
        ApplyCommand::Uncomplete { list, task } => {
            let (list, task) = ids(&list, &task)?;
            patch(api, &list, &task, completion(false)).await
        }
        ApplyCommand::Create { list, title } => {
            let list = list_id(&list)?;
            let title = title_text(&title)?;
            let created = api
                .insert_task(
                    &list,
                    NewTask {
                        title,
                        ..NewTask::default()
                    },
                )
                .await?;
            Ok(entry(&created))
        }
        ApplyCommand::Retitle { list, task, title } => {
            let (list, task) = ids(&list, &task)?;
            let display = title_text(&title)?;
            let current = api.get_task(&list, &task).await?;
            // `apply`, never `retype`: this is the ordinary edit path, and an
            // edit must not quietly repair a foreign prefix the user did not
            // mention. `t` in the TUI is what normalises those.
            let title = current.entry_type().apply(&display);
            patch(
                api,
                &list,
                &task,
                TaskPatch {
                    title: Some(title),
                    ..TaskPatch::default()
                },
            )
            .await
        }
        ApplyCommand::SetDue { list, task, due } => {
            let (list, task) = ids(&list, &task)?;
            patch(api, &list, &task, due_patch(Some(due))).await
        }
        ApplyCommand::ClearDue { list, task } => {
            let (list, task) = ids(&list, &task)?;
            patch(api, &list, &task, due_patch(None)).await
        }
        ApplyCommand::Delete { list, task } => {
            let (list, task) = ids(&list, &task)?;
            api.delete_task(&list, &task).await?;
            // No entry to echo: it is gone. Naming what went keeps the answer
            // useful to a caller reconciling its own copy.
            Ok(json!({ "deleted": { "list": list.0, "id": task.0 } }))
        }
        ApplyCommand::Migrate { list, task } => {
            let (list, task) = ids(&list, &task)?;
            let current = api.get_task(&list, &task).await?;
            let due = migrate_target(&current, today)?;
            patch(api, &list, &task, due_patch(Some(due))).await
        }
    }
}

/// The date a Migrate writes, or the refusal that stops it.
///
/// Both refusals are [`ErrorKind::Refused`] — oxidone declining, as against
/// Google declining — and both are the TUI's own rules, reached through the same
/// [`migrated_due`].
fn migrate_target(task: &Task, today: NaiveDate) -> Result<NaiveDate, CliError> {
    if task.status == Status::Completed {
        return Err(CliError::new(
            ErrorKind::Refused,
            "completed entries are not migrated",
        ));
    }
    migrated_due(task.due, today).ok_or_else(|| {
        CliError::new(
            ErrorKind::Refused,
            "there is no day after this one to migrate to",
        )
    })
}

/// Patch a Task and answer with what came back — the server's own post-write
/// state, so a caller updates its copy without a second read.
async fn patch(
    api: &dyn TasksApi,
    list: &ListId,
    task: &TaskId,
    patch: TaskPatch,
) -> Result<Value, CliError> {
    let updated = api.patch_task(list, task, patch).await?;
    Ok(entry(&updated))
}

fn entry(task: &Task) -> Value {
    json!({ "entry": Entry::new(task) })
}

fn completion(completed: bool) -> TaskPatch {
    TaskPatch {
        completed: Some(completed),
        ..TaskPatch::default()
    }
}

/// `Some(date)` sets, `None` clears. The double `Option` is `TaskPatch`'s own
/// "leave it alone" / "write this" distinction; the outer one is always `Some`
/// here because both ops do write the field.
fn due_patch(due: Option<NaiveDate>) -> TaskPatch {
    TaskPatch {
        due: Some(due),
        ..TaskPatch::default()
    }
}

fn ids(list: &str, task: &str) -> Result<(ListId, TaskId), CliError> {
    let list = list_id(list)?;
    if task.is_empty() {
        return Err(CliError::usage(
            "oxidone json apply: `task` must not be empty",
        ));
    }
    Ok((list, TaskId(task.to_string())))
}

fn list_id(list: &str) -> Result<ListId, CliError> {
    if list.is_empty() {
        return Err(CliError::usage(
            "oxidone json apply: `list` must not be empty",
        ));
    }
    Ok(ListId(list.to_string()))
}

/// A title that is only whitespace is not a title. Refused here rather than sent,
/// because Google accepts it and the entry becomes unnameable — and the TUI's own
/// edit path refuses the same thing.
fn title_text(title: &str) -> Result<String, CliError> {
    if title.trim().is_empty() {
        return Err(CliError::usage(
            "oxidone json apply: `title` must not be empty",
        ));
    }
    Ok(title.to_string())
}
