//! OAuth: BYO credentials + loopback consent flow via `yup-oauth2` (ADR-0002),
//! with the refresh exchange oxidone's own (ADR-0009).
//! Token persistence is behind `TokenStore` so a keychain backend can replace
//! the plaintext-600 file later without touching call sites.

mod consent;
mod headless;
mod oauth;
mod refresh;
mod single_flight;
mod store;

pub use consent::{ConsentPrompt, ConsentSink};
pub use headless::RefreshOnlyProvider;
pub use oauth::{login, YupTokenProvider};
pub use refresh::cached_or_refreshed;
pub use single_flight::{SingleFlight, CONSENT_TIMEOUT};
pub use store::FileTokenStore;

use crate::api::ApiError;

/// Where the refresh token lives. v1 impl: `chmod 600` file in the config dir.
pub trait TokenStore: Send + Sync {
    fn load(&self) -> anyhow::Result<Option<String>>;
    fn save(&self, token: &str) -> anyhow::Result<()>;
    fn clear(&self) -> anyhow::Result<()>;

    /// Take the store's exclusive lock, which spans a whole
    /// load → refresh → save (see [`cached_or_refreshed`]) rather than any one
    /// of them.
    ///
    /// `Ok(None)` means somebody else holds it *right now* — the caller retries;
    /// `Err` means the lock itself could not be established, which is a broken
    /// store and never a missing grant.
    ///
    /// It belongs to the store rather than to the refresh exchange because only
    /// the store knows what its contents *are*: a file needs an OS lock, and a
    /// keychain backend — the reason this trait exists at all (ADR-0002) — would
    /// hand back a guard that locks nothing. `refresh` stays storage-agnostic.
    ///
    /// Exclusion is across **processes**, which is the point: `SingleFlight`
    /// coalesces within one, and since ADR-0010 a `oxidone json` call and the TUI
    /// can both want the same grant.
    fn try_lock(&self) -> anyhow::Result<Option<Box<dyn TokenGuard>>>;
}

/// A held [`TokenStore::try_lock`]. Releasing is dropping it; there is nothing
/// to call, so a lock cannot be leaked past its scope by forgetting to.
pub trait TokenGuard: Send {}

/// Hands out a fresh bearer token, refreshing as needed.
#[async_trait::async_trait]
pub trait TokenProvider: Send + Sync {
    async fn bearer(&self) -> Result<String, ApiError>;

    /// Force a token refresh and return the new bearer, used to retry once after
    /// a 401 (the cached token may look valid to the provider but be rejected by
    /// the server). Defaults to `bearer` for providers with no refresh concept.
    async fn refresh(&self) -> Result<String, ApiError> {
        self.bearer().await
    }
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
