//! The consent prompt's paste field, as the reducer sees it.
//!
//! The field exists for the machine whose loopback a browser cannot reach — over
//! SSH, `localhost` in the browser is somebody else's `localhost` — so the
//! callback URL is carried back by hand. `update` is pure, so everything here is
//! asserted without a terminal.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Message, Model, Overlay};

const URL: &str = "https://accounts.google.com/o/oauth2/auth?scope=tasks&client_id=x";
const CALLBACK: &str = "http://127.0.0.1:37137/?code=4/0AX4&state=abc";

fn prompted() -> Model {
    let mut model = Model::new();
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));
    model
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn chord(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// Type `text` into whatever currently has the keys.
fn type_in(model: &mut Model, text: &str) -> Vec<Command> {
    let mut commands = Vec::new();
    for c in text.chars() {
        commands.extend(update(model, key(KeyCode::Char(c))));
    }
    commands
}

fn field(model: &Model) -> &str {
    &model.auth_prompt.as_ref().expect("a prompt is up").input
}

#[test]
fn typing_goes_into_the_field_and_enter_submits_it() {
    let mut model = prompted();
    assert!(type_in(&mut model, CALLBACK).is_empty());
    assert_eq!(field(&model), CALLBACK);

    assert_eq!(
        update(&mut model, key(KeyCode::Enter)),
        vec![Command::SubmitAuthCallback(CALLBACK.to_string())]
    );
    assert_eq!(
        field(&model),
        "",
        "the field kept the paste it just handed over"
    );
}

/// The verb keys type here. `q` would otherwise quit mid-URL, and `a` would
/// spring a capture over the prompt the user is answering.
#[test]
fn the_keymaps_verbs_are_letters_here() {
    let mut model = prompted();
    type_in(&mut model, "qaAwc");
    assert_eq!(field(&model), "qaAwc");
    assert!(!model.should_quit);
    assert!(model.overlay.is_none());
}

/// The exception, and the reason there is one: the prompt can stand for
/// `CONSENT_TIMEOUT`, and `q` cannot be passed through a field holding a URL.
#[test]
fn ctrl_q_still_quits_with_a_half_typed_callback_in_the_field() {
    let mut model = prompted();
    type_in(&mut model, "http://127.0.0.1:37137/?co");

    assert!(update(&mut model, chord('q')).is_empty());
    assert!(model.should_quit);
}

/// Control chords edit the line or do nothing; none of them types a character,
/// and none reaches the keymap underneath. `^C` in particular still means
/// "toggle Completed" app-wide and must not gain a second meaning here.
#[test]
fn control_chords_do_not_type_and_do_not_fall_through() {
    let mut model = prompted();
    type_in(&mut model, "abc");
    for c in ['c', 'n', 'p', 'r'] {
        assert!(update(&mut model, chord(c)).is_empty());
    }
    assert_eq!(field(&model), "abc");
    assert!(!model.should_quit);
    assert!(model.overlay.is_none());
}

#[test]
fn the_line_editing_chords_work_as_they_do_everywhere_else() {
    let mut model = prompted();
    type_in(&mut model, "paste this url");

    // `^W` takes the word behind the caret, `^U` the whole line — the same two
    // the overlay legends teach.
    update(&mut model, chord('w'));
    assert_eq!(field(&model), "paste this ");
    update(&mut model, chord('u'));
    assert_eq!(field(&model), "");

    // And the caret keys reach the ends, so a paste can be corrected rather than
    // retyped.
    type_in(&mut model, "code=abc");
    update(&mut model, chord('a'));
    update(&mut model, key(KeyCode::Delete));
    assert_eq!(field(&model), "ode=abc");
    update(&mut model, chord('e'));
    update(&mut model, key(KeyCode::Backspace));
    assert_eq!(field(&model), "ode=ab");
}

/// `Esc` clears the field, not the prompt: the flow owns the prompt's lifetime
/// and is still waiting for an answer.
#[test]
fn esc_clears_the_field_and_leaves_the_prompt_up() {
    let mut model = prompted();
    type_in(&mut model, CALLBACK);
    update(
        &mut model,
        Message::AuthPasteRejected("that URL carried no authorization code".to_string()),
    );

    assert!(update(&mut model, key(KeyCode::Esc)).is_empty());
    assert_eq!(field(&model), "");
    let prompt = model.auth_prompt.as_ref().expect("still waiting");
    assert_eq!(prompt.url, URL);
    assert_eq!(prompt.rejected, None);
}

/// An empty submission is not an answer, so it is not sent: the flow would only
/// reject it, and the user would be told off for a keystroke they did not aim.
#[test]
fn enter_on_an_empty_field_submits_nothing() {
    let mut model = prompted();
    assert!(update(&mut model, key(KeyCode::Enter)).is_empty());
    type_in(&mut model, "   ");
    assert!(update(&mut model, key(KeyCode::Enter)).is_empty());
}

#[test]
fn a_rejection_is_shown_and_retired_by_the_next_submission() {
    let mut model = prompted();
    update(
        &mut model,
        Message::AuthPasteRejected("that URL carried no authorization code".to_string()),
    );
    assert_eq!(
        model.auth_prompt.as_ref().expect("a prompt is up").rejected,
        Some("that URL carried no authorization code".to_string())
    );

    type_in(&mut model, CALLBACK);
    update(&mut model, key(KeyCode::Enter));
    assert_eq!(
        model.auth_prompt.as_ref().expect("a prompt is up").rejected,
        None,
        "the verdict on the previous paste still stands under the new one"
    );
}

/// A rejection that arrives after the flow settled has nothing to annotate.
#[test]
fn a_rejection_without_a_prompt_is_dropped() {
    let mut model = Model::new();
    assert!(update(&mut model, Message::AuthPasteRejected("late".to_string())).is_empty());
    assert!(model.auth_prompt.is_none());
}

/// The rule that keeps the prompt from eating a capture: an overlay was opened
/// by the *user*, the prompt arrived from a worker, and the keys stay where the
/// user put them.
#[test]
fn an_open_overlay_keeps_the_keys() {
    let mut model = Model::new();
    update(&mut model, key(KeyCode::Char('A')));
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));

    type_in(&mut model, "groceries");

    match &model.overlay {
        Some(Overlay::AddList { buffer }) => assert_eq!(buffer, "groceries"),
        other => panic!("the capture lost its keys: {other:?}"),
    }
    assert_eq!(field(&model), "");
}

/// And once that overlay is gone the field has them, without the prompt having
/// had to be reopened.
#[test]
fn closing_the_overlay_hands_the_keys_to_the_field() {
    let mut model = Model::new();
    update(&mut model, key(KeyCode::Char('A')));
    update(&mut model, Message::AuthPromptOpened(URL.to_string()));
    update(&mut model, key(KeyCode::Esc));

    type_in(&mut model, CALLBACK);
    assert_eq!(field(&model), CALLBACK);
}
