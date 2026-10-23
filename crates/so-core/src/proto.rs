//! JSON-RPC 2.0 over newline-delimited JSON between clients and the supervisor.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{Choice, ErrorKind, Event, Harness, Headroom, Isolation, MessageKind, RunState};

pub const JSONRPC: &str = "2.0";

// Standard JSON-RPC error codes.
pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
// Application error codes.
pub const DEPTH_LIMIT: i64 = -32001;
pub const CAP_EXCEEDED: i64 = -32002;
pub const NOT_FOUND: i64 = -32003;
pub const UNAVAILABLE: i64 = -32004;
/// A call took longer than its deadline.
pub const TIMEOUT: i64 = -32005;
/// A configured routing decider could not be reached (after its fallback model).
pub const DECIDER_UNREACHABLE: i64 = -32006;

// Method names.
pub const METHOD_HELLO: &str = "hello";
pub const METHOD_STATUS: &str = "status";
pub const METHOD_ROUTE: &str = "route";
pub const METHOD_SPAWN_AGENT: &str = "spawn_agent";
pub const METHOD_WAIT_AGENT: &str = "wait_agent";
pub const METHOD_GET_RUN: &str = "get_run";
pub const METHOD_SEND_MESSAGE: &str = "send_message";
pub const METHOD_READ_MESSAGES: &str = "read_messages";
pub const METHOD_CANCEL_AGENT: &str = "cancel_agent";
pub const METHOD_ACCEPT_RUN: &str = "accept_run";
pub const METHOD_REJECT_RUN: &str = "reject_run";
pub const METHOD_FORK_AGENT: &str = "fork_agent";
pub const METHOD_LIST_HARNESSES: &str = "list_harnesses";
pub const METHOD_LIST_AGENTS: &str = "list_agents";
pub const METHOD_RUN_WORKFLOW: &str = "run_workflow";
pub const METHOD_WORKFLOW_STATUS: &str = "workflow_status";
/// An extension of SPEC 13 (which lists no way to stop a workflow): not an MCP tool, only
/// the CLI (`workflow cancel`) and the web API use it.
pub const METHOD_CANCEL_WORKFLOW: &str = "cancel_workflow";
pub const METHOD_REPORT_PROGRESS: &str = "report_progress";
pub const METHOD_ASK_PARENT: &str = "ask_parent";
/// Returns the command a person runs to join a run (CLI `join`).
pub const METHOD_JOIN_RUN: &str = "join_run";
/// Starts the web UI inside the supervisor (CLI `ui`).
pub const METHOD_UI_START: &str = "ui_start";

fn jsonrpc() -> String {
    JSONRPC.to_string()
}

/// A JSON-RPC request. `id` is a number or a string.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Request {
    #[serde(default = "jsonrpc")]
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(
        id: impl Into<Value>,
        method: &str,
        params: impl Serialize,
    ) -> serde_json::Result<Self> {
        Ok(Self {
            jsonrpc: jsonrpc(),
            id: id.into(),
            method: method.to_string(),
            params: serde_json::to_value(params)?,
        })
    }
}

/// A JSON-RPC notification: a request without an id and without a response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notification {
    #[serde(default = "jsonrpc")]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// A JSON-RPC response carrying either `result` or `error`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default = "jsonrpc")]
    pub jsonrpc: String,
    pub id: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl Response {
    pub fn ok(id: Value, result: impl Serialize) -> serde_json::Result<Self> {
        Ok(Self {
            jsonrpc: jsonrpc(),
            id,
            result: Some(serde_json::to_value(result)?),
            error: None,
        })
    }

    pub fn err(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: jsonrpc(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

// ---------------------------------------------------------------- params

#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HelloParams {
    pub client: String,
    pub cwd: String,
    #[serde(default)]
    pub run_id: Option<String>,
    /// Secret proof for `run_id`; never included in a hello result or public run view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_token: Option<String>,
    #[serde(default)]
    pub pid: Option<u32>,
    /// Read or control calls only: no session row is opened (SPEC 3.1: only `mcp` opens a
    /// session at startup), so `spawn_agent` and `run_workflow` are refused on the connection.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub inspect: bool,
}

impl std::fmt::Debug for HelloParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HelloParams")
            .field("client", &self.client)
            .field("cwd", &self.cwd)
            .field("run_id", &self.run_id)
            .field("run_token", &self.run_token.as_ref().map(|_| "[REDACTED]"))
            .field("pid", &self.pid)
            .field("inspect", &self.inspect)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HelloResult {
    pub session_id: String,
    #[serde(default)]
    pub run_id: Option<String>,
    pub depth: u32,
    /// Build id of the daemon's executable (see `buildinfo`); absent from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    /// Set when the daemon's executable was replaced on disk after it started.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusResult {
    pub pid: u32,
    pub version: String,
    pub uptime_s: u64,
    pub sessions: u32,
    pub active_runs: u32,
    /// Build id of the daemon's executable; absent from older daemons.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<String>,
    /// The daemon's executable was replaced on disk after it started: it runs old code.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RouteParams {
    pub prompt: String,
    #[serde(default)]
    pub role: Option<String>,
    /// Choice ids to drop from the eligible set.
    #[serde(default)]
    pub exclude: Vec<String>,
    /// Account names to skip (failover). A choice left with no account is ineligible.
    #[serde(default)]
    pub exclude_accounts: Vec<String>,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RouteSource {
    Jev,
    Heuristic,
    Pinned,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteAlternative {
    pub choice_id: String,
    #[serde(default)]
    pub account: Option<String>,
    /// Source-dependent, so scores are not comparable across results: for `jev` it is the
    /// decider's probability for the choice, for `heuristic` and `pinned` the heuristic score.
    #[serde(default)]
    pub score: Option<f64>,
}

/// Result of `route` (SPEC 8.3 step 6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteResult {
    pub choice_id: String,
    pub harness: Harness,
    pub account: String,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
    pub source: RouteSource,
    pub rationale: String,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub probabilities: BTreeMap<String, f64>,
    #[serde(default)]
    pub usage: RouteUsage,
    #[serde(default)]
    pub alternatives: Vec<RouteAlternative>,
    #[serde(default)]
    pub config_fingerprint: String,
    /// The decider model that produced the answer (the response's model id when it gave one).
    /// Also set when its answer was rejected; absent for pinned and key-less routing.
    #[serde(default)]
    pub decider_model: Option<String>,
    /// The decider models in effect for this call and where each came from. Absent when no
    /// decider was configured (no key) and for pinned calls.
    #[serde(default)]
    pub decider_models: Option<crate::config::JevModels>,
    /// Usable accounts of the chosen choice, best first (`account` is the first). Failover
    /// walks this list, then re-routes with `exclude_accounts`.
    #[serde(default)]
    pub ranked_accounts: Vec<String>,
}

/// Parameters of `spawn_agent` (SPEC 7.2).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SpawnParams {
    /// Task brief; required unless `agent` supplies one.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Custom agent name.
    #[serde(default)]
    pub agent: Option<String>,
    /// `auto` (default) or a harness name.
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Relative to the caller's cwd; subagents must stay inside their run's worktree/cwd.
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub isolation: Option<Isolation>,
    /// JSON Schema for the final result.
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub review: bool,
    /// `"ci"`, `"pr_threads"` or `"cmd:<shell command>"`. Only depth-0 callers may use `cmd:`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    /// Block until the run finishes.
    #[serde(default, skip_serializing_if = "is_false")]
    pub wait: bool,
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// Duration string such as `30m`.
    #[serde(default)]
    pub timeout: Option<String>,
    /// Short label for the UI.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SpawnResult {
    pub run_id: String,
    pub state: RunState,
    /// Final result, present when `wait` was set and the run finished.
    #[serde(default)]
    pub result: Option<Value>,
    /// Non-fatal notes for the caller, e.g. a dirty tree or a missing git repository.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
    /// Why the run failed, when `wait` was set and it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WaitParams {
    pub runs: Vec<String>,
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaitResult {
    /// Run ids that finished.
    #[serde(default)]
    pub finished: Vec<String>,
    /// Progress and question events posted while waiting.
    #[serde(default)]
    pub updates: Vec<Event>,
    #[serde(default)]
    pub timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunRef {
    pub run: String,
}

/// Params of `get_run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetRunParams {
    /// Run id or unique prefix.
    pub run: String,
    /// Include `diff`: what the run changed in its worktree (default false).
    #[serde(default)]
    pub diff: bool,
    /// MCP only: return the full record instead of the short view. The supervisor ignores it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub verbose: bool,
}

/// Params of `accept_run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AcceptParams {
    /// Run id or unique prefix.
    pub run: String,
    /// Also merge the run's branch into the branch the base checkout is on (default false).
    /// Refused, with the run left at the review gate, when the base checkout has tracked
    /// changes or is on another branch, or when the merge conflicts (it is aborted).
    #[serde(default)]
    pub merge: bool,
}

/// Result of `accept_run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AcceptResult {
    pub ok: bool,
    /// The run's branch was merged by this call.
    #[serde(default)]
    pub merged: bool,
    /// The run's branch when its work is still unmerged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The command that merges `branch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub merge_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What a run changed: `get_run` with `diff: true`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunDiff {
    /// The commit the diff starts from (the merge base of the run's branch).
    pub base: String,
    pub branch: String,
    /// `git diff --stat`, plus a line per untracked file.
    pub stat: String,
    /// Committed and uncommitted changes, truncated at 64 KiB.
    pub patch: String,
    #[serde(default)]
    pub truncated: bool,
    /// Why the diff could not be made (the other fields are then empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UsageTotals {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunView {
    pub run_id: String,
    /// Session the run belongs to.
    #[serde(default)]
    pub session_id: String,
    pub state: RunState,
    #[serde(default)]
    pub harness: Option<Harness>,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub recent_events: Vec<Event>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub usage: UsageTotals,
    #[serde(default)]
    pub worktree: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub join_command: Option<String>,
    /// What the run changed (`get_run` with `diff: true`, for a run with a worktree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diff: Option<RunDiff>,
    /// Directory the join command runs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join_cwd: Option<String>,
    /// Names (never values) of the environment variables the join command sets.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub join_env: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub depth: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_kind: Option<ErrorKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Messages sent to or from the run, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageView>,
}

/// Result of `join_run`: the command a person runs to join a run. Secrets travel only in
/// `env`; `Debug` prints names only.
#[derive(Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JoinResult {
    pub argv: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
    pub cwd: String,
}

impl std::fmt::Debug for JoinResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinResult")
            .field("argv", &self.argv)
            .field(
                "env_keys",
                &self.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            )
            .field("cwd", &self.cwd)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendMessageParams {
    pub run: String,
    pub text: String,
    #[serde(default)]
    pub kind: Option<MessageKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MessageView {
    pub id: String,
    #[serde(default)]
    pub from_run: Option<String>,
    #[serde(default)]
    pub to_run: Option<String>,
    pub kind: MessageKind,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReadMessagesResult {
    #[serde(default)]
    pub messages: Vec<MessageView>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AckResult {
    pub ok: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectParams {
    pub run: String,
    pub feedback: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkParams {
    pub run: String,
    pub prompt: String,
}

/// Sign-in state from an auth-file existence check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInState {
    SignedIn,
    SignedOut,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountView {
    pub name: String,
    pub harness: Harness,
    pub signed_in: SignInState,
    pub enabled: bool,
    pub cap: u32,
    /// Slot-holding runs plus active interactive sessions.
    pub active_runs: u32,
    #[serde(default)]
    pub headroom: Vec<Headroom>,
    /// Set when this account could not be inspected (e.g. an unreadable account file);
    /// the other fields are then placeholders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Routing priority (lower is tried first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
    /// Quota pools of the account (agy: gemini and claude).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pools: Option<Vec<String>>,
    /// Active manual or automatic cooldowns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<Vec<AccountFlag>>,
}

/// A cooldown on an account (or one of its pools).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AccountFlag {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
    pub until: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Parameters of `ui_start`; unset fields fall back to `[ui]` in `config.toml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiStartParams {
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub token: Option<String>,
}

/// Result of `ui_start`: where the UI listens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UiStartResult {
    pub url: String,
    /// True when the UI was already running (the parameters were then ignored).
    pub already_running: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessesView {
    #[serde(default)]
    pub accounts: Vec<AccountView>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Set when the daemon's executable was replaced after it started; `stale_reason` says
    /// what to do.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentView {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Where the agent was found: `user`, `superorchestrator` or `claude` (read-only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The harness the agent asks for (`auto` or a harness name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentsView {
    #[serde(default)]
    pub agents: Vec<AgentView>,
}

fn yes() -> bool {
    true
}

/// Parameters of `run_workflow` (SPEC 11): a saved workflow by `name`, or an inline `script`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunWorkflowParams {
    /// A saved workflow (`<config>/workflows/<name>.js` or `.superorchestrator/workflows`).
    /// Subagent callers may use only workflows from the user config directory.
    #[serde(default)]
    pub name: Option<String>,
    /// Inline JavaScript instead of a saved one. Only depth-0 callers may supply this.
    #[serde(default)]
    pub script: Option<String>,
    /// Passed to the script as `args`.
    #[serde(default)]
    pub args: Option<Value>,
    /// Block until the workflow finishes (default). With `false` the id comes back at once;
    /// follow it with `workflow_status`.
    #[serde(default = "yes")]
    pub wait: bool,
    /// Ignore cached steps and run everything again (new results are still cached).
    #[serde(default)]
    pub fresh: bool,
}

impl Default for RunWorkflowParams {
    fn default() -> Self {
        Self {
            name: None,
            script: None,
            args: None,
            wait: true,
            fresh: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunWorkflowResult {
    pub id: String,
    /// `running`, `succeeded`, `failed` or `cancelled`.
    #[serde(default)]
    pub state: Option<String>,
    /// The script's return value, when it finished successfully.
    #[serde(default)]
    pub result: Option<Value>,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowStatusParams {
    /// A workflow run id or a unique prefix of it.
    pub id: String,
}

/// One `agent()` call of a workflow run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStep {
    /// Order in which the script started the step.
    pub index: u32,
    /// The phase the step ran in, or `step N`.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    /// `running`, `waiting`, `stalled`, `succeeded`, `failed`, `cached` or `cancelled`.
    pub state: String,
    /// The run behind the step; none for a cached step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// A workflow run in a list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowRunView {
    pub id: String,
    pub name: String,
    pub state: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// State, steps and result of a workflow run (`workflow_status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowStatusResult {
    pub id: String,
    #[serde(default)]
    pub name: String,
    /// `running`, `succeeded`, `failed` or `cancelled`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

/// The payload of the `workflow` message on the web stream: a run changed state or one of
/// its steps did.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowEventData {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<WorkflowStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub ts: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReportProgressParams {
    pub step: String,
    pub status: String,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AskParentParams {
    pub question: String,
    #[serde(default)]
    pub options: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AskParentResult {
    /// The answer text, or `timeout` when `runs.ask_timeout` elapsed.
    pub answer: String,
    #[serde(default)]
    pub timed_out: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let r = Request::new(1, METHOD_GET_RUN, RunRef { run: "r1".into() }).unwrap();
        let line = serde_json::to_string(&r).unwrap();
        assert!(!line.contains('\n'));
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.jsonrpc, "2.0");
        let p: RunRef = serde_json::from_value(back.params).unwrap();
        assert_eq!(p.run, "r1");
    }

    #[test]
    fn response_result_and_error_shapes() {
        let ok = Response::ok(Value::from("a"), AckResult { ok: true }).unwrap();
        let s = serde_json::to_string(&ok).unwrap();
        assert!(s.contains("\"result\"") && !s.contains("\"error\""));
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), ok);

        let err = Response::err(Value::from(2), RpcError::new(NOT_FOUND, "no such run"));
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("-32003") && !s.contains("\"result\""));
        assert_eq!(serde_json::from_str::<Response>(&s).unwrap(), err);
    }

    #[test]
    fn notification_has_no_id() {
        let n = Notification {
            jsonrpc: jsonrpc(),
            method: "event".into(),
            params: serde_json::json!({"a": 1}),
        };
        let s = serde_json::to_string(&n).unwrap();
        assert!(!s.contains("\"id\""));
        assert_eq!(serde_json::from_str::<Notification>(&s).unwrap(), n);
    }

    #[test]
    fn error_codes() {
        assert_eq!(
            [
                PARSE_ERROR,
                INVALID_REQUEST,
                METHOD_NOT_FOUND,
                INVALID_PARAMS,
                INTERNAL_ERROR
            ],
            [-32700, -32600, -32601, -32602, -32603]
        );
        assert_eq!(
            [DEPTH_LIMIT, CAP_EXCEEDED, NOT_FOUND, UNAVAILABLE],
            [-32001, -32002, -32003, -32004]
        );
    }

    #[test]
    fn params_reject_unknown_fields() {
        assert!(serde_json::from_str::<SpawnParams>(r#"{"prompt":"x","bogus":1}"#).is_err());
        let p: SpawnParams =
            serde_json::from_str(r#"{"prompt":"x","isolation":"none","review":true}"#).unwrap();
        assert_eq!(p.isolation, Some(Isolation::None));
        assert!(p.review && !p.wait);
    }

    #[test]
    fn spawn_params_schema_generation() {
        let schema = schemars::schema_for!(SpawnParams);
        let v = serde_json::to_value(&schema).unwrap();
        let props = v["properties"].as_object().unwrap();
        for k in [
            "prompt",
            "agent",
            "harness",
            "account",
            "model",
            "effort",
            "cwd",
            "isolation",
            "output_schema",
            "review",
            "checks",
            "wait",
            "max_turns",
            "timeout",
            "name",
        ] {
            assert!(props.contains_key(k), "missing {k}");
        }
        assert_eq!(v["additionalProperties"], serde_json::json!(false));
    }

    #[test]
    fn route_result_round_trip() {
        let r = RouteResult {
            choice_id: "c".into(),
            harness: Harness::Agy,
            account: "agy-a".into(),
            model: "m".into(),
            effort: None,
            source: RouteSource::Heuristic,
            rationale: "why".into(),
            confidence: None,
            probabilities: BTreeMap::new(),
            usage: RouteUsage::default(),
            alternatives: vec![],
            config_fingerprint: "f".into(),
            decider_model: None,
            decider_models: None,
            ranked_accounts: vec![],
        };
        let j = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<RouteResult>(&j).unwrap(), r);
        assert!(j.contains("\"source\":\"heuristic\""));
    }

    #[test]
    fn hello_debug_never_discloses_run_token() {
        let hello: super::HelloParams = serde_json::from_value(serde_json::json!({
            "client": "test", "cwd": "/", "run_id": "r", "run_token": "private-test-token"
        }))
        .unwrap();
        let debug = format!("{hello:?}");
        assert!(!debug.contains("private-test-token"));
        assert!(debug.contains("[REDACTED]"));
    }
}
