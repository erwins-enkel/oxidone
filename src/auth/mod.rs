//! OAuth: BYO credentials + loopback flow via `yup-oauth2` (ADR-0002).
//! Token persistence is behind `TokenStore` so a keychain backend can replace
//! the plaintext-600 file later without touching call sites.

mod oauth;
mod store;

pub use oauth::{login, YupTokenProvider};
pub use store::FileTokenStore;

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

/// A fixed bearer token. Used by the `wiremock` contract suite to drive
/// `RestClient` without touching real OAuth.
pub struct StaticTokenProvider(pub String);

#[async_trait::async_trait]
impl TokenProvider for StaticTokenProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        Ok(self.0.clone())
    }
}
