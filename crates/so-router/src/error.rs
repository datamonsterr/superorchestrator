//! Router errors.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    /// Nothing can take the task: the message says why (no choice, no account, all excluded).
    #[error("{0}")]
    NoEligible(String),
    #[error("invalid route request: {0}")]
    Invalid(String),
    #[error("could not read {path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("could not parse {path}: {reason}")]
    Parse { path: PathBuf, reason: String },
    #[error("store error: {0}")]
    Store(#[from] so_store::StoreError),
    #[error("usage error: {0}")]
    Usage(#[from] so_usage::UsageError),
    #[error("core error: {0}")]
    Core(#[from] so_core::CoreError),
}

pub type Result<T> = std::result::Result<T, RouterError>;
