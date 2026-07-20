//! Reducer tests for the Task read path (ticket #5): selecting a List requests
//! its Tasks (a `Command`), `TasksLoaded` fills the pane, and the cursor +
//! focus behave. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Message, Model};
use oxidone::domain::{List, ListId, Selection, Task};
use std::collections::HashMap;

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn tab() -> Message {
    Message::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()))
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

/// Startup lands on the pinned Today view (#61): loading Lists emits a single
/// `LoadToday` (which itself fans out every List in its worker), not the per-List
/// meter cascade — so the reducer does not double-fetch.
#[tokio::test]
async fn loading_lists_on_today_emits_load_today_only() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();

    let mut m = Model::new();
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(m.selected, Selection::Today);
    assert!(matches!(cmds.as_slice(), [Command::LoadToday { .. }]));
}

/// With a **List** active, the lazy fan-out fills the sidebar meters: an empty
/// aggregate leaves every List uncovered, so `set_lists` requests Tasks for all of
/// them — the active List first (via `request_selected`), then the rest — so a
/// List never opened on this machine still gets a meter.
#[tokio::test]
async fn loading_lists_fans_out_to_uncovered_lists_active_first() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists.clone())); // lands on Today
    update(&mut m, press('j')); // select Work (List 0), leaving Today
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(
        cmds,
        vec![
            Command::LoadTasks(lists[0].id.clone()),
            Command::LoadTasks(lists[1].id.clone()),
        ]
    );
}

/// A List the cache aggregate already covers is left alone — its meter is already
/// drawable, and re-fetching it would be the network churn the lazy policy avoids.
#[tokio::test]
async fn list_fan_out_skips_lists_the_aggregate_covers() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let (work, home) = (lists[0].id.clone(), lists[1].id.clone());

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists.clone()));
    update(&mut m, press('j')); // Work active
                                // Cover Home so only an uncovered List is fetched.
    let mut counts = HashMap::new();
    counts.insert(home, (1usize, 2usize));
    update(&mut m, Message::CountsLoaded(counts));

    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    // Work (active) is emitted; Home is covered, so it is not re-fetched.
    assert_eq!(cmds, vec![Command::LoadTasks(work)]);
}

/// A manual Refresh is a **full** fan-out: every List's Tasks, even ones the
/// aggregate already covers, because `r` promises the latest from Google for
/// *all* meters. Active List first, then the rest in List order.
#[tokio::test]
async fn a_full_refresh_fans_out_to_every_list_active_first() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    api.insert_list("Play").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let (work, home, play) = (
        lists[0].id.clone(),
        lists[1].id.clone(),
        lists[2].id.clone(),
    );

    let mut m = Model::new();
    m.api_available = true;
    update(&mut m, Message::ListsLoaded(lists.clone()));
    update(&mut m, press('j')); // Work active
                                // Cover every List, so only a *full* refresh re-fetches them.
    let mut counts = HashMap::new();
    counts.insert(work.clone(), (0usize, 1usize));
    counts.insert(home.clone(), (0usize, 1usize));
    counts.insert(play.clone(), (0usize, 1usize));
    update(&mut m, Message::CountsLoaded(counts));

    update(&mut m, press('r'));
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(
        cmds,
        vec![
            Command::LoadTasks(work),
            Command::LoadTasks(home),
            Command::LoadTasks(play),
        ]
    );
}

/// The full-fan-out flag is consumed by the one cascade it triggers: a second
/// `ListsLoaded` with no fresh `r` is lazy again, so covered Lists stay untouched.
#[tokio::test]
async fn the_full_refresh_flag_is_consumed_after_one_cascade() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let (work, home) = (lists[0].id.clone(), lists[1].id.clone());

    let mut m = Model::new();
    m.api_available = true;
    update(&mut m, Message::ListsLoaded(lists.clone()));
    update(&mut m, press('j')); // Work active
    let mut counts = HashMap::new();
    counts.insert(work.clone(), (0usize, 1usize));
    counts.insert(home, (0usize, 1usize));
    update(&mut m, Message::CountsLoaded(counts));

    update(&mut m, press('r'));
    update(&mut m, Message::ListsLoaded(lists.clone()));
    // That full cascade consumed the flag. With no new `r` this one is lazy, and
    // every List is covered, so only the active List is (re-)requested.
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(cmds, vec![Command::LoadTasks(work)]);
}

/// An offline `r` returns before setting the flag, so it never latches a `full`
/// into the next (lazy) cascade.
#[tokio::test]
async fn an_offline_refresh_does_not_latch_the_full_flag() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let (work, home) = (lists[0].id.clone(), lists[1].id.clone());

    let mut m = Model::new();
    m.api_available = false;
    let mut counts = HashMap::new();
    counts.insert(work.clone(), (0usize, 1usize));
    counts.insert(home, (0usize, 1usize));
    update(&mut m, Message::CountsLoaded(counts));
    update(&mut m, Message::ListsLoaded(lists.clone()));
    update(&mut m, press('j')); // Work active

    assert!(
        update(&mut m, press('r')).is_empty(),
        "offline r emits nothing"
    );
    // The flag never set: the next cascade stays lazy (all covered → active only).
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(cmds, vec![Command::LoadTasks(work)]);
}

/// A background List's load failure is dropped: no `TasksLoaded` means no counts,
/// means no meter on that row — the fail-closed state — and it must not overwrite
/// the active pane's status line.
#[tokio::test]
async fn a_background_lists_load_failure_is_dropped() {
    let (l, tasks) = list_with_tasks(&["a"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    update(
        &mut m,
        Message::TasksLoadFailed {
            list: ListId("other".to_string()),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(
        m.status_line, None,
        "a background List's failure must not touch the status line"
    );
}

/// The active List's load failure does reach the status line.
#[tokio::test]
async fn the_active_lists_load_failure_reaches_the_status_line() {
    let (l, tasks) = list_with_tasks(&["a"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    update(
        &mut m,
        Message::TasksLoadFailed {
            list: l.id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn tasks_loaded_fills_the_pane_and_selects_first() {
    let (l, tasks) = list_with_tasks(&["a", "b"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    assert_eq!(m.tasks.len(), 2);
    assert_eq!(m.selected_task, Some(0));
}

#[tokio::test]
async fn changing_list_requests_new_tasks_and_clears_the_pane() {
    let api = FakeTasksApi::new();
    let work = api.insert_list("Work").await.unwrap();
    let home = api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists.clone())); // lands on Today
    update(&mut m, press('j')); // select Work (List 0)
    let wtasks = api.list_tasks(&work.id, true, false, None).await.unwrap();
    update(&mut m, Message::TasksLoaded(work.id.clone(), wtasks));

    // Sidebar-focused `j` moves to Home and asks for its Tasks.
    let cmds = update(&mut m, press('j'));
    assert_eq!(m.selected, Selection::List(1));
    assert!(m.tasks.is_empty());
    assert_eq!(cmds, vec![Command::LoadTasks(home.id.clone())]);
}

#[tokio::test]
async fn cursor_moves_in_the_task_pane_when_it_is_focused() {
    let (l, tasks) = list_with_tasks(&["a", "b", "c"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    update(&mut m, tab()); // focus the task pane
    let cmds = update(&mut m, press('j'));
    assert!(cmds.is_empty());
    assert_eq!(m.selected_task, Some(1));
    assert_eq!(m.selected, Selection::List(0)); // list selection unchanged
}

#[tokio::test]
async fn stale_tasks_loaded_for_another_list_is_ignored() {
    let api = FakeTasksApi::new();
    let work = api.insert_list("Work").await.unwrap();
    let home = api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let htasks = api.list_tasks(&home.id, true, false, None).await.unwrap();
    let _ = &work;

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists)); // lands on Today
    update(&mut m, press('j')); // select Work (List 0)
    update(&mut m, Message::TasksLoaded(home.id.clone(), htasks)); // stale (Home not active)
    assert!(m.tasks.is_empty());
}

// --- `t` / `T` cycle entry type --------------------------------------------
//
// The type lives in the title (ADR-0008), so these are title writes riding
// `SetTitle`. `T` exists so every type is one press from any other: forward-only
// would put Note two presses from Task, and the second lands inside the first's
// flight window and is refused.

/// One task titled `title`, task pane focused, cursor on it.
async fn model_with_one(title: &str) -> (Model, List, Vec<Task>) {
    let (l, tasks) = list_with_tasks(&[title]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, tab());
    (m, l, tasks)
}

/// Apply `key`, then acknowledge the write so the next press is not guarded.
fn cycle(m: &mut Model, key: char) -> Vec<Command> {
    let cmds = update(m, press(key));
    let done = m.tasks[0].clone();
    update(m, Message::TaskUpdated(done));
    cmds
}

#[tokio::test]
async fn t_cycles_task_to_event_to_note_and_back() {
    let (mut m, _l, _t) = model_with_one("Standup").await;
    for expected in ["○ Standup", "— Standup", "Standup"] {
        cycle(&mut m, 't');
        assert_eq!(m.tasks[0].title, expected);
    }
}

#[tokio::test]
async fn shift_t_cycles_the_other_way_and_reaches_note_in_one_press() {
    let (mut m, _l, _t) = model_with_one("Standup").await;
    // The whole reason `T` exists: Note is one press from Task, not two.
    cycle(&mut m, 'T');
    assert_eq!(m.tasks[0].title, "— Standup");
    for expected in ["○ Standup", "Standup"] {
        cycle(&mut m, 'T');
        assert_eq!(m.tasks[0].title, expected);
    }
}

#[tokio::test]
async fn cycling_emits_a_settitle_with_the_raw_title() {
    let (mut m, l, tasks) = model_with_one("Standup").await;
    let cmds = update(&mut m, press('t'));
    assert_eq!(
        cmds,
        vec![Command::SetTitle {
            list: l.id,
            task: tasks[0].id.clone(),
            title: "○ Standup".to_string(),
        }]
    );
}

#[tokio::test]
async fn cycling_an_already_typed_entry_replaces_rather_than_stacks() {
    let (mut m, _l, _t) = model_with_one("○ Standup").await;
    cycle(&mut m, 't');
    assert_eq!(m.tasks[0].title, "— Standup"); // not "— ○ Standup"
}

#[tokio::test]
async fn t_self_heals_a_foreign_glyph_title_on_the_first_press() {
    // Written by Google's own client: glyph-prefixed but not canonical, so it
    // parses as an untyped Task whose display title still leads with the glyph.
    // A plain prefix would stack into "○ ○Standup" here.
    let (mut m, _l, _t) = model_with_one("○Standup").await;
    cycle(&mut m, 't');
    assert_eq!(m.tasks[0].title, "○ Standup");
    cycle(&mut m, 't');
    assert_eq!(m.tasks[0].title, "— Standup");
}

#[tokio::test]
async fn a_title_that_strips_to_nothing_cannot_be_typed() {
    let (mut m, _l, _t) = model_with_one("○").await;
    let first = update(&mut m, press('t'));
    assert!(first.is_empty());
    assert_eq!(m.tasks[0].title, "○"); // untouched, not "○ ○"
    assert!(m.status_line.is_some());

    // A second press must not stack either.
    assert!(update(&mut m, press('t')).is_empty());
    assert_eq!(m.tasks[0].title, "○");
}

#[tokio::test]
async fn typing_a_subtask_works_the_same_as_a_top_level_task() {
    let (l, tasks) = list_with_tasks(&["parent", "child"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    let mut child = tasks[1].clone();
    child.parent = Some(tasks[0].id.clone());
    update(
        &mut m,
        Message::TasksLoaded(l.id.clone(), vec![tasks[0].clone(), child]),
    );
    update(&mut m, tab());
    update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::empty())),
    );

    cycle(&mut m, 't');
    assert_eq!(m.tasks[1].title, "○ child");
}

#[tokio::test]
async fn t_and_shift_t_are_no_ops_with_the_sidebar_focused() {
    let (mut m, _l, _t) = model_with_one("Standup").await;
    update(&mut m, tab()); // back to the sidebar
    for key in ['t', 'T'] {
        assert!(update(&mut m, press(key)).is_empty(), "{key}");
    }
    assert_eq!(m.tasks[0].title, "Standup");
}

#[tokio::test]
async fn t_and_shift_t_are_no_ops_with_no_selection() {
    let mut m = Model::new();
    update(&mut m, tab());
    for key in ['t', 'T'] {
        assert!(update(&mut m, press(key)).is_empty(), "{key}");
    }
}

#[tokio::test]
async fn a_type_change_while_a_write_is_in_flight_is_guarded() {
    let (mut m, _l, _t) = model_with_one("Standup").await;
    assert_eq!(update(&mut m, press('t')).len(), 1);

    let second = update(&mut m, press('t'));
    assert!(second.is_empty());
    assert_eq!(m.tasks[0].title, "○ Standup"); // still the first
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn a_failed_type_change_rolls_back_to_the_prior_title() {
    let (mut m, _l, tasks) = model_with_one("Standup").await;
    update(&mut m, press('t'));
    assert_eq!(m.tasks[0].title, "○ Standup"); // optimistic

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].title, "Standup"); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}
