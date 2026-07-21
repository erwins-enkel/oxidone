//! Reducer tests for the app shell (ticket #3) — the pure `update` seam and the
//! keymap table. No terminal: `update` is a pure function over `Model`.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Focus, Message, Model, Overlay};
use oxidone::keymap::{self, Action, LegendContext, LegendKeys};
use std::collections::HashSet;

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
    // Startup lands on the pinned Today row (#61); select the List so its Tasks
    // fill the pane and the pane-focused verbs act on it.
    m.selected = oxidone::domain::Selection::List(0);
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
    let (list, task) = (m.selected, m.selected_task);

    for k in [key(KeyCode::Left), press('h')] {
        let cmds = update(&mut m, k);
        assert!(
            cmds.is_empty(),
            "focusing left from the sidebar emitted work"
        );
        assert_eq!(m.focus, Focus::Sidebar);
        assert_eq!(m.selected, list);
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
        assert_eq!(m.selected, list);
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
    assert_eq!(
        keymap::resolve(key_ev(KeyCode::Char('r'))),
        Some(Action::Refresh)
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
    assert!(keymap::bindings()
        .iter()
        .any(|b| b.action == Action::Refresh));
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
const CONTEXTS: [LegendContext; 9] = [
    LegendContext::Tasks,
    LegendContext::Sidebar,
    LegendContext::TextInput,
    LegendContext::TaskCapture,
    LegendContext::Confirm,
    LegendContext::LinkPicker,
    LegendContext::ListPicker,
    LegendContext::Filter,
    LegendContext::SearchFilter,
];

#[test]
fn move_to_list_is_documented_in_the_cheatsheet() {
    // Data-level, like `the_focus_keys_are_bound_and_documented` above: `M` has
    // no always-visible legend cell (the 80-column TASKS row is full), so the `?`
    // table is where it must appear.
    assert_eq!(
        keymap::resolve(key_ev(KeyCode::Char('M'))),
        Some(Action::MoveToList)
    );
    let rows = keymap::cheatsheet_rows();
    assert!(
        rows.iter()
            .any(|(keys, help)| keys == "M" && *help == "move to another list"),
        "M is missing from the cheatsheet: {rows:?}"
    );
}

#[test]
fn contexts_covers_every_legend_context() {
    // `CONTEXTS` is a fixed-size literal, so a new variant compiles fine here
    // and silently shrinks every guard that iterates it — which is exactly what
    // adding `LinkPicker` did. The match is what makes the array's claim true:
    // a new variant now stops the build, right next to the array it belongs in.
    for context in CONTEXTS {
        match context {
            LegendContext::Tasks
            | LegendContext::Sidebar
            | LegendContext::TextInput
            | LegendContext::TaskCapture
            | LegendContext::Confirm
            | LegendContext::LinkPicker
            | LegendContext::ListPicker
            | LegendContext::Filter
            | LegendContext::SearchFilter => {}
        }
    }
}

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

// --- The `?` cheatsheet ---------------------------------------------------
//
// A third view of the same binding table, and the widest: every binding appears,
// with aliases collapsed onto one row. The layout and rendering live in `ui`
// (private, tested inline).

#[test]
fn the_cheatsheet_labels_every_bound_key_exactly_once() {
    // The `/`-split below is only sound while no single label contains a slash.
    // `key_label` falls through to `format!("{other:?}")` for unlisted keys, so
    // a future `KeyCode` could Debug-format with one and split a label in two —
    // which would pass the set comparison for the wrong reason.
    assert!(
        keymap::bindings()
            .iter()
            .all(|b| !keymap::key_label(b.key).contains('/')),
        "a key label contains '/', which the cheatsheet uses as its join"
    );

    let labelled: HashSet<String> = keymap::cheatsheet_rows()
        .iter()
        .flat_map(|(label, _)| label.split('/').map(str::to_string))
        .collect();
    let bound: HashSet<String> = keymap::bindings()
        .iter()
        .map(|b| keymap::key_label(b.key))
        .collect();

    // Both directions: no binding missing from the cheatsheet, and nothing in
    // the cheatsheet that isn't bound. `contains` would not do — `"n"` is a
    // substring of `"j/Down"`, and `"a"` and `"c"` of `"Space"`.
    assert_eq!(labelled, bound);
}

#[test]
fn the_cheatsheet_collapses_aliases_onto_one_row() {
    let rows = keymap::cheatsheet_rows();

    // Derived, never a literal: one row per distinct (action, help) pair.
    // `Action` is `PartialEq` but not `Hash`, so count with a `Vec` rather than
    // widen the enum's derives for a test's convenience.
    let mut distinct: Vec<(Action, &str)> = Vec::new();
    for b in keymap::bindings() {
        if !distinct.contains(&(b.action, b.help)) {
            distinct.push((b.action, b.help));
        }
    }
    assert_eq!(rows.len(), distinct.len());
    assert!(
        rows.len() < keymap::bindings().len(),
        "no aliases collapsed"
    );

    let joined: Vec<&str> = rows
        .iter()
        .map(|(label, _)| label.as_str())
        .filter(|label| label.contains('/'))
        .collect();
    assert_eq!(joined, ["h/Left", "l/Right", "j/Down", "k/Up", "e/Enter"]);
}

#[test]
fn the_cheatsheet_keeps_binding_table_order() {
    // Load-bearing: `ui` splits these rows into columns positionally, so the
    // order decides each column's width. A HashMap group-by would reshuffle the
    // popup between runs.
    //
    // Rows are identified by their leading key, not by help text alone: two
    // distinct verbs are free to share the same help string, and comparing only
    // that column would let such a pair swap places undetected. A key appears in
    // `bindings` once, so the first label of each row names it uniquely.
    let mut seen: Vec<(Action, &str)> = Vec::new();
    let mut expected: Vec<(String, &str)> = Vec::new();
    for b in keymap::bindings() {
        if !seen.contains(&(b.action, b.help)) {
            seen.push((b.action, b.help));
            expected.push((keymap::key_label(b.key), b.help));
        }
    }

    let actual: Vec<(String, &str)> = keymap::cheatsheet_rows()
        .iter()
        .map(|(label, help)| {
            let first = label.split('/').next().expect("split yields one part");
            (first.to_string(), *help)
        })
        .collect();
    assert_eq!(actual, expected);
}
