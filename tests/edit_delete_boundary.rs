//! Boundary tests for edit-title + delete (ticket #8): `sync::write_title` and
//! `sync::delete_task` over a seeded `FakeTasksApi` + in-memory cache.

use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
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
                title: "old".to_string(),
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
async fn write_title_patches_title_and_cache() {
    let (api, cache, list, task) = seed().await;
    let updated = sync::write_title(&api, &cache, &list.id, &task.id, "new")
        .await
        .unwrap();
    assert_eq!(updated.title, "new");
    assert_eq!(cache.tasks(&list.id).unwrap()[0].title, "new");
}

#[tokio::test]
async fn delete_task_removes_from_google_and_cache() {
    let (api, cache, list, task) = seed().await;
    sync::delete_task(&api, &cache, &list.id, &task.id)
        .await
        .unwrap();
    assert!(cache.tasks(&list.id).unwrap().is_empty());
    // The Task is gone from Google too.
    assert!(api
        .list_tasks(&list.id, true, true, None)
        .await
        .unwrap()
        .is_empty());
}
