//! The interactive consent flow, end to end, with no browser and no Google
//! account (ADR-0011).
//!
//! Owning the flow is what makes this reachable at all: the authorization URL,
//! both roads a callback can arrive by, the `state` check that guards the pasted
//! one, and the PKCE-carrying code exchange are all exercised here against a
//! `wiremock` token endpoint.
//!
//! Everything goes through `auth::login`, the same entry point the binary calls
//! — the flow's own types stay private, so what is asserted is the behaviour the
//! app gets, not the shape of the code behind it.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};
use yup_oauth2::storage::TokenInfo;

use oxidone::auth::{self, ConsentPrompt, ConsentSink, FileTokenStore, TokenStore};

/// Long enough to outlast a loaded CI box, short enough that a flow which never
/// answers fails the test instead of hanging it — `CONSENT_TIMEOUT` is ten
/// minutes, and waiting that out is not a test result.
const PATIENCE: Duration = Duration::from_secs(10);

/// Records what the flow told the user, and lets the test read the consent URL
/// back out — which is how it learns this attempt's `state` and loopback port
/// without either being exposed by the library.
struct RecordingSink {
    presented: UnboundedSender<String>,
    rejected: Arc<Mutex<Vec<String>>>,
}

impl ConsentSink for RecordingSink {
    fn present(&self, url: &str) {
        // Deliberately no browser hand-off: that is the sink's job, and a test
        // sink has no business opening one.
        self.presented
            .send(url.to_string())
            .expect("the test is still listening");
    }

    fn reject(&self, reason: &str) {
        self.rejected.lock().unwrap().push(reason.to_string());
    }

    fn dismiss(&self, _reason: Option<&str>) {}
}

/// One consent in flight: the URL it presented, the channel a pasted callback
/// goes in by, and the rejections it has reported so far.
struct Flow {
    url: String,
    paste: UnboundedSender<String>,
    rejected: Arc<Mutex<Vec<String>>>,
    /// `Option` so `finish` can take the handle without consuming the flow — the
    /// tests go on to read what it stored.
    login: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    store: Arc<FileTokenStore>,
}

impl Flow {
    /// The `state` this attempt will accept a callback for.
    fn state(&self) -> String {
        self.query("state")
    }

    /// The loopback URL Google would redirect to, port and all.
    fn redirect_uri(&self) -> String {
        self.query("redirect_uri")
    }

    fn query(&self, key: &str) -> String {
        reqwest::Url::parse(&self.url)
            .expect("a consent URL")
            .query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
            .unwrap_or_else(|| panic!("no {key} in the consent URL: {}", self.url))
    }

    /// This attempt's callback, as the browser's address bar would show it.
    fn callback(&self, code: &str) -> String {
        format!(
            "{}/?code={code}&state={}",
            self.redirect_uri(),
            self.state()
        )
    }

    async fn finish(&mut self) -> anyhow::Result<()> {
        let login = self.login.take().expect("the flow is only finished once");
        tokio::time::timeout(PATIENCE, login)
            .await
            .expect("the consent flow never settled")
            .expect("the login task panicked")
    }

    fn stored(&self) -> TokenInfo {
        let blob = self
            .store
            .load()
            .expect("reading the token store")
            .expect("a grant on disk");
        serde_json::from_str(&blob).expect("a parseable TokenInfo")
    }
}

/// Start a consent against `server`, and wait until it has presented its URL.
async fn start(server: &MockServer, dir: &tempfile::TempDir) -> Flow {
    let secret_path = dir.path().join("client_secret.json");
    fs::write(
        &secret_path,
        json!({
            "installed": {
                "client_id": "client-id",
                "client_secret": "client-secret",
                "auth_uri": format!("{}/auth", server.uri()),
                "token_uri": format!("{}/token", server.uri()),
                "redirect_uris": ["http://127.0.0.1"],
            }
        })
        .to_string(),
    )
    .expect("writing the client secret");

    let store = Arc::new(FileTokenStore::new(dir.path().join("token.json")));
    let (presented, mut urls): (UnboundedSender<String>, UnboundedReceiver<String>) =
        mpsc::unbounded_channel();
    let rejected = Arc::new(Mutex::new(Vec::new()));
    let prompt = Arc::new(ConsentPrompt::new(Box::new(RecordingSink {
        presented,
        rejected: Arc::clone(&rejected),
    })));
    let (paste, input) = auth::callback_channel();

    let login_store = Arc::clone(&store) as Arc<dyn TokenStore>;
    let login = tokio::spawn(async move {
        auth::login(&secret_path, login_store, prompt, Arc::new(input)).await
    });

    let url = tokio::time::timeout(PATIENCE, urls.recv())
        .await
        .expect("the flow never presented a URL")
        .expect("the flow dropped its sink");

    Flow {
        url,
        paste,
        rejected,
        login: Some(login),
        store,
    }
}

/// Google's answer to a good code exchange.
fn granted() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({
        "access_token": "fresh-access",
        "refresh_token": "the-grant",
        "expires_in": 3600,
        "token_type": "Bearer",
    }))
}

async fn mount_token(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .respond_with(response)
        .mount(server)
        .await;
}

/// Everything the exchange posted, as `key=value` pairs. Parsed through a URL
/// because the body is form-encoded and `reqwest` re-exports the parser oxidone
/// already depends on.
fn posted(request: &Request) -> Vec<(String, String)> {
    let body = String::from_utf8(request.body.clone()).expect("a form-encoded body");
    reqwest::Url::parse(&format!("http://exchange/?{body}"))
        .expect("a parseable body")
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// The value posted under `key`, which must be there and must not be blank.
fn posted_value<'a>(posted: &'a [(String, String)], key: &str) -> &'a str {
    let value = posted
        .iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("no {key} in the exchange: {posted:?}"));
    assert!(!value.1.is_empty(), "{key} was posted blank: {posted:?}");
    &value.1
}

/// Deliver a callback the way a browser on *this* machine would: straight to the
/// loopback listener.
async fn redirect_to(flow: &Flow, code: &str) -> String {
    let address = flow
        .redirect_uri()
        .trim_start_matches("http://")
        .to_string();
    let mut stream = TcpStream::connect(&address)
        .await
        .expect("the loopback listener is up");
    let request = format!(
        "GET /?code={code}&state={} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n",
        flow.state()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("writing the redirect");

    let mut answer = String::new();
    stream
        .read_to_string(&mut answer)
        .await
        .expect("reading the answer");
    answer
}

/// The road this feature exists for: the browser is on another machine, so the
/// address it lands on comes back by hand.
#[tokio::test]
async fn a_pasted_callback_completes_the_authorization() {
    let server = MockServer::start().await;
    mount_token(&server, granted()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    flow.paste
        .send(flow.callback("4/0AX4"))
        .expect("the flow is waiting");
    let outcome = flow.finish().await;
    assert!(outcome.is_ok(), "{outcome:?}");

    let stored = flow.stored();
    assert_eq!(stored.refresh_token.as_deref(), Some("the-grant"));
    assert_eq!(stored.access_token.as_deref(), Some("fresh-access"));

    let exchange = &server.received_requests().await.expect("recorded requests")[0];
    let posted = posted(exchange);
    assert_eq!(posted_value(&posted, "code"), "4/0AX4");
    assert_eq!(posted_value(&posted, "client_secret"), "client-secret");
    // The PKCE secret that never travelled with the URL, proving the exchange
    // is the only place it goes.
    assert!(!flow.url.contains(posted_value(&posted, "code_verifier")));
}

/// The listener is still there the whole time, so a local run is unchanged: the
/// browser's redirect settles the flow with nothing typed.
#[tokio::test]
async fn the_loopback_redirect_still_completes_the_authorization() {
    let server = MockServer::start().await;
    mount_token(&server, granted()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    let answer = redirect_to(&flow, "4/0AX4").await;
    assert!(
        answer.contains("200 OK") && answer.contains("close this tab"),
        "the browser was left with nothing to read: {answer}"
    );

    assert!(flow.finish().await.is_ok());
}

/// The exchange must name the very port the authorization URL named — Google
/// compares the two, and a mismatch is an opaque refusal.
#[tokio::test]
async fn the_exchange_echoes_the_redirect_uri_from_the_authorization_url() {
    let server = MockServer::start().await;
    mount_token(&server, granted()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;
    let redirect_uri = flow.redirect_uri();

    flow.paste
        .send(flow.callback("4/0AX4"))
        .expect("the flow is waiting");
    assert!(flow.finish().await.is_ok());

    let exchange = &server.received_requests().await.expect("recorded requests")[0];
    assert!(
        posted(exchange)
            .iter()
            .any(|(k, v)| k == "redirect_uri" && *v == redirect_uri),
        "the exchange named a different redirect than the authorization URL did"
    );
}

/// A paste that is not this attempt's callback is reported and the flow keeps
/// waiting — the whole point of a field a human types into.
#[tokio::test]
async fn a_bad_paste_is_reported_and_the_flow_keeps_waiting() {
    let server = MockServer::start().await;
    mount_token(&server, granted()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    for bad in [
        "4/0AX4".to_string(),
        format!("{}/?code=&state={}", flow.redirect_uri(), flow.state()),
        format!("{}/?code=stolen&state=somebody-else", flow.redirect_uri()),
        format!(
            "{}/?error=access_denied&state={}",
            flow.redirect_uri(),
            flow.state()
        ),
    ] {
        flow.paste.send(bad).expect("the flow is waiting");
    }

    // Then the real one, which is what proves the flow was still there.
    flow.paste
        .send(flow.callback("4/0AX4"))
        .expect("the flow is waiting");
    assert!(flow.finish().await.is_ok());

    let rejected = flow.rejected.lock().unwrap().clone();
    assert_eq!(
        rejected.len(),
        4,
        "not every bad paste was reported: {rejected:?}"
    );
    assert!(rejected[0].contains("address bar"), "{rejected:?}");
    assert!(
        rejected[1].contains("no authorization code"),
        "{rejected:?}"
    );
    assert!(
        rejected[2].contains("different authorization attempt"),
        "{rejected:?}"
    );
    assert!(rejected[3].contains("access_denied"), "{rejected:?}");
}

/// A grant with no refresh token behind it works until its access token expires
/// and then sends the user back to the browser — every day, for ever. It is
/// refused instead of stored.
#[tokio::test]
async fn a_grant_with_no_refresh_token_is_refused_rather_than_stored() {
    let server = MockServer::start().await;
    mount_token(
        &server,
        ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "fresh-access",
            "expires_in": 3600,
            "token_type": "Bearer",
        })),
    )
    .await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    flow.paste
        .send(flow.callback("4/0AX4"))
        .expect("the flow is waiting");
    let outcome = flow.finish().await;
    assert!(outcome.is_err(), "a dead-end grant was accepted");
    assert!(
        FileTokenStore::new(dir.path().join("token.json"))
            .load()
            .expect("reading the token store")
            .is_none(),
        "a dead-end grant was written to the store"
    );
}

/// A code Google refuses ends the flow with a reason, rather than leaving the
/// prompt up for ever over a code that can never work.
#[tokio::test]
async fn a_refused_code_ends_the_flow() {
    let server = MockServer::start().await;
    mount_token(
        &server,
        ResponseTemplate::new(400).set_body_json(json!({
            "error": "invalid_grant",
            "error_description": "Bad Request",
        })),
    )
    .await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    flow.paste
        .send(flow.callback("already-used"))
        .expect("the flow is waiting");
    let error = flow.finish().await.expect_err("a refused code succeeded");
    assert!(
        format!("{error:#}").contains("already have been used"),
        "the refusal does not say what to do about it: {error:#}"
    );
}

/// The authorization URL has to carry what makes the grant refreshable and the
/// pasted road safe. Asserted here too, not only in the unit test, because this
/// is the URL a real `login` actually produced.
#[tokio::test]
async fn the_presented_url_asks_for_a_refreshable_grant() {
    let server = MockServer::start().await;
    mount_token(&server, granted()).await;
    let dir = tempfile::tempdir().expect("temp dir");
    let mut flow = start(&server, &dir).await;

    assert_eq!(flow.query("access_type"), "offline");
    assert_eq!(flow.query("prompt"), "consent");
    assert_eq!(flow.query("code_challenge_method"), "S256");
    assert!(!flow.query("code_challenge").is_empty());
    assert!(flow.redirect_uri().starts_with("http://127.0.0.1:"));

    flow.paste
        .send(flow.callback("4/0AX4"))
        .expect("the flow is waiting");
    assert!(flow.finish().await.is_ok());
    assert_eq!(flow.stored().refresh_token.as_deref(), Some("the-grant"));
}
