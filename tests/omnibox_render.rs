//! The Omnibox as actually drawn. Like `tests/search_render.rs`, these need a
//! terminal: the popup's layout, its panel title and the row format are private
//! to `ui`, so the only way to assert what reaches the screen is to draw a frame
//! and read the buffer.
//!
//! Two things here are not assertable anywhere else. The **reasons** every row
//! is required to carry only fit because the popup is wider than a picker's and
//! reserves the trail before truncating the label — a whole-trail rule at
//! `OVERLAY_WIDTH` would clip away the thing each row exists to say. And the
//! **header count**, because `ListState` scrolls the selected row into view, so a
//! popup several rows too short still passes a reversed-line check.

use chrono::{TimeZone, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use oxidone::app::{update, Message, Model, Overlay};
use oxidone::config::Flavor;
use oxidone::domain::{List, ListId, Selection};
use oxidone::ui;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::Terminal;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

fn key(code: KeyCode) -> Message {
    Message::Key(KeyEvent::new(code, KeyModifiers::empty()))
}

fn ch(c: char) -> Message {
    key(KeyCode::Char(c))
}

fn list(title: &str) -> List {
    List {
        id: ListId(title.to_string()),
        title: title.to_string(),
        etag: "e".to_string(),
        updated: Utc.timestamp_opt(1_700_000_000, 0).unwrap(),
    }
}

fn open_with(titles: &[&str]) -> Model {
    let mut model = Model::new();
    update(
        &mut model,
        Message::ListsLoaded(titles.iter().map(|t| list(t)).collect()),
    );
    model.selected = Selection::List(0);
    update(&mut model, ch('p'));
    model
}

fn typed(model: &mut Model, s: &str) {
    for c in s.chars() {
        update(model, ch(c));
    }
}

fn rows_at(model: &Model, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("TestBackend");
    terminal.draw(|frame| ui::draw(model, frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol().to_string())
                .collect()
        })
        .collect()
}

fn rows(model: &Model) -> Vec<String> {
    rows_at(model, WIDTH, HEIGHT)
}

/// The popup's own rectangle: `(x0, x1, y0, y1)`, inclusive.
///
/// Scoped in **both** axes, deliberately. The popup is 72 columns of an 80-column
/// frame, so its columns overlap the sidebar behind it — a column-only scope
/// would answer with the sidebar's own reversed cursor.
fn popup_rect(drawn: &[String]) -> (usize, usize, usize, usize) {
    let top = drawn
        .iter()
        .position(|r| r.contains("Omnibox"))
        .expect("no Omnibox popup drawn");
    let row: Vec<char> = drawn[top].chars().collect();
    // The *nearest* corner before the title, not the first in the row: a tall
    // popup starts on row 0, where the panes' own top-left corner also sits.
    let title_at = row
        .iter()
        .collect::<String>()
        .find("Omnibox")
        .expect("the title");
    let x0 = row[..title_at]
        .iter()
        .rposition(|c| *c == '╭')
        .expect("a left corner");
    let x1 = row
        .iter()
        .enumerate()
        .skip(title_at)
        .find(|(_, c)| **c == '╮')
        .map(|(i, _)| i)
        .expect("a right corner");
    let bottom = drawn
        .iter()
        .enumerate()
        .skip(top)
        .find(|(_, r)| r.chars().nth(x0) == Some('╰'))
        .map(|(y, _)| y)
        .expect("a bottom border");
    (x0, x1, top, bottom)
}

/// The popup's rows, cropped to its own rectangle.
fn popup_rows(model: &Model) -> Vec<String> {
    let drawn = rows(model);
    let (x0, x1, y0, y1) = popup_rect(&drawn);
    drawn[y0..=y1]
        .iter()
        .map(|r| r.chars().skip(x0).take(x1 - x0 + 1).collect())
        .collect()
}

/// Whether the popup — not the frame — shows `text`.
fn shows(model: &Model, text: &str) -> bool {
    popup_rows(model).iter().any(|r| r.contains(text))
}

/// The reversed row **inside the popup**. The sidebar's selected List is
/// reversed too, so this is scoped to the popup's columns.
fn highlighted(model: &Model) -> Option<String> {
    let drawn = rows(model);
    let (x0, x1, y0, y1) = popup_rect(&drawn);
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend");
    terminal.draw(|frame| ui::draw(model, frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (y0..=y1)
        .find(|y| {
            (x0..=x1).any(|x| {
                buffer[(x as u16, *y as u16)]
                    .style()
                    .add_modifier
                    .contains(Modifier::REVERSED)
            })
        })
        .map(|y| {
            (x0..=x1)
                .map(|x| buffer[(x as u16, y as u16)].symbol().to_string())
                .collect()
        })
}

// ─── the panel title ────────────────────────────────────────────────────────

/// The base always names the box, so an empty query is not an untitled popup;
/// the caret is unconditional, the Omnibox having no committed state to
/// distinguish from a live one.
#[test]
fn the_panel_title_names_the_box_and_carries_a_caret() {
    let mut model = open_with(&["work"]);
    assert!(shows(&model, "Omnibox"));

    typed(&mut model, "wo");
    assert!(shows(&model, "Omnibox  wo▏"));
}

/// Clipped from the **left**, keeping the tail: `truncate` drops the tail, which
/// here would hide the characters just typed and the caret with them. Budgeted
/// off `popup.width`, so a frame narrower than `OMNIBOX_WIDTH` still fits inside
/// its own border.
#[test]
fn a_long_query_keeps_its_tail_and_fits_a_narrow_frame() {
    let mut model = open_with(&["work"]);
    typed(
        &mut model,
        "abcdefghijklmnopqrstuvwxyz0123456789abcdefghijklmnopqrstuvwxyz0123456789",
    );

    for width in [WIDTH, 40] {
        let drawn = rows_at(&model, width, HEIGHT);
        let title = drawn
            .iter()
            .find(|r| r.contains("Omnibox"))
            .unwrap_or_else(|| panic!("no title at {width} columns"))
            .clone();
        assert!(title.contains('…'), "no leading ellipsis: {title:?}");
        assert!(title.contains("6789▏"), "the tail was clipped: {title:?}");
        assert!(
            title.trim_end().width_chars() <= width as usize,
            "title overflowed {width} columns: {title:?}"
        );
    }
}

trait WidthChars {
    fn width_chars(&self) -> usize;
}
impl WidthChars for str {
    fn width_chars(&self) -> usize {
        self.chars().count()
    }
}

// ─── the popup ──────────────────────────────────────────────────────────────

/// Every row **and** every group header is drawn. `picker_height` must size off
/// the header-inclusive item vector; sized off the row count the popup is up to
/// four rows short, and `ListState`'s scrolling hides that from any
/// reversed-line assertion.
#[test]
fn the_popup_is_tall_enough_for_its_headers() {
    // A List whose title shares a prefix with a command, so all four groups are
    // non-empty at once.
    let mut model = open_with(&["flavour notes"]);
    typed(&mut model, "fla");

    let drawn = popup_rows(&model);
    for header in [
        "JUMP",
        "COMMAND · settings are session only",
        "SEARCH",
        "CAPTURE",
    ] {
        assert!(
            drawn.iter().any(|r| r.contains(header)),
            "{header:?} missing: {drawn:#?}"
        );
    }
    assert!(
        drawn.iter().any(|r| r.contains("Search all Lists")),
        "the row below the last header is cut off: {drawn:#?}"
    );
}

/// The header carries the caveat once, rather than a dozen cells of it on every
/// command row — and scopes it to the *settings*, because `:refresh` shares the
/// group and sets nothing.
#[test]
fn the_command_header_scopes_session_only_to_the_settings() {
    let model = open_with(&["work"]);
    assert!(shows(&model, "COMMAND · settings are session only"));
}

/// The row vector is headerless while the drawn item list is not, so the
/// highlight has to be remapped — passed through unmapped it lands N lines high,
/// one per header above it.
#[test]
fn the_highlight_lands_on_the_intended_row_below_its_headers() {
    let mut model = open_with(&["work"]);
    typed(&mut model, "work");
    // Past JUMP's single row and its header, onto the SEARCH row.
    update(&mut model, key(KeyCode::Down));

    let row = highlighted(&model).expect("a highlighted row");
    assert!(
        row.contains("Search all Lists"),
        "the highlight missed its row: {row:?}"
    );
}

/// Enough Lists to overflow the frame: the selected row is still drawn, and it
/// is still the intended one.
#[test]
fn a_scrolled_popup_still_highlights_the_intended_row() {
    let titles: Vec<String> = (0..40).map(|i| format!("list{i:02}")).collect();
    let mut model = open_with(&titles.iter().map(String::as_str).collect::<Vec<_>>());
    // Two pinned rows (Today, Week) precede the Lists, so `list38` is 40 down.
    for _ in 0..40 {
        update(&mut model, key(KeyCode::Down));
    }

    let row = highlighted(&model).expect("a highlighted row after scrolling");
    assert!(row.contains("list38"), "{row:?}");
}

// ─── the row format ─────────────────────────────────────────────────────────

/// Every reason and effect the plan requires on screen is legible at 80 columns,
/// with `trail` intact and only `lead` shortened.
#[test]
fn reasons_and_effects_are_legible_at_eighty_columns() {
    let mut model = open_with(&["work"]);
    typed(&mut model, "flavor purple");
    assert!(
        shows(&model, "unknown — latte|frappe|macchiato|mocha"),
        "the valid set is the longest trail in the design"
    );

    // `:ascii` — a non-`:horizon` valid effect.
    let mut model = open_with(&["work"]);
    model.ascii = false;
    typed(&mut model, "ascii on");
    assert!(shows(&model, "off → on"));

    // `:horizon` outside Search with the filter off: the effect *and* the
    // advisory.
    let mut model = open_with(&["work"]);
    model.hide_distant = false;
    model.horizon_days = 14;
    typed(&mut model, "horizon 30");
    assert!(shows(&model, "14 → 30"));
    assert!(shows(&model, "· filter off (w)"));

    // The advisory is on the `NeedsArgument` row too — a field asserted on three
    // states must be drawn on more than one.
    let mut model = open_with(&["work"]);
    model.hide_distant = false;
    typed(&mut model, "horizon");
    assert!(shows(&model, "· filter off (w)"));
}

/// In Search the row states *that* refusal and carries no `w` advisory: `w` is
/// itself refused there, so naming it would be false.
#[test]
fn the_search_refusal_is_drawn_without_the_advisory() {
    let mut model = open_with(&["work"]);
    update(&mut model, key(KeyCode::Esc));
    update(&mut model, ch('S'));
    update(&mut model, key(KeyCode::Enter));
    update(&mut model, ch('p'));
    typed(&mut model, "horizon 30");

    assert!(shows(&model, "not in Search"));
    assert!(!shows(&model, "filter off (w)"));
}

/// A long **List title**, short query: the destination truncates inside its
/// quotes, and the peeled title and due survive.
#[test]
fn a_long_list_title_truncates_and_keeps_the_title_and_due() {
    let mut model = open_with(&["a list with a really quite long name indeed"]);
    typed(&mut model, "call the notary");

    assert!(
        shows(&model, "→ call the notary"),
        "{:#?}",
        popup_rows(&model)
    );
}

/// A long **query**, short List title: the destination survives in full. This is
/// the case the per-part reserve exists for — a whole-trail rule would let the
/// user's own text eat the one thing the row must name.
#[test]
fn a_long_query_never_costs_the_capture_destination() {
    let mut model = open_with(&["work"]);
    typed(
        &mut model,
        "renew the passport and also the driving licence before the trip",
    );

    assert!(
        shows(&model, "Create task in \"work\""),
        "the destination was eaten: {:#?}",
        popup_rows(&model)
    );
}

// ─── the commands reach the frame ───────────────────────────────────────────

/// Driven end to end: `:flavor latte` through `update`, then `ui::draw`. Test 20
/// proves the seam composes; this proves the command reaches it.
#[test]
fn flavor_latte_repaints_the_frame() {
    let mut model = open_with(&["work"]);
    model.flavor = Flavor::Mocha;
    let before = background(&model);

    // Row 0 is the `:flavor` row: no List is named `flavor latte`, so JUMP is
    // empty and COMMAND is the first group.
    typed(&mut model, "flavor latte");
    update(&mut model, key(KeyCode::Enter));

    assert_eq!(model.flavor, Flavor::Latte);
    assert_ne!(before, background(&model), "the frame did not repaint");
}

#[test]
fn ascii_on_reaches_the_renderer() {
    let mut model = open_with(&["work"]);
    model.ascii = false;

    typed(&mut model, "ascii on");
    update(&mut model, key(KeyCode::Enter));

    assert!(model.ascii);
    assert!(model.overlay.is_none());
}

fn background(model: &Model) -> ratatui::style::Color {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("TestBackend");
    terminal.draw(|frame| ui::draw(model, frame)).expect("draw");
    terminal.backend().buffer()[(0, 0)].bg
}

/// The popup never covers the status line or the legend below it — that legend
/// is what advertises the overlay's own keys.
#[test]
fn the_popup_clears_the_status_line_and_the_legend() {
    let mut model = open_with(&["work"]);
    typed(&mut model, "work");

    let drawn = rows(&model);
    let legend = drawn.last().expect("a legend row");
    assert!(legend.contains("Enter run"), "{legend:?}");
    assert!(
        !legend.contains('│'),
        "the popup reached the legend row: {legend:?}"
    );
    assert!(
        matches!(model.overlay, Some(Overlay::Omnibox { .. })),
        "still open"
    );
}

/// A command row echoes the argument it was built from, so the row names what it
/// will act on rather than the bare verb — `:flavor purple`, not `:flavor`. Only
/// `NeedsArgument` shows the `‹arg›` placeholder.
#[test]
fn a_command_row_echoes_its_typed_argument() {
    let mut model = open_with(&["work"]);
    typed(&mut model, "flavor purple");
    assert!(shows(&model, ":flavor purple"), "{:#?}", popup_rows(&model));

    let mut model = open_with(&["work"]);
    model.horizon_days = 14;
    typed(&mut model, "horizon 30");
    assert!(shows(&model, ":horizon 30"), "{:#?}", popup_rows(&model));

    // No argument yet: the placeholder, not an echo.
    let mut model = open_with(&["work"]);
    typed(&mut model, "horizon");
    assert!(shows(&model, ":horizon ‹arg›"), "{:#?}", popup_rows(&model));
}

/// `:refresh` is the one verb that is `Valid` with **no** argument, so it is the
/// only row that reaches `command_arg_suffix` with nothing to append. The bare
/// verb is what must come out: a `‹arg›` here would advertise an argument that
/// makes the row refuse.
#[test]
fn the_argumentless_verb_renders_bare() {
    let mut model = open_with(&["work"]);
    model.api_available = true;
    typed(&mut model, "refresh");

    let drawn = popup_rows(&model);
    let row = drawn
        .iter()
        .find(|r| r.contains(":refresh"))
        .unwrap_or_else(|| panic!("no `:refresh` row: {drawn:#?}"));
    assert!(
        !row.contains('‹'),
        "a placeholder on a complete verb: {row:?}"
    );
    assert!(row.contains("pull from Google"), "{row:?}");
}

/// A short title must not be discarded into blank padding. `lead_budget` already
/// reserved room for it, so once the destination takes that reservation the
/// title's budget equals its width exactly — a flat floor then dropped every
/// title under 8 cells despite it fitting.
///
/// The critic's repro: at 80 columns the row has 68 cells, the destination takes
/// 60, and `→ milk` fits in the 8 that remain.
#[test]
fn a_short_title_survives_beside_a_long_destination() {
    let mut model = open_with(&["a list with a really quite long name indeed"]);
    typed(&mut model, "milk");

    assert!(
        shows(&model, "→ milk"),
        "a title that fits was dropped: {:#?}",
        popup_rows(&model)
    );
}

/// The floor still does its job where it was meant to: a title with too little
/// room to say anything useful drops **whole** rather than rendering as `→ …`,
/// and the destination survives at its floor.
///
/// Driven at **38 columns**, because the branch does not exist above it. The
/// popup caps at `OMNIBOX_WIDTH`, so at 80 columns a row has 68 cells; with the
/// destination floored at 24 the title still gets 40 and renders truncated. The
/// budget only falls under the floor once the frame does — 40 columns leaves
/// exactly 8, and 38 leaves 6. Narrow frames are supported and exercised:
/// `legend_render` drives 30 and 22.
#[test]
fn a_long_title_with_no_room_drops_whole_rather_than_eliding() {
    let mut model = open_with(&["a list with a really quite long name indeed"]);
    typed(&mut model, "renew the passport before the trip abroad");

    let drawn = rows_at(&model, 38, HEIGHT);
    let capture = drawn
        .iter()
        .find(|r| r.contains("Create task"))
        .unwrap_or_else(|| panic!("no CAPTURE row at 38 columns: {drawn:#?}"));

    assert!(
        !capture.contains('→'),
        "the title should be dropped whole, not elided: {capture:?}"
    );
    assert!(
        capture.contains("Create task in"),
        "the destination must survive at its floor: {capture:?}"
    );
}
