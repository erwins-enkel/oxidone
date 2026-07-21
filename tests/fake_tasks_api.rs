//! Behavioural tests for `FakeTasksApi` at the `TasksApi` seam (ticket #2).
//! Everything here is black-box: it only touches the public trait + fake, no
//! terminal, network, or Google account.

use oxidone::api::{ApiError, NewTask, TaskPatch, TasksApi};
use oxidone::domain::{ListId, Status, TaskId};

fn fake() -> oxidone::api::FakeTasksApi {
    oxidone::api::FakeTasksApi::new()
}

fn new_task(title: &str) -> NewTask {
    NewTask {
        title: title.to_string(),
        ..Default::default()
    }
}

// ---- Lists ----

#[tokio::test]
async fn seed_and_list_lists() {
    let api = fake();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();

    let lists = api.list_lists().await.unwrap();
    let titles: Vec<_> = lists.iter().map(|l| l.title.as_str()).collect();
    assert_eq!(titles, ["Work", "Home"]);
}

#[tokio::test]
async fn default_list_falls_back_to_first_then_honours_an_override() {
    let api = fake();
    let work = api.insert_list("Work").await.unwrap();
    let home = api.insert_list("Home").await.unwrap();

    // No override: `@default` is the first List (Google returns the default first).
    assert_eq!(api.default_list().await.unwrap().id, work.id);

    // Pointed elsewhere, it resolves to that List.
    api.set_default_list(&home.id);
    assert_eq!(api.default_list().await.unwrap().id, home.id);
}

#[tokio::test]
async fn default_list_with_no_lists_is_not_found() {
    let api = fake();
    assert_eq!(api.default_list().await.unwrap_err(), ApiError::NotFound);
}

#[tokio::test]
async fn patch_list_updates_title() {
    let api = fake();
    let l = api.insert_list("Wrok").await.unwrap();
    let patched = api.patch_list(&l.id, "Work").await.unwrap();
    assert_eq!(patched.title, "Work");
    assert_eq!(patched.id, l.id);
}

#[tokio::test]
async fn patch_missing_list_is_not_found() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    api.delete_list(&l.id).await.unwrap();
    let err = api.patch_list(&l.id, "Nope").await.unwrap_err();
    assert_eq!(err, ApiError::NotFound);
}

#[tokio::test]
async fn delete_list_removes_its_tasks() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    api.insert_task(&l.id, new_task("a")).await.unwrap();
    api.delete_list(&l.id).await.unwrap();

    let lists = api.list_lists().await.unwrap();
    assert!(lists.is_empty());
    // Listing tasks of a deleted list is a not-found.
    let err = api.list_tasks(&l.id, true, true, None).await.unwrap_err();
    assert_eq!(err, ApiError::NotFound);
}

// ---- Tasks ----

#[tokio::test]
async fn insert_task_into_missing_list_is_not_found() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    api.delete_list(&l.id).await.unwrap();
    let err = api.insert_task(&l.id, new_task("a")).await.unwrap_err();
    assert_eq!(err, ApiError::NotFound);
}

#[tokio::test]
async fn insert_then_list_tasks_preserves_order() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    api.insert_task(&l.id, new_task("first")).await.unwrap();
    api.insert_task(&l.id, new_task("second")).await.unwrap();

    let tasks = api.list_tasks(&l.id, false, false, None).await.unwrap();
    let titles: Vec<_> = tasks.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["first", "second"]);
    assert!(tasks.iter().all(|t| t.status == Status::NeedsAction));
}

#[tokio::test]
async fn list_tasks_filters_completed_by_default() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let t = api.insert_task(&l.id, new_task("done me")).await.unwrap();
    api.insert_task(&l.id, new_task("open")).await.unwrap();

    let completed = api
        .patch_task(
            &l.id,
            &t.id,
            TaskPatch {
                completed: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(completed.status, Status::Completed);
    assert!(completed.completed_at.is_some());

    // Default view hides completed.
    let visible = api.list_tasks(&l.id, false, false, None).await.unwrap();
    let titles: Vec<_> = visible.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["open"]);

    // show_completed reveals it.
    let all = api.list_tasks(&l.id, true, false, None).await.unwrap();
    assert_eq!(all.len(), 2);
}

#[tokio::test]
async fn uncompleting_clears_completed_at() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let t = api.insert_task(&l.id, new_task("x")).await.unwrap();
    api.patch_task(
        &l.id,
        &t.id,
        TaskPatch {
            completed: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let reopened = api
        .patch_task(
            &l.id,
            &t.id,
            TaskPatch {
                completed: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(reopened.status, Status::NeedsAction);
    assert!(reopened.completed_at.is_none());
}

#[tokio::test]
async fn clear_completed_hides_completed_tasks() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let t = api.insert_task(&l.id, new_task("done")).await.unwrap();
    api.insert_task(&l.id, new_task("open")).await.unwrap();
    api.patch_task(
        &l.id,
        &t.id,
        TaskPatch {
            completed: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    api.clear_completed(&l.id).await.unwrap();

    // Even asking for completed, hidden ones stay hidden.
    let shown = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let titles: Vec<_> = shown.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["open"]);

    // show_hidden brings the cleared task back.
    let with_hidden = api.list_tasks(&l.id, true, true, None).await.unwrap();
    assert_eq!(with_hidden.len(), 2);
}

#[tokio::test]
async fn updated_min_returns_only_newer_tasks() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let first = api.insert_task(&l.id, new_task("first")).await.unwrap();
    let second = api.insert_task(&l.id, new_task("second")).await.unwrap();
    assert!(second.updated > first.updated);

    let newer = api
        .list_tasks(&l.id, true, false, Some(second.updated))
        .await
        .unwrap();
    let titles: Vec<_> = newer.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["second"]);
}

#[tokio::test]
async fn delete_task_removes_it_from_listing() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let t = api.insert_task(&l.id, new_task("bye")).await.unwrap();
    api.delete_task(&l.id, &t.id).await.unwrap();

    let tasks = api.list_tasks(&l.id, true, true, None).await.unwrap();
    assert!(tasks.is_empty());
    // Deleting again is a not-found.
    let err = api.delete_task(&l.id, &t.id).await.unwrap_err();
    assert_eq!(err, ApiError::NotFound);
}

#[tokio::test]
async fn move_task_reorders_and_reparents() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let a = api.insert_task(&l.id, new_task("a")).await.unwrap();
    let b = api.insert_task(&l.id, new_task("b")).await.unwrap();
    let c = api.insert_task(&l.id, new_task("c")).await.unwrap();

    // Move c to sit right after a -> order a, c, b.
    let moved = api
        .move_task(&l.id, &c.id, None, Some(&a.id))
        .await
        .unwrap();
    assert!(!moved.is_subtask());
    let order: Vec<_> = api
        .list_tasks(&l.id, true, true, None)
        .await
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(order, ["a", "c", "b"]);

    // Make b a subtask of a.
    let sub = api
        .move_task(&l.id, &b.id, Some(&a.id), None)
        .await
        .unwrap();
    assert_eq!(sub.parent.as_ref(), Some(&a.id));
    assert!(sub.is_subtask());
}

#[tokio::test]
async fn move_rejects_nesting_a_subtask_under_a_subtask() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let a = api.insert_task(&l.id, new_task("a")).await.unwrap();
    let b = api.insert_task(&l.id, new_task("b")).await.unwrap();
    let c = api.insert_task(&l.id, new_task("c")).await.unwrap();

    // b becomes a Subtask of a — fine (one level).
    api.move_task(&l.id, &b.id, Some(&a.id), None)
        .await
        .unwrap();
    // Nesting c under the Subtask b would make two levels — rejected.
    let err = api
        .move_task(&l.id, &c.id, Some(&b.id), None)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Rejected { status: 400, .. }));
}

#[tokio::test]
async fn move_rejects_demoting_a_parent_into_a_subtask() {
    let api = fake();
    let l = api.insert_list("Work").await.unwrap();
    let a = api.insert_task(&l.id, new_task("a")).await.unwrap();
    let b = api.insert_task(&l.id, new_task("b")).await.unwrap();
    let c = api.insert_task(&l.id, new_task("c")).await.unwrap();

    // b is a Subtask of a, so a now has children.
    api.move_task(&l.id, &b.id, Some(&a.id), None)
        .await
        .unwrap();
    // Making a (which has children) a Subtask of c would create two levels.
    let err = api
        .move_task(&l.id, &a.id, Some(&c.id), None)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::Rejected { status: 400, .. }));
}

#[tokio::test]
async fn move_task_to_list_relocates_to_the_head_of_the_destination() {
    let api = fake();
    let src = api.insert_list("Work").await.unwrap();
    let dst = api.insert_list("Home").await.unwrap();
    let a = api.insert_task(&src.id, new_task("a")).await.unwrap();
    let b = api.insert_task(&src.id, new_task("b")).await.unwrap();
    let z = api.insert_task(&dst.id, new_task("z")).await.unwrap();

    let moved = api
        .move_task_to_list(&src.id, &a.id, &dst.id)
        .await
        .unwrap();
    assert_eq!(moved.list, dst.id);

    // Gone from the source, which renumbers behind it.
    let left: Vec<_> = api
        .list_tasks(&src.id, true, true, None)
        .await
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(left, ["b"]);
    assert_eq!(
        b.id,
        api.list_tasks(&src.id, true, true, None).await.unwrap()[0].id
    );

    // At the *head* of the destination: the move carries no `previous`.
    let arrived: Vec<_> = api
        .list_tasks(&dst.id, true, true, None)
        .await
        .unwrap()
        .iter()
        .map(|t| t.title.clone())
        .collect();
    assert_eq!(arrived, ["a", "z"]);
    assert_ne!(z.position, "");
}

#[tokio::test]
async fn move_task_to_list_lands_a_subtask_top_level() {
    let api = fake();
    let src = api.insert_list("Work").await.unwrap();
    let dst = api.insert_list("Home").await.unwrap();
    let parent = api.insert_task(&src.id, new_task("parent")).await.unwrap();
    let child = api.insert_task(&src.id, new_task("child")).await.unwrap();
    api.move_task(&src.id, &child.id, Some(&parent.id), None)
        .await
        .unwrap();

    let moved = api
        .move_task_to_list(&src.id, &child.id, &dst.id)
        .await
        .unwrap();
    // The parent lives in the source List and cannot follow, so the Subtask
    // arrives top-level — the guarantee `rest.rs` enforces on the wire side.
    assert_eq!(moved.parent, None);
    assert!(!moved.is_subtask());
    assert_eq!(moved.position, format!("{:020}", 0));
}

#[tokio::test]
async fn move_task_to_list_rejects_an_unknown_destination() {
    let api = fake();
    let src = api.insert_list("Work").await.unwrap();
    let a = api.insert_task(&src.id, new_task("a")).await.unwrap();

    let err = api
        .move_task_to_list(&src.id, &a.id, &ListId("nope".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound));
    // Rejected before `tick()`, so the Task is untouched where it stands.
    let still = api.list_tasks(&src.id, true, true, None).await.unwrap();
    assert_eq!(still[0].etag, a.etag);
}

#[tokio::test]
async fn move_task_to_list_rejects_an_unknown_task() {
    let api = fake();
    let src = api.insert_list("Work").await.unwrap();
    let dst = api.insert_list("Home").await.unwrap();

    let err = api
        .move_task_to_list(&src.id, &TaskId("nope".into()), &dst.id)
        .await
        .unwrap_err();
    assert!(matches!(err, ApiError::NotFound));
}

// Deliberately no test that the fake refuses a parent with Subtasks: it does not.
// That rule is oxidone's (Google's behaviour here is unverified), it lives in
// `sync::move_task_to_list`, and teaching it to the fake would mask the boundary
// test that pins it — see `tests/move_to_list_boundary.rs`.

// ---- Fault injection ----

#[tokio::test]
async fn fail_next_injects_one_error_then_recovers() {
    let api = fake();
    api.fail_next(ApiError::AuthExpired);
    let err = api.list_lists().await.unwrap_err();
    assert_eq!(err, ApiError::AuthExpired);

    // One-shot: the next call succeeds.
    let ok = api.list_lists().await;
    assert!(ok.is_ok());
}

#[tokio::test]
async fn fail_next_applies_to_writes_too() {
    let api = fake();
    api.fail_next(ApiError::Network("boom".into()));
    let err = api.insert_list("Work").await.unwrap_err();
    assert_eq!(err, ApiError::Network("boom".into()));
    // The failed write left no state behind.
    assert!(api.list_lists().await.unwrap().is_empty());
}
