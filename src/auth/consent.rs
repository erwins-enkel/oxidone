//! How an interactive consent flow reaches the user, in both directions.
//!
//! A consent URL written with `println!` scrolls the frame apart when the TUI
//! owns the terminal, so everything that presents one goes through
//! [`ConsentPrompt`] and *where* it lands is the caller's decision: stdout
//! before the TUI is up, the `Message` channel after. [`CallbackInput`] is the
//! same decision for the answer coming back — the terminal before, the reducer's
//! paste field after.

use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::Mutex;

/// Where a consent URL is shown, and where the flow's outcome is reported.
///
/// "Shown" includes handing the URL to a browser — [`open_in_browser`] — because
/// that is one of the ways a URL reaches a user, and which ways apply is what
/// distinguishes one sink from another.
pub trait ConsentSink: Send + Sync {
    /// Show `url`: the flow is waiting for the user to visit it.
    fn present(&self, url: &str);
    /// Something offered as the callback was not one, and the flow is *still*
    /// waiting. Distinct from [`ConsentSink::dismiss`] because nothing has
    /// settled: the user pastes again.
    fn reject(&self, reason: &str);
    /// The flow settled. `reason` is `Some` when it failed, carrying the text to
    /// show the user — a flow that ends badly must not just vanish.
    fn dismiss(&self, reason: Option<&str>);
}

/// Where a pasted callback URL comes *from*, when the browser's redirect cannot
/// reach this machine.
///
/// The mirror of [`ConsentSink`], and split out for the same reason: stdin
/// before the TUI is up, a channel from the reducer after it, and neither is
/// something [`super::flow`] should know about. Both deliver the same thing —
/// whatever a human offered as the callback URL — so the flow parses one kind of
/// input however it arrived.
#[async_trait::async_trait]
pub trait CallbackInput: Send + Sync {
    /// The next line a human offered, or `None` when no more ever will.
    async fn next(&self) -> Option<String>;
}

/// A [`CallbackInput`] with nobody behind it, for a run with no way to paste —
/// stdin is not a terminal, so the loopback redirect is the only road in.
pub struct NoCallbackInput;

#[async_trait::async_trait]
impl CallbackInput for NoCallbackInput {
    async fn next(&self) -> Option<String> {
        None
    }
}

/// A [`CallbackInput`] fed through a channel: the TUI's paste field on one end,
/// the waiting consent flow on the other.
pub struct ChannelCallbackInput {
    /// `Mutex` because receiving needs `&mut` and the trait hands out `&self`.
    /// Uncontended in practice — one flow waits at a time, `SingleFlight` sees
    /// to that.
    rx: Mutex<UnboundedReceiver<String>>,
}

/// The two ends of the paste path: a sender for whoever collects the text, and
/// the [`CallbackInput`] the flow waits on.
pub fn callback_channel() -> (UnboundedSender<String>, ChannelCallbackInput) {
    let (tx, rx) = mpsc::unbounded_channel();
    (tx, ChannelCallbackInput { rx: Mutex::new(rx) })
}

#[async_trait::async_trait]
impl CallbackInput for ChannelCallbackInput {
    async fn next(&self) -> Option<String> {
        self.rx.lock().await.recv().await
    }
}

/// A [`ConsentSink`] with an open/closed lifecycle: `dismiss` reaches the sink
/// only when a `present` opened it.
///
/// That idempotence is load-bearing rather than tidiness. `dismiss` is called
/// after *every* token acquisition, and almost all of them are cache hits that
/// presented nothing; without the flag each one would send a message and repaint
/// the frame.
pub struct ConsentPrompt {
    sink: Box<dyn ConsentSink>,
    open: AtomicBool,
}

impl ConsentPrompt {
    pub fn new(sink: Box<dyn ConsentSink>) -> Self {
        Self {
            sink,
            open: AtomicBool::new(false),
        }
    }

    /// The flow needs the user to visit `url`.
    pub fn present(&self, url: &str) {
        self.open.store(true, Ordering::Release);
        self.sink.present(url);
    }

    /// Something offered as the callback was not one. Reaches the sink only
    /// while a prompt is showing — there is nothing to correct otherwise — and
    /// leaves it open, because the flow is still waiting.
    pub fn reject(&self, reason: &str) {
        if self.open.load(Ordering::Acquire) {
            self.sink.reject(reason);
        }
    }

    /// The flow settled; retract the prompt if one is showing, and say why when
    /// it failed. A no-op when nothing was presented.
    pub fn dismiss(&self, reason: Option<&str>) {
        if self.open.swap(false, Ordering::AcqRel) {
            self.sink.dismiss(reason);
        }
    }
}

/// Writes the consent URL to stdout. For the first-run flow, which runs *before*
/// the TUI starts and so has no frame to corrupt — and no other way to reach the
/// user.
pub struct StdoutConsentSink {
    /// Whether a [`CallbackInput`] is reading this terminal, and so whether
    /// there is anywhere to paste. Told rather than assumed: with stdin
    /// redirected there is no paste to offer, and inviting one would be an
    /// instruction the user cannot follow.
    pub paste_offered: bool,
}

impl ConsentSink for StdoutConsentSink {
    fn present(&self, url: &str) {
        println!(
            "Please direct your browser to {url} and follow the instructions displayed there."
        );
        if self.paste_offered {
            println!(
                "If that browser cannot reach this machine, paste the address it \
                 ends up on here and press Enter:"
            );
        }
        open_in_browser(url);
    }

    fn reject(&self, reason: &str) {
        eprintln!("oxidone: {reason}");
    }

    fn dismiss(&self, reason: Option<&str>) {
        // A scrolling terminal has nothing to retract, so only a failure has
        // anything left to say. The success case is deliberately silent: the
        // caller's own output already reports what happened next.
        if let Some(reason) = reason {
            eprintln!("oxidone: {reason}");
        }
    }
}

/// Hand `url` to the platform browser on a blocking thread — the spawn itself is
/// synchronous work — mirroring the Task-link opener in `main.rs`.
///
/// Best effort by design: the prompt is already showing the URL, so a browser
/// that will not start costs the user a copy-paste, not their session — and on
/// the machine the paste path exists for there is no browser to start at all.
/// The scheme check is not bypassed just because this URL is one we built.
pub fn open_in_browser(url: &str) {
    let Some(target) = crate::links::OpenableUrl::parse(url) else {
        tracing::warn!(%url, "consent url is not openable; showing it only");
        return;
    };
    tokio::task::spawn_blocking(move || {
        if let Err(e) = open::that_detached(target.as_str()) {
            tracing::warn!(error = %e, "could not open the browser for consent");
        }
    });
}
