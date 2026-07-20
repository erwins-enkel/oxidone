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
    model_with_titles(&["alpha", "beta"]).await
}

async fn model_with_titles(titles: &[&str]) -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for t in titles {
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
async fn enter_opens_the_editor_like_e() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Enter));
    match &m.overlay {
        Some(Overlay::EditTitle { buffer, .. }) => assert_eq!(buffer, "alpha"),
        other => panic!("expected EditTitle overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn enter_in_the_sidebar_opens_nothing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Tab)); // back to the sidebar
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
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

// --- Entry types and the edit path -----------------------------------------
//
// The type lives in the title, so `e` has to strip it on open and re-apply it on
// save. Critically it re-applies with `EntryType::apply`, which never strips —
// only `t`/`T` repair a title, and only because they are rebuilding a prefix.

/// One task with `title` exactly as Google would return it, cursor on it.
async fn model_with_raw_title(title: &str) -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: title.to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, key(KeyCode::Tab));
    (m, l, tasks)
}

#[tokio::test]
async fn e_opens_a_typed_entry_on_its_display_title_without_the_glyph() {
    let (mut m, _l, _t) = model_with_raw_title("○ Standup").await;
    update(&mut m, ch('e'));
    match &m.overlay {
        Some(Overlay::EditTitle { buffer, .. }) => assert_eq!(buffer, "Standup"),
        other => panic!("expected EditTitle overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn editing_a_notes_title_preserves_its_type() {
    let (mut m, _l, _t) = model_with_raw_title("— idea").await;
    update(&mut m, ch('e'));
    for _ in 0.."idea".len() {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "better idea");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks[0].title, "— better idea");
}

#[tokio::test]
async fn editing_an_untyped_title_does_not_acquire_a_type() {
    let (mut m, _l, _t) = model_with_raw_title("alpha").await;
    update(&mut m, ch('e'));
    typed(&mut m, "!");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks[0].title, "alpha!");
}

#[tokio::test]
async fn saving_a_foreign_glyph_title_unchanged_writes_it_back_byte_identical() {
    // "○Standup" — no space — is glyph-prefixed but not canonical, so it parses
    // as an untyped Task whose display title still leads with the glyph. Pressing
    // Enter without editing means "save what is already there"; a stripping
    // re-apply here would silently write "Standup" instead.
    let (mut m, _l, _t) = model_with_raw_title("○Standup").await;
    update(&mut m, ch('e'));
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert_eq!(m.tasks[0].title, "○Standup");
    match cmds.as_slice() {
        [Command::SetTitle { title, .. }] => assert_eq!(title, "○Standup"),
        other => panic!("expected one SetTitle, got {other:?}"),
    }
}

#[tokio::test]
async fn clearing_a_notes_title_writes_nothing_rather_than_a_bare_glyph() {
    // The empty-check must run before the type is re-applied, or this writes
    // "— " — pure encoding with no content.
    let (mut m, _l, _t) = model_with_raw_title("— idea").await;
    update(&mut m, ch('e'));
    for _ in 0.."idea".len() {
        update(&mut m, key(KeyCode::Backspace));
    }
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].title, "— idea"); // untouched
}

#[tokio::test]
async fn a_bare_glyph_is_a_legitimate_title_and_is_saved() {
    // A Task may be titled "—" today, so a Note must be able to be too. The
    // result round-trips: raw "— —" parses back to a Note titled "—".
    let (mut m, _l, _t) = model_with_raw_title("— idea").await;
    update(&mut m, ch('e'));
    for _ in 0.."idea".len() {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "—");
    update(&mut m, key(KeyCode::Enter));

    assert_eq!(m.tasks[0].title, "— —");
    assert_eq!(m.tasks[0].entry_type(), oxidone::domain::EntryType::Note);
    assert_eq!(m.tasks[0].display_title(), "—");
}

#[tokio::test]
async fn a_type_change_landing_mid_edit_is_preserved_not_reverted() {
    // The type is read at save time, not captured when the overlay opened — so a
    // refetch that retypes the entry underneath the editor is not clobbered.
    let (mut m, _l, tasks) = model_with_raw_title("alpha").await;
    update(&mut m, ch('e'));
    typed(&mut m, "!");

    // A reconcile arrives while the editor is open: the entry is now an Event.
    update(
        &mut m,
        Message::TaskUpdated(Task {
            title: "○ alpha".to_string(),
            ..tasks[0].clone()
        }),
    );
    update(&mut m, key(KeyCode::Enter));

    assert_eq!(m.tasks[0].title, "○ alpha!");
}

#[tokio::test]
async fn the_delete_prompt_shows_the_display_title_not_the_glyph() {
    let (mut m, _l, _t) = model_with_raw_title("○ Standup").await;
    update(&mut m, ch('x'));
    match &m.overlay {
        Some(Overlay::Confirm(c)) => {
            assert!(c.prompt.contains("Standup"), "{:?}", c.prompt);
            assert!(!c.prompt.contains('○'), "glyph leaked: {:?}", c.prompt);
        }
        other => panic!("expected a Confirm overlay, got {other:?}"),
    }
}

// ---- A delete reply landing *before* a stale refresh (ticket #65) ----
//
// `x` removes the row optimistically. When the success `TaskDeleted` lands first
// it drops the rollback snapshot, leaving nothing to record the row is gone — so
// a *stale* refresh (its fetch issued before Google applied the delete) would
// resurrect it. A tombstone armed on the confirmation lets `set_tasks` drop the
// id from that stale fetch. Evicted once a fetch of the List omits the id.

#[tokio::test]
async fn a_confirmed_delete_tombstones_a_stale_refresh_that_still_lists_it() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y')); // optimistic delete of "alpha"
    update(&mut m, Message::TaskDeleted(tasks[0].id.clone())); // confirmed first
    assert_eq!(m.tasks.len(), 1);

    // The refresh still reports "alpha": its fetch predated the delete.
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["beta"] // dropped by the tombstone; the untouched row stayed
    );
}

#[tokio::test]
async fn a_tombstone_is_evicted_once_a_fetch_omits_the_id() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y'));
    update(&mut m, Message::TaskDeleted(tasks[0].id.clone()));

    // Google has caught up: this fetch omits "alpha", so the tombstone is spent.
    update(
        &mut m,
        Message::TasksLoaded(l.id.clone(), vec![tasks[1].clone()]),
    );
    assert_eq!(m.tasks.len(), 1);

    // A later fetch that lists "alpha" again is honoured — the tombstone did not
    // suppress the id forever.
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
}

#[tokio::test]
async fn a_spurious_task_deleted_arms_no_tombstone() {
    let (mut m, l, tasks) = model_with_tasks().await;
    // No optimistic delete is in flight, so there is no snapshot: the reply must
    // arm nothing, or a later fetch of that id would wrongly drop a live row.
    update(&mut m, Message::TaskDeleted(tasks[1].id.clone()));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );
}

// ---- A refresh landing inside the delete round-trip (ticket #51) ----
//
// `x` removes the row optimistically. A refresh in that window fetches a set
// Google has not yet applied the delete to, and `set_tasks` puts the row back.
// The success reply has to re-remove it — the failure twin already guards the
// same interleaving.

#[tokio::test]
async fn a_refresh_mid_delete_is_undone_by_the_confirmed_delete() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y')); // optimistic delete of "alpha"
    assert_eq!(m.tasks.len(), 1);

    // The refresh still reports "alpha": Google has not processed the delete.
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    assert_eq!(m.tasks.len(), 2); // resurrected

    update(&mut m, Message::TaskDeleted(tasks[0].id.clone()));
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["beta"] // gone again, and the untouched row stayed
    );
}

#[tokio::test]
async fn a_refresh_mid_delete_keeps_a_cursor_the_user_moved_elsewhere() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y'));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    // The delete stepped the cursor off "alpha" onto "beta", which the refresh
    // then re-anchored at index 1 (behind the resurrected "alpha").
    assert_eq!(m.selected_task, Some(1));

    update(&mut m, Message::TaskDeleted(tasks[0].id.clone()));
    // Removing "alpha" shifts "beta" down a slot; the cursor follows it by id
    // rather than staying on a stale index.
    assert_eq!(m.tasks.len(), 1);
    assert_eq!(m.selected_task, Some(0));
    assert_eq!(m.tasks[0].title, "beta");
}

#[tokio::test]
async fn a_refresh_mid_delete_steps_a_cursor_off_the_resurrected_row() {
    let (mut m, l, tasks) = model_with_titles(&["a", "b", "c"]).await;
    update(&mut m, ch('j')); // cursor onto "b"
    update(&mut m, ch('x'));
    update(&mut m, ch('y')); // optimistic delete of "b"
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    assert_eq!(m.tasks.len(), 3); // resurrected
    update(&mut m, ch('k')); // and the user parks the cursor back on it
    assert_eq!(m.selected_task, Some(1));

    update(&mut m, Message::TaskDeleted(tasks[1].id.clone()));
    // The row under the cursor goes, so the cursor steps to its successor —
    // not to the top of the pane.
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );
    assert_eq!(m.selected_task, Some(1));
    assert_eq!(m.tasks[1].title, "c");
}

#[tokio::test]
async fn a_confirmed_delete_with_nothing_resurrected_changes_nothing() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, ch('x'));
    update(&mut m, ch('y'));
    let (before_tasks, before_cursor) = (m.tasks.clone(), m.selected_task);

    // The ordinary case: no refresh intervened, so the reply must not disturb
    // the pane or the cursor the user may since have moved.
    update(&mut m, Message::TaskDeleted(tasks[0].id.clone()));
    assert_eq!(m.tasks, before_tasks);
    assert_eq!(m.selected_task, before_cursor);
}

#[tokio::test]
async fn a_task_deleted_for_a_task_we_are_not_deleting_removes_nothing() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    // No optimistic delete is in flight, so there is no snapshot to match and
    // the reply must not take a live row with it.
    update(&mut m, Message::TaskDeleted(tasks[1].id.clone()));
    assert_eq!(m.tasks.len(), 2);
}
