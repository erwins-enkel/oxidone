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
    assert!(
        !legend.contains("migrate"),
        "migrate must not fit 80 columns, or it displaced a cell: {legend}"
    );
}

#[test]
fn a_wide_pane_reveals_migrate_between_completed_and_edit() {
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
    // `link` is last and must stay that way — the reason WIDE is 130, not 120.
    assert!(legend.ends_with("? help"), "{legend}");
    assert!(at("s sort") < at("u link"), "{legend}");
}
