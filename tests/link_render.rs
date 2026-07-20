//! The link feature *as drawn*. `link_marker` and `picker_height` are pure and
//! unit-tested next to the view, but both pass whether or not `view` ever puts
//! their result on screen — that is what this file covers, following
//! `legend_render.rs`.

use oxidone::app::{Focus, Message, Model};
use oxidone::domain::{List, ListId, Status, Task, TaskId};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const HEIGHT: u16 = 24;

/// Draw a frame `width` columns wide and return its rows as strings.
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
    rows_at(model, 80)
}

fn task(id: &str, title: &str, notes: Option<&str>) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: None,
        title: title.into(),
        notes: notes.map(str::to_string),
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        position: id.into(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    }
}

/// A model showing one List and the given Tasks, task pane focused.
fn model_with(tasks: Vec<Task>) -> Model {
    let list = List {
        id: ListId("l".into()),
        title: "L".into(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    };
    let mut model = Model::new();
    ui_update(&mut model, Message::ListsLoaded(vec![list]));
    ui_update(&mut model, Message::TasksLoaded(ListId("l".into()), tasks));
    model.focus = Focus::Tasks;
    model
}

fn ui_update(model: &mut Model, message: Message) {
    let _ = oxidone::app::update(model, message);
}

#[test]
fn a_task_with_an_openable_url_in_its_notes_is_marked() {
    let model = model_with(vec![task("1", "alpha", Some("ticket https://a.dev/1"))]);

    let drawn = rows(&model).join("\n");

    assert!(
        drawn.contains("alpha ⧉"),
        "expected the marker after the title, got:\n{drawn}"
    );
}

#[test]
fn a_task_whose_notes_hold_only_unopenable_urls_is_not_marked() {
    // The marker promises `u` will open something. A `file:` URL is found but
    // refused, so the row must stay bare.
    let model = model_with(vec![task("1", "alpha", Some("backup file:///srv/dump"))]);

    let drawn = rows(&model).join("\n");

    assert!(drawn.contains("alpha"), "the task itself should render");
    assert!(!drawn.contains('⧉'), "expected no marker, got:\n{drawn}");
}

#[test]
fn a_task_without_notes_is_not_marked() {
    let model = model_with(vec![task("1", "alpha", None)]);

    assert!(!rows(&model).join("\n").contains('⧉'));
}

#[test]
fn only_the_rows_with_links_are_marked() {
    let model = model_with(vec![
        task("1", "alpha", Some("https://a.dev/1")),
        task("2", "beta", None),
    ]);

    let drawn = rows(&model);
    let alpha = drawn
        .iter()
        .find(|r| r.contains("alpha"))
        .expect("alpha row");
    let beta = drawn.iter().find(|r| r.contains("beta")).expect("beta row");

    assert!(alpha.contains('⧉'), "alpha has a link: {alpha:?}");
    assert!(!beta.contains('⧉'), "beta has none: {beta:?}");
}

#[test]
fn the_picker_draws_its_urls() {
    let mut model = model_with(vec![task(
        "1",
        "alpha",
        Some("https://a.dev/1 and https://b.dev/2"),
    )]);
    ui_update(&mut model, Message::Key(press('u')));

    let drawn = rows(&model).join("\n");

    assert!(
        drawn.contains("Links"),
        "expected the picker panel:\n{drawn}"
    );
    assert!(
        drawn.contains("https://a.dev/1"),
        "first URL missing:\n{drawn}"
    );
    assert!(
        drawn.contains("https://b.dev/2"),
        "second URL missing:\n{drawn}"
    );
}

#[test]
fn the_picker_legend_advertises_its_own_keys_not_the_text_input_ones() {
    let mut model = model_with(vec![task(
        "1",
        "alpha",
        Some("https://a.dev/1 and https://b.dev/2"),
    )]);
    ui_update(&mut model, Message::Key(press('u')));

    let legend = rows(&model).last().expect("a bottom row").clone();

    assert_eq!(legend.trim_end(), "j/k move  Enter open  Esc cancel");
    assert!(
        !legend.contains("save"),
        "Enter opens here, it does not save: {legend:?}"
    );
}

#[test]
fn an_over_long_url_is_truncated_rather_than_overflowing_the_popup() {
    let long = format!("https://a.dev/{}", "x".repeat(120));
    let mut model = model_with(vec![task(
        "1",
        "alpha",
        Some(&format!("{long} https://b.dev/2")),
    )]);
    ui_update(&mut model, Message::Key(press('u')));

    let drawn = rows(&model);
    assert!(
        drawn.iter().any(|r| r.contains('…')),
        "expected an ellipsis on the truncated row:\n{}",
        drawn.join("\n")
    );
    // The popup is 50 wide; nothing may spill past it into the pane behind.
    assert!(!drawn.join("\n").contains(&long));
}

#[test]
fn a_double_width_url_is_truncated_by_cells_not_characters() {
    // 60 CJK characters are 60 chars but 120 display cells. Budgeting by chars
    // would keep ~45 of them — ~90 cells — and overrun the 50-cell popup, which
    // ratatui then clips with no ellipsis: a truncated URL reading as a whole one.
    let wide = format!("https://例え.jp/{}", "テ".repeat(60));
    let mut model = model_with(vec![task(
        "1",
        "alpha",
        Some(&format!("{wide} https://b.dev/2")),
    )]);
    ui_update(&mut model, Message::Key(press('u')));

    let drawn = rows(&model);
    let truncated = drawn
        .iter()
        .find(|r| r.contains('…'))
        .unwrap_or_else(|| panic!("expected a truncated row:\n{}", drawn.join("\n")));
    let plain = drawn
        .iter()
        .find(|r| r.contains("https://b.dev/2"))
        .expect("the short URL renders untruncated");

    // Border *columns*, not byte offsets — the truncated row is full of
    // multibyte characters. Identical layouts on both picker rows is precisely
    // what "did not overrun the popup" means, and what a char-based budget breaks.
    let borders = |row: &str| -> Vec<usize> {
        row.chars()
            .enumerate()
            .filter(|(_, c)| *c == '│')
            .map(|(i, _)| i)
            .collect()
    };
    assert_eq!(
        borders(truncated),
        borders(plain),
        "the wide row overran the popup:\n{}",
        drawn.join("\n")
    );
    assert!(!drawn.join("\n").contains(&wide), "nothing may spill whole");
}

#[test]
fn the_link_cell_is_absent_at_eighty_columns_and_present_when_it_fits() {
    // The accepted cost, pinned: at the default width `u` is only in `?`.
    let model = model_with(vec![task("1", "alpha", None)]);

    let narrow = rows_at(&model, 80).last().expect("a bottom row").clone();
    assert!(
        !narrow.contains("u link"),
        "80 cols has no room: {narrow:?}"
    );

    let wide = rows_at(&model, 120).last().expect("a bottom row").clone();
    assert!(
        wide.contains("u link"),
        "expected the cell at 120: {wide:?}"
    );
}

fn press(c: char) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Char(c),
        crossterm::event::KeyModifiers::empty(),
    )
}
