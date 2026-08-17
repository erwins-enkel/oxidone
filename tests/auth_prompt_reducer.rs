//! Reducer arms for the Google consent prompt: an interactive authorization that
//! fires from a background worker, long after startup, when the cached token turns
//! out to be unusable. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model, Overlay};

const URL: &str = "https://accounts.google.com/o/oauth2/auth?scope=tasks&client_id=x";

fn opened() -> Message {
    Message::AuthPromptOpened(URL.to_string())
}

fn closed(reason: Option<&str>) -> Message {
    Message::AuthPromptClosed {
        reason: reason.map(str::to_owned),
    }
}

#[test]
fn opening_holds_the_url_for_the_view() {
    let mut model = Model::new();
    assert_eq!(update(&mut model, opened()), Vec::new());
    assert_eq!(model.auth_prompt.as_deref(), Some(URL));
}

#[test]
fn closing_clears_it() {
    let mut model = Model::new();
    update(&mut model, opened());
    update(&mut model, closed(None));
    assert_eq!(model.auth_prompt, None);
    assert_eq!(
        model.status_line, None,
        "an authorization that worked has nothing to report"
    );
}

/// A flow that ends badly must not just vanish: the reason reaches the status line.
#[test]
fn closing_with_a_reason_reports_it() {
    let mut model = Model::new();
    update(&mut model, opened());
    update(
        &mut model,
        closed(Some("authorization was not completed within 180s")),
    );
    assert_eq!(model.auth_prompt, None);
    assert_eq!(
        model.status_line.as_deref(),
        Some("authorization was not completed within 180s")
    );
}

/// The reason this is a field of its own and not an `Overlay` variant: it arrives
/// from a worker at a moment the user did not choose, and must not destroy a
/// half-typed capture.
#[test]
fn opening_leaves_a_half_typed_capture_alone() {
    let mut model = Model::new();
    // `A` opens the add-list capture; then type into it.
    update(&mut model, key(KeyCode::Char('A')));
    for c in "groceries".chars() {
        update(&mut model, key(KeyCode::Char(c)));
    }

    update(&mut model, opened());

    match &model.overlay {
        Some(Overlay::AddList { buffer }) => assert_eq!(buffer, "groceries"),
        other => panic!("the capture was destroyed: {other:?}"),
    }
    assert_eq!(model.auth_prompt.as_deref(), Some(URL));
}

/// And closing it does not close the capture either.
#[test]
fn closing_leaves_a_half_typed_capture_alone() {
    let mut model = Model::new();
    update(&mut model, key(KeyCode::Char('A')));
    update(&mut model, opened());
    update(&mut model, closed(None));

    assert!(matches!(model.overlay, Some(Overlay::AddList { .. })));
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}
