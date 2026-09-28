//! **Tags** as drawn: the `tag` hue on a row's `#name`, and the tag picker's
//! popup. Through the public `ui::view`, as `meters_render.rs` does.

use chrono::{Local, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Color;
use ratatui::Terminal;

const WIDTH: u16 = 100;
const HEIGHT: u16 = 24;

fn theme() -> Theme {
    Theme::from_flavor("mocha")
}

fn buffer(model: &Model) -> Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    terminal
        .draw(|frame| ui::view(model, &theme(), false, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn list() -> List {
    List {
        id: ListId("L".into()),
        title: "Meeting".into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

fn task(title: &str) -> Task {
    Task {
        id: TaskId(title.into()),
        list: ListId("L".into()),
        parent: None,
        title: title.into(),
        notes: None,
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        links: Vec::new(),
        position: title.into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

fn now(m: &mut Model) {
    m.now = Local.with_ymd_and_hms(2026, 3, 4, 12, 0, 0).unwrap();
}

/// A focused List pane holding `tasks`, the cursor on the first.
fn list_model(tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    now(&mut m);
    m.show_completed = true;
    m.lists = vec![list()];
    m.selected = Selection::List(0);
    m.tasks = tasks;
    m.selected_task = Some(0);
    m.focus = Focus::Tasks;
    m
}

/// The row `needle` is drawn on, and the x its first character starts at.
fn find(buffer: &Buffer, needle: &str) -> (u16, u16) {
    let first = needle.chars().next().unwrap().to_string();
    for y in 0..HEIGHT {
        let line: String = (0..WIDTH)
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect();
        if let Some(byte) = line.find(needle) {
            let col = line[..byte].chars().count() as u16;
            assert_eq!(buffer[(col, y)].symbol(), first);
            return (col, y);
        }
    }
    panic!("{needle:?} not drawn");
}

/// The foreground of each cell `needle` covers.
fn fgs(buffer: &Buffer, needle: &str) -> Vec<Option<Color>> {
    let (x, y) = find(buffer, needle);
    (0..needle.chars().count() as u16)
        .map(|i| buffer[(x + i, y)].style().fg)
        .collect()
}

#[test]
fn a_tag_draws_in_the_tag_hue_and_the_rest_does_not() {
    let m = list_model(vec![task("first"), task("ask #alice: budget")]);
    let b = buffer(&m);
    let tag = Some(theme().tag);
    assert!(fgs(&b, "#alice").iter().all(|fg| *fg == tag));
    assert!(fgs(&b, "ask").iter().all(|fg| *fg != tag));
    assert!(fgs(&b, ": budget").iter().all(|fg| *fg != tag));
}

#[test]
fn the_title_text_is_drawn_verbatim() {
    let m = list_model(vec![task("first"), task("ask #Alice: budget #team")]);
    find(&buffer(&m), "ask #Alice: budget #team");
}

#[test]
fn a_completed_row_does_not_color_its_tags() {
    let m = list_model(vec![
        task("first"),
        Task {
            status: Status::Completed,
            ..task("old #alice")
        },
    ]);
    let b = buffer(&m);
    assert!(fgs(&b, "#alice")
        .iter()
        .all(|fg| *fg == Some(theme().subtext)));
}

/// The selected row wears the list highlight whole, over its tags, exactly as
/// it does over the overdue red: the highlight rewrites every cell's colour.
#[test]
fn the_selected_row_wears_the_highlight_over_its_tags() {
    let m = list_model(vec![task("ask #alice")]);
    let b = buffer(&m);
    assert!(fgs(&b, "#alice")
        .iter()
        .all(|fg| *fg == Some(theme().accent)));
}

#[test]
fn today_colors_tags_too() {
    let mut m = Model::new();
    now(&mut m);
    m.lists = vec![list()];
    m.selected = Selection::Today;
    let today = m.now.date_naive();
    let due = |title: &str| Task {
        due: Some(today),
        ..task(title)
    };
    update(
        &mut m,
        Message::TodayLoaded {
            tasks: vec![due("a first"), due("sync #bob")],
            failed: Vec::new(),
        },
    );
    m.focus = Focus::Sidebar;
    let b = buffer(&m);
    assert!(fgs(&b, "#bob").iter().all(|fg| *fg == Some(theme().tag)));
}

#[test]
fn the_weekly_spread_colors_tags_too() {
    let mut m = Model::new();
    now(&mut m);
    m.lists = vec![list()];
    m.selected = Selection::List(0);
    update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Char('W'), KeyModifiers::empty())),
    );
    update(
        &mut m,
        Message::WeekLoaded {
            tasks: vec![task("a first"), task("pool #carol")],
            failed: Vec::new(),
            live: true,
        },
    );
    m.focus = Focus::Sidebar;
    let b = buffer(&m);
    assert!(fgs(&b, "#carol").iter().all(|fg| *fg == Some(theme().tag)));
}

#[test]
fn the_picker_lists_tags_with_their_open_counts() {
    let mut m = list_model(vec![task("a #bob"), task("b #bob"), task("c #alice")]);
    update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Char('#'), KeyModifiers::empty())),
    );
    let b = buffer(&m);
    let (_, bob) = find(&b, "#bob  2 open");
    let (_, alice) = find(&b, "#alice  1 open");
    assert!(bob < alice);
    find(&b, "Filter by tag");
}
