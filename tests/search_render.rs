//! The Search pane as actually drawn. Like `tests/filter_render.rs`, these need a
//! terminal (`TestBackend`) because `header_title`, the column rules, and the
//! legend selection are private to `ui`: the only way to assert what reaches the
//! screen is to draw a frame through `ui::view` and read the buffer.

use chrono::{Local, NaiveDate, TimeZone};
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

fn press(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn list(id: &str, title: &str) -> List {
    List {
        id: ListId(id.into()),
        title: title.into(),
        etag: "e".into(),
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
        etag: "e".into(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn undated(id: &str, list: &str) -> Task {
    task(id, list, None, Status::NeedsAction)
}

/// Draw `model` at `width` and return its rows as strings (cell-per-char).
fn rows_at(model: &Model, width: u16) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(width, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn rows(model: &Model) -> Vec<String> {
    rows_at(model, WIDTH)
}

/// The task-pane header line — the top border of the right-hand panel, row 0.
fn header(model: &Model) -> String {
    rows(model).into_iter().next().expect("at least one row")
}

/// The always-visible legend — the bottom row.
fn legend(model: &Model, width: u16) -> String {
    rows_at(model, width)
        .last()
        .expect("a bottom row")
        .trim_end()
        .to_string()
}

/// A model with `lists` known, clock pinned to 2026-07-20, task pane focused.
fn base(lists: &[(&str, &str)]) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 7, 20, 12, 0, 0)
        .single()
        .unwrap();
    m.lists = lists.iter().map(|(id, title)| list(id, title)).collect();
    m.focus = oxidone::app::Focus::Tasks;
    m
}

/// Enter Search over `corpus`, delivering the live send so `search_pending`
/// clears, and commit the empty query so the header is the bare title.
fn search_model(lists: &[(&str, &str)], corpus: Vec<Task>) -> Model {
    let mut m = base(lists);
    m.selected = Selection::List(0);
    update(&mut m, press('S'));
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: corpus,
            failed: Vec::new(),
            live: true,
        },
    );
    update(&mut m, key(KeyCode::Enter)); // commit the empty query
    m
}

/// A List pane over the same tasks, for the baselines that prove the difference.
fn list_model(lists: &[(&str, &str)], tasks: Vec<Task>) -> Model {
    let mut m = base(lists);
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::TasksLoaded(ListId(lists[0].0.into()), tasks),
    );
    m
}

#[test]
fn the_header_names_the_pane() {
    let m = search_model(&[("work", "WORK")], vec![undated("a", "work")]);
    assert!(header(&m).contains("SEARCH"), "header should name Search");
}

#[test]
fn the_query_shows_with_a_caret_only_while_the_input_is_open() {
    let mut m = base(&[("work", "WORK")]);
    m.selected = Selection::List(0);
    update(&mut m, press('S'));
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("tax", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    for c in "tax".chars() {
        update(&mut m, press(c));
    }
    assert!(
        header(&m).contains("/tax▏"),
        "an open input shows the query and caret: {:?}",
        header(&m)
    );
    update(&mut m, key(KeyCode::Enter)); // commit
    let h = header(&m);
    assert!(h.contains("/tax"), "committed query still shows: {h:?}");
    assert!(!h.contains('▏'), "no caret once the input is closed: {h:?}");
}

#[test]
fn the_pending_notice_shows_while_the_corpus_is_incomplete() {
    let mut m = base(&[("work", "WORK")]);
    m.selected = Selection::List(0);
    update(&mut m, press('S')); // pending, cache paint not yet even delivered
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("a", "work")],
            failed: Vec::new(),
            live: false, // cache paint: still pending
        },
    );
    assert!(
        header(&m).contains("searching all lists"),
        "an incomplete corpus says so: {:?}",
        header(&m)
    );
}

#[test]
fn neither_header_widget_is_drawn_in_search() {
    // The same three needsAction Tasks draw a `0/3` completion meter in a List
    // pane; Search must show no such ratio.
    let corpus = vec![
        undated("a", "work"),
        undated("b", "work"),
        undated("c", "work"),
    ];
    let list = list_model(&[("work", "WORK")], corpus.clone());
    assert!(
        header(&list).contains("0/3"),
        "the List pane baseline should draw a meter: {:?}",
        header(&list)
    );
    let search = search_model(&[("work", "WORK")], corpus);
    assert!(
        !header(&search).contains("0/3"),
        "Search must not draw the completion meter: {:?}",
        header(&search)
    );
}

#[test]
fn rows_carry_the_list_name_column() {
    let m = search_model(
        &[("work", "WORK"), ("home", "HOME")],
        vec![undated("alpha", "work"), undated("beta", "home")],
    );
    let body = rows(&m).join("\n");
    assert!(
        body.contains("WORK"),
        "the row's home List is shown: {body:?}"
    );
    assert!(body.contains("HOME"), "and the other List's: {body:?}");
}

#[test]
fn a_future_date_prints_even_when_nothing_is_overdue() {
    // The two axis bugs the flat/spread split avoids: with nothing overdue the due
    // column must still be drawn, and a non-overdue (future) row must print its
    // date rather than blanking.
    let m = search_model(
        &[("work", "WORK")],
        vec![
            undated("undated-row", "work"),
            task(
                "future-row",
                "work",
                Some(ymd(2026, 12, 1)),
                Status::NeedsAction,
            ),
        ],
    );
    let body = rows(&m).join("\n");
    assert!(
        body.contains("2026-12-01"),
        "the future date must render (due column drawn, date not blanked): {body:?}"
    );
}

#[test]
fn search_is_not_a_journal_spread() {
    let m = search_model(
        &[("work", "WORK")],
        vec![task(
            "overdue",
            "work",
            Some(ymd(2026, 7, 1)),
            Status::NeedsAction,
        )],
    );
    let body = rows(&m).join("\n");
    assert!(
        !body.contains("Overdue"),
        "Search draws no Overdue header: {body:?}"
    );
}

#[test]
fn the_open_input_legend_says_leave_search_not_clear() {
    let mut m = base(&[("work", "WORK")]);
    m.selected = Selection::List(0);
    update(&mut m, press('S'));
    update(
        &mut m,
        Message::SearchLoaded {
            tasks: vec![undated("a", "work")],
            failed: Vec::new(),
            live: true,
        },
    );
    // The input is open (S opened it).
    let line = legend(&m, WIDTH);
    assert!(
        line.contains("Esc leave search"),
        "Search's filter legend must not promise clear: {line:?}"
    );
    assert!(
        !line.contains("Esc clear"),
        "and it must not say clear: {line:?}"
    );
}

#[test]
fn a_list_pane_filter_still_says_clear() {
    // The `/` legend from #96 is untouched in a List pane.
    let mut m = list_model(&[("work", "WORK")], vec![undated("a", "work")]);
    update(&mut m, press('/'));
    let line = legend(&m, WIDTH);
    assert!(
        line.contains("Esc clear"),
        "a List-pane filter keeps the clear affordance: {line:?}"
    );
}

#[test]
fn the_eighty_column_task_legend_is_identical_in_search_and_a_list_pane() {
    // `S` claimed no always-visible cell, so the budgeted 80-column row is
    // unchanged — `c completed` was not evicted.
    let list = list_model(&[("work", "WORK")], vec![undated("a", "work")]);
    let search = search_model(&[("work", "WORK")], vec![undated("a", "work")]);
    assert_eq!(
        legend(&list, 80),
        legend(&search, 80),
        "the 80-column task legend must match, cell for cell"
    );
}
