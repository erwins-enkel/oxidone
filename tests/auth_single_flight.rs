//! `SingleFlight`, the guard that stops a token-cache miss from driving more than
//! one interactive consent flow.
//!
//! The live flow needs a browser and a Google account, so it is the *guard* that
//! carries the coverage: fake `TokenProvider`s stand in for `yup-oauth2`, one
//! recording how many callers it ever held at once, one that presents a URL and
//! then never answers — the abandoned loopback listener. No network, no
//! `client_secret.json`, no terminal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use oxidone::api::ApiError;
use oxidone::auth::{ConsentPrompt, ConsentSink, SingleFlight, TokenProvider};

/// Long enough that a passing test never reaches it; short enough that the
/// timeout tests cost milliseconds, not minutes.
const PATIENT: Duration = Duration::from_secs(30);
const IMPATIENT: Duration = Duration::from_millis(50);

const CONSENT_URL: &str = "https://accounts.google.com/o/oauth2/auth?scope=tasks";

/// Records the greatest number of callers ever inside it at once. `delay` widens
/// the window, so a genuine race would be observed rather than merely possible.
#[derive(Clone)]
struct CountingProvider {
    inside: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
    delay: Duration,
}

impl CountingProvider {
    fn new(delay: Duration) -> Self {
        Self {
            inside: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
            calls: Arc::new(AtomicUsize::new(0)),
            delay,
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::Acquire)
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Acquire)
    }
}

#[async_trait]
impl TokenProvider for CountingProvider {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let inside = self.inside.fetch_add(1, Ordering::AcqRel) + 1;
        self.peak.fetch_max(inside, Ordering::AcqRel);
        tokio::time::sleep(self.delay).await;
        self.inside.fetch_sub(1, Ordering::AcqRel);
        Ok("token".to_string())
    }
}

/// Presents a URL — as the real flow delegate does from inside `yup-oauth2` — and
/// then never answers, like a loopback listener awaiting a redirect nobody sent.
struct AbandonedFlow {
    prompt: Arc<ConsentPrompt>,
}

#[async_trait]
impl TokenProvider for AbandonedFlow {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.prompt.present(CONSENT_URL);
        std::future::pending().await
    }
}

/// Presents a URL and then fails outright.
struct FailedFlow {
    prompt: Arc<ConsentPrompt>,
}

#[async_trait]
impl TokenProvider for FailedFlow {
    async fn bearer(&self) -> Result<String, ApiError> {
        self.prompt.present(CONSENT_URL);
        Err(ApiError::Network("no route".to_string()))
    }
}

/// Presents on its first call and answers from cache thereafter — the real
/// sequence once a flow has persisted its token.
struct FlowThenCache {
    prompt: Arc<ConsentPrompt>,
    calls: AtomicUsize,
}

#[async_trait]
impl TokenProvider for FlowThenCache {
    async fn bearer(&self) -> Result<String, ApiError> {
        if self.calls.fetch_add(1, Ordering::AcqRel) == 0 {
            self.prompt.present(CONSENT_URL);
        }
        Ok("token".to_string())
    }
}

/// A `ConsentSink` that records every call, in order, into a shared log.
struct RecordingSink {
    events: Arc<Mutex<Vec<String>>>,
}

impl ConsentSink for RecordingSink {
    fn present(&self, url: &str) {
        self.log(format!("present {url}"));
    }

    fn dismiss(&self, reason: Option<&str>) {
        self.log(match reason {
            Some(reason) => format!("dismiss {reason}"),
            None => "dismiss".to_string(),
        });
    }
}

impl RecordingSink {
    fn log(&self, event: String) {
        self.events.lock().expect("sink log mutex").push(event);
    }
}

/// A prompt over a fresh recording sink, plus the log to read it back from.
fn recording_prompt() -> (Arc<ConsentPrompt>, Arc<Mutex<Vec<String>>>) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = RecordingSink {
        events: Arc::clone(&events),
    };
    (Arc::new(ConsentPrompt::new(Box::new(sink))), events)
}

fn events(log: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
    log.lock().expect("sink log mutex").clone()
}

/// The instrument, checked first: unguarded, the same three callers *do* pile into
/// the provider together. Without this the assertion below would pass just as well
/// on a fake that can never observe a race.
#[tokio::test]
async fn the_counting_provider_observes_an_unguarded_race() {
    let inner = Arc::new(CountingProvider::new(Duration::from_millis(20)));

    let calls: Vec<_> = (0..3)
        .map(|_| {
            let inner = Arc::clone(&inner);
            tokio::spawn(async move { inner.bearer().await })
        })
        .collect();
    for call in calls {
        call.await.expect("join").expect("bearer");
    }

    assert_eq!(inner.peak(), 3);
}

/// The defect this guard exists for: three callers racing an unusable token cache
/// must enter the provider one at a time. Two concurrent entries is two consent
/// URLs on two loopback ports — the doubled hint in the report.
#[tokio::test]
async fn concurrent_callers_never_enter_the_provider_together() {
    let inner = CountingProvider::new(Duration::from_millis(20));
    let (prompt, _log) = recording_prompt();
    let guard = Arc::new(SingleFlight::new(inner.clone(), prompt, PATIENT));

    let calls: Vec<_> = (0..3)
        .map(|_| {
            let guard = Arc::clone(&guard);
            tokio::spawn(async move { guard.bearer().await })
        })
        .collect();
    for call in calls {
        assert_eq!(call.await.expect("join").expect("bearer"), "token");
    }

    assert_eq!(inner.peak(), 1, "the provider was entered concurrently");
    assert_eq!(inner.calls(), 3, "every caller must still get a token");
}

/// A refresh takes the same gate as a plain fetch: a forced refresh also falls
/// through to a full consent flow when the grant is gone, so letting one run
/// beside a fetch would be a second flow all the same.
#[tokio::test]
async fn a_refresh_shares_the_gate_with_a_fetch() {
    let inner = CountingProvider::new(Duration::from_millis(20));
    let (prompt, _log) = recording_prompt();
    let guard = Arc::new(SingleFlight::new(inner.clone(), prompt, PATIENT));

    let fetch = {
        let guard = Arc::clone(&guard);
        tokio::spawn(async move { guard.bearer().await })
    };
    let refresh = {
        let guard = Arc::clone(&guard);
        tokio::spawn(async move { guard.refresh().await })
    };
    fetch.await.expect("join").expect("bearer");
    refresh.await.expect("join").expect("refresh");

    assert_eq!(inner.peak(), 1);
    assert_eq!(inner.calls(), 2);
}

/// An abandoned flow must not hold the gate for the rest of the session: it times
/// out, says why, and the next caller gets through.
#[tokio::test]
async fn an_abandoned_flow_times_out_and_releases_the_gate() {
    let (prompt, log) = recording_prompt();
    let guard = SingleFlight::new(
        AbandonedFlow {
            prompt: Arc::clone(&prompt),
        },
        prompt,
        IMPATIENT,
    );

    let outcome = guard.bearer().await;
    assert!(
        matches!(outcome, Err(ApiError::AuthExpired)),
        "expected AuthExpired, got {outcome:?}"
    );

    // The gate is free again: the second call reaches the provider (and times out
    // on its own) instead of blocking behind the first for ever.
    assert!(matches!(guard.bearer().await, Err(ApiError::AuthExpired)));

    let events = events(&log);
    assert_eq!(events.len(), 4, "unexpected sink traffic: {events:?}");
    assert_eq!(events[0], format!("present {CONSENT_URL}"));
    assert!(
        events[1].starts_with("dismiss authorization was not completed within"),
        "the timeout must say why: {:?}",
        events[1]
    );
}

/// A failed acquisition retracts the prompt and carries the provider's reason — a
/// flow that ends badly must not leave the prompt standing.
#[tokio::test]
async fn a_failed_acquisition_reports_its_reason() {
    let (prompt, log) = recording_prompt();
    let guard = SingleFlight::new(
        FailedFlow {
            prompt: Arc::clone(&prompt),
        },
        prompt,
        PATIENT,
    );

    assert!(matches!(guard.bearer().await, Err(ApiError::Network(_))));

    let events = events(&log);
    assert_eq!(
        events,
        vec![
            format!("present {CONSENT_URL}"),
            "dismiss authorization failed: network error: no route".to_string(),
        ]
    );
}

/// The overwhelmingly common path: a token straight from cache, no prompt ever
/// presented. It must send *nothing* — a dismissal per API call would repaint the
/// frame dozens of times over for a prompt that was never shown.
#[tokio::test]
async fn a_cache_hit_never_touches_the_prompt() {
    let (prompt, log) = recording_prompt();
    let guard = SingleFlight::new(CountingProvider::new(Duration::ZERO), prompt, PATIENT);

    for _ in 0..5 {
        guard.bearer().await.expect("bearer");
    }

    assert!(
        events(&log).is_empty(),
        "silent path sent {:?}",
        events(&log)
    );
}

/// One flow, then cache hits: the prompt is retracted once, not once per call.
#[tokio::test]
async fn a_presented_prompt_is_dismissed_exactly_once() {
    let (prompt, log) = recording_prompt();
    let guard = SingleFlight::new(
        FlowThenCache {
            prompt: Arc::clone(&prompt),
            calls: AtomicUsize::new(0),
        },
        prompt,
        PATIENT,
    );

    for _ in 0..3 {
        guard.bearer().await.expect("bearer");
    }

    assert_eq!(
        events(&log),
        vec![format!("present {CONSENT_URL}"), "dismiss".to_string()]
    );
}
