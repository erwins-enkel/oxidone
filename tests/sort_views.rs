//! Tests for Local Sort views (ticket #12): a read-only lens over the task pane
//! that cycles Manual → Due → Title and never mutates Manual order nor writes.
//! Two seams: the pure reducer (`update`) and the pure `sorted_tasks` helper.

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Message, Model};
use oxidone::domain::{List, SortView, Task};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// Build a List whose Tasks carry the given `(title, due)` pairs, in this order
/// (which becomes their Manual/`position` order via the fake's insertion).
async fn list_with(specs: &[(&str, Option<NaiveDate>)]) -> (List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    for (title, due) in specs {
        api.insert_task(
            &l.id,
            NewTask {
                title: title.to_string(),
                due: *due,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    (l, tasks)
}

fn titles(tasks: &[&Task]) -> Vec<String> {
    tasks.iter().map(|t| t.title.clone()).collect()
}

// --- Reducer: the sort key cycles the lens without writing ------------------

#[tokio::test]
async fn sort_key_cycles_manual_due_title_manual() {
    let (l, tasks) = list_with(&[("a", None)]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    assert_eq!(m.sort, SortView::Manual); // default home state

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Due);
    assert!(cmds.is_empty(), "sorting must not emit any Command");

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Title);
    assert!(cmds.is_empty());

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Manual); // back to the home state
    assert!(cmds.is_empty());
}

#[tokio::test]
async fn sort_never_mutates_manual_order() {
    // Stored (Manual) order is c, a, b — deliberately not sorted.
    let (l, tasks) = list_with(&[("c", None), ("a", None), ("b", None)]).await;
    let stored: Vec<String> = tasks.iter().map(|t| t.title.clone()).collect();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    // Cycle through every view; `model.tasks` must never be reordered.
    for _ in 0..4 {
        update(&mut m, press('s'));
        let now: Vec<String> = m.tasks.iter().map(|t| t.title.clone()).collect();
        assert_eq!(now, stored, "Manual order (tasks Vec) must stay untouched");
    }
}

#[tokio::test]
async fn cursor_stays_on_same_task_across_a_sort_change() {
    // Stored order c, a, b; put the cursor on "a" (index 1).
    let (l, tasks) = list_with(&[("c", None), ("a", None), ("b", None)]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    m.selected_task = Some(1);
    let selected_id = m.tasks[1].id.clone();

    update(&mut m, press('s')); // Manual -> Due
    update(&mut m, press('s')); // Due -> Title

    // `selected_task` indexes the (unchanged) tasks Vec, so it still points at
    // the same Task by id — the view maps it to a display position.
    let now = m.selected_task.and_then(|i| m.tasks.get(i)).map(|t| &t.id);
    assert_eq!(now, Some(&selected_id));
}

// --- Pure helper: display order for Due and Title ---------------------------

#[tokio::test]
async fn due_sort_orders_by_date_and_sinks_no_due_to_the_bottom() {
    // Mixed: some dated, some undated; stored order jumbled.
    let (l, tasks) = list_with(&[
        ("later", Some(ymd(2026, 3, 10))),
        ("undated-1", None),
        ("soon", Some(ymd(2026, 1, 5))),
        ("undated-2", None),
        ("mid", Some(ymd(2026, 2, 1))),
    ])
    .await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    m.sort = SortView::Due;

    // Dated ascending, then the no-due tail in stored order (deterministic).
    assert_eq!(
        titles(&m.sorted_tasks()),
        vec!["soon", "mid", "later", "undated-1", "undated-2"],
    );
}

#[tokio::test]
async fn title_sort_is_case_insensitive() {
    let (l, tasks) = list_with(&[
        ("banana", None),
        ("Apple", None),
        ("cherry", None),
        ("apricot", None),
    ])
    .await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    m.sort = SortView::Title;

    // "Apple" sorts before "apricot" despite the capital A (case-insensitive).
    assert_eq!(
        titles(&m.sorted_tasks()),
        vec!["Apple", "apricot", "banana", "cherry"],
    );
}

#[tokio::test]
async fn manual_sort_is_the_stored_order() {
    let (l, tasks) = list_with(&[("c", None), ("a", None), ("b", None)]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    assert_eq!(m.sort, SortView::Manual);
    assert_eq!(titles(&m.sorted_tasks()), vec!["c", "a", "b"]);
}
