# oxidone

A single-user terminal (TUI) client for Google Tasks, in Rust. It is a *daily-driver* cockpit — you live in it to triage, create, complete, and reorder tasks across multiple lists — styled in btop's structural language (rounded panels, gradient meters, braille) with a Catppuccin palette.

## Language

The vocabulary is deliberately close to Google's own model. The distinctions that bite — especially the four ways a Task can leave view — are made explicit below.

### Core model

**List**:
A Google TaskList; a named container of Tasks.
_Avoid_: folder, group, category, tasklist.

**Task**:
A single item in a List, and one of the three **Entry types** — the actionable one, as against an Event or a Note. Unqualified "Task" still means the actionable type; **entry** is the umbrella term when the type does not matter.
_Avoid_: todo, item.

**Subtask**:
A Task with a `parent`. Capped at **one nesting level** — a Subtask cannot itself have Subtasks — matching Google's own clients.
_Avoid_: child, nested todo.

**Status**:
The only two states a Task can be in: `needsAction` or `completed`. There is no "in progress," no priority.
_Avoid_: open, done-ish, state.

**Entry type**:
Which of Bullet Journal's three kinds an entry is: a **Task**, an **Event**, or a **Note**. Derived from the title's leading glyph, never stored as a field (ADR-0008) — `Task` is the default and carries no glyph.
_Avoid_: kind, category, entry kind.

**Event**:
An entry that happens on a day, written `○ ` before the title. Occupies the Due-load, never the Completion meter — an Event is not work you finish.
_Avoid_: appointment, meeting.

**Note** (the entry type):
A jotting, written `— ` before the title. Counted by neither the Completion meter nor the Due-load.
_Avoid_: notes (that is the field below), memo, comment.

**Notes** (the field):
Google's free-text body on a Task, edited with `n`. Unrelated to the **Note** entry type: a Note is what an entry *is*, notes are what it *carries*. Any entry type may have notes.
_Avoid_: description, body, note.

**Display title**:
A Task's title with its type glyph removed — what the pane shows and the editor opens on. Equal to the raw title for a Task. Note this means "prefix removed", not "glyph-free": a title Google stores in a non-canonical form (`○Standup`, no space) is read as an untyped Task and keeps its glyph on screen until `t` normalises it.
_Avoid_: clean title, stripped title.

**Due date**:
A **date, never a time**. Google's API discards the time portion, so oxidone never stores or shows a due time.
_Avoid_: deadline, due time, due-at.

**Today**:
The pinned, cross-List view of what is due — the sidebar's first row, always selectable, never a real List. Membership is `due <= today` (`domain::due_on_or_before`, the one definition, shared by the cache aggregate and the view filter); an **undated** entry is therefore never in it. A Completed row shows only if it was completed today, so the pane answers "among what was due, what got done". Flat and read-only in ordering terms: no Subtask nesting, no Manual lens (`position` is per-List, so a cross-List hand order is undefined), and a Manual lens carried in from a List is normalised to Due on entry. Renders as a **Journal spread**.
_Avoid_: today list, agenda, inbox, dashboard.

### Ordering

**Manual order**:
The user's hand-arranged sequence of Tasks (Google's `position`), shown as "My order" in the Google app. Written *only* by a Move.
_Avoid_: sort order, custom order.

**Sort view**:
A *local, read-only* regrouping of the visible Tasks (by due date, by title). Subtasks stay under their parent in every view; only the order of and within groups changes. Never mutates Manual order, never writes `position` or `parent`. (Ordinary edits — completing, retitling, deleting — write from any view; the *lens* is what writes nothing.) **Due** is the home state the app opens in.
_Avoid_: sort order.

**Move**:
Repositioning, reparenting, or **relocating** a Task (Google's `move` operation). The only action that writes Manual order or changes an existing Task's `parent`. Moves compute against stored order, so a Move pressed from a Sort view switches the pane back to Manual and stops — the next press performs the Move, against the adjacency now on screen.

Relocating (`M`, "move to list") is the third axis: the same operation with a `destinationTasklist`, sending a Task to another List. It writes no Manual order in the pane it leaves, so unlike the other Moves it neither needs nor switches the Sort lens, and it works in **Today** — where the source is the row's own List, not the selected one. The Task lands at the **top** of the destination, the one position Google permits for every Task including a Cleared one, and a Subtask arrives **top-level**: its parent stays behind and cannot follow.

A Task that still *has* Subtasks is refused. Google does not document whether children follow their parent across Lists, and a half-moved subtree cannot be undone — so oxidone declines rather than guesses. The refusal is decided by a live query with `show_hidden=true`, because a Cleared Subtask appears in neither the pane nor the cache.

### The four dispositions

Bullet Journal's daily review asks one question of every entry still `needsAction`: what becomes of it? These four answers are *not* the same list as **The four exits** below — two of them are not departures at all.

| Disposition | BuJo signifier | oxidone | Leads to |
| --- | --- | --- | --- |
| Complete | `X` | `Space` | the **Completed** exit |
| Scheduled | `<` | `d` | no exit — only the due date moves |
| Migrated | `>` | `m` | no exit — only the due date moves |
| Irrelevant | ~~strikethrough~~ | `x` | the **Deleted** exit |

Two traps worth naming. BuJo's `X` means *complete*; oxidone's `x` key means *delete*, which is the opposite — the two must never be conflated. And `>`/`<` are unavailable as bindings (they are Indent and Outdent), so migration binds `m`, the verb's initial.

**Migrate**:
Pushing an entry's due date to `max(today, due) + 1 day` — Bullet Journal's `>`. **Not an exit**: the entry stays `needsAction` and nothing but its due date changes. Repeated migrations compose, a day at a time. Refused on a Completed entry, where re-dating means nothing.
_Avoid_: defer, snooze, postpone, push (as a noun).

### The four exits

**Completed**:
A Task with `status=completed`. Still present and visible (struck-through/dimmed), just checked off.

**Cleared** (a.k.a. Hidden):
A Completed Task swept out of the active view via a Clear (`hidden=true`). Recoverable in Google; not destroyed.
_Avoid_: archived, hidden (as a verb).

**Deleted**:
A soft-deleted Task (`deleted=true`). A distinct fate from Cleared.

### Sync & local state

**Refresh**:
A manual pull from Google into the cache. Distinct from the (future) background poll.

**Pure mirror**:
The guiding constraint on the live-task cache: it models *exactly* what Google stores — no local-only fields, no augmentation. When Google clears or deletes a Task, the mirror drops it too.

**Dirty**:
A local change not yet confirmed by Google. Dormant in v1 (failed writes roll back); the seed of future offline editing.

**Completion log**:
A local, append-only record of completion events (`task_id`, `list_id`, `title`, `completed_at`), kept *separately* from the pure-mirror cache. Feeds future activity views. It is **per-machine and non-authoritative** — it does not sync across machines and is never Google's truth.

`title` holds the **Display title**, not the raw one: the log is human-readable history, not a mirror, so it records what an entry was called rather than the type encoding. Rows are keyed `(task_id, completed_at)` and written `INSERT OR IGNORE`, so first observation wins — a later retype or rename never reaches an already-logged row.

### Visual vocabulary

**Signifier**:
The glyph a row carries for its **Entry type** — `○ ` Event, `— ` Note, blank for a Task. Sits between the Subtask indent and the title, and degrades to `o `/`- ` under `ascii_fallback`. Absent entirely when every entry in view is a Task — except in the **Journal spread**, which reserves the cell always, so a title holds its column as Events and Notes enter and leave the day. That fixed position is what makes it a gutter there rather than a cell.
_Avoid_: bullet, icon, marker (a marker *trails* the title — the link `⧉` or the **Notes marker** `≡`).

**Journal spread**:
How **Today** is laid out: a **dateline** row (`Monday 20 July 2026`), then the entries under an **Overdue** and a **Today** group header. Three non-selectable rows in the ordinary task pane — the sidebar stays visible, the focus model does not fork, and the panel title still names the Sort view like every other pane.

Two rules that read alike and are not: an entry is in the **Overdue** group when it is dated strictly before today (`domain::due_before`, **status-blind** — a Completed overdue entry groups by its date like any other, which is what keeps the group a contiguous prefix of the pane and lets the renderer count it rather than partition). What the header *counts* is narrower: only the entries still `needsAction`, because the count answers the migration ritual's question — what is left to move. So `Overdue 1` above two drawn rows is right, not an off-by-one. At zero outstanding the count and its red both drop.

The due gutter exists here on exactly the Overdue group's condition, so the two appear and vanish together: with overdue entries the group prints its dates and a today-due row's cell is blank at the same width (titles stay aligned); with none there is no column at all.
_Avoid_: section, bucket, page, agenda.

**Notes marker**:
The `≡` a row carries when its Task's **notes** hold anything visible. Trails the title, after the link `⧉` and before the Subtask meter, and degrades to `=` under `ascii_fallback`. A body of only whitespace or invisible formatting draws nothing — the marker promises text `n` will show.

Not the same thing as an **Entry type** of `Note`, despite the word: that is a *signifier*, it *leads* the row, and it says what an entry **is**. `≡` says the entry **has a notes body**. The two are independent — a Note need not carry notes, and any entry type may (`— call the notary ≡`).
_Avoid_: note marker (ambiguous with the Entry type), notes icon.

**Notes preview**:
The first reader-visible line of a Task's **notes**, drawn dim at the very end of the row after every bounded widget (the `≡` marker, the link `⧉`, the Subtask meter). Shown only when the row can spare a minimum of cells for it; otherwise the `≡` marker stands alone. A line that is *nothing but* a URL collapses to that URL's authority (`https://a.dev/1` → `a.dev`) — the `⧉` already says a link is there, and the host is what a preview can usefully add. Layout-hostile characters (controls, a tab, the bidi format controls that would reorder the row) are replaced with a space before drawing; the preview keeps the row's strike on a Completed Task, unlike the meter.
_Avoid_: notes snippet, notes excerpt.

**Completion meter**:
A braille-cell progress bar of done ÷ total over **Task**-typed entries only — Events and Notes are not work you finish, and counting them would make the meter permanently under-report. Shown in the task-pane header, per List in the sidebar, and per parent Task for its Subtasks. Braille gives 8× horizontal resolution over a block bar.

The three agree for the **active** List, which derives its counts live from the loaded pane. A List you have not selected is counted in SQL over the mirror, which does not read the type prefix — so a background List holding Events or Notes reads high until you select it. Known seam, not a rounding error: teaching the query the encoding would be a second definition of it, free to drift from `EntryType::parse`.

**Due-load**:
A braille histogram of counts per upcoming day — the "workload ahead" strip. Counts Tasks and Events, not Notes. Deliberately narrower than the per-row due gutter, which shows a date for *any* dated entry: the gutter answers "does this carry a date?", the strip answers "how much is coming?".
