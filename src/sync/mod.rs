//! Reconciliation between the cache and Google.
//!
//! v1 (ADR-0001): the cache is the source of truth for reads. A refresh pulls
//! from Google via `TasksApi`, mirrors the result into the cache, and returns
//! the cached view. Write-through and the offline queue land in later slices.

use anyhow::Result;

use crate::api::TasksApi;
use crate::cache::Cache;
use crate::domain::List;

/// Refresh Lists: fetch from Google, mirror into the cache (dropping Lists that
/// no longer exist), and return the cached Lists for the Model.
pub async fn load_lists(api: &dyn TasksApi, cache: &Cache) -> Result<Vec<List>> {
    let lists = api.list_lists().await?;
    cache.replace_lists(&lists)?;
    cache.lists()
}
