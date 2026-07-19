//! The real client: hand-rolled `reqwest` calls against the Google Tasks REST
//! API. Auth tokens come from `yup-oauth2` via the `auth` module. Kept thin —
//! its one job that a fake can't verify (request-building + JSON) is covered by
//! the `wiremock` suite in `tests/`.

/// Base URL for the Tasks API v1.
#[allow(dead_code)] // used once the REST methods are implemented
pub const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

pub struct RestClient {
    // http: reqwest::Client,
    // auth: std::sync::Arc<dyn crate::auth::TokenProvider>,
}

// impl TasksApi for RestClient { ... }  // maps each method to one HTTP call,
// injecting a fresh bearer token and retrying ONCE on AuthExpired (ADR-0002).
