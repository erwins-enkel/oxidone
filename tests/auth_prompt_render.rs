//! The consent prompt, as actually drawn.
//!
//! This is the half a reducer test cannot see: the URL used to reach the user as
//! a `println!` from inside the OAuth library, which scrolled the frame apart. It
//! has to be *in* the frame, whole, and over anything else on it — and so does
//! the field the callback is pasted back into, for the machine whose loopback the
//! browser cannot reach.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::{Backend, TestBackend};
use ratatui::layout::Position;
use ratatui::Terminal;

/// The smallest terminal oxidone supports — the prompt has to work there, since a
/// user who cannot read the URL cannot authorize.
const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// A real Google consent URL: one unbroken token, far wider than any popup.
const URL: &str = "https://accounts.google.com/o/oauth2/auth?scope=https://www.googleapis.com/auth/tasks&access_type=offline&redirect_uri=http://localhost:37137&response_type=code&client_id=1001161534011-c8tf5fur0hdvrkb83t7oks42qoatjglk.apps.googleusercontent.com";

fn rows(model: &Model) -> Vec<String> {
    frame(model).0
}

/// Where the cursor is parked before a draw. `ratatui` moves it only for a frame
/// that asked for one, so finding it still here afterwards is how this suite
/// reads "no caret" — the backend exposes the position but not the visibility.
const NO_CURSOR: Position = Position {
    x: WIDTH - 1,
    y: HEIGHT - 1,
};

/// The drawn rows and where the terminal's own cursor ended up, if the frame
/// asked for one at all.
fn frame(model: &Model) -> (Vec<String>, Option<(u16, u16)>) {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .backend_mut()
        .set_cursor_position(NO_CURSOR)
        .expect("park the cursor");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    let cursor = terminal
        .backend_mut()
        .get_cursor_position()
        .expect("cursor position");

    let buffer = terminal.backend().buffer().clone();
    let rows = (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect();
    (rows, (cursor != NO_CURSOR).then_some((cursor.x, cursor.y)))
}

fn prompted() -> Model {
    let mut model = Model::new();
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));
    model
}

/// Every character of the URL reaches the frame — hard-wrapped across lines, but
/// nothing truncated away and nothing reordered. A URL missing its tail is a URL
/// the user cannot authorize with.
///
/// Reconstructing it from the drawn rows is the assertion, so *where* the wrap
/// breaks stays a layout detail this test does not pin.
#[test]
fn the_whole_url_is_drawn() {
    let mut remaining = URL;
    for row in rows(&prompted()) {
        for piece in ascii_tokens(&row) {
            if let Some(rest) = remaining.strip_prefix(piece.as_str()) {
                remaining = rest;
            }
        }
    }
    assert!(
        remaining.is_empty(),
        "the URL stops on the frame before: {remaining}"
    );
}

/// A row's ASCII words. The frame's box drawing is all non-ASCII and a consent URL
/// is all ASCII, so what survives on a URL row is the URL's piece by itself —
/// unglued from the popup border it sits against.
fn ascii_tokens(row: &str) -> Vec<String> {
    row.chars()
        .map(|c| if c.is_ascii_graphic() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_prompt_is_titled_and_says_what_to_do() {
    let drawn = rows(&prompted());
    let text = prompt_text(&drawn);
    assert!(
        drawn.join("\n").contains("Authorize with Google"),
        "no title on the frame:\n{}",
        drawn.join("\n")
    );
    assert!(
        text.contains("cannot reach this machine"),
        "no fallback instruction on the frame:\n{text}"
    );
    assert!(
        text.contains("Ctrl-Q quits"),
        "the only way out of a prompt that takes every other key is unadvertised:\n{text}"
    );
}

/// What is typed into the field is on the frame, and the terminal's own cursor is
/// on it — the prompt takes the keys while no overlay has them, so it has to show
/// where they are going.
#[test]
fn the_pasted_callback_and_its_caret_are_drawn() {
    let mut model = prompted();
    for c in "http://127.0.0.1:37137/?code=abc".chars() {
        update(&mut model, key(KeyCode::Char(c)));
    }

    let (drawn, cursor) = frame(&model);
    let text = prompt_text(&drawn);
    assert!(
        text.contains("http://127.0.0.1:37137/?code=abc"),
        "the pasted callback is not on the frame:\n{text}"
    );
    let (col, row) = cursor.expect("the field has the keys, so it has the caret");
    let caret_row = &drawn[row as usize];
    assert!(
        caret_row.contains("code=abc"),
        "the caret is not on the field's row:\n{text}"
    );
    assert_eq!(
        caret_row.chars().nth(col as usize),
        Some(' '),
        "the caret is not one past the text it is editing:\n{text}"
    );
}

/// A rejected paste says so where the retry happens. The flow is still waiting,
/// so the prompt stays whole around it.
#[test]
fn a_rejected_paste_is_reported_in_the_prompt() {
    let mut model = prompted();
    update(
        &mut model,
        Message::AuthPasteRejected("that URL carried no authorization code".to_string()),
    );

    let drawn = rows(&model);
    let text = prompt_text(&drawn);
    assert!(
        text.contains("that URL carried no authorization code"),
        "the rejection is not on the frame:\n{text}"
    );
    assert!(drawn.join("\n").contains("Authorize with Google"), "{text}");
}

/// While an overlay holds the keys the field does not, and a caret left on the
/// covered overlay — or put on a field nothing is typing into — would point at
/// the wrong place.
#[test]
fn the_caret_is_not_drawn_while_an_overlay_holds_the_keys() {
    let mut model = Model::new();
    update(&mut model, key(KeyCode::Char('A')));
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));

    assert_eq!(frame(&model).1, None);
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

/// The popup's own text, unwrapped: its interior columns from every row, joined,
/// with runs of padding collapsed.
///
/// Needed because the popup hard-wraps mid-word — a consent URL leaves it no
/// choice — so a phrase the user reads as one is two rows on the frame. The
/// popup's edges are found on the frame rather than hardcoded, so this says
/// nothing about where the popup is.
fn prompt_text(rows: &[String]) -> String {
    let title_row = rows
        .iter()
        .find(|row| row.contains("Authorize with Google"))
        .expect("the popup is on the frame");
    let left = title_row
        .chars()
        .position(|c| c == '╭')
        .expect("the popup has a left edge");
    let right = title_row
        .chars()
        .position(|c| c == '╮')
        .expect("the popup has a right edge");
    let inner: String = rows
        .iter()
        .map(|row| {
            row.chars()
                .skip(left + 1)
                .take(right - left - 1)
                .collect::<String>()
        })
        .collect();
    inner.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Nothing is drawn when no authorization is pending — the prompt is not a
/// permanent fixture of the frame.
#[test]
fn nothing_is_drawn_without_a_pending_authorization() {
    let drawn = rows(&Model::new()).join("\n");
    assert!(!drawn.contains("Authorize with Google"), "{drawn}");
}

/// It is drawn last, so it sits over an open overlay rather than under it: the
/// user cannot answer a prompt they cannot see.
#[test]
fn it_draws_over_an_open_overlay() {
    // The overlay first: a prompt arrives from a worker, and once it is up the
    // keys are the field's, so `A` would type rather than open anything.
    let mut model = Model::new();
    update(&mut model, key(KeyCode::Char('A')));
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));

    let drawn = rows(&model).join("\n");
    assert!(drawn.contains("Authorize with Google"), "{drawn}");
    assert!(
        !drawn.contains("Add list"),
        "the overlay's title is still visible, so the prompt did not cover it:\n{drawn}"
    );
}
