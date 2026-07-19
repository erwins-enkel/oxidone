# Local SQLite cache with write-through live writes; offline queue deferred

The UI reads from a local SQLite cache (a mirror of Google's Tasks resources plus sync-metadata columns `dirty`, `etag`, `local_updated`) so startup is instant and lists are viewable offline. Writes go **live** to Google and patch the cache from the response; there is deliberately **no offline write-queue in v1** — the app requires connectivity to *change* tasks, only to *view* them offline.

This is the C-then-B trajectory: we chose cached-reads/live-writes (C) now and left room to grow into full offline-first sync (B) later. SQLite (over a JSON snapshot) was chosen because it is the natural substrate once `dirty`/`etag` conflict tracking and a write-queue arrive — a blob file would be thrown away at that point.

## Consequences

- The `dirty` column exists from day one but stays dormant (failed writes roll back, see ADR-0006-adjacent behavior).
- Graduating to offline-first is adding new reducer arms + a queue at one seam, not a rewrite.
