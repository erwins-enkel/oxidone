//! Boundary tests for the notes write path (ticket #10): `sync::write_notes`
//! over a seeded `FakeTasksApi` + in-memory cache — set, clear, and error.

use oxidone::api::{ApiError, FakeTasksApi, NewTask, TasksApi};
use oxidone::cache::Cache;
use oxidone::sync;

async fn seed() -> (
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
                title: "do it".to_string(),
                notes: Some("first".to_string()),
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
async fn write_notes_sets_notes_and_patches_cache() {
    let (api, cache, list, task) = seed().await;

    let updated = sync::write_notes(&api, &cache, &list.id, &task.id, Some("second".to_string()))
        .await
        .unwrap();
    assert_eq!(updated.notes.as_deref(), Some("second"));

    // The cache reflects the write (patched from the response, not re-fetched).
    let cached = cache.tasks(&list.id).unwrap();
    assert_eq!(cached[0].notes.as_deref(), Some("second"));
}

#[tokio::test]
async fn write_notes_clears_notes() {
    let (api, cache, list, task) = seed().await;
    let cleared = sync::write_notes(&api, &cache, &list.id, &task.id, None)
        .await
        .unwrap();
    assert_eq!(cleared.notes, None);
    assert_eq!(cache.tasks(&list.id).unwrap()[0].notes, None);
}

#[tokio::test]
async fn write_notes_surfaces_api_errors() {
    let (api, cache, list, task) = seed().await;
    api.fail_next(ApiError::Network("boom".to_string()));
    let result =
        sync::write_notes(&api, &cache, &list.id, &task.id, Some("nope".to_string())).await;
    assert!(result.is_err());
    // The optimistic write never reached the cache.
    assert_eq!(
        cache.tasks(&list.id).unwrap()[0].notes.as_deref(),
        Some("first")
    );
}
