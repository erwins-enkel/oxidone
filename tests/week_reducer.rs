//! Reducer tests for the Weekly spread (`W`). `update` is pure — no terminal, no
//! network. The window is stamped by `model.now`, which these tests fix to a
//! Monday, so membership, the day columns and `]`/`[` are all deterministic.

use chrono::{Local, NaiveDate, TimeZone};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Command, Focus, Message, Model};
use oxidone::domain::{List, ListId, Selection, SortView, Status, Task, TaskId};

fn press(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty()))
}

fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

/// A fixed "today": **Monday** 17 August 2026, so the displayed week is
/// 17–21 August and today is column 0.
const TODAY: (i32, u32, u32) = (2026, 8, 17);

fn today() -> NaiveDate {
    ymd(TODAY.0, TODAY.1, TODAY.2)
}

/// The `n`-th day of the current week, `0` being Monday.
fn day(n: i64) -> Option<NaiveDate> {
    Some(today() + chrono::Duration::days(n))
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.into()),
        title: id.to_uppercase(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn task(id: &str, list: &str, due: Option<NaiveDate>, status: Status) -> Task {
    Task {
        id: TaskId(id.into()),
        list: ListId(list.into()),
        parent: None,
        title: id.into(),
        notes: None,
        status,
        due,
        completed_at: None,
        links: Vec::new(),
        position: id.into(),
        etag: String::new(),
        updated: Local.timestamp_opt(0, 0).unwrap().to_utc(),
    }
}

fn open(id: &str, list: &str, due: Option<NaiveDate>) -> Task {
    task(id, list, due, Status::NeedsAction)
}

/// An open Task with its Manual-order key stated explicitly, for the one test
/// that asserts on it.
fn at(id: &str, list: &str, due: Option<NaiveDate>, position: &str) -> Task {
    Task {
        position: position.into(),
        ..open(id, list, due)
    }
}

/// A Model with `lists` known and the whole corpus loaded, sitting on List `w`
/// (so the pool is `w`) with the task pane focused. The spread is **not** open;
/// each test presses `W` itself, so what the key does is part of the assertion.
fn model(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = Model::new();
    m.now = Local
        .with_ymd_and_hms(TODAY.0, TODAY.1, TODAY.2, 12, 0, 0)
        .unwrap();
    m.lists = lists.iter().map(|id| list(id)).collect();
    m.selected = Selection::List(0);
    m.tasks = tasks;
    m.focus = Focus::Tasks;
    m
}

/// A Model already in the spread.
fn in_week(lists: &[&str], tasks: Vec<Task>) -> Model {
    let mut m = model(lists, tasks);
    let corpus = m.tasks.clone();
    update(&mut m, press('W'));
    // `W` clears the pane and asks for a load; hand the corpus straight back, as
    // `LoadWeek`'s reply would.
    update(
        &mut m,
        Message::WeekLoaded {
            tasks: corpus,
            failed: Vec::new(),
            live: true,
        },
    );
    m.focus = Focus::Tasks;
    m
}

fn visible(m: &Model) -> Vec<String> {
    m.visible_tasks()
        .iter()
        .map(|t| t.display_title().to_string())
        .collect()
}

fn select(m: &mut Model, title: &str) {
    let at = m
        .tasks
        .iter()
        .position(|t| t.display_title() == title)
        .unwrap_or_else(|| panic!("{title} is not in the corpus"));
    m.selected_task = Some(at);
}

fn due_of(m: &Model, title: &str) -> Option<NaiveDate> {
    m.tasks
        .iter()
        .find(|t| t.display_title() == title)
        .expect("task is in the corpus")
        .due
}

// --- Entering, and the Today interaction ----------------------------------

/// `Model::new` opens on Today, so the very first `W` a user can press is from
/// there. Without `today_active()`'s `!week` gate both lenses would be live:
/// `within_today` would drop the whole undated pool *and* every row due after
/// today, and the pane would still be claimed by the journal spread.
#[test]
fn w_from_the_opening_state_shows_the_pool_and_the_whole_week() {
    let mut m = model(
        &["w"],
        vec![
            open("pool", "w", None),
            open("friday", "w", day(4)),
            open("monday", "w", day(0)),
        ],
    );
    m.selected = Selection::Today;
    m.default_list = Some(ListId("w".into()));
    let corpus = m.tasks.clone();
    update(&mut m, press('W'));
    update(
        &mut m,
        Message::WeekLoaded {
            tasks: corpus,
            failed: Vec::new(),
            live: true,
        },
    );

    assert!(m.week_active());
    assert!(
        !m.today_active(),
        "the Today rules must be disarmed, or the spread is a Today aggregate"
    );
    assert_eq!(visible(&m), ["pool", "monday", "friday"]);
}

/// Entering leaves the sidebar cursor alone — that is what lets it keep naming
/// the pool List — and leaving returns to whatever pane it still names.
#[test]
fn the_lens_toggles_without_moving_the_sidebar_cursor() {
    let mut m = in_week(&["w", "h"], vec![open("a", "w", None)]);
    assert_eq!(m.selected, Selection::List(0));

    let commands = update(&mut m, press('W'));
    assert!(!m.week_active());
    assert_eq!(m.selected, Selection::List(0));
    assert_eq!(commands, vec![Command::LoadTasks(ListId("w".into()))]);
}

/// The pane must never read "nothing planned this week" while a List it has not
/// mirrored is still in flight. The cache paint alone does not clear the notice.
#[test]
fn the_pending_notice_survives_the_cache_paint_and_clears_on_the_live_reply() {
    let mut m = model(&["w"], vec![open("a", "w", day(0))]);
    let corpus = m.tasks.clone();

    let commands = update(&mut m, press('W'));
    assert_eq!(
        commands,
        vec![Command::LoadWeek {
            lists: m.lists.clone()
        }]
    );
    assert!(m.week_pending);

    update(
        &mut m,
        Message::WeekLoaded {
            tasks: corpus.clone(),
            failed: Vec::new(),
            live: false,
        },
    );
    assert!(m.week_pending, "the cache paint is not the whole corpus");

    update(
        &mut m,
        Message::WeekLoaded {
            tasks: corpus,
            failed: Vec::new(),
            live: true,
        },
    );
    assert!(!m.week_pending);
}

// --- Membership -----------------------------------------------------------

/// The pool is one List's undated rows; the scheduled half spans every List.
#[test]
fn the_pool_is_one_list_and_the_week_is_all_of_them() {
    let m = in_week(
        &["w", "h"],
        vec![
            open("pool-w", "w", None),
            open("pool-h", "h", None),
            open("week-w", "w", day(1)),
            open("week-h", "h", day(2)),
        ],
    );
    assert_eq!(visible(&m), ["pool-w", "week-w", "week-h"]);
}

/// Moving the sidebar cursor re-scopes the pool **without** leaving the spread,
/// and without a fetch: the corpus already spans every List.
#[test]
fn a_sidebar_move_rescopes_the_pool_and_stays_in_the_spread() {
    let mut m = in_week(
        &["w", "h"],
        vec![open("pool-w", "w", None), open("pool-h", "h", None)],
    );
    m.focus = Focus::Sidebar;

    let commands = update(&mut m, press('j'));

    assert!(m.week_active(), "a sidebar move is not a pane change here");
    assert_eq!(m.selected, Selection::List(1));
    assert!(commands.is_empty(), "the corpus already covers every List");
    assert_eq!(visible(&m), ["pool-h"]);
}

/// Saturday, Sunday and anything before Monday are outside the window — the
/// spread plans weekdays, and overdue is Today's job.
#[test]
fn the_window_is_monday_to_friday_and_nothing_else() {
    let m = in_week(
        &["w"],
        vec![
            open("sunday-before", "w", day(-1)),
            open("monday", "w", day(0)),
            open("friday", "w", day(4)),
            open("saturday", "w", day(5)),
            open("sunday-after", "w", day(6)),
            open("next-monday", "w", day(7)),
        ],
    );
    assert_eq!(visible(&m), ["monday", "friday"]);
}

/// `sync` fetches with `show_completed: true`, so a mature List carries every
/// completed-but-uncleared undated Task. The pool has no date window to age them
/// out, so its own status clause must — while a completed row *in* the week
/// keeps its cell, which is the whole point of the `✕`.
#[test]
fn a_completed_row_is_kept_in_the_week_and_kept_out_of_the_pool() {
    let m = in_week(
        &["w"],
        vec![
            task("old-pool", "w", None, Status::Completed),
            open("live-pool", "w", None),
            task("done-tue", "w", day(1), Status::Completed),
        ],
    );
    assert!(!m.show_completed, "the default is what makes this a test");
    assert_eq!(visible(&m), ["live-pool", "done-tue"]);
}

/// The grid reaches eleven days out, so any shorter horizon would silently drop
/// planned rows — an empty week reading as nothing planned.
#[test]
fn a_short_horizon_does_not_hide_a_planned_row() {
    let mut m = in_week(&["w"], vec![open("next-fri", "w", day(11))]);
    m.hide_distant = true;
    m.horizon_days = 3;

    update(&mut m, press(']'));
    assert_eq!(visible(&m), ["next-fri"]);
}

// --- Ordering -------------------------------------------------------------

/// The pool leads in its List's Manual order; the week follows by day. Both
/// blocks are contiguous, which is what lets the renderer count the pool as a
/// prefix instead of partitioning.
#[test]
fn the_pool_leads_in_manual_order_and_the_week_follows_by_day() {
    // Manual order is `position`, stated here rather than left to fall out of the
    // ids: `second` is hand-ordered *after* `first` while sorting above it by
    // every other key, so only the intended order passes.
    let mut m = in_week(
        &["w"],
        vec![
            at("thu", "w", day(3), "c"),
            at("second", "w", None, "b"),
            at("mon", "w", day(0), "d"),
            at("first", "w", None, "a"),
        ],
    );
    assert_eq!(visible(&m), ["first", "second", "mon", "thu"]);

    // And the fixed order ignores the lens entirely.
    m.sort = SortView::Title;
    assert_eq!(visible(&m), ["first", "second", "mon", "thu"]);
}

// --- The day cursor -------------------------------------------------------

/// The cursor opens at home, walks right to Friday and clamps, then walks back
/// to home and *falls through* to the sidebar — so `h` never stops meaning
/// "left, eventually out of the pane".
#[test]
fn the_day_cursor_walks_the_grid_and_falls_through_to_the_sidebar() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    assert_eq!(m.week_day, None, "the pane opens at home");

    for expected in 0..5 {
        update(&mut m, press('l'));
        assert_eq!(m.week_day, Some(expected));
    }
    update(&mut m, press('l'));
    assert_eq!(m.week_day, Some(4), "clamped at Friday");
    assert_eq!(m.focus, Focus::Tasks, "and never focuses away mid-plan");

    for expected in (0..4).rev() {
        update(&mut m, press('h'));
        assert_eq!(m.week_day, Some(expected));
    }
    update(&mut m, press('h'));
    assert_eq!(m.week_day, None, "back to home");
    assert_eq!(m.focus, Focus::Tasks);

    update(&mut m, press('h'));
    assert_eq!(m.focus, Focus::Sidebar, "home falls through to the sidebar");
}

/// With the sidebar focused, `h`/`l` are focus keys again: the grid's rebinding
/// is scoped to the pane it belongs to.
#[test]
fn the_grid_keys_do_not_capture_h_and_l_from_the_sidebar() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    m.focus = Focus::Sidebar;

    update(&mut m, press('l'));
    assert_eq!(m.focus, Focus::Tasks);
    assert_eq!(m.week_day, None, "the sidebar never moved the day cursor");
}

// --- Space, the digits, and unscheduling ----------------------------------

/// The four cases of one rule: Space acts on the cell under the cursor. Asserted
/// with `show_completed` at its `false` default, since that is what would take
/// the row off screen before the third and fourth cases could be reached.
#[test]
fn space_acts_on_the_cell_under_the_cursor() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    select(&mut m, "a");
    update(&mut m, press('l'));
    update(&mut m, press('l'));
    update(&mut m, press('l')); // Wednesday

    // Empty cell, unscheduled row: schedule here.
    let commands = update(&mut m, Message::Key(space()));
    assert_eq!(due_of(&m, "a"), day(2));
    assert!(matches!(commands.as_slice(), [Command::SetDue { .. }]));

    // The row's own dot: complete it.
    m.pending_writes_cleared();
    update(&mut m, Message::Key(space()));
    assert_eq!(status_of(&m, "a"), Status::Completed);
    assert_eq!(due_of(&m, "a"), day(2), "completing does not move the dot");
    assert_eq!(visible(&m), ["a"], "and the ✕ row stays on screen");

    // The ✕: un-complete it.
    m.pending_writes_cleared();
    update(&mut m, Message::Key(space()));
    assert_eq!(status_of(&m, "a"), Status::NeedsAction);
}

/// Completing a **scheduled** row leaves it on screen with its `✕`, so the cursor
/// must stay on it — `Space` is documented to un-complete the cell under the
/// cursor, and the row under the cursor has not moved.
///
/// Two rows, deliberately: with one, `display_successor` answers `None` and a
/// cursor that should have moved stays put by accident.
#[test]
fn completing_a_scheduled_row_leaves_the_cursor_on_it() {
    let mut m = in_week(&["w"], vec![open("a", "w", day(2)), open("b", "w", day(2))]);
    select(&mut m, "a");
    assert!(!m.show_completed, "the default is what makes this a test");

    update(&mut m, Message::Key(space()));
    assert_eq!(status_of(&m, "a"), Status::Completed);
    assert_eq!(visible(&m), ["a", "b"], "the ✕ row is still drawn");
    assert_eq!(
        selected_title(&m),
        Some("a".to_string()),
        "the cursor must not walk to the neighbour"
    );

    // And the documented contract holds: the next Space un-completes *this* row.
    m.pending_writes_cleared();
    update(&mut m, Message::Key(space()));
    assert_eq!(status_of(&m, "a"), Status::NeedsAction);
    assert_eq!(status_of(&m, "b"), Status::NeedsAction);
}

/// The sharp end of the same bug: had the cursor slipped to a neighbour due on
/// another day, the next `Space` would have taken the schedule branch and
/// silently re-dated a row the user never selected.
#[test]
fn completing_a_scheduled_row_never_redates_its_neighbour() {
    let mut m = in_week(
        &["w"],
        vec![open("mon", "w", day(0)), open("thu", "w", day(3))],
    );
    select(&mut m, "mon");
    update(&mut m, press('l')); // cursor on Monday, where `mon` sits

    update(&mut m, Message::Key(space()));
    m.pending_writes_cleared();
    let commands = update(&mut m, Message::Key(space()));

    assert_eq!(due_of(&m, "thu"), day(3), "the neighbour keeps its day");
    assert!(
        !commands
            .iter()
            .any(|c| matches!(c, Command::SetDue { task, .. } if task == &TaskId("thu".into()))),
        "no write against an unselected row: {commands:?}"
    );
}

/// The counterpart, and why a blanket "never move in the spread" would be wrong:
/// an **undated pool** row completed *does* leave, since the pool half admits
/// only `needsAction`. The cursor follows it out.
#[test]
fn completing_a_pool_row_moves_the_cursor_because_the_row_leaves() {
    let mut m = in_week(&["w"], vec![open("a", "w", None), open("b", "w", None)]);
    select(&mut m, "a");

    update(&mut m, Message::Key(space()));

    assert_eq!(visible(&m), ["b"], "the completed pool row is gone");
    assert_eq!(selected_title(&m), Some("b".to_string()));
}

/// A cell that is not the row's own dot moves the dot there, wherever it was.
#[test]
fn space_on_another_day_moves_the_dot() {
    let mut m = in_week(&["w"], vec![open("a", "w", day(2))]);
    select(&mut m, "a");
    update(&mut m, press('l')); // Monday

    update(&mut m, Message::Key(space()));
    assert_eq!(due_of(&m, "a"), day(0));
}

/// At home, Space is the completion key it is in every other pane — which is
/// what lets a pool row (no dot cell anywhere) be completed at all.
#[test]
fn space_at_home_completes_a_pool_row() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    select(&mut m, "a");
    assert_eq!(m.week_day, None);

    update(&mut m, Message::Key(space()));
    assert_eq!(status_of(&m, "a"), Status::Completed);
    assert_eq!(due_of(&m, "a"), None, "completing never schedules");
}

/// `1`–`5` jump-assign past the cursor; `0` returns the row to the pool.
#[test]
fn the_digits_assign_a_day_and_zero_unschedules() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    select(&mut m, "a");

    for (digit, offset) in [('1', 0), ('3', 2), ('5', 4)] {
        m.pending_writes_cleared();
        update(&mut m, press(digit));
        assert_eq!(due_of(&m, "a"), day(offset), "{digit}");
    }

    m.pending_writes_cleared();
    update(&mut m, press('0'));
    assert_eq!(due_of(&m, "a"), None);
}

/// `6`..`9` name no column, so they fall through rather than scheduling a
/// Saturday the grid cannot draw.
#[test]
fn a_digit_past_friday_is_not_a_day() {
    let mut m = in_week(&["w"], vec![open("a", "w", None)]);
    select(&mut m, "a");

    update(&mut m, press('6'));
    assert_eq!(due_of(&m, "a"), None);
}

/// Unscheduling a Completed row would drop it from the spread entirely — the
/// pool admits only `needsAction`, and it would have no date left to be in the
/// week by. Refused rather than vanished.
#[test]
fn a_completed_row_cannot_be_unscheduled_out_of_sight() {
    let mut m = in_week(&["w"], vec![task("a", "w", day(1), Status::Completed)]);
    select(&mut m, "a");

    let commands = update(&mut m, press('0'));
    assert!(commands.is_empty());
    assert_eq!(due_of(&m, "a"), day(1));
    assert!(m.status_line.is_some(), "and it says why");
}

/// Closing the spread forgets which week was on screen, as it forgets the day
/// cursor: `W` always opens on the week you are in, and `]` is one keystroke
/// away. Left set, the spread would reopen on next week with only the panel
/// title saying so.
#[test]
fn reopening_the_spread_lands_on_the_current_week() {
    let mut m = in_week(&["w"], vec![open("next", "w", day(9))]);
    update(&mut m, press(']'));
    assert_eq!(visible(&m), ["next"]);

    update(&mut m, press('W'));
    update(&mut m, press('W'));
    assert_eq!(m.week_offset, 0);
    assert!(visible(&m).is_empty(), "next week's row is not in this one");
}

/// `]` shows next week, `[` returns — re-windowing the corpus already loaded,
/// with no fetch.
#[test]
fn the_week_pages_forward_and_back_without_a_fetch() {
    let mut m = in_week(
        &["w"],
        vec![open("this", "w", day(2)), open("next", "w", day(9))],
    );

    let commands = update(&mut m, press(']'));
    assert!(commands.is_empty());
    assert_eq!(visible(&m), ["next"]);

    update(&mut m, press('['));
    assert_eq!(visible(&m), ["this"]);
}

/// A JUMP names a pane, so it *leaves* the spread — unlike the sidebar's `j`/`k`,
/// which re-scopes the pool in place. Jumping to Today while in the spread and
/// landing on a week grid would answer a different question than the one asked.
#[test]
fn an_omnibox_jump_leaves_the_spread() {
    let mut m = in_week(&["w", "h"], vec![open("a", "w", None)]);

    update(&mut m, press('p'));
    for c in "H".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(enter()));

    assert!(!m.week_active(), "a jump names the pane to land in");
    assert!(!m.week_pending, "and takes its pending notice with it");
    assert_eq!(m.selected, Selection::List(1));
}

/// A failed corpus read must take the pending notice with it, or the pane
/// promises work that will never arrive — the same rule Search's notice follows.
#[test]
fn a_failed_load_clears_the_pending_notice() {
    let mut m = model(&["w"], Vec::new());
    update(&mut m, press('W'));
    assert!(m.week_pending);

    update(&mut m, Message::LoadFailed("disk on fire".to_string()));

    assert!(!m.week_pending);
    assert_eq!(m.status_line.as_deref(), Some("disk on fire"));
}

// --- Capture --------------------------------------------------------------

/// `a` captures **undated** into the pool List — the brain-dump half of the
/// ritual. Undated even from Today, whose own branch would have dated it today
/// and dropped it straight into the grid.
#[test]
fn a_captures_undated_into_the_pool_list() {
    let mut m = in_week(&["w", "h"], Vec::new());
    m.selected = Selection::List(1);

    update(&mut m, press('a'));
    for c in "venue".chars() {
        update(&mut m, press(c));
    }
    let commands = update(&mut m, Message::Key(enter()));

    match commands.as_slice() {
        [Command::AddTask { list, due, .. }] => {
            assert_eq!(list, &ListId("h".into()), "the pool List, not the default");
            assert_eq!(*due, None, "undated, so it lands in UNSCHEDULED");
        }
        other => panic!("expected one AddTask, got {other:?}"),
    }
}

/// On Today the pool falls back to `default_list`; with neither resolvable the
/// capture is refused **loudly**, not dropped the way a stale List index is.
#[test]
fn a_refuses_loudly_when_no_pool_list_resolves() {
    let mut m = in_week(&["w"], Vec::new());
    m.selected = Selection::Today;
    m.default_list = None;

    update(&mut m, press('a'));
    assert!(m.overlay.is_none(), "no capture overlay to type into");
    assert!(m.status_line.is_some(), "and it says why");
}

/// `A` names a pane to land in, so it leaves the spread — unlike a sidebar
/// `j`/`k`, which re-scopes the pool in place. Left in, `finish_add_list`'s
/// `model.tasks.clear()` would strand the spread on an empty corpus: nothing
/// reloads it (`ListInserted` is gated on `selected_list_id()`, `None` here) and
/// `week_pending` is `false`, so the pane would read as a week with nothing
/// planned and no notice saying otherwise.
#[test]
fn adding_a_list_leaves_the_spread_rather_than_stranding_an_empty_corpus() {
    let mut m = in_week(&["w"], vec![open("ship", "w", day(2))]);
    m.focus = Focus::Sidebar;

    update(&mut m, press('A'));
    for c in "Home".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(enter()));

    assert!(!m.week_active(), "A names the new List's pane");
    assert!(!m.week_pending);
}

/// A `ListsLoaded` must not read as a target change while the spread is up, or
/// `request_selected`'s `clear_pane` prologue drops the `/` filter under the
/// user: `W`, a query, then `r`.
#[test]
fn a_list_set_refresh_keeps_the_filter_in_the_spread() {
    let mut m = in_week(
        &["w"],
        vec![
            open("ship the release", "w", day(2)),
            open("call", "w", day(3)),
        ],
    );
    update(&mut m, press('/'));
    for c in "ship".chars() {
        update(&mut m, press(c));
    }
    update(&mut m, Message::Key(enter()));
    assert_eq!(m.filter.as_deref(), Some("ship"));
    assert_eq!(visible(&m), ["ship the release"]);

    // What `r` delivers: the List set, unchanged.
    let lists = m.lists.clone();
    update(&mut m, Message::ListsLoaded(lists));

    assert_eq!(m.filter.as_deref(), Some("ship"), "the filter survives `r`");
    assert!(m.week_active());
}

// --- Refusals -------------------------------------------------------------

/// Each of these guards tested `today_active()`, which the `!week` gate now
/// answers `false` for — so each is asserted with a **cross-List** row selected
/// as well as a pool one. That is the bug being closed: the guard passing while
/// `selected_list_id()` still named the parked pool List.
#[test]
fn the_pane_refuses_every_per_list_verb_on_a_cross_list_row() {
    for row in ["pool", "foreign"] {
        let mut m = in_week(
            &["w", "h"],
            vec![open("pool", "w", None), open("foreign", "h", day(1))],
        );
        select(&mut m, row);

        for key in ['o', 'C', 'J', 'K', '>', '<'] {
            m.status_line = None;
            let commands = update(&mut m, press(key));
            assert!(commands.is_empty(), "{key} on {row} emitted {commands:?}");
            assert!(m.status_line.is_some(), "{key} on {row} refused silently");
            assert!(m.overlay.is_none(), "{key} on {row} opened an overlay");
        }
    }
}

/// The three view filters are exempted at their own predicates, so a working key
/// would change a flag invisibly here and hand it to the List pane on return.
/// Refused, and the flag is not flipped.
#[test]
fn the_view_filters_are_refused_without_flipping_their_flags() {
    let mut m = in_week(&["w"], vec![open("a", "w", day(0))]);
    let (sort, completed, distant) = (m.sort, m.show_completed, m.hide_distant);

    for key in ['s', 'c', 'w'] {
        m.status_line = None;
        update(&mut m, press(key));
        assert!(m.status_line.is_some(), "{key} refused silently");
    }
    assert_eq!(
        (m.sort, m.show_completed, m.hide_distant),
        (sort, completed, distant)
    );
}

// --- Helpers that need Model internals ------------------------------------

fn space() -> KeyEvent {
    KeyEvent::new(KeyCode::Char(' '), KeyModifiers::empty())
}

fn enter() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())
}

fn selected_title(m: &Model) -> Option<String> {
    m.selected_task
        .and_then(|i| m.tasks.get(i))
        .map(|t| t.display_title().to_string())
}

fn status_of(m: &Model, title: &str) -> Status {
    m.tasks
        .iter()
        .find(|t| t.display_title() == title)
        .expect("task is in the corpus")
        .status
}

/// Clear the single-flight guard between presses. The tests drive several writes
/// against one row without a server reply in between, which the guard would
/// otherwise (correctly) refuse.
trait ClearPending {
    fn pending_writes_cleared(&mut self);
}

impl ClearPending for Model {
    fn pending_writes_cleared(&mut self) {
        let ids: Vec<TaskId> = self.tasks.iter().map(|t| t.id.clone()).collect();
        for id in ids {
            update(
                self,
                Message::TaskUpdated(
                    self.tasks
                        .iter()
                        .find(|t| t.id == id)
                        .expect("present")
                        .clone(),
                ),
            );
        }
    }
}
