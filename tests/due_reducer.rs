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

/// A `Ctrl`-chord: CONTROL alone.
fn chord(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// AltGr, as a Windows console reports it: CONTROL and ALT together. This is how
/// `@ \ [ ] { } ~ | €` are typed on German, Polish and Nordic layouts.
fn altgr(c: char) -> Message {
    Message::Key(KeyEvent::new(
        KeyCode::Char(c),
        KeyModifiers::CONTROL | KeyModifiers::ALT,
    ))
}

/// `Alt` alone, as a macOS terminal with Option-as-Meta sends `Option`+letter.
fn alt(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::ALT))
}

fn buffer_of(m: &Model) -> &str {
    match &m.overlay {
        Some(Overlay::EditDue { buffer, .. }) => buffer,
        other => panic!("expected an EditDue overlay, got {other:?}"),
    }
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
    // One press: the prefill is selected, so Backspace clears the whole line.
    update(&mut m, key(KeyCode::Backspace));
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
                             // The prefill is selected, so one Backspace empties the buffer outright.
    update(&mut m, key(KeyCode::Backspace));
    let cmds = update(&mut m, key(KeyCode::Enter));
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
    update(&mut m, key(KeyCode::Backspace)); // selected prefill: clears it
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
    update(&mut m, key(KeyCode::Backspace)); // selected prefill: clears it
    typed(&mut m, "2027-01-01");
    let first = update(&mut m, key(KeyCode::Enter));
    assert_eq!(first.len(), 1); // write in flight

    // A second edit of the same Task must not race the first.
    update(&mut m, ch('d'));
    update(&mut m, key(KeyCode::Backspace)); // selected prefill: clears it
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

// ---- Modifier policy ----
//
// `^U`/`^W` are the first modifier-bearing keys in the app, so these pin the
// predicate from both sides. Three ways of typing an ordinary character carry a
// modifier — SHIFT on capitals, CONTROL|ALT on AltGr, and ALT alone — and all
// three must still reach the buffer.

#[tokio::test]
async fn a_control_chord_that_is_not_bound_types_nothing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u')); // clear the prefill
    update(&mut m, chord('a'));
    assert_eq!(buffer_of(&m), "", "^A must not insert a literal 'a'");
}

/// AltGr arrives as CONTROL|ALT, and on several European layouts it is the only
/// way to type `@ \ [ ] { } ~ | €`. Crucially the letters tested here are the
/// ones the chords are bound to: a chord arm written as a bare
/// `contains(CONTROL)` fires on AltGr and would kill the line or the word, and
/// no other letter would catch it.
#[tokio::test]
async fn altgr_types_a_character_even_on_a_chord_letter() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d')); // prefilled "2026-08-01", selected

    // On the pristine prefill AltGr+u types, replacing the selection like any
    // other character — where a chord would have cleared it and left "".
    update(&mut m, altgr('u'));
    assert_eq!(buffer_of(&m), "u", "AltGr+u must type, not kill the line");

    // And once editing, it appends rather than clearing.
    update(&mut m, altgr('w'));
    assert_eq!(buffer_of(&m), "uw", "AltGr+w must type, not delete a word");
    update(&mut m, altgr('@'));
    assert_eq!(buffer_of(&m), "uw@");

    // The contrast, on the same letters: CONTROL alone really is a chord.
    update(&mut m, chord('w'));
    assert_eq!(buffer_of(&m), "", "^W deletes the word AltGr just typed");
}

/// ALT alone is untouched by this change: today it pushes the literal character
/// (there is no modifier check at all), and swallowing it would be a second
/// silent behaviour change on top of the one being fixed.
#[tokio::test]
async fn alt_alone_still_types() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    update(&mut m, alt('n'));
    assert_eq!(buffer_of(&m), "n");
}

#[tokio::test]
async fn capitals_still_type_when_the_terminal_sends_shift() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    update(
        &mut m,
        Message::Key(KeyEvent::new(KeyCode::Char('Y'), KeyModifiers::SHIFT)),
    );
    assert_eq!(buffer_of(&m), "Y");
}

#[tokio::test]
async fn control_u_clears_the_line_and_control_w_deletes_a_word() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "next friday");
    update(&mut m, chord('w'));
    assert_eq!(buffer_of(&m), "next ");
    update(&mut m, chord('w'));
    assert_eq!(buffer_of(&m), "");

    typed(&mut m, "2027-01-01");
    update(&mut m, chord('u'));
    assert_eq!(buffer_of(&m), "");
}

// ---- The selected prefill, and stepping ----

#[tokio::test]
async fn the_first_character_replaces_the_selected_prefill() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d')); // prefilled "2026-08-01"
    typed(&mut m, "friday");
    assert_eq!(
        buffer_of(&m),
        "friday",
        "the prefill is selected; the first character replaces it"
    );
}

#[tokio::test]
async fn backspace_after_an_edit_pops_one_character() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    typed(&mut m, "abc"); // no longer pristine
    update(&mut m, key(KeyCode::Backspace));
    assert_eq!(buffer_of(&m), "ab");
}

#[tokio::test]
async fn arrows_step_a_day_and_page_keys_step_a_week() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d')); // prefilled 2026-08-01

    update(&mut m, key(KeyCode::Down));
    assert_eq!(buffer_of(&m), "2026-08-02", "Down is later");
    update(&mut m, key(KeyCode::Up));
    update(&mut m, key(KeyCode::Up));
    assert_eq!(buffer_of(&m), "2026-07-31", "Up is earlier, across a month");
    update(&mut m, key(KeyCode::PageDown));
    assert_eq!(buffer_of(&m), "2026-08-07");
    update(&mut m, key(KeyCode::PageUp));
    assert_eq!(buffer_of(&m), "2026-07-31");
}

/// A step resolves the *buffer*, not the Task's stored date, so it composes with
/// what was typed. 2026-08-14 is a Friday.
#[tokio::test]
async fn stepping_composes_with_typing() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 8, 10, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, ch('d'));
    typed(&mut m, "friday");
    update(&mut m, key(KeyCode::Up));
    assert_eq!(
        buffer_of(&m),
        "2026-08-13",
        "the Thursday before that Friday"
    );
}

/// Stepping is never a dead key on an undated Task: either direction lands on
/// today first, then steps from there.
#[tokio::test]
async fn stepping_from_an_empty_or_unparsable_buffer_lands_on_today() {
    for (press, expected) in [(KeyCode::Down, "2026-03-15"), (KeyCode::Up, "2026-03-13")] {
        let (mut m, _l, _t) = model_with_tasks().await;
        m.now = chrono::Local
            .with_ymd_and_hms(2026, 3, 14, 9, 0, 0)
            .single()
            .unwrap();
        update(&mut m, key(KeyCode::Down)); // select "beta" (no due)
        update(&mut m, ch('d')); // empty buffer
        update(&mut m, key(press));
        assert_eq!(buffer_of(&m), "2026-03-14", "first press lands on today");
        update(&mut m, key(press));
        assert_eq!(buffer_of(&m), expected, "then steps from there");
    }

    // Same for a buffer that does not parse.
    let (mut m, _l, _t) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 3, 14, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "not a date");
    update(&mut m, key(KeyCode::Down));
    assert_eq!(buffer_of(&m), "2026-03-14");
}

#[tokio::test]
async fn a_step_then_enter_writes_the_stepped_date() {
    let (mut m, l, tasks) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, key(KeyCode::PageDown));
    let cmds = update(&mut m, key(KeyCode::Enter));

    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 8)));
    assert_eq!(
        cmds,
        vec![Command::SetDue {
            list: l.id,
            task: tasks[0].id.clone(),
            due: Some(ymd(2026, 8, 8)),
        }]
    );
}

/// A crash guard, not a boundary proof.
///
/// `+262143-12-31` is `NaiveDate::MAX` spelled out, and looks like the way to
/// drive a step off the end of the calendar from the keyboard. It is not: the
/// parser rejects it, so the step falls back to today. That makes this a weak
/// assertion by construction — it cannot distinguish "the step was checked" from
/// "the buffer never reached the boundary" — and it is kept only because a
/// keystroke on an extreme buffer must not panic the TUI whichever way that
/// goes. The real boundary proof is `dateparse::shift_days`' own unit tests,
/// which reach `NaiveDate::MAX` directly and do not depend on any string
/// parsing; that is why the helper is separately testable.
#[tokio::test]
async fn stepping_on_an_extreme_buffer_does_not_panic() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 3, 14, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "+262143-12-31");
    update(&mut m, key(KeyCode::Down));
    assert_eq!(
        buffer_of(&m),
        "2026-03-14",
        "unparsable, so it lands on today"
    );
}

/// A parse error keeps the overlay open so the input can be fixed — and leaves
/// it non-pristine, or the next character would wipe the text being corrected.
#[tokio::test]
async fn a_rejected_buffer_stays_open_and_editable() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "not a date");
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(buffer_of(&m), "not a date", "kept for correction");

    update(&mut m, ch('!'));
    assert_eq!(
        buffer_of(&m),
        "not a date!",
        "the next character appends; it must not wipe the correction"
    );
}

#[tokio::test]
async fn a_bare_number_sets_the_next_day_of_that_month() {
    let (mut m, _l, _t) = model_with_tasks().await;
    m.now = chrono::Local
        .with_ymd_and_hms(2026, 7, 22, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, ch('d'));
    typed(&mut m, "15"); // replaces the prefill
    update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks[0].due, Some(ymd(2026, 8, 15)));
}

/// `Ctrl-U` then a space is empty *after trimming*, which `finish_edit_due`
/// treats as a clear. The preview says so too — see `tests/due_render.rs`.
#[tokio::test]
async fn a_whitespace_only_buffer_clears_the_due_date() {
    let (mut m, _l, _t) = model_with_tasks().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "   ");
    let cmds = update(&mut m, key(KeyCode::Enter));
    assert_eq!(m.tasks[0].due, None);
    assert!(matches!(cmds[..], [Command::SetDue { due: None, .. }]));
}
