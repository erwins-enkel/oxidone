//! Tests for Local Sort views (ticket #12): a read-only lens over the task pane
//! that cycles Due → Title → Manual from the `Due` home state, keeps Subtasks
//! grouped under their parent in every view, and never mutates Manual order nor
//! writes `position`/`parent`.
//! Two seams: the pure reducer (`update`) and the pure `sorted_tasks` helper.

use chrono::NaiveDate;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{renders_as_subtask, update, Focus, Message, Model};
use oxidone::domain::{List, ListId, SortView, Status, Task, TaskId};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::empty())
}

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

// --- The home state ---------------------------------------------------------

#[test]
fn default_sort_is_due() {
    assert_eq!(Model::new().sort, SortView::Due);
}

#[test]
fn every_lens_names_itself_in_the_pane_title() {
    // The header reads "Tasks — {label}". With Due the home state, an unlabelled
    // view would make Manual the silent one, so all three name themselves.
    assert_eq!(SortView::Due.label(), "due");
    assert_eq!(SortView::Title.label(), "title");
    assert_eq!(SortView::Manual.label(), "my order");
}

// --- Reducer: the sort key cycles the lens without writing ------------------

#[tokio::test]
async fn sort_key_cycles_due_title_manual_due() {
    let (l, tasks) = list_with(&[("a", None)]).await;
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));

    assert_eq!(m.sort, SortView::Due); // default home state

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Title);
    assert!(cmds.is_empty(), "sorting must not emit any Command");

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Manual);
    assert!(cmds.is_empty());

    let cmds = update(&mut m, press('s'));
    assert_eq!(m.sort, SortView::Due); // back to the home state
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

    update(&mut m, press('s')); // Due -> Title
    update(&mut m, press('s')); // Title -> Manual

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
    m.sort = SortView::Manual; // the subject here is Manual's order, not the default

    assert_eq!(titles(&m.sorted_tasks()), vec!["c", "a", "b"]);
}

// --- Hierarchy: every lens groups Subtasks under their parent ---------------

/// A Task built directly, so a test can pin `parent`, `due` and `status`
/// independently of what the fake's insertion path allows.
fn task(id: &str, parent: Option<&str>, due: Option<NaiveDate>, status: Status) -> Task {
    Task {
        id: TaskId(id.to_string()),
        list: ListId("L".to_string()),
        parent: parent.map(|p| TaskId(p.to_string())),
        title: id.to_string(),
        notes: None,
        status,
        due,
        completed_at: None,
        position: String::new(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    }
}

fn open(id: &str, parent: Option<&str>, due: Option<NaiveDate>) -> Task {
    task(id, parent, due, Status::NeedsAction)
}

/// A Model holding `tasks` in the given Vec (Manual) order, in `sort`.
fn model(tasks: Vec<Task>, sort: SortView) -> Model {
    let mut m = Model::new();
    m.tasks = tasks;
    m.sort = sort;
    m
}

#[test]
fn due_sort_keeps_subtasks_under_their_parent() {
    // Stored order interleaves the groups; each parent must still carry its own.
    let m = model(
        vec![
            open("B", None, Some(ymd(2026, 3, 1))),
            open("b1", Some("B"), Some(ymd(2026, 9, 9))),
            open("A", None, Some(ymd(2026, 1, 1))),
            open("a1", Some("A"), Some(ymd(2026, 8, 8))),
        ],
        SortView::Due,
    );
    assert_eq!(titles(&m.sorted_tasks()), vec!["A", "a1", "B", "b1"]);
}

#[test]
fn title_sort_keeps_subtasks_under_their_parent() {
    let m = model(
        vec![
            open("B", None, None),
            open("b1", Some("B"), None),
            open("A", None, None),
            open("a1", Some("A"), None),
        ],
        SortView::Title,
    );
    assert_eq!(titles(&m.sorted_tasks()), vec!["A", "a1", "B", "b1"]);
}

#[test]
fn subtasks_are_ordered_by_due_within_their_parent() {
    let m = model(
        vec![
            open("A", None, Some(ymd(2026, 1, 1))),
            open("late", Some("A"), Some(ymd(2026, 6, 1))),
            open("undated", Some("A"), None),
            open("early", Some("A"), Some(ymd(2026, 2, 1))),
        ],
        SortView::Due,
    );
    assert_eq!(
        titles(&m.sorted_tasks()),
        vec!["A", "early", "late", "undated"],
    );
}

// --- The group key: earliest due among the group's INCOMPLETE Tasks ---------

#[test]
fn a_subtask_due_sooner_pulls_its_parent_group_up() {
    // "later" is dated; "undated" has no due of its own but owns a Subtask due
    // today, so its group must sort above "later".
    let m = model(
        vec![
            open("later", None, Some(ymd(2026, 5, 1))),
            open("undated", None, None),
            open("urgent", Some("undated"), Some(ymd(2026, 1, 1))),
        ],
        SortView::Due,
    );
    assert_eq!(
        titles(&m.sorted_tasks()),
        vec!["undated", "urgent", "later"],
    );
}

#[test]
fn a_completed_subtask_never_sets_the_group_key() {
    // "parent" is undated; its only dated Subtask is Completed, so the group has
    // no key and sinks below the dated "later" — with the filter off AND on.
    let tasks = vec![
        open("later", None, Some(ymd(2026, 5, 1))),
        open("parent", None, None),
        task(
            "done",
            Some("parent"),
            Some(ymd(2020, 1, 1)),
            Status::Completed,
        ),
    ];

    let hidden = model(tasks.clone(), SortView::Due);
    assert_eq!(titles(&hidden.visible_tasks()), vec!["later", "parent"]);

    let mut shown = model(tasks, SortView::Due);
    shown.show_completed = true;
    assert_eq!(
        titles(&shown.sorted_tasks()),
        vec!["later", "parent", "done"],
        "revealing a Completed Task must not reorder the groups",
    );
}

// --- Stability over stored order --------------------------------------------

#[test]
fn equal_due_groups_keep_stored_order() {
    let m = model(
        vec![
            open("second", None, Some(ymd(2026, 1, 1))),
            open("first", None, Some(ymd(2026, 1, 1))),
        ],
        SortView::Due,
    );
    assert_eq!(titles(&m.sorted_tasks()), vec!["second", "first"]);
}

#[test]
fn undated_groups_keep_stored_order() {
    let m = model(
        vec![
            open("dated", None, Some(ymd(2026, 1, 1))),
            open("undated-1", None, None),
            open("undated-2", None, None),
        ],
        SortView::Due,
    );
    assert_eq!(
        titles(&m.sorted_tasks()),
        vec!["dated", "undated-1", "undated-2"],
    );
}

#[test]
fn equal_due_subtasks_keep_manual_order_within_their_parent() {
    let m = model(
        vec![
            open("A", None, Some(ymd(2026, 1, 1))),
            open("second", Some("A"), Some(ymd(2026, 2, 2))),
            open("first", Some("A"), Some(ymd(2026, 2, 2))),
        ],
        SortView::Due,
    );
    assert_eq!(titles(&m.sorted_tasks()), vec!["A", "second", "first"]);
}

// --- Orphans: visible, sorted on their own key, never drawn as someone's child

#[test]
fn orphaned_subtasks_sort_by_their_own_key() {
    // "orphan" names a parent that isn't in the set. Under Due it sorts on its
    // own (earliest) date rather than sinking below the undated group.
    let tasks = vec![
        open("undated", None, None),
        open("orphan", Some("gone"), Some(ymd(2020, 1, 1))),
        open("later", None, Some(ymd(2026, 5, 1))),
    ];

    let due = model(tasks.clone(), SortView::Due);
    assert_eq!(
        titles(&due.sorted_tasks()),
        vec!["orphan", "later", "undated"],
    );

    // Manual keeps appending orphans last, unchanged.
    let manual = model(tasks, SortView::Manual);
    assert_eq!(
        titles(&manual.sorted_tasks()),
        vec!["undated", "later", "orphan"],
    );
}

#[test]
fn renders_as_subtask_requires_the_parent_to_be_present() {
    let mut m = model(
        vec![
            open("A", None, None),
            open("a1", Some("A"), None),
            open("orphan", Some("gone"), None),
        ],
        SortView::Due,
    );

    fn by(m: &Model, id: &str) -> Task {
        m.tasks.iter().find(|t| t.id.0 == id).unwrap().clone()
    }
    let top = m.top_level_ids();
    assert!(
        !renders_as_subtask(&top, &by(&m, "A")),
        "top-level never indents"
    );
    assert!(renders_as_subtask(&top, &by(&m, "a1")), "parent present");
    assert!(
        !renders_as_subtask(&top, &by(&m, "orphan")),
        "parent absent"
    );

    // A hidden Completed parent is still a parent: the indent must not flicker
    // when `show_completed` toggles.
    m.tasks[0].status = Status::Completed;
    assert!(renders_as_subtask(&m.top_level_ids(), &by(&m, "a1")));
    m.show_completed = true;
    assert!(renders_as_subtask(&m.top_level_ids(), &by(&m, "a1")));
}

// --- The cursor re-anchors in display order ---------------------------------

/// A focused task pane holding `tasks` in Due order, cursor on the row at
/// `display_pos` of the *displayed* order.
fn pane(tasks: Vec<Task>, display_pos: usize) -> Model {
    let mut m = Model::new();
    let l = List {
        id: ListId("L".to_string()),
        title: "L".to_string(),
        etag: String::new(),
        updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
    };
    m.lists = vec![l];
    m.selected_list = Some(0);
    m.tasks = tasks;
    m.focus = Focus::Tasks;
    let id = m.sorted_tasks()[display_pos].id.clone();
    m.selected_task = m.tasks.iter().position(|t| t.id == id);
    m
}

fn selected_title(m: &Model) -> Option<String> {
    m.selected_task
        .and_then(|i| m.tasks.get(i))
        .map(|t| t.title.clone())
}

/// Stored order is deliberately not display order: c (undated) sorts last, so
/// the pane reads b, a, c.
fn scrambled() -> Vec<Task> {
    vec![
        open("c", None, None),
        open("a", None, Some(ymd(2026, 8, 1))),
        open("b", None, Some(ymd(2026, 7, 21))),
    ]
}

#[test]
fn deleting_a_task_anchors_the_next_by_due_date() {
    let mut m = pane(scrambled(), 0); // on "b", the first displayed row
    assert_eq!(selected_title(&m), Some("b".to_string()));

    update(&mut m, press('x'));
    update(&mut m, press('y')); // confirm

    assert_eq!(
        selected_title(&m),
        Some("a".to_string()),
        "the next row by due date, not stored index 0 (\"c\")",
    );
}

#[test]
fn deleting_the_last_displayed_task_anchors_the_previous_one() {
    let mut m = pane(scrambled(), 2); // on "c", the last displayed row
    update(&mut m, press('x'));
    update(&mut m, press('y'));
    assert_eq!(selected_title(&m), Some("a".to_string()));
}

/// A rollback arrives asynchronously, so it must not yank the cursor off
/// whatever the user has moved to meanwhile — but re-inserting shifts every
/// later index, so the selection still has to be re-resolved by id.
#[test]
fn a_delete_rollback_restores_the_task_without_moving_the_cursor() {
    let mut m = pane(scrambled(), 0); // on "b"
    let deleted = m.tasks[m.selected_task.unwrap()].id.clone();
    update(&mut m, press('x'));
    update(&mut m, press('y'));
    assert_eq!(selected_title(&m), Some("a".to_string()));

    // The user carries on and moves to another row before the failure lands.
    update(&mut m, press('j'));
    assert_eq!(selected_title(&m), Some("c".to_string()));

    update(
        &mut m,
        Message::TaskDeleteFailed {
            task: deleted,
            reason: "boom".to_string(),
        },
    );

    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a", "c"], "restored");
    assert_eq!(
        selected_title(&m),
        Some("c".to_string()),
        "the cursor stays where the user left it",
    );
}

/// An add failure only re-homes a cursor that is *on* the placeholder.
#[test]
fn add_failure_leaves_an_unrelated_cursor_alone() {
    let mut m = pane(scrambled(), 0);
    update(&mut m, press('a'));
    for c in "zz".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(key(KeyCode::Enter)));
    let temp = m.tasks[m.selected_task.unwrap()].id.clone();

    // Move off the placeholder before the failure arrives.
    let target = m.tasks.iter().position(|t| t.title == "a").unwrap();
    m.selected_task = Some(target);

    update(
        &mut m,
        Message::TaskAddFailed {
            temp,
            reason: "boom".to_string(),
        },
    );

    assert_eq!(
        selected_title(&m),
        Some("a".to_string()),
        "an unrelated cursor is left where the user put it, index shift and all",
    );
}

#[test]
fn add_failure_anchors_the_next_in_display_order() {
    let mut m = pane(scrambled(), 0);
    update(&mut m, press('a'));
    for c in "zz".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(key(KeyCode::Enter)));

    // The placeholder is undated, so it displays last; the cursor follows it.
    let temp = m.tasks[m.selected_task.unwrap()].id.clone();
    assert_eq!(selected_title(&m), Some("zz".to_string()));

    update(
        &mut m,
        Message::TaskAddFailed {
            temp,
            reason: "boom".to_string(),
        },
    );

    assert_eq!(
        selected_title(&m),
        Some("c".to_string()),
        "anchors on the display neighbour, not a clamped stored index",
    );
}

#[test]
fn tasks_loaded_without_the_selected_task_falls_back_to_first_displayed() {
    let mut m = pane(scrambled(), 1); // on "a"
    let list = m.lists[0].id.clone();

    // A refresh drops "a" entirely.
    let remaining: Vec<Task> = m.tasks.iter().filter(|t| t.title != "a").cloned().collect();
    update(&mut m, Message::TasksLoaded(list, remaining));

    assert_eq!(
        selected_title(&m),
        Some("b".to_string()),
        "first displayed row, not stored index 0 (\"c\")",
    );
}

#[test]
fn completing_a_task_anchors_the_next_by_due_date() {
    let mut m = pane(scrambled(), 0); // on "b"
    update(&mut m, press(' ')); // complete it; completed are hidden

    assert_eq!(
        selected_title(&m),
        Some("a".to_string()),
        "the next Task by due date, the Google-app behaviour",
    );
}

#[test]
fn add_task_keeps_the_cursor_on_the_new_task_at_the_tail() {
    let mut m = pane(scrambled(), 0);
    update(&mut m, press('a'));
    for c in "new".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(key(KeyCode::Enter)));

    // `add_task_placeholder` inserts at stored index 0 and selects that index.
    assert_eq!(m.selected_task, Some(0));
    assert_eq!(selected_title(&m), Some("new".to_string()));

    // Undated, so it renders in the tail — below every dated Task. It is not the
    // final row: "c" is undated too and follows it in stored order.
    assert_eq!(titles(&m.visible_tasks()), vec!["b", "a", "new", "c"]);
}

#[test]
fn every_lens_keeps_every_task_exactly_once() {
    // Empty model: no panics anywhere in the new paths.
    let m = model(vec![], SortView::Due);
    assert!(m.sorted_tasks().is_empty());
    assert!(m.visible_tasks().is_empty());

    // A parent whose only child is an orphan-by-id of itself is impossible, but a
    // group of exactly one must not panic on the `group[1..]` slice.
    let m = model(vec![open("solo", None, None)], SortView::Due);
    assert_eq!(titles(&m.sorted_tasks()), vec!["solo"]);

    // Every Task appears exactly once, whatever the lens.
    let tasks = vec![
        open("A", None, Some(ymd(2026, 1, 1))),
        open("a1", Some("A"), None),
        open("orphan", Some("gone"), None),
        open("B", None, None),
    ];
    for sort in [SortView::Manual, SortView::Due, SortView::Title] {
        let m = model(tasks.clone(), sort);
        let mut got = titles(&m.sorted_tasks());
        got.sort();
        assert_eq!(got, vec!["A", "B", "a1", "orphan"], "{sort:?} lost a Task");
    }
}

#[test]
fn completing_the_only_visible_task_clears_the_cursor() {
    let mut m = pane(vec![open("only", None, None)], 0);
    update(&mut m, press(' '));
    assert_eq!(m.selected_task, None, "nothing visible left to anchor on");
}

#[test]
fn deleting_the_only_task_clears_the_cursor() {
    let mut m = pane(vec![open("only", None, None)], 0);
    update(&mut m, press('x'));
    update(&mut m, press('y'));
    assert_eq!(m.selected_task, None);
}

#[test]
fn a_child_orphaned_by_a_parent_delete_stays_visible_and_flush_left() {
    // Deleting a parent orphans its child; the child must stay visible, and stop
    // rendering as a Subtask.
    let mut m = pane(
        vec![open("A", None, None), open("a1", Some("A"), None)],
        0, // on "A"
    );
    update(&mut m, press('x'));
    update(&mut m, press('y'));

    assert_eq!(titles(&m.visible_tasks()), vec!["a1"]);
    let child = m.tasks[0].clone();
    assert!(
        !renders_as_subtask(&m.top_level_ids(), &child),
        "its parent is gone, so it must not draw indented"
    );
}

// --- Mutations elsewhere must keep the cursor on the same Task ---------------

#[test]
fn clearing_completed_keeps_the_cursor_on_the_same_task() {
    // A Completed Task ahead of the cursor: sweeping it shifts every later index.
    let mut m = pane(
        vec![
            task("done", None, None, Status::Completed),
            open("a", None, Some(ymd(2026, 1, 1))),
            open("b", None, Some(ymd(2026, 2, 1))),
            open("c", None, Some(ymd(2026, 3, 1))),
        ],
        2, // displayed: a, b, c (done is hidden) -> on "c"
    );
    m.show_completed = true; // reveal so the cursor sits past the swept row
    let on = selected_title(&m);
    assert_eq!(on, Some("c".to_string()));

    update(&mut m, press('C')); // clear completed
    update(&mut m, press('y'));

    assert_eq!(m.tasks.len(), 3, "the Completed Task was actually swept");
    assert_eq!(
        selected_title(&m),
        Some("c".to_string()),
        "the cursor must stay on the Task it was on, not slide to a neighbour",
    );
}

#[test]
fn a_failed_clear_restores_without_moving_the_cursor() {
    let mut m = pane(
        vec![
            task("done", None, None, Status::Completed),
            open("a", None, Some(ymd(2026, 1, 1))),
            open("b", None, Some(ymd(2026, 2, 1))),
        ],
        1, // on "b"
    );
    m.show_completed = true;
    let list = m.lists[0].id.clone();
    update(&mut m, press('C'));
    update(&mut m, press('y'));
    assert_eq!(m.tasks.len(), 2, "the Completed Task was actually swept");
    assert_eq!(selected_title(&m), Some("b".to_string()));

    update(
        &mut m,
        Message::ClearCompletedFailed {
            list,
            reason: "boom".to_string(),
        },
    );

    assert_eq!(
        selected_title(&m),
        Some("b".to_string()),
        "re-inserting the swept Tasks must not slide the cursor",
    );
}

#[test]
fn a_confirmed_insert_after_a_refresh_keeps_the_cursor() {
    // The placeholder is gone (a refresh wiped it), so the confirmed Task is
    // inserted at stored index 0 — shifting every index under the cursor.
    let mut m = pane(scrambled(), 0); // on "b"
    let on = selected_title(&m);

    update(
        &mut m,
        Message::TaskInserted {
            temp: TaskId("temp-gone".to_string()),
            task: open("server", None, Some(ymd(2026, 1, 1))),
        },
    );

    assert_eq!(m.tasks.len(), 4, "the confirmed Task is not lost");
    assert_eq!(
        selected_title(&m),
        on,
        "an insert elsewhere must not drag the cursor to another Task",
    );
}

#[test]
fn clearing_the_row_under_the_cursor_anchors_the_neighbour() {
    // Cursor on a Completed row that the Clear is about to sweep: it must land on
    // the neighbouring surviving row, not jump to the top of the pane.
    let mut m = pane(
        vec![
            open("a", None, Some(ymd(2026, 1, 1))),
            open("b", None, Some(ymd(2026, 2, 1))),
            task("done", None, Some(ymd(2026, 3, 1)), Status::Completed),
            open("d", None, Some(ymd(2026, 4, 1))),
        ],
        0,
    );
    m.show_completed = true;
    // A Completed Task has no group key, so it sits in the tail: a, b, d, done.
    let done = m.tasks.iter().position(|t| t.title == "done").unwrap();
    m.selected_task = Some(done);
    assert_eq!(titles(&m.visible_tasks()), vec!["a", "b", "d", "done"]);

    update(&mut m, press('C'));
    update(&mut m, press('y'));

    assert_eq!(m.tasks.len(), 3, "the Completed Task was swept");
    assert_eq!(
        selected_title(&m),
        Some("d".to_string()),
        "the nearest surviving neighbour — here the row above, since the swept \
         Completed row has no group key and sits last",
    );
}

#[test]
fn a_confirmed_insert_into_an_empty_pane_takes_the_cursor() {
    // A refresh emptied the pane before the add reply landed, so there is no
    // selection to preserve. The confirmed Task must still get the highlight —
    // otherwise the pane renders a row no keypress is pointing at.
    let mut m = pane(scrambled(), 0);
    let list = m.lists[0].id.clone();
    update(&mut m, Message::TasksLoaded(list, vec![]));
    assert_eq!(m.selected_task, None, "nothing left to select");

    update(
        &mut m,
        Message::TaskInserted {
            temp: TaskId("temp-gone".to_string()),
            task: open("server", None, None),
        },
    );

    assert_eq!(
        selected_title(&m),
        Some("server".to_string()),
        "the only row must be selected, not left unhighlighted",
    );
}

#[test]
fn a_task_parented_to_a_subtask_is_not_drawn_as_a_child() {
    // Malformed depth-2 data (the one-level cap should prevent it, but Google is
    // the source of truth). `groups` cannot nest it — it only nests under
    // top-level Tasks — so it becomes its own group, and the indent must agree.
    let m = model(
        vec![
            open("A", None, None),
            open("a1", Some("A"), None),
            open("deep", Some("a1"), None),
        ],
        SortView::Manual,
    );
    let top = m.top_level_ids();
    let deep = m.tasks.iter().find(|t| t.title == "deep").unwrap();

    assert_eq!(titles(&m.sorted_tasks()), vec!["A", "a1", "deep"]);
    assert!(
        !renders_as_subtask(&top, deep),
        "grouping treats it as top-level, so the indent must too",
    );
}

#[test]
fn a_new_subtask_renders_at_the_head_of_its_groups_undated_tail() {
    // The placeholder goes in directly after its parent in stored order, but it
    // has no due date — so under Due it sits in the group's undated tail. It
    // leads that tail rather than ending it: the sort is stable and it is the
    // first child in stored order. The existing undated "someday" pins that; with
    // only dated children the placeholder would be last, which is the weaker case.
    let mut m = pane(
        vec![
            open("A", None, Some(ymd(2026, 1, 1))),
            open("early", Some("A"), Some(ymd(2026, 2, 1))),
            open("someday", Some("A"), None),
            open("late", Some("A"), Some(ymd(2026, 3, 1))),
        ],
        0, // on "A"
    );

    update(&mut m, press('o'));
    for c in "new".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(key(KeyCode::Enter)));

    assert_eq!(m.tasks[1].title, "new", "stored: first child of A");
    assert_eq!(
        titles(&m.visible_tasks()),
        vec!["A", "early", "late", "new", "someday"],
        "displayed: dated children first, then the undated tail in stored order",
    );
    assert_eq!(selected_title(&m), Some("new".to_string()));
}
