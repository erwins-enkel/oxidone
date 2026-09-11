//! The interactive consent flow, oxidone's own (ADR-0011).
//!
//! Two things can answer a consent, and this module's whole shape is that they
//! race: the loopback redirect a browser on *this* machine delivers, and a
//! callback URL a human pastes because the browser is on a different machine
//! and `localhost` there is not `localhost` here. Neither is a mode and neither
//! has to be chosen in advance — whichever arrives first settles the flow, so
//! there is no flag to discover once you are already locked out.
//!
//! Everything except the browser hand-off is testable without one: the URL is
//! built by a pure function, both roads end in the same parser
//! ([`super::callback`]), and the exchange is a POST like any other.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use yup_oauth2::ApplicationSecret;

use super::callback::{parse_callback, request_target};
use super::consent::{CallbackInput, ConsentPrompt};
use super::pkce::Challenge;
use super::refresh;
use super::TokenStore;
use crate::api::ApiError;

/// Cap on the request line read off a loopback connection. A real callback is a
/// couple of hundred bytes; the cap is what stops something that is not a
/// browser from streaming into memory.
const MAX_REQUEST_LINE: u64 = 8 * 1024;

/// How long a loopback connection has to say what it wants.
///
/// Browsers open speculative connections to an origin and then send nothing on
/// them. Without a bound, the first such connection would park the listener in
/// `read_line` and the redirect behind it would never be read — the paste path
/// would still work, which is exactly how a bug like this survives a test.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// What the browser is left looking at. Deliberately neutral about the outcome:
/// it is written before the callback has been parsed, and a page claiming
/// success over a rejected `state` would be a lie.
const BROWSER_RESPONSE: &str = "oxidone received the callback. You can close this tab \
                                and return to your terminal.";

/// Run one interactive authorization and store the grant it produces, returning
/// its access token.
///
/// The loopback listener binds `127.0.0.1` on an ephemeral port — Google accepts
/// any loopback port on a Desktop client without registering it — and that port
/// is what the `redirect_uri` names, in the authorization URL and again in the
/// exchange, where Google compares the two.
///
/// A callback that does not parse is **not** fatal: it is reported through
/// `prompt` and the flow keeps waiting, because the usual cause is a human
/// pasting the wrong thing and the remedy is to paste the right one. Only the
/// token exchange settles a consent. There is no timeout here either — the
/// caller's [`super::SingleFlight`] owns that, so one bound covers the whole
/// acquisition rather than each half separately.
pub(super) async fn consent(
    http: &reqwest::Client,
    secret: &ApplicationSecret,
    store: &dyn TokenStore,
    prompt: &ConsentPrompt,
    input: &dyn CallbackInput,
    scope: &str,
) -> Result<String, ApiError> {
    let challenge = Challenge::new()?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| ApiError::ConsentFailed(format!("no loopback listener: {e}")))?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}",
        listener
            .local_addr()
            .map_err(|e| ApiError::ConsentFailed(format!("no loopback address: {e}")))?
            .port()
    );

    // Presenting *is* the hand-off, browser included: which ways a URL reaches
    // the user is the sink's business, not this function's (see `ConsentSink`).
    prompt.present(&authorization_url(secret, scope, &redirect_uri, &challenge));

    // Cleared once the paste side says no more can come (stdin was not a
    // terminal, or the TUI's channel closed); without it `select!` would take
    // that arm's instant `None` for ever and spin.
    let mut pasted_open = true;
    loop {
        let offered = tokio::select! {
            target = next_callback(&listener) => target?,
            pasted = input.next(), if pasted_open => match pasted {
                Some(line) => line,
                None => {
                    pasted_open = false;
                    continue;
                }
            },
        };

        match parse_callback(&offered, &challenge.state) {
            Ok(code) => {
                return refresh::exchange_code(
                    http,
                    secret,
                    store,
                    &code,
                    &challenge.verifier,
                    &redirect_uri,
                )
                .await
            }
            Err(e) => prompt.reject(&e.to_string()),
        }
    }
}

/// Google's authorization URL for this attempt.
///
/// Pure, and pinned by a test, because every parameter here is one Google
/// answers with the same opaque refusal when it is wrong or missing:
///
/// - `access_type=offline` is what makes the grant yield a refresh token at all.
/// - `prompt=consent` is what makes it yield one *every time*. Google omits the
///   refresh token when re-authorizing a client the user has already granted,
///   and a grant with no refresh token behind it sends the user back to the
///   browser as soon as its access token expires.
/// - `state` and `code_challenge` are what make the pasted road safe: the
///   redirect lands on the loopback of whatever machine the browser is on, where
///   oxidone is not listening and anything else might be.
fn authorization_url(
    secret: &ApplicationSecret,
    scope: &str,
    redirect_uri: &str,
    challenge: &Challenge,
) -> String {
    let mut url = match reqwest::Url::parse(&secret.auth_uri) {
        Ok(url) => url,
        // Not `expect`: `auth_uri` comes out of the user's `client_secret.json`,
        // so a broken one is a broken file, not a broken program. The URL is
        // returned as it stands and Google's own refusal reports it.
        Err(_) => return secret.auth_uri.clone(),
    };
    url.query_pairs_mut()
        .append_pair("client_id", &secret.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", scope)
        .append_pair("access_type", "offline")
        .append_pair("prompt", "consent")
        .append_pair("state", &challenge.state)
        .append_pair("code_challenge", &challenge.challenge)
        .append_pair("code_challenge_method", "S256");
    url.into()
}

/// Wait for the next loopback request that could be a callback, answer the
/// browser, and hand back its request target.
///
/// Anything that is not a `GET` of the redirect path is answered and dropped
/// without being offered as a callback — a browser asking for `/favicon.ico`
/// would otherwise be reported to the user as "that URL carried no
/// authorization code".
///
/// `Err` only for a listener that has stopped working; a single bad connection
/// is skipped, because the redirect may still be on its way.
async fn next_callback(listener: &TcpListener) -> Result<String, ApiError> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| ApiError::ConsentFailed(format!("the loopback listener failed: {e}")))?;
        let (reader, mut writer) = stream.split();

        // `take` before `BufReader`: the cap has to bound what is *read*, not
        // what is handed back, or a request line with no newline in it would be
        // buffered until the connection or the memory ran out.
        let mut reader = BufReader::new(reader.take(MAX_REQUEST_LINE));
        let mut line = String::new();
        let read = tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut line)).await;

        // Answered before the target is judged: the browser is owed a page
        // either way, and this is the last chance to write one.
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{BROWSER_RESPONSE}",
            BROWSER_RESPONSE.len()
        );
        if let Err(e) = writer.write_all(response.as_bytes()).await {
            tracing::warn!(error = %e, "could not answer the loopback callback");
        }

        match read {
            Ok(Ok(_)) => {
                // Only the redirect path itself. `split('?')` rather than a full
                // parse: this is a raw request line, and its query is the
                // parser's business, not ours.
                match request_target(&line).map(|target| target.split('?').next() == Some("/")) {
                    Some(true) => return Ok(line_target(&line)),
                    _ => tracing::debug!(
                        request = %line.trim_end(),
                        "ignored a non-callback loopback request"
                    ),
                }
            }
            Ok(Err(e)) => tracing::warn!(error = %e, "could not read a loopback request"),
            Err(_) => tracing::debug!("a loopback connection said nothing; dropped it"),
        }
    }
}

/// The request target of `line`, which [`next_callback`] has already confirmed
/// it has — split out only because the borrow of `line` cannot outlive the
/// `match` that checked it.
fn line_target(line: &str) -> String {
    request_target(line)
        .expect("the target was just matched")
        .to_string()
}

/// Shared ownership of the pieces one consent needs, so [`consent`] can be
/// reached from a `'static` closure without borrowing the provider.
pub(super) struct ConsentParts {
    pub(super) http: reqwest::Client,
    pub(super) secret: ApplicationSecret,
    pub(super) store: Arc<dyn TokenStore>,
    pub(super) prompt: Arc<ConsentPrompt>,
    pub(super) input: Arc<dyn CallbackInput>,
    pub(super) scope: &'static str,
}

impl ConsentParts {
    pub(super) async fn run(&self) -> Result<String, ApiError> {
        consent(
            &self.http,
            &self.secret,
            &*self.store,
            &self.prompt,
            &*self.input,
            self.scope,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret() -> ApplicationSecret {
        ApplicationSecret {
            client_id: "1001.apps.googleusercontent.com".to_string(),
            client_secret: "shh".to_string(),
            auth_uri: "https://accounts.google.com/o/oauth2/auth".to_string(),
            token_uri: "https://oauth2.googleapis.com/token".to_string(),
            ..Default::default()
        }
    }

    /// The parameter set, exactly. Every one of these is a silent failure when
    /// it is wrong: a dropped `access_type`/`prompt` costs the refresh token and
    /// re-prompts daily, a dropped `code_challenge` breaks the exchange that
    /// sends the verifier, and a dropped `state` retires the only check the
    /// pasted road has.
    #[test]
    fn the_authorization_url_carries_what_google_needs() {
        let challenge = Challenge::new().expect("system randomness");
        let url = authorization_url(
            &secret(),
            "https://www.googleapis.com/auth/tasks",
            "http://127.0.0.1:37137",
            &challenge,
        );
        let parsed = reqwest::Url::parse(&url).expect("a URL");
        let params: Vec<(String, String)> = parsed
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        assert_eq!(
            params,
            vec![
                (
                    "client_id".to_string(),
                    "1001.apps.googleusercontent.com".to_string()
                ),
                (
                    "redirect_uri".to_string(),
                    "http://127.0.0.1:37137".to_string()
                ),
                ("response_type".to_string(), "code".to_string()),
                (
                    "scope".to_string(),
                    "https://www.googleapis.com/auth/tasks".to_string()
                ),
                ("access_type".to_string(), "offline".to_string()),
                ("prompt".to_string(), "consent".to_string()),
                ("state".to_string(), challenge.state.clone()),
                ("code_challenge".to_string(), challenge.challenge.clone()),
                ("code_challenge_method".to_string(), "S256".to_string()),
            ]
        );
        assert_eq!(parsed.path(), "/o/oauth2/auth");
    }

    /// The regression this guards: the verifier is the one PKCE secret that must
    /// never leave the process, and the authorization URL is the half of the
    /// flow that travels through a browser, a paste buffer and possibly a chat
    /// window.
    #[test]
    fn the_authorization_url_never_carries_the_verifier() {
        let challenge = Challenge::new().expect("system randomness");
        let url = authorization_url(&secret(), "scope", "http://127.0.0.1:1", &challenge);
        assert!(!url.contains(&challenge.verifier));
    }
}
