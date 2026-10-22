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

/// What a workflow script can call, from `so_workflow::GLOBALS` and `prelude.js`.
const RUN_WORKFLOW_DESCRIPTION: &str = "Run a JavaScript workflow (a saved one by name, or an inline `script`) whose agent() calls are runs on any harness. Streams progress and returns the script's return value; with wait=false it returns the id at once, then follow it with workflow_status.\n\
Script API (top-level await and return work; no import; Date.now() and Math.random() are disabled):\n\
- agent(prompt, opts) -> the run's result; throws if the run fails. opts: harness, account, model, effort, cwd, isolation ('worktree'|'none'), schema (JSON schema for the result), name, maxTurns, timeout, stallMs, agent (custom agent name).\n\
- parallel([fn | promise, ...]) -> results in order, null for a failed agent. Each fn(ag) gets an ag() that defaults to isolation 'worktree'.\n\
- pipeline(items, fn) -> fn(item, i) per item concurrently, null for failures; pipeline([stage, ...]) runs stages in order, each given the previous result.\n\
- phase(name, fn) groups steps in progress output; log(msg) writes a progress line; args is the `args` param.\n\
Example:\n\
const [a, b] = await parallel(args.files.map(f => ag => ag('Add tests for ' + f, {harness: 'codex', name: f})));\n\
return await phase('review', async () => {\n\
  log('reviewing');\n\
  return agent('Review ' + JSON.stringify([a, b]), {schema: {type: 'object'}});\n\
});";

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
        "Get a run's state, recent events, final result, usage, worktree and join command. With diff=true and a worktree, also what the run changed: diff = {base, branch, stat, patch} (committed and uncommitted changes, patch cut at 64 KiB with truncated=true). Review a run with this instead of reading its files.",
        false,
        true,
        schema::<GetRunParams>,
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
        "Accept a run that is waiting at the review gate. With merge=true, also merges the run's branch into the checkout it was started from (fast-forward when possible, else a merge commit) and removes the worktree. The merge is refused, and the run stays at the gate, when that checkout has tracked changes or is on another branch, when the run's worktree has uncommitted changes, or on a conflict (aborted; the error names the files). Without merge, the result gives branch and merge_command for the unmerged work. merge is a no-op for isolation none.",
        false,
        false,
        schema::<AcceptParams>,
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
        RUN_WORKFLOW_DESCRIPTION,
        true,
        false,
        schema::<RunWorkflowParams>,
    ),
    spec(
        "workflow_status",
        "Get a workflow run's state, its steps (with their runs) and its result. Accepts an id prefix.",
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn run_workflow_description_names_every_script_global() {
        for name in so_workflow::GLOBALS {
            assert!(
                RUN_WORKFLOW_DESCRIPTION.contains(&format!("{name}(")) || *name == "args",
                "{name} is not documented"
            );
        }
        assert!(RUN_WORKFLOW_DESCRIPTION.contains("args"));
    }

    #[test]
    fn get_run_and_accept_run_document_their_flags() {
        let desc = |n: &str| {
            ORCHESTRATOR_TOOLS
                .iter()
                .find(|t| t.name == n)
                .unwrap()
                .description
        };
        assert!(desc("get_run").contains("diff=true"));
        assert!(desc("accept_run").contains("merge=true"));
    }
}
