//! The filter indicator as actually drawn. Unlike the reducer tests this one
//! needs a terminal — a `TestBackend` — because `header_title` is private to the
//! `ui` module: the only way to assert what reaches the screen is to draw a frame
//! through `ui::view` and read the buffer, the way `tests/meters_render.rs` does.

use chrono::{Local, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 24;

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

/// Draw a frame and return its rows as strings (cell-per-char).
fn rows(model: &Model) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// The task-pane header line — the top border of the right-hand panel, row 0.
fn header(model: &Model) -> String {
    rows(model).into_iter().next().expect("at least one row")
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

fn model() -> Model {
    let l = List {
        id: ListId("L".to_string()),
        title: "L".to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    };
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 3, 1, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(l.id.clone(), vec![task("report"), task("other")]),
    );
    update(&mut m, key(KeyCode::Tab)); // focus the task pane
    m
}

#[test]
fn no_filter_shows_no_query_in_the_header() {
    let m = model();
    let header = header(&m);
    // The filter segment is always `  /query`; the plain `/` alone would false-
    // positive on the completion meter's `done/total` (rendered "0/2"). Assert the
    // filter's own two-space-then-slash marker is absent.
    assert!(
        !header.contains("  /"),
        "header unexpectedly shows a filter: {header:?}"
    );
}

#[test]
fn editing_shows_the_query_with_a_caret() {
    let mut m = model();
    update(&mut m, key(KeyCode::Char('/')));
    for c in "rep".chars() {
        update(&mut m, key(KeyCode::Char(c)));
    }
    let header = header(&m);
    assert!(
        header.contains("/rep▏"),
        "editing header should show the query and caret: {header:?}"
    );
}

#[test]
fn a_committed_filter_shows_the_query_without_a_caret() {
    let mut m = model();
    update(&mut m, key(KeyCode::Char('/')));
    for c in "rep".chars() {
        update(&mut m, key(KeyCode::Char(c)));
    }
    update(&mut m, key(KeyCode::Enter)); // commit; input closes
    let header = header(&m);
    assert!(
        header.contains("/rep"),
        "committed header should still show the query: {header:?}"
    );
    assert!(
        !header.contains('▏'),
        "a committed filter must not show the editing caret: {header:?}"
    );
}
