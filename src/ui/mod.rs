//! Rendering. A pure `view(&Model)` over ratatui. btop structural language
//! (rounded panels) with a Catppuccin palette (ADR-0006). Two-pane: List
//! sidebar + task pane, a one-line status bar, and an always-visible hotkey
//! legend below it. Both the `?` overlay and the legend are drawn straight from
//! the keymap table — the legend as a curated, priority-ordered subset.
//!
//! The smallest supported terminal is 80x24; the `?` cheatsheet is required to
//! fit there in full, which `help_layout` guarantees by sizing against the frame
//! rather than the row count.

pub mod theme;
pub mod widgets;

use chrono::{DateTime, Local, NaiveDate};
use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::{renders_as_subtask, Focus, Model, Overlay};
use crate::dateparse::{format_due_relative, split_title_and_due};
use crate::domain::{due_before, due_on_or_before, EntryType, ListId, Selection, Status, Task};
use crate::keymap;
use crate::links::{self, Link};
use theme::Theme;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};
use widgets::{dueload, meter};

/// Days of "workload ahead" bucketed into the due-load strip (today + 6).
const DUE_LOAD_DAYS: usize = 7;
/// Braille/ASCII cells the header completion meter occupies.
const HEADER_METER_WIDTH: u16 = 10;
/// Cells a bordered pane spends on its own frame across the width: one per side.
/// Anything budgeting a row's real text width must subtract it, along with
/// [`LIST_CURSOR`]'s gutter.
const PANEL_BORDERS: u16 = 2;
/// Braille/ASCII cells a sidebar List row's completion meter occupies. Narrower
/// than the header's: the sidebar is a 30% pane and the bar shares the row with
/// the title it belongs to.
const SIDEBAR_METER_WIDTH: u16 = 6;
/// Braille/ASCII cells a parent Task row's Subtask meter occupies. Subtask counts
/// are small, so a short bar reads them well enough — the ratio does the rest.
const SUBTASK_METER_WIDTH: u16 = 4;

/// Render the whole frame. Never mutates state. `ascii` reflects
/// `config.ascii_fallback`: braille data widgets degrade to ASCII when set.
pub fn view(model: &Model, theme: &Theme, ascii: bool, frame: &mut Frame) {
    let area = frame.area();
    frame.render_widget(Block::default().style(Style::new().bg(theme.base)), area);

    // Content row, a status line, then the always-visible hotkey legend. The
    // legend gets its own row rather than sharing the status line so a transient
    // message never hides it.
    let [content, status, legend] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(content);
    render_sidebar(frame, panes[0], model, ascii, theme);
    render_task_pane(frame, panes[1], model, ascii, theme);
    render_status(frame, status, model, theme);
    render_legend(frame, legend, model, theme);

    if model.show_help {
        render_help(frame, area, theme);
    }
    if let Some(overlay) = &model.overlay {
        render_overlay(frame, area, overlay, model.now, theme);
    }
}

/// Width shared by every overlay, so the picker lines up with the text popups
/// rather than introducing a second modal size.
const OVERLAY_WIDTH: u16 = 50;
/// Cells a bordered overlay spends on its own frame, on either axis: one per
/// side, so two off the height *and* two off the usable text width. Used for
/// both deliberately — a `Block::bordered` costs the same in each direction.
const OVERLAY_BORDERS: u16 = 2;
/// Rows `view` reserves at the bottom of the frame for the status line and the
/// legend. The picker is the one overlay tall enough to reach them, and it must
/// not — the legend down there is what advertises its own keys.
const BOTTOM_CHROME_ROWS: u16 = 2;

fn render_overlay(
    frame: &mut Frame,
    area: Rect,
    overlay: &Overlay,
    now: DateTime<Local>,
    theme: &Theme,
) {
    // Every overlay but the picker is one or two lines of text in a popup. The
    // add-entry captures grow to a second line when a trailing date is
    // recognised (see `capture_lines`); the rest are always a single line.
    let (title, lines): (&str, Vec<Line>) = match overlay {
        Overlay::EditTitle { buffer, .. } => ("Edit title", vec![input_line(buffer)]),
        Overlay::AddTask { buffer } => ("Add task", capture_lines(buffer, now, theme)),
        Overlay::AddSubtask { buffer, .. } => ("Add subtask", capture_lines(buffer, now, theme)),
        Overlay::EditDue { buffer, .. } => {
            ("Edit due date (blank clears)", vec![input_line(buffer)])
        }
        Overlay::EditNotes { buffer, .. } => {
            ("Edit notes (blank clears)", vec![input_line(buffer)])
        }
        Overlay::AddList { buffer } => ("Add list", vec![input_line(buffer)]),
        Overlay::RenameList { buffer, .. } => ("Rename list", vec![input_line(buffer)]),
        Overlay::Confirm(confirm) => ("Confirm", vec![Line::from(confirm.prompt.clone())]),
        // The one overlay that is a list, not a line — and the only one whose
        // height is not fixed.
        Overlay::OpenLink { links, selected } => {
            return render_link_picker(frame, area, links, *selected, theme)
        }
        Overlay::MoveToList {
            targets, selected, ..
        } => return render_list_picker(frame, area, targets, *selected, theme),
        // The filter input draws no popup — the pane header carries its query and
        // caret (see `header_title`), so the narrowed pane stays fully visible.
        Overlay::Filter => return,
    };
    let height = u16::try_from(lines.len()).unwrap_or(1).max(1);
    let popup = centered(area, OVERLAY_WIDTH, height + OVERLAY_BORDERS);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(panel(title, true, theme)),
        popup,
    );
}

/// The editable line of a text overlay: the buffer trailed by a cursor bar.
fn input_line(buffer: &str) -> Line<'static> {
    Line::from(format!("{buffer}▏"))
}

/// Lines for an add-entry capture: the input line, plus — only when a trailing
/// date is recognised — a dim preview of the `title · due` split that submitting
/// (with `Enter`) will produce. `Tab` submits the buffer verbatim, so what the
/// preview shows is exactly what a plain `Enter` commits.
fn capture_lines(buffer: &str, now: DateTime<Local>, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = vec![input_line(buffer)];
    if let (title, Some(due)) = split_title_and_due(buffer, now) {
        let preview = format!("→ {title} · {}", format_due_relative(due, now.date_naive()));
        lines.push(Line::styled(preview, Style::new().fg(theme.subtext)));
    }
    lines
}

/// Height of the link picker: one row per link plus its borders, never taller
/// than the space available.
fn picker_height(links: usize, available: u16) -> u16 {
    u16::try_from(links)
        .unwrap_or(u16::MAX)
        .saturating_add(OVERLAY_BORDERS)
        .min(available)
}

/// The link picker. Raised only for more than one link, so it always has rows.
fn render_link_picker(
    frame: &mut Frame,
    area: Rect,
    links: &[Link],
    selected: usize,
    theme: &Theme,
) {
    // Centre within the content rows only. A Task with enough URLs would
    // otherwise grow a popup over the status line and over the legend spelling
    // out `j/k move  Enter open  Esc cancel` — hiding the instructions for the
    // very thing on screen.
    let body = Rect {
        height: area.height.saturating_sub(BOTTOM_CHROME_ROWS),
        ..area
    };
    let popup = centered(body, OVERLAY_WIDTH, picker_height(links.len(), body.height));
    // By characters, not bytes: a link's URL or description may be multibyte, and
    // slicing one mid-codepoint would panic. The gutter comes off the budget
    // too — `render_selectable` spends it on every row.
    let width =
        (popup.width.saturating_sub(OVERLAY_BORDERS) as usize).saturating_sub(LIST_CURSOR.width());
    let items: Vec<ListItem> = links
        .iter()
        .map(|link| ListItem::new(truncate(&link.display(), width, "…")))
        .collect();
    frame.render_widget(Clear, popup);
    render_selectable(frame, popup, "Links", items, Some(selected), true, theme);
}

/// The move-to-List picker. Raised only when there is at least one candidate,
/// so it always has rows.
fn render_list_picker(
    frame: &mut Frame,
    area: Rect,
    targets: &[crate::domain::List],
    selected: usize,
    theme: &Theme,
) {
    // Same reasoning as the link picker: keep it clear of the status line and the
    // legend spelling out `j/k move  Enter move here  Esc cancel`.
    let body = Rect {
        height: area.height.saturating_sub(BOTTOM_CHROME_ROWS),
        ..area
    };
    let popup = centered(
        body,
        OVERLAY_WIDTH,
        picker_height(targets.len(), body.height),
    );
    let width =
        (popup.width.saturating_sub(OVERLAY_BORDERS) as usize).saturating_sub(LIST_CURSOR.width());
    let items: Vec<ListItem> = targets
        .iter()
        .map(|list| ListItem::new(truncate(&list.title, width, "…")))
        .collect();
    frame.render_widget(Clear, popup);
    render_selectable(
        frame,
        popup,
        "Move to list",
        items,
        Some(selected),
        true,
        theme,
    );
}

/// `text` cut to `width` *display cells*, the last spent on `ellipsis` so a
/// truncated URL or preview never reads as a complete one.
///
/// Cells, not chars: a URL pasted from an IRI can carry double-width characters
/// (`https://例え.jp/…`), and budgeting by `chars().count()` would under-measure
/// them — ratatui lays out by cell, so the row would overflow and be clipped
/// with no ellipsis to show for it, which is the very thing this prevents.
///
/// `ellipsis` is a parameter, not a constant, because the notes preview folds `…`
/// down to `...` under `ascii_fallback`; its own display width is reserved, so a
/// three-cell `...` still leaves the result within `width`.
fn truncate(text: &str, width: usize, ellipsis: &str) -> String {
    if text.width() <= width {
        return text.to_string();
    }
    let budget = width.saturating_sub(ellipsis.width());
    let mut kept = String::new();
    let mut used = 0;
    for c in text.chars() {
        let cell = c.width().unwrap_or(0);
        if used + cell > budget {
            break;
        }
        kept.push(c);
        used += cell;
    }
    kept.push_str(ellipsis);
    kept
}

fn render_sidebar(frame: &mut Frame, area: Rect, model: &Model, ascii: bool, theme: &Theme) {
    let focused = model.focus == Focus::Sidebar;
    // The pinned Today row sits above the real Lists (no meter — its cross-List
    // completion is only known while it is the active pane). The cursor spans
    // `[Today, …lists]`, so the highlight index is offset by the one pinned row.
    let mut items: Vec<ListItem> = Vec::with_capacity(model.lists.len() + 1);
    items.push(ListItem::new("Today"));
    for l in &model.lists {
        items.push(ListItem::new(sidebar_row(
            &l.title,
            model.list_meter(&l.id),
            area.width,
            ascii,
        )));
    }
    let selected = match model.selected {
        Selection::Today => Some(0),
        Selection::List(i) => Some(i + 1),
    };
    render_selectable(frame, area, "Lists", items, selected, focused, theme);
}

/// A sidebar row: the List title, then its completion meter flush right.
///
/// Degrades in two stages, braille before text (ADR-0006): the bar goes first,
/// leaving the `done/total` that carries the actual number, and then that goes
/// too. When nothing fits the title is returned **unchanged** — the sidebar has
/// always let ratatui clip an over-long title, and right-aligning a meter is no
/// reason to start truncating here.
///
/// `area_width` is the pane's full width; the borders and the cursor gutter come
/// off inside, so callers cannot budget them wrongly.
fn sidebar_row(
    title: &str,
    counts: Option<(usize, usize)>,
    area_width: u16,
    ascii: bool,
) -> String {
    let Some((done, total)) = counts else {
        return title.to_string();
    };
    let usable =
        (area_width.saturating_sub(PANEL_BORDERS) as usize).saturating_sub(LIST_CURSOR.width());
    let title_width = title.width();

    // Each candidate is built and then measured, never predicted: the spacing
    // lives in the `format!` alone, so changing it cannot leave an arithmetic
    // twin behind. It also measures the ratio for free — `103/247` is 7 columns
    // where `3/8` is 3.
    let ratio = format!("{done}/{total}");
    let with_bar = format!(
        "  {} {ratio}",
        meter::render(done, total, SIDEBAR_METER_WIDTH, ascii)
    );
    let text_only = format!("  {ratio}");

    let segment = if title_width + with_bar.width() <= usable {
        with_bar
    } else if title_width + text_only.width() <= usable {
        text_only
    } else {
        return title.to_string();
    };

    // Pad rather than truncate: `segment` is only ever appended when the title
    // already fits alongside it.
    let pad = usable - title_width - segment.width();
    format!("{title}{}{segment}", " ".repeat(pad))
}

/// Width of the leading due-date column. Derived from the formatter's own
/// contract rather than restated here, so the column can never be narrower than
/// what `format_due_relative` may emit.
const DUE_WIDTH: usize = crate::dateparse::MAX_RENDERED_WIDTH;

/// The Subtask meter trailing a parent Task's row, or `""` when it will not fit.
///
/// Degrades in the same two stages as the sidebar's, braille before text
/// (ADR-0006): the bar drops first, leaving `done/total`, then that drops too.
///
/// `area_width` is the whole pane; the borders, the cursor gutter and the due
/// column come off inside. The gutter matters as much here as in the sidebar —
/// the task pane goes through the same `render_selectable`, which spends it on
/// every row — and leaving it out would clip the meter by two columns.
fn subtask_segment(
    counts: Option<(usize, usize)>,
    area_width: u16,
    due_gutter: bool,
    title_width: usize,
    marker_width: usize,
    ascii: bool,
) -> String {
    let Some((done, total)) = counts else {
        return String::new();
    };
    let gutter = if due_gutter { DUE_WIDTH + 2 } else { 0 };
    let usable = (area_width.saturating_sub(PANEL_BORDERS) as usize)
        .saturating_sub(LIST_CURSOR.width())
        .saturating_sub(gutter);
    let Some(room) = usable.checked_sub(title_width + marker_width) else {
        return String::new();
    };

    // Built then measured, as in `sidebar_row`: the spacing has one home.
    let ratio = format!("{done}/{total}");
    let with_bar = format!(
        "  {} {ratio}",
        meter::render(done, total, SUBTASK_METER_WIDTH, ascii)
    );
    let text_only = format!("  {ratio}");

    if with_bar.width() <= room {
        with_bar
    } else if text_only.width() <= room {
        text_only
    } else {
        String::new()
    }
}

/// Indent prefix for a Subtask row (nesting is capped at one level).
const SUBTASK_INDENT: &str = "  ";

/// The trailing mark on a Task whose notes hold an openable URL, or `None` when
/// there is nothing to open. Degrades to ASCII with the braille widgets
/// (ADR-0006) rather than drawing a glyph the terminal cannot show.
fn link_marker(has_urls: bool, ascii: bool) -> Option<&'static str> {
    has_urls.then_some(if ascii { " *" } else { " ⧉" })
}

/// The trailing mark on a Task carrying notes — the free-text body edited with
/// `n` — or `None` when it has none.
///
/// Not to be confused with [`EntryType::Note`], whose `—` signifier *leads* the
/// row: the two can share a line (`— call the notary ≡`). They are unrelated — a
/// Note-typed entry need not have notes, and any entry type may. "Notes" here
/// always means the body; the entry type is always spelled `EntryType::Note`.
///
/// Degrades to ASCII with the braille widgets (ADR-0006), following
/// [`link_marker`]: `=` echoes `≡` without colliding with the link marker's `*`.
/// `unicode-width` reports `≡` as one cell under its non-CJK default, and
/// `ascii_fallback` is the remedy for a terminal that disagrees.
fn notes_marker(has_notes: bool, ascii: bool) -> Option<&'static str> {
    has_notes.then_some(if ascii { " =" } else { " ≡" })
}

/// The first line of `notes` a reader could see — the source for both the `≡`
/// marker and the inline preview, found in one scan of the body.
///
/// The marker is `is_some()`; the preview is built from the same line by
/// [`notes_preview_segment`] when the row has room. Sharing one scan is the point:
/// an 8192-char body costs the first visible character in the common case, where
/// two scans would pay for it twice per visible row per frame.
///
/// Selects on [`is_invisible`], **not** on `str::trim`: a line of only
/// layout-hostile characters is non-blank yet sanitises to spaces, so a
/// trim-first test would pick it and then draw nothing, and skip a later line that
/// does have prose. Because [`is_layout_hostile`] `⊆` [`is_invisible`], the line
/// returned here always keeps a character through sanitising — the drawn preview
/// is never empty.
fn notes_preview_line(notes: &str) -> Option<&str> {
    notes
        .lines()
        .find(|line| line.chars().any(|c| !is_invisible(c)))
}

/// The authority to show in place of a preview line that is *nothing but* a URL —
/// `https://a.dev/1` → `a.dev` — sparing a preview that only restates what the
/// `⧉` marker already announced, and clips mid-path doing it. `None` when the line
/// carries prose (shown as-is) or the URL has no authority (`file:///x`).
///
/// Gated on the scanner seeing exactly one URL spanning the whole line, so
/// [`links::authority`] only ever slices a token [`links::scan_urls`] has already
/// validated — the two cannot disagree about *where* the URL is.
fn url_only_authority(line: &str) -> Option<&str> {
    match links::scan_urls(line).as_slice() {
        [only] if *only == line => links::authority(only),
        _ => None,
    }
}

/// The inline notes preview drawn at the very end of a row: [`PREVIEW_SEPARATOR`]
/// then `line` — sanitised, a URL-only line shortened to its authority, truncated
/// to what remains — or `None` when the row cannot spare [`MIN_PREVIEW_CELLS`]
/// after everything else.
///
/// Ordered last, after the Subtask meter, so this variable-length tail can never
/// clip a bounded widget; the meter keeps priority for scarce columns. `spent` is
/// every cell the row already drew *before* the preview — the signifier cell, the
/// *display* title (never `t.title`), the two markers, and the Subtask meter — so
/// the caller keeps the single definition of what a row has spent. The Subtask
/// indent is subtracted here too, which [`subtask_segment`] never has to: the
/// meter draws only on non-indented parent rows, the preview draws on every row.
fn notes_preview_segment(
    line: &str,
    area_width: u16,
    due_gutter: bool,
    is_subtask: bool,
    spent: usize,
    ascii: bool,
) -> Option<String> {
    let gutter = if due_gutter { DUE_WIDTH + 2 } else { 0 };
    let indent = if is_subtask {
        SUBTASK_INDENT.width()
    } else {
        0
    };
    let usable = (area_width.saturating_sub(PANEL_BORDERS) as usize)
        .saturating_sub(LIST_CURSOR.width())
        .saturating_sub(gutter)
        .saturating_sub(indent);
    let budget = usable.checked_sub(spent + PREVIEW_SEPARATOR.width())?;
    if budget < MIN_PREVIEW_CELLS {
        return None;
    }

    // Sanitise, then re-trim: leading or trailing hostile characters have become
    // spaces. The chosen line carries a reader-visible character, and
    // `is_layout_hostile ⊆ is_invisible`, so that character survives here — what
    // remains is never empty.
    let sanitised: String = line
        .chars()
        .map(|c| if is_layout_hostile(c) { ' ' } else { c })
        .collect();
    let trimmed = sanitised.trim();
    let shown = url_only_authority(trimmed).unwrap_or(trimmed);
    let ellipsis = if ascii { "..." } else { "…" };
    Some(format!(
        "{PREVIEW_SEPARATOR}{}",
        truncate(shown, budget, ellipsis)
    ))
}

/// The least room, in cells, worth spending on a notes preview; below it the row
/// carries the `≡` marker alone. A taste knob — small enough that a scrap of prose
/// still earns its column, large enough that a two-character sliver does not.
const MIN_PREVIEW_CELLS: usize = 8;

/// The single space charged between a row's trailing widgets and its notes
/// preview. Charged once, in [`notes_preview_segment`]'s budget.
const PREVIEW_SEPARATOR: &str = " ";

/// Whether `c` occupies no visible space of its own: whitespace, a control, or
/// one of the Unicode format characters that steer bidirectional text
/// ([`is_bidi_control`]).
///
/// #54's marker predicate — asks whether a notes body has anything a reader could
/// see, so a `≡` beside it does not promise text the editor will not show. A
/// different question from [`is_layout_hostile`]: this decides *whether to draw*,
/// that decides *what to neutralise* in text being laid out.
///
/// Combining marks are deliberately absent — they are zero-width by design but
/// part of legitimate text (a decomposed `é`), and a body holding one *is* visible.
fn is_invisible(c: char) -> bool {
    c.is_whitespace() || c.is_control() || is_bidi_control(c)
}

/// Whether `c` must be replaced with a space before its line is laid out.
///
/// [`truncate`] measures with `c.width().unwrap_or(0)`, counting a control or
/// format character as zero cells the terminal does not spend: a bidi control
/// ([`is_bidi_control`]) reorders the whole drawn row, due gutter and all, and a
/// C0/C1 control such as a mid-line tab expands to a tab stop and shifts it.
/// Neutralising both lets the row be measured and drawn honestly.
///
/// Narrower than [`is_invisible`], and deliberately so: combining marks are kept
/// (zero-width legitimate text), and `is_layout_hostile ⊆ is_invisible` — every
/// hostile character is also invisible, so a line chosen by [`notes_preview_line`]
/// (which has a *non*-invisible character) always survives sanitising non-empty.
/// VS16/ZWJ under-measure rather than reorder, so they clip; mangling user text to
/// buy a column back is worse than the residual.
fn is_layout_hostile(c: char) -> bool {
    c.is_control() || is_bidi_control(c)
}

/// The nine Unicode format characters (`Cf`) that steer bidirectional text.
///
/// Enumerated rather than derived: `char::is_control` covers only `Cc` and misses
/// these, and nine code points do not justify a Unicode-category dependency. One
/// home, shared by [`is_invisible`] and [`is_layout_hostile`] so the set cannot
/// drift between "is this visible" and "must this be neutralised".
fn is_bidi_control(c: char) -> bool {
    matches!(c,
        '\u{061c}'                    // ARABIC LETTER MARK
        | '\u{200e}' | '\u{200f}'     // LRM, RLM
        | '\u{202a}'..='\u{202e}'     // LRE, RLE, PDF, LRO, RLO
        | '\u{2066}'..='\u{2069}') // LRI, RLI, FSI, PDI
}

/// The Bullet Journal signifier for an entry type: `Event` and `Note` carry a
/// glyph, `Task` a blank of the same width — every variant occupies the same
/// cell so titles stay aligned down the pane
/// (`every_signifier_occupies_the_same_cell` pins that).
///
/// Degrades with the braille widgets (ADR-0006), following `link_marker`: both
/// are per-row *data* glyphs, and data is what `ascii_fallback` governs. Chrome
/// does not follow it — the panel borders, the `LIST_CURSOR` arrow and the pane
/// title's em dash stay Unicode either way — so this is consistency with the
/// marker beside it, not a claim about every glyph on screen.
///
/// Rendering only: `EntryType::apply` always writes the Unicode glyph, or
/// toggling the flag would silently revert every typed entry to `Task` on the
/// next read.
///
/// `○` and `—` are East Asian Ambiguous, so a terminal configured to render
/// Ambiguous as double-width shifts signifier rows by a column. `ascii_fallback`
/// is the remedy: `o` and `-` are unambiguously single-width.
fn signifier(entry: EntryType, ascii: bool) -> &'static str {
    match (entry, ascii) {
        // A Task's blank is a *rendering* fact — `prefix()` is "" for a Task,
        // because it writes nothing — so it is the one arm stated here.
        (EntryType::Task, _) => "  ",
        (EntryType::Event, true) => "o ",
        (EntryType::Note, true) => "- ",
        // Derived, never restated: the glyph drawn is the glyph written, so the
        // two cannot drift into a state where a typed entry renders its raw
        // title inline with no signifier beside it.
        (typed, false) => typed.prefix(),
    }
}

/// Style for a Task's due-date cell. Overdue reads in the palette's red so it
/// catches the eye when scanning the column — but Completed wins: a done Task
/// is settled, so its date stays dim alongside the struck-through title.
///
/// The date test is `due_before`, shared with Today's Overdue group, so the two
/// cannot drift. The Completed exemption is *this* call site's alone: the group
/// is status-blind by necessity (a Completed overdue row must still sort into the
/// contiguous prefix the spread counts), while the colour is a nudge to act, and
/// there is nothing left to do about a row already done.
fn due_style(task: &Task, today: NaiveDate, theme: &Theme) -> Style {
    let overdue = task.status != Status::Completed && due_before(task.due, today);
    Style::new().fg(if overdue {
        theme.overdue
    } else {
        theme.subtext
    })
}

fn render_task_pane(frame: &mut Frame, area: Rect, model: &Model, ascii: bool, theme: &Theme) {
    let focused = model.focus == Focus::Tasks;
    // The displayed rows are a read-only lens over `tasks`: the current sort's
    // order, keeping what passes every view filter at once (`Model::is_visible` —
    // Completed unless revealed, the distant-due horizon, and in Today membership
    // plus completion recency). `tasks` (Manual order) stays untouched.
    //
    // The header meter does not read this lens, so the two disagree by design.
    // It counts over `tasks`, narrowed on two axes only: Task-typed entries (so
    // its `total` is *not* `model.tasks.len()` on a pane holding Events or Notes)
    // and, in Today, `due <= today`. Hiding Completed Tasks therefore never moves
    // the meter, and neither does the horizon or recency dropping a row from view
    // — but leaving Today's membership does. See `header_title`.
    let ordered = model.visible_tasks();
    // Overdue is a property of the date against today, decided here in the view
    // — `model.now` keeps that testable rather than reading the wall clock.
    let today = model.now.date_naive();
    // Two independent axes, both true only for Today today (Search joins `flat`).
    // `flat`: a cross-List pane — no Subtask indent or meters (per-List hierarchy
    // concepts), and each row carries a muted List name so its home is visible
    // where rows from different Lists sit together. `spread`: the journal spread
    // and the two column rules that serve it — the Overdue group, the always-on
    // signifier gutter, and the overdue-only due column.
    let flat = model.today_active();
    let spread = model.today_active();
    // The Overdue group, as a count of rows: `cross_list_ordered` sorts them to the
    // front, so they are a contiguous prefix and `take_while` sees all of them.
    // Zero outside a spread, where there is no such group.
    let overdue_rows = if spread {
        ordered
            .iter()
            .take_while(|t| due_before(t.due, today))
            .count()
    } else {
        0
    };
    // Due dates lead the row in a fixed-width gutter so they scan vertically.
    // The gutter only exists when something in view has a due date — otherwise
    // every title would sit behind a column of blanks.
    //
    // In a spread every row is dated, so that test would always pass and every
    // today-due row would read "today" down a 12-cell column. The column exists
    // there on the *Overdue group's* condition instead — exactly the one that
    // draws the `Overdue` header — so the two appear and vanish together and
    // titles never shift without the header announcing it.
    let due_gutter = if spread {
        overdue_rows > 0
    } else {
        ordered.iter().any(|t| t.due.is_some())
    };
    // Like the due gutter: the cell only exists when something in view is typed.
    // On an all-Task pane — the overwhelmingly common case — a column of blanks
    // would spend width to say "ordinary".
    //
    // The spread is the exception: it reserves the gutter always, so titles hold
    // their column as Events and Notes enter and leave the day. That fixed position
    // is what makes it a gutter rather than a cell.
    let signifiers = spread || ordered.iter().any(|t| t.entry_type() != EntryType::Task);
    // Built once per render: the per-row indent check is then a hash lookup, not
    // a scan of every Task.
    let top_level = model.top_level_ids();
    // Shares that set rather than deriving its own, so the meter counts exactly
    // the rows the indent rule nests — and stays one pass over `tasks`.
    let subtask_counts = model.subtask_counts(&top_level);
    let list_titles: HashMap<&ListId, &str> = if flat {
        model
            .lists
            .iter()
            .map(|l| (&l.id, l.title.as_str()))
            .collect()
    } else {
        HashMap::new()
    };
    let items: Vec<ListItem> = ordered
        .iter()
        .map(|t| {
            // Completed Tasks read dim + struck-through until cleared.
            let style = if t.status == Status::Completed {
                Style::new()
                    .fg(theme.subtext)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else {
                Style::new()
            };
            let mut spans = Vec::new();
            if due_gutter {
                // Relative to the model's clock stamp, so the view reads no
                // clock of its own. Left-aligned in the column: the relative
                // forms are all shorter than the ISO fallback, so they pad
                // rather than truncate and the titles stay aligned.
                //
                // In a spread only the Overdue group prints a date. A today-due
                // row's cell is blank *at full width*, so titles stay aligned
                // across the fold — the `Today` header two rows up already said
                // what the date would be.
                let prints_date = !spread || due_before(t.due, today);
                let due = match t.due {
                    Some(d) if prints_date => format_due_relative(d, today),
                    _ => String::new(),
                };
                spans.push(Span::styled(
                    format!("{due:<DUE_WIDTH$}  "),
                    due_style(t, today, theme),
                ));
            }
            // Subtasks sit indented under their parent so the hierarchy reads.
            // An orphan (parent gone) draws flush-left rather than claiming the
            // row above it as its parent. Never in a flat pane.
            let is_subtask = !flat && renders_as_subtask(&top_level, t);
            if is_subtask {
                spans.push(Span::raw(SUBTASK_INDENT));
            }
            // *After* the indent: hoisted outside it, a Subtask's glyph would
            // share a column with its parent's and flatten the only cue telling
            // them apart. Inherits the row style, like the link marker — a
            // Completed Event reads as one settled line.
            let cell = signifiers.then(|| signifier(t.entry_type(), ascii));
            if let Some(cell) = cell {
                spans.push(Span::styled(cell, style));
            }
            spans.push(Span::styled(t.display_title().to_string(), style));
            // What the row actually put on screen, not what the Task stores: the
            // signifier cell plus the *display* title. The Subtask meter budgets
            // against this, so it must be derived from the same two values that
            // were just drawn — `t.title` is neither of them, and on a pane with
            // signifiers an untyped row's raw title understates the drawn width
            // by the cell, handing the meter room the row does not have.
            let drawn_width = cell.map_or(0, |c| c.width()) + t.display_title().width();
            // Trails the title so the due gutter and Subtask indent stay
            // aligned. Driven by the cheap predicate, not by collecting the
            // URLs: this runs for every visible row on every frame.
            let notes = t.notes.as_deref().unwrap_or_default();
            let has_urls = links::has_openable_link(&t.links, notes);
            let marker = link_marker(has_urls, ascii);
            if let Some(marker) = marker {
                // Inherits the row's style, so on a Completed Task it reads dim
                // and struck-through with the title — its links still open.
                spans.push(Span::styled(marker, style));
            }
            // One scan of the body: the first reader-visible line drives both the
            // `≡` mark and the preview below. `⧉` and `≡` are the same class of
            // thing — facts about this Task's own text — so they read at the same
            // brightness, and a row with links carries both: `u` has something to
            // open, `n` has something to read.
            let preview_line = notes_preview_line(notes);
            let notes_mark = notes_marker(preview_line.is_some(), ascii);
            if let Some(notes_mark) = notes_mark {
                spans.push(Span::styled(notes_mark, style));
            }
            let marker_width =
                marker.map_or(0, |m| m.width()) + notes_mark.map_or(0, |m| m.width());
            // The Subtask meter trails both markers, because they belong to this
            // Task's own text while the meter summarises the rows beneath it.
            // Neither marker is dropped for the meter's sake: they are not this
            // widget's information to spend — so both widths come off its budget,
            // or it would lay itself out over cells the row has already spent.
            // A flat pane has no Subtask meter, so it is skipped there.
            let segment = if flat {
                String::new()
            } else {
                subtask_segment(
                    subtask_counts.get(&t.id).copied(),
                    area.width,
                    due_gutter,
                    drawn_width,
                    marker_width,
                    ascii,
                )
            };
            let meter_width = segment.width();
            if !segment.is_empty() {
                // The row's style *minus* the strike: braille struck through is
                // unreadable, but dropping the style outright would leave the
                // meter the brightest thing on a deliberately dimmed row.
                spans.push(Span::styled(
                    segment,
                    style.remove_modifier(Modifier::CROSSED_OUT),
                ));
            }
            // Flat panes only: the List name, trailing the markers/meter. Painted
            // `muted` — a step below the `subtext` preview that follows it, so the
            // two tails do not compete: the name is context about the row, not the
            // row's own text. Its width comes off the notes-preview budget below
            // (like the meter's), so the variable-length preview tail can never
            // clip it. Strike removed — it is stable context, not the Task's own
            // struck-through text.
            let list_seg = list_titles.get(&t.list).map(|name| format!("  {name}"));
            let list_seg_width = list_seg.as_ref().map_or(0, |s| s.width());
            if let Some(seg) = list_seg {
                spans.push(Span::styled(
                    seg,
                    style.remove_modifier(Modifier::CROSSED_OUT).fg(theme.muted),
                ));
            }
            // Last of all, after the meter, so this variable-length tail can never
            // clip a bounded widget. Dim prose (`subtext`), and the strike is
            // *kept* on a Completed row — struck prose stays legible, the opposite
            // of the meter just above, whose braille it would render unreadable.
            if let Some(line) = preview_line {
                if let Some(preview) = notes_preview_segment(
                    line,
                    area.width,
                    due_gutter,
                    is_subtask,
                    drawn_width + marker_width + meter_width + list_seg_width,
                    ascii,
                ) {
                    spans.push(Span::styled(preview, style.fg(theme.subtext)));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // `selected_task` indexes `tasks`; translate it to the cursor's position in
    // the displayed (sorted) order so the highlight tracks the same Task by id.
    let selected = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .and_then(|sel| ordered.iter().position(|t| t.id == sel.id));

    // A spread interleaves the journal spread's header rows, which shifts every
    // display index below them — so the cursor is translated in the same place
    // the rows are inserted, and cannot be left behind.
    let (items, selected) = if spread {
        journal_spread(items, selected, &ordered, overdue_rows, today, theme)
    } else {
        (items, selected)
    };

    let base = format!("Tasks — {}", model.sort.label());
    // Inline btop-style data widgets in the header: a completion meter for the
    // active List and a due-load strip. Both drop out (never the text) when the
    // pane is too narrow — braille degrades before the title (ADR-0006).
    let inner_width = area.width.saturating_sub(PANEL_BORDERS);
    let title = header_title(&base, model, inner_width, ascii);
    render_selectable(frame, area, &title, items, selected, focused, theme);
}

/// How the spread's dateline renders the day: `Wednesday 30 September 2026`.
/// Unpadded day-of-month (`%-d`), because a journal page writes "1 July", not
/// "01 July".
const DATELINE_FORMAT: &str = "%A %-d %B %Y";

/// Interleave Today's journal-spread header rows into the Task rows, and shift
/// the cursor to match.
///
/// Three non-selectable rows, in the same `List` widget as the Tasks: the
/// dateline, then an `Overdue` and a `Today` group header. One widget, not a
/// `Paragraph` above the pane — a second widget would need its own scroll, and
/// would detach a label from the rows it heads the moment the pane moved.
///
/// A group header is drawn when its group has **rows**; its count is only the
/// rows still `needsAction`. The two rules differ deliberately: membership must
/// be status-blind for the Overdue prefix to hold (see [`due_before`]), while the
/// count answers the migration ritual's question — what is *left* to move — so a
/// struck-through row is not in it. At zero outstanding the count and the urgent
/// colour both drop: the rows are settled, and nothing is owed.
///
/// The dateline is drawn even on an empty day. It is the page, not a label for
/// the rows.
fn journal_spread<'a>(
    rows: Vec<ListItem<'a>>,
    selected: Option<usize>,
    ordered: &[&Task],
    overdue_rows: usize,
    today: NaiveDate,
    theme: &Theme,
) -> (Vec<ListItem<'a>>, Option<usize>) {
    debug_assert_eq!(rows.len(), ordered.len(), "one row per displayed Task");
    let (overdue, rest) = ordered.split_at(overdue_rows);
    let outstanding = |group: &[&Task]| {
        group
            .iter()
            .filter(|t| t.status != Status::Completed)
            .count()
    };

    let mut out = Vec::with_capacity(rows.len() + 3);
    out.push(ListItem::new(Line::from(Span::styled(
        today.format(DATELINE_FORMAT).to_string(),
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
    ))));
    let mut rows = rows.into_iter();
    if !overdue.is_empty() {
        out.push(spread_header("Overdue", outstanding(overdue), true, theme));
        out.extend(rows.by_ref().take(overdue.len()));
    }
    if !rest.is_empty() {
        out.push(spread_header("Today", outstanding(rest), false, theme));
        out.extend(rows);
    }

    // The dateline sits above every row; the `Overdue` header above every row
    // when it exists at all; the `Today` header only above the rows past the
    // prefix. A cursor at `p` is pushed down by however many of those precede it.
    let selected =
        selected.map(|p| p + 1 + usize::from(!overdue.is_empty()) + usize::from(p >= overdue_rows));
    (out, selected)
}

/// One group header of the journal spread: a bold label, then the count of rows
/// still owed. `urgent` paints a non-zero count's label in the palette's overdue
/// red — the same colour the dates below it carry — so the migration worklist
/// announces itself; a spent group falls back to the dim label every other header
/// wears. The count is omitted entirely at zero rather than printed as `0`.
fn spread_header(
    label: &'static str,
    outstanding: usize,
    urgent: bool,
    theme: &Theme,
) -> ListItem<'static> {
    let fg = if urgent && outstanding > 0 {
        theme.overdue
    } else {
        theme.subtext
    };
    let mut spans = vec![Span::styled(
        label,
        Style::new().fg(fg).add_modifier(Modifier::BOLD),
    )];
    if outstanding > 0 {
        spans.push(Span::styled(
            format!(" {outstanding}"),
            Style::new().fg(theme.subtext),
        ));
    }
    ListItem::new(Line::from(spans))
}

/// Compose the task-pane header: the base title, then — only while they fit — a
/// completion meter (`done/total` of the active List) and a due-load strip.
/// Widgets are added greedily and dropped before the text on a narrow pane.
fn header_title(base: &str, model: &Model, inner_width: u16, ascii: bool) -> String {
    let inner = inner_width as usize;
    let mut title = base.to_string();

    // The active title/notes filter (`/`), shown before the optional data widgets
    // so a narrowed — or empty — pane always says why. A caret trails the query
    // only while the input is open (`Overlay::Filter`), distinguishing a live edit
    // from a committed filter. Appended unconditionally like the base title: it is
    // state, not a droppable widget, so the braille meter and strip below degrade
    // before it if the pane is narrow.
    if let Some(query) = &model.filter {
        let caret = if matches!(model.overlay, Some(Overlay::Filter)) {
            "▏"
        } else {
            ""
        };
        title.push_str(&format!("  /{query}{caret}"));
    }

    // Completion meter over Task-typed entries only: Events and Notes are not work
    // you complete, so counting them would make the meter permanently under-report.
    // Numerator and denominator come from the *same* set — a completed Note counts
    // in neither — or the label could read "4/3" while the bar clamped to full.
    //
    // A pane holding only Events and Notes therefore shows no meter at all, via the
    // `total > 0` guard: there is no completion to report. In Today the count also
    // honours membership (`due <= today`), so a row optimistically migrated past
    // today leaves the meter in the same frame it leaves the pane.
    //
    // Membership is the only view filter it honours. The meter reports over the
    // whole `due <= today` aggregate, so it deliberately counts Completed rows the
    // pane no longer draws — `Model::within_completion_day` hides a row completed
    // on an earlier day, and that row stays in this ratio. Today's completion is a
    // property of the day's workload, not of what survives the view filters.
    let today = model.now.date_naive();
    let today_active = model.today_active();
    let actionable = || {
        model
            .tasks
            .iter()
            .filter(|t| t.entry_type() == EntryType::Task)
            .filter(move |t| !today_active || due_on_or_before(t.due, today))
    };
    let total = actionable().count();
    if total > 0 {
        let done = actionable()
            .filter(|t| t.status == Status::Completed)
            .count();
        let bar = meter::render(done, total, HEADER_METER_WIDTH, ascii);
        let segment = format!("  {bar} {done}/{total}");
        if title.chars().count() + segment.chars().count() <= inner {
            title.push_str(&segment);
        }
    }

    // Due-load strip: workload ahead over the next `DUE_LOAD_DAYS` days. Dropped
    // in Today — every row there is due<=today, so the strip would fold the whole
    // pane into a single "today" bucket and forecast nothing. The completion meter
    // above stays: it reports today's actionable completion over the whole
    // `due <= today` aggregate (not over the rows the pane draws — see there).
    let counts = if model.today_active() {
        vec![0; DUE_LOAD_DAYS]
    } else {
        due_load_counts(&model.tasks, model.now, DUE_LOAD_DAYS)
    };
    if counts.iter().any(|&c| c > 0) {
        let strip = dueload::render(&counts, ascii);
        let segment = format!("  {strip}");
        if title.chars().count() + segment.chars().count() <= inner {
            title.push_str(&segment);
        }
    }

    title
}

/// Bucket incomplete entries by due date into `days` daily buckets of "workload
/// ahead": `[0]` = due today (and anything overdue, folded forward), `[1]` =
/// tomorrow, ... Completed entries and those with no due date are excluded.
///
/// Notes are excluded too: the strip forecasts work, and a Note is not work.
/// Events are counted — they occupy a day even though you never complete them.
///
/// This is deliberately narrower than the due gutter beside each row, which
/// shows a date for *any* dated entry including a Note. The gutter answers "does
/// this carry a date?"; the strip answers "how much is coming?".
fn due_load_counts(
    tasks: &[Task],
    now: chrono::DateTime<chrono::Local>,
    days: usize,
) -> Vec<usize> {
    let today = now.date_naive();
    let mut counts = vec![0usize; days];
    for task in tasks {
        if task.status == Status::Completed || task.entry_type() == EntryType::Note {
            continue;
        }
        let Some(due) = task.due else { continue };
        let delta = (due - today).num_days();
        // Overdue folds into today's load; beyond the window is ignored.
        let bucket = delta.max(0) as usize;
        if bucket < days {
            counts[bucket] += 1;
        }
    }
    counts
}

/// The cursor gutter `render_selectable` puts before every row, focused or not.
/// Callers computing how much room a row's text really has must subtract it —
/// otherwise ratatui clips the overflow silently, with no ellipsis to show for it.
/// The two must stay the same width; `the_cursor_gutter_is_the_same_width_either_way`
/// pins that.
const LIST_CURSOR: &str = "› ";
const LIST_CURSOR_BLANK: &str = "  ";

/// A rounded, focus-aware panel wrapping a selectable list. The selection is
/// highlighted strongly when the pane is focused, faintly when it isn't — so
/// both the focused pane and the cursor are always visible.
fn render_selectable(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<ListItem>,
    selected: Option<usize>,
    focused: bool,
    theme: &Theme,
) {
    let highlight = if focused {
        Style::new()
            .fg(theme.accent)
            .add_modifier(Modifier::REVERSED)
    } else {
        Style::new().fg(theme.accent)
    };
    let list = List::new(items)
        .block(panel(title, focused, theme))
        .style(Style::new().bg(theme.base).fg(theme.text))
        .highlight_style(highlight)
        .highlight_symbol(if focused {
            LIST_CURSOR
        } else {
            LIST_CURSOR_BLANK
        });

    let mut state = ListState::default();
    state.select(selected);
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_status(frame: &mut Frame, area: Rect, model: &Model, theme: &Theme) {
    let text = model.status_line.as_deref().unwrap_or("");
    frame.render_widget(
        Paragraph::new(text).style(Style::new().bg(theme.base).fg(theme.subtext)),
        area,
    );
}

/// Columns between adjacent legend cells, and between the last cell and the
/// pinned help. Matches the `"  "` joins the pane header uses.
const LEGEND_GAP: usize = 2;

/// Rendered width of the pinned help cell. A function, not a `const`, because
/// `str::chars` is not a const fn — and deriving it beats writing `6`, which
/// would drift the moment the label changes.
fn help_width() -> usize {
    keymap::HELP.text().chars().count()
}

/// Which legend the model's state calls for. An open overlay wins over pane
/// focus: `update` routes keys to `overlay_key` before the keymap, so every
/// pane verb is false while one is up.
///
/// `show_help` is deliberately not consulted — it is a plain flag that does not
/// gate `keymap::resolve`, so the pane's verbs keep firing underneath the
/// cheatsheet and the legend stays true.
fn legend_context(model: &Model) -> keymap::LegendContext {
    // Listed per variant rather than caught by a `Some(_)` arm: a new overlay
    // must be made to declare its legend, not silently inherit the text-input
    // one and advertise keys it does not have.
    match &model.overlay {
        Some(Overlay::Confirm(_)) => keymap::LegendContext::Confirm,
        Some(Overlay::OpenLink { .. }) => keymap::LegendContext::LinkPicker,
        Some(Overlay::MoveToList { .. }) => keymap::LegendContext::ListPicker,
        Some(Overlay::Filter) => keymap::LegendContext::Filter,
        // The add-entry captures parse a trailing date and bind `Tab` for a
        // literal submit, so they get their own legend rather than `TextInput`'s.
        Some(Overlay::AddTask { .. } | Overlay::AddSubtask { .. }) => {
            keymap::LegendContext::TaskCapture
        }
        Some(
            Overlay::EditTitle { .. }
            | Overlay::EditDue { .. }
            | Overlay::EditNotes { .. }
            | Overlay::AddList { .. }
            | Overlay::RenameList { .. },
        ) => keymap::LegendContext::TextInput,
        None => match model.focus {
            Focus::Tasks => keymap::LegendContext::Tasks,
            Focus::Sidebar => keymap::LegendContext::Sidebar,
        },
    }
}

/// The leading cells that fit in `width` once `reserved` columns are spoken
/// for, and the columns they occupy. Cells are taken left to right — their
/// order is their priority — and the first that does not fit stops the run, so
/// the tail drops whole rather than truncating mid-word or back-filling with a
/// shorter cell further down.
///
/// The width comes back with the slice so the caller never re-derives it: two
/// copies of this arithmetic would be two things to keep in step.
///
/// `reserved` covers the pinned help *and* the gap before it; overlay contexts
/// pass 0, so they are charged for neither.
fn fit_legend(
    cells: &[keymap::LegendEntry],
    reserved: usize,
    width: usize,
) -> (&[keymap::LegendEntry], usize) {
    // Saturating: a bare subtraction underflows for any width below `reserved`,
    // which in release would wrap to a budget large enough to "fit" everything.
    let budget = width.saturating_sub(reserved);
    let mut used = 0;
    let mut taken = 0;
    for cell in cells {
        let cost = cell.text().chars().count() + if taken == 0 { 0 } else { LEGEND_GAP };
        if used + cost > budget {
            break;
        }
        used += cost;
        taken += 1;
    }
    (&cells[..taken], used)
}

/// A cell as two spans: the keys in the accent colour, the label dimmer, so the
/// row scans as keys first.
fn legend_cell_spans(cell: &keymap::LegendEntry, theme: &Theme) -> [Span<'static>; 2] {
    [
        Span::styled(cell.key_text(), Style::new().fg(theme.accent)),
        Span::styled(format!(" {}", cell.label), Style::new().fg(theme.subtext)),
    ]
}

/// The legend row: the cells that fit, then — in pane contexts — the help cell
/// pushed flush against the right edge, which keeps it in one place as the
/// terminal resizes and cells drop away.
///
/// No leading space: `render_status` draws flush at column 0 and the panels
/// above start there too, so an indent here would sit visibly out of line.
fn legend_spans(
    cells: &[keymap::LegendEntry],
    pinned: bool,
    width: usize,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let reserved = if pinned { help_width() + LEGEND_GAP } else { 0 };
    let (fitted, used) = fit_legend(cells, reserved, width);

    let mut spans = Vec::new();
    for cell in fitted {
        if !spans.is_empty() {
            spans.push(Span::raw(" ".repeat(LEGEND_GAP)));
        }
        spans.extend(legend_cell_spans(cell, theme));
    }

    // Below the help cell's own width there is nowhere to put it; the row then
    // carries whatever cells fit, or nothing at all.
    if !pinned || width < help_width() {
        return spans;
    }
    let pad = width.saturating_sub(help_width() + used);
    spans.push(Span::raw(" ".repeat(pad)));
    spans.extend(legend_cell_spans(&keymap::HELP, theme));
    spans
}

/// The always-visible hotkey legend.
fn render_legend(frame: &mut Frame, area: Rect, model: &Model, theme: &Theme) {
    let context = legend_context(model);
    // Overlays get no pinned help: `?` would type a literal `?` into the buffer
    // rather than opening the cheatsheet.
    let pinned = matches!(
        context,
        keymap::LegendContext::Tasks | keymap::LegendContext::Sidebar
    );
    let spans = legend_spans(keymap::legend(context), pinned, area.width as usize, theme);
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::new().bg(theme.base).fg(theme.subtext)),
        area,
    );
}

/// Columns between adjacent cheatsheet cells. One quantity, three readers:
/// `HelpLayout::total`, the gap span in `draw_help`, and the offset delta the
/// render tests assert.
const HELP_COL_GAP: usize = 1;

/// A cheatsheet row: the key label(s) and the help text they trigger.
type HelpRow = (String, &'static str);

/// The widest label and help in one cheatsheet column.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ColumnWidths {
    label: usize,
    help: usize,
}

impl ColumnWidths {
    /// The cell this column draws: `" {label} {help}"`.
    fn cell(&self) -> usize {
        1 + self.label + 1 + self.help
    }
}

/// How the `?` cheatsheet is laid out in a given frame.
///
/// Sized against the frame, never against the row count — that inversion is the
/// whole point of the type. `hidden` and `truncated` report what did not fit on
/// each axis, so overflow is announced rather than silently clipped.
#[derive(Debug, Default, PartialEq, Eq)]
struct HelpLayout {
    /// One entry per column; `cols.len()` *is* the column count.
    cols: Vec<ColumnWidths>,
    rows_per_col: usize,
    /// The drawn size, already clamped to the frame.
    width: u16,
    height: u16,
    /// Rows that did not fit vertically.
    hidden: usize,
    /// Whether the columns are wider than the frame allows.
    truncated: bool,
}

impl HelpLayout {
    /// The rows drawn in column `c` — the single definition of the partition.
    ///
    /// Bounded at both ends: never spills past `rows_per_col`, never past the
    /// slice. `help_layout` derives `cols` through this, and `draw_help` draws
    /// through it, so the widths and the grid cannot disagree.
    fn column_rows<'a>(&self, c: usize, rows: &'a [HelpRow]) -> &'a [HelpRow] {
        let start = (c * self.rows_per_col).min(rows.len());
        let end = start.saturating_add(self.rows_per_col).min(rows.len());
        &rows[start..end]
    }

    /// Total drawn width of the columns, gaps included, borders excluded.
    fn total(&self) -> usize {
        let cells: usize = self.cols.iter().map(ColumnWidths::cell).sum();
        cells + self.cols.len().saturating_sub(1) * HELP_COL_GAP
    }
}

/// A candidate layout for `n` columns: the partition and the widths it implies.
///
/// `column_rows` is `&self` and the layout does not exist yet, but it reads only
/// `rows_per_col` — so the provisional value below is enough to derive `cols`
/// through the same accessor the renderer uses. Slicing inline here instead
/// would be a second definition of the partition, free to drift from the first.
fn candidate(n: usize, rows: &[HelpRow], inner_h: usize) -> HelpLayout {
    let mut layout = HelpLayout {
        rows_per_col: rows.len().div_ceil(n).min(inner_h),
        ..HelpLayout::default()
    };
    layout.cols = (0..n)
        .map(|c| {
            let slice = layout.column_rows(c, rows);
            ColumnWidths {
                label: slice
                    .iter()
                    .map(|(label, _)| label.chars().count())
                    .max()
                    .unwrap_or(0),
                help: slice
                    .iter()
                    .map(|(_, help)| help.chars().count())
                    .max()
                    .unwrap_or(0),
            }
        })
        .collect();
    layout
}

/// Lay the cheatsheet out for `area`, in as many columns as the frame allows.
///
/// Picks the fewest columns that fit the rows vertically, then narrows until
/// they fit horizontally. Whatever still does not fit is reported — `hidden`
/// rows and `truncated` text — never quietly dropped.
fn help_layout(area: Rect, rows: &[HelpRow]) -> HelpLayout {
    let inner_w = area.width.saturating_sub(2) as usize;
    let inner_h = area.height.saturating_sub(2) as usize;

    // Nothing to draw, or nowhere to draw it. Either way the popup shrinks to
    // its borders — clamped, so a frame too small for even those still fits —
    // and reports whatever it could not show.
    if inner_w == 0 || inner_h == 0 || rows.is_empty() {
        return HelpLayout {
            hidden: rows.len(),
            truncated: !rows.is_empty(),
            width: 2.min(area.width),
            height: 2.min(area.height),
            ..HelpLayout::default()
        };
    }

    let cols_by_height = rows.len().div_ceil(inner_h);
    let mut layout = (1..=cols_by_height)
        .rev()
        .map(|n| candidate(n, rows, inner_h))
        .find(|c| c.total() <= inner_w)
        .unwrap_or_else(|| candidate(1, rows, inner_h));

    let shown = rows.len().min(layout.cols.len() * layout.rows_per_col);
    layout.hidden = rows.len() - shown;
    layout.truncated = layout.total() > inner_w;
    // The clamp is load-bearing: when truncated, `total()` exceeds the frame by
    // construction, and handing `centered` an oversized rect is precisely the
    // silent clip this layout exists to remove.
    layout.width = (layout.total() + 2).min(area.width as usize) as u16;
    layout.height = (layout.rows_per_col + 2).min(area.height as usize) as u16;
    layout
}

/// What the popup could not show, as a line for its bottom border.
///
/// Deliberately terse: at 30 columns there are 28 cells to say it in, so a
/// fuller sentence would itself be truncated by the popup it is warning about.
fn overflow_notice(layout: &HelpLayout) -> Option<String> {
    match (layout.hidden, layout.truncated) {
        (0, false) => None,
        (0, true) => Some("clipped".to_string()),
        (n, false) => Some(format!("+{n} more")),
        (n, true) => Some(format!("+{n} more, clipped")),
    }
}

/// The two spans of one cheatsheet cell: accented keys, then the help text.
///
/// Both are padded to the column's width — the label so the help columns line
/// up, the help so the *next* column starts at a fixed x. Two spans rather than
/// one formatted string because the accent is per-span; collapsing them would
/// lose it silently.
fn help_cell_spans(
    row: &HelpRow,
    widths: &ColumnWidths,
    last_col: bool,
    theme: &Theme,
) -> [Span<'static>; 2] {
    let (label, help) = row;
    [
        Span::styled(
            format!(" {label:<width$} ", width = widths.label),
            Style::new().fg(theme.accent),
        ),
        Span::styled(
            if last_col {
                // Padding the final column would only add trailing blanks.
                (*help).to_string()
            } else {
                format!("{help:<width$}", width = widths.help)
            },
            Style::new().fg(theme.text),
        ),
    ]
}

fn render_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    draw_help(frame, area, &keymap::cheatsheet_rows(), theme);
}

/// Draw the cheatsheet popup over `area`.
///
/// Split out from `render_help` so tests can supply their own rows: the column
/// partition only goes ragged when the row count is not a multiple of the column
/// count, which the real table need not exhibit today.
fn draw_help(frame: &mut Frame, area: Rect, rows: &[HelpRow], theme: &Theme) {
    let layout = help_layout(area, rows);
    let last_col = layout.cols.len().saturating_sub(1);

    // Row-major over the columns, so column-major reading order comes out of a
    // row-wise draw. `get` rather than `zip`: only the last column can be short,
    // and a `zip` would stop at it and silently drop the final row.
    let lines: Vec<Line> = (0..layout.rows_per_col)
        .map(|r| {
            let mut spans: Vec<Span> = Vec::new();
            for (c, widths) in layout.cols.iter().enumerate() {
                if let Some(row) = layout.column_rows(c, rows).get(r) {
                    if !spans.is_empty() {
                        spans.push(Span::raw(" ".repeat(HELP_COL_GAP)));
                    }
                    spans.extend(help_cell_spans(row, widths, c == last_col, theme));
                }
            }
            Line::from(spans)
        })
        .collect();

    let mut block = panel("Help", true, theme);
    if let Some(notice) = overflow_notice(&layout) {
        block = block.title_bottom(notice);
    }

    let popup = centered(area, layout.width, layout.height);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(lines).block(block), popup);
}

/// A rounded-border panel titled `title`, its border accented when `focused`.
fn panel<'a>(title: &'a str, focused: bool, theme: &Theme) -> Block<'a> {
    let border_color = if focused { theme.accent } else { theme.surface };
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(theme.base).fg(theme.text))
}

/// A centered rectangle `width`×`height` cells inside `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let [row] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    let [cell] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(row);
    cell
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{ListId, TaskId};
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::Terminal;

    /// The smallest terminal oxidone supports; the cheatsheet must fit it whole.
    ///
    /// A test fixture rather than a `const` in the module above: nothing on the
    /// production path reads it — `help_layout` takes whatever frame it is given
    /// — so up there it would be dead code. The contract itself is stated in the
    /// module doc and the README.
    const MIN_TERM: (u16, u16) = (80, 24);

    fn frame_of(size: (u16, u16)) -> Rect {
        Rect::new(0, 0, size.0, size.1)
    }

    fn help_rows() -> Vec<HelpRow> {
        keymap::cheatsheet_rows()
    }

    /// `n` synthetic rows, wide enough to behave like the real ones.
    fn synthetic_rows(n: usize) -> Vec<HelpRow> {
        (0..n)
            .map(|i| (format!("k{i}"), "some help text"))
            .collect()
    }

    // --- The `?` cheatsheet layout ---------------------------------------

    #[test]
    fn the_whole_cheatsheet_fits_the_smallest_supported_terminal() {
        // The gate. When the binding table outgrows the popup, this fails
        // rather than the surplus rows quietly vanishing off the bottom.
        let rows = help_rows();
        let layout = help_layout(frame_of(MIN_TERM), &rows);

        assert_eq!(layout.hidden, 0, "rows dropped at {MIN_TERM:?}");
        assert!(!layout.truncated, "help text clipped at {MIN_TERM:?}");
    }

    #[test]
    fn a_tall_frame_collapses_to_a_single_column() {
        let rows = help_rows();
        let layout = help_layout(Rect::new(0, 0, 80, 40), &rows);

        assert_eq!(layout.cols.len(), 1);
        assert_eq!(layout.rows_per_col, rows.len());
        assert_eq!(layout.hidden, 0);
    }

    #[test]
    fn a_wide_short_frame_reaches_for_a_third_column() {
        // The branch that separates an uncapped search from a fixed cap of two.
        // Reachable on a real terminal, so it is exercised on the real table.
        let rows = help_rows();
        let layout = help_layout(Rect::new(0, 0, 120, 14), &rows);

        assert_eq!(layout.cols.len(), 3);
        assert_eq!(layout.hidden, 0);
        assert!(!layout.truncated);

        // Column lengths derived, never restated: the last is the short one.
        let lengths: Vec<usize> = (0..layout.cols.len())
            .map(|c| layout.column_rows(c, &rows).len())
            .collect();
        assert_eq!(lengths.iter().sum::<usize>(), rows.len());
        assert!(lengths[lengths.len() - 1] <= layout.rows_per_col);
    }

    #[test]
    fn a_narrow_frame_reports_both_overflows() {
        // Below the single-column minimum: the help text cannot fit the width,
        // and the rows cannot fit the height either.
        let rows = help_rows();
        let layout = help_layout(Rect::new(0, 0, 30, 24), &rows);

        assert_eq!(layout.cols.len(), 1);
        assert!(layout.truncated);
        assert_eq!(layout.hidden, rows.len() - layout.rows_per_col);
        assert!(
            layout.width <= 30 && layout.height <= 24,
            "popup exceeds frame"
        );
    }

    #[test]
    fn degenerate_frames_produce_a_layout_rather_than_a_panic() {
        let rows = help_rows();

        // 1x1: no room for even the borders — the early return.
        let tiny = help_layout(Rect::new(0, 0, 1, 1), &rows);
        assert!(tiny.cols.is_empty());
        assert_eq!(tiny.rows_per_col, 0);
        assert_eq!(tiny.hidden, rows.len());
        assert!(tiny.truncated);

        // An empty table is not the same as no room: the popup collapses to
        // its borders rather than spreading over the whole frame.
        let empty = help_layout(frame_of(MIN_TERM), &[]);
        assert_eq!((empty.width, empty.height), (2, 2));
        assert_eq!(empty.hidden, 0);
        assert!(!empty.truncated);

        // 4x3: one row of two cells to draw into, so nothing fits but the
        // layout still describes a single column.
        let small = help_layout(Rect::new(0, 0, 4, 3), &rows);
        assert_eq!(small.cols.len(), 1);
        assert_eq!(small.rows_per_col, 1);
        assert_eq!(small.hidden, rows.len() - 1);
        assert!(small.truncated);
    }

    #[test]
    fn the_partition_covers_every_row_without_overlapping() {
        // `column_rows` is the only definition of the split, so its bounds are
        // worth pinning directly — including when the row count is not a
        // multiple of the column count and the last column comes up short.
        for n in [7usize, 25, 26, 27] {
            let rows = synthetic_rows(n);
            let layout = help_layout(frame_of(MIN_TERM), &rows);

            let mut seen = 0;
            for c in 0..layout.cols.len() {
                let slice = layout.column_rows(c, &rows);
                assert!(slice.len() <= layout.rows_per_col);
                seen += slice.len();
            }
            assert_eq!(seen + layout.hidden, n, "{n} rows: partition lost some");
        }
    }

    // --- The `?` cheatsheet, as actually drawn ---------------------------
    //
    // The layout above can be right while the draw is wrong: a dropped span, a
    // missing pad, a partition recomputed differently. These go through a real
    // backend and read the buffer back.

    /// Draw `rows` into a `size` frame and return the buffer, line by line,
    /// alongside the layout that produced it.
    fn drawn(size: (u16, u16), rows: &[HelpRow]) -> (Vec<String>, Buffer, HelpLayout) {
        let (width, height) = size;
        let mut terminal =
            Terminal::new(TestBackend::new(width, height)).expect("TestBackend terminal");
        let theme = Theme::from_flavor("mocha");
        terminal
            .draw(|frame| draw_help(frame, frame_of(size), rows, &theme))
            .expect("draw");

        let buffer = terminal.backend().buffer().clone();
        let lines = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect();
        (lines, buffer, help_layout(frame_of(size), rows))
    }

    /// The cell text column `c` draws for `row`, padding included.
    fn cell_text(row: &HelpRow, layout: &HelpLayout, c: usize) -> String {
        let widths = &layout.cols[c];
        let (label, help) = row;
        let last = c + 1 == layout.cols.len();
        let help = if last {
            help.to_string()
        } else {
            format!("{help:<width$}", width = widths.help)
        };
        format!(" {label:<width$} {help}", width = widths.label)
    }

    /// Where `needle` starts in `line`, counted in cells rather than bytes.
    ///
    /// `str::find` answers in bytes, and the popup's border glyphs are three
    /// bytes each — so a byte offset indexes the wrong buffer cell as soon as
    /// anything non-ASCII precedes the match.
    fn cell_offset(line: &str, needle: &str) -> Option<usize> {
        line.find(needle).map(|byte| line[..byte].chars().count())
    }

    /// Where each column's cells start, discovered from the buffer rather than
    /// recomputed from `centered`'s arithmetic — a test that recomputed the
    /// origin would agree with a renderer that placed the popup wrongly.
    fn column_offsets(lines: &[String], layout: &HelpLayout, rows: &[HelpRow]) -> Vec<usize> {
        (0..layout.cols.len())
            .map(|c| {
                let first = &layout.column_rows(c, rows)[0];
                let needle = cell_text(first, layout, c);
                lines
                    .iter()
                    .find_map(|line| cell_offset(line, &needle))
                    .unwrap_or_else(|| panic!("column {c} not found in the buffer"))
            })
            .collect()
    }

    #[test]
    fn every_row_is_drawn_padded_and_column_aligned() {
        let rows = help_rows();
        let (lines, _, layout) = drawn(MIN_TERM, &rows);
        assert_eq!(layout.hidden, 0, "fixture should show every row");

        // Content: every row present, with its padding, on a single line. A
        // whole-buffer join would let a needle straddle the column boundary.
        for c in 0..layout.cols.len() {
            for row in layout.column_rows(c, &rows) {
                let needle = cell_text(row, &layout, c);
                assert!(
                    lines.iter().any(|line| line.contains(&needle)),
                    "missing cell {needle:?}"
                );
            }
        }

        // Alignment: each column starts at one x, and the gap between them is
        // the width that was budgeted — not a ragged edge that happens to fit.
        let offsets = column_offsets(&lines, &layout, &rows);
        for (c, offset) in offsets.iter().enumerate() {
            for row in layout.column_rows(c, &rows) {
                let needle = cell_text(row, &layout, c);
                let found = lines
                    .iter()
                    .find_map(|line| cell_offset(line, &needle))
                    .expect("cell present");
                assert_eq!(found, *offset, "column {c} is ragged at {needle:?}");
            }
        }
        for c in 1..layout.cols.len() {
            assert_eq!(
                offsets[c] - offsets[c - 1],
                layout.cols[c - 1].cell() + HELP_COL_GAP,
                "gap between columns {} and {c} is not the budgeted width",
                c - 1
            );
        }
    }

    #[test]
    fn the_keys_keep_their_accent() {
        // Buffer text alone cannot tell two spans from one: collapsing the cell
        // into a single format string renders identically and loses the accent.
        //
        // Every column, every row — the last column takes the other branch of
        // `help_cell_spans` (its help is unpadded), so checking only the first
        // would leave that branch's styling unasserted.
        let rows = help_rows();
        let (lines, buffer, layout) = drawn(MIN_TERM, &rows);
        let theme = Theme::from_flavor("mocha");

        let offsets = column_offsets(&lines, &layout, &rows);
        for (c, offset) in offsets.iter().enumerate() {
            for row in layout.column_rows(c, &rows) {
                let needle = cell_text(row, &layout, c);
                let y = lines
                    .iter()
                    .position(|line| line.contains(&needle))
                    .expect("cell present") as u16;

                // The label sits one cell past the leading space; the help
                // follows the label's field and its separating space.
                let label_x = (offset + 1) as u16;
                let help_x = (offset + 1 + layout.cols[c].label + 1) as u16;
                assert_eq!(
                    buffer[(label_x, y)].fg,
                    theme.accent,
                    "column {c}: key label lost its accent at {needle:?}"
                );
                assert_eq!(
                    buffer[(help_x, y)].fg,
                    theme.text,
                    "column {c}: help text is not the body colour at {needle:?}"
                );
            }
        }
    }

    #[test]
    fn a_short_frame_draws_what_fits_and_says_what_it_dropped() {
        // Two columns with a hidden tail — the one regime where the layout's
        // partition and the renderer's could disagree without either looking
        // wrong on its own.
        let rows = help_rows();
        let size = (80, 12);
        let (lines, _, layout) = drawn(size, &rows);

        assert_eq!(layout.cols.len(), 2);
        assert!(layout.hidden > 0, "fixture should overflow vertically");

        // Each column draws exactly its share, one row per line, no more. The
        // popup's own height bounds it: borders plus `rows_per_col`, so a
        // renderer that sliced further would have nowhere to put the surplus.
        assert_eq!(layout.height as usize, layout.rows_per_col + 2);
        for c in 0..layout.cols.len() {
            let expected = layout.column_rows(c, &rows);
            let drawn_here = lines
                .iter()
                .filter(|line| {
                    expected
                        .iter()
                        .any(|row| line.contains(&cell_text(row, &layout, c)))
                })
                .count();
            assert_eq!(
                drawn_here,
                expected.len(),
                "column {c} drew the wrong number of rows"
            );
        }

        // The tail is absent, and the popup says so. Matched as a rendered
        // cell rather than a bare help string: help text is not unique enough
        // to search for on its own, so one row's help becoming a substring of
        // another's would fail this spuriously. A hidden row belongs to no
        // column, so it is checked against every column's padding — whichever
        // one a regression drew it in.
        let shown = layout.cols.len() * layout.rows_per_col;
        for row in &rows[shown..] {
            for c in 0..layout.cols.len() {
                let needle = cell_text(row, &layout, c);
                assert!(
                    !lines.iter().any(|line| line.contains(&needle)),
                    "hidden row {needle:?} was drawn in column {c}"
                );
            }
        }
        let notice = overflow_notice(&layout).expect("overflow at 80x12");
        assert!(
            lines.iter().any(|line| line.contains(&notice)),
            "notice {notice:?} never reached the buffer"
        );
    }

    #[test]
    fn a_narrow_frame_announces_the_clip_in_the_buffer() {
        let rows = help_rows();
        let size = (30, 24);
        let (lines, _, layout) = drawn(size, &rows);

        let notice = overflow_notice(&layout).expect("overflow at 30x24");
        assert!(
            lines.iter().any(|line| line.contains(&notice)),
            "notice {notice:?} never reached the buffer"
        );
    }

    #[test]
    fn a_ragged_last_column_still_draws_its_final_row() {
        // 27 rows over two columns gives 14 and 13. A `zip`-based assembly would
        // stop at the shorter column and drop row 27 — present in the layout,
        // absent from the screen, with the gate still green.
        let rows = synthetic_rows(27);
        let (lines, _, layout) = drawn(MIN_TERM, &rows);

        assert_eq!(layout.hidden, 0);
        assert!(
            layout.column_rows(layout.cols.len() - 1, &rows).len() < layout.rows_per_col,
            "fixture should leave the last column short"
        );

        let last = rows.last().expect("rows are not empty");
        let needle = cell_text(last, &layout, layout.cols.len() - 1);
        assert!(
            lines.iter().any(|line| line.contains(&needle)),
            "final row {needle:?} was dropped"
        );
    }

    #[test]
    fn degenerate_frames_draw_without_panicking() {
        // The layout tests cover the arithmetic; the panic risk is in the draw,
        // where an empty partition must not be indexed.
        let rows = help_rows();
        for size in [(1, 1), (4, 3)] {
            let _ = drawn(size, &rows);
        }
    }

    #[test]
    fn the_overflow_notice_names_the_axis_that_overflowed() {
        let fits = HelpLayout::default();
        assert_eq!(overflow_notice(&fits), None);

        let clipped = HelpLayout {
            truncated: true,
            ..HelpLayout::default()
        };
        assert_eq!(overflow_notice(&clipped).as_deref(), Some("clipped"));

        let dropped = HelpLayout {
            hidden: 6,
            ..HelpLayout::default()
        };
        assert_eq!(overflow_notice(&dropped).as_deref(), Some("+6 more"));

        let both = HelpLayout {
            hidden: 4,
            truncated: true,
            ..HelpLayout::default()
        };
        assert_eq!(overflow_notice(&both).as_deref(), Some("+4 more, clipped"));
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn task(due: Option<NaiveDate>, status: Status) -> Task {
        titled("t", due, status)
    }

    /// `task`, but with the raw title spelled out — so a fixture can carry a
    /// type prefix.
    fn titled(title: &str, due: Option<NaiveDate>, status: Status) -> Task {
        Task {
            id: TaskId("t".into()),
            list: ListId("l".into()),
            parent: None,
            title: title.into(),
            notes: None,
            status,
            due,
            completed_at: None,
            links: Vec::new(),
            position: "0".into(),
            etag: String::new(),
            updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }
    }

    #[test]
    fn a_past_due_date_reads_overdue() {
        let theme = Theme::from_flavor("mocha");
        let style = due_style(
            &task(Some(ymd(2026, 3, 9)), Status::NeedsAction),
            ymd(2026, 3, 10),
            &theme,
        );
        assert_eq!(style.fg, Some(theme.overdue));
    }

    #[test]
    fn today_and_later_stay_dim() {
        let theme = Theme::from_flavor("mocha");
        let today = ymd(2026, 3, 10);
        for due in [ymd(2026, 3, 10), ymd(2026, 3, 11)] {
            let style = due_style(&task(Some(due), Status::NeedsAction), today, &theme);
            assert_eq!(style.fg, Some(theme.subtext), "{due} should not be overdue");
        }
    }

    #[test]
    fn a_task_with_no_due_date_stays_dim() {
        let theme = Theme::from_flavor("mocha");
        let style = due_style(&task(None, Status::NeedsAction), ymd(2026, 3, 10), &theme);
        assert_eq!(style.fg, Some(theme.subtext));
    }

    #[test]
    fn completed_wins_over_overdue() {
        let theme = Theme::from_flavor("mocha");
        let style = due_style(
            &task(Some(ymd(2026, 3, 9)), Status::Completed),
            ymd(2026, 3, 10),
            &theme,
        );
        assert_eq!(style.fg, Some(theme.subtext));
    }

    // --- Legend: fitting, row assembly, and context ----------------------

    /// The row as the terminal would show it, assembled the same way
    /// `render_legend` assembles it — one path, so a test can't drift from
    /// what renders.
    fn legend_text(context: keymap::LegendContext, pinned: bool, width: usize) -> String {
        let theme = Theme::from_flavor("mocha");
        legend_spans(keymap::legend(context), pinned, width, &theme)
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    fn tasks_cells() -> &'static [keymap::LegendEntry] {
        keymap::legend(keymap::LegendContext::Tasks)
    }

    #[test]
    fn a_wide_terminal_fits_every_cell() {
        let cells = tasks_cells();
        assert_eq!(fit_legend(cells, 0, 500).0.len(), cells.len());
    }

    #[test]
    fn a_narrow_terminal_drops_from_the_right() {
        let cells = tasks_cells();
        let (fitted, _) = fit_legend(cells, help_width() + LEGEND_GAP, 40);
        assert!(fitted.len() < cells.len(), "expected cells to drop");
        // Whatever survives is the priority prefix, never a reshuffle.
        for (kept, original) in fitted.iter().zip(cells) {
            assert_eq!(kept.text(), original.text());
        }
    }

    #[test]
    fn widths_below_the_reserve_yield_no_cells_and_do_not_panic() {
        // This range is exactly where an unsaturated `width - reserved` would
        // underflow: a panic in debug, and in release a wrap to a budget large
        // enough to "fit" the whole table into a handful of columns.
        let reserved = help_width() + LEGEND_GAP;
        for width in 0..=reserved {
            assert!(
                fit_legend(tasks_cells(), reserved, width).0.is_empty(),
                "width {width} should fit nothing"
            );
        }
    }

    #[test]
    fn an_unpinned_context_is_charged_for_no_help_gap() {
        // Overlay rows draw no help cell, so they must not pay for the gap
        // before one. At this width the reserve is the only difference.
        let cells = keymap::legend(keymap::LegendContext::Confirm);
        let width = cells[0].text().chars().count();
        assert_eq!(fit_legend(cells, 0, width).0.len(), 1);
        assert!(fit_legend(cells, help_width() + LEGEND_GAP, width)
            .0
            .is_empty());
    }

    #[test]
    fn a_row_too_narrow_for_help_is_empty() {
        for width in 0..help_width() {
            assert_eq!(
                legend_text(keymap::LegendContext::Tasks, true, width),
                "",
                "width {width}"
            );
        }
    }

    #[test]
    fn a_row_that_fits_only_help_carries_it_alone() {
        // No cell fits yet, so the row is help and nothing else — still flush
        // right, which at widths above its own is padding, not an indent.
        for width in help_width()..=help_width() + LEGEND_GAP {
            let row = legend_text(keymap::LegendContext::Tasks, true, width);
            assert_eq!(row.chars().count(), width, "width {width}");
            assert_eq!(row.trim_start(), keymap::HELP.text(), "width {width}");
        }
    }

    #[test]
    fn help_is_pinned_flush_against_the_right_edge() {
        let row = legend_text(keymap::LegendContext::Tasks, true, 80);
        assert_eq!(row.chars().count(), 80);
        assert!(row.ends_with(&keymap::HELP.text()));
        assert!(!row.starts_with(' '), "no leading space");
    }

    #[test]
    fn an_overlay_context_maps_from_the_overlay_not_the_focus() {
        let mut model = Model::new();
        model.focus = Focus::Tasks;

        // The add-entry captures carry the date-parsing/`Tab`-literal legend, not
        // the plain text-input one.
        model.overlay = Some(Overlay::AddTask {
            buffer: String::new(),
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::TaskCapture);

        model.overlay = Some(Overlay::AddSubtask {
            parent: TaskId("p".into()),
            buffer: String::new(),
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::TaskCapture);

        // A different capture (edit due) keeps the plain text-input legend.
        model.overlay = Some(Overlay::EditDue {
            task: TaskId("t".into()),
            buffer: String::new(),
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::TextInput);

        model.overlay = Some(Overlay::Confirm(crate::app::Confirm {
            prompt: "sure?".into(),
            action: crate::app::ConfirmAction::DeleteList {
                list: ListId("l".into()),
            },
        }));
        assert_eq!(legend_context(&model), keymap::LegendContext::Confirm);

        // The picker has no text buffer either; a catch-all arm would have sent
        // it to `TextInput` and advertised `Enter save`.
        model.overlay = Some(Overlay::OpenLink {
            links: Vec::new(),
            selected: 0,
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::LinkPicker);

        // Same for the move-to-List picker: `Enter` moves rather than saves, and
        // it has no buffer for `TextInput`'s legend to describe.
        model.overlay = Some(Overlay::MoveToList {
            task: TaskId("t".into()),
            source: ListId("l".into()),
            targets: Vec::new(),
            selected: 0,
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::ListPicker);
    }

    #[test]
    fn the_link_marker_appears_only_when_there_is_something_to_open() {
        assert_eq!(link_marker(false, false), None);
        assert_eq!(link_marker(false, true), None);
        assert_eq!(link_marker(true, false), Some(" ⧉"));
        assert_eq!(link_marker(true, true), Some(" *"));
    }

    #[test]
    fn the_notes_marker_appears_only_when_there_is_something_to_read() {
        assert_eq!(notes_marker(false, false), None);
        assert_eq!(notes_marker(false, true), None);
        assert_eq!(notes_marker(true, false), Some(" ≡"));
        assert_eq!(notes_marker(true, true), Some(" ="));
    }

    #[test]
    fn both_markers_are_the_same_width_so_a_row_carrying_both_stays_predictable() {
        assert_eq!(notes_marker(true, false).map(str::width), Some(2));
        assert_eq!(notes_marker(true, true).map(str::width), Some(2));
        assert_eq!(
            notes_marker(true, false).map(str::width),
            link_marker(true, false).map(str::width),
        );
    }

    #[test]
    fn a_notes_body_of_nothing_visible_yields_no_preview_line() {
        // Each of these renders as blank, so no line is selected — and the marker,
        // which is `is_some()` of this, is absent too.
        for blank in [
            "",
            "   ",
            "\n\n",
            "\t",
            "\r\n  \r\n",
            "\u{202e}",             // a lone RLO
            "\u{2066}\u{2069}",     // LRI immediately closed
            " \u{200e}\n\u{061c} ", // whitespace and marks, several lines
        ] {
            assert!(
                notes_preview_line(blank).is_none(),
                "expected no visible content in {blank:?}",
            );
        }
    }

    #[test]
    fn a_notes_body_with_any_visible_character_yields_a_preview_line() {
        for body in [
            "buy milk",
            "\n\n  ring first\n",
            "\u{202e}reversed",       // hostile *and* visible: still content
            "e\u{301}",               // a combining mark is part of the text
            "❤\u{fe0f}",              // VS16 emoji
            "👩\u{200d}👩\u{200d}👧", // ZWJ sequence
            ".",
        ] {
            assert!(
                notes_preview_line(body).is_some(),
                "expected visible content in {body:?}"
            );
        }
    }

    #[test]
    fn the_preview_line_skips_a_hostile_only_line_for_a_later_prose_one() {
        // Selecting on `trim` would pick the RLO line (non-blank, sanitises to
        // spaces) and draw nothing; selecting on `is_invisible` falls through.
        assert_eq!(
            notes_preview_line("\u{202e}\n  \nring Bob"),
            Some("ring Bob")
        );
        assert_eq!(notes_preview_line("first line\nsecond"), Some("first line"));
    }

    #[test]
    fn is_layout_hostile_covers_controls_and_bidi_not_marks() {
        assert!(is_layout_hostile('\t')); // a tab expands to a tab stop
        assert!(is_layout_hostile('\u{7}')); // a C0 control
        assert!(is_layout_hostile('\u{202e}')); // RLO, reorders the row
        assert!(is_layout_hostile('\u{61c}')); // ALM: Cf that is_control misses
        assert!(!is_layout_hostile('\u{301}')); // combining acute: legitimate text
        assert!(!is_layout_hostile('a'));
        assert!(!is_layout_hostile(' '));
        // The invariant the single scan leans on: every hostile char is invisible.
        for c in ['\t', '\u{7}', '\u{202e}', '\u{61c}'] {
            assert!(
                is_layout_hostile(c) && is_invisible(c),
                "{c:?} must be both"
            );
        }
    }

    #[test]
    fn url_only_authority_shortens_only_a_whole_line_url() {
        assert_eq!(url_only_authority("https://a.dev/1"), Some("a.dev"));
        assert_eq!(
            url_only_authority("https://a.dev:8080/x"),
            Some("a.dev:8080")
        );
        // Prose beside the URL is not URL-only — shown as-is.
        assert_eq!(url_only_authority("see https://a.dev/1"), None);
        assert_eq!(url_only_authority("https://a.dev/1 and more"), None);
        // Nothing to shorten, or no authority to show.
        assert_eq!(url_only_authority("file:///x"), None);
        assert_eq!(url_only_authority("just prose"), None);
    }

    #[test]
    fn the_cursor_gutter_is_the_same_width_either_way() {
        // The picker's truncation budget subtracts `LIST_CURSOR`; if the blank
        // drifted wider, focused and unfocused rows would wrap differently.
        assert_eq!(LIST_CURSOR.width(), LIST_CURSOR_BLANK.width());
    }

    #[test]
    fn the_picker_is_as_tall_as_its_urls_plus_borders() {
        // Two is the smallest count that can occur — one URL opens directly.
        assert_eq!(picker_height(2, 24), 4);
        assert_eq!(picker_height(7, 24), 9);
    }

    #[test]
    fn the_picker_never_outgrows_the_frame() {
        assert_eq!(picker_height(40, 12), 12);
    }

    #[test]
    fn without_an_overlay_the_context_follows_the_focused_pane() {
        let mut model = Model::new();
        model.overlay = None;

        model.focus = Focus::Tasks;
        assert_eq!(legend_context(&model), keymap::LegendContext::Tasks);

        model.focus = Focus::Sidebar;
        assert_eq!(legend_context(&model), keymap::LegendContext::Sidebar);
    }

    #[test]
    fn the_cheatsheet_being_open_does_not_change_the_legend() {
        // `show_help` is a plain flag, not an Overlay: it does not gate
        // `keymap::resolve`, so the pane's verbs keep firing underneath it and
        // the legend must keep telling the truth.
        let mut model = Model::new();
        model.focus = Focus::Tasks;
        let before = legend_context(&model);
        model.show_help = true;
        assert_eq!(legend_context(&model), before);
    }

    /// Usable row width for a sidebar of `area_width`, mirroring what
    /// `sidebar_row` budgets against.
    fn sidebar_usable(area_width: u16) -> usize {
        (area_width.saturating_sub(PANEL_BORDERS) as usize) - LIST_CURSOR.width()
    }

    #[test]
    fn a_sidebar_row_right_aligns_its_meter() {
        let row = sidebar_row("Work", Some((3, 8)), 30, false);
        assert_eq!(row.width(), sidebar_usable(30));
        assert!(row.starts_with("Work"), "{row:?}");
        assert!(row.ends_with(" 3/8"), "{row:?}");
    }

    #[test]
    fn a_sidebar_row_without_counts_is_the_bare_title() {
        assert_eq!(sidebar_row("Work", None, 30, false), "Work");
    }

    #[test]
    fn a_sidebar_meter_drops_the_bar_before_the_numbers() {
        // Braille degrades before text (ADR-0006): at a width that cannot hold
        // both, the ratio is what survives — it carries the actual number.
        let title = "A fairly long list";
        let wide = sidebar_row(title, Some((3, 8)), 40, false);
        assert!(
            wide.contains('\u{2800}') || wide.contains('\u{28FF}'),
            "{wide:?}"
        );
        assert!(wide.ends_with(" 3/8"));

        let narrow = sidebar_row(title, Some((3, 8)), 28, false);
        assert!(narrow.ends_with("3/8"), "{narrow:?}");
        assert!(
            !narrow.contains('\u{2800}') && !narrow.contains('\u{28FF}'),
            "the bar should have gone first: {narrow:?}"
        );
    }

    #[test]
    fn a_sidebar_meter_that_cannot_fit_leaves_the_title_untouched() {
        // The sidebar has always let ratatui clip an over-long title; adding a
        // meter must not turn that into truncation performed here.
        let title = "A list whose name is far too long for this pane";
        assert_eq!(sidebar_row(title, Some((3, 8)), 24, false), title);
    }

    #[test]
    fn a_sidebar_meter_falls_back_to_ascii() {
        let row = sidebar_row("Work", Some((4, 8)), 30, true);
        assert!(row.contains('#') && row.contains('-'), "{row:?}");
        assert!(
            !row.contains('\u{2800}') && !row.contains('\u{28FF}'),
            "{row:?}"
        );
    }

    #[test]
    fn a_sidebar_meter_measures_wide_ratios() {
        // `103/247` is seven cells where `3/8` is three; a hardcoded width would
        // overrun the row here.
        let row = sidebar_row("Work", Some((103, 247)), 40, false);
        assert_eq!(row.width(), sidebar_usable(40));
        assert!(row.ends_with(" 103/247"), "{row:?}");
    }

    #[test]
    fn a_sidebar_row_never_exceeds_its_budget() {
        for width in 0u16..=40 {
            for counts in [None, Some((0, 0)), Some((3, 8)), Some((103, 247))] {
                for ascii in [false, true] {
                    let row = sidebar_row("Work", counts, width, ascii);
                    // Either the bare title (ratatui clips it, as before) or a
                    // composed row that fits exactly.
                    assert!(
                        row == "Work" || row.width() == sidebar_usable(width),
                        "width {width}, counts {counts:?}, ascii {ascii}: {row:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_subtask_meter_degrades_bar_then_numbers_then_away() {
        let full = subtask_segment(Some((2, 5)), 60, false, 10, 0, false);
        assert!(
            full.contains('\u{2800}') || full.contains('\u{28FF}'),
            "{full:?}"
        );
        assert!(full.ends_with(" 2/5"));

        let text_only = subtask_segment(Some((2, 5)), 22, false, 10, 0, false);
        assert_eq!(text_only, "  2/5");

        assert_eq!(subtask_segment(Some((2, 5)), 14, false, 10, 0, false), "");
        assert_eq!(subtask_segment(None, 60, false, 10, 0, false), "");
    }

    #[test]
    fn a_subtask_meter_yields_room_to_the_link_marker() {
        // The marker is #57's information, not this widget's to spend, so the
        // meter is what shrinks when both want the same columns. 25 is the width
        // where the bar fits without a marker but not with one.
        let width = 25;
        let without = subtask_segment(Some((2, 5)), width, false, 10, 0, false);
        let with = subtask_segment(Some((2, 5)), width, false, 10, 2, false);
        assert!(without.width() > with.width(), "{without:?} vs {with:?}");
    }

    #[test]
    fn a_subtask_meter_budgets_the_cursor_gutter_and_due_column() {
        // The task pane goes through the same `render_selectable`, so it spends
        // the cursor gutter on every row too. A segment that ignored it would
        // clip by exactly that much.
        let area = 40u16;
        let title = 10;
        let seg = subtask_segment(Some((2, 5)), area, true, title, 2, false);
        let usable =
            (area as usize - PANEL_BORDERS as usize) - LIST_CURSOR.width() - (DUE_WIDTH + 2);
        assert!(
            title + 2 + seg.width() <= usable,
            "row overruns: title {title} + marker 2 + {seg:?} > {usable}"
        );
    }

    #[test]
    fn a_subtask_meter_never_exceeds_its_budget() {
        for width in 0u16..=40 {
            for due_gutter in [false, true] {
                // 0, one marker, and both: a row can carry `⧉` and `≡` at once.
                // This pins the arithmetic *inside* the segment for that width —
                // whether the call site actually passes both is a question only a
                // rendered row can answer, and `notes_render.rs` asks it.
                for marker in [0usize, 2, 4] {
                    for ascii in [false, true] {
                        let title = 8usize;
                        let seg = subtask_segment(
                            Some((103, 247)),
                            width,
                            due_gutter,
                            title,
                            marker,
                            ascii,
                        );
                        if seg.is_empty() {
                            continue;
                        }
                        let gutter = if due_gutter { DUE_WIDTH + 2 } else { 0 };
                        let usable = (width.saturating_sub(PANEL_BORDERS) as usize)
                            .saturating_sub(LIST_CURSOR.width())
                            .saturating_sub(gutter);
                        assert!(
                            title + marker + seg.width() <= usable,
                            "width {width}, due {due_gutter}, marker {marker}: {seg:?}"
                        );
                    }
                }
            }
        }
    }

    // --- The inline notes preview ----------------------------------------

    #[test]
    fn truncate_reserves_the_ellipsis_width() {
        // A one-cell `…` and a three-cell `...` both leave the result within the
        // budget — the whole reason the ellipsis is a parameter.
        assert_eq!(truncate("hello world", 8, "…").width(), 8);
        assert!(truncate("hello world", 8, "...").width() <= 8);
        // Fits whole: no ellipsis, either spelling.
        assert_eq!(truncate("hi", 8, "…"), "hi");
    }

    #[test]
    fn a_notes_preview_needs_min_cells_of_room() {
        // Concrete widths, not `MIN_PREVIEW_CELLS`, pin the floor: at a computed
        // budget of 7 the row shows only the marker, at 8 the preview appears.
        // usable = (40-2)-2 = 36; budget = 36 - spent - 1(sep) = 35 - spent.
        let seg = |spent| notes_preview_segment("some prose here", 40, false, false, spent, false);
        assert_eq!(seg(28), None, "spent 28 leaves budget 7 — below the floor");
        assert!(
            seg(27).is_some(),
            "spent 27 leaves budget 8 — clears the floor"
        );
    }

    #[test]
    fn a_url_only_preview_line_is_shortened_to_its_authority() {
        // The whole point of the operator's choice: a bare-URL line collapses.
        let seg = notes_preview_segment("https://a.dev/some/deep/path", 80, false, false, 9, false)
            .expect("room at 80 cols");
        assert_eq!(seg, " a.dev");
    }

    #[test]
    fn a_truncated_preview_folds_its_ellipsis_to_ascii_under_fallback() {
        let long = "prose that certainly will not fit in a very narrow budget here";
        let braille = notes_preview_segment(long, 30, false, false, 5, false).expect("room");
        let ascii = notes_preview_segment(long, 30, false, false, 5, true).expect("room");
        assert!(braille.ends_with('…'), "{braille:?}");
        assert!(ascii.ends_with("..."), "{ascii:?}");
        assert!(
            !ascii.contains('…'),
            "no braille-era glyph under fallback: {ascii:?}"
        );
    }

    #[test]
    fn a_combining_mark_only_line_rides_the_separator_space() {
        // Accepted residual: a lone combining mark is legitimate zero-width text,
        // not `is_invisible`, so it earns a marker and a preview — one that
        // attaches to the leading separator space (a space-with-accent, width 1,
        // no layout shift).
        let line = notes_preview_line("\u{301}").expect("a combining mark is visible");
        let seg =
            notes_preview_segment(line, 80, false, false, 5, false).expect("a wide row has room");
        assert_eq!(seg, " \u{301}", "the mark rides the separator space");
    }

    #[test]
    fn a_notes_preview_never_exceeds_its_budget() {
        // The segment's own arithmetic, re-derived independently — as the Subtask
        // meter's budget test does, and for the same reason: a shared helper would
        // cancel a bug on both sides of the inequality.
        // `spent` sweeps the title/marker/meter combinations a real row produces:
        // a bare title, one and both markers, and a text or bar meter beside them.
        for area in 0u16..=60 {
            for due_gutter in [false, true] {
                for is_subtask in [false, true] {
                    for spent in [6usize, 8, 10, 16, 20] {
                        for ascii in [false, true] {
                            let Some(seg) = notes_preview_segment(
                                "a fairly long preview line of prose",
                                area,
                                due_gutter,
                                is_subtask,
                                spent,
                                ascii,
                            ) else {
                                continue;
                            };
                            let gutter = if due_gutter { DUE_WIDTH + 2 } else { 0 };
                            let indent = if is_subtask {
                                SUBTASK_INDENT.width()
                            } else {
                                0
                            };
                            let usable = (area.saturating_sub(PANEL_BORDERS) as usize)
                                .saturating_sub(LIST_CURSOR.width())
                                .saturating_sub(gutter)
                                .saturating_sub(indent);
                            assert!(
                                spent + seg.width() <= usable,
                                "area {area}, due {due_gutter}, sub {is_subtask}, spent {spent}: {seg:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    // --- Entry-type signifiers and counters ------------------------------

    /// A Model with `tasks` on a selected List `L` — the ordinary (non-Today)
    /// task pane these render tests exercise. A real List selection matters now
    /// that the default landing is Today, whose flat pane and `due <= today`
    /// membership would otherwise change what the pane draws and counts.
    fn model_with(tasks: Vec<Task>) -> Model {
        model_with_active_list(tasks).0
    }

    /// `model_with`, also returning the active `ListId` — for tests asserting on
    /// `list_meter`'s live branch.
    fn model_with_active_list(tasks: Vec<Task>) -> (Model, ListId) {
        let id = ListId("l".into());
        let mut model = Model::new();
        model.tasks = tasks;
        model.lists = vec![crate::domain::List {
            id: id.clone(),
            title: "L".into(),
            etag: String::new(),
            updated: chrono::DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }];
        model.selected = Selection::List(0);
        (model, id)
    }

    #[test]
    fn every_signifier_occupies_the_same_cell() {
        // Derived, not a magic constant: whatever width `Task`'s blank is, the
        // glyphs must match it or titles stagger down the pane.
        // Columns, not chars — the property is on-screen alignment, and it is
        // the same measure `the_cursor_gutter_is_the_same_width_either_way`
        // uses on the gutter this cell sits beside.
        for ascii in [false, true] {
            let width = signifier(EntryType::Task, ascii).width();
            for entry in [EntryType::Event, EntryType::Note] {
                assert_eq!(
                    signifier(entry, ascii).width(),
                    width,
                    "{entry:?} ascii={ascii}"
                );
            }
        }
    }

    #[test]
    fn signifiers_degrade_to_ascii_with_the_braille_widgets() {
        assert_eq!(signifier(EntryType::Event, false), "○ ");
        assert_eq!(signifier(EntryType::Note, false), "— ");
        assert_eq!(signifier(EntryType::Event, true), "o ");
        assert_eq!(signifier(EntryType::Note, true), "- ");
        // A Task is blank either way — rendering `•` on ~90% of rows would spend
        // a column to say "ordinary".
        assert_eq!(signifier(EntryType::Task, true).trim(), "");
    }

    #[test]
    fn the_meter_counts_only_task_typed_entries() {
        // Two Tasks, one done, plus a completed Note. The Note counts in neither
        // numerator nor denominator, so the label reads 1/2 — never 2/3, and
        // never the "2/1" a filtered denominator over an unfiltered numerator
        // would produce.
        let model = model_with(vec![
            titled("alpha", None, Status::NeedsAction),
            titled("beta", None, Status::Completed),
            titled("— jotting", None, Status::Completed),
        ]);
        let title = header_title("Tasks", &model, 200, true);
        assert!(title.contains(" 1/2"), "expected 1/2 in {title:?}");
    }

    #[test]
    fn a_list_with_no_task_typed_entries_shows_no_meter() {
        // There is no completion to report, so the meter is absent rather than
        // rendering an empty bar or 0/0.
        let model = model_with(vec![
            titled("○ standup", None, Status::NeedsAction),
            titled("— jotting", None, Status::NeedsAction),
        ]);
        let title = header_title("Tasks", &model, 200, true);
        assert_eq!(title, "Tasks", "expected no meter, got {title:?}");
    }

    #[test]
    fn due_load_counts_events_but_not_notes() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 3, 10, 9, 0, 0)
            .single()
            .expect("a valid local time");
        let today = Some(ymd(2026, 3, 10));
        let counts = due_load_counts(
            &[
                titled("alpha", today, Status::NeedsAction),
                titled("○ standup", today, Status::NeedsAction),
                titled("— jotting", today, Status::NeedsAction),
            ],
            now,
            3,
        );
        // Task + Event, not the Note.
        assert_eq!(counts[0], 2, "{counts:?}");
    }

    #[test]
    fn the_header_and_sidebar_meters_agree_for_the_active_list() {
        // `Model::list_meter` promises the two meters for the active List always
        // agree. Entry types can break that promise from one side: the header
        // counts only Task-typed entries, so a sidebar row counting Events
        // beside it would contradict the row it sits next to — two numbers for
        // one List, on screen at once.
        let (model, list) = model_with_active_list(vec![
            titled("alpha", None, Status::NeedsAction),
            titled("beta", None, Status::Completed),
            titled(
                &EntryType::Event.apply("standup"),
                None,
                Status::NeedsAction,
            ),
            titled(&EntryType::Note.apply("jotting"), None, Status::Completed),
        ]);

        let (done, total) = model.list_meter(&list).expect("an active-List meter");
        assert_eq!(
            (done, total),
            (1, 2),
            "the sidebar must skip the Event and the Note"
        );
        assert!(
            header_title("Tasks", &model, 200, true).contains(&format!(" {done}/{total}")),
            "header and sidebar disagree for the same List"
        );
    }

    #[test]
    fn a_subtask_meter_skips_typed_children() {
        // Same argument one level down: a Note nested under a parent is a
        // jotting about it, not a step toward it.
        let parent = titled("parent", None, Status::NeedsAction);
        let mut done_child = titled("step", None, Status::Completed);
        done_child.id = TaskId("c1".into());
        done_child.parent = Some(parent.id.clone());
        let mut note_child = titled(&EntryType::Note.apply("aside"), None, Status::NeedsAction);
        note_child.id = TaskId("c2".into());
        note_child.parent = Some(parent.id.clone());

        let model = model_with(vec![parent.clone(), done_child, note_child]);
        let top_level = model.top_level_ids();
        let counts = model.subtask_counts(&top_level);

        assert_eq!(
            counts.get(&parent.id).copied(),
            Some((1, 1)),
            "the Note child should not be counted"
        );
    }
}
