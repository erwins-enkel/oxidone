//! How an interactive consent flow reaches the user.
//!
//! `yup-oauth2`'s default delegate presents the consent URL with `println!`,
//! which scrolls the frame apart when the TUI owns the terminal. Everything that
//! presents a URL goes through [`ConsentPrompt`] instead, so *where* it lands is
//! the caller's decision: stdout before the TUI is up, the `Message` channel
//! after.

use std::sync::atomic::{AtomicBool, Ordering};

/// Where a consent URL is shown, and where the flow's outcome is reported.
pub trait ConsentSink: Send + Sync {
    /// Show `url`: the flow is waiting for the user to visit it.
    fn present(&self, url: &str);
    /// The flow settled. `reason` is `Some` when it failed, carrying the text to
    /// show the user — a flow that ends badly must not just vanish.
    fn dismiss(&self, reason: Option<&str>);
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

    /// The flow settled; retract the prompt if one is showing, and say why when
    /// it failed. A no-op when nothing was presented.
    pub fn dismiss(&self, reason: Option<&str>) {
        if self.open.swap(false, Ordering::AcqRel) {
            self.sink.dismiss(reason);
        }
    }
}

/// Writes the consent URL to stdout, the way `yup-oauth2`'s own delegate does.
/// For the first-run flow, which runs *before* the TUI starts and so has no
/// frame to corrupt — and no other way to reach the user.
pub struct StdoutConsentSink;

impl ConsentSink for StdoutConsentSink {
    fn present(&self, url: &str) {
        println!(
            "Please direct your browser to {url} and follow the instructions displayed there."
        );
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
