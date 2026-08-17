//! The Weekly spread as actually drawn: the day grid, its three cell glyphs and
//! their ASCII fallbacks, the column header, the UNSCHEDULED/WEEK group headers,
//! the today accent, the day cursor's brackets, the reserved grid width, and the
//! pending notice. `view` decides all of it and none of it is visible to a
//! reducer test — the same division `today_render.rs` follows.

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::Terminal;

const HEIGHT: u16 = 24;
/// The documented minimum terminal, as in the sibling render tests.
const WIDTH: u16 = 80;

/// A fixed "today": **Monday** 17 August 2026, so the week is 17–21 August and
/// today is the first column.
const TODAY: (i32, u32, u32) = (2026, 8, 17);

fn today() -> NaiveDate {
    NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, TODAY.2).expect("valid date")
}

fn day(n: i64) -> Option<NaiveDate> {
    Some(today() + chrono::Duration::days(n))
}

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.into()),
        title: id.to_uppercase(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn task(title: &str, list: &str, due: Option<NaiveDate>, status: Status) -> Task {
    Task {
        id: TaskId(title.into()),
        list: ListId(list.into()),
        parent: None,
        title: title.into(),
        notes: None,
        status,
        due,
        completed_at: None,
        links: Vec::new(),
        position: title.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn open(title: &str, list: &str, due: Option<NaiveDate>) -> Task {
    task(title, list, due, Status::NeedsAction)
}

/// A Model in the spread, clock fixed to `today()`, corpus handed in as
/// `WeekLoaded` would deliver it. Sits on the first List, so that is the pool.
fn week_model(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = lists.iter().map(|id| list(id)).collect();
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

fn buffer_with(model: &Model, width: u16, ascii: bool) -> Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(width, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, ascii, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn buffer(model: &Model) -> Buffer {
    buffer_with(model, WIDTH, false)
}

fn row_text(buffer: &Buffer, y: u16, width: u16) -> String {
    (0..width).map(|x| buffer[(x, y)].symbol()).collect()
}

/// The task pane's columns of every row. A terminal row spans *both* panes, and
/// the sidebar draws List titles of its own, so a search over whole lines would
/// read the sidebar's cells. The split is `Percentage(30)`.
fn pane_rows_at(model: &Model, width: u16, ascii: bool) -> Vec<String> {
    let buffer = buffer_with(model, width, ascii);
    let split = width as usize * 30 / 100;
    (0..HEIGHT)
        .map(|y| row_text(&buffer, y, width).chars().skip(split).collect())
        .collect()
}

fn pane_rows(model: &Model) -> Vec<String> {
    pane_rows_at(model, WIDTH, false)
}

/// The sidebar's columns of every row — the complement of `pane_rows`, for the
/// rows the sidebar draws and the cursor it carries.
fn sidebar_rows(model: &Model) -> Vec<String> {
    let buffer = buffer(model);
    let split = WIDTH as usize * 30 / 100;
    (0..HEIGHT)
        .map(|y| row_text(&buffer, y, WIDTH).chars().take(split).collect())
        .collect()
}

/// The sidebar row carrying `needle`.
fn sidebar_row_with(model: &Model, needle: &str) -> String {
    sidebar_rows(model)
        .into_iter()
        .find(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no sidebar row contains {needle:?}"))
}

/// The task-pane row carrying `needle`. Panics rather than returning `None`: a
/// missing row is the bug these tests exist to catch.
fn row_with(model: &Model, needle: &str) -> String {
    pane_rows(model)
        .into_iter()
        .find(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("no task-pane row contains {needle:?}"))
}

/// The foreground colour of the first cell of `needle` in the task pane.
fn fg_of(model: &Model, needle: &str) -> Option<ratatui::style::Color> {
    let buffer = buffer(model);
    let split = WIDTH as usize * 30 / 100;
    for y in 0..HEIGHT {
        let row: String = row_text(&buffer, y, WIDTH).chars().skip(split).collect();
        if let Some(at) = row.find(needle) {
            let x = (split + row[..at].chars().count()) as u16;
            return buffer[(x, y)].style().fg;
        }
    }
    panic!("no task-pane row contains {needle:?}");
}

// --- The grid --------------------------------------------------------------

/// The column header labels the five weekdays, and nothing else — Saturday and
/// Sunday have no column to be drawn in.
#[test]
fn the_column_header_names_monday_through_friday() {
    let model = week_model(&["w"], vec![open("ship", "w", day(2))]);
    let header = row_with(&model, "Mo");
    assert!(header.contains("Mo Tu We Th Fr"), "{header:?}");
    assert!(!header.contains("Sa"), "{header:?}");
    assert!(!header.contains("Su"), "{header:?}");
}

/// The three cell states, on one screen: an empty cell, a planned dot, and the
/// cross a completed row wears. Asserted with `show_completed` at its `false`
/// default — the exemption is what makes the `✕` reachable at all.
#[test]
fn the_three_cell_glyphs_draw_on_their_own_days() {
    let model = week_model(
        &["w"],
        vec![
            open("planned", "w", day(2)),
            task("finished", "w", day(1), Status::Completed),
        ],
    );
    assert!(
        !model.show_completed,
        "the default is what makes this a test"
    );

    // Wednesday is the third of five columns; the dot sits in it and the rest of
    // the row's cells are empty.
    let planned = row_with(&model, "planned");
    assert!(planned.contains("·  ·  •  ·  ·"), "{planned:?}");
    let finished = row_with(&model, "finished");
    assert!(finished.contains("·  ✕  ·  ·  ·"), "{finished:?}");
}

/// The glyphs degrade with every other one under `ascii_fallback`, and the grid
/// keeps its alignment.
#[test]
fn the_grid_degrades_to_ascii() {
    let model = week_model(
        &["w"],
        vec![
            open("planned", "w", day(2)),
            task("finished", "w", day(1), Status::Completed),
        ],
    );
    let rows = pane_rows_at(&model, WIDTH, true);
    let planned = rows
        .iter()
        .find(|r| r.contains("planned"))
        .expect("a row for planned");
    let finished = rows
        .iter()
        .find(|r| r.contains("finished"))
        .expect("a row for finished");

    assert!(planned.contains(".  .  *  .  ."), "{planned:?}");
    assert!(finished.contains(".  x  .  .  ."), "{finished:?}");
    for row in [planned, finished] {
        assert!(
            !row.contains('·') && !row.contains('•') && !row.contains('✕'),
            "{row:?}"
        );
    }
}

/// A row holds at most one dot, which falls out of the data model — a Task has
/// one due date — rather than being enforced by the renderer.
#[test]
fn a_row_holds_exactly_one_dot() {
    let model = week_model(&["w"], vec![open("ship", "w", day(3))]);
    let row = row_with(&model, "ship");
    assert_eq!(row.matches('•').count(), 1, "{row:?}");
}

// --- The cursor ------------------------------------------------------------

/// The cursor brackets the cell it is on, and only on the selected row — the
/// cursor is a (row, day) pair, so brackets everywhere would claim five cursors.
#[test]
fn the_day_cursor_brackets_one_cell_on_the_selected_row() {
    let mut model = week_model(
        &["w"],
        vec![open("selected", "w", None), open("other", "w", None)],
    );
    model.selected_task = Some(0);
    update(&mut model, press('l'));
    update(&mut model, press('l')); // Tuesday

    let selected = row_with(&model, "selected");
    assert!(selected.contains("· [·] ·"), "{selected:?}");
    let other = row_with(&model, "other");
    assert!(!other.contains('['), "{other:?}");
}

/// At home the grid wears no brackets at all: the cursor is on the title, where
/// `Space` means what it means in every other pane.
#[test]
fn the_home_position_brackets_nothing() {
    let mut model = week_model(&["w"], vec![open("a", "w", None)]);
    model.selected_task = Some(0);
    assert_eq!(model.week_day, None);

    let row = row_with(&model, "a");
    assert!(!row.contains('['), "{row:?}");
}

// --- The group headers -----------------------------------------------------

/// Each header is drawn only when its block has rows, so a pane with only
/// scheduled entries carries no UNSCHEDULED header and vice versa.
#[test]
fn a_group_header_is_drawn_only_for_a_block_that_has_rows() {
    let scheduled_only = week_model(&["w"], vec![open("ship", "w", day(0))]);
    let rows = pane_rows(&scheduled_only);
    assert!(rows.iter().any(|r| r.contains("WEEK 34")));
    assert!(!rows.iter().any(|r| r.contains("UNSCHEDULED")));

    let pool_only = week_model(&["w"], vec![open("jot", "w", None)]);
    let rows = pane_rows(&pool_only);
    assert!(rows.iter().any(|r| r.contains("UNSCHEDULED (W)")));
    assert!(!rows.iter().any(|r| r.contains("WEEK 34")));
}

/// The WEEK header names the days on display, and `]` moves them.
#[test]
fn the_week_header_names_its_days_and_follows_the_paging() {
    let mut model = week_model(
        &["w"],
        vec![open("this", "w", day(2)), open("next", "w", day(9))],
    );
    let header = row_with(&model, "WEEK 3");
    assert!(
        header.contains("WEEK 34 · Mon 17 – Fri 21 Aug"),
        "{header:?}"
    );

    update(&mut model, press(']'));
    let header = row_with(&model, "WEEK 3");
    assert!(
        header.contains("WEEK 35 · Mon 24 – Fri 28 Aug"),
        "{header:?}"
    );
}

/// Fail closed: on the pinned Week row, with no default resolved, there is no pool
/// to draw from, and the header must say so rather than let an absent block read
/// as "nothing undated".
#[test]
fn an_unresolvable_pool_says_so_instead_of_drawing_nothing() {
    let mut model = week_model(&["w"], vec![open("ship", "w", day(0))]);
    model.selected = Selection::Week;
    model.default_list = None;

    let header = row_with(&model, "UNSCHEDULED");
    assert!(header.contains("no list selected"), "{header:?}");
}

// --- Today's column --------------------------------------------------------

/// Today's column is accented in the header — but only while the week on screen
/// contains today, or `]` would leave a stale marker behind.
#[test]
fn todays_column_is_accented_only_on_the_current_week() {
    let mut model = week_model(&["w"], vec![open("ship", "w", day(0))]);
    let theme = Theme::from_flavor("mocha");

    // Today is Monday, so the first label carries the accent and the second does
    // not — asserted as a contrast, so a theme change cannot make it vacuous.
    assert_eq!(fg_of(&model, "Mo"), Some(theme.accent));
    assert_ne!(fg_of(&model, "Tu"), Some(theme.accent));

    update(&mut model, press(']'));
    assert_ne!(
        fg_of(&model, "Mo"),
        Some(theme.accent),
        "next week has no today"
    );
}

// --- Width and the pending notice ------------------------------------------

/// The grid is the view, so its width is reserved unconditionally: a narrow pane
/// clips the title to an ellipsis rather than letting the List widget truncate
/// the columns off the right edge.
#[test]
fn a_narrow_pane_clips_the_title_and_keeps_the_whole_grid() {
    let model = week_model(
        &["w"],
        vec![open(
            "a title far longer than the pane can hold",
            "w",
            day(2),
        )],
    );
    let rows = pane_rows_at(&model, 46, false);
    let row = rows
        .iter()
        .find(|r| r.contains('•'))
        .expect("the planned row still draws its dot");

    assert!(row.contains('…'), "the title is clipped: {row:?}");
    assert!(row.contains("·  ·  •  ·  ·"), "the grid survives: {row:?}");
}

/// While the fan-out is outstanding the pane says so — an empty week must read
/// as "not yet", never as "nothing planned".
#[test]
fn the_pending_notice_shows_while_the_corpus_is_incomplete() {
    let mut model = Model::new();
    model.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    model.lists = vec![list("w")];
    model.selected = Selection::List(0);
    update(&mut model, press('W'));

    assert!(model.week_pending);
    let rows = pane_rows(&model);
    assert!(
        rows.iter().any(|r| r.contains("reading all lists")),
        "no pending notice: {rows:?}"
    );
}

/// The pending notice belongs to the pane it describes. `week` survives an `S`
/// — deliberately, so `Esc` returns to the spread — and while Search holds the
/// pane `WeekLoaded` is dropped by its own guard, so nothing would ever clear the
/// flag. Ungated, the notice would stick in the Search header for the session.
#[test]
fn the_pending_notice_does_not_leak_into_the_search_header() {
    let mut model = Model::new();
    model.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    model.lists = vec![list("w")];
    model.selected = Selection::List(0);
    update(&mut model, press('W'));
    assert!(model.week_pending);

    update(&mut model, press('S'));

    assert!(model.week_pending, "the flag survives; the notice must not");
    // Wide enough that the panel title is not clipped — at 80 columns the pane
    // truncates it, and the assertion below would pass without meaning anything.
    let rows = pane_rows_at(&model, 160, false);
    assert!(
        rows.iter().any(|r| r.contains("SEARCH")),
        "Search holds the pane: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("reading all lists")),
        "the week's notice leaked into Search: {rows:?}"
    );
}

/// Pool rows whose List resolves to no *title* are still headed by their own
/// block. The "no list selected" notice denies they exist, so it is keyed on
/// there being no rows either.
///
/// The fixture is the only shape that reaches it: the pinned Week row, with
/// `default_list` naming a List absent from `lists` and the pool rows in that same
/// List — so `week_pool_list()` answers `Some`, `within_week` admits them, and the
/// title lookup still comes back empty.
#[test]
fn pool_rows_are_never_headed_by_the_no_pool_notice() {
    let mut model = week_model(&["w"], vec![open("jot", "ghost", None)]);
    model.selected = Selection::Week;
    model.default_list = Some(ListId("ghost".into()));

    let rows = pane_rows(&model);
    assert!(
        rows.iter().any(|r| r.contains("jot")),
        "the pool row is drawn: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("UNSCHEDULED")),
        "and headed: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("no list selected")),
        "a notice denying the rows below it: {rows:?}"
    );
}

/// `?` opens the cheatsheet in the spread — `week_key` declines it and lets the
/// keymap have it — so the legend must pin the help cell there, as it does in
/// every other *pane* context. Only overlays go without, where `?` would type a
/// literal `?` into a buffer.
#[test]
fn the_spread_pins_the_help_cell_like_every_other_pane() {
    let model = week_model(&["w"], vec![open("ship", "w", day(2))]);
    // The legend spans the whole terminal, not just the task pane, and sits on
    // the last row above the status line.
    let buffer = buffer_with(&model, WIDTH, false);
    let legend = (0..HEIGHT)
        .map(|y| row_text(&buffer, y, WIDTH))
        .find(|r| r.contains("? help"))
        .unwrap_or_else(|| {
            let all: Vec<String> = (0..HEIGHT).map(|y| row_text(&buffer, y, WIDTH)).collect();
            panic!("no pinned help cell in the spread: {all:?}")
        });
    // Pinned means flush right, not merely present.
    assert!(legend.trim_end().ends_with("? help"), "{legend:?}");
    // And it is the spread's own legend below it, not the ordinary Tasks one.
    assert!(legend.contains("plan/done"), "{legend:?}");
}

/// The panel names the pane and the week it shows. No Sort label: the spread has
/// one fixed order.
#[test]
fn the_panel_title_names_the_spread_and_its_week() {
    let model = week_model(&["w"], vec![open("ship", "w", day(0))]);
    let title = row_with(&model, "WEEKLY SPREAD");
    assert!(title.contains("week 34"), "{title:?}");
    assert!(!title.contains("due"), "no sort lens is named: {title:?}");
}

/// And it names the **scope**, because the scope is a filter that can empty the
/// pane: a List by title, the pinned Week row as every List.
#[test]
fn the_panel_title_names_the_scope() {
    let mut model = week_model(&["w", "h"], vec![open("ship", "w", day(0))]);
    let scoped = row_with(&model, "WEEKLY SPREAD");
    assert!(scoped.contains("— W"), "the List's own title: {scoped:?}");

    model.selected = Selection::Week;
    let every = row_with(&model, "WEEKLY SPREAD");
    assert!(every.contains("all lists"), "{every:?}");
}

// --- The sidebar's Week row ------------------------------------------------

/// The Week row is a cursor stop, so the cursor is drawn on it — the same gutter
/// marker every other selectable row gets, and the proof the sidebar's slot
/// arithmetic agrees with the reducer's.
#[test]
fn the_sidebar_cursor_is_drawn_on_the_week_row() {
    let mut model = week_model(&["w"], vec![open("ship", "w", day(0))]);
    model.selected = Selection::Week;
    model.focus = Focus::Sidebar;

    let week = sidebar_row_with(&model, "Week");
    assert!(week.contains("› Week"), "{week:?}");
    // And nowhere else: one cursor, on the row the model names.
    let today = sidebar_row_with(&model, "Today");
    assert!(!today.contains('›'), "{today:?}");
}

/// A List-scoped week still lights the Week row, even with the cursor parked on the
/// List it is scoped to — the accent says *the lens is up*, independently of where
/// the cursor is.
#[test]
fn the_week_row_is_accented_while_a_lists_week_is_up() {
    let model = week_model(&["w"], vec![open("ship", "w", day(0))]);
    assert_eq!(
        model.selected,
        Selection::List(0),
        "the cursor is on the List"
    );

    let theme = Theme::from_flavor("mocha");
    let buffer = buffer(&model);
    let rows = sidebar_rows(&model);
    let y = rows
        .iter()
        .position(|r| r.contains("Week"))
        .expect("a sidebar Week row") as u16;
    // Cells, not bytes: the panel's border glyph is multi-byte.
    let x = rows[y as usize]
        .chars()
        .position(|c| c == 'W')
        .expect("the label starts somewhere") as u16;
    assert_eq!(buffer[(x, y)].style().fg, Some(theme.accent));
}
