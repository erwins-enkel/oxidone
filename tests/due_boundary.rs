//! Boundary tests for due dates (ticket #9): `sync::write_due` over a seeded
//! `FakeTasksApi` + in-memory cache asserts the cache is patched — for both a
//! set and a clear (`Some(None)`).

use chrono::NaiveDate;
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::cache::Cache;
use oxidone::sync;

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

async fn seed(
    initial_due: Option<NaiveDate>,
) -> (
    FakeTasksApi,
    Cache,
    oxidone::domain::List,
    oxidone::domain::Task,
) {
    let api = FakeTasksApi::new();
    let list = api.insert_list("Work").await.unwrap();
    let task = api
        .insert_task(
            &list.id,
            NewTask {
                title: "task".to_string(),
                due: initial_due,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache
        .replace_tasks(&list.id, std::slice::from_ref(&task))
        .unwrap();
    (api, cache, list, task)
}

#[tokio::test]
async fn write_due_sets_the_date_in_google_and_cache() {
    let (api, cache, list, task) = seed(None).await;
    let updated = sync::write_due(&api, &cache, &list.id, &task.id, Some(ymd(2026, 8, 1)))
        .await
        .unwrap();
    assert_eq!(updated.due, Some(ymd(2026, 8, 1)));
    assert_eq!(cache.tasks(&list.id).unwrap()[0].due, Some(ymd(2026, 8, 1)));
}

#[tokio::test]
async fn write_due_clears_the_date_in_google_and_cache() {
    let (api, cache, list, task) = seed(Some(ymd(2026, 8, 1))).await;
    let updated = sync::write_due(&api, &cache, &list.id, &task.id, None)
        .await
        .unwrap();
    assert_eq!(updated.due, None);
    assert_eq!(cache.tasks(&list.id).unwrap()[0].due, None);
}
