//! Boundary tests for the write path (ticket #6): `sync::write_completed` over a
//! seeded `FakeTasksApi` + in-memory cache, including retry-once on auth expiry.

use oxidone::api::{ApiError, FakeTasksApi, NewTask, TasksApi};
use oxidone::cache::Cache;
use oxidone::domain::Status;
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
async fn write_completed_marks_task_completed_and_patches_cache() {
    let (api, cache, list, task) = seed().await;

    let updated = sync::write_completed(&api, &cache, &list.id, &task.id, true)
        .await
        .unwrap();
    assert_eq!(updated.status, Status::Completed);
    assert!(updated.completed_at.is_some());

    // The cache reflects the write (patched from the response, not re-fetched).
    let cached = cache.tasks(&list.id).unwrap();
    assert_eq!(cached[0].status, Status::Completed);
}

#[tokio::test]
async fn write_completed_can_uncomplete() {
    let (api, cache, list, task) = seed().await;
    sync::write_completed(&api, &cache, &list.id, &task.id, true)
        .await
        .unwrap();
    let reopened = sync::write_completed(&api, &cache, &list.id, &task.id, false)
        .await
        .unwrap();
    assert_eq!(reopened.status, Status::NeedsAction);
    assert!(reopened.completed_at.is_none());
}

// NB: auth-expiry retry (force-refresh + retry once) lives in `RestClient::send`
// and is covered by the wiremock contract suite (tests/rest_contract.rs), not
// here — `FakeTasksApi` has no token/HTTP layer to exercise it.

#[tokio::test]
async fn write_completed_surfaces_api_errors() {
    let (api, cache, list, task) = seed().await;
    api.fail_next(ApiError::Network("boom".to_string()));
    let result = sync::write_completed(&api, &cache, &list.id, &task.id, true).await;
    assert!(result.is_err());
    // The optimistic write never reached the cache.
    assert_eq!(
        cache.tasks(&list.id).unwrap()[0].status,
        Status::NeedsAction
    );
}
