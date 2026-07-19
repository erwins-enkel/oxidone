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
use yup_oauth2::storage::{TokenInfo, TokenStorage, TokenStorageError};
use yup_oauth2::{read_application_secret, InstalledFlowAuthenticator, InstalledFlowReturnMethod};

use super::{TokenProvider, TokenStore};
use crate::api::ApiError;

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
}

impl YupTokenProvider {
    /// Build an authenticator from the BYO `client_secret.json`, persisting its
    /// token cache through `store`. Does not itself trigger the interactive
    /// flow — that happens lazily on the first `bearer()` if no cached token
    /// exists (see [`login`]).
    pub async fn new(
        client_secret_path: &Path,
        store: Arc<dyn TokenStore>,
    ) -> anyhow::Result<Self> {
        let secret = read_application_secret(client_secret_path)
            .await
            .with_context(|| {
                format!("reading BYO client secret {}", client_secret_path.display())
            })?;

        let auth =
            InstalledFlowAuthenticator::builder(secret, InstalledFlowReturnMethod::HTTPRedirect)
                .with_storage(Box::new(StoreBridge { inner: store }))
                .build()
                .await
                .context("building yup-oauth2 authenticator")?;

        let auth = Arc::new(auth);
        let scopes: Arc<[String]> = Arc::from(vec![TASKS_SCOPE.to_string()]);

        let fetch: BearerFn = Box::new(move || {
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

        Ok(Self { fetch })
    }
}

#[async_trait]
impl TokenProvider for YupTokenProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        (self.fetch)().await
    }
}

/// First-run: build the authenticator and force one token acquisition, which
/// opens the system browser to Google's consent screen, runs the `localhost`
/// loopback listener, exchanges the code, and persists the refresh token via
/// the `TokenStore`.
pub async fn login(client_secret_path: &Path, store: Arc<dyn TokenStore>) -> anyhow::Result<()> {
    let provider = YupTokenProvider::new(client_secret_path, store).await?;
    provider
        .bearer()
        .await
        .map_err(|e| anyhow::anyhow!("initial authorization failed: {e}"))?;
    Ok(())
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
