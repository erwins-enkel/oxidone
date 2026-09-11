//! Every `ApiError` an `oxidone json` call can meet, and what a caller is told
//! about it (ADR-0010).
//!
//! This is the table the plugin maps to bar states, so it is asserted variant by
//! variant rather than sampled. The mapping itself is an exhaustive `match` with
//! no wildcard arm — the only one on `ApiError` in the codebase — so a new
//! variant breaks the build there; this fixes what the existing ones mean.

use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;

use oxidone::api::{ApiError, FakeTasksApi, TasksApi};
use oxidone::cli::{run_remote, CliError, ErrorKind, Job};

/// The reference instant these calls resolve against. `run_remote` takes an
/// instant rather than a date: Today's completion-recency rule compares a UTC
/// `completed_at` against the caller's own day.
fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 7, 20, 9, 0, 0)
        .single()
        .expect("valid instant")
}

/// Inject `error` at the API seam and report what the CLI makes of it.
async fn surfaced(error: ApiError) -> CliError {
    let api = FakeTasksApi::new();
    api.insert_list("Work").await.expect("seeding a List");
    api.fail_next(error);
    run_remote(&api, Job::Lists, now())
        .await
        .expect_err("the injected failure")
}

#[tokio::test]
async fn every_api_failure_has_a_kind_and_an_exit_code() {
    let cases = [
        (
            ApiError::Network("connection reset".into()),
            ErrorKind::Network,
            4,
        ),
        (ApiError::AuthExpired, ErrorKind::AuthExpired, 3),
        (
            ApiError::TokenStoreFailed("permission denied".into()),
            ErrorKind::TokenStoreFailed,
            3,
        ),
        (ApiError::NotFound, ErrorKind::NotFound, 6),
        (
            ApiError::Rejected {
                status: 400,
                message: "bad request".into(),
            },
            ErrorKind::Rejected,
            5,
        ),
        (
            ApiError::Pagination("cursor went nowhere".into()),
            ErrorKind::Pagination,
            4,
        ),
        (ApiError::RateLimited, ErrorKind::RateLimited, 4),
        (
            ApiError::QuotaExhausted {
                message: "daily limit".into(),
            },
            ErrorKind::QuotaExhausted,
            5,
        ),
    ];

    for (api_error, kind, code) in cases {
        let error = surfaced(api_error.clone()).await;
        assert_eq!(error.kind, kind, "{api_error:?}");
        assert_eq!(error.kind.exit_code(), code, "{api_error:?}");
        assert!(!error.message.is_empty(), "{api_error:?}");
    }
}

#[tokio::test]
async fn a_dead_grant_names_the_remedy_a_headless_caller_actually_has() {
    // There is no browser here to send anybody to: `oxidone json` never runs the
    // consent flow, because it is what a bar plugin runs unattended every few
    // minutes. So the message says the one thing that fixes it.
    let error = surfaced(ApiError::AuthExpired).await;
    assert_eq!(error.kind, ErrorKind::AuthExpired);
    assert!(
        error.message.contains("run `oxidone` once"),
        "unhelpful message: {}",
        error.message
    );
}

#[tokio::test]
async fn a_store_failure_is_not_an_expired_grant() {
    // The distinction ADR-0009 exists for, carried out to the CLI: a broken token
    // file is not a missing one. They share an exit code — both mean "not
    // authorized" — and a caller reading `kind` can still tell them apart, which
    // is the difference between "fix the file" and "authorize again".
    let store = surfaced(ApiError::TokenStoreFailed("permission denied".into())).await;
    let expired = surfaced(ApiError::AuthExpired).await;
    assert_ne!(store.kind, expired.kind);
    assert_eq!(store.kind.exit_code(), expired.kind.exit_code());
    assert!(store.message.contains("permission denied"));
}

#[tokio::test]
async fn a_failure_prints_the_documented_envelope_and_nothing_else() {
    let error = surfaced(ApiError::RateLimited).await;
    let parsed: Value = serde_json::from_str(&error.to_json()).expect("valid JSON on stderr");
    assert_eq!(parsed["error"]["kind"], "rate_limited");
    assert!(parsed["error"]["message"].is_string());

    let mut keys: Vec<&str> = parsed["error"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["kind", "message"]);
    assert_eq!(
        parsed.as_object().unwrap().len(),
        1,
        "only `error` at the top"
    );
}
