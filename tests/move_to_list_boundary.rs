//! Boundary tests for the cross-List Move: `sync::move_task_to_list` and
//! `sync::write_move_to_list` over a seeded `FakeTasksApi` + in-memory cache.

use oxidone::api::{ApiError, FakeTasksApi, NewTask, TaskPatch, TasksApi};
use oxidone::cache::Cache;
use oxidone::domain::{List, ListId, Task, TaskId};
use oxidone::sync;

fn new_task(title: &str) -> NewTask {
    NewTask {
        title: title.to_string(),
        ..Default::default()
    }
}

/// Two Lists and one Task in the source, mirrored into the cache.
async fn seed() -> (FakeTasksApi, Cache, List, List, Task) {
    let api = FakeTasksApi::new();
    let source = api.insert_list("Work").await.unwrap();
    let destination = api.insert_list("Home").await.unwrap();
    let task = api
        .insert_task(&source.id, new_task("relocate me"))
        .await
        .unwrap();
    let cache = Cache::open_in_memory().unwrap();
    cache
        .replace_tasks(&source.id, std::slice::from_ref(&task))
        .unwrap();
    (api, cache, source, destination, task)
}

#[tokio::test]
async fn write_move_to_list_relocates_the_row_rather_than_duplicating_it() {
    let (api, cache, source, destination, task) = seed().await;

    let moved = sync::write_move_to_list(&api, &cache, &source.id, &task.id, &destination.id)
        .await
        .unwrap();
    assert_eq!(moved.list, destination.id);

    // `tasks` is keyed by id and written INSERT OR REPLACE, so one upsert moves
    // the row: no delete on the source is needed, and none may be duplicated.
    assert!(cache.tasks(&source.id).unwrap().is_empty());
    let arrived = cache.tasks(&destination.id).unwrap();
    assert_eq!(arrived.len(), 1);
    assert_eq!(arrived[0].id, task.id);
    assert_eq!(
        cache
            .all_tasks()
            .unwrap()
            .iter()
            .filter(|t| t.id == task.id)
            .count(),
        1,
        "exactly one row across the whole cache"
    );
}

#[tokio::test]
async fn a_subtask_arrives_top_level() {
    let (api, cache, source, destination, parent) = seed().await;
    let child = api
        .insert_task(&source.id, new_task("child"))
        .await
        .unwrap();
    api.move_task(&source.id, &child.id, Some(&parent.id), None)
        .await
        .unwrap();

    let moved = sync::write_move_to_list(&api, &cache, &source.id, &child.id, &destination.id)
        .await
        .unwrap();
    // Its parent stayed behind and cannot follow, so it is promoted.
    assert_eq!(moved.parent, None);
    assert_eq!(cache.tasks(&destination.id).unwrap()[0].parent, None);
}

#[tokio::test]
async fn a_parent_with_a_visible_child_is_refused_before_any_write() {
    let (api, cache, source, destination, parent) = seed().await;
    let child = api
        .insert_task(&source.id, new_task("child"))
        .await
        .unwrap();
    api.move_task(&source.id, &child.id, Some(&parent.id), None)
        .await
        .unwrap();

    let err = sync::write_move_to_list(&api, &cache, &source.id, &parent.id, &destination.id)
        .await
        .unwrap_err();
    // The *message* is the assertion that matters: the fake implements no
    // children-rejection, so this sentence can only come from the pre-check.
    // Deleting that check fails here, where state alone would not notice.
    assert_eq!(
        err.to_string(),
        "can't move a task with subtasks to another list"
    );

    // Corroboration: nothing moved.
    let still = api.list_tasks(&source.id, true, true, None).await.unwrap();
    assert!(still.iter().any(|t| t.id == parent.id));
    assert!(api
        .list_tasks(&destination.id, true, true, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_cleared_child_still_refuses_the_move() {
    // The case neither the pane nor the cache can see: `fetch_active_tasks` asks
    // with `show_hidden=false`, so a Cleared child is absent from the cache
    // entirely. Only the live `show_hidden=true` query catches it.
    let (api, cache, source, destination, parent) = seed().await;
    let child = api
        .insert_task(&source.id, new_task("child"))
        .await
        .unwrap();
    api.move_task(&source.id, &child.id, Some(&parent.id), None)
        .await
        .unwrap();
    // The fake has no `hidden` setter: complete the child, then sweep the List.
    api.patch_task(
        &source.id,
        &child.id,
        TaskPatch {
            completed: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    api.clear_completed(&source.id).await.unwrap();

    // Re-mirror the way a refresh would, proving the child is invisible locally.
    let active = sync::fetch_active_tasks(&api, &source.id).await.unwrap();
    sync::mirror_tasks(&cache, &source.id, &active).unwrap();
    assert!(
        !cache
            .tasks(&source.id)
            .unwrap()
            .iter()
            .any(|t| t.id == child.id),
        "a Cleared child is not in the cache at all"
    );

    let err = sync::write_move_to_list(&api, &cache, &source.id, &parent.id, &destination.id)
        .await
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "can't move a task with subtasks to another list"
    );
    assert!(api
        .list_tasks(&destination.id, true, true, None)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_failing_move_post_leaves_the_cache_untouched() {
    // Reaching the POST's failure path without `fail_next`, which is a single
    // slot consumed by the pre-check's `list_tasks`: an unknown destination
    // passes the check and is rejected by the move itself.
    let (api, cache, source, _destination, task) = seed().await;

    let err = sync::write_move_to_list(
        &api,
        &cache,
        &source.id,
        &task.id,
        &ListId("no-such-list".into()),
    )
    .await
    .unwrap_err();
    assert_eq!(err.to_string(), "failed to move task");
    assert!(matches!(
        err.downcast_ref::<ApiError>(),
        Some(ApiError::NotFound)
    ));

    assert_eq!(cache.tasks(&source.id).unwrap().len(), 1);
}

#[tokio::test]
async fn a_failing_pre_check_never_reaches_the_move() {
    let (api, cache, source, destination, task) = seed().await;
    // One-shot and positional: `list_tasks` runs first, so this is the check.
    api.fail_next(ApiError::Network("down".into()));

    let err = sync::write_move_to_list(&api, &cache, &source.id, &task.id, &destination.id)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "failed to check for subtasks");

    let still = api.list_tasks(&source.id, true, true, None).await.unwrap();
    assert_eq!(still.len(), 1);
    assert!(api
        .list_tasks(&destination.id, true, true, None)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(cache.tasks(&destination.id).unwrap().len(), 0);
}

#[tokio::test]
async fn moving_an_unknown_task_is_not_found() {
    let (api, cache, source, destination, _task) = seed().await;
    let err = sync::write_move_to_list(
        &api,
        &cache,
        &source.id,
        &TaskId("ghost".into()),
        &destination.id,
    )
    .await
    .unwrap_err();
    assert!(matches!(
        err.downcast_ref::<ApiError>(),
        Some(ApiError::NotFound)
    ));
}
