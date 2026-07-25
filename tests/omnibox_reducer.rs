//! Reducer tests for the **Omnibox** (`p`).
//!
//! `omnibox_rows` is a pure function of `(&Model, &str)`, so group membership and
//! order, the COMMAND split, prefix matching, command validity and the advisory
//! are all assertable with no terminal and no `Overlay`. The key handling is
//! driven through `update` with `Message::Key`, as `tests/search_reducer.rs` does.

use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{
    omnibox_rows, update, CaptureRow, CommandState, Focus, Group, JumpTarget, Message, Model,
    OmniCommand, OmniRow, Overlay,
};
use oxidone::config::Flavor;
use oxidone::domain::{List, ListId, Selection};

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn chord(c: char) -> Message {
    Message::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

fn list(id: &str) -> List {
    List {
        id: ListId(id.to_string()),
        title: id.to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

/// A model holding `titles` as Lists, with the Omnibox open on an empty query.
fn open_with(titles: &[&str]) -> Model {
    let mut model = Model::new();
    update(
        &mut model,
        Message::ListsLoaded(titles.iter().map(|t| list(t)).collect()),
    );
    update(&mut model, ch('p'));
    model
}

fn overlay(model: &Model) -> (&str, usize) {
    match &model.overlay {
        Some(Overlay::Omnibox { query, selected }) => (query.as_str(), *selected),
        other => panic!("expected an open Omnibox, got {other:?}"),
    }
}

fn query(model: &Model) -> String {
    overlay(model).0.to_string()
}

fn selected(model: &Model) -> usize {
    overlay(model).1
}

fn rows(model: &Model) -> Vec<OmniRow> {
    omnibox_rows(model, &query(model))
}

fn groups(model: &Model) -> Vec<Group> {
    rows(model).iter().map(OmniRow::group).collect()
}

/// The COMMAND rows for `q`, in row order.
fn commands(model: &Model, q: &str) -> Vec<(OmniCommand, CommandState, Option<&'static str>)> {
    omnibox_rows(model, q)
        .into_iter()
        .filter_map(|row| match row {
            OmniRow::Command(c) => Some((c.command, c.state, c.advisory)),
            _ => None,
        })
        .collect()
}

// ─── opening ────────────────────────────────────────────────────────────────

/// `p` opens it, and `Ctrl-P` does too — free, because `keymap::resolve` is
/// modifier-blind. The `Out of scope` reasoning leans on that, and #105 is
/// exactly the change that would retire it silently.
#[test]
fn ctrl_p_opens_the_omnibox_like_p() {
    use oxidone::keymap::{resolve, Action};
    assert_eq!(
        resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)),
        Some(Action::Omnibox)
    );
    assert_eq!(
        resolve(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::empty())),
        Some(Action::Omnibox)
    );
}

/// Opening sets the overlay and nothing else. Asserted from the **sidebar**:
/// from `Focus::Tasks` it would pass even if the open had stolen focus.
#[test]
fn opening_touches_neither_focus_nor_the_status_line() {
    let mut model = Model::new();
    model.focus = Focus::Sidebar;
    model.status_line = Some("kept".into());
    update(&mut model, ch('p'));

    assert_eq!(model.focus, Focus::Sidebar);
    assert_eq!(model.status_line.as_deref(), Some("kept"));
    assert_eq!(overlay(&model), ("", 0));
}

#[test]
fn esc_closes_and_mutates_nothing() {
    let mut model = open_with(&["work"]);
    model.focus = Focus::Sidebar;
    let before = (model.selected, model.sort, model.filter.clone());

    update(&mut model, key(KeyCode::Esc));

    assert!(model.overlay.is_none());
    assert_eq!(model.focus, Focus::Sidebar);
    assert_eq!((model.selected, model.sort, model.filter.clone()), before);
}

// ─── rows and order ─────────────────────────────────────────────────────────

/// The empty query lists Today, then the Lists in order, then the three
/// commands — and **no** SEARCH row. So `selected == 0` names Today, and
/// `p`+`Enter` on an untouched Omnibox goes there.
#[test]
fn the_empty_query_lists_today_then_lists_then_commands() {
    let model = open_with(&["work", "home"]);

    assert_eq!(
        rows(&model)
            .into_iter()
            .map(|row| match row {
                OmniRow::Jump(JumpTarget::Today) => "Today".to_string(),
                OmniRow::Jump(JumpTarget::List { title, .. }) => title,
                OmniRow::Command(c) => format!(":{}", c.command.verb()),
                OmniRow::Search { .. } => "SEARCH".to_string(),
                OmniRow::Capture(_) => "CAPTURE".to_string(),
            })
            .collect::<Vec<_>>(),
        ["Today", "work", "home", ":horizon", ":flavor", ":ascii"]
    );
    assert_eq!(selected(&model), 0);
}

/// A whitespace-only query behaves **exactly** as the empty one: matching runs
/// on the trimmed query, so JUMP is full and row 0 is still Today.
#[test]
fn a_whitespace_only_query_behaves_as_the_empty_one() {
    let model = open_with(&["work"]);
    assert_eq!(omnibox_rows(&model, "   "), omnibox_rows(&model, ""));
}

/// Row 0 is the first row of the first non-empty group — the whole
/// legibility promise of `selected == 0`.
#[test]
fn row_zero_is_the_first_row_of_the_first_non_empty_group() {
    let model = open_with(&["work"]);

    assert_eq!(groups(&model)[0], Group::Jump);
    assert!(matches!(
        omnibox_rows(&model, "tod")[0],
        OmniRow::Jump(JumpTarget::Today)
    ));
    assert!(matches!(
        omnibox_rows(&model, "work")[0],
        OmniRow::Jump(JumpTarget::List { .. })
    ));
    // Matches no List and no verb prefix, so JUMP and COMMAND are both empty.
    assert!(matches!(
        omnibox_rows(&model, "zon 30")[0],
        OmniRow::Search { .. }
    ));
}

/// Today is filtered like any other JUMP row. Pinning it would make it row 0 on
/// *every* query, shadowing the row the user meant.
#[test]
fn today_is_filtered_like_any_other_jump_row() {
    let model = open_with(&["work"]);
    assert!(!omnibox_rows(&model, "work")
        .iter()
        .any(|r| matches!(r, OmniRow::Jump(JumpTarget::Today))));
}

/// Matching runs on the trimmed query, so a leading space does not empty JUMP.
#[test]
fn a_leading_space_still_matches_a_list() {
    let model = open_with(&["work"]);
    assert_eq!(omnibox_rows(&model, " work"), omnibox_rows(&model, "work"));
}

// ─── the COMMAND split ──────────────────────────────────────────────────────

/// Prefix, not substring: `zon` fires nothing, and `a` names `:ascii` alone.
/// Substring would put an invalid `:flavor a` row at `selected == 0`.
#[test]
fn the_verb_matches_by_prefix_not_substring() {
    let model = open_with(&[]);

    assert!(commands(&model, "zon 30").is_empty());
    assert_eq!(
        commands(&model, "a on")
            .iter()
            .map(|(c, ..)| *c)
            .collect::<Vec<_>>(),
        [OmniCommand::Ascii]
    );
}

/// The leading trim: on the raw query `" hor"` splits to an empty verb, which
/// prefix-matches all three commands and lands three `Invalid` rows at
/// `selected == 0`.
#[test]
fn a_leading_space_leaves_one_command_row_not_three() {
    let model = open_with(&[]);
    let rows = commands(&model, " hor");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, OmniCommand::Horizon);
    assert!(matches!(rows[0].1, CommandState::NeedsArgument { .. }));
}

/// Whitespace never breaks a valid argument, and never makes one appear.
#[test]
fn the_argument_is_trimmed_before_it_is_parsed() {
    let model = open_with(&[]);

    for q in ["horizon 30", "horizon  30", ":horizon 30"] {
        assert!(
            matches!(commands(&model, q)[0].1, CommandState::Valid { .. }),
            "{q:?} should parse"
        );
    }
    assert!(matches!(
        commands(&model, "ascii on ")[0].1,
        CommandState::Valid { .. }
    ));
    for q in ["horizon", "horizon ", "horizon   "] {
        assert!(
            matches!(commands(&model, q)[0].1, CommandState::NeedsArgument { .. }),
            "{q:?} should need an argument"
        );
    }
}

/// Completion is additive in content: the target is built, so a typed `:`
/// survives and only leading whitespace is normalised away. `None` exactly when
/// the target equals the query — a bare `horizon` still gains its space.
#[test]
fn completion_targets_are_built_not_appended() {
    let model = open_with(&[]);
    let completion = |q: &str| match &commands(&model, q)[0].1 {
        CommandState::NeedsArgument { completion } => completion.clone(),
        other => panic!("{q:?} is {other:?}"),
    };

    assert_eq!(completion("hor").as_deref(), Some("horizon "));
    assert_eq!(completion(":hor").as_deref(), Some(":horizon "));
    assert_eq!(completion(" hor").as_deref(), Some("horizon "));
    assert_eq!(completion("horizon").as_deref(), Some("horizon "));
    assert_eq!(completion(":horizon").as_deref(), Some(":horizon "));
    // Already its own target: nothing to add, which is what makes Enter inert.
    assert_eq!(completion("horizon "), None);
}

// ─── command validity ───────────────────────────────────────────────────────

#[test]
fn arguments_parse_case_insensitively() {
    let model = open_with(&[]);
    for q in ["flavor Latte", "flavor LATTE", "ascii ON", "ascii Off"] {
        assert!(
            matches!(commands(&model, q)[0].1, CommandState::Valid { .. }),
            "{q:?} should parse"
        );
    }
}

#[test]
fn an_invalid_argument_states_its_reason() {
    let model = open_with(&[]);
    let reason = |q: &str| match &commands(&model, q)[0].1 {
        CommandState::Invalid { reason } => reason.clone(),
        other => panic!("{q:?} is {other:?}"),
    };

    assert!(reason("flavor purple").contains("latte"));
    assert_eq!(reason("horizon abc"), "0–65535");
    // Beyond a `u16`, so the range is the whole of what the field can hold.
    assert_eq!(reason("horizon 70000"), "0–65535");
    // The floor is legal, not an error.
    assert!(matches!(
        commands(&model, "horizon 0")[0].1,
        CommandState::Valid { .. }
    ));
}

/// `:horizon` is refused in Search exactly as `w` is — the argument is *valid*,
/// so this is not the `Invalid` path.
#[test]
fn horizon_is_refused_in_search() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::List(0);
    update(&mut model, ch('S'));

    assert!(matches!(
        commands(&model, "horizon 30")[0].1,
        CommandState::RefusedHere { .. }
    ));
    // The other two are visible immediately in Search, so neither refuses.
    assert!(matches!(
        commands(&model, "flavor latte")[0].1,
        CommandState::Valid { .. }
    ));
}

/// The advisory names the command as well as the pane. Built from
/// `hide_distant`/`search_active()` alone it would attach to `:flavor` and
/// `:ascii` too — telling the user `w` applies a palette change.
#[test]
fn the_advisory_is_horizons_alone_and_absent_in_search() {
    let mut model = open_with(&["work"]);
    model.hide_distant = false;

    // Outside Search, filter off: every `:horizon` state carries it…
    for q in ["horizon", "horizon ", "horizon 30", "horizon abc"] {
        assert!(
            commands(&model, q)[0].2.is_some(),
            "{q:?} should carry the advisory"
        );
    }
    // …and no other command does, in that same state.
    for (command, _, advisory) in commands(&model, "") {
        assert_eq!(
            advisory.is_some(),
            command == OmniCommand::Horizon,
            "{command:?}"
        );
    }

    // With the filter on there is nothing to advise.
    model.hide_distant = true;
    assert!(commands(&model, "horizon 30")[0].2.is_none());

    // And in Search it is absent whatever the flag — `within_horizon` returns
    // early there, so "`w` applies it" would be false, and `w` is itself refused.
    model.hide_distant = false;
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::List(0);
    update(&mut model, ch('S'));
    for q in ["horizon", "horizon 30"] {
        assert!(commands(&model, q)[0].2.is_none(), "{q:?} in Search");
    }
}

/// The effect reads `old → new` for all three, so a row says what changes
/// without a per-command sentence.
#[test]
fn every_valid_effect_reads_old_to_new() {
    let mut model = open_with(&[]);
    model.horizon_days = 14;
    model.flavor = Flavor::Mocha;
    model.ascii = false;

    let effect = |q: &str| match &commands(&model, q)[0].1 {
        CommandState::Valid { effect } => effect.clone(),
        other => panic!("{q:?} is {other:?}"),
    };
    assert_eq!(effect("horizon 30"), "14 → 30");
    assert_eq!(effect("flavor latte"), "mocha → latte");
    assert_eq!(effect("ascii on"), "off → on");
}

// ─── keys ───────────────────────────────────────────────────────────────────

/// `j` and `k` **type**, as in every other overlay with a buffer. `picker_key`
/// moves on them only because `OpenLink`/`MoveToList` have none.
#[test]
fn j_and_k_type_into_the_query() {
    let mut model = open_with(&["work"]);
    update(&mut model, ch('j'));
    update(&mut model, ch('k'));

    assert_eq!(query(&model), "jk");
    assert_eq!(selected(&model), 0);
}

#[test]
fn up_and_down_move_and_clamp() {
    let mut model = open_with(&["work", "home"]);
    let last = rows(&model).len() - 1;

    for _ in 0..last + 3 {
        update(&mut model, key(KeyCode::Down));
    }
    assert_eq!(selected(&model), last, "clamped at the last row");

    for _ in 0..last + 3 {
        update(&mut model, key(KeyCode::Up));
    }
    assert_eq!(selected(&model), 0, "clamped at the first");
}

/// `^N`/`^P` are unbound inside, and `keymap::resolve` is untouched — so `^N`
/// outside an overlay still reaches `EditNotes`, exactly as today.
#[test]
fn ctrl_n_and_ctrl_p_do_nothing_inside() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Down));
    let before = (query(&model), selected(&model));

    update(&mut model, chord('n'));
    update(&mut model, chord('p'));

    assert_eq!((query(&model), selected(&model)), before);
}

#[test]
fn backspace_kill_word_and_kill_line_all_edit() {
    let mut model = open_with(&[]);
    for c in "two words".chars() {
        update(&mut model, ch(c));
    }

    update(&mut model, key(KeyCode::Backspace));
    assert_eq!(query(&model), "two word");

    update(&mut model, chord('w'));
    assert_eq!(query(&model), "two ");

    update(&mut model, chord('u'));
    assert_eq!(query(&model), "");
}

/// The reset keys on the **query string changing**, not on which key was
/// pressed: a `Backspace` on an empty query leaves the buffer byte-identical and
/// must leave the highlight alone.
#[test]
fn a_no_op_edit_leaves_the_selection_alone() {
    let mut model = open_with(&["work", "home"]);
    update(&mut model, key(KeyCode::Down));
    update(&mut model, key(KeyCode::Down));
    let parked = selected(&model);
    assert!(parked > 0);

    for no_op in [key(KeyCode::Backspace), chord('u'), chord('w')] {
        update(&mut model, no_op);
        assert_eq!(selected(&model), parked, "a no-op edit moved the highlight");
        assert_eq!(query(&model), "");
    }

    // A real edit does reset it.
    update(&mut model, ch('w'));
    assert_eq!(selected(&model), 0);
}

/// A `ListsLoaded` can shrink the rows under an open Omnibox — `update` routes
/// only `Message::Key` to `omnibox_key`. Parked on the **last row overall** (the
/// `:ascii` row), dropping one List is enough to strand the index; parked on the
/// last JUMP row it would take four.
///
/// The first keystroke is a swallowed `Left`: it must reach the write-back
/// without resetting `selected` to 0, and must not be the `Up` itself — or the
/// two assertions would contradict each other.
#[test]
fn a_shrinking_lists_loaded_is_repaired_on_the_next_keystroke() {
    let mut model = open_with(&["a", "b", "c"]);
    let last = rows(&model).len() - 1;
    for _ in 0..last {
        update(&mut model, key(KeyCode::Down));
    }
    assert_eq!(selected(&model), last);

    update(&mut model, Message::ListsLoaded(vec![list("a"), list("b")]));
    let shrunk = rows(&model).len();
    assert_eq!(shrunk, last, "one List fewer");

    update(&mut model, key(KeyCode::Left));
    assert_eq!(
        selected(&model),
        shrunk - 1,
        "the clamp did not repair a stale selection"
    );

    update(&mut model, key(KeyCode::Up));
    assert_eq!(selected(&model), shrunk - 2);
}

// ─── JUMP and SEARCH dispatch ───────────────────────────────────────────────

/// A JUMP lands with the task pane focused — asserted from `Focus::Sidebar`,
/// because from `Focus::Tasks` it would pass even with the line deleted. That
/// line sits at the call site, deliberately outside `open_selection`, so this is
/// the only thing standing between it and every sidebar `j`/`k` stealing focus.
#[test]
fn a_jump_focuses_the_task_pane() {
    let mut model = open_with(&["work"]);
    model.focus = Focus::Sidebar;
    for c in "work".chars() {
        update(&mut model, ch(c));
    }

    update(&mut model, key(KeyCode::Enter));

    assert_eq!(model.selected, Selection::List(0));
    assert_eq!(model.focus, Focus::Tasks);
    assert!(model.overlay.is_none());
}

/// Naming the pane you are already parked on is not inert — it is how you leave
/// Search for the List you came from. `move_list_selection`'s no-movement guard
/// is for a clamped cursor and deliberately not shared.
#[test]
fn a_jump_to_the_parked_list_still_leaves_search() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::List(0);
    update(&mut model, ch('S'));
    // `Enter`, not `Esc`: `S` leaves its query input open, and `Esc` *there*
    // exits Search outright — which would make the assertion below vacuous.
    update(&mut model, key(KeyCode::Enter));
    assert!(model.search_active());

    update(&mut model, ch('p'));
    for c in "work".chars() {
        update(&mut model, ch(c));
    }
    let commands = update(&mut model, key(KeyCode::Enter));

    assert!(!model.search_active(), "a named jump is never inert");
    assert_eq!(model.selected, Selection::List(0));
    assert!(!commands.is_empty(), "the pane reloads");
}

/// Through `enter_search`, never `Command::LoadSearch` directly — that is the
/// only path that arms `search_pending` and clears the inherited cursor.
#[test]
fn the_search_row_enters_search_with_the_trimmed_query() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::List(0);
    update(&mut model, ch('p'));
    for c in "  work  ".chars() {
        update(&mut model, ch(c));
    }
    // JUMP matches `work` too, so step past it to the SEARCH row.
    let search = rows(&model)
        .iter()
        .position(|r| matches!(r, OmniRow::Search { .. }))
        .expect("a SEARCH row");
    for _ in 0..search {
        update(&mut model, key(KeyCode::Down));
    }
    update(&mut model, key(KeyCode::Enter));

    assert!(model.search_active());
    assert!(model.search_pending);
    assert!(model.tasks.is_empty(), "the inherited cursor is cleared");
    assert_eq!(model.focus, Focus::Tasks);
    // Trimmed: `matches_filter` never trims, so a raw needle would hide a Task
    // titled exactly `work`.
    assert_eq!(model.filter.as_deref(), Some("work"));
    assert!(matches!(model.overlay, Some(Overlay::Filter)));
}

/// Already in Search: no reload, the corpus survives — and the cursor
/// re-anchors, which `Action::Search`'s own arm has no need to do because
/// `open_filter` never changes the query.
#[test]
fn the_search_row_inside_search_reloads_nothing() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::List(0);
    update(&mut model, ch('S'));
    // As above: `Enter` closes the query input and stays in Search.
    update(&mut model, key(KeyCode::Enter));
    assert!(model.search_active());
    update(&mut model, ch('p'));
    for c in "zz".chars() {
        update(&mut model, ch(c));
    }
    let search = rows(&model)
        .iter()
        .position(|r| matches!(r, OmniRow::Search { .. }))
        .expect("a SEARCH row");
    for _ in 0..search {
        update(&mut model, key(KeyCode::Down));
    }

    let commands = update(&mut model, key(KeyCode::Enter));

    assert!(commands.is_empty(), "no redundant LoadSearch");
    assert!(model.search_active());
    assert_eq!(model.filter.as_deref(), Some("zz"));
    assert!(matches!(model.overlay, Some(Overlay::Filter)));
}

/// Enter fires `rows[len - 1]` after a shrink — the row the renderer will draw.
/// Not "fires nothing": phase (b) has already clamped, and `omnibox_rows` is
/// never empty, so `rows.get` is always `Some`.
#[test]
fn enter_after_a_shrink_fires_the_clamped_row() {
    let mut model = open_with(&["a", "b", "c"]);
    let last = rows(&model).len() - 1;
    for _ in 0..last {
        update(&mut model, key(KeyCode::Down));
    }

    update(&mut model, Message::ListsLoaded(vec![list("a"), list("b")]));
    // The last row is `:ascii`, a COMMAND row: it keeps the overlay rather than
    // firing, which is exactly what the clamped index should reach.
    update(&mut model, key(KeyCode::Enter));

    assert!(
        matches!(model.overlay, Some(Overlay::Omnibox { .. })),
        "the clamped row was fired, not a stale one"
    );
}

// ─── CAPTURE ────────────────────────────────────────────────────────────────

fn capture(model: &Model, q: &str) -> Option<CaptureRow> {
    omnibox_rows(model, q)
        .into_iter()
        .find_map(|row| match row {
            OmniRow::Capture(c) => Some(c),
            _ => None,
        })
}

/// The row names its destination *and* the peeled title and effective due, so
/// what `Enter` does is legible before it is pressed.
#[test]
fn the_capture_row_states_its_destination_title_and_due() {
    let mut model = open_with(&["work"]);
    model.selected = Selection::List(0);

    assert_eq!(
        capture(&model, "call mum tomorrow"),
        Some(CaptureRow::Ready {
            list_title: "work".into(),
            title: "call mum".into(),
            due: Some(model.now.date_naive() + chrono::Duration::days(1)),
        })
    );
}

/// `split_title_and_due` never peels word 0, so a lone date-word stays a title.
/// Its **due** depends on the pane, which is why both are pinned: a List leaves
/// it undated, Today defaults it to today so the entry stays on its page.
#[test]
fn a_lone_date_word_is_a_title_and_its_due_follows_the_pane() {
    let mut model = open_with(&["work"]);

    model.selected = Selection::List(0);
    assert_eq!(
        capture(&model, "tomorrow"),
        Some(CaptureRow::Ready {
            list_title: "work".into(),
            title: "tomorrow".into(),
            due: None,
        })
    );

    model.selected = Selection::Today;
    model.default_list = Some(ListId("work".into()));
    assert_eq!(
        capture(&model, "tomorrow"),
        Some(CaptureRow::Ready {
            list_title: "work".into(),
            title: "tomorrow".into(),
            due: Some(model.now.date_naive()),
        })
    );
}

/// Neither SEARCH nor CAPTURE on an empty or whitespace-only query: both gate on
/// the trimmed one, so `matches_filter`'s "empty matches everything" — right for
/// JUMP — does not leak into an offer.
#[test]
fn neither_search_nor_capture_appears_without_a_query() {
    let mut model = open_with(&["work"]);
    model.selected = Selection::List(0);

    for q in ["", "   "] {
        let groups: Vec<_> = omnibox_rows(&model, q).iter().map(OmniRow::group).collect();
        assert!(!groups.contains(&Group::Search), "{q:?}");
        assert!(!groups.contains(&Group::Capture), "{q:?}");
    }
}

/// Today with `default_list` unresolved: a reason on the row, and `Enter` inert
/// — it must not delegate, because `finish_add_task` would write that reason to
/// the status line, which an unfireable row promises not to do.
#[test]
fn a_refused_capture_states_its_reason_and_enter_is_inert() {
    let mut model = open_with(&["work"]);
    assert_eq!(model.selected, Selection::Today);
    assert!(model.default_list.is_none());
    model.status_line = Some("kept".into());

    for c in "buy bread".chars() {
        update(&mut model, ch(c));
    }
    assert!(matches!(
        capture(&model, "buy bread"),
        Some(CaptureRow::Refused { .. })
    ));

    let last = rows(&model).len() - 1;
    for _ in 0..last {
        update(&mut model, key(KeyCode::Down));
    }
    let commands = update(&mut model, key(KeyCode::Enter));

    assert!(commands.is_empty(), "nothing was created");
    assert!(
        matches!(model.overlay, Some(Overlay::Omnibox { .. })),
        "an unfireable row keeps the overlay"
    );
    assert_eq!(model.status_line.as_deref(), Some("kept"));
}

/// The two **nameless** cases emit no row at all — no destination to name and no
/// reason to print, and drawing "Create task…" with nowhere to put it would be
/// the one outright lie.
#[test]
fn a_nameless_target_emits_no_capture_row() {
    // A List selection that resolves to nothing.
    let mut model = Model::new();
    model.selected = Selection::List(0);
    assert_eq!(capture(&model, "buy bread"), None);

    // And a resolved `default_list` naming a List `lists` does not hold —
    // `DefaultListResolved` never cross-checks, so this is reachable at startup.
    let mut model = open_with(&["work"]);
    model.selected = Selection::Today;
    model.default_list = Some(ListId("ghost".into()));
    assert_eq!(capture(&model, "buy bread"), None);
}

/// `a` is unaffected by the row's stricter rule: it needs the id, not a title.
#[test]
fn a_still_captures_where_the_omnibox_row_is_absent() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    model.selected = Selection::Today;
    model.default_list = Some(ListId("ghost".into()));

    update(&mut model, ch('a'));

    assert!(
        matches!(model.overlay, Some(Overlay::AddTask { .. })),
        "`a` opened its capture overlay"
    );
}

/// Firing it creates through `finish_add_task` and closes the overlay.
#[test]
fn firing_the_capture_row_creates_and_closes() {
    let mut model = open_with(&["work"]);
    model.selected = Selection::List(0);
    for c in "buy bread".chars() {
        update(&mut model, ch(c));
    }
    let last = rows(&model).len() - 1;
    assert!(matches!(rows(&model)[last], OmniRow::Capture(_)));
    for _ in 0..last {
        update(&mut model, key(KeyCode::Down));
    }

    let commands = update(&mut model, key(KeyCode::Enter));

    assert!(model.overlay.is_none());
    assert!(!commands.is_empty(), "the insert was requested");
    assert!(model.tasks.iter().any(|t| t.title == "buy bread"));
}
