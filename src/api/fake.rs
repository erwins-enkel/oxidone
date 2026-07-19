//! In-memory `TasksApi` for tests: lets sync/reducer logic run with no network
//! and no live Google account (ADR-0004/0005). Supports fault injection so the
//! optimistic-rollback path (ADR-0001 behavior) can be exercised.

/// In-memory fake. Holds lists/tasks in maps; can be told to fail the next call.
#[derive(Default)]
pub struct FakeTasksApi {
    // state: Mutex<FakeState>,
    // next_error: Mutex<Option<ApiError>>,
}

// impl TasksApi for FakeTasksApi { ... }
