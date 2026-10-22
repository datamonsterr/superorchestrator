//! Error type for the store.

use so_core::RunState;

/// Errors raised by `so-store`.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{entity} `{id}` not found")]
    NotFound { entity: &'static str, id: String },
    #[error("{entity} prefix `{prefix}` is ambiguous")]
    AmbiguousPrefix {
        entity: &'static str,
        prefix: String,
    },
    #[error("transition {from} -> {to} is not allowed")]
    InvalidTransition { from: RunState, to: RunState },
    #[error("run `{id}` is in state {actual}, expected {expected}")]
    StateConflict {
        id: String,
        expected: RunState,
        actual: String,
    },
    #[error("database schema version {found} is newer than supported {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
