//! Rendering. A pure `view(&Model)` over ratatui. btop structural language
//! (rounded panels) with a Catppuccin palette (ADR-0006). Two-pane: List
//! sidebar + task pane, a one-line status bar, and an always-visible hotkey
//! legend below it. Both the `?` overlay and the legend are drawn straight from
//! the keymap table — the legend as a curated, priority-ordered subset.

pub mod theme;
pub mod widgets;

use chrono::NaiveDate;
use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{renders_as_subtask, Focus, Model, Overlay};
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
    // Built once per render: the per-row indent check is then a hash lookup, not
    // a scan of every Task.
    let top_level = model.top_level_ids();
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
            // An orphan (parent gone) draws flush-left rather than claiming the
            // row above it as its parent.
            if renders_as_subtask(&top_level, t) {
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

    let base = format!("Tasks — {}", model.sort.label());
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

fn render_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let rows: Vec<Line> = keymap::bindings()
        .iter()
        .map(|b| {
            Line::from(vec![
                Span::styled(
                    format!(" {:<5} ", keymap::key_label(b.key)),
                    Style::new().fg(theme.accent),
                ),
                Span::styled(b.help, Style::new().fg(theme.text)),
            ])
        })
        .collect();

    let popup = centered(area, 44, rows.len() as u16 + 2);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(rows).block(panel("Help", true, theme)),
        popup,
    );
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
