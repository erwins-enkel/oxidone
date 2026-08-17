//! The Weekly spread's write lifecycle, at the reducer boundary: the `SetDue` a
//! dot emits, the optimistic update and its rollback, and the cross-List Move
//! (`M`) — whose repair is what `PaneKey::Week` exists for.
//!
//! `update` is pure, so the races are expressed by delivering messages in the
//! hostile order rather than by simulating timing, as `move_to_list_reducer.rs`
//! does.

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Focus, Message, Model, Overlay};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

/// A fixed "today": **Monday** 17 August 2026.
const TODAY: (i32, u32, u32) = (2026, 8, 17);

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).expect("valid date")
}

fn day(n: i64) -> Option<NaiveDate> {
    Some(today() + chrono::Duration::days(n))
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.into()),
        title: id.to_uppercase(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn task(id: &str, list: &str, due: Option<NaiveDate>) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId(list.into()),
        parent: None,
        title: id.into(),
        notes: None,
        status: Status::NeedsAction,
        due,
        completed_at: None,
        links: Vec::new(),
        position: id.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

/// A Model in the spread over Lists `w` (the pool) and `h`.
fn in_week(tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = vec![list("w"), list("h")];
    m.selected = Selection::List(0);
    update(&mut m, press('W'));
    update(
        &mut m,
        Message::WeekLoaded {
            tasks,
            failed: Vec::new(),
            live: true,
        },
    );
    m.focus = Focus::Tasks;
    m
}

/// The same, on the pinned **Week** row: the week across every List, reached by
/// `W` from Today the way a user reaches it.
fn in_week_row(tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = vec![list("w"), list("h")];
    m.selected = Selection::Today;
    update(&mut m, press('W'));
    assert_eq!(m.selected, Selection::Week, "`W` on Today lands on the row");
    update(
        &mut m,
        Message::WeekLoaded {
            tasks,
            failed: Vec::new(),
            live: true,
        },
    );
    m.focus = Focus::Tasks;
    m
}

fn select(m: &mut Model, id: &str) {
    let at = m
        .tasks
        .iter()
        .position(|t| t.id == TaskId(id.into()))
        .unwrap_or_else(|| panic!("{id} is not in the corpus"));
    m.selected_task = Some(at);
}

fn visible(m: &Model) -> Vec<String> {
    m.visible_tasks().iter().map(|t| t.id.0.clone()).collect()
}

fn due_of(m: &Model, id: &str) -> Option<NaiveDate> {
    m.tasks
        .iter()
        .find(|t| t.id == TaskId(id.into()))
        .expect("task is in the corpus")
        .due
}

/// Pick `target` in the open move-to-List picker and fire it.
fn choose_list(m: &mut Model, target: &str) -> Vec<Command> {
    let at = match &m.overlay {
        Some(Overlay::MoveToList { targets, .. }) => targets
            .iter()
            .position(|t| t.id == ListId(target.into()))
            .unwrap_or_else(|| panic!("{target} is not a target")),
        other => panic!("expected the picker, got {other:?}"),
    };
    for _ in 0..at {
        update(m, key(KeyCode::Down));
    }
    update(m, key(KeyCode::Enter))
}

// --- The dot's write -------------------------------------------------------

/// A dot rides the same optimistic `SetDue` path `d` and `m` do, against the
/// row's *own* List — the spread spans Lists, so the pool's is not the target.
#[test]
fn a_dot_writes_set_due_against_the_rows_own_list() {
    let mut m = in_week(vec![task("foreign", "h", None)]);
    select(&mut m, "foreign");

    let commands = update(&mut m, press('3'));

    assert_eq!(
        commands,
        vec![Command::SetDue {
            list: ListId("h".into()),
            task: TaskId("foreign".into()),
            due: day(2),
        }]
    );
    assert_eq!(
        due_of(&m, "foreign"),
        day(2),
        "optimistically, before the reply"
    );
}

/// On failure the optimistic date rolls back and the row returns to the pool.
#[test]
fn a_failed_write_rolls_the_dot_back() {
    let mut m = in_week(vec![task("a", "w", None)]);
    select(&mut m, "a");
    update(&mut m, press('3'));
    assert_eq!(due_of(&m, "a"), day(2));

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: TaskId("a".into()),
            reason: "nope".to_string(),
        },
    );

    assert_eq!(due_of(&m, "a"), None, "back in the pool");
    assert_eq!(m.status_line.as_deref(), Some("nope"));
}

/// The single-flight guard is inherited from the shared write path: a second dot
/// on a row already mid-write is refused rather than racing it.
#[test]
fn a_second_dot_while_a_write_is_in_flight_is_refused() {
    let mut m = in_week(vec![task("a", "w", None)]);
    select(&mut m, "a");
    update(&mut m, press('3'));

    m.status_line = None;
    let commands = update(&mut m, press('5'));

    assert!(commands.is_empty());
    assert_eq!(due_of(&m, "a"), day(2), "the first write still stands");
    assert!(m.status_line.is_some());
}

// --- `M`, and why `PaneKey::Week` exists -----------------------------------

/// On the pinned Week row, a **scheduled** row relocated to another List stays on
/// screen: the week half of `within_week` spans every List there, so its new home
/// changes nothing about its membership. Without a `PaneKey::Week` arm the repair
/// would not fire and the row — optimistically removed by `finish_move_to_list` —
/// would silently vanish.
#[test]
fn a_relocated_scheduled_row_is_bridged_back_and_reloads() {
    let mut m = in_week_row(vec![task("ship", "w", day(2))]);
    select(&mut m, "ship");

    update(&mut m, press('M'));
    choose_list(&mut m, "h");
    assert_eq!(visible(&m), Vec::<String>::new(), "removed optimistically");

    let commands = update(&mut m, Message::MovedToList(task("ship", "h", day(2))));

    assert_eq!(visible(&m), ["ship"], "bridged back into the spread");
    assert_eq!(
        commands,
        vec![Command::LoadWeek {
            lists: m.lists.clone()
        }],
        "and re-asked, so the row returns under its new List"
    );
}

/// A **pool** row relocated away leaves the spread — its new List is not the pool
/// List. The bridge re-insert is unconditional and `within_week` decides, which
/// is already the right rule; nothing special-cases this.
#[test]
fn a_relocated_pool_row_leaves_the_spread() {
    let mut m = in_week(vec![task("jot", "w", None)]);
    select(&mut m, "jot");

    update(&mut m, press('M'));
    choose_list(&mut m, "h");
    update(&mut m, Message::MovedToList(task("jot", "h", None)));

    assert!(
        m.tasks.iter().any(|t| t.id == TaskId("jot".into())),
        "still in the corpus"
    );
    assert_eq!(visible(&m), Vec::<String>::new(), "but not in this pool");
}

/// A failed `M` restores the row only into the pane it was removed from. After
/// leaving the spread for the scope List's own pane, the snapshot must **not**
/// match — which is exactly what a shared `PaneKey::List(scope)` identity would
/// have got wrong, putting a spread row back into a single-List pane that has since
/// been reloaded from Google.
#[test]
fn a_failed_move_does_not_restore_into_the_scope_lists_own_pane() {
    let mut m = in_week(vec![task("ship", "w", day(1))]);
    select(&mut m, "ship");

    update(&mut m, press('M'));
    choose_list(&mut m, "h");

    // Leave the spread; `selected` still names List `w`, whose pane now loads.
    update(&mut m, press('W'));
    assert!(!m.week_active());
    update(
        &mut m,
        Message::TasksLoaded(ListId("w".into()), vec![task("local", "w", None)]),
    );

    update(
        &mut m,
        Message::MoveToListFailed {
            task: TaskId("ship".into()),
            reason: "nope".to_string(),
        },
    );

    assert_eq!(
        m.tasks.iter().map(|t| t.id.0.clone()).collect::<Vec<_>>(),
        ["local"],
        "no spread row restored into List w's pane"
    );
}
