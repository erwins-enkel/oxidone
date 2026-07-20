//! The notes marker as actually drawn (#54). `notes_marker` and `has_notes_body`
//! are pure and unit-tested next to the view, but both pass whether or not `view`
//! ever puts their result on a row — and whether or not the row it lands on still
//! has room for everything else. That is what this file covers, following
//! `link_render.rs` and `meters_render.rs`.
//!
//! "Notes" here always means a Task's free-text body, edited with `n`. The Bullet
//! Journal `EntryType::Note` is a different thing that happens to share the word:
//! its `—` signifier *leads* the row, while this marker trails the title.

use chrono::{TimeZone, Utc};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::collections::HashMap;

const HEIGHT: u16 = 24;
/// The documented minimum terminal. Every budget assertion here is at this width,
/// because it is the one where the trailing cells actually compete.
const MIN_WIDTH: u16 = 80;

/// Draw a frame and return its rows as strings.
fn rows_at(model: &Model, width: u16, ascii: bool) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(width, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, ascii, frame))
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
    rows_at(model, MIN_WIDTH, false)
}

/// Whether each cell of row `y` spanning `text` carries `modifier`. `None` when
/// the row does not contain `text`.
fn modifier_over(model: &Model, y: usize, text: &str, modifier: Modifier) -> Option<bool> {
    let buffer = buffer_at(model, MIN_WIDTH);
    let row: String = (0..MIN_WIDTH)
        .map(|x| buffer[(x, y as u16)].symbol().to_string())
        .collect();
    let start = row.find(text)?;
    // `find` gives a byte offset; the rows here are cell-per-char, so walk the
    // chars to reach the matching column.
    let col = row[..start].chars().count();
    let span = text.chars().count();
    Some((col..col + span).all(|x| {
        buffer[(x as u16, y as u16)]
            .style()
            .add_modifier
            .contains(modifier)
    }))
}

fn buffer_at(model: &Model, width: u16) -> ratatui::buffer::Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(width, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn task(id: &str, title: &str, status: Status, parent: Option<&str>, notes: Option<&str>) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: parent.map(|p| TaskId(p.into())),
        title: title.into(),
        notes: notes.map(str::to_string),
        status,
        due: None,
        completed_at: None,
        position: format!("{id:0>20}"),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

/// `task`, with a due date — the branch that turns the pane's due gutter on.
fn task_due(id: &str, title: &str, parent: Option<&str>, notes: Option<&str>) -> Task {
    Task {
        due: Some(chrono::NaiveDate::from_ymd_opt(2026, 7, 20).expect("a real date")),
        ..task(id, title, Status::NeedsAction, parent, notes)
    }
}

/// One List named `L` holding `tasks`, task pane focused.
fn model_with(tasks: Vec<Task>) -> Model {
    let list = List {
        id: ListId("l".into()),
        title: "L".into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    };
    let mut model = Model::new();
    update(&mut model, Message::ListsLoaded(vec![list]));
    update(&mut model, Message::CountsLoaded(HashMap::new()));
    update(&mut model, Message::TasksLoaded(ListId("l".into()), tasks));
    model.focus = Focus::Tasks;
    model
}

/// Just the task pane's columns of a row. A terminal row spans *both* panes, so a
/// Task row shares its line with a sidebar row; asserting "no meter here" against
/// the whole line would trip over the sidebar's. The split is `Percentage(30)`.
fn task_pane(row: &str, width: u16) -> String {
    row.chars().skip(width as usize * 30 / 100).collect()
}

/// The drawn task-pane row containing `needle`.
fn pane_row(model: &Model, needle: &str) -> String {
    rows(model)
        .iter()
        .find(|r| r.contains(needle))
        .map(|r| task_pane(r, MIN_WIDTH))
        .unwrap_or_else(|| panic!("no row for {needle}"))
}

#[test]
fn a_task_with_a_notes_body_is_marked() {
    let model = model_with(vec![task(
        "1",
        "alpha",
        Status::NeedsAction,
        None,
        Some("ring the notary first"),
    )]);

    assert!(
        pane_row(&model, "alpha").contains('≡'),
        "expected the marker: {:?}",
        pane_row(&model, "alpha")
    );
}

#[test]
fn a_task_without_notes_is_not_marked() {
    let model = model_with(vec![task("1", "alpha", Status::NeedsAction, None, None)]);

    assert!(!pane_row(&model, "alpha").contains('≡'));
}

#[test]
fn a_notes_body_that_renders_blank_is_not_marked() {
    // The marker promises text `n` will show. Whitespace and bidi controls draw
    // nothing, so a marked row would be a promise the editor cannot keep.
    for blank in ["   ", "\n\t\n", "\u{202e}", " \u{200e}\u{061c} "] {
        let model = model_with(vec![task(
            "1",
            "alpha",
            Status::NeedsAction,
            None,
            Some(blank),
        )]);

        let row = pane_row(&model, "alpha");
        assert!(!row.contains('≡'), "notes {blank:?} drew a marker: {row:?}");
    }
}

#[test]
fn only_the_rows_with_a_notes_body_are_marked() {
    let model = model_with(vec![
        task("1", "alpha", Status::NeedsAction, None, Some("with notes")),
        task("2", "bravo", Status::NeedsAction, None, None),
    ]);

    assert!(pane_row(&model, "alpha").contains('≡'));
    assert!(!pane_row(&model, "bravo").contains('≡'));
}

#[test]
fn the_marker_degrades_to_ascii_with_the_braille_widgets() {
    let model = model_with(vec![task(
        "1",
        "alpha",
        Status::NeedsAction,
        None,
        Some("ring first"),
    )]);

    let row: String = rows_at(&model, MIN_WIDTH, true)
        .iter()
        .find(|r| r.contains("alpha"))
        .map(|r| task_pane(r, MIN_WIDTH))
        .expect("the alpha row");

    assert!(row.contains('='), "expected the ASCII marker: {row:?}");
    assert!(!row.contains('≡'), "no braille-era glyph: {row:?}");
}

#[test]
fn a_task_with_links_and_notes_carries_both_markers_in_order() {
    // They answer different questions — `u` has something to open, `n` has
    // something to read — so neither suppresses the other.
    let model = model_with(vec![task(
        "1",
        "alpha",
        Status::NeedsAction,
        None,
        Some("see https://a.dev/1"),
    )]);

    let row = pane_row(&model, "alpha");
    let link = row.find('⧉').expect("the link marker");
    let notes = row.find('≡').expect("the notes marker");
    assert!(link < notes, "expected `⧉` before `≡`: {row:?}");
}

#[test]
fn a_completed_row_strikes_its_notes_marker_with_its_title() {
    // `≡` is a fact about this Task's own text, like `⧉` — so it reads dim and
    // struck with the title. The Subtask meter is the one that keeps its
    // legibility instead.
    let mut model = model_with(vec![
        task(
            "p",
            "parent",
            Status::Completed,
            None,
            Some("see https://a.dev/1"),
        ),
        task("c1", "child", Status::Completed, Some("p"), None),
    ]);
    model.show_completed = true;

    let y = rows(&model)
        .iter()
        .position(|r| r.contains("parent"))
        .expect("the parent row");

    assert_eq!(
        modifier_over(&model, y, "≡", Modifier::CROSSED_OUT),
        Some(true),
        "the notes marker is struck with the title"
    );
    assert_eq!(
        modifier_over(&model, y, "⧉", Modifier::CROSSED_OUT),
        Some(true),
        "and reads the same as the link marker beside it"
    );
    assert_eq!(
        modifier_over(&model, y, "1/1", Modifier::CROSSED_OUT),
        Some(false),
        "the meter is not struck through"
    );
}

#[test]
fn a_parent_row_keeps_its_subtask_meter_beside_both_markers() {
    // The meter budgets around the markers, so it must be told about *both*. Fed
    // only the link marker's width it believes it has two more columns than the
    // row does, draws the wide bar form, and ratatui clips the overrun silently —
    // leaving a half-written ratio and no error anywhere.
    //
    // The title is sized into the window where that actually shows. At 80 columns
    // a top-level row with no due gutter has 52 usable cells; both markers take 4,
    // so a 40-cell title leaves 8 — enough for the bare `  0/1` (5) but not the
    // bar form (10). Under-fed, the meter measures 10 against a believed 10 and
    // draws the bar, which does not fit. A shorter title leaves slack and the bug
    // stays invisible, which is what an earlier version of this test proved.
    let title = "a title of exactly forty chars total xxx";
    assert_eq!(title.chars().count(), 40, "the window this test aims at");

    let model = model_with(vec![
        task(
            "p",
            title,
            Status::NeedsAction,
            None,
            Some("see https://a.dev/1"),
        ),
        task("c1", "child", Status::NeedsAction, Some("p"), None),
    ]);

    let row = pane_row(&model, title);
    assert!(
        row.contains('⧉') && row.contains('≡'),
        "both marks: {row:?}"
    );
    // The complete ratio is the assertion that matters: an under-fed budget draws
    // the wide bar form, ratatui clips it at the pane's inner edge — leaving the
    // border intact and a half-written `0/` behind — and nothing else notices.
    assert!(
        row.contains("0/1"),
        "the meter was clipped to fit cells the markers had already spent: {row:?}"
    );
}

#[test]
fn a_parent_row_with_a_due_date_keeps_its_meter_too() {
    // The due gutter is the other and larger branch of the meter's usable width
    // (`DUE_WIDTH + 2`), so the same mistake surfaces at a different title length:
    // 40 usable cells here, less 4 for the markers, so 27 leaves 9.
    let title = "twenty-eight chars of title";
    assert_eq!(title.chars().count(), 27);

    let model = model_with(vec![
        task_due("p", title, None, Some("see https://a.dev/1")),
        task_due("c1", "child", Some("p"), None),
    ]);

    let row = pane_row(&model, title);
    assert!(
        row.contains('⧉') && row.contains('≡'),
        "both marks: {row:?}"
    );
    assert!(
        row.contains("0/1"),
        "the meter was clipped under the due gutter: {row:?}"
    );
}

#[test]
fn the_due_gutter_stays_aligned_when_a_row_gains_a_marker() {
    // The markers trail the title, so a leading fixed-width column must not move.
    let model = model_with(vec![
        task_due("1", "alpha", None, Some("with notes")),
        task_due("2", "bravo", None, None),
    ]);

    // By *column*, not byte offset: the pane border and the cursor arrow are
    // multibyte, so `str::find` would report two aligned rows as differing purely
    // because one carries `›` and the other two spaces.
    let column_of = |needle: &str| -> usize {
        let row = pane_row(&model, needle);
        let at = row
            .find("2026")
            .unwrap_or_else(|| panic!("no due date on {needle}: {row:?}"));
        row[..at].chars().count()
    };
    assert_eq!(column_of("alpha"), column_of("bravo"));
}

#[test]
fn a_typed_pane_still_marks_and_still_fits() {
    // An Event in view turns on the signifier cell, which costs every row two
    // columns — so the markers and the meter compete for two fewer, and the
    // sensitive title is two shorter than on an untyped pane.
    let title = "a title of thirty-eight chars total xx";
    assert_eq!(title.chars().count(), 38);

    let model = model_with(vec![
        task(
            "p",
            title,
            Status::NeedsAction,
            None,
            Some("see https://a.dev/1"),
        ),
        task("c1", "child", Status::NeedsAction, Some("p"), None),
        task("e", "○ an event", Status::NeedsAction, None, Some("notes")),
    ]);

    let row = pane_row(&model, title);
    assert!(row.contains('≡'), "still marked on a typed pane: {row:?}");
    assert!(row.contains("0/1"), "the meter survives: {row:?}");
}
