//! The shapes `oxidone json` prints. This is the **public contract** (ADR-0010):
//! callers depend on these field names and these spellings, so changing one is a
//! breaking change gated on the release number, not an edit.
//!
//! Every type here is `Serialize` only. Nothing oxidone reads comes back through
//! these — `apply`'s input has its own shapes in [`super::apply`] — so a field
//! added here can never widen what the CLI *accepts*.
//!
//! The design rule is that the caller re-derives nothing. An [`Entry`] carries
//! both the raw `title` and the **Display title**, and the **Entry type** beside
//! them, so a plugin strips no glyphs and parses no prefixes: `EntryType::parse`
//! stays the single definition (ADR-0008).

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

use crate::domain::{EntryType, List, Status, Task};

/// One entry, as a caller sees it.
///
/// Deliberately not a mirror of [`Task`]: `etag` and `updated` are oxidone's own
/// sync machinery, and `links` is not in the plugin's brief. What is here is what
/// renders a row without a second question.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub list: String,
    /// `Some` => a **Subtask**. Nesting is capped at one level, so a caller can
    /// group by this without recursing.
    pub parent: Option<String>,
    /// The title exactly as Google stores it, type glyph and all.
    pub title: String,
    /// The title without its type prefix — what a row should show.
    pub display_title: String,
    #[serde(rename = "type")]
    pub entry_type: EntryKind,
    /// Whether Google's free-text `notes` field is non-empty. The field itself is
    /// not exposed: a bar row shows that notes *exist* (the `≡` marker), and
    /// shipping the body would put arbitrary user text through a pipe for nothing.
    pub has_notes: bool,
    /// A **date, never a time** — Google discards the time portion (CONTEXT.md).
    pub due: Option<NaiveDate>,
    pub status: EntryStatus,
    pub completed_at: Option<DateTime<Utc>>,
    /// Google's opaque Manual-order key. Sorting by it is what "My order" means;
    /// reading anything else into it is not supported.
    pub position: String,
}

impl Entry {
    /// Project a [`Task`] onto the wire. The only place the mapping exists.
    pub fn new(task: &Task) -> Self {
        Self {
            id: task.id.0.clone(),
            list: task.list.0.clone(),
            parent: task.parent.as_ref().map(|p| p.0.clone()),
            title: task.title.clone(),
            display_title: task.display_title().to_string(),
            entry_type: EntryKind::new(task.entry_type()),
            // `Some("")` is reachable — Google stores an empty string for notes
            // that were cleared in some clients — and an empty body is not a note
            // the user has. Presence means content.
            has_notes: task.notes.as_deref().is_some_and(|n| !n.trim().is_empty()),
            due: task.due,
            status: EntryStatus::new(task.status),
            completed_at: task.completed_at,
            position: task.position.clone(),
        }
    }
}

/// The **Entry type** (ADR-0008), lower-cased.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Task,
    Event,
    Note,
}

impl EntryKind {
    fn new(ty: EntryType) -> Self {
        match ty {
            EntryType::Task => Self::Task,
            EntryType::Event => Self::Event,
            EntryType::Note => Self::Note,
        }
    }
}

/// **Status**, in Google's own spellings — the ones CONTEXT.md names, so the
/// glossary, the API and this contract all say the same two words.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub enum EntryStatus {
    #[serde(rename = "needsAction")]
    NeedsAction,
    #[serde(rename = "completed")]
    Completed,
}

impl EntryStatus {
    fn new(status: Status) -> Self {
        match status {
            Status::NeedsAction => Self::NeedsAction,
            Status::Completed => Self::Completed,
        }
    }
}

/// One **List**. Its `etag`/`updated` are sync machinery and stay out.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ListRow {
    pub id: String,
    pub title: String,
}

impl ListRow {
    pub fn new(list: &List) -> Self {
        Self {
            id: list.id.0.clone(),
            title: list.title.clone(),
        }
    }
}

/// What a failure prints on **stderr**. stdout carries payload or nothing, so a
/// caller can pipe it to a parser without first asking whether the call worked.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ErrorEnvelope<'a> {
    pub error: ErrorBody<'a>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ErrorBody<'a> {
    /// A stable machine-readable class. Finer-grained than the exit code — three
    /// kinds share exit 3 — so a caller can map the code to a bar state and still
    /// show the user which of them happened.
    pub kind: &'a str,
    pub message: String,
}
