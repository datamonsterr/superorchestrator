//! Error type shared by the core modules.

use std::path::PathBuf;

/// Errors raised by `so-core`.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse {path}: {source}")]
    TomlDe {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot serialise toml: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error(
        "invalid account name `{0}`: must match ^[a-z][a-z0-9-]{{1,31}}$ and not be a harness name"
    )]
    InvalidAccountName(String),
    #[error("unknown config key `{0}`")]
    InvalidConfigKey(String),
    #[error("invalid value for `{key}`: {reason}")]
    InvalidValue { key: String, reason: String },
    #[error("invalid duration `{0}`: expected forms like 20s, 5m, 5h, 7d")]
    InvalidDuration(String),
    #[error("unknown {kind} `{value}`")]
    UnknownValue { kind: &'static str, value: String },
    #[error("invalid account file {path}: {reason}")]
    InvalidAccountFile { path: PathBuf, reason: String },
    #[error("cannot edit toml {path}: {message}")]
    TomlEdit { path: PathBuf, message: String },
    #[error("unknown harness `{0}`")]
    UnknownHarness(String),
    #[error("cannot determine the home directory (HOME is unset)")]
    NoHomeDir,
}

impl CoreError {
    pub fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
