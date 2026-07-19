//! Rendering. Pure `view(&Model)` over ratatui. btop structural language
//! (rounded panels, dense inline meters) with a Catppuccin palette (ADR-0006).
//! Two-pane: list sidebar + task pane; stat widgets ride inline in panel
//! headers/rows (not a separate panel in v1).

pub mod theme;
pub mod widgets;

use crate::app::Model;

/// Render the whole frame. Never mutates state.
pub fn view(_model: &Model /*, frame: &mut ratatui::Frame */) {
    // Layout: sidebar (lists + per-list completion meters)
    //       | task pane (tasks, subtasks indented; title bar carries due-load)
    //       + status line (transient messages, sort/filter indicators).
    todo!("compose the two-pane frame")
}
