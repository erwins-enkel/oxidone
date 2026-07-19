# Local completion log, per-machine and non-syncing

Separately from the pure-mirror cache, oxidone keeps an **append-only `completion_log`** table recording completion events (`task_id`, `list_id`, `title`, `completed_at`) as they are observed. This exists to feed future activity views (braille sparklines) with history that Google discards when tasks are cleared.

Keeping this out of the mirror is what preserves ADR-0003: the live-task cache still drops cleared/deleted tasks exactly as Google does, so it stays an honest mirror, while the log accumulates history in its own store. The log is deliberately **local-only and does not sync across machines**.

## Consequences

- The user runs oxidone on multiple machines; therefore future sparklines built from this log will be **per-machine and partial** — machine A cannot see what was completed on machine B. This is an accepted limitation, not a bug. Syncing the log across machines is explicitly out of scope.
- Analytics/activity views read the log; nothing treats it as authoritative task state.
