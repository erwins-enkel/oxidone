//! Reducer tests for the manual Refresh verb (ticket #45): `r` emits the
//! refresh `Command`, the cascade reaches the active List's Tasks, and the pane
//! survives it. `update` is pure — no terminal, no network.

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Focus, Message, Model, Overlay, OFFLINE};
use oxidone::domain::{List, Selection, Task};

fn titles(tasks: &[&Task]) -> Vec<String> {
    tasks.iter().map(|t| t.title.clone()).collect()
}

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

async fn list_with_tasks(titles: &[&str]) -> (List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for t in titles {
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
    (l, tasks)
}

/// A Model with one List selected, its Tasks loaded, and an API available.
async fn seeded(titles: &[&str]) -> (Model, List) {
    let (list, tasks) = list_with_tasks(titles).await;
    let mut m = Model::new();
    m.api_available = true;
    update(&mut m, Message::ListsLoaded(vec![list.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(list.id.clone(), tasks));
    (m, list)
}

#[tokio::test]
async fn r_emits_the_refresh_command_and_shows_a_transient_status() {
    let (mut m, _) = seeded(&["a"]).await;
    let cmds = update(&mut m, press('r'));
    assert_eq!(cmds, vec![Command::RefreshLists]);
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn r_without_an_api_reports_offline_and_emits_nothing() {
    let (mut m, _) = seeded(&["a"]).await;
    m.api_available = false;
    let cmds = update(&mut m, press('r'));
    assert!(cmds.is_empty(), "offline refresh must not reach a worker");
    assert_eq!(m.status_line.as_deref(), Some(OFFLINE));
}

#[tokio::test]
async fn r_is_modeless_and_fires_from_either_pane() {
    for focus in [Focus::Sidebar, Focus::Tasks] {
        let (mut m, _) = seeded(&["a"]).await;
        m.focus = focus;
        assert_eq!(
            update(&mut m, press('r')),
            vec![Command::RefreshLists],
            "{focus:?}"
        );
    }
}

/// Keys route to the overlay before the keymap, so `r` typed into a text input
/// is a literal `r`. Note this is `model.overlay`, *not* `show_help` — see the
/// test below.
#[tokio::test]
async fn a_text_overlay_swallows_r() {
    let (mut m, _) = seeded(&["a"]).await;
    update(&mut m, press('a')); // open the add-task capture overlay
    let cmds = update(&mut m, press('r'));
    assert!(cmds.is_empty(), "overlay must swallow the verb");
    match &m.overlay {
        Some(Overlay::AddTask { buffer }) => assert_eq!(buffer, "r"),
        other => panic!("expected the add-task overlay, got {other:?}"),
    }
}

/// The `?` cheatsheet is a *visual* overlay only: it is not consulted in the key
/// path, so every modeless verb still works under it. `r` joining `j`/`k`/`Tab`
/// is consistency — do not "fix" this by guarding the key path on `show_help`.
#[tokio::test]
async fn the_help_popup_does_not_swallow_r() {
    let (mut m, _) = seeded(&["a"]).await;
    update(&mut m, press('?'));
    assert!(m.show_help);
    assert_eq!(update(&mut m, press('r')), vec![Command::RefreshLists]);
}

/// The Tasks half of the refresh: the worker's `ListsLoaded` cascades through
/// `set_lists` into a `LoadTasks` for the active List.
#[tokio::test]
async fn the_refresh_cascades_into_the_active_lists_tasks() {
    let (mut m, list) = seeded(&["a"]).await;
    update(&mut m, press('r'));
    let cmds = update(&mut m, Message::ListsLoaded(vec![list.clone()]));
    assert_eq!(cmds, vec![Command::LoadTasks(list.id)]);
}

/// The user-visible promise of `r`: an unchanged active List refreshes in place.
/// `set_lists` takes its `clear_pane = false` branch, so the pane keeps its
/// Tasks and the cursor stays on the same Task rather than snapping to the top.
#[tokio::test]
async fn a_refresh_preserves_the_task_pane_and_cursor() {
    let (mut m, list) = seeded(&["a", "b", "c"]).await;
    m.focus = Focus::Tasks;
    update(&mut m, press('j')); // move off the first Task
    let before = m.selected_task;
    let on = m.tasks[before.unwrap()].id.clone();
    assert_ne!(before, Some(0), "cursor must start off the first row");

    update(&mut m, press('r'));
    update(&mut m, Message::ListsLoaded(vec![list]));

    assert_eq!(m.tasks.len(), 3, "the pane must not blank mid-refresh");
    assert_eq!(m.selected_task, before);
    assert_eq!(m.tasks[m.selected_task.unwrap()].id, on);
}

/// The transient clears when the Lists half lands — it deliberately does not
/// span the cascaded Tasks fetch (that would need in-flight state across two
/// Messages).
#[tokio::test]
async fn the_transient_status_clears_when_lists_arrive() {
    let (mut m, list) = seeded(&["a"]).await;
    update(&mut m, press('r'));
    assert!(m.status_line.is_some());
    update(&mut m, Message::ListsLoaded(vec![list]));
    assert_eq!(m.status_line, None);
}

/// `r` is a modeless keypress, so the `set_tasks` fallback is reachable in one
/// key: refresh, and Google no longer has the Task the cursor was on. The cursor
/// must land on the first *displayed* row — under the default Due lens that is
/// the soonest-due Task, not stored index 0.
#[tokio::test]
async fn a_refresh_that_drops_the_selected_task_anchors_the_first_displayed() {
    let (list, mut tasks) = list_with_tasks(&["c", "a", "b"]).await;
    // Dates make display order differ from stored order: the pane reads b, a, c.
    tasks[1].due = NaiveDate::from_ymd_opt(2026, 8, 1);
    tasks[2].due = NaiveDate::from_ymd_opt(2026, 7, 21);

    let mut m = Model::new();
    m.api_available = true;
    update(&mut m, Message::ListsLoaded(vec![list.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(list.id.clone(), tasks.clone()));
    m.focus = Focus::Tasks;
    m.selected_task = Some(1); // "a"
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a", "c"]);

    update(&mut m, press('r'));
    // The refresh comes back without "a".
    let remaining: Vec<Task> = tasks.into_iter().filter(|t| t.title != "a").collect();
    update(&mut m, Message::TasksLoaded(list.id, remaining));

    let selected = m
        .selected_task
        .and_then(|i| m.tasks.get(i))
        .map(|t| &t.title);
    assert_eq!(
        selected.map(String::as_str),
        Some("b"),
        "first displayed row, not stored index 0 (\"c\")",
    );
}
