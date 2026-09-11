//! The cross-process lock on the refresh exchange (ADR-0010).
//!
//! Two `oxidone` processes can now want the same grant — the TUI and an
//! `oxidone json` call — and `SingleFlight` only coalesces within one process.
//! Without the lock they both POST `grant_type=refresh_token`, and since Google
//! may rotate the refresh token, the loser is left having sent one that has just
//! been replaced.
//!
//! These tests drive it from a single process on purpose. An advisory file lock
//! is held by the *open file description*, not by the process, so two
//! `FileTokenStore` values over one path contend exactly as two processes do —
//! and `wiremock` can then count what Google actually received.

use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use time::{Duration, OffsetDateTime};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};
use yup_oauth2::storage::TokenInfo;
use yup_oauth2::ApplicationSecret;

use oxidone::api::ApiError;
use oxidone::auth::{cached_or_refreshed, FileTokenStore, TokenStore};

/// The BYO client credentials, pointed at the mock instead of Google.
fn secret(server: &MockServer) -> ApplicationSecret {
    ApplicationSecret {
        client_id: "client-id".to_string(),
        client_secret: "client-secret".to_string(),
        token_uri: format!("{}/token", server.uri()),
        auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
        redirect_uris: vec!["http://localhost".to_string()],
        ..Default::default()
    }
}

/// A stored token cache in the yup-oauth2 blob format, `expires_in` seconds from
/// now (negative for one that is already spent).
fn stored(access: &str, refresh: &str, expires_in: i64) -> String {
    let token = TokenInfo {
        access_token: Some(access.to_string()),
        refresh_token: Some(refresh.to_string()),
        expires_at: Some(OffsetDateTime::now_utc() + Duration::seconds(expires_in)),
        id_token: None,
    };
    serde_json::to_string(&token).expect("serializing a TokenInfo")
}

/// A store over `<dir>/token.json`. Called twice per test with the same `dir`,
/// which is the whole point: two independent handles on one token file.
fn store_at(dir: &Path) -> FileTokenStore {
    FileTokenStore::new(dir.join("token.json"))
}

#[test]
fn a_second_holder_is_told_to_wait_rather_than_handed_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let first = store_at(dir.path());
    let second = store_at(dir.path());

    let guard = first
        .try_lock()
        .expect("taking the lock")
        .expect("an uncontended lock");
    assert!(
        second.try_lock().expect("trying the lock").is_none(),
        "a held lock must not be handed out twice"
    );

    // Releasing is dropping it — there is nothing to call.
    drop(guard);
    assert!(
        second.try_lock().expect("trying the lock").is_some(),
        "the lock must be free once its guard is gone"
    );
}

#[test]
fn the_lock_file_is_a_sibling_and_never_the_token_itself() {
    // It has to be a separate file: `save` truncates `token.json` and `clear`
    // *unlinks* it, and a lock on an unlinked inode excludes nobody — the next
    // process opens the replacement and locks that instead.
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    let guard = store.try_lock().unwrap().expect("an uncontended lock");

    assert!(dir.path().join("token.lock").exists());
    assert!(
        !store.path().exists(),
        "locking must not bring the token file into existence"
    );

    // The lock survives the token file being written and then cleared.
    store.save(&stored("a", "grant-1", 3600)).unwrap();
    store.clear().unwrap();
    assert!(
        store_at(dir.path()).try_lock().unwrap().is_none(),
        "clearing the token must not release the lock"
    );
    drop(guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_callers_sharing_one_token_file_produce_exactly_one_refresh() {
    let server = MockServer::start().await;
    // `expires_in` is deliberately long: whoever posts second would be visible as
    // a second request, not as a second *expiry*.
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({
                    "access_token": "fresh-access-token",
                    "expires_in": 3599,
                    "token_type": "Bearer"
                }))
                // Long enough that the two calls genuinely overlap: without the
                // lock, the second reads the spent token while the first is still
                // in flight and posts too.
                .set_delay(std::time::Duration::from_millis(300)),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let blob = stored("spent", "grant-1", -60);
    store_at(dir.path()).save(&blob).expect("priming the store");

    let secret = Arc::new(secret(&server));
    let http = Arc::new(reqwest::Client::new());
    let run = |dir: std::path::PathBuf| {
        let (secret, http) = (Arc::clone(&secret), Arc::clone(&http));
        async move { cached_or_refreshed(&http, &secret, &store_at(&dir), false).await }
    };

    let (first, second) =
        tokio::join!(run(dir.path().to_path_buf()), run(dir.path().to_path_buf()));

    assert_eq!(first.expect("a bearer"), "fresh-access-token");
    // The queued caller re-read the store under the lock and answered from what
    // the first one persisted — it did not post a refresh token that had just
    // been through an exchange.
    assert_eq!(second.expect("a bearer"), "fresh-access-token");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "the refresh exchange must happen once, not once per caller"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_caller_inherits_a_dead_grant_instead_of_posting_again() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(400)
                .set_body_json(json!({
                    "error": "invalid_grant",
                    "error_description": "Token has been expired or revoked."
                }))
                .set_delay(std::time::Duration::from_millis(300)),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let blob = stored("spent", "grant-1", -60);
    store_at(dir.path()).save(&blob).expect("priming the store");

    let secret = Arc::new(secret(&server));
    let http = Arc::new(reqwest::Client::new());
    let run = |dir: std::path::PathBuf| {
        let (secret, http) = (Arc::clone(&secret), Arc::clone(&http));
        async move { cached_or_refreshed(&http, &secret, &store_at(&dir), false).await }
    };

    let (first, second) =
        tokio::join!(run(dir.path().to_path_buf()), run(dir.path().to_path_buf()));

    // The first call classifies the grant as dead and clears the store. The
    // second finds nothing left to exchange and says so — one refusal, not two.
    assert_eq!(first.expect_err("a dead grant"), ApiError::AuthExpired);
    assert_eq!(second.expect_err("a dead grant"), ApiError::AuthExpired);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "a grant Google has already refused must not be posted a second time"
    );
}

#[tokio::test]
async fn a_valid_cached_token_is_answered_without_taking_the_lock() {
    // The common case, and the reason the fast path sits above the lock: a
    // process holding the lock must not be able to stall every other process's
    // *cached* reads.
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store_at(dir.path());
    store
        .save(&stored("still-good", "grant-1", 3600))
        .expect("priming the store");

    let held = store.try_lock().unwrap().expect("an uncontended lock");
    let bearer = cached_or_refreshed(
        &reqwest::Client::new(),
        &secret(&server),
        &store_at(dir.path()),
        false,
    )
    .await
    .expect("the cached bearer");

    assert_eq!(bearer, "still-good");
    assert!(server.received_requests().await.unwrap().is_empty());
    drop(held);
}
