//! Rendering. A pure `view(&Model)` over ratatui. btop structural language
//! (rounded panels) with a Catppuccin palette (ADR-0006). Two-pane: List
//! sidebar + task pane. The `?` overlay is drawn straight from the keymap table.

pub mod theme;
pub mod widgets;

use ratatui::layout::{Constraint, Direction, Flex, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use ratatui::Frame;

use crate::app::{Focus, Model};
use crate::keymap;
use theme::Theme;

/// Render the whole frame. Never mutates state.
pub fn view(model: &Model, theme: &Theme, frame: &mut Frame) {
    let area = frame.area();

    // Paint the window background first.
    frame.render_widget(Block::default().style(Style::new().bg(theme.base)), area);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(area);
    render_pane(
        frame,
        panes[0],
        "Lists",
        model.focus == Focus::Sidebar,
        theme,
    );
    render_pane(frame, panes[1], "Tasks", model.focus == Focus::Tasks, theme);

    if model.show_help {
        render_help(frame, area, theme);
    }
}

fn render_pane(frame: &mut Frame, area: Rect, title: &str, focused: bool, theme: &Theme) {
    let border_color = if focused { theme.accent } else { theme.surface };
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(theme.base).fg(theme.text));
    frame.render_widget(block, area);
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
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title("Help")
        .border_style(Style::new().fg(theme.accent))
        .style(Style::new().bg(theme.base).fg(theme.subtext));
    frame.render_widget(Paragraph::new(rows).block(block), popup);
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
