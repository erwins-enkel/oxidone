# `oxidone json` — the machine-readable interface

A second entry point onto the same core the TUI drives, for scripts and plugins.
Read subcommands print JSON to stdout; every write goes through one `apply`
subcommand that reads its command as JSON on **stdin**.

Why it exists, and what was rejected on the way, is
[ADR-0010](adr/0010-machine-readable-json-cli.md). This file is the contract.

## Stability

The shapes below are versioned by oxidone's own release number. Gate on it:

```sh
oxidone --version        # oxidone 1.0.0
```

Changing a field name, a value spelling or an exit code is a **breaking change**
and ships as one. Fields are only ever added, so a caller that ignores unknown
keys keeps working across a minor release.

The **order of keys within an object is not part of the contract** — JSON objects
are unordered, and oxidone emits them alphabetically. Examples below are written
in reading order; parse by name.

## Two constraints worth knowing

**Network-only.** No subcommand opens `oxidone.db`. oxidone's cache is
single-writer by design ([ADR-0001](adr/0001-local-sqlite-cache-write-through.md)),
and this process stays out of it — so there is no offline read path here, and a
change made through `json` does not appear in a running TUI until the next
Refresh, exactly as a change made on a phone does not. Keep your own
last-known-good copy.

**It never opens a browser.** Authorizing needs a person, and `oxidone json` is
what a plugin runs unattended. With no usable grant it exits `3` and tells you to
run `oxidone` once; it will not start a consent flow, bind a loopback port, or
wait for a redirect nobody is there to complete.

## Reads

### `oxidone json today`

The **Today** set: every entry due on or before today, across every List.

```json
{
  "today": "2026-07-20",
  "entries": [ /* Entry, … */ ]
}
```

Membership is `due <= today` and nothing else — the same definition the TUI's
Today pane uses, so a bar and the pane can never name different sets. An
**undated** entry is therefore never in it.

It is **status-blind**: Completed entries due on or before today are included, so
a caller that wants a count filters to `status == "needsAction"` itself.

> The TUI's Today *pane* additionally hides a Completed entry that was not
> completed today. That is a display rule of the pane, not a second definition of
> Today — the `needsAction` count is identical either way.

Ordered by due date (overdue first), then display title, then id. The tail of that
is only there to make the order total, so two entries on one day do not swap
between polls.

One request per List, and one List that will not load fails the whole call: a
short set is indistinguishable from a light day.

### `oxidone json lists`

```json
{
  "lists": [
    { "id": "MTIzNDU2", "title": "Inbox" },
    { "id": "Nzg5MDEy", "title": "Work" }
  ],
  "default_list": "MTIzNDU2"
}
```

`default_list` is the **concrete id** `@default` resolves to, never the alias, so
it matches an `id` in the same payload.

### `oxidone json tasks --list <id>`

One List's entries in **Manual order** — Google's `position` order, shown as "My
order" in the Google app.

```json
{
  "list": "MTIzNDU2",
  "entries": [ /* Entry, … */ ]
}
```

Subtasks are identified by `parent`, not nested. Nesting is capped at one level,
so grouping by that field needs no recursion.

### `oxidone json due <expr>`

Resolve a due-date phrase — the same vocabulary the TUI's `d` key accepts.

```console
$ oxidone json due "+3d"
{"input":"+3d","due":"2026-07-23"}
```

Accepts `today`, `tomorrow`, weekday and month names, `+3d`, a bare day-of-month
(`25` → the next 25th), and ISO `YYYY-MM-DD`. A phrase that is not a date is
refused rather than guessed at — `milk` is an error, not today.

Pure: no credentials, no network. It exists so a caller can show the user the
resolved date **before** committing to it, and so nobody has to write a second
parser.

## The Entry object

```json
{
  "id": "cUhIcWNPYWxfaVJI",
  "list": "MTIzNDU2",
  "parent": null,
  "title": "○ Standup",
  "display_title": "Standup",
  "type": "event",
  "has_notes": false,
  "due": "2026-07-20",
  "status": "needsAction",
  "completed_at": null,
  "position": "00000000000000000000"
}
```

| field | type | meaning |
| --- | --- | --- |
| `id` | string | Google's Task id. |
| `list` | string | The id of the List it is in. |
| `parent` | string \| null | Non-null ⇒ a **Subtask**. One level only. |
| `title` | string | The title exactly as Google stores it, type glyph and all. |
| `display_title` | string | The title with its type prefix removed — what a row should show. |
| `type` | `"task"` \| `"event"` \| `"note"` | The **Entry type**. |
| `has_notes` | boolean | Whether the entry carries non-empty `notes`. |
| `due` | date \| null | `YYYY-MM-DD`. A **date, never a time** — Google discards the time part. |
| `status` | `"needsAction"` \| `"completed"` | Google's own two spellings. |
| `completed_at` | timestamp \| null | RFC 3339, UTC. |
| `position` | string | Opaque Manual-order key. Sort by it; read nothing else into it. |

**Do not parse `title` yourself.** The Entry type lives in the title's leading
glyph ([ADR-0008](adr/0008-entry-type-in-title.md)), and `display_title` and
`type` are that one definition already applied. A second parser in a caller is how
the two surfaces start disagreeing about what an entry is called.

`notes` itself is not exposed — a row shows that notes *exist*, and shipping
arbitrary body text through a pipe buys nothing.

Reads show the **active view**: Completed entries are included, **Cleared**
(hidden) ones are not. A Cleared entry is still reachable by id through `apply`,
exactly as it is in Google.

## `oxidone json apply`

One JSON command on stdin, one result on stdout.

```console
$ echo '{"op":"complete","list":"MTIzNDU2","task":"cUhIcWNPYWxfaVJI"}' \
    | oxidone json apply
{"entry":{ … }}
```

**The payload is on stdin, never in flags.** `/proc/<pid>/cmdline` is readable by
every process running as the same user, so a `--title` flag would publish your
task titles to anything else on the machine.

One command per invocation. An array is not a command — batching may be added
later, and could not be removed once callers depended on the shape.

Unknown fields are **refused**, not ignored: a field name you got wrong is told to
you rather than silently dropped.

### The operations

| `op` | fields | does |
| --- | --- | --- |
| `complete` | `list`, `task` | Sets `status` to `completed`. |
| `uncomplete` | `list`, `task` | Reopens it and clears `completed_at`. |
| `create` | `list`, `title` | Creates an entry from a title alone. |
| `retitle` | `list`, `task`, `title` | Renames, **preserving the Entry type**. |
| `set_due` | `list`, `task`, `due` | Sets the due date. ISO `YYYY-MM-DD` only. |
| `clear_due` | `list`, `task` | Removes the due date. |
| `delete` | `list`, `task` | Deletes (Google's soft delete). |
| `migrate` | `list`, `task` | Defers to `max(today, due) + 1 day`. |

```json
{"op": "complete",   "list": "L", "task": "T"}
{"op": "uncomplete", "list": "L", "task": "T"}
{"op": "create",     "list": "L", "title": "Buy milk"}
{"op": "retitle",    "list": "L", "task": "T", "title": "Daily sync"}
{"op": "set_due",    "list": "L", "task": "T", "due": "2026-12-25"}
{"op": "clear_due",  "list": "L", "task": "T"}
{"op": "delete",     "list": "L", "task": "T"}
{"op": "migrate",    "list": "L", "task": "T"}
```

Notes on three of them:

- **`retitle` takes a Display title** — no glyph. The entry's current type is read
  first and re-applied, so `"Daily sync"` on an Event becomes `"○ Daily sync"`.
  This is exactly what the TUI's `e` does. It does not repair a non-canonical
  prefix; that is what `t` in the TUI is for.
- **`create` writes the title verbatim**, so its type is whatever the title
  parses as — the same thing that happens to a title typed into Google's own web
  client. The answer carries the resulting `type`.
- **`migrate`** is Bullet Journal's `>`. It is *not* an exit: the entry stays
  `needsAction` and only its date moves, so repeated migrations defer a day at a
  time. It is **refused on a Completed entry** (exit 7), exactly as the `m` key
  is.

`set_due` and `clear_due` are separate operations rather than one with a nullable
`due`, so a mistyped field name can never read as "clear the date".

### Answers

Every operation but `delete` answers with the entry as the server left it, so you
can update your copy without a second read:

```json
{"entry": { /* Entry */ }}
```

`delete` has no entry to echo, and names what went:

```json
{"deleted": {"list": "MTIzNDU2", "id": "cUhIcWNPYWxfaVJI"}}
```

## Errors

Failures print JSON to **stderr** and exit non-zero. stdout stays empty, so you
can pipe it to a parser without first asking whether the call worked.

```json
{"error":{"kind":"auth_expired","message":"no usable Google authorization; run `oxidone` once to authorize"}}
```

| exit | `kind` | means |
| --- | --- | --- |
| 0 | — | success |
| 1 | `internal` | something local went wrong that is not your call's fault |
| 2 | `usage` | the invocation or the stdin command was not well-formed |
| 2 | `invalid_due` | `due <expr>` was not a date |
| 3 | `not_configured` | no BYO credentials, or nowhere to keep them |
| 3 | `auth_expired` | no usable grant — run `oxidone` once to authorize |
| 3 | `token_store_failed` | the token file could not be read or written |
| 4 | `network` | the request did not reach Google, or the answer did not come back |
| 4 | `rate_limited` | a short-term limit that backing off did not clear |
| 4 | `pagination` | a paged read never reached its last page |
| 5 | `rejected` | Google refused the request |
| 5 | `quota_exhausted` | Google's daily cap; waiting *shortly* will not help |
| 6 | `not_found` | no such List or entry |
| 7 | `refused` | oxidone declined — Migrate on a Completed entry |

Branch on the **exit code** for a state (authorize / retry / give up) and read
`kind` when you want to say which of them happened. Three kinds share exit 3
because the answer to all three is "you are not authorized"; only `auth_expired`
is fixed by authorizing.

`usage`, `invalid_due` and a malformed `apply` command are all reported **before**
anything authorizes, so you can develop against the contract on a machine that has
never been connected to Google.

## Worked example: a bar count

```sh
# Capture oxidone's own output and its own status first. Two traps here, and
# both silently give you the wrong number:
#   * piping straight into jq throws the code away — `$?` after a pipeline is
#     jq's, not oxidone's;
#   * `if ! payload=$(...)` throws it away too — `!` replaces it with 0 or 1.
payload=$(oxidone json today 2>/dev/null)
status=$?

if [ "$status" -ne 0 ]; then
  case "$status" in
    3) echo "not authorized — configure credentials, or run oxidone once" ;;
    4) exit 0 ;;   # transient — keep showing the last good count
    *) echo "error" ;;
  esac
  exit 1
fi

count=$(printf '%s' "$payload" |
  jq '[.entries[] | select(.status == "needsAction")] | length')
echo "$count"
```

## Concurrency

`oxidone json` and a running TUI may both want to refresh the same OAuth grant.
They cannot both post one: the refresh exchange takes a cross-process file lock
and re-reads the stored token under it, so the second process uses the token the
first just wrote ([ADR-0009](adr/0009-own-the-refresh-exchange.md),
[ADR-0010](adr/0010-machine-readable-json-cli.md)).
