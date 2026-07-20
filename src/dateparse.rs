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

/// How far either side of `today` a due date still reads as a day count. Beyond
/// this an offset stops being legible ("in 43d" says less than a date), so the
/// absolute ISO date takes over.
const RELATIVE_HORIZON_DAYS: i64 = 7;

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

    /// The task pane lays due dates out in a fixed-width column sized to the ISO
    /// fallback (`ui::DUE_WIDTH`), so no relative form may render wider than one
    /// — a longer string would push the titles out of alignment.
    #[test]
    fn no_rendering_is_wider_than_the_iso_fallback() {
        let today = ymd(2026, 7, 20);
        for offset in -400..=400 {
            let due = today + chrono::Duration::days(offset);
            let rendered = format_due_relative(due, today);
            assert!(
                rendered.chars().count() <= 10,
                "{rendered:?} (offset {offset}) exceeds the 10-cell due column"
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
