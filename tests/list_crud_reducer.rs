//! Reducer tests for List CRUD (ticket #14): the sidebar overlay flows and the
//! optimistic create / rename / delete write-through, mirroring the Task-CRUD
//! reducer tests. `update` is pure — no terminal, no network.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, ConfirmAction, Message, Model, Overlay};
use oxidone::domain::{List, ListId, Selection, Task};

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

/// Build two Lists ("Work", then "Home") each holding one Task, and a Model
/// seeded with them, focused on the sidebar (the default focus).
async fn model_with_two_lists() -> (Model, Vec<List>, Vec<Task>) {
    let api = FakeTasksApi::new();
    let mut lists = Vec::new();
    let mut tasks = Vec::new();
    for name in ["Work", "Home"] {
        let l = api.insert_list(name).await.unwrap();
        let t = api
            .insert_task(
                &l.id,
                NewTask {
                    title: format!("{name}-task"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        lists.push(l);
        tasks.push(t);
    }
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists.clone()));
    // Startup lands on the pinned Today row (#61); select the first List so the
    // sidebar CRUD verbs (which are focused-List-gated) act on a real List.
    m.selected = Selection::List(0);
    let first_tasks = api
        .list_tasks(&lists[0].id, true, false, None)
        .await
        .unwrap();
    update(
        &mut m,
        Message::TasksLoaded(lists[0].id.clone(), first_tasks),
    );
    (m, lists, tasks)
}

// ---- Create ----

#[tokio::test]
async fn shift_a_opens_the_add_list_overlay() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    match &m.overlay {
        Some(Overlay::AddList { buffer }) => assert_eq!(buffer, ""),
        other => panic!("expected AddList overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn typing_and_enter_appends_a_placeholder_and_requests_insert() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    typed(&mut m, "Errands");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.lists.len(), 3);
    assert_eq!(m.lists[2].title, "Errands"); // Google appends new Lists
    assert_eq!(m.selected, Selection::List(2)); // becomes active
    assert!(m.tasks.is_empty()); // a fresh List is empty
    assert_eq!(
        cmds,
        vec![Command::AddList {
            temp: ListId("temp-list-0".to_string()),
            title: "Errands".to_string(),
        }]
    );
    let _ = lists;
}

#[tokio::test]
async fn an_empty_list_title_adds_nothing() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(m.lists.len(), 2);
}

#[tokio::test]
async fn esc_cancels_the_add_list_capture() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    typed(&mut m, "nope");
    update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert_eq!(m.lists.len(), 2);
}

#[tokio::test]
async fn list_inserted_replaces_the_placeholder_with_the_server_list() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    typed(&mut m, "Errands");
    update(&mut m, key(KeyCode::Enter));

    let mut server = m.lists[2].clone();
    server.id = ListId("real-list-9".to_string());
    server.etag = "e9".to_string();
    let cmds = update(
        &mut m,
        Message::ListInserted {
            temp: ListId("temp-list-0".to_string()),
            list: server,
        },
    );
    assert_eq!(m.lists.len(), 3);
    assert_eq!(m.lists[2].id, ListId("real-list-9".to_string()));
    // The reconciled List is active, so its (server) Tasks are requested.
    assert_eq!(
        cmds,
        vec![Command::LoadTasks(ListId("real-list-9".to_string()))]
    );
}

#[tokio::test]
async fn a_failed_list_add_drops_the_placeholder_and_reselects() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    typed(&mut m, "Errands");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.lists.len(), 3);
    assert_eq!(m.selected, Selection::List(2));

    let cmds = update(
        &mut m,
        Message::ListAddFailed {
            temp: ListId("temp-list-0".to_string()),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.lists.len(), 2); // placeholder removed
    assert_eq!(m.selected, Selection::List(1)); // clamped back onto a real List
    assert_eq!(m.status_line.as_deref(), Some("boom"));
    // The now-active List's Tasks are reloaded.
    assert_eq!(cmds, vec![Command::LoadTasks(lists[1].id.clone())]);
}

#[tokio::test]
async fn temp_list_ids_are_unique_across_adds() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('A'));
    typed(&mut m, "one");
    update(&mut m, key(KeyCode::Enter));
    update(&mut m, ch('A'));
    typed(&mut m, "two");
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert_eq!(
        cmds,
        vec![Command::AddList {
            temp: ListId("temp-list-1".to_string()),
            title: "two".to_string(),
        }]
    );
}

// ---- Rename ----

#[tokio::test]
async fn shift_r_opens_the_rename_overlay_prefilled() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('R'));
    match &m.overlay {
        Some(Overlay::RenameList { buffer, .. }) => assert_eq!(buffer, "Work"),
        other => panic!("expected RenameList overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn renaming_and_enter_writes_through_optimistically() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('R'));
    for _ in 0..4 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "Job");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.lists[0].title, "Job"); // optimistic
    assert_eq!(
        cmds,
        vec![Command::RenameList {
            list: lists[0].id.clone(),
            title: "Job".to_string(),
        }]
    );
}

#[tokio::test]
async fn an_empty_rename_is_not_submitted() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('R'));
    for _ in 0..4 {
        update(&mut m, key(KeyCode::Backspace));
    }
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(m.lists[0].title, "Work");
}

#[tokio::test]
async fn a_failed_rename_rolls_back_to_the_old_title() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('R'));
    for _ in 0..4 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "Job");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.lists[0].title, "Job");

    update(
        &mut m,
        Message::ListWriteFailed {
            list: lists[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.lists[0].title, "Work"); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn list_updated_adopts_the_server_list() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('R'));
    for _ in 0..4 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "Job");
    update(&mut m, key(KeyCode::Enter));

    let mut server = m.lists[0].clone();
    server.title = "Job".to_string();
    server.etag = "server-etag".to_string();
    update(&mut m, Message::ListUpdated(server));
    assert_eq!(m.lists[0].etag, "server-etag");
    assert_eq!(m.lists[0].title, "Job");
    let _ = lists;
}

// ---- Delete ----

#[tokio::test]
async fn shift_x_opens_a_delete_list_confirmation() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('X'));
    match &m.overlay {
        Some(Overlay::Confirm(c)) => {
            assert!(
                matches!(&c.action, ConfirmAction::DeleteList { list } if *list == lists[0].id)
            );
        }
        other => panic!("expected Confirm overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn confirming_deletes_the_list_optimistically_and_reselects() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('X'));
    let cmds = update(&mut m, ch('y'));

    assert!(m.overlay.is_none());
    assert_eq!(m.lists.len(), 1); // "Work" removed optimistically
    assert_eq!(m.lists[0].title, "Home");
    assert_eq!(m.selected, Selection::List(0)); // "Home" is now active
                                                // The delete is requested and the newly-active List's Tasks reloaded.
    assert_eq!(
        cmds,
        vec![
            Command::DeleteList {
                list: lists[0].id.clone(),
            },
            Command::LoadTasks(lists[1].id.clone()),
        ]
    );
}

#[tokio::test]
async fn declining_keeps_the_list() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, ch('X'));
    let cmds = update(&mut m, ch('n'));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
    assert_eq!(m.lists.len(), 2);
}

#[tokio::test]
async fn a_failed_list_delete_reinserts_and_reselects_it() {
    // Mirrors Google's undeletable default List: the delete fails, so the
    // optimistic removal is rolled back cleanly and surfaced on the status line.
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('X'));
    update(&mut m, ch('y')); // optimistic delete of "Work" (index 0)
    assert_eq!(m.lists.len(), 1);

    let cmds = update(
        &mut m,
        Message::ListDeleteFailed {
            list: lists[0].id.clone(),
            reason: "cannot delete the default list".to_string(),
        },
    );
    assert_eq!(m.lists.len(), 2);
    assert_eq!(m.lists[0].title, "Work"); // back at index 0
    assert_eq!(m.selected, Selection::List(0)); // and reselected
    assert_eq!(
        m.status_line.as_deref(),
        Some("cannot delete the default list")
    );
    // The restored List's Tasks are reloaded.
    assert_eq!(cmds, vec![Command::LoadTasks(lists[0].id.clone())]);
}

#[tokio::test]
async fn a_confirmed_list_delete_is_final_and_cannot_be_rolled_back() {
    let (mut m, lists, _t) = model_with_two_lists().await;
    update(&mut m, ch('X'));
    update(&mut m, ch('y'));
    update(&mut m, Message::ListDeleted(lists[0].id.clone()));
    // A late failure for an already-finalized delete must not resurrect it.
    update(
        &mut m,
        Message::ListDeleteFailed {
            list: lists[0].id.clone(),
            reason: "stale".to_string(),
        },
    );
    assert_eq!(m.lists.len(), 1);
    assert_eq!(m.lists[0].title, "Home");
}

// ---- Focus gating ----

#[tokio::test]
async fn list_verbs_are_inert_while_the_task_pane_is_focused() {
    let (mut m, _l, _t) = model_with_two_lists().await;
    update(&mut m, key(KeyCode::Tab)); // focus the task pane
    update(&mut m, ch('A'));
    assert!(m.overlay.is_none());
    update(&mut m, ch('R'));
    assert!(m.overlay.is_none());
    update(&mut m, ch('X'));
    assert!(m.overlay.is_none());
}
