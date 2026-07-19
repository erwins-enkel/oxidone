//! The real client: hand-rolled `reqwest` calls against the Google Tasks REST
//! API. Auth tokens come from `yup-oauth2` via the `auth` module. Kept thin —
//! its one job that a fake can't verify (request-building + JSON) is covered by
//! the `wiremock` suite in `tests/`.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::{ApiError, NewTask, TaskPatch, TasksApi};
use crate::auth::TokenProvider;
use crate::domain::{List, ListId, Status, Task, TaskId};

/// Base URL for the Tasks API v1.
pub const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

/// A single `list_tasks` page cap. Google defaults to 20 and maxes at 100; we
/// ask for 100 to keep each trait method to a single HTTP call (pagination is a
/// deliberate non-goal here — see the ticket). A List with >100 live Tasks is
/// out of scope for v1.
const MAX_RESULTS: &str = "100";

/// Hand-rolled Google Tasks client. Two seams keep it testable without a live
/// Google account (ADR-0004):
/// - `base` is configurable, so `wiremock` can point it at a local mock server;
/// - the bearer token comes from a `TokenProvider`, so tests inject a static
///   token and never touch real OAuth.
pub struct RestClient {
    http: reqwest::Client,
    base: String,
    auth: Arc<dyn TokenProvider>,
}

impl RestClient {
    /// Production constructor: talks to the real Google endpoint.
    pub fn new(auth: Arc<dyn TokenProvider>) -> Self {
        Self::with_base(BASE, auth)
    }

    /// Test/seam constructor: point `base` at a mock server (no trailing slash).
    pub fn with_base(base: impl Into<String>, auth: Arc<dyn TokenProvider>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: base.into(),
            auth,
        }
    }

    /// Inject a fresh bearer token, send, and map transport failures to
    /// `ApiError::Network`. On a 401 the token is force-refreshed and the request
    /// retried once (ADR-0002) before the status is surfaced by [`check_status`].
    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
        // Clone up front so the request can be replayed on a 401. Our bodies are
        // in-memory JSON, so `try_clone` always succeeds; if it ever doesn't, we
        // simply skip the retry.
        let retry = req.try_clone();
        let bearer = self.auth.bearer().await?;
        let resp = req
            .bearer_auth(bearer)
            .send()
            .await
            .map_err(|e| ApiError::Network(e.to_string()))?;

        if resp.status().as_u16() == 401 {
            if let Some(retry) = retry {
                let bearer = self.auth.refresh().await?;
                let resp = retry
                    .bearer_auth(bearer)
                    .send()
                    .await
                    .map_err(|e| ApiError::Network(e.to_string()))?;
                return check_status(resp).await;
            }
        }
        check_status(resp).await
    }

    /// Send and decode a JSON body into `T`.
    async fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<T, ApiError> {
        let resp = self.send(req).await?;
        resp.json::<T>()
            .await
            .map_err(|e| ApiError::Network(format!("decoding response: {e}")))
    }

    /// Send and discard the body (for `delete`/`clear`, which return no content
    /// we use).
    async fn send_empty(&self, req: reqwest::RequestBuilder) -> Result<(), ApiError> {
        self.send(req).await?;
        Ok(())
    }
}

/// Map an unsuccessful HTTP response to the right `ApiError`. `yup-oauth2`
/// refreshes tokens proactively at the token layer, so a 401 here means a
/// revoked/expired grant that a bare retry wouldn't fix — it surfaces as
/// `AuthExpired` so the consuming slice can prompt re-authentication, distinct
/// from a hard rejection.
async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, ApiError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    match code {
        401 => Err(ApiError::AuthExpired),
        404 => Err(ApiError::NotFound),
        _ => {
            let body = resp.text().await.unwrap_or_default();
            Err(ApiError::Rejected {
                status: code,
                message: google_error_message(&body),
            })
        }
    }
}

/// Google error bodies look like `{"error":{"code":404,"message":"…"}}`. Pull
/// out the human message, falling back to the raw body.
fn google_error_message(body: &str) -> String {
    #[derive(Deserialize)]
    struct Wrapper {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        message: String,
    }
    serde_json::from_str::<Wrapper>(body)
        .map(|w| w.error.message)
        .unwrap_or_else(|_| body.to_string())
}

/// A due date is date-only: Google keeps only the date portion of the RFC3339
/// timestamp, so we send midnight UTC.
fn due_to_wire(date: NaiveDate) -> String {
    date.and_hms_opt(0, 0, 0)
        .expect("midnight is always valid")
        .and_utc()
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn ts_to_wire(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---- Wire types (Google's JSON), kept private to this module ----

#[derive(Deserialize)]
struct WireLists {
    #[serde(default)]
    items: Vec<WireList>,
}

#[derive(Deserialize)]
struct WireList {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    etag: String,
    updated: Option<String>,
}

impl WireList {
    fn into_domain(self) -> List {
        List {
            id: ListId(self.id),
            title: self.title,
            etag: self.etag,
            updated: updated_or_now(self.updated.as_deref()),
        }
    }
}

#[derive(Deserialize)]
struct WireTasks {
    #[serde(default)]
    items: Vec<WireTask>,
}

#[derive(Deserialize)]
struct WireTask {
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    etag: String,
    updated: Option<String>,
    parent: Option<String>,
    notes: Option<String>,
    status: Option<String>,
    due: Option<String>,
    completed: Option<String>,
    #[serde(default)]
    position: String,
}

impl WireTask {
    /// The list id is not in Google's Task JSON, so the caller supplies it.
    fn into_domain(self, list: ListId) -> Task {
        let status = match self.status.as_deref() {
            Some("completed") => Status::Completed,
            _ => Status::NeedsAction,
        };
        Task {
            id: TaskId(self.id),
            list,
            parent: self.parent.map(TaskId),
            title: self.title,
            notes: self.notes,
            status,
            due: self.due.as_deref().and_then(parse_date),
            completed_at: parse_ts(self.completed.as_deref()),
            position: self.position,
            etag: self.etag,
            updated: updated_or_now(self.updated.as_deref()),
        }
    }
}

fn parse_ts(s: Option<&str>) -> Option<DateTime<Utc>> {
    let s = s?;
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// `updated` drives incremental Refresh, so a missing/unparseable value is
/// substituted with `now` — but logged, since silently inventing a timestamp
/// would quietly corrupt an `updatedMin` sync window.
fn updated_or_now(raw: Option<&str>) -> DateTime<Utc> {
    parse_ts(raw).unwrap_or_else(|| {
        tracing::warn!(value = ?raw, "missing/unparseable `updated`; substituting current time");
        Utc::now()
    })
}

fn parse_date(s: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.date_naive())
}

// ---- Request bodies ----

#[derive(Serialize)]
struct TitleBody<'a> {
    title: &'a str,
}

#[derive(Serialize)]
struct NewTaskBody {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due: Option<String>,
}

/// A patch body. `None` fields are omitted (left untouched); an explicit `null`
/// (via `Some(None)` on the domain patch) clears the field, which Google honors
/// on a PATCH.
#[derive(Serialize, Default)]
struct TaskPatchBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    // Cleared alongside `status` when un-completing.
    #[serde(skip_serializing_if = "Option::is_none")]
    completed: Option<Option<String>>,
}

#[async_trait]
impl TasksApi for RestClient {
    async fn list_lists(&self) -> Result<Vec<List>, ApiError> {
        let url = format!("{}/users/@me/lists", self.base);
        let body: WireLists = self.send_json(self.http.get(url)).await?;
        Ok(body.items.into_iter().map(WireList::into_domain).collect())
    }

    async fn insert_list(&self, title: &str) -> Result<List, ApiError> {
        let url = format!("{}/users/@me/lists", self.base);
        let body: WireList = self
            .send_json(self.http.post(url).json(&TitleBody { title }))
            .await?;
        Ok(body.into_domain())
    }

    async fn patch_list(&self, id: &ListId, title: &str) -> Result<List, ApiError> {
        let url = format!("{}/users/@me/lists/{}", self.base, id.0);
        let body: WireList = self
            .send_json(self.http.patch(url).json(&TitleBody { title }))
            .await?;
        Ok(body.into_domain())
    }

    async fn delete_list(&self, id: &ListId) -> Result<(), ApiError> {
        let url = format!("{}/users/@me/lists/{}", self.base, id.0);
        self.send_empty(self.http.delete(url)).await
    }

    async fn list_tasks(
        &self,
        list: &ListId,
        show_completed: bool,
        show_hidden: bool,
        updated_min: Option<DateTime<Utc>>,
    ) -> Result<Vec<Task>, ApiError> {
        let url = format!("{}/lists/{}/tasks", self.base, list.0);
        let mut query: Vec<(&str, String)> = vec![
            ("showCompleted", show_completed.to_string()),
            ("showHidden", show_hidden.to_string()),
            ("maxResults", MAX_RESULTS.to_string()),
        ];
        if let Some(min) = updated_min {
            query.push(("updatedMin", ts_to_wire(min)));
        }
        let body: WireTasks = self.send_json(self.http.get(url).query(&query)).await?;
        let list = list.clone();
        Ok(body
            .items
            .into_iter()
            .map(|t| t.into_domain(list.clone()))
            .collect())
    }

    async fn insert_task(&self, list: &ListId, task: NewTask) -> Result<Task, ApiError> {
        let url = format!("{}/lists/{}/tasks", self.base, list.0);
        let body = NewTaskBody {
            title: task.title,
            notes: task.notes,
            due: task.due.map(due_to_wire),
        };
        // A Subtask is created by naming its parent as a query param.
        let mut req = self.http.post(url).json(&body);
        if let Some(parent) = &task.parent {
            req = req.query(&[("parent", &parent.0)]);
        }
        let wire: WireTask = self.send_json(req).await?;
        Ok(wire.into_domain(list.clone()))
    }

    async fn patch_task(
        &self,
        list: &ListId,
        id: &TaskId,
        patch: TaskPatch,
    ) -> Result<Task, ApiError> {
        let url = format!("{}/lists/{}/tasks/{}", self.base, list.0, id.0);
        let mut body = TaskPatchBody {
            title: patch.title,
            notes: patch.notes,
            due: patch.due.map(|d| d.map(due_to_wire)),
            ..Default::default()
        };
        if let Some(completed) = patch.completed {
            if completed {
                body.status = Some("completed");
            } else {
                body.status = Some("needsAction");
                // Explicitly clear the completion timestamp when reopening.
                body.completed = Some(None);
            }
        }
        let wire: WireTask = self.send_json(self.http.patch(url).json(&body)).await?;
        Ok(wire.into_domain(list.clone()))
    }

    async fn delete_task(&self, list: &ListId, id: &TaskId) -> Result<(), ApiError> {
        let url = format!("{}/lists/{}/tasks/{}", self.base, list.0, id.0);
        self.send_empty(self.http.delete(url)).await
    }

    async fn move_task(
        &self,
        list: &ListId,
        id: &TaskId,
        parent: Option<&TaskId>,
        previous: Option<&TaskId>,
    ) -> Result<Task, ApiError> {
        let url = format!("{}/lists/{}/tasks/{}/move", self.base, list.0, id.0);
        let mut query: Vec<(&str, &str)> = Vec::new();
        if let Some(p) = parent {
            query.push(("parent", &p.0));
        }
        if let Some(prev) = previous {
            query.push(("previous", &prev.0));
        }
        let wire: WireTask = self.send_json(self.http.post(url).query(&query)).await?;
        Ok(wire.into_domain(list.clone()))
    }

    async fn clear_completed(&self, list: &ListId) -> Result<(), ApiError> {
        let url = format!("{}/lists/{}/tasks/clear", self.base, list.0);
        self.send_empty(self.http.post(url)).await
    }
}
