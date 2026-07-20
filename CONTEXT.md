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

**Completion meter**:
A braille-cell progress bar of done ÷ total, shown per List and per parent Task. Braille gives 8× horizontal resolution over a block bar.

**Due-load**:
A braille histogram of Task counts per upcoming day — the "workload ahead" strip.
