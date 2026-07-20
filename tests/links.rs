//! The pure link layer: what counts as a URL in free-text notes, and what
//! oxidone is willing to open.

use oxidone::domain::TaskLink;
use oxidone::links::{
    authority, has_openable_link, has_openable_url, openable_links, openable_urls, scan_urls,
    OpenableUrl,
};

/// A `links[]` entry, terse for the merge tests below.
fn link(url: &str, description: Option<&str>) -> TaskLink {
    TaskLink {
        url: url.into(),
        description: description.map(str::to_string),
        kind: None,
    }
}

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
fn prose_punctuation_in_front_of_a_url_does_not_swallow_it() {
    // The backward walk over scheme characters sweeps up a list marker or a
    // bullet; a scheme must start with a letter, so it steps forward to one
    // rather than discarding the token and reporting "no links".
    let cases = [
        ("1.https://a.dev", "https://a.dev"),
        ("2.https://b.dev/x", "https://b.dev/x"),
        ("-https://a.dev", "https://a.dev"),
        ("+https://a.dev", "https://a.dev"),
        ("see 3.https://c.dev/y please", "https://c.dev/y"),
    ];
    for (notes, expected) in cases {
        assert_eq!(scan_urls(notes), vec![expected], "notes: {notes:?}");
        assert!(has_openable_url(notes), "marker missing for {notes:?}");
    }
}

#[test]
fn a_run_with_no_letter_at_all_is_not_a_scheme() {
    assert!(scan_urls("123://nope").is_empty());
    assert!(scan_urls("...://nope").is_empty());
}

#[test]
fn a_word_abutting_a_url_is_not_absorbed_into_the_scheme() {
    // The backward walk cannot tell prose from scheme, so the run is searched
    // right-to-left for an openable scheme. Without that, `e.g.https://x.dev`
    // reads as one unopenable link and an ordinary URL never opens.
    let cases = [
        ("a.https://x.dev", "https://x.dev"),
        ("e.g.https://x.dev", "https://x.dev"),
        ("cf.http://x.dev/y", "http://x.dev/y"),
        ("see e.g.https://x.dev/a for more", "https://x.dev/a"),
    ];
    for (notes, expected) in cases {
        assert_eq!(scan_urls(notes), vec![expected], "notes: {notes:?}");
        assert!(has_openable_url(notes), "marker missing for {notes:?}");
    }
}

#[test]
fn a_custom_dotted_scheme_is_kept_whole_rather_than_trimmed() {
    // The fallback when nothing in the run is openable: leftmost, so a real
    // scheme stays one refused link instead of being cut down to something the
    // user never wrote.
    for notes in ["myapp.custom://x", "x1https://a.dev", "foo.bar://baz"] {
        assert_eq!(scan_urls(notes), vec![notes], "notes: {notes:?}");
        assert!(openable_urls(notes).is_empty(), "notes: {notes:?}");
    }
}

#[test]
fn urls_glued_by_prose_punctuation_are_two_links_not_one() {
    // Whitespace alone would swallow both into a single token that `parse`
    // accepts — scheme `https`, non-empty rest — handing the browser a
    // malformed address instead of offering the choice.
    for notes in [
        "https://a.dev,https://b.dev",
        "https://a.dev;https://b.dev",
        "https://a.dev),https://b.dev",
    ] {
        assert_eq!(
            scan_urls(notes),
            vec!["https://a.dev", "https://b.dev"],
            "notes: {notes:?}"
        );
    }
}

#[test]
fn a_comma_inside_a_single_url_is_left_alone() {
    // `,` is a legal sub-delimiter (RFC 3986). Splitting on every internal comma
    // would corrupt ordinary map links, which is worse than the case it fixes.
    let notes = "https://maps.example.com/@52.5,13.4,15z";
    assert_eq!(scan_urls(notes), vec![notes]);
}

#[test]
fn a_nested_url_in_a_query_string_stays_with_its_parent() {
    // The `=`/`&` before the inner scheme is URL structure, not prose, so a
    // redirect or share link keeps its payload.
    for notes in [
        "https://a.dev/login?next=https://b.dev/x",
        "https://a.dev/x?a=1&u=https://b.dev",
    ] {
        assert_eq!(scan_urls(notes), vec![notes], "notes: {notes:?}");
    }
}

#[test]
fn a_nested_url_does_not_hide_a_glued_sibling_behind_it() {
    // Every inner `://` is examined, not just the first: the nested one is URL
    // structure and must not split, but the comma-glued one after it still must.
    assert_eq!(
        scan_urls("https://a.dev/x?u=https://b.dev,https://c.dev"),
        vec!["https://a.dev/x?u=https://b.dev", "https://c.dev"],
    );
    assert_eq!(
        scan_urls("https://a.dev/x?u=https://b.dev;https://c.dev,https://d.dev"),
        vec![
            "https://a.dev/x?u=https://b.dev",
            "https://c.dev",
            "https://d.dev"
        ],
    );
    // Two nested parameters and no prose separator anywhere: still one link.
    let both_nested = "https://a.dev/x?u=https://b.dev&v=https://c.dev";
    assert_eq!(scan_urls(both_nested), vec![both_nested]);
}

#[test]
fn bracket_wrapped_urls_are_separate_links() {
    // Markdown and angle-bracket forms are ordinary in notes. Ending only at
    // whitespace ran the first URL through its closer and into the second,
    // yielding one address that is not a URL — which `is_openable` still accepts.
    assert_eq!(
        scan_urls("[a](https://a.dev)[b](https://b.dev)"),
        vec!["https://a.dev", "https://b.dev"],
    );
    assert_eq!(
        scan_urls("<https://a.dev><https://b.dev>"),
        vec!["https://a.dev", "https://b.dev"],
    );
    assert_eq!(scan_urls("[a](https://a.dev)"), vec!["https://a.dev"]);
}

#[test]
fn a_url_keeps_a_closing_bracket_it_owns() {
    // The other half of the same rule: `)` ends the URL when prose opened it,
    // and belongs to the URL when the URL opened it.
    assert_eq!(
        scan_urls("https://en.wikipedia.org/wiki/Foo_(bar)"),
        vec!["https://en.wikipedia.org/wiki/Foo_(bar)"],
    );
    // Wrapped *and* self-parenthesised: nesting is counted, so it ends at the
    // outer closer rather than the inner one.
    assert_eq!(
        scan_urls("(https://en.wikipedia.org/wiki/Foo_(bar))"),
        vec!["https://en.wikipedia.org/wiki/Foo_(bar)"],
    );
    // A stray closer with nothing to match is still prose.
    assert_eq!(scan_urls("see https://a.dev/x)"), vec!["https://a.dev/x"]);
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
        "1.https://a.dev",
        "-https://a.dev",
        "123://nope",
        "x1https://a.dev",
        "a.https://x.dev",
        "e.g.https://x.dev",
        "myapp.custom://x",
        "https://a.dev,https://b.dev",
        "https://maps.example.com/@52.5,13.4",
        "https://a.dev/x?u=https://b.dev",
        "https://a.dev/x?u=https://b.dev,https://c.dev",
        "[a](https://a.dev)[b](https://b.dev)",
        "<https://a.dev><https://b.dev>",
        "https://en.wikipedia.org/wiki/Foo_(bar)",
    ];
    for notes in fixtures {
        assert_eq!(
            has_openable_url(notes),
            !openable_urls(notes).is_empty(),
            "disagreement on {notes:?}"
        );
    }
}

// ---- merging Google's `links[]` with notes URLs (#55) ----

#[test]
fn links_come_before_notes_urls_in_the_merge() {
    let links = [link("https://gmail.example/msg", Some("Re: subject"))];
    let merged = openable_links(&links, "later https://a.dev/1");

    let shown: Vec<String> = merged.openable.iter().map(|l| l.display()).collect();
    assert_eq!(
        shown,
        vec![
            "Re: subject — https://gmail.example/msg".to_string(),
            "https://a.dev/1".to_string(),
        ],
    );
    assert_eq!(merged.found, 2);
}

#[test]
fn a_links_entry_and_the_same_url_in_notes_collapse_to_one_row() {
    // Exact-string dedup across the two sources: a Gmail link the user also
    // pasted into notes is one link, not two — and the `links[]` description
    // wins because it comes first.
    let links = [link("https://a.dev/1", Some("the ticket"))];
    let merged = openable_links(&links, "see https://a.dev/1 again");

    assert_eq!(merged.found, 1, "counted once");
    let shown: Vec<String> = merged.openable.iter().map(|l| l.display()).collect();
    assert_eq!(shown, vec!["the ticket — https://a.dev/1".to_string()]);
}

#[test]
fn a_non_openable_links_entry_counts_as_found_but_never_opens() {
    // Plausible for a `type=email` Gmail link: a `mailto:` is mirrored and
    // counted, but the http/https allowlist refuses it, so nothing opens.
    let links = [link("mailto:a@b.c", Some("email the reporter"))];
    let merged = openable_links(&links, "");

    assert_eq!(merged.found, 1, "the mailto: is a found link");
    assert!(merged.openable.is_empty(), "but not an openable one");
}

#[test]
fn openable_filtering_applies_to_links_the_same_as_notes() {
    let links = [
        link("file:///srv/dump", None),
        link("https://a.dev/1", Some("keep")),
    ];
    let merged = openable_links(&links, "smb://n/v and https://b.dev/2");

    assert_eq!(merged.found, 4, "all four distinct URLs are found");
    let shown: Vec<String> = merged.openable.iter().map(|l| l.display()).collect();
    assert_eq!(
        shown,
        vec![
            "keep — https://a.dev/1".to_string(),
            "https://b.dev/2".to_string(),
        ],
    );
}

#[test]
fn a_blank_description_renders_the_bare_url() {
    // Google may hand back an empty description; a dangling `" — url"` would be
    // worse than none.
    let links = [link("https://a.dev/1", Some("   "))];
    let merged = openable_links(&links, "");
    assert_eq!(
        merged
            .openable
            .iter()
            .map(|l| l.display())
            .collect::<Vec<_>>(),
        vec!["https://a.dev/1".to_string()],
    );
}

#[test]
fn has_openable_link_sees_a_links_only_task_with_no_notes() {
    // The marker's `links[]`-aware predicate: a Gmail-created Task with an
    // openable link but empty notes must still be marked.
    assert!(has_openable_link(&[link("https://a.dev/1", None)], ""));
    // A non-openable link alone is not a marker.
    assert!(!has_openable_link(&[link("mailto:a@b.c", None)], ""));
    // Falls through to the notes scan when `links[]` has nothing openable.
    assert!(has_openable_link(
        &[link("mailto:a@b.c", None)],
        "https://a.dev/1"
    ));
    assert!(!has_openable_link(&[], "no links here"));
}

#[test]
fn has_openable_link_agrees_with_the_merge_on_whether_anything_opens() {
    // The cheap predicate behind the marker and the collecting merge behind `u`
    // must never disagree, or the `⧉` glyph lies about what the key will do.
    let cases: [(&[TaskLink], &str); 5] = [
        (&[], ""),
        (&[], "https://a.dev/1"),
        (&[link("https://a.dev/1", None)], ""),
        (&[link("mailto:a@b.c", None)], ""),
        (&[link("mailto:a@b.c", None)], "file:///x"),
    ];
    for (links, notes) in cases {
        assert_eq!(
            has_openable_link(links, notes),
            !openable_links(links, notes).openable.is_empty(),
            "disagreement on links={links:?} notes={notes:?}",
        );
    }
}

#[test]
fn authority_is_the_slice_between_the_scheme_and_the_path() {
    // The common case the preview leans on: a URL-only notes line collapses to
    // just this, dropping the path that would clip mid-row.
    assert_eq!(authority("https://a.dev/1"), Some("a.dev"));
    assert_eq!(authority("https://a.dev"), Some("a.dev"));
    assert_eq!(authority("https://a.dev?q=1"), Some("a.dev"));
    assert_eq!(authority("https://a.dev#frag"), Some("a.dev"));
}

#[test]
fn authority_keeps_userinfo_and_port_it_is_a_slice_not_a_parse() {
    // Deliberately un-parsed: this crate has no URL parser, and a fuller-but-never
    // -wrong preview beats inventing one.
    assert_eq!(authority("https://a.dev:8080/x"), Some("a.dev:8080"));
    assert_eq!(authority("https://u@a.dev/x"), Some("u@a.dev"));
    assert_eq!(
        authority("https://u:p@a.dev:8080/x"),
        Some("u:p@a.dev:8080")
    );
    assert_eq!(authority("https://[::1]:8080/x"), Some("[::1]:8080"));
}

#[test]
fn an_empty_authority_is_none() {
    // `file:///x` has no authority; a schemeless string has no `://` at all.
    assert_eq!(authority("file:///srv/dump"), None);
    assert_eq!(authority("https://"), None);
    assert_eq!(authority("not a url"), None);
}
