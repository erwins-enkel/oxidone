//! Reducer tests for the app shell (ticket #3) — the pure `update` seam and the
//! keymap table. No terminal: `update` is a pure function over `Model`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Focus, Message, Model, Overlay};
use oxidone::keymap::{self, Action, LegendContext, LegendKeys};

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

// ---- directional pane focus ----

/// Seed a Model with one List and two Tasks, so selection assertions below are
/// about real indices rather than a vacuous `None`.
async fn seeded() -> Model {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for t in ["a", "b"] {
        api.insert_task(
            &l.id,
            NewTask {
                title: t.to_string(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let lists = api.list_lists().await.unwrap();

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists));
    update(&mut m, Message::TasksLoaded(l.id, tasks));
    m
}

#[tokio::test]
async fn right_focuses_the_task_pane_and_left_returns_to_the_sidebar() {
    let mut m = seeded().await;
    assert_eq!(m.focus, Focus::Sidebar);
    update(&mut m, key(KeyCode::Right));
    assert_eq!(m.focus, Focus::Tasks);
    update(&mut m, key(KeyCode::Left));
    assert_eq!(m.focus, Focus::Sidebar);
}

#[tokio::test]
async fn l_and_h_move_focus_like_the_arrows() {
    let mut m = seeded().await;
    update(&mut m, press('l'));
    assert_eq!(m.focus, Focus::Tasks);
    update(&mut m, press('h'));
    assert_eq!(m.focus, Focus::Sidebar);
}

#[tokio::test]
async fn focusing_past_the_edge_changes_nothing() {
    // No wrap: the focus keys are idempotent at the layout's two edges, and a
    // no-op means the *whole* model is untouched — not just `focus`.
    let mut m = seeded().await;
    let (list, task) = (m.selected_list, m.selected_task);

    for k in [key(KeyCode::Left), press('h')] {
        let cmds = update(&mut m, k);
        assert!(
            cmds.is_empty(),
            "focusing left from the sidebar emitted work"
        );
        assert_eq!(m.focus, Focus::Sidebar);
        assert_eq!(m.selected_list, list);
        assert_eq!(m.selected_task, task);
    }

    update(&mut m, key(KeyCode::Right));
    for k in [key(KeyCode::Right), press('l')] {
        let cmds = update(&mut m, k);
        assert!(
            cmds.is_empty(),
            "focusing right from the tasks emitted work"
        );
        assert_eq!(m.focus, Focus::Tasks);
        assert_eq!(m.selected_list, list);
        assert_eq!(m.selected_task, task);
    }
}

#[tokio::test]
async fn a_text_overlay_swallows_the_focus_keys() {
    // `h`/`l` are ordinary characters once an input overlay is open, and the
    // arrows are inert there — neither may reach the focus verbs behind it.
    let mut m = seeded().await;
    update(&mut m, key(KeyCode::Right));
    update(&mut m, press('a')); // AddTask overlay
    assert!(m.overlay.is_some());

    update(&mut m, press('h'));
    update(&mut m, press('l'));
    update(&mut m, key(KeyCode::Left));
    update(&mut m, key(KeyCode::Right));

    match &m.overlay {
        Some(Overlay::AddTask { buffer }) => assert_eq!(buffer, "hl"),
        other => panic!("expected the AddTask overlay, got {other:?}"),
    }
    assert_eq!(m.focus, Focus::Tasks);
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
fn the_focus_keys_are_bound_and_documented() {
    // Data-level, not popup-level: the `?` cheatsheet renders from this table,
    // so a key that is in the table with help text is documented — whether or
    // not the popup can currently draw every row (see issue on its overflow).
    for (code, action) in [
        (KeyCode::Char('h'), Action::FocusLeft),
        (KeyCode::Left, Action::FocusLeft),
        (KeyCode::Char('l'), Action::FocusRight),
        (KeyCode::Right, Action::FocusRight),
    ] {
        assert_eq!(keymap::resolve(key_ev(code)), Some(action), "{code:?}");
        let bound = keymap::bindings().iter().find(|b| b.key == code);
        assert!(
            bound.is_some_and(|b| !b.help.is_empty()),
            "{code:?} has no cheatsheet text"
        );
    }
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

// --- The always-visible legend -------------------------------------------
//
// A second, curated view of the same binding table. These cover the tables and
// their key derivation; the fitting and rendering live in `ui` (private, tested
// inline) and `tests/legend_render.rs`.

/// Every context, so a new one can't skip the guards below.
const CONTEXTS: [LegendContext; 4] = [
    LegendContext::Tasks,
    LegendContext::Sidebar,
    LegendContext::TextInput,
    LegendContext::Confirm,
];

#[test]
fn every_derived_legend_action_is_bound() {
    // Without this, deleting a binding leaves a legend cell rendering a bare
    // label with no key — a silent lie rather than a build error.
    let cells = CONTEXTS
        .iter()
        .flat_map(|c| keymap::legend(*c))
        .chain(std::iter::once(&keymap::HELP));
    for cell in cells {
        if let LegendKeys::Derived(actions) = cell.keys {
            for action in actions {
                assert!(
                    keymap::bindings().iter().any(|b| b.action == *action),
                    "legend cell {cell:?} names unbound {action:?}"
                );
            }
        }
    }
}

#[test]
fn derived_keys_take_the_first_binding_in_slice_order() {
    // Three shapes, each load-bearing. The pairs must read letter-then-letter
    // (the arrow aliases are bound second), and in the slice's order — spelling
    // the navigation cell `[SelectPrev, SelectNext]` would render "k/j".
    let cell = |actions: &'static [Action]| keymap::LegendEntry {
        keys: LegendKeys::Derived(actions),
        label: "x",
    };
    assert_eq!(
        cell(&[Action::SelectNext, Action::SelectPrev]).key_text(),
        "j/k"
    );
    assert_eq!(
        cell(&[Action::FocusLeft, Action::FocusRight]).key_text(),
        "h/l"
    );
    // `Enter` aliases `e` (a second binding for the same Action); first wins.
    assert_eq!(cell(&[Action::EditTitle]).key_text(), "e");
}

#[test]
fn each_pane_legend_carries_its_own_exclusive_verb() {
    let tasks = keymap::legend(LegendContext::Tasks);
    let sidebar = keymap::legend(LegendContext::Sidebar);

    let names = |cells: &'static [keymap::LegendEntry], action: Action| {
        cells.iter().any(|c| match c.keys {
            LegendKeys::Derived(actions) => actions.contains(&action),
            LegendKeys::Literal(_) => false,
        })
    };

    assert!(names(tasks, Action::ToggleComplete));
    assert!(!names(tasks, Action::AddList));
    assert!(names(sidebar, Action::AddList));
    assert!(!names(sidebar, Action::ToggleComplete));
}

#[test]
fn overlay_legends_advertise_only_keys_the_overlay_handles() {
    // While an overlay is up the reducer routes keys to `overlay_key` before
    // the keymap, so a pane verb here would be false.
    let text: Vec<String> = keymap::legend(LegendContext::TextInput)
        .iter()
        .map(|c| c.text())
        .collect();
    assert_eq!(text, ["Enter save", "Esc cancel"]);

    let confirm: Vec<String> = keymap::legend(LegendContext::Confirm)
        .iter()
        .map(|c| c.text())
        .collect();
    assert_eq!(confirm, ["y yes", "n no", "Esc cancel"]);
}
