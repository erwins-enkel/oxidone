//! Boundary tests for `sync::mirror_and_aggregate` — the spawn-free core the
//! Today and Search fan-out workers share (`main.rs` keeps only the `JoinSet`
//! glue). Runs against a real in-memory `Cache`; the fan-out results are handed in
//! as already-awaited `(ListId, Result<Vec<Task>>)` pairs, exactly as the worker
//! collects them, so failure attribution and the aggregate read are covered here
//! rather than in the untestable `main.rs` spawn.

use anyhow::anyhow;
use chrono::{NaiveDate, TimeZone, Utc};
use oxidone::cache::Cache;
use oxidone::domain::{ListId, Status, Task, TaskId};
use oxidone::sync::{self, Aggregate};

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

const TODAY: (i32, u32, u32) = (2026, 7, 20);

fn today() -> NaiveDate {
    ymd(TODAY.0, TODAY.1, TODAY.2)
}

fn task(id: &str, list: &str, due: Option<NaiveDate>) -> Task {
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
        position: id.into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn ids(mut tasks: Vec<Task>) -> Vec<String> {
    tasks.sort_by(|a, b| a.id.0.cmp(&b.id.0));
    tasks.into_iter().map(|t| t.id.0).collect()
}

#[test]
fn mirrors_every_list_and_returns_the_whole_corpus() {
    let cache = Cache::open_in_memory().unwrap();
    let fetched = vec![
        (
            ListId("work".into()),
            Ok(vec![
                task("a", "work", None),
                task("b", "work", Some(today())),
            ]),
        ),
        (
            ListId("home".into()),
            Ok(vec![task("c", "home", Some(ymd(2026, 12, 1)))]),
        ),
    ];

    let (corpus, failed) = sync::mirror_and_aggregate(&cache, fetched, Aggregate::All).unwrap();

    assert!(failed.is_empty(), "no failures");
    assert_eq!(
        ids(corpus),
        vec!["a", "b", "c"],
        "the whole corpus, unfiltered"
    );
    // Mirrored, so a later cache read sees the same rows.
    assert_eq!(cache.all_tasks().unwrap().len(), 3);
}

#[test]
fn a_failed_fetch_is_named_and_the_others_still_mirror() {
    let cache = Cache::open_in_memory().unwrap();
    let fetched = vec![
        (ListId("work".into()), Ok(vec![task("a", "work", None)])),
        (ListId("home".into()), Err(anyhow!("home fetch exploded"))),
    ];

    let (corpus, failed) = sync::mirror_and_aggregate(&cache, fetched, Aggregate::All).unwrap();

    assert_eq!(
        failed,
        vec![ListId("home".into())],
        "the failed List is named, not dropped"
    );
    assert_eq!(
        ids(corpus),
        vec!["a"],
        "the surviving List still contributes"
    );
}

#[test]
fn the_today_aggregate_filters_the_same_input_to_due_on_or_before_today() {
    // The generalisation must not change Today's behaviour: the same fetched set,
    // read back under `Aggregate::Today`, keeps only `due <= today`.
    let cache = Cache::open_in_memory().unwrap();
    let fetched = vec![(
        ListId("work".into()),
        Ok(vec![
            task("overdue", "work", Some(ymd(2026, 7, 19))),
            task("due-today", "work", Some(today())),
            task("undated", "work", None),
            task("future", "work", Some(ymd(2026, 12, 1))),
        ]),
    )];

    let (aggregate, failed) =
        sync::mirror_and_aggregate(&cache, fetched, Aggregate::Today(today())).unwrap();

    assert!(failed.is_empty());
    assert_eq!(
        ids(aggregate),
        vec!["due-today", "overdue"],
        "undated and future are excluded from Today, unlike the corpus"
    );
    // The cache still holds every mirrored row, aggregate or not.
    assert_eq!(cache.all_tasks().unwrap().len(), 4);
}
