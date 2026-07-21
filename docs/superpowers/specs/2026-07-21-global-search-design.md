# Global Search — design

A cross-List Search pane, opened with `S`, alongside the per-List `/` filter
that landed in #96.

## Problem

`/` narrows the current pane. There is no way to answer "where did I put that
Task?" without walking every List by hand. Today is the only cross-List view,
and it answers a different question (`due <= today`), so an undated Task in a
List you have not opened is unreachable.

## Shape

**Search is a pane, not an overlay.** It holds the cross-List corpus in
`model.tasks` exactly as Today holds its aggregate. Rows are actionable in
place — you complete, retitle, reschedule and relocate from the results, never
bouncing back to a home List first.

That is a **requirement, not a default**. The alternative — a picker overlay
holding the results and jumping to the chosen row's List — is materially
cheaper: it needs none of the per-List write paths, load guards or layout
splits below, because it never puts foreign rows in `model.tasks` at all. It
was offered and declined: triaging results *across* Lists without entering each
one is the feature. The picker remains the fallback design if that ever stops
being worth its cost.

**Search is the corpus times the existing filter.** The query is
`Model::filter`; the predicate is `matches_filter`. Nothing new is introduced
for matching. The Search pane differs from a List pane in exactly one respect:
what is loaded into `tasks`. That is why an empty query showing everything
falls out for free rather than needing a special case — `matches_filter`
already treats an empty query as "match all".

## Model

**`Model::search: bool`, orthogonal to `Selection` — not a third `Selection`
arm.** `Selection` is the *sidebar cursor*: every consumer asks "which row is
highlighted, and which List do sidebar verbs act on". Search highlights no row
and is not a List, so an arm would force a meaningless answer at four sites
that have nothing to do with searching — the sidebar highlight, sidebar
navigation, selection clamping, and `focused_list` (which backs `R`/`X`) — and
would change `domain::Selection` besides.

With the flag, the sidebar cursor simply never moves, which *is* the "parked,
dimmed" behaviour we want; exiting is clearing the flag and re-requesting the
pane `selected` still names; and no "where did I come from" field is needed at
all.

The trade is real, and it is not the exhaustiveness one it first appears to be.
The flag's actual cost is that a parked `Selection` keeps answering
`selected_list_id()` for a pane that is not on screen, so gates asking "are
these rows for the visible pane?" quietly say yes; that is what every guard in
the plan's contract corrects. Its compensating advantage is that the parked
cursor *is* `selected`, so every existing path that repairs a selection — after
a List is deleted, renamed or fails to insert — maintains it for free. An enum
arm would fix the first by moving the cursor into a second field that nothing
maintains, trading one defect family for another of similar size. Coverage is
therefore a review obligation, discharged by enumerating every pane-kind branch
(see the plan) and testing each.

`PaneKey::Search` *is* added: that enum is a third **pane identity** — the key
pairing an optimistic cross-List Move with the reply that repairs it — not a
sidebar cursor.

### Parking the cursor aliases Today

Parking `selected` has a consequence worth naming, because it is the subtle
failure the flag invites. Entering Search **from Today** leaves
`selected == Selection::Today`, so `Model::today_active()` stays true and every
Today-shaped branch fires *inside Search* — filtering the corpus to
`due <= today`, hiding earlier-day Completions, capturing with a forced due
date, and shadowing the pane's own load branch.

The fix belongs at the definition, not at the call sites. `today_active()`
means **strictly Today**, and a third predicate names the shared axis:

- `today_active()` — `matches!(selected, Today) && !search`. The Today-only
  rules: `due <= today` membership, completion-day hiding, the journal spread.
- `search_active()` — the flag. The Search-only rules: the verb refusals and
  the load-message guards.
- `flat_pane()` — either. The *flat cross-List* axis, and nothing else:
  ordering, the `s` cycle, and the view's `flat` layout.

Three predicates, one axis each, mirroring the view's `flat`/`spread` split.
Consumers that must ask "is this pane flat?" ask `flat_pane()`; consumers that
must ask "is this Today?" get a strict answer.

## Keys

| Key | In Search |
| --- | --- |
| `S` | enter Search (from any pane); reopen the query input if already in it |
| `/` | reopen the query input — the same input `S` opens |
| typing | narrows live, per keystroke, locally |
| `Enter` | close the input, keep pane and query; browse with `j`/`k` |
| `Esc` | leave Search, restore the prior pane — from the input or from the pane |

`Esc` leaves outright rather than stepping down a ladder: the corpus with no
query is not a state worth stopping at, and `Enter` already covers "keep the
query, put the input away".

`S` is unbound today. Its cheatsheet help is `search all lists`. Like `/`, it
gets no always-visible legend cell — the 80-column TASKS row is full.

## Data

`Command::LoadSearch { lists }` mirrors `Command::LoadToday`:

1. Instant paint from the cache — a new `sync::all_from_cache`, the same
   borrow-based, spawn-free shape as `sync::today_from_cache` (which is
   `cache.all_tasks()` plus a date filter; Search is `cache.all_tasks()`
   unfiltered).
2. Online, the concurrent per-List fan-out, repainting via
   `Message::SearchLoaded { tasks, failed, live }` — see the pending notice
   below for what `live` carries.

The command carries no query: the corpus is query-independent, so it fires once
on entry and on `r`, never per keystroke. Typing never touches the network.

`spawn_load_today` and the Search fan-out are **the same worker** —
generalised (parameterised by the final cache read and the message it sends),
not copy-pasted. The fan-out already fetches with `show_completed=true,
show_hidden=false`, so Completed rows reach the cache and Cleared rows do not,
which is exactly what Search wants.

`set_search_tasks` mirrors `set_today_tasks`: it keeps the cursor on the same
Task by id, and applies both of that function's suppressions — see "What the
corpus suppresses" below, which also settles why it never retires a tombstone.

**Nothing scoped to a single List may write the corpus while Search is active.**
A dozen paths ask the same question to decide whether incoming rows belong to
the pane on screen — "is this the selected List?" — and in Search every one of
them got the wrong answer, because the parked cursor still named the List you
opened Search from.

The fix is at the accessor, not at the dozen call sites: **`selected_list_id()`
returns `None` while Search is active**, as it already does for Today. Search is
not a List, so a `Some` there was the model asserting something false, and every
gate built on it was correct the moment it stopped. That covers the per-List
fetch path, the in-list Move's reconcile and rollback, both Clear repairs, the
sidebar meter, and the List create/fail repairs — including several that were
never spotted while enumerating them by hand.

Discarding those repairs in Search costs nothing: the authoritative order is
already mirrored into the cache, and the optimistic reorder they undo was never
written there, so leaving Search reloads the List correctly either way.

What the accessor cannot do is speak to the user. Sites that owe a refusal
message, a pane identity, or a capture target keep explicit Search arms — an
inert `None` would make them silent no-ops, which is the failure this design
refuses everywhere else.

This is the general rule, not a special case: a pane-kind check protecting state
that many callers reach belongs at the choke point they share. Applied three
times now — to `today_active()`, to the per-List write path, and here.

The same reasoning settles the pane-switch machinery. `clear_pane` — "the pane
is being replaced, so wipe it and drop the query" — is a *List-pane* concept.
The corpus is every cached Task regardless of which List is parked, so no
List-set change invalidates it, and Search simply opts out: its branch ignores
the flag and the query survives. Without that, deleting the parked List, or any
List create/delete reply landing mid-Search, would leave the user searching with
their query silently gone. Entry and exit then clear what they own explicitly.

### What the corpus suppresses

Two kinds of row are held out of the corpus, both inherited from the Today
aggregate and neither safe to leave implicit.

A Task **deleted or swept this session** is remembered by its tombstone until
Google's list endpoint stops returning it. Without honouring that, deleting a
Task in a List and then opening Search brings it back — from the cache, or from
a fan-out whose request predated the delete. The fetch's "no hidden rows" flag
does not cover this: it excludes *Cleared* Tasks, not ones whose deletion is
still in flight.

A Task with a **cross-List Move in flight** is held out entirely, appearing in
neither its old List nor its new one until the reply lands. Today reasons the
same way, and the corpus makes the case stronger rather than weaker: it holds
both Lists at once, so a row shown mid-move would be shown under a List it may
already have left.

Search **never retires** a tombstone. Today declines to for a reason that does
not apply here — its aggregate is date-filtered, so an absent row may simply be
undated. The corpus is unfiltered, so absence really would mean the row is gone.
The reason it must still not retire one is different: retiring a tombstone
requires the evidence that *a fetch of that List* omitted the id, and the
reducer only ever receives a flat cross-List aggregate. Acting on it would drop
the guard against a race that was never observed, leaving an in-flight per-List
fetch free to resurrect the row in its own pane. The ordinary per-List path
still retires it the next time that List loads.

### The corpus is incomplete until the fan-out lands, and says so

Because the cache paint comes first, a List never mirrored on this machine
contributes nothing until the live result arrives — so a query whose only match
lives there would render an empty pane, answering "not here" when it means "not
yet". For a retrieval surface that is a fail-open result, exactly what the house
rules forbid. The load message therefore distinguishes its two sends, and the
pane carries a pending notice until the live one lands. Offline the cache read
*is* the final answer, so it is sent as live and the notice never appears.

The notice is **derived from its own field**, not written to the status line.
The status line is a single overwrite-anywhere slot, and Search adds several
refusal messages of its own; any of them landing mid-load would erase the notice
and leave a half-loaded corpus reading as a complete one. A guarantee that
exists to fail closed cannot sit somewhere any other message may clear. The
field is cleared only by the live result and on leaving Search, and the header
renders it beside the pane name — unconditionally, since it changes what the
rows on screen *mean* rather than adding information about them.

## Ordering

Search and Today share one flat cross-List ordering seam. Today's
`today_ordered` becomes `cross_list_ordered` and `SortView::next_today` becomes
`next_flat` — names kept honest now that two panes use them. (`next_flat` lives
in `src/domain/mod.rs`, and its rename edits the unit test there.)

Order: overdue as a contiguous prefix, then due ascending with undated last,
List title then `position` as tiebreaks; the Title lens sorts on display title
with the same tiebreaks. Flat — no Subtask nesting, since parent/child grouping
is a per-List concept. `Manual` is normalised to `Due` on entry and excluded
from the `s` cycle, because cross-List `position` is undefined.

Search renders as the **ordinary flat pane**. The Journal spread stays
Today-only: its headers are due-based, and undated rows dominate a search, so
reusing it would file rows under headers that do not describe them.

## Behaviour parity

Search behaves as Today does except where noted.

- `Space`, `e`, `n`, `d`, `m`, `x`, `t`/`T` — work in place, against the row's
  own List.
- `M` (relocate) — works; the source is the row's own List, not a selected one.
  Both replies need Search arms. On **success** the row must stay in the
  corpus — it still exists, merely under a new List, and the fresh source
  tombstone would hide it — so Search takes Today's bridge re-insert and
  re-issues its own pane load. On **failure** the rollback is keyed by pane
  identity, which is what `PaneKey::Search` is for.
- `J`/`K`/`>`/`<` (Manual Moves) — refused **with a status line**, as in Today.
  `move_preconditions` needs an explicit Search arm: left alone, its
  `selected_list_id()?` returns `None` in Search and the keys become a *silent*
  no-op, which is the failure mode the house rules call failing open.
- `c` — reveals Completed, and is the *only* thing that governs Completed in
  Search. Today's extra "completed today only" rule does not apply: it exists
  to answer "among what was due, what got done", which Search does not ask.
  `within_today` and `within_completion_day` are gated on `today_active()` —
  which delivers this **only once that predicate is strict**. Left as-is, a
  Search opened from Today would filter the corpus to `due <= today` and hide
  earlier-day Completions, contradicting both this and "empty query shows
  everything".
- `w` — **refused with a status line**, and the toggle is left unflipped. The
  horizon does not apply in Search (below), so a working `w` would change
  nothing visible while still flipping `hide_distant` — and that flip lasts the
  session, seeded from config at startup and never written back, so it would
  silently narrow the List pane the user returns to. A view toggle that quietly
  reconfigures a *different* pane is worse than one that says no.
- `A` (add List) — **leaves Search**, landing in the List just created. It
  moves the cursor deliberately, as `j`/`k` do, so it drops the flag and the
  query with it rather than being refused.
- The horizon (`hide_distant`) — **does not apply in Search.** The one view toggle
  Search is exempt from, which needs saying because it looks inconsistent
  beside `c`. The rule across the whole `is_visible` chain: Search inherits the
  filters that select by **status**, and is exempt from those that select by
  **date**. The horizon is a triage filter — "what is near" — while Search is a
  retrieval surface — "where is it"; a retrieval that presumes a date range
  cannot answer its own question. Since `hide_distant` is off by default,
  inheriting it would hide far-future Tasks from search only for the users who
  turned it on, which is precisely the kind of defect that ships unnoticed.
- `a` — captures into the resolved default List, **undated**. Both
  `open_add_task` and `finish_add_task` branch on `today_active()` then
  `selected_list_id()`, so without a Search arm in each, `a` silently does
  nothing. Today's `due.or(today)` default is a Today rule — it keeps the new
  row inside `due <= today` membership; Search has no membership to preserve,
  so forcing a due date would invent a schedule the user did not ask for. A
  trailing parsed date is honoured as everywhere, and an unresolved
  `default_list` fails closed with Today's message.
- `r` — re-issues `LoadSearch` and keeps the query. This needs a **Search
  branch in `request_selected`**, placed before the `selected_list_id` match:
  left alone, Search falls to the `None` arm, which clears `tasks` — so `r`
  would blank the pane. The filter itself survives because `request_selected`
  drops it only on `clear_pane`, which Refresh does not set.
- `o` (add Subtask) and `C` (Clear Completed) — **refused with a status line.**
  `o` for Today's reason: Subtasks are per-List and `position`-shaped, and
  Search is flat and cross-List. `C` because it is destructive and neither
  existing scoping is defensible here — the Today path would sweep every
  Completed row in the corpus across every List, the List path would sweep the
  *parked* List, and neither is what the pane draws. A sweep scoped to
  "everything you searched" is unbounded, and the confirmation's count could
  not honestly describe it.
- Selecting a List in the sidebar leaves Search through the ordinary
  `request_selected` path.

## Entering and leaving

Pressed while Search is **already** active, `S` re-opens the query input over
the existing query and does nothing else — no clearing, no reload. That is what
`/` already does here, so the two are the same key once Search is open; running
the full entry again would discard what the user typed and re-fetch every List.

Otherwise `S` sets the flag, routes through the ordinary pane-switch path (clearing
`tasks` and `selected_task` and emitting the load), sets `Focus::Tasks`, and
seeds the empty query into the `/` input. The query is seeded *after* the
switch path runs, or its `clear_pane` filter drop wipes it.

Clearing `selected_task` is load-bearing: it is a raw index into the parked
pane's `tasks`, and reinterpreting it against a corpus of different length and
order would leave the cursor on an unrelated Task or out of range. It
re-anchors when the corpus arrives, as on any pane switch.

**While the query input is open there are no key bindings.** The reducer routes
every key to the overlay before consulting the keymap or the focus, so `j`,
`?`, `c` and the rest type themselves into the query; only `Enter`, `Esc`,
`Backspace` and printable characters mean anything. Every verb described below
is therefore an input-*closed* behaviour. `Focus::Tasks` matters for that same
state: after `Enter`, `j` must steer the pane you are reading rather than the
sidebar you entered from.

`Esc` exits Search in **one press, at every layer** — from the open input, and
from the pane after `Enter` has put the input away. Both layers already have an
`Esc` meaning (clear the query and close the input; clear a persisted filter),
and in Search both defer to exiting. The cheatsheet is the one exception: with
the input closed it is drawn on top, so `Esc` closes it and leaves you in Search.

`j`/`k` with the sidebar focused also leave Search — that is the "open the List
instead" gesture, and it drops the flag where the cursor moves, not in the
shared reload path (which `r` depends on to *keep* Search). So does `A`, which
moves the cursor into the List it creates. Merely changing focus (`Tab`, `h`,
`l`) keeps Search and the results on screen.

All three exits share one teardown, clearing the flag **and** the query. The
query has to be cleared there rather than left to the reload path, because `A`
bypasses that path entirely — otherwise a stale search query would narrow the
brand-new empty List.

Leaving clears the flag and re-requests the pane `selected` still names. The
sidebar cursor is restored exactly, since `selected` never moved. The pane's
own `/` filter is **not**: a query persisted against the List you left is
overwritten on entry and dropped on exit, the same way every pane switch drops
it — a query typed against one pane must never silently narrow the next.

## UI

Pane title reads `SEARCH`; `header_title` already appends `  /query` plus a
caret while the input is open, so the query display needs no new code.

### `today_view` splits into two axes

The view's `today_view` flag drives eight decisions on one boolean. It is
**split**, not renamed — reusing it wholesale for Search would ship two bugs.

- **`flat`** (Today **or** Search) — cross-List pane: no Subtask indent, no
  Subtask meter, and a muted List-name column.
- **`spread`** (Today **only**) — the journal spread and what serves it: the
  overdue-row count, the spread call, the always-reserved signifier gutter, and
  the two column rules.

The two bugs the split avoids:

- **The due gutter.** Today conditions it on `overdue_rows > 0`, because every
  Today row is dated and the column exists on the Overdue group's condition.
  Search must use the ordinary "anything in view is dated" rule — otherwise a
  result set with nothing overdue draws **no due column at all**, hiding every
  date in the pane.
- **`prints_date`.** In Today's spread only the Overdue group prints a date;
  today-due rows are deliberately blank because the `Today` header said it.
  Search must **always** print, or every non-overdue row renders as a blank
  12-cell column.

Signifiers use the ordinary "anything in view is typed" rule in Search — Today
reserves the gutter always because the spread's rows shift, which Search's do
not.

### The footer legend

The always-visible legend must not promise a key Search will not honour. Its
filter context reads `Esc clear`, which is false here — `Esc` leaves Search from
the open input — so Search gets its own variant reading `Esc leave search`. The
`/` legend in a List pane is untouched.

`S` itself claims **no** always-visible cell, only the `?` cheatsheet. The task
row's cells already fill its 80-column budget exactly through `c completed`, so
any cell added at or above that evicts one; `/` was held out for the same reason
when it landed. While Search is active the pane title says `SEARCH`, which is
where a user learns they are in it.

With the input closed the ordinary legend applies and stays honest: every verb
it advertises works in Search, and the verbs Search refuses have no cell.

### Header widgets are suppressed in Search

`header_title` computes the completion meter and due-load strip over
`model.tasks` — in Search, the whole corpus. Both are dropped:

- The meter would read `412/1180` while `/tax` shows three rows. It ignores
  view filters in every pane by documented design, so recomputing it over the
  visible set would diverge from that rule rather than fix it — and a
  whole-corpus completion ratio is not a fact about the pane you are reading.
- The strip forecasts a workload; "every Task in every List" is not one.

Today already drops the strip for the same class of reason.

## Testing

The cross-List fan-out has no coverage today: it lives wholly in `src/main.rs`,
which has no test module, so generalising it would be an untested change to a
working Today path. Its spawn-free core — mirroring, failure attribution, and
the aggregate read that differs between the two panes — moves into `sync`, where
a boundary suite can drive it against the fake API and a real cache. The
`tokio::spawn` fan-out stays in `main.rs`, as the codebase requires, and is
reduced to branch-free glue. Today's fan-out gains its first coverage as a side
effect.

`tests/search_reducer.rs`, against the in-memory fake API:

- **enter from Today**: the corpus is not filtered to `due <= today`, an
  undated Task is present, and `c` reveals a row completed on an earlier day —
  the aliasing regression, one test per predicate
- enter from a List and from Today; `Esc` restores the prior pane with the
  sidebar cursor exactly where it was, having never moved
- `Esc` precedence: one press exits from the open input; one press exits after
  `Enter`; with the input closed and `?` open, it closes the cheatsheet and
  stays in Search
- `S` opens the input and sets `Focus::Tasks`; `j` while the input is open types
  into the query, and after `Enter` it moves the task cursor
- `enter_search` clears `selected_task`; the cursor re-anchors on the corpus
- `j`/`k` with the sidebar focused leaves Search and loads that List, as does
  `A` into the List it creates; `Tab`/`h`/`l` keep Search, and so do `j`/`k`
  pressed at the sidebar's clamped edges, where nothing moves
- deleting the parked List mid-Search, and any List create/delete reply landing
  while Search is open, keep the query and the corpus
- the pending notice shows while only the cache paint has landed, clears on the
  live result, and never appears offline — and **survives a refusal message**
  landing between the two, which is why it is not held in the status line
- `w` in Search is refused and leaves `hide_distant` unchanged, checked by
  returning to the List pane
- a persisted `/` query on the List left behind is not restored on exit, while
  the sidebar cursor is
- no message for another pane overwrites the corpus — loads, and both replies
  of an in-list Move left in flight by pressing `J` then `S` — and no
  `SearchLoaded` overwrites a pane restored by `Esc`
- `o` and `C` refused *with a status line*, `C` emitting no command; a Clear
  reply for another pane's sweep leaves the corpus untouched but still
  tombstones
- the parked List's sidebar meter does not report corpus counts
- corpus paints from cache, then from the fan-out; a failed List is reported,
  not silently dropped
- a tombstoned id never appears in the corpus — checked on both arrival routes —
  and its tombstone survives the aggregate rather than being retired by it
- a Task with a cross-List Move in flight appears nowhere, in neither the corpus
  nor its source List, until the reply lands
- a second `S` after `Enter` re-opens the input over the typed query and emits
  no reload
- live narrowing per keystroke; empty query shows the whole corpus
- `r` in Search keeps **both the query and the corpus** — the
  `request_selected` regression — and fetches the account **once**: exactly one
  `LoadSearch`, no `LoadTasks` fan-out behind it
- with `hide_distant` on, a far-future Task is still found in Search, and still
  hidden in its own List pane
- `M` succeeding from Search keeps the row in the corpus under its new List
- `c` reveals Completed; a Task completed on an earlier day still shows
- ordering: overdue prefix contiguous, undated last, List-title tiebreak, both
  lenses; `Manual` normalised to `Due` on entry and absent from the `s` cycle
- a sidebar List switch leaves Search and drops the query
- `J`/`K`/`>`/`<` refused *with* a status line; a cross-List Move started in
  Search rolls back into Search
- `a` captures undated into the default List; fails closed when unresolved

`tests/search_render.rs`:

- header shows `SEARCH  /tax▏`, caret only while the input is open
- **no completion meter and no due-load strip**, over a corpus that would draw
  both in a List pane
- List-name column present; no Subtask indent
- **a result set with an undated row and a future-dated row: the due column is
  drawn, and the future date prints rather than blanking** — the two bugs the
  axis split avoids
- a result set with nothing overdue still shows dates, with no `Overdue` header
  and no Journal spread
- the legend says `Esc leave search` while the query input is open in Search,
  and still says `Esc clear` for the same overlay in a List pane
- the 80-column task legend row is identical in Search and in a List pane —
  `c completed` was not evicted

## Risks

- The renames (`today_ordered`, `next_today`) and the `today_view` axis split
  touch live Today paths. Today's reducer and render suites are the guard and
  must stay green **unedited** — the one existing test that does change is
  `src/domain/mod.rs`'s `next_today` unit test, and only its symbol name.
- Tightening `today_active()` is a one-line change with eleven consumers. It
  lands while Search is still unreachable, so it is provably a no-op at that
  commit.
- The pane-mode flag is not compiler-enforced, so a missed pane-kind branch is
  a silent no-op rather than a build error. Mitigated by enumerating every such
  site in the plan and testing each. `list_meter` — whose sidebar meter would
  have counted the whole corpus — is the site that proves the risk is real: it
  was found by re-walking that contract, not by the compiler.

## Out of scope

- Searching Cleared Tasks (not in the cache; would need `show_hidden=true`).
- Fuzzy or relevance-ranked matching — substring, case-insensitive, as `/` does.
- A persistent search history or saved searches.
- A sidebar row for Search.
