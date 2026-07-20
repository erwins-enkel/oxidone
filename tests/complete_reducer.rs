//! Reducer tests for the write path (ticket #6): optimistic toggle, rollback on
//! failure, and reconciliation with the server task. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Focus, Message, Model};
use oxidone::domain::{List, Selection, Status, Task};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn space() -> Message {
    key(KeyCode::Char(' '))
}

/// A Model focused on the task pane with two Tasks loaded.
async fn model_with_tasks() -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for t in ["a", "b"] {
        api.insert_task(
            &l.id,
            NewTask {
                title: t.to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, key(KeyCode::Tab)); // focus the task pane
    (m, l, tasks)
}

#[tokio::test]
async fn space_completes_the_selected_task_optimistically_and_emits_a_command() {
    let (mut m, l, tasks) = model_with_tasks().await;
    assert_eq!(m.tasks[0].status, Status::NeedsAction);

    let cmds = update(&mut m, space());
    assert_eq!(m.tasks[0].status, Status::Completed); // optimistic
    assert_eq!(
        cmds,
        vec![Command::SetCompleted {
            list: l.id.clone(),
            task: tasks[0].id.clone(),
            completed: true,
        }]
    );
}

#[tokio::test]
async fn space_toggles_a_completed_task_back_to_needs_action() {
    let (mut m, l, tasks) = model_with_tasks().await;
    m.show_completed = true; // keep the completed Task visible + selected to toggle it back
    update(&mut m, space()); // complete
                             // The write resolves (clears the single-flight guard) before the next toggle.
    let mut done = tasks[0].clone();
    done.status = Status::Completed;
    update(&mut m, Message::TaskUpdated(done));

    let cmds = update(&mut m, space()); // un-complete
    assert_eq!(m.tasks[0].status, Status::NeedsAction);
    assert_eq!(m.tasks[0].completed_at, None);
    assert_eq!(
        cmds,
        vec![Command::SetCompleted {
            list: l.id,
            task: tasks[0].id.clone(),
            completed: false,
        }]
    );
}

#[tokio::test]
async fn toggle_is_a_no_op_when_the_sidebar_is_focused() {
    let (mut m, _l, _tasks) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Tab)); // back to sidebar
    assert_eq!(m.focus, Focus::Sidebar);
    let cmds = update(&mut m, space());
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].status, Status::NeedsAction);
}

#[tokio::test]
async fn write_failure_rolls_back_the_optimistic_change_and_shows_status() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, space()); // optimistic complete
    assert_eq!(m.tasks[0].status, Status::Completed);

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "offline".to_string(),
        },
    );
    assert_eq!(m.tasks[0].status, Status::NeedsAction); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("offline"));
}

#[tokio::test]
async fn a_second_toggle_is_ignored_while_a_write_is_in_flight() {
    let (mut m, l, tasks) = model_with_tasks().await;
    m.show_completed = true; // stay on the just-completed Task to retry its toggle
    let first = update(&mut m, space()); // completes, write in flight
    assert_eq!(m.tasks[0].status, Status::Completed);
    assert_eq!(first.len(), 1);

    // Single-flight: the second toggle is dropped (no command, no state change)
    // until the in-flight write resolves.
    let second = update(&mut m, space());
    assert!(second.is_empty());
    assert_eq!(m.tasks[0].status, Status::Completed);

    // Once the write resolves, toggling works again.
    let mut updated = tasks[0].clone();
    updated.status = Status::Completed;
    update(&mut m, Message::TaskUpdated(updated));
    let third = update(&mut m, space());
    assert_eq!(
        third,
        vec![Command::SetCompleted {
            list: l.id,
            task: tasks[0].id.clone(),
            completed: false,
        }]
    );
}

#[tokio::test]
async fn rolling_back_an_uncomplete_restores_the_completed_timestamp() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    // Start from a genuinely-completed Task (server truth with a timestamp).
    let mut completed = tasks[0].clone();
    completed.status = Status::Completed;
    completed.completed_at = Some(chrono::Utc::now());
    update(&mut m, Message::TaskUpdated(completed.clone()));
    assert!(m.tasks[0].completed_at.is_some());

    update(&mut m, space()); // optimistic un-complete: clears completed_at
    assert_eq!(m.tasks[0].status, Status::NeedsAction);
    assert_eq!(m.tasks[0].completed_at, None);

    // The un-complete write fails → restore the exact prior state (timestamp back).
    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].status, Status::Completed);
    assert_eq!(m.tasks[0].completed_at, completed.completed_at);
}

#[tokio::test]
async fn task_updated_reconciles_with_the_server_version() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    // A server task carrying the authoritative completed_at + etag.
    let mut server = tasks[0].clone();
    server.status = Status::Completed;
    server.etag = "server-etag".to_string();

    update(&mut m, Message::TaskUpdated(server.clone()));
    assert_eq!(m.tasks[0].status, Status::Completed);
    assert_eq!(m.tasks[0].etag, "server-etag");
}
