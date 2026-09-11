//! `oxidone json apply` — the eight write operations (ADR-0010), against the
//! in-memory `TasksApi` with an injected clock.
//!
//! The command arrives as a `&str` exactly as it would off stdin, so the parsing
//! and the write are covered together without spawning a process.

use chrono::NaiveDate;
use serde_json::{json, Value};

use oxidone::api::{FakeTasksApi, NewTask, TaskPatch, TasksApi};
use oxidone::cli::{resolve, run_remote, CliError, ErrorKind, Remote};
use oxidone::domain::{ListId, TaskId};

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn today() -> NaiveDate {
    ymd(2026, 7, 20)
}

async fn seed_list(api: &FakeTasksApi) -> ListId {
    api.insert_list("Work").await.expect("seeding a List").id
}

async fn seed(api: &FakeTasksApi, list: &ListId, title: &str, due: Option<NaiveDate>) -> TaskId {
    api.insert_task(
        list,
        NewTask {
            title: title.to_string(),
            due,
            ..NewTask::default()
        },
    )
    .await
    .expect("seeding a Task")
    .id
}

/// Drive one command exactly as stdin would deliver it: resolve it (which is
/// where parsing happens) and then run it.
async fn apply(api: &FakeTasksApi, command: Value) -> Result<Value, CliError> {
    run(api, &command.to_string()).await
}

async fn run(api: &FakeTasksApi, stdin: &str) -> Result<Value, CliError> {
    let job = resolve(Remote::Apply, stdin)?;
    run_remote(api, job, today()).await
}

async fn ok(api: &FakeTasksApi, command: Value) -> Value {
    apply(api, command).await.expect("a successful write")
}

async fn refused(api: &FakeTasksApi, command: Value) -> CliError {
    apply(api, command).await.expect_err("a refusal")
}

// ---- completion ----

#[tokio::test]
async fn complete_and_uncomplete_round_trip_and_echo_the_new_state() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    let done = ok(
        &api,
        json!({"op": "complete", "list": list.0, "task": task.0}),
    )
    .await;
    assert_eq!(done["entry"]["status"], json!("completed"));
    // The server's own post-write state, so a caller updates its copy without a
    // second read.
    assert!(done["entry"]["completed_at"].is_string());

    let reopened = ok(
        &api,
        json!({"op": "uncomplete", "list": list.0, "task": task.0}),
    )
    .await;
    assert_eq!(reopened["entry"]["status"], json!("needsAction"));
    assert_eq!(reopened["entry"]["completed_at"], Value::Null);
}

// ---- create ----

#[tokio::test]
async fn create_takes_a_title_alone_and_reports_the_type_it_landed_as() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;

    let plain = ok(
        &api,
        json!({"op": "create", "list": list.0, "title": "Buy milk"}),
    )
    .await;
    assert_eq!(plain["entry"]["title"], json!("Buy milk"));
    assert_eq!(plain["entry"]["type"], json!("task"));
    assert_eq!(plain["entry"]["due"], Value::Null);

    // The title is written verbatim, so a caller that *does* write a glyph gets
    // the type that glyph means — the same thing Google's own web client does.
    // The answer names it, so nothing is left to guess.
    let event = ok(
        &api,
        json!({"op": "create", "list": list.0, "title": "○ Standup"}),
    )
    .await;
    assert_eq!(event["entry"]["type"], json!("event"));
    assert_eq!(event["entry"]["display_title"], json!("Standup"));
}

// ---- retitle ----

#[tokio::test]
async fn retitle_takes_a_display_title_and_preserves_the_entry_type() {
    // The caller strips no glyphs and writes none (ADR-0008): it sends what the
    // user typed, and the type the entry already had survives.
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "○ Standup", None).await;

    let renamed = ok(
        &api,
        json!({"op": "retitle", "list": list.0, "task": task.0, "title": "Daily sync"}),
    )
    .await;
    assert_eq!(renamed["entry"]["title"], json!("○ Daily sync"));
    assert_eq!(renamed["entry"]["display_title"], json!("Daily sync"));
    assert_eq!(renamed["entry"]["type"], json!("event"));
}

#[tokio::test]
async fn retitling_an_untyped_entry_writes_exactly_what_was_sent() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    // A foreign prefix: parses as an untyped Task. A retitle must not quietly
    // repair it — that is `t`'s job in the TUI, not an edit's.
    let task = seed(&api, &list, "○Standup", None).await;

    let renamed = ok(
        &api,
        json!({"op": "retitle", "list": list.0, "task": task.0, "title": "○Daily"}),
    )
    .await;
    assert_eq!(renamed["entry"]["title"], json!("○Daily"));
    assert_eq!(renamed["entry"]["type"], json!("task"));
}

// ---- due ----

#[tokio::test]
async fn set_due_takes_iso_only_and_clear_due_removes_it() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    let dated = ok(
        &api,
        json!({"op": "set_due", "list": list.0, "task": task.0, "due": "2026-12-25"}),
    )
    .await;
    assert_eq!(dated["entry"]["due"], json!("2026-12-25"));

    let cleared = ok(
        &api,
        json!({"op": "clear_due", "list": list.0, "task": task.0}),
    )
    .await;
    assert_eq!(cleared["entry"]["due"], Value::Null);
}

#[tokio::test]
async fn set_due_refuses_a_phrase_rather_than_resolving_one() {
    // `apply`'s `due` is one unambiguous type. A phrase is resolved by
    // `oxidone json due`, so the user sees the date before it is written.
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    let error = refused(
        &api,
        json!({"op": "set_due", "list": list.0, "task": task.0, "due": "tomorrow"}),
    )
    .await;
    assert_eq!(error.kind, ErrorKind::Usage);
}

// ---- delete ----

#[tokio::test]
async fn delete_names_what_went_since_there_is_no_entry_to_echo() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    let deleted = ok(
        &api,
        json!({"op": "delete", "list": list.0, "task": task.0}),
    )
    .await;
    assert_eq!(deleted, json!({"deleted": {"list": list.0, "id": task.0}}));
    assert!(api
        .list_tasks(&list, true, false, None)
        .await
        .unwrap()
        .is_empty());
}

// ---- migrate ----

#[tokio::test]
async fn migrate_defers_to_one_day_past_the_later_of_today_and_the_due_date() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;

    for (seeded, expected) in [
        // Overdue lands on tomorrow, not on the day after the date it missed.
        (Some(ymd(2026, 7, 1)), "2026-07-21"),
        (Some(today()), "2026-07-21"),
        // A future date shifts by a day from itself.
        (Some(ymd(2026, 8, 1)), "2026-08-02"),
        // Undated gets tomorrow — `max` has nothing to compare against.
        (None, "2026-07-21"),
    ] {
        let task = seed(&api, &list, &format!("{seeded:?}"), seeded).await;
        let migrated = ok(
            &api,
            json!({"op": "migrate", "list": list.0, "task": task.0}),
        )
        .await;
        assert_eq!(migrated["entry"]["due"], json!(expected), "{seeded:?}");
        // Not an exit: the entry stays `needsAction` and only its date moves.
        assert_eq!(migrated["entry"]["status"], json!("needsAction"));
    }
}

#[tokio::test]
async fn repeated_migrations_compose_a_day_at_a_time() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", Some(ymd(2026, 7, 1))).await;

    for expected in ["2026-07-21", "2026-07-22", "2026-07-23"] {
        let migrated = ok(
            &api,
            json!({"op": "migrate", "list": list.0, "task": task.0}),
        )
        .await;
        assert_eq!(migrated["entry"]["due"], json!(expected));
    }
}

#[tokio::test]
async fn migrate_is_refused_on_a_completed_entry_exactly_as_the_m_key_is() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", Some(today())).await;
    api.patch_task(
        &list,
        &task,
        TaskPatch {
            completed: Some(true),
            ..TaskPatch::default()
        },
    )
    .await
    .unwrap();

    let error = refused(
        &api,
        json!({"op": "migrate", "list": list.0, "task": task.0}),
    )
    .await;
    // oxidone declining, not Google: its own exit code, so a caller can tell a
    // rule from a rejection.
    assert_eq!(error.kind, ErrorKind::Refused);
    assert_eq!(error.kind.exit_code(), 7);
    assert!(error.message.contains("completed"));

    // Refused means nothing was written.
    let unchanged = api.get_task(&list, &task).await.unwrap();
    assert_eq!(unchanged.due, Some(today()));
}

#[tokio::test]
async fn migrate_declines_at_the_end_of_the_calendar_rather_than_panicking() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", Some(NaiveDate::MAX)).await;

    let error = refused(
        &api,
        json!({"op": "migrate", "list": list.0, "task": task.0}),
    )
    .await;
    assert_eq!(error.kind, ErrorKind::Refused);
    assert!(error.message.contains("no day after"));
}

// ---- the input contract ----

#[tokio::test]
async fn an_unknown_field_is_refused_rather_than_dropped() {
    // The guarantee `deny_unknown_fields` buys: a caller that got a field name
    // wrong is told, instead of getting "the write succeeded, differently".
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    let error = refused(
        &api,
        json!({"op": "complete", "list": list.0, "task": task.0, "titel": "typo"}),
    )
    .await;
    assert_eq!(error.kind, ErrorKind::Usage);
    assert_eq!(error.kind.exit_code(), 2);
}

#[tokio::test]
async fn a_malformed_command_is_a_usage_failure_not_a_write() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let task = seed(&api, &list, "Buy milk", None).await;

    for command in [
        // Not JSON at all.
        "{".to_string(),
        // Nothing on stdin.
        String::new(),
        "   \n".to_string(),
        // An op that does not exist.
        json!({"op": "reschedule", "list": list.0, "task": task.0}).to_string(),
        // A required field missing.
        json!({"op": "complete", "list": list.0}).to_string(),
        json!({"op": "retitle", "list": list.0, "task": task.0}).to_string(),
        // Empty ids, which Google would answer with a confusing 400.
        json!({"op": "complete", "list": "", "task": task.0}).to_string(),
        json!({"op": "complete", "list": list.0, "task": ""}).to_string(),
        // A title that is only whitespace is not a title.
        json!({"op": "create", "list": list.0, "title": "  "}).to_string(),
        // A batch: one command per invocation, and an array is not one.
        json!([{"op": "complete", "list": list.0, "task": task.0}]).to_string(),
    ] {
        let outcome = run(&api, &command).await;
        let error = match outcome {
            Err(error) => error,
            Ok(value) => panic!("{command} should not have written anything, got {value}"),
        };
        assert_eq!(error.kind, ErrorKind::Usage, "{command}");
        assert_eq!(error.kind.exit_code(), 2, "{command}");
    }

    // Nothing above reached Google: the entry is untouched and still open.
    let untouched = api.get_task(&list, &task).await.unwrap();
    assert_eq!(untouched.title, "Buy milk");
    assert_eq!(untouched.status, oxidone::domain::Status::NeedsAction);
    assert_eq!(
        api.list_tasks(&list, true, false, None)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn an_unknown_task_is_not_found() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api).await;
    let error = refused(
        &api,
        json!({"op": "migrate", "list": list.0, "task": "nope"}),
    )
    .await;
    assert_eq!(error.kind, ErrorKind::NotFound);
    assert_eq!(error.kind.exit_code(), 6);
}
