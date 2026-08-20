//! The title editor as actually drawn: the caret is the terminal's own cursor,
//! so the line shows the buffer and nothing but the buffer.
//!
//! The regression this pins is a rendering one and only visible in a frame: the
//! caret used to be a `▏` glyph *spliced into the text*, which cost a cell and
//! pushed everything after the caret one column right — `Auswertung` edited
//! mid-word read as `Auswertu ng`, a space nobody typed. A reducer test could
//! never see it (the buffer was always correct), and a unit test of the windowing
//! helper could not see the cursor land on the wrong column either, so the
//! assertions here go through `ui::view` — the same entry point `main.rs` draws
//! with — and read the row *and* the backend's cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Message, Model};
use oxidone::domain::Selection;
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const HEIGHT: u16 = 24;
const WIDTH: u16 = 80;

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

/// One frame of `model`, drawn the way `main.rs` draws it. Returned whole
/// because both halves of every assertion below come from it: the rows say what
/// was printed, the backend's cursor says where the caret is.
fn drawn(model: &Model) -> Terminal<TestBackend> {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    terminal
}

fn rows(model: &Model) -> Vec<String> {
    let terminal = drawn(model);
    let buffer = terminal.backend().buffer().clone();
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// Where the cursor lands, or `None` when the frame leaves it hidden.
fn cursor_at(model: &Model) -> Option<(u16, u16)> {
    let terminal = drawn(model);
    let backend = terminal.backend();
    let position = backend.cursor_position();
    backend.cursor_visible().then_some((position.x, position.y))
}

/// The row the editor's input line sits on: the one below its popup title.
fn input_row(model: &Model) -> u16 {
    rows(model)
        .iter()
        .position(|r| r.contains("Edit title"))
        .expect("the title editor is open") as u16
        + 1
}

/// The column `text` starts at on row `y`.
fn column_of(model: &Model, y: u16, text: &str) -> u16 {
    let row = &rows(model)[y as usize];
    let byte = row
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} is not on row {y}: {row:?}"));
    row[..byte].chars().count() as u16
}

/// A task pane holding one Task titled `title`, with the editor closed.
async fn model_titled(title: &str) -> Model {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: title.to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab)); // focus the task pane
    m
}

/// The reported bug, from the screenshot: the caret between `u` and `n` used to
/// render a space that was never in the title.
#[tokio::test]
async fn the_caret_inserts_no_space_into_the_line_it_sits_in() {
    let mut m = model_titled("Auswertung, PEN Test").await;
    update(&mut m, ch('e'));
    // The editor opens with the caret at the end; walk it back between the `u`
    // and the `n`, where the screenshot has it.
    update(&mut m, key(KeyCode::Home));
    for _ in 0..8 {
        update(&mut m, key(KeyCode::Right));
    }

    let row = input_row(&m);
    let line = &rows(&m)[row as usize];
    assert!(
        line.contains("Auswertung, PEN Test"),
        "the title was drawn broken: {line:?}"
    );
    assert!(
        !line.contains("Auswertu ng"),
        "a phantom space at the caret: {line:?}"
    );

    let start = column_of(&m, row, "Auswertung");
    assert_eq!(
        cursor_at(&m),
        Some((start + 8, row)),
        "the caret sits on the `n`, eight cells into the title"
    );
}

/// The cursor is the caret, so it must be on screen exactly while a text overlay
/// is: parked on a pane row with no overlay open, it would read as an edit that
/// is not happening.
#[tokio::test]
async fn the_cursor_shows_only_while_the_editor_is_open() {
    let mut m = model_titled("alpha").await;
    assert_eq!(cursor_at(&m), None, "no overlay, no caret");

    update(&mut m, ch('e'));
    assert!(cursor_at(&m).is_some(), "the open editor draws a caret");

    update(&mut m, key(KeyCode::Esc));
    assert_eq!(cursor_at(&m), None, "the caret leaves with the overlay");
}

/// An authorization prompt draws over the editor without dismissing it, so the
/// caret has to go with the line it belongs to: left behind, the cursor would
/// blink on a popup with nothing to type into.
#[tokio::test]
async fn the_caret_goes_out_while_the_authorization_popup_covers_the_editor() {
    let mut m = model_titled("alpha").await;
    update(&mut m, ch('e'));
    assert!(cursor_at(&m).is_some(), "the open editor draws a caret");

    update(
        &mut m,
        Message::AuthPromptOpened("https://accounts.google.com/o/oauth2/auth".to_string()),
    );
    assert_eq!(cursor_at(&m), None, "covered, so no caret");

    update(&mut m, Message::AuthPromptClosed { reason: None });
    assert!(
        cursor_at(&m).is_some(),
        "the editor is back on top, and so is its caret"
    );
}

/// A title wider than the popup scrolls onto the caret, which stays inside the
/// popup's last text column — the window reserves that cell for it rather than
/// letting the cursor land on the border.
#[tokio::test]
async fn the_caret_stays_inside_the_popup_on_a_line_that_overflows_it() {
    // 60 cells against a 50-cell popup, 48 of them text. Deliberately not a
    // repeating filler: a periodic title makes "the head scrolled off" pass
    // against a window that never moved.
    let long = format!("HEAD-{}-TAIL", "x".repeat(50));
    let mut m = model_titled(&long).await;
    update(&mut m, ch('e'));

    let row = input_row(&m);
    let line = &rows(&m)[row as usize];
    assert!(
        line.contains("-TAIL"),
        "the window did not follow the caret to the tail: {line:?}"
    );
    assert!(
        !line.contains("HEAD"),
        "the head should have scrolled off: {line:?}"
    );

    // The popup's inner text starts where its own title is drawn, one cell in
    // from the left border, and is 48 cells wide.
    let inner_x = column_of(&m, row - 1, "Edit title");
    assert_eq!(
        cursor_at(&m),
        Some((inner_x + 47, row)),
        "the caret took the popup's last text cell, not its border"
    );
}
