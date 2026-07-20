//! Reducer tests for Subtasks & reorder (ticket #11) — the Move operations.
//! `update` is pure: the hierarchy, indent/outdent, reorder, and add-subtask all
//! run with no terminal and no network. The optimistic Move manipulates `tasks`
//! and reconciles via a refetch (`MoveSucceeded`); failure rolls back.

use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Message, Model, Overlay};
use oxidone::domain::{List, ListId, Status, Task, TaskId};

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

fn tid(s: &str) -> TaskId {
    TaskId(s.to_string())
}

fn list() -> List {
    List {
        id: ListId("L".to_string()),
        title: "L".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn task(id: &str, parent: Option<&str>) -> Task {
    Task {
        id: tid(id),
        list: ListId("L".to_string()),
        parent: parent.map(tid),
        title: id.to_string(),
        notes: None,
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        position: format!("{id:0>20}"),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

/// A focused task pane (Manual sort) seeded with `tasks` in the given Vec order.
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

fn lid() -> ListId {
    ListId("L".to_string())
}

// ---- rendering the hierarchy -------------------------------------------------

#[test]
fn subtasks_render_grouped_under_their_parent_regardless_of_vec_order() {
    // Children are NOT contiguous with parents in the Vec (as with Google's
    // per-sibling positions); the display still groups them correctly.
    let m = model_with(vec![
        task("A", None),
        task("B", None),
        task("a1", Some("A")),
        task("b1", Some("B")),
    ]);
    assert_eq!(titles(&m.visible_tasks()), vec!["A", "a1", "B", "b1"]);
}

#[test]
fn navigation_follows_the_displayed_hierarchy_not_the_vec() {
    let mut m = model_with(vec![
        task("A", None),
        task("B", None),
        task("a1", Some("A")), // Vec index 2, but displays right after A
    ]);
    // display order: A, a1, B
    assert_eq!(
        m.selected_task.map(|i| m.tasks[i].id.clone()),
        Some(tid("A"))
    );
    update(&mut m, key(KeyCode::Down));
    assert_eq!(
        m.selected_task.map(|i| m.tasks[i].id.clone()),
        Some(tid("a1"))
    );
    update(&mut m, key(KeyCode::Down));
    assert_eq!(
        m.selected_task.map(|i| m.tasks[i].id.clone()),
        Some(tid("B"))
    );
}

// ---- add subtask -------------------------------------------------------------

#[test]
fn o_adds_a_subtask_under_the_selected_top_level_task() {
    let mut m = model_with(vec![task("a", None)]);
    update(&mut m, ch('o'));
    match &m.overlay {
        Some(Overlay::AddSubtask { parent, .. }) => assert_eq!(parent, &tid("a")),
        other => panic!("expected AddSubtask overlay, got {other:?}"),
    }
    typed(&mut m, "step");
    let cmds = update(&mut m, key(KeyCode::Enter));
    // Placeholder inserted as a child of "a", cursor on it.
    assert_eq!(m.tasks[1].parent, Some(tid("a")));
    assert_eq!(m.tasks[1].title, "step");
    assert_eq!(m.selected_task, Some(1));
    assert_eq!(
        cmds,
        vec![Command::AddTask {
            list: lid(),
            temp: tid("temp-0"),
            title: "step".to_string(),
            parent: Some(tid("a")),
        }]
    );
}

#[test]
fn o_on_a_subtask_adds_a_sibling_under_the_same_parent() {
    // One-level cap: a Subtask can't own Subtasks, so `o` targets its parent.
    let mut m = model_with(vec![task("a", None), task("s", Some("a"))]);
    update(&mut m, key(KeyCode::Down)); // select "s"
    update(&mut m, ch('o'));
    match &m.overlay {
        Some(Overlay::AddSubtask { parent, .. }) => assert_eq!(parent, &tid("a")),
        other => panic!("expected AddSubtask under 'a', got {other:?}"),
    }
}

// ---- indent ------------------------------------------------------------------

#[test]
fn indent_makes_the_task_a_subtask_of_the_previous_top_level() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    update(&mut m, key(KeyCode::Down)); // select "b"
    let cmds = update(&mut m, ch('>'));
    assert_eq!(m.tasks[1].parent, Some(tid("a"))); // optimistic
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("b"),
            parent: Some(tid("a")),
            previous: None, // "a" had no children yet
        }]
    );
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "b"]); // b now nested under a
}

#[test]
fn indent_lands_after_the_parents_existing_last_child() {
    let mut m = model_with(vec![task("a", None), task("c", Some("a")), task("b", None)]);
    update(&mut m, key(KeyCode::Down)); // display: a, c, b — Down lands on c? no: a->c->b
                                        // Move selection to "b" (the third displayed row).
    update(&mut m, key(KeyCode::Down));
    assert_eq!(
        m.selected_task.map(|i| m.tasks[i].id.clone()),
        Some(tid("b"))
    );
    let cmds = update(&mut m, ch('>'));
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("b"),
            parent: Some(tid("a")),
            previous: Some(tid("c")), // after a's current last child
        }]
    );
}

#[test]
fn indent_is_rejected_for_an_already_nested_subtask() {
    let mut m = model_with(vec![task("a", None), task("s", Some("a"))]);
    update(&mut m, key(KeyCode::Down)); // select "s"
    let cmds = update(&mut m, ch('>'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
    assert_eq!(m.tasks[1].parent, Some(tid("a"))); // unchanged
}

#[test]
fn indent_is_rejected_for_a_task_that_has_subtasks() {
    let mut m = model_with(vec![task("x", None), task("a", None), task("c", Some("a"))]);
    update(&mut m, key(KeyCode::Down)); // select "a" (has child c)
    let cmds = update(&mut m, ch('>'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
}

#[test]
fn indent_is_rejected_with_no_previous_top_level() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    // "a" is the first task — nothing to indent under.
    let cmds = update(&mut m, ch('>'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
}

// ---- outdent -----------------------------------------------------------------

#[test]
fn outdent_promotes_a_subtask_to_top_level_after_its_parent() {
    let mut m = model_with(vec![task("a", None), task("b", Some("a"))]);
    update(&mut m, key(KeyCode::Down)); // select "b"
    let cmds = update(&mut m, ch('<'));
    assert_eq!(m.tasks[1].parent, None); // optimistic
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("b"),
            parent: None,
            previous: Some(tid("a")),
        }]
    );
}

#[test]
fn outdent_is_a_noop_on_a_top_level_task() {
    let mut m = model_with(vec![task("a", None)]);
    let cmds = update(&mut m, ch('<'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
}

// ---- reorder -----------------------------------------------------------------

#[test]
fn move_down_swaps_with_the_next_sibling() {
    let mut m = model_with(vec![task("a", None), task("b", None), task("c", None)]);
    let cmds = update(&mut m, ch('J')); // move "a" down
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a", "c"]);
    assert_eq!(
        m.selected_task.map(|i| m.tasks[i].id.clone()),
        Some(tid("a"))
    ); // cursor follows
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("a"),
            parent: None,
            previous: Some(tid("b")),
        }]
    );
}

#[test]
fn move_up_swaps_with_the_previous_sibling() {
    let mut m = model_with(vec![task("a", None), task("b", None), task("c", None)]);
    update(&mut m, key(KeyCode::Down));
    update(&mut m, key(KeyCode::Down)); // select "c"
    let cmds = update(&mut m, ch('K')); // move "c" up
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "c", "b"]);
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("c"),
            parent: None,
            previous: Some(tid("a")), // lands after a (before b)
        }]
    );
}

#[test]
fn move_up_to_first_has_no_previous() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    update(&mut m, key(KeyCode::Down)); // select "b"
    let cmds = update(&mut m, ch('K'));
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a"]);
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("b"),
            parent: None,
            previous: None,
        }]
    );
}

#[test]
fn reorder_is_a_noop_at_the_ends() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    let up = update(&mut m, ch('K')); // "a" already first
    assert!(up.is_empty());
    update(&mut m, key(KeyCode::Down)); // "b"
    let down = update(&mut m, ch('J')); // "b" already last
    assert!(down.is_empty());
}

#[test]
fn reorder_among_subtasks_stays_within_the_parent() {
    let mut m = model_with(vec![
        task("p", None),
        task("s1", Some("p")),
        task("s2", Some("p")),
    ]);
    update(&mut m, key(KeyCode::Down)); // select "s1"
    let cmds = update(&mut m, ch('J'));
    assert_eq!(titles(&m.visible_tasks()), vec!["p", "s2", "s1"]);
    assert_eq!(
        cmds,
        vec![Command::Move {
            list: lid(),
            task: tid("s1"),
            parent: Some(tid("p")),
            previous: Some(tid("s2")),
        }]
    );
}

#[test]
fn reordering_a_parent_carries_its_subtree() {
    let mut m = model_with(vec![
        task("A", None),
        task("a1", Some("A")),
        task("B", None),
        task("b1", Some("B")),
    ]);
    let _ = update(&mut m, ch('J')); // move "A" down past "B"
    assert_eq!(titles(&m.visible_tasks()), vec!["B", "b1", "A", "a1"]);
}

// ---- optimism: single-flight, rollback, reconcile ----------------------------

#[test]
fn a_second_move_while_one_is_in_flight_is_guarded() {
    let mut m = model_with(vec![task("a", None), task("b", None), task("c", None)]);
    let first = update(&mut m, ch('J')); // move in flight
    assert_eq!(first.len(), 1);
    let second = update(&mut m, ch('J'));
    assert!(second.is_empty());
    assert!(m.status_line.is_some());
}

#[test]
fn a_move_is_blocked_while_the_task_has_a_field_write_in_flight() {
    // The Move reconcile replaces the pane wholesale, so it must not race an
    // optimistic field edit still in flight on the same Task.
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    update(&mut m, ch('d')); // edit due on "a"
    typed(&mut m, "2026-01-01");
    update(&mut m, key(KeyCode::Enter)); // write in flight for "a"
    let cmds = update(&mut m, ch('J'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
}

#[test]
fn a_failed_move_rolls_back_to_the_prior_order() {
    let mut m = model_with(vec![task("a", None), task("b", None), task("c", None)]);
    update(&mut m, ch('J')); // optimistic: b, a, c
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a", "c"]);
    update(
        &mut m,
        Message::MoveFailed {
            list: lid(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "b", "c"]); // restored
    assert_eq!(m.status_line.as_deref(), Some("boom"));
    // A fresh move is possible again (the single-flight lock was released).
    assert_eq!(update(&mut m, ch('J')).len(), 1);
}

#[test]
fn a_successful_move_reconciles_to_the_authoritative_order() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    update(&mut m, ch('J'));
    // Server confirms with the (possibly re-normalised) order.
    update(
        &mut m,
        Message::MoveSucceeded {
            list: lid(),
            tasks: vec![task("b", None), task("a", None)],
        },
    );
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a"]);
    // Snapshot dropped: a later stray failure can't resurrect the old order.
    update(
        &mut m,
        Message::MoveFailed {
            list: lid(),
            reason: "late".to_string(),
        },
    );
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a"]);
}

// ---- sort views are read-only ------------------------------------------------

#[test]
fn moves_are_rejected_under_a_sort_view() {
    let mut m = model_with(vec![task("a", None), task("b", None)]);
    update(&mut m, ch('s')); // Manual -> Due
    let cmds = update(&mut m, ch('J'));
    assert!(cmds.is_empty());
    assert!(m.status_line.is_some());
    assert_eq!(m.tasks[0].id, tid("a")); // Manual order untouched
}
