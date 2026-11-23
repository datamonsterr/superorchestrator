//! Operations shared by the CLI and the web API, so neither duplicates the other.

pub mod accounts;
pub mod decider;
pub mod insights;
pub mod models;
pub mod sessions;
pub mod usage;
pub mod worktrees;

/// Serializes supervisor transactions that update config.toml and associated files.
pub fn config_write_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
