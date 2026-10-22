//! Common driver API (SPEC 4, 12). The supervisor run manager drives every
//! harness through [`Driver`], [`RunHandle`] and [`RunControl`].
//!
//! Dispatch is by enum ([`AnyDriver`], [`RunControl`]) rather than `dyn`, so the
//! trait can use native `async fn`-style methods. A new driver adds one variant
//! to each enum. Transports differ per harness (claude: stdio NDJSON, codex:
//! JSON-RPC, opencode: HTTP and SSE); the API only exposes events and controls.

use std::future::Future;
use std::path::PathBuf;

use serde::Serialize;
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use tokio::sync::mpsc;

use crate::claude::{ClaudeControl, ClaudeDriver};

/// Errors from starting or controlling a harness process.
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServerSpec {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// Everything a driver needs to start (or resume) one run.
#[derive(Debug, Clone)]
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

/// What is needed to build a "join" command for a person (SPEC 7.4).
#[derive(Debug, Clone, Default)]
pub struct JoinInfo {
    /// Account name; its wrapper is the command that sets the account env.
    pub account_name: String,
    pub native_session_id: String,
    /// Server endpoint for harnesses that attach to a live server.
    pub endpoint: Option<String>,
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

/// Harness-neutral event (SPEC 12).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HarnessEvent {
    SessionStarted {
        native_session_id: String,
    },
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
    TurnFinished {
        result_text: Option<String>,
        structured: Option<serde_json::Value>,
        is_error: bool,
        error_text: Option<String>,
    },
    Error {
        kind: ErrorKind,
        message: String,
    },
    /// A raw stdout line, already appended to the transcript at `offset`.
    Raw {
        line: String,
        offset: u64,
    },
    /// A raw stderr line (transcript `dir: "err"`).
    Stderr {
        line: String,
    },
    Exited {
        code: Option<i32>,
    },
}

/// A started run: the event stream plus the means to control it.
#[derive(Debug)]
pub struct RunHandle {
    pub events: mpsc::Receiver<HarnessEvent>,
    pub control: RunControl,
    pub native_session_id: Option<String>,
}

/// Live control of one run.
#[derive(Debug)]
pub enum RunControl {
    Claude(ClaudeControl),
}

impl RunControl {
    /// Sends a follow-up user message.
    pub async fn send(&mut self, text: &str) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.send(text).await,
        }
    }

    /// Interrupts the current turn; the run stays alive for more input.
    pub async fn interrupt(&mut self) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.interrupt().await,
        }
    }

    /// Ends the run: closes stdin, waits a grace period, then kills.
    pub async fn shutdown(&mut self) -> Result<(), DriverError> {
        match self {
            RunControl::Claude(c) => c.shutdown().await,
        }
    }

    pub fn pid(&self) -> Option<u32> {
        match self {
            RunControl::Claude(c) => c.pid(),
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

    /// Argv of the command a person runs to join the session, run from the
    /// run's working directory. Element 0 is the account wrapper.
    fn join_command(&self, j: &JoinInfo) -> Vec<String>;

    fn classify_error(&self, text: &str) -> ErrorKind;
}

/// Runtime driver selection.
#[derive(Debug, Clone)]
pub enum AnyDriver {
    Claude(ClaudeDriver),
}

impl AnyDriver {
    /// Default driver for a harness; harnesses without a driver yet are
    /// [`DriverError::Unsupported`].
    pub fn for_harness(h: Harness) -> Result<Self, DriverError> {
        match h {
            Harness::Claude => Ok(AnyDriver::Claude(ClaudeDriver::default())),
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

    fn join_command(&self, j: &JoinInfo) -> Vec<String> {
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

/// Text-pattern classification shared by all drivers (ADR-0004 cheat sheet).
/// Quota is checked first because quota text often also carries a 429.
pub fn classify_common(text: &str) -> ErrorKind {
    let t = text.to_lowercase();
    let has = |pats: &[&str]| pats.iter().any(|p| t.contains(p));
    if has(&[
        "quota",
        "usage limit",
        "resource_exhausted",
        "insufficient funds",
        "insufficient account funds",
    ]) {
        ErrorKind::Quota
    } else if has(&["rate limit", "rate_limit", "429", "too many requests"]) {
        ErrorKind::RateLimit
    } else if has(&[
        "401",
        "unauthorized",
        "not signed in",
        "eligibility",
        "login",
        "invalid api key",
    ]) {
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
            ("Invalid API key. Please run /login", Auth),
            ("Eligibility check failed", Auth),
            ("401 Unauthorized", Auth),
            ("There's an issue with the selected model", Other),
        ];
        for (text, want) in cases {
            assert_eq!(classify_common(text), want, "{text}");
        }
    }

    #[test]
    fn unsupported_harness() {
        assert!(matches!(
            AnyDriver::for_harness(Harness::Codex),
            Err(DriverError::Unsupported(Harness::Codex))
        ));
    }
}
