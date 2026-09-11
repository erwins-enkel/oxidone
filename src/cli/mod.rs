//! `oxidone json` — the machine-readable entry point onto the same core the TUI
//! drives (ADR-0010).
//!
//! It lives in the library, not in `main.rs`, for one reason: `main.rs` is a
//! binary crate that `tests/*.rs` cannot link against, so anything put there is
//! untested by construction. The binary's whole share of this is reading stdin,
//! writing stdout and choosing an exit code — [`main`] — while everything with a
//! rule in it is a `pub` item here with an injected clock and an injected
//! [`TasksApi`].
//!
//! Two constraints shape the surface:
//!
//! - **Network-only.** No subcommand opens `oxidone.db`. The single-writer design
//!   (ADR-0001) survives a second process only because that process never writes
//!   the cache; a caller that wants an offline answer keeps its own last-known-good
//!   copy.
//! - **No new domain logic.** Today membership is [`crate::domain::due_on_or_before`]
//!   narrowed by [`crate::domain::within_completion_day`], Migrate is
//!   [`crate::domain::migrated_due`], the **Entry type** is `EntryType::parse`, and
//!   a date phrase is [`crate::dateparse`]. A second definition here would be a way
//!   for the bar and the TUI to disagree — and was, until #135.

mod apply;
mod read;
mod wire;

pub use apply::ApplyCommand;
pub use wire::{Entry, EntryKind, EntryStatus, ErrorBody, ErrorEnvelope, ListRow};

use std::process::ExitCode;
use std::sync::Arc;

use chrono::{DateTime, Local, TimeZone};
use serde_json::Value;

use crate::api::{ApiError, RestClient, TasksApi};
use crate::auth::{FileTokenStore, RefreshOnlyProvider, TokenProvider, TokenStore};
use crate::config::Config;
use crate::domain::ListId;

/// Usage text for `oxidone json --help` and every fail-closed argument path.
pub const USAGE: &str = "\
oxidone json — machine-readable Google Tasks, for scripts and plugins

Usage:
  oxidone json today             entries due on or before today, across every List
                                 (a Completed one only if it was completed today)
  oxidone json lists             the Lists, and which one is the default
  oxidone json tasks --list ID   one List's entries, in Manual order
  oxidone json due EXPR          resolve a due-date phrase (tomorrow, mon, +3d, ISO)
  oxidone json apply             apply one JSON command read from stdin

Reads print JSON to stdout. Failures print JSON to stderr and exit non-zero:
  2 bad request   3 not authorized   4 network   5 refused by Google
  6 not found     7 refused by oxidone

Writes take their payload on stdin, never in flags: argv is readable by every
process running as you, so a --title flag would publish your task titles.";

/// What the caller asked for. Split by what it *needs*: `Help` and `Due` are
/// answerable with nothing but the clock, so they must not be made to depend on
/// credentials that a preview has no business requiring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// Print [`USAGE`] and exit 0.
    Help,
    /// Resolve a due-date phrase. Pure; no network, no token, no config.
    Due { expr: String },
    /// Needs a live Google client.
    Remote(Remote),
}

/// The invocations that talk to Google, as argv spells them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Remote {
    Today,
    Lists,
    Tasks {
        list: ListId,
    },
    /// The command itself arrives on stdin, so this is not yet runnable — see
    /// [`resolve`].
    Apply,
}

/// A remote invocation with everything it needs, `apply`'s command included.
///
/// Separate from [`Remote`] so the two sources of a bad request — argv and stdin
/// — are both spent *before* anything authorizes. A caller that got the command
/// wrong is told so on a machine with no credentials at all, which is the only
/// answer that is any use while writing the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    Today,
    Lists,
    Tasks { list: ListId },
    Apply(ApplyCommand),
}

/// Complete a [`Remote`] into a runnable [`Job`], reading `apply`'s command from
/// `stdin`. Pure; `stdin` is ignored by everything but `apply`.
pub fn resolve(command: Remote, stdin: &str) -> Result<Job, CliError> {
    Ok(match command {
        Remote::Today => Job::Today,
        Remote::Lists => Job::Lists,
        Remote::Tasks { list } => Job::Tasks { list },
        Remote::Apply => Job::Apply(apply::parse(stdin)?),
    })
}

/// A failure, in the two forms a caller consumes it: a stable `kind` string and
/// an exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub kind: ErrorKind,
    pub message: String,
}

/// The failure classes. Finer-grained than the exit codes on purpose — three of
/// these share exit 3 — so a bar can key its state off the code and still say
/// which of them happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// Something local went wrong that is not the caller's doing.
    Internal,
    /// The invocation was not well-formed: bad arguments, or a stdin command
    /// that is not one.
    Usage,
    /// A due-date phrase that is not a date.
    InvalidDue,
    /// No BYO credentials, or nowhere to keep them.
    NotConfigured,
    /// There is no usable grant. `oxidone json` never runs the consent flow —
    /// authorizing needs a user, and this is what a plugin runs unattended — so
    /// the remedy is to start the TUI once.
    AuthExpired,
    TokenStoreFailed,
    Network,
    RateLimited,
    Pagination,
    Rejected,
    QuotaExhausted,
    NotFound,
    /// oxidone itself declined: Migrate on a Completed entry. Distinct from
    /// [`ErrorKind::Rejected`], which is Google declining.
    Refused,
}

impl ErrorKind {
    /// The wire spelling. Part of the contract.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Usage => "usage",
            Self::InvalidDue => "invalid_due",
            Self::NotConfigured => "not_configured",
            Self::AuthExpired => "auth_expired",
            Self::TokenStoreFailed => "token_store_failed",
            Self::Network => "network",
            Self::RateLimited => "rate_limited",
            Self::Pagination => "pagination",
            Self::Rejected => "rejected",
            Self::QuotaExhausted => "quota_exhausted",
            Self::NotFound => "not_found",
            Self::Refused => "refused",
        }
    }

    /// The process exit code. Coarser than the kind because it is what a shell
    /// caller branches on: "fix your call", "authorize", "the network", "Google
    /// said no", "gone", "oxidone said no".
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::Usage | Self::InvalidDue => 2,
            Self::NotConfigured | Self::AuthExpired | Self::TokenStoreFailed => 3,
            Self::Network | Self::RateLimited | Self::Pagination => 4,
            Self::Rejected | Self::QuotaExhausted => 5,
            Self::NotFound => 6,
            Self::Refused => 7,
        }
    }
}

impl CliError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, message)
    }

    /// The JSON a failure prints on stderr.
    pub fn to_json(&self) -> String {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                kind: self.kind.as_str(),
                message: self.message.clone(),
            },
        };
        // A struct of two strings cannot fail to serialize; if it somehow did,
        // saying so is still better than printing nothing at all.
        serde_json::to_string(&envelope)
            .unwrap_or_else(|e| format!(r#"{{"error":{{"kind":"internal","message":"{e}"}}}}"#))
    }
}

/// Classify an [`ApiError`] for the wire.
///
/// **Exhaustive, with no wildcard arm** — deliberately, and the only match on
/// `ApiError` in the codebase that is. Elsewhere retry decisions key off status
/// codes, so a variant nobody mapped just falls into whatever the catch-all says;
/// here a new variant breaks the build, which is the point of a contract that
/// promises what each failure means.
impl From<ApiError> for CliError {
    fn from(error: ApiError) -> Self {
        let message = error.to_string();
        let kind = match error {
            ApiError::Network(_) => ErrorKind::Network,
            ApiError::AuthExpired => ErrorKind::AuthExpired,
            ApiError::TokenStoreFailed(_) => ErrorKind::TokenStoreFailed,
            ApiError::NotFound => ErrorKind::NotFound,
            ApiError::Rejected { .. } => ErrorKind::Rejected,
            ApiError::Pagination(_) => ErrorKind::Pagination,
            ApiError::RateLimited => ErrorKind::RateLimited,
            ApiError::QuotaExhausted { .. } => ErrorKind::QuotaExhausted,
        };
        // `AuthExpired` is the one whose own words are useless to a plugin
        // author: there is no browser here to send them to, so name the remedy.
        if kind == ErrorKind::AuthExpired {
            return Self::new(
                kind,
                "no usable Google authorization; run `oxidone` once to authorize",
            );
        }
        Self::new(kind, message)
    }
}

/// Parse the arguments that follow `oxidone json`.
///
/// Pure, and fails closed: an unrecognized subcommand, a missing `--list`, or a
/// trailing argument nobody asked for is [`ErrorKind::Usage`], never a silent
/// default. The one deliberate looseness is `due`, which joins the rest of the
/// arguments — `oxidone json due next monday` is how a phrase is typed, and
/// requiring quotes there would be a papercut with no safety behind it.
pub fn parse_args<I>(args: I) -> Result<Invocation, CliError>
where
    I: IntoIterator<Item = String>,
{
    let args: Vec<String> = args.into_iter().collect();
    let Some(verb) = args.first() else {
        return Err(CliError::usage("oxidone json: no subcommand given"));
    };

    match verb.as_str() {
        "--help" | "-h" => expect_no_more(&args[1..], Invocation::Help),
        "today" => expect_no_more(&args[1..], Invocation::Remote(Remote::Today)),
        "lists" => expect_no_more(&args[1..], Invocation::Remote(Remote::Lists)),
        "apply" => expect_no_more(&args[1..], Invocation::Remote(Remote::Apply)),
        "tasks" => match &args[1..] {
            [flag, id] if flag == "--list" && !id.is_empty() => {
                Ok(Invocation::Remote(Remote::Tasks {
                    list: ListId(id.clone()),
                }))
            }
            _ => Err(CliError::usage(
                "oxidone json tasks: expected --list <list id>",
            )),
        },
        "due" => {
            let expr = args[1..].join(" ");
            if expr.trim().is_empty() {
                return Err(CliError::usage(
                    "oxidone json due: expected a date expression",
                ));
            }
            Ok(Invocation::Due { expr })
        }
        other => Err(CliError::usage(format!(
            "oxidone json: unrecognized subcommand '{other}'"
        ))),
    }
}

/// Accept `invocation` only if nothing follows the subcommand.
fn expect_no_more(rest: &[String], invocation: Invocation) -> Result<Invocation, CliError> {
    match rest.first() {
        None => Ok(invocation),
        Some(extra) => Err(CliError::usage(format!(
            "oxidone json: unexpected argument '{extra}'"
        ))),
    }
}

/// Resolve a due-date phrase, echoing the input beside the answer.
///
/// A read subcommand rather than a field `apply` interprets, so a caller can show
/// the user the date *before* committing to it — and so `apply`'s own `due` stays
/// one unambiguous type (ISO) that cannot fail halfway through a write.
///
/// Generic over the timezone because relative phrases are resolved in the
/// caller's zone; `main` passes `Local::now()` and tests pass a fixed instant.
pub fn run_due<Tz: TimeZone>(expr: &str, now: DateTime<Tz>) -> Result<Value, CliError> {
    let due = crate::dateparse::parse_due_relative_to(expr, now)
        .map_err(|e| CliError::new(ErrorKind::InvalidDue, e.to_string()))?;
    Ok(serde_json::json!({ "input": expr, "due": due }))
}

/// Run a job against Google.
///
/// `now` is injected rather than read from the clock, so Today membership and
/// Migrate's arithmetic are deterministic without touching the machine clock.
///
/// A zone-aware instant, not a bare date, for one reason: Today's
/// completion-recency rule compares a UTC `completed_at` against the *user's*
/// day, and a `NaiveDate` carries no zone to do that in. Generic over the zone
/// like [`run_due`] — `main` passes `Local::now()`, tests a fixed instant.
pub async fn run_remote<Tz: TimeZone>(
    api: &dyn TasksApi,
    job: Job,
    now: DateTime<Tz>,
) -> Result<Value, CliError> {
    let today = now.date_naive();
    match job {
        Job::Today => read::today(api, &now).await,
        Job::Lists => read::lists(api).await,
        Job::Tasks { list } => read::tasks(api, &list).await,
        Job::Apply(command) => apply::run(api, command, today).await,
    }
}

/// Build the live client for a caller with no terminal and no user.
///
/// Uses [`RefreshOnlyProvider`], so an absent or dead grant is reported rather
/// than answered with a browser window — see that type for why that matters at
/// five-minute poll intervals. There is no `SingleFlight` here and none is
/// needed: the cross-process lock inside the refresh exchange already keeps
/// concurrent refreshes to one, in this process and across them.
pub async fn google_client() -> Result<RestClient, CliError> {
    let config = Config::load();
    let Some(secret) = config.client_secret_path.as_ref() else {
        return Err(CliError::new(
            ErrorKind::NotConfigured,
            "no client_secret_path configured; see `oxidone --print-config-path`",
        ));
    };
    let Some(store) = FileTokenStore::in_config_dir() else {
        return Err(CliError::new(
            ErrorKind::NotConfigured,
            "no config directory (is a home dir set?)",
        ));
    };
    let store: Arc<dyn TokenStore> = Arc::new(store);
    let provider = RefreshOnlyProvider::new(secret, store).await.map_err(|e| {
        // A `client_secret.json` that cannot be read is a configuration fault,
        // not a dead grant: consent would meet the same missing file.
        CliError::new(ErrorKind::NotConfigured, format!("{e:#}"))
    })?;
    let provider: Arc<dyn TokenProvider> = Arc::new(provider);
    Ok(RestClient::new(provider))
}

/// The binary's share: arguments in, bytes out, exit code back.
///
/// Everything with a decision in it has already happened by the time this runs —
/// it exists so `main.rs` holds no logic that `tests/` cannot reach.
pub async fn main(args: Vec<String>) -> ExitCode {
    let invocation = match parse_args(args) {
        Ok(invocation) => invocation,
        Err(e) => return fail(&e),
    };

    let outcome = match invocation {
        Invocation::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Invocation::Due { expr } => run_due(&expr, Local::now()),
        Invocation::Remote(command) => {
            // stdin is read only for `apply`; a read subcommand must not block on
            // a terminal that will never send anything.
            let stdin = if command == Remote::Apply {
                match std::io::read_to_string(std::io::stdin()) {
                    Ok(text) => text,
                    Err(e) => {
                        return fail(&CliError::new(
                            ErrorKind::Usage,
                            format!("reading the command from stdin: {e}"),
                        ))
                    }
                }
            } else {
                String::new()
            };
            // Resolved before authorizing, so a malformed command is reported on
            // a machine that has never been authorized at all.
            match resolve(command, &stdin) {
                Ok(job) => match google_client().await {
                    Ok(api) => run_remote(&api, job, Local::now()).await,
                    Err(e) => Err(e),
                },
                Err(e) => Err(e),
            }
        }
    };

    match outcome.and_then(emit) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

/// Write the payload to stdout.
///
/// Not `println!`, which *panics* if the write fails — and it can: `oxidone json
/// today | head -1` closes the pipe under us, and a panic there would exit 101
/// with a Rust backtrace where the contract promises an exit code. A broken pipe
/// is somebody's shell, not a reason to abort.
fn emit(value: Value) -> Result<(), CliError> {
    use std::io::Write;

    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|()| stdout.flush())
        .map_err(|e| CliError::new(ErrorKind::Internal, format!("writing to stdout: {e}")))
}

/// Print a failure the way the contract says: JSON on stderr, stdout untouched.
fn fail(error: &CliError) -> ExitCode {
    // `eprintln!` deliberately: if stderr is gone too there is nowhere left to
    // report anything, and the exit code still carries the answer.
    eprintln!("{}", error.to_json());
    ExitCode::from(error.kind.exit_code())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every kind is on the wire under its own name and lands on exactly one
    /// exit code. A duplicate spelling would make two failures indistinguishable
    /// to a caller that only reads `kind`.
    #[test]
    fn every_error_kind_has_a_distinct_name_and_a_code_in_range() {
        const ALL: [ErrorKind; 13] = [
            ErrorKind::Internal,
            ErrorKind::Usage,
            ErrorKind::InvalidDue,
            ErrorKind::NotConfigured,
            ErrorKind::AuthExpired,
            ErrorKind::TokenStoreFailed,
            ErrorKind::Network,
            ErrorKind::RateLimited,
            ErrorKind::Pagination,
            ErrorKind::Rejected,
            ErrorKind::QuotaExhausted,
            ErrorKind::NotFound,
            ErrorKind::Refused,
        ];
        let mut names: Vec<&str> = ALL.iter().map(|k| k.as_str()).collect();
        names.sort_unstable();
        let unique = names.len();
        names.dedup();
        assert_eq!(names.len(), unique, "two kinds share a wire name");

        for kind in ALL {
            let code = kind.exit_code();
            // 0 would read as success; the table stops at 7.
            assert!(
                (1..=7).contains(&code),
                "{} has exit code {code}",
                kind.as_str()
            );
        }
    }

    #[test]
    fn the_error_envelope_is_the_documented_shape() {
        let error = CliError::new(ErrorKind::NotFound, "no such List");
        assert_eq!(
            error.to_json(),
            r#"{"error":{"kind":"not_found","message":"no such List"}}"#
        );
    }
}
