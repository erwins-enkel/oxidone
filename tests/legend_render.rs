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

/// Draw a frame and return its rows as strings.
fn rows(model: &Model) -> Vec<String> {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
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
