//! BYO OAuth loopback flow via `yup-oauth2` (ADR-0002, ADR-0004). Auth is the
//! one part we do *not* hand-roll: the loopback listener, code exchange, and
//! transparent refresh all come from `yup-oauth2`. We only bridge its token
//! cache onto our `TokenStore` so the refresh token lands in the `chmod 600`
//! file.
//!
//! The interactive first-run path (opening a browser, capturing the loopback
//! redirect) cannot run headless, so it is compile-verified only. The parts
//! that don't need a browser — the token store and the REST layer — carry the
//! real test coverage.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use yup_oauth2::storage::{TokenInfo, TokenStorage, TokenStorageError};
use yup_oauth2::{read_application_secret, InstalledFlowAuthenticator, InstalledFlowReturnMethod};

use super::consent::{ConsentPrompt, StdoutConsentSink};
use super::single_flight::{SingleFlight, CONSENT_TIMEOUT};
use super::{TokenProvider, TokenStore};
use crate::api::ApiError;
use crate::links::OpenableUrl;

/// Full read/write access to the user's Google Tasks.
const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

type BearerFuture = Pin<Box<dyn Future<Output = Result<String, ApiError>> + Send>>;
type BearerFn = Box<dyn Fn() -> BearerFuture + Send + Sync>;

/// A `TokenProvider` backed by a live `yup-oauth2` authenticator. Each
/// `bearer()` returns a valid access token, transparently refreshing (and
/// re-persisting via the `TokenStore`) when the cached one has expired.
///
/// The concrete authenticator's connector type is erased behind a boxed
/// closure, so this struct stays a plain, nameable type.
pub struct YupTokenProvider {
    fetch: BearerFn,
    force: BearerFn,
}

impl YupTokenProvider {
    /// Build an authenticator from the BYO `client_secret.json`, persisting its
    /// token cache through `store`. Does not itself trigger the interactive
    /// flow — that happens lazily on the first `bearer()` if no cached token
    /// exists (see [`login`]).
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

        let auth =
            InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
                .with_storage(Box::new(StoreBridge { inner: store }))
                .flow_delegate(Box::new(PromptFlowDelegate { prompt }))
                .build()
                .await
                .context("building yup-oauth2 authenticator")?;

        let auth = Arc::new(auth);
        let scopes: Arc<[String]> = Arc::from(vec![TASKS_SCOPE.to_string()]);

        let fetch: BearerFn = {
            let auth = Arc::clone(&auth);
            let scopes = Arc::clone(&scopes);
            Box::new(move || {
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
            })
        };

        // Force a fresh access token even if the cached one still looks valid —
        // used to retry after a server 401.
        let force: BearerFn = Box::new(move || {
            let auth = Arc::clone(&auth);
            let scopes = Arc::clone(&scopes);
            Box::pin(async move {
                let token = auth
                    .force_refreshed_token(scopes.as_ref())
                    .await
                    .map_err(|e| map_token_error(&e))?;
                token
                    .token()
                    .map(str::to_owned)
                    .ok_or(ApiError::AuthExpired)
            })
        });

        Ok(Self { fetch, force })
    }
}

#[async_trait]
impl TokenProvider for YupTokenProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        (self.fetch)().await
    }

    async fn refresh(&self) -> Result<String, ApiError> {
        (self.force)().await
    }
}

/// First-run: build the authenticator and force one token acquisition, which
/// opens the system browser to Google's consent screen, runs the `localhost`
/// loopback listener, exchanges the code, and persists the refresh token via
/// the `TokenStore`.
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

/// Best-effort classification of a `yup-oauth2` token error. A refused refresh
/// (expired/revoked grant) surfaces as `AuthExpired` so a caller can prompt for
/// re-login; anything else is treated as a transport failure.
fn map_token_error(err: &yup_oauth2::Error) -> ApiError {
    match err {
        yup_oauth2::Error::AuthError(_) => ApiError::AuthExpired,
        other => ApiError::Network(other.to_string()),
    }
}

/// Adapts our single-blob `TokenStore` to yup-oauth2's per-scope `TokenStorage`.
/// oxidone uses one fixed scope set, so a single serialized `TokenInfo` blob is
/// sufficient; the scope key is ignored.
struct StoreBridge {
    inner: Arc<dyn TokenStore>,
}

#[async_trait]
impl TokenStorage for StoreBridge {
    async fn set(&self, _scopes: &[&str], token: TokenInfo) -> Result<(), TokenStorageError> {
        let json = serde_json::to_string(&token)
            .map_err(|e| TokenStorageError::Other(e.to_string().into()))?;
        self.inner
            .save(&json)
            .map_err(|e| TokenStorageError::Other(e.to_string().into()))
    }

    async fn get(&self, _scopes: &[&str]) -> Option<TokenInfo> {
        // yup-oauth2's storage API is `Option`-returning, so a read/parse
        // failure can only degrade to "no token" (forcing re-login). Log it so
        // a transient read error or a corrupt file isn't silently swallowed.
        let blob = match self.inner.load() {
            Ok(blob) => blob?,
            Err(e) => {
                tracing::warn!(error = %e, "reading cached token failed; will re-authenticate");
                return None;
            }
        };
        match serde_json::from_str(&blob) {
            Ok(token) => Some(token),
            Err(e) => {
                tracing::warn!(error = %e, "cached token is corrupt; will re-authenticate");
                None
            }
        }
    }
}
