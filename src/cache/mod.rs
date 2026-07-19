//! SQLite persistence. Two distinct stores in one DB:
//!
//! 1. **Pure-mirror cache** (`lists`, `tasks`) — mirrors Google exactly
//!    (ADR-0003) plus sync-metadata columns `dirty`, `etag`, `local_updated`
//!    (ADR-0001). Cleared/deleted tasks drop out, just like Google.
//! 2. **Completion log** (`completion_log`) — append-only history of completion
//!    events, kept OUT of the mirror (ADR-0007). Per-machine, non-authoritative.

/// Embedded, ordered migrations run at startup.
pub const MIGRATIONS: &[&str] = &[
    // 0001_init
    "CREATE TABLE lists (
        id TEXT PRIMARY KEY, title TEXT NOT NULL, etag TEXT NOT NULL,
        updated TEXT NOT NULL, local_updated TEXT NOT NULL, dirty INTEGER NOT NULL DEFAULT 0
     );
     CREATE TABLE tasks (
        id TEXT PRIMARY KEY, list_id TEXT NOT NULL REFERENCES lists(id) ON DELETE CASCADE,
        parent TEXT, title TEXT NOT NULL, notes TEXT,
        status TEXT NOT NULL, due TEXT, completed_at TEXT,
        position TEXT NOT NULL, etag TEXT NOT NULL, updated TEXT NOT NULL,
        local_updated TEXT NOT NULL, dirty INTEGER NOT NULL DEFAULT 0
     );
     CREATE INDEX tasks_by_list ON tasks(list_id, position);
     CREATE TABLE completion_log (
        task_id TEXT NOT NULL, list_id TEXT NOT NULL, title TEXT NOT NULL,
        completed_at TEXT NOT NULL
     );",
];

pub struct Cache {
    // conn: rusqlite::Connection,
}
