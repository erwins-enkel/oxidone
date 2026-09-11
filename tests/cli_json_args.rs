//! Argument parsing for `oxidone json` (ADR-0010).
//!
//! Pure: no network, no token, no process. `parse_args` is what decides whether
//! an invocation is well-formed at all, and every way of getting it wrong has to
//! fail closed — a silent default here would be a plugin reading the wrong List
//! and never being told.

use oxidone::cli::{parse_args, ErrorKind, Invocation, Remote};
use oxidone::domain::ListId;

fn parse(args: &[&str]) -> Result<Invocation, oxidone::cli::CliError> {
    parse_args(args.iter().map(|a| a.to_string()))
}

fn usage(args: &[&str]) -> String {
    let error = parse(args).expect_err("a usage failure");
    assert_eq!(error.kind, ErrorKind::Usage, "{args:?}");
    assert_eq!(error.kind.exit_code(), 2);
    error.message
}

#[test]
fn each_read_subcommand_parses_to_its_own_invocation() {
    assert_eq!(
        parse(&["today"]).unwrap(),
        Invocation::Remote(Remote::Today)
    );
    assert_eq!(
        parse(&["lists"]).unwrap(),
        Invocation::Remote(Remote::Lists)
    );
    assert_eq!(
        parse(&["apply"]).unwrap(),
        Invocation::Remote(Remote::Apply)
    );
    assert_eq!(
        parse(&["tasks", "--list", "L1"]).unwrap(),
        Invocation::Remote(Remote::Tasks {
            list: ListId("L1".into())
        })
    );
}

#[test]
fn help_is_its_own_invocation_and_needs_no_credentials() {
    // Both spellings, matching the binary's own `--help`/`-h`.
    assert_eq!(parse(&["--help"]).unwrap(), Invocation::Help);
    assert_eq!(parse(&["-h"]).unwrap(), Invocation::Help);
}

#[test]
fn a_due_phrase_may_span_several_arguments() {
    // `oxidone json due next monday` is how a phrase is typed; requiring quotes
    // would be a papercut with no safety behind it.
    assert_eq!(
        parse(&["due", "next", "monday"]).unwrap(),
        Invocation::Due {
            expr: "next monday".into()
        }
    );
    assert_eq!(
        parse(&["due", "+3d"]).unwrap(),
        Invocation::Due { expr: "+3d".into() }
    );
}

#[test]
fn no_subcommand_fails_closed() {
    assert!(usage(&[]).contains("no subcommand"));
}

#[test]
fn an_unrecognized_subcommand_fails_closed() {
    assert!(usage(&["taks"]).contains("unrecognized subcommand 'taks'"));
}

#[test]
fn tasks_without_a_list_fails_closed() {
    // Every one of these is a way to end up reading *some* List rather than the
    // one meant, if the parser were willing to guess.
    for args in [
        &["tasks"][..],
        &["tasks", "L1"],
        &["tasks", "--list"],
        &["tasks", "--list", ""],
        &["tasks", "--lists", "L1"],
        &["tasks", "--list", "L1", "L2"],
    ] {
        assert!(usage(args).contains("--list"), "{args:?}");
    }
}

#[test]
fn due_without_an_expression_fails_closed() {
    assert!(usage(&["due"]).contains("date expression"));
    assert!(usage(&["due", "   "]).contains("date expression"));
}

#[test]
fn a_trailing_argument_is_never_ignored() {
    // Dropping it silently is how `oxidone json today --list L1` would answer a
    // question nobody asked.
    for args in [
        &["today", "--list"][..],
        &["lists", "extra"],
        &["apply", "-"],
        &["--help", "today"],
    ] {
        assert!(usage(args).contains("unexpected argument"), "{args:?}");
    }
}
