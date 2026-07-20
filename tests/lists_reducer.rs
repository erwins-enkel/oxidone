//! Reducer tests for the List read path (ticket #4): `ListsLoaded` handling and
//! sidebar selection. `update` is pure — no terminal.
//!
//! The sidebar cursor spans `[Today, …lists]`: the pinned Today view (#61) sits
//! above the real Lists, and startup lands on it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, TasksApi};
use oxidone::app::{update, Message, Model};
use oxidone::domain::{List, Selection};

async fn two_lists() -> Vec<List> {
    // Build real `List` values via the fake (Work, then Home).
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    api.list_lists().await.unwrap()
}

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

#[tokio::test]
async fn lists_loaded_lands_on_today() {
    // Today is pinned and is the startup landing, so loading Lists keeps the
    // cursor on it rather than auto-selecting the first List.
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(two_lists().await));
    assert_eq!(m.lists.len(), 2);
    assert_eq!(m.selected, Selection::Today);
}

#[tokio::test]
async fn empty_lists_stays_on_today() {
    // With no Lists, Today is still selectable (pinned) — there is no "nothing
    // selected" state to fall into.
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![]));
    assert!(m.lists.is_empty());
    assert_eq!(m.selected, Selection::Today);
}

#[tokio::test]
async fn j_and_k_move_selection_across_today_and_the_lists() {
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(two_lists().await));
    assert_eq!(m.selected, Selection::Today);

    update(&mut m, press('j'));
    assert_eq!(m.selected, Selection::List(0)); // Work
    update(&mut m, press('j'));
    assert_eq!(m.selected, Selection::List(1)); // Home
    update(&mut m, press('j')); // clamped at the end
    assert_eq!(m.selected, Selection::List(1));
    update(&mut m, press('k'));
    assert_eq!(m.selected, Selection::List(0));
    update(&mut m, press('k'));
    assert_eq!(m.selected, Selection::Today); // back up to the pinned row
    update(&mut m, press('k')); // clamped at the start
    assert_eq!(m.selected, Selection::Today);
}

#[tokio::test]
async fn reload_preserves_the_selected_list_by_id() {
    let mut m = Model::new();
    let lists = two_lists().await;
    let home = lists[1].clone();
    update(&mut m, Message::ListsLoaded(lists));
    update(&mut m, press('j')); // Today -> Work
    update(&mut m, press('j')); // Work  -> Home (index 1)
    assert_eq!(m.selected, Selection::List(1));

    // Reload with Work gone — Home is now index 0 and should stay selected.
    update(&mut m, Message::ListsLoaded(vec![home]));
    assert_eq!(m.lists.len(), 1);
    assert_eq!(m.selected, Selection::List(0));
    assert_eq!(m.lists[0].title, "Home");
}

#[tokio::test]
async fn load_failed_sets_the_status_line() {
    let mut m = Model::new();
    update(&mut m, Message::LoadFailed("boom".to_string()));
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}
