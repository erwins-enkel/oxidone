//! Cache round-trip for Google's output-only `links[]` (#55, migration 0004).
//! The JSON `links` column mirrors `{url, description, kind}` and survives both
//! write paths (`replace_tasks`, `upsert_task`) back through `tasks()`.

use chrono::{DateTime, TimeZone, Utc};
use oxidone::cache::Cache;
use oxidone::domain::{ListId, Status, Task, TaskId, TaskLink};

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
}

fn task(id: &str, links: Vec<TaskLink>) -> Task {
    Task {
        id: TaskId(id.to_string()),
        list: ListId("L".to_string()),
        parent: None,
        title: format!("task {id}"),
        notes: None,
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        links,
        position: format!("{id:0>20}"),
        etag: "etag".to_string(),
        updated: ts(0),
    }
}

fn link(url: &str, description: Option<&str>, kind: Option<&str>) -> TaskLink {
    TaskLink {
        url: url.to_string(),
        description: description.map(str::to_string),
        kind: kind.map(str::to_string),
    }
}

#[test]
fn replace_tasks_round_trips_links_including_kind() {
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    let links = vec![
        link(
            "https://mail.google.com/x",
            Some("Re: subject"),
            Some("email"),
        ),
        // A kind oxidone has never seen must survive verbatim, not break parsing.
        link("https://keep.google.com/y", None, Some("some_future_kind")),
    ];

    cache
        .replace_tasks(&list, &[task("a", links.clone())])
        .unwrap();

    let got = cache.tasks(&list).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].links, links);
}

#[test]
fn upsert_task_round_trips_links() {
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());
    let links = vec![link("https://a.dev/1", Some("ticket"), Some("generic"))];

    cache.upsert_task(&task("a", links.clone())).unwrap();

    let got = cache.tasks(&list).unwrap();
    assert_eq!(got[0].links, links);
}

#[test]
fn a_task_with_no_links_reads_back_empty() {
    let cache = Cache::open_in_memory().unwrap();
    let list = ListId("L".to_string());

    cache
        .replace_tasks(&list, &[task("a", Vec::new())])
        .unwrap();

    let got = cache.tasks(&list).unwrap();
    assert!(got[0].links.is_empty());
}
