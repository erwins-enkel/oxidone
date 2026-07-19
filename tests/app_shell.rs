//! Reducer tests for the app shell (ticket #3) — the pure `update` seam and the
//! keymap table. No terminal: `update` is a pure function over `Model`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::keymap::{self, Action};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

#[test]
fn q_quits() {
    let mut m = Model::new();
    assert!(!m.should_quit);
    update(&mut m, press('q'));
    assert!(m.should_quit);
}

#[test]
fn question_mark_toggles_help() {
    let mut m = Model::new();
    assert!(!m.show_help);
    update(&mut m, press('?'));
    assert!(m.show_help);
    update(&mut m, press('?'));
    assert!(!m.show_help);
}

#[test]
fn tab_switches_focus_between_panes() {
    let mut m = Model::new();
    assert_eq!(m.focus, Focus::Sidebar);
    update(&mut m, key(KeyCode::Tab));
    assert_eq!(m.focus, Focus::Tasks);
    update(&mut m, key(KeyCode::Tab));
    assert_eq!(m.focus, Focus::Sidebar);
}

#[test]
fn esc_closes_the_help_overlay() {
    let mut m = Model::new();
    update(&mut m, press('?'));
    assert!(m.show_help);
    update(&mut m, key(KeyCode::Esc));
    assert!(!m.show_help);
}

#[test]
fn unbound_key_is_a_no_op() {
    let mut m = Model::new();
    update(&mut m, press('z'));
    assert!(!m.should_quit);
    assert!(!m.show_help);
    assert_eq!(m.focus, Focus::Sidebar);
}

// ---- keymap-as-data ----

#[test]
fn resolve_maps_keys_to_actions() {
    let q = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::empty());
    assert_eq!(keymap::resolve(q), Some(Action::Quit));
    assert_eq!(
        keymap::resolve(key_ev(KeyCode::Tab)),
        Some(Action::SwitchPane)
    );
    assert_eq!(keymap::resolve(key_ev(KeyCode::Char('z'))), None);
}

#[test]
fn help_overlay_is_generated_from_the_binding_table() {
    // Every binding contributes a help entry — the cheatsheet is the table.
    assert!(keymap::bindings().iter().any(|b| b.action == Action::Quit));
    assert!(keymap::bindings().iter().all(|b| !b.help.is_empty()));
}

fn key_ev(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}
