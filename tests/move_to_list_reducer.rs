//! Reducer tests for "move to list" — the cross-List Move (`M`). `update` is
//! pure, so the races this feature has to survive are expressed directly, by
//! delivering messages in the hostile order rather than simulating timing.

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Focus, Message, Model, Overlay};
use oxidone::domain::{List, ListId, Selection, SortView, Status, Task, TaskId};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

const TODAY: (i32, u32, u32) = (2026, 7, 20);

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).expect("valid date")
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.into()),
        title: id.to_uppercase(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn task(id: &str, list: &str) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId(list.into()),
        parent: None,
        title: id.into(),
        notes: None,
        status: Status::NeedsAction,
        due: Some(today()),
        completed_at: None,
        links: Vec::new(),
        position: id.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn base(lists: &[&str]) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = lists.iter().map(|id| list(id)).collect();
    m.focus = Focus::Tasks;
    m.api_available = true;
    m
}

/// A Model showing List `a`'s pane, with `tasks` loaded and the cursor on the
/// first row. Manual, so nothing here depends on a sort lens.
fn list_model(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = base(lists);
    m.selected = Selection::List(0);
    m.sort = SortView::Manual;
    update(&mut m, Message::TasksLoaded(ListId("a".into()), tasks));
    m.focus = Focus::Tasks;
    m
}

/// A Model on the pinned Today aggregate.
fn today_model(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = base(lists);
    m.selected = Selection::Today;
    update(
        &mut m,
        Message::TodayLoaded {
            tasks,
            failed: Vec::new(),
        },
    );
    m.focus = Focus::Tasks;
    m
}

fn ids(m: &Model) -> Vec<String> {
    m.tasks.iter().map(|t| t.id.0.clone()).collect()
}

fn targets(m: &Model) -> Vec<String> {
    match m.overlay.as_ref() {
        Some(Overlay::MoveToList { targets, .. }) => {
            targets.iter().map(|l| l.id.0.clone()).collect()
        }
        other => panic!("expected the move-to-list picker, got {other:?}"),
    }
}

/// Open the picker on the selected row and confirm the first candidate.
fn move_selected(m: &mut Model) -> Vec<Command> {
    update(m, press('M'));
    update(m, key(KeyCode::Enter))
}

// ---- The picker ----

#[test]
fn the_picker_offers_every_list_but_the_tasks_own() {
    let mut m = list_model(&["a", "b", "c"], vec![task("t1", "a")]);
    update(&mut m, press('M'));
    assert_eq!(targets(&m), ["b", "c"]);
}

#[test]
fn in_today_the_picker_excludes_the_rows_own_list_not_the_selected_one() {
    // The row lives in `b`, while the "selected list" is Today (none). Excluding
    // by selection would offer `b` — a move to where it already is.
    let mut m = today_model(&["a", "b", "c"], vec![task("t1", "b")]);
    update(&mut m, press('M'));
    assert_eq!(targets(&m), ["a", "c"]);
}

#[test]
fn a_single_list_leaves_nothing_to_pick() {
    let mut m = list_model(&["a"], vec![task("t1", "a")]);
    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(m.status_line.as_deref(), Some("no other list to move to"));
}

#[test]
fn a_placeholder_list_is_never_a_destination() {
    let mut m = list_model(&["a", "temp-list-0"], vec![task("t1", "a")]);
    update(&mut m, press('M'));
    // The only other List has no id on Google yet, so there is nothing to pick.
    assert!(m.overlay.is_none());
    assert_eq!(m.status_line.as_deref(), Some("no other list to move to"));
}

#[test]
fn enter_moves_the_task_captured_when_the_picker_opened() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), task("t2", "a")]);
    update(&mut m, press('M'));
    // A fetch lands while the picker is open, shifting every index.
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("a".into()),
            vec![task("t0", "a"), task("t1", "a"), task("t2", "a")],
        ),
    );
    let cmds = update(&mut m, key(KeyCode::Enter));
    // Still `t1` — the id was captured at open time, not read off an index now.
    assert_eq!(
        cmds,
        vec![Command::MoveToList {
            source: ListId("a".into()),
            task: TaskId("t1".into()),
            destination: ListId("b".into()),
        }]
    );
}

#[test]
fn enter_on_a_task_that_has_since_left_the_pane_refuses_out_loud() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    update(&mut m, press('M'));
    // Completed and Cleared elsewhere, deleted on another device, …
    update(&mut m, Message::TasksLoaded(ListId("a".into()), Vec::new()));
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("that task is no longer here")
    );
}

#[test]
fn esc_cancels_without_writing() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    update(&mut m, press('M'));
    let cmds = update(&mut m, key(KeyCode::Esc));
    assert!(cmds.is_empty());
    assert!(m.overlay.is_none());
    assert_eq!(ids(&m), ["t1"]);
}

// ---- Refusals ----

#[test]
fn a_task_with_subtasks_is_refused() {
    let mut child = task("c1", "a");
    child.parent = Some(TaskId("t1".into()));
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), child]);
    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("can't move a task with subtasks to another list")
    );
}

#[test]
fn a_task_still_being_added_cannot_be_moved() {
    // `a` captures a title; the placeholder carries a `temp-N` id until the
    // server replies. Moving it would 404 rather than fail closed.
    let mut m = list_model(&["a", "b"], Vec::new());
    update(&mut m, press('a'));
    update(&mut m, press('x'));
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks.len(), 1, "placeholder inserted optimistically");

    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("still saving — try again in a moment")
    );
}

#[test]
fn a_task_in_a_not_yet_created_list_is_covered_by_the_placeholder_guard() {
    // A Task added into a List that does not exist on Google yet is itself a
    // placeholder — its insert cannot have been confirmed — so the `temp-N` Task
    // guard catches it and no separate source-List check is needed.
    let mut m = base(&["a"]);
    m.selected = Selection::List(0);
    m.sort = SortView::Manual;
    let mut placeholder = task("temp-7", "temp-list-0");
    placeholder.title = "in a pending list".into();
    m.tasks = vec![placeholder];
    m.selected_task = Some(0);

    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("still saving — try again in a moment")
    );
}

#[test]
fn m_is_refused_while_an_in_list_move_is_in_flight() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), task("t2", "a")]);
    // `J` starts an in-list Move and parks a whole-pane rollback snapshot.
    let cmds = update(&mut m, press('J'));
    assert!(matches!(cmds.as_slice(), [Command::Move { .. }]));

    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("a move is already in progress")
    );
}

#[test]
fn m_is_refused_while_a_field_write_is_in_flight() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    m.show_completed = true; // keep the row selectable once it is ticked off
    update(&mut m, press(' ')); // completion toggle, now pending
    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
    assert_eq!(
        m.status_line.as_deref(),
        Some("a write is already in progress for this task")
    );
}

// ---- The optimistic path ----

#[test]
fn the_row_leaves_the_pane_and_the_cursor_follows_its_successor() {
    let mut m = list_model(
        &["a", "b"],
        vec![task("t1", "a"), task("t2", "a"), task("t3", "a")],
    );
    m.selected_task = Some(1); // cursor on the row being moved
    let cmds = move_selected(&mut m);

    assert_eq!(
        cmds,
        vec![Command::MoveToList {
            source: ListId("a".into()),
            task: TaskId("t2".into()),
            destination: ListId("b".into()),
        }]
    );
    assert_eq!(ids(&m), ["t1", "t3"]);
    // The successor in display order, not index 0 and not the row above.
    assert_eq!(m.selected_task, Some(1));
    assert_eq!(m.tasks[1].id, TaskId("t3".into()));
}

#[test]
fn a_cursor_moved_off_the_row_while_the_picker_was_open_is_left_alone() {
    // `M` always targets the selected row, so "the cursor is elsewhere" can only
    // arise asynchronously: a fetch lands while the picker is open and re-anchors
    // the selection. Then the moved row is not the selected one, and the cursor
    // must not be dragged to its successor.
    let mut m = list_model(
        &["a", "b"],
        vec![task("t1", "a"), task("t2", "a"), task("t3", "a")],
    );
    m.selected_task = Some(0); // picker opens on t1
    update(&mut m, press('M'));

    // A refresh drops t1 from the fetch, so the cursor re-anchors onto t2.
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t2", "a"), task("t3", "a")]),
    );
    // …but the row is suppressed anyway, so `Enter` finds nothing to move.
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert!(cmds.is_empty());
    assert_eq!(
        m.status_line.as_deref(),
        Some("that task is no longer here")
    );
    assert_eq!(m.tasks[m.selected_task.unwrap()].id, TaskId("t2".into()));
}

// ---- The provisional tombstone ----

#[test]
fn a_refresh_landing_mid_move_cannot_put_the_row_back() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), task("t2", "a")]);
    move_selected(&mut m);
    assert_eq!(ids(&m), ["t2"]);

    // A fetch issued before the move, still listing the row. The permanent
    // tombstone is not written until success, so only the provisional one can
    // suppress this.
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t1", "a"), task("t2", "a")]),
    );
    // Asserted *before* the reply lands: checking only the final state would
    // pass with no suppression at all.
    assert_eq!(ids(&m), ["t2"], "suppressed while the move is in flight");

    update(&mut m, Message::MovedToList(task("t1", "b")));
    assert_eq!(ids(&m), ["t2"]);
}

#[test]
fn suppression_is_scoped_to_the_source_list() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    move_selected(&mut m);

    // Switch to the destination while the move is in flight, and let its fetch
    // land. The row is arriving there legitimately and must not be stripped.
    m.selected = Selection::List(1);
    update(
        &mut m,
        Message::TasksLoaded(ListId("b".into()), vec![task("t1", "b")]),
    );
    assert_eq!(ids(&m), ["t1"]);
}

#[test]
fn an_in_list_move_failing_mid_move_cannot_resurrect_the_row() {
    let mut m = list_model(
        &["a", "b"],
        vec![task("t1", "a"), task("t2", "a"), task("t3", "a")],
    );
    move_selected(&mut m); // t1 leaves
    assert_eq!(ids(&m), ["t2", "t3"]);

    // An in-list Move on another row is allowed during a cross-List move; its
    // snapshot is taken now, so it post-dates the removal.
    update(&mut m, press('J'));
    update(
        &mut m,
        Message::MoveFailed {
            list: ListId("a".into()),
            reason: "nope".into(),
        },
    );
    assert_eq!(
        ids(&m),
        ["t2", "t3"],
        "the wholesale restore has no t1 in it"
    );
}

#[test]
fn an_in_list_move_key_does_nothing_while_the_picker_is_open() {
    // The `pending_move` guard fires at `M`, but the removal happens at `Enter`.
    // Nothing may start an in-list Move in between — `overlay_key` routes every
    // key to the picker.
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), task("t2", "a")]);
    update(&mut m, press('M'));
    let cmds = update(&mut m, press('J'));
    assert!(
        cmds.is_empty(),
        "no Move may be issued from inside the picker"
    );
    assert!(matches!(m.overlay, Some(Overlay::MoveToList { .. })));
}

#[test]
fn the_row_cannot_be_reselected_while_its_move_is_in_flight() {
    // Which is *why* the single-flight guard in `open_move_to_list` is belt to the
    // suppression's braces rather than the thing doing the work: a row that never
    // comes back on screen cannot be picked a second time.
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    move_selected(&mut m);
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t1", "a")]),
    );
    assert!(m.tasks.is_empty(), "still suppressed");
    // Nothing selectable, so `M` finds no Task and stays silent.
    update(&mut m, press('M'));
    assert!(m.overlay.is_none());
}

// ---- Success ----

#[test]
fn a_move_from_a_list_pane_leaves_the_row_gone_and_tombstones_it() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a"), task("t2", "a")]);
    move_selected(&mut m);
    let cmds = update(&mut m, Message::MovedToList(task("t1", "b")));
    assert!(cmds.is_empty(), "a List pane needs no reload");
    assert_eq!(ids(&m), ["t2"]);

    // The permanent tombstone has taken over from the provisional one: a stale
    // fetch of the source still cannot resurrect it.
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t1", "a"), task("t2", "a")]),
    );
    assert_eq!(ids(&m), ["t2"]);
}

#[test]
fn a_move_from_today_puts_the_row_back_and_asks_for_a_reload() {
    let mut m = today_model(&["a", "b"], vec![task("t1", "a"), task("t2", "b")]);
    m.selected_task = Some(0);
    move_selected(&mut m);
    assert_eq!(ids(&m), ["t2"]);

    let cmds = update(&mut m, Message::MovedToList(task("t1", "b")));
    // Restored at its snapshotted index — asserted here, before the reload's own
    // TodayLoaded re-derives the order from the cache.
    assert_eq!(ids(&m), ["t1", "t2"]);
    assert_eq!(m.tasks[0].list, ListId("b".into()));
    assert_eq!(
        cmds,
        vec![Command::LoadToday {
            lists: m.lists.clone(),
            today: today(),
        }]
    );
}

#[test]
fn moving_a_task_out_and_back_does_not_lose_it() {
    // A→B leaves a tombstone under A. Moving back must evict it, or `set_tasks`
    // drops the row from its own List for the rest of the session.
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    move_selected(&mut m);
    update(&mut m, Message::MovedToList(task("t1", "b")));

    // Now in B, and moved home.
    m.selected = Selection::List(1);
    update(
        &mut m,
        Message::TasksLoaded(ListId("b".into()), vec![task("t1", "b")]),
    );
    m.focus = Focus::Tasks;
    m.selected_task = Some(0);
    move_selected(&mut m);
    update(&mut m, Message::MovedToList(task("t1", "a")));

    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t1", "a")]),
    );
    assert_eq!(ids(&m), ["t1"], "the round trip must not swallow the row");
}

// ---- Failure ----

#[test]
fn a_failed_move_restores_the_row_at_its_index_with_the_reason() {
    let mut m = list_model(
        &["a", "b"],
        vec![task("t1", "a"), task("t2", "a"), task("t3", "a")],
    );
    m.selected_task = Some(1);
    move_selected(&mut m);
    assert_eq!(ids(&m), ["t1", "t3"]);

    update(
        &mut m,
        Message::MoveToListFailed {
            task: TaskId("t2".into()),
            reason: "can't move a task with subtasks to another list".into(),
        },
    );
    assert_eq!(ids(&m), ["t1", "t2", "t3"]);
    assert_eq!(
        m.status_line.as_deref(),
        Some("can't move a task with subtasks to another list")
    );
}

#[test]
fn a_failed_move_always_clears_its_bookkeeping_even_from_another_pane() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    move_selected(&mut m);

    // Switch away, so the failure has no pane of ours to repair.
    m.selected = Selection::List(1);
    update(
        &mut m,
        Message::MoveToListFailed {
            task: TaskId("t1".into()),
            reason: "network error".into(),
        },
    );

    // Back home: the row must not still be suppressed, and a second `M` must be
    // accepted — a latched entry would do both for the rest of the session.
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(ListId("a".into()), vec![task("t1", "a")]),
    );
    assert_eq!(ids(&m), ["t1"], "no longer suppressed");

    m.focus = Focus::Tasks;
    m.selected_task = Some(0);
    update(&mut m, press('M'));
    assert!(
        matches!(m.overlay, Some(Overlay::MoveToList { .. })),
        "single-flight guard must have been released"
    );
}

#[test]
fn a_failure_for_a_move_we_never_made_changes_nothing_but_the_status_line() {
    let mut m = list_model(&["a", "b"], vec![task("t1", "a")]);
    update(
        &mut m,
        Message::MoveToListFailed {
            task: TaskId("ghost".into()),
            reason: "stray".into(),
        },
    );
    assert_eq!(ids(&m), ["t1"]);
    assert_eq!(m.status_line.as_deref(), Some("stray"));
}
