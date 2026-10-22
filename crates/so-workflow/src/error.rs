use so_store::StoreError;

#[derive(Debug, thiserror::Error)]
pub enum WorkflowError {
    /// The script threw, failed to compile, or an `agent()` call failed and was not handled.
    #[error("{0}")]
    Script(String),
    #[error("workflow cancelled")]
    Cancelled,
    #[error("workflow exceeded its maximum duration")]
    Timeout,
    #[error("saved workflow not found: {0}")]
    NotFound(String),
    #[error("{0}")]
    Io(String),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("engine error: {0}")]
    Engine(String),
}

pub type Result<T> = std::result::Result<T, WorkflowError>;
