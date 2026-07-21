//! The ubiquitous language, as Rust types. Mirrors Google's model exactly
//! (ADR-0003: pure mirror). See `CONTEXT.md` for definitions.

use chrono::{DateTime, NaiveDate, Utc};

/// A Google TaskList — a named container of Tasks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct List {
    pub id: ListId,
    pub title: String,
    pub etag: String,
    pub updated: DateTime<Utc>,
}

/// A single Task. A Subtask is simply a Task whose `parent` is `Some`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    pub id: TaskId,
    pub list: ListId,
    /// `Some` => this is a Subtask. Capped at one level: a Task with a parent
    /// may never itself be a parent.
    pub parent: Option<TaskId>,
    pub title: String,
    pub notes: Option<String>,
    pub status: Status,
    /// Date only — the API discards any time component (see CONTEXT.md).
    pub due: Option<NaiveDate>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Google's output-only `links[]`: URLs a Task acquired from the surface it
    /// was created on (Gmail, Chat, Keep, Docs). A pure mirror (ADR-0003), never
    /// written back — `NewTask`/`TaskPatch` carry no links. Empty for the common
    /// case of a Task created in oxidone or a plain Google client.
    pub links: Vec<TaskLink>,
    /// Opaque Manual-order key; changed only via a Move.
    pub position: String,
    pub etag: String,
    pub updated: DateTime<Utc>,
}

/// One entry of Google's output-only `links[]` on a Task — the faithful mirror
/// of `{type, description, link}` (ADR-0003).
///
/// `kind` is Google's `type` and stays a `String`: the API documents an open set
/// (`email`, `generic`, `chat_message`, `keep_note`, …) and a value oxidone has
/// never seen must not break parsing. It is mirrored and persisted but not
/// surfaced — the picker shows the description, not the type, matching Google's
/// own clients.
///
/// `Serialize`/`Deserialize` are for the cache's JSON `links` column (this is
/// oxidone's own on-disk shape, not Google's wire format — the wire mapping lives
/// in `api::rest`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskLink {
    /// The link target, stored verbatim. May be non-openable (a `mailto:` or a
    /// deep link): the http/https allowlist in `crate::links` decides that at
    /// open time, not here — the mirror keeps what Google sent.
    pub url: String,
    pub description: Option<String>,
    pub kind: Option<String>,
}

impl Task {
    /// A Subtask is any Task with a parent. Nesting is capped at one level, so
    /// `is_subtask()` also means "cannot itself be a parent".
    pub fn is_subtask(&self) -> bool {
        self.parent.is_some()
    }

    /// This entry's type, derived from `title` (ADR-0008). Never stored.
    pub fn entry_type(&self) -> EntryType {
        EntryType::parse(&self.title).0
    }

    /// The title without its type prefix — what the user reads and edits.
    ///
    /// Equal to `title` for a `Task`, and for any *foreign* glyph-prefixed title
    /// that `parse` declines to classify. "Display title" means "prefix removed",
    /// not "glyph-free": oxidone cannot tell a foreign encoding from a title
    /// someone meant literally, so it shows what is stored until `t` normalises it.
    pub fn display_title(&self) -> &str {
        EntryType::parse(&self.title).1
    }
}

/// The only two states a Task can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    NeedsAction,
    Completed,
}

/// A local, read-only regrouping of the visible Tasks. Every view keeps Subtasks
/// under their parent and only reorders the groups; none of them writes Manual
/// order or a Task's `parent` — only a Move does. Attempting a Move from a Sort
/// view switches the pane back to `Manual` first (see `move_preconditions`), so
/// the reorder lands against the adjacency the user can actually see.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortView {
    /// Google's `position` order ("My order").
    Manual,
    /// By due date; Tasks with no due date sink to the bottom deterministically.
    /// The home state — a daily driver opens on what is due.
    Due,
    /// Case-insensitive by title.
    Title,
}

impl SortView {
    /// The next view in the triage cycle, starting from the `Due` home state:
    /// Due → Title → Manual → Due.
    pub fn next(self) -> Self {
        match self {
            SortView::Manual => SortView::Due,
            SortView::Due => SortView::Title,
            SortView::Title => SortView::Manual,
        }
    }

    /// The next view in the **Today** cycle: Due ↔ Title only. Manual is excluded
    /// there — `position` is per-List, so a hand-arranged order across Lists is
    /// genuinely undefined (Moves are disabled in Today). `Manual` maps to `Due`
    /// so a lens carried in from a real List lands on the home state on first `s`.
    pub fn next_today(self) -> Self {
        match self {
            SortView::Due => SortView::Title,
            SortView::Title | SortView::Manual => SortView::Due,
        }
    }

    /// A short lower-case label for the pane title. Every view names itself, so
    /// the header always says which lens is active — with `Due` the home state,
    /// an unlabelled pane would make Manual the silent one.
    pub fn label(self) -> &'static str {
        match self {
            SortView::Manual => "my order",
            SortView::Due => "due",
            SortView::Title => "title",
        }
    }
}

/// A Bullet Journal entry type, derived from the Task's title rather than stored
/// (ADR-0008). `Task` is the default and carries no prefix, so every Task that
/// already exists — and everything created in Google's own clients — is
/// correctly typed with no backfill.
///
/// The glyphs are always the Unicode ones. `config.ascii_fallback` degrades how
/// they *render*, never what is written: an ASCII prefix on the wire would have
/// to be parsed back too, and toggling the flag would otherwise silently revert
/// every typed entry to `Task`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    /// The default. Actionable, and the only type the Completion meter counts.
    Task,
    /// Something that happens on a day. Occupies the Due-load, never the meter.
    Event,
    /// A jotting. Counted by neither.
    ///
    /// Distinct from `Task::notes`, which is Google's free-text field (see
    /// CONTEXT.md): a Note is what an entry *is*, notes are what it *carries*.
    Note,
}

impl EntryType {
    /// The literal prefix this type writes, `""` for `Task`.
    pub fn prefix(self) -> &'static str {
        match self {
            EntryType::Task => "",
            EntryType::Event => "○ ",
            EntryType::Note => "— ",
        }
    }

    /// Classify a raw title into its type and display title.
    ///
    /// Total, and deliberately never rewrites: a title is typed only when it
    /// begins with the exact glyph, then exactly one space, then a **non-empty**
    /// remainder. Anything else is a `Task` whose display title is the raw title
    /// verbatim — including a bare `"○ "`, which would otherwise classify as a
    /// typed entry with no content.
    ///
    /// So `parse` is the exact inverse of [`EntryType::apply`] for *canonical*
    /// titles. Raw titles do not all come from oxidone — Google's web and mobile
    /// clients write them too — and repairing the rest is [`EntryType::retype`]'s
    /// job, not this one.
    pub fn parse(title: &str) -> (Self, &str) {
        for ty in [EntryType::Event, EntryType::Note] {
            if let Some(rest) = title.strip_prefix(ty.prefix()) {
                if !rest.is_empty() {
                    return (ty, rest);
                }
            }
        }
        (EntryType::Task, title)
    }

    /// Build a raw title from a display title, prefix only.
    ///
    /// **Never strips.** This is the ordinary write path's function — the one
    /// behind an `e` edit — and an edit that changes nothing must write back
    /// exactly what was there. Stripping here would silently rewrite titles
    /// nobody touched.
    ///
    /// Callers must pass a non-empty `display`; the edit path's own empty-check
    /// already guarantees it.
    pub fn apply(self, display: &str) -> String {
        format!("{}{display}", self.prefix())
    }

    /// Build a raw title for a *type change*, repairing a foreign prefix first.
    ///
    /// The only stripping function, and the only one [`EntryType::apply`] is not.
    /// A raw title from Google may be glyph-prefixed without being canonical —
    /// `"○Standup"`, `"○  Standup"`, a bare `"○"` — and each parses as an
    /// untyped `Task` whose display title still leads with the glyph. Prefixing
    /// that directly would stack (`"○ ○Standup"`), so the leading run of glyph
    /// characters and whitespace is removed and the prefix *rebuilt*. Stacking is
    /// therefore unreachable rather than guarded against, and `t` self-heals a
    /// foreign title into a canonical one on first press.
    ///
    /// Returns `None` when nothing survives the strip (`"○"`, `"— "`, `"○ ○"`,
    /// or an empty title — reachable, since Google may return a Task with no
    /// title at all). A type is not something an unnameable entry can carry, and
    /// `None` makes that unrepresentable rather than a documented precondition.
    ///
    /// Lossy by construction: `"—— dashes"` becomes `"○ dashes"` and cycling will
    /// not bring the dash back. That is the price of making stacking impossible,
    /// and it is confined to this function's single caller.
    pub fn retype(self, display: &str) -> Option<String> {
        let stripped =
            display.trim_start_matches(|c: char| c == '○' || c == '—' || c.is_whitespace());
        (!stripped.is_empty()).then(|| self.apply(stripped))
    }

    /// The next type in the forward cycle: Task → Event → Note → Task.
    pub fn next(self) -> Self {
        match self {
            EntryType::Task => EntryType::Event,
            EntryType::Event => EntryType::Note,
            EntryType::Note => EntryType::Task,
        }
    }

    /// The next type in the reverse cycle: Task → Note → Event → Task.
    ///
    /// Exists so every type is one keypress from any other. Cycling forward-only
    /// would put Note two presses from Task, and because a type change rides the
    /// title-write path, the second press inside the first's flight window is
    /// refused — turning a flip into press, wait for the network, press again.
    pub fn prev(self) -> Self {
        match self {
            EntryType::Task => EntryType::Note,
            EntryType::Note => EntryType::Event,
            EntryType::Event => EntryType::Task,
        }
    }
}

/// What the sidebar cursor points at: the pinned **Today** view (a cross-List
/// aggregate of what is due), or a real List by index into `Model::lists`.
///
/// Replaces a bare `Option<usize>`: Today is always selectable (it is pinned and
/// needs no List), so there is no "nothing selected" state to model. `List(i)`
/// with `i` out of range is transient (a List just removed) and clamped back to a
/// valid selection — falling to `Today`, which is always valid — by the reducer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Today,
    List(usize),
}

/// Whether an entry belongs to the **Today** view: dated on or before `today`.
/// Undated entries are excluded — `None` is not `<= today` — so a Task with no due
/// date never appears in Today. The single definition of Today membership, shared
/// by the cache aggregate (`sync::today_from_cache`) and the view filter
/// (`Model::is_visible`) so the two cannot drift.
pub fn due_on_or_before(due: Option<NaiveDate>, today: NaiveDate) -> bool {
    due.is_some_and(|d| d <= today)
}

/// Whether an entry sits in Today's **Overdue** group: dated strictly before
/// `today`. The single definition of the Overdue/Today split, shared by the
/// ordering (`Model::today_ordered`, which sorts these rows to the front) and the
/// journal spread that counts them, so the header can never name a different set
/// than the one drawn beneath it.
///
/// Status-blind, unlike the overdue *styling* in the view: a Completed overdue
/// row groups by its date like any other, or it would break the contiguous
/// prefix the spread counts. The Completed exemption belongs to the date's
/// colour, not to the group, and is stated separately at that one call site.
pub fn due_before(due: Option<NaiveDate>, today: NaiveDate) -> bool {
    due.is_some_and(|d| d < today)
}

// Newtypes keep List and Task ids from being swapped by accident.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ListId(pub String);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TaskId(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [EntryType; 3] = [EntryType::Task, EntryType::Event, EntryType::Note];

    #[test]
    fn a_canonical_title_round_trips_through_apply_and_parse() {
        for ty in ALL {
            let raw = ty.apply("Standup");
            assert_eq!(EntryType::parse(&raw), (ty, "Standup"), "{ty:?}");
        }
    }

    #[test]
    fn an_untyped_title_is_a_task_verbatim() {
        assert_eq!(EntryType::parse("Buy milk"), (EntryType::Task, "Buy milk"));
        assert_eq!(EntryType::parse(""), (EntryType::Task, ""));
    }

    #[test]
    fn a_glyph_with_an_empty_remainder_is_not_a_typed_entry() {
        // Without the non-empty clause these would classify as typed entries
        // with no content, and `apply` would then write a title that is pure
        // encoding.
        assert_eq!(EntryType::parse("○ "), (EntryType::Task, "○ "));
        assert_eq!(EntryType::parse("— "), (EntryType::Task, "— "));
    }

    #[test]
    fn a_title_that_genuinely_starts_with_a_glyph_reads_as_typed() {
        // Accepted and documented: rare, visible, and one `t` press from
        // correction. The strict single-space rule keeps the blast radius small.
        assert_eq!(EntryType::parse("— dashes"), (EntryType::Note, "dashes"));
    }

    #[test]
    fn apply_never_strips() {
        // The edit path's function. An `e` that changes nothing must write back
        // exactly what was there, foreign glyph and all.
        assert_eq!(EntryType::Task.apply("○Standup"), "○Standup");
        assert_eq!(EntryType::Task.apply("○ "), "○ ");
        assert_eq!(EntryType::Note.apply("—"), "— —");
    }

    #[test]
    fn retype_is_idempotent_in_the_glyph_prefix() {
        // Stacking is unreachable because the prefix is rebuilt, not appended:
        // retyping an already-typed title yields the same string as typing the
        // bare one, however many times it is applied.
        let bare = EntryType::Event.retype("Standup").unwrap();
        for ty in ALL {
            let once = ty.retype("Standup").unwrap();
            let twice = ty.retype(&once).unwrap();
            assert_eq!(once, twice, "{ty:?} stacked");
        }
        assert_eq!(bare, "○ Standup");
    }

    #[test]
    fn retype_repairs_foreign_titles_on_the_first_press() {
        // Each of these parses as an untyped Task whose display title still
        // leads with the glyph, so a plain prefix would stack on press one.
        for foreign in ["○Standup", "○  Standup", "○ ○ Standup", "  Standup"] {
            let (_, display) = EntryType::parse(foreign);
            assert_eq!(
                EntryType::Event.retype(display).as_deref(),
                Some("○ Standup"),
                "{foreign:?}"
            );
        }
    }

    #[test]
    fn retype_declines_a_title_that_strips_to_nothing() {
        // `""` is reachable: WireTask.title is `#[serde(default)]`, so Google
        // may hand back a Task with no title at all.
        for empty in ["", "○", "— ", "○ ○", "   "] {
            let (_, display) = EntryType::parse(empty);
            assert_eq!(EntryType::Event.retype(display), None, "{empty:?}");
        }
    }

    #[test]
    fn next_today_cycles_due_and_title_only_never_manual() {
        // Today has no Manual order (position is per-List), so the cycle is a
        // two-state flip and Manual folds into Due on the way in.
        assert_eq!(SortView::Due.next_today(), SortView::Title);
        assert_eq!(SortView::Title.next_today(), SortView::Due);
        assert_eq!(SortView::Manual.next_today(), SortView::Due);
        // Manual is never reachable by repeated Today cycling.
        let mut sort = SortView::Manual;
        for _ in 0..6 {
            sort = sort.next_today();
            assert_ne!(sort, SortView::Manual);
        }
    }

    #[test]
    fn next_and_prev_are_inverses_and_walk_opposite_ways() {
        for ty in ALL {
            assert_eq!(ty.next().prev(), ty, "{ty:?}");
            assert_eq!(ty.prev().next(), ty, "{ty:?}");
        }
        assert_eq!(EntryType::Task.next(), EntryType::Event);
        assert_eq!(EntryType::Task.prev(), EntryType::Note); // Note in one press
    }

    #[test]
    fn task_accessors_derive_from_the_raw_title() {
        let mut t = Task {
            id: TaskId("t".into()),
            list: ListId("l".into()),
            parent: None,
            title: "○ Standup".into(),
            notes: None,
            status: Status::NeedsAction,
            due: None,
            completed_at: None,
            links: Vec::new(),
            position: "0".into(),
            etag: String::new(),
            updated: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        };
        assert_eq!(t.entry_type(), EntryType::Event);
        assert_eq!(t.display_title(), "Standup");

        // A foreign prefix stays visible until `t` normalises it.
        t.title = "○Standup".into();
        assert_eq!(t.entry_type(), EntryType::Task);
        assert_eq!(t.display_title(), "○Standup");
    }

    /// The two date predicates differ on exactly one day — today itself — and
    /// that is the whole Overdue/Today split. Undated is in neither: a Task with
    /// no due date is not in Today at all, so it can never be its Overdue group.
    #[test]
    fn the_overdue_split_is_due_on_or_before_minus_today_itself() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 20).expect("valid date");
        let day = |d: u32| Some(NaiveDate::from_ymd_opt(2026, 7, d).expect("valid date"));

        for due in [day(19), day(1)] {
            assert!(due_before(due, today), "{due:?} is overdue");
            assert!(due_on_or_before(due, today), "{due:?} is in Today");
        }
        // Today itself: in the view, not in the Overdue group.
        assert!(!due_before(day(20), today));
        assert!(due_on_or_before(day(20), today));
        // Future and undated are in neither.
        for due in [day(21), None] {
            assert!(!due_before(due, today), "{due:?} is not overdue");
            assert!(!due_on_or_before(due, today), "{due:?} is not in Today");
        }
    }
}
