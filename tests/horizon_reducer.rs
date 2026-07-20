//! Reducer tests for the distant-due view filter (`hide_distant`): the horizon
//! predicate, the `w` toggle with cursor re-anchoring, and the parent/Subtask
//! and entry-type decisions from the design. `update` is pure, so these run with
//! no terminal and no network; every case pins `model.now` so the horizon is
//! measured against a fixed today, not the wall clock.

use chrono::{Local, NaiveDate, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};

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

/// A top-level Task with the given title, due date, and status.
fn task(title: &str, due: Option<NaiveDate>, status: Status) -> Task {
    Task {
        id: TaskId(title.to_string()),
        list: ListId("L".to_string()),
        parent: None,
        title: title.to_string(),
        notes: None,
        status,
        due,
        completed_at: match status {
            Status::Completed => Some(Utc.timestamp_opt(1_700_000_100, 0).unwrap()),
            Status::NeedsAction => None,
        },
        links: Vec::new(),
        position: format!("{title:0>20}"),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

/// A needsAction top-level Task — the common case.
fn open(title: &str, due: Option<NaiveDate>) -> Task {
    task(title, due, Status::NeedsAction)
}

/// A Subtask of `parent`, needsAction.
fn subtask(title: &str, parent: &str, due: Option<NaiveDate>) -> Task {
    Task {
        parent: Some(TaskId(parent.to_string())),
        ..open(title, due)
    }
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A focused task pane seeded with `tasks`, its clock pinned to 2026-03-01, and
/// the distant-due filter armed with the default 14-day horizon (still off).
fn model_with(tasks: Vec<Task>) -> Model {
    let l = list();
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 3, 1, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    m
}

fn titles(tasks: &[&Task]) -> Vec<String> {
    tasks.iter().map(|t| t.title.clone()).collect()
}

/// today = 2026-03-01, so today + 14 = 2026-03-15 and today + 15 = 2026-03-16.
#[test]
fn off_by_default_hides_nothing_distant() {
    let m = model_with(vec![
        open("near", Some(ymd(2026, 3, 3))),
        open("far", Some(ymd(2026, 6, 1))),
    ]);
    assert!(!m.hide_distant);
    assert_eq!(titles(&m.visible_tasks()), vec!["near", "far"]);
}

#[test]
fn on_hides_beyond_horizon_keeps_near_undated_and_overdue() {
    let mut m = model_with(vec![
        open("overdue", Some(ymd(2026, 2, 20))),
        open("near", Some(ymd(2026, 3, 10))),
        open("undated", None),
        open("far", Some(ymd(2026, 6, 1))),
    ]);
    m.hide_distant = true;
    // Order follows the Due lens: overdue, near, then undated (undated sorts last).
    assert_eq!(
        titles(&m.visible_tasks()),
        vec!["overdue", "near", "undated"]
    );
}

#[test]
fn horizon_boundary_is_inclusive() {
    let mut m = model_with(vec![
        open("day14", Some(ymd(2026, 3, 15))), // exactly today + 14: visible
        open("day15", Some(ymd(2026, 3, 16))), // today + 15: hidden
    ]);
    m.hide_distant = true;
    assert_eq!(titles(&m.visible_tasks()), vec!["day14"]);
}

#[test]
fn horizon_days_sets_the_cutoff() {
    let mut m = model_with(vec![
        open("day5", Some(ymd(2026, 3, 6))),   // today + 5
        open("day10", Some(ymd(2026, 3, 11))), // today + 10
    ]);
    m.hide_distant = true;
    m.horizon_days = 7;
    assert_eq!(titles(&m.visible_tasks()), vec!["day5"]);
}

#[test]
fn w_toggles_and_reanchors_the_cursor_off_a_hidden_row() {
    let mut m = model_with(vec![
        open("near", Some(ymd(2026, 3, 3))),
        open("far", Some(ymd(2026, 6, 1))),
    ]);
    // Park the cursor on the row `w` is about to hide.
    m.selected_task = m.tasks.iter().position(|t| t.title == "far");

    update(&mut m, ch('w'));
    assert!(m.hide_distant);
    assert_eq!(titles(&m.visible_tasks()), vec!["near"]);
    // Cursor must not sit on the now-hidden "far".
    let selected = m.selected_task.map(|i| m.tasks[i].title.clone());
    assert_eq!(selected.as_deref(), Some("near"));

    // Toggling back reveals it and leaves the cursor valid.
    update(&mut m, ch('w'));
    assert!(!m.hide_distant);
    assert_eq!(titles(&m.visible_tasks()), vec!["near", "far"]);
}

#[test]
fn completed_and_horizon_filters_compose() {
    let mut m = model_with(vec![
        open("open_near", Some(ymd(2026, 3, 3))),
        task("done_near", Some(ymd(2026, 3, 3)), Status::Completed),
        open("open_far", Some(ymd(2026, 6, 1))),
        task("done_far", Some(ymd(2026, 6, 1)), Status::Completed),
    ]);
    m.hide_distant = true;
    // Completed hidden: only the near open Task survives both filters.
    assert_eq!(titles(&m.visible_tasks()), vec!["open_near"]);

    m.show_completed = true;
    // Completed revealed, but the horizon still hides both far rows.
    assert_eq!(titles(&m.visible_tasks()), vec!["open_near", "done_near"]);
}

#[test]
fn a_within_horizon_subtask_survives_a_distant_hidden_parent_and_surfaces_high() {
    // Distant parent (hidden), near Subtask (visible), and a standalone Task due
    // later than the Subtask but still within the horizon. `group_due_key` keys
    // the parent's group on the near Subtask, so it sorts above the standalone.
    let mut m = model_with(vec![
        open("project", Some(ymd(2026, 6, 1))), // parent, distant
        subtask("step", "project", Some(ymd(2026, 3, 3))), // near step
        open("errand", Some(ymd(2026, 3, 10))), // later, still within horizon
    ]);
    m.hide_distant = true;
    // Parent gone; the near step sorts ahead of the later errand.
    assert_eq!(titles(&m.visible_tasks()), vec!["step", "errand"]);
}

#[test]
fn a_distant_subtask_is_hidden_under_a_within_horizon_parent() {
    let mut m = model_with(vec![
        open("project", Some(ymd(2026, 3, 3))), // parent, near
        subtask("step", "project", Some(ymd(2026, 6, 1))), // distant step
    ]);
    m.hide_distant = true;
    assert_eq!(titles(&m.visible_tasks()), vec!["project"]);
}

#[test]
fn a_distant_event_is_hidden_like_a_task() {
    // The filter keys on the due date, not the entry type, so a far-future Event
    // hides the same as a far-future Task.
    let mut m = model_with(vec![
        open("near", Some(ymd(2026, 3, 3))),
        open("○ party", Some(ymd(2026, 6, 1))),
    ]);
    m.hide_distant = true;
    assert_eq!(titles(&m.visible_tasks()), vec!["near"]);
}
