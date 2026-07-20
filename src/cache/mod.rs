//! SQLite persistence. The `lists`/`tasks` tables mirror Google's resources
//! (ADR-0003) plus sync-metadata columns `dirty`, `etag`, `local_updated`
//! (ADR-0001); a later `completion_log` (ADR-0007) is kept out of the mirror.
//!
//! Migrations are an ordered, append-only list applied via SQLite's
//! `user_version`, so later slices add tables by appending — never editing
//! shipped migrations.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use rusqlite::{params, Connection};

use crate::domain::{List, ListId, Status, Task, TaskId};

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
    // 0002 — Tasks (pure mirror + sync metadata). `parent` NULL = top-level.
    "CREATE TABLE tasks (
        id            TEXT PRIMARY KEY,
        list_id       TEXT NOT NULL,
        parent        TEXT,
        title         TEXT NOT NULL,
        notes         TEXT,
        status        TEXT NOT NULL,
        due           TEXT,
        completed_at  TEXT,
        position      TEXT NOT NULL,
        etag          TEXT NOT NULL,
        updated       TEXT NOT NULL,
        local_updated TEXT NOT NULL,
        dirty         INTEGER NOT NULL DEFAULT 0
     );
     CREATE INDEX tasks_by_list ON tasks (list_id, position);",
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

    /// Patch a single List into the cache from a write response (an insert or a
    /// rename). Uses UPSERT (not INSERT OR REPLACE) so an existing row keeps its
    /// `rowid` — and thus its sidebar position — across a rename; a genuinely new
    /// List lands at the end, matching where Google appends it.
    pub fn upsert_list(&self, list: &List) -> Result<()> {
        self.conn.execute(
            "INSERT INTO lists (id, title, etag, updated, local_updated, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, 0)
             ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 etag = excluded.etag,
                 updated = excluded.updated,
                 local_updated = excluded.local_updated,
                 dirty = 0",
            params![
                list.id.0,
                list.title,
                list.etag,
                list.updated.to_rfc3339(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Remove a single List from the cache along with its Tasks (mirrors a
    /// delete-through; a deleted List takes its Tasks with it on Google).
    pub fn delete_list(&self, list: &ListId) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM lists WHERE id = ?1", params![list.0])?;
        tx.execute("DELETE FROM tasks WHERE list_id = ?1", params![list.0])?;
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

    /// Replace the cached Tasks of one List (a pure-mirror refresh). Other
    /// Lists' Tasks are untouched. See the `replace_lists` TODO re: writes.
    pub fn replace_tasks(&self, list: &ListId, tasks: &[Task]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM tasks WHERE list_id = ?1", params![list.0])?;
        {
            let now = Utc::now().to_rfc3339();
            let mut stmt = tx.prepare(
                "INSERT INTO tasks
                 (id, list_id, parent, title, notes, status, due, completed_at,
                  position, etag, updated, local_updated, dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)",
            )?;
            for t in tasks {
                stmt.execute(params![
                    t.id.0,
                    t.list.0,
                    t.parent.as_ref().map(|p| &p.0),
                    t.title,
                    t.notes,
                    status_str(t.status),
                    t.due.map(|d| d.format("%Y-%m-%d").to_string()),
                    t.completed_at.map(|c| c.to_rfc3339()),
                    t.position,
                    t.etag,
                    t.updated.to_rfc3339(),
                    now,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Patch a single Task into the cache from a write response (INSERT OR
    /// REPLACE). Used by write-through so a mutation lands in the cache without
    /// re-fetching the whole List.
    pub fn upsert_task(&self, task: &Task) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tasks
             (id, list_id, parent, title, notes, status, due, completed_at,
              position, etag, updated, local_updated, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 0)",
            params![
                task.id.0,
                task.list.0,
                task.parent.as_ref().map(|p| &p.0),
                task.title,
                task.notes,
                status_str(task.status),
                task.due.map(|d| d.format("%Y-%m-%d").to_string()),
                task.completed_at.map(|c| c.to_rfc3339()),
                task.position,
                task.etag,
                task.updated.to_rfc3339(),
                Utc::now().to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Remove a single Task from the cache (mirrors a delete-through).
    pub fn delete_task(&self, task: &TaskId) -> Result<()> {
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", params![task.0])?;
        Ok(())
    }

    /// The cached Tasks of a List, in Manual order (`position`).
    pub fn tasks(&self, list: &ListId) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, parent, title, notes, status, due, completed_at, position, etag, updated
             FROM tasks WHERE list_id = ?1 ORDER BY position",
        )?;
        let rows = stmt.query_map(params![list.0], |row| {
            Ok(TaskRow {
                id: row.get(0)?,
                parent: row.get(1)?,
                title: row.get(2)?,
                notes: row.get(3)?,
                status: row.get(4)?,
                due: row.get(5)?,
                completed_at: row.get(6)?,
                position: row.get(7)?,
                etag: row.get(8)?,
                updated: row.get(9)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?.into_task(list)?);
        }
        Ok(tasks)
    }
}

/// Raw Task columns, before parsing timestamps/enums into domain types.
struct TaskRow {
    id: String,
    parent: Option<String>,
    title: String,
    notes: Option<String>,
    status: String,
    due: Option<String>,
    completed_at: Option<String>,
    position: String,
    etag: String,
    updated: String,
}

impl TaskRow {
    fn into_task(self, list: &ListId) -> Result<Task> {
        Ok(Task {
            id: TaskId(self.id),
            list: list.clone(),
            parent: self.parent.map(TaskId),
            title: self.title,
            notes: self.notes,
            status: parse_status(&self.status)?,
            due: self
                .due
                .map(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d"))
                .transpose()
                .with_context(|| "parsing due date")?,
            completed_at: self.completed_at.as_deref().map(parse_ts).transpose()?,
            position: self.position,
            etag: self.etag,
            updated: parse_ts(&self.updated)?,
        })
    }
}

fn status_str(status: Status) -> &'static str {
    match status {
        Status::NeedsAction => "needsAction",
        Status::Completed => "completed",
    }
}

fn parse_status(s: &str) -> Result<Status> {
    match s {
        "needsAction" => Ok(Status::NeedsAction),
        "completed" => Ok(Status::Completed),
        other => anyhow::bail!("unknown task status {other:?} in cache"),
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
