//! Due-load histogram: braille bars of Task counts per upcoming day ("workload
//! ahead"), derived from cached due dates. Rides in the task pane's title bar.
//!
//! Braille encodes DATA only (ADR-0006): each dot-column is one day's count, its
//! height the scaled load. A braille cell is 2 columns wide and 4 dots tall, so
//! one glyph packs two days at four vertical levels — btop's data density. On
//! `ascii_fallback` it degrades to a one-char-per-day ASCII ramp.

/// The empty braille cell, U+2800.
const BRAILLE_BLANK: char = '\u{2800}';

/// Left dot-column bar masks for heights 0..=4 (dots 7,3,2,1 filling bottom→top).
/// Added to U+2800: dot7=0x40 dot3=0x04 dot2=0x02 dot1=0x01.
const LEFT: [u32; 5] = [0x00, 0x40, 0x44, 0x46, 0x47];
/// Right dot-column bar masks for heights 0..=4 (dots 8,6,5,4 bottom→top).
/// Added to U+2800: dot8=0x80 dot6=0x20 dot5=0x10 dot4=0x08.
const RIGHT: [u32; 5] = [0x00, 0x80, 0xA0, 0xB0, 0xB8];

/// Braille bars are 4 dots tall.
const BRAILLE_LEVELS: usize = 4;

/// One char per day, low→high load. Index by the scaled height 0..=4.
const ASCII_RAMP: [char; 5] = [' ', '.', ':', '+', '#'];

/// `counts[0]` = due today, `counts[1]` = +1 day, ... Rendered as a braille strip
/// (or an ASCII ramp on fallback).
///
/// Heights are scaled to the busiest day: the max maps to a full bar, and every
/// non-zero day shows at least one level so real load is never rounded to
/// nothing. An empty slice (no upcoming Tasks) renders an empty string. When all
/// counts are zero the strip is blank cells (braille) or spaces (ASCII).
pub fn render(counts: &[usize], ascii: bool) -> String {
    if counts.is_empty() {
        return String::new();
    }
    let max = counts.iter().copied().max().unwrap_or(0);
    let heights: Vec<usize> = counts.iter().map(|&c| scale(c, max)).collect();

    if ascii {
        return heights.iter().map(|&h| ASCII_RAMP[h]).collect();
    }

    // Pack two days per braille cell: left column = even day, right = the next.
    let mut strip = String::with_capacity(heights.len().div_ceil(2));
    for pair in heights.chunks(2) {
        let left = LEFT[pair[0]];
        let right = pair.get(1).map_or(0, |&h| RIGHT[h]);
        strip.push(cell(left | right));
    }
    strip
}

/// Scale a raw count against the busiest day to a bar height in `0..=4`. Zero
/// stays zero; any non-zero count rounds *up* to at least one level so it stays
/// visible.
fn scale(count: usize, max: usize) -> usize {
    if count == 0 || max == 0 {
        return 0;
    }
    (count * BRAILLE_LEVELS).div_ceil(max).min(BRAILLE_LEVELS)
}

/// Build a braille glyph from a dot mask (0x00..=0xFF over U+2800).
fn cell(mask: u32) -> char {
    char::from_u32(0x2800 + mask).unwrap_or(BRAILLE_BLANK)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_empty_string() {
        assert_eq!(render(&[], false), "");
        assert_eq!(render(&[], true), "");
    }

    #[test]
    fn all_zero_renders_blank() {
        let braille = render(&[0, 0, 0, 0], false);
        assert_eq!(braille.chars().count(), 2); // two days per cell
        assert!(braille.chars().all(|c| c == BRAILLE_BLANK));

        assert_eq!(render(&[0, 0, 0, 0], true), "    ");
    }

    #[test]
    fn single_tall_bar_is_full_height() {
        // One day, busy: its column fills to the top (4 dots).
        let braille = render(&[9], false);
        assert_eq!(braille.chars().count(), 1);
        let c = braille.chars().next().unwrap();
        assert_eq!(c, cell(LEFT[4])); // left column full, right empty

        assert_eq!(render(&[9], true), "#");
    }

    #[test]
    fn ascii_ramp_is_one_char_per_day() {
        assert_eq!(render(&[0, 1, 2, 3, 8], true).chars().count(), 5);
    }

    #[test]
    fn mixed_counts_scale_to_the_busiest_day() {
        // max = 8 => 8 maps to full (4), 4 to half (2), 2 to one level, 0 to none.
        assert_eq!(render(&[0, 2, 4, 8], true), " .:#");
    }

    #[test]
    fn nonzero_load_never_rounds_to_nothing() {
        // A single Task against a very busy day still shows one level, not blank.
        assert_eq!(scale(1, 100), 1);
        assert_eq!(render(&[100, 1], true), "#.");
    }

    #[test]
    fn braille_packs_two_days_per_cell() {
        // Five days => ceil(5/2) = 3 cells.
        assert_eq!(render(&[1, 2, 3, 4, 5], false).chars().count(), 3);
        // Four days => 2 cells.
        assert_eq!(render(&[1, 2, 3, 4], false).chars().count(), 2);
    }

    #[test]
    fn odd_trailing_day_leaves_right_column_empty() {
        // Three days: last cell has only its left column lit.
        let braille = render(&[4, 4, 4], false);
        assert_eq!(braille.chars().count(), 2);
        let last = braille.chars().nth(1).unwrap();
        assert_eq!(last, cell(LEFT[4])); // right column empty
    }
}
