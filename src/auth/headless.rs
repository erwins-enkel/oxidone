//! A [`TokenProvider`] for callers with no terminal and no user: the JSON CLI
//! (ADR-0010).
//!
//! It answers from the stored grant, refreshing it against Google when the cached
//! access token is spent, and stops there. Where [`super::YupTokenProvider`]
//! answers [`ApiError::AuthExpired`] by opening a browser, this returns it.
//!
//! That difference is the whole type. `oxidone json` is what a bar plugin runs
//! every few minutes with nobody watching: a consent flow there would open a
//! browser window at a machine that may be locked, bind a loopback port, and sit
//! on it for [`super::CONSENT_TIMEOUT`] waiting for a redirect that cannot
//! arrive — once per poll. Making consent *unreachable* rather than merely
//! unused is why this builds no `InstalledFlowAuthenticator` at all: there is no
//! browser path to take, not a browser path guarded by a flag.
//!
//! Authorizing stays the TUI's job, which is where a user already is.

use std::path::Path;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use yup_oauth2::{read_application_secret, ApplicationSecret};

use super::{refresh, TokenProvider, TokenStore};
use crate::api::ApiError;

/// Hands out bearers from an existing grant, and never acquires a new one.
pub struct RefreshOnlyProvider {
    http: reqwest::Client,
    secret: ApplicationSecret,
    store: Arc<dyn TokenStore>,
}

impl RefreshOnlyProvider {
    /// Build a provider from the BYO `client_secret.json`, reading and writing
    /// the grant through `store`.
    ///
    /// Fails only on the secret itself — a missing or malformed
    /// `client_secret.json`. Whether a *grant* exists is not asked here: that is
    /// [`TokenProvider::bearer`]'s answer, and it is the one the caller has to
    /// report anyway.
    pub async fn new(
        client_secret_path: &Path,
        store: Arc<dyn TokenStore>,
    ) -> anyhow::Result<Self> {
        let secret = read_application_secret(client_secret_path)
            .await
            .with_context(|| {
                format!("reading BYO client secret {}", client_secret_path.display())
            })?;
        Ok(Self {
            http: reqwest::Client::new(),
            secret,
            store,
        })
    }
}

#[async_trait]
impl TokenProvider for RefreshOnlyProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        refresh::cached_or_refreshed(&self.http, &self.secret, &*self.store, false).await
    }

    /// The 401 replay: a token the cache still believes in can already have been
    /// rejected by the server.
    ///
    /// Every error passes through as it is, [`ApiError::AuthExpired`] included —
    /// which here means "there is no grant left to refresh", the one thing a
    /// headless caller cannot fix and must therefore report.
    async fn refresh(&self) -> Result<String, ApiError> {
        refresh::cached_or_refreshed(&self.http, &self.secret, &*self.store, true).await
    }
}
