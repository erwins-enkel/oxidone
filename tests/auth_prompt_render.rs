//! The consent prompt, as actually drawn.
//!
//! This is the half of the bug that a reducer test cannot see: the URL used to
//! reach the user as a `println!` from inside `yup-oauth2`, which scrolled the
//! frame apart. It has to be *in* the frame, whole, and over anything else on it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model};
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// The smallest terminal oxidone supports — the prompt has to work there, since a
/// user who cannot read the URL cannot authorize.
const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// A real Google consent URL: one unbroken token, far wider than any popup.
const URL: &str = "https://accounts.google.com/o/oauth2/auth?scope=https://www.googleapis.com/auth/tasks&access_type=offline&redirect_uri=http://localhost:37137&response_type=code&client_id=1001161534011-c8tf5fur0hdvrkb83t7oks42qoatjglk.apps.googleusercontent.com";

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
    let drawn = rows(&prompted()).join("\n");
    assert!(
        drawn.contains("Authorize with Google"),
        "no title on the frame:\n{drawn}"
    );
    assert!(
        drawn.contains("If it did not open, visit the URL above"),
        "no fallback instruction on the frame:\n{drawn}"
    );
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
    let mut model = prompted();
    update(
        &mut model,
        Message::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::empty())),
    );

    let drawn = rows(&model).join("\n");
    assert!(drawn.contains("Authorize with Google"), "{drawn}");
    assert!(
        !drawn.contains("Add list"),
        "the overlay's title is still visible, so the prompt did not cover it:\n{drawn}"
    );
}
