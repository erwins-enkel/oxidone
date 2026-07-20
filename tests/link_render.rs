//! The link feature *as drawn*. `link_marker` and `picker_height` are pure and
//! unit-tested next to the view, but both pass whether or not `view` ever puts
//! their result on screen — that is what this file covers, following
//! `legend_render.rs`.

use oxidone::app::{Focus, Message, Model};
use oxidone::domain::{List, ListId, Status, Task, TaskId, TaskLink};
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

/// The link picker popup's own rows: its `Links` title down through its rounded
/// bottom border. Both panes draw `│` and `╰` of their own, so a row cannot be
/// told from a pane row by counting borders — the popup's title and its
/// bottom-right `╯` (`BorderType::Rounded`) are what bound it. Anchoring here keeps
/// a picker assertion from being met instead by the notes preview now drawn on the
/// pane behind the popup.
fn picker_lines(drawn: &[String]) -> Vec<String> {
    let start = drawn
        .iter()
        .position(|r| r.contains("Links"))
        .expect("the picker's Links title");
    let mut out = Vec::new();
    for row in &drawn[start..] {
        out.push(row.clone());
        if row.contains('╯') {
            break;
        }
    }
    out
}

fn task(id: &str, title: &str, notes: Option<&str>) -> Task {
    task_with_links(id, title, notes, Vec::new())
}

fn task_with_links(id: &str, title: &str, notes: Option<&str>, links: Vec<TaskLink>) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId("l".into()),
        parent: None,
        title: title.into(),
        notes: notes.map(str::to_string),
        status: Status::NeedsAction,
        due: None,
        completed_at: None,
        links,
        position: id.into(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    }
}

fn link(url: &str, description: Option<&str>) -> TaskLink {
    TaskLink {
        url: url.into(),
        description: description.map(str::to_string),
        kind: Some("email".into()),
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
fn a_task_with_an_openable_links_entry_but_no_notes_is_marked() {
    // #55: a Gmail-created Task has its URL in `links[]`, not its notes.
    let model = model_with(vec![task_with_links(
        "1",
        "alpha",
        None,
        vec![link("https://mail.example/msg", Some("Re: hi"))],
    )]);

    let drawn = rows(&model).join("\n");

    assert!(
        drawn.contains("alpha ⧉"),
        "expected the marker after the title, got:\n{drawn}"
    );
}

#[test]
fn a_task_with_only_a_non_openable_links_entry_is_not_marked() {
    // A `mailto:` in `links[]` is refused by the http/https allowlist, so the
    // marker must not promise `u` will open it.
    let model = model_with(vec![task_with_links(
        "1",
        "alpha",
        None,
        vec![link("mailto:a@b.c", Some("email"))],
    )]);

    let drawn = rows(&model).join("\n");

    assert!(drawn.contains("alpha"), "the task itself should render");
    assert!(!drawn.contains('⧉'), "expected no marker, got:\n{drawn}");
}

#[test]
fn the_picker_draws_a_links_entry_as_description_then_url() {
    let mut model = model_with(vec![task_with_links(
        "1",
        "alpha",
        Some("also https://b.dev/2"),
        vec![link("https://a.dev/1", Some("from Gmail"))],
    )]);
    ui_update(&mut model, Message::Key(press('u')));

    let drawn = rows(&model);
    let frame = drawn.join("\n");
    let picker = picker_lines(&drawn).join("\n");

    // Frame-wide: the pane preview shows the notes URL, never the `links[]`
    // description, so only the picker can carry this row.
    assert!(
        frame.contains("from Gmail — https://a.dev/1"),
        "expected the described links[] row:\n{frame}"
    );
    // Scoped: the notes `also https://b.dev/2` now draw whole on the pane behind
    // the popup, so this must prove the picker drew the URL, not the preview.
    assert!(
        picker.contains("https://b.dev/2"),
        "expected the bare notes URL row in the picker:\n{frame}"
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

    let drawn = rows(&model);
    let picker = picker_lines(&drawn).join("\n");

    assert!(
        picker.contains("Links"),
        "expected the picker panel:\n{}",
        drawn.join("\n")
    );
    // Scoped to the picker: this Task's 35-cell notes fit the pane preview whole,
    // so an unscoped search would pass without the picker drawing anything.
    assert!(
        picker.contains("https://a.dev/1"),
        "first URL missing from the picker:\n{}",
        drawn.join("\n")
    );
    assert!(
        picker.contains("https://b.dev/2"),
        "second URL missing from the picker:\n{}",
        drawn.join("\n")
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
        picker_lines(&drawn).iter().any(|r| r.contains('…')),
        "expected an ellipsis on the truncated picker row:\n{}",
        drawn.join("\n")
    );
    // Frame-wide, and a stronger claim than the ellipsis: nothing may spill the
    // whole URL anywhere — the pane preview truncates it too.
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
    // Both rows are scoped to the picker: the wide URL's preview now draws on the
    // pane too, so an unscoped `find` would compare a pane row's borders against a
    // popup row's — different layouts, and the assertion below would be meaningless.
    let picker = picker_lines(&drawn);
    let truncated = picker
        .iter()
        .find(|r| r.contains('…'))
        .unwrap_or_else(|| panic!("expected a truncated picker row:\n{}", drawn.join("\n")));
    let plain = picker
        .iter()
        .find(|r| r.contains("https://b.dev/2"))
        .expect("the short URL renders untruncated in the picker");

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
fn a_long_picker_never_covers_the_status_line_or_its_own_legend() {
    // 30 URLs is more rows than the frame has. The popup must stop above the
    // bottom chrome — the legend down there is what tells you `Enter` opens.
    let many: String = (0..30)
        .map(|i| format!("https://a.dev/{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut model = model_with(vec![task("1", "alpha", Some(&many))]);
    model.status_line = Some("Synced 5 tasks".to_string());
    ui_update(&mut model, Message::Key(press('u')));

    let drawn = rows(&model);
    assert_eq!(
        drawn.last().expect("a bottom row").trim_end(),
        "j/k move  Enter open  Esc cancel",
        "the picker covered its own legend:\n{}",
        drawn.join("\n")
    );
    assert_eq!(
        drawn[drawn.len() - 2].trim_end(),
        "Synced 5 tasks",
        "the picker covered the status line:\n{}",
        drawn.join("\n")
    );
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
