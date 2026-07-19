//! OAuth: BYO credentials + loopback flow via `yup-oauth2` (ADR-0002).
//! Token persistence is behind `TokenStore` so a keychain backend can replace
//! the plaintext-600 file later without touching call sites.

use crate::api::ApiError;

/// Where the refresh token lives. v1 impl: `chmod 600` file in the config dir.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> anyhow::Result<Option<String>>;
    fn save(&self, token: &str) -> anyhow::Result<()>;
    fn clear(&self) -> anyhow::Result<()>;
}

/// Hands out a fresh bearer token, refreshing as needed.
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    async fn bearer(&self) -> Result<String, ApiError>;
}

/// First-run: open the system browser to Google's consent URL, run the
/// `localhost` loopback listener, exchange the code, persist via `TokenStore`.
pub async fn login(/* client_secret, store */) -> anyhow::Result<()> {
    todo!("yup-oauth2 InstalledFlow, HTTPRedirect (loopback)")
}
