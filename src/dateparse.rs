//! Pure due-date parsing, arithmetic and display. Parsing accepts a bare
//! day-of-month (`15`), natural language (`today`, `tomorrow`, `mon`, `+3d`) via
//! `interim` — screened word by word against an exact date vocabulary, because
//! `interim` matches its own names by prefix and would otherwise read `milk` as
//! a date — and ISO `YYYY-MM-DD`; it resolves everything in the caller's
//! reference timezone and strips any time component down to a
//! `chrono::NaiveDate` (CONTEXT.md: a due date is a date, never a time). Display
//! is the inverse: a date rendered relative to a reference day. Between them sits
//! one computation *over* dates, [`shift_days`], for callers nudging a date a day
//! or a week at a time.
//!
//! No I/O and no clock of its own: the entry points that need a reference take an
//! explicit one (`now` / `today`), so relative expressions, local-boundary
//! behaviour and relative rendering are resolved by the caller (the runtime
//! stamps the clock at the impure edge) and are deterministically testable
//! without touching the machine clock.

use chrono::{DateTime, Datelike, NaiveDate, TimeZone};
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
/// Recognises, in order: a bare day-of-month 1–31 (`15` → the next 15th, on or
/// after today), ISO `YYYY-MM-DD` (unambiguous, date-only fast path), then
/// `interim`'s natural language (`today`, `tomorrow`, weekday names, `+3d`, month
/// names, …) — but only once every word in it is date vocabulary
/// ([`every_letter_run_is_a_date_word`]), since `interim` alone reads `milk` as a
/// date. Any time component the parser infers is discarded.
pub fn parse_due_relative_to<Tz: TimeZone>(
    input: &str,
    now: DateTime<Tz>,
) -> Result<NaiveDate, DueParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(DueParseError(input.to_string()));
    }
    // Bare day-of-month, ahead of the `+` strip below so a signed number keeps
    // whatever the paths after it make of it rather than becoming a day-of-month
    // (`+15` stays `interim`'s reading, year 15 — odd, but long-standing and not
    // this branch's to change). Two guards, deliberately:
    //
    //   * the ASCII-digit test, because `u32::from_str` accepts a leading sign —
    //     `"+15".parse()` succeeds, so a parse-only guard would swallow `+15`
    //     and `+3` into this branch;
    //   * the *fallible* parse, because the digit test bounds shape and not
    //     magnitude: an all-digit string can still overflow `u32`, and must fall
    //     through rather than reach an `unwrap`.
    if trimmed.bytes().all(|b| b.is_ascii_digit()) {
        if let Ok(day) = trimmed.parse::<u32>() {
            if let Some(date) = next_day_of_month(now.date_naive(), day) {
                return Ok(date);
            }
        }
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
    // `interim` matches its weekday, month and unit names on two- and
    // three-character *prefixes*, so it reads any number of ordinary English
    // words as dates (`milk` → minutes, `monitor` → Monday). Require every word
    // to be date vocabulary before handing it over, or a typo in this field
    // silently becomes today (#107).
    if !every_letter_run_is_a_date_word(relative) {
        return Err(DueParseError(input.to_string()));
    }
    // Natural language, resolved in `now`'s timezone; `date_naive` strips the
    // time in that same zone (never a UTC-shifted date).
    parse_date_string(relative, now, Dialect::Uk)
        .map(|dt| dt.date_naive())
        .map_err(|_| DueParseError(input.to_string()))
}

/// The next occurrence of `day` as a day-of-month, on or after `today` —
/// `None` if `day` is not a possible day-of-month at all.
///
/// Rolls forward: on 2026-07-22, `15` is 2026-08-15 while `25` is 2026-07-25 and
/// `22` is today. Due dates are overwhelmingly future-facing, so a bare number
/// that has already passed this month means next month's.
///
/// Month length is handled by `from_ymd_opt` returning `None` for a day the month
/// does not have, which makes the rule "the next month that *has* that day":
/// on 2026-02-05, `31` is 2026-03-31.
fn next_day_of_month(today: NaiveDate, day: u32) -> Option<NaiveDate> {
    if !(1..=31).contains(&day) {
        return None;
    }
    let (mut year, mut month) = (today.year(), today.month());
    // Bounded rather than looping until it lands: every day in 1..=31 occurs
    // within twelve months, and the bound keeps this total if the calendar (or
    // the range above) ever stops guaranteeing that.
    for _ in 0..12 {
        if let Some(candidate) = NaiveDate::from_ymd_opt(year, month, day) {
            if candidate >= today {
                return Some(candidate);
            }
        }
        (year, month) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
    }
    None
}

/// The date `delta` days from `base`, or `None` at the ends of the calendar.
///
/// Checked, not `+ Duration::days(delta)`, because `NaiveDate`'s `Add` panics on
/// overflow and callers step dates that came from outside this module — a parsed
/// buffer, a Task Google sent us. Whether any particular input can actually reach
/// `NaiveDate::MAX` is a property of whatever produced the date, not something a
/// step should have to know: the ends of the calendar are reachable in principle,
/// and a keystroke must never be able to take the TUI down. A caller that cannot
/// move simply does not move.
pub fn shift_days(base: NaiveDate, delta: i64) -> Option<NaiveDate> {
    base.checked_add_signed(chrono::Duration::days(delta))
}

/// Split a capture buffer into a display title and an optional due date by
/// peeling a trailing natural-language date phrase off the end (`Launch website
/// 3d` → `("Launch website", Some(today + 3))`). The date is resolved against
/// `now`, exactly as [`parse_due_relative_to`] — same test seam, same timezone
/// rules.
///
/// It peels the **longest trailing word-suffix** that both looks like a date
/// ([`looks_like_date_phrase`]) and parses, while leaving at least one word in
/// the title. The gate rejects a candidate containing any word that is not date
/// vocabulary (`Bob tomorrow`, `report May`), so scanning longest-first cannot
/// swallow title words — it is the gate that guarantees this and not `interim`,
/// which happily reads `Bob` as a date. When nothing peels — including when the
/// whole buffer is a date, since the first word must stay — the trimmed buffer is
/// the title and there is no due date.
///
/// A rejected candidate makes the scan retry a shorter suffix, so the gate
/// decides the *date* as well as the title: `Ship it 3 days from now` peels
/// nothing precisely because `now` is a time rather than a day, where a laxer
/// gate would fall back to it and stamp today.
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
/// candidate specific enough to *mean* a date?
///
/// It has to be an allowlist, because `interim` is far looser than it looks.
/// `types.rs` matches weekday and month names on their first **three**
/// characters and time units on their first **two** (`se mi ho da we mo ye`,
/// plus the bare letters `s m h d w y`), and `parser.rs` multiplies a unit with
/// no `next`/`last` by **zero** — so `milk` is minutes-times-nothing, i.e.
/// today, `mom` is months, `monitor` is Monday and `marketing` is March. Screen
/// only the shapes we know (and let the parser judge the rest) and an ordinary
/// title loses its last word (#107).
///
/// So **every** token must classify ([`classify_token`]), and a lone token must
/// carry a date on its own — which a bare number, month, unit or qualifier does
/// not (`Buy milk 2`, `Prep for May`, `count the days`, `do this`).
///
/// Deliberately stricter than [`parse_due_relative_to`], which the due editor
/// uses: that is an explicit date field, so its text only has to *be* date
/// vocabulary. This is a **guess** about ambiguous prose, so it demands
/// date-specific evidence — a sub-day unit or a time never peels, since a due
/// date is never a time and such a peel could only ever have meant today.
fn looks_like_date_phrase(candidate: &str) -> bool {
    let mut tokens = candidate.split_whitespace();
    let Some(first) = tokens.next() else {
        return false;
    };
    let Some(first_class) = classify_token(first) else {
        return false;
    };
    let mut lone = true;
    for token in tokens {
        if classify_token(token).is_none() {
            return false;
        }
        lone = false;
    }
    !lone || matches!(first_class, Token::SelfSufficient)
}

/// How much one token of a candidate is worth on its own.
enum Token {
    /// Carries a date by itself: a weekday, `today`/`tomorrow`/`yesterday`, an
    /// offset (`3d`), or an ISO or slash date.
    SelfSufficient,
    /// Date vocabulary that needs company: a bare number, month name, unit or
    /// qualifier. `May` alone is an ordinary word; `May 3` is a date.
    NeedsCompany,
}

/// Classify one whitespace token of a peel candidate, or `None` if it is not
/// part of a date at all.
///
/// One trailing comma is stripped first: it is the separator `interim` itself
/// wants between day and year, so `Jul 4, 2017` (and `3d,`) must survive. The
/// strip is uniform rather than per-shape because a per-shape exception would be
/// arbitrary, and it costs nothing — `friday,` and `2026-08-01,` now reach the
/// parser, which rejects them exactly as it did before.
fn classify_token(token: &str) -> Option<Token> {
    let token = token.strip_suffix(',').unwrap_or(token);
    if token.is_empty() {
        return None;
    }
    if token.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Token::NeedsCompany);
    }
    if token.bytes().all(|b| b.is_ascii_alphabetic()) {
        return match date_word(token)? {
            DateWord::Anchor => Some(Token::SelfSufficient),
            DateWord::Month | DateWord::Unit | DateWord::Qualifier => Some(Token::NeedsCompany),
            // A due date is never a time, so peeling one could only ever have
            // resolved to today — the false positive itself, not a date.
            DateWord::Time => None,
        };
    }
    if is_offset(token) || is_iso_date(token) || is_slash_date(token) {
        return Some(Token::SelfSufficient);
    }
    None
}

/// A signed-or-bare offset written as **one** token: digits followed by a
/// date-scale unit, as in `3d`, `+2w`, `3mo`. The spaced spelling (`2 weeks`)
/// never reaches here — it is two tokens, classified as Number then Unit. `3h`
/// and `3m` are excluded with the rest of the sub-day units, so this is where
/// the unit being *date*-scale is enforced — see [`DateWord::Time`].
fn is_offset(token: &str) -> bool {
    let body = token
        .strip_prefix('+')
        .or_else(|| token.strip_prefix('-'))
        .unwrap_or(token);
    let split = body
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(body.len());
    let (digits, unit) = body.split_at(split);
    !digits.is_empty()
        && unit.bytes().all(|b| b.is_ascii_alphabetic())
        && matches!(date_word(unit), Some(DateWord::Unit))
}

/// `YYYY-M-D` by shape only. Whether the fields name a real day is the parser's
/// call, so `2026-13-99` passes here and fails there.
fn is_iso_date(token: &str) -> bool {
    let mut fields = token.split('-');
    match (fields.next(), fields.next(), fields.next(), fields.next()) {
        (Some(year), Some(month), Some(day), None) => {
            year.len() == 4 && all_digits(year) && all_digits(month) && all_digits(day)
        }
        _ => false,
    }
}

/// `D/M` or `D/M/YY(YY)`, again by shape only — `interim` resolves it in the UK
/// dialect (day first).
fn is_slash_date(token: &str) -> bool {
    let mut fields = token.split('/');
    match (fields.next(), fields.next(), fields.next(), fields.next()) {
        (Some(day), Some(month), None, None) => all_digits(day) && all_digits(month),
        (Some(day), Some(month), Some(year), None) => {
            all_digits(day) && all_digits(month) && all_digits(year)
        }
        _ => false,
    }
}

/// A non-empty run of ASCII digits.
fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Whether every maximal run of ASCII letters in `s` is a word the date
/// vocabulary knows — the pre-check that keeps `interim`'s prefix matching from
/// reading an ordinary word as a date (see [`looks_like_date_phrase`] for what
/// it does and why).
///
/// Letter *runs*, not whitespace tokens, so the check is blind to the
/// punctuation it must not care about: `3d` → `d`, `9am` → `am`,
/// `2026-08-01T18:30:00Z` → `T`, `Z`, and `milk,` → `milk`. Digit-only inputs
/// have no runs at all, which is why the day-of-month and ISO paths above are
/// untouched.
fn every_letter_run_is_a_date_word(s: &str) -> bool {
    s.split(|c: char| !c.is_ascii_alphabetic())
        .filter(|run| !run.is_empty())
        .all(|run| date_word(run).is_some())
}

/// A word the date vocabulary knows, and how much a lone one is worth.
enum DateWord {
    /// A weekday, or `today`/`tomorrow`/`yesterday`: a date on its own.
    Anchor,
    /// A month name. `Prep for May` is not a dated Task.
    Month,
    /// A date-scale unit (`day`, `w`, `months`). `count the days` is not either.
    Unit,
    /// `next`/`last`/`this`/`ago`: meaningless without something to qualify.
    Qualifier,
    /// A sub-day unit, a time marker, or `now` — an instant, not a day. Google
    /// stores no time, so these are accepted by [`parse_due_relative_to`] but
    /// never peeled off a title.
    Time,
}

/// Look a word up in the date vocabulary, case-insensitively and **exactly**.
///
/// Exactly, because that is the whole fix: `interim` reaches `monitor` from
/// `mon` and `milk` from `mi`, so anything short of an exact match inherits its
/// false positives. Every spelling lives in exactly one list, so there is no
/// precedence to get wrong — and where two families nearly collide, the listing
/// matches how `interim` itself resolves the word: `mon` is Monday (its
/// `month_name` has no `mon`, so `week_day` wins) and README documents it, `may`
/// is a month (it checks month names first), `mo` is months while `m` is
/// minutes, and `t`/`z` are the ISO separator and zero offset rather than
/// Tuesday.
///
/// The bare prefixes `interim` also accepts — `we`, `da`, `se`, `ho`, `ye`,
/// `mi` — are deliberately absent: these are the spellings a person types, and
/// `we` is a pronoun.
fn date_word(word: &str) -> Option<DateWord> {
    const WEEKDAYS: &[&str] = &[
        "mon",
        "monday",
        "tue",
        "tues",
        "tuesday",
        "wed",
        "weds",
        "wednesday",
        "thu",
        "thur",
        "thurs",
        "thursday",
        "fri",
        "friday",
        "sat",
        "saturday",
        "sun",
        "sunday",
    ];
    const DAY_NAMES: &[&str] = &["today", "tomorrow", "yesterday"];
    const MONTHS: &[&str] = &[
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
        "sept",
        "september",
        "oct",
        "october",
        "nov",
        "november",
        "dec",
        "december",
    ];
    const UNITS: &[&str] = &[
        "d", "day", "days", "w", "week", "weeks", "mo", "month", "months", "y", "year", "years",
    ];
    const QUALIFIERS: &[&str] = &["next", "last", "this", "ago"];
    const TIMES: &[&str] = &[
        "h", "hour", "hours", "m", "min", "mins", "minute", "minutes", "s", "sec", "secs",
        "second", "seconds", "am", "pm", "z", "t", "now",
    ];
    let lower = word.to_ascii_lowercase();
    let word = lower.as_str();
    if WEEKDAYS.contains(&word) || DAY_NAMES.contains(&word) {
        Some(DateWord::Anchor)
    } else if MONTHS.contains(&word) {
        Some(DateWord::Month)
    } else if UNITS.contains(&word) {
        Some(DateWord::Unit)
    } else if QUALIFIERS.contains(&word) {
        Some(DateWord::Qualifier)
    } else if TIMES.contains(&word) {
        Some(DateWord::Time)
    } else {
        None
    }
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
    fn a_bare_number_is_the_next_day_of_that_month() {
        // `now()` is 2026-07-20.
        let cases = [
            // Already past this month, so it rolls forward.
            ("15", ymd(2026, 8, 15)),
            // Still to come this month.
            ("25", ymd(2026, 7, 25)),
            // Today itself — "on or after", not "strictly after".
            ("20", ymd(2026, 7, 20)),
            ("31", ymd(2026, 7, 31)),
            ("1", ymd(2026, 8, 1)),
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
    fn a_bare_number_skips_a_month_that_lacks_that_day() {
        // 2026-02-05: February has no 31st, so the next 31st is in March.
        let february = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2026, 2, 5, 12, 0, 0)
            .unwrap();
        assert_eq!(parse_due_relative_to("31", february), Ok(ymd(2026, 3, 31)));
        // 2026 is not a leap year, so February has no 30th either.
        assert_eq!(parse_due_relative_to("30", february), Ok(ymd(2026, 3, 30)));
        // 28 does exist in February, and is still to come.
        assert_eq!(parse_due_relative_to("28", february), Ok(ymd(2026, 2, 28)));
    }

    /// A signed number must not reach the day-of-month branch. `u32::from_str`
    /// accepts a leading sign, so `"+15".parse()` succeeds — a parse-only guard
    /// would read `+15` as the 15th. The ASCII-digit test is what stops it.
    ///
    /// What `+15` *does* mean is `interim`'s business and unchanged by this
    /// module: it strips the `+` and reads the bare number as a year. That is
    /// long-standing behaviour, asserted here only to pin that the new branch
    /// left it alone — the year reading is what makes it unmistakably *not* a
    /// day-of-month.
    #[test]
    fn a_signed_number_never_reaches_the_day_of_month_branch() {
        assert_eq!(parse_due_relative_to("+15", now()), Ok(ymd(15, 1, 1)));
        assert_eq!(parse_due_relative_to("+3", now()), Ok(ymd(3, 1, 1)));
        // The day-of-month reading these would have had, for contrast.
        assert_ne!(parse_due_relative_to("+15", now()), Ok(ymd(2026, 8, 15)));
    }

    /// Digits outside 1–31 fall through the day-of-month branch untouched.
    /// `interim` then reads them as years, exactly as before this branch existed
    /// — the assertion is "unchanged", not "rejected".
    #[test]
    fn an_out_of_range_number_is_not_a_day_of_month() {
        for (input, expected) in [
            ("0", ymd(0, 1, 1)),
            ("32", ymd(32, 1, 1)),
            ("99", ymd(99, 1, 1)),
        ] {
            assert_eq!(
                parse_due_relative_to(input, now()),
                Ok(expected),
                "input {input:?}"
            );
        }
    }

    /// All digits and far past `u32`. The fallible parse is what keeps this a
    /// parse error rather than a panic — the case that fails loudly if the guard
    /// is ever rewritten as an `unwrap` on "already-validated digits".
    #[test]
    fn an_oversized_all_digit_string_is_an_error_not_a_panic() {
        assert!(parse_due_relative_to("99999999999999999999", now()).is_err());
        assert!(parse_due_relative_to(&"9".repeat(400), now()).is_err());
    }

    #[test]
    fn shift_days_moves_by_whole_days() {
        let base = ymd(2026, 7, 20);
        assert_eq!(shift_days(base, 1), Some(ymd(2026, 7, 21)));
        assert_eq!(shift_days(base, -1), Some(ymd(2026, 7, 19)));
        assert_eq!(shift_days(base, 7), Some(ymd(2026, 7, 27)));
        assert_eq!(shift_days(base, -7), Some(ymd(2026, 7, 13)));
        // Across a month and a year boundary.
        assert_eq!(shift_days(ymd(2026, 7, 31), 1), Some(ymd(2026, 8, 1)));
        assert_eq!(shift_days(ymd(2026, 12, 31), 1), Some(ymd(2027, 1, 1)));
        assert_eq!(shift_days(base, 0), Some(base));
    }

    /// The unconditional boundary proof. A reducer-level test cannot stand in
    /// for this: it depends on chrono accepting a wide-year buffer, and if it
    /// declines the step simply falls back to today and the test passes for the
    /// wrong reason.
    #[test]
    fn shift_days_declines_at_the_ends_of_the_calendar() {
        assert_eq!(shift_days(NaiveDate::MAX, 1), None);
        assert_eq!(shift_days(NaiveDate::MIN, -1), None);
        assert_eq!(shift_days(NaiveDate::MAX, 7), None);
        assert_eq!(shift_days(NaiveDate::MIN, -7), None);
        // The ends themselves are still reachable, so this is a boundary and
        // not an off-by-one.
        assert_eq!(shift_days(NaiveDate::MAX, 0), Some(NaiveDate::MAX));
        assert_eq!(shift_days(NaiveDate::MAX, -1), NaiveDate::MAX.pred_opt());
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
        // Including the RFC-3339 spelling, whose `T` and `Z` are the reason the
        // word screen scans letter *runs* rather than whitespace tokens.
        assert_eq!(
            parse_due_relative_to("2026-08-01T18:30:00Z", now()),
            Ok(ymd(2026, 8, 1))
        );
    }

    /// The due editor's half of #107. `interim` reads all of these as dates from
    /// a two- or three-character prefix, so without the word screen typing `milk`
    /// into the `d` overlay silently stamped today and `monitor` stamped Monday.
    #[test]
    fn an_unknown_word_is_not_a_date() {
        for input in ["milk", "monitor", "marketing", "mom", "west", "milk,"] {
            assert!(
                parse_due_relative_to(input, now()).is_err(),
                "expected error for {input:?}"
            );
        }
    }

    /// The due editor is an explicit date field, so its screen is looser than the
    /// capture gate's: the text only has to *be* date vocabulary. `3h` and `now`
    /// therefore still resolve here while never peeling off a title — asserted on
    /// both sides so the asymmetry cannot drift into an accident.
    #[test]
    fn the_editor_still_accepts_every_vocabulary_spelling() {
        let cases = [
            ("9am", ymd(2026, 7, 20)),
            ("2 days ago", ymd(2026, 7, 18)),
            ("1/8/2026", ymd(2026, 8, 1)),
            ("12/8", ymd(2026, 8, 12)),
            ("sept", ymd(2026, 9, 1)),
            ("tues", ymd(2026, 7, 21)),
            // 2026-07-20 *is* a Monday, and interim's UK dialect reads a plain
            // weekday as the next one — so `mon` is a week out, not today.
            ("mon", ymd(2026, 7, 27)),
            ("3 mo", ymd(2026, 10, 20)),
            ("3h", ymd(2026, 7, 20)),
            ("now", ymd(2026, 7, 20)),
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
            // A qualifier plus a unit, both directions.
            ("Plan next week", "Plan", Some(ymd(2026, 7, 27))),
            ("Review last week", "Review", Some(ymd(2026, 7, 13))),
            // Day-then-month, and a spelled-out month with a year.
            ("Party 1 Jul", "Party", Some(ymd(2026, 7, 1))),
            ("Meet 4 July 2017", "Meet", Some(ymd(2017, 7, 4))),
            ("Bug from 2 days ago", "Bug from", Some(ymd(2026, 7, 18))),
            ("Deploy this friday", "Deploy", Some(ymd(2026, 7, 24))),
            // Four/five-letter abbreviations, which `interim` reaches by prefix
            // and the vocabulary therefore has to list explicitly.
            ("Call tues", "Call", Some(ymd(2026, 7, 21))),
            ("Retro thurs", "Retro", Some(ymd(2026, 7, 23))),
            // Slash dates, day-first (UK dialect), with and without a year —
            // `is_slash_date`'s two arms. The yearless form is the one that keeps
            // `Sprint 1/2` reading as 1 February: ambiguous against a fraction,
            // but `1/2` genuinely is a date in this notation and narrowing it is
            // a separate call from #107.
            ("Pay it 1/8/2026", "Pay it", Some(ymd(2026, 8, 1))),
            ("Sprint 1/2", "Sprint", Some(ymd(2026, 2, 1))),
            // Month-scale offset, and a backwards one: a past due date is a
            // legitimate capture (the pane renders it as `3d ago`).
            ("Sprint 3mo", "Sprint", Some(ymd(2026, 10, 20))),
            ("Task -3d", "Task", Some(ymd(2026, 7, 17))),
            // Connectives are not date words, so the scan falls back past them
            // and the dangling word stays in the title. Long-standing, pinned
            // here because the gate now decides these fallbacks.
            ("Ship it in 3 days", "Ship it in", Some(ymd(2026, 7, 23))),
            ("Renew on friday", "Renew on", Some(ymd(2026, 7, 24))),
            ("Ship on 2026-08-01", "Ship on", Some(ymd(2026, 8, 1))),
            // The Today-capture case from the reducer tests, at this layer.
            ("call bob tomorrow", "call bob", Some(ymd(2026, 7, 21))),
            // The comma `interim` wants between day and year. Each of these
            // loses its date if `classify_token` stops tolerating one.
            ("Meet Jul 4, 2017", "Meet", Some(ymd(2017, 7, 4))),
            ("Party Jul 1, 2020", "Party", Some(ymd(2020, 7, 1))),
            ("Party May 3, 2020", "Party", Some(ymd(2020, 5, 3))),
            ("Ship it 3d,", "Ship it", Some(ymd(2026, 7, 23))),
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
        // them as ordinary words instead of silently dating the Task. `sept` is
        // in the list because interim reaches it from `sep` by prefix, so before
        // the vocabulary was exact it peeled while `june` did not.
        for input in [
            "Prep for May",
            "Buy milk 2",
            "Sprint 17",
            "Plan june",
            "Ship sept",
        ] {
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

    /// The #107 table. `interim` matches weekday and month names on their first
    /// three characters and units on their first two, then multiplies a
    /// direction-less unit by zero — so every trailing word here parses, and all
    /// but one of them used to peel: `milk`/`mom`/`west` as today, `monitor` as
    /// Monday, `marketing` as 1 March. An exact vocabulary is the only thing that
    /// keeps them ordinary words.
    #[test]
    fn a_non_date_trailing_word_stays_in_the_title() {
        for input in [
            "buy milk",
            "call mom",
            "buy new monitor",
            "Decide marketing",
            "head west",
            "launch website",
            "import data",
            "call dad",
            "read more",
            "call mother",
            "plan the move",
            "say yes",
            "just send",
            "do not set",
            // The one that never parsed, kept as the control.
            "Build the widget",
        ] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// A lone unit is as unspecific as a lone month: `interim` reads it as
    /// zero-of-that-unit from now, i.e. today.
    #[test]
    fn a_bare_unit_or_letter_stays_in_the_title() {
        for input in ["count the days", "vitamin d", "swap the w", "plan the week"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// A lone qualifier is meaningless without something to qualify. These do not
    /// peel on the parser's own account either, but resting on that would be
    /// resting on the parser this gate exists to distrust — so the gate rejects
    /// them itself.
    #[test]
    fn a_bare_qualifier_stays_in_the_title() {
        for input in ["Ship it last", "filed ago", "do this"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// A due date is a date, never a time (CONTEXT.md), so peeling a sub-day
    /// offset or a clock time could only ever have resolved to today — the false
    /// positive itself. `now` is in here for the same reason: it is an instant.
    #[test]
    fn a_sub_day_unit_or_time_never_peels() {
        for input in [
            "ship it 3 hours",
            "sync 3h",
            "wait 3m",
            "standup 9am",
            "room 5:30",
            "grade 10:00",
            "task 3.5",
            "Call it now",
            // The fallback trap: the parser rejects `3 days from now`, so a laxer
            // gate walks down to the lone `now` and stamps today.
            "Ship it 3 days from now",
            "Renew in 2 weeks from now",
        ] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// Every token has to be date vocabulary, not just the first. Multi-token
    /// candidates used to be waived entirely ("let the parser be the judge"),
    /// which is how `2 milk` read as two minutes from now.
    #[test]
    fn a_multi_word_candidate_needs_every_word_to_be_a_date() {
        for input in ["buy 2 milk", "Decide marketing campaign"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// A trailing date carrying a *time* stops peeling, deliberately: the time
    /// tokens are not date shapes, and admitting them only when a date token sits
    /// beside them would restore the positional waiver this gate replaced. The
    /// date-only spellings still peel (see `splits_a_trailing_date_off_the_title`)
    /// and `d` still edits the due date.
    #[test]
    fn a_trailing_date_with_a_time_stays_in_the_title() {
        for input in ["Pay rent 2026-08-01 18:30", "Meeting 2026-08-01T18:30:00Z"] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
    }

    /// The bound on the comma tolerance in `classify_token`. None of these peel
    /// today — `interim` has no ordinals and rejects a comma after a word — and
    /// tolerating the day/year comma must not start peeling them.
    #[test]
    fn an_ordinal_or_punctuated_word_stays_in_the_title() {
        for input in [
            "Party May 3rd",
            "Fireworks July 4th",
            "Launch 21st July",
            "Retro friday,",
            "Ship it tomorrow,",
            "Pay rent 2026-08-01,",
            "Call mon,",
            "Deadline (friday)",
        ] {
            assert_eq!(
                split_title_and_due(input, now()),
                (input.to_string(), None),
                "input {input:?}"
            );
        }
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
