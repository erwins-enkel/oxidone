//! The `grant_type=refresh_token` exchange against Google's token endpoint,
//! owned by oxidone (ADR-0009).
//!
//! This exists because `yup_oauth2::Authenticator::find_token_info` answers
//! *every* failed refresh by running the whole installed flow: a revoked grant
//! and a connection reset on resume-from-sleep are the same event to it, so a
//! network blip costs a browser window and the log says nothing about which
//! happened. The decision is made inside the library, between two of its own
//! calls, where no caller can reach it.
//!
//! Here it is a pure function over Google's status and body ([`classify`]), and
//! exactly three outcomes are allowed to mean "open a browser": nothing usable is
//! stored (a first run, or a blob that is not a token), the stored blob has no
//! refresh token to send, and a refusal Google itself labelled `invalid_grant`.
//! Every other outcome keeps its own [`ApiError`] class and leaves the stored
//! grant where it is — including a [`TokenStore`] that *fails*, which is a broken
//! file rather than a missing grant and would otherwise prompt on every launch.

use reqwest::StatusCode;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use yup_oauth2::storage::TokenInfo;
use yup_oauth2::ApplicationSecret;

use super::{TokenGuard, TokenStore};
use crate::api::ApiError;

/// Cap on how much of Google's body is quoted into an [`ApiError`]. The token
/// endpoint's own errors are a line of JSON, but a proxy in the way can answer
/// with a whole HTML page, and this text ends up in a single-row status line.
const MAX_QUOTED_BODY: usize = 200;

/// How long to wait for the other process's refresh before giving up. A refresh
/// is one POST, so this is generous by an order of magnitude — it is sized to
/// outlast a slow exchange, not to outlast a hung one, and a wait that ends is
/// what keeps a wedged peer from wedging us too.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Gap between attempts at the store lock. `try_lock` cannot be awaited, so the
/// wait is a poll — but an *async* one, which is the point: a blocking acquire
/// would park a runtime worker for the whole of somebody else's network round
/// trip.
const LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(50);

/// Hand back a usable bearer for the stored grant, refreshing it against
/// Google's token endpoint when the cached access token is spent — or whenever
/// `force`, which is the 401 replay in [`crate::api::rest`]: a token the cache
/// still believes in can already have been rejected by the server.
///
/// `Err(ApiError::AuthExpired)` is the *only* outcome that means "run the
/// interactive consent flow". Every other failure keeps its own class, so a
/// dropped connection, a rejected `client_secret.json`, or a token file that
/// cannot be read or written can never be mistaken for a dead grant and answered
/// with a browser window.
///
/// The exchange itself runs under the store's cross-process lock (ADR-0010), so
/// a `oxidone json` call and the TUI cannot both POST and race to rewrite the
/// stored grant. Serializing the two POSTs would not be enough on its own —
/// Google may rotate the refresh token, and the loser would then be sending one
/// that has just been replaced — so the store is **re-read under the lock** and
/// the decision to refresh remade against what is actually on disk now. A caller
/// that queued behind a successful refresh returns *that* token and never posts
/// at all.
///
/// The common case never takes the lock: a cached, unexpired access token is
/// answered before the slow path begins.
pub async fn cached_or_refreshed(
    http: &reqwest::Client,
    secret: &ApplicationSecret,
    store: &dyn TokenStore,
    force: bool,
) -> Result<String, ApiError> {
    // `?` first: a store that *failed* is not a missing grant, and answering it
    // with consent would prompt on every launch for as long as the file is broken.
    let Some(stored) = load(store)? else {
        // Nothing usable cached: only consent can produce a grant from here.
        return Err(ApiError::AuthExpired);
    };

    if let Some(access) = usable(&stored, force) {
        return Ok(access);
    }

    // Whichever access token we just rejected. Under the lock, a *different* one
    // is proof that somebody else refreshed while we queued — which retires our
    // `force`, since `force` means "the server rejected the token we had", and
    // this is no longer that token.
    let rejected = stored.access_token;

    let _guard = lock(store).await?;

    let Some(stored) = load(store)? else {
        // Reachable: the process ahead of us can have found the grant dead and
        // cleared the store (see `refused`). Consent is the remedy, once.
        return Err(ApiError::AuthExpired);
    };
    let force = force && stored.access_token == rejected;
    if let Some(access) = usable(&stored, force) {
        return Ok(access);
    }

    let Some(refresh_token) = stored.refresh_token.as_deref() else {
        // An access token with nothing behind it. Google issues this when the
        // consent that produced it was not offline-capable; there is nothing to
        // exchange, so the blob goes and consent starts over.
        tracing::warn!("the cached token has no refresh token; re-authorization needed");
        clear(store);
        return Err(ApiError::AuthExpired);
    };

    let response = http
        .post(&secret.token_uri)
        .form(&[
            ("client_id", secret.client_id.as_str()),
            ("client_secret", secret.client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    if !status.is_success() {
        return Err(refused(classify(status, &body), store));
    }

    let refreshed: TokenResponse = serde_json::from_str(&body).map_err(|e| {
        // A 200 that does not carry a token is a protocol violation, not a
        // transient fault: backing off and retrying would be the wrong advice.
        ApiError::Rejected {
            status: status.as_u16(),
            message: format!("malformed token response: {e}"),
        }
    })?;

    // Google omits `refresh_token` from a refresh response in the normal case,
    // and the grant it belongs to is still ours — so the stored one is carried
    // forward rather than dropped.
    let token = token_info(refreshed, Some(refresh_token));
    let bearer = token
        .access_token
        .clone()
        .expect("token_info always sets the access token");
    persist(store, &token).map_err(ApiError::TokenStoreFailed)?;
    Ok(bearer)
}

/// Exchange a fresh authorization code for a grant, store it, and hand back its
/// access token.
///
/// The other half of what ADR-0009 moved in-house, and here for the same
/// reasons: it is the same endpoint, the same response shape, and the same
/// [`classify`] rules as the refresh above, so there is one place that knows
/// what Google's token endpoint says and one place that writes what it returns.
///
/// `redirect_uri` must be byte-identical to the one in the authorization URL —
/// Google compares them — and `verifier` is the PKCE secret that never left this
/// process.
///
/// The *write* is taken under the store's cross-process lock, so a consent
/// finishing while a peer refreshes cannot interleave two writes of the grant.
/// Only the write: unlike a refresh, this exchange is not a read-modify-write of
/// what is stored — the grant it brings back replaces whatever was there — so
/// there is nothing to re-read under the lock and no reason to hold it across
/// the round trip.
///
/// A grant with no refresh token in it is refused rather than stored: it would
/// work until its access token expired and then send the user back to the
/// browser, which is exactly the "it asks me to authorize every day" failure
/// this module exists to make impossible.
pub(super) async fn exchange_code(
    http: &reqwest::Client,
    secret: &ApplicationSecret,
    store: &dyn TokenStore,
    code: &str,
    verifier: &str,
    redirect_uri: &str,
) -> Result<String, ApiError> {
    let response = http
        .post(&secret.token_uri)
        .form(&[
            ("client_id", secret.client_id.as_str()),
            ("client_secret", secret.client_secret.as_str()),
            ("code", code),
            ("code_verifier", verifier),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| ApiError::Network(e.to_string()))?;

    if !status.is_success() {
        return Err(code_refused(classify(status, &body), status.as_u16()));
    }

    let granted: TokenResponse = serde_json::from_str(&body).map_err(|e| ApiError::Rejected {
        status: status.as_u16(),
        message: format!("malformed token response: {e}"),
    })?;
    if granted.refresh_token.is_none() {
        return Err(ApiError::Rejected {
            status: status.as_u16(),
            message: "google granted no refresh token; oxidone would have to ask again tomorrow"
                .to_string(),
        });
    }

    let token = token_info(granted, None);
    let bearer = token
        .access_token
        .clone()
        .expect("token_info always sets the access token");

    let _guard = lock(store).await?;
    persist(store, &token).map_err(ApiError::TokenStoreFailed)?;
    Ok(bearer)
}

/// What a refused *code* exchange means. Deliberately not [`refused`]: nothing
/// here says anything about a stored grant — there is none yet — so none is
/// cleared, and `invalid_grant` means the code itself was spent or stale rather
/// than that an authorization died.
fn code_refused(refusal: Refusal, status: u16) -> ApiError {
    match refusal {
        Refusal::GrantDead { description, .. } => {
            tracing::error!(
                error = "invalid_grant",
                description = description.as_deref().unwrap_or("<none>"),
                "google refused the authorization code"
            );
            ApiError::Rejected {
                status,
                message: "the authorization code was refused; it may already have been used — \
                          authorize again"
                    .to_string(),
            }
        }
        Refusal::Refused { status, message } => {
            tracing::error!(status, message = %message, "google refused the authorization code");
            ApiError::Rejected { status, message }
        }
        Refusal::Transient(message) => ApiError::Network(message),
    }
}

/// Build the `TokenInfo` to store from what Google answered.
///
/// `carried_refresh` is the refresh token already held, used only when the
/// response omits one — which a refresh normally does and a code exchange never
/// may.
fn token_info(granted: TokenResponse, carried_refresh: Option<&str>) -> TokenInfo {
    TokenInfo {
        access_token: Some(granted.access_token),
        refresh_token: granted
            .refresh_token
            .or_else(|| carried_refresh.map(str::to_owned)),
        // `checked_add`, because `expires_in` comes off the wire and adding an
        // absurd one to `now` panics. `None` there means "no expiry known", the
        // same as an answer that omitted it: the token is used until a 401 forces
        // a refresh, which is a worse deal than a real expiry and better than a
        // crash.
        expires_at: granted
            .expires_in
            .and_then(|seconds| OffsetDateTime::now_utc().checked_add(Duration::seconds(seconds))),
        id_token: granted.id_token,
    }
}

/// The access token to answer with, or `None` when one has to be fetched.
///
/// `force` is the 401 replay: the server has already rejected whatever is
/// cached, so nothing cached can satisfy the caller however fresh it looks.
///
/// Otherwise `TokenInfo::is_expired` decides, carrying yup-oauth2's one-minute
/// margin — borrowed rather than reinvented, so there is a single definition of
/// "spent" in the codebase.
///
/// Split out because it is asked twice: once before taking the store lock and
/// again under it, against whatever another process left behind. Two spellings
/// of this rule could disagree about a token by one minute, which is a bug that
/// would surface as an occasional extra POST and nothing else.
fn usable(stored: &TokenInfo, force: bool) -> Option<String> {
    if force || stored.is_expired() {
        return None;
    }
    stored.access_token.clone()
}

/// Take the store's cross-process lock, waiting for a peer's refresh to finish.
///
/// Polled rather than blocked on: [`TokenStore::try_lock`] is synchronous, and
/// blocking a runtime worker for the length of another process's network round
/// trip would stall every other task on that thread — in the TUI, the frame.
///
/// Bounded by [`LOCK_TIMEOUT`], so a peer that hangs mid-exchange costs this
/// process 30 seconds rather than the rest of its life. The timeout reports as
/// [`ApiError::TokenStoreFailed`]: the store is what was unavailable, the grant
/// is untouched, and — like every other store failure — it must never be
/// answered with a browser window.
async fn lock(store: &dyn TokenStore) -> Result<Box<dyn TokenGuard>, ApiError> {
    let deadline = tokio::time::Instant::now() + LOCK_TIMEOUT;
    loop {
        match store.try_lock() {
            Ok(Some(guard)) => return Ok(guard),
            Ok(None) => {}
            Err(e) => {
                let detail = format!("{e:#}");
                tracing::error!(error = %detail, "the token lock could not be taken");
                return Err(ApiError::TokenStoreFailed(detail));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let detail =
                format!("another oxidone process has held the token lock for {LOCK_TIMEOUT:?}");
            tracing::error!(error = %detail, "gave up waiting for the token lock");
            return Err(ApiError::TokenStoreFailed(detail));
        }
        tokio::time::sleep(LOCK_POLL).await;
    }
}

/// Read and parse the stored token cache.
///
/// `Ok(None)` means nothing usable is stored: no file yet, or contents that are
/// not a token. Consent is the remedy for both, and it overwrites the file either
/// way. `Err` means the *store* failed — a file we are not allowed to read, an
/// I/O error, bytes that are not UTF-8. That is a broken file, not a missing
/// grant: it gets its own class ([`ApiError::TokenStoreFailed`]) so it can never
/// be answered with a browser window, which is what a root-owned `token.json`
/// would otherwise earn on every single launch.
pub(super) fn load(store: &dyn TokenStore) -> Result<Option<TokenInfo>, ApiError> {
    let stored = store.load().map_err(|e| {
        let detail = format!("{e:#}");
        tracing::error!(
            error = %detail,
            "the stored token could not be read; oxidone cannot authorize until this is fixed"
        );
        ApiError::TokenStoreFailed(detail)
    })?;
    let Some(blob) = stored else {
        return Ok(None);
    };
    match serde_json::from_str(&blob) {
        Ok(token) => Ok(Some(token)),
        Err(e) => {
            tracing::warn!(error = %e, "the cached token is corrupt; will re-authenticate");
            Ok(None)
        }
    }
}

/// Serialize `token` into the store, loudly. `Err` carries why it could not be
/// written — the caller decides what to wrap it in, so the reason is stated once
/// however many layers it passes through.
///
/// Every caller turns this into [`ApiError::TokenStoreFailed`] and never anything
/// else: the acquisition succeeded, so calling it a network error would invite a
/// retry that cannot help, and calling it an expired grant would answer a full
/// disk with another consent flow. Logged at `error!` because the alternative — a
/// session that works today and asks for consent again tomorrow — is invisible
/// from the outside, which is precisely how a grant appears to die daily.
pub(super) fn persist(store: &dyn TokenStore, token: &TokenInfo) -> Result<(), String> {
    let json = serde_json::to_string(token).map_err(|e| format!("serializing the token: {e}"))?;
    store.save(&json).map_err(|e| {
        let detail = format!("{e:#}");
        tracing::error!(
            error = %detail,
            "the token could not be saved; the next start will have to re-authorize"
        );
        detail
    })
}

/// Drop the stored grant. Best effort: it is already unusable, and failing the
/// caller over the cleanup would replace a recoverable "authorize again" with a
/// hard error.
fn clear(store: &dyn TokenStore) {
    if let Err(e) = store.clear() {
        tracing::warn!(error = %format!("{e:#}"), "could not remove the unusable token file");
    }
}

/// Turn a classified refusal into the error the caller acts on, taking the one
/// side effect a dead grant deserves: the stored token goes, so the consent flow
/// that follows starts from an empty cache rather than re-failing on a token we
/// already know Google refuses.
fn refused(refusal: Refusal, store: &dyn TokenStore) -> ApiError {
    match refusal {
        Refusal::GrantDead {
            description,
            subtype,
        } => {
            // Verbatim, because this is the line that says *why* a grant died:
            // a revoked consent, a Testing-status project's 7-day refresh-token
            // expiry, or a Workspace session-control policy (which Google
            // distinguishes only by `error_subtype`).
            tracing::error!(
                error = "invalid_grant",
                description = description.as_deref().unwrap_or("<none>"),
                subtype = subtype.as_deref().unwrap_or("<none>"),
                "google refused the refresh: the grant is gone, re-authorization needed"
            );
            clear(store);
            ApiError::AuthExpired
        }
        Refusal::Refused { status, message } => {
            tracing::error!(status, message = %message, "google refused the refresh");
            ApiError::Rejected { status, message }
        }
        Refusal::Transient(message) => ApiError::Network(message),
    }
}

/// What a non-success answer from the token endpoint means for the grant.
#[derive(Debug, PartialEq, Eq)]
enum Refusal {
    /// The grant is gone — revoked, expired, or refused by a policy. The only
    /// refusal that may open a browser.
    GrantDead {
        description: Option<String>,
        subtype: Option<String>,
    },
    /// Google answered, and said something else: a rejected `client_secret.json`
    /// (`invalid_client`), a malformed request. Consent would meet the same
    /// refusal, so it never opens one.
    Refused { status: u16, message: String },
    /// Nobody said anything about the grant — a 5xx from Google or something in
    /// front of it. Retryable, and the stored grant stays untouched.
    Transient(String),
}

/// Classify Google's refusal from its status and body. Pure, so the rules can be
/// asserted without a socket.
fn classify(status: StatusCode, body: &str) -> Refusal {
    // Before reading the body: a 502 from a proxy can carry anything at all, and
    // a server error says nothing about the grant.
    if status.is_server_error() {
        return Refusal::Transient(format!("google token endpoint: {status}"));
    }
    match serde_json::from_str::<TokenErrorBody>(body) {
        Ok(TokenErrorBody {
            error: Some(error),
            error_description,
            error_subtype,
        }) if error == "invalid_grant" => Refusal::GrantDead {
            description: error_description,
            subtype: error_subtype,
        },
        Ok(TokenErrorBody {
            error: Some(error),
            error_description,
            ..
        }) => Refusal::Refused {
            status: status.as_u16(),
            message: match error_description {
                Some(description) => format!("{error}: {description}"),
                None => error,
            },
        },
        // Parsed, but not as an OAuth error — and an unparseable body is no
        // different. Either way Google refused and did not say the grant is
        // dead, so quote what came back rather than guessing at it.
        Ok(_) | Err(_) => Refusal::Refused {
            status: status.as_u16(),
            message: quote(body),
        },
    }
}

/// Google's error body for a refused token request.
#[derive(Deserialize)]
struct TokenErrorBody {
    error: Option<String>,
    error_description: Option<String>,
    /// Set when a Google Workspace session-control policy, rather than a revoked
    /// grant, is what refused the refresh. Both mean "authorize again", so it
    /// changes nothing we *do* — it is the only thing that says which happened.
    error_subtype: Option<String>,
}

/// The fields of a successful token response oxidone uses, from either exchange.
/// `refresh_token` is normally absent from a *refresh* and always present on a
/// code exchange; `expires_in` is documented as always present, and `None` is
/// carried as "no expiry known", exactly as `yup-oauth2` does.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

/// A body fragment safe to put in a one-line error, trimmed and length-capped.
fn quote(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return "<empty body>".to_string();
    }
    match trimmed.char_indices().nth(MAX_QUOTED_BODY) {
        Some((cut, _)) => format!("{}…", &trimmed[..cut]),
        None => trimmed.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bad_request(body: &str) -> Refusal {
        classify(StatusCode::BAD_REQUEST, body)
    }

    #[test]
    fn invalid_grant_is_a_dead_grant() {
        assert_eq!(
            bad_request(r#"{"error":"invalid_grant","error_description":"Bad Request"}"#),
            Refusal::GrantDead {
                description: Some("Bad Request".to_string()),
                subtype: None,
            }
        );
    }

    #[test]
    fn dead_grant_keeps_googles_subtype() {
        assert_eq!(
            bad_request(
                r#"{"error":"invalid_grant","error_description":"reauth related error","error_subtype":"invalid_rapt"}"#
            ),
            Refusal::GrantDead {
                description: Some("reauth related error".to_string()),
                subtype: Some("invalid_rapt".to_string()),
            }
        );
    }

    #[test]
    fn other_oauth_errors_are_refusals_not_dead_grants() {
        assert_eq!(
            bad_request(
                r#"{"error":"invalid_client","error_description":"The OAuth client was not found."}"#
            ),
            Refusal::Refused {
                status: 400,
                message: "invalid_client: The OAuth client was not found.".to_string(),
            }
        );
    }

    #[test]
    fn a_body_that_is_not_an_oauth_error_is_quoted() {
        assert_eq!(
            bad_request("<html>go away</html>"),
            Refusal::Refused {
                status: 400,
                message: "<html>go away</html>".to_string(),
            }
        );
        assert_eq!(
            classify(StatusCode::UNAUTHORIZED, ""),
            Refusal::Refused {
                status: 401,
                message: "<empty body>".to_string(),
            }
        );
    }

    #[test]
    fn server_errors_are_transient_whatever_the_body_says() {
        // A 5xx is classified before the body is read: a proxy is free to answer
        // with anything, and one that echoes `invalid_grant` must not be able to
        // talk us into throwing away a working grant.
        assert_eq!(
            classify(
                StatusCode::SERVICE_UNAVAILABLE,
                r#"{"error":"invalid_grant"}"#
            ),
            Refusal::Transient("google token endpoint: 503 Service Unavailable".to_string())
        );
    }

    #[test]
    fn quoted_bodies_are_length_capped_on_a_char_boundary() {
        let long = "ü".repeat(MAX_QUOTED_BODY * 2);
        let quoted = quote(&long);
        assert_eq!(quoted.chars().count(), MAX_QUOTED_BODY + 1);
        assert!(quoted.ends_with('…'));
    }
}
