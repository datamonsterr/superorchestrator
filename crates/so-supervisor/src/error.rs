//! Supervisor errors.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    #[error("another supervisor is already running (pid {})", pid.map_or("unknown".to_string(), |p| p.to_string()))]
    AlreadyRunning { pid: Option<u32> },
    #[error("socket directory {0} is not owned by the current user")]
    InsecureSocketDir(PathBuf),
    #[error("{0} exists and is not a socket")]
    NotASocket(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store error: {0}")]
    Store(#[from] so_store::StoreError),
    #[error("core error: {0}")]
    Core(#[from] so_core::CoreError),
}

pub type Result<T> = std::result::Result<T, SupervisorError>;
