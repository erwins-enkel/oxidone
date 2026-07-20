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

use crate::domain::TaskLink;

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
    /// Accept `raw` only if [`is_openable`] does. The original spelling is what
    /// gets stored, displayed and opened — normalising it would show the user a
    /// URL they did not write.
    pub fn parse(raw: &str) -> Option<Self> {
        is_openable(raw).then(|| Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Whether `raw` is a URL [`OpenableUrl::parse`] would accept: an
/// `http`/`https` scheme with something after the `://`.
///
/// The same test parse applies, without constructing the value — so the row
/// marker can ask its yes/no question for every visible Task on every frame
/// without allocating a `String` it immediately drops. Sharing it with `parse`
/// is what keeps the cheap path and the guarded path from disagreeing.
fn is_openable(raw: &str) -> bool {
    match raw.split_once("://") {
        Some((scheme, rest)) => !rest.is_empty() && is_openable_scheme(scheme),
        None => false,
    }
}

/// Whether `c` can sit between two URLs glued together in prose. The
/// punctuation that ends a URL, plus the brackets that open one — `<a><b>` and
/// `](a)[…](b)` separate two links just as surely as a comma does.
fn is_glue(c: char) -> bool {
    TRAILING.contains(&c) || matches!(c, '(' | '[' | '{' | '<')
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

/// The authority of a `scheme://…` URL — the substring between `://` and the
/// first `/`, `?`, or `#` — or `None` when it is empty (`file:///x`).
///
/// A deliberate *slice*, not a parse: userinfo and port are kept
/// (`https://u@a.dev:8080/x` → `u@a.dev:8080`). This crate has no URL parser (see
/// the module header) and the notes preview does not need one — a bare authority
/// already drops the path that would clip mid-row and restate what `⧉` said.
pub fn authority(url: &str) -> Option<&str> {
    let (_scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    (!authority.is_empty()).then_some(authority)
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
/// allocates nothing — it tests with [`is_openable`] rather than building an
/// [`OpenableUrl`] — where collecting would build a `Vec` plus a `String` per
/// URL just to test emptiness.
pub fn has_openable_url(notes: &str) -> bool {
    url_tokens(notes).any(is_openable)
}

/// A link oxidone will open, merged from a Task's `links[]` and its notes URLs.
///
/// Carries an [`OpenableUrl`], not a raw string: possessing a `Link` is proof
/// the URL passed the http/https allowlist, so the picker can hand it straight to
/// the [`OpenableUrl`]-typed open path with no re-check. The description is shown
/// in the picker; a notes-derived link has none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    url: OpenableUrl,
    description: Option<String>,
}

impl Link {
    pub fn url(&self) -> &OpenableUrl {
        &self.url
    }

    /// The picker row: `description — url` when the link carries one, else the
    /// bare URL. Notes-derived links render bare, so the two sources read alike
    /// on screen except where `links[]` adds the human label Google supplied.
    pub fn display(&self) -> String {
        match &self.description {
            Some(d) => format!("{d} — {}", self.url.as_str()),
            None => self.url.as_str().to_string(),
        }
    }
}

/// The result of merging a Task's `links[]` with its notes URLs: the openable
/// rows the picker shows, plus `found` — the count of *distinct* URLs discovered
/// (openable or not), which the "none openable" report needs so it can tell
/// "nothing here" from "nothing a browser should touch".
pub struct MergedLinks {
    pub openable: Vec<Link>,
    pub found: usize,
}

/// Merge a Task's `links[]` with the URLs scanned from its `notes`.
///
/// `links[]` come first (they carry Google's descriptions), then notes URLs. The
/// union is deduped by **exact URL string, before** the openable filter — the
/// same exact-match rule [`scan_urls`] uses within notes, extended across the two
/// sources — so a Gmail link and the identical URL pasted into notes collapse to
/// one row and count once. `found` is the size of that deduped union; `openable`
/// its http/https subset, in the same order.
pub fn openable_links(task_links: &[TaskLink], notes: &str) -> MergedLinks {
    let from_links = task_links
        .iter()
        .map(|l| (l.url.as_str(), l.description.as_deref()));
    let from_notes = scan_urls(notes).into_iter().map(|url| (url, None));

    let mut seen: Vec<&str> = Vec::new();
    let mut openable = Vec::new();
    for (url, description) in from_links.chain(from_notes) {
        if seen.contains(&url) {
            continue;
        }
        seen.push(url);
        if let Some(url) = OpenableUrl::parse(url) {
            openable.push(Link {
                url,
                // A blank description is no description: it would render as a
                // dangling `" — url"` in the picker.
                description: description
                    .map(str::to_string)
                    .filter(|d| !d.trim().is_empty()),
            });
        }
    }

    MergedLinks {
        found: seen.len(),
        openable,
    }
}

/// Whether a Task has at least one openable URL, across `links[]` and notes.
///
/// The `links[]`-aware companion to [`has_openable_url`]: same allocation-free,
/// short-circuiting contract, because the `⧉` row marker asks it for every
/// visible row on every frame. Tests `links[]` through [`is_openable`] directly —
/// no [`OpenableUrl`] built, no `Vec` collected.
pub fn has_openable_link(task_links: &[TaskLink], notes: &str) -> bool {
    task_links.iter().any(|l| is_openable(&l.url)) || has_openable_url(notes)
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
            // A wrapped URL ends at its own closer, before any glue search:
            // `[a](url)` and `<url>` bound the URL more tightly than whitespace.
            let end = wrapped_end(self.rest, start, after, end).unwrap_or(end);
            let end = self.next_url_start(start, after, end).unwrap_or(end);
            let token = trim_token(&self.rest[start..end]);
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
                .filter(|&next| is_glue(char::from(bytes[next - 1])));
            if split.is_some() {
                return split;
            }
            from = sep + "://".len();
        }
        None
    }
}

/// End of a URL that prose wrapped in brackets — `[text](url)`, `<url>`, or a
/// parenthetical aside — at the closer matching the opener in front of it.
///
/// Nesting is counted, so a URL that legitimately contains the same bracket
/// survives: `(https://en.wikipedia.org/wiki/Foo_(bar))` ends at the *outer*
/// `)`, not the inner one.
///
/// This is what makes `[a](https://a.dev)[b](https://b.dev)` two links. Ending
/// only at whitespace would run the first URL through the closing bracket and
/// on into the second, producing one address that is not a URL at all — and
/// that [`is_openable`] would nonetheless accept.
fn wrapped_end(text: &str, start: usize, after: usize, end: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let opener = *bytes.get(start.checked_sub(1)?)?;
    let closer = match opener {
        b'(' => b')',
        b'[' => b']',
        b'{' => b'}',
        b'<' => b'>',
        _ => return None,
    };
    // ASCII brackets only, so a UTF-8 continuation byte can never match one.
    let mut depth = 1usize;
    for (i, b) in bytes.iter().enumerate().take(end).skip(after) {
        if *b == opener {
            depth += 1;
        } else if *b == closer {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Strip the prose punctuation trailing a URL, keeping a closing bracket the
/// URL actually owns.
///
/// `https://a.dev)` ends in a stray bracket from the surrounding sentence, but
/// `https://en.wikipedia.org/wiki/Foo_(bar)` ends in one of its own — trimming
/// by character alone cannot tell them apart and silently breaks the second.
fn trim_token(token: &str) -> &str {
    let mut end = token.len();
    while let Some(last) = token[..end].chars().last() {
        let strip = match last {
            ')' | ']' | '}' => !bracket_balanced(&token[..end], last),
            other => TRAILING.contains(&other),
        };
        if !strip {
            break;
        }
        end -= last.len_utf8();
    }
    &token[..end]
}

/// Whether `slice` opens `closer`'s bracket at least as often as it closes it —
/// i.e. whether the final closer has an opener to match.
fn bracket_balanced(slice: &str, closer: char) -> bool {
    let opener = match closer {
        ')' => '(',
        ']' => '[',
        _ => '{',
    };
    slice.matches(opener).count() >= slice.matches(closer).count()
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
