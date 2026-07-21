//! The Today pane's trailing List name as actually drawn. `view` decides both
//! *whether* the name is emitted (only on Today, only for a known List) and how
//! dim it reads; neither is visible to a reducer test, which is what this file
//! covers, following `link_render.rs` and `notes_render.rs`.

use chrono::{Local, NaiveDate, TimeZone};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const HEIGHT: u16 = 24;
/// The documented minimum terminal, as in the sibling render tests.
const WIDTH: u16 = 80;

/// A fixed "today" so Today membership is deterministic.
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

fn task(title: &str, list: &str) -> Task {
    Task {
        id: TaskId(title.into()),
        list: ListId(list.into()),
        parent: None,
        title: title.into(),
        notes: None,
        status: Status::NeedsAction,
        due: Some(today()),
        completed_at: None,
        links: Vec::new(),
        position: title.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

/// A Model on Today, clock fixed to `today()`, with `lists` known and `tasks`
/// handed in as the aggregate — as `tests/today_reducer.rs` builds it. `lists` is
/// load-bearing here: the List name resolves through it, so an empty `lists`
/// draws no name at all.
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

fn buffer(model: &Model) -> Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn row_text(buffer: &Buffer, y: u16) -> String {
    (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect()
}

/// The task pane's first column. A terminal row spans *both* panes and the
/// sidebar draws the very same List titles, so a search over the whole line
/// reads the sidebar's cells, not the span under test. The split is
/// `Percentage(30)`.
fn pane_x() -> usize {
    WIDTH as usize * 30 / 100
}

/// The foreground of every cell of `name` where the *task pane* draws it on the
/// row carrying `title`. Panics rather than returning `None`: a missing name is
/// the bug this test exists to catch, and a silent `None` would pass it.
fn list_name_fg(model: &Model, title: &str, name: &str) -> Vec<Option<ratatui::style::Color>> {
    let buffer = buffer(model);
    let y = (0..HEIGHT)
        .find(|&y| row_text(&buffer, y).contains(title))
        .unwrap_or_else(|| panic!("no row for {title}"));
    let row = row_text(&buffer, y);
    // Byte offsets are not columns; the rows here are cell-per-char, so the
    // enumeration index *is* the column. Matches before the pane's origin are
    // the sidebar's and are skipped — if the layout ever changes, this panics
    // rather than silently asserting on the wrong cells.
    let col = row
        .char_indices()
        .enumerate()
        .find(|(col, (byte, _))| *col >= pane_x() && row[*byte..].starts_with(name))
        .map(|(col, _)| col)
        .unwrap_or_else(|| panic!("no {name} in the task pane of the {title} row"));
    (col..col + name.chars().count())
        .map(|x| buffer[(x as u16, y)].style().fg)
        .collect()
}

#[test]
fn the_list_name_reads_muted() {
    let theme = Theme::from_flavor("mocha");
    // Two rows, cursor pinned to the first: the focused pane paints the cursor
    // row `accent`, which would mask the name's own colour, so the assertion is
    // on the other row.
    let mut model = today_model(
        &["work", "home"],
        vec![task("alpha", "work"), task("bravo", "home")],
    );
    model.selected_task = Some(0);
    let fg = list_name_fg(&model, "bravo", "HOME");
    // Positive, not merely "not `subtext`": the sidebar's copy of the same title
    // paints `text`, which a negative assertion would happily accept.
    assert!(
        fg.iter().all(|c| *c == Some(theme.muted)),
        "the List name should read `muted`, one step below the notes preview: {fg:?}"
    );
}
