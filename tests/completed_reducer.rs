//! Reducer tests for Completed handling (ticket #13): hide-by-default + reveal,
//! cursor skipping hidden rows, and the optimistic Clear with rollback. `update`
//! is pure, so these run with no terminal and no network.

use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, ConfirmAction, Message, Model, Overlay};
use oxidone::domain::{List, ListId, Status, Task, TaskId};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn list() -> List {
    List {
        id: ListId("L".to_string()),
        title: "L".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn task(id: &str, status: Status) -> Task {
    Task {
        id: TaskId(id.to_string()),
        list: ListId("L".to_string()),
        parent: None,
        title: id.to_string(),
        notes: None,
        status,
        due: None,
        completed_at: match status {
            Status::Completed => Some(Utc.timestamp_opt(1_700_000_100, 0).unwrap()),
            Status::NeedsAction => None,
        },
        links: Vec::new(),
        position: format!("{id:0>20}"),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

/// A focused task pane seeded with `tasks` in the given order.
fn model_with(tasks: Vec<Task>) -> Model {
    let l = list();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    m
}

fn titles(tasks: &[&Task]) -> Vec<String> {
    tasks.iter().map(|t| t.title.clone()).collect()
}

#[test]
fn completed_tasks_are_hidden_by_default() {
    let m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
    ]);
    assert!(!m.show_completed);
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "c"]);
}

#[test]
fn c_reveals_and_hides_completed() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('c'));
    assert!(m.show_completed);
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "b"]);
    update(&mut m, ch('c'));
    assert!(!m.show_completed);
    assert_eq!(titles(&m.visible_tasks()), vec!["a"]);
}

#[test]
fn the_cursor_skips_hidden_completed_rows() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
    ]);
    assert_eq!(m.selected_task, Some(0)); // "a"
    update(&mut m, key(KeyCode::Down)); // skip hidden "b"
    assert_eq!(m.selected_task, Some(2)); // "c"
    update(&mut m, key(KeyCode::Up)); // skip hidden "b" again
    assert_eq!(m.selected_task, Some(0)); // "a"
}

#[test]
fn a_load_never_anchors_the_cursor_on_a_hidden_task() {
    // "a" is Completed and hidden, so the initial cursor lands on the first
    // visible Task, not index 0.
    let m = model_with(vec![
        task("a", Status::Completed),
        task("b", Status::NeedsAction),
    ]);
    assert_eq!(m.selected_task, Some(1)); // "b"
}

#[test]
fn completing_a_task_moves_the_cursor_off_it_while_hidden() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::NeedsAction),
    ]);
    assert_eq!(m.selected_task, Some(0)); // "a"
    let cmds = update(&mut m, ch(' ')); // complete "a"
    assert!(matches!(cmds.as_slice(), [Command::SetCompleted { .. }]));
    assert_eq!(m.selected_task, Some(1)); // cursor moved to visible "b"
    assert_eq!(titles(&m.visible_tasks()), vec!["b"]);
}

#[test]
fn revealing_then_hiding_reanchors_the_cursor() {
    let mut m = model_with(vec![
        task("a", Status::Completed),
        task("b", Status::NeedsAction),
    ]);
    update(&mut m, ch('c')); // reveal
    update(&mut m, key(KeyCode::Up)); // move onto "a" (index 0), now visible
    assert_eq!(m.selected_task, Some(0));
    update(&mut m, ch('c')); // hide again => "a" hidden
    assert_eq!(m.selected_task, Some(1)); // reanchored onto visible "b"
}

#[test]
fn capital_c_with_no_completed_is_a_noop_with_a_notice() {
    let mut m = model_with(vec![task("a", Status::NeedsAction)]);
    let cmds = update(&mut m, ch('C'));
    assert!(cmds.is_empty());
    assert!(m.overlay.is_none());
    assert!(m.status_line.is_some());
}

#[test]
fn capital_c_opens_the_clear_confirm_when_completed_present() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    match &m.overlay {
        Some(Overlay::Confirm(c)) => {
            assert!(matches!(c.action, ConfirmAction::ClearCompleted { .. }));
            assert!(c.prompt.contains('1')); // one completed task
        }
        other => panic!("expected a Clear confirm, got {other:?}"),
    }
}

#[test]
fn confirming_clear_optimistically_removes_completed_and_emits_the_command() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    let cmds = update(&mut m, ch('y'));
    assert_eq!(
        cmds,
        vec![Command::ClearCompleted {
            list: ListId("L".to_string())
        }]
    );
    // Completed Tasks are gone from the model immediately.
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a"]
    );
}

#[test]
fn a_failed_clear_rolls_the_completed_tasks_back() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // optimistic sweep
    assert_eq!(m.tasks.len(), 1);

    update(
        &mut m,
        Message::ClearCompletedFailed {
            list: ListId("L".to_string()),
            reason: "boom".to_string(),
        },
    );
    // "b" is back.
    assert_eq!(m.tasks.len(), 2);
    assert!(m.tasks.iter().any(|t| t.title == "b"));
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[test]
fn a_failed_clear_restores_interleaved_completed_in_original_order() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
        task("d", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b, d
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );
    update(
        &mut m,
        Message::ClearCompletedFailed {
            list: ListId("L".to_string()),
            reason: "boom".to_string(),
        },
    );
    // b and d land back at their original positions.
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "b", "c", "d"]
    );
}

#[test]
fn a_confirmed_clear_drops_the_snapshot_so_a_late_failure_cannot_restore() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    update(&mut m, ch('y'));
    update(
        &mut m,
        Message::ClearedCompleted(ListId("L".to_string())), // success
    );
    // A stray failure afterwards must not resurrect the swept Task.
    update(
        &mut m,
        Message::ClearCompletedFailed {
            list: ListId("L".to_string()),
            reason: "late".to_string(),
        },
    );
    assert_eq!(m.tasks.len(), 1);
}

#[test]
fn clear_is_single_flight() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    let first = update(&mut m, ch('y')); // first clear in flight
    assert_eq!(first.len(), 1);

    // A new completed Task appears (say, from a refresh) and the user tries to
    // Clear again before the first Clear has reported back.
    m.tasks.push(task("d", Status::Completed));
    update(&mut m, ch('C'));
    let second = update(&mut m, ch('y'));
    assert!(second.is_empty()); // guarded: a clear is already in flight
    assert!(m.status_line.is_some());
}

fn titles_of(m: &Model) -> Vec<String> {
    m.tasks.iter().map(|t| t.title.clone()).collect()
}

// ---- A Clear reply landing *before* a stale refresh (ticket #65) ----
//
// `C` sweeps the Completed rows optimistically. When `ClearedCompleted` lands
// first it drops the rollback snapshot, so a *stale* refresh — its fetch issued
// before Google swept — would resurrect them. Tombstoning the swept ids on the
// confirmation lets `set_tasks` drop them from that stale fetch, by id so a Task
// completed after the sweep is never taken with them.

#[test]
fn a_confirmed_clear_tombstones_a_stale_refresh_that_still_lists_the_swept_rows() {
    let all = vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
        task("d", Status::Completed),
    ];
    let mut m = model_with(all.clone());
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b, d
    update(&mut m, Message::ClearedCompleted(ListId("L".to_string()))); // confirmed first

    // The refresh still reports b and d: its fetch predated the Clear.
    update(&mut m, Message::TasksLoaded(ListId("L".to_string()), all));
    assert_eq!(titles_of(&m), vec!["a", "c"]);
}

#[test]
fn a_confirmed_clear_tombstone_keeps_tasks_completed_after_the_sweep() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b
    update(&mut m, Message::ClearedCompleted(ListId("L".to_string())));

    // The stale refresh brings back the swept "b" *and* an "e" completed after
    // the sweep — which this Clear never swept, so Google still has it.
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("L".to_string()),
            vec![
                task("a", Status::NeedsAction),
                task("b", Status::Completed),
                task("e", Status::Completed),
            ],
        ),
    );
    // Only the tombstoned ids go. Matching by status would have eaten "e".
    assert_eq!(titles_of(&m), vec!["a", "e"]);
}

// A tombstone is keyed by its List, so a fetch of *another* List must not evict
// it — otherwise switching away and back would reopen the race on the first List.
#[test]
fn a_tombstone_survives_a_fetch_of_another_list() {
    let other = List {
        id: ListId("M".to_string()),
        title: "M".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    };
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list(), other.clone()]));
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("L".to_string()),
            vec![
                task("a", Status::NeedsAction),
                task("b", Status::NeedsAction),
            ],
        ),
    );
    update(&mut m, key(KeyCode::Tab)); // focus task pane, cursor on "a"
    update(&mut m, ch('x'));
    update(&mut m, ch('y')); // optimistic delete of "a"
    update(&mut m, Message::TaskDeleted(TaskId("a".to_string()))); // tombstone under L
    assert_eq!(titles_of(&m), vec!["b"]);

    // The user switches to M and its Tasks load — L's tombstone must be untouched.
    update(&mut m, key(KeyCode::Tab)); // back to the sidebar
    update(&mut m, key(KeyCode::Down)); // select M
    let mut m_task = task("m1", Status::NeedsAction);
    m_task.list = other.id.clone();
    update(&mut m, Message::TasksLoaded(other.id.clone(), vec![m_task]));
    assert_eq!(titles_of(&m), vec!["m1"]);

    // Back on L, a stale in-flight fetch for L still lists "a": the tombstone
    // survived the M fetch, so the row is dropped again rather than resurrected.
    update(&mut m, key(KeyCode::Up)); // select L
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("L".to_string()),
            vec![
                task("a", Status::NeedsAction),
                task("b", Status::NeedsAction),
            ],
        ),
    );
    assert_eq!(titles_of(&m), vec!["b"]);
}

// ---- A refresh landing inside the Clear round-trip (ticket #51) ----
//
// `C` sweeps the Completed rows optimistically. A refresh in that window fetches
// a set Google has not yet cleared, and `set_tasks` puts them back. The success
// reply has to re-remove them — by snapshot id, so Tasks completed *after* the
// sweep are not swept along with them.

#[test]
fn a_refresh_mid_clear_is_undone_by_the_confirmed_clear() {
    let all = vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
        task("d", Status::Completed),
    ];
    let mut m = model_with(all.clone());
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b, d
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );

    // The refresh still reports b and d: Google has not processed the Clear.
    update(&mut m, Message::TasksLoaded(ListId("L".to_string()), all));
    assert_eq!(m.tasks.len(), 4); // resurrected

    update(&mut m, Message::ClearedCompleted(ListId("L".to_string())));
    // Swept again, and the Tasks that were never Completed kept their order.
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );
}

#[test]
fn a_refresh_mid_clear_steps_a_cursor_off_a_resurrected_swept_row() {
    let all = vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
        task("c", Status::NeedsAction),
    ];
    let mut m = model_with(all.clone());
    update(&mut m, ch('c')); // reveal Completed, so the cursor can sit on "b"
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b
    update(&mut m, Message::TasksLoaded(ListId("L".to_string()), all));
    assert_eq!(m.tasks.len(), 3); // resurrected
    update(&mut m, key(KeyCode::Down)); // the user parks the cursor on "b"
    assert_eq!(m.selected_task, Some(1));

    update(&mut m, Message::ClearedCompleted(ListId("L".to_string())));
    // The row under the cursor goes, so the cursor steps to the nearest row that
    // survives — not to the top of the pane.
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "c"]
    );
    assert_eq!(m.selected_task, Some(1)); // "c"
}

#[test]
fn a_confirmed_clear_keeps_tasks_completed_after_the_sweep() {
    let mut m = model_with(vec![
        task("a", Status::NeedsAction),
        task("b", Status::Completed),
    ]);
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b

    // The refresh brings back the swept "b" *and* an "e" completed after the
    // sweep — which this Clear never swept, so Google still has it.
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("L".to_string()),
            vec![
                task("a", Status::NeedsAction),
                task("b", Status::Completed),
                task("e", Status::Completed),
            ],
        ),
    );

    update(&mut m, Message::ClearedCompleted(ListId("L".to_string())));
    // Only the snapshot ids go. Re-removing by status would have eaten "e".
    assert_eq!(
        m.tasks.iter().map(|t| t.title.clone()).collect::<Vec<_>>(),
        vec!["a", "e"]
    );
}

#[test]
fn a_confirmed_clear_for_an_inactive_list_touches_nothing() {
    let other = List {
        id: ListId("M".to_string()),
        title: "M".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    };
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list(), other.clone()]));
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("L".to_string()),
            vec![task("a", Status::NeedsAction), task("b", Status::Completed)],
        ),
    );
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    update(&mut m, ch('C'));
    update(&mut m, ch('y')); // sweep b in list L

    // The user switches to M before the Clear reports back.
    update(&mut m, key(KeyCode::Tab)); // back to the sidebar
    update(&mut m, key(KeyCode::Down)); // select M
    let mut m_task = task("m1", Status::Completed);
    m_task.list = other.id.clone();
    update(&mut m, Message::TasksLoaded(other.id.clone(), vec![m_task]));
    let before = m.tasks.clone();

    update(&mut m, Message::ClearedCompleted(ListId("L".to_string())));
    // The reply belongs to a pane that is no longer on screen.
    assert_eq!(m.tasks, before);
}
