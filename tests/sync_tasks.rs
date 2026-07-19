//! Boundary tests for the Task read path (ticket #5): `sync::load_tasks` over a
//! seeded `FakeTasksApi` + in-memory SQLite. No terminal, no network.

use chrono::NaiveDate;
use oxidone::api::{FakeTasksApi, NewTask, TaskPatch, TasksApi};
use oxidone::cache::Cache;
use oxidone::domain::Status;
use oxidone::sync;

fn new_task(title: &str) -> NewTask {
    NewTask {
        title: title.to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn load_tasks_populates_cache_and_returns_them() {
    let api = FakeTasksApi::new();
    let l = api.insert_list("Work").await.unwrap();
    api.insert_task(&l.id, new_task("a")).await.unwrap();
    api.insert_task(&l.id, new_task("b")).await.unwrap();
    let cache = Cache::open_in_memory().unwrap();

    let tasks = sync::load_tasks(&api, &cache, &l.id).await.unwrap();
    let titles: Vec<_> = tasks.iter().map(|t| t.title.clone()).collect();
    assert_eq!(titles, ["a", "b"]);
    assert_eq!(cache.tasks(&l.id).unwrap().len(), 2);
}

#[tokio::test]
async fn load_tasks_includes_completed_but_excludes_cleared() {
    let api = FakeTasksApi::new();
    let l = api.insert_list("Work").await.unwrap();
    let done = api.insert_task(&l.id, new_task("done")).await.unwrap();
    api.insert_task(&l.id, new_task("open")).await.unwrap();
    api.patch_task(
        &l.id,
        &done.id,
        TaskPatch {
            completed: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let cache = Cache::open_in_memory().unwrap();

    // Completed-but-not-cleared is included (show_completed=true).
    let tasks = sync::load_tasks(&api, &cache, &l.id).await.unwrap();
    assert_eq!(tasks.len(), 2);
    assert!(tasks.iter().any(|t| t.status == Status::Completed));

    // After Clear, the cleared task is hidden (show_hidden=false).
    api.clear_completed(&l.id).await.unwrap();
    let tasks = sync::load_tasks(&api, &cache, &l.id).await.unwrap();
    let titles: Vec<_> = tasks.iter().map(|t| t.title.clone()).collect();
    assert_eq!(titles, ["open"]);
}

#[tokio::test]
async fn task_fields_round_trip_through_cache() {
    let api = FakeTasksApi::new();
    let l = api.insert_list("Work").await.unwrap();
    let due = NaiveDate::from_ymd_opt(2026, 8, 1).unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "with due".to_string(),
            notes: Some("a note".to_string()),
            due: Some(due),
            parent: None,
        },
    )
    .await
    .unwrap();
    let done = api.insert_task(&l.id, new_task("done")).await.unwrap();
    api.patch_task(
        &l.id,
        &done.id,
        TaskPatch {
            completed: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let cache = Cache::open_in_memory().unwrap();

    let tasks = sync::load_tasks(&api, &cache, &l.id).await.unwrap();
    let cached = cache.tasks(&l.id).unwrap();
    assert_eq!(cached, tasks); // stable through the DB

    let with_due = cached.iter().find(|t| t.title == "with due").unwrap();
    assert_eq!(with_due.due, Some(due));
    assert_eq!(with_due.notes.as_deref(), Some("a note"));
    let done = cached.iter().find(|t| t.title == "done").unwrap();
    assert_eq!(done.status, Status::Completed);
    assert!(done.completed_at.is_some());
}

#[tokio::test]
async fn load_tasks_mirrors_deletions_per_list() {
    let api = FakeTasksApi::new();
    let work = api.insert_list("Work").await.unwrap();
    let home = api.insert_list("Home").await.unwrap();
    let a = api.insert_task(&work.id, new_task("a")).await.unwrap();
    api.insert_task(&work.id, new_task("b")).await.unwrap();
    api.insert_task(&home.id, new_task("h")).await.unwrap();
    let cache = Cache::open_in_memory().unwrap();
    sync::load_tasks(&api, &cache, &work.id).await.unwrap();
    sync::load_tasks(&api, &cache, &home.id).await.unwrap();

    api.delete_task(&work.id, &a.id).await.unwrap();
    let work_tasks = sync::load_tasks(&api, &cache, &work.id).await.unwrap();

    assert_eq!(work_tasks.len(), 1);
    assert_eq!(work_tasks[0].title, "b");
    // The other List's cache is untouched.
    assert_eq!(cache.tasks(&home.id).unwrap().len(), 1);
}
