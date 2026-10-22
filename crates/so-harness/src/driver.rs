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

use crate::claude::{ClaudeControl, ClaudeDriver};

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
    #[error("{0}")]
    Other(String),
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
}

impl RunControl {
    /// Sends a follow-up user message.
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.send(text).await,
        }
    }

    /// Interrupts the current turn and waits (5 s) for the harness to
    /// acknowledge; the run stays alive for more input.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.interrupt().await,
        }
    }

    /// Ends the run: closes input, waits up to `deadline` for a clean exit,
    /// then kills the process group.
    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        match self {
            RunControl::Claude(c) => c.shutdown(deadline).await,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        match self {
            RunControl::Claude(c) => c.pid(),
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
}

impl AnyDriver {
    /// Driver for a harness; harnesses without a driver yet are
    /// [`DriverError::Unsupported`].
    pub fn for_harness(h: Harness, config: DriverConfig) -> Result<Self, DriverError> {
        match h {
            Harness::Claude => Ok(AnyDriver::Claude(ClaudeDriver::new(config))),
            other => Err(DriverError::Unsupported(other)),
        }
    }
}

impl Driver for AnyDriver {
    fn harness(&self) -> Harness {
        match self {
            AnyDriver::Claude(d) => d.harness(),
        }
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        match self {
            AnyDriver::Claude(d) => d.launch(spec).await,
        }
    }

    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        match self {
            AnyDriver::Claude(d) => d.join_command(j),
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        match self {
            AnyDriver::Claude(d) => d.classify_error(text),
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
/// Numeric codes and "login" match whole words only.
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
    ]) || word(&["401", "login"])
    {
        ErrorKind::Auth
    } else {
        ErrorKind::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        ];
        for (text, want) in cases {
            assert_eq!(classify_common(text), want, "{text}");
        }
    }

    #[test]
    fn unsupported_harness() {
        assert!(matches!(
            AnyDriver::for_harness(Harness::Codex, DriverConfig::default()),
            Err(DriverError::Unsupported(Harness::Codex))
        ));
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
