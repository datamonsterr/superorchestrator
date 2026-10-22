//! Errors raised by `so-accounts`.

use std::path::PathBuf;

use so_core::CoreError;

#[derive(Debug, thiserror::Error)]
pub enum AccountsError {
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "agy account `{0}` needs bwrap, which is Linux-only; on macOS only the default ~/.gemini works"
    )]
    AgyMultiAccountUnsupported(String),
    #[error("binary `{0}` not found on PATH")]
    BinaryNotFound(String),
    #[error("account `{0}` already exists")]
    AlreadyExists(String),
    #[error("account `{0}` not found")]
    NotFound(String),
    #[error("credential home {0} does not exist")]
    HomeMissing(PathBuf),
    #[error("invalid account name `{0}`")]
    InvalidName(String),
    #[error("refusing to purge {path}: {reason}")]
    UnsafePurge { path: PathBuf, reason: String },
}

impl AccountsError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, AccountsError>;
