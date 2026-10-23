//! Antigravity (`agy`) driver (ADR-0004 "agy", ADR-0005): `agy -p=` with
//! stream-json on stdin and stdout. One process serves many turns; closing
//! stdin ends it.
//!
//! The account is selected with `--gemini_dir=<run_dir>/gemini`, a per-run
//! directory that links the account's state and carries a run-specific
//! `config/mcp_config.json` (agy has no `--mcp-config`). Supervised runs are
//! wrapped in `bwrap` only for `--die-with-parent --new-session`; without
//! bwrap agy is launched directly and the process group kill still applies.
//!
//! agy has no system-prompt flag (`agy --help`), so the system addendum is
//! prepended to the first user message of the process, inside a delimited
//! block. `max_turns` has no agy equivalent and is ignored.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value, json};
use so_core::{ErrorKind, Harness};
use tokio::process::Command;
use tokio::sync::watch;

use crate::driver::{
    Driver, DriverConfig, DriverError, HarnessEvent, JoinCommand, JoinInfo, LaunchSpec, RunControl,
    RunHandle, ShutdownOutcome, check_launch_values, classify_common,
};
use crate::process::{
    LineParser, ProcessHandle, create_private_dir, exited, spawn_supervised, write_private,
};

/// How long `interrupt` waits for agy to end the interrupted turn and exit.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long `launch` waits for agy's `init` event (the conversation id).
const INIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Entries of the account's gemini dir that a run shares by symlink.
const LINKED: [&str; 4] = ["antigravity-cli", "plugins", "projects", "skills"];
const GEMINI_DIR_FLAG: &str = "--gemini_dir=";

/// Driver for `agy`.
#[derive(Debug, Clone)]
pub struct AgyDriver {
    config: DriverConfig,
    /// `bwrap` binary to wrap supervised runs in, when available.
    bwrap: Option<PathBuf>,
}

impl AgyDriver {
    /// Finds `bwrap` on `PATH`; without it agy is launched directly.
    pub fn new(config: DriverConfig) -> Self {
        let path = std::env::var("PATH").ok();
        let bwrap = so_accounts::resolve_binary("bwrap", path.as_deref()).ok();
        Self { config, bwrap }
    }

    /// Explicit bwrap choice (`None` = launch agy directly).
    pub fn with_bwrap(config: DriverConfig, bwrap: Option<PathBuf>) -> Self {
        Self { config, bwrap }
    }
}

/// agy arguments (after `program argv_prefix extra_args`). The prompt is never
/// part of argv: `-p=` is an empty prompt and messages arrive on stdin.
///
/// `--dangerously-skip-permissions` is deliberate: SPEC 4 ("Full permissions")
/// requires workers to run unattended with every permission granted.
pub fn build_args(spec: &LaunchSpec) -> Vec<String> {
    let mut a = vec![
        format!("{GEMINI_DIR_FLAG}{}", gemini_dir(&spec.run_dir).display()),
        "-p=".into(),
        "--input-format".into(),
        "stream-json".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--dangerously-skip-permissions".into(),
    ];
    if let Some(m) = &spec.model {
        a.extend(["--model".into(), m.clone()]);
    }
    // `thinking` is the router hint for a model that reasons by itself: no flag.
    if let Some(e) = Harness::Agy.launch_effort(spec.effort.as_deref()) {
        a.extend(["--effort".into(), e.to_string()]);
    }
    if let Some(s) = &spec.output_schema {
        a.extend(["--json-schema".into(), s.to_string()]);
    }
    if let Some(id) = &spec.resume {
        a.extend(["--conversation".into(), id.clone()]);
    }
    a
}

/// The bwrap prefix: only `--die-with-parent` and the new session; the whole
/// filesystem stays visible (ADR-0005).
fn bwrap_prefix() -> Vec<String> {
    [
        "--die-with-parent",
        "--new-session",
        "--bind",
        "/",
        "/",
        "--dev-bind",
        "/dev",
        "/dev",
        "--",
    ]
    .map(String::from)
    .into()
}

fn gemini_dir(run_dir: &Path) -> PathBuf {
    run_dir.join("gemini")
}

/// The account's real gemini dir, from the launch recipe's `--gemini_dir=`.
fn account_gemini_dir(spec: &LaunchSpec) -> Result<PathBuf, DriverError> {
    spec.launch
        .extra_args
        .iter()
        .find_map(|a| a.strip_prefix(GEMINI_DIR_FLAG))
        .map(PathBuf::from)
        .ok_or_else(|| DriverError::Other("agy launch has no --gemini_dir".into()))
}

fn trusted_config_root(metadata: &std::fs::Metadata, uid: u32) -> bool {
    metadata.is_dir() && metadata.uid() == uid && metadata.mode() & 0o022 == 0
}

/// Builds `<run_dir>/gemini` (ADR-0005): symlinks to the account's shared
/// state, a real `config/` copied from the account (files only), and a
/// `config/mcp_config.json` holding the account's servers plus ours.
/// Directories are 0700, files 0600. An existing dir is rebuilt.
///
/// Credentials and conversations live under the account's `antigravity-cli/`
/// (ADR-0004), which is linked, so sign-in and `--conversation` resume work and
/// refreshed tokens are written back to the account. `config/` is copied so
/// the run's `mcp_config.json` never edits the account's. Removing old run
/// dirs is the supervisor's job (run retention); this function only rebuilds.
pub fn prepare_gemini_dir(spec: &LaunchSpec, account: &Path) -> std::io::Result<PathBuf> {
    let dir = gemini_dir(&spec.run_dir);
    create_private_dir(&spec.run_dir)?;
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    create_private_dir(&dir)?;
    for name in LINKED {
        let src = account.join(name);
        if src.exists() {
            std::os::unix::fs::symlink(&src, dir.join(name))?;
        }
    }

    let config = dir.join("config");
    create_private_dir(&config)?;
    let account = account.canonicalize()?;
    let source_config = account.join("config");
    let denied = || {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "agy shared config root must be a directory owned by the current uid without group/world write access; entries must stay inside the config root or account directory",
        )
    };
    match std::fs::symlink_metadata(&source_config) {
        Ok(root_metadata) => {
            // Preserve local account-directory permissions. Operator-managed shared
            // roots (ADR-0005) need the additional ownership and mode checks.
            let source_config = source_config.canonicalize()?;
            let metadata = std::fs::metadata(&source_config)?;
            let shared = root_metadata.is_symlink() || !source_config.starts_with(&account);
            if !metadata.is_dir()
                || (shared && !trusted_config_root(&metadata, rustix::process::getuid().as_raw()))
            {
                return Err(denied());
            }
            for entry in std::fs::read_dir(&source_config)? {
                let entry = entry?;
                let path = entry.path();
                let metadata = std::fs::symlink_metadata(&path)?;
                if metadata.is_file() || metadata.is_symlink() {
                    let source = match path.canonicalize() {
                        Ok(source) => source,
                        Err(e)
                            if metadata.is_symlink()
                                && e.kind() == std::io::ErrorKind::NotFound =>
                        {
                            tracing::warn!(path = %path.display(), "skipping dangling agy config symlink");
                            continue;
                        }
                        Err(e) => return Err(e),
                    };
                    if !source.starts_with(&account) && !source.starts_with(&source_config) {
                        return Err(denied());
                    }
                    if std::fs::metadata(&source)?.is_file() {
                        write_private(&config.join(entry.file_name()), &std::fs::read(source)?)?;
                    }
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    // An unreadable or invalid account file is replaced rather than blocking
    // the run.
    let mut root: Map<String, Value> = match std::fs::read(config.join("mcp_config.json")) {
        Ok(b) => match serde_json::from_slice::<Value>(&b) {
            Ok(Value::Object(m)) => m,
            _ => {
                tracing::warn!("account mcp_config.json is invalid; starting from an empty one");
                Map::new()
            }
        },
        Err(_) => Map::new(),
    };
    if !root.get("mcpServers").is_some_and(Value::is_object) {
        root.insert("mcpServers".into(), Value::Object(Map::new()));
    }
    let env: Map<String, Value> = spec
        .mcp
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    if let Some(Value::Object(servers)) = root.get_mut("mcpServers") {
        servers.insert(
            "superorchestrator".into(),
            json!({
                "command": spec.mcp.command,
                "args": spec.mcp.args,
                "env": env,
                "disabled": false,
            }),
        );
    }
    write_private(
        &config.join("mcp_config.json"),
        &serde_json::to_vec_pretty(&Value::Object(root))?,
    )?;
    Ok(dir)
}

/// First user message: the system addendum, delimited, then the prompt.
fn first_message(spec: &LaunchSpec) -> String {
    if spec.system_addendum.trim().is_empty() {
        return spec.prompt.clone();
    }
    format!(
        "<superorchestrator-system-instructions>\n{}\n</superorchestrator-system-instructions>\n\n{}",
        spec.system_addendum.trim_end(),
        spec.prompt
    )
}

fn user_message(text: &str) -> Value {
    json!({"event": "user", "message": {"content": text}})
}

impl Driver for AgyDriver {
    fn harness(&self) -> Harness {
        Harness::Agy
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        check_launch_values(
            Harness::Agy,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            spec.resume.as_deref(),
        )?;
        if spec.fork {
            return Err(DriverError::Other("agy cannot fork a session".into()));
        }
        let account = account_gemini_dir(&spec)?;
        let prep = spec.clone();
        tokio::task::spawn_blocking(move || prepare_gemini_dir(&prep, &account))
            .await
            .map_err(|e| DriverError::Other(format!("gemini dir task failed: {e}")))??;

        let mut cmd = match &self.bwrap {
            Some(bwrap) => {
                let mut c = Command::new(bwrap);
                c.args(bwrap_prefix()).arg(&spec.launch.program);
                c
            }
            None => Command::new(&spec.launch.program),
        };
        cmd.args(&spec.launch.argv_prefix)
            .args(
                spec.launch
                    .extra_args
                    .iter()
                    .filter(|a| !a.starts_with(GEMINI_DIR_FLAG)),
            )
            .args(build_args(&spec))
            .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd);

        let (id_tx, mut id_rx) = watch::channel(spec.resume.clone());
        let sup = spawn_supervised(
            cmd,
            &spec.run_dir,
            self.config.shutdown_grace,
            AgyParser::new(spec.output_schema.is_some(), spec.resume.is_some(), id_tx),
        )?;
        let control = AgyControl {
            proc: Arc::new(sup.handle),
            bwrapped: self.bwrap.is_some(),
        };
        if let Err(e) = control.send(&first_message(&spec)).await {
            control.proc.shutdown(Duration::ZERO).await;
            return Err(e);
        }
        // The conversation id is only known from `init`. Wait briefly so the
        // handle and join info carry it. If agy exits first the launch fails.
        // If 10 s pass without `init` the launch still succeeds with no id:
        // callers must then take the id from `SessionStarted`, and
        // `JoinInfo.native_session_id` is empty until they do.
        let mut exit = control.proc.exit.clone();
        let peek = id_rx.clone();
        let mut events = sup.events;
        let (id, exited_early) = tokio::select! {
            // Err: the parser is gone, i.e. stdout closed because agy exited.
            r = id_rx.wait_for(Option::is_some) => match r {
                Ok(v) => (v.clone(), false),
                Err(_) => (None, true),
            },
            _ = exit.wait_for(|v| v.is_some()) => (None, true),
            () = tokio::time::sleep(INIT_TIMEOUT) => (None, false),
        };
        let id = id.or_else(|| peek.borrow().clone());
        if exited_early && id.is_none() {
            return Err(early_exit_error(&mut events, &control).await);
        }
        Ok(RunHandle {
            events,
            control: RunControl::Agy(control),
            native_session_id: id.clone(),
            join: JoinInfo {
                account_name: spec.account.name.clone(),
                native_session_id: id.unwrap_or_default(),
                endpoint: None,
                cwd: spec.cwd.clone(),
            },
        })
    }

    /// `<account wrapper> --conversation <id>`, run from the run's cwd with
    /// the session paused or ended (the live process owns stdin).
    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        JoinCommand {
            argv: vec![
                j.account_name.clone(),
                "--conversation".into(),
                j.native_session_id.clone(),
            ],
            env: vec![],
            cwd: j.cwd.clone(),
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        classify_agy(text)
    }
}

/// Error for an agy that exited before `init`: its failed `result` or stderr,
/// classified. Drains the (already finished) event stream.
async fn early_exit_error(
    events: &mut tokio::sync::mpsc::UnboundedReceiver<HarnessEvent>,
    control: &AgyControl,
) -> DriverError {
    let mut error_text = None;
    let mut stderr = Vec::new();
    let drain = async {
        while let Some(e) = events.recv().await {
            match e {
                HarnessEvent::TurnFinished {
                    error_text: Some(t),
                    ..
                } => error_text = Some(t),
                HarnessEvent::Raw {
                    channel: crate::driver::Channel::Stderr,
                    line,
                    ..
                } => stderr.push(line),
                HarnessEvent::Exited { .. } => break,
                _ => {}
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(3), drain).await;
    let code = control.proc.exit.borrow().flatten();
    let text = error_text
        .or_else(|| (!stderr.is_empty()).then(|| stderr.join("; ")))
        .unwrap_or_else(|| "no output".into());
    let text: String = text.chars().take(500).collect();
    let kind = classify_agy(&text);
    DriverError::Exited {
        kind,
        text: format!("agy exited before init (code {code:?}, {kind:?}): {text}"),
    }
}

/// `classify_common` plus the agy texts from ADR-0004.
fn classify_agy(text: &str) -> ErrorKind {
    let t = text.to_lowercase();
    if t.contains("individual quota reached") || t.contains("resource_exhausted") {
        ErrorKind::Quota
    } else if t.contains("eligibility check failed") {
        ErrorKind::Auth
    } else {
        classify_common(text)
    }
}

/// Clonable control handle for a running agy process. Dropping the last clone
/// kills the process group.
#[derive(Debug, Clone)]
pub struct AgyControl {
    proc: Arc<ProcessHandle>,
    /// The leader is bwrap, which forks agy into its own session.
    bwrapped: bool,
}

impl AgyControl {
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        self.proc
            .stdin
            .write_line(user_message(text).to_string())
            .await
    }

    /// Sends SIGINT to agy, which ends the turn with an error `result` and
    /// exits 1 (ADR-0004); the conversation stays resumable with
    /// `--conversation`. Waits (5 s) for that exit. Under bwrap the signal goes
    /// to bwrap's child: bwrap forks agy into a new session, so signalling the
    /// bwrap pid or its group would kill agy ungracefully.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        if self.proc.exit.borrow().is_some() {
            return Err(DriverError::Closed);
        }
        let leader = self.proc.pid.ok_or(DriverError::Closed)?;
        #[cfg(target_os = "linux")]
        let identity = if self.bwrapped {
            Some(
                first_child_identity(Path::new("/proc"), leader).ok_or_else(|| {
                    DriverError::Other("agy process under bwrap not found".into())
                })?,
            )
        } else {
            None
        };
        #[cfg(target_os = "linux")]
        let target = identity.map_or(leader, |child| child.pid);
        #[cfg(not(target_os = "linux"))]
        let target = if self.bwrapped {
            first_child(leader)
                .ok_or_else(|| DriverError::Other("agy process under bwrap not found".into()))?
        } else {
            leader
        };
        let pid = i32::try_from(target)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .ok_or_else(|| DriverError::Other("invalid agy pid".into()))?;
        // The scan and signal are separate syscalls. Re-read both parent and start time
        // at the last possible moment so an observed PID reuse fails closed.
        #[cfg(target_os = "linux")]
        if identity.is_some_and(|child| !child.is_current(Path::new("/proc"))) {
            return Err(DriverError::Closed);
        }
        #[cfg(not(target_os = "linux"))]
        if self.bwrapped && first_child(leader) != Some(target) {
            return Err(DriverError::Closed);
        }
        rustix::process::kill_process(pid, rustix::process::Signal::INT).map_err(|e| {
            if e == rustix::io::Errno::SRCH {
                DriverError::Closed
            } else {
                DriverError::Other(format!("SIGINT failed: {e}"))
            }
        })?;
        match exited(&self.proc.exit, INTERRUPT_TIMEOUT).await {
            Some(_) => Ok(()),
            None => Err(DriverError::Other(
                "agy did not stop within 5 s of the interrupt".into(),
            )),
        }
    }

    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        Ok(self.proc.shutdown(deadline).await)
    }

    pub fn pid(&self) -> Option<u32> {
        self.proc.pid
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChildIdentity {
    pid: u32,
    parent: u32,
    start_time: u64,
}

#[cfg(target_os = "linux")]
impl ChildIdentity {
    fn read(root: &Path, pid: u32) -> Option<Self> {
        let stat = std::fs::read_to_string(root.join(pid.to_string()).join("stat")).ok()?;
        let (head, rest) = stat.rsplit_once(") ")?;
        if head.split_once(' ')?.0.parse::<u32>().ok()? != pid {
            return None;
        }
        let fields: Vec<_> = rest.split_whitespace().collect();
        Some(Self {
            pid,
            parent: fields.get(1)?.parse().ok()?,
            start_time: fields.get(19)?.parse().ok()?,
        })
    }

    fn is_current(self, root: &Path) -> bool {
        Self::read(root, self.pid) == Some(self)
    }
}

/// Lowest-pid direct child, retaining the kernel start time for a later identity check.
#[cfg(target_os = "linux")]
fn first_child_identity(root: &Path, parent: u32) -> Option<ChildIdentity> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| ChildIdentity::read(root, pid))
        .filter(|child| child.parent == parent)
        .min_by_key(|child| child.pid)
}

#[cfg(all(test, target_os = "linux"))]
fn first_child(parent: u32) -> Option<u32> {
    first_child_identity(Path::new("/proc"), parent).map(|child| child.pid)
}

/// Lowest-pid direct child of `parent` on platforms without `/proc`.
#[cfg(not(target_os = "linux"))]
fn first_child(parent: u32) -> Option<u32> {
    pgrep_child(parent)
}

/// Lowest-pid direct child of `parent` via `pgrep -P`. Argv only, no shell.
/// `pgrep` exits 1 when nothing matches, which leaves no output and yields `None`.
#[cfg(any(test, not(target_os = "linux")))]
fn pgrep_child(parent: u32) -> Option<u32> {
    let out = std::process::Command::new("pgrep")
        .arg("-P")
        .arg(parent.to_string())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .min()
}

/// Stateful stdout parser: joins streamed text per step, turns cumulative
/// usage into per-turn deltas, and tracks turn boundaries.
#[derive(Debug)]
pub struct AgyParser {
    expect_structured: bool,
    session: Option<String>,
    id_tx: watch::Sender<Option<String>>,
    in_turn: bool,
    /// Text of agent_response steps still streaming, by step index.
    text: HashMap<u64, String>,
    tools: HashMap<u64, String>,
    started_tools: HashSet<u64>,
    /// Highest cumulative `(input, output)` seen so far.
    reported: (u64, u64),
    /// A resumed conversation's first result only sets the baseline: its
    /// cumulative usage includes earlier processes, which were already counted.
    baseline_pending: bool,
}

impl AgyParser {
    pub fn new(
        expect_structured: bool,
        resumed: bool,
        id_tx: watch::Sender<Option<String>>,
    ) -> Self {
        Self {
            expect_structured,
            session: None,
            id_tx,
            in_turn: false,
            text: HashMap::new(),
            tools: HashMap::new(),
            started_tools: HashSet::new(),
            reported: (0, 0),
            baseline_pending: resumed,
        }
    }

    /// Parser without a conversation-id consumer (tests).
    pub fn standalone(expect_structured: bool) -> Self {
        Self::new(expect_structured, false, watch::channel(None).0)
    }

    /// Normalised events for one stdout line (without the `Raw` event).
    /// Non-JSON lines produce nothing.
    pub fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            return vec![];
        };
        match v["event"].as_str() {
            Some("init") => self.init(&v),
            Some("step_update") => self.step(&v["step_update"]),
            Some("result") => self.result(&v["result"]),
            _ => vec![],
        }
    }

    fn begin_turn(&mut self, out: &mut Vec<HarnessEvent>) {
        if !self.in_turn {
            self.in_turn = true;
            out.push(HarnessEvent::TurnStarted);
        }
    }

    /// agy sends `init` once per process; the turn start comes from here or
    /// from the first `user_input` step, whichever is first.
    fn init(&mut self, v: &Value) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        if self.session.is_none()
            && let Some(id) = v["conversation_id"].as_str().filter(|s| !s.is_empty())
        {
            self.session = Some(id.to_string());
            self.id_tx.send_replace(Some(id.to_string()));
            out.push(HarnessEvent::SessionStarted {
                native_session_id: id.to_string(),
            });
        }
        self.begin_turn(&mut out);
        out
    }

    fn step(&mut self, s: &Value) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        let index = s["step_index"].as_u64().unwrap_or(0);
        let done = s["state"] == "DONE";
        match s["step_type"].as_str() {
            Some("user_input") => self.begin_turn(&mut out),
            Some("agent_response") => {
                if let Some(d) = s["text_delta"].as_str() {
                    self.text.entry(index).or_default().push_str(d);
                }
                if done && let Some(t) = self.text.remove(&index) {
                    let t = t.trim_end();
                    if !t.is_empty() {
                        out.push(HarnessEvent::AssistantMessage { text: t.into() });
                    }
                }
            }
            Some("tool") => {
                let name = s["tool_name"]
                    .as_str()
                    .or_else(|| s["tool_info"]["name"].as_str());
                if let Some(n) = name {
                    self.tools.insert(index, n.to_string());
                }
                let name = self
                    .tools
                    .get(&index)
                    .cloned()
                    .unwrap_or_else(|| "unknown".into());
                if self.started_tools.insert(index) {
                    out.push(HarnessEvent::ToolCallStarted { name: name.clone() });
                }
                if done {
                    self.started_tools.remove(&index);
                    self.tools.remove(&index);
                    out.push(HarnessEvent::ToolCallFinished { name, ok: true });
                }
            }
            _ => {}
        }
        out
    }

    fn result(&mut self, r: &Value) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        // A turn that failed before any step (bad model) still has a result.
        self.begin_turn(&mut out);
        self.in_turn = false;
        self.text.clear();
        self.tools.clear();
        self.started_tools.clear();

        // `result.usage` is cumulative over the conversation: report the delta
        // above the highest value seen per field. Empty usage (failed turns)
        // changes nothing; a resumed process's first result is the baseline.
        let u = &r["usage"];
        let n = |k: &str| u[k].as_u64().unwrap_or(0);
        let input = n("input_tokens") + n("cache_read_tokens");
        let output = n("output_tokens") + n("thinking_tokens");
        let delta = (
            input.saturating_sub(self.reported.0),
            output.saturating_sub(self.reported.1),
        );
        self.reported = (self.reported.0.max(input), self.reported.1.max(output));
        let baseline = self.baseline_pending && (input, output) != (0, 0);
        if baseline {
            self.baseline_pending = false;
        }
        if !baseline && delta != (0, 0) {
            out.push(HarnessEvent::Usage {
                input_tokens: delta.0,
                output_tokens: delta.1,
                cost: None,
                rate_limits: None,
            });
        }

        let is_error = r["status"] != "SUCCESS";
        let result_text = r["response"].as_str().map(String::from);
        let error_text = is_error.then(|| {
            r["error"]
                .as_str()
                .filter(|e| !e.is_empty())
                .map(String::from)
                .or_else(|| result_text.clone().filter(|t| !t.is_empty()))
                .unwrap_or_else(|| "agy reported an error".into())
        });
        let error_kind = error_text.as_deref().map(classify_agy);
        let structured = (self.expect_structured && !is_error)
            .then(|| {
                result_text
                    .as_deref()
                    .and_then(|t| serde_json::from_str::<Value>(t.trim()).ok())
            })
            .flatten();
        out.push(HarnessEvent::TurnFinished {
            result_text,
            structured,
            is_error,
            error_kind,
            error_text,
        });
        out
    }
}

impl LineParser for AgyParser {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        AgyParser::parse_line(self, line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/agy/transcript.jsonl");

    #[test]
    fn parses_real_fixture() {
        let mut p = AgyParser::standalone(false);
        let mut events = Vec::new();
        for l in FIXTURE.lines() {
            let v: Value = serde_json::from_str(l).unwrap();
            if v["dir"] == "in" {
                events.extend(p.parse_line(&v["msg"].to_string()));
            }
        }
        use HarnessEvent::*;
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                SessionStarted { .. } => "start",
                TurnStarted => "turn_start",
                AssistantMessage { .. } => "msg",
                ToolCallStarted { .. } => "tool+",
                ToolCallFinished { .. } => "tool-",
                Usage { .. } => "usage",
                TurnFinished { .. } => "turn_end",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            [
                "start",
                "turn_start",
                "tool+",
                "tool-",
                "msg",
                "usage",
                "turn_end",
                "turn_start",
                "msg",
                "usage",
                "turn_end"
            ]
        );
        assert_eq!(
            events[0],
            SessionStarted {
                native_session_id: "c0f1599d-37b9-4c82-be74-b28e7c59be25".into()
            }
        );
        assert_eq!(
            events[2],
            ToolCallStarted {
                name: "write_file".into()
            }
        );
        assert_eq!(
            events[4],
            AssistantMessage {
                text: "done".into()
            }
        );
        // Cumulative result usage becomes per-turn deltas.
        assert!(matches!(
            events[5],
            Usage {
                input_tokens: 18488,
                output_tokens: 85,
                ..
            }
        ));
        assert!(matches!(
            events[9],
            Usage {
                input_tokens: 9570,
                output_tokens: 15,
                ..
            }
        ));
        assert!(
            matches!(&events[6], TurnFinished { is_error: false, error_kind: None, result_text: Some(t), .. } if t == "done\n")
        );
    }

    #[test]
    fn error_result_is_classified_and_structured_is_parsed() {
        let mut p = AgyParser::standalone(true);
        let ev = p.parse_line(
            r#"{"event":"result","result":{"conversation_id":"c","status":"ERROR","error":"Individual quota reached. Resets in 40h.","usage":{}}}"#,
        );
        // The turn start is synthesised when the failure came before any step.
        assert!(matches!(ev[0], HarnessEvent::TurnStarted));
        assert!(matches!(
            ev.last(),
            Some(HarnessEvent::TurnFinished {
                is_error: true,
                error_kind: Some(ErrorKind::Quota),
                structured: None,
                ..
            })
        ));
        let ev = p.parse_line(
            r#"{"event":"result","result":{"status":"SUCCESS","response":"{\"a\":1}\n"}}"#,
        );
        assert!(matches!(
            ev.last(),
            Some(HarnessEvent::TurnFinished { structured: Some(s), .. }) if s["a"] == 1
        ));
        assert!(p.parse_line("agy: not json").is_empty());
    }

    #[test]
    fn empty_usage_is_skipped_and_resume_sets_a_baseline() {
        let ok = |i: u64, o: u64| {
            format!(
                r#"{{"event":"result","result":{{"status":"SUCCESS","response":"x","usage":{{"input_tokens":{i},"output_tokens":{o}}}}}}}"#
            )
        };
        let usages = |ev: &[HarnessEvent]| -> Vec<(u64, u64)> {
            ev.iter()
                .filter_map(|e| match e {
                    HarnessEvent::Usage {
                        input_tokens,
                        output_tokens,
                        ..
                    } => Some((*input_tokens, *output_tokens)),
                    _ => None,
                })
                .collect()
        };
        // An error result with empty usage reports nothing and loses nothing.
        let mut p = AgyParser::standalone(false);
        let ev = p
            .parse_line(r#"{"event":"result","result":{"status":"ERROR","error":"x","usage":{}}}"#);
        assert!(usages(&ev).is_empty());
        assert_eq!(usages(&p.parse_line(&ok(100, 10))), [(100, 10)]);
        // A lower cumulative value never goes negative or rewinds the maximum.
        assert!(usages(&p.parse_line(&ok(90, 10))).is_empty());
        assert_eq!(usages(&p.parse_line(&ok(150, 12))), [(50, 2)]);

        // Resumed: the first result is the baseline, later ones are deltas.
        let mut p = AgyParser::new(false, true, watch::channel(None).0);
        assert!(usages(&p.parse_line(&ok(5000, 500))).is_empty());
        assert_eq!(usages(&p.parse_line(&ok(5200, 530))), [(200, 30)]);
    }

    #[test]
    fn classification() {
        let d = AgyDriver::with_bwrap(DriverConfig::default(), None);
        let cases = [
            (
                "error: Individual quota reached. Resets in 40h19m40s.",
                ErrorKind::Quota,
            ),
            ("RESOURCE_EXHAUSTED (code 429)", ErrorKind::Quota),
            (
                "error: Eligibility check failed: not eligible",
                ErrorKind::Auth,
            ),
            ("invalid model selection (--model \"x\")", ErrorKind::Other),
        ];
        for (t, k) in cases {
            assert_eq!(d.classify_error(t), k, "{t}");
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn child_identity_rejects_reuse_reparenting_and_missing_stat() {
        let root = tempfile::tempdir().unwrap();
        let proc_dir = root.path().join("42");
        std::fs::create_dir(&proc_dir).unwrap();
        let stat = |parent: u32, start: u64| {
            format!(
                "42 (agy ) tricky name) S {parent} {} {start} 0\n",
                "0 ".repeat(17)
            )
        };
        std::fs::write(proc_dir.join("stat"), stat(7, 1234)).unwrap();
        let child = ChildIdentity::read(root.path(), 42).unwrap();
        assert_eq!(
            child,
            ChildIdentity {
                pid: 42,
                parent: 7,
                start_time: 1234
            }
        );
        assert!(child.is_current(root.path()));
        assert_eq!(first_child_identity(root.path(), 7), Some(child));
        std::fs::write(proc_dir.join("stat"), stat(8, 1234)).unwrap();
        assert!(
            !child.is_current(root.path()),
            "reparented process must not be signalled"
        );
        assert_eq!(first_child_identity(root.path(), 7), None);
        std::fs::write(proc_dir.join("stat"), stat(7, 5678)).unwrap();
        assert!(
            !child.is_current(root.path()),
            "reused pid must not be signalled"
        );
        std::fs::write(proc_dir.join("stat"), "malformed").unwrap();
        assert!(!child.is_current(root.path()));
        std::fs::remove_file(proc_dir.join("stat")).unwrap();
        assert!(!child.is_current(root.path()));
    }

    #[test]
    fn shared_config_root_checks_owner_and_write_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let uid = rustix::process::getuid().as_raw();
        let metadata = std::fs::metadata(root.path()).unwrap();
        assert!(trusted_config_root(&metadata, uid));
        assert!(!trusted_config_root(&metadata, uid.wrapping_add(1)));
        for mode in [0o775, 0o757] {
            std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(!trusted_config_root(
                &std::fs::metadata(root.path()).unwrap(),
                uid
            ));
        }
    }

    fn assert_finds_child(find: fn(u32) -> Option<u32>) {
        let mut c = std::process::Command::new("sh")
            .args(["-c", "sleep 5 & wait"])
            .spawn()
            .unwrap();
        let mut found = None;
        for _ in 0..50 {
            found = find(c.id());
            if found.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = c.kill();
        let _ = c.wait();
        assert!(found.is_some());
    }

    #[test]
    fn first_child_finds_a_child() {
        assert_finds_child(first_child);
    }

    #[cfg(unix)]
    #[test]
    fn pgrep_child_finds_a_child() {
        assert_finds_child(pgrep_child);
    }
}
