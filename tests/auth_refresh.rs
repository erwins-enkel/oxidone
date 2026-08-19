//! Contract tests for the refresh exchange oxidone owns (ADR-0009), against a
//! `wiremock` server standing in for Google's token endpoint. The reason to own
//! the exchange is that every outcome becomes reachable from a test — no browser,
//! no live Google account — and the one that matters most is the difference
//! between "the grant is gone" (authorize again) and "the call failed" (do not
//! throw a browser window at it).

use std::fs;
use std::path::Path;

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
fn stored(access: Option<&str>, refresh: Option<&str>, expires_in: i64) -> String {
    let token = TokenInfo {
        access_token: access.map(str::to_owned),
        refresh_token: refresh.map(str::to_owned),
        expires_at: Some(OffsetDateTime::now_utc() + Duration::seconds(expires_in)),
        id_token: None,
    };
    serde_json::to_string(&token).expect("serializing a TokenInfo")
}

/// A store in a fresh temp dir, primed with `blob`.
fn store(dir: &Path, blob: &str) -> FileTokenStore {
    let store = FileTokenStore::new(dir.join("token.json"));
    store.save(blob).expect("priming the token store");
    store
}

fn read_stored(store: &FileTokenStore) -> TokenInfo {
    let blob = store
        .load()
        .expect("reading the token store")
        .expect("a token on disk");
    serde_json::from_str(&blob).expect("a parseable TokenInfo")
}

async fn respond_with(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(response)
        .mount(server)
        .await;
}

#[tokio::test]
async fn a_refresh_persists_the_new_token_and_carries_the_grant_forward() {
    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "expires_in": 3599,
            "token_type": "Bearer"
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), Some("grant-1"), -60));

    let bearer = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect("a refreshed bearer");

    assert_eq!(bearer, "fresh-access-token");
    let saved = read_stored(&store);
    assert_eq!(saved.access_token.as_deref(), Some("fresh-access-token"));
    // Google omits `refresh_token` from a refresh response; dropping it here is
    // what would turn one expiry into a consent prompt.
    assert_eq!(saved.refresh_token.as_deref(), Some("grant-1"));
    assert!(saved.expires_at.unwrap() > OffsetDateTime::now_utc());
    // The grant we hold is the one that was sent.
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(String::from_utf8_lossy(&requests[0].body).contains("refresh_token=grant-1"));
}

#[tokio::test]
async fn invalid_grant_clears_the_dead_token_and_asks_to_authorize_again() {
    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Token has been expired or revoked."
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), Some("grant-1"), -60));

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect_err("a dead grant");

    assert_eq!(error, ApiError::AuthExpired);
    // Cleared, so the consent flow that follows starts from an empty cache
    // instead of re-failing on a token Google has already refused.
    assert!(!store.path().exists(), "the dead token should be gone");
}

#[tokio::test]
async fn a_server_error_is_transient_and_leaves_the_grant_untouched() {
    let server = MockServer::start().await;
    respond_with(&server, ResponseTemplate::new(503)).await;
    let dir = tempfile::tempdir().unwrap();
    let blob = stored(Some("spent"), Some("grant-1"), -60);
    let store = store(dir.path(), &blob);

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect_err("a transient failure");

    assert!(
        matches!(error, ApiError::Network(_)),
        "a 503 must not read as a dead grant, got {error:?}"
    );
    assert_eq!(
        fs::read_to_string(store.path()).unwrap(),
        blob,
        "a transient failure must not touch the stored grant"
    );
}

#[tokio::test]
async fn a_rejected_client_secret_is_not_a_dead_grant() {
    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_client",
            "error_description": "The OAuth client was not found."
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let blob = stored(Some("spent"), Some("grant-1"), -60);
    let store = store(dir.path(), &blob);

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect_err("a refusal");

    // Consent would meet the same refusal, so this must not be `AuthExpired`: a
    // wrong `client_secret.json` is fixed in the config, not in the browser.
    assert_eq!(
        error,
        ApiError::Rejected {
            status: 400,
            message: "invalid_client: The OAuth client was not found.".to_string(),
        }
    );
    assert_eq!(fs::read_to_string(store.path()).unwrap(), blob);
}

#[tokio::test]
async fn an_unexpired_access_token_is_answered_without_calling_google() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(
        dir.path(),
        &stored(Some("still-good"), Some("grant-1"), 600),
    );

    let bearer = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect("the cached bearer");

    assert_eq!(bearer, "still-good");
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "a valid cached token must not cost a round trip"
    );
}

#[tokio::test]
async fn force_refreshes_a_token_the_cache_still_believes_in() {
    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "expires_in": 3599,
            "token_type": "Bearer"
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(
        dir.path(),
        &stored(Some("still-good"), Some("grant-1"), 600),
    );

    // The 401 replay in `RestClient::send` depends on this: the server has
    // rejected a token the cache is still happy with.
    let bearer = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, true)
        .await
        .expect("a forced refresh");

    assert_eq!(bearer, "fresh-access-token");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_cache_with_no_refresh_token_asks_to_authorize_again() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), None, -60));

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect_err("nothing left to exchange");

    assert_eq!(error, ApiError::AuthExpired);
    assert!(!store.path().exists(), "an unusable token should be gone");
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "there is no grant to send"
    );
}

#[tokio::test]
async fn an_absurd_expiry_is_dropped_rather_than_panicking() {
    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "expires_in": i64::MAX,
            "token_type": "Bearer"
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), Some("grant-1"), -60));

    // `now + expires_in` is date arithmetic on a number off the wire, and the
    // overflowing one used to be a panic.
    let bearer = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false)
        .await
        .expect("a refreshed bearer");

    assert_eq!(bearer, "fresh-access-token");
    assert_eq!(read_stored(&store).expires_at, None);
}

/// A grant that cannot be *read* is not a missing grant. Answering it with consent
/// is what makes a root-owned `token.json` prompt on every single launch.
#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_token_file_does_not_ask_for_consent() {
    use std::os::unix::fs::PermissionsExt;

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), Some("grant-1"), -60));
    fs::set_permissions(store.path(), fs::Permissions::from_mode(0o000)).unwrap();

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false).await;

    fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        matches!(error, Err(ApiError::TokenStoreFailed(_))),
        "an unreadable store must not read as an expired grant, got {error:?}"
    );
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "there is no grant to send"
    );
    assert!(
        store.path().exists(),
        "a file we could not read must not be deleted"
    );
}

/// A refresh that Google grants but the disk refuses must not report itself as a
/// network error: retrying cannot help, and the class would let a caller shrug it
/// off — leaving a session that works today and re-authorizes tomorrow.
#[cfg(unix)]
#[tokio::test]
async fn a_token_that_cannot_be_written_is_not_a_network_error() {
    use std::os::unix::fs::PermissionsExt;

    let server = MockServer::start().await;
    respond_with(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access-token",
            "expires_in": 3599,
            "token_type": "Bearer"
        })),
    )
    .await;
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path(), &stored(Some("spent"), Some("grant-1"), -60));
    // Readable, so the stored grant is still found; not writable, so persisting
    // the refreshed token fails. On the file rather than the directory: an
    // existing file is truncated in place, which needs no directory write.
    fs::set_permissions(store.path(), fs::Permissions::from_mode(0o400)).unwrap();

    let error = cached_or_refreshed(&reqwest::Client::new(), &secret(&server), &store, false).await;

    fs::set_permissions(store.path(), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(
        matches!(error, Err(ApiError::TokenStoreFailed(_))),
        "an unwritable store must say so, got {error:?}"
    );
}
