//! Reducer tests for notes (ticket #10): the two entry paths (external editor
//! vs the inline fallback overlay), optimistic write-through, rollback, the
//! no-op-when-unchanged guard, and the single-flight guard. `update` is pure, so
//! the editor decision is driven by `model.editor_available`, not the real env.

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

/// Two tasks; "alpha" (index 0) starts with notes, "beta" without. `now` is left
/// at the placeholder — notes never touch the clock.
async fn model_with_tasks() -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "alpha".to_string(),
            notes: Some("hello".to_string()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "beta".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    (m, l, tasks)
}

#[tokio::test]
async fn n_with_no_editor_opens_the_inline_overlay_prefilled() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.editor_available = false;
    let cmds = update(&mut m, ch('n'));
    assert!(cmds.is_empty()); // inline path emits no command yet
    match &m.overlay {
        Some(Overlay::EditNotes { buffer, .. }) => assert_eq!(buffer, "hello"),
        other => panic!("expected EditNotes overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn n_with_no_editor_on_a_task_without_notes_opens_an_empty_overlay() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.editor_available = false;
    update(&mut m, key(KeyCode::Down)); // select "beta" (no notes)
    update(&mut m, ch('n'));
    match &m.overlay {
        Some(Overlay::EditNotes { buffer, .. }) => assert!(buffer.is_empty()),
        other => panic!("expected empty EditNotes overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn n_with_an_editor_emits_spawneditor_and_opens_no_overlay() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    m.editor_available = true;
    let cmds = update(&mut m, ch('n'));
    assert!(m.overlay.is_none());
    assert_eq!(
        cmds,
        vec![Command::SpawnEditor {
            task: tasks[0].id.clone(),
            notes: Some("hello".to_string()),
        }]
    );
}

#[tokio::test]
async fn submitting_inline_notes_sets_them_optimistically_and_emits_setnotes() {
    let (mut m, l, tasks) = model_with_tasks().await;
    m.editor_available = false;
    update(&mut m, key(KeyCode::Down)); // "beta", no notes yet
    update(&mut m, ch('n'));
    typed(&mut m, "buy milk");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks[1].notes.as_deref(), Some("buy milk")); // optimistic
    assert_eq!(
        cmds,
        vec![Command::SetNotes {
            list: l.id,
            task: tasks[1].id.clone(),
            notes: Some("buy milk".to_string()),
        }]
    );
}

#[tokio::test]
async fn an_empty_inline_buffer_clears_the_notes() {
    let (mut m, l, tasks) = model_with_tasks().await;
    m.editor_available = false;
    update(&mut m, ch('n')); // "alpha", notes "hello"
    for _ in 0..5 {
        update(&mut m, key(KeyCode::Backspace)); // wipe "hello"
    }
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks[0].notes, None); // cleared optimistically
    assert_eq!(
        cmds,
        vec![Command::SetNotes {
            list: l.id,
            task: tasks[0].id.clone(),
            notes: None,
        }]
    );
}

#[tokio::test]
async fn the_notes_edited_message_writes_through_optimistically() {
    // The external-editor path: the runtime feeds the edited text back as a
    // `NotesEdited` message, which drives the same optimistic write.
    let (mut m, l, tasks) = model_with_tasks().await;
    let cmds = update(
        &mut m,
        Message::NotesEdited {
            task: tasks[0].id.clone(),
            notes: Some("world".to_string()),
        },
    );
    assert_eq!(m.tasks[0].notes.as_deref(), Some("world"));
    assert_eq!(
        cmds,
        vec![Command::SetNotes {
            list: l.id,
            task: tasks[0].id.clone(),
            notes: Some("world".to_string()),
        }]
    );
}

#[tokio::test]
async fn unchanged_notes_are_a_noop() {
    // Exiting the editor without edits must not spawn a needless write.
    let (mut m, _l, tasks) = model_with_tasks().await;
    let cmds = update(
        &mut m,
        Message::NotesEdited {
            task: tasks[0].id.clone(),
            notes: Some("hello".to_string()), // identical to the current notes
        },
    );
    assert!(cmds.is_empty());
    assert!(m.status_line.is_none()); // not a single-flight rejection either
}

#[tokio::test]
async fn a_failed_notes_write_rolls_back_to_the_snapshot() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(
        &mut m,
        Message::NotesEdited {
            task: tasks[0].id.clone(),
            notes: Some("world".to_string()),
        },
    );
    assert_eq!(m.tasks[0].notes.as_deref(), Some("world")); // optimistic

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].notes.as_deref(), Some("hello")); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn a_second_notes_edit_while_one_is_in_flight_is_guarded() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    let first = update(
        &mut m,
        Message::NotesEdited {
            task: tasks[0].id.clone(),
            notes: Some("world".to_string()),
        },
    );
    assert_eq!(first.len(), 1); // write in flight

    let second = update(
        &mut m,
        Message::NotesEdited {
            task: tasks[0].id.clone(),
            notes: Some("again".to_string()),
        },
    );
    assert!(second.is_empty());
    assert_eq!(m.tasks[0].notes.as_deref(), Some("world")); // still the first edit
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn n_is_refused_while_a_write_is_in_flight() {
    // Opening the editor over an in-flight write would lose the edit on submit;
    // the guard refuses up front (for both the external and inline paths).
    let (mut m, _l, _t) = model_with_tasks().await;
    m.editor_available = true;
    m.show_completed = true; // keep the cursor on "alpha" after completing it
    update(&mut m, ch(' ')); // toggle complete on "alpha" => write in flight
    let cmds = update(&mut m, ch('n'));
    assert!(cmds.is_empty()); // no SpawnEditor
    assert!(m.overlay.is_none()); // and no inline overlay
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn esc_cancels_inline_notes_without_writing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.editor_available = false;
    update(&mut m, ch('n'));
    typed(&mut m, " scratch");
    let cmds = update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].notes.as_deref(), Some("hello")); // unchanged
}
