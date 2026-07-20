//! Boundary test for add-task (ticket #7): `sync::insert_task` over a seeded
//! `FakeTasksApi` + in-memory cache.

use oxidone::api::{FakeTasksApi, TasksApi};
use oxidone::cache::Cache;
use oxidone::sync;

#[tokio::test]
async fn insert_task_adds_to_google_and_cache() {
    let api = FakeTasksApi::new();
    let list = api.insert_list("Work").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();

    let task = sync::insert_task(&api, &cache, &list.id, "new one")
        .await
        .unwrap();
    assert_eq!(task.title, "new one");

    // Present in Google and mirrored into the cache.
    assert_eq!(
        api.list_tasks(&list.id, true, false, None)
            .await
            .unwrap()
            .len(),
        1
    );
    let cached = cache.tasks(&list.id).unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].title, "new one");
}
