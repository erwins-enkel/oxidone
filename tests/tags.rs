//! The **Tag** grammar (`domain::tag_ranges`, `tags`, `tag_name`): pure, so
//! table-tested here with no Model at all.

use oxidone::domain::{tag_name, tag_ranges, tags};

#[test]
fn a_tag_starts_the_title_or_follows_whitespace() {
    assert_eq!(tags("#alice budget"), ["alice"]);
    assert_eq!(tags("budget #alice"), ["alice"]);
    assert_eq!(tags("budget\t#alice"), ["alice"]);
}

#[test]
fn a_hash_mid_word_is_not_a_tag() {
    assert!(tags("learn C# today").is_empty());
    assert!(tags("see issue#12").is_empty());
}

#[test]
fn a_bare_hash_or_doubled_hash_names_nothing() {
    assert!(tags("# heading").is_empty());
    assert!(tags("trailing #").is_empty());
    assert!(tags("##alice").is_empty());
}

#[test]
fn punctuation_ends_a_tag() {
    assert_eq!(tags("ask #alice: budget"), ["alice"]);
    assert_eq!(tags("#alice, #bob."), ["alice", "bob"]);
    assert_eq!(tags("(#alice)"), Vec::<String>::new());
}

#[test]
fn letters_digits_dash_and_underscore_belong_to_the_name() {
    assert_eq!(
        tags("#team-meeting #q3_plan #2026"),
        ["team-meeting", "q3_plan", "2026"]
    );
}

#[test]
fn unicode_letters_are_letters() {
    assert_eq!(tags("#Jürgen #équipe"), ["jürgen", "équipe"]);
}

#[test]
fn names_are_lower_cased_and_deduplicated_in_first_appearance_order() {
    assert_eq!(tags("#Bob #alice #bob #ALICE"), ["bob", "alice"]);
}

#[test]
fn ranges_cover_the_hash_and_the_name_in_bytes() {
    let title = "ask #jürgen: now #x";
    let ranges = tag_ranges(title);
    let spans: Vec<&str> = ranges.iter().map(|r| &title[r.clone()]).collect();
    assert_eq!(spans, ["#jürgen", "#x"]);
}

#[test]
fn tag_name_accepts_only_a_whole_tag_token() {
    assert_eq!(tag_name("#Alice").as_deref(), Some("alice"));
    assert_eq!(tag_name("#team-meeting").as_deref(), Some("team-meeting"));
    assert_eq!(tag_name("#"), None);
    assert_eq!(tag_name("#al:"), None);
    assert_eq!(tag_name("alice"), None);
}
