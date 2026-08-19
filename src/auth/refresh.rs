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
//! only one outcome — a refusal Google itself labelled `invalid_grant`, or a
//! stored blob with no refresh token to send — is allowed to mean "open a
//! browser". Everything else keeps its own [`ApiError`] class and leaves the
//! stored grant where it is.

use reqwest::StatusCode;
use serde::Deserialize;
use time::{Duration, OffsetDateTime};
use yup_oauth2::storage::TokenInfo;
use yup_oauth2::ApplicationSecret;

use super::TokenStore;
use crate::api::ApiError;

/// Cap on how much of Google's body is quoted into an [`ApiError`]. The token
/// endpoint's own errors are a line of JSON, but a proxy in the way can answer
/// with a whole HTML page, and this text ends up in a single-row status line.
const MAX_QUOTED_BODY: usize = 200;

/// Hand back a usable bearer for the stored grant, refreshing it against
/// Google's token endpoint when the cached access token is spent — or whenever
/// `force`, which is the 401 replay in [`crate::api::rest`]: a token the cache
/// still believes in can already have been rejected by the server.
///
/// `Err(ApiError::AuthExpired)` is the *only* outcome that means "run the
/// interactive consent flow". Every other failure keeps its own class, so a
/// dropped connection, a rejected `client_secret.json`, or a config dir that
/// refuses the write can never be mistaken for a dead grant and answered with a
/// browser window.
pub async fn cached_or_refreshed(
    http: &reqwest::Client,
    secret: &ApplicationSecret,
    store: &dyn TokenStore,
    force: bool,
) -> Result<String, ApiError> {
    let Some(stored) = load(store) else {
        // Nothing usable cached: only consent can produce a grant from here.
        return Err(ApiError::AuthExpired);
    };

    if !force {
        if let Some(access) = stored.access_token.as_deref() {
            // `TokenInfo::is_expired` carries yup-oauth2's one-minute margin —
            // borrowed rather than reinvented, so there is a single definition of
            // "spent" in the codebase.
            if !stored.is_expired() {
                return Ok(access.to_owned());
            }
        }
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

    let refreshed: RefreshResponse = serde_json::from_str(&body).map_err(|e| {
        // A 200 that does not carry a token is a protocol violation, not a
        // transient fault: backing off and retrying would be the wrong advice.
        ApiError::Rejected {
            status: status.as_u16(),
            message: format!("malformed token response: {e}"),
        }
    })?;

    let bearer = refreshed.access_token;
    let token = TokenInfo {
        access_token: Some(bearer.clone()),
        // Google omits `refresh_token` from a refresh response in the normal
        // case, and the grant it belongs to is still ours — so the stored one is
        // carried forward rather than dropped.
        refresh_token: Some(
            refreshed
                .refresh_token
                .unwrap_or_else(|| refresh_token.to_owned()),
        ),
        // `checked_add`, because `expires_in` comes off the wire and adding an
        // absurd one to `now` panics. `None` there means "no expiry known", the
        // same as an answer that omitted it: the token is used until a 401 forces
        // a refresh, which is a worse deal than a real expiry and better than a
        // crash.
        expires_at: refreshed
            .expires_in
            .and_then(|seconds| OffsetDateTime::now_utc().checked_add(Duration::seconds(seconds))),
        id_token: refreshed.id_token,
    };
    persist(store, &token).map_err(ApiError::TokenNotPersisted)?;
    Ok(bearer)
}

/// Read and parse the stored token cache, or `None` if there is nothing usable
/// there. A read or parse failure degrades to `None` — re-authorizing is the only
/// remedy either way — but it is logged, so a corrupt file or a transient read
/// error is not silently indistinguishable from a first run.
pub(super) fn load(store: &dyn TokenStore) -> Option<TokenInfo> {
    let blob = match store.load() {
        Ok(blob) => blob?,
        Err(e) => {
            tracing::warn!(error = %format!("{e:#}"), "reading the cached token failed; will re-authenticate");
            return None;
        }
    };
    match serde_json::from_str(&blob) {
        Ok(token) => Some(token),
        Err(e) => {
            tracing::warn!(error = %e, "the cached token is corrupt; will re-authenticate");
            None
        }
    }
}

/// Serialize `token` into the store, loudly. `Err` carries why it could not be
/// written — the caller decides what to wrap it in, so the reason is stated once
/// however many layers it passes through.
///
/// Every caller turns this into [`ApiError::TokenNotPersisted`] and never anything
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

/// The fields of a successful refresh oxidone uses. `refresh_token` is normally
/// absent; `expires_in` is documented as always present, and `None` is carried as
/// "no expiry known", exactly as `yup-oauth2` does.
#[derive(Deserialize)]
struct RefreshResponse {
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
