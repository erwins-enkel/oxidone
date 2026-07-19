//! Reconciliation between the cache and Google.
//!
//! v1 (ADR-0001): **write-through** — a write hits Google, then patches the
//! cache from the response. **Refresh** is manual (`r` / startup): pull each
//! list and diff into the mirror; append newly-seen completions to the
//! completion_log (ADR-0007). No background poll, no offline queue yet.
//!
//! The seam for the future is here: swap "rollback on write failure" for
//! "keep dirty + retry", and add a `updated_min` background poll.

pub struct Sync {
    // api: Arc<dyn TasksApi>,
    // cache: Cache,
}

// impl Sync {
//   async fn refresh(&self, list: &ListId) -> Result<Vec<Task>, ApiError>
//   async fn write_through(&self, cmd: Command) -> Result<(), ApiError>
// }
