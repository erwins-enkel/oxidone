//! Per-List completion counts (#46): the cache aggregate that feeds the sidebar
//! meters, and the reducer rules that decide which List reads which source.
//!
//! The runtime re-derives counts from the cache after every change and hands
//! them over as `CountsLoaded`; these tests stand in for that edge by applying
//! the Message directly. `update` is pure — no terminal.

use chrono::{DateTime, TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, TasksApi};
use oxidone::app::{update, Focus, Message, Model};
use oxidone::cache::Cache;
use oxidone::domain::{List, ListId, Status, Task, TaskId};
use std::collections::HashMap;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
}

/// A Task in list `L`. `parent` is a raw id so the depth-2 and orphan shapes can
/// be built directly — the point of those cases is what the grouping rules make
/// of a `parent` that is not a top-level Task.
fn task(id: &str, status: Status, parent: Option<&str>) -> Task {
    Task {
        id: TaskId(id.to_string()),
        list: ListId("L".to_string()),
        parent: parent.map(|p| TaskId(p.to_string())),
        title: format!("task {id}"),
        notes: None,
        status,
        due: None,
        completed_at: (status == Status::Completed).then(|| ts(10)),
        position: format!("{id:0>20}"),
        etag: "etag".to_string(),
        updated: ts(0),
    }
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.to_string()),
        title: format!("list {id}"),
        etag: String::new(),
        updated: ts(0),
    }
}

fn counts(pairs: &[(&str, (usize, usize))]) -> HashMap<ListId, (usize, usize)> {
    pairs
        .iter()
        .map(|(id, c)| (ListId((*id).to_string()), *c))
        .collect()
}

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

// ---------------------------------------------------------------- cache seam

#[test]
fn counts_done_and_total_per_list() {
    let cache = Cache::open_in_memory().unwrap();
    let (a, b) = (ListId("A".to_string()), ListId("B".to_string()));
    cache
        .replace_tasks(
            &a,
            &[
                Task {
                    list: a.clone(),
                    ..task("1", Status::Completed, None)
                },
                Task {
                    list: a.clone(),
                    ..task("2", Status::NeedsAction, None)
                },
            ],
        )
        .unwrap();
    cache
        .replace_tasks(
            &b,
            &[Task {
                list: b.clone(),
                ..task("3", Status::NeedsAction, None)
            }],
        )
        .unwrap();

    let counts = cache.list_counts().unwrap();
    assert_eq!(counts.get(&a), Some(&(1, 2)));
    assert_eq!(counts.get(&b), Some(&(0, 1)));
}

#[test]
fn subtasks_count_like_top_level_tasks() {
    // One definition of `done/total` across the app: the sidebar meter must
    // agree with the task-pane header, which counts every Task in the List.
    let cache = Cache::open_in_memory().unwrap();
    let l = ListId("L".to_string());
    cache
        .replace_tasks(
            &l,
            &[
                task("p", Status::NeedsAction, None),
                task("c1", Status::Completed, Some("p")),
                task("c2", Status::NeedsAction, Some("p")),
            ],
        )
        .unwrap();

    assert_eq!(cache.list_counts().unwrap().get(&l), Some(&(1, 3)));
}

#[test]
fn a_list_with_no_tasks_is_omitted_not_zeroed() {
    // Absent, never `(0, 0)`: it keeps "never fetched" and "empty" the same
    // shape (both draw no meter), and leaves a List whose fetch failed looking
    // uncovered rather than authoritatively empty.
    let cache = Cache::open_in_memory().unwrap();
    let l = ListId("L".to_string());
    cache.replace_lists(&[list("L")]).unwrap();
    cache.replace_tasks(&l, &[]).unwrap();

    assert_eq!(cache.list_counts().unwrap().get(&l), None);
}

#[test]
fn done_survives_a_real_task_round_trip() {
    // Round-tripped through `replace_tasks` rather than writing the status
    // string by hand: that is what makes this fail if the stored spelling and
    // the aggregate's bound parameter ever drift apart.
    let cache = Cache::open_in_memory().unwrap();
    let l = ListId("L".to_string());
    cache
        .replace_tasks(
            &l,
            &[
                task("a", Status::Completed, None),
                task("b", Status::Completed, None),
                task("c", Status::NeedsAction, None),
            ],
        )
        .unwrap();

    assert_eq!(cache.list_counts().unwrap().get(&l), Some(&(2, 1 + 2)));
}

#[test]
fn a_clear_shaped_rewrite_lowers_done_and_total() {
    // The write Clear Completed performs: re-mirror the List without its
    // Completed rows. Pins that the aggregate moves for it, so recounting after
    // a cache change is guaranteed to observe a Clear.
    let cache = Cache::open_in_memory().unwrap();
    let l = ListId("L".to_string());
    cache
        .replace_tasks(
            &l,
            &[
                task("a", Status::Completed, None),
                task("b", Status::NeedsAction, None),
            ],
        )
        .unwrap();
    assert_eq!(cache.list_counts().unwrap().get(&l), Some(&(1, 2)));

    cache
        .replace_tasks(&l, &[task("b", Status::NeedsAction, None)])
        .unwrap();
    assert_eq!(cache.list_counts().unwrap().get(&l), Some(&(0, 1)));
}

// -------------------------------------------------------------- reducer seam

#[tokio::test]
async fn counts_and_lists_may_arrive_in_either_order() {
    // The runtime recounts at the event edge, so `CountsLoaded` can land either
    // side of `ListsLoaded`. Neither order may lose a meter.
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let id = lists[0].id.clone();
    let seeded = counts(&[(&id.0, (2, 5))]);

    let mut lists_first = Model::new();
    update(&mut lists_first, Message::ListsLoaded(lists.clone()));
    update(&mut lists_first, Message::CountsLoaded(seeded.clone()));

    let mut counts_first = Model::new();
    update(&mut counts_first, Message::CountsLoaded(seeded));
    update(&mut counts_first, Message::ListsLoaded(lists));

    assert_eq!(lists_first.list_meter(&id), Some((2, 5)));
    assert_eq!(counts_first.list_meter(&id), lists_first.list_meter(&id));
}

#[test]
fn counts_loaded_replaces_rather_than_merges() {
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list("A"), list("B")]));
    update(
        &mut m,
        Message::CountsLoaded(counts(&[("A", (1, 2)), ("B", (0, 3))])),
    );
    update(&mut m, Message::CountsLoaded(counts(&[("A", (2, 2))])));

    assert_eq!(m.list_meter(&ListId("A".to_string())), Some((2, 2)));
    // B is gone from the map, not merged forward from the previous snapshot.
    assert_eq!(m.list_meter(&ListId("B".to_string())), None);
}

#[test]
fn a_count_for_an_unknown_list_is_harmless() {
    // Unfiltered on purpose: an entry for a List the sidebar does not draw can
    // never be looked up, so filtering would only make the arm order-sensitive.
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list("A")]));
    update(
        &mut m,
        Message::CountsLoaded(counts(&[("A", (1, 2)), ("gone", (9, 9))])),
    );

    assert_eq!(m.list_meter(&ListId("A".to_string())), Some((1, 2)));
    assert_eq!(m.lists.len(), 1);
}

#[tokio::test]
async fn a_newly_active_list_reports_its_counts_before_its_tasks_land() {
    // Switching empties the pane; the row must fall back to the cache-derived
    // count rather than deriving `(0, 0)` from a pane that is merely not loaded.
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    api.insert_list("Home").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let (work, home) = (lists[0].id.clone(), lists[1].id.clone());

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists));
    update(
        &mut m,
        Message::CountsLoaded(counts(&[(&work.0, (1, 4)), (&home.0, (3, 3))])),
    );
    update(
        &mut m,
        Message::TasksLoaded(work.clone(), vec![task("a", Status::NeedsAction, None)]),
    );

    // Sidebar-focused `j` moves to Home; its Tasks have not arrived yet.
    update(&mut m, press('j'));
    assert!(m.tasks.is_empty());
    assert_eq!(m.list_meter(&home), Some((3, 3)));
    // And switching back, still before either load lands, is equally safe.
    update(&mut m, press('k'));
    assert_eq!(m.list_meter(&work), Some((1, 4)));
}

#[tokio::test]
async fn completing_a_task_moves_the_active_meter_in_the_same_pass() {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let id = lists[0].id.clone();

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists));
    update(&mut m, Message::CountsLoaded(counts(&[(&id.0, (0, 2))])));
    update(
        &mut m,
        Message::TasksLoaded(
            id.clone(),
            vec![
                task("a", Status::NeedsAction, None),
                task("b", Status::NeedsAction, None),
            ],
        ),
    );
    assert_eq!(m.list_meter(&id), Some((0, 2)));

    m.focus = Focus::Tasks;
    // Space completes the selected Task. The cache still says `(0, 2)` — the
    // write has not been confirmed — so this can only come from the live arm.
    update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty())),
    );
    assert_eq!(m.list_meter(&id), Some((1, 2)));

    // A rejected write rolls the Task back, and the meter with it.
    update(
        &mut m,
        Message::TaskWriteFailed {
            task: TaskId("a".to_string()),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.list_meter(&id), Some((0, 2)));
}

#[tokio::test]
async fn emptying_the_active_list_falls_back_until_the_recount_lands() {
    // The accepted cost of keying the live arm on an empty pane: for one
    // round-trip the row shows its pre-delete count. Pinned so it is a known
    // state rather than a surprise.
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.unwrap();
    let lists = api.list_lists().await.unwrap();
    let id = lists[0].id.clone();

    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(lists));
    update(&mut m, Message::CountsLoaded(counts(&[(&id.0, (0, 1))])));
    update(
        &mut m,
        Message::TasksLoaded(id.clone(), vec![task("a", Status::NeedsAction, None)]),
    );

    // `x` then `y` deletes the only Task, optimistically emptying the pane.
    m.focus = Focus::Tasks;
    update(&mut m, press('x'));
    update(&mut m, press('y'));
    assert!(m.tasks.is_empty());
    assert_eq!(m.list_meter(&id), Some((0, 1)), "stale until the recount");

    // Once the delete is mirrored the aggregate omits the List entirely.
    update(&mut m, Message::CountsLoaded(HashMap::new()));
    assert_eq!(m.list_meter(&id), None);
}

#[test]
fn an_empty_or_never_fetched_list_shows_no_meter() {
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list("A")]));
    // Never fetched: no entry at all.
    assert_eq!(m.list_meter(&ListId("A".to_string())), None);
    // Fetched and empty is deliberately the same shape, not `Some((0, 0))`.
    update(&mut m, Message::CountsLoaded(counts(&[("A", (0, 0))])));
    assert_eq!(m.list_meter(&ListId("A".to_string())), None);
}

// --------------------------------------------------- per-parent subtask counts

/// One parent's Subtask counts, the way the renderer asks for them: over the
/// `top_level` set that decides which rows are drawn indented.
fn subtask_meter(m: &Model, parent: &str) -> Option<(usize, usize)> {
    let top_level = m.top_level_ids();
    m.subtask_counts(&top_level)
        .get(&TaskId(parent.to_string()))
        .copied()
}

/// Load `tasks` into a single-List Model, task pane focused.
fn model_with(tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![list("L")]));
    update(&mut m, Message::TasksLoaded(ListId("L".to_string()), tasks));
    m
}

#[test]
fn a_parent_counts_only_its_own_subtasks() {
    let m = model_with(vec![
        task("p", Status::NeedsAction, None),
        task("c1", Status::Completed, Some("p")),
        task("c2", Status::NeedsAction, Some("p")),
        task("other", Status::Completed, None),
    ]);

    assert_eq!(subtask_meter(&m, "p"), Some((1, 2)));
    assert_eq!(subtask_meter(&m, "other"), None);
}

#[test]
fn the_meter_ignores_the_completed_filter() {
    // The rows are a lens; the meter is over the model. With the filter on, a
    // parent whose Subtasks are all done shows no children at all — the meter is
    // then the only evidence they exist, so `0/2` would be the exact opposite of
    // the truth.
    let m = model_with(vec![
        task("p", Status::NeedsAction, None),
        task("c1", Status::Completed, Some("p")),
        task("c2", Status::Completed, Some("p")),
    ]);

    assert!(!m.show_completed);
    assert_eq!(subtask_meter(&m, "p"), Some((2, 2)));
    assert_eq!(m.visible_tasks().len(), 1, "only the parent is drawn");
}

#[test]
fn a_depth_two_task_is_nobodys_subtask() {
    // `deep`'s parent is itself a Subtask, so the pane draws `deep` flush-left as
    // its own group. Counting raw `parent` would give `a1` a meter and credit it
    // a child drawn elsewhere.
    let m = model_with(vec![
        task("A", Status::NeedsAction, None),
        task("a1", Status::NeedsAction, Some("A")),
        task("deep", Status::Completed, Some("a1")),
    ]);

    assert_eq!(subtask_meter(&m, "A"), Some((0, 1)));
    assert_eq!(subtask_meter(&m, "a1"), None);
    assert_eq!(subtask_meter(&m, "deep"), None);
}

#[test]
fn an_orphan_and_its_child_are_both_their_own_groups() {
    // The orphan's parent is not in the List at all, so it draws flush-left; its
    // child is parented to a non-top-level Task and does the same.
    let m = model_with(vec![
        task("orphan", Status::NeedsAction, Some("missing")),
        task("child", Status::Completed, Some("orphan")),
    ]);

    assert_eq!(subtask_meter(&m, "orphan"), None);
    assert_eq!(subtask_meter(&m, "child"), None);
}
