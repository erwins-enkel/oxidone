//! The due editor's overlay as actually drawn: the selected prefill and the
//! preview line beneath it.
//!
//! Every assertion here that concerns *style* — the reversed prefill, the
//! unreversed cursor bar, the red on an unparsable buffer — reads
//! `buffer[(x, y)].style()`, never a row string. A row string is built from
//! `Cell::symbol()`, so it discards style entirely: a reversed prefill and an
//! unstyled one produce byte-identical text, and a "renders reversed" assertion
//! routed through one would pass against an implementation that never sets the
//! modifier at all. `modifier_over` here follows the copies in
//! `notes_render.rs` and `meters_render.rs`; the colour check follows
//! `type_render.rs`, which compares `.style()` directly because a foreground is
//! not a `Modifier`.

use chrono::{Local, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::api::{FakeTasksApi, NewTask, TasksApi};
use oxidone::app::{update, Message, Model};
use oxidone::domain::Selection;
use oxidone::ui::{self, theme::Theme};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;

const HEIGHT: u16 = 24;
const WIDTH: u16 = 80;

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn chord(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn typed(m: &mut Model, s: &str) {
    for c in s.chars() {
        update(m, ch(c));
    }
}

fn buffer_of(model: &Model) -> ratatui::buffer::Buffer {
    let mut terminal =
        Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend terminal");
    let theme = Theme::from_flavor("mocha");
    terminal
        .draw(|frame| ui::view(model, &theme, false, frame))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn rows(model: &Model) -> Vec<String> {
    let buffer = buffer_of(model);
    (0..HEIGHT)
        .map(|y| {
            (0..WIDTH)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

/// The row the overlay's popup title sits on. Every style assertion anchors to
/// this rather than scanning the frame: the Task's due date is also drawn in the
/// task pane's due column *behind* the popup, so a whole-frame search for
/// `2026-08-14` can land on the pane row and assert nothing about the overlay at
/// all. (It did, in an earlier draft: deleting the `REVERSED` modifier outright
/// left the test passing.)
fn overlay_title_row(model: &Model) -> u16 {
    rows(model)
        .iter()
        .position(|r| r.contains("Edit due date"))
        .expect("the due overlay is open") as u16
}

/// The column `text` starts at on row `y`.
fn column_of(model: &Model, y: u16, text: &str) -> Option<u16> {
    let row = &rows(model)[y as usize];
    row.find(text)
        .map(|byte| row[..byte].chars().count() as u16)
}

/// Whether every cell spanning `text` on row `y` carries `modifier`. `None` when
/// `text` is not on that row — distinguishing "drawn, unstyled" from "not
/// drawn", which a bare bool would collapse.
fn modifier_over(model: &Model, y: u16, text: &str, modifier: Modifier) -> Option<bool> {
    let x = column_of(model, y, text)?;
    let buffer = buffer_of(model);
    Some(
        (x..x + text.chars().count() as u16)
            .all(|col| buffer[(col, y)].style().add_modifier.contains(modifier)),
    )
}

/// One Task due 2026-08-14 (a Friday), with the task pane focused and the clock
/// pinned to 2026-07-22 (a Wednesday) — 23 days apart.
async fn model_with_due() -> Model {
    let api = FakeTasksApi::new();
    let l = api.insert_list("L").await.unwrap();
    api.insert_task(
        &l.id,
        NewTask {
            title: "alpha".to_string(),
            due: Some(chrono::NaiveDate::from_ymd_opt(2026, 8, 14).unwrap()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let tasks = api.list_tasks(&l.id, true, false, None).await.unwrap();
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(2026, 7, 22, 9, 0, 0)
        .single()
        .unwrap();
    update(&mut m, Message::ListsLoaded(vec![l.clone()]));
    m.selected = Selection::List(0);
    update(&mut m, Message::TasksLoaded(l.id.clone(), tasks));
    update(&mut m, key(KeyCode::Tab)); // focus task pane
    m
}

/// The same fixture with a second, undated Task ("beta") below the dated one.
async fn model_with_an_undated_task() -> Model {
    let mut m = model_with_due().await;
    let mut tasks = m.tasks.clone();
    let mut beta = tasks[0].clone();
    beta.id = oxidone::domain::TaskId("beta".into());
    beta.title = "beta".to_string();
    beta.due = None;
    tasks.push(beta);
    m.tasks = tasks;
    update(&mut m, key(KeyCode::Down)); // select "beta"
    m
}

/// Opening `d` on a Task that has no due date must not announce a clear: there
/// is nothing to clear, and `Enter` changes nothing visible. This is the same
/// objection that ruled out never prefilling at all — an editor that greets you
/// with a destructive-sounding outcome you did not ask for.
#[tokio::test]
async fn an_undated_task_is_not_told_its_date_will_be_cleared() {
    let mut m = model_with_an_undated_task().await;
    update(&mut m, ch('d'));
    let rows = rows(&m);
    assert!(
        rows.iter().any(|r| r.contains("→ leaves it undated")),
        "{rows:?}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("clears the due date")),
        "nothing to clear on an undated Task: {rows:?}"
    );
}

/// A no-op keystroke must not make the message scarier. `Backspace` and `^U` on
/// an already-empty buffer change nothing, but they do clear `pristine` — which
/// an earlier version used as a stand-in for "the Task had no date", so the line
/// flipped to threatening a clear. Backspace-out-of-habit on a just-opened
/// editor is the likely path.
#[tokio::test]
async fn emptying_an_already_empty_buffer_does_not_threaten_a_clear() {
    for press in [
        Message::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty())),
        chord('u'),
    ] {
        let mut m = model_with_an_undated_task().await;
        update(&mut m, ch('d'));
        update(&mut m, press);
        let rows = rows(&m);
        assert!(
            rows.iter().any(|r| r.contains("→ leaves it undated")),
            "{rows:?}"
        );
        assert!(
            !rows.iter().any(|r| r.contains("clears the due date")),
            "still nothing to clear: {rows:?}"
        );
    }
}

/// The converse: on a Task that *does* have a date, an empty buffer really will
/// clear it, whether the user emptied it deliberately or the editor just opened.
#[tokio::test]
async fn an_emptied_buffer_on_a_dated_task_still_announces_the_clear() {
    for press in [
        Message::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::empty())),
        chord('u'),
    ] {
        let mut m = model_with_due().await;
        update(&mut m, ch('d'));
        update(&mut m, press);
        let rows = rows(&m);
        assert!(
            rows.iter().any(|r| r.contains("→ clears the due date")),
            "{rows:?}"
        );
    }
}

#[tokio::test]
async fn the_untouched_prefill_is_drawn_selected() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    let input = overlay_title_row(&m) + 1;
    assert_eq!(
        modifier_over(&m, input, "2026-08-14", Modifier::REVERSED),
        Some(true),
        "the prefill is selected until the first keystroke"
    );
}

/// The cursor bar sits outside the reversed span: reversed it renders as a
/// filled block, reading as a second cursor or a stray cell of highlight past
/// the end of the selection.
#[tokio::test]
async fn the_cursor_bar_is_not_part_of_the_selection() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    let input = overlay_title_row(&m) + 1;
    assert_eq!(
        modifier_over(&m, input, "▏", Modifier::REVERSED),
        Some(false),
        "drawn, but not reversed — `None` here would mean it vanished"
    );
}

#[tokio::test]
async fn the_selection_is_gone_once_the_buffer_is_edited() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    typed(&mut m, "friday");
    let input = overlay_title_row(&m) + 1;
    assert_eq!(
        modifier_over(&m, input, "friday", Modifier::REVERSED),
        Some(false)
    );
}

/// The whole line, not a substring: a `contains("2026-08-14")` check would pass
/// against a preview that dropped the weekday or the distance entirely.
#[tokio::test]
async fn the_preview_names_the_weekday_the_date_and_the_distance() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    assert!(
        rows(&m)
            .iter()
            .any(|r| r.contains("→ Fri 2026-08-14 · in 23d")),
        "preview line missing or incomplete: {:?}",
        rows(&m)
    );
}

/// Past `RELATIVE_HORIZON_DAYS` the task pane's own formatter falls back to the
/// ISO date and drops the distance, because its column is width-capped. The
/// preview is not, and must keep both — 23 days out is well past that horizon,
/// so the case above already proves it; this pins the reverse direction.
#[tokio::test]
async fn a_past_date_reads_as_days_ago() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "2026-07-01");
    assert!(
        rows(&m)
            .iter()
            .any(|r| r.contains("→ Wed 2026-07-01 · 21d ago")),
        "{:?}",
        rows(&m)
    );
}

/// The words match `format_due_relative`'s where they overlap, so one date never
/// reads two ways with the pane visible behind the popup.
#[tokio::test]
async fn near_dates_use_the_same_words_as_the_task_pane() {
    for (typed_date, expected) in [
        ("2026-07-22", "→ Wed 2026-07-22 · today"),
        ("2026-07-23", "→ Thu 2026-07-23 · tomorrow"),
        ("2026-07-21", "→ Tue 2026-07-21 · yesterday"),
    ] {
        let mut m = model_with_due().await;
        update(&mut m, ch('d'));
        update(&mut m, chord('u'));
        typed(&mut m, typed_date);
        assert!(
            rows(&m).iter().any(|r| r.contains(expected)),
            "expected {expected:?} in {:?}",
            rows(&m)
        );
    }
}

#[tokio::test]
async fn an_empty_buffer_previews_the_clear() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    assert!(rows(&m).iter().any(|r| r.contains("→ clears the due date")));
}

/// A whitespace-only buffer is a *clear* to `finish_edit_due`, which trims before
/// testing for empty — so the preview must trim too, or it would render "not a
/// date" in red while `Enter` cleared the date.
#[tokio::test]
async fn a_whitespace_only_buffer_previews_the_clear_not_an_error() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "   ");
    let rows = rows(&m);
    assert!(
        rows.iter().any(|r| r.contains("→ clears the due date")),
        "{rows:?}"
    );
    assert!(!rows.iter().any(|r| r.contains("→ not a date")), "{rows:?}");
}

#[tokio::test]
async fn an_unparsable_buffer_previews_an_error_in_the_overdue_colour() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    update(&mut m, chord('u'));
    typed(&mut m, "zzz");
    assert!(rows(&m).iter().any(|r| r.contains("→ not a date")));

    let y = overlay_title_row(&m) + 2; // title, input, preview
    let x = column_of(&m, y, "→ not a date").expect("the error preview");
    let theme = Theme::from_flavor("mocha");
    assert_eq!(
        buffer_of(&m)[(x, y)].style().fg,
        Some(theme.overdue),
        "an unparsable buffer reads in the overdue colour, not the dim one"
    );
}

/// The preview follows the buffer, so a step re-renders it — weekday included.
#[tokio::test]
async fn stepping_updates_the_weekday() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    update(&mut m, key(KeyCode::Down));
    assert!(
        rows(&m)
            .iter()
            .any(|r| r.contains("→ Sat 2026-08-15 · in 24d")),
        "{:?}",
        rows(&m)
    );
}

/// The parenthetical is gone from the title: the preview says it live, when it
/// actually applies. The notes editor keeps its own, having no preview line.
#[tokio::test]
async fn the_title_does_not_restate_what_the_preview_says() {
    let mut m = model_with_due().await;
    update(&mut m, ch('d'));
    let rows = rows(&m);
    assert!(rows.iter().any(|r| r.contains("Edit due date")), "{rows:?}");
    assert!(
        !rows.iter().any(|r| r.contains("blank clears")),
        "the title must not repeat the preview's empty branch: {rows:?}"
    );
}
