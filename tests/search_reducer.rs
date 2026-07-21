//! Reducer tests for the cross-List Search pane (`S`). `update` is pure — no
//! terminal, no network — so the corpus is delivered as `Message::SearchLoaded`
//! exactly as the runtime's cache paint and fan-out would. These pin the pane's
//! behaviour and every regression the coverage contract names.

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

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

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

/// A full Task builder: id, home List, due, status.
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

/// An undated needsAction Task.
fn undated(id: &str, list: &str) -> Task {
    task(id, list, None, Status::NeedsAction)
}

/// A base Model, clock pinned to `today()`, `lists` known, online, task pane
/// focused. `selected` is left at its default (Today); callers set it.
fn base(lists: &[&str]) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = lists.iter().map(|id| list(id)).collect();
    m.api_available = true;
    m.focus = Focus::Tasks;
    m
}

/// Enter Search from the current `selected`, then deliver the cache paint
/// (`live: false`) with `corpus`, as the runtime does. Returns the commands the
/// `S` press emitted.
fn enter_search(m: &mut Model, corpus: Vec<Task>) -> Vec<Command> {
    let cmds = update(m, press('S'));
    update(
        m,
        Message::SearchLoaded {
            tasks: corpus,
            failed: Vec::new(),
            live: false,
        },
    );
    cmds
}

/// Commit the open query input (`Enter`), leaving Search with the input closed.
fn commit(m: &mut Model) {
    update(m, key(KeyCode::Enter));
}

/// Type each character into the model in order.
fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        update(m, press(c));
    }
}

fn visible_ids(m: &Model) -> Vec<String> {
    m.visible_tasks().iter().map(|t| t.id.0.clone()).collect()
}

// ---- Entry, the corpus, and the Today aliasing ----

#[test]
fn s_emits_loadsearch_and_enters_the_pane() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(0);
    let cmds = update(&mut m, press('S'));
    assert!(m.search_active(), "S enters Search");
    assert!(
        m.search_pending,
        "the corpus is pending until the live fan-out"
    );
    assert_eq!(m.focus, Focus::Tasks, "S focuses the task pane");
    assert!(
        matches!(m.overlay, Some(Overlay::Filter)),
        "the input opens"
    );
    assert_eq!(m.filter.as_deref(), Some(""), "over an empty query");
    assert_eq!(
        cmds,
        vec![Command::LoadSearch {
            lists: m.lists.clone()
        }]
    );
}

#[test]
fn entering_from_today_shows_the_whole_corpus_not_the_day() {
    // The aliasing regression: opened from Today, `selected` stays Today, but the
    // corpus must not be filtered to `due <= today`.
    let mut m = base(&["work", "home"]);
    m.selected = Selection::Today;
    enter_search(
        &mut m,
        vec![
            task(
                "overdue",
                "work",
                Some(ymd(2026, 7, 19)),
                Status::NeedsAction,
            ),
            undated("undated", "home"),
            task(
                "future",
                "work",
                Some(ymd(2026, 12, 1)),
                Status::NeedsAction,
            ),
        ],
    );
    assert!(!m.today_active(), "Search from Today is not Today");
    let mut ids = visible_ids(&m);
    ids.sort();
    assert_eq!(ids, vec!["future", "overdue", "undated"]);
}

#[test]
fn c_alone_governs_completed_in_search_including_earlier_days() {
    // A row completed on an earlier day is hidden in Today, but Search only obeys
    // `show_completed`, so `c` reveals it.
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    let mut done = task("done", "work", Some(ymd(2026, 7, 10)), Status::Completed);
    done.completed_at = Some(
        Local
            .with_ymd_and_hms(2026, 7, 10, 9, 0, 0)
            .unwrap()
            .to_utc(),
    );
    enter_search(&mut m, vec![undated("open", "work"), done]);
    commit(&mut m);
    assert_eq!(visible_ids(&m), vec!["open"], "Completed hidden by default");
    update(&mut m, press('c'));
    let mut ids = visible_ids(&m);
    ids.sort();
    assert_eq!(
        ids,
        vec!["done", "open"],
        "c reveals the earlier-day Completion"
    );
}

#[test]
fn empty_query_shows_everything_and_typing_narrows_live() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(
        &mut m,
        vec![undated("file tax", "work"), undated("groceries", "work")],
    );
    assert_eq!(visible_ids(&m).len(), 2, "empty query matches all");
    typed(&mut m, "tax");
    assert_eq!(visible_ids(&m), vec!["file tax"], "narrows per keystroke");
}

// ---- Leaving, Esc precedence, focus ----

#[test]
fn esc_from_the_open_input_restores_the_prior_pane() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(1);
    enter_search(&mut m, vec![undated("a", "work")]);
    let cmds = update(&mut m, key(KeyCode::Esc));
    assert!(!m.search_active(), "Esc leaves Search");
    assert!(m.filter.is_none(), "the query is dropped");
    assert_eq!(
        m.selected,
        Selection::List(1),
        "the sidebar cursor never moved"
    );
    assert_eq!(cmds, vec![Command::LoadTasks(ListId("home".into()))]);
}

#[test]
fn esc_exits_in_one_press_after_enter_closed_the_input() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    assert!(m.overlay.is_none(), "Enter closed the input");
    update(&mut m, key(KeyCode::Esc));
    assert!(!m.search_active(), "one Esc still exits");
}

#[test]
fn esc_with_the_cheatsheet_open_closes_it_and_stays_in_search() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, press('?')); // open the cheatsheet
    assert!(m.show_help);
    update(&mut m, key(KeyCode::Esc));
    assert!(!m.show_help, "Esc closes the cheatsheet first");
    assert!(m.search_active(), "and stays in Search");
}

#[test]
fn a_second_s_reopens_the_input_over_the_query_without_reloading() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("tax", "work")]);
    typed(&mut m, "tax");
    commit(&mut m);
    let corpus_before = m.tasks.clone();
    let cmds = update(&mut m, press('S'));
    assert!(cmds.is_empty(), "no fresh LoadSearch on a second S");
    assert_eq!(m.filter.as_deref(), Some("tax"), "the typed query is kept");
    assert!(
        matches!(m.overlay, Some(Overlay::Filter)),
        "the input reopens"
    );
    assert_eq!(m.tasks, corpus_before, "the corpus is untouched");
}

#[test]
fn while_the_input_is_open_j_types_into_the_query() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work")]);
    update(&mut m, press('j'));
    assert_eq!(
        m.filter.as_deref(),
        Some("j"),
        "j is a query character here"
    );
}

#[test]
fn entering_search_clears_the_inherited_cursor() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(ListId("work".into()), vec![undated("a", "work")]),
    );
    m.selected_task = Some(0);
    update(&mut m, press('S'));
    assert_eq!(
        m.selected_task, None,
        "the parked-pane index is cleared on entry"
    );
}

// ---- Sidebar navigation ----

#[test]
fn jk_with_the_sidebar_focused_leaves_search_and_loads_the_list() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, key(KeyCode::Tab)); // focus the sidebar
    let cmds = update(&mut m, press('j')); // Today -> work
    assert!(!m.search_active(), "a genuine sidebar move leaves Search");
    assert_eq!(m.selected, Selection::List(0));
    assert_eq!(cmds, vec![Command::LoadTasks(ListId("work".into()))]);
}

#[test]
fn jk_at_the_sidebar_edge_is_inert_in_search() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today; // slot 0, the top edge
    enter_search(&mut m, vec![undated("a", "work")]);
    typed(&mut m, "a");
    commit(&mut m);
    update(&mut m, key(KeyCode::Tab)); // focus the sidebar
    let cmds = update(&mut m, press('k')); // up from the pinned Today row: no move
    assert!(
        m.search_active(),
        "a clamped edge press must not drop the corpus"
    );
    assert!(cmds.is_empty());
    assert_eq!(m.filter.as_deref(), Some("a"), "the query survives");
    assert_eq!(m.tasks.len(), 1, "and so does the corpus");
}

#[test]
fn tab_only_changes_focus_and_keeps_search() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, key(KeyCode::Tab));
    assert!(m.search_active(), "Tab keeps Search");
    assert_eq!(m.focus, Focus::Sidebar);
}

// ---- The pending notice ----

#[test]
fn the_pending_notice_holds_until_the_live_send() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]); // cache paint: live=false
    assert!(m.search_pending, "still pending after the cache paint");
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("a", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    assert!(!m.search_pending, "the live send clears it");
}

#[test]
fn an_offline_live_send_clears_the_pending_notice() {
    // Offline, `main.rs` sends the cache read with `live: true`, so the notice
    // never sticks when there is no fan-out to wait for.
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    update(&mut m, press('S'));
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("a", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    assert!(
        !m.search_pending,
        "an offline live send cannot leave it pending"
    );
}

#[test]
fn the_pending_notice_survives_a_status_line_refusal() {
    // The regression that would return if the notice lived on `status_line`:
    // a refusal (`w`) landing between the cache paint and the live send.
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work")]); // live=false, pending
    commit(&mut m);
    update(&mut m, press('w')); // refused, writes the status line
    assert!(m.status_line.is_some(), "the refusal spoke");
    assert!(m.search_pending, "but the pending notice is untouched");
}

#[test]
fn leaving_search_clears_the_pending_notice() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work")]);
    update(&mut m, key(KeyCode::Esc));
    assert!(!m.search_pending, "no stale notice outlives the pane");
}

// ---- Corpus suppressions ----

#[test]
fn a_tombstoned_id_never_appears_in_the_corpus_and_is_not_evicted() {
    // Delete a Task in a List, then Search: the stale corpus still carries the id,
    // but the tombstone drops it — and stays held, since the reducer never sees the
    // per-List omission eviction is defined against.
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("work".into()),
            vec![undated("keep", "work"), undated("gone", "work")],
        ),
    );
    m.selected_task = Some(1);
    update(&mut m, press('x')); // opens the delete confirm
    update(&mut m, press('y')); // confirm: optimistic removal + pending delete
    update(&mut m, Message::TaskDeleted(TaskId("gone".into()))); // tombstones it
                                                                 // The cache paint still lists the stale id; it must be suppressed on both routes.
    enter_search(
        &mut m,
        vec![undated("keep", "work"), undated("gone", "work")],
    );
    assert_eq!(
        m.tasks.iter().filter(|t| t.id.0 == "gone").count(),
        0,
        "cache paint drops it"
    );
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("keep", "work"), undated("gone", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    assert_eq!(
        m.tasks.iter().filter(|t| t.id.0 == "gone").count(),
        0,
        "the live send drops it too"
    );
}

// ---- Staleness guards ----

#[test]
fn a_tasksloaded_during_search_does_not_replace_the_corpus() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work"), undated("b", "home")]);
    update(
        &mut m,
        Message::TasksLoaded(ListId("work".into()), vec![undated("only-work", "work")]),
    );
    assert_eq!(
        m.tasks.len(),
        2,
        "a per-List fetch cannot overwrite the corpus"
    );
}

#[test]
fn a_todayloaded_during_search_does_not_replace_the_corpus() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    enter_search(&mut m, vec![undated("a", "work"), undated("b", "work")]);
    update(
        &mut m,
        Message::TodayLoaded {
            tasks: vec![undated("a", "work")],
            failed: Vec::new(),
        },
    );
    assert_eq!(
        m.tasks.len(),
        2,
        "a stale Today aggregate cannot overwrite the corpus"
    );
}

#[test]
fn a_searchloaded_after_esc_does_not_overwrite_the_restored_pane() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(ListId("work".into()), vec![undated("real", "work")]),
    );
    enter_search(&mut m, vec![undated("corpus", "work")]);
    update(&mut m, key(KeyCode::Esc)); // back to the List pane
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("late", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    assert!(
        !m.tasks.iter().any(|t| t.id.0 == "late"),
        "a late corpus is ignored"
    );
}

#[test]
fn an_in_list_move_reply_during_search_leaves_the_corpus() {
    // Press J (in-list reorder) on a List, then S before the reply lands.
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(0);
    m.sort = SortView::Manual;
    update(
        &mut m,
        Message::TasksLoaded(
            ListId("work".into()),
            vec![undated("t1", "work"), undated("t2", "work")],
        ),
    );
    m.selected_task = Some(0);
    let moved = update(&mut m, press('J'));
    assert!(!moved.is_empty(), "the reorder is in flight");
    enter_search(
        &mut m,
        vec![
            undated("t1", "work"),
            undated("t2", "work"),
            undated("other", "home"),
        ],
    );
    // The success reply reconciles a single List; in Search it must not touch the corpus.
    update(
        &mut m,
        Message::MoveSucceeded {
            list: ListId("work".into()),
            tasks: vec![undated("t2", "work"), undated("t1", "work")],
        },
    );
    assert_eq!(
        m.tasks.len(),
        3,
        "MoveSucceeded cannot overwrite the corpus in Search"
    );
}

// ---- Refusals ----

#[test]
fn manual_moves_subtasks_and_clear_are_refused_with_a_message() {
    for k in ['J', 'K', '>', '<', 'o', 'C'] {
        let mut m = base(&["work"]);
        m.selected = Selection::List(0);
        enter_search(&mut m, vec![undated("a", "work")]);
        commit(&mut m);
        m.selected_task = Some(0);
        let cmds = update(&mut m, press(k));
        assert!(m.status_line.is_some(), "{k} should refuse with a message");
        assert!(cmds.is_empty(), "{k} must emit no command");
        assert_eq!(m.tasks.len(), 1, "{k} must not mutate the corpus");
    }
}

#[test]
fn w_is_refused_and_leaves_hide_distant_unchanged() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    m.hide_distant = false;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, press('w'));
    assert!(
        !m.hide_distant,
        "w must not flip the session-persistent flag"
    );
    assert!(m.status_line.is_some(), "and it says why");
}

#[test]
fn search_is_exempt_from_hide_distant() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    m.hide_distant = true; // horizon on, carried in from a List
    enter_search(
        &mut m,
        vec![
            undated("near", "work"),
            task("far", "work", Some(ymd(2027, 1, 1)), Status::NeedsAction),
        ],
    );
    let mut ids = visible_ids(&m);
    ids.sort();
    assert_eq!(
        ids,
        vec!["far", "near"],
        "a far-future match is still found"
    );
}

// ---- Add, refresh, ordering ----

#[test]
fn a_captures_undated_into_the_default_list() {
    let mut m = base(&["work", "inbox"]);
    m.selected = Selection::Today;
    m.default_list = Some(ListId("inbox".into()));
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, press('a'));
    typed(&mut m, "groceries");
    update(&mut m, key(KeyCode::Enter));
    let placeholder = m
        .tasks
        .iter()
        .find(|t| t.title == "groceries")
        .expect("captured");
    assert_eq!(
        placeholder.list,
        ListId("inbox".into()),
        "into the default List"
    );
    assert_eq!(
        placeholder.due, None,
        "undated — Search has no membership to preserve"
    );
}

#[test]
fn a_fails_closed_when_the_default_list_is_unresolved() {
    let mut m = base(&["work"]);
    m.selected = Selection::Today;
    m.default_list = None;
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    update(&mut m, press('a'));
    assert!(
        m.overlay.is_none(),
        "no capture overlay opens without a target"
    );
    assert!(m.status_line.is_some());
}

#[test]
fn refresh_in_search_keeps_query_and_corpus_with_one_loadsearch() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work"), undated("b", "home")]);
    typed(&mut m, "a");
    commit(&mut m);
    update(&mut m, press('r')); // -> RefreshLists -> (worker) ListsLoaded
    let same_lists = m.lists.clone();
    let cmds = update(&mut m, Message::ListsLoaded(same_lists));
    assert!(m.search_active(), "r stays in Search");
    assert_eq!(m.filter.as_deref(), Some("a"), "and keeps the query");
    assert_eq!(m.tasks.len(), 2, "and the corpus");
    assert_eq!(
        cmds.iter()
            .filter(|c| matches!(c, Command::LoadSearch { .. }))
            .count(),
        1,
        "exactly one LoadSearch"
    );
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::LoadTasks(_))),
        "and no per-List fan-out: {cmds:?}"
    );
}

#[test]
fn refresh_survives_the_parked_list_disappearing() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::List(1); // parked on "home"
    enter_search(&mut m, vec![undated("a", "work"), undated("b", "home")]);
    typed(&mut m, "a");
    commit(&mut m);
    update(&mut m, press('r'));
    // "home" is gone server-side; ListsLoaded returns only "work".
    let cmds = update(&mut m, Message::ListsLoaded(vec![list("work")]));
    assert!(m.search_active(), "still in Search");
    assert_eq!(m.filter.as_deref(), Some("a"), "query kept");
    assert_eq!(
        m.tasks.len(),
        2,
        "corpus kept — the target_changed regression"
    );
    assert!(cmds.iter().any(|c| matches!(c, Command::LoadSearch { .. })));
}

#[test]
fn overdue_rows_form_a_contiguous_prefix() {
    let mut m = base(&["work", "home"]);
    m.selected = Selection::Today;
    enter_search(
        &mut m,
        vec![
            undated("z-undated", "home"),
            task(
                "b-overdue",
                "work",
                Some(ymd(2026, 7, 18)),
                Status::NeedsAction,
            ),
            task("a-today", "home", Some(today()), Status::NeedsAction),
            task(
                "c-overdue",
                "home",
                Some(ymd(2026, 7, 19)),
                Status::NeedsAction,
            ),
        ],
    );
    let ids = visible_ids(&m);
    // Overdue first (contiguous), then dated ascending, then undated last.
    assert_eq!(ids, vec!["b-overdue", "c-overdue", "a-today", "z-undated"]);
}

#[test]
fn s_cycles_due_and_title_only_never_manual() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work")]);
    commit(&mut m);
    assert_eq!(m.sort, SortView::Due, "Search opens on Due");
    update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Title);
    update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Due, "cycles back to Due, never Manual");
}

// ---- The accessor and the sidebar meter ----

#[test]
fn the_parked_list_meter_reports_cached_counts_not_the_corpus() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    // Seed the cached sidebar count for "work": 1 of 3.
    update(
        &mut m,
        Message::CountsLoaded(
            [(ListId("work".into()), (1usize, 3usize))]
                .into_iter()
                .collect(),
        ),
    );
    // A corpus far larger than that count.
    enter_search(
        &mut m,
        vec![
            undated("a", "work"),
            undated("b", "work"),
            undated("c", "work"),
            undated("d", "work"),
        ],
    );
    assert_eq!(
        m.list_meter(&ListId("work".into())),
        Some((1, 3)),
        "the meter must not count the whole corpus against the parked List"
    );
}

#[test]
fn selected_list_id_is_none_in_search() {
    let mut m = base(&["work"]);
    m.selected = Selection::List(0);
    enter_search(&mut m, vec![undated("a", "work")]);
    assert_eq!(m.selected_list_id(), None, "Search is not a List");
}
