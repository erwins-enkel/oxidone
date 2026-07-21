//! The Today pane as actually drawn: its trailing List name, and the journal
//! spread of #62 — the dateline row, the `Overdue`/`Today` group headers, the
//! due column that comes and goes with the Overdue group, and the always-reserved
//! signifier gutter. `view` decides all of it, and none of it is visible to a
//! reducer test, which is what this file covers, following `link_render.rs` and
//! `notes_render.rs`.

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
    buffer_with(model, false)
}

fn buffer_with(model: &Model, ascii: bool) -> Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, ascii, frame))
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

// --- The journal spread (#62) ----------------------------------------------

/// The task pane's inner text on terminal row `y`, borders and the sidebar
/// stripped, trailing blanks trimmed. The pane starts at [`pane_x`] and spends
/// one column on each border.
fn pane_line(buffer: &Buffer, y: u16) -> String {
    let inner = pane_x() as u16 + 1..WIDTH - 1;
    inner
        .map(|x| buffer[(x, y)].symbol())
        .collect::<String>()
        .trim_end()
        .to_string()
}

/// The pane's inner rows, from the first below the border down, with the trailing
/// empty ones dropped — i.e. exactly the spread as drawn.
fn spread(model: &Model) -> Vec<String> {
    spread_of(&buffer(model))
}

fn spread_of(buffer: &Buffer) -> Vec<String> {
    let mut rows: Vec<String> = (1..HEIGHT - 3).map(|y| pane_line(buffer, y)).collect();
    while rows.last().is_some_and(|r| r.is_empty()) {
        rows.pop();
    }
    rows
}

/// The pane's title, as drawn into the top border.
fn pane_title(model: &Model) -> String {
    pane_line(&buffer(model), 0)
}

/// The foreground of every cell of `text` where the task pane draws it. Panics
/// rather than returning `None`: a missing label is the bug under test.
fn pane_fg(buffer: &Buffer, text: &str) -> Vec<Option<ratatui::style::Color>> {
    let y = (1..HEIGHT - 3)
        .find(|&y| pane_line(buffer, y).contains(text))
        .unwrap_or_else(|| panic!("no pane row containing {text}"));
    let row: String = (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect();
    let col = row
        .char_indices()
        .enumerate()
        .find(|(col, (byte, _))| *col >= pane_x() && row[*byte..].starts_with(text))
        .map(|(col, _)| col)
        .unwrap_or_else(|| panic!("no {text} in the task pane"));
    (col..col + text.chars().count())
        .map(|x| buffer[(x as u16, y)].style().fg)
        .collect()
}

/// A pane row past the two-cell cursor gutter `render_selectable` spends on every
/// row, so a cursor row and a plain one can be compared column for column.
fn body(row: &str) -> String {
    row.chars().skip(2).collect()
}

/// The *column* where `text` starts — never `str::find`, whose byte offsets skew
/// by two the moment the row carries the three-byte `›`.
fn col_of(row: &str, text: &str) -> Option<usize> {
    let row: Vec<char> = row.chars().collect();
    let pat: Vec<char> = text.chars().collect();
    (0..=row.len().saturating_sub(pat.len())).find(|&i| row[i..i + pat.len()] == pat[..])
}

fn dated(mut t: Task, due: NaiveDate) -> Task {
    t.due = Some(due);
    t
}

fn completed(mut t: Task) -> Task {
    t.status = Status::Completed;
    t
}

fn day(d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(TODAY.0, TODAY.1, d).expect("valid date")
}

/// A day with both groups: two overdue rows (one of them settled) and two due
/// today. `show_completed` is on so the settled row is actually drawn — which is
/// the point of the count assertions below.
fn both_groups() -> Model {
    let mut m = today_model(
        &["work", "home"],
        vec![
            dated(task("Dentist", "home"), day(18)),
            completed(dated(task("File taxes", "work"), day(19))),
            task("Ship 62", "work"),
            task("Standup", "home"),
        ],
    );
    m.show_completed = true;
    m
}

#[test]
fn the_dateline_is_the_first_row_and_the_panel_title_is_untouched() {
    let m = both_groups();
    assert_eq!(spread(&m)[0].trim(), "Monday 20 July 2026");
    // The dateline is a *row*, so the title keeps naming its lens exactly as
    // every other pane does, and `header_title`'s width budget is unspent.
    assert!(
        pane_title(&m).starts_with("Tasks — due"),
        "got {:?}",
        pane_title(&m)
    );
}

#[test]
fn the_dateline_is_drawn_on_an_empty_day() {
    // The dateline is the page, not a label for the rows: an empty Today still
    // opens on its date rather than on nothing at all.
    let m = today_model(&["work"], vec![]);
    assert_eq!(spread(&m), vec!["Monday 20 July 2026".to_string()]);
}

#[test]
fn the_dividers_track_the_overdue_and_today_split() {
    let overdue = dated(task("Dentist", "home"), day(18));
    let due_today = task("Ship 62", "work");

    // Both groups: each divider is drawn, with its outstanding count.
    let both = spread(&both_groups()).join("\n");
    assert!(
        both.contains("Overdue") && both.contains("Today 2"),
        "{both}"
    );

    // Only overdue: the `Overdue` divider is drawn; the `Today` divider is not,
    // since the group it would head has no rows.
    let only_overdue = spread(&today_model(&["home"], vec![overdue]));
    assert!(only_overdue.iter().any(|r| r.trim() == "Overdue 1"));
    assert!(
        !only_overdue.iter().any(|r| r.trim().starts_with("Today")),
        "{only_overdue:?}"
    );

    // Only today (nothing overdue): neither divider is drawn. The `Today` divider
    // would only echo the dateline, so it is dropped — its count rides the
    // dateline instead (asserted in `the_dateline_carries_the_count_...`).
    let only_today = spread(&today_model(&["work"], vec![due_today])).join("\n");
    assert!(!only_today.contains("Overdue"), "{only_today}");
    assert!(!only_today.contains("Today"), "{only_today}");

    // Empty day: dateline only, no dividers.
    let empty = spread(&today_model(&["work"], vec![])).join("\n");
    assert!(
        !empty.contains("Overdue") && !empty.contains("Today"),
        "{empty}"
    );
}

#[test]
fn the_dateline_carries_the_count_when_nothing_is_overdue() {
    // With no Overdue group the redundant `Today` divider is dropped, and its one
    // unique signal — the count still outstanding today — rides the dateline. Two
    // open and one settled: the settled row is drawn but not counted, so the
    // dateline reads `2`, not `3`.
    let mut m = today_model(
        &["work"],
        vec![
            task("Ship 62", "work"),
            task("Standup", "work"),
            completed(task("Groceries", "work")),
        ],
    );
    m.show_completed = true;
    let rows = spread(&m);

    assert!(
        rows[0].contains("Monday 20 July 2026  2"),
        "the dateline carries the outstanding count: {rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.trim().starts_with("Today")),
        "no redundant `Today` divider when nothing is overdue: {rows:?}"
    );
    assert!(
        rows.iter().any(|r| r.contains("Groceries")),
        "the settled row is still drawn, just not counted: {rows:?}"
    );
}

#[test]
fn the_header_count_is_outstanding_work_not_the_rows_drawn() {
    // Both headers, not just `Overdue`: they are documented to mean the same
    // thing, so the rule is asserted on each. A settled row in either group is
    // still *drawn* — it is only not *counted*, because the count answers what is
    // left to migrate rather than what is on screen.
    //
    // Its own fixture, deliberately asymmetric (2 overdue of which 1 settled, 3
    // due today of which 1 settled): equal group sizes would let a renderer that
    // counted the wrong group pass.
    let mut m = today_model(
        &["work", "home"],
        vec![
            dated(task("Dentist", "home"), day(18)),
            completed(dated(task("File taxes", "work"), day(19))),
            task("Ship 62", "work"),
            task("Standup", "home"),
            completed(task("Groceries", "home")),
        ],
    );
    m.show_completed = true;
    let rows = spread(&m);

    assert!(rows.iter().any(|r| r.trim() == "Overdue 1"), "{rows:?}");
    assert!(
        rows.iter().any(|r| r.trim() == "Today 2"),
        "a Completed today-due row is not owed either: {rows:?}"
    );
    for settled in ["File taxes", "Groceries"] {
        assert!(
            rows.iter().any(|r| r.contains(settled)),
            "{settled} is still drawn, it is just not counted: {rows:?}"
        );
    }
}

#[test]
fn a_fully_settled_overdue_group_drops_its_count_and_its_red() {
    let theme = Theme::from_flavor("mocha");
    let mut m = today_model(
        &["work"],
        vec![completed(dated(task("File taxes", "work"), day(19)))],
    );
    m.show_completed = true;
    let rows = spread(&m);
    assert!(
        rows.iter().any(|r| r.trim() == "Overdue"),
        "no count at zero outstanding, not `Overdue 0`: {rows:?}"
    );
    let fg = pane_fg(&buffer(&m), "Overdue");
    assert!(
        fg.iter().all(|c| *c == Some(theme.subtext)),
        "a spent group wears the dim label, not the urgent one: {fg:?}"
    );
}

#[test]
fn the_overdue_header_reads_urgent_while_work_is_owed() {
    let theme = Theme::from_flavor("mocha");
    // Positive, not merely "not `subtext`": the label must carry the same red the
    // dates beneath it do, which a negative assertion would not pin.
    let fg = pane_fg(&buffer(&both_groups()), "Overdue");
    assert!(
        fg.iter().all(|c| *c == Some(theme.overdue)),
        "the migration worklist announces itself: {fg:?}"
    );
}

#[test]
fn the_cursor_lands_on_the_selected_task_and_never_on_a_header() {
    // The header rows shift every display index below them. Walking the pane
    // top to bottom must find `›` beside the selected Task on every step — an
    // off-by-one in the translation puts it on a header or on a neighbour.
    let mut m = both_groups();
    let order = ["Dentist", "File taxes", "Ship 62", "Standup"];
    for (i, want) in order.iter().enumerate() {
        m.selected_task = m.tasks.iter().position(|t| t.title == *want);
        assert!(m.selected_task.is_some(), "{want} is in the aggregate");
        let cursor = spread(&m)
            .into_iter()
            .find(|r| r.starts_with('›'))
            .unwrap_or_else(|| panic!("no cursor row with {want} selected"));
        assert!(
            cursor.contains(want),
            "step {i}: cursor on {cursor:?}, expected {want}"
        );
    }
}

#[test]
fn the_due_column_comes_and_goes_with_the_overdue_group() {
    // With overdue rows: they print a date, and a today-due row's cell is blank
    // *at the same width*, so both titles start in the same column.
    let rows = spread(&both_groups());
    let overdue_row = rows
        .iter()
        .find(|r| r.contains("Dentist"))
        .expect("Dentist");
    let today_row = rows
        .iter()
        .find(|r| r.contains("Ship 62"))
        .expect("Ship 62");
    assert!(overdue_row.contains("2d ago"), "{overdue_row:?}");
    assert!(
        !today_row.contains("today"),
        "a today-due row's cell is blank, never the word: {today_row:?}"
    );
    assert_eq!(
        col_of(&body(overdue_row), "Dentist"),
        col_of(&body(today_row), "Ship 62"),
        "titles stay aligned across the fold: {overdue_row:?} / {today_row:?}"
    );

    // With none, the column is gone entirely rather than standing blank — and
    // the `Overdue` header goes in the same frame, so nothing shifts silently.
    let clean = spread(&today_model(&["work"], vec![task("Ship 62", "work")]));
    let row = clean
        .iter()
        .find(|r| r.contains("Ship 62"))
        .expect("Ship 62");
    assert!(!clean.join("\n").contains("Overdue"));
    assert_eq!(
        col_of(&body(row), "Ship 62"),
        Some(SIGNIFIER_GUTTER),
        "with no due column the title sits just past the signifier gutter: {row:?}"
    );
}

/// Cells the signifier gutter occupies: the glyph and the space after it.
const SIGNIFIER_GUTTER: usize = 2;

#[test]
fn the_signifier_gutter_is_reserved_even_on_an_all_task_day() {
    // Outside Today the cell only exists when something is typed. In the spread
    // it is always there, so a title holds its column as an Event or Note enters
    // or leaves the day.
    let plain = spread(&today_model(&["work"], vec![task("Ship 62", "work")]));
    let plain_row = plain.iter().find(|r| r.contains("Ship 62")).expect("row");

    let typed = spread(&today_model(
        &["work"],
        vec![task("Ship 62", "work"), task("○ Standup", "work")],
    ));
    let typed_row = typed.iter().find(|r| r.contains("Ship 62")).expect("row");

    assert_eq!(
        col_of(&body(plain_row), "Ship 62"),
        col_of(&body(typed_row), "Ship 62"),
        "the title must not shift when a typed entry joins the day",
    );
    assert!(
        typed.iter().any(|r| r.contains("○ Standup")),
        "the Event still draws its signifier: {typed:?}"
    );
}

#[test]
fn the_spread_degrades_with_the_rest_of_the_glyphs_under_ascii_fallback() {
    // The spread's own text is ASCII already — the dateline and both headers —
    // so `ascii_fallback` must leave them alone while the signifier degrades.
    let m = today_model(
        &["work"],
        vec![
            dated(task("Dentist", "work"), day(18)),
            task("○ Standup", "work"),
        ],
    );
    let rows = spread_of(&buffer_with(&m, true));
    let joined = rows.join("\n");
    assert!(joined.contains("Monday 20 July 2026"), "{joined}");
    assert!(
        joined.contains("Overdue 1") && joined.contains("Today 1"),
        "{joined}"
    );
    assert!(
        rows.iter().any(|r| r.contains("o Standup")),
        "the Event signifier degrades to ASCII: {rows:?}"
    );
}
