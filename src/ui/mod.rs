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

use chrono::NaiveDate;
use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{Focus, Model, Overlay};
use crate::dateparse::format_due_relative;
use crate::domain::{Status, Task};
use crate::keymap;
use theme::Theme;
use widgets::{dueload, meter};

/// Days of "workload ahead" bucketed into the due-load strip (today + 6).
const DUE_LOAD_DAYS: usize = 7;
/// Braille/ASCII cells the header completion meter occupies.
const HEADER_METER_WIDTH: u16 = 10;

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
    render_sidebar(frame, panes[0], model, theme);
    render_task_pane(frame, panes[1], model, ascii, theme);
    render_status(frame, status, model, theme);
    render_legend(frame, legend, model, theme);

    if model.show_help {
        render_help(frame, area, theme);
    }
    if let Some(overlay) = &model.overlay {
        render_overlay(frame, area, overlay, theme);
    }
}

fn render_overlay(frame: &mut Frame, area: Rect, overlay: &Overlay, theme: &Theme) {
    let (title, body): (&str, String) = match overlay {
        Overlay::EditTitle { buffer, .. } => ("Edit title", format!("{buffer}▏")),
        Overlay::AddTask { buffer } => ("Add task", format!("{buffer}▏")),
        Overlay::AddSubtask { buffer, .. } => ("Add subtask", format!("{buffer}▏")),
        Overlay::EditDue { buffer, .. } => ("Edit due date (blank clears)", format!("{buffer}▏")),
        Overlay::EditNotes { buffer, .. } => ("Edit notes (blank clears)", format!("{buffer}▏")),
        Overlay::AddList { buffer } => ("Add list", format!("{buffer}▏")),
        Overlay::RenameList { buffer, .. } => ("Rename list", format!("{buffer}▏")),
        Overlay::Confirm(confirm) => ("Confirm", confirm.prompt.clone()),
    };
    let popup = centered(area, 50, 3);
    frame.render_widget(Clear, popup);
    frame.render_widget(Paragraph::new(body).block(panel(title, true, theme)), popup);
}

fn render_sidebar(frame: &mut Frame, area: Rect, model: &Model, theme: &Theme) {
    let focused = model.focus == Focus::Sidebar;
    let items: Vec<ListItem> = model
        .lists
        .iter()
        .map(|l| ListItem::new(l.title.clone()))
        .collect();
    render_selectable(
        frame,
        area,
        "Lists",
        items,
        model.selected_list,
        focused,
        theme,
    );
}

/// Width of the leading due-date column. Derived from the formatter's own
/// contract rather than restated here, so the column can never be narrower than
/// what `format_due_relative` may emit.
const DUE_WIDTH: usize = crate::dateparse::MAX_RENDERED_WIDTH;

/// Indent prefix for a Subtask row (nesting is capped at one level).
const SUBTASK_INDENT: &str = "  ";

/// Style for a Task's due-date cell. Overdue reads in the palette's red so it
/// catches the eye when scanning the column — but Completed wins: a done Task
/// is settled, so its date stays dim alongside the struck-through title.
fn due_style(task: &Task, today: NaiveDate, theme: &Theme) -> Style {
    let overdue = task.status != Status::Completed && task.due.is_some_and(|d| d < today);
    Style::new().fg(if overdue {
        theme.overdue
    } else {
        theme.subtext
    })
}

fn render_task_pane(frame: &mut Frame, area: Rect, model: &Model, ascii: bool, theme: &Theme) {
    let focused = model.focus == Focus::Tasks;
    // The displayed rows are a read-only lens over `tasks`: the current sort's
    // order with Completed Tasks filtered out unless revealed. `tasks` (Manual
    // order) stays untouched, and the header meter still counts over all of it.
    let ordered = model.visible_tasks();
    // Due dates lead the row in a fixed-width gutter so they scan vertically.
    // The gutter only exists when something in view has a due date — otherwise
    // every title would sit behind a column of blanks.
    let due_gutter = ordered.iter().any(|t| t.due.is_some());
    // Overdue is a property of the date against today, decided here in the view
    // — `model.now` keeps that testable rather than reading the wall clock.
    let today = model.now.date_naive();
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
                let due = match t.due {
                    Some(d) => format_due_relative(d, today),
                    None => String::new(),
                };
                spans.push(Span::styled(
                    format!("{due:<DUE_WIDTH$}  "),
                    due_style(t, today, theme),
                ));
            }
            // Subtasks sit indented under their parent so the hierarchy reads.
            if t.is_subtask() {
                spans.push(Span::raw(SUBTASK_INDENT));
            }
            spans.push(Span::styled(t.title.clone(), style));
            ListItem::new(Line::from(spans))
        })
        .collect();

    // `selected_task` indexes `tasks`; translate it to the cursor's position in
    // the displayed (sorted) order so the highlight tracks the same Task by id.
    let selected = model
        .selected_task
        .and_then(|i| model.tasks.get(i))
        .and_then(|sel| ordered.iter().position(|t| t.id == sel.id));

    let base = match model.sort.label() {
        Some(label) => format!("Tasks — {label}"),
        None => "Tasks".to_string(),
    };
    // Inline btop-style data widgets in the header: a completion meter for the
    // active List and a due-load strip. Both drop out (never the text) when the
    // pane is too narrow — braille degrades before the title (ADR-0006).
    let inner_width = area.width.saturating_sub(2); // rounded borders
    let title = header_title(&base, model, inner_width, ascii);
    render_selectable(frame, area, &title, items, selected, focused, theme);
}

/// Compose the task-pane header: the base title, then — only while they fit — a
/// completion meter (`done/total` of the active List) and a due-load strip.
/// Widgets are added greedily and dropped before the text on a narrow pane.
fn header_title(base: &str, model: &Model, inner_width: u16, ascii: bool) -> String {
    let inner = inner_width as usize;
    let mut title = base.to_string();

    // Completion meter for the active List (done / total of the loaded Tasks).
    let total = model.tasks.len();
    if total > 0 {
        let done = model
            .tasks
            .iter()
            .filter(|t| t.status == Status::Completed)
            .count();
        let bar = meter::render(done, total, HEADER_METER_WIDTH, ascii);
        let segment = format!("  {bar} {done}/{total}");
        if title.chars().count() + segment.chars().count() <= inner {
            title.push_str(&segment);
        }
    }

    // Due-load strip: workload ahead over the next `DUE_LOAD_DAYS` days.
    let counts = due_load_counts(&model.tasks, model.now, DUE_LOAD_DAYS);
    if counts.iter().any(|&c| c > 0) {
        let strip = dueload::render(&counts, ascii);
        let segment = format!("  {strip}");
        if title.chars().count() + segment.chars().count() <= inner {
            title.push_str(&segment);
        }
    }

    title
}

/// Bucket incomplete Tasks by due date into `days` daily buckets of "workload
/// ahead": `[0]` = due today (and anything overdue, folded forward), `[1]` =
/// tomorrow, ... Completed Tasks and Tasks with no due date are excluded.
fn due_load_counts(
    tasks: &[Task],
    now: chrono::DateTime<chrono::Local>,
    days: usize,
) -> Vec<usize> {
    let today = now.date_naive();
    let mut counts = vec![0usize; days];
    for task in tasks {
        if task.status == Status::Completed {
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
        .highlight_symbol(if focused { "› " } else { "  " });

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
    match &model.overlay {
        Some(Overlay::Confirm(_)) => keymap::LegendContext::Confirm,
        Some(_) => keymap::LegendContext::TextInput,
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
    fn drawn(size: (u16, u16), rows: &[HelpRow]) -> (Vec<String>, HelpLayout) {
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
        (lines, help_layout(frame_of(size), rows))
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
                    .find_map(|line| line.find(&needle))
                    .unwrap_or_else(|| panic!("column {c} not found in the buffer"))
            })
            .collect()
    }

    #[test]
    fn every_row_is_drawn_padded_and_column_aligned() {
        let rows = help_rows();
        let (lines, layout) = drawn(MIN_TERM, &rows);
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
                    .find_map(|line| line.find(&needle))
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
        let rows = help_rows();
        let (lines, layout) = drawn(MIN_TERM, &rows);
        let theme = Theme::from_flavor("mocha");

        let offsets = column_offsets(&lines, &layout, &rows);
        let first = &layout.column_rows(0, &rows)[0];
        let needle = cell_text(first, &layout, 0);
        let y = lines
            .iter()
            .position(|line| line.contains(&needle))
            .expect("first cell present");

        let mut terminal =
            Terminal::new(TestBackend::new(MIN_TERM.0, MIN_TERM.1)).expect("TestBackend terminal");
        terminal
            .draw(|frame| draw_help(frame, frame_of(MIN_TERM), &rows, &theme))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();

        // The label sits one cell past the leading space; the help follows the
        // label's field and its separating space.
        let label_x = offsets[0] + 1;
        let help_x = offsets[0] + 1 + layout.cols[0].label + 1;
        assert_eq!(
            buffer[(label_x as u16, y as u16)].fg,
            theme.accent,
            "key label lost its accent"
        );
        assert_eq!(
            buffer[(help_x as u16, y as u16)].fg,
            theme.text,
            "help text is not the body colour"
        );
    }

    #[test]
    fn a_short_frame_draws_what_fits_and_says_what_it_dropped() {
        // Two columns with a hidden tail — the one regime where the layout's
        // partition and the renderer's could disagree without either looking
        // wrong on its own.
        let rows = help_rows();
        let size = (80, 12);
        let (lines, layout) = drawn(size, &rows);

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

        // The tail is absent, and the popup says so.
        let shown = layout.cols.len() * layout.rows_per_col;
        for (label, help) in &rows[shown..] {
            assert!(
                !lines.iter().any(|line| line.contains(help.trim())),
                "hidden row {label:?} was drawn anyway"
            );
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
        let (lines, layout) = drawn(size, &rows);

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
        let (lines, layout) = drawn(MIN_TERM, &rows);

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
        Task {
            id: TaskId("t".into()),
            list: ListId("l".into()),
            parent: None,
            title: "t".into(),
            notes: None,
            status,
            due,
            completed_at: None,
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

        model.overlay = Some(Overlay::AddTask {
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
}
