//! The real client: hand-rolled `reqwest` calls against the Google Tasks REST
//! API. Auth tokens come from `yup-oauth2` via the `auth` module. Kept thin —
//! its one job that a fake can't verify (request-building + JSON) is covered by
//! the `wiremock` suite in `tests/`.

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use reqwest::header::CONTENT_LENGTH;
use serde::{Deserialize, Serialize};

use super::{ApiError, NewTask, TaskPatch, TasksApi};
use crate::auth::TokenProvider;
use crate::domain::{List, ListId, Status, Task, TaskId, TaskLink};

/// Base URL for the Tasks API v1.
pub const BASE: &str = "https://tasks.googleapis.com/tasks/v1";

/// `tasks.list` page size. Google defaults to 20 and maxes at 100, so asking
/// for 100 is what keeps a normal List to a single round trip; the pages beyond
/// it are followed by [`RestClient::fetch_all_pages`]. `tasklists.list` sends no
/// equivalent — there Google's default *is* its maximum (1000), so naming it
/// would be a no-op parameter.
const TASKS_PAGE_SIZE: &str = "100";

/// Backstop for the pagination loop, above every documented ceiling: Google caps
/// an account at 100,000 Tasks, and `show_hidden=true` can put all of one List's
/// Cleared history in scope, so 1000 full pages is the real worst case — doubled
/// here, because Google may return short pages and a tight bound would reject a
/// legitimate account's data. `tasklists.list` needs 2 pages at its own ceiling
/// of 2000 Lists, so one shared number covers both reads.
const MAX_PAGES: usize = 2000;

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

    /// A POST with no body. reqwest omits `Content-Length` entirely for a
    /// bodyless request, and Google's HTTP/1.1 frontend answers 411 without it —
    /// the request never reaches the Tasks API, so the error body is the
    /// frontend's HTML rather than Google's JSON. Set it explicitly.
    ///
    /// Every bodyless POST must go through here; `self.http.post` alone is wrong
    /// for this API. Each such endpoint asserts the header in the contract suite.
    fn post_no_body(&self, url: String) -> reqwest::RequestBuilder {
        self.http.post(url).header(CONTENT_LENGTH, "0")
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

    /// Follow `nextPageToken` until Google stops handing one out, concatenating
    /// every page's `items` in order.
    ///
    /// `request` is called **once per page** and must rebuild the whole request
    /// from scratch, cursor included. It is deliberately not a builder that gets
    /// a `pageToken` appended: [`reqwest::RequestBuilder::query`] *merges* rather
    /// than replaces, so reusing one builder across pages would send
    /// `pageToken=P2&pageToken=P3` from the third page on. Rebuilding makes that
    /// unrepresentable — and every other filter is re-sent for free, which
    /// Google requires (it does not remember them across a cursor).
    ///
    /// Two guards keep the loop terminating, and both fail closed — an error,
    /// never the pages gathered so far:
    /// - a cursor Google has already handed out is a cycle, of any length;
    /// - [`MAX_PAGES`] bounds any other way a chain could fail to end.
    async fn fetch_all_pages<T>(
        &self,
        request: impl Fn(Option<&str>) -> reqwest::RequestBuilder,
    ) -> Result<Vec<T>, ApiError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let mut out: Vec<T> = Vec::new();
        // Every cursor seen so far, not just the last one: a cycle need not be
        // tight. `P2 → P3 → P2 → …` never repeats *adjacent* cursors, so
        // comparing against the previous one alone would let it run to
        // `MAX_PAGES` — thousands of round trips, duplicating the cycle's items
        // on every lap — before the backstop caught it.
        let mut seen: HashSet<String> = HashSet::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let page: WirePage<T> = self.send_json(request(token.as_deref())).await?;
            out.extend(page.items);
            // An empty string is not a cursor; Google omits the field when done,
            // but treating "" as absent costs nothing and avoids a pointless
            // extra round trip if it ever does send one.
            let Some(next) = page.next_page_token.filter(|t| !t.is_empty()) else {
                return Ok(out);
            };
            // The cursor itself stays out of the message: it is an opaque Google
            // blob of unbounded length, and this error is rendered in the
            // one-line status bar.
            if !seen.insert(next.clone()) {
                return Err(ApiError::Pagination(format!(
                    "google re-issued a page cursor after {} items",
                    out.len()
                )));
            }
            token = Some(next);
        }
        Err(ApiError::Pagination(format!("more than {MAX_PAGES} pages")))
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

/// One page of a Google collection response: the items, plus the cursor to the
/// next page if Google handed one out. `tasklists.list` and `tasks.list` share
/// this envelope exactly, so one generic type serves both.
#[derive(Deserialize)]
struct WirePage<T> {
    // `default = "Vec::new"`, not a bare `default`: the latter makes serde's
    // derive demand `T: Default`, which no wire type has any reason to be.
    #[serde(default = "Vec::new")]
    items: Vec<T>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
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
    #[serde(default)]
    links: Vec<WireLink>,
}

/// One entry of Google's output-only `links[]`. `type` is a Rust keyword, so it
/// deserialises into `kind`; all three fields are optional in practice, so a
/// partial object never fails the whole `list_tasks` page.
#[derive(Deserialize)]
struct WireLink {
    #[serde(default)]
    link: String,
    description: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl WireLink {
    fn into_domain(self) -> TaskLink {
        TaskLink {
            url: self.link,
            description: self.description,
            kind: self.kind,
        }
    }
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
            links: self.links.into_iter().map(WireLink::into_domain).collect(),
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
        let wire: Vec<WireList> = self
            .fetch_all_pages(|token| {
                let req = self.http.get(&url);
                match token {
                    Some(token) => req.query(&[("pageToken", token)]),
                    None => req,
                }
            })
            .await?;
        Ok(wire.into_iter().map(WireList::into_domain).collect())
    }

    async fn default_list(&self) -> Result<List, ApiError> {
        // `@default` is Google's alias for the user's primary List; the response
        // carries the concrete id, which is what we keep (ADR-0003).
        let url = format!("{}/users/@me/lists/@default", self.base);
        let body: WireList = self.send_json(self.http.get(url)).await?;
        Ok(body.into_domain())
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
        // Built once and re-sent verbatim with every page: Google does not
        // remember a request's filters across a `pageToken`, so dropping any of
        // these mid-chain would silently change what the later pages contain.
        let mut query: Vec<(&str, String)> = vec![
            ("showCompleted", show_completed.to_string()),
            ("showHidden", show_hidden.to_string()),
            ("maxResults", TASKS_PAGE_SIZE.to_string()),
        ];
        if let Some(min) = updated_min {
            query.push(("updatedMin", ts_to_wire(min)));
        }
        let wire: Vec<WireTask> = self
            .fetch_all_pages(|token| {
                let req = self.http.get(&url).query(&query);
                match token {
                    Some(token) => req.query(&[("pageToken", token)]),
                    None => req,
                }
            })
            .await?;
        let list = list.clone();
        Ok(wire
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
        let wire: WireTask = self.send_json(self.post_no_body(url).query(&query)).await?;
        Ok(wire.into_domain(list.clone()))
    }

    async fn move_task_to_list(
        &self,
        list: &ListId,
        id: &TaskId,
        destination: &ListId,
    ) -> Result<Task, ApiError> {
        let url = format!("{}/lists/{}/tasks/{}/move", self.base, list.0, id.0);
        let query = [("destinationTasklist", &destination.0)];
        let wire: WireTask = self.send_json(self.post_no_body(url).query(&query)).await?;
        // Stamp the destination, as `move_task` stamps `list` — and clear
        // `parent`, which the response may still echo from the source List. Such
        // an id names a Task that does not exist in `destination`, and
        // `Model::groups` would render the row as a trailing orphan group. The
        // move omits `parent`, so top-level is what was actually asked for.
        let mut task = wire.into_domain(destination.clone());
        task.parent = None;
        Ok(task)
    }

    async fn clear_completed(&self, list: &ListId) -> Result<(), ApiError> {
        let url = format!("{}/lists/{}/tasks/clear", self.base, list.0);
        self.send_empty(self.post_no_body(url)).await
    }
}
