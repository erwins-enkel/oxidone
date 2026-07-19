//! Boundary tests for the List read path (ticket #4): `sync::load_lists` over a
//! seeded `FakeTasksApi` + an in-memory SQLite cache. No terminal, no network.

use oxidone::api::{FakeTasksApi, TasksApi};
use oxidone::cache::Cache;
use oxidone::sync;

#[tokio::test]
async fn load_lists_populates_cache_and_returns_them() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();

    let lists = sync::load_lists(&api, &cache).await.unwrap();
    let titles: Vec<_> = lists.iter().map(|l| l.title.clone()).collect();
    assert_eq!(titles, ["Work", "Home"]);

    // The cache is now the source of truth for reads.
    assert_eq!(cache.lists().unwrap().len(), 2);
}

#[tokio::test]
async fn load_lists_mirrors_deletions() {
    let api = FakeTasksApi::new();
    let work = api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();
    sync::load_lists(&api, &cache).await.unwrap();

    api.delete_list(&work.id).await.unwrap();
    let lists = sync::load_lists(&api, &cache).await.unwrap();

    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0].title, "Home");
    assert_eq!(cache.lists().unwrap().len(), 1);
}

#[tokio::test]
async fn cache_round_trips_a_list() {
    let api = FakeTasksApi::new();
    let a = api.insert_list("A").await.unwrap();
    let cache = Cache::open_in_memory().unwrap();

    cache.replace_lists(std::slice::from_ref(&a)).unwrap();
    let got = cache.lists().unwrap();

    assert_eq!(got.len(), 1);
    assert_eq!(got[0].id, a.id);
    assert_eq!(got[0].title, a.title);
    assert_eq!(got[0].etag, a.etag);
}

#[tokio::test]
async fn load_lists_surfaces_api_errors() {
    let api = FakeTasksApi::new();
    api.fail_next(oxidone::api::ApiError::AuthExpired);
    let cache = Cache::open_in_memory().unwrap();
    assert!(sync::load_lists(&api, &cache).await.is_err());
}
