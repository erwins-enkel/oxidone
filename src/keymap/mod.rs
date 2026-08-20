//! Keymap-as-data (ADR-0005 spirit): modeless single-key bindings expressed as
//! a table of `(key -> Action)`, not a match sprawl. The `?` cheatsheet is
//! rendered straight from this table, and user rebinding (a later ticket) is a
//! matter of loading a different table. Context-sensitivity (per-pane keys)
//! joins the table as slices need it.
//!
//! The always-visible legend is a second, curated view of the same data: see
//! `legend`, whose cells name `Action`s and resolve their key text through
//! `bindings()` rather than restating it.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// A user-facing verb. Grows as slices add behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleHelp,
    CloseOverlay,
    SwitchPane,
    // Directional pane focus, alongside `SwitchPane`'s toggle. Idempotent at the
    // edges: there is no wrap, so focusing left from the sidebar is a no-op.
    FocusLeft,
    FocusRight,
    SelectNext,
    SelectPrev,
    ToggleComplete,
    AddTask,
    EditTitle,
    EditDue,
    /// Bullet Journal migration — the `>` disposition: push an entry forward a
    /// day. Not an exit: the Task stays `needsAction`, only its due date moves.
    /// `>` itself is `Indent`, so the binding is the verb's initial.
    Migrate,
    /// Cycle the selected entry's Bullet Journal type forward:
    /// Task → Event → Note → Task.
    CycleType,
    /// Cycle it backward: Task → Note → Event → Task. With three types that
    /// puts every type one press from any other — see `EntryType::prev`.
    CycleTypeBack,
    EditNotes,
    DeleteTask,
    CycleSort,
    ToggleShowCompleted,
    /// Hide/reveal entries due beyond the configured horizon (`hide_distant`).
    ToggleHideDistant,
    /// Open the title/notes filter input (`/`): a live view filter narrowing the
    /// task pane by a case-insensitive substring of a row's title or notes.
    Filter,
    /// Enter the cross-List **Search** pane (`S`): the whole cached corpus,
    /// narrowed live by the `/` query. When Search is already active, reopens the
    /// query input over the existing query.
    Search,
    /// Open the **Omnibox** (`p`): one grouped list over a query, offering the
    /// Lists to jump to, the commands (keyless, bar `:refresh` and `:week`, which
    /// `r` and `W` also fire), a hand-off to Search, and — last — capturing the
    /// query as a Task.
    Omnibox,
    /// Open a URL found in the selected Task's notes.
    OpenLink,
    ClearCompleted,
    // Manual Refresh: re-pull the List set (and, via the cascade, the active
    // List's Tasks) from Google. Modeless — it is not gated on a pane.
    Refresh,
    // Add a Subtask under the selected Task. An insert, not a Move: it sets
    // `parent` at creation, so it is not gated on the lens.
    AddSubtask,
    // The Move operations (task pane). Each writes Manual order or `parent` and
    // computes against stored order, so one pressed from a Sort view switches
    // the pane back to Manual first; the next press performs it.
    Indent,
    Outdent,
    MoveDown,
    MoveUp,
    /// Relocate the selected Task to another List — a cross-List Move, opened as
    /// a picker. Unlike the four above it writes no Manual order, so it neither
    /// needs nor switches the Sort lens and works in Today.
    MoveToList,
    // Sidebar List management. Bound to capitals so they never clash with the
    // task-pane verbs (`a`/`e`/`x`); the reducer additionally gates them on the
    // sidebar being focused.
    AddList,
    RenameList,
    DeleteList,
    /// Open or close the **Weekly spread** (`W`): a planning surface over
    /// Monday–Friday, scoped by the sidebar row it is opened on. `w` is already
    /// `ToggleHideDistant`, so this takes the shift-pair, as `t`/`T` and `J`/`K` do.
    ///
    /// The spread's *own* keys — the day cursor, the dot, `]`/`[` — are not
    /// `Action`s: they are routed ahead of this table in the reducer, because
    /// they mean something different inside that one pane. `LegendContext::Week`
    /// is where they are advertised.
    ToggleWeek,
}

/// One row of the keymap: the key, the verb it triggers, and its cheatsheet text.
pub struct Binding {
    pub key: KeyCode,
    pub action: Action,
    pub help: &'static str,
}

/// The default, hardcoded binding table.
pub fn bindings() -> &'static [Binding] {
    const BINDINGS: &[Binding] = &[
        Binding {
            key: KeyCode::Char('q'),
            action: Action::Quit,
            help: "quit",
        },
        Binding {
            key: KeyCode::Char('?'),
            action: Action::ToggleHelp,
            help: "toggle this help",
        },
        Binding {
            key: KeyCode::Tab,
            action: Action::SwitchPane,
            help: "switch pane",
        },
        Binding {
            key: KeyCode::Esc,
            action: Action::CloseOverlay,
            help: "close overlay",
        },
        // Directional counterparts to `Tab`. Vim key first, then the arrow, the
        // way `j`/`Down` and `k`/`Up` already pair.
        Binding {
            key: KeyCode::Char('h'),
            action: Action::FocusLeft,
            help: "focus pane left",
        },
        Binding {
            key: KeyCode::Left,
            action: Action::FocusLeft,
            help: "focus pane left",
        },
        Binding {
            key: KeyCode::Char('l'),
            action: Action::FocusRight,
            help: "focus pane right",
        },
        Binding {
            key: KeyCode::Right,
            action: Action::FocusRight,
            help: "focus pane right",
        },
        Binding {
            key: KeyCode::Char('j'),
            action: Action::SelectNext,
            help: "select next",
        },
        Binding {
            key: KeyCode::Down,
            action: Action::SelectNext,
            help: "select next",
        },
        Binding {
            key: KeyCode::Char('k'),
            action: Action::SelectPrev,
            help: "select previous",
        },
        Binding {
            key: KeyCode::Up,
            action: Action::SelectPrev,
            help: "select previous",
        },
        Binding {
            key: KeyCode::Char(' '),
            action: Action::ToggleComplete,
            help: "toggle complete",
        },
        Binding {
            key: KeyCode::Char('a'),
            action: Action::AddTask,
            help: "add task",
        },
        Binding {
            key: KeyCode::Char('e'),
            action: Action::EditTitle,
            help: "edit title",
        },
        // `Enter` is the natural "open this row" affordance; for now it is an
        // alias of `e`. Overlay keys are routed before the keymap, so this never
        // shadows Enter-to-submit inside an overlay.
        Binding {
            key: KeyCode::Enter,
            action: Action::EditTitle,
            help: "edit title",
        },
        Binding {
            key: KeyCode::Char('d'),
            action: Action::EditDue,
            help: "edit due date",
        },
        // Directly after `d`: a due verb, and the position is load-bearing.
        // `cheatsheet_rows` preserves this order, `help_layout` partitions it
        // sequentially into columns and drops hidden rows from the *tail* — so
        // appending at the end would put new verbs first in line to be dropped.
        Binding {
            key: KeyCode::Char('m'),
            action: Action::Migrate,
            help: "migrate (forward one day)",
        },
        Binding {
            key: KeyCode::Char('n'),
            action: Action::EditNotes,
            help: "edit notes ($EDITOR)",
        },
        // After `n`: entry-attribute verbs, alongside title and notes. Mid-table
        // for the same reason as `m` above — `help_layout` drops cheatsheet rows
        // from the tail, and new verbs should not be first in line for that.
        Binding {
            key: KeyCode::Char('t'),
            action: Action::CycleType,
            help: "cycle entry type",
        },
        Binding {
            key: KeyCode::Char('T'),
            action: Action::CycleTypeBack,
            help: "cycle entry type back",
        },
        Binding {
            key: KeyCode::Char('x'),
            action: Action::DeleteTask,
            help: "delete task",
        },
        Binding {
            key: KeyCode::Char('u'),
            action: Action::OpenLink,
            help: "open link",
        },
        Binding {
            key: KeyCode::Char('s'),
            action: Action::CycleSort,
            help: "cycle sort (due/title/my order)",
        },
        Binding {
            key: KeyCode::Char('c'),
            action: Action::ToggleShowCompleted,
            help: "show/hide completed",
        },
        // Beside `c`, the other silent view-toggle, and deliberately not at the
        // tail: `help_layout` drops cheatsheet rows from the end, so a new verb
        // appended after the sidebar capitals would be first to vanish on a
        // short pane. No always-visible legend cell — the 80-column TASKS row is
        // already full through `c completed` (see `legend`); this lives in `?`.
        Binding {
            key: KeyCode::Char('w'),
            action: Action::ToggleHideDistant,
            help: "show/hide distant tasks",
        },
        // With the view toggles, and mid-table for the same reason as `m`/`w`/`M`:
        // `help_layout` drops cheatsheet rows from the tail, so a new verb appended
        // after the sidebar capitals would be first to vanish on a short pane. No
        // always-visible legend cell — the 80-column TASKS row is already full
        // through `c completed` (see `legend`); this lives in `?` and, while active,
        // in the pane header.
        Binding {
            key: KeyCode::Char('/'),
            action: Action::Filter,
            help: "filter by title/notes",
        },
        // Beside `/`, and like it: no always-visible legend cell (the 80-column
        // TASKS row is full through `c completed` — see `legend`), so this lives in
        // `?` and, while active, in the `SEARCH` pane title.
        Binding {
            key: KeyCode::Char('S'),
            action: Action::Search,
            help: "search all lists",
        },
        // Beside `/` and `S`, the other query surfaces, and mid-table for the
        // reason they and `m`/`w`/`M` each give: `help_layout` drops cheatsheet
        // rows from the tail, so a new verb appended after the sidebar capitals
        // would be first to vanish on a short pane. No always-visible legend
        // cell — the 80-column TASKS row is already full through `c completed`
        // (see `legend`); this lives in `?`.
        //
        // `Ctrl-P` opens it too, free: `resolve` is modifier-blind, exactly as
        // `Ctrl-Q` already quits. Unadvertised, because the table cannot express
        // a chord (#105) and `p` is the key the cheatsheet teaches.
        Binding {
            key: KeyCode::Char('p'),
            action: Action::Omnibox,
            help: "jump, run or capture",
        },
        Binding {
            key: KeyCode::Char('C'),
            action: Action::ClearCompleted,
            help: "clear completed",
        },
        Binding {
            key: KeyCode::Char('r'),
            action: Action::Refresh,
            help: "refresh from Google",
        },
        Binding {
            key: KeyCode::Char('o'),
            action: Action::AddSubtask,
            help: "add subtask",
        },
        Binding {
            key: KeyCode::Char('>'),
            action: Action::Indent,
            help: "indent (make subtask)",
        },
        Binding {
            key: KeyCode::Char('<'),
            action: Action::Outdent,
            help: "outdent (to top-level)",
        },
        Binding {
            key: KeyCode::Char('J'),
            action: Action::MoveDown,
            help: "move task down",
        },
        Binding {
            key: KeyCode::Char('K'),
            action: Action::MoveUp,
            help: "move task up",
        },
        // With the Move group, and mid-table for the same reason as `m`/`w`:
        // `help_layout` drops cheatsheet rows from the tail. `M` is the initial
        // of the verb, not the capital of `m` (which is Migrate) — the two are
        // unrelated, and the help text carries the distinction. No
        // always-visible legend cell: the 80-column TASKS row is already full
        // through `c completed` (see `legend`); this lives in `?`.
        Binding {
            key: KeyCode::Char('M'),
            action: Action::MoveToList,
            help: "move to another list",
        },
        Binding {
            key: KeyCode::Char('W'),
            action: Action::ToggleWeek,
            help: "toggle weekly spread",
        },
        Binding {
            key: KeyCode::Char('A'),
            action: Action::AddList,
            help: "add list",
        },
        Binding {
            key: KeyCode::Char('R'),
            action: Action::RenameList,
            help: "rename list",
        },
        Binding {
            key: KeyCode::Char('X'),
            action: Action::DeleteList,
            help: "delete list",
        },
    ];
    BINDINGS
}

/// Resolve a key press to its bound `Action`, if any.
///
/// The verbs are plain keys, so modifiers are ignored — with one exception: the
/// four text-editing chords this app advertises resolve to nothing, because
/// `a`/`e`/`u`/`w` are all bound to verbs, one of which opens a browser. Every
/// other Ctrl chord still resolves as if unmodified (see the body).
pub fn resolve(key: KeyEvent) -> Option<Action> {
    // `^A`/`^E`/`^U`/`^W` are advertised in the overlay legends, so they become
    // muscle memory — and a press landing just outside an overlay (a beat before
    // `d` opens it, a beat after `Enter` closes it) arrives here instead. Without
    // this, `^U` would resolve to `u` → `OpenLink`, which spawns a browser for a
    // single-link Task, `^W` to `w` → `ToggleHideDistant`, silently emptying the
    // pane, `^A` to `a` → `AddTask` and `^E` to `e` → `EditTitle`, each springing
    // the very overlay the user just left.
    //
    // Scoped to those four keys rather than to every Ctrl chord, deliberately.
    // This table is modifier-blind throughout — `Ctrl-Q` quits, `Ctrl-C` toggles
    // Completed — and that is left exactly as it was: gating the lot would
    // silently change two more keys in a change that neither introduced nor
    // advertised them. Making the whole table modifier-aware is a decision of its
    // own, with its own tests, tracked in #105. What is gated here is only what
    // this app now *teaches* the user to press.
    if is_control_chord(key.modifiers) && matches!(key.code, KeyCode::Char('a' | 'e' | 'u' | 'w')) {
        return None;
    }
    bindings()
        .iter()
        .find(|b| b.key == key.code)
        .map(|b| b.action)
}

/// Whether this keystroke is a `Ctrl` chord rather than text — `CONTROL` on any
/// key, not just the two this app binds. Callers that care about a *particular*
/// chord match the key code as well; callers that just need "this is not a
/// character" (the text overlays, so `^A` does not type an `a`) use it as it is.
///
/// `CONTROL` **without** `ALT`, not "either of them":
///
/// - `ALT` alongside `CONTROL` is AltGr, which Windows consoles report as
///   `LEFT_CTRL | RIGHT_ALT`. It is how `@ \ [ ] { } ~ | €` are typed on German,
///   Polish and Nordic layouts, so it means a character.
/// - `ALT` alone is left to mean whatever it means today (macOS Option-as-Meta
///   sends it for `Option`+letter); this predicate does not claim it.
/// - `SHIFT` is ignored entirely — capitals carry it, and several bindings are
///   capitals.
///
/// Lives here rather than in `app` because [`resolve`] reads it too, and `app`
/// depends on `keymap` and not the reverse. Deciding whether a keystroke is a
/// chord is this module's job anyway.
pub fn is_control_chord(m: KeyModifiers) -> bool {
    m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT)
}

/// The cheatsheet's rows: one per distinct `(action, help)` pair, labelled with
/// every key bound to it, joined with `/` (`j/Down`, `e/Enter`).
///
/// Rows come back in first-appearance order within `bindings()`, and that is
/// part of the contract, not an accident of the implementation: the `?` popup
/// splits this slice into columns positionally, so the order decides which rows
/// share a column and therefore how wide each column is. Hence the linear
/// group-by rather than a `HashMap`, whose iteration order would reshuffle the
/// popup between runs.
///
/// Distinct from `LegendEntry::key_text`, which resolves a *curated* list of
/// `Action`s to the *first* key bound to each. The legend wants one compact key
/// per verb; the cheatsheet wants every key that triggers one.
pub fn cheatsheet_rows() -> Vec<(String, &'static str)> {
    let mut groups: Vec<(Action, &'static str, Vec<String>)> = Vec::new();
    for b in bindings() {
        match groups
            .iter_mut()
            .find(|(action, help, _)| *action == b.action && *help == b.help)
        {
            Some((_, _, labels)) => labels.push(key_label(b.key)),
            None => groups.push((b.action, b.help, vec![key_label(b.key)])),
        }
    }
    groups
        .into_iter()
        .map(|(_, help, labels)| (labels.join("/"), help))
        .collect()
}

/// A short label for a key, for the cheatsheet.
pub fn key_label(code: KeyCode) -> String {
    match code {
        KeyCode::Char(' ') => "Space".to_string(),
        // Named like the other special keys, and load-bearing: `cheatsheet_rows`
        // and `LegendEntry::key_text` join a verb's keys with `/`, so a label of
        // literal "/" would split a row in two. "Slash" is the one printable key
        // that must spell its name.
        KeyCode::Char('/') => "Slash".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        other => format!("{other:?}"),
    }
}

/// Which legend the current state calls for. Owned here rather than taken as
/// `(Focus, Overlay)` so this module keeps depending on nothing but crossterm;
/// the view maps its own state onto it at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegendContext {
    Tasks,
    Sidebar,
    /// A text-capture overlay: chars go in at the caret, Enter saves, Esc
    /// cancels, and the readline chords `^U`/`^W`/`^A`/`^E` clear the line, kill
    /// the word before the caret, and jump to either end.
    TextInput,
    /// The due-date editor: like `TextInput`, plus arrows and page keys that step
    /// the date a day or a week at a time, so the legend advertises them.
    DueInput,
    /// An add-entry capture (add task / subtask): like `TextInput`, but `Enter`
    /// peels a trailing natural-language date off the title and `Tab` submits it
    /// verbatim, so the legend advertises the extra key.
    TaskCapture,
    /// A Confirm overlay: only y/n/Esc fire.
    Confirm,
    /// The link picker: j/k move, Enter opens, Esc cancels.
    LinkPicker,
    /// The move-to-List picker: j/k move, Enter moves, Esc cancels.
    ListPicker,
    /// The title/notes filter input: characters narrow the pane live, `Enter`
    /// keeps the filter applied, `Esc` drops it entirely, and `^U` clears just
    /// the query text. `Esc`'s cell names its scope rather than saying "clear",
    /// which would read as interchangeable with `^U`'s.
    Filter,
    /// The **Omnibox** (`p`): a grouped result list over a query. Movement is
    /// `Up`/`Down` only — `j`/`k` type, as they do in every other text overlay —
    /// and `Enter` runs whichever row is highlighted.
    Omnibox,
    /// The **Weekly spread**'s task pane. `h`/`l` walk the day columns instead of
    /// moving focus and `Space` acts on the cell under the cursor, so the ordinary
    /// `Tasks` legend would advertise two keys that no longer do what it says.
    Week,
    /// The same input open in **Search**, where `Esc` exits Search rather than
    /// clearing the query — so the `Esc` cell must read "leave search", not
    /// "clear", or the legend promises an affordance the pane does not honour.
    /// `^U` is the only way to empty the query here, precisely because of that.
    SearchFilter,
}

/// Where a legend cell's key text comes from.
#[derive(Debug)]
pub enum LegendKeys {
    /// Looked up in `bindings()` — the first row matching each `Action`, joined
    /// with `/`. The slice's order *is* the rendered order.
    Derived(&'static [Action]),
    /// Literal keys for contexts handled outside this table, i.e. the overlay
    /// keys hardcoded in the reducer's `overlay_key`. No table to derive from,
    /// so a change there must be mirrored here by hand.
    Literal(&'static str),
}

/// One cell of the always-visible legend: the keys it advertises and a terse
/// label. Deliberately shorter than a `Binding`'s `help` — "move", not
/// "select next" — because the legend pays for every column it occupies.
#[derive(Debug)]
pub struct LegendEntry {
    pub keys: LegendKeys,
    pub label: &'static str,
}

impl LegendEntry {
    /// The cell's key text: derived keys resolved through `bindings()`, literal
    /// keys as written.
    ///
    /// An `Action` with no binding contributes nothing rather than panicking a
    /// render. That swallow is only safe because it cannot happen unnoticed:
    /// `every_derived_legend_action_is_bound` fails the build if a legend cell
    /// ever names an unbound verb.
    pub fn key_text(&self) -> String {
        match self.keys {
            LegendKeys::Derived(actions) => actions
                .iter()
                .filter_map(|action| {
                    bindings()
                        .iter()
                        .find(|b| b.action == *action)
                        .map(|b| key_label(b.key))
                })
                .collect::<Vec<_>>()
                .join("/"),
            LegendKeys::Literal(keys) => keys.to_string(),
        }
    }

    /// The cell as rendered: `"{keys} {label}"`.
    pub fn text(&self) -> String {
        format!("{} {}", self.key_text(), self.label)
    }
}

/// The pinned help cell. Not a member of any `legend()` slice — the view
/// right-aligns it and reserves its width before fitting anything else, so the
/// pointer to the full cheatsheet survives every width that can show it.
pub const HELP: LegendEntry = LegendEntry {
    keys: LegendKeys::Derived(&[Action::ToggleHelp]),
    label: "help",
};

/// The legend cells for a context, in priority order — the view drops from the
/// right, so the order *is* the drop order.
///
/// Priority is set by how recoverable a verb is if unknown, not by raw
/// frequency: orientation first, then triage, then verbs that silently change
/// what is on screen, and last those that are aliased or announce themselves.
pub fn legend(context: LegendContext) -> &'static [LegendEntry] {
    // Navigation reads `j/k` and `h/l` only because the letters are bound
    // before their arrow aliases and the slices list next-then-previous —
    // `[SelectPrev, SelectNext]` would render "k/j".
    const MOVE: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::SelectNext, Action::SelectPrev]),
        label: "move",
    };
    const PANE: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::FocusLeft, Action::FocusRight]),
        label: "pane",
    };
    const QUIT: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::Quit]),
        label: "quit",
    };
    const ADD: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::AddTask]),
        label: "add",
    };
    // `c` hides Completed Tasks with nothing on screen to say so, which is why
    // it outranks `s` — the pane title already names the active Sort view.
    const COMPLETED: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::ToggleShowCompleted]),
        label: "completed",
    };
    const SORT: LegendEntry = LegendEntry {
        keys: LegendKeys::Derived(&[Action::CycleSort]),
        label: "sort",
    };

    const TASKS: &[LegendEntry] = &[
        MOVE,
        PANE,
        QUIT,
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::ToggleComplete]),
            label: "done",
        },
        ADD,
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::EditDue]),
            label: "due",
        },
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::DeleteTask]),
            label: "del",
        },
        COMPLETED,
        // Below `completed`, deliberately. At 80 columns the row's budget is 72
        // and the cells above already total exactly 72, so *any* cell inserted
        // at or above `completed` evicts it — and `c` outranks `m` on the same
        // recoverability grounds that put `completed` above `link`: not knowing
        // `c` means your Tasks vanished, not knowing `m` means you reach for `d`.
        // Placed here, the 80-column row is unchanged and `migrate` shows only
        // on wider panes.
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::Migrate]),
            label: "migrate",
        },
        // The last four announce themselves elsewhere, so they drop first on a
        // narrow pane: `Enter` already aliases `e`, the pane title names the
        // active Sort view, a Task with links carries the `⧉` link marker, and a
        // typed entry carries its signifier. Promoting `link` far enough to show
        // at 80 columns would drop `c completed`, which outranks it because
        // hiding Completed Tasks changes the screen with nothing on it to say so.
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::EditTitle]),
            label: "edit",
        },
        SORT,
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::OpenLink]),
            label: "link",
        },
        // Last, and below `link` deliberately. The row drops from the right, so
        // anything inserted above `link` evicts it at the width where it used to
        // fit — and `type` has the weakest claim to displace it: the signifier
        // column already announces an entry's type on every row that has one,
        // which is the same reason `link` ranks low. Both keys share one cell.
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::CycleType, Action::CycleTypeBack]),
            label: "type",
        },
    ];

    // The spread's keys are hardcoded in the reducer's `week_key`, not in
    // `bindings()`, so they are literal — a change there must be mirrored here by
    // hand, exactly as the overlay legends are.
    //
    // Ordered by what it costs not to know. `day` and `plan` are the pane: without
    // them the grid is inert. `done` reads differently here than anywhere else —
    // it is `Space` acting on a cell — so it earns third place over the digits,
    // which are only a shortcut for what `day` + `plan` already do. `week` is
    // last: the panel title already names the week on screen.
    const WEEK: &[LegendEntry] = &[
        MOVE,
        QUIT,
        LegendEntry {
            keys: LegendKeys::Literal("h/l"),
            label: "day",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Space"),
            label: "plan/done",
        },
        LegendEntry {
            keys: LegendKeys::Literal("1-5"),
            label: "mon-fri",
        },
        LegendEntry {
            keys: LegendKeys::Literal("0"),
            label: "unschedule",
        },
        ADD,
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::ToggleWeek]),
            label: "close",
        },
        LegendEntry {
            keys: LegendKeys::Literal("]/["),
            label: "week",
        },
    ];

    const SIDEBAR: &[LegendEntry] = &[
        MOVE,
        PANE,
        QUIT,
        // `a` is not focus-gated — it captures into the highlighted List — so
        // it earns a slot here too. `A add list` beside it carries the contrast.
        ADD,
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::AddList]),
            label: "add list",
        },
        LegendEntry {
            keys: LegendKeys::Derived(&[Action::RenameList]),
            label: "rename",
        },
        COMPLETED,
        SORT,
    ];

    // Overlay keys live in the reducer, not `bindings()`, so they are literal.
    //
    // The three chords go last in every slice they join, in this order: the row
    // drops from the right, and not knowing `Esc` strands you in the overlay,
    // where not knowing `^U` costs you a few Backspaces and not knowing `^A`/`^E`
    // costs you a few arrow presses — the cheapest of the three to miss.
    const KILL_LINE: LegendEntry = LegendEntry {
        keys: LegendKeys::Literal("^U"),
        label: "clear",
    };
    const KILL_WORD: LegendEntry = LegendEntry {
        keys: LegendKeys::Literal("^W"),
        label: "word",
    };
    // Caret motion, by its readline names only: `←`/`→` and `Home`/`End` do the
    // same work and need no advertising, where a cell costs columns on every row
    // it joins.
    const CARET: LegendEntry = LegendEntry {
        keys: LegendKeys::Literal("^A/^E"),
        label: "ends",
    };

    const TEXT_INPUT: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "save",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
        KILL_LINE,
        KILL_WORD,
        CARET,
    ];

    // The due editor: `Enter`/`Esc` first (the escape hatches outrank
    // everything), then the stepping keys, then the chords. This is the one
    // slice already full at 80 columns, so `CARET` shows only from 88 on — the
    // drop rule working as designed, pinned by `the_due_editor_legend_*` tests.
    //
    // The signs read `-/+` in *key* order: `Up` and `PageUp` step backwards, so
    // `+/-day` would pair the first key with the wrong sign. ASCII throughout —
    // `render_legend` takes no `ascii` flag, so a cell has no way to degrade, and
    // every other `Literal` in this file is ASCII already. `Up/Down` is what
    // `key_label` would print for those codes; `PgUp/PgDn` deliberately is not,
    // because `PageUp/PageDown` is 15 cells and would evict a real verb.
    const DUE_INPUT: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "save",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Up/Down"),
            label: "-/+day",
        },
        LegendEntry {
            keys: LegendKeys::Literal("PgUp/PgDn"),
            label: "-/+week",
        },
        KILL_LINE,
        KILL_WORD,
        CARET,
    ];

    // `Tab` submits the title verbatim (no date parsing) — a key the plain
    // text-input legend would not have said.
    const TASK_CAPTURE: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "save",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Tab"),
            label: "literal",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
        KILL_LINE,
        KILL_WORD,
        CARET,
    ];

    const CONFIRM: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("y"),
            label: "yes",
        },
        LegendEntry {
            keys: LegendKeys::Literal("n"),
            label: "no",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
    ];

    // `Enter` opens rather than saves, and `j`/`k` move — neither of which the
    // text-input legend would have said.
    const LINK_PICKER: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("j/k"),
            label: "move",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "open",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
    ];

    // Not the link picker's shape: this picker type-aheads, so every printable
    // key narrows the candidates and the cursor keys are `Up`/`Down` — with
    // `^N`/`^P` advertised alongside, unlike `CARET`'s silent synonyms, because
    // the `?` cheatsheet covers pane keys only and would leave them undiscoverable.
    // Both pairs share one cell rather than repeating the label "move" twice.
    //
    // `^N`/`^P` are bound here where the Omnibox refuses them, and the reason the
    // Omnibox gives still stands: `resolve` is modifier-blind, so a `^N` landing
    // just outside the overlay reaches `n` → `EditNotes` and suspends the TUI into
    // `$EDITOR`. That is a hazard of the pane rather than of this picker — a `^N`
    // with no overlay up does it today — and this picker is the one place a
    // home-row cursor pair is worth the exposure, its `j`/`k` having gone to the
    // query.
    const LIST_PICKER: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Up/Down ^N/^P"),
            label: "move",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "move here",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
        KILL_LINE,
        KILL_WORD,
    ];

    // The filter narrows live as you type; `Enter` keeps it applied and `Esc`
    // discards it entirely — neither of which the plain text-input legend would
    // have said.
    //
    // `Esc` reads "drop filter", not "clear": `^U` below also clears, and two
    // cells reading the same word would assert the keys are interchangeable when
    // they are not. `Esc` empties the query *and* closes the input *and*
    // unfilters the pane; `^U` empties it and leaves you typing. Naming the scope
    // on `Esc` — as `SEARCH_FILTER` already does — keeps `^U clear` reading
    // identically in all four text legends.
    const FILTER: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "keep",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "drop filter",
        },
        KILL_LINE,
        KILL_WORD,
    ];

    // The same input in Search: `Esc` exits Search rather than clearing the query,
    // so that cell reads "leave search" — the pane behind the input is the corpus,
    // not a List, and an `Esc` promising "clear" would promise landing on a full
    // result set.
    //
    // `^U` is the only way to empty the query here, precisely because `Esc`
    // leaves instead — which is why it earns a cell even though the row is
    // otherwise the shortest in the file.
    const SEARCH_FILTER: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "keep",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "leave search",
        },
        KILL_LINE,
        KILL_WORD,
    ];

    // The Omnibox: escape hatches first, then movement, then the chords — the
    // `DUE_INPUT` order, for the reason given there. ASCII throughout: `Up/Down`
    // is what `key_label` prints for those codes, and `render_legend` takes no
    // `ascii` flag, so a cell has no way to degrade.
    //
    // `Enter` reads "run", not "save": the highlighted row may jump, search,
    // move or capture, and only two of those write anything. No cell for `^N`/`^P` —
    // they are not bound, deliberately: `resolve` is modifier-blind, so a `^N`
    // landing just outside the overlay would reach `n` → `EditNotes` and suspend
    // the TUI into `$EDITOR`.
    const OMNIBOX: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "run",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "close",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Up/Down"),
            label: "move",
        },
        KILL_LINE,
        KILL_WORD,
    ];

    match context {
        LegendContext::Tasks => TASKS,
        LegendContext::Week => WEEK,
        LegendContext::Sidebar => SIDEBAR,
        LegendContext::TextInput => TEXT_INPUT,
        LegendContext::DueInput => DUE_INPUT,
        LegendContext::TaskCapture => TASK_CAPTURE,
        LegendContext::Confirm => CONFIRM,
        LegendContext::LinkPicker => LINK_PICKER,
        LegendContext::ListPicker => LIST_PICKER,
        LegendContext::Omnibox => OMNIBOX,
        LegendContext::Filter => FILTER,
        LegendContext::SearchFilter => SEARCH_FILTER,
    }
}
