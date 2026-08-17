//! Serializes token acquisition, so a cache miss can drive at most **one**
//! interactive consent flow.
//!
//! `yup_oauth2::Authenticator` deduplicates nothing: every concurrent `token()`
//! that misses its storage runs the whole installed flow itself, on a loopback
//! listener of its own. oxidone fires two API calls the moment it starts, and one
//! per List in the startup fan-out, so an unusable cached token produced a consent
//! URL per caller on a port per caller — and the browser's redirect can only ever
//! reach one of them.
//!
//! Serializing them is necessary but not sufficient: a failure has to be *shared*
//! with everyone already queued, or they take their turns at the same unusable
//! cache one after another and the flood comes back single file.

use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
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
    /// The gate, carrying the last acquisition's failure so the callers queued
    /// behind it can read the outcome instead of repeating what produced it.
    gate: Mutex<Option<Failure>>,
    /// Acquisitions finished so far, readable *without* taking the gate — a caller
    /// has to know what it is queueing behind before it starts to wait, and
    /// reading that through the gate would mean waiting first.
    finished: AtomicU64,
}

/// A failed acquisition, and the [`SingleFlight::finished`] count it landed at.
struct Failure {
    at: u64,
    error: ApiError,
}

impl<P> SingleFlight<P> {
    pub fn new(inner: P, prompt: Arc<ConsentPrompt>, timeout: Duration) -> Self {
        Self {
            inner,
            prompt,
            timeout,
            gate: Mutex::new(None),
            finished: AtomicU64::new(0),
        }
    }

    /// Run `acquire` alone, bounded by `self.timeout`, then retract the prompt —
    /// unless an acquisition already failed on our behalf while we queued, in which
    /// case its outcome is ours and nothing is run at all.
    ///
    /// Serializing is only half of it. The gate is held for the *whole*
    /// acquisition, so a caller that takes it after a **successful** one re-enters
    /// the provider and answers from the token it just persisted. But a caller that
    /// takes it after a **failed** one would find the same unusable cache and open
    /// its own consent flow: another URL, another browser window, another
    /// `self.timeout` of every other caller's time — one per queued caller, and a
    /// cache read that fails for everyone (a corrupt `token.json`) queues one per
    /// List in the startup fan-out. That is the doubled prompt this type exists to
    /// prevent, arriving by a slower road, so a failure is shared with everyone
    /// already waiting on it.
    ///
    /// Shared with *those* callers only. A failure from before we began waiting
    /// says nothing about now — the user may have re-authorized since — so it is
    /// retried rather than latched, and one timeout cannot shut the session out of
    /// authorizing again.
    async fn guarded<Fut>(&self, acquire: Fut) -> Result<String, ApiError>
    where
        Fut: Future<Output = Result<String, ApiError>> + Send,
    {
        let waiting_since = self.finished.load(Ordering::Acquire);
        let mut gate = self.gate.lock().await;
        // `>`, not `>=`: a failure recorded at exactly the count we read had already
        // finished when we read it, which makes us a fresh caller, not a queued one.
        if let Some(failure) = gate.as_ref().filter(|failure| failure.at > waiting_since) {
            return Err(failure.error.clone());
        }

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

        // Bumped and recorded under the gate, so the count and the failure beside it
        // can never disagree about which acquisition it belongs to.
        let finished = self.finished.fetch_add(1, Ordering::AcqRel) + 1;
        *gate = outcome.as_ref().err().map(|error| Failure {
            at: finished,
            error: error.clone(),
        });

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
