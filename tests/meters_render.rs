//! The completion meters as actually drawn (#46). Unlike the reducer tests this
//! one needs a terminal — a `TestBackend` one — because the width budgeting can
//! be perfectly correct in isolation while `view` never calls it, calls it with
//! the wrong width, or drops the `ascii` flag on the way through.

use chrono::{TimeZone, Utc};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::domain::{List, ListId, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;
use std::collections::HashMap;

const HEIGHT: u16 = 24;

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
    rows_at(model, 100, false)
}

/// Whether each cell of row `y` spanning `text` carries `modifier`. Returns
/// `None` when the row does not contain `text` at all.
fn modifier_over(model: &Model, y: usize, text: &str, modifier: Modifier) -> Option<bool> {
    let width = 100;
    let mut terminal =
        Terminal::new(TestBackend::new(width, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    let buffer = terminal.backend().buffer().clone();
    let row: String = (0..width)
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

fn task(id: &str, title: &str, status: Status, parent: Option<&str>, notes: Option<&str>) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: parent.map(|p| TaskId(p.into())),
        title: title.into(),
        notes: notes.map(str::to_string),
        status,
        due: None,
        completed_at: (status == Status::Completed).then(|| Utc.timestamp_opt(1, 0).unwrap()),
        position: format!("{id:0>20}"),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

/// One List named `L` holding `tasks`, task pane focused, with `counts` seeded
/// for the sidebar. `Model::new()` has an empty sidebar, so the List has to be
/// loaded before any meter can be drawn.
fn model_with(tasks: Vec<Task>, counts: &[(&str, (usize, usize))]) -> Model {
    let list = List {
        id: ListId("l".into()),
        title: "L".into(),
        etag: String::new(),
        updated: Utc.timestamp_opt(0, 0).unwrap(),
    };
    let mut model = Model::new();
    update(&mut model, Message::ListsLoaded(vec![list]));
    let seeded: HashMap<ListId, (usize, usize)> = counts
        .iter()
        .map(|(id, c)| (ListId((*id).to_string()), *c))
        .collect();
    update(&mut model, Message::CountsLoaded(seeded));
    update(&mut model, Message::TasksLoaded(ListId("l".into()), tasks));
    model.focus = Focus::Tasks;
    model
}

/// The sidebar row for List `L` — the first row inside the panel border.
fn sidebar_row(rows: &[String]) -> String {
    rows[1].clone()
}

/// Just the task pane's columns of a row. A terminal row spans *both* panes, so
/// the first Task row shares its line with the sidebar's first List row — and
/// asserting "this Task has no meter" against the whole line would trip over the
/// sidebar's. The split is ratatui's `Percentage(30)`.
fn task_pane(row: &str, width: u16) -> String {
    row.chars().skip(width as usize * 30 / 100).collect()
}

/// The first `<digits>/<digits>` in `s`. Parsed by scanning rather than by
/// splitting on whitespace: in the pane header the ratio runs straight into the
/// panel's border dashes with no space between.
fn first_ratio(s: &str) -> Option<(usize, usize)> {
    let chars: Vec<char> = s.chars().collect();
    let slash_positions = chars
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '/')
        .map(|(i, _)| i);
    for slash in slash_positions {
        let start = chars[..slash]
            .iter()
            .rposition(|c| !c.is_ascii_digit())
            .map_or(0, |i| i + 1);
        let end = chars[slash + 1..]
            .iter()
            .position(|c| !c.is_ascii_digit())
            .map_or(chars.len(), |i| slash + 1 + i);
        if start == slash || end == slash + 1 {
            continue; // digits missing on one side
        }
        let done: usize = chars[start..slash]
            .iter()
            .collect::<String>()
            .parse()
            .ok()?;
        let total: usize = chars[slash + 1..end]
            .iter()
            .collect::<String>()
            .parse()
            .ok()?;
        return Some((done, total));
    }
    None
}

#[test]
fn a_sidebar_list_row_draws_its_meter() {
    let model = model_with(
        vec![
            task("1", "alpha", Status::Completed, None, None),
            task("2", "beta", Status::NeedsAction, None, None),
        ],
        &[("l", (1, 2))],
    );

    let row = sidebar_row(&rows(&model));
    assert!(row.contains('L'), "the title still renders: {row:?}");
    assert!(row.contains("1/2"), "expected the ratio: {row:?}");
    assert!(
        row.contains('\u{28FF}') || row.contains('\u{2800}'),
        "expected a braille bar: {row:?}"
    );
}

#[test]
fn the_sidebar_meter_honours_the_ascii_fallback() {
    // Only a real frame can prove the flag survives the trip from `view` into
    // the sidebar — the pure width helper is handed `ascii` either way.
    let model = model_with(
        vec![task("1", "alpha", Status::NeedsAction, None, None)],
        &[("l", (1, 2))],
    );

    let row = sidebar_row(&rows_at(&model, 100, true));
    assert!(row.contains('#') || row.contains('-'), "{row:?}");
    assert!(
        !row.contains('\u{28FF}') && !row.contains('\u{2800}'),
        "no braille under the fallback: {row:?}"
    );
}

#[test]
fn the_sidebar_meter_agrees_with_the_task_pane_header() {
    // Two meters for one List must never disagree. `done/total` cannot be read
    // back out of a bar — `meter::render` quantises, and the two bars are
    // different widths — so re-render the header's ratio at the sidebar's width
    // and compare the cells.
    let model = model_with(
        vec![
            task("1", "alpha", Status::Completed, None, None),
            task("2", "beta", Status::NeedsAction, None, None),
            task("3", "gamma", Status::NeedsAction, Some("1"), None),
        ],
        &[("l", (9, 9))], // deliberately wrong: the active List derives live
    );

    let drawn = rows(&model);
    let header = drawn[0].clone();
    let (done, total) =
        first_ratio(&header).unwrap_or_else(|| panic!("no ratio in the header: {header:?}"));
    assert_eq!((done, total), (1, 3), "Subtasks count in both meters");

    let expected = oxidone::ui::widgets::meter::render(done, total, 6, false);
    assert!(
        sidebar_row(&drawn).contains(&expected),
        "sidebar bar should be the header's ratio at the sidebar's width:\n{}",
        drawn.join("\n")
    );
}

#[test]
fn a_list_with_no_counts_draws_only_its_title() {
    let model = model_with(vec![], &[]);
    let row = sidebar_row(&rows(&model));
    assert!(row.contains('L'));
    assert!(!row.contains('/'), "no ratio without counts: {row:?}");
    assert!(
        !row.contains('\u{28FF}') && !row.contains('\u{2800}'),
        "no bar without counts: {row:?}"
    );
}

#[test]
fn a_narrow_sidebar_keeps_the_title_and_drops_the_meter() {
    let model = model_with(
        vec![task("1", "alpha", Status::NeedsAction, None, None)],
        &[("l", (1, 2))],
    );

    let row = sidebar_row(&rows_at(&model, 40, false));
    assert!(row.contains('L'), "the title always survives: {row:?}");
    assert!(
        !row.contains('\u{28FF}') && !row.contains('\u{2800}'),
        "braille goes before text: {row:?}"
    );
}

#[test]
fn a_parent_row_draws_its_subtask_meter() {
    let model = model_with(
        vec![
            task("p", "parent", Status::NeedsAction, None, None),
            task("c1", "child one", Status::Completed, Some("p"), None),
            task("c2", "child two", Status::NeedsAction, Some("p"), None),
        ],
        &[("l", (1, 3))],
    );

    let parent = rows(&model)
        .into_iter()
        .find(|r| r.contains("parent"))
        .map(|r| task_pane(&r, 100))
        .expect("the parent row");
    assert!(
        parent.contains("1/2"),
        "expected the subtask ratio: {parent:?}"
    );
}

#[test]
fn a_parent_with_a_link_draws_both_the_marker_and_the_meter() {
    // #57's marker is not this widget's information to spend, so both fit.
    let model = model_with(
        vec![
            task(
                "p",
                "parent",
                Status::NeedsAction,
                None,
                Some("see https://example.com/x"),
            ),
            task("c1", "child one", Status::Completed, Some("p"), None),
        ],
        &[("l", (1, 2))],
    );

    let parent = rows(&model)
        .into_iter()
        .find(|r| r.contains("parent"))
        .map(|r| task_pane(&r, 100))
        .expect("the parent row");
    assert!(parent.contains('⧉'), "marker kept: {parent:?}");
    assert!(parent.contains("1/1"), "meter kept: {parent:?}");
}

#[test]
fn a_completed_parent_strikes_its_title_and_marker_but_not_its_meter() {
    // Braille struck through is unreadable, so the meter drops the strike — but
    // it keeps the dimmed foreground, or it would be the brightest run on a row
    // deliberately faded out.
    let model = model_with(
        vec![
            task(
                "p",
                "parent",
                Status::Completed,
                None,
                Some("see https://example.com/x"),
            ),
            task("c1", "child one", Status::Completed, Some("p"), None),
        ],
        &[("l", (2, 2))],
    );
    let mut model = model;
    model.show_completed = true;

    let y = rows(&model)
        .iter()
        .position(|r| r.contains("parent"))
        .expect("the parent row");

    assert_eq!(
        modifier_over(&model, y, "parent", Modifier::CROSSED_OUT),
        Some(true),
        "the title is struck through"
    );
    assert_eq!(
        modifier_over(&model, y, "⧉", Modifier::CROSSED_OUT),
        Some(true),
        "#57's marker belongs to the Task's text, so it is struck too"
    );
    assert_eq!(
        modifier_over(&model, y, "1/1", Modifier::CROSSED_OUT),
        Some(false),
        "the meter is not struck through"
    );
}

#[test]
fn the_subtask_meter_ignores_the_completed_filter() {
    // With the filter on, a parent whose Subtasks are all done shows no children
    // at all — the meter is then the only evidence they exist, so `0/2` would be
    // the exact opposite of the truth.
    let model = model_with(
        vec![
            task("p", "parent", Status::NeedsAction, None, None),
            task("c1", "child one", Status::Completed, Some("p"), None),
            task("c2", "child two", Status::Completed, Some("p"), None),
        ],
        &[("l", (2, 3))],
    );

    let drawn = rows(&model);
    assert!(!model.show_completed);
    assert!(
        !drawn.join("\n").contains("child"),
        "no child rows are drawn:\n{}",
        drawn.join("\n")
    );
    let parent = drawn
        .iter()
        .find(|r| r.contains("parent"))
        .map(|r| task_pane(r, 100))
        .expect("the parent row");
    assert!(parent.contains("2/2"), "not 0/2: {parent:?}");
}

#[test]
fn a_depth_two_row_is_nobodys_subtask_on_screen() {
    // `deep`'s parent is itself a Subtask, so the pane draws `deep` flush-left as
    // its own group. A meter counting raw `parent` would give `a1` one and credit
    // `A` a child it does not nest.
    let model = model_with(
        vec![
            task("A", "alpha", Status::NeedsAction, None, None),
            task("a1", "bravo", Status::NeedsAction, Some("A"), None),
            task("deep", "charlie", Status::Completed, Some("a1"), None),
        ],
        &[("l", (1, 3))],
    );
    let mut model = model;
    model.show_completed = true;

    let drawn = rows(&model);
    let row_of = |needle: &str| {
        let row = drawn
            .iter()
            .find(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("no row for {needle}"));
        task_pane(row, 100)
    };

    assert!(row_of("alpha").contains("0/1"), "A counts a1 only");
    assert!(!row_of("bravo").contains('/'), "a1 gets no meter");
    assert!(!row_of("charlie").contains('/'), "deep gets no meter");
}

#[test]
fn an_orphan_and_its_child_draw_without_meters() {
    let model = model_with(
        vec![
            task(
                "orphan",
                "alpha",
                Status::NeedsAction,
                Some("missing"),
                None,
            ),
            task("child", "bravo", Status::Completed, Some("orphan"), None),
        ],
        &[("l", (1, 2))],
    );
    let mut model = model;
    model.show_completed = true;

    let drawn = rows(&model);
    for needle in ["alpha", "bravo"] {
        let row = drawn
            .iter()
            .find(|r| r.contains(needle))
            .unwrap_or_else(|| panic!("no row for {needle}"));
        let pane = task_pane(row, 100);
        assert!(!pane.contains('/'), "{needle} gets no meter: {pane:?}");
    }
}
