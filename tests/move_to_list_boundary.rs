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
async fn a_parent_carries_its_visible_child_into_the_destination() {
    let (api, cache, source, destination, parent) = seed().await;
    let child = api
        .insert_task(&source.id, new_task("child"))
        .await
        .unwrap();
    api.move_task(&source.id, &child.id, Some(&parent.id), None)
        .await
        .unwrap();
    // Mirror the child too, the way a refresh would: it is the row that would be
    // left naming the source if the reconcile stopped at the parent (#94).
    let active = sync::fetch_active_tasks(&api, &source.id).await.unwrap();
    sync::mirror_tasks(&cache, &source.id, &active).unwrap();

    sync::write_move_to_list(&api, &cache, &source.id, &parent.id, &destination.id)
        .await
        .unwrap();

    // Google's side: both are in the destination, the child still naming its
    // parent — the behaviour #86 verified and `FakeTasksApi` models.
    let arrived = api
        .list_tasks(&destination.id, true, true, None)
        .await
        .unwrap();
    assert_eq!(arrived.len(), 2);
    let moved_child = arrived.iter().find(|t| t.id == child.id).unwrap();
    assert_eq!(moved_child.parent.as_ref(), Some(&parent.id));
    assert!(api
        .list_tasks(&source.id, true, true, None)
        .await
        .unwrap()
        .is_empty());

    // The mirror agrees (ADR-0003). Without `relocate_subtasks` the child's row
    // still reads `list = source` here while Google has it in the destination.
    assert!(cache.tasks(&source.id).unwrap().is_empty());
    let cached: Vec<_> = cache
        .tasks(&destination.id)
        .unwrap()
        .into_iter()
        .map(|t| t.id)
        .collect();
    assert!(cached.contains(&parent.id));
    assert!(cached.contains(&child.id));
}

#[tokio::test]
async fn a_cleared_child_follows_and_leaves_no_stale_cache_row() {
    // The case neither the pane nor the cache can see: `fetch_active_tasks` asks
    // with `show_hidden=false`, so a Cleared child is absent from the cache
    // entirely — which is also why it needs no reconcile. It has no row to leave
    // behind, and the local `UPDATE … WHERE parent` never has to reach it.
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

    sync::write_move_to_list(&api, &cache, &source.id, &parent.id, &destination.id)
        .await
        .unwrap();

    // It followed on Google — visible only under `show_hidden=true`.
    let arrived = api
        .list_tasks(&destination.id, true, true, None)
        .await
        .unwrap();
    assert!(arrived.iter().any(|t| t.id == child.id));
    // And left nothing stale behind: the cache holds no row for it at all, under
    // either List.
    assert!(!cache.all_tasks().unwrap().iter().any(|t| t.id == child.id));
    assert!(cache.tasks(&source.id).unwrap().is_empty());
}

#[tokio::test]
async fn a_failing_move_post_leaves_the_cache_untouched() {
    // An unknown destination, so the failure comes from the move's own
    // validation rather than injection — `fail_next` has its own test below.
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
async fn an_injected_failure_lands_on_the_move_itself() {
    // `fail_next` is one-shot and positional, so this pins the round-trip count:
    // the move is the *first* call a relocation makes. Re-introducing any
    // pre-check — the `list_tasks` #93 removed, or another — spends the slot
    // elsewhere and this reads its context line instead.
    let (api, cache, source, destination, task) = seed().await;
    api.fail_next(ApiError::Network("down".into()));

    let err = sync::write_move_to_list(&api, &cache, &source.id, &task.id, &destination.id)
        .await
        .unwrap_err();
    assert_eq!(err.to_string(), "failed to move task");

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
