# oxidone

A single-user terminal (TUI) client for Google Tasks, in Rust. It is a *daily-driver* cockpit — you live in it to triage, create, complete, and reorder tasks across multiple lists — styled in btop's structural language (rounded panels, gradient meters, braille) with a Catppuccin palette.

## Language

The vocabulary is deliberately close to Google's own model. The distinctions that bite — especially the four ways a Task can leave view — are made explicit below.

### Core model

**List**:
A Google TaskList; a named container of Tasks.
_Avoid_: folder, group, category, tasklist.

**Task**:
A single item in a List.
_Avoid_: todo, item, entry.

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

### Ordering

**Manual order**:
The user's hand-arranged sequence of Tasks (Google's `position`), shown as "My order" in the Google app. Written *only* by a Move.
_Avoid_: sort order, custom order.

**Sort view**:
A *local, read-only* regrouping of the visible Tasks (by due date, by title). Subtasks stay under their parent in every view; only the order of and within groups changes. Never mutates Manual order, never writes `position` or `parent`. (Ordinary edits — completing, retitling, deleting — write from any view; the *lens* is what writes nothing.) **Due** is the home state the app opens in.
_Avoid_: sort order.

**Move**:
Repositioning or reparenting a Task (Google's `move` operation). The only action that writes Manual order or changes an existing Task's `parent`. Moves compute against stored order, so a Move pressed from a Sort view switches the pane back to Manual and stops — the next press performs the Move, against the adjacency now on screen.

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

### Visual vocabulary

**Signifier**:
The glyph a row carries for its **Entry type** — `○ ` Event, `— ` Note, blank for a Task. Sits between the Subtask indent and the title, and degrades to `o `/`- ` under `ascii_fallback`. Absent entirely when every entry in view is a Task.
_Avoid_: bullet, icon, marker (that is the link `⧉`).

**Completion meter**:
A braille-cell progress bar of done ÷ total over **Task**-typed entries only — Events and Notes are not work you finish, and counting them would make the meter permanently under-report. Shown today in the task-pane header for the active List; per-List sidebar meters and per-parent Subtask meters are follow-ups. Braille gives 8× horizontal resolution over a block bar.

**Due-load**:
A braille histogram of counts per upcoming day — the "workload ahead" strip. Counts Tasks and Events, not Notes. Deliberately narrower than the per-row due gutter, which shows a date for *any* dated entry: the gutter answers "does this carry a date?", the strip answers "how much is coming?".
