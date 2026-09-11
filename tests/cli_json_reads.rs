//! The `oxidone json` read subcommands, against the in-memory `TasksApi`
//! (ADR-0005) with an injected clock.
//!
//! These pin the **contract** (ADR-0010), not just the behaviour: a caller
//! depends on these field names and this ordering, so a change that breaks one
//! of these tests is a breaking change and should read like one.

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};

use oxidone::api::{FakeTasksApi, NewTask, TaskPatch, TasksApi};
use oxidone::cli::{run_due, run_remote, ErrorKind, Job};
use oxidone::domain::{ListId, TaskId};

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// The reference **instant** every test resolves against. Fixed, so Today
/// membership and relative date phrases are the same in July as in December —
/// and zone-aware, because Today's completion-recency rule compares a UTC
/// `completed_at` against the caller's own day, not UTC's.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, 9, 0, 0).unwrap()
}

/// The reference day, for seeding due dates. Derived from [`now`] so the two
/// cannot drift apart.
fn today() -> NaiveDate {
    now().date_naive()
}

/// Seed a Completed Task completed at a particular instant — or, with `None`, one
/// Google never stamped. Completing alone is not enough: the fake stamps
/// `completed_at` from its own fixed clock, which is never the reference day.
async fn complete_at(api: &FakeTasksApi, list: &ListId, id: &TaskId, at: Option<DateTime<Utc>>) {
    api.patch_task(
        list,
        id,
        TaskPatch {
            completed: Some(true),
            ..TaskPatch::default()
        },
    )
    .await
    .expect("completing a seeded Task");
    api.set_completed_at(id, at);
}

async fn seed_list(api: &FakeTasksApi, title: &str) -> ListId {
    api.insert_list(title).await.expect("seeding a List").id
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

async fn run(api: &FakeTasksApi, job: Job) -> Value {
    run_remote(api, job, now())
        .await
        .expect("a successful read")
}

/// The entries of a read payload, by id — the shape assertions live in their own
/// test, so ordering and membership tests stay about ordering and membership.
fn ids(value: &Value) -> Vec<String> {
    value["entries"]
        .as_array()
        .expect("an entries array")
        .iter()
        .map(|e| e["id"].as_str().expect("an id").to_string())
        .collect()
}

// ---- today ----

#[tokio::test]
async fn today_is_due_on_or_before_today_across_every_list() {
    let api = FakeTasksApi::new();
    let work = seed_list(&api, "Work").await;
    let home = seed_list(&api, "Home").await;

    let overdue = seed(&api, &work, "overdue", Some(ymd(2026, 7, 1))).await;
    let due_today = seed(&api, &home, "due today", Some(today())).await;
    // Neither of these is in Today: tomorrow is not `<= today`, and `None` is
    // not `<= today` either — an undated entry is never in Today.
    seed(&api, &work, "tomorrow", Some(ymd(2026, 7, 21))).await;
    seed(&api, &home, "undated", None).await;

    let payload = run(&api, Job::Today).await;
    assert_eq!(payload["today"], json!("2026-07-20"));
    assert_eq!(ids(&payload), [overdue.0, due_today.0]);
}

#[tokio::test]
async fn today_carries_completed_rows_and_leaves_the_bar_count_to_the_caller() {
    // The plugin's bar count is this set filtered to `needsAction`. Completing an
    // entry must therefore change the count without dropping the row: what got
    // done today is part of the answer to "what was due today".
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    let open = seed(&api, &list, "open", Some(today())).await;
    let done = seed(&api, &list, "done", Some(today())).await;
    complete_at(&api, &list, &done, Some(now())).await;

    let payload = run(&api, Job::Today).await;
    // Both are in the set; only their status differs. (They share a due date, so
    // they are ordered by display title — "done" before "open".)
    assert_eq!(ids(&payload), [done.0.clone(), open.0.clone()]);
    let status_of = |id: &TaskId| {
        payload["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(id.0))
            .expect("a seeded row")["status"]
            .clone()
    };
    assert_eq!(status_of(&open), json!("needsAction"));
    assert_eq!(status_of(&done), json!("completed"));

    let needs_action = payload["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["status"] == json!("needsAction"))
        .count();
    assert_eq!(needs_action, 1, "the bar count is the caller's own filter");
}

#[tokio::test]
async fn today_drops_a_completed_entry_completed_on_an_earlier_day() {
    // #135, the regression this closes: an entry due in the past and completed the
    // day after stayed in `json today` forever, because membership was
    // `due <= today` and nothing else — while the TUI's pane had not shown it since
    // the day it was ticked off. The glossary's rule is the shared one: a Completed
    // row belongs to Today only if it was completed today.
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    let stale = seed(&api, &list, "stale", Some(ymd(2026, 7, 1))).await;
    let fresh = seed(&api, &list, "fresh", Some(ymd(2026, 7, 2))).await;
    let open = seed(&api, &list, "open", Some(ymd(2026, 7, 3))).await;
    complete_at(
        &api,
        &list,
        &stale,
        Some(Utc.with_ymd_and_hms(2026, 7, 2, 8, 0, 0).unwrap()),
    )
    .await;
    complete_at(&api, &list, &fresh, Some(now())).await;

    // The stale one is gone; the one completed today is not. Both are still `due
    // <= today` — it is recency, not membership, that separates them.
    assert_eq!(ids(&run(&api, Job::Today).await), [fresh.0, open.0]);
}

#[tokio::test]
async fn today_keeps_a_completed_entry_with_no_completion_timestamp() {
    // Benefit of the doubt, shared with the TUI: there, completing is optimistic
    // and leaves `completed_at` for the server to fill, so hiding on `None` would
    // blink a row off screen mid-keystroke. A Completed entry Google never stamped
    // takes the same treatment rather than a second rule.
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    let unstamped = seed(&api, &list, "unstamped", Some(today())).await;
    complete_at(&api, &list, &unstamped, None).await;

    let payload = run(&api, Job::Today).await;
    assert_eq!(ids(&payload), [unstamped.0]);
    assert_eq!(payload["entries"][0]["completed_at"], Value::Null);
}

#[tokio::test]
async fn today_resolves_the_completion_day_in_the_callers_timezone() {
    // `completed_at` is UTC; "today" is the user's day. One instant, two zones,
    // two answers — and deliberately an instant whose *date* is the same in both,
    // so this fails if the zone is dropped anywhere between `run_remote` and the
    // predicate, rather than passing on a coincidence of the reference day.
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    let done = seed(&api, &list, "done", Some(ymd(2026, 7, 20))).await;
    complete_at(
        &api,
        &list,
        &done,
        Some(Utc.with_ymd_and_hms(2026, 7, 20, 1, 0, 0).unwrap()),
    )
    .await;

    // 23:00 UTC on the 20th. Two hours west it is 21:00 on the same day, so both
    // callers are asking about 2026-07-20 — but the completion at 01:00 UTC was
    // still the 19th there.
    let evening = Utc.with_ymd_and_hms(2026, 7, 20, 23, 0, 0).unwrap();
    let west = FixedOffset::west_opt(2 * 3600).expect("valid offset");

    let in_utc = run_remote(&api, Job::Today, evening)
        .await
        .expect("a successful read");
    let in_west = run_remote(&api, Job::Today, evening.with_timezone(&west))
        .await
        .expect("a successful read");

    assert_eq!(
        in_utc["today"], in_west["today"],
        "the same day, either way"
    );
    assert_eq!(ids(&in_utc), [done.0]);
    assert!(
        ids(&in_west).is_empty(),
        "completed the previous day in the caller's zone"
    );
}

#[tokio::test]
async fn today_is_ordered_overdue_first_then_by_day_then_deterministically() {
    let api = FakeTasksApi::new();
    let a = seed_list(&api, "A").await;
    let b = seed_list(&api, "B").await;

    // Seeded out of order and across Lists, so response order cannot be what
    // produces the answer.
    let zulu = seed(&api, &b, "zulu", Some(today())).await;
    let old = seed(&api, &a, "old", Some(ymd(2026, 6, 1))).await;
    let alpha = seed(&api, &a, "alpha", Some(today())).await;
    let older = seed(&api, &b, "older", Some(ymd(2026, 5, 1))).await;

    let payload = run(&api, Job::Today).await;
    assert_eq!(ids(&payload), [older.0, old.0, alpha.0, zulu.0]);
}

#[tokio::test]
async fn today_fails_closed_when_a_list_will_not_load() {
    // A short set is indistinguishable from a light day, so one List that will
    // not load fails the call rather than quietly shrinking the answer.
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    seed(&api, &list, "due today", Some(today())).await;
    api.fail_next(oxidone::api::ApiError::RateLimited);

    let error = run_remote(&api, Job::Today, now())
        .await
        .expect_err("a failed read");
    assert_eq!(error.kind, ErrorKind::RateLimited);
}

// ---- lists ----

#[tokio::test]
async fn lists_names_every_list_and_the_concrete_default_id() {
    let api = FakeTasksApi::new();
    let work = seed_list(&api, "Work").await;
    let home = seed_list(&api, "Home").await;
    api.set_default_list(&home);

    let payload = run(&api, Job::Lists).await;
    assert_eq!(
        payload["lists"],
        json!([
            { "id": work.0, "title": "Work" },
            { "id": home.0, "title": "Home" },
        ])
    );
    // The concrete id, never the `@default` alias (ADR-0003) — so a caller can
    // match it against the ids in this very payload.
    assert_eq!(payload["default_list"], json!(home.0));
}

// ---- tasks ----

#[tokio::test]
async fn tasks_are_in_manual_order_with_subtasks_named_by_parent() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    let first = seed(&api, &list, "first", None).await;
    let second = seed(&api, &list, "second", None).await;
    let child = api
        .insert_task(
            &list,
            NewTask {
                title: "child".into(),
                parent: Some(first.clone()),
                ..NewTask::default()
            },
        )
        .await
        .unwrap()
        .id;

    let payload = run(&api, Job::Tasks { list: list.clone() }).await;
    assert_eq!(payload["list"], json!(list.0));

    // Whatever the order, it is `position` order — the same thing the cache's own
    // `ORDER BY position` calls Manual order.
    let positions: Vec<&str> = payload["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["position"].as_str().unwrap())
        .collect();
    let mut sorted = positions.clone();
    sorted.sort_unstable();
    assert_eq!(positions, sorted, "entries must be in position order");

    // A Subtask is identified by `parent`, not by nesting: one level only, so a
    // caller groups by the field without recursing.
    let rows = payload["entries"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let by_id = |id: &TaskId| {
        rows.iter()
            .find(|e| e["id"] == json!(id.0))
            .expect("a seeded row")
            .clone()
    };
    assert_eq!(by_id(&child)["parent"], json!(first.0));
    assert_eq!(by_id(&first)["parent"], Value::Null);
    assert_eq!(by_id(&second)["parent"], Value::Null);
}

#[tokio::test]
async fn an_unknown_list_is_not_found() {
    let api = FakeTasksApi::new();
    let error = run_remote(
        &api,
        Job::Tasks {
            list: ListId("nope".into()),
        },
        now(),
    )
    .await
    .expect_err("a missing List");
    assert_eq!(error.kind, ErrorKind::NotFound);
    assert_eq!(error.kind.exit_code(), 6);
}

// ---- the entry shape ----

#[tokio::test]
async fn an_entry_carries_everything_a_caller_needs_to_render_a_row() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    // A typed entry (ADR-0008): the glyph lives in the title, and the caller must
    // never have to strip it.
    let id = api
        .insert_task(
            &list,
            NewTask {
                title: "○ Standup".into(),
                notes: Some("daily".into()),
                due: Some(today()),
                ..NewTask::default()
            },
        )
        .await
        .unwrap()
        .id;

    let payload = run(&api, Job::Tasks { list: list.clone() }).await;
    let row = &payload["entries"].as_array().unwrap()[0];

    assert_eq!(row["id"], json!(id.0));
    assert_eq!(row["list"], json!(list.0));
    assert_eq!(row["parent"], Value::Null);
    assert_eq!(row["title"], json!("○ Standup"));
    assert_eq!(row["display_title"], json!("Standup"));
    assert_eq!(row["type"], json!("event"));
    assert_eq!(row["has_notes"], json!(true));
    assert_eq!(row["due"], json!("2026-07-20"));
    assert_eq!(row["status"], json!("needsAction"));
    assert_eq!(row["completed_at"], Value::Null);
    assert!(row["position"].is_string());

    // Every documented field, and nothing else: an undocumented field is one a
    // caller will start depending on.
    let mut keys: Vec<&str> = row
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "completed_at",
            "display_title",
            "due",
            "has_notes",
            "id",
            "list",
            "parent",
            "position",
            "status",
            "title",
            "type",
        ]
    );
}

#[tokio::test]
async fn an_untyped_title_is_a_task_and_reads_back_verbatim() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    // A foreign glyph prefix — no space — is *not* a typed entry. It stays a Task
    // whose display title keeps the glyph, until `t` in the TUI normalises it.
    seed(&api, &list, "○Standup", None).await;

    let payload = run(&api, Job::Tasks { list }).await;
    let row = &payload["entries"].as_array().unwrap()[0];
    assert_eq!(row["type"], json!("task"));
    assert_eq!(row["title"], json!("○Standup"));
    assert_eq!(row["display_title"], json!("○Standup"));
}

#[tokio::test]
async fn notes_presence_is_content_not_merely_a_field() {
    let api = FakeTasksApi::new();
    let list = seed_list(&api, "Work").await;
    for (notes, expected) in [
        (None, false),
        (Some(""), false),
        (Some("   "), false),
        (Some("something"), true),
    ] {
        let id = api
            .insert_task(
                &list,
                NewTask {
                    title: format!("{notes:?}"),
                    notes: notes.map(str::to_string),
                    ..NewTask::default()
                },
            )
            .await
            .unwrap()
            .id;
        let payload = run(&api, Job::Tasks { list: list.clone() }).await;
        let row = payload["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["id"] == json!(id.0))
            .expect("the seeded row")
            .clone();
        assert_eq!(row["has_notes"], json!(expected), "{notes:?}");
    }
}

// ---- due ----

#[tokio::test]
async fn due_resolves_the_same_vocabulary_the_d_key_offers() {
    for (expr, expected) in [
        ("today", "2026-07-20"),
        ("tomorrow", "2026-07-21"),
        ("+3d", "2026-07-23"),
        ("2026-12-25", "2026-12-25"),
        // A bare day-of-month rolls forward to its next occurrence.
        ("25", "2026-07-25"),
        ("1", "2026-08-01"),
    ] {
        let payload = run_due(expr, now()).expect(expr);
        assert_eq!(payload["due"], json!(expected), "{expr}");
        // The input is echoed so a caller can show "+3d → Thu 23 Jul" without
        // holding on to what it asked.
        assert_eq!(payload["input"], json!(expr));
    }
}

#[test]
fn a_phrase_that_is_not_a_date_is_refused_rather_than_guessed() {
    // The gate that keeps `interim` from reading `milk` as minutes-times-nothing
    // and stamping today (#107). A CLI caller passes whatever the user typed, so
    // this is the path that matters most.
    for expr in ["milk", "Bob tomorrow", "", "   "] {
        let error = run_due(expr, now()).expect_err(expr);
        assert_eq!(error.kind, ErrorKind::InvalidDue, "{expr}");
        assert_eq!(error.kind.exit_code(), 2);
    }
}
