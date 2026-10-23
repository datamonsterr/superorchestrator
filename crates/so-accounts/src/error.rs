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
        "agy account `{0}` is not supported on macOS yet; only the default ~/.gemini works there"
    )]
    AgyMultiAccountUnsupported(String),
    #[error(
        "cannot use a second agy account `{name}` on macOS: `{existing}` is already enabled \
         (agy multi-account is Linux-only, ADR-0005)"
    )]
    AgySecondAccount { name: String, existing: String },
    #[error("binary `{0}` not found on PATH")]
    BinaryNotFound(String),
    #[error("account `{0}` already exists")]
    AlreadyExists(String),
    #[error("account `{0}` not found")]
    NotFound(String),
    #[error("credential home {0} does not exist")]
    HomeMissing(PathBuf),
    #[error("account name `{name}` would shadow {}; pick another name", path.display())]
    ShadowsCommand { name: String, path: PathBuf },
    #[error("invalid account name `{0}`")]
    InvalidName(String),
    #[error("refusing to write wrapper {path}: it is the running executable")]
    WrapperIsSelf { path: PathBuf },
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
