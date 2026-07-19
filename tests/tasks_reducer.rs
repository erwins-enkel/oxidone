//! Reducer tests for the Task read path (ticket #5): selecting a List requests
//! its Tasks (a `Command`), `TasksLoaded` fills the pane, and the cursor +
//! focus behave. `update` is pure.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Message, Model};
use oxidone::domain::{List, Task};

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

#[tokio::test]
async fn loading_lists_requests_tasks_for_the_selected_list() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();

    let mut m = Model::new();
    let cmds = update(&mut m, Message::ListsLoaded(lists.clone()));
    assert_eq!(cmds, vec![Command::LoadTasks(lists[0].id.clone())]);
}

#[tokio::test]
async fn tasks_loaded_fills_the_pane_and_selects_first() {
    let (l, tasks) = list_with_tasks(&["a", "b"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
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
    update(&mut m, Message::ListsLoaded(lists.clone()));
    let wtasks = api.list_tasks(&work.id, true, false, None).await.unwrap();
    update(&mut m, Message::TasksLoaded(work.id.clone(), wtasks));

    // Sidebar-focused `j` moves to Home and asks for its Tasks.
    let cmds = update(&mut m, press('j'));
    assert_eq!(m.selected_list, Some(1));
    assert!(m.tasks.is_empty());
    assert_eq!(cmds, vec![Command::LoadTasks(home.id.clone())]);
}

#[tokio::test]
async fn cursor_moves_in_the_task_pane_when_it_is_focused() {
    let (l, tasks) = list_with_tasks(&["a", "b", "c"]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    update(&mut m, tab()); // focus the task pane
    let cmds = update(&mut m, press('j'));
    assert!(cmds.is_empty());
    assert_eq!(m.selected_task, Some(1));
    assert_eq!(m.selected_list, Some(0)); // list selection unchanged
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
    update(&mut m, Message::ListsLoaded(lists)); // Work selected
    update(&mut m, Message::TasksLoaded(home.id.clone(), htasks)); // stale
    assert!(m.tasks.is_empty());
}
