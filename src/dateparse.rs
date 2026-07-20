//! Pure due-date parsing and display. Parsing accepts natural language (`today`,
//! `tomorrow`, `mon`, `+3d`) via `interim` and ISO `YYYY-MM-DD`, resolves
//! everything in the caller's reference timezone, and strips any time component
//! down to a `chrono::NaiveDate` (CONTEXT.md: a due date is a date, never a
//! time). Display is the inverse: a date rendered relative to a reference day.
//!
//! No I/O and no clock of its own: both entry points take an explicit reference
//! (`now` / `today`), so relative expressions, local-boundary behaviour and
//! relative rendering are resolved by the caller (the runtime stamps the clock at
//! the impure edge) and are deterministically testable without touching the
//! machine clock.

use chrono::{DateTime, NaiveDate, TimeZone};
use interim::{parse_date_string, Dialect};

/// The input could not be understood as a due date. Carries the offending text
/// so callers can surface it on the status line.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("could not parse due date: {0:?}")]
pub struct DueParseError(pub String);

/// Parse a due date, resolving relative expressions against `now` and returning
/// the date in `now`'s timezone. This is the pure test seam: pass a fixed `now`
/// (in any `TimeZone`) to exercise natural-language and local-boundary cases
/// deterministically.
///
/// Recognises, in order: ISO `YYYY-MM-DD` (unambiguous, date-only fast path),
/// then `interim`'s natural language (`today`, `tomorrow`, weekday names, `+3d`,
/// month names, …). Any time component the parser infers is discarded.
pub fn parse_due_relative_to<Tz: TimeZone>(
    input: &str,
    now: DateTime<Tz>,
) -> Result<NaiveDate, DueParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DueParseError(input.to_string()));
    }
    // ISO fast path: unambiguous and already date-only, so it never depends on
    // `now` or the dialect.
    if let Ok(date) = NaiveDate::parse_from_str(trimmed, "%Y-%m-%d") {
        return Ok(date);
    }
    // `interim` reads a bare `3d` as a relative offset but rejects the `+3d`
    // shorthand (it treats a leading `+` as a dangling duration with no base
    // date). Strip one leading `+` so both spellings mean "3 days from now".
    let relative = trimmed.strip_prefix('+').map_or(trimmed, str::trim_start);
    // Natural language, resolved in `now`'s timezone; `date_naive` strips the
    // time in that same zone (never a UTC-shifted date).
    parse_date_string(relative, now, Dialect::Uk)
        .map(|dt| dt.date_naive())
        .map_err(|_| DueParseError(input.to_string()))
}

/// Split a capture buffer into a display title and an optional due date by
/// peeling a trailing natural-language date phrase off the end (`Launch website
/// 3d` → `("Launch website", Some(today + 3))`). The date is resolved against
/// `now`, exactly as [`parse_due_relative_to`] — same test seam, same timezone
/// rules.
///
/// It peels the **longest trailing word-suffix** that both looks like a date
/// ([`looks_like_date_phrase`]) and parses, while leaving at least one word in
/// the title. `interim` rejects a candidate that opens with a non-date word
/// (`Bob tomorrow`, `report May`), so scanning longest-first cannot swallow
/// title words. When nothing peels — including when the whole buffer is a date,
/// since the first word must stay — the trimmed buffer is the title and there is
/// no due date.
pub fn split_title_and_due<Tz: TimeZone>(
    input: &str,
    now: DateTime<Tz>,
) -> (String, Option<NaiveDate>) {
    let trimmed = input.trim();
    // Byte offset where each word begins. Word 0 is never a candidate start, so
    // the title keeps at least one word.
    let word_starts = word_start_offsets(trimmed);
    for &offset in word_starts.iter().skip(1) {
        let candidate = &trimmed[offset..];
        if looks_like_date_phrase(candidate) {
            if let Ok(date) = parse_due_relative_to(candidate, now.clone()) {
                let title = trimmed[..offset].trim_end().to_string();
                return (title, Some(date));
            }
        }
    }
    (trimmed.to_string(), None)
}

/// Byte offsets where each whitespace-separated word begins, in order.
fn word_start_offsets(s: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut prev_ws = true;
    for (i, c) in s.char_indices() {
        if !c.is_whitespace() && prev_ws {
            starts.push(i);
        }
        prev_ws = c.is_whitespace();
    }
    starts
}

/// The false-positive gate for [`split_title_and_due`]: is this trailing
/// candidate specific enough to *mean* a date? `interim` will happily read a
/// bare month name as the first of that month and a bare number as a (year)
/// date, which would silently eat ordinary title words (`Prep for May`, `Buy
/// milk 2`). So a **single** token that is a bare month name or all digits is
/// rejected; every other single token (`3d`, `friday`, `tomorrow`, an ISO date)
/// and every multi-token candidate is left for the parser to accept or reject.
fn looks_like_date_phrase(candidate: &str) -> bool {
    let mut tokens = candidate.split_whitespace();
    let (Some(first), None) = (tokens.next(), tokens.next()) else {
        // Multi-token (or empty): let the parser be the judge.
        return true;
    };
    let bare_number = !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit());
    !(bare_number || is_month_name(first))
}

/// Whether `token` is an English month name or its common three-letter
/// abbreviation, case-insensitively — the words `interim` reads as a bare month.
fn is_month_name(token: &str) -> bool {
    const MONTHS: [&str; 23] = [
        "jan",
        "january",
        "feb",
        "february",
        "mar",
        "march",
        "apr",
        "april",
        "may",
        "jun",
        "june",
        "jul",
        "july",
        "aug",
        "august",
        "sep",
        "september",
        "oct",
        "october",
        "nov",
        "november",
        "dec",
        "december",
    ];
    let lower = token.to_ascii_lowercase();
    MONTHS.contains(&lower.as_str())
}

/// How far either side of `today` a due date still reads as a day count. Beyond
/// this an offset stops being legible ("in 43d" says less than a date), so the
/// absolute ISO date takes over.
const RELATIVE_HORIZON_DAYS: i64 = 7;

/// The widest string `format_due_relative` can return, in cells: the
/// `YYYY-MM-DD` fallback. Every relative form is shorter. Exported because the
/// task pane sizes its due column to it — this is the formatter's contract with
/// any caller laying dates out in a fixed width, and
/// `no_rendering_is_wider_than_the_iso_fallback` holds it to it.
pub const MAX_RENDERED_WIDTH: usize = 10;

/// Render `due` relative to `today`: `today`, `tomorrow`, `yesterday`, `in 3d`,
/// `3d ago` — falling back to ISO `YYYY-MM-DD` past `RELATIVE_HORIZON_DAYS` in
/// either direction. Pure, with `today` injected, so the view stays clock-free.
pub fn format_due_relative(due: NaiveDate, today: NaiveDate) -> String {
    match (due - today).num_days() {
        0 => "today".to_string(),
        1 => "tomorrow".to_string(),
        -1 => "yesterday".to_string(),
        d if (2..=RELATIVE_HORIZON_DAYS).contains(&d) => format!("in {d}d"),
        d if (-RELATIVE_HORIZON_DAYS..=-2).contains(&d) => format!("{}d ago", -d),
        _ => due.format("%Y-%m-%d").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{FixedOffset, TimeZone};

    /// A fixed reference clock: 2026-07-20 (a Monday) at 12:00 in UTC. Relative
    /// expressions resolve against this, so the table is deterministic.
    fn now() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 7, 20, 12, 0, 0)
            .unwrap()
    }

    fn ymd(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn parses_natural_language_and_iso() {
        let cases = [
            ("today", ymd(2026, 7, 20)),
            ("tomorrow", ymd(2026, 7, 21)),
            ("yesterday", ymd(2026, 7, 19)),
            ("+3d", ymd(2026, 7, 23)),
            ("+3 days", ymd(2026, 7, 23)),
            // 2026-07-20 is a Monday; the next Friday is the 24th.
            ("friday", ymd(2026, 7, 24)),
            // ISO, unambiguous and date-only.
            ("2026-08-01", ymd(2026, 8, 1)),
            ("2027-01-15", ymd(2027, 1, 15)),
        ];
        for (input, expected) in cases {
            assert_eq!(
                parse_due_relative_to(input, now()),
                Ok(expected),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn surrounding_whitespace_is_ignored() {
        assert_eq!(
            parse_due_relative_to("  2026-08-01  ", now()),
            Ok(ymd(2026, 8, 1))
        );
    }

    #[test]
    fn garbage_is_a_parse_error() {
        for input in ["", "   ", "not a date", "2026-13-99", "next lunar eclipse"] {
            assert!(
                parse_due_relative_to(input, now()).is_err(),
                "expected error for {input:?}"
            );
        }
    }

    #[test]
    fn a_time_component_is_stripped_to_the_date() {
        // interim accepts a trailing time; it must not leak into the result.
        assert_eq!(
            parse_due_relative_to("2026-08-01 18:30", now()),
            Ok(ymd(2026, 8, 1))
        );
    }

    #[test]
    fn formats_near_dates_as_day_offsets() {
        let today = ymd(2026, 7, 20);
        let cases = [
            (ymd(2026, 7, 20), "today"),
            (ymd(2026, 7, 21), "tomorrow"),
            (ymd(2026, 7, 19), "yesterday"),
            (ymd(2026, 7, 22), "in 2d"),
            (ymd(2026, 7, 18), "2d ago"),
            // The horizon itself is still relative, on both sides.
            (ymd(2026, 7, 27), "in 7d"),
            (ymd(2026, 7, 13), "7d ago"),
        ];
        for (due, expected) in cases {
            assert_eq!(format_due_relative(due, today), expected, "due {due}");
        }
    }

    #[test]
    fn formats_far_dates_as_absolute_iso() {
        let today = ymd(2026, 7, 20);
        // One day past the horizon, each way, and far out.
        assert_eq!(format_due_relative(ymd(2026, 7, 28), today), "2026-07-28");
        assert_eq!(format_due_relative(ymd(2026, 7, 12), today), "2026-07-12");
        assert_eq!(format_due_relative(ymd(2027, 1, 15), today), "2027-01-15");
    }

    /// The task pane lays due dates out in a fixed-width column sized to
    /// `MAX_RENDERED_WIDTH`, so nothing may render wider than that — a longer
    /// string would push the titles out of alignment. Asserted against the
    /// constant itself, so widening the column can't silently outrun the test.
    #[test]
    fn no_rendering_is_wider_than_the_iso_fallback() {
        let today = ymd(2026, 7, 20);
        for offset in -400..=400 {
            let due = today + chrono::Duration::days(offset);
            let rendered = format_due_relative(due, today);
            assert!(
                rendered.chars().count() <= MAX_RENDERED_WIDTH,
                "{rendered:?} (offset {offset}) exceeds the \
                 {MAX_RENDERED_WIDTH}-cell due column"
            );
        }
    }

    #[test]
    fn formats_across_a_month_boundary_by_elapsed_days_not_calendar_fields() {
        // 31 Jul → 2 Aug is two days, though the month and day-of-month both jump.
        assert_eq!(
            format_due_relative(ymd(2026, 8, 2), ymd(2026, 7, 31)),
            "in 2d"
        );
    }

    #[test]
    fn splits_a_trailing_date_off_the_title() {
        let cases = [
            (
                "Launch website 3d",
                "Launch website",
                Some(ymd(2026, 7, 23)),
            ),
            ("Call Bob tomorrow", "Call Bob", Some(ymd(2026, 7, 21))),
            // 2026-07-20 is a Monday; the next Tuesday is the 21st.
            (
                "Decide marketing campaign Tuesday",
                "Decide marketing campaign",
                Some(ymd(2026, 7, 21)),
            ),
            (
                "Book flight next friday",
                "Book flight",
                Some(ymd(2026, 7, 31)),
            ),
            ("Pay rent 2026-08-01", "Pay rent", Some(ymd(2026, 8, 1))),
            // Month + day is specific enough to peel; the day number stays with it.
            ("Party May 3", "Party", Some(ymd(2026, 5, 3))),
            // `N days` (number + unit) is a date just like the `3d` short form.
            ("Ship it 3 days", "Ship it", Some(ymd(2026, 7, 23))),
        ];
        for (input, title, due) in cases {
            assert_eq!(
                split_title_and_due(input, now()),
                (title.to_string(), due),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn a_bare_month_or_number_stays_in_the_title() {
        // interim would read these as the 1st of a month / a year; the gate keeps
        // them as ordinary words instead of silently dating the Task.
        for input in ["Prep for May", "Buy milk 2", "Sprint 17", "Plan june"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn a_whole_buffer_that_is_a_date_stays_the_title() {
        // The first word must remain, so there is nothing to peel a date from.
        for input in ["tomorrow", "friday", "3d"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    #[test]
    fn a_non_date_trailing_word_stays_in_the_title() {
        // No trailing suffix parses as a date, so nothing is peeled.
        assert_eq!(
            split_title_and_due("Build the widget", now()),
            ("Build the widget".to_string(), None)
        );
    }

    #[test]
    fn split_preserves_internal_title_whitespace_and_trims_edges() {
        // Only the trailing date is removed; the title's own spacing is intact.
        assert_eq!(
            split_title_and_due("  Two   spaces tomorrow  ", now()),
            ("Two   spaces".to_string(), Some(ymd(2026, 7, 21)))
        );
    }

    #[test]
    fn resolves_today_in_the_reference_timezone_not_utc() {
        // 01:00 at +05:00 is still 2026-03-10 locally, but 2026-03-09 in UTC.
        // "today" must follow the local (reference) zone, proving no UTC shift.
        let local = FixedOffset::east_opt(5 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 10, 1, 0, 0)
            .unwrap();
        assert_eq!(parse_due_relative_to("today", local), Ok(ymd(2026, 3, 10)));

        // Symmetrically, 22:00 at -05:00 is still the 9th locally though it is
        // the 10th in UTC.
        let west = FixedOffset::west_opt(5 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 3, 9, 22, 0, 0)
            .unwrap();
        assert_eq!(parse_due_relative_to("today", west), Ok(ymd(2026, 3, 9)));
    }
}
