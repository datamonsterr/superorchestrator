//! Runs: the run manager and the tools built on it (SPEC 7, 13).

mod handlers;
mod instructions;
mod manager;
mod pump;
pub mod worktree;

use std::sync::Arc;

pub use manager::{RunManager, WaitOutcome, display_argv};

use crate::server::SupervisorState;

/// Registers the run tools and connects the manager to the supervisor's activity probe
/// and reconcile hook.
pub(crate) fn register(state: &Arc<SupervisorState>) {
    handlers::register(state);
    state.set_probe(Arc::new(state.runs.clone()));
    state.set_reconcile_hook(Arc::new(state.runs.clone()));
}
