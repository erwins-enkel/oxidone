//! Rendering. A pure `view(&Model)` over ratatui. btop structural language
//! (rounded panels) with a Catppuccin palette (ADR-0006). Two-pane: List
//! sidebar + task pane, with a one-line status bar. The `?` overlay is drawn
//! straight from the keymap table.

pub mod theme;
pub mod widgets;

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

    // Content row + a single status line at the bottom.
    let [content, status] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(content);
    render_sidebar(frame, panes[0], model, theme);
    render_task_pane(frame, panes[1], model, ascii, theme);
    render_status(frame, status, model, theme);

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
        Overlay::EditDue { buffer, .. } => ("Edit due date (blank clears)", format!("{buffer}▏")),
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

fn render_task_pane(frame: &mut Frame, area: Rect, model: &Model, ascii: bool, theme: &Theme) {
    let focused = model.focus == Focus::Tasks;
    // The current sort is a read-only lens over `tasks`: the display order comes
    // from `sorted_tasks()`, while `tasks` (Manual order) stays untouched.
    let ordered = model.sorted_tasks();
    // Due dates lead the row in a fixed-width gutter so they scan vertically.
    // The gutter only exists when something in view has a due date — otherwise
    // every title would sit behind a column of blanks.
    let due_gutter = ordered.iter().any(|t| t.due.is_some());
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
                    Some(d) => format_due_relative(d, model.now.date_naive()),
                    None => String::new(),
                };
                spans.push(Span::styled(
                    format!("{due:<DUE_WIDTH$}  "),
                    Style::new().fg(theme.subtext),
                ));
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
