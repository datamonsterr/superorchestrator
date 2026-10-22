//! QuickJS runtime, script API and result cache (SPEC section 11).
//!
//! `parallel()` defaults agents to `isolation: "worktree"` only for calls made through the
//! `agent` argument passed to each item function, or synchronously while the item function
//! starts. Calls after an `await` in the item, and promises created before `parallel()`,
//! keep their own options.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod engine;
mod error;
mod host;
mod saved;
mod script;
mod step;
#[cfg(feature = "test-util")]
pub mod testing;

pub use engine::{CancelToken, EngineOptions, RunOptions, WorkflowEngine};
pub use error::{Result, WorkflowError};
pub use host::{BoxFut, HostError, StepEvent, StepOutcome, StepRequest, WorkflowHost};
pub use saved::{SavedSource, SavedWorkflow, SavedWorkflows};

/// The globals a workflow script can use (defined by `prelude.js`). The `run_workflow` tool
/// description documents each.
pub const GLOBALS: &[&str] = &["agent", "parallel", "pipeline", "phase", "log", "args"];

#[cfg(test)]
mod tests {
    use super::GLOBALS;

    #[test]
    fn globals_match_what_the_prelude_defines() {
        let prelude = include_str!("prelude.js");
        let mut defined: Vec<&str> = prelude
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("g."))
            .filter_map(|r| r.split(' ').next())
            // `Date` is patched, not added; `__drain` is internal.
            .filter(|n| !n.starts_with("__") && *n != "Date")
            .collect();
        defined.sort_unstable();
        let mut listed = GLOBALS.to_vec();
        listed.sort_unstable();
        assert_eq!(defined, listed);
    }
}
