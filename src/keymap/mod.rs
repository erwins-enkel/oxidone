//! Keymap-as-data (ADR-0005 spirit): modeless single-key bindings expressed as
//! a table of `(key -> Action)`, not a match sprawl. The `?` cheatsheet is
//! rendered straight from this table, and user rebinding (a later ticket) is a
//! matter of loading a different table. Context-sensitivity (per-pane keys)
//! joins the table as slices need it.
//!
//! The always-visible legend is a second, curated view of the same data: see
//! `legend`, whose cells name `Action`s and resolve their key text through
//! `bindings()` rather than restating it.

use crossterm::event::{KeyCode, KeyEvent};

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
    // Sidebar List management. Bound to capitals so they never clash with the
    // task-pane verbs (`a`/`e`/`x`); the reducer additionally gates them on the
    // sidebar being focused.
    AddList,
    RenameList,
    DeleteList,
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

/// Resolve a key press to its bound `Action`, if any. Modifiers are ignored for
/// now — the shell's verbs are all plain keys.
pub fn resolve(key: KeyEvent) -> Option<Action> {
    bindings()
        .iter()
        .find(|b| b.key == key.code)
        .map(|b| b.action)
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
    /// A text-capture overlay: chars go to the buffer, Enter saves, Esc cancels.
    TextInput,
    /// An add-entry capture (add task / subtask): like `TextInput`, but `Enter`
    /// peels a trailing natural-language date off the title and `Tab` submits it
    /// verbatim, so the legend advertises the extra key.
    TaskCapture,
    /// A Confirm overlay: only y/n/Esc fire.
    Confirm,
    /// The link picker: j/k move, Enter opens, Esc cancels.
    LinkPicker,
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
    const TEXT_INPUT: &[LegendEntry] = &[
        LegendEntry {
            keys: LegendKeys::Literal("Enter"),
            label: "save",
        },
        LegendEntry {
            keys: LegendKeys::Literal("Esc"),
            label: "cancel",
        },
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

    match context {
        LegendContext::Tasks => TASKS,
        LegendContext::Sidebar => SIDEBAR,
        LegendContext::TextInput => TEXT_INPUT,
        LegendContext::TaskCapture => TASK_CAPTURE,
        LegendContext::Confirm => CONFIRM,
        LegendContext::LinkPicker => LINK_PICKER,
    }
}
