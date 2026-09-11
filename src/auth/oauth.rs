//! The BYO OAuth provider: a grant read from the store, refreshed when spent,
//! and acquired interactively when it is gone (ADR-0002, ADR-0009, ADR-0011).
//!
//! Both exchanges against Google's token endpoint are oxidone's own
//! ([`super::refresh`]) and so is the consent flow itself ([`super::flow`]);
//! `yup-oauth2` is left holding the `client_secret.json` reader and the
//! `TokenInfo` shape the store persists.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use yup_oauth2::{read_application_secret, ApplicationSecret};

use super::consent::{CallbackInput, ConsentPrompt};
use super::flow::ConsentParts;
use super::refresh;
use super::single_flight::{SingleFlight, CONSENT_TIMEOUT};
use super::{TokenProvider, TokenStore};
use crate::api::ApiError;

/// Full read/write access to the user's Google Tasks.
const TASKS_SCOPE: &str = "https://www.googleapis.com/auth/tasks";

type BearerFuture = Pin<Box<dyn Future<Output = Result<String, ApiError>> + Send>>;
type BearerFn = Box<dyn Fn() -> BearerFuture + Send + Sync>;

/// A `TokenProvider` over the BYO client credentials. `bearer()` answers from the
/// stored grant — the cached access token, or a refresh of it that oxidone
/// performs itself — and reaches the interactive consent flow only when that
/// grant is *gone*, never when the attempt merely failed.
pub struct GoogleTokenProvider {
    http: reqwest::Client,
    secret: ApplicationSecret,
    store: Arc<dyn TokenStore>,
    consent: BearerFn,
}

impl GoogleTokenProvider {
    /// Build a provider from the BYO `client_secret.json`, persisting tokens
    /// through `store`. Does not itself trigger the interactive flow — that
    /// happens lazily on the first `bearer()` that finds no usable grant (see
    /// [`login`]).
    ///
    /// `prompt` is where a consent URL goes when that lazy flow does fire, and
    /// `input` is where a pasted callback URL comes back from. Neither is
    /// optional: a consent that cannot reach the user is one that cannot finish,
    /// and on a machine whose loopback the browser cannot reach the paste is the
    /// only road in.
    pub async fn new(
        client_secret_path: &Path,
        store: Arc<dyn TokenStore>,
        prompt: Arc<ConsentPrompt>,
        input: Arc<dyn CallbackInput>,
    ) -> anyhow::Result<Self> {
        let secret = read_application_secret(client_secret_path)
            .await
            .with_context(|| {
                format!("reading BYO client secret {}", client_secret_path.display())
            })?;

        // One client for both exchanges: `reqwest::Client` is a handle on a
        // shared connection pool, so cloning it is how you get a second user of
        // it rather than a second pool.
        let http = reqwest::Client::new();

        // The consent flow, and only the consent flow. It is reached only when
        // there is no usable grant left — cleared because Google refused it,
        // never written, or unparseable — so nothing here retries a refresh that
        // has already been classified.
        let parts = Arc::new(ConsentParts {
            http: http.clone(),
            secret: secret.clone(),
            store: Arc::clone(&store),
            prompt,
            input,
            scope: TASKS_SCOPE,
        });
        let consent: BearerFn = Box::new(move || {
            let parts = Arc::clone(&parts);
            Box::pin(async move { parts.run().await })
        });

        Ok(Self {
            http,
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
impl TokenProvider for GoogleTokenProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.grant_or_consent(false).await
    }

    async fn refresh(&self) -> Result<String, ApiError> {
        self.grant_or_consent(true).await
    }
}

/// First-run: build the provider and force one token acquisition, which presents
/// the consent URL, opens the system browser on it, runs the loopback listener
/// *and* accepts a pasted callback URL, exchanges the code, and persists the
/// refresh token via the `TokenStore`.
///
/// This one runs *before* the TUI, so `prompt` and `input` are the terminal's —
/// stdout and the line the user types there. It is wrapped in [`SingleFlight`]
/// for the timeout rather than the serialization (there is a single caller
/// here): both roads into a consent wait indefinitely, and "starting offline" is
/// a better answer to an abandoned first run than a silent hang.
pub async fn login(
    client_secret_path: &Path,
    store: Arc<dyn TokenStore>,
    prompt: Arc<ConsentPrompt>,
    input: Arc<dyn CallbackInput>,
) -> anyhow::Result<()> {
    let provider =
        GoogleTokenProvider::new(client_secret_path, store, Arc::clone(&prompt), input).await?;
    SingleFlight::new(provider, prompt, CONSENT_TIMEOUT)
        .bearer()
        .await
        .map_err(|e| anyhow::anyhow!("initial authorization failed: {e}"))?;
    Ok(())
}
