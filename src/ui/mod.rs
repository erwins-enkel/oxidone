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

use crate::app::{Focus, Model};
use crate::keymap;
use theme::Theme;

/// Render the whole frame. Never mutates state.
pub fn view(model: &Model, theme: &Theme, frame: &mut Frame) {
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
    render_pane(frame, panes[1], "Tasks", model.focus == Focus::Tasks, theme);
    render_status(frame, status, model, theme);

    if model.show_help {
        render_help(frame, area, theme);
    }
}

fn render_sidebar(frame: &mut Frame, area: Rect, model: &Model, theme: &Theme) {
    let focused = model.focus == Focus::Sidebar;
    let items: Vec<ListItem> = model
        .lists
        .iter()
        .map(|l| ListItem::new(l.title.clone()))
        .collect();
    let list = List::new(items)
        .block(panel("Lists", focused, theme))
        .style(Style::new().bg(theme.base).fg(theme.text))
        .highlight_style(
            Style::new()
                .fg(theme.accent)
                .add_modifier(Modifier::REVERSED),
        )
        .highlight_symbol("› ");

    let mut state = ListState::default();
    state.select(model.selected_list);
    frame.render_stateful_widget(list, area, &mut state);
}

fn render_pane(frame: &mut Frame, area: Rect, title: &str, focused: bool, theme: &Theme) {
    frame.render_widget(panel(title, focused, theme), area);
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
