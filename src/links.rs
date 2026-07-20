//! Pure URL extraction from a Task's notes, and the one definition of what
//! oxidone is willing to hand to a browser.
//!
//! Notes are free text, so the URLs in them are *derived*, never stored — the
//! cache stays a pure mirror of what Google holds (ADR-0003) and there is no
//! local copy to drift out of sync with the notes themselves.
//!
//! The scanner deliberately tokenises **any** `scheme://`, not just the two
//! schemes that can be opened. Narrowing it here would make a notes blob holding
//! only `file:///etc/passwd` report "no links" — a false statement about the
//! user's own data. Counting them and refusing them are different outcomes, and
//! the reducer says so differently.
//!
//! No I/O and no terminal: every entry point is a pure function over `&str`.

/// A URL that oxidone is willing to open.
///
/// The field is private and [`OpenableUrl::parse`] is the only constructor, so
/// possessing one *is* the proof that its scheme was checked — no caller can
/// route a `file:` or `javascript:` URL to the browser by assembling the value
/// itself. That matters because the strings come from remote, user-editable
/// data and the platform opener will act on far more than HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenableUrl(String);

impl OpenableUrl {
    /// Accept `raw` only if it is an `http`/`https` URL with something after the
    /// `://`. The scheme compares case-insensitively (`HTTPS://` is valid per
    /// RFC 3986), but the original spelling is what gets stored, displayed and
    /// opened — normalising it would show the user a URL they did not write.
    pub fn parse(raw: &str) -> Option<Self> {
        let (scheme, rest) = raw.split_once("://")?;
        if rest.is_empty() {
            return None;
        }
        let openable = scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
        openable.then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Trailing characters stripped from a token. Prose wraps URLs in punctuation
/// far more often than URLs end in it.
const TRAILING: &[char] = &['.', ',', ';', ':', '!', '?', ')', ']', '}', '>', '"', '\''];

/// Every `scheme://` token in `notes`, in order, deduplicated by exact string
/// with the first occurrence kept.
///
/// Exact matching is the whole dedup rule: it collapses the case that actually
/// happens (the same URL pasted twice) without inventing a normalisation this
/// crate cannot justify — `https://x.dev` vs `https://x.dev/`, port 443,
/// userinfo and IPv6 literals are questions for a URL parser, and there isn't
/// one in this dependency tree.
pub fn scan_urls(notes: &str) -> Vec<&str> {
    let mut found: Vec<&str> = Vec::new();
    for token in url_tokens(notes) {
        if !found.contains(&token) {
            found.push(token);
        }
    }
    found
}

/// The openable subset of [`scan_urls`], in the same order.
pub fn openable_urls(notes: &str) -> Vec<OpenableUrl> {
    scan_urls(notes)
        .into_iter()
        .filter_map(OpenableUrl::parse)
        .collect()
}

/// Whether `notes` holds at least one openable URL.
///
/// Separate from [`openable_urls`] because the row marker asks this question for
/// every visible Task on every frame: this short-circuits on the first hit and
/// allocates nothing, where collecting would build a `Vec` plus a `String` per
/// URL just to test emptiness.
pub fn has_openable_url(notes: &str) -> bool {
    url_tokens(notes).any(|token| OpenableUrl::parse(token).is_some())
}

/// Iterate the `scheme://` tokens of `notes` lazily, so callers that only need
/// "is there one?" can stop at the first.
fn url_tokens(notes: &str) -> UrlTokens<'_> {
    UrlTokens { rest: notes }
}

struct UrlTokens<'a> {
    rest: &'a str,
}

impl<'a> Iterator for UrlTokens<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        loop {
            let sep = self.rest.find("://")?;
            let after = sep + "://".len();
            let Some(start) = scheme_start(self.rest, sep) else {
                // No valid scheme in front of this `://`; skip past it so the
                // scan makes progress instead of matching it again.
                self.rest = &self.rest[after..];
                continue;
            };
            // A URL runs to the first whitespace; prose punctuation that trails
            // it is not part of it.
            let end = self.rest[after..]
                .find(char::is_whitespace)
                .map_or(self.rest.len(), |i| after + i);
            let token = self.rest[start..end].trim_end_matches(TRAILING);
            self.rest = &self.rest[end..];
            // Reject a bare `https://` with no authority: it is a scheme, not a
            // link, and counting it would inflate "N links found".
            if token.len() > after - start {
                return Some(token);
            }
        }
    }
}

/// Byte index where the scheme preceding `sep` starts, if there is a valid one.
///
/// Walks back over RFC 3986 scheme characters — `ALPHA *( ALPHA / DIGIT / "+" /
/// "-" / "." )` — then forward to the first letter, because a scheme must begin
/// with one and the backward walk cannot know where the URL stops and the prose
/// in front of it starts. `1.https://a.dev` (a numbered list) and
/// `-https://a.dev` (a bullet) both sweep punctuation into the run; rejecting
/// the whole token there would lose the URL entirely rather than trim it.
///
/// Byte-wise backtracking is safe because every character it accepts is ASCII,
/// so a UTF-8 continuation byte can never be mistaken for one.
fn scheme_start(text: &str, sep: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut start = sep;
    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.') {
            start -= 1;
        } else {
            break;
        }
    }
    // Everything from here to `sep` is already a legal scheme body, so the first
    // letter in the run is the first byte that can legally open a scheme.
    while start < sep && !bytes[start].is_ascii_alphabetic() {
        start += 1;
    }
    (start < sep).then_some(start)
}
