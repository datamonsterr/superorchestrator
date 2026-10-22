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
