//! `classify_event` maps terminal events to the `Message` that wakes the reducer
//! loop. Dropping `Event::Resize` here was the whole "weird split on resize" bug:
//! a resize never woke the loop, so the frame stayed sized for the old terminal.
//! Only key **presses** and resizes drive the loop; everything else is `None`.

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use oxidone::app::{classify_event, Message};

#[test]
fn resize_wakes_the_loop() {
    assert!(matches!(
        classify_event(Event::Resize(80, 24)),
        Some(Message::Resize)
    ));
}

#[test]
fn key_press_is_forwarded() {
    let press = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::empty());
    assert!(matches!(
        classify_event(Event::Key(press)),
        Some(Message::Key(_))
    ));
}

#[test]
fn key_release_is_ignored() {
    let release = KeyEvent::new_with_kind(
        KeyCode::Char('j'),
        KeyModifiers::empty(),
        KeyEventKind::Release,
    );
    assert!(classify_event(Event::Key(release)).is_none());
}

#[test]
fn mouse_is_ignored() {
    let mouse = Event::Mouse(MouseEvent {
        kind: MouseEventKind::Moved,
        column: 1,
        row: 1,
        modifiers: KeyModifiers::empty(),
    });
    assert!(classify_event(mouse).is_none());
}

#[test]
fn focus_is_ignored() {
    assert!(classify_event(Event::FocusGained).is_none());
    assert!(classify_event(Event::FocusLost).is_none());
}
