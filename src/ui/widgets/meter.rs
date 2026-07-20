//! Completion meter: a braille-cell progress bar of `done / total`, giving 8x
//! horizontal resolution over a block bar. Drawn three ways: the active List in
//! the task-pane header, each List in the sidebar, and each parent Task's
//! Subtasks on its own row.
//!
//! The sidebar's counts come from a cache aggregate, so they cover the Lists
//! whose Tasks have been mirrored; fetching counts for a List never opened on
//! this machine is a follow-up.
//!
//! Braille encodes DATA only (ADR-0006): the bar is the completion ratio, never
//! decoration, and degrades to an ASCII `#`/`-` block bar when `ascii_fallback`
//! is set.

/// The empty braille cell, U+2800.
const BRAILLE_BLANK: char = '\u{2800}';
/// A fully-lit braille cell (all 8 dots), U+28FF.
const BRAILLE_FULL: char = '\u{28FF}';

/// Braille cells for a partial fill of 0..=8 dots within one cell. The dots are
/// lit to read as a rising bar: the left dot-column fills top-to-bottom, then the
/// right column, so `[n]` is a monotically fuller glyph as `n` grows.
///
/// Dot bit values (added to U+2800): dot1=0x01 dot2=0x02 dot3=0x04 dot7=0x40
/// (left column, top→bottom); dot4=0x08 dot5=0x10 dot6=0x20 dot8=0x80 (right).
const PARTIAL: [char; 9] = [
    BRAILLE_BLANK, // 0 dots
    '\u{2801}',    // dot1
    '\u{2803}',    // dot1..2
    '\u{2807}',    // dot1..3
    '\u{2847}',    // dot1..3 + dot7 (left column full)
    '\u{284F}',    // + dot4
    '\u{285F}',    // + dot5
    '\u{287F}',    // + dot6
    BRAILLE_FULL,  // + dot8 (all 8)
];

/// Render `done / total` into `width` cells of braille (or ASCII on fallback).
///
/// Braille packs 8 dots per cell, so a `width`-cell bar resolves the ratio to
/// `width * 8` steps: whole cells fill solid, and the boundary cell shows the
/// sub-cell remainder. The ASCII fallback is a coarser `#`/`-` block bar of the
/// same `width`. `total == 0` (an empty List) renders an all-empty bar rather
/// than dividing by zero. `width == 0` renders an empty string.
pub fn render(done: usize, total: usize, width: u16, ascii: bool) -> String {
    let width = width as usize;
    if width == 0 {
        return String::new();
    }
    // Clamp so an over-count (shouldn't happen) can't overflow the bar.
    let done = done.min(total);

    if ascii {
        // Coarse block bar: round the ratio to whole cells.
        let filled = if total == 0 {
            0
        } else {
            ((done * width * 2 + total) / (total * 2)).min(width) // round to nearest
        };
        return "#".repeat(filled) + &"-".repeat(width - filled);
    }

    // Braille: work in eighth-of-a-cell dots across the whole bar.
    let steps = width * 8;
    let lit = if total == 0 {
        0
    } else {
        ((done * steps * 2 + total) / (total * 2)).min(steps) // round to nearest dot
    };
    let full_cells = lit / 8;
    let remainder = lit % 8;

    let mut bar = String::with_capacity(width);
    for _ in 0..full_cells {
        bar.push(BRAILLE_FULL);
    }
    if full_cells < width {
        bar.push(PARTIAL[remainder]);
        for _ in (full_cells + 1)..width {
            bar.push(BRAILLE_BLANK);
        }
    }
    bar
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_width_is_empty() {
        assert_eq!(render(3, 10, 0, false), "");
        assert_eq!(render(3, 10, 0, true), "");
    }

    #[test]
    fn empty_list_renders_blank_bar() {
        // total == 0 must not divide by zero; the bar is all-empty.
        let braille = render(0, 0, 4, false);
        assert_eq!(braille.chars().count(), 4);
        assert!(braille.chars().all(|c| c == BRAILLE_BLANK));

        let ascii = render(0, 0, 4, true);
        assert_eq!(ascii, "----");
    }

    #[test]
    fn zero_percent_is_all_empty() {
        let braille = render(0, 10, 6, false);
        assert_eq!(braille.chars().count(), 6);
        assert!(braille.chars().all(|c| c == BRAILLE_BLANK));
        assert_eq!(render(0, 10, 6, true), "------");
    }

    #[test]
    fn hundred_percent_is_all_full() {
        let braille = render(10, 10, 6, false);
        assert_eq!(braille.chars().count(), 6);
        assert!(braille.chars().all(|c| c == BRAILLE_FULL));
        assert_eq!(render(10, 10, 6, true), "######");
    }

    #[test]
    fn fifty_percent_fills_half() {
        // 8 cells, half done => 4 full cells then empties (braille).
        let braille = render(1, 2, 8, false);
        assert_eq!(braille.chars().count(), 8);
        let full = braille.chars().filter(|&c| c == BRAILLE_FULL).count();
        assert_eq!(full, 4);
        assert!(braille.chars().skip(4).all(|c| c == BRAILLE_BLANK));

        assert_eq!(render(1, 2, 8, true), "####----");
    }

    #[test]
    fn width_is_always_honored() {
        for w in 1u16..=32 {
            for (done, total) in [(0, 10), (3, 10), (7, 9), (10, 10), (0, 0)] {
                assert_eq!(render(done, total, w, false).chars().count(), w as usize);
                assert_eq!(render(done, total, w, true).chars().count(), w as usize);
            }
        }
    }

    #[test]
    fn braille_resolves_sub_cell_fractions() {
        // A single-cell bar at 1/8 lights the first partial glyph, not blank and
        // not full: braille's whole point over an ASCII block bar (which rounds
        // 1/8 down to empty).
        let braille = render(1, 8, 1, false);
        assert_eq!(braille.chars().count(), 1);
        let c = braille.chars().next().unwrap();
        assert_ne!(c, BRAILLE_BLANK);
        assert_ne!(c, BRAILLE_FULL);

        assert_eq!(render(1, 8, 1, true), "-"); // ASCII rounds the sub-cell away
    }

    #[test]
    fn over_count_is_clamped() {
        // done > total can't overflow the bar; it caps at full.
        let braille = render(15, 10, 4, false);
        assert_eq!(braille.chars().count(), 4);
        assert!(braille.chars().all(|c| c == BRAILLE_FULL));
    }
}
