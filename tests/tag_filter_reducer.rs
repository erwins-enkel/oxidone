//! Reducer tests for **Tag** filtering: the `/` filter's exact-Tag words, the
//! `#` tag picker, and the Omnibox TAG band. `update` is pure, so these run with
//! no terminal and no network.

use chrono::{Local, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{
    omnibox_rows, tag_candidates, update, Focus, Group, Message, Model, OmniRow, Overlay, TagRow,
};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};

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

fn task(title: &str) -> Task {
    Task {
        id: TaskId(title.to_string()),
        list: ListId("L".to_string()),
        parent: None,
        title: title.to_string(),
        notes: None,
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        links: Vec::new(),
        position: format!("{title:0>20}"),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn done(title: &str) -> Task {
    Task {
        status: Status::Completed,
        ..task(title)
    }
}

/// A focused task pane on List "L" seeded with `tasks`, Completed rows shown.
fn model_with(tasks: Vec<Task>) -> Model {
    let l = List {
        id: ListId("L".to_string()),
        title: "Meeting".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    };
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 3, 1, 9, 0, 0)
        .single()
        .unwrap();
    m.show_completed = true;
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab));
    m
}

fn visible(m: &Model) -> Vec<String> {
    let mut titles: Vec<String> = m.visible_tasks().iter().map(|t| t.title.clone()).collect();
    titles.sort();
    titles
}

fn filter(m: &mut Model, query: &str) {
    update(m, ch('/'));
    typed(m, query);
    update(m, key(KeyCode::Enter));
}

fn row(name: &str, count: usize) -> TagRow {
    TagRow {
        name: name.to_string(),
        count,
    }
}

// ─── the `/` filter ─────────────────────────────────────────────────────────

#[test]
fn a_tag_word_matches_the_whole_tag_only() {
    let mut m = model_with(vec![task("budget #al"), task("hiring #alex")]);
    filter(&mut m, "#al");
    assert_eq!(visible(&m), ["budget #al"]);
}

#[test]
fn a_tag_word_matches_case_insensitively() {
    let mut m = model_with(vec![task("budget #Alice"), task("other")]);
    filter(&mut m, "#aLiCe");
    assert_eq!(visible(&m), ["budget #Alice"]);
}

#[test]
fn tag_words_and_plain_words_combine() {
    let mut m = model_with(vec![
        task("budget #alice #team"),
        task("hiring #alice"),
        task("budget #bob"),
    ]);
    filter(&mut m, "#alice budget");
    assert_eq!(visible(&m), ["budget #alice #team"]);
}

#[test]
fn every_tag_word_must_match() {
    let mut m = model_with(vec![task("budget #alice #team"), task("hiring #alice")]);
    filter(&mut m, "#team #alice");
    assert_eq!(visible(&m), ["budget #alice #team"]);
}

#[test]
fn a_bare_hash_is_plain_text() {
    let mut m = model_with(vec![task("issue# 12"), task("other")]);
    filter(&mut m, "# 1");
    assert_eq!(visible(&m), ["issue# 12"]);
}

#[test]
fn a_tag_in_the_notes_is_not_a_tag() {
    let mut m = model_with(vec![Task {
        notes: Some("#alice".to_string()),
        ..task("budget")
    }]);
    filter(&mut m, "#alice");
    assert!(visible(&m).is_empty());
}

#[test]
fn a_tagged_parent_does_not_bring_its_subtasks() {
    let child = Task {
        parent: Some(TaskId("agenda #team".to_string())),
        ..task("child")
    };
    let mut m = model_with(vec![task("agenda #team"), child]);
    filter(&mut m, "#team");
    assert_eq!(visible(&m), ["agenda #team"]);
}

// ─── candidates ─────────────────────────────────────────────────────────────

#[test]
fn candidates_count_open_entries_most_first_ties_by_name() {
    let m = model_with(vec![
        task("a #bob"),
        task("b #bob"),
        task("c #alice"),
        task("d #carol"),
        done("e #carol"),
        done("f #dave"),
    ]);
    assert_eq!(
        tag_candidates(&m),
        [row("bob", 2), row("alice", 1), row("carol", 1)]
    );
}

#[test]
fn candidates_ignore_the_active_filter() {
    let mut m = model_with(vec![task("a #alice"), task("b #bob")]);
    filter(&mut m, "#alice");
    assert_eq!(tag_candidates(&m), [row("alice", 1), row("bob", 1)]);
}

// ─── the `#` picker ─────────────────────────────────────────────────────────

#[test]
fn hash_opens_the_picker_over_the_candidates() {
    let mut m = model_with(vec![task("a #bob"), task("b #bob"), task("c #alice")]);
    update(&mut m, ch('#'));
    let Some(Overlay::TagPicker {
        tags,
        query,
        selected,
    }) = &m.overlay
    else {
        panic!("picker not open: {:?}", m.overlay);
    };
    assert_eq!(tags, &[row("bob", 2), row("alice", 1)]);
    assert!(query.is_empty());
    assert_eq!(*selected, 0);
}

#[test]
fn hash_with_no_tags_in_view_says_so() {
    let mut m = model_with(vec![task("plain")]);
    update(&mut m, ch('#'));
    assert!(m.overlay.is_none());
    assert_eq!(m.status_line.as_deref(), Some("no tags in view"));
}

#[test]
fn typing_narrows_and_enter_filters_by_the_highlighted_tag() {
    let mut m = model_with(vec![task("a #bob"), task("b #alice"), task("c #alex")]);
    update(&mut m, ch('#'));
    typed(&mut m, "ali");
    update(&mut m, key(KeyCode::Enter));
    assert!(m.overlay.is_none());
    assert_eq!(m.filter.as_deref(), Some("#alice"));
    assert_eq!(visible(&m), ["b #alice"]);
    assert_eq!(m.focus, Focus::Tasks);
}

#[test]
fn a_typed_leading_hash_still_narrows() {
    let mut m = model_with(vec![task("a #bob"), task("b #alice")]);
    update(&mut m, ch('#'));
    typed(&mut m, "#bo");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.filter.as_deref(), Some("#bob"));
}

#[test]
fn down_moves_the_highlight() {
    let mut m = model_with(vec![task("a #bob"), task("b #bob"), task("c #alice")]);
    update(&mut m, ch('#'));
    update(&mut m, key(KeyCode::Down));
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.filter.as_deref(), Some("#alice"));
}

#[test]
fn esc_cancels_without_touching_the_filter() {
    let mut m = model_with(vec![task("a #bob")]);
    update(&mut m, ch('#'));
    update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert_eq!(m.filter, None);
}

#[test]
fn enter_on_no_match_keeps_the_picker_up() {
    let mut m = model_with(vec![task("a #bob")]);
    update(&mut m, ch('#'));
    typed(&mut m, "zzz");
    update(&mut m, key(KeyCode::Enter));
    assert!(matches!(m.overlay, Some(Overlay::TagPicker { .. })));
    assert_eq!(m.filter, None);
}

#[test]
fn the_picker_switches_from_one_tag_to_another() {
    let mut m = model_with(vec![task("a #alice"), task("b #bob")]);
    filter(&mut m, "#alice");
    update(&mut m, ch('#'));
    typed(&mut m, "bob");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(visible(&m), ["b #bob"]);
}

// ─── the Omnibox TAG band ───────────────────────────────────────────────────

#[test]
fn a_hash_query_draws_the_tag_band_after_jump() {
    let m = model_with(vec![task("a #alice"), task("b #alex"), task("c #bob")]);
    let rows = omnibox_rows(&m, "#al");
    let tags: Vec<&TagRow> = rows
        .iter()
        .filter_map(|r| match r {
            OmniRow::Tag(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(tags, [&row("alex", 1), &row("alice", 1)]);
    assert_eq!(rows[0].group(), Group::Tag);
}

#[test]
fn a_query_without_a_leading_hash_draws_no_tag_band() {
    let m = model_with(vec![task("a #alice")]);
    assert!(omnibox_rows(&m, "alice")
        .iter()
        .all(|r| r.group() != Group::Tag));
}

#[test]
fn enter_on_a_tag_row_filters_the_pane() {
    let mut m = model_with(vec![task("a #alice"), task("b #bob")]);
    update(&mut m, ch('p'));
    typed(&mut m, "#bob");
    update(&mut m, key(KeyCode::Enter));
    assert!(m.overlay.is_none());
    assert_eq!(m.filter.as_deref(), Some("#bob"));
    assert_eq!(visible(&m), ["b #bob"]);
}
