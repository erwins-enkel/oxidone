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
The pinned, cross-List view of what is due — the first of the sidebar's two pinned rows (the **Weekly spread** is the second), always selectable, never a real List. Membership is `due <= today` (`domain::due_on_or_before`, the one definition, shared by the cache aggregate and the view filter); an **undated** entry is therefore never in it. A Completed row shows only if it was completed today, so the pane answers "among what was due, what got done". Flat and read-only in ordering terms: no Subtask nesting, no Manual lens (`position` is per-List, so a cross-List hand order is undefined), and a Manual lens carried in from a List is normalised to Due on entry. Renders as a **Journal spread**.

Answers *what am I doing now*; the **Weekly spread** answers *what am I doing this week, and on which day*. The two never coexist — each has its own sidebar row, and landing on Today takes the spread down — and both read the same due date.
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

Relocating (`M`, or the **Omnibox**'s MOVE band, "move to list") is the third axis: the same operation with a `destinationTasklist`, sending a Task to another List. The picker and the band offer the same destinations and refuse the same Tasks in the same words — one definition of each, so the two surfaces cannot drift into disagreeing about what is movable. It writes no Manual order in the pane it leaves, so unlike the other Moves it neither needs nor switches the Sort lens, and it works in **Today** — where the source is the row's own List, not the selected one. The Task lands at the **top** of the destination, the one position Google permits for every Task including a Cleared one, and a Subtask arrives **top-level**: its parent stays behind and cannot follow.

A Task that still *has* Subtasks goes too, and takes them with it. **Google moves the subtree intact**: the children follow their parent into the destination, still naming it, and every `id` survives the move (verified 2026-07-21 — one account, two runs of two fixtures each; see #86). oxidone used to refuse such a Task, on the reasoning that a half-moved subtree could not be undone; nothing is half-moved, so the rule and the live query that decided it are gone (#93) — a relocation is now one request.

Only the parent comes back in the reply, so both mirrors follow the children themselves: the cache relocates their rows by `parent`, and the pane drops them alongside their parent when the Move confirms. A **Cleared** Subtask follows on Google but is in neither surface to begin with, so it needs no local repair — and the children's `position` stays a source-List value until the next Refresh, since reading the destination's numbering back would cost the very round trip that was just removed.

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
The glyph a row carries for its **Entry type** — `○ ` Event, `— ` Note, blank for a Task. Sits between the Subtask indent and the title, and degrades to `o `/`- ` under `ascii_fallback`. Absent entirely when every entry in view is a Task — except in the **Journal spread** and the **Weekly spread**, which reserve the cell always, so a title holds its column as Events and Notes enter and leave the day. That fixed position is what makes it a gutter there rather than a cell.
_Avoid_: bullet, icon, marker (a marker *trails* the title — the link `⧉` or the **Notes marker** `≡`).

**Journal spread**:
How **Today** is laid out: a **dateline** row (`Monday 20 July 2026`), then the entries under an **Overdue** and a **Today** group header. Non-selectable rows in the ordinary task pane — the sidebar stays visible, the focus model does not fork, and the panel title still names the Sort view like every other pane.

A group header is drawn only when its group has entries, so the spread is at most three such rows and rarely all three: a clean morning is the dateline and `Today`, an empty day the dateline alone. The dateline is always drawn — it is the page, not a label for the rows.

Two rules that read alike and are not: an entry is in the **Overdue** group when it is dated strictly before today (`domain::due_before`, **status-blind** — a Completed overdue entry groups by its date like any other, which is what keeps the group a contiguous prefix of the pane and lets the renderer count it rather than partition). What the header *counts* is narrower: only the entries still `needsAction`, because the count answers the migration ritual's question — what is left to move. So `Overdue 1` above two drawn rows is right, not an off-by-one. At zero outstanding the count and its red both drop.

The due gutter exists here on exactly the Overdue group's condition, so the two appear and vanish together: with overdue entries the group prints its dates and a today-due row's cell is blank at the same width (titles stay aligned); with none there is no column at all.
_Avoid_: section, bucket, page, agenda.

**Weekly spread**:
The planning surface: a **Day grid** of Monday–Friday columns beside the rows, in which a **Dot** marks the day an entry is planned for. Opened with `W` (`w` is the distant filter), by moving the sidebar cursor to the **Week row**, or from the **Omnibox** — whose JUMP band offers that row beside Today while `:week` fires `W`.

Two blocks: **Unscheduled** is the undated entries still `needsAction`, **Week** is those dated within the five days on display. Nothing else is in it — an entry dated Saturday, Sunday or before Monday is simply absent, since there is no column to draw it in and overdue is **Today**'s work. Flat, like Today, and its order is fixed (pool in Manual order, then by day), so the Sort lens is refused rather than silently ignored.

The sidebar row is what **scopes** it, and the row is likewise the source of truth for whether the spread is up at all — so the highlighted row always names what the task pane shows. On a List, both blocks are that List's. On the pinned **Week row** the Week block spans every List, while the pool stays the one `default_list` names: the pool is where `a` captures, and it needs one unambiguous target. Landing on **Today** takes the spread down, Today being a pane of its own; walking between Lists re-scopes it and stays in it. `W` follows from that: on a List it flips the lens without moving the cursor, and on either pinned row it walks between the two.

Reads *every* List whatever its scope — scoping is a filter, not a narrower fetch, so walking between Lists costs nothing — which is why it takes the same whole-corpus load Search does, and the same pending notice: an incomplete corpus must never read as a week with nothing planned.
_Avoid_: week view, planner, calendar, agenda, board.

**Week row**:
The second of the sidebar's two pinned rows, below **Today** and above the Lists. A cursor stop like Today, and the one place the **Weekly spread** spans every List. Lit in the accent whenever the spread is up — including a List-scoped week, whose cursor sits on the List rather than here — which is independent of the cursor highlight the row carries when the cursor is on it.
_Avoid_: week tab, week entry, all-lists week.

**Day grid**:
The five fixed-width cells trailing each row of the **Weekly spread**, one per weekday, under a `Mo Tu We Th Fr` header. Its width is reserved unconditionally — a narrow pane clips the title, never the columns, because the grid *is* the view. Today's column is accented, and only while the week on screen contains today.

A cell is `·` empty, `•` planned, or `✕` planned-and-completed, degrading to `.`/`*`/`x` under `ascii_fallback`. The **Day cursor**'s cell is bracketed (`[•]`) rather than coloured, so it survives the selected row's reverse.
_Avoid_: table, calendar, matrix, tracker.

**Dot**:
The `•` in a **Day grid** cell: the day an entry is planned for. It *is* the entry's **Due date** — placing one writes `due`, clearing one clears it — so a plan syncs to every other client and shows up in **Today** on its day. Not a local-only field; ADR-0003 stands.

One per row, which falls out of the model rather than being enforced: a Task has one due date. Completing it crosses it to `✕`, which is Bullet Journal's own gesture and the reason the spread always draws Completed rows in its Week block.
_Avoid_: mark, bullet (that is the **Signifier**'s family), pin, flag.

**Unscheduled pool**:
The **Weekly spread**'s first block: the undated, still-`needsAction` entries of the List the sidebar cursor names, or of `default_list` on either pinned row — the week's brain-dump, and where `a` captures to. Single-List even on the **Week row**, where the Week block is not: a capture surface needs one target. The status clause bounds it: unlike the Week block, whose `✕` rows are bounded by five days, the pool has no window to age old completions out of.
_Avoid_: backlog, inbox, staging, unplanned.

**Day cursor**:
Where the **Weekly spread** is aimed: **home** (on the title) or one of the day columns. `h`/`l` walk it, and `h` falls through at Monday — home, then the sidebar — so "left, eventually out of the pane" still holds.

Home is what keeps `Space` whole. `Space` acts on the cell under the cursor: empty schedules, the row's own **Dot** completes, a `✕` un-completes — and at home it is the ordinary completion key, which is the only way to finish a row still in the **Unscheduled pool**, since that row has no dot cell anywhere.
_Avoid_: selection, caret (that is the text inputs'), cell cursor.

**Omnibox**:
The modal surface `p` opens: one query over a grouped result list, offering the
two pinned rows and the Lists to **jump** to, the **commands** (keyless, bar
`:refresh` and `:week`, which `r` and `W` also fire), a hand-off to **Search**,
**moving** the selected Task to another List, and — pinned last — **capturing**
the query as a Task. `Enter` runs whichever row is highlighted, so what it will do
is legible before it is pressed, and a write is never what happens by default.

Its five bands are **groups**, in the **Journal spread**'s sense — a labelled run
of rows under a group header, exactly as **Overdue** and **Today** are there.
Not the sense the **List** entry avoids: a List is never called a group, and the
JUMP group is a grouping of *rows* that happen to name Lists.

MOVE is the one band a query has to *ask* for: it draws only where the query's
first word is a non-empty prefix of `move`, one row per destination the **Move**
entry admits, filtered by whatever follows the verb (`:move ho` → `→ home`). A
band drawn on every query would put a write in the middle of the list — and
`move` is not an **Omnibox** command in the `:refresh` sense: it names a band of
rows rather than one row with an argument. The two write bands, MOVE and CAPTURE,
come last for the same reason, and the SEARCH row above them — offered for every
non-empty query — is what keeps either off the row `Enter` starts on.

The *setting* commands are **session-only** — they change the running app, never
`config.toml` — which the COMMAND group header says once rather than every row
repeating it. The header scopes that caveat to the settings because `:refresh`
and `:week` share the group and set nothing: they act, and there is no value of
either to outlive the session.
_Avoid_: command bar, palette (that is Catppuccin's), launcher, fuzzy finder,
section.

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
