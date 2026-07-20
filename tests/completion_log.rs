//! Cache tests for the completion log (ticket #13, ADR-0007): completions are
//! observed at the mirror seam (`upsert_task` / `replace_tasks`), idempotently,
//! and survive the Task being cleared away from the mirror.

use chrono::{DateTime, TimeZone, Utc};
use oxidone::cache::Cache;
use oxidone::domain::{ListId, Status, Task, TaskId};

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
}

fn task(id: &str, status: Status, completed_at: Option<DateTime<Utc>>) -> Task {
    Task {
        id: TaskId(id.to_string()),
        list: ListId("L".to_string()),
        parent: None,
        title: format!("task {id}"),
        notes: None,
        status,
        due: None,
        completed_at,
        position: "00000000000000000001".to_string(),
        etag: "etag".to_string(),
        updated: ts(0),
    }
}

#[test]
fn upsert_task_logs_a_completion() {
    let cache = Cache::open_in_memory().unwrap();
    cache
        .upsert_task(&task("a", Status::Completed, Some(ts(10))))
        .unwrap();

    let log = cache.completions().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].task, TaskId("a".to_string()));
    assert_eq!(log[0].completed_at, ts(10));
}

#[test]
fn an_open_task_is_not_logged() {
    let cache = Cache::open_in_memory().unwrap();
    cache
        .upsert_task(&task("a", Status::NeedsAction, None))
        .unwrap();
    assert!(cache.completions().unwrap().is_empty());
}

#[test]
fn observing_the_same_completion_twice_is_idempotent() {
    let cache = Cache::open_in_memory().unwrap();
    let t = task("a", Status::Completed, Some(ts(10)));
    cache.upsert_task(&t).unwrap();
    cache.upsert_task(&t).unwrap(); // e.g. a later refresh re-observes it
    assert_eq!(cache.completions().unwrap().len(), 1);
}

#[test]
fn re_completing_after_reopening_is_a_new_event() {
    let cache = Cache::open_in_memory().unwrap();
    cache
        .upsert_task(&task("a", Status::Completed, Some(ts(10))))
        .unwrap();
    // Reopened then completed again at a different time => a distinct event.
    cache
        .upsert_task(&task("a", Status::Completed, Some(ts(50))))
        .unwrap();
    let log = cache.completions().unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].completed_at, ts(10)); // ordered oldest-first
    assert_eq!(log[1].completed_at, ts(50));
}

#[test]
fn replace_tasks_logs_only_completed_tasks() {
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    cache
        .replace_tasks(
            &list,
            &[
                task("open", Status::NeedsAction, None),
                task("done", Status::Completed, Some(ts(20))),
            ],
        )
        .unwrap();
    let log = cache.completions().unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].task, TaskId("done".to_string()));
}

#[test]
fn a_cleared_completion_survives_in_the_log() {
    // The mirror drops a Task once Google clears it, but the log keeps the
    // history — the whole point of ADR-0007.
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    cache
        .replace_tasks(&list, &[task("done", Status::Completed, Some(ts(20)))])
        .unwrap();
    // A later refresh no longer sees the (now-cleared) Task.
    cache.replace_tasks(&list, &[]).unwrap();

    assert!(cache.tasks(&list).unwrap().is_empty()); // gone from the mirror
    assert_eq!(cache.completions().unwrap().len(), 1); // kept in the log
}

#[test]
fn the_log_records_the_display_title_while_the_mirror_keeps_the_raw_one() {
    // The mirror is a mirror (ADR-0003) and the log is human-readable history
    // (ADR-0007), so a typed entry stores its glyph in one and not the other.
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    let mut event = task("1", Status::Completed, Some(ts(10)));
    event.title = "○ Standup".to_string();

    cache
        .replace_tasks(&list, std::slice::from_ref(&event))
        .unwrap();

    assert_eq!(cache.tasks(&list).unwrap()[0].title, "○ Standup");
    let logged = cache.completions().unwrap();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].title, "Standup");
}

#[test]
fn a_later_retype_does_not_rewrite_an_already_logged_completion() {
    // `INSERT OR IGNORE` on (task_id, completed_at): first observation wins, so
    // renaming or retyping afterwards never reaches the logged row.
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    let mut t = task("1", Status::Completed, Some(ts(10)));
    t.title = "○ Standup".to_string();
    cache
        .replace_tasks(&list, std::slice::from_ref(&t))
        .unwrap();

    t.title = "— Standup".to_string();
    cache
        .replace_tasks(&list, std::slice::from_ref(&t))
        .unwrap();

    let logged = cache.completions().unwrap();
    assert_eq!(logged.len(), 1, "one event, not two");
    assert_eq!(logged[0].title, "Standup"); // unchanged by the retype
}
