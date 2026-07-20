//! Reducer tests for the pinned Today cross-List view (#61). `update` is pure —
//! no terminal, no network. Today membership (`due <= today`) and ordering are
//! stamped by `model.now`, which these tests set to a fixed date.

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, SortView, Status, Task, TaskId};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// A fixed "today" so membership and overdue are deterministic.
const TODAY: (i32, u32, u32) = (2026, 7, 20);

fn today() -> NaiveDate {
    ymd(TODAY.0, TODAY.1, TODAY.2)
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.into()),
        title: id.to_uppercase(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn task(id: &str, list: &str, due: Option<NaiveDate>, status: Status) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId(list.into()),
        parent: None,
        title: id.into(),
        notes: None,
        status,
        due,
        completed_at: None,
        links: Vec::new(),
        position: id.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

/// A Model on Today, clock fixed to `today()`, with `lists` known (for name
/// resolution) and `tasks` handed straight in as the aggregate (as `TodayLoaded`
/// would). The task pane is focused.
fn today_model(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = lists.iter().map(|id| list(id)).collect();
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

fn visible_titles(m: &Model) -> Vec<String> {
    m.visible_tasks()
        .iter()
        .map(|t| t.display_title().to_string())
        .collect()
}

#[test]
fn today_loaded_names_failed_lists_on_the_status_line() {
    let mut m = Model::new();
    m.lists = vec![list("work"), list("home")];
    m.selected = Selection::Today;
    update(
        &mut m,
        Message::TodayLoaded {
            tasks: vec![task("a", "work", Some(today()), Status::NeedsAction)],
            failed: vec![ListId("home".into())],
        },
    );
    // The List title, not its id, and never silently dropped (fail closed).
    assert_eq!(m.status_line.as_deref(), Some("failed to load: HOME"));
}

#[test]
fn a_stale_today_load_is_ignored_once_a_list_is_selected() {
    let mut m = Model::new();
    m.lists = vec![list("work")];
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TodayLoaded {
            tasks: vec![task("a", "work", Some(today()), Status::NeedsAction)],
            failed: Vec::new(),
        },
    );
    assert!(
        m.tasks.is_empty(),
        "a Today aggregate must not fill a List pane"
    );
}

#[test]
fn membership_keeps_overdue_and_today_across_lists_drops_undated_and_future() {
    // All three entry types are eligible; only the due date decides membership.
    let m = today_model(
        &["work", "home"],
        vec![
            task(
                "overdue",
                "work",
                Some(ymd(2026, 7, 19)),
                Status::NeedsAction,
            ),
            task("due-today", "home", Some(today()), Status::NeedsAction),
            task("○ event", "work", Some(today()), Status::NeedsAction),
            task(
                "future",
                "home",
                Some(ymd(2026, 7, 21)),
                Status::NeedsAction,
            ),
            task("undated", "work", None, Status::NeedsAction),
        ],
    );
    let titles = visible_titles(&m);
    assert!(titles.contains(&"overdue".to_string()));
    assert!(titles.contains(&"due-today".to_string()));
    assert!(titles.contains(&"event".to_string())); // Event type, still due<=today
    assert!(!titles.contains(&"future".to_string()));
    assert!(!titles.contains(&"undated".to_string()));
}

#[test]
fn membership_does_not_leak_into_a_normal_list_pane() {
    // Regression guard: the `due <= today` filter is gated on Today. In a real
    // List a future-dated row stays visible AND navigable — `j` reaches it.
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = vec![list("work")];
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("work".into()),
            vec![
                task("now", "work", Some(today()), Status::NeedsAction),
                task(
                    "later",
                    "work",
                    Some(ymd(2026, 12, 25)),
                    Status::NeedsAction,
                ),
            ],
        ),
    );
    m.focus = Focus::Tasks;
    // Both rows visible (future not filtered outside Today).
    assert_eq!(visible_titles(&m), vec!["now", "later"]);
    // And navigable: from the first row, `j` reaches the future-dated one.
    update(&mut m, press('j'));
    let sel = m
        .selected_task
        .and_then(|i| m.tasks.get(i))
        .map(|t| &t.title);
    assert_eq!(sel.map(String::as_str), Some("later"));
}

#[test]
fn today_orders_flat_by_due_then_list_title_then_position() {
    // Same due date on two Lists → tie broken by List title (HOME before WORK).
    let m = today_model(
        &["work", "home"],
        vec![
            task("w-today", "work", Some(today()), Status::NeedsAction),
            task("h-today", "home", Some(today()), Status::NeedsAction),
            task(
                "overdue",
                "work",
                Some(ymd(2026, 7, 18)),
                Status::NeedsAction,
            ),
        ],
    );
    // Overdue first (earliest due), then the two due-today ordered by List title.
    assert_eq!(visible_titles(&m), vec!["overdue", "h-today", "w-today"]);
}

#[test]
fn s_in_today_cycles_due_and_title_only() {
    let mut m = today_model(&["work"], vec![]);
    assert_eq!(m.sort, SortView::Due);
    update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Title);
    update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Due); // never Manual
}

#[test]
fn entering_today_normalises_a_manual_lens_to_due() {
    // Today has no Manual order, so a Manual lens carried in from a List is
    // normalised to Due on entry — the pane order and the header label agree.
    let mut m = Model::new();
    m.lists = vec![list("work")];
    m.selected = Selection::List(0);
    m.sort = SortView::Manual;
    m.focus = Focus::Sidebar;
    update(&mut m, press('k')); // move the sidebar cursor up to the pinned Today row
    assert_eq!(m.selected, Selection::Today);
    assert_eq!(m.sort, SortView::Due);
}

#[test]
fn write_verbs_target_the_rows_own_list_not_the_selection() {
    // Today's selection is not a List (`selected_list_id()` is None), so a write
    // must read the focused row's own `list`.
    for (idx, want_list) in [(0usize, "work"), (1usize, "home")] {
        let mut m = today_model(
            &["work", "home"],
            vec![
                task("a", "work", Some(today()), Status::NeedsAction),
                task("b", "home", Some(today()), Status::NeedsAction),
            ],
        );
        m.selected_task = Some(idx);
        let cmds = update(&mut m, press(' ')); // toggle complete
        assert_eq!(
            cmds,
            vec![Command::SetCompleted {
                list: ListId(want_list.into()),
                task: m.tasks[idx].id.clone(),
                completed: true,
            }]
        );
    }
}

#[test]
fn migrate_in_today_drops_the_row_and_reanchors_the_cursor() {
    let mut m = today_model(
        &["work"],
        vec![
            task("a", "work", Some(today()), Status::NeedsAction),
            task("b", "work", Some(today()), Status::NeedsAction),
        ],
    );
    m.selected_task = Some(0); // "a"
    let cmds = update(&mut m, press('m'));
    // Pushed to tomorrow → out of the due<=today set → gone from view.
    assert!(!visible_titles(&m).contains(&"a".to_string()));
    // Cursor re-anchored onto a still-visible row rather than a hidden one.
    let sel = m
        .selected_task
        .and_then(|i| m.tasks.get(i))
        .map(|t| &t.title);
    assert_eq!(sel.map(String::as_str), Some("b"));
    // The write went to the row's own List, with tomorrow's date.
    assert_eq!(
        cmds,
        vec![Command::SetDue {
            list: ListId("work".into()),
            task: TaskId("a".into()),
            due: Some(ymd(2026, 7, 21)),
        }]
    );
}

#[test]
fn a_captures_into_the_resolved_default_with_due_today() {
    let mut m = today_model(&["work", "home"], vec![]);
    m.default_list = Some(ListId("home".into())); // resolved @default
    update(&mut m, press('a'));
    for c in "call the dentist".chars() {
        update(&mut m, press(c));
    }
    let cmds = update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
    );
    // Optimistic placeholder is on the default List, dated today, so it stays in view.
    assert_eq!(m.tasks[0].list, ListId("home".into()));
    assert_eq!(m.tasks[0].due, Some(today()));
    assert!(visible_titles(&m).contains(&"call the dentist".to_string()));
    match cmds.as_slice() {
        [Command::AddTask { list, due, .. }] => {
            assert_eq!(list, &ListId("home".into()));
            assert_eq!(due, &Some(today()));
        }
        other => panic!("expected one AddTask, got {other:?}"),
    }
}

#[test]
fn a_today_capture_honours_an_explicit_trailing_date() {
    // A trailing date parsed off the title (#80) wins over the today default, so
    // an entry the user scheduled for later correctly leaves the Today set.
    let mut m = today_model(&["home"], vec![]);
    m.default_list = Some(ListId("home".into()));
    update(&mut m, press('a'));
    for c in "call bob tomorrow".chars() {
        update(&mut m, press(c));
    }
    let cmds = update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())),
    );
    let tomorrow = ymd(2026, 7, 21);
    match cmds.as_slice() {
        [Command::AddTask { title, due, .. }] => {
            assert_eq!(title, "call bob");
            assert_eq!(due, &Some(tomorrow));
        }
        other => panic!("expected one AddTask, got {other:?}"),
    }
    // Scheduled for tomorrow → it is not part of today's view.
    assert!(!visible_titles(&m).contains(&"call bob".to_string()));
}

#[test]
fn a_is_refused_when_the_default_is_unresolved() {
    let mut m = today_model(&["work"], vec![]);
    m.default_list = None; // offline / not yet resolved
    update(&mut m, press('a'));
    assert!(m.overlay.is_none(), "no capture overlay without a target");
    assert!(
        m.status_line.is_some(),
        "the refusal is explained (fail closed)"
    );
}

#[test]
fn clear_completed_in_today_sweeps_each_contributing_list() {
    let mut m = today_model(
        &["work", "home"],
        vec![
            task("a", "work", Some(today()), Status::Completed),
            task("b", "home", Some(today()), Status::Completed),
            task("c", "work", Some(today()), Status::NeedsAction),
        ],
    );
    update(&mut m, press('C')); // opens the confirm overlay
    assert!(m.overlay.is_some());
    let cmds = update(&mut m, press('y')); // confirm
                                           // One ClearCompleted per contributing List, in first-seen order.
    assert_eq!(
        cmds,
        vec![
            Command::ClearCompleted {
                list: ListId("work".into())
            },
            Command::ClearCompleted {
                list: ListId("home".into())
            },
        ]
    );
    // The completed rows are optimistically gone; the incomplete one stays.
    let ids: Vec<&str> = m.tasks.iter().map(|t| t.id.0.as_str()).collect();
    assert_eq!(ids, vec!["c"]);
}

#[test]
fn moves_and_add_subtask_are_disabled_in_today() {
    let mut m = today_model(
        &["work"],
        vec![task("a", "work", Some(today()), Status::NeedsAction)],
    );
    m.selected_task = Some(0);
    for key in ['J', 'K', '>', '<', 'o'] {
        let cmds = update(&mut m, press(key));
        assert!(cmds.is_empty(), "{key} must not act in Today");
        assert!(m.status_line.is_some(), "{key} explains why it no-ops");
        m.status_line = None;
    }
}
