//! Reducer tests for the title/notes filter (`/`): the `matches_filter`
//! predicate, opening/typing/committing/clearing the input, cursor re-anchoring,
//! the clear-on-List-switch (vs keep-on-Refresh) seam, and composition with the
//! other view filters. `update` is pure, so these run with no terminal and no
//! network.

use chrono::{Local, NaiveDate, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn list_id(id: &str) -> List {
    List {
        id: ListId(id.to_string()),
        title: id.to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

/// A top-level needsAction Task in list `L` with the given title and notes.
fn task_in(list: &str, title: &str, notes: Option<&str>) -> Task {
    Task {
        id: TaskId(title.to_string()),
        list: ListId(list.to_string()),
        parent: None,
        title: title.to_string(),
        notes: notes.map(str::to_string),
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        links: Vec::new(),
        position: format!("{title:0>20}"),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn open(title: &str) -> Task {
    task_in("L", title, None)
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A focused task pane on List "L" seeded with `tasks`, clock pinned to
/// 2026-03-01.
fn model_with(tasks: Vec<Task>) -> Model {
    let l = list_id("L");
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

/// A `Ctrl`-chord: CONTROL alone.
fn chord(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// AltGr, as a Windows console reports it: CONTROL and ALT together.
fn altgr(c: char) -> Message {
    Message::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ))
}

/// Type each character of `s` into the model in order.
fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        update(m, ch(c));
    }
}

#[test]
fn title_substring_narrows_case_insensitively() {
    let mut m = model_with(vec![open("Buy milk"), open("Read book"), open("Milk run")]);
    update(&mut m, ch('/'));
    typed(&mut m, "MILK");
    assert_eq!(titles(&m.visible_tasks()), vec!["Buy milk", "Milk run"]);
}

#[test]
fn matches_the_notes_body_not_only_the_title() {
    let mut m = model_with(vec![
        task_in("L", "Errand", Some("pick up dry cleaning")),
        open("Other"),
    ]);
    update(&mut m, ch('/'));
    typed(&mut m, "cleaning");
    assert_eq!(titles(&m.visible_tasks()), vec!["Errand"]);
}

#[test]
fn no_match_empties_the_pane() {
    let mut m = model_with(vec![open("alpha"), open("beta")]);
    update(&mut m, ch('/'));
    typed(&mut m, "zzz");
    assert!(m.visible_tasks().is_empty());
}

#[test]
fn an_empty_query_matches_everything() {
    let mut m = model_with(vec![open("alpha"), open("beta")]);
    update(&mut m, ch('/')); // opened, nothing typed yet
    assert_eq!(m.filter.as_deref(), Some(""));
    assert_eq!(titles(&m.visible_tasks()), vec!["alpha", "beta"]);
}

#[test]
fn slash_focuses_tasks_opens_the_input_and_narrows_live() {
    let mut m = model_with(vec![open("report"), open("other")]);
    m.selected = Selection::List(0);
    update(&mut m, key(KeyCode::Tab)); // to sidebar
    update(&mut m, ch('/'));
    // Focus jumps to the task pane; the input is open.
    assert_eq!(m.focus, Focus::Tasks);
    assert!(m.filter.is_some());
    typed(&mut m, "rep");
    assert_eq!(titles(&m.visible_tasks()), vec!["report"]);
    // Cursor sits on a surviving row, never the filtered-out "other".
    let selected = m.selected_task.map(|i| m.tasks[i].title.clone());
    assert_eq!(selected.as_deref(), Some("report"));
}

#[test]
fn enter_keeps_the_filter_and_closes_the_input() {
    let mut m = model_with(vec![open("report"), open("other")]);
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    update(&mut m, key(KeyCode::Enter));
    // Filter persists, pane stays narrowed, and further keys are the normal keymap
    // again (the overlay is closed). "j" now moves the cursor rather than typing.
    assert_eq!(m.filter.as_deref(), Some("rep"));
    assert_eq!(titles(&m.visible_tasks()), vec!["report"]);
    update(&mut m, ch('j'));
    assert_eq!(m.filter.as_deref(), Some("rep")); // 'j' navigated, did not type
}

#[test]
fn enter_on_an_empty_query_clears_the_filter() {
    let mut m = model_with(vec![open("alpha")]);
    update(&mut m, ch('/'));
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.filter, None);
    assert_eq!(titles(&m.visible_tasks()), vec!["alpha"]);
}

#[test]
fn esc_while_editing_clears_and_restores_the_pane() {
    let mut m = model_with(vec![open("report"), open("other")]);
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    update(&mut m, key(KeyCode::Esc));
    assert_eq!(m.filter, None);
    assert_eq!(titles(&m.visible_tasks()), vec!["report", "other"]);
}

#[test]
fn esc_on_a_persisted_filter_clears_it() {
    let mut m = model_with(vec![open("report"), open("other")]);
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    update(&mut m, key(KeyCode::Enter)); // committed, input closed
    assert_eq!(m.filter.as_deref(), Some("rep"));
    update(&mut m, key(KeyCode::Esc)); // no overlay open: clears the filter
    assert_eq!(m.filter, None);
    assert_eq!(titles(&m.visible_tasks()), vec!["report", "other"]);
}

#[test]
fn backspace_widens_the_filter() {
    let mut m = model_with(vec![open("report"), open("read")]);
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    assert_eq!(titles(&m.visible_tasks()), vec!["report"]);
    update(&mut m, key(KeyCode::Backspace)); // "re"
    assert_eq!(titles(&m.visible_tasks()), vec!["report", "read"]);
}

#[test]
fn typing_reanchors_the_cursor_off_a_filtered_row() {
    let mut m = model_with(vec![open("alpha"), open("report")]);
    // Park the cursor on the row the query is about to hide.
    m.selected_task = m.tasks.iter().position(|t| t.title == "alpha");
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    let selected = m.selected_task.map(|i| m.tasks[i].title.clone());
    assert_eq!(selected.as_deref(), Some("report"));
}

#[test]
fn switching_list_clears_the_filter() {
    // Two Lists; a filter typed on the first must not follow to the second.
    let (a, b) = (list_id("A"), list_id("B"));
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 3, 1, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, Message::ListsLoaded(vec![a.clone(), b.clone()]));
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(a.id.clone(), vec![task_in("A", "aa", None)]),
    );
    update(&mut m, ch('/'));
    typed(&mut m, "aa");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.filter.as_deref(), Some("aa"));

    // Move the sidebar cursor to List B (slot 1 -> slot 2).
    update(&mut m, key(KeyCode::Tab)); // focus sidebar
    update(&mut m, ch('j'));
    assert_eq!(m.selected, Selection::List(1));
    assert_eq!(m.filter, None);
}

#[test]
fn deleting_the_active_list_clears_the_filter() {
    let (a, b) = (list_id("A"), list_id("B"));
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 3, 1, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, Message::ListsLoaded(vec![a.clone(), b.clone()]));
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(a.id.clone(), vec![task_in("A", "aa", None)]),
    );
    m.filter = Some("aa".to_string());

    // Delete the active List from the sidebar: X, then confirm with y. Focus
    // defaults to the sidebar, where `X` is gated. The selection re-points to the
    // surviving List (a non-move reselect), which must drop the filter just like
    // an explicit switch does.
    update(&mut m, ch('X'));
    update(&mut m, ch('y'));
    assert_eq!(m.filter, None);
}

#[test]
fn refresh_of_the_same_list_keeps_the_filter() {
    // A Refresh reloads the *same* selection (`ListsLoaded` with the target
    // unchanged), so it must not drop the filter the switch case drops.
    let l = list_id("L");
    let mut m = model_with(vec![open("report"), open("other")]);
    m.filter = Some("rep".to_string());
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    assert_eq!(m.selected, Selection::List(0));
    assert_eq!(m.filter.as_deref(), Some("rep"));
}

#[test]
fn composes_with_the_completed_filter() {
    let mut m = model_with(vec![
        open("report open"),
        Task {
            status: Status::Completed,
            completed_at: Some(Utc.timestamp_opt(1_700_000_100, 0).unwrap()),
            ..open("report done")
        },
    ]);
    update(&mut m, ch('/'));
    typed(&mut m, "report");
    // Completed hidden by default: only the open matching row survives both.
    assert_eq!(titles(&m.visible_tasks()), vec!["report open"]);
    m.show_completed = true;
    assert_eq!(
        titles(&m.visible_tasks()),
        vec!["report open", "report done"]
    );
}

#[test]
fn composes_with_the_distant_due_horizon() {
    let mut m = model_with(vec![
        task_in("L", "report near", None),
        Task {
            due: Some(ymd(2026, 6, 1)),
            ..task_in("L", "report far", None)
        },
    ]);
    m.hide_distant = true;
    update(&mut m, ch('/'));
    typed(&mut m, "report");
    // Both match the query, but the horizon still hides the far one.
    assert_eq!(titles(&m.visible_tasks()), vec!["report near"]);
}

#[test]
fn matches_the_display_title_not_the_raw_glyph() {
    // A typed Event stores "○ standup"; the filter matches the display title
    // ("standup"), so a query of "standup" finds it and "○" is not required.
    let mut m = model_with(vec![open("○ standup"), open("lunch")]);
    update(&mut m, ch('/'));
    typed(&mut m, "standup");
    assert_eq!(titles(&m.visible_tasks()), vec!["○ standup"]);
}

// ---- Kill chords in the query input ----
//
// `filter_key` owns its own chord arms, so it can carry the bare-`contains`
// defect independently of the shared text arm — hence the AltGr case here too.
// `^U` matters most in **Search**, where `Esc` leaves the pane rather than
// clearing (since the global Search pane landed), so it is the only way to empty
// the query short of holding Backspace.

#[test]
fn control_u_empties_the_query_and_rewidens_the_pane() {
    let mut m = model_with(vec![
        task_in("L", "report", None),
        task_in("L", "standup", None),
    ]);
    update(&mut m, ch('/'));
    typed(&mut m, "rep");
    assert_eq!(m.filter.as_deref(), Some("rep"));

    update(&mut m, chord('u'));
    assert_eq!(m.filter.as_deref(), Some(""), "^U empties the query");
    // Emptied, not closed: the input is still open for a fresh query.
    assert!(m.overlay.is_some());
}

#[test]
fn control_w_deletes_a_word_and_types_no_literal() {
    let mut m = model_with(vec![task_in("L", "weekly report", None)]);
    update(&mut m, ch('/'));
    typed(&mut m, "weekly rep");

    update(&mut m, chord('w'));
    assert_eq!(
        m.filter.as_deref(),
        Some("weekly "),
        "^W deletes the last word, and must not append a literal 'w'"
    );
}

#[test]
fn altgr_types_into_the_query_even_on_a_chord_letter() {
    let mut m = model_with(vec![task_in("L", "mail", None)]);
    update(&mut m, ch('/'));
    typed(&mut m, "a");

    update(&mut m, altgr('u'));
    update(&mut m, altgr('w'));
    update(&mut m, altgr('@'));
    assert_eq!(
        m.filter.as_deref(),
        Some("auw@"),
        "AltGr must type into a filter query — `@` is a common thing to search for"
    );
}
