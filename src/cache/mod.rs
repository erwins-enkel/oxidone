//! SQLite persistence. The `lists`/`tasks` tables mirror Google's resources
//! (ADR-0003) plus sync-metadata columns `dirty`, `etag`, `local_updated`
//! (ADR-0001); a later `completion_log` (ADR-0007) is kept out of the mirror.
//!
//! Migrations are an ordered, append-only list applied via SQLite's
//! `user_version`, so later slices add tables by appending — never editing
//! shipped migrations.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::domain::{List, ListId};

/// Ordered migrations. Index + 1 is the resulting `user_version`.
const MIGRATIONS: &[&str] = &[
    // 0001 — Lists (pure mirror + sync metadata).
    "CREATE TABLE lists (
        id            TEXT PRIMARY KEY,
        title         TEXT NOT NULL,
        etag          TEXT NOT NULL,
        updated       TEXT NOT NULL,
        local_updated TEXT NOT NULL,
        dirty         INTEGER NOT NULL DEFAULT 0
     );",
];

pub struct Cache {
    conn: Connection,
}

impl Cache {
    /// Open (creating if needed) a cache at `path` and run migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("opening cache db at {}", path.display()))?;
        Self::from_conn(conn)
    }

    /// An in-memory cache, for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::from_conn(Connection::open_in_memory()?)
    }

    fn from_conn(conn: Connection) -> Result<Self> {
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Replace the cached List set with `lists` (a pure-mirror refresh): Lists
    /// gone from Google drop out. Insertion order is preserved for the sidebar.
    ///
    /// TODO(write-through, ADR-0001): once Lists are locally editable, a refresh
    /// must diff rather than wipe — preserving `dirty` rows and their real
    /// `local_updated`, not stamping every row with the refresh time.
    pub fn replace_lists(&self, lists: &[List]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM lists", [])?;
        {
            let now = Utc::now().to_rfc3339();
            let mut stmt = tx.prepare(
                "INSERT INTO lists (id, title, etag, updated, local_updated, dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
            )?;
            for list in lists {
                stmt.execute(params![
                    list.id.0,
                    list.title,
                    list.etag,
                    list.updated.to_rfc3339(),
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// All cached Lists, in their stored (insertion) order.
    pub fn lists(&self) -> Result<Vec<List>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, title, etag, updated FROM lists ORDER BY rowid")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut lists = Vec::new();
        for row in rows {
            let (id, title, etag, updated) = row?;
            lists.push(List {
                id: ListId(id),
                title,
                etag,
                updated: parse_ts(&updated)?,
            });
        }
        Ok(lists)
    }
}

fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        if (i as i64) < version {
            continue;
        }
        // Apply the migration and bump `user_version` atomically: `user_version`
        // is transactional, so a crash mid-migration rolls back both, leaving a
        // clean state to retry rather than a half-applied schema.
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as i64 + 1)?;
        tx.commit()?;
    }
    Ok(())
}

fn parse_ts(s: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(s)
        .with_context(|| format!("parsing timestamp {s:?}"))?
        .with_timezone(&Utc))
}
