//! Reducer tests for add-task (ticket #7): capture overlay, optimistic append
//! with a placeholder, and reconcile/rollback. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Message, Model, Overlay};
use oxidone::domain::{List, Status, Task, TaskId};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        update(m, ch(c));
    }
}

async fn model_with_one_task() -> (Model, List, Task) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    let t = api
        .insert_task(
            &l.id,
            NewTask {
                title: "first".to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab));
    (m, l, t)
}

#[tokio::test]
async fn a_opens_the_capture_overlay() {
    let (mut m, _l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    match &m.overlay {
        Some(Overlay::AddTask { buffer }) => assert_eq!(buffer, ""),
        other => panic!("expected AddTask overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn typing_and_enter_inserts_a_placeholder_at_the_top_and_requests_insert() {
    let (mut m, l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    typed(&mut m, "second");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks.len(), 2);
    let placeholder = &m.tasks[0]; // Google adds new Tasks to the top
    assert_eq!(placeholder.title, "second"); // optimistic
    assert_eq!(placeholder.status, Status::NeedsAction);
    assert_eq!(m.selected_task, Some(0)); // cursor moves to the new Task
    assert_eq!(
        cmds,
        vec![Command::AddTask {
            list: l.id,
            temp: TaskId("temp-0".to_string()),
            title: "second".to_string(),
        }]
    );
}

#[tokio::test]
async fn an_empty_title_adds_nothing() {
    let (mut m, _l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(m.tasks.len(), 1);
}

#[tokio::test]
async fn esc_cancels_the_capture() {
    let (mut m, _l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    typed(&mut m, "nope");
    update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert_eq!(m.tasks.len(), 1);
}

#[tokio::test]
async fn inserted_replaces_the_placeholder_with_the_server_task() {
    let (mut m, l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    typed(&mut m, "second");
    update(&mut m, key(KeyCode::Enter));

    // Server assigns a real id.
    let mut server = m.tasks[0].clone();
    server.id = TaskId("real-99".to_string());
    server.etag = "e99".to_string();
    update(
        &mut m,
        Message::TaskInserted {
            temp: TaskId("temp-0".to_string()),
            task: server,
        },
    );
    assert_eq!(m.tasks.len(), 2);
    assert_eq!(m.tasks[0].id, TaskId("real-99".to_string()));
    let _ = l;
}

#[tokio::test]
async fn a_failed_add_drops_the_placeholder() {
    let (mut m, _l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    typed(&mut m, "second");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks.len(), 2);

    update(
        &mut m,
        Message::TaskAddFailed {
            temp: TaskId("temp-0".to_string()),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks.len(), 1); // placeholder removed
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn temp_ids_are_unique_across_adds() {
    let (mut m, l, _t) = model_with_one_task().await;
    update(&mut m, ch('a'));
    typed(&mut m, "two");
    update(&mut m, key(KeyCode::Enter));
    update(&mut m, ch('a'));
    typed(&mut m, "three");
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert_eq!(
        cmds,
        vec![Command::AddTask {
            list: l.id,
            temp: TaskId("temp-1".to_string()),
            title: "three".to_string(),
        }]
    );
}
