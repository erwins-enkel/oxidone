//! Reducer tests for edit-title + delete (ticket #8): overlay flows, optimistic
//! title write, optimistic delete with confirm + rollback. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Message, Model, Overlay};
use oxidone::domain::{List, Task};

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

async fn model_with_tasks() -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for t in ["alpha", "beta"] {
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
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    (m, l, tasks)
}

// ---- Edit title ----

#[tokio::test]
async fn e_opens_the_editor_prefilled_with_the_title() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('e'));
    match &m.overlay {
        Some(Overlay::EditTitle { buffer, .. }) => assert_eq!(buffer, "alpha"),
        other => panic!("expected EditTitle overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn editing_and_enter_writes_through_optimistically() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('e'));
    // Clear "alpha" and type a new title.
    for _ in 0..5 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "alptra");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks[0].title, "alptra"); // optimistic
    assert_eq!(
        cmds,
        vec![Command::SetTitle {
            list: l.id,
            task: tasks[0].id.clone(),
            title: "alptra".to_string(),
        }]
    );
}

#[tokio::test]
async fn esc_cancels_the_edit_without_writing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('e'));
    typed(&mut m, "zzz");
    let cmds = update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].title, "alpha"); // unchanged
}

#[tokio::test]
async fn an_empty_title_is_not_submitted() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('e'));
    for _ in 0..5 {
        update(&mut m, key(KeyCode::Backspace));
    }
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].title, "alpha");
}

#[tokio::test]
async fn a_failed_title_write_rolls_back_to_the_old_title() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, ch('e'));
    for _ in 0..5 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "renamed");
    update(&mut m, key(KeyCode::Enter)); // optimistic
    assert_eq!(m.tasks[0].title, "renamed");

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].title, "alpha"); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

// ---- Delete ----

#[tokio::test]
async fn x_opens_a_delete_confirmation() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('x'));
    assert!(matches!(m.overlay, Some(Overlay::Confirm(_))));
}

#[tokio::test]
async fn confirming_deletes_optimistically_and_emits_a_command() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    let cmds = update(&mut m, ch('y'));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks.len(), 1); // removed optimistically
    assert_eq!(m.tasks[0].title, "beta");
    assert_eq!(
        cmds,
        vec![Command::DeleteTask {
            list: l.id,
            task: tasks[0].id.clone(),
        }]
    );
}

#[tokio::test]
async fn declining_keeps_the_task() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('x'));
    let cmds = update(&mut m, ch('n'));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
    assert_eq!(m.tasks.len(), 2);
}

#[tokio::test]
async fn a_failed_delete_reinserts_the_task_at_its_place() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y')); // optimistic delete of "alpha" (index 0)
    assert_eq!(m.tasks.len(), 1);

    update(
        &mut m,
        Message::TaskDeleteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks.len(), 2);
    assert_eq!(m.tasks[0].title, "alpha"); // back at index 0
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn a_confirmed_delete_is_final_and_cannot_be_rolled_back() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y'));
    update(&mut m, Message::TaskDeleted(tasks[0].id.clone()));
    // A late failure for an already-finalized delete must not resurrect it.
    update(
        &mut m,
        Message::TaskDeleteFailed {
            task: tasks[0].id.clone(),
            reason: "stale".to_string(),
        },
    );
    assert_eq!(m.tasks.len(), 1);
}
