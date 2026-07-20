//! Entry-type signifiers *as drawn*. `signifier` is pure and unit-tested next to
//! the view, but it passes whether or not `view` ever puts its result on screen —
//! and where it lands in the row is the part that can go wrong. Follows
//! `link_render.rs`.

use oxidone::app::{Focus, Model};
use oxidone::domain::{List, ListId, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn rows(model: &Model, ascii: bool) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, ascii, frame))
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

/// Draw a frame and return the buffer, so a test can read cell *styles* and not
/// just symbols.
fn buffer_of(model: &Model) -> ratatui::buffer::Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// The (x, y) of the first cell whose symbol is `needle`.
fn cell_at(buffer: &ratatui::buffer::Buffer, needle: &str) -> (u16, u16) {
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if buffer[(x, y)].symbol() == needle {
                return (x, y);
            }
        }
    }
    panic!("{needle:?} not drawn");
}

/// The x of the first cell on row `y` whose symbol is `needle`. Row-scoped
/// because the pane borders carry titles of their own — a bare search for "s"
/// finds the "s" in "Lists" long before it reaches a task row.
fn cell_on_row(buffer: &ratatui::buffer::Buffer, y: u16, needle: &str) -> u16 {
    (0..WIDTH)
        .find(|&x| buffer[(x, y)].symbol() == needle)
        .unwrap_or_else(|| panic!("{needle:?} not on row {y}"))
}

fn task(id: &str, title: &str) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: None,
        title: title.into(),
        notes: None,
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        position: id.into(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    }
}

fn model_with(tasks: Vec<Task>) -> Model {
    let mut model = Model::new();
    model.lists = vec![List {
        id: ListId("l".into()),
        title: "L".into(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    }];
    model.selected_list = Some(0);
    model.selected_task = Some(0);
    model.tasks = tasks;
    model.focus = Focus::Tasks;
    model
}

/// Column (not byte) index of `needle` in `row`. The pane is full of multi-byte
/// glyphs — borders, the cursor arrow, the signifiers themselves — so `str::find`
/// would compare byte offsets where the assertion means screen columns.
fn column_of(row: &str, needle: char) -> usize {
    row.chars()
        .position(|c| c == needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"))
}

/// Column index where `needle` starts, in characters.
fn column_where(row: &str, needle: &str) -> usize {
    let byte = row
        .find(needle)
        .unwrap_or_else(|| panic!("{needle:?} not in {row:?}"));
    row[..byte].chars().count()
}

/// The row a title appears on, trailing blanks trimmed.
fn row_with<'a>(drawn: &'a [String], needle: &str) -> &'a str {
    drawn
        .iter()
        .find(|r| r.contains(needle))
        .unwrap_or_else(|| panic!("{needle:?} not drawn in:\n{}", drawn.join("\n")))
        .trim_end()
}

#[test]
fn a_typed_entry_draws_its_signifier_before_the_title() {
    let model = model_with(vec![task("1", "○ standup"), task("2", "— jotting")]);
    let drawn = rows(&model, false);

    assert!(row_with(&drawn, "standup").contains("○ standup"));
    assert!(row_with(&drawn, "jotting").contains("— jotting"));
}

#[test]
fn an_all_task_pane_draws_no_signifier_cell_at_all() {
    // Conditional like the due gutter: on the overwhelmingly common all-Task
    // pane, a column of blanks would spend width to say "ordinary".
    let model = model_with(vec![task("1", "alpha"), task("2", "beta")]);
    let drawn = rows(&model, false);

    let alpha = row_with(&drawn, "alpha");
    let beta = row_with(&drawn, "beta");
    // The cursor gutter is the only thing between the border and the title.
    assert!(alpha.contains("› alpha"), "{alpha:?}");
    assert!(beta.contains("  beta"), "{beta:?}");
}

#[test]
fn a_task_sharing_a_pane_with_a_typed_entry_is_padded_to_the_same_column() {
    // Both rows must start their title at the same x, or the pane staggers.
    let model = model_with(vec![task("1", "○ standup"), task("2", "alpha")]);
    let drawn = rows(&model, false);

    let typed = row_with(&drawn, "standup");
    let plain = row_with(&drawn, "alpha");
    assert_eq!(
        column_where(typed, "standup"),
        column_where(plain, "alpha"),
        "titles misaligned:\n{typed:?}\n{plain:?}"
    );
}

#[test]
fn signifiers_degrade_with_the_braille_widgets() {
    let model = model_with(vec![task("1", "○ standup"), task("2", "— jotting")]);
    let drawn = rows(&model, true);

    let standup = row_with(&drawn, "standup");
    let jotting = row_with(&drawn, "jotting");
    assert!(standup.contains("o standup"), "{standup:?}");
    assert!(jotting.contains("- jotting"), "{jotting:?}");

    // Scoped to the rows, not the frame: the pane title carries its own em-dash
    // ("Tasks — due"), which is chrome and not a signifier.
    for row in [standup, jotting] {
        assert!(!row.contains('○'), "circle survived ascii mode: {row:?}");
        assert!(!row.contains('—'), "em dash survived ascii mode: {row:?}");
    }
}

#[test]
fn a_subtasks_signifier_is_indented_with_it_not_hoisted_out() {
    // Hoisted outside the indent, a Subtask's glyph would share a column with
    // its parent's and flatten the only cue telling them apart.
    let mut child = task("2", "○ child");
    child.parent = Some(TaskId("1".into()));
    let model = model_with(vec![task("1", "○ parent"), child]);
    let drawn = rows(&model, false);

    let parent = row_with(&drawn, "parent");
    let kid = row_with(&drawn, "child");
    assert!(
        column_of(kid, '○') > column_of(parent, '○'),
        "the subtask's signifier should sit further right:\n{parent:?}\n{kid:?}"
    );
}

#[test]
fn a_completed_entrys_signifier_is_styled_like_its_title() {
    // The signifier is row *content*, not chrome: a Completed Event must read as
    // one settled line — glyph, title and link marker all dim and struck through
    // together. Pushing the span with `Style::new()` instead of the row's style
    // would leave every other test green while the glyph rendered bright beside
    // its struck-out title.
    let mut done = task("1", "○ standup");
    done.status = Status::Completed;
    let mut model = model_with(vec![done]);
    model.show_completed = true; // Completed are hidden by default

    let buffer = buffer_of(&model);
    let (gx, row) = cell_at(&buffer, "○");
    let tx = cell_on_row(&buffer, row, "s"); // first letter of "standup"

    assert_eq!(
        buffer[(gx, row)].style(),
        buffer[(tx, row)].style(),
        "the signifier must carry the row style, not its own"
    );
    assert!(
        buffer[(gx, row)]
            .style()
            .add_modifier
            .contains(ratatui::style::Modifier::CROSSED_OUT),
        "a Completed entry's signifier should be struck through with its title"
    );
}

#[test]
fn a_dated_note_still_shows_its_date_in_the_due_gutter() {
    // The other half of a deliberate asymmetry: Notes are excluded from the
    // due-load histogram but *not* from the per-row gutter. The gutter answers
    // "does this entry carry a date?" — a dated Note does — while the histogram
    // answers "how much work is coming?", which a Note is not. Without this,
    // a later reader could "fix" the gutter to match the histogram and think
    // they were removing an inconsistency.
    // Pin the clock rather than leaning on the wall one: `Model::new()` does not
    // stamp `now` from it, and the gutter falls back to an ISO date beyond the
    // relative horizon.
    use chrono::TimeZone;
    let now = chrono::Local
        .with_ymd_and_hms(2026, 3, 10, 9, 0, 0)
        .single()
        .expect("a valid local time");
    let mut note = task("1", "— jotting");
    note.due = chrono::NaiveDate::from_ymd_opt(2026, 3, 11); // tomorrow
    let mut model = model_with(vec![note]);
    model.now = now;

    let drawn = rows(&model, false);
    let row = row_with(&drawn, "jotting");
    assert!(
        row.contains("tomorrow"),
        "a dated Note should still show its date in the gutter: {row:?}"
    );
}
