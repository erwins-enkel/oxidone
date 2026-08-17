//! Serializes token acquisition, so a cache miss can drive at most **one**
//! interactive consent flow.
//!
//! `yup_oauth2::Authenticator` deduplicates nothing: every concurrent `token()`
//! that misses its storage runs the whole installed flow itself, on a loopback
//! listener of its own. oxidone fires two API calls the moment it starts, so an
//! unusable cached token produced two consent URLs on two ports — and the
//! browser's redirect can only ever reach one of them.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::{ConsentPrompt, TokenProvider};
use crate::api::ApiError;

/// How long one acquisition may run before it is abandoned. Sized for a real
/// consent flow — an account chooser, a password, a 2FA prompt on a second
/// device — not for a token refresh, which takes a fraction of a second.
pub const CONSENT_TIMEOUT: Duration = Duration::from_secs(180);

/// Wraps a [`TokenProvider`] so at most one acquisition runs at a time, bounded
/// by a timeout, retracting the [`ConsentPrompt`] whichever way it ends.
pub struct SingleFlight<P> {
    inner: P,
    prompt: Arc<ConsentPrompt>,
    timeout: Duration,
    gate: Mutex<()>,
}

impl<P> SingleFlight<P> {
    pub fn new(inner: P, prompt: Arc<ConsentPrompt>, timeout: Duration) -> Self {
        Self {
            inner,
            prompt,
            timeout,
            gate: Mutex::new(()),
        }
    }

    /// Run `acquire` alone, bounded by `self.timeout`, then retract the prompt.
    ///
    /// The gate is held for the *whole* acquisition, which is what collapses the
    /// race: a caller that takes it second re-enters the provider, whose storage
    /// now holds the token the first caller persisted, so it answers from cache
    /// instead of opening a second flow.
    async fn guarded<Fut>(&self, acquire: Fut) -> Result<String, ApiError>
    where
        Fut: Future<Output = Result<String, ApiError>> + Send,
    {
        let _gate = self.gate.lock().await;
        let (outcome, reason) = match tokio::time::timeout(self.timeout, acquire).await {
            Ok(Ok(token)) => (Ok(token), None),
            Ok(Err(e)) => {
                let reason = format!("authorization failed: {e}");
                (Err(e), Some(reason))
            }
            // Dropping the acquisition future drops `yup-oauth2`'s loopback
            // listener with it, so an abandoned consent flow cannot hold the gate
            // — or its port — for the rest of the session. Without this bound the
            // gate above would turn one unanswered browser prompt into a
            // permanently wedged API layer: the listener awaits its redirect
            // forever.
            Err(_) => (
                Err(ApiError::AuthExpired),
                Some(format!(
                    "authorization was not completed within {:?}",
                    self.timeout
                )),
            ),
        };
        self.prompt.dismiss(reason.as_deref());
        outcome
    }
}

#[async_trait]
impl<P: TokenProvider> TokenProvider for SingleFlight<P> {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.guarded(self.inner.bearer()).await
    }

    /// Shares `bearer`'s gate rather than taking one of its own: a forced refresh
    /// also falls through to a full consent flow when the refresh token is gone
    /// or Google rejects it, so a 401 retry racing a startup fetch would be a
    /// second flow all the same.
    async fn refresh(&self) -> Result<String, ApiError> {
        self.guarded(self.inner.refresh()).await
    }
}
