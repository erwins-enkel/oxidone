//! Completion meter: a braille-cell progress bar of `done / total`, giving 8x
//! horizontal resolution over a block bar. Rides inline in sidebar list rows
//! and on parent-task rows (subtask progress).

/// Render `done/total` into `width` cells of braille (or ASCII on fallback).
pub fn render(_done: usize, _total: usize, _width: u16, _ascii: bool) -> String {
    todo!("map the fill ratio onto U+2800 columns; ASCII '#'/'-' fallback")
}
