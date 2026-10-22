//! Error type for the store.

use so_core::RunState;

/// Errors raised by `so-store`.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite error: {0}")]
    Sqlite(rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("constraint violation: {0}")]
    Constraint(String),
    #[error("invalid schema version {0}")]
    InvalidSchemaVersion(i64),
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
        actual: RunState,
    },
    #[error("database schema version {found} is newer than supported {supported}")]
    SchemaTooNew { found: i64, supported: i64 },
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        match &e {
            rusqlite::Error::SqliteFailure(f, msg)
                if f.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                StoreError::Constraint(msg.clone().unwrap_or_else(|| e.to_string()))
            }
            _ => StoreError::Sqlite(e),
        }
    }
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;
