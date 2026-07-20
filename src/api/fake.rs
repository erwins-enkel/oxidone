//! In-memory `TasksApi` for tests: lets sync/reducer logic run with no network
//! and no live Google account (ADR-0004/0005). Supports seeding (via the trait
//! itself) and fault injection so failure paths (e.g. optimistic rollback) can
//! be exercised.
//!
//! Faithfulness to Google, kept out of the domain:
//! - `hidden`/`deleted` are tracked here, not on `domain::Task` — they are exit
//!   states, not display fields (see `CONTEXT.md`).
//! - The one-level Subtask cap is enforced server-side by Google; the fake
//!   mirrors it by rejecting illegal Moves, so it never lets buggy callers
//!   create two-level nesting that Google would refuse.
//! - Timestamps come from a deterministic seq-based clock, not the wall clock,
//!   so `updated` / `updated_min` and ordering are stable in tests.

use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};

use super::{ApiError, NewTask, TaskPatch, TasksApi};
use crate::domain::{List, ListId, Status, Task, TaskId, TaskLink};

/// Fixed epoch for the deterministic clock (2023-11-14T22:13:20Z). Each mutation
/// advances one second, so timestamps strictly increase in creation order.
const CLOCK_BASE: i64 = 1_700_000_000;

struct Entry {
    task: Task,
    hidden: bool,
    deleted: bool,
}

#[derive(Default)]
struct State {
    lists: Vec<List>,
    tasks: Vec<Entry>,
    seq: i64,
    next_error: Option<ApiError>,
}

impl State {
    /// Advance the clock and return the new sequence number + timestamp.
    fn tick(&mut self) -> (i64, DateTime<Utc>) {
        self.seq += 1;
        let ts = Utc
            .timestamp_opt(CLOCK_BASE + self.seq, 0)
            .single()
            .unwrap();
        (self.seq, ts)
    }

    /// Consume an injected error, if one is pending.
    fn take_error(&mut self) -> Result<(), ApiError> {
        match self.next_error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn list_exists(&self, id: &ListId) -> bool {
        self.lists.iter().any(|l| &l.id == id)
    }

    /// Index of the live (non-deleted) Task with this id in this List.
    fn entry_pos(&self, list: &ListId, id: &TaskId) -> Option<usize> {
        self.tasks
            .iter()
            .position(|e| &e.task.list == list && &e.task.id == id && !e.deleted)
    }

    /// Does any live Task in this List have `id` as its parent?
    fn has_children(&self, list: &ListId, id: &TaskId) -> bool {
        self.tasks
            .iter()
            .any(|e| &e.task.list == list && e.task.parent.as_ref() == Some(id) && !e.deleted)
    }

    fn live_count(&self, list: &ListId) -> usize {
        self.tasks
            .iter()
            .filter(|e| &e.task.list == list && !e.deleted)
            .count()
    }

    /// Live Task ids of a List in current Manual order (by `position`).
    fn live_ids_in_order(&self, list: &ListId) -> Vec<TaskId> {
        let mut entries: Vec<&Entry> = self
            .tasks
            .iter()
            .filter(|e| &e.task.list == list && !e.deleted)
            .collect();
        entries.sort_by(|a, b| a.task.position.cmp(&b.task.position));
        entries.into_iter().map(|e| e.task.id.clone()).collect()
    }
}

/// In-memory fake API. State behind a mutex so it satisfies `Send + Sync` for
/// the async trait.
#[derive(Default)]
pub struct FakeTasksApi {
    state: Mutex<State>,
}

impl FakeTasksApi {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make the *next* trait call fail with `err` (one-shot).
    pub fn fail_next(&self, err: ApiError) {
        self.state.lock().unwrap().next_error = Some(err);
    }

    /// Seed a Task's output-only `links[]` (#55). There is no write path for
    /// links in the real API, so this is the only way for a test to give a stored
    /// Task the links Google would have attached at creation — `list_tasks` then
    /// returns them like any other mirrored field. A no-op if `id` is unknown.
    pub fn set_links(&self, id: &TaskId, links: Vec<TaskLink>) {
        let mut st = self.state.lock().unwrap();
        if let Some(entry) = st.tasks.iter_mut().find(|e| &e.task.id == id) {
            entry.task.links = links;
        }
    }
}

fn rejected(message: &str) -> ApiError {
    ApiError::Rejected {
        status: 400,
        message: message.to_string(),
    }
}

#[async_trait]
impl TasksApi for FakeTasksApi {
    async fn list_lists(&self) -> Result<Vec<List>, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        Ok(st.lists.clone())
    }

    async fn insert_list(&self, title: &str) -> Result<List, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        let (seq, ts) = st.tick();
        let list = List {
            id: ListId(format!("list-{seq}")),
            title: title.to_string(),
            etag: format!("etag-{seq}"),
            updated: ts,
        };
        st.lists.push(list.clone());
        Ok(list)
    }

    async fn patch_list(&self, id: &ListId, title: &str) -> Result<List, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        if !st.list_exists(id) {
            return Err(ApiError::NotFound);
        }
        let (seq, ts) = st.tick();
        let list = st.lists.iter_mut().find(|l| &l.id == id).unwrap();
        list.title = title.to_string();
        list.etag = format!("etag-{seq}");
        list.updated = ts;
        Ok(list.clone())
    }

    async fn delete_list(&self, id: &ListId) -> Result<(), ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        if !st.list_exists(id) {
            return Err(ApiError::NotFound);
        }
        st.lists.retain(|l| &l.id != id);
        st.tasks.retain(|e| &e.task.list != id);
        Ok(())
    }

    async fn list_tasks(
        &self,
        list: &ListId,
        show_completed: bool,
        show_hidden: bool,
        updated_min: Option<DateTime<Utc>>,
    ) -> Result<Vec<Task>, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        if !st.list_exists(list) {
            return Err(ApiError::NotFound);
        }
        let mut out: Vec<Task> = st
            .tasks
            .iter()
            .filter(|e| &e.task.list == list && !e.deleted)
            .filter(|e| show_hidden || !e.hidden)
            .filter(|e| show_completed || e.task.status != Status::Completed)
            .filter(|e| updated_min.map_or(true, |min| e.task.updated >= min))
            .map(|e| e.task.clone())
            .collect();
        out.sort_by(|a, b| a.position.cmp(&b.position));
        Ok(out)
    }

    async fn insert_task(&self, list: &ListId, task: NewTask) -> Result<Task, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        if !st.list_exists(list) {
            return Err(ApiError::NotFound);
        }
        // New tasks append to the end of Manual order.
        let position = format!("{:020}", st.live_count(list));
        let (seq, ts) = st.tick();
        let task = Task {
            id: TaskId(format!("task-{seq}")),
            list: list.clone(),
            parent: task.parent,
            title: task.title,
            notes: task.notes,
            status: Status::NeedsAction,
            due: task.due,
            completed_at: None,
            // `links[]` is output-only, so a newly inserted Task never has one
            // (there is no way to write it). Tests seed it via `set_links`.
            links: Vec::new(),
            position,
            etag: format!("etag-{seq}"),
            updated: ts,
        };
        st.tasks.push(Entry {
            task: task.clone(),
            hidden: false,
            deleted: false,
        });
        Ok(task)
    }

    async fn patch_task(
        &self,
        list: &ListId,
        id: &TaskId,
        patch: TaskPatch,
    ) -> Result<Task, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        let pos = st.entry_pos(list, id).ok_or(ApiError::NotFound)?;
        let (seq, ts) = st.tick();
        let entry = &mut st.tasks[pos];
        if let Some(title) = patch.title {
            entry.task.title = title;
        }
        if let Some(notes) = patch.notes {
            entry.task.notes = notes;
        }
        if let Some(due) = patch.due {
            entry.task.due = due;
        }
        if let Some(completed) = patch.completed {
            if completed {
                entry.task.status = Status::Completed;
                entry.task.completed_at = Some(ts);
            } else {
                entry.task.status = Status::NeedsAction;
                entry.task.completed_at = None;
            }
        }
        entry.task.etag = format!("etag-{seq}");
        entry.task.updated = ts;
        Ok(entry.task.clone())
    }

    async fn delete_task(&self, list: &ListId, id: &TaskId) -> Result<(), ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        let pos = st.entry_pos(list, id).ok_or(ApiError::NotFound)?;
        st.tasks[pos].deleted = true;
        Ok(())
    }

    async fn move_task(
        &self,
        list: &ListId,
        id: &TaskId,
        parent: Option<&TaskId>,
        previous: Option<&TaskId>,
    ) -> Result<Task, ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;

        // Validate everything before advancing the clock, so a rejected Move
        // leaves the deterministic timestamps untouched.
        if st.entry_pos(list, id).is_none() {
            return Err(ApiError::NotFound);
        }
        if let Some(p) = parent {
            match st.entry_pos(list, p) {
                None => return Err(rejected("parent task not found in list")),
                // One-level cap: the parent must itself be top-level, and the
                // task being moved must not already have children.
                Some(pi) if st.tasks[pi].task.parent.is_some() => {
                    return Err(rejected(
                        "cannot nest a subtask under a subtask (one level max)",
                    ));
                }
                Some(_) => {}
            }
            if st.has_children(list, id) {
                return Err(rejected("cannot make a task with subtasks into a subtask"));
            }
        }

        let (seq, ts) = st.tick();

        // Reparent + bump the moved task.
        let pos = st.entry_pos(list, id).unwrap();
        st.tasks[pos].task.parent = parent.cloned();
        st.tasks[pos].task.etag = format!("etag-{seq}");
        st.tasks[pos].task.updated = ts;

        // Splice the moved task into Manual order: right after `previous`, else
        // as the first child of `parent`, else at the front. Then renumber the
        // whole List so `position` stays a single coherent index scheme.
        let mut order = st.live_ids_in_order(list);
        order.retain(|t| t != id);
        let insert_at = if let Some(prev) = previous {
            order
                .iter()
                .position(|t| t == prev)
                .map_or(order.len(), |i| i + 1)
        } else if let Some(p) = parent {
            order.iter().position(|t| t == p).map_or(0, |i| i + 1)
        } else {
            0
        };
        order.insert(insert_at, id.clone());
        for (i, tid) in order.iter().enumerate() {
            let p = st.entry_pos(list, tid).unwrap();
            st.tasks[p].task.position = format!("{i:020}");
        }

        let moved = st.tasks[st.entry_pos(list, id).unwrap()].task.clone();
        Ok(moved)
    }

    async fn clear_completed(&self, list: &ListId) -> Result<(), ApiError> {
        let mut st = self.state.lock().unwrap();
        st.take_error()?;
        if !st.list_exists(list) {
            return Err(ApiError::NotFound);
        }
        for entry in st.tasks.iter_mut() {
            if &entry.task.list == list && entry.task.status == Status::Completed && !entry.deleted
            {
                entry.hidden = true;
            }
        }
        Ok(())
    }
}
