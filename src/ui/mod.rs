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

use chrono::{DateTime, Datelike, Local, NaiveDate};
use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use std::collections::HashMap;

use crate::app::text_input::TextInput;
use crate::app::{
    omnibox_rows, on_off, renders_as_subtask, split_command, CaptureRow, CommandState, Focus,
    JumpTarget, Model, OmniCommand, OmniRow, Overlay,
};
use crate::dateparse::{self, format_due_relative, split_title_and_due};
use crate::domain::{
    due_before, due_on_or_before, week_column, EntryType, ListId, Selection, Status, Task, TaskId,
    WEEK_DAYS,
};
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

/// Compose the palette from the Model and render one frame.
///
/// The one place `Model::flavor`/`Model::ascii` become a `Theme` and a flag, so a
/// render test can drive the same composition `main.rs` does — `main.rs` itself
/// is unreachable from the suite (see `tests/cli_args.rs`), which would otherwise
/// leave "`:flavor latte` repaints" verifiable only by inspection.
///
/// Deliberately four lines: [`view`] stays the frame renderer, and every existing
/// render test still calls it with a `Theme` of its own.
pub fn draw(model: &Model, frame: &mut Frame) {
    view(
        model,
        &Theme::from_flavor(model.flavor.as_str()),
        model.ascii,
        frame,
    );
}

/// Render the whole frame. Never mutates state. `ascii` reflects
/// [`Model::ascii`]: braille data widgets degrade to ASCII when set.
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
        render_overlay(frame, area, overlay, model, theme);
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

fn render_overlay(frame: &mut Frame, area: Rect, overlay: &Overlay, model: &Model, theme: &Theme) {
    let now = model.now;
    // The popup's inner text width, which every input line windows itself to.
    // Derived from the frame the way the pickers derive their row width, not
    // assumed to be `OVERLAY_WIDTH`: `centered` clamps the popup to the frame,
    // so a terminal narrower than the popup gets a narrower line.
    let text_width = OVERLAY_WIDTH
        .min(area.width)
        .saturating_sub(OVERLAY_BORDERS) as usize;
    // Every overlay but the picker is one or two lines of text in a popup, in
    // two shapes: the add-entry captures grow a second line only when a trailing
    // date is recognised (see `capture_lines`), while the due editor always has
    // one (see `due_lines`) because its whole job is to say what `Enter` will do.
    // The rest are a single line.
    let (title, lines): (&str, Vec<Line>) = match overlay {
        Overlay::EditTitle { buffer, .. } => ("Edit title", vec![input_line(buffer, text_width)]),
        Overlay::AddTask { buffer } => ("Add task", capture_lines(buffer, now, text_width, theme)),
        Overlay::AddSubtask { buffer, .. } => {
            ("Add subtask", capture_lines(buffer, now, text_width, theme))
        }
        // No "(blank clears)" here, unlike the notes editor below: `due_lines`
        // says it live, on the line beneath, exactly when the buffer is empty.
        Overlay::EditDue {
            task,
            buffer,
            pristine,
        } => (
            "Edit due date",
            due_lines(
                buffer,
                *pristine,
                stored_due(model, task),
                now,
                text_width,
                theme,
            ),
        ),
        Overlay::EditNotes { buffer, .. } => (
            "Edit notes (blank clears)",
            vec![input_line(buffer, text_width)],
        ),
        Overlay::AddList { buffer } => ("Add list", vec![input_line(buffer, text_width)]),
        Overlay::RenameList { buffer, .. } => ("Rename list", vec![input_line(buffer, text_width)]),
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
        Overlay::Omnibox { query, selected } => {
            render_omnibox(frame, area, model, query, *selected, theme);
            return;
        }
    };
    let height = u16::try_from(lines.len()).unwrap_or(1).max(1);
    let popup = centered(area, OVERLAY_WIDTH, height + OVERLAY_BORDERS);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(panel(title, true, theme)),
        popup,
    );
}

/// The editable line of a text overlay: the buffer with the caret bar drawn
/// *at* the caret, windowed to the `width` cells the popup has for it.
fn input_line(buffer: &TextInput, width: usize) -> Line<'static> {
    Line::from(input_window(buffer, buffer.caret(), width))
}

/// The visible slice of an input line: `text` with the caret bar inserted at
/// byte offset `caret`, windowed to `width` cells so the bar is always on
/// screen.
///
/// Stateless — the window is a function of the caret, so there is no scroll
/// offset to keep in step with the buffer. Two positions, one rule: while the
/// caret still fits, the window is anchored at the head, and past that it is
/// anchored on the caret, which is then the rightmost cell. No ellipsis marks
/// the hidden text; the line scrolls, as a terminal's own single-line inputs do.
///
/// Measured in display cells with `unicode_width`, as [`truncate`] measures, and
/// a character that would straddle either edge is dropped rather than split —
/// half of a wide character is not a character.
fn input_window(text: &str, caret: usize, width: usize) -> String {
    const CARET: &str = "▏";
    let caret_col = text[..caret].width();
    let line = format!("{}{CARET}{}", &text[..caret], &text[caret..]);
    // The bar occupies `[caret_col, caret_col + 1)`. It fits at the head while
    // that end column is within `width`; past that, the window ends on it.
    let start = (caret_col + CARET.width()).saturating_sub(width);
    let mut out = String::new();
    let mut col = 0;
    for c in line.chars() {
        let cell = c.width().unwrap_or(0);
        if col >= start && col + cell <= start + width {
            out.push(c);
        }
        col += cell;
    }
    out
}

/// The due date the model currently holds for `task`, or `None` if it has none —
/// or if the Task is no longer there at all.
///
/// Read at render time rather than captured when the overlay opened, because it
/// is the *current* Task the write will land on: a refresh landing mid-edit can
/// give the Task a due date, and an empty buffer submitted after that really does
/// clear one. Looking it up by id rather than by index is what makes this safe to
/// re-read — the hazard `Overlay::MoveToList` documents is stale *indices* into a
/// list that a `TasksLoaded` can reorder underneath, and an id does not move.
fn stored_due(model: &Model, task: &TaskId) -> Option<NaiveDate> {
    model
        .tasks
        .iter()
        .find(|t| t.id == *task)
        .and_then(|t| t.due)
}

/// Lines for the due editor: the input line, plus an always-present preview of
/// what `Enter` will do.
///
/// While `pristine` the buffer is drawn reversed — the terminal's own selection
/// idiom — because the next character replaces it. The trailing cursor bar stays
/// *outside* that span: reversed it would render as a filled block, reading as a
/// second cursor or a stray cell of highlight past the end of the selection.
///
/// The preview branches on `buffer.trim()`, matching `finish_edit_due`'s own
/// trim. That is load-bearing, not tidiness: a whitespace-only buffer is a
/// *clear* to the reducer but a parse error to `parse_due_relative_to`, so
/// branching on the raw buffer would render "not a date" in red while `Enter`
/// cheerfully cleared the date.
///
/// The empty branch splits on `stored_due`, the Task's *actual* due date, not on
/// `pristine`. An earlier version used `pristine` as a stand-in for "the Task had
/// no date" — true when the overlay opens, but a single `Backspace` on an
/// already-empty buffer clears the flag without changing anything, and the line
/// would flip to threatening a clear on a Task with nothing to clear.
/// Backspace-out-of-habit on a just-opened editor is a likely path, and a no-op
/// keystroke must not make the message scarier. Keyed off the date itself, the
/// wording depends only on what `Enter` will actually do.
///
/// The line is present either way: it is what fixes the popup's height, so
/// dropping it would make the frame jump as you type.
fn due_lines(
    buffer: &TextInput,
    pristine: bool,
    stored_due: Option<NaiveDate>,
    now: DateTime<Local>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let input = if pristine {
        Line::from(vec![
            Span::styled(
                buffer.to_string(),
                Style::new().add_modifier(Modifier::REVERSED),
            ),
            Span::raw("▏"),
        ])
    } else {
        input_line(buffer, width)
    };
    let trimmed = buffer.trim();
    let (preview, fg) = if trimmed.is_empty() {
        // Only a Task that *has* a date can have one cleared. On any other,
        // submitting an empty buffer changes nothing visible, and saying
        // "clears" would announce a destructive outcome that cannot happen.
        if stored_due.is_some() {
            ("→ clears the due date".to_string(), theme.subtext)
        } else {
            ("→ leaves it undated".to_string(), theme.subtext)
        }
    } else {
        match dateparse::parse_due_relative_to(trimmed, now) {
            Ok(due) => (
                format!("→ {}", format_due_preview(due, now.date_naive())),
                theme.subtext,
            ),
            Err(_) => ("→ not a date".to_string(), theme.overdue),
        }
    };
    vec![input, Line::styled(preview, Style::new().fg(fg))]
}

/// A resolved due date, for the editor's preview: weekday, ISO date, and how far
/// off it is — `Fri 2026-08-14 · in 23d`.
///
/// Deliberately not [`format_due_relative`], which serves the task pane's fixed
/// due column and is capped at `MAX_RENDERED_WIDTH`; that cap is why it must drop
/// the distance in favour of the ISO date past a week, showing one fact or the
/// other. The preview has the width for both, and needs both: the ISO date
/// answers "which day", the distance answers "how soon", and a date typed as
/// `friday` or `15` gives you neither until it is echoed back.
///
/// The distance words match `format_due_relative`'s where they overlap, so one
/// date never reads two ways with the pane on screen behind the popup.
fn format_due_preview(due: NaiveDate, today: NaiveDate) -> String {
    let distance = match (due - today).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        d if d > 0 => format!("in {d}d"),
        d => format!("{}d ago", -d),
    };
    format!("{} · {distance}", due.format("%a %Y-%m-%d"))
}

/// Lines for an add-entry capture: the input line, plus — only when a trailing
/// date is recognised — a dim preview of the `title · due` split that submitting
/// (with `Enter`) will produce. `Tab` submits the buffer verbatim, so what the
/// preview shows is exactly what a plain `Enter` commits.
fn capture_lines(
    buffer: &TextInput,
    now: DateTime<Local>,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = vec![input_line(buffer, width)];
    if let (title, Some(due)) = split_title_and_due(buffer, now) {
        let preview = format!("→ {title} · {}", format_due_relative(due, now.date_naive()));
        lines.push(Line::styled(preview, Style::new().fg(theme.subtext)));
    }
    lines
}

/// The Omnibox is wider than the pickers.
///
/// Its rows carry a label *and* a trailing reason or effect, where a picker's
/// carry only a name. At `OVERLAY_WIDTH` a row has
/// `50 - OVERLAY_BORDERS - LIST_CURSOR.width()` = 46 cells, and `truncate` drops
/// the *tail* — which is exactly where every reason lives, so the row would
/// clip away the thing it exists to say. Clamped to the frame, so a narrow
/// terminal still fits.
const OMNIBOX_WIDTH: u16 = 72;

/// Cells between a row's label and its trailing reason.
const OMNIBOX_GAP: usize = 2;

/// The narrowest a CAPTURE row's destination may be squeezed to before its
/// peeled title starts giving way instead. `Create task in "…"` and a few cells
/// of the List name.
const CAPTURE_LEAD_FLOOR: usize = 24;

/// Below this the `→ title` part is dropped whole rather than shown as `→ …`.
const CAPTURE_TITLE_FLOOR: usize = 8;

/// The Omnibox: a query in the panel title over a grouped result list.
fn render_omnibox(
    frame: &mut Frame,
    area: Rect,
    model: &Model,
    query: &str,
    selected: usize,
    theme: &Theme,
) {
    // Clear of the status line and the legend, as both pickers are, and for the
    // same reason: that legend is what advertises this overlay's own keys.
    let body = Rect {
        height: area.height.saturating_sub(BOTTOM_CHROME_ROWS),
        ..area
    };
    let rows = omnibox_rows(model, query);

    // Items first, headers included — `picker_height` must size off *these*, not
    // off `rows`, or the popup is up to four rows short. That failure is
    // invisible to a reversed-line assertion, because `ListState` scrolls the
    // selected row into view either way.
    let width = OMNIBOX_WIDTH.min(body.width);
    let text_width =
        (width.saturating_sub(OVERLAY_BORDERS) as usize).saturating_sub(LIST_CURSOR.width());
    let mut items = Vec::new();
    let mut drawn_selected = None;
    let mut group = None;
    for (i, row) in rows.iter().enumerate() {
        if group != Some(row.group()) {
            group = Some(row.group());
            items.push(ListItem::new(Line::styled(
                row.group().header(),
                Style::new().fg(theme.muted).add_modifier(Modifier::BOLD),
            )));
        }
        // The row vector is headerless while `items` is not, so the highlight
        // has to be remapped or it lands N lines high.
        if i == selected.min(rows.len().saturating_sub(1)) {
            drawn_selected = Some(items.len());
        }
        items.push(ListItem::new(omnibox_line(
            model, row, query, text_width, theme,
        )));
    }

    let popup = centered(body, width, picker_height(items.len(), body.height));
    frame.render_widget(Clear, popup);
    render_selectable(
        frame,
        popup,
        &omnibox_title(query, text_width),
        items,
        drawn_selected,
        true,
        theme,
    );
}

/// The panel title: a constant base, the query, and a caret.
///
/// The base is always drawn, so the box is never nameless on an empty query, and
/// the caret is unconditional — unlike `header_title`'s, which distinguishes a
/// live filter from a committed one; the Omnibox has no committed state.
///
/// **Clipped from the left**, keeping the query's tail. `truncate` drops the
/// tail, which here would hide the characters just typed *and* the caret with
/// them; a leading `…` is what an input field does, and says text is hidden
/// rather than clipping in silence.
fn omnibox_title(query: &str, text_width: usize) -> String {
    const BASE: &str = "Omnibox";
    const CARET: &str = "▏";
    if query.is_empty() {
        return BASE.to_string();
    }
    let budget = text_width.saturating_sub(BASE.width() + OMNIBOX_GAP + CARET.width());
    if query.width() <= budget {
        return format!("{BASE}  {query}{CARET}");
    }
    // Walk backwards in cells, reserving the ellipsis's own width.
    let budget = budget.saturating_sub("…".width());
    let mut used = 0;
    let mut start = query.len();
    for (i, c) in query.char_indices().rev() {
        let w = c.width().unwrap_or(0);
        if used + w > budget {
            break;
        }
        used += w;
        start = i;
    }
    format!("{BASE}  …{}{CARET}", &query[start..])
}

/// One row: `{lead}{gap}{trail}`, with **`trail` reserved before `lead` is
/// truncated**.
///
/// The same shape `legend_spans` uses for its pinned help cell: reserve the part
/// that must survive, spend the rest. What shortens is the echo of what the user
/// typed — never the reason they need to read.
fn omnibox_line(
    model: &Model,
    row: &OmniRow,
    query: &str,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (lead, trail) = match row {
        OmniRow::Jump(JumpTarget::Today) => ("Today".to_string(), String::new()),
        OmniRow::Jump(JumpTarget::List { id, title }) => (
            title.clone(),
            match model.list_meter(id) {
                Some((done, total)) => format!("{done}/{total}"),
                None => String::new(),
            },
        ),
        OmniRow::Command(command) => {
            let mut trail = match &command.state {
                CommandState::NeedsArgument { .. } => match command.command {
                    OmniCommand::Horizon => format!("now {}", model.horizon_days),
                    OmniCommand::Flavor => format!("now {}", model.flavor.as_str()),
                    OmniCommand::Ascii => format!("now {}", on_off(model.ascii)),
                    // `:refresh` and `:week` take no argument, so `command_state`
                    // never hands them this state, and they set no value there
                    // would be a `now …` for. Empty rather than `unreachable!` — a
                    // missing trail is not worth panicking the TUI over.
                    OmniCommand::Refresh | OmniCommand::Week => String::new(),
                },
                CommandState::Invalid { reason } => reason.clone(),
                CommandState::RefusedHere { reason } => (*reason).to_string(),
                CommandState::Valid { effect } => effect.clone(),
            };
            // Appended to *whichever* state is showing, matching the row-level
            // field: drawing it on `Valid` alone would leave a value asserted on
            // three rows visible on one.
            if let Some(advisory) = command.advisory {
                trail.push(' ');
                trail.push_str(advisory);
            }
            let arg = split_command(query.trim()).1;
            (
                format!(
                    ":{}{}",
                    command.command.verb(),
                    command_arg_suffix(&command.state, arg)
                ),
                trail,
            )
        }
        OmniRow::Search { query } => (format!("Search all Lists for \"{query}\""), String::new()),
        OmniRow::Capture(CaptureRow::Refused { reason }) => {
            ("Create task".to_string(), (*reason).to_string())
        }
        OmniRow::Capture(CaptureRow::Ready {
            list_title,
            title,
            due,
        }) => return capture_line(list_title, title, *due, model.now, width, theme),
    };

    let trail_cells = if trail.is_empty() {
        0
    } else {
        trail.width() + OMNIBOX_GAP
    };
    let lead = truncate(&lead, width.saturating_sub(trail_cells), "…");
    let pad = width.saturating_sub(lead.width() + trail.width());
    if trail.is_empty() {
        return Line::raw(lead);
    }
    Line::from(vec![
        Span::raw(lead),
        Span::raw(" ".repeat(pad)),
        Span::styled(trail, Style::new().fg(theme.subtext)),
    ])
}

/// The typed argument, echoed after the verb, or the placeholder when there is
/// none — so the row shows what it will act on rather than the bare verb.
///
/// `arg` is the one the row was built from, re-split from the query here rather
/// than carried on `CommandState`: only the renderer wants it, and the states
/// that have one already carry what it *means* (a reason, or an effect).
fn command_arg_suffix(state: &CommandState, arg: Option<&str>) -> String {
    match state {
        CommandState::NeedsArgument { .. } => " ‹arg›".to_string(),
        CommandState::Invalid { .. }
        | CommandState::RefusedHere { .. }
        | CommandState::Valid { .. } => arg.map(|a| format!(" {a}")).unwrap_or_default(),
    }
}

/// The CAPTURE row, which reserves **per part** rather than whole-trail.
///
/// Every other row's trail is a constant this file chose; this one's holds the
/// peeled title — user text of unbounded length. Reserving the whole trail would
/// let a long title eat `lead` to nothing and lose the destination List, which
/// is the one thing the row exists to name.
fn capture_line(
    list_title: &str,
    title: &str,
    due: Option<NaiveDate>,
    now: DateTime<Local>,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let due_part = due
        .map(|d| format!(" · {}", format_due_relative(d, now.date_naive())))
        .unwrap_or_default();

    // 1. the date, reserved first and bounded. 2. the destination, which yields
    // to the title *down to its floor* but no further — that floor is what makes
    // "the row always names where the Task goes" true. 3. the title, whatever
    // remains.
    let remaining = width.saturating_sub(due_part.width());
    let lead_full = format!("Create task in \"{list_title}\"");
    let title_full = "→ ".width() + title.width();
    let lead_budget = lead_full
        .width()
        .min(remaining.saturating_sub(OMNIBOX_GAP + title_full))
        // Never squeezed below the floor, and never wider than it needs to be:
        // a short List title keeps its whole name even beside a long query.
        .max(CAPTURE_LEAD_FLOOR.min(lead_full.width()))
        .min(remaining);
    let lead = truncate(&lead_full, lead_budget, "…");

    let title_budget = remaining.saturating_sub(lead.width() + OMNIBOX_GAP + "→ ".width());
    // The floor guards against a useless `→ …`, so it applies only when the title
    // must be *truncated*. A flat `title_budget < FLOOR` discarded titles that fit
    // whole: `lead_budget` above already reserved this room for them, so once the
    // destination takes its reservation `title_budget == title.width()` exactly,
    // and every title under the floor was dropped into blank padding it fitted in.
    let title_part = if title_budget < CAPTURE_TITLE_FLOOR.min(title.width()) {
        // Dropped whole rather than shown as `→ …`: the row still says where the
        // Task goes and when it is due, and the title is already on screen in the
        // panel-title query the user just typed.
        String::new()
    } else {
        format!("→ {}", truncate(title, title_budget, "…"))
    };

    let trail = format!("{title_part}{due_part}");
    let pad = width.saturating_sub(lead.width() + trail.width());
    Line::from(vec![
        Span::raw(lead),
        Span::raw(" ".repeat(pad)),
        Span::styled(trail, Style::new().fg(theme.subtext)),
    ])
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
    let mut items: Vec<ListItem> = Vec::with_capacity(model.lists.len() + 2);
    items.push(ListItem::new("Today"));
    // The Weekly spread's row: an indicator, never a cursor stop. It cannot be
    // selectable, because the cursor is what names the pool List the spread draws
    // UNSCHEDULED from — landing the cursor here would leave the spread with no
    // pool. So it shows the lens's state and the key that toggles it, and the
    // cursor steps straight over it (a non-selectable row among selectable ones,
    // exactly as the journal spread's headers are in the task pane).
    items.push(week_sidebar_row(model.week_active(), theme));
    for l in &model.lists {
        items.push(ListItem::new(sidebar_row(
            &l.title,
            model.list_meter(&l.id),
            area.width,
            ascii,
        )));
    }
    // Offset by the two pinned rows above the Lists, not one.
    let selected = match model.selected {
        Selection::Today => Some(0),
        Selection::List(i) => Some(i + 2),
    };
    render_selectable(frame, area, "Lists", items, selected, focused, theme);
}

/// The sidebar's Weekly spread row: the label, and the key that toggles it.
///
/// Lit in the accent while the lens is on, dim otherwise — it is the one place
/// the sidebar says which pane the task pane is showing, since the cursor stays
/// parked on a List either way.
///
/// Carries no glyph, deliberately. A `●`/`○` pair would read as an **Entry
/// type** signifier — `○ ` is the Event glyph, two rows from Tasks that wear it
/// — and it would have to degrade under `ascii_fallback` into `-`, which is the
/// Note glyph. The label's own weight and colour say enough, and the panel title
/// names the pane besides.
fn week_sidebar_row(active: bool, theme: &Theme) -> ListItem<'static> {
    let style = if active {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.subtext)
    };
    ListItem::new(Line::from(vec![
        Span::styled("Week", style),
        Span::styled("  W", Style::new().fg(theme.muted)),
    ]))
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
    let flat = model.flat_pane();
    let spread = model.today_active();
    // The Weekly spread: a grid of day columns instead of a due gutter, and the
    // pane's own header rows. Mutually exclusive with `spread` — `today_active()`
    // is gated on `!week` — so the two never interleave their headers.
    let week = model.week_active();
    let week_start = model.week_start();
    // The day cursor, drawn only on the selected row: the cursor is a (row, day)
    // pair, so a bracket on every row would claim five cursors.
    let cursor_day = model.week_day;
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
    // In the Weekly spread the grid *is* the date: a gutter repeating it would
    // spend twelve cells restating which column already has the dot.
    let due_gutter = if week {
        false
    } else if spread {
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
    // The Weekly spread reserves it for the journal spread's reason: titles must
    // hold their column as Events and Notes enter and leave the week.
    let signifiers = spread || week || ordered.iter().any(|t| t.entry_type() != EntryType::Task);
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
    // The row width the List widget leaves for content: the panel's borders and
    // the cursor gutter come off it. Read once, outside the per-row closure.
    let inner_row_width =
        (area.width.saturating_sub(PANEL_BORDERS) as usize).saturating_sub(LIST_CURSOR.width());
    let grid = WeekGrid {
        start: week_start,
        today_column: week_column(Some(today), week_start),
        inner_width: inner_row_width,
    };
    // The selected Task's id, for the day cursor's brackets. `selected` below is a
    // *display* position, computed after this map, so the id is what the rows can
    // compare against.
    let selected_id = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .map(|t| t.id.clone());
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
            //
            // Suppressed in the Weekly spread, where the grid is the row's tail:
            // an unbounded preview would push the columns off the right edge, and
            // the `≡` marker above already says there is something to read.
            if let (false, Some(line)) = (week, preview_line) {
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
            if week {
                // Clip the row's own text to the budget the grid leaves, then pad
                // to the right edge. Clipping here rather than letting the List
                // widget do it is what keeps the grid on screen: the widget would
                // truncate from the right, taking the columns first.
                let budget = week_text_budget(grid.inner_width);
                let mut used = spans_width(&spans);
                if used > budget {
                    spans = clip_spans(spans, budget);
                    used = spans_width(&spans);
                }
                spans.push(Span::raw(week_grid_pad(used, grid.inner_width)));
                spans.extend(week_row_cells(
                    t,
                    &grid,
                    // Only the selected row wears the cursor.
                    (selected_id.as_ref() == Some(&t.id))
                        .then_some(cursor_day)
                        .flatten(),
                    ascii,
                    theme,
                ));
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
    // the rows are inserted, and cannot be left behind. The Weekly spread does the
    // same with its own headers; the two are mutually exclusive.
    let (items, selected) = if spread {
        journal_spread(items, selected, &ordered, overdue_rows, today, theme)
    } else if week {
        // The pool as a count: `week_ordered` sorts the undated rows to the front,
        // so they are a contiguous prefix and `take_while` sees all of them.
        let pool_rows = ordered.iter().take_while(|t| t.due.is_none()).count();
        let pool_title = model.week_pool_list().and_then(|id| {
            model
                .lists
                .iter()
                .find(|l| &l.id == id)
                .map(|l| l.title.as_str())
        });
        week_spread(items, selected, pool_rows, pool_title, &grid, theme)
    } else {
        (items, selected)
    };

    // Search and the Weekly spread name themselves in the header — neither has a
    // sidebar row of its own and the parked cursor still highlights the List it
    // was opened from, so the title is where the user reads which pane this is.
    let base = if model.search_active() {
        format!("SEARCH — {}", model.sort.label())
    } else if week {
        // No Sort label: the spread has one fixed order, which is why `s` is
        // refused in it. The week it shows takes that slot instead.
        format!("WEEKLY SPREAD — week {}", week_start.iso_week().week())
    } else {
        format!("Tasks — {}", model.sort.label())
    };
    // Inline btop-style data widgets in the header: a completion meter for the
    // active List and a due-load strip. Both drop out (never the text) when the
    // pane is too narrow — braille degrades before the title (ADR-0006).
    let inner_width = area.width.saturating_sub(PANEL_BORDERS);
    let title = header_title(&base, model, inner_width, ascii);
    render_selectable(frame, area, &title, items, selected, focused, theme);
}

/// One day column, in display cells: a space, the glyph, a space — or the two
/// brackets around it. Wide enough for the two-letter header label too.
const WEEK_CELL_WIDTH: usize = 3;
/// The whole grid's width, derived from the column count rather than restated,
/// so a day added to `WEEK_DAYS` widens every reservation that reads this.
const WEEK_GRID_WIDTH: usize = WEEK_DAYS * WEEK_CELL_WIDTH;
/// Gap between a row's text and the grid, so a full-width title never touches it.
const WEEK_GRID_GAP: usize = 2;
/// Two-letter day labels, one per column. Length is checked against `WEEK_DAYS`
/// by `week_labels_cover_every_column`.
const WEEK_LABELS: [&str; WEEK_DAYS] = ["Mo", "Tu", "We", "Th", "Fr"];
/// The spread's dateline range: `Mon 17 – Fri 21 Aug`.
const WEEK_DAY_FORMAT: &str = "%a %-d";
const WEEK_END_FORMAT: &str = "%a %-d %b";

/// The Weekly spread's geometry for one frame: which week is on screen, which
/// of its columns is today, and how wide the rows are.
///
/// One value rather than three parameters threaded through every grid function —
/// the header, the row cells and the padding all answer to the same three facts,
/// and passing them separately let a caller pair a `start` with another week's
/// `today_column`.
struct WeekGrid {
    /// Monday of the displayed week.
    start: NaiveDate,
    /// Today's column, or `None` once the spread is paged off the current week —
    /// so a stale accent never marks a column that is not today.
    today_column: Option<usize>,
    /// The row width the List widget leaves for content, borders and the cursor
    /// gutter already deducted.
    inner_width: usize,
}

/// What a day cell holds. Derived from the row, never stored — the dot *is* the
/// due date (ADR-0003), and the cross is its status.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WeekCell {
    /// No dot on this day.
    Empty,
    /// Planned here and still `needsAction`.
    Planned,
    /// Planned here and completed — bullet journal's dot crossed out.
    Done,
}

/// The glyph for a cell, degrading to ASCII under `ascii_fallback` exactly as the
/// signifier and marker glyphs do.
fn week_glyph(cell: WeekCell, ascii: bool) -> &'static str {
    match (cell, ascii) {
        (WeekCell::Empty, false) => "·",
        (WeekCell::Empty, true) => ".",
        (WeekCell::Planned, false) => "•",
        (WeekCell::Planned, true) => "*",
        (WeekCell::Done, false) => "✕",
        (WeekCell::Done, true) => "x",
    }
}

/// One day cell, three display cells wide: the glyph centred, bracketed when the
/// day cursor is on it.
///
/// Brackets rather than a colour or a reverse: the selected row is already drawn
/// reversed by the List's highlight, so any style-based marker would have to
/// survive being inverted. `[•]` reads the same under every theme, in ASCII
/// fallback, and on a monochrome terminal — and it costs no extra width, since
/// the cell reserves three cells either way.
fn week_cell(cell: WeekCell, cursor: bool, ascii: bool) -> String {
    let glyph = week_glyph(cell, ascii);
    if cursor {
        format!("[{glyph}]")
    } else {
        format!(" {glyph} ")
    }
}

/// The grid's column header, right-aligned over the cells it labels. Each label
/// sits one cell in, the same offset the glyph below it takes, so the two line up.
///
/// The grid's `today_column` is painted in the accent, and is `None` unless the
/// week on screen actually contains today — paging to next week must not leave a
/// stale "today" marker behind.
fn week_header_cells(grid: &WeekGrid, theme: &Theme) -> Vec<Span<'static>> {
    WEEK_LABELS
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let style = if grid.today_column == Some(i) {
                Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(theme.subtext)
            };
            Span::styled(format!(" {label}"), style)
        })
        .collect()
}

/// The grid for one row: a cell per day, the dot in whichever column the entry's
/// due date names.
///
/// A row can hold at most one dot, and that falls straight out of the data model
/// rather than being enforced here — a Task has one due date.
fn week_row_cells(
    task: &Task,
    grid: &WeekGrid,
    cursor: Option<usize>,
    ascii: bool,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let column = week_column(task.due, grid.start);
    (0..WEEK_DAYS)
        .map(|i| {
            let cell = match (column == Some(i), task.status == Status::Completed) {
                (false, _) => WeekCell::Empty,
                (true, false) => WeekCell::Planned,
                (true, true) => WeekCell::Done,
            };
            // An empty cell is scaffolding, so it recedes; a dot is the row's own
            // information and reads at full strength. Today's column keeps the
            // accent it carries in the header, so the eye finds the day it is on.
            let style = match (cell, grid.today_column == Some(i)) {
                (WeekCell::Empty, _) => Style::new().fg(theme.surface),
                (_, true) => Style::new().fg(theme.accent),
                (_, false) => Style::new().fg(theme.text),
            };
            Span::styled(week_cell(cell, cursor == Some(i), ascii), style)
        })
        .collect()
}

/// The spread's header rows, interleaved into the Task rows, and the cursor
/// shifted to match — the Weekly spread's answer to [`journal_spread`].
///
/// - The **column header** always leads. It is the grid's legend, and like the
///   journal spread's dateline it is drawn even when the pane below is empty.
/// - **UNSCHEDULED** heads the pool: the undated entries of the List the sidebar
///   cursor names. Drawn when the pool has rows, *or* when there is no pool List
///   to draw from — an absent block would otherwise read as "nothing undated"
///   when it means "nowhere to look".
/// - **WEEK n** heads the scheduled rows, and names the days on display.
///
/// `pool_rows` is a count, not a partition: `week_ordered` sorts the undated rows
/// to the front, so they are a contiguous prefix exactly as the journal spread's
/// Overdue group is.
fn week_spread<'a>(
    rows: Vec<ListItem<'a>>,
    selected: Option<usize>,
    pool_rows: usize,
    pool_title: Option<&str>,
    grid: &WeekGrid,
    theme: &Theme,
) -> (Vec<ListItem<'a>>, Option<usize>) {
    let scheduled_rows = rows.len() - pool_rows;
    let pool_header = pool_rows > 0 || pool_title.is_none();

    let mut header = vec![Span::raw(week_grid_pad(0, grid.inner_width))];
    header.extend(week_header_cells(grid, theme));
    let mut out = Vec::with_capacity(rows.len() + 3);
    out.push(ListItem::new(Line::from(header)));

    let mut rows = rows.into_iter();
    if pool_header {
        out.push(match pool_title {
            Some(title) => week_header(format!("UNSCHEDULED ({title})"), theme),
            // Fail closed: say why the block is empty rather than letting an
            // absent pool read as an empty one.
            None => week_header(
                "UNSCHEDULED — no list selected, and no default list".to_string(),
                theme,
            ),
        });
        out.extend(rows.by_ref().take(pool_rows));
    }
    if scheduled_rows > 0 {
        let end = grid.start + chrono::Duration::days(WEEK_DAYS as i64 - 1);
        out.push(week_header(
            format!(
                "WEEK {} · {} – {}",
                grid.start.iso_week().week(),
                grid.start.format(WEEK_DAY_FORMAT),
                end.format(WEEK_END_FORMAT)
            ),
            theme,
        ));
    }
    out.extend(rows);

    let selected = selected.map(|p| week_offset(p, pool_rows, pool_header));
    (out, selected)
}

/// Rows the Weekly spread inserts above the Task at display position `p`: the
/// column header always, the UNSCHEDULED header when it is drawn, and the WEEK
/// header once past the pool.
fn week_offset(p: usize, pool_rows: usize, pool_header: bool) -> usize {
    p + 1 + usize::from(pool_header) + usize::from(p >= pool_rows)
}

/// One group header of the Weekly spread. No count: the grid answers "how much,
/// which day" cell by cell, so a tally at the top would restate it less precisely.
fn week_header(label: String, theme: &Theme) -> ListItem<'static> {
    ListItem::new(Line::from(Span::styled(
        label,
        Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD),
    )))
}

/// Padding that right-aligns the grid: whatever is left of the row after `used`
/// cells of text and the grid's own width, floored at the gap so the two never
/// touch even when the text has taken the whole row.
fn week_grid_pad(used: usize, inner_width: usize) -> String {
    let pad = inner_width
        .saturating_sub(used + WEEK_GRID_WIDTH)
        .max(WEEK_GRID_GAP);
    " ".repeat(pad)
}

/// The text budget a spread row has before the grid: the row minus the grid and
/// its gap. Floored at a few cells so a very narrow pane clips the title to an
/// ellipsis rather than to nothing.
fn week_text_budget(inner_width: usize) -> usize {
    inner_width
        .saturating_sub(WEEK_GRID_WIDTH + WEEK_GRID_GAP)
        .max(WEEK_TITLE_FLOOR)
}

/// The narrowest a spread row's text is ever clipped to. Below this the grid is
/// still drawn in full — it is the view, and a spread without its columns is not
/// a narrower spread but a different one.
const WEEK_TITLE_FLOOR: usize = 4;

/// How many display cells a run of spans has already taken.
fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// Clip a styled run to `budget` display cells, ending in an ellipsis.
///
/// Span-aware because a spread row is several styled pieces — signifier, title,
/// markers, List name — and clipping the joined text would lose their styles.
/// Each span keeps its own; the one straddling the budget is cut with
/// [`truncate`], which measures in cells and never splits a wide character.
fn clip_spans(spans: Vec<Span<'_>>, budget: usize) -> Vec<Span<'_>> {
    let mut out = Vec::with_capacity(spans.len());
    let mut used = 0usize;
    for span in spans {
        let width = span.content.width();
        if used + width <= budget {
            used += width;
            out.push(span);
            continue;
        }
        let room = budget - used;
        let style = span.style;
        let cut = truncate(&span.content, room, "…");
        // `truncate` spends its ellipsis unconditionally, so at zero room it
        // answers a one-cell `…` — which would overflow the very budget being
        // enforced and push the grid's last column off the row. Dropped rather
        // than trusted.
        if !cut.is_empty() && cut.width() <= room {
            out.push(Span::styled(cut, style));
        }
        break;
    }
    out
}

/// How the spread's dateline renders the day: `Wednesday 30 September 2026`.
/// Unpadded day-of-month (`%-d`), because a journal page writes "1 July", not
/// "01 July".
const DATELINE_FORMAT: &str = "%A %-d %B %Y";

/// Interleave Today's journal-spread header rows into the Task rows, and shift
/// the cursor to match.
///
/// Non-selectable rows share the Tasks' `List` widget — not a `Paragraph` above
/// the pane, which would need its own scroll and would detach a label from the
/// rows it heads the moment the pane moved:
///
/// - The **dateline** always leads. It is the page, not a label for the rows, so
///   it is drawn even on an empty day.
/// - When an **Overdue** group splits the pane, an `Overdue` header heads it and a
///   `Today` header heads the rows due today — the two dividers of the migration
///   ritual. A divider is drawn when its group has **rows**; its count is only the
///   rows still `needsAction`. Membership is status-blind so the Overdue prefix
///   holds (see [`due_before`]), while the count answers what is *left* to move,
///   so a struck-through row is not in it. At zero outstanding the count and the
///   urgent colour both drop.
/// - With **no Overdue** group the `Today` header would only echo the dateline
///   (which is itself today), so it is dropped; the dateline carries the
///   outstanding count instead.
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

    // The dateline. With no Overdue group there is nothing for a `Today` divider
    // to divide from, so it would only echo the dateline — carry its one unique
    // signal, the count still outstanding today, here instead. Two spaces set the
    // count off from the long date text, where the divider's tight `label N` would
    // read as cramped. Omitted at zero, as a divider's count is.
    let mut dateline = vec![Span::styled(
        today.format(DATELINE_FORMAT).to_string(),
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
    )];
    if overdue.is_empty() {
        let n = outstanding(rest);
        if n > 0 {
            dateline.push(Span::styled(
                format!("  {n}"),
                Style::new().fg(theme.subtext),
            ));
        }
    }

    let mut out = Vec::with_capacity(rows.len() + 3);
    out.push(ListItem::new(Line::from(dateline)));
    let mut rows = rows.into_iter();
    if !overdue.is_empty() {
        out.push(spread_header("Overdue", outstanding(overdue), true, theme));
        out.extend(rows.by_ref().take(overdue.len()));
        if !rest.is_empty() {
            out.push(spread_header("Today", outstanding(rest), false, theme));
        }
    }
    out.extend(rows);

    // A cursor at display position `p` is pushed down by the header rows above it.
    let selected = selected.map(|p| spread_offset(p, overdue_rows));
    (out, selected)
}

/// Rows the journal spread inserts above the Task at display position `p`: the
/// dateline always, plus the `Overdue`/`Today` dividers when an Overdue group
/// splits the pane. With no Overdue group there are no dividers — the dateline
/// carries the count instead — so every row shifts by one.
fn spread_offset(p: usize, overdue_rows: usize) -> usize {
    p + 1
        + if overdue_rows == 0 {
            0
        } else {
            1 + usize::from(p >= overdue_rows)
        }
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

    // The pending notice while Search awaits its live fan-out: a List never
    // mirrored on this machine contributes nothing until it lands, so an empty
    // pane could read "no match" when it means "not yet". Appended unconditionally
    // like the query — it is state, not a droppable widget — so a narrow pane clips
    // it as it clips the title, never silently dropping the one cue that tells the
    // two apart. Held on `search_pending`, so no status-line write can erase it.
    if model.search_pending {
        title.push_str("  · searching all lists…");
    }
    // The same notice for the Weekly spread, which reads the same whole corpus:
    // until the fan-out lands a never-mirrored List contributes nothing, and an
    // empty week must read as "not yet", never as "nothing planned".
    if model.week_pending {
        title.push_str("  · reading all lists…");
    }

    // Both header widgets are suppressed in Search and the Weekly spread: the
    // meter would report a whole-corpus ratio (a fact about no pane you are
    // reading), and the strip forecasts a workload "every Task in every List" is
    // not. The spread's grid already answers "how much, which day" for the days it
    // covers, and does it per row.
    if model.search_active() || model.week_active() {
        return title;
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
        // Its own legend: `j`/`k` type, movement is `Up`/`Down`, and `Enter`
        // runs a row rather than saving a buffer — none of which `TextInput`
        // would have said.
        Some(Overlay::Omnibox { .. }) => keymap::LegendContext::Omnibox,
        // The same overlay in Search advertises `Esc leave search` rather than
        // `Esc drop filter`: the pane behind it is the corpus, not a List, so
        // `Esc` leaves Search outright (matching `filter_key`'s Search-aware
        // `Esc`) instead of unfiltering a pane you would stay in. `^U clear` is
        // what empties the query in both.
        Some(Overlay::Filter) if model.search_active() => keymap::LegendContext::SearchFilter,
        Some(Overlay::Filter) => keymap::LegendContext::Filter,
        // The add-entry captures parse a trailing date and bind `Tab` for a
        // literal submit, so they get their own legend rather than `TextInput`'s.
        Some(Overlay::AddTask { .. } | Overlay::AddSubtask { .. }) => {
            keymap::LegendContext::TaskCapture
        }
        // The due editor binds four stepping keys on top of the text ones, so it
        // declares its own legend rather than advertising `TextInput`'s.
        Some(Overlay::EditDue { .. }) => keymap::LegendContext::DueInput,
        Some(
            Overlay::EditTitle { .. }
            | Overlay::EditNotes { .. }
            | Overlay::AddList { .. }
            | Overlay::RenameList { .. },
        ) => keymap::LegendContext::TextInput,
        None => match model.focus {
            // The spread rebinds `h`/`l` and `Space` in this pane, so it needs
            // its own legend or the row would advertise two keys that no longer
            // do what it says.
            Focus::Tasks if model.week_active() => keymap::LegendContext::Week,
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
    use ratatui::style::Color;
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

    #[test]
    fn the_journal_spread_inserts_a_header_per_group_and_shifts_the_cursor() {
        let theme = Theme::from_flavor("mocha");
        let today = ymd(2026, 7, 20);
        // Three Tasks; `overdue_rows` alone decides how the spread groups them.
        let tasks = [
            task(Some(today), Status::NeedsAction),
            task(Some(today), Status::NeedsAction),
            task(Some(today), Status::NeedsAction),
        ];
        let ordered: Vec<&Task> = tasks.iter().collect();
        let build = || -> Vec<ListItem<'static>> {
            (0..ordered.len()).map(|_| ListItem::new("row")).collect()
        };

        // No Overdue group: the dateline is the only inserted row.
        let (out, cursor) = journal_spread(build(), Some(1), &ordered, 0, today, &theme);
        assert_eq!(out.len() - ordered.len(), 1, "dateline only");
        assert_eq!(cursor, Some(2), "shift past the dateline");

        // All overdue, none due today: dateline + `Overdue`, and no `Today`.
        let (out, cursor) =
            journal_spread(build(), Some(0), &ordered, ordered.len(), today, &theme);
        assert_eq!(out.len() - ordered.len(), 2, "dateline + Overdue");
        assert_eq!(cursor, Some(2), "shift past dateline + Overdue header");

        // Both groups: dateline + `Overdue` + `Today`.
        let (out, prefix) = journal_spread(build(), Some(0), &ordered, 1, today, &theme);
        assert_eq!(out.len() - ordered.len(), 3, "dateline + Overdue + Today");
        assert_eq!(
            prefix,
            Some(2),
            "a row in the Overdue prefix clears two headers"
        );
        let (_, past) = journal_spread(build(), Some(2), &ordered, 1, today, &theme);
        assert_eq!(
            past,
            Some(5),
            "a row past the prefix clears all three header rows"
        );
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
            buffer: TextInput::default(),
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::TaskCapture);

        model.overlay = Some(Overlay::AddSubtask {
            parent: TaskId("p".into()),
            buffer: TextInput::default(),
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::TaskCapture);

        // The due editor declares its own legend: it binds the stepping keys,
        // which `TextInput`'s cells would not have said.
        model.overlay = Some(Overlay::EditDue {
            task: TaskId("t".into()),
            buffer: TextInput::default(),
            pristine: false,
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::DueInput);

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

        // The Omnibox *does* have a buffer, and still declares its own: `j`/`k`
        // type but movement is `Up`/`Down`, and `Enter` runs a row rather than
        // saving — none of which `TextInput`'s two cells would have said.
        model.overlay = Some(Overlay::Omnibox {
            query: String::new(),
            selected: 0,
        });
        assert_eq!(legend_context(&model), keymap::LegendContext::Omnibox);
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

    // --- The input line's caret window ------------------------------------

    /// The caret bar is drawn where the caret is, not after the text.
    #[test]
    fn the_caret_bar_sits_at_the_caret() {
        assert_eq!(input_window("abcd", 4, 20), "abcd▏");
        assert_eq!(input_window("abcd", 2, 20), "ab▏cd");
        assert_eq!(input_window("abcd", 0, 20), "▏abcd");
        assert_eq!(input_window("", 0, 20), "▏", "an empty line is all caret");
    }

    /// Past the fold the window follows the caret, and every window is exactly
    /// the cells it was given — the point being that the bar is never off-screen.
    #[test]
    fn a_line_longer_than_the_popup_windows_onto_the_caret() {
        let long = "abcdefghij";

        // Caret at the head: anchored left, the tail hidden.
        assert_eq!(input_window(long, 0, 8), "▏abcdefg");
        // Still at the head while the bar fits within the width.
        assert_eq!(input_window(long, 4, 8), "abcd▏efg");
        // Past it: anchored on the bar, which is now the rightmost cell.
        assert_eq!(input_window(long, 9, 8), "cdefghi▏");
        assert_eq!(input_window(long, 10, 8), "defghij▏");

        for caret in 0..=long.len() {
            assert!(
                input_window(long, caret, 8).width() <= 8,
                "caret {caret} overflowed the window"
            );
        }
    }

    /// A wide character that would straddle an edge is dropped, not split: the
    /// window then runs a cell short rather than printing half a glyph.
    #[test]
    fn a_wide_character_at_the_window_edge_is_dropped_whole() {
        // 日本語abc is 2+2+2+1+1+1 = 9 cells; the bar makes 10.
        assert_eq!(input_window("日本語abc", 12, 8), "本語abc▏");
        let clipped = input_window("日本語abc", 12, 7);
        assert_eq!(clipped, "語abc▏", "`本` would have straddled the left edge");
        assert_eq!(clipped.width(), 6, "a cell left blank rather than split");
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

    // --- The Weekly spread's grid ----------------------------------------

    /// The label table and the column count are one fact stated twice; a day
    /// added to one without the other would draw a header the cells do not match.
    #[test]
    fn week_labels_cover_every_column() {
        assert_eq!(WEEK_LABELS.len(), WEEK_DAYS);
        assert_eq!(WEEK_GRID_WIDTH, WEEK_DAYS * WEEK_CELL_WIDTH);
    }

    /// Every cell is exactly `WEEK_CELL_WIDTH` wide, bracketed or not, in either
    /// glyph set — which is what lets the grid's width be reserved up front.
    #[test]
    fn every_day_cell_is_the_same_width() {
        for cell in [WeekCell::Empty, WeekCell::Planned, WeekCell::Done] {
            for cursor in [false, true] {
                for ascii in [false, true] {
                    let drawn = week_cell(cell, cursor, ascii);
                    assert_eq!(
                        drawn.width(),
                        WEEK_CELL_WIDTH,
                        "{drawn:?} (cursor {cursor}, ascii {ascii})"
                    );
                }
            }
        }
    }

    /// The cursor is drawn with brackets rather than a style, so it survives the
    /// List widget's reverse on the selected row and reads on a monochrome
    /// terminal. The glyph inside is unchanged.
    #[test]
    fn the_cursor_brackets_the_cell_without_changing_its_glyph() {
        assert_eq!(week_cell(WeekCell::Planned, false, false), " • ");
        assert_eq!(week_cell(WeekCell::Planned, true, false), "[•]");
        assert_eq!(week_cell(WeekCell::Done, true, true), "[x]");
    }

    /// The pad right-aligns the grid, and never lets a full-width row touch it:
    /// at any width the gap survives, so the columns are always readable as
    /// columns.
    #[test]
    fn the_grid_pad_right_aligns_and_never_closes_the_gap() {
        // Room to spare: text + pad + grid fills the row exactly.
        let pad = week_grid_pad(10, 60);
        assert_eq!(10 + pad.width() + WEEK_GRID_WIDTH, 60);

        // No room at all — the pad floors at the gap rather than vanishing.
        assert_eq!(week_grid_pad(60, 60).width(), WEEK_GRID_GAP);
        assert_eq!(week_grid_pad(0, 0).width(), WEEK_GRID_GAP);
    }

    /// The text budget floors above zero, so a very narrow pane clips a title to
    /// an ellipsis rather than to nothing at all.
    #[test]
    fn the_text_budget_floors_instead_of_reaching_zero() {
        assert_eq!(week_text_budget(80), 80 - WEEK_GRID_WIDTH - WEEK_GRID_GAP);
        assert_eq!(week_text_budget(0), WEEK_TITLE_FLOOR);
        assert_eq!(week_text_budget(WEEK_GRID_WIDTH), WEEK_TITLE_FLOOR);
    }

    /// Clipping is span-aware: spans that fit keep their own styles, the one
    /// straddling the budget is cut with an ellipsis, and the rest are dropped.
    #[test]
    fn clipping_keeps_whole_spans_and_cuts_the_straddling_one() {
        let spans = vec![
            Span::styled("ab", Style::new().fg(Color::Red)),
            Span::styled("cdefgh", Style::new().fg(Color::Blue)),
            Span::raw("ignored"),
        ];
        let clipped = clip_spans(spans, 5);

        assert_eq!(spans_width(&clipped), 5);
        assert_eq!(clipped.len(), 2, "the third span is past the budget");
        assert_eq!(clipped[0].content, "ab");
        assert_eq!(clipped[0].style.fg, Some(Color::Red));
        assert!(
            clipped[1].content.ends_with('…'),
            "{:?}",
            clipped[1].content
        );
        assert_eq!(clipped[1].style.fg, Some(Color::Blue));
    }

    /// Zero room spends no ellipsis. `truncate` answers `"…"` even at width 0, and
    /// trusting it would overflow the very budget being enforced by a cell —
    /// enough to push the grid's last column off the row.
    #[test]
    fn clipping_at_zero_room_drops_the_span_rather_than_overflowing() {
        let spans = vec![Span::raw("ab"), Span::raw("cd")];
        let clipped = clip_spans(spans, 2);
        assert_eq!(spans_width(&clipped), 2);
        assert_eq!(clipped.len(), 1, "no ellipsis span past the budget");
    }

    /// A run that already fits is returned untouched — clipping must not spend an
    /// ellipsis on a row that had room.
    #[test]
    fn clipping_leaves_a_run_that_fits_alone() {
        let spans = vec![Span::raw("abc"), Span::raw("de")];
        let clipped = clip_spans(spans, 5);
        assert_eq!(clipped.len(), 2);
        assert_eq!(spans_width(&clipped), 5);
    }

    /// `week_offset` must count exactly the rows `week_spread` inserts, or the
    /// cursor lands on a header. Derived by building the spread and finding the
    /// row, never by restating the arithmetic.
    #[test]
    fn the_cursor_offset_counts_the_headers_the_spread_inserts() {
        let theme = Theme::from_flavor("mocha");
        let grid = WeekGrid {
            start: NaiveDate::from_ymd_opt(2026, 8, 17).expect("valid date"),
            today_column: Some(0),
            inner_width: 60,
        };
        // Two pool rows then two scheduled ones, and every cursor position over
        // them — including the first row of each block, where the offset changes.
        for (pool_rows, pool_title) in [(2usize, Some("Work")), (0, Some("Work")), (0, None)] {
            let rows: Vec<ListItem> = (0..4)
                .map(|i| ListItem::new(Line::from(format!("row{i}"))))
                .collect();
            for p in 0..rows.len() {
                let (out, selected) =
                    week_spread(rows.clone(), Some(p), pool_rows, pool_title, &grid, &theme);
                let at = selected.expect("a cursor in, a cursor out");
                assert_eq!(
                    out[at].clone(),
                    ListItem::new(Line::from(format!("row{p}"))),
                    "pool_rows {pool_rows}, title {pool_title:?}, p {p}"
                );
            }
        }
    }
}
