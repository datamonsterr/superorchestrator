//! Typed row structs.

use chrono::{DateTime, Utc};
use so_core::{ErrorKind, Harness, Isolation, MessageKind, RunState, SessionKind};

#[derive(Debug, Clone, PartialEq)]
pub struct SessionRow {
    pub id: String,
    pub kind: SessionKind,
    pub client: String,
    pub cwd: Option<String>,
    pub account: Option<String>,
    pub pid: Option<i64>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: String,
}

/// Which sessions `list_sessions` returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFilter {
    Active,
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RunRow {
    pub id: String,
    pub session_id: String,
    pub parent_run_id: Option<String>,
    pub depth: i64,
    pub name: Option<String>,
    pub agent: Option<String>,
    pub choice_id: Option<String>,
    pub harness: Harness,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub cwd: Option<String>,
    pub worktree_path: Option<String>,
    pub branch: Option<String>,
    pub native_session_id: Option<String>,
    pub state: RunState,
    pub review: bool,
    pub output_schema: Option<String>,
    pub result_json: Option<String>,
    pub error_kind: Option<ErrorKind>,
    pub prompt: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub isolation: Isolation,
    pub max_turns: Option<u32>,
    /// `HEAD` of the repository when the run's worktree was made.
    pub worktree_base: Option<String>,
    /// Top level of the repository the worktree belongs to.
    pub worktree_repo: Option<String>,
    pub timeout_ms: Option<u64>,
}

/// Result of [`crate::Store::insert_run_checked`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertRun {
    Inserted,
    /// The parent already has this many active children; nothing was inserted.
    ChildCap {
        active: usize,
    },
}

/// Fields to set on a run. `None` leaves the column unchanged; for clearable columns
/// `Some(None)` sets it to NULL.
///
/// Changing `harness`/`account` is meant for failover only; new runs get them at insert.
#[derive(Debug, Clone, Default)]
pub struct RunPatch {
    pub choice_id: Option<String>,
    pub harness: Option<Harness>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub account: Option<Option<String>>,
    pub worktree_path: Option<Option<String>>,
    pub branch: Option<Option<String>>,
    pub worktree_base: Option<Option<String>>,
    pub worktree_repo: Option<Option<String>>,
    pub native_session_id: Option<Option<String>>,
    pub result_json: Option<Option<String>>,
    pub error_kind: Option<Option<ErrorKind>>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<Option<DateTime<Utc>>>,
}

/// Fields written together with the final state change in `finish_run`.
#[derive(Debug, Clone, Default)]
pub struct FinishPatch {
    pub result_json: Option<String>,
    pub error_kind: Option<ErrorKind>,
    pub finished_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttemptRow {
    pub id: String,
    pub run_id: String,
    pub account: String,
    pub pool: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub outcome: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageRow {
    pub id: String,
    pub session_id: String,
    pub from_run: Option<String>,
    pub to_run: Option<String>,
    pub kind: MessageKind,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LedgerEntry {
    pub account: String,
    pub pool: String,
    pub run_id: Option<String>,
    pub ts: DateTime<Utc>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: f64,
}

/// Aggregate of ledger entries for one (account, pool).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LedgerSums {
    /// Distinct runs that recorded usage.
    pub runs: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlagRow {
    pub account: String,
    pub pool: String,
    pub until: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowCacheRow {
    pub key_hash: String,
    pub workflow: String,
    pub result_json: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct WorkflowRunRow {
    pub id: String,
    pub name: String,
    pub session_id: Option<String>,
    pub state: String,
    pub args_json: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
