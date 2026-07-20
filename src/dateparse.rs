//! Pure due-date parser. Accepts natural language (`today`, `tomorrow`, `mon`,
//! `+3d`) via `interim` and ISO `YYYY-MM-DD`, resolves everything in the caller's
//! reference timezone, and strips any time component down to a `chrono::NaiveDate`
//! (CONTEXT.md: a due date is a date, never a time).
//!
//! No I/O and no clock of its own: the timezone-aware entry point takes an
//! explicit `now`, so relative expressions and local-boundary behaviour are
//! resolved by the caller (the runtime stamps the clock at the impure edge) and
//! are deterministically testable without touching the machine clock.

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
