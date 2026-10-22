//! Tool table: name, description, input schema and the supervisor method it forwards to.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use so_core::proto::*;

/// One MCP tool and the supervisor method it forwards to (always the same name).
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The tool can run for a long time and streams MCP progress while it does.
    pub long: bool,
    pub read_only: bool,
    schema: fn() -> Arc<JsonObject>,
}

impl ToolSpec {
    pub fn tool(&self) -> Tool {
        let mut t = Tool::new(self.name, self.description, (self.schema)());
        t.annotations = Some(ToolAnnotations::new().read_only(self.read_only));
        t
    }
}

fn schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    Tool::new("", "", JsonObject::new())
        .with_input_schema::<T>()
        .input_schema
}

const fn spec(
    name: &'static str,
    description: &'static str,
    long: bool,
    read_only: bool,
    schema: fn() -> Arc<JsonObject>,
) -> ToolSpec {
    ToolSpec {
        name,
        description,
        long,
        read_only,
        schema,
    }
}

/// The 14 tools every session gets.
pub const ORCHESTRATOR_TOOLS: &[ToolSpec] = &[
    spec(
        "route",
        "Preview which harness, account and model the router would pick for a prompt. Runs nothing.",
        false,
        true,
        schema::<RouteParams>,
    ),
    spec(
        "spawn_agent",
        "Start a subagent run on a harness and account. Returns the run id, or the final result when wait is true.",
        true,
        false,
        schema::<SpawnParams>,
    ),
    spec(
        "wait_agent",
        "Wait until any listed run finishes or posts a question or progress, with a timeout. Streams progress notifications.",
        true,
        true,
        schema::<WaitParams>,
    ),
    spec(
        "get_run",
        "Get a run's state, recent events, final result, usage, worktree and join command.",
        false,
        true,
        schema::<RunRef>,
    ),
    spec(
        "send_message",
        "Send a message to a run. Resumes a finished run.",
        false,
        false,
        schema::<SendMessageParams>,
    ),
    spec(
        "read_messages",
        "Read the unread messages addressed to the caller.",
        false,
        false,
        schema::<EmptyParams>,
    ),
    spec(
        "cancel_agent",
        "Cancel a run and its subtree.",
        false,
        false,
        schema::<RunRef>,
    ),
    spec(
        "accept_run",
        "Accept a run that is waiting at the review gate.",
        false,
        false,
        schema::<RunRef>,
    ),
    spec(
        "reject_run",
        "Reject a run at the review gate with feedback; the run continues with the feedback.",
        false,
        false,
        schema::<RejectParams>,
    ),
    spec(
        "fork_agent",
        "Fork a finished run into a new run with a new prompt.",
        false,
        false,
        schema::<ForkParams>,
    ),
    spec(
        "list_harnesses",
        "List accounts, signed-in state, headroom, active runs and routing choices.",
        false,
        true,
        schema::<EmptyParams>,
    ),
    spec(
        "list_agents",
        "List the custom agents available to spawn_agent.",
        false,
        true,
        schema::<EmptyParams>,
    ),
    spec(
        "run_workflow",
        "Run a workflow by name or inline script and return its result.",
        true,
        false,
        schema::<RunWorkflowParams>,
    ),
    spec(
        "workflow_status",
        "Get the state and result of a workflow run.",
        false,
        true,
        schema::<WorkflowStatusParams>,
    ),
];

/// The 2 extra tools a subagent (a process with `SUPERORCHESTRATOR_RUN_ID`) gets.
pub const SUBAGENT_TOOLS: &[ToolSpec] = &[
    spec(
        "report_progress",
        "Report a step and its status to the parent run.",
        false,
        false,
        schema::<ReportProgressParams>,
    ),
    spec(
        "ask_parent",
        "Ask the parent run a question and wait for the answer.",
        false,
        false,
        schema::<AskParentParams>,
    ),
];

/// The tools listed for a session.
pub fn tool_specs(subagent: bool) -> impl Iterator<Item = &'static ToolSpec> {
    ORCHESTRATOR_TOOLS
        .iter()
        .chain(SUBAGENT_TOOLS.iter().filter(move |_| subagent))
}
