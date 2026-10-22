//! The boundary between the workflow engine and the supervisor.

use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};
use serde_json::Value;

/// Boxed future returned by [`WorkflowHost`] methods (keeps the trait object-safe).
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A host-side failure of one step (spawn error, run failed, ...).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct HostError(pub String);

/// One `agent()` call, as handed to the host.
#[derive(Debug, Clone, PartialEq)]
pub struct StepRequest {
    pub workflow_run_id: String,
    /// Order in which the step was started within this workflow run.
    pub step_index: usize,
    pub phase: Option<String>,
    pub prompt: String,
    /// SPEC 7.2 spawn options with snake_case keys (`output_schema`, `max_turns`, ...).
    pub opts: Value,
    /// Stall window from the script's `stallMs` option; the host detects stalls.
    pub stall_ms: Option<u64>,
}

/// How a spawn attempt ended.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// The run finished; `result` is its final result (object or string).
    Done {
        result: Value,
        run_id: Option<String>,
    },
    /// No account can take the step before `retry_at` (SPEC 8.5).
    Unavailable { retry_at: DateTime<Utc> },
    /// The run produced no output for the stall window.
    Stalled { run_id: Option<String> },
}

/// Progress of one step, for `workflow_status` and `runs watch`.
#[derive(Debug, Clone, PartialEq)]
pub struct StepEvent {
    pub workflow_run_id: String,
    pub step_index: usize,
    pub phase: Option<String>,
    pub prompt_hash: String,
    pub run_id: Option<String>,
    /// `running`, `waiting`, `stalled`, `succeeded`, `failed` or `cached`.
    pub state: String,
}

/// What the engine needs from the supervisor. All methods are called from the engine's
/// task; implementations must be cheap to call concurrently.
pub trait WorkflowHost: Send + Sync {
    /// Spawns a run for the step and waits for its final result.
    fn spawn_and_wait<'a>(
        &'a self,
        req: &'a StepRequest,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>>;

    /// Restarts a stalled step (`attempt` counts restarts, from 1) and waits for the new run.
    fn restart<'a>(
        &'a self,
        req: &'a StepRequest,
        stalled_run_id: Option<&'a str>,
        attempt: u32,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>>;

    fn log<'a>(&'a self, msg: &'a str) -> BoxFut<'a, ()>;
    fn phase_started<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()>;
    fn phase_finished<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()>;
    fn step_status<'a>(&'a self, event: &'a StepEvent) -> BoxFut<'a, ()>;
}
