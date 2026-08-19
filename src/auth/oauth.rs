//! BYO OAuth loopback flow via `yup-oauth2` (ADR-0002, ADR-0004, ADR-0009).
//! `yup-oauth2` owns exactly one thing: the interactive consent flow — the
//! loopback listener, the browser hand-off, and the code exchange. Refreshing an
//! existing grant is oxidone's ([`super::refresh`]), because yup's own refresh
//! path cannot tell a dead grant from a dropped connection and answers both by
//! opening a browser.
//!
//! The interactive path (opening a browser, capturing the loopback redirect)
//! cannot run headless, so it is compile-verified only. Everything that does not
//! need a browser — the refresh exchange, the token store, the REST layer —
//! carries the real test coverage.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use yup_oauth2::storage::{TokenInfo, TokenStorage, TokenStorageError};
use yup_oauth2::{
    read_application_secret, ApplicationSecret, InstalledFlowAuthenticator,
    InstalledFlowReturnMethod,
};

use super::consent::{ConsentPrompt, StdoutConsentSink};
use super::refresh;
use super::single_flight::{SingleFlight, CONSENT_TIMEOUT};
use super::{TokenProvider, TokenStore};
use crate::api::ApiError;
use crate::links::OpenableUrl;

/// Full read/write access to the user's Google Tasks.
const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

type BearerFuture = Pin<Box<dyn Future<Output = Result<String, ApiError>> + Send>>;
type BearerFn = Box<dyn Fn() -> BearerFuture + Send + Sync>;

/// A `TokenProvider` over the BYO client credentials. `bearer()` answers from the
/// stored grant — the cached access token, or a refresh of it that oxidone
/// performs itself — and reaches the interactive consent flow only when that
/// grant is *gone*, never when the attempt merely failed.
///
/// The authenticator behind the consent flow has a connector type parameter, so
/// it is erased behind a boxed closure and this struct stays a plain, nameable
/// type.
pub struct YupTokenProvider {
    http: reqwest::Client,
    secret: ApplicationSecret,
    store: Arc<dyn TokenStore>,
    consent: BearerFn,
}

impl YupTokenProvider {
    /// Build a provider from the BYO `client_secret.json`, persisting tokens
    /// through `store`. Does not itself trigger the interactive flow — that
    /// happens lazily on the first `bearer()` that finds no usable grant (see
    /// [`login`]).
    ///
    /// `prompt` is where a consent URL goes when that lazy flow does fire. It is
    /// not optional: the default `yup-oauth2` delegate writes the URL to stdout,
    /// which scrolls the frame apart if the TUI is up by then.
    pub async fn new(
        client_secret_path: &Path,
        store: Arc<dyn TokenStore>,
        prompt: Arc<ConsentPrompt>,
    ) -> anyhow::Result<Self> {
        let secret = read_application_secret(client_secret_path)
            .await
            .with_context(|| {
                format!("reading BYO client secret {}", client_secret_path.display())
            })?;

        let auth = InstalledFlowAuthenticator::builder(
            secret.clone(),
            InstalledFlowReturnMethod::HTTPRedirect,
        )
        .with_storage(Box::new(StoreBridge {
            inner: Arc::clone(&store),
        }))
        .flow_delegate(Box::new(PromptFlowDelegate { prompt }))
        .build()
        .await
        .context("building yup-oauth2 authenticator")?;

        let auth = Arc::new(auth);
        let scopes: Arc<[String]> = Arc::from(vec![TASKS_SCOPE.to_string()]);

        // The consent flow, and only the consent flow. It is reached only when
        // there is no usable grant left — cleared because Google refused it, never
        // written, or unparseable — so yup's own storage read comes back empty and
        // it goes straight to the browser instead of retrying a refresh we have
        // already classified.
        let consent: BearerFn = Box::new(move || {
            let auth = Arc::clone(&auth);
            let scopes = Arc::clone(&scopes);
            Box::pin(async move {
                let token = auth
                    .token(scopes.as_ref())
                    .await
                    .map_err(|e| map_token_error(&e))?;
                token
                    .token()
                    .map(str::to_owned)
                    .ok_or(ApiError::AuthExpired)
            })
        });

        Ok(Self {
            http: reqwest::Client::new(),
            secret,
            store,
            consent,
        })
    }

    /// Answer from the stored grant, falling through to consent only on
    /// [`ApiError::AuthExpired`] — which [`refresh::cached_or_refreshed`] returns
    /// for a grant Google refused as `invalid_grant`, for a blob with nothing left
    /// to exchange, and for a cache with nothing usable in it at all (a first run,
    /// or contents that are not a token). Every other error is returned as it is:
    /// no browser. A `TokenStore` that *fails* is in that second group — it is a
    /// broken file, not a missing grant, and consenting would only prompt again on
    /// the next launch.
    async fn grant_or_consent(&self, force: bool) -> Result<String, ApiError> {
        match refresh::cached_or_refreshed(&self.http, &self.secret, &*self.store, force).await {
            Err(ApiError::AuthExpired) => (self.consent)().await,
            outcome => outcome,
        }
    }
}

#[async_trait]
impl TokenProvider for YupTokenProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.grant_or_consent(false).await
    }

    async fn refresh(&self) -> Result<String, ApiError> {
        self.grant_or_consent(true).await
    }
}

/// First-run: build the provider and force one token acquisition, which opens the
/// system browser to Google's consent screen, runs the `localhost` loopback
/// listener, exchanges the code, and persists the refresh token via the
/// `TokenStore`.
///
/// This one runs *before* the TUI, so its consent URL goes to stdout — the only
/// place the user can see it yet. It is wrapped in [`SingleFlight`] for the
/// timeout rather than the serialization (there is a single caller here): the
/// loopback listener waits for its redirect for ever, and "starting offline" is a
/// better answer to an abandoned first run than a silent hang.
pub async fn login(client_secret_path: &Path, store: Arc<dyn TokenStore>) -> anyhow::Result<()> {
    let prompt = Arc::new(ConsentPrompt::new(Box::new(StdoutConsentSink)));
    let provider = YupTokenProvider::new(client_secret_path, store, Arc::clone(&prompt)).await?;
    SingleFlight::new(provider, prompt, CONSENT_TIMEOUT)
        .bearer()
        .await
        .map_err(|e| anyhow::anyhow!("initial authorization failed: {e}"))?;
    Ok(())
}

/// Presents the consent URL through a [`ConsentPrompt`] and hands it to the
/// browser, in place of `yup-oauth2`'s default delegate — which `println!`s the
/// URL, corrupting the frame whenever the TUI owns the terminal.
struct PromptFlowDelegate {
    prompt: Arc<ConsentPrompt>,
}

impl InstalledFlowDelegate for PromptFlowDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            if need_code {
                // Unreachable under `HTTPRedirect`, the only return method
                // oxidone builds with. Refuse rather than inherit the default's
                // blocking read from stdin, which no TUI user could answer — and
                // which would sit there invisibly behind the alternate screen.
                return Err("oxidone supports only the loopback redirect flow".to_string());
            }
            self.prompt.present(url);
            open_in_browser(url);
            Ok(String::new())
        })
    }
}

/// Hand `url` to the platform browser on a blocking thread — the spawn itself is
/// synchronous work — mirroring the Task-link opener in `main.rs`.
///
/// Best effort by design: the prompt is already showing the URL, so a browser
/// that will not start costs the user a copy-paste, not their session. The scheme
/// check is not bypassed just because this URL is one we built.
fn open_in_browser(url: &str) {
    let Some(target) = OpenableUrl::parse(url) else {
        tracing::warn!(%url, "consent url is not openable; showing it only");
        return;
    };
    tokio::task::spawn_blocking(move || {
        if let Err(e) = open::that_detached(target.as_str()) {
            tracing::warn!(error = %e, "could not open the browser for consent");
        }
    });
}

/// Classify a `yup-oauth2` error from the consent flow.
///
/// The three cases are kept apart because they call for different things: a
/// refused authorization means authorize again, a token store that could not take
/// the token means fix the file (and will otherwise re-prompt on every start), and
/// anything else is the transport. A store failure used to fall through to
/// `ApiError::Network`, where the retry advice is wrong and the class is exactly
/// the one a caller is entitled to ignore.
fn map_token_error(err: &yup_oauth2::Error) -> ApiError {
    match err {
        yup_oauth2::Error::AuthError(_) => ApiError::AuthExpired,
        yup_oauth2::Error::StorageError(e) => ApiError::TokenStoreFailed(e.to_string()),
        other => ApiError::Network(other.to_string()),
    }
}

/// Adapts our single-blob `TokenStore` to yup-oauth2's per-scope `TokenStorage`,
/// for the consent flow's own write. oxidone uses one fixed scope set, so a single
/// serialized `TokenInfo` blob is sufficient; the scope key is ignored.
struct StoreBridge {
    inner: Arc<dyn TokenStore>,
}

#[async_trait]
impl TokenStorage for StoreBridge {
    async fn set(&self, _scopes: &[&str], token: TokenInfo) -> Result<(), TokenStorageError> {
        // Shared with the refresh path, so a consent's token and a refreshed one
        // are written — and a failure logged — by the same code.
        refresh::persist(&*self.inner, &token).map_err(|e| TokenStorageError::Other(e.into()))
    }

    async fn get(&self, _scopes: &[&str]) -> Option<TokenInfo> {
        // yup-oauth2's storage API is `Option`-returning, so a *failed* read can
        // only be reported here as "nothing stored". That is safe on this path and
        // nowhere else: the consent flow is only ever reached once
        // `cached_or_refreshed` has read the store itself and classified what it
        // found, and a read failure never gets that far. `load` has already logged
        // it either way.
        refresh::load(&*self.inner).ok().flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yup_oauth2::error::{AuthError, AuthErrorCode, TokenStorageError};

    #[test]
    fn a_refused_authorization_is_an_expired_grant() {
        let err = yup_oauth2::Error::AuthError(AuthError {
            error: AuthErrorCode::InvalidGrant,
            error_description: Some("Token has been expired or revoked.".to_string()),
            error_uri: None,
        });
        assert_eq!(map_token_error(&err), ApiError::AuthExpired);
    }

    #[test]
    fn a_token_that_could_not_be_stored_is_not_a_network_error() {
        // The regression this guards: a read-only config dir reporting itself as a
        // transient network fault, which is the one class the provider's callers
        // are entitled to shrug off and retry later.
        let err = yup_oauth2::Error::StorageError(TokenStorageError::Other(
            "writing token file /nope/token.json: Permission denied".into(),
        ));
        assert_eq!(
            map_token_error(&err),
            ApiError::TokenStoreFailed(
                "writing token file /nope/token.json: Permission denied".to_string()
            )
        );
    }

    #[test]
    fn anything_else_is_the_transport() {
        let err = yup_oauth2::Error::UserError("no listener".to_string());
        assert_eq!(
            map_token_error(&err),
            ApiError::Network("Invalid user input: no listener".to_string())
        );
    }
}
