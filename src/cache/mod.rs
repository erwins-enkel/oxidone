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

use crate::domain::{List, ListId, Status, Task, TaskId, TaskLink};

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
    // 0004 — Google's output-only `links[]` (#55, ADR-0003 pure mirror), stored
    // as a JSON array of `{url, description, kind}`. Append-only migration:
    // existing rows keep NULL, which reads back as an empty vec until the next
    // Refresh repopulates them from Google.
    "ALTER TABLE tasks ADD COLUMN links TEXT;",
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
    /// gone from Google drop out, and so do their Tasks. Insertion order is
    /// preserved for the sidebar.
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
        // Purge Tasks orphaned by a List that vanished from Google (pure mirror,
        // ADR-0003): a List gone from Google takes its Tasks with it, exactly as
        // `delete_list` does for a local delete. Without this a cross-List read
        // (`all_tasks`, the Today view) would surface ghost rows with no live List
        // — unwritable, and never purged since the fan-out only visits `lists`.
        tx.execute(
            "DELETE FROM tasks WHERE list_id NOT IN (SELECT id FROM lists)",
            [],
        )?;
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
                  links, position, etag, updated, local_updated, dirty)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)",
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
                    links_json(&t.links)?,
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
                    // The *display* title: the log is human-readable history
                    // (ADR-0007), not a mirror, so it carries what the entry was
                    // called rather than the type encoding.
                    log.execute(params![
                        t.id.0,
                        t.list.0,
                        t.display_title(),
                        at.to_rfc3339()
                    ])?;
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
              links, position, etag, updated, local_updated, dirty)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 0)",
            params![
                task.id.0,
                task.list.0,
                task.parent.as_ref().map(|p| &p.0),
                task.title,
                task.notes,
                status_str(task.status),
                task.due.map(|d| d.format("%Y-%m-%d").to_string()),
                task.completed_at.map(|c| c.to_rfc3339()),
                links_json(&task.links)?,
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
    ///
    /// Records the *display* title. `INSERT OR IGNORE` on `(task_id,
    /// completed_at)` means first observation wins, so a later retype or rename
    /// never reaches the logged row — all the more reason to log what the entry
    /// was called rather than an encoding the user never typed.
    pub fn log_completion(&self, task: &Task) -> Result<()> {
        let (Status::Completed, Some(at)) = (task.status, task.completed_at) else {
            return Ok(());
        };
        self.conn.execute(
            "INSERT OR IGNORE INTO completion_log (task_id, list_id, title, completed_at)
             VALUES (?1, ?2, ?3, ?4)",
            // Display title, not raw — see `replace_tasks`.
            params![
                task.id.0,
                task.list.0,
                task.display_title(),
                at.to_rfc3339()
            ],
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
            "SELECT id, parent, title, notes, status, due, completed_at, links, position, etag, updated
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
                links: row.get(7)?,
                position: row.get(8)?,
                etag: row.get(9)?,
                updated: row.get(10)?,
            })
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            tasks.push(row?.into_task(list)?);
        }
        Ok(tasks)
    }

    /// Every cached Task across all Lists, each carrying its own `list`. Ordered
    /// by `(list_id, position)` — the `tasks_by_list` index order — so the base is
    /// stable; a caller needing a cross-List order (the Today view) re-sorts.
    ///
    /// Unlike [`tasks`](Self::tasks) the `list_id` is read from each row rather
    /// than passed in, since the rows span Lists.
    pub fn all_tasks(&self) -> Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, list_id, parent, title, notes, status, due, completed_at, links, position, etag, updated
             FROM tasks ORDER BY list_id, position",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                TaskRow {
                    id: row.get(0)?,
                    parent: row.get(2)?,
                    title: row.get(3)?,
                    notes: row.get(4)?,
                    status: row.get(5)?,
                    due: row.get(6)?,
                    completed_at: row.get(7)?,
                    links: row.get(8)?,
                    position: row.get(9)?,
                    etag: row.get(10)?,
                    updated: row.get(11)?,
                },
            ))
        })?;
        let mut tasks = Vec::new();
        for row in rows {
            let (list_id, task_row) = row?;
            tasks.push(task_row.into_task(&ListId(list_id))?);
        }
        Ok(tasks)
    }

    /// `(done, total)` per List over the mirrored Tasks, for the sidebar meters.
    ///
    /// The `tasks_by_list` index groups the scan by `list_id`, though `status`
    /// is read from the rows themselves — it is not a covering index. Cheap
    /// enough at personal scale to re-run whenever the cache changes. Subtasks
    /// count alongside top-level Tasks — the same definition the task-pane
    /// header meter uses, so the two agree.
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
    links: Option<String>,
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
            links: parse_links(self.links.as_deref())?,
            position: self.position,
            etag: self.etag,
            updated: parse_ts(&self.updated)?,
        })
    }
}

/// Encode a Task's `links[]` for the JSON `links` column. Always writes a JSON
/// array — even `"[]"` for the common empty case — so a written row is never
/// ambiguous with a legacy `NULL` (which predates migration 0004).
fn links_json(links: &[TaskLink]) -> Result<String> {
    serde_json::to_string(links).with_context(|| "encoding task links")
}

/// Decode the JSON `links` column. `NULL` (a legacy row) reads as an empty vec;
/// anything else must parse — a corrupt value fails closed rather than dropping
/// links silently (ADR-0003, and CLAUDE.md's fail-closed rule).
fn parse_links(raw: Option<&str>) -> Result<Vec<TaskLink>> {
    match raw {
        None => Ok(Vec::new()),
        Some(s) => serde_json::from_str(s).with_context(|| format!("parsing task links {s:?}")),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str, kind: Option<&str>) -> TaskLink {
        TaskLink {
            url: url.to_string(),
            description: None,
            kind: kind.map(str::to_string),
        }
    }

    #[test]
    fn a_legacy_null_links_column_reads_as_empty() {
        // Migration 0004 adds `links` with no default, so rows written before it
        // hold NULL. That must read as "no links", never a parse error.
        assert!(parse_links(None).unwrap().is_empty());
    }

    #[test]
    fn links_json_and_parse_links_round_trip() {
        let links = vec![link("https://a.dev/1", Some("email"))];
        let encoded = links_json(&links).unwrap();
        assert_eq!(parse_links(Some(&encoded)).unwrap(), links);
        // Empty encodes to a JSON array, not NULL, and reads back empty.
        assert_eq!(links_json(&[]).unwrap(), "[]");
        assert!(parse_links(Some("[]")).unwrap().is_empty());
    }

    #[test]
    fn a_corrupt_links_column_fails_closed() {
        // Fail-closed (CLAUDE.md): a garbled value surfaces an error rather than
        // silently dropping links and reporting "you have none".
        assert!(parse_links(Some("{not json")).is_err());
    }

    fn list(id: &str) -> List {
        List {
            id: ListId(id.into()),
            title: id.into(),
            etag: String::new(),
            updated: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }
    }

    fn task(id: &str, list: &str, position: &str) -> Task {
        Task {
            id: TaskId(id.into()),
            list: ListId(list.into()),
            parent: None,
            title: id.into(),
            notes: None,
            status: Status::NeedsAction,
            due: None,
            completed_at: None,
            links: Vec::new(),
            position: position.into(),
            etag: String::new(),
            updated: DateTime::from_timestamp(0, 0).expect("epoch is valid"),
        }
    }

    #[test]
    fn replace_lists_purges_tasks_of_a_vanished_list() {
        // Pure mirror (ADR-0003): a List gone from Google on the next refresh takes
        // its Tasks with it, so a cross-List read never surfaces orphan ghost rows.
        let cache = Cache::open_in_memory().unwrap();
        cache.replace_lists(&[list("a"), list("b")]).unwrap();
        cache
            .replace_tasks(&ListId("a".into()), &[task("a1", "a", "1")])
            .unwrap();
        cache
            .replace_tasks(&ListId("b".into()), &[task("b1", "b", "1")])
            .unwrap();

        // A refresh returns only List "a"; "b" (and its Task) must drop out.
        cache.replace_lists(&[list("a")]).unwrap();
        let all = cache.all_tasks().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id.0, "a1");
    }

    #[test]
    fn all_tasks_spans_lists_carrying_each_row_s_own_list() {
        // The cross-List read the Today view builds on: every cached Task, each
        // reconstructed with the `list_id` from its own row (not a passed-in List).
        let cache = Cache::open_in_memory().unwrap();
        cache.replace_lists(&[list("a"), list("b")]).unwrap();
        cache
            .replace_tasks(&ListId("a".into()), &[task("a1", "a", "1")])
            .unwrap();
        cache
            .replace_tasks(
                &ListId("b".into()),
                &[task("b1", "b", "1"), task("b2", "b", "2")],
            )
            .unwrap();

        let all = cache.all_tasks().unwrap();
        assert_eq!(all.len(), 3);
        // Each Task carries the List it was stored under.
        for t in &all {
            assert!(t.id.0.starts_with(&t.list.0), "{t:?}");
        }
        assert!(all.iter().any(|t| t.id.0 == "a1" && t.list.0 == "a"));
        assert!(all.iter().any(|t| t.id.0 == "b2" && t.list.0 == "b"));
    }
}
