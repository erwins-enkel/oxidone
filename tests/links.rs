//! The pure link layer: what counts as a URL in free-text notes, and what
//! oxidone is willing to open.

use oxidone::links::{has_openable_url, openable_urls, scan_urls, OpenableUrl};

#[test]
fn a_plain_https_url_is_openable() {
    let url = OpenableUrl::parse("https://example.com/a").expect("https is openable");
    assert_eq!(url.as_str(), "https://example.com/a");
}

#[test]
fn the_scheme_compares_case_insensitively_but_the_spelling_is_kept() {
    let url = OpenableUrl::parse("HTTPS://Example.COM/A").expect("scheme case is irrelevant");
    // Preserved verbatim: normalising would open — and show — a URL the user
    // never wrote.
    assert_eq!(url.as_str(), "HTTPS://Example.COM/A");
}

#[test]
fn only_http_and_https_can_be_opened() {
    for raw in [
        "file:///etc/passwd",
        "smb://server/share",
        "javascript:alert(1)",
        "mailto:a@b.c",
        "ftp://example.com",
        "example.com",
        "",
        // A scheme with no authority is not a link.
        "https://",
    ] {
        assert!(
            OpenableUrl::parse(raw).is_none(),
            "expected {raw:?} to be refused"
        );
    }
}

#[test]
fn trailing_prose_punctuation_is_not_part_of_the_url() {
    let cases = [
        ("see https://example.com/a.", "https://example.com/a"),
        ("see (https://example.com/a)", "https://example.com/a"),
        ("see https://example.com/a,", "https://example.com/a"),
        ("\"https://example.com/a\"", "https://example.com/a"),
        ("https://example.com/a?q=1;", "https://example.com/a?q=1"),
    ];
    for (notes, expected) in cases {
        assert_eq!(scan_urls(notes), vec![expected], "notes: {notes:?}");
    }
}

#[test]
fn several_urls_on_one_line_are_all_found_in_order() {
    let notes = "ticket https://a.dev/1 and pr https://b.dev/2 thanks";
    assert_eq!(scan_urls(notes), vec!["https://a.dev/1", "https://b.dev/2"]);
}

#[test]
fn urls_across_lines_are_found() {
    let notes = "https://a.dev/1\nsome prose\n\thttps://b.dev/2\n";
    assert_eq!(scan_urls(notes), vec!["https://a.dev/1", "https://b.dev/2"]);
}

#[test]
fn a_schemeless_host_is_not_a_url() {
    assert!(scan_urls("see www.example.com for details").is_empty());
}

#[test]
fn schemes_without_a_double_slash_are_never_tokenised() {
    // Neither reaches the scanner, yet `parse` still refuses them — #55 will
    // feed Google's `links[]` strings straight in without the scanner.
    let notes = "javascript:alert(1) and mailto:a@b.c";
    assert!(scan_urls(notes).is_empty());
    assert!(OpenableUrl::parse("javascript:alert(1)").is_none());
    assert!(OpenableUrl::parse("mailto:a@b.c").is_none());
}

#[test]
fn non_openable_schemes_are_still_counted_as_links() {
    // The distinction the reducer needs: found, but refused. Reporting "no
    // links" here would be a false statement about the user's own notes.
    let notes = "backup at file:///srv/dump and share smb://nas/vol";
    assert_eq!(scan_urls(notes).len(), 2);
    assert!(openable_urls(notes).is_empty());
}

#[test]
fn an_exact_duplicate_collapses_to_the_first_occurrence() {
    let notes = "https://a.dev/1 then again https://a.dev/1";
    assert_eq!(scan_urls(notes), vec!["https://a.dev/1"]);
}

#[test]
fn urls_differing_only_in_a_trailing_slash_are_kept_apart() {
    // Deliberate: collapsing these needs a URL parser, not a string rule.
    let notes = "https://a.dev and https://a.dev/";
    assert_eq!(scan_urls(notes), vec!["https://a.dev", "https://a.dev/"]);
}

#[test]
fn empty_notes_yield_nothing() {
    assert!(scan_urls("").is_empty());
    assert!(openable_urls("").is_empty());
    assert!(!has_openable_url(""));
}

#[test]
fn a_bare_scheme_separator_is_skipped_without_stalling_the_scan() {
    // The `://` with no scheme in front must not swallow the real URL after it.
    let notes = ":// then https://a.dev/1";
    assert_eq!(scan_urls(notes), vec!["https://a.dev/1"]);
}

#[test]
fn multibyte_prose_around_a_url_does_not_split_a_codepoint() {
    // Byte-indexed scanning over UTF-8 notes: a panic here would be a crash on
    // ordinary notes, not an edge case.
    let notes = "Größe — siehe https://a.dev/größe … und café https://b.dev/ü";
    assert_eq!(
        scan_urls(notes),
        vec!["https://a.dev/größe", "https://b.dev/ü"]
    );
}

#[test]
fn openable_urls_keeps_order_and_drops_the_rest() {
    let notes = "file:///x then https://a.dev/1 then smb://n/v then https://b.dev/2";
    let urls: Vec<String> = openable_urls(notes)
        .iter()
        .map(|u| u.as_str().to_string())
        .collect();
    assert_eq!(urls, vec!["https://a.dev/1", "https://b.dev/2"]);
}

#[test]
fn has_openable_url_agrees_with_openable_urls_on_every_fixture() {
    // The marker uses the cheap predicate and `u` uses the collecting one; if
    // they ever disagree the glyph starts lying about what the key will do.
    let fixtures = [
        "",
        "no links here",
        "www.example.com",
        "https://a.dev/1",
        "see (https://a.dev/1).",
        "file:///etc/passwd",
        "file:///etc/passwd and https://a.dev/1",
        "javascript:alert(1)",
        "https://",
        ":// stray",
        "https://a.dev/1 https://a.dev/1",
        "Größe https://a.dev/größe",
    ];
    for notes in fixtures {
        assert_eq!(
            has_openable_url(notes),
            !openable_urls(notes).is_empty(),
            "disagreement on {notes:?}"
        );
    }
}
