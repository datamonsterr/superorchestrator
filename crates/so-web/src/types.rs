//! Types of the `/api/v1` wire format. Every type here derives `JsonSchema`; the TypeScript
//! file the frontend uses is generated from them (see [`crate::tsgen`]).

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use so_core::proto::{
    AccountView, MessageView, RouteResult, RunView, SignInState, StatusResult, UsageTotals,
};
use so_core::{Choice, Config, ErrorKind, Event, Harness, RunState};

/// One page of a list. `next_cursor` is opaque; pass it back as `cursor` (or `after`).
#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// `RunView` without the join command. A distinct struct, so a join field cannot leak by
/// adding it to `RunView`; the only route to a join command is `POST /runs/:id/join`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunViewPublic {
    pub run_id: String,
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
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    /// Messages to and from the run, oldest first; bodies are redacted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<MessageView>,
}

impl From<RunView> for RunViewPublic {
    fn from(v: RunView) -> Self {
        // Destructured without `..`, so a new `RunView` field fails to compile here until
        // it is deliberately mapped or dropped.
        let RunView {
            run_id,
            session_id,
            state,
            harness,
            account,
            model,
            recent_events,
            result,
            usage,
            worktree,
            branch,
            join_command: _,
            join_cwd: _,
            join_env: _,
            name,
            depth,
            parent_run,
            error_kind,
            cwd,
            created_at,
            finished_at,
            messages,
        } = v;
        Self {
            run_id,
            session_id,
            state,
            harness,
            account,
            model,
            recent_events,
            result,
            usage,
            worktree,
            branch,
            name,
            depth,
            parent_run,
            error_kind,
            cwd,
            created_at,
            finished_at,
            messages,
        }
    }
}

/// Result of `POST /runs/:id/join`: the command to run, with secrets masked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JoinView {
    pub argv: Vec<String>,
    pub cwd: String,
    /// Names (never values) of the environment variables the command needs.
    pub env_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Caps {
    pub global: u32,
}

/// `GET /status`: the supervisor status plus the configured caps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusView {
    pub pid: u32,
    pub version: String,
    pub uptime_s: u64,
    pub sessions: u32,
    pub active_runs: u32,
    pub caps: Caps,
}

impl StatusView {
    pub fn new(s: StatusResult, caps: Caps) -> Self {
        Self {
            pid: s.pid,
            version: s.version,
            uptime_s: s.uptime_s,
            sessions: s.sessions,
            active_runs: s.active_runs,
            caps,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Active,
    Escalated,
    Failed,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionSummary {
    pub id: String,
    pub kind: so_core::SessionKind,
    pub client: String,
    pub cwd: Option<String>,
    pub account: Option<String>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
    pub run_count: u32,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SessionDetail {
    #[serde(flatten)]
    pub summary: SessionSummary,
    pub runs: Vec<RunViewPublic>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DailyUsage {
    /// `YYYY-MM-DD`, UTC.
    pub day: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost: f64,
    /// Cost per account.
    pub by_account: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DashboardView {
    pub status: StatusView,
    pub accounts: Vec<AccountView>,
    pub live_runs: Vec<RunViewPublic>,
    pub escalations: Vec<Event>,
    pub daily: Vec<DailyUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptTool {
    pub name: String,
    pub ok: Option<bool>,
}

/// One normalised, redacted transcript line. Raw harness lines are never served.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptLine {
    /// Byte offset of the transcript entry this line came from.
    pub offset: u64,
    pub role: TranscriptRole,
    pub text: String,
    pub tool: Option<TranscriptTool>,
    pub ts: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TranscriptPage {
    pub lines: Vec<TranscriptLine>,
    /// Where to continue reading; also set at the end, so a poller can pass it back.
    pub next_offset: Option<u64>,
    pub total_bytes: u64,
}

/// One row of the Route log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteRecord {
    pub ts: DateTime<Utc>,
    pub run_id: Option<String>,
    pub prompt_excerpt: String,
    pub result: RouteResult,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorktreeView {
    pub path: String,
    pub branch: Option<String>,
    pub run_id: String,
    pub size_bytes: u64,
    /// The run is finished but its worktree is still on disk.
    pub orphaned: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PruneRequest {
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PruneResult {
    /// Ids of the runs whose worktrees were (or, with `dry_run`, would be) removed.
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ChoicesView {
    pub choices: Vec<Choice>,
    pub routing_instruction: String,
    /// The file differs from what the running supervisor loaded.
    pub restart_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SecretOrigin {
    Env,
    File,
    Unset,
}

/// Where a secret comes from; the value itself is never sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SecretSource {
    pub source: SecretOrigin,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ConfigView {
    /// `config.toml` with `ui.token` removed.
    pub config: Config,
    pub ui_token: SecretSource,
    pub openrouter_key: SecretSource,
    /// `linux` or `macos`.
    pub platform: String,
    pub restart_required: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddAccountRequest {
    pub harness: Harness,
    pub name: String,
    #[serde(default)]
    pub home: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportAccountRequest {
    pub name: String,
    pub harness: Harness,
    pub home: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportCandidate {
    pub name: String,
    pub harness: Harness,
    pub home: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PatchAccountRequest {
    #[serde(default)]
    pub cap: Option<u32>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub pools: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlagRequest {
    #[serde(default)]
    pub pool: Option<String>,
    /// Cooldown end, RFC 3339.
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
    #[serde(default)]
    pub hours: Option<f64>,
    #[serde(default)]
    pub clear: bool,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum SignInStart {
    /// A one-time URL and code (not produced yet; every harness uses `manual`).
    DeviceAuth {
        url: String,
        code: String,
        expires_at: DateTime<Utc>,
    },
    /// Run `command` in a terminal.
    Manual {
        command: String,
        cwd: Option<String>,
        note: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInMode {
    DeviceAuth,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignInProgress {
    Pending,
    Done,
    Expired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SignInStatus {
    pub signed_in: SignInState,
    pub mode: SignInMode,
    pub state: SignInProgress,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenRequest {
    pub token: String,
}

/// `{ "error": { code, message, data? } }`. `code` is the JSON-RPC code for errors that come
/// from the supervisor, and a short string (`unauthorized`, `forbidden`, `rate_limited`,
/// `not_implemented`, `not_found`) for errors the web layer raises itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApiErrorDetail {
    pub code: ApiErrorCode,
    pub message: String,
    pub data: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ApiErrorCode {
    Rpc(i64),
    Named(String),
}

/// Payload of the SSE `hello` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StreamHello {
    pub seq: u64,
    pub server_time: DateTime<Utc>,
    pub boot_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SendMessageBody {
    pub text: String,
    #[serde(default)]
    pub kind: Option<so_core::MessageKind>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RejectBody {
    pub feedback: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ForkBody {
    /// First prompt of the fork; defaults to a request to continue.
    #[serde(default)]
    pub prompt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelRow {
    pub harness: Harness,
    pub id: String,
    pub tier: String,
    pub cost: String,
    pub pool: Option<String>,
    pub efforts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ModelCatalog {
    /// When `models.toml` was last written.
    pub refreshed_at: Option<DateTime<Utc>>,
    pub models: Vec<ModelRow>,
    /// The refresh report (sources, added models, unused models); only after a refresh.
    pub report: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentDetail {
    pub name: String,
    pub description: String,
    pub path: String,
    /// The raw `AGENT.md`.
    pub source: String,
    /// `user`, `superorchestrator` or `claude`.
    pub origin: String,
    /// Imported from `.claude/agents`: cannot be edited or removed here.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAgentRequest {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAgentRequest {
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentWriteResult {
    pub ok: bool,
    /// What the agent's harness cannot honour.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WorkflowSummary {
    pub name: String,
    pub path: String,
    /// `project` or `config`.
    pub source: String,
}
