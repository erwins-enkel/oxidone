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
        is_openable_scheme(scheme).then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The only schemes oxidone will hand to a browser. Shared by
/// [`OpenableUrl::parse`] and the scanner's scheme-boundary search so the two
/// can never disagree about what "openable" means.
fn is_openable_scheme(scheme: &str) -> bool {
    scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")
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
            let end = self.next_url_start(start, after, end).unwrap_or(end);
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

impl UrlTokens<'_> {
    /// Where a *second* URL begins inside `start..end`, if one does.
    ///
    /// Notes glue URLs together with prose punctuation — `https://a.dev,https://b.dev`
    /// — and whitespace alone would swallow both into one token that `parse`
    /// happily accepts, handing the browser a malformed address instead of
    /// offering a choice of two.
    ///
    /// The split only happens across the punctuation that already ends a URL
    /// ([`TRAILING`]). A comma is a legal sub-delimiter, so
    /// `https://maps.example.com/@52.5,13.4` must stay whole, and so must a
    /// nested `https://a.dev/x?u=https://b.dev`, where the `=` before the inner
    /// scheme is URL structure rather than prose.
    ///
    /// *Every* inner `://` is examined, not just the first: one URL can carry a
    /// nested one and still be glued to a sibling
    /// (`https://a.dev/x?u=https://b.dev,https://c.dev`), so stopping at the
    /// first non-prose separator would miss the split that does exist.
    fn next_url_start(&self, start: usize, after: usize, end: usize) -> Option<usize> {
        let bytes = self.rest.as_bytes();
        let mut from = after;
        // `find` only matches a `://` lying wholly inside `from..end`, so
        // `sep + 3 <= end` and the cursor always advances.
        while let Some(hit) = self.rest[from..end].find("://") {
            let sep = from + hit;
            let split = scheme_start(self.rest, sep)
                .filter(|&next| next > start)
                .filter(|&next| TRAILING.contains(&char::from(bytes[next - 1])));
            if split.is_some() {
                return split;
            }
            from = sep + "://".len();
        }
        None
    }
}

/// Byte index where the scheme preceding `sep` starts, if there is a valid one.
///
/// Walks back over RFC 3986 scheme characters — `ALPHA *( ALPHA / DIGIT / "+" /
/// "-" / "." )` — which cannot know where prose stops and the URL begins: all of
/// `1.`, `-`, `a.` and `e.g.` are swept into the run by a text that simply
/// abuts a URL.
///
/// So the run is searched **right to left** for a position that could legally
/// open a scheme (the run's own start, or any letter following `.`/`-`/`+`), and
/// the first one whose scheme is openable wins. Prose butted against a URL is
/// far more common than a custom dotted scheme, so `e.g.https://x.dev` should
/// yield `https://x.dev` rather than the unopenable `e.g.https://x.dev`.
///
/// With nothing openable it falls back to the **leftmost** candidate, which
/// keeps a genuine custom scheme whole: `myapp.custom://x` stays one refused
/// link rather than being trimmed down to something it never was.
///
/// Byte-wise backtracking is safe because every character it accepts is ASCII,
/// so a UTF-8 continuation byte can never be mistaken for one.
fn scheme_start(text: &str, sep: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut run = sep;
    while run > 0 {
        let b = bytes[run - 1];
        if b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.') {
            run -= 1;
        } else {
            break;
        }
    }
    // Right to left, so the first openable hit is the rightmost one. No
    // allocation: `has_openable_url` runs per visible row, per frame.
    let mut leftmost = None;
    let mut i = sep;
    while i > run {
        i -= 1;
        let opens_a_scheme = i == run || matches!(bytes[i - 1], b'.' | b'-' | b'+');
        if opens_a_scheme && bytes[i].is_ascii_alphabetic() {
            if is_openable_scheme(&text[i..sep]) {
                return Some(i);
            }
            leftmost = Some(i);
        }
    }
    leftmost
}
