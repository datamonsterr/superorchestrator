//! Errors of the usage crate.

use thiserror::Error;

pub type Result<T> = std::result::Result<T, UsageError>;

#[derive(Debug, Error)]
pub enum UsageError {
    /// The account is signed out or not eligible; the probe printed no usage.
    #[error("account is not eligible or not signed in")]
    NotEligible,
    #[error("probe timed out after {0}s")]
    Timeout(u64),
    #[error("probe failed: {0}")]
    Probe(String),
    #[error("unexpected probe output: {0}")]
    Parse(String),
    #[error(transparent)]
    Store(#[from] so_store::StoreError),
    #[error(transparent)]
    Accounts(#[from] so_accounts::AccountsError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
