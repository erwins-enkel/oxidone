//! Reducer tests for due dates (ticket #9): the `d` overlay flow, optimistic
//! set/clear write-through, rollback, and the single-flight guard. `update` is
//! pure, so ISO input keeps these deterministic (no dependence on "today").

use chrono::{NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Command, Message, Model, Overlay};
use oxidone::domain::{List, Selection, Task};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        update(m, ch(c));
    }
}

fn ymd(y: i32, mo: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, mo, d).unwrap()
}

/// Two tasks; "alpha" (index 0) starts with a due date, "beta" without.
async fn model_with_tasks() -> (Model, List, Vec<Task>) {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "alpha".to_string(),
            due: Some(ymd(2026, 8, 1)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "beta".to_string(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks.clone()));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    (m, l, tasks)
}

#[tokio::test]
async fn d_opens_the_due_editor_prefilled_with_the_current_due() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    match &m.overlay {
        Some(Overlay::EditDue { buffer, .. }) => assert_eq!(buffer, "2026-08-01"),
        other => panic!("expected EditDue overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn a_relative_due_resolves_against_the_injected_now_not_the_wall_clock() {
    // `update` is pure: relative dates resolve against `model.now`, which the
    // runtime stamps — so this is deterministic without touching the clock.
    let (mut m, _l, _t) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 3, 14, 9, 0, 0)
        .single()
        .unwrap();

    update(&mut m, ch('d'));
    for _ in 0..10 {
        update(&mut m, key(KeyCode::Backspace)); // clear the prefilled ISO date
    }
    typed(&mut m, "tomorrow");
    update(&mut m, key(KeyCode::Enter));

    assert_eq!(m.tasks[0].due, Some(ymd(2026, 3, 15))); // now + 1 day
}

#[tokio::test]
async fn d_on_a_task_without_a_due_opens_an_empty_editor() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Down)); // select "beta" (no due)
    update(&mut m, ch('d'));
    match &m.overlay {
        Some(Overlay::EditDue { buffer, .. }) => assert!(buffer.is_empty()),
        other => panic!("expected empty EditDue overlay, got {other:?}"),
    }
}

#[tokio::test]
async fn submitting_a_valid_date_sets_due_optimistically_and_emits_setdue() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Down)); // "beta", no due yet
    update(&mut m, ch('d'));
    typed(&mut m, "2026-09-15");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(m.overlay.is_none());
    assert_eq!(m.tasks[1].due, Some(ymd(2026, 9, 15))); // optimistic
    assert_eq!(
        cmds,
        vec![Command::SetDue {
            list: l.id,
            task: tasks[1].id.clone(),
            due: Some(ymd(2026, 9, 15)),
        }]
    );
}

#[tokio::test]
async fn an_empty_buffer_clears_the_due_date() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('d')); // "alpha", due 2026-08-01
    update(&mut m, key(KeyCode::Backspace)); // wipe the prefilled ISO date
    let cmds = {
        // clear the whole "2026-08-01" (10 chars minus the one above).
        for _ in 0..10 {
            update(&mut m, key(KeyCode::Backspace));
        }
        update(&mut m, key(KeyCode::Enter))
    };
    assert!(m.overlay.is_none());
    assert_eq!(m.tasks[0].due, None); // cleared optimistically
    assert_eq!(
        cmds,
        vec![Command::SetDue {
            list: l.id,
            task: tasks[0].id.clone(),
            due: None,
        }]
    );
}

#[tokio::test]
async fn esc_cancels_without_writing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    typed(&mut m, "-nonsense");
    let cmds = update(&mut m, key(KeyCode::Esc));
    assert!(m.overlay.is_none());
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 1))); // unchanged
}

#[tokio::test]
async fn an_unparseable_date_keeps_the_overlay_open_and_reports() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, key(KeyCode::Down)); // "beta"
    update(&mut m, ch('d'));
    typed(&mut m, "notadate");
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert!(cmds.is_empty());
    assert!(matches!(m.overlay, Some(Overlay::EditDue { .. }))); // still open
    assert_eq!(m.tasks[1].due, None); // not touched
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn a_failed_due_write_rolls_back_to_the_snapshot() {
    let (mut m, _l, tasks) = model_with_tasks().await;
    update(&mut m, ch('d')); // "alpha", due 2026-08-01
    for _ in 0..10 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "2027-01-01");
    update(&mut m, key(KeyCode::Enter)); // optimistic
    assert_eq!(m.tasks[0].due, Some(ymd(2027, 1, 1)));

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 1))); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}

#[tokio::test]
async fn a_second_due_edit_while_one_is_in_flight_is_guarded() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    for _ in 0..10 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "2027-01-01");
    let first = update(&mut m, key(KeyCode::Enter));
    assert_eq!(first.len(), 1); // write in flight

    // A second edit of the same Task must not race the first.
    update(&mut m, ch('d'));
    for _ in 0..10 {
        update(&mut m, key(KeyCode::Backspace));
    }
    typed(&mut m, "2027-02-02");
    let second = update(&mut m, key(KeyCode::Enter));
    assert!(second.is_empty());
    assert_eq!(m.tasks[0].due, Some(ymd(2027, 1, 1))); // still the first edit
    assert!(m.status_line.is_some());
}

// --- `m` migrate: Bullet Journal's `>` disposition -------------------------
//
// `max(today, due) + 1` rather than a flat "tomorrow", so the verb composes.
// Every case pins `m.now`, keeping these independent of the wall clock.

/// `model_with_tasks` plus a fixed clock: 2026-08-10, nine days past "alpha"'s
/// due date of 2026-08-01, so "alpha" is overdue and "beta" undated.
async fn model_at_2026_08_10() -> (Model, List, Vec<Task>) {
    let (mut m, l, tasks) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 8, 10, 9, 0, 0)
        .single()
        .unwrap();
    (m, l, tasks)
}

#[tokio::test]
async fn m_on_an_overdue_task_migrates_to_tomorrow_not_to_the_day_after_its_due() {
    let (mut m, l, tasks) = model_at_2026_08_10().await;
    let cmds = update(&mut m, ch('m')); // "alpha", due 2026-08-01, overdue

    // The whole point of `max(today, due)`: a naive `due + 1` would land on
    // 2026-08-02, still in the past.
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 11)));
    assert_eq!(
        cmds,
        vec![Command::SetDue {
            list: l.id,
            task: tasks[0].id.clone(),
            due: Some(ymd(2026, 8, 11)),
        }]
    );
}

#[tokio::test]
async fn m_on_a_future_task_shifts_it_one_day_later() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    m.tasks[0].due = Some(ymd(2026, 12, 25));
    update(&mut m, ch('m'));
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 12, 26)));
}

#[tokio::test]
async fn m_on_a_task_due_today_migrates_to_tomorrow() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    m.tasks[0].due = Some(ymd(2026, 8, 10));
    update(&mut m, ch('m'));
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 11)));
}

#[tokio::test]
async fn m_on_an_undated_task_lands_on_tomorrow() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    update(&mut m, key(KeyCode::Down)); // "beta", no due date
    update(&mut m, ch('m'));
    assert_eq!(m.tasks[1].due, Some(ymd(2026, 8, 11)));
}

#[tokio::test]
async fn repeated_migrations_compose_a_day_at_a_time() {
    let (mut m, _l, tasks) = model_at_2026_08_10().await;

    // Each press must clear the in-flight guard before the next, or the second
    // and third are refused — which is what `T` exists to spare the user.
    for expected in [ymd(2026, 8, 11), ymd(2026, 8, 12), ymd(2026, 8, 13)] {
        update(&mut m, ch('m'));
        assert_eq!(m.tasks[0].due, Some(expected));
        update(
            &mut m,
            Message::TaskUpdated(oxidone::domain::Task {
                due: Some(expected),
                ..tasks[0].clone()
            }),
        );
    }
}

#[tokio::test]
async fn m_refuses_a_completed_task() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    // Set the status directly rather than pressing Space: Space hides the Task
    // (Completed are hidden by default) and re-anchors the cursor onto "beta",
    // so `m` would migrate the wrong Task — and it would also leave a write in
    // flight, which would make this pass on the single-flight guard instead of
    // the Completed check. Reveal them so the cursor still sits on a visible row.
    update(&mut m, ch('c'));
    m.tasks[0].status = oxidone::domain::Status::Completed;
    let before = m.tasks[0].due;

    let cmds = update(&mut m, ch('m'));

    assert!(cmds.is_empty(), "a completed task is not migrated");
    assert_eq!(m.tasks[0].due, before);
    assert_eq!(
        m.status_line.as_deref(),
        Some("completed tasks are not migrated")
    );
}

#[tokio::test]
async fn m_is_a_no_op_with_the_sidebar_focused() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    update(&mut m, key(KeyCode::Tab)); // back to the sidebar
    let cmds = update(&mut m, ch('m'));
    assert!(cmds.is_empty());
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 1))); // untouched
}

#[tokio::test]
async fn m_is_a_no_op_with_no_selection() {
    let mut m = Model::new();
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 8, 10, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, key(KeyCode::Tab)); // task pane, but no tasks at all
    assert!(update(&mut m, ch('m')).is_empty());
}

#[tokio::test]
async fn a_migration_while_a_write_is_in_flight_is_guarded() {
    let (mut m, _l, _t) = model_at_2026_08_10().await;
    let first = update(&mut m, ch('m'));
    assert_eq!(first.len(), 1);

    let second = update(&mut m, ch('m'));
    assert!(second.is_empty());
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 11))); // still the first
    assert!(m.status_line.is_some());
}

#[tokio::test]
async fn a_failed_migration_rolls_back_to_the_prior_due_date() {
    let (mut m, _l, tasks) = model_at_2026_08_10().await;
    update(&mut m, ch('m'));
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 11))); // optimistic

    update(
        &mut m,
        Message::TaskWriteFailed {
            task: tasks[0].id.clone(),
            reason: "boom".to_string(),
        },
    );
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 1))); // rolled back
    assert_eq!(m.status_line.as_deref(), Some("boom"));
}
