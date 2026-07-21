//! The real client: hand-rolled `reqwest` calls against the Google Tasks REST
//! API. Auth tokens come from `yup-oauth2` via the `auth` module. Kept thin —
//! its one job that a fake can't verify (request-building + JSON) is covered by
//! the `wiremock` suite in `tests/`.
//!
//! Retry/backoff is the one piece of policy that does live here.
//! [`RestClient::send`] is the single seam every `TasksApi` method reaches the
//! network through, so rate limits and transient read failures are handled
//! there once instead of separately in every method. Everything the retry loop
//! decides is a pure function ([`retry_delay`], [`jitter`], [`is_retriable`],
//! [`is_rate_limit`], [`is_quota_exhausted`]), so the rules are unit-testable
//! without a socket.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use reqwest::header::{HeaderValue, AUTHORIZATION, CONTENT_LENGTH, RETRY_AFTER};
use reqwest::{Method, StatusCode};
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

/// Short-window limits: seconds, not days, so backing off is exactly the remedy
/// and these are the only 403 reasons [`is_retriable`] replays.
///
/// Google Workspace APIs signal a limit with *either* a `429` or a `403`
/// carrying a reason in `error.errors[].reason`, and the Tasks docs specify
/// neither — so keying only off the status would leave the retry path dormant
/// against a real account.
const SHORT_TERM_RATE_LIMIT_REASONS: [&str; 2] = ["rateLimitExceeded", "userRateLimitExceeded"];

/// The other kind of limit — Google's daily cap. A handful of retries seconds
/// apart cannot clear a per-day quota, and each attempt spends more of it, so
/// these are never replayed and surface as [`ApiError::QuotaExhausted`]:
/// "try again shortly" would be wrong advice, and Google's own message ("Quota
/// Exceeded") says more than we could.
///
/// Kept as a set disjoint from [`SHORT_TERM_RATE_LIMIT_REASONS`] rather than as
/// a subset of one combined list, so the retriable and non-retriable halves
/// cannot drift into overlapping.
const QUOTA_REASONS: [&str; 2] = ["quotaExceeded", "dailyLimitExceeded"];

/// How [`RestClient::send`] backs off when Google says "not now".
///
/// Production values are [`RetryPolicy::default`]; the contract tests swap in a
/// near-zero ladder via [`RestClient::with_retry`] so the gate does not spend
/// real seconds asleep.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    /// Retries *after* the first attempt — a request is sent at most
    /// `max_retries + 1` times.
    pub max_retries: u32,
    /// First rung of the ladder; each rung doubles the last.
    pub base_delay: Duration,
    /// The longest `Retry-After` worth honouring. Anything above it is not
    /// retried at all — see [`retry_delay`].
    pub retry_after_ceiling: Duration,
    /// Total time one request may spend asleep across all of its retries. This,
    /// not the per-delay ceiling, is what bounds a single request: against the
    /// 60s default, three consecutive `Retry-After: 30`s sleep twice (30s + 30s
    /// exactly fills the budget) and the third is refused — 60s of waiting, not
    /// the 90s an unbounded ladder would spend. A delay is taken only if it fits
    /// whole; the budget is never overshot.
    pub sleep_budget: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_millis(250),
            retry_after_ceiling: Duration::from_secs(60),
            sleep_budget: Duration::from_secs(60),
        }
    }
}

/// Hand-rolled Google Tasks client. Two seams keep it testable without a live
/// Google account (ADR-0004):
/// - `base` is configurable, so `wiremock` can point it at a local mock server;
/// - the bearer token comes from a `TokenProvider`, so tests inject a static
///   token and never touch real OAuth.
///
/// A third, [`RestClient::with_retry`], exists for the same reason: it lets the
/// contract tests exercise the backoff rules in milliseconds.
pub struct RestClient {
    http: reqwest::Client,
    base: String,
    auth: Arc<dyn TokenProvider>,
    retry: RetryPolicy,
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
            retry: RetryPolicy::default(),
        }
    }

    /// Test seam: shrink the backoff so a retry test costs milliseconds, not
    /// seconds. Production always uses [`RetryPolicy::default`].
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
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
    /// `ApiError::Network`. The single seam every `TasksApi` method reaches the
    /// network through, and so the one place two kinds of replay live:
    ///
    /// **Auth.** On a 401 the token is force-refreshed and the request replayed.
    /// That refresh is available *once per call*, tracked separately from the
    /// retry budget: a 401 consumes no retry, and a spent retry budget does not
    /// disable it — a 429 on attempt 1 followed by a 401 on attempt 2 still gets
    /// its refresh. Once spent, a second 401 is `AuthExpired` immediately; the
    /// token was just refreshed, so a bare replay cannot help.
    ///
    /// **Backoff.** [`is_retriable`] decides what is worth replaying: any rate
    /// limit Google says is short-window (`429`, or `403` with a
    /// [`SHORT_TERM_RATE_LIMIT_REASONS`] reason), and 5xx on `GET` only — a 5xx
    /// on a write may mean the write landed and only the response was lost, and
    /// replaying `insert_task` there would duplicate a Task. Timing comes from
    /// [`retry_delay`]: Google's `Retry-After` when it sent a parseable one,
    /// otherwise a jittered exponential ladder, all bounded by
    /// [`RetryPolicy::sleep_budget`].
    ///
    /// Transport failures (reset, timeout, DNS) are *not* retried: they surface
    /// before any status exists, and whether a timed-out write reached Google is
    /// unknowable — the same hazard that keeps 5xx off writes.
    ///
    /// The response body is read at most once, and only on failure: a success
    /// returns the `Response` untouched so [`RestClient::send_json`] can still
    /// decode it, while a failure hands its body to [`parse_google_error`] —
    /// whose `reasons` the retry decision needs and whose `message` the terminal
    /// [`status_to_error`] needs.
    async fn send(&self, req: reqwest::RequestBuilder) -> Result<reqwest::Response, ApiError> {
        // Built once, cloned per attempt. Beyond making the replay explicit this
        // exposes the HTTP method, which `is_retriable` needs to keep 5xx retries
        // off writes.
        let req = req
            .build()
            .map_err(|e| ApiError::Network(format!("building request: {e}")))?;
        let mut bearer = self.auth.bearer().await?;
        let mut refreshed = false;
        let mut attempt: u32 = 0;
        let mut slept = Duration::ZERO;

        loop {
            let Some(mut next) = req.try_clone() else {
                // Our bodies are in-memory JSON, so this is unreachable in
                // practice. Fail closed anyway: returning anything else here
                // would be inventing a result for a request never sent.
                return Err(ApiError::Network("request body is not replayable".into()));
            };
            let value = HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|e| ApiError::Network(format!("invalid bearer token: {e}")))?;
            next.headers_mut().insert(AUTHORIZATION, value);

            let resp = self
                .http
                .execute(next)
                .await
                .map_err(|e| ApiError::Network(e.to_string()))?;

            let status = resp.status();
            if status.is_success() {
                return Ok(resp);
            }

            if status == StatusCode::UNAUTHORIZED && !refreshed {
                refreshed = true;
                bearer = self.auth.refresh().await?;
                continue;
            }

            // Read the header before the body: `text()` consumes the response.
            let retry_after = resp.headers().get(RETRY_AFTER).and_then(parse_retry_after);
            let code = status.as_u16();
            let body = resp.text().await.unwrap_or_default();
            let error = parse_google_error(&body);

            if is_retriable(code, &error.reasons, req.method()) {
                attempt += 1;
                if let Some(delay) = retry_delay(attempt, retry_after, &self.retry) {
                    // Jitter the ladder but never a `Retry-After`: that is an
                    // instruction, not an estimate. The ladder needs it because
                    // the Today fan-out retries N Lists at once and would
                    // otherwise have them collide again in lockstep.
                    let delay = match retry_after {
                        Some(_) => delay,
                        None => jitter(delay, clock_nanos()),
                    };
                    if slept + delay <= self.retry.sleep_budget {
                        slept += delay;
                        tracing::debug!(
                            status = code,
                            attempt,
                            delay_ms = delay.as_millis(),
                            "retrying after a transient google failure"
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }
                }
            }
            return Err(status_to_error(code, error));
        }
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
    ///
    /// Each page is a separate [`RestClient::send`], so each carries its **own**
    /// retry budget: a rate limit on page 7 is backed off without spending page
    /// 1's. The flip side is that a long chain has no shared ceiling on how long
    /// it may spend retrying, and that exhausting the budget mid-chain still
    /// discards the pages already fetched — see #98.
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

/// Map a failed status (plus its already-read body) to the right `ApiError`.
///
/// Takes values rather than the `Response` because [`RestClient::send`] has
/// already consumed the body to classify it — the body can only be read once,
/// and the retry decision needs it first.
///
/// `yup-oauth2` refreshes tokens proactively at the token layer, so a 401 that
/// survives `send`'s forced refresh means a revoked/expired grant that no retry
/// would fix — it surfaces as `AuthExpired` so the consuming slice can prompt
/// re-authentication, distinct from a hard rejection.
///
/// The two limit cases are kept apart because the advice differs: a short-term
/// limit that reaches here has already been backed off, so `RateLimited` says
/// "try again shortly"; a daily quota was never worth backing off, so
/// `QuotaExhausted` says "later" and keeps Google's own wording. A bare `429`
/// counts as short-term — Google does not say which kind it is, and the
/// recoverable reading is the one that does not tell the user to give up for the
/// day.
fn status_to_error(status: u16, error: GoogleError) -> ApiError {
    match status {
        401 => ApiError::AuthExpired,
        404 => ApiError::NotFound,
        _ if is_quota_exhausted(status, &error.reasons) => ApiError::QuotaExhausted {
            message: error.message,
        },
        _ if is_rate_limit(status, &error.reasons) => ApiError::RateLimited,
        _ => ApiError::Rejected {
            status,
            message: error.message,
        },
    }
}

/// Is this Google telling us we are briefly over a limit? Either the status says
/// so, or a `403` carries one of [`SHORT_TERM_RATE_LIMIT_REASONS`] — Google
/// Workspace APIs use both forms, and the Tasks docs specify neither.
fn is_rate_limit(status: u16, reasons: &[String]) -> bool {
    status == 429
        || (status == 403
            && reasons
                .iter()
                .any(|r| SHORT_TERM_RATE_LIMIT_REASONS.contains(&r.as_str())))
}

/// Is this the daily cap rather than a momentary limit? Only ever a `403`: a
/// bare `429` carries no reason to tell us, and guessing "daily" from it would
/// turn a few seconds' wait into advice to stop for the day.
fn is_quota_exhausted(status: u16, reasons: &[String]) -> bool {
    status == 403 && reasons.iter().any(|r| QUOTA_REASONS.contains(&r.as_str()))
}

/// Is replaying this failure worth it?
///
/// Narrower than [`is_rate_limit`] plus [`is_quota_exhausted`] on two axes: a
/// daily quota is a limit no backoff clears, and a 5xx is only safe to replay
/// when the request could not have changed anything — which is why the method
/// matters.
fn is_retriable(status: u16, reasons: &[String], method: &Method) -> bool {
    match status {
        429 => true,
        403 => reasons
            .iter()
            .any(|r| SHORT_TERM_RATE_LIMIT_REASONS.contains(&r.as_str())),
        500 | 502 | 503 | 504 => method == Method::GET,
        _ => false,
    }
}

/// How long to wait before retry number `attempt` (1-based), or `None` when this
/// request should not be retried again.
///
/// Deterministic — jitter is applied by the caller — so the ladder, the ceiling
/// and the `Retry-After` override can all be asserted at exact values.
///
/// A `Retry-After` is an instruction, so it wins over the ladder outright, with
/// one exception: above [`RetryPolicy::retry_after_ceiling`] the honest answer
/// is to stop. Sleeping minutes helps nobody, and retrying earlier than Google
/// asked would only spend quota against a limit we were just told is in force.
fn retry_delay(
    attempt: u32,
    retry_after: Option<Duration>,
    policy: &RetryPolicy,
) -> Option<Duration> {
    if attempt == 0 || attempt > policy.max_retries {
        return None;
    }
    match retry_after {
        Some(after) if after > policy.retry_after_ceiling => None,
        Some(after) => Some(after),
        // 250ms, 500ms, 1s, … The shift is clamped so an outlandish
        // `max_retries` overflows into a long wait the sleep budget will refuse,
        // rather than panicking.
        None => Some(policy.base_delay * (1u32 << attempt.saturating_sub(1).min(16))),
    }
}

/// Spread `base` over ±25%, using `nanos` as the entropy.
///
/// The entropy is a parameter rather than read in here so the function stays
/// pure and assertable at exact values; `send` passes the wall clock's
/// sub-second part. That is also why there is no `rand` dependency — this is one
/// modulo, and a crate that `cargo machete` has to be told about is a poor trade.
fn jitter(base: Duration, nanos: u32) -> Duration {
    // The full span is half of `base`, centred by starting a quarter below it.
    let span = (base / 2).as_nanos() as u64;
    if span == 0 {
        return base;
    }
    base - base / 4 + Duration::from_nanos(u64::from(nanos) % span)
}

/// Entropy for [`jitter`]: the sub-second part of the wall clock. A clock before
/// the epoch yields 0 — jitter degrades to the ladder's lower bound, which is
/// harmless.
fn clock_nanos() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0)
}

/// `Retry-After` in its delta-seconds form.
///
/// RFC 9110 also permits an HTTP-date, which Google does not use here; reading
/// one as seconds would be far worse than ignoring it, so anything that is not a
/// plain integer falls back to the ladder.
fn parse_retry_after(value: &HeaderValue) -> Option<Duration> {
    let secs: u64 = value.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs))
}

/// Google's error envelope, parsed once into the two halves that get used:
/// `reasons` decides whether to retry, `message` renders the terminal error.
/// The body can only be read from the response once, so one parse serves both.
#[derive(Debug, Default, PartialEq, Eq)]
struct GoogleError {
    message: String,
    reasons: Vec<String>,
}

/// Parse `{"error":{"code":403,"message":"…","errors":[{"reason":"…"}]}}`.
///
/// A body that is not that shape — Google's HTML frontend, say — keeps the raw
/// body as the message and reports *no* reasons. That is what makes an
/// unclassifiable failure fail closed: no reasons means no rate limit, so it is
/// never mistaken for something worth replaying.
fn parse_google_error(body: &str) -> GoogleError {
    #[derive(Deserialize)]
    struct Wrapper {
        error: Inner,
    }
    #[derive(Deserialize)]
    struct Inner {
        message: String,
        #[serde(default)]
        errors: Vec<Detail>,
    }
    #[derive(Deserialize)]
    struct Detail {
        #[serde(default)]
        reason: String,
    }
    match serde_json::from_str::<Wrapper>(body) {
        Ok(w) => GoogleError {
            message: w.error.message,
            reasons: w
                .error
                .errors
                .into_iter()
                .map(|d| d.reason)
                .filter(|r| !r.is_empty())
                .collect(),
        },
        Err(_) => GoogleError {
            message: body.to_string(),
            reasons: Vec::new(),
        },
    }
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

/// Every decision `send`'s retry loop makes is a pure function, so the rules are
/// pinned here at exact values. The wiremock suite in `tests/rest_contract.rs`
/// then only has to prove the wiring — that the header is read, the loop
/// replays — rather than re-enumerating this matrix over a socket.
#[cfg(test)]
mod tests {
    use super::*;

    fn reasons(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    // ---- parse_google_error ----

    #[test]
    fn parses_message_and_reasons() {
        let body = r#"{"error":{"code":403,"message":"Rate Limit Exceeded","errors":[
            {"domain":"usageLimits","reason":"rateLimitExceeded"}]}}"#;
        assert_eq!(
            parse_google_error(body),
            GoogleError {
                message: "Rate Limit Exceeded".into(),
                reasons: reasons(&["rateLimitExceeded"]),
            }
        );
    }

    #[test]
    fn parses_a_body_with_no_errors_array() {
        let body = r#"{"error":{"code":404,"message":"Not Found"}}"#;
        assert_eq!(
            parse_google_error(body),
            GoogleError {
                message: "Not Found".into(),
                reasons: Vec::new(),
            }
        );
    }

    /// An unparseable body keeps the raw text as the message and yields no
    /// reasons — which is what makes an unclassifiable 403 fail closed.
    #[test]
    fn an_unparseable_body_becomes_the_message_with_no_reasons() {
        let body = "<html>411 Length Required</html>";
        assert_eq!(
            parse_google_error(body),
            GoogleError {
                message: body.into(),
                reasons: Vec::new(),
            }
        );
    }

    /// A reason-less entry must not become an empty-string reason: it would
    /// match nothing, but it would make `reasons` look populated.
    #[test]
    fn empty_reasons_are_dropped() {
        let body = r#"{"error":{"code":400,"message":"Bad","errors":[{"domain":"global"}]}}"#;
        assert!(parse_google_error(body).reasons.is_empty());
    }

    // ---- is_rate_limit / is_retriable: the classification matrix ----

    #[test]
    fn a_429_is_a_rate_limit_and_retriable_on_any_method() {
        for method in [Method::GET, Method::POST, Method::PATCH, Method::DELETE] {
            assert!(is_rate_limit(429, &[]));
            assert!(is_retriable(429, &[], &method), "429 on {method}");
        }
    }

    #[test]
    fn a_403_with_a_short_window_reason_is_retriable() {
        for reason in SHORT_TERM_RATE_LIMIT_REASONS {
            let reasons = reasons(&[reason]);
            assert!(is_rate_limit(403, &reasons), "{reason}");
            assert!(is_retriable(403, &reasons, &Method::POST), "{reason}");
        }
    }

    /// A daily quota is its own thing: not retriable (no ladder short enough to
    /// matter clears a per-day cap) and *not* a short-term rate limit either, so
    /// the user is told "later" rather than "shortly".
    #[test]
    fn a_403_with_a_daily_quota_reason_is_quota_exhausted_not_a_rate_limit() {
        for reason in QUOTA_REASONS {
            let reasons = reasons(&[reason]);
            assert!(is_quota_exhausted(403, &reasons), "{reason}");
            assert!(!is_rate_limit(403, &reasons), "{reason}");
            assert!(!is_retriable(403, &reasons, &Method::GET), "{reason}");
        }
    }

    /// Google sends no reason with a bare 429, so we cannot know it is the daily
    /// cap — and must not guess, or a few seconds' wait becomes "give up for the
    /// day".
    #[test]
    fn a_429_is_never_read_as_a_daily_quota() {
        assert!(!is_quota_exhausted(429, &[]));
        assert!(!is_quota_exhausted(429, &reasons(&["quotaExceeded"])));
    }

    /// The two reason sets must stay disjoint: an overlap would make a limit both
    /// retriable and terminal, and which one won would depend on match order.
    #[test]
    fn the_reason_sets_do_not_overlap() {
        for reason in SHORT_TERM_RATE_LIMIT_REASONS {
            assert!(!QUOTA_REASONS.contains(&reason), "{reason} is in both sets");
        }
    }

    #[test]
    fn a_permission_403_is_neither() {
        let reasons = reasons(&["insufficientPermissions"]);
        assert!(!is_rate_limit(403, &reasons));
        assert!(!is_quota_exhausted(403, &reasons));
        assert!(!is_retriable(403, &reasons, &Method::GET));
    }

    /// The body that failed to parse: no reasons, so a 403 stays terminal.
    #[test]
    fn a_403_with_no_reasons_is_neither() {
        assert!(!is_rate_limit(403, &[]));
        assert!(!is_retriable(403, &[], &Method::GET));
    }

    #[test]
    fn a_5xx_retries_on_reads_only() {
        for status in [500, 502, 503, 504] {
            assert!(is_retriable(status, &[], &Method::GET), "{status} GET");
            for method in [Method::POST, Method::PATCH, Method::DELETE] {
                assert!(
                    !is_retriable(status, &[], &method),
                    "{status} must not replay a {method}"
                );
            }
            // Transient, but not a rate limit — it stays a `Rejected`.
            assert!(!is_rate_limit(status, &[]));
        }
    }

    #[test]
    fn a_501_is_not_retriable() {
        // Not in the transient set: "not implemented" will not fix itself.
        assert!(!is_retriable(501, &[], &Method::GET));
    }

    // ---- status_to_error ----

    #[test]
    fn status_to_error_maps_each_class() {
        let empty = GoogleError::default;
        assert_eq!(status_to_error(401, empty()), ApiError::AuthExpired);
        assert_eq!(status_to_error(404, empty()), ApiError::NotFound);
        assert_eq!(status_to_error(429, empty()), ApiError::RateLimited);
        assert_eq!(
            status_to_error(
                403,
                GoogleError {
                    message: "Rate Limit Exceeded".into(),
                    reasons: reasons(&["rateLimitExceeded"]),
                }
            ),
            ApiError::RateLimited
        );
        // The daily cap keeps Google's own wording instead of "try shortly".
        assert_eq!(
            status_to_error(
                403,
                GoogleError {
                    message: "Quota Exceeded".into(),
                    reasons: reasons(&["quotaExceeded"]),
                }
            ),
            ApiError::QuotaExhausted {
                message: "Quota Exceeded".into()
            }
        );
        assert_eq!(
            status_to_error(
                400,
                GoogleError {
                    message: "Invalid value".into(),
                    reasons: Vec::new(),
                }
            ),
            ApiError::Rejected {
                status: 400,
                message: "Invalid value".into(),
            }
        );
    }

    // ---- retry_delay ----

    #[test]
    fn the_ladder_doubles_and_then_stops() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_delay(1, None, &policy),
            Some(Duration::from_millis(250))
        );
        assert_eq!(
            retry_delay(2, None, &policy),
            Some(Duration::from_millis(500))
        );
        assert_eq!(retry_delay(3, None, &policy), Some(Duration::from_secs(1)));
        // Budget spent — `max_retries` is 3.
        assert_eq!(retry_delay(4, None, &policy), None);
        // Not reachable from `send`, which is 1-based, but pinned so the
        // subtraction below can never underflow.
        assert_eq!(retry_delay(0, None, &policy), None);
    }

    #[test]
    fn a_retry_after_overrides_the_ladder() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_delay(1, Some(Duration::from_secs(7)), &policy),
            Some(Duration::from_secs(7))
        );
        // Zero is a valid instruction: retry now.
        assert_eq!(
            retry_delay(1, Some(Duration::ZERO), &policy),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn a_retry_after_at_the_ceiling_is_honoured_and_above_it_is_refused() {
        let policy = RetryPolicy::default();
        assert_eq!(
            retry_delay(1, Some(policy.retry_after_ceiling), &policy),
            Some(policy.retry_after_ceiling)
        );
        assert_eq!(
            retry_delay(
                1,
                Some(policy.retry_after_ceiling + Duration::from_secs(1)),
                &policy
            ),
            None
        );
    }

    /// An outlandish `max_retries` must not panic on the shift.
    #[test]
    fn a_huge_attempt_saturates_instead_of_overflowing() {
        let policy = RetryPolicy {
            max_retries: u32::MAX,
            ..RetryPolicy::default()
        };
        assert!(retry_delay(64, None, &policy).is_some());
    }

    // ---- jitter ----

    #[test]
    fn jitter_spans_minus_25_to_plus_25_percent() {
        let base = Duration::from_secs(1);
        // The span is half of `base` (500ms), starting a quarter below it.
        assert_eq!(jitter(base, 0), Duration::from_millis(750));
        assert_eq!(jitter(base, 250_000_000), base);
        assert_eq!(
            jitter(base, 499_999_999),
            Duration::from_millis(750) + Duration::from_nanos(499_999_999)
        );
        // Entropy beyond the span wraps rather than escaping it.
        assert_eq!(jitter(base, 500_000_000), Duration::from_millis(750));
    }

    #[test]
    fn jitter_of_zero_is_zero() {
        // A near-zero test policy has no span to spread over; it must not divide
        // by zero or spin.
        assert_eq!(jitter(Duration::ZERO, 12_345), Duration::ZERO);
    }
}
