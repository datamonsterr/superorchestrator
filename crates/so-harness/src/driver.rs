//! Common driver API (SPEC 4, 12). The supervisor run manager drives every
//! harness through [`Driver`], [`RunHandle`] and [`RunControl`].
//!
//! Dispatch is by enum ([`AnyDriver`], [`RunControl`]) rather than `dyn`, so the
//! trait can use native async methods. A new driver adds one variant to each
//! enum. Transports differ per harness (claude: stdio NDJSON, codex: JSON-RPC
//! over ws, opencode: HTTP and SSE); the API only exposes events and controls.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use tokio::sync::mpsc;

use crate::agy::{AgyControl, AgyDriver};
use crate::claude::{ClaudeControl, ClaudeDriver};
use crate::codex::{CodexControl, CodexDriver};
use crate::opencode::{OpencodeControl, OpencodeDriver};

/// Errors from starting or controlling a harness.
#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to spawn {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("harness stdin is closed")]
    Closed,
    #[error("no driver for harness {0}")]
    Unsupported(Harness),
    /// A start-up failure that the harness itself reported (its process output, or the
    /// status code of its server), with the `kind` the driver classified it as. The
    /// supervisor trusts only this variant, so an unrelated error that merely mentions
    /// a word like "quota" never passes for a limit.
    #[error("{text}")]
    Exited { kind: ErrorKind, text: String },
    #[error("{0}")]
    Other(String),
}

/// Fails a launch whose effort the harness CLI would refuse, before anything is spawned
/// (ADR-0004). The router hint `thinking` passes for agy and is dropped from its argv.
pub fn check_effort(harness: Harness, effort: Option<&str>) -> Result<(), DriverError> {
    match effort.and_then(|e| harness.effort_problem(e)) {
        Some(problem) => Err(DriverError::Other(problem)),
        None => Ok(()),
    }
}

/// Validates model/effort selectors: nonempty, no leading '-', ASCII alphanumerics
/// or `:/_.@-[]` (including Claude's `sonnet[1m]` model suffix).
pub fn check_selector(model: Option<&str>, effort: Option<&str>) -> Result<(), DriverError> {
    for (name, value) in [("model", model), ("effort", effort)] {
        if let Some(value) = value
            && (value.is_empty()
                || value.starts_with('-')
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b":/_.@-[]".contains(&b)))
        {
            return Err(DriverError::Other(format!(
                "invalid {name}: expected a nonempty value without a leading '-' using only A-Za-z0-9:/_.@-[]"
            )));
        }
    }
    Ok(())
}

/// Validates resolved launch values before any process or launch files are created.
pub fn check_launch_values(
    harness: Harness,
    model: Option<&str>,
    effort: Option<&str>,
    resume: Option<&str>,
) -> Result<(), DriverError> {
    check_selector(model, effort)?;
    if let Some(id) = resume {
        let safe = !id.is_empty()
            && !id.starts_with('-')
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:-".contains(&b));
        let (valid, expected) = match harness {
            Harness::Agy | Harness::Claude | Harness::Codex => (
                safe && id.len() == 36 && uuid::Uuid::parse_str(id).is_ok(),
                "a hyphenated UUID",
            ),
            Harness::Opencode => (
                safe && id
                    .strip_prefix("ses_")
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric())),
                "ses_ followed by ASCII letters and digits",
            ),
        };
        if !valid {
            return Err(DriverError::Other(format!(
                "invalid resume id: expected {expected}"
            )));
        }
    }
    check_effort(harness, effort)?;
    Ok(())
}

/// How the harness reaches the superorchestrator MCP server. The `env` goes
/// into the harness's MCP config, never into argv or the harness process env.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct McpServerSpec {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

impl fmt::Debug for McpServerSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpServerSpec")
            .field("command", &self.command)
            .field("args", &format_args!("<{} args>", self.args.len()))
            .field(
                "env_keys",
                &self.env.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// What a custom agent adds to a launch (SPEC 10), already in the harness's native form.
/// The default adds nothing.
#[derive(Clone, Default, PartialEq)]
pub struct AgentLaunch {
    /// Extra `claude` argv: tool lists, `--mcp-config`, `--settings`, `--add-dir`.
    pub extra_args: Vec<String>,
    /// Codex: merged into the `thread/start` config. OpenCode: merged into the run's config.
    pub config_fragment: Option<serde_json::Value>,
    /// OpenCode: the agent the session runs as.
    pub agent_name: Option<String>,
}

impl AgentLaunch {
    /// Merges the agent's config fragment into a harness config object. The agent can add
    /// and change entries, but never replaces `protected_path`'s entry (the superorchestrator
    /// MCP server), which the run needs to reach the supervisor.
    pub fn merge_into(&self, base: &mut serde_json::Value, protected: (&str, &str)) {
        let Some(fragment) = &self.config_fragment else {
            return;
        };
        let keep = base
            .get(protected.0)
            .and_then(|m| m.get(protected.1))
            .cloned();
        merge_json(base, fragment);
        // The fragment may have replaced the whole map with something else.
        if let Some(keep) = keep
            && let Some(root) = base.as_object_mut()
        {
            let slot = root
                .entry(protected.0.to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !slot.is_object() {
                *slot = serde_json::json!({});
            }
            if let Some(map) = slot.as_object_mut() {
                map.insert(protected.1.to_string(), keep);
            }
        }
    }
}

/// Objects merge key by key; anything else in `extra` replaces what is in `base`.
fn merge_json(base: &mut serde_json::Value, extra: &serde_json::Value) {
    match (base, extra) {
        (serde_json::Value::Object(b), serde_json::Value::Object(e)) => {
            for (k, v) in e {
                match b.get_mut(k) {
                    Some(slot) => merge_json(slot, v),
                    None => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (slot, v) => *slot = v.clone(),
    }
}

/// Everything a driver needs to start (or resume) one run.
#[derive(Clone)]
pub struct LaunchSpec {
    pub run_id: String,
    pub run_dir: PathBuf,
    pub cwd: PathBuf,
    pub account: AccountFile,
    /// Launch recipe for the account environment (from `so-accounts`).
    pub launch: Launch,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// First user message. Sent over the protocol, never on argv.
    pub prompt: String,
    pub system_addendum: String,
    pub mcp: McpServerSpec,
    pub output_schema: Option<serde_json::Value>,
    pub max_turns: Option<u32>,
    /// Native session id to resume instead of starting a new session.
    pub resume: Option<String>,
    /// With `resume`: continue a copy of that session under a new native id instead of
    /// the session itself (only where [`AnyDriver::supports_fork`]).
    pub fork: bool,
    /// The custom agent's additions (SPEC 10); empty without one.
    pub agent: AgentLaunch,
}

// Redacting: no prompt text, no env values (account vars or MCP env).
// `so_accounts::Launch` and `AccountFile` derive Debug and would print env
// values, so they are summarised here.
impl fmt::Debug for LaunchSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchSpec")
            .field("run_id", &self.run_id)
            .field("run_dir", &self.run_dir)
            .field("cwd", &self.cwd)
            .field("account", &self.account.name)
            .field("program", &self.launch.program)
            .field(
                "env_keys",
                &self
                    .launch
                    .vars
                    .iter()
                    .map(|(k, _)| k.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("model", &self.model)
            .field("effort", &self.effort)
            .field("prompt_len", &self.prompt.len())
            .field("system_addendum_len", &self.system_addendum.len())
            .field("mcp", &self.mcp)
            .field("has_output_schema", &self.output_schema.is_some())
            .field("max_turns", &self.max_turns)
            .field("resume", &self.resume)
            .field("fork", &self.fork)
            .field("agent_args", &self.agent.extra_args.len())
            .finish()
    }
}

/// Server a harness exposes for joining a live session (codex ws, opencode HTTP).
#[derive(Clone, PartialEq, Eq)]
pub struct Endpoint {
    pub url: String,
    /// Secret passed to the join command only through its environment:
    /// `(variable name, value)`.
    pub secret_env: Option<(String, String)>,
    /// Directory the server expects (opencode `x-opencode-directory`).
    pub directory: Option<PathBuf>,
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Endpoint")
            .field("url", &self.url)
            .field("secret_env", &self.secret_env.as_ref().map(|(k, _)| k))
            .field("directory", &self.directory)
            .finish()
    }
}

/// What is needed to build a "join" command for a person (SPEC 7.4).
#[derive(Debug, Clone)]
pub struct JoinInfo {
    /// Account name; its wrapper is the command that sets the account env.
    pub account_name: String,
    /// Empty when the harness had not reported its session id at launch
    /// (agy, after a 10 s wait): then take it from `SessionStarted`.
    pub native_session_id: String,
    pub endpoint: Option<Endpoint>,
    pub cwd: PathBuf,
}

/// A command to run in a terminal. Secrets travel only in `env`, never argv.
#[derive(Clone, PartialEq, Eq)]
pub struct JoinCommand {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}

impl fmt::Debug for JoinCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinCommand")
            .field("argv", &self.argv)
            .field(
                "env_keys",
                &self.env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Five-hour and weekly usage as reported by the harness (percent used 0-100,
/// reset times in epoch seconds).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RateLimits {
    pub five_hour_used_pct: Option<f64>,
    pub five_hour_resets_at: Option<i64>,
    pub weekly_used_pct: Option<f64>,
    pub weekly_resets_at: Option<i64>,
}

/// Where a raw line came from (transcript `channel`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Stdout,
    Stderr,
    Stdin,
    Ws,
    Http,
    Sse,
}

/// Harness-neutral event (SPEC 12).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    SessionStarted {
        native_session_id: String,
    },
    TurnStarted,
    /// Full assistant text; the supervisor excerpts it.
    AssistantMessage {
        text: String,
    },
    ToolCallStarted {
        name: String,
    },
    ToolCallFinished {
        name: String,
        ok: bool,
    },
    /// Input tokens include cache reads and writes.
    Usage {
        input_tokens: u64,
        output_tokens: u64,
        cost: Option<f64>,
        rate_limits: Option<RateLimits>,
    },
    /// A failed turn carries `is_error`, `error_kind` and `error_text` here
    /// and has no separate `Error` event.
    TurnFinished {
        result_text: Option<String>,
        structured: Option<serde_json::Value>,
        is_error: bool,
        error_kind: Option<ErrorKind>,
        error_text: Option<String>,
    },
    /// A failure outside a turn (rate-limit rejection, read or transcript error).
    Error {
        kind: ErrorKind,
        message: String,
    },
    /// A raw line, already appended to the transcript at `offset` (`None` if
    /// the transcript write failed).
    Raw {
        channel: Channel,
        line: String,
        offset: Option<u64>,
    },
    /// The harness process exited (servers emit it when their process exits).
    Exited {
        code: Option<i32>,
    },
}

/// A started run: the event stream plus the means to control it.
#[derive(Debug)]
pub struct RunHandle {
    /// Unbounded on purpose: the harness and transcript never block on the
    /// consumer. The supervisor must drain it (it persists every event to the
    /// database); an undrained handle only costs memory. `Exited` is the last
    /// event; the channel closes after it.
    pub events: mpsc::UnboundedReceiver<HarnessEvent>,
    pub control: RunControl,
    pub native_session_id: Option<String>,
    pub join: JoinInfo,
}

/// How a shutdown ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownOutcome {
    /// The harness exited by itself within the deadline.
    Exited(Option<i32>),
    /// The harness had to be killed (SIGTERM, then SIGKILL, to the group).
    Killed,
}

/// Cheap clonable handle for live control of one run. Dropping the last clone
/// kills whatever is left of the harness process group.
#[derive(Debug, Clone)]
pub enum RunControl {
    Claude(ClaudeControl),
    Agy(AgyControl),
    Opencode(OpencodeControl),
    Codex(CodexControl),
}

impl RunControl {
    /// Sends a follow-up user message.
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.send(text).await,
            RunControl::Agy(c) => c.send(text).await,
            RunControl::Opencode(c) => c.send(text).await,
            RunControl::Codex(c) => c.send(text).await,
        }
    }

    /// Interrupts the current turn and waits (5 s) for the harness to
    /// acknowledge; the run stays alive for more input.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.interrupt().await,
            RunControl::Agy(c) => c.interrupt().await,
            RunControl::Opencode(c) => c.interrupt().await,
            RunControl::Codex(c) => c.interrupt().await,
        }
    }

    /// Ends the run: closes input, waits up to `deadline` for a clean exit,
    /// then kills the process group.
    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        match self {
            RunControl::Claude(c) => c.shutdown(deadline).await,
            RunControl::Agy(c) => c.shutdown(deadline).await,
            RunControl::Opencode(c) => c.shutdown(deadline).await,
            RunControl::Codex(c) => c.shutdown(deadline).await,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        match self {
            RunControl::Claude(c) => c.pid(),
            RunControl::Agy(c) => c.pid(),
            RunControl::Opencode(c) => c.pid(),
            RunControl::Codex(c) => c.pid(),
        }
    }
}

/// Settings shared by all drivers.
#[derive(Debug, Clone, Copy)]
pub struct DriverConfig {
    /// Time between SIGTERM and SIGKILL when a run must be killed.
    pub shutdown_grace: Duration,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            shutdown_grace: Duration::from_secs(2),
        }
    }
}

/// One harness. Use [`AnyDriver`] for runtime selection.
pub trait Driver {
    fn harness(&self) -> Harness;

    fn launch(
        &self,
        spec: LaunchSpec,
    ) -> impl Future<Output = Result<RunHandle, DriverError>> + Send;

    /// The command a person runs to join the session.
    fn join_command(&self, j: &JoinInfo) -> JoinCommand;

    fn classify_error(&self, text: &str) -> ErrorKind;
}

/// Runtime driver selection.
#[derive(Debug, Clone)]
pub enum AnyDriver {
    Claude(ClaudeDriver),
    Agy(AgyDriver),
    Opencode(OpencodeDriver),
    Codex(CodexDriver),
}

impl AnyDriver {
    /// Whether `LaunchSpec::fork` works: claude (`--resume <id> --fork-session`), codex
    /// (`thread/fork`) and opencode (`POST /session/{id}/fork`). Agy has no fork.
    pub fn supports_fork(&self) -> bool {
        !matches!(self, AnyDriver::Agy(_))
    }

    /// Driver for a harness (all four have one; the `Result` is kept for
    /// future harnesses, see [`DriverError::Unsupported`]).
    pub fn for_harness(h: Harness, config: DriverConfig) -> Result<Self, DriverError> {
        match h {
            Harness::Claude => Ok(AnyDriver::Claude(ClaudeDriver::new(config))),
            Harness::Agy => Ok(AnyDriver::Agy(AgyDriver::new(config))),
            Harness::Opencode => Ok(AnyDriver::Opencode(OpencodeDriver::new(config))),
            Harness::Codex => Ok(AnyDriver::Codex(CodexDriver::new(config))),
        }
    }
}

impl Driver for AnyDriver {
    fn harness(&self) -> Harness {
        match self {
            AnyDriver::Claude(d) => d.harness(),
            AnyDriver::Agy(d) => d.harness(),
            AnyDriver::Opencode(d) => d.harness(),
            AnyDriver::Codex(d) => d.harness(),
        }
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        match self {
            AnyDriver::Claude(d) => d.launch(spec).await,
            AnyDriver::Agy(d) => d.launch(spec).await,
            AnyDriver::Opencode(d) => d.launch(spec).await,
            AnyDriver::Codex(d) => d.launch(spec).await,
        }
    }

    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        match self {
            AnyDriver::Claude(d) => d.join_command(j),
            AnyDriver::Agy(d) => d.join_command(j),
            AnyDriver::Opencode(d) => d.join_command(j),
            AnyDriver::Codex(d) => d.join_command(j),
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        match self {
            AnyDriver::Claude(d) => d.classify_error(text),
            AnyDriver::Agy(d) => d.classify_error(text),
            AnyDriver::Opencode(d) => d.classify_error(text),
            AnyDriver::Codex(d) => d.classify_error(text),
        }
    }
}

/// True if `needle` occurs in `hay` not touching an alphanumeric or `_`.
fn has_word(hay: &str, needle: &str) -> bool {
    let word = |c: char| c.is_alphanumeric() || c == '_';
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        !before.is_some_and(word) && !after.is_some_and(word)
    })
}

/// Text-pattern classification shared by all drivers (ADR-0004 cheat sheet).
/// Quota is checked first because quota text often also carries a 429.
/// Numeric codes match whole words only; sign-in is matched as phrases.
pub fn classify_common(text: &str) -> ErrorKind {
    let t = text.to_lowercase();
    let has = |pats: &[&str]| pats.iter().any(|p| t.contains(p));
    let word = |pats: &[&str]| pats.iter().any(|p| has_word(&t, p));
    if has(&[
        "quota",
        "usage limit",
        "resource_exhausted",
        "insufficient funds",
        "insufficient account funds",
    ]) {
        ErrorKind::Quota
    } else if has(&["rate limit", "rate_limit", "too many requests"]) || word(&["429"]) {
        ErrorKind::RateLimit
    } else if has(&[
        "unauthorized",
        "not signed in",
        "eligibility",
        "invalid api key",
        // Sign-in prompts, as phrases only: a bare "login" is also a binary and a path.
        "not logged in",
        "run /login",
        "please login",
        "please log in",
        "log in again",
    ]) || word(&["401"])
    {
        ErrorKind::Auth
    } else {
        ErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn invalid_launch_values_fail_before_preparation_for_every_driver() {
        let dir = tempfile::tempdir().unwrap();
        for h in [
            Harness::Agy,
            Harness::Claude,
            Harness::Codex,
            Harness::Opencode,
        ] {
            for field in ["model", "effort", "resume"] {
                let mut spec = LaunchSpec {
                    run_id: "r".into(),
                    run_dir: dir.path().join(format!("{h}-{field}")),
                    cwd: dir.path().into(),
                    account: AccountFile::new("test-acc", h, "/unused", "test").unwrap(),
                    launch: Launch {
                        program: "/bin/true".into(),
                        vars: vec![],
                        argv_prefix: vec![],
                        extra_args: vec![],
                    },
                    model: None,
                    effort: None,
                    resume: None,
                    prompt: "x".into(),
                    system_addendum: String::new(),
                    mcp: McpServerSpec::default(),
                    output_schema: None,
                    max_turns: None,
                    fork: false,
                    agent: AgentLaunch::default(),
                };
                match field {
                    "model" => spec.model = Some("--help".into()),
                    "effort" => spec.effort = Some("--help".into()),
                    _ => spec.resume = Some("--help".into()),
                }
                let run_dir = spec.run_dir.clone();
                let error = AnyDriver::for_harness(h, DriverConfig::default())
                    .unwrap()
                    .launch(spec)
                    .await
                    .unwrap_err();
                assert!(error.to_string().contains(field), "{h}: {error}");
                assert!(
                    !run_dir.exists(),
                    "{h}: created launch files before rejecting {field}"
                );
            }
        }
    }

    #[test]
    fn launch_selectors_reject_each_unsafe_rule() {
        for bad in [
            "",
            "-model",
            "--help",
            "has space",
            "a\nb",
            "a\0b",
            "a=b",
            "a;b",
            "é",
        ] {
            assert!(check_selector(Some(bad), None).is_err(), "model {bad:?}");
            assert!(check_selector(None, Some(bad)).is_err(), "effort {bad:?}");
        }
        for good in [
            "provider/model:v1",
            "gpt-6.1-sol",
            "sonnet[1m]",
            "claude-sonnet-5-5[1m]",
            "org/model@latest",
            "a_b.c-2",
            "low",
        ] {
            check_selector(Some(good), Some(good)).unwrap();
        }
    }

    #[test]
    fn resume_ids_follow_native_formats() {
        let uuid = "01a162ee-5ba3-7400-be98-4eb69d75fead";
        for h in [Harness::Agy, Harness::Claude, Harness::Codex] {
            check_launch_values(h, None, None, Some(uuid)).unwrap();
            for bad in ["", "--help", "abc", "../x", "a/b", "a b", "a\nb", "ses_abc"] {
                assert!(
                    check_launch_values(h, None, None, Some(bad)).is_err(),
                    "{h} {bad:?}"
                );
            }
        }
        check_launch_values(
            Harness::Opencode,
            None,
            None,
            Some("ses_e9d0e831fffejyPMluk5gnwrFx"),
        )
        .unwrap();
        for bad in ["", "--help", "ses_", "ses_a/b", "ses_a?x", "ses_a b", uuid] {
            assert!(
                check_launch_values(Harness::Opencode, None, None, Some(bad)).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn common_classification() {
        use ErrorKind::*;
        let cases = [
            ("Claude AI usage limit reached", Quota),
            ("RESOURCE_EXHAUSTED (code 429): x", Quota),
            ("Upstream request failed: Insufficient account funds", Quota),
            ("error: Individual quota reached.", Quota),
            ("rate limit exceeded", RateLimit),
            ("HTTP 429", RateLimit),
            ("status=429,", RateLimit),
            ("Invalid API key. Please run /login", Auth),
            ("Eligibility check failed", Auth),
            ("401 Unauthorized", Auth),
            ("There's an issue with the selected model", Other),
            ("listening on port 14012 at 0x4291", Other),
            ("loaded 1429 files, see blogin.rs", Other),
            ("You are not logged in", Auth),
            ("Please login", Auth),
            ("session expired, please log in again", Auth),
            ("failed to spawn /usr/bin/login: not found", Other),
            ("cannot read /srv/login/config.toml", Other),
        ];
        for (text, want) in cases {
            assert_eq!(classify_common(text), want, "{text}");
        }
    }

    #[test]
    fn every_harness_has_a_driver() {
        for h in [
            Harness::Claude,
            Harness::Codex,
            Harness::Opencode,
            Harness::Agy,
        ] {
            let d = AnyDriver::for_harness(h, DriverConfig::default()).unwrap();
            assert_eq!(d.harness(), h);
        }
    }

    #[test]
    fn agent_config_merges_deeply_and_keeps_our_server() {
        use serde_json::json;
        let agent = AgentLaunch {
            config_fragment: Some(json!({
                "mcp": {"docs": {"command": "d"}, "superorchestrator": {"command": "evil"}},
                "agent": {"reviewer": {"prompt": "p"}},
                "permission": {"*": "ask"},
            })),
            ..AgentLaunch::default()
        };
        let mut base = json!({
            "mcp": {"superorchestrator": {"command": "ours"}},
            "permission": {"*": "allow"},
        });
        agent.merge_into(&mut base, ("mcp", "superorchestrator"));
        assert_eq!(base["mcp"]["superorchestrator"]["command"], "ours");
        assert_eq!(base["mcp"]["docs"]["command"], "d");
        assert_eq!(base["agent"]["reviewer"]["prompt"], "p");
        assert_eq!(base["permission"]["*"], "ask");
        // A fragment that replaces the map with a non-object cannot drop our server.
        let mut odd = json!({"mcp": {"superorchestrator": {"command": "ours"}}});
        AgentLaunch {
            config_fragment: Some(json!({"mcp": "nope"})),
            ..AgentLaunch::default()
        }
        .merge_into(&mut odd, ("mcp", "superorchestrator"));
        assert_eq!(odd["mcp"]["superorchestrator"]["command"], "ours");
        // Nothing to merge: untouched.
        let mut same = base.clone();
        AgentLaunch::default().merge_into(&mut same, ("mcp", "superorchestrator"));
        assert_eq!(same, base);
    }

    #[test]
    fn debug_is_redacted() {
        let spec = LaunchSpec {
            run_id: "r".into(),
            run_dir: "/run".into(),
            cwd: "/cwd".into(),
            account: AccountFile::new("claude-acc1", Harness::Claude, "~/.c", "claude").unwrap(),
            launch: Launch {
                program: "/bin/claude".into(),
                vars: vec![("TOKEN_VAR".into(), "launch-secret".into())],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: None,
            effort: None,
            prompt: "very private prompt".into(),
            system_addendum: "sys".into(),
            mcp: McpServerSpec {
                command: "/bin/mcp".into(),
                args: vec!["--token=abc".into()],
                env: vec![("MCP_KEY".into(), "mcp-secret".into())],
            },
            output_schema: None,
            max_turns: None,
            resume: None,
            fork: false,
            agent: AgentLaunch::default(),
        };
        let dbg = format!("{spec:?}");
        for leaked in ["launch-secret", "mcp-secret", "very private", "--token=abc"] {
            assert!(!dbg.contains(leaked), "{leaked} leaked in {dbg}");
        }
        assert!(dbg.contains("TOKEN_VAR") && dbg.contains("MCP_KEY") && dbg.contains("prompt_len"));
        let ep = Endpoint {
            url: "ws://x".into(),
            secret_env: Some(("SO_WS_TOKEN".into(), "tok".into())),
            directory: None,
        };
        assert!(!format!("{ep:?}").contains("\"tok\""));
    }
}
