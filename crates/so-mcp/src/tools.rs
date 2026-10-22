//! Tool table: name, description, input schema and the supervisor method it forwards to.
//!
//! Every byte here is paid by each orchestrator session in `tools/list`, so descriptions are
//! short and imperative, and the input schemas are trimmed (see [`schema_of`]).

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool, ToolAnnotations};
use schemars::JsonSchema;
use serde_json::Value;
use so_core::proto::*;

/// One MCP tool and the supervisor method it forwards to (always the same name).
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The tool can run for a long time and streams MCP progress while it does.
    pub long: bool,
    pub read_only: bool,
    schema: fn(&str) -> Arc<JsonObject>,
}

impl ToolSpec {
    pub fn tool(&self) -> Tool {
        let mut t = Tool::new(self.name, self.description, (self.schema)(self.name));
        // `readOnlyHint` defaults to false, so only the read-only tools say anything.
        if self.read_only {
            t.annotations = Some(ToolAnnotations::new().read_only(true));
        }
        t
    }
}

/// Parameter descriptions as `(tool, param, text)`. A parameter not listed has none: its name
/// and type say enough. The supervisor validates the values.
const PARAM_DOCS: &[(&str, &str, &str)] = &[
    ("route", "exclude", "choice ids to skip"),
    ("route", "exclude_accounts", "account names to skip"),
    (
        "spawn_agent",
        "prompt",
        "brief: goal, acceptance, out-of-scope, pointers",
    ),
    (
        "spawn_agent",
        "harness",
        "auto (default), claude, codex, opencode, agy",
    ),
    ("spawn_agent", "model", "e.g. a free opencode model"),
    ("spawn_agent", "isolation", "default worktree"),
    ("spawn_agent", "output_schema", "JSON Schema of the result"),
    (
        "spawn_agent",
        "review",
        "hold at pending_review: accept_run or reject_run",
    ),
    ("spawn_agent", "checks", "ci, pr_threads or cmd:<shell>"),
    ("spawn_agent", "timeout", "e.g. 30m"),
    ("send_message", "kind", "answer replies to a question"),
    ("run_workflow", "wait", "default true"),
    ("run_workflow", "fresh", "ignore cached steps"),
    ("report_progress", "step", "what you are doing"),
];

/// The input schema of `T`, trimmed for tokens: refs inlined, nullable types plain, no
/// defaults, format hints or `additionalProperties`, and only the descriptions in
/// [`PARAM_DOCS`] kept. The supervisor still validates every call.
fn schema_of<T: JsonSchema + 'static>(tool: &str) -> Arc<JsonObject> {
    let full = Tool::new("", "", JsonObject::new())
        .with_input_schema::<T>()
        .input_schema;
    let mut root = Value::Object((*full).clone());
    let defs = root.get("$defs").cloned().unwrap_or(Value::Null);
    trim(&mut root, &defs, tool);
    match root {
        Value::Object(o) => Arc::new(o),
        _ => Arc::new(JsonObject::new()),
    }
}

fn trim(node: &mut Value, defs: &Value, tool: &str) {
    let Value::Object(map) = node else { return };
    if let Some(Value::String(r)) = map.get("$ref") {
        let def = r
            .strip_prefix("#/$defs/")
            .and_then(|n| defs.get(n))
            .cloned();
        if let Some(mut inner) = def {
            trim(&mut inner, defs, tool);
            *node = inner;
            return;
        }
    }
    // A union loses its null arm; with one arm left it becomes that arm, with more it stays
    // a union of trimmed arms.
    for key in ["anyOf", "oneOf"] {
        if let Some(Value::Array(arms)) = map.get_mut(key) {
            arms.retain(|a| a["type"] != "null");
            arms.iter_mut().for_each(|a| trim(a, defs, tool));
            if arms.len() == 1 {
                let mut only = arms.remove(0);
                trim(&mut only, defs, tool);
                *node = only;
                return;
            }
        }
    }
    if let Some(Value::Array(arms)) = map.get_mut("allOf") {
        arms.iter_mut().for_each(|a| trim(a, defs, tool));
    }
    for key in [
        "$schema",
        "$defs",
        "title",
        "default",
        "format",
        "minimum",
        "additionalProperties",
        "description",
    ] {
        map.remove(key);
    }
    if let Some(Value::Array(types)) = map.get("type") {
        let plain: Vec<_> = types.iter().filter(|t| *t != "null").collect();
        if let [only] = plain.as_slice() {
            let only = (*only).clone();
            map.insert("type".into(), only);
        }
    }
    if let Some(Value::Object(props)) = map.get_mut("properties") {
        for (name, prop) in props.iter_mut() {
            trim(prop, defs, tool);
            let doc = PARAM_DOCS.iter().find(|(t, p, _)| *t == tool && p == name);
            if let (Some((_, _, text)), Value::Object(o)) = (doc, prop) {
                o.insert("description".into(), Value::String((*text).into()));
            }
        }
    }
    if let Some(items) = map.get_mut("items") {
        trim(items, defs, tool);
    }
}

const fn spec(
    name: &'static str,
    description: &'static str,
    long: bool,
    read_only: bool,
    schema: fn(&str) -> Arc<JsonObject>,
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
const RUN_WORKFLOW_DESCRIPTION: &str = "Run a JS workflow (saved `name` or inline `script`) whose agent() calls are runs on any harness. Streams progress, returns the script's return value; wait=false returns the id, then workflow_status.\n\
Top-level await and return; no import; Date.now, Math.random disabled.\n\
agent(prompt, {harness, account, model, effort, cwd, isolation, schema, name, maxTurns, timeout, stallMs, agent}) -> result; throws on failure.\n\
parallel([fn(ag) | promise]) -> results in order, null for failures; ag defaults to isolation worktree.\n\
pipeline(items, fn(item, i)) -> concurrent, null for failures; pipeline([stage, ...]) runs stages in order.\n\
phase(name, fn); log(msg); args.\n\
Example: return await parallel(args.files.map(f => ag => ag('Test ' + f, {name: f})));";

/// The 14 tools every session gets.
pub const ORCHESTRATOR_TOOLS: &[ToolSpec] = &[
    spec(
        "route",
        "Preview which harness, account and model the router picks. Runs nothing. Optional pins: harness, account, model, effort.",
        false,
        true,
        schema_of::<RouteParams>,
    ),
    spec(
        "spawn_agent",
        "Start a run; unpinned harness, account and model are auto-routed. Returns run_id and state; wait=true blocks until it finishes or reaches the review gate and adds the result.",
        true,
        false,
        schema_of::<SpawnParams>,
    ),
    spec(
        "wait_agent",
        "Block until a run finishes or posts a question or progress, or timeout_ms. Returns finished (state, result), updates (answer questions with send_message kind=answer), timed_out.",
        true,
        true,
        schema_of::<WaitParams>,
    ),
    spec(
        "get_run",
        "Run state, result, branch, worktree, key events, usage. diff=true adds what the worktree changed (stat, patch cut at 64 KiB): review with this, not by reading files. verbose=true: full record, every event.",
        false,
        true,
        schema_of::<GetRunParams>,
    ),
    spec(
        "send_message",
        "Message a run. Resumes a finished run.",
        false,
        false,
        schema_of::<SendMessageParams>,
    ),
    spec(
        "read_messages",
        "Read unread messages for the caller.",
        false,
        false,
        schema_of::<EmptyParams>,
    ),
    spec(
        "cancel_agent",
        "Cancel a run and its subtree.",
        false,
        false,
        schema_of::<RunRef>,
    ),
    spec(
        "accept_run",
        "Accept a run at the review gate. merge=true also merges its branch into the checkout it started from and removes the worktree; refused, run stays at the gate, if that checkout has tracked changes or is on another branch, the worktree is dirty, or the merge conflicts (aborted, files named). Without merge, the result gives branch and merge_command. No-op for isolation none.",
        false,
        false,
        schema_of::<AcceptParams>,
    ),
    spec(
        "reject_run",
        "Reject a run at the review gate; it continues with the feedback.",
        false,
        false,
        schema_of::<RejectParams>,
    ),
    spec(
        "fork_agent",
        "Fork a finished run into a new run with a new prompt.",
        false,
        false,
        schema_of::<ForkParams>,
    ),
    spec(
        "list_harnesses",
        "Call first. Accounts (signed_in, headroom, load, cooldown), routing choices (eligible, free) and free models.",
        false,
        true,
        schema_of::<EmptyParams>,
    ),
    spec(
        "list_agents",
        "List custom agents for spawn_agent.",
        false,
        true,
        schema_of::<EmptyParams>,
    ),
    spec(
        "run_workflow",
        RUN_WORKFLOW_DESCRIPTION,
        true,
        false,
        schema_of::<RunWorkflowParams>,
    ),
    spec(
        "workflow_status",
        "Workflow run state, steps (with their runs) and result. Takes an id prefix.",
        false,
        true,
        schema_of::<WorkflowStatusParams>,
    ),
];

/// The 2 extra tools a subagent (a process with `SUPERORCHESTRATOR_RUN_ID`) gets.
pub const SUBAGENT_TOOLS: &[ToolSpec] = &[
    spec(
        "report_progress",
        "Report a step and its status to the parent run.",
        false,
        false,
        schema_of::<ReportProgressParams>,
    ),
    spec(
        "ask_parent",
        "Ask the parent run a question and wait for the answer.",
        false,
        false,
        schema_of::<AskParentParams>,
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
        assert!(desc("get_run").contains("verbose=true"));
        assert!(desc("accept_run").contains("merge=true"));
    }

    #[test]
    fn schemas_are_trimmed_but_keep_names_types_and_required() {
        let tool = |n: &str| {
            tool_specs(true)
                .find(|t| t.name == n)
                .unwrap()
                .tool()
                .input_schema
        };
        let spawn = Value::Object((*tool("spawn_agent")).clone());
        let text = spawn.to_string();
        for noise in [
            "$schema",
            "$defs",
            "$ref",
            "anyOf",
            "\"default\":",
            "additionalProperties",
            "uint32",
        ] {
            assert!(!text.contains(noise), "{noise} in {text}");
        }
        assert_eq!(
            spawn["properties"]["isolation"]["enum"],
            serde_json::json!(["worktree", "none"])
        );
        assert_eq!(spawn["properties"]["max_turns"]["type"], "integer");
        assert_eq!(spawn["properties"]["wait"]["type"], "boolean");
        assert_eq!(spawn["properties"]["checks"]["items"]["type"], "string");
        let get = Value::Object((*tool("get_run")).clone());
        assert_eq!(get["required"], serde_json::json!(["run"]));
        assert!(get["properties"]["verbose"].is_object());
    }

    #[test]
    fn every_tool_schema_is_self_contained() {
        fn walk(v: &Value, tool: &str) {
            match v {
                Value::Object(m) => {
                    assert!(!m.contains_key("$ref"), "{tool}: $ref");
                    assert!(!m.contains_key("$defs"), "{tool}: $defs");
                    m.values().for_each(|c| walk(c, tool));
                }
                Value::Array(a) => a.iter().for_each(|c| walk(c, tool)),
                _ => {}
            }
        }
        for spec in tool_specs(true) {
            let schema = Value::Object((*spec.tool().input_schema).clone());
            walk(&schema, spec.name);
            assert_eq!(schema["type"], "object", "{}", spec.name);
        }
        // Tools with required parameters keep them.
        for (tool, required) in [
            ("route", "prompt"),
            ("wait_agent", "runs"),
            ("get_run", "run"),
            ("send_message", "text"),
            ("accept_run", "run"),
            ("reject_run", "feedback"),
            ("fork_agent", "prompt"),
            ("workflow_status", "id"),
            ("ask_parent", "question"),
        ] {
            let t = tool_specs(true).find(|t| t.name == tool).unwrap().tool();
            let req = t.input_schema["required"].as_array().unwrap();
            assert!(req.contains(&Value::from(required)), "{tool}.{required}");
        }
    }

    #[test]
    fn a_union_with_several_arms_is_kept_and_a_nullable_one_is_narrowed() {
        let defs = serde_json::json!({"X": {"type": "string", "enum": ["a"]}});
        let mut one = serde_json::json!({"anyOf": [{"$ref": "#/$defs/X"}, {"type": "null"}]});
        trim(&mut one, &defs, "t");
        assert_eq!(one, serde_json::json!({"type": "string", "enum": ["a"]}));
        let mut many = serde_json::json!({"anyOf": [{"type": "string"}, {"type": "integer", "minimum": 0}, {"type": "null"}]});
        trim(&mut many, &defs, "t");
        assert_eq!(
            many,
            serde_json::json!({"anyOf": [{"type": "string"}, {"type": "integer"}]})
        );
    }

    #[test]
    fn every_param_doc_names_a_real_parameter() {
        for (tool, param, _) in PARAM_DOCS {
            let t = tool_specs(true).find(|t| t.name == *tool).unwrap().tool();
            assert!(
                t.input_schema["properties"].get(param).is_some(),
                "{tool}.{param} is not a parameter"
            );
        }
    }
}
