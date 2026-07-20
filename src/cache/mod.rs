//! SQLite persistence. The `lists`/`tasks` tables mirror Google's resources
//! (ADR-0003) plus sync-metadata columns `dirty`, `etag`, `local_updated`
//! (ADR-0001); the append-only `completion_log` (ADR-0007) is kept *out* of the
//! mirror — it accumulates completion history Google discards on Clear.
//!
//! Migrations are an ordered, append-only list applied via SQLite's
//! `user_version`, so later slices add tables by appending — never editing
//! shipped migrations.

use std::collections::HashMap;
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
    // 0003 — Completion log (ADR-0007), append-only and out of the mirror. The
    // (task_id, completed_at) primary key makes re-observing a completion (on
    // every refresh) idempotent; re-completing after un-completing yields a new
    // `completed_at`, hence a new event.
    "CREATE TABLE completion_log (
        task_id      TEXT NOT NULL,
        list_id      TEXT NOT NULL,
        title        TEXT NOT NULL,
        completed_at TEXT NOT NULL,
        PRIMARY KEY (task_id, completed_at)
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
        // Observe completions for the log in the same transaction (ADR-0007), so
        // a refresh that first sees a Completed Task — including one completed on
        // another surface — records it before Google can Clear it away.
        {
            let mut log = tx.prepare(
                "INSERT OR IGNORE INTO completion_log (task_id, list_id, title, completed_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )?;
            for t in tasks {
                if let (Status::Completed, Some(at)) = (t.status, t.completed_at) {
                    log.execute(params![t.id.0, t.list.0, t.title, at.to_rfc3339()])?;
                }
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
        // Mirroring a Task is also where its completion is observed for the log
        // (ADR-0007). A log failure must not fail the (authoritative) cache write
        // — the log is non-authoritative and self-heals on the next full refresh
        // via `replace_tasks` — so it is surfaced to the trace, not propagated.
        if let Err(e) = self.log_completion(task) {
            tracing::warn!(error = %e, task = %task.id.0, "failed to append completion to log");
        }
        Ok(())
    }

    /// Append an observed completion to the append-only `completion_log`
    /// (ADR-0007). A no-op unless the Task is Completed with a `completed_at`;
    /// idempotent on `(task_id, completed_at)`. Deliberately separate from the
    /// pure-mirror `tasks` table (ADR-0003).
    pub fn log_completion(&self, task: &Task) -> Result<()> {
        let (Status::Completed, Some(at)) = (task.status, task.completed_at) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT OR IGNORE INTO completion_log (task_id, list_id, title, completed_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![task.id.0, task.list.0, task.title, at.to_rfc3339()],
        )?;
        Ok(())
    }

    /// The logged completion events, oldest first. Feeds future activity views;
    /// never treated as authoritative task state.
    pub fn completions(&self) -> Result<Vec<Completion>> {
        let mut stmt = self.conn.prepare(
            "SELECT task_id, list_id, title, completed_at
             FROM completion_log ORDER BY completed_at, task_id",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (task, list, title, at) = row?;
            out.push(Completion {
                task: TaskId(task),
                list: ListId(list),
                title,
                completed_at: parse_ts(&at)?,
            });
        }
        Ok(out)
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

    /// `(done, total)` per List over the mirrored Tasks, for the sidebar meters.
    ///
    /// Covered by the `tasks_by_list` index, so it stays cheap enough to re-run
    /// whenever the cache changes. Subtasks count alongside top-level Tasks —
    /// the same definition the task-pane header meter uses, so the two agree.
    ///
    /// A List with no cached Tasks yields **no entry** rather than `(0, 0)`.
    /// That keeps "we have never seen this List's Tasks" indistinguishable from
    /// "it is empty" — both render no meter — and, unlike a `LEFT JOIN` over
    /// `lists`, leaves a List whose fetch failed uncovered so the next attempt
    /// still has reason to fetch it.
    pub fn list_counts(&self) -> Result<HashMap<ListId, (usize, usize)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT list_id, COUNT(*), SUM(status = ?1) FROM tasks GROUP BY list_id")?;
        // Bound, never inlined: the stored spelling of a status lives only in
        // `status_str`, and a literal here would silently stop matching if it
        // ever changed — leaving `done` at 0 with nothing to fail.
        let rows = stmt.query_map(params![status_str(Status::Completed)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut counts = HashMap::new();
        for row in rows {
            let (list, total, done) = row?;
            counts.insert(ListId(list), (done as usize, total as usize));
        }
        Ok(counts)
    }
}

/// A logged completion event (ADR-0007): local-only, non-authoritative history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Completion {
    pub task: TaskId,
    pub list: ListId,
    pub title: String,
    pub completed_at: DateTime<Utc>,
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
