use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("agent file {path}: {msg}")]
    Parse { path: PathBuf, msg: String },
    #[error(
        "invalid agent name `{0}`: use letters, digits, `-`, `_` or `.`, and do not start with `-`"
    )]
    InvalidName(String),
    #[error("no agent named `{0}`")]
    NotFound(String),
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl AgentError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, AgentError>;
