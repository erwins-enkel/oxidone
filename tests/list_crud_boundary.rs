//! Boundary tests for List CRUD (ticket #14): the `sync` List helpers
//! (`insert_list` / `write_list_title` / `delete_list`) over a seeded
//! `FakeTasksApi` + an in-memory SQLite cache assert the cache is mirrored.
//! No terminal, no network.

use oxidone::api::{ApiError, FakeTasksApi, NewTask, TasksApi};
use oxidone::cache::Cache;
use oxidone::sync;

#[tokio::test]
async fn insert_list_creates_on_google_and_mirrors_into_cache() {
    let api = FakeTasksApi::new();
    let cache = Cache::open_in_memory().unwrap();

    let list = sync::insert_list(&api, &cache, "Errands").await.unwrap();
    assert_eq!(list.title, "Errands");

    // Cached (the source of truth for reads) and present on Google.
    let cached = cache.lists().unwrap();
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, list.id);
    assert_eq!(api.list_lists().await.unwrap().len(), 1);
}

#[tokio::test]
async fn write_list_title_renames_on_google_and_in_cache() {
    let api = FakeTasksApi::new();
    let list = api.insert_list("Work").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache.replace_lists(std::slice::from_ref(&list)).unwrap();

    let updated = sync::write_list_title(&api, &cache, &list.id, "Job")
        .await
        .unwrap();
    assert_eq!(updated.title, "Job");
    assert_eq!(cache.lists().unwrap()[0].title, "Job");
}

#[tokio::test]
async fn rename_preserves_sidebar_order() {
    // Renaming must not reorder the sidebar (the cache keys order off rowid).
    let api = FakeTasksApi::new();
    let work = api.insert_list("Work").await.unwrap();
    let home = api.insert_list("Home").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache.replace_lists(&[work.clone(), home.clone()]).unwrap();

    sync::write_list_title(&api, &cache, &work.id, "Job")
        .await
        .unwrap();

    let titles: Vec<_> = cache
        .lists()
        .unwrap()
        .into_iter()
        .map(|l| l.title)
        .collect();
    assert_eq!(titles, ["Job", "Home"]); // "Job" stays first
}

#[tokio::test]
async fn delete_list_removes_from_google_and_cache_with_its_tasks() {
    let api = FakeTasksApi::new();
    let list = api.insert_list("Work").await.unwrap();
    let task = api
        .insert_task(
            &list.id,
            NewTask {
                title: "t".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache.replace_lists(std::slice::from_ref(&list)).unwrap();
    cache
        .replace_tasks(&list.id, std::slice::from_ref(&task))
        .unwrap();

    sync::delete_list(&api, &cache, &list.id).await.unwrap();

    assert!(cache.lists().unwrap().is_empty());
    assert!(cache.tasks(&list.id).unwrap().is_empty()); // Tasks went with it
    assert!(api.list_lists().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_rejected_delete_leaves_google_and_cache_untouched() {
    // Models Google refusing to delete the default List: the error propagates
    // and nothing is mirrored away.
    let api = FakeTasksApi::new();
    let list = api.insert_list("Work").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache.replace_lists(std::slice::from_ref(&list)).unwrap();

    api.fail_next(ApiError::Rejected {
        status: 400,
        message: "cannot delete the default list".to_string(),
    });
    let result = sync::delete_list(&api, &cache, &list.id).await;
    assert!(result.is_err());
    assert_eq!(cache.lists().unwrap().len(), 1); // still cached
    assert_eq!(api.list_lists().await.unwrap().len(), 1); // still on Google
}

#[tokio::test]
async fn insert_list_surfaces_api_errors() {
    let api = FakeTasksApi::new();
    let cache = Cache::open_in_memory().unwrap();
    api.fail_next(ApiError::AuthExpired);
    assert!(sync::insert_list(&api, &cache, "X").await.is_err());
    assert!(cache.lists().unwrap().is_empty());
}
