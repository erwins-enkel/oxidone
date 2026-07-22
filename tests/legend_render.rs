//! The always-visible hotkey legend, as actually drawn. Unlike the rest of the
//! integration suite this one needs a terminal — a `TestBackend` one — because
//! the fitting logic passes whether or not `view` ever renders the row.
//!
//! The width assertions are pinned at 80×24 on purpose: an earlier draft of the
//! legend fitted fine in the abstract while silently dropping `q quit` off the
//! default terminal.

use oxidone::app::{Focus, Model};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// Draw a frame `width` columns wide and return its rows as strings. Widened
/// from a fixed 80 so the cells that fall below the default terminal's budget
/// can still be pinned somewhere — following `link_render.rs`.
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

fn model_with_status() -> Model {
    let mut model = Model::new();
    model.status_line = Some("Synced 5 tasks".to_string());
    model
}

#[test]
fn the_task_pane_legend_fits_eighty_columns_exactly() {
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    let rows = rows(&model);
    assert_eq!(
        rows.last().expect("a bottom row").trim_end(),
        "j/k move  h/l pane  q quit  Space done  a add  d due  x del  c completed  ? help"
    );
}

#[test]
fn the_sidebar_legend_swaps_in_the_list_verbs() {
    let mut model = model_with_status();
    model.focus = Focus::Sidebar;

    let rows = rows(&model);
    assert_eq!(
        rows.last().expect("a bottom row").trim_end(),
        "j/k move  h/l pane  q quit  a add  A add list  R rename  c completed      ? help"
    );
}

#[test]
fn the_legend_sits_below_the_status_line_without_hiding_it() {
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    let rows = rows(&model);
    let status = &rows[rows.len() - 2];
    assert_eq!(status.trim_end(), "Synced 5 tasks");
    assert!(
        rows.last().expect("a bottom row").contains("? help"),
        "the legend takes the row below the status line"
    );
}

#[test]
fn help_is_pinned_to_the_last_column() {
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    let legend = rows(&model).last().expect("a bottom row").clone();
    assert!(
        legend.ends_with("? help"),
        "expected help flush right, got {legend:?}"
    );
}

// --- Cells below the 80-column budget --------------------------------------
//
// The task legend's 80-column budget is 72 and the cells through `c completed`
// already total exactly 72, so anything added at or above `completed` evicts it.
// `m migrate` (and later `t/T type`) therefore sit *below* it and are invisible
// at the default width — these two tests pin both halves of that bargain.

/// Wide enough for every cell: the thirteen cells cost 117 cumulative and the
/// pinned help cell reserves 8, so 125 is the floor. 130 leaves five columns of
/// slack, so a one-character label change does not silently drop `u link`.
const WIDE: u16 = 130;

#[test]
fn migrate_is_below_the_eighty_column_cut_and_evicts_nothing() {
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    // The exact-string test above already pins the full row; this one states the
    // intent — `c completed` survives and `m migrate` is the one that yields.
    let legend = rows(&model)
        .last()
        .expect("a bottom row")
        .trim_end()
        .to_string();
    assert!(
        legend.contains("c completed"),
        "completed evicted: {legend}"
    );
    for below in ["migrate", "type"] {
        assert!(
            !legend.contains(below),
            "{below} must not fit 80 columns, or it displaced a cell: {legend}"
        );
    }
}

#[test]
fn a_wide_pane_reveals_migrate_above_edit_and_type_below_link() {
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    let rows = rows_at(&model, WIDE);
    let legend = rows.last().expect("a bottom row").trim_end().to_string();

    let at = |needle: &str| {
        legend
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} missing from {legend:?}"))
    };
    assert!(at("c completed") < at("m migrate"), "{legend}");
    assert!(at("m migrate") < at("e edit"), "{legend}");
    // `type` sits below `link`: anything above it evicts `link` at 120, where
    // `link_render.rs` pins it as present.
    assert!(at("u link") < at("t/T type"), "{legend}");
    assert!(legend.ends_with("? help"), "{legend}");
}

#[test]
fn the_new_cells_do_not_evict_link_at_the_width_where_it_fits() {
    // Regression: `t/T type` first landed above `link`, which pushed `link` off
    // at 120 — the width `link_render.rs` pins it as present. Adding a cell must
    // never cost an existing verb the width it already had.
    let mut model = model_with_status();
    model.focus = Focus::Tasks;

    let legend = rows_at(&model, 120)
        .last()
        .expect("a bottom row")
        .trim_end()
        .to_string();
    assert!(legend.contains("u link"), "link evicted at 120: {legend}");
    assert!(legend.contains("m migrate"), "{legend}");
}

// --- The due editor's legend -------------------------------------------------
//
// Its cells total 76 columns (10 + 10 + 14 + 17 + 8 + 7, plus five 2-column
// gaps), so the whole row fits the default terminal — but "it fits" is the
// weaker half. The row drops from the right, so the *order* is the drop order,
// and only a narrow width can observe it. A wide-only assertion would pass
// against a row that evicts `Esc cancel` first.

/// The due overlay, open on a Task with no due date (so the buffer is empty and
/// nothing about the prefill matters here).
fn model_with_due_editor() -> Model {
    let mut model = Model::new();
    model.overlay = Some(oxidone::app::Overlay::EditDue {
        task: oxidone::domain::TaskId("t".into()),
        buffer: String::new(),
        pristine: false,
    });
    model
}

#[test]
fn the_due_editor_legend_fits_the_default_terminal() {
    let model = model_with_due_editor();
    let row = rows(&model).last().expect("a legend row").clone();
    for cell in [
        "Enter save",
        "Esc cancel",
        "Up/Down -/+day",
        "PgUp/PgDn -/+week",
        "^U clear",
        "^W word",
    ] {
        assert!(
            row.contains(cell),
            "{cell:?} missing at 80 columns: {row:?}"
        );
    }
}

/// Narrowing evicts from the right, in order: `^W`, then `^U`, then the stepping
/// keys — and `Enter`/`Esc` survive longest, because not knowing them strands
/// you in the overlay while not knowing `^U` costs a few Backspaces.
#[test]
fn the_due_editor_legend_drops_the_chords_before_the_escape_hatches() {
    let model = model_with_due_editor();

    let narrow = rows_at(&model, 70).last().expect("a legend row").clone();
    assert!(
        !narrow.contains("^W word"),
        "^W should drop first at 70 columns: {narrow:?}"
    );
    assert!(narrow.contains("Enter save"), "{narrow:?}");
    assert!(narrow.contains("Esc cancel"), "{narrow:?}");

    let narrower = rows_at(&model, 30).last().expect("a legend row").clone();
    assert!(!narrower.contains("^U clear"), "{narrower:?}");
    assert!(!narrower.contains("Up/Down"), "{narrower:?}");
    assert!(
        narrower.contains("Enter save") && narrower.contains("Esc cancel"),
        "the escape hatches are the last to go: {narrower:?}"
    );
}
