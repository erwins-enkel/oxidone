# Entry type encoded in the Task title

oxidone marks an entry as a Bullet Journal **Task**, **Event**, or **Note** by a
glyph at the front of Google's own `title`: `○ ` for an Event, `— ` for a Note,
nothing for a Task. The type is *derived* on read and *rebuilt* on write; no
local field stores it.

`Task` being the prefix-free default is what makes this cheap: every Task that
already exists, and everything created in Google's web and mobile clients, is
already correctly typed. There is no backfill and no migration.

## Why not a sidecar store

ADR-0003 forbids it outright, and the one narrow exception — ADR-0007's
completion log — was bought by that log being *non-authoritative history*.
Entry type is not history: it drives rendering and it drives the Completion
meter and Due-load counters. The user runs oxidone on several machines, so a
sidecar would make an entry a Note on one and a Task on another. That is a
defect, not the accepted per-machine partiality of the log.

## Why `title` and not `notes`

ADR-0003 sanctions encoding "as text in the `notes`/`title` field" — **both** —
so using the escape hatch is not itself the decision. Between the two:

- `notes` is free-form prose the user edits in `$EDITOR` (`n`). It is a field
  whose entire point is that oxidone does not own its structure; one stray edit
  and the type is gone, indistinguishable from an entry that never had one.
- `notes` is frequently `None`. Typing an entry would materialise a notes body
  that exists only to hold a machine marker — and Google's clients show a notes
  indicator on such entries, so every typed entry would sprout a false "has
  notes" hint.
- `title` is short, structured, always present, and already the thing the user is
  naming. A leading glyph reads as annotation there, which is exactly how Bullet
  Journal writes it on paper.

## Three functions, three jobs

The encoding is safe because repair lives in exactly one place.

- **`parse`** classifies and never rewrites. A title is typed only on an exact
  glyph, then one space, then a non-empty remainder; anything else is a `Task`
  whose display title is the raw title verbatim.
- **`apply`** prefixes and never strips. This is the ordinary edit path's
  function: opening `e` on a title and pressing Enter unchanged must write it
  back byte-identical, foreign glyph and all.
- **`retype`** strips a leading run of glyph characters and whitespace, then
  rebuilds the prefix. It is the only stripping function and `cycle_type` is its
  only caller.

Raw titles do not all come from oxidone. Google may hold `○Standup` (no space),
`○  Standup` (two), or a bare `○` — each parses as an *untyped* Task whose
display title still leads with the glyph. Prefixing those directly would stack
into `○ ○Standup`. Because `retype` rebuilds rather than appends, stacking is
unreachable rather than guarded against, and `t` self-heals a foreign title into
a canonical one on first press.

`retype` returns `None` when nothing survives the strip, so an entry with no
nameable content simply cannot be typed — unrepresentable rather than a
documented precondition. (An empty title is reachable: `WireTask.title` is
`#[serde(default)]`.)

## Consequences

- **Google's own apps show the raw glyph.** A typed entry reads as `○ Standup` on
  the web and on the phone. This is the deliberate price of a type that syncs.
- **`t`/`T` are not lossless-title operations.** `retype` strips a leading
  glyph/whitespace run, so `—— dashes` becomes `○ dashes` and cycling will not
  bring the dash back. Bounded to those two keys: `retype` has one caller, and
  the `e` edit path uses `apply` and rewrites nothing.
- **A title that legitimately starts `○ ` or `— ` is read as typed.** Rare,
  visible in the signifier column, and one `t` press from correction.
- **Upgrade note, and it is the quiet one.** That same reclassification moves an
  entry out of the Completion meter's denominator and out of the Due-load. On
  first run after this change, a pre-existing task titled `— follow up` stops
  being counted, with nothing on screen announcing it. Accepted: counting Notes
  would reintroduce a meter that reads `4/3`, and a one-off migration pass would
  mean writing to Google on upgrade. It affects only titles already starting with
  a glyph plus one space, and one `t` press restores it.
- **The persisted glyph never varies with `ascii_fallback`.** That flag degrades
  *rendering* to `o `/`- `; `apply` always writes the Unicode glyph. An ASCII
  prefix on the wire would have to be parsed back too, and toggling the flag
  would otherwise silently revert every typed entry to `Task`.
- **`○` and `—` are East Asian Ambiguous.** A terminal rendering Ambiguous as
  double-width shifts signifier rows by a column. `ascii_fallback` is the
  supported remedy — consistent with ADR-0006, which already assumes a capable
  terminal for braille.
- **The completion log stores the display title**, not the raw one: it is
  human-readable history (ADR-0007), not a mirror. `INSERT OR IGNORE` on
  `(task_id, completed_at)` means first observation wins, so a later retype or
  rename never reaches the logged row.
- **Type-aware counting stops at the SQL boundary.** The task-pane header, the
  active List's sidebar meter and the Subtask meters all count Task-typed entries
  only, and agree because all three derive from the loaded pane. Sidebar counts
  for *other* Lists come from a `GROUP BY` over the mirror (#64), which does not
  read the prefix, so a background List holding Events or Notes reads high until
  you select it. Teaching that query the encoding would put a second definition
  of it in SQL — and a `LIKE '○ %'` would not even agree with `parse`, which
  rejects a glyph with an empty remainder. Recounting every List in Rust instead
  would undo the indexed scan #64 chose deliberately. Left as a seam.
- **Nothing fails the build if a new `.title` read site is added.** The rule is:
  a `.title` site is raw unless it renders, sorts, prompts, or logs. Today that
  is enforced by a test per known site plus review. If a missed site ever ships,
  the escalation is to make `Task.title` private behind `raw_title()` /
  `display_title()` so the compiler rejects the ambiguity — deliberately not done
  here, as it touches every construction site across `cache`, `api`, and `sync`.
