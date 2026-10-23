//! opencode driver (ADR-0004 "opencode"): `opencode serve` over HTTP, with
//! `GET /event` server-sent events. The server is owned by the run; the
//! password travels only in `OPENCODE_SERVER_PASSWORD`, never in argv, the
//! transcript or `Debug` output.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::Read as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use so_core::{ErrorKind, Harness};
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::driver::{
    Channel, Driver, DriverConfig, DriverError, Endpoint, HarnessEvent, JoinCommand, JoinInfo,
    LaunchSpec, RunControl, RunHandle, ShutdownOutcome, classify_common,
};
use crate::process::{
    ExitWatch, LineParser, MAX_LINE_BYTES, ProcessHandle, TRUNCATED_MARK, create_private_dir,
    default_truncated_event, record, spawn_supervised, startup_failure, write_private,
};
use crate::transcript::{Dir, SharedTranscript, TranscriptWriter};

/// Time allowed for the server to print its URL and answer `/global/health`.
const READY_TIMEOUT: Duration = Duration::from_secs(20);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const USERNAME: &str = "opencode";
const PASSWORD_ENV: &str = "OPENCODE_SERVER_PASSWORD";
/// Longest HTTP body kept in a transcript summary.
const MAX_BODY_LOG: usize = 2048;
const STRUCTURED_RETRIES: u32 = 2;
/// No bytes (not even a heartbeat, sent about every 30 s) for this long means
/// the stream is dead.
const SSE_IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// Waits before each of the three reconnect attempts.
const RECONNECT_BACKOFF: [Duration; 3] = [
    Duration::from_millis(500),
    Duration::from_secs(1),
    Duration::from_secs(2),
];

/// Where the server comes from.
#[derive(Clone, Default)]
enum ServerMode {
    /// Spawn `opencode serve` (production).
    #[default]
    Spawn,
    /// Use a server that is already running (tests, external servers). The
    /// driver neither starts nor kills it.
    Existing { url: String, password: String },
}

impl fmt::Debug for ServerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ServerMode::Spawn => f.write_str("Spawn"),
            ServerMode::Existing { url, .. } => f
                .debug_struct("Existing")
                .field("url", url)
                .finish_non_exhaustive(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct OpencodeDriver {
    config: DriverConfig,
    server: ServerMode,
}

impl OpencodeDriver {
    pub fn new(config: DriverConfig) -> Self {
        Self {
            config,
            server: ServerMode::Spawn,
        }
    }

    /// Test hook: talk to an already running server instead of spawning one.
    /// No process is owned, so `pid()` is `None` and shutdown only stops the
    /// event stream.
    pub fn with_existing_server(
        config: DriverConfig,
        url: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self {
            config,
            server: ServerMode::Existing {
                url: url.into().trim_end_matches('/').to_string(),
                password: password.into(),
            },
        }
    }
}

/// `{"mcp": ..., "permission": ...}` for `OPENCODE_CONFIG` (run-config.example.json).
fn run_config(spec: &LaunchSpec) -> Value {
    let env: Map<String, Value> = spec
        .mcp
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    let mut command = vec![Value::String(spec.mcp.command.display().to_string())];
    command.extend(spec.mcp.args.iter().cloned().map(Value::String));
    let mut config = json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {"superorchestrator": {
            "type": "local",
            "command": command,
            "environment": env,
            "enabled": true,
        }},
        "permission": {"*": "allow"},
    });
    spec.agent
        .merge_into(&mut config, ("mcp", "superorchestrator"));
    config
}

/// 32 random bytes as hex.
fn random_password() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Splits `provider/model` at the first slash.
fn split_model(model: &str) -> Result<(String, String), DriverError> {
    match model.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => Ok((p.into(), m.into())),
        _ => Err(DriverError::Other(format!(
            "opencode model must be provider/model, got {model:?}"
        ))),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...[truncated]", &s[..end])
}

// ---------------------------------------------------------------------------
// Output: transcript plus event channel, shared by every task of one run.
// ---------------------------------------------------------------------------

/// The single event sender of a run. Closing it (after `Exited`) lets the
/// consumer's channel end.
struct Out {
    tx: Mutex<Option<mpsc::UnboundedSender<HarnessEvent>>>,
    transcript: SharedTranscript,
}

impl Out {
    fn new(tx: mpsc::UnboundedSender<HarnessEvent>, transcript: SharedTranscript) -> Arc<Self> {
        Arc::new(Self {
            tx: Mutex::new(Some(tx)),
            transcript,
        })
    }

    /// Records `raw` in the transcript, then emits `Raw` and `events`.
    fn emit(&self, dir: Dir, channel: Channel, raw: &str, events: Vec<HarnessEvent>) {
        let guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        let Some(tx) = guard.as_ref() else { return };
        let offset = record(&self.transcript, dir, channel, raw, &events, tx);
        let _ = tx.send(HarnessEvent::Raw {
            channel,
            line: raw.to_string(),
            offset,
        });
        for e in events {
            let _ = tx.send(e);
        }
    }

    fn event(&self, e: HarnessEvent) {
        let guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(e);
        }
    }

    /// Sends the terminal event and closes the channel.
    fn close_with(&self, last: HarnessEvent) {
        let mut guard = self.tx.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(tx) = guard.take() {
            let _ = tx.send(last);
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

/// Authenticated access to one server. Cheap to clone.
#[derive(Clone)]
struct Api {
    http: reqwest::Client,
    base: String,
    password: String,
    directory: PathBuf,
    out: Arc<Out>,
}

impl fmt::Debug for Api {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Api")
            .field("base", &self.base)
            .field("directory", &self.directory)
            .finish_non_exhaustive()
    }
}

/// An error answer of the server. The status code is the harness's own signal: 401 and
/// 403 are auth, 429 a rate limit, and both are returned as [`DriverError::Exited`] so the
/// supervisor can fail the run over. The body is never used to classify.
fn http_error(method: &str, path: &str, status: reqwest::StatusCode, body: &str) -> DriverError {
    let text = format!(
        "opencode {method} {path}: HTTP {status} {}",
        truncate(body.trim(), 300)
    );
    match status.as_u16() {
        401 | 403 => DriverError::Exited {
            kind: so_core::ErrorKind::Auth,
            text,
        },
        429 => DriverError::Exited {
            kind: so_core::ErrorKind::RateLimit,
            text,
        },
        _ => DriverError::Other(text),
    }
}

impl Api {
    fn builder(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .basic_auth(USERNAME, Some(&self.password))
            .header("x-opencode-directory", self.directory.display().to_string())
    }

    /// Sends a request and records a summary of both sides (never the auth
    /// header) in the transcript.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String, DriverError> {
        let mut req = self.builder(method.clone(), path).timeout(REQUEST_TIMEOUT);
        let summary = match body {
            Some(b) => {
                req = req.json(b);
                format!("{method} {path} {}", truncate(&b.to_string(), MAX_BODY_LOG))
            }
            None => format!("{method} {path}"),
        };
        self.out.emit(Dir::Out, Channel::Http, &summary, vec![]);
        let resp = req.send().await.map_err(|e| {
            DriverError::Other(format!("opencode {method} {path}: request failed: {e}"))
        })?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        self.out.emit(
            Dir::In,
            Channel::Http,
            &format!(
                "{} {method} {path} {}",
                status.as_u16(),
                truncate(&text, MAX_BODY_LOG)
            ),
            vec![],
        );
        if status.is_success() {
            Ok(text)
        } else {
            Err(http_error(method.as_str(), path, status, &text))
        }
    }

    /// Health probe that is not recorded. `Ok(true)` when healthy.
    async fn healthy(&self) -> Result<bool, DriverError> {
        match self
            .builder(reqwest::Method::GET, "/global/health")
            .timeout(Duration::from_secs(3))
            .send()
            .await
        {
            Ok(r) if r.status().is_success() => Ok(true),
            Ok(r) if matches!(r.status().as_u16(), 401 | 403) => {
                Err(http_error("GET", "/global/health", r.status(), ""))
            }
            _ => Ok(false),
        }
    }

    /// Opens `GET /event`.
    async fn open_events(&self) -> Result<reqwest::Response, DriverError> {
        let resp = self
            .builder(reqwest::Method::GET, "/event")
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| DriverError::Other(format!("opencode GET /event: request failed: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(http_error("GET", "/event", status, &body));
        }
        Ok(resp)
    }

    /// Polls `/global/health` until `deadline`; fails early when the server
    /// process exits.
    async fn wait_ready(
        &self,
        deadline: tokio::time::Instant,
        mut exit: Option<ExitWatch>,
    ) -> Result<(), DriverError> {
        let timeout =
            || DriverError::Other("opencode server did not become healthy within 20 s".into());
        loop {
            let probe = tokio::time::timeout_at(deadline, self.healthy());
            let healthy = match exit.as_mut() {
                Some(e) => tokio::select! {
                    r = probe => r.map_err(|_| timeout())??,
                    _ = e.wait_for(|v| v.is_some()) => {
                        return Err(DriverError::Other(
                            "opencode serve exited before becoming healthy".into(),
                        ));
                    }
                },
                None => probe.await.map_err(|_| timeout())??,
            };
            if healthy {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(timeout());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Event mapping
// ---------------------------------------------------------------------------

/// Turns the server-wide event stream into [`HarnessEvent`]s for one session.
#[derive(Debug)]
pub struct EventMapper {
    session_id: String,
    expect_structured: bool,
    roles: HashMap<String, String>,
    /// Finished text parts whose message role is not known yet.
    pending_text: HashMap<String, Vec<(String, String)>>,
    text_done: HashSet<String>,
    tools_started: HashSet<String>,
    tools_finished: HashSet<String>,
    usage_done: HashSet<String>,
    active: bool,
    last_text: Option<String>,
    structured: Option<Value>,
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_str)
}

fn num_at(v: &Value, path: &[&str]) -> u64 {
    path.iter()
        .try_fold(v, |v, k| v.get(*k))
        .and_then(Value::as_f64)
        .map_or(0, |n| n.max(0.0) as u64)
}

/// opencode rejects an id it does not list with `Model not found: <id>. Did you mean: ...`.
/// That is the run's configuration, not the account, so it is checked before the shared
/// patterns (the "Did you mean" list can carry words those match).
fn classify_text(text: &str) -> ErrorKind {
    if text.to_lowercase().contains("model not found") {
        ErrorKind::Config
    } else {
        classify_common(text)
    }
}

/// Kind and text of a `session.error` / message `error` object.
fn classify_api_error(error: &Value) -> (ErrorKind, String) {
    let name = error.get("name").and_then(Value::as_str).unwrap_or("Error");
    let data = error.get("data").unwrap_or(&Value::Null);
    let message = data.get("message").and_then(Value::as_str).unwrap_or("");
    let status = data.get("statusCode").and_then(Value::as_u64);
    let text = match status {
        Some(s) => format!("{name} {s}: {message}"),
        None => format!("{name}: {message}"),
    };
    let kind = match (name, status) {
        (_, Some(402)) => ErrorKind::Quota,
        (_, Some(429)) => ErrorKind::RateLimit,
        (_, Some(401 | 403)) | ("ProviderAuthError", _) => ErrorKind::Auth,
        _ => classify_text(&text),
    };
    (kind, text)
}

impl EventMapper {
    pub fn new(session_id: &str, expect_structured: bool) -> Self {
        Self {
            session_id: session_id.to_string(),
            expect_structured,
            roles: HashMap::new(),
            pending_text: HashMap::new(),
            text_done: HashSet::new(),
            tools_started: HashSet::new(),
            tools_finished: HashSet::new(),
            usage_done: HashSet::new(),
            active: false,
            last_text: None,
            structured: None,
        }
    }

    /// Whether a turn is running (between busy and idle or error).
    pub fn active(&self) -> bool {
        self.active
    }

    /// Maps one `data:` payload. Unparseable payloads and other sessions'
    /// events yield nothing.
    pub fn map_line(&mut self, data: &str) -> Vec<HarnessEvent> {
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return vec![];
        };
        let kind = v.get("type").and_then(Value::as_str).unwrap_or("");
        let props = v.get("properties").unwrap_or(&Value::Null);
        let sid = str_at(props, &["sessionID"])
            .or_else(|| str_at(props, &["part", "sessionID"]))
            .or_else(|| str_at(props, &["info", "sessionID"]));
        // A `session.error` without a session id while a turn runs is ours:
        // the server has one session per run.
        let ours = sid == Some(self.session_id.as_str())
            || (sid.is_none() && kind == "session.error" && self.active);
        if !ours {
            return vec![];
        }
        let mut out = Vec::new();
        match kind {
            "session.status" if str_at(props, &["status", "type"]) == Some("busy") => {
                self.start_turn(&mut out);
            }
            "message.updated" => {
                self.on_message(props.get("info").unwrap_or(&Value::Null), &mut out)
            }
            "message.part.updated" => {
                self.on_part(props.get("part").unwrap_or(&Value::Null), &mut out)
            }
            "session.error" => {
                let error = props.get("error").unwrap_or(&Value::Null);
                let (error_kind, text) = if str_at(error, &["name"]) == Some("MessageAbortedError")
                {
                    (ErrorKind::Other, "turn aborted".to_string())
                } else {
                    classify_api_error(error)
                };
                self.start_turn(&mut out);
                self.finish(true, Some(error_kind), Some(text), &mut out);
            }
            // `session.status` idle and `session.idle` both end the turn; the
            // second one finds no active turn and is ignored.
            "session.idle" if self.active => self.finish(false, None, None, &mut out),
            "session.status"
                if self.active && str_at(props, &["status", "type"]) == Some("idle") =>
            {
                self.finish(false, None, None, &mut out);
            }
            _ => {}
        }
        out
    }

    /// The session was found idle after a reconnect (events were missed).
    pub fn on_idle_detected(&mut self) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        if self.active {
            self.finish(false, None, None, &mut out);
        }
        out
    }

    /// The stream ended; a running turn can no longer finish.
    pub fn on_stream_end(&mut self, reason: &str) -> Vec<HarnessEvent> {
        let mut out = Vec::new();
        if self.active {
            self.finish(
                true,
                Some(ErrorKind::Other),
                Some(reason.to_string()),
                &mut out,
            );
        }
        out
    }

    fn start_turn(&mut self, out: &mut Vec<HarnessEvent>) {
        if !self.active {
            self.active = true;
            self.last_text = None;
            self.structured = None;
            out.push(HarnessEvent::TurnStarted);
        }
    }

    fn finish(
        &mut self,
        is_error: bool,
        error_kind: Option<ErrorKind>,
        error_text: Option<String>,
        out: &mut Vec<HarnessEvent>,
    ) {
        self.active = false;
        let result_text = self.last_text.take();
        let mut structured = self.structured.take();
        if structured.is_none() && self.expect_structured && !is_error {
            structured = result_text
                .as_deref()
                .and_then(|t| serde_json::from_str(t.trim()).ok());
        }
        out.push(HarnessEvent::TurnFinished {
            result_text,
            structured,
            is_error,
            error_kind,
            error_text,
        });
    }

    fn on_message(&mut self, info: &Value, out: &mut Vec<HarnessEvent>) {
        let (Some(id), Some(role)) = (str_at(info, &["id"]), str_at(info, &["role"])) else {
            return;
        };
        self.roles.insert(id.to_string(), role.to_string());
        if role != "assistant" {
            self.pending_text.remove(id);
            return;
        }
        if let Some(parts) = self.pending_text.remove(id) {
            for (part_id, text) in parts {
                self.assistant_text(&part_id, text, out);
            }
        }
        if let Some(s) = info.get("structured").filter(|s| !s.is_null()) {
            self.structured = Some(s.clone());
        }
        let completed = info
            .pointer("/time/completed")
            .is_some_and(|c| !c.is_null());
        if completed && self.usage_done.insert(id.to_string()) {
            let t = info.get("tokens").unwrap_or(&Value::Null);
            out.push(HarnessEvent::Usage {
                input_tokens: num_at(t, &["input"])
                    + num_at(t, &["cache", "read"])
                    + num_at(t, &["cache", "write"]),
                output_tokens: num_at(t, &["output"]) + num_at(t, &["reasoning"]),
                cost: info.get("cost").and_then(Value::as_f64),
                rate_limits: None,
            });
        }
    }

    fn on_part(&mut self, part: &Value, out: &mut Vec<HarnessEvent>) {
        match str_at(part, &["type"]) {
            Some("text") => {
                if part.pointer("/time/end").is_none_or(Value::is_null) {
                    return;
                }
                let (Some(pid), Some(mid)) = (str_at(part, &["id"]), str_at(part, &["messageID"]))
                else {
                    return;
                };
                let text = str_at(part, &["text"]).unwrap_or("").to_string();
                match self.roles.get(mid).map(String::as_str) {
                    Some("assistant") => self.assistant_text(pid, text, out),
                    Some(_) => {}
                    None => self
                        .pending_text
                        .entry(mid.to_string())
                        .or_default()
                        .push((pid.to_string(), text)),
                }
            }
            Some("tool") => {
                let name = str_at(part, &["tool"]).unwrap_or("tool").to_string();
                let Some(key) = str_at(part, &["id"]).or_else(|| str_at(part, &["callID"])) else {
                    return;
                };
                let status = str_at(part, &["state", "status"]).unwrap_or("");
                if self.tools_started.insert(key.to_string()) {
                    out.push(HarnessEvent::ToolCallStarted { name: name.clone() });
                }
                if matches!(status, "completed" | "error") && self.tools_finished.insert(key.into())
                {
                    out.push(HarnessEvent::ToolCallFinished {
                        name,
                        ok: status == "completed",
                    });
                }
            }
            _ => {}
        }
    }

    fn assistant_text(&mut self, part_id: &str, text: String, out: &mut Vec<HarnessEvent>) {
        if text.is_empty() || !self.text_done.insert(part_id.to_string()) {
            return;
        }
        self.last_text = Some(text.clone());
        out.push(HarnessEvent::AssistantMessage { text });
    }
}

// ---------------------------------------------------------------------------
// SSE
// ---------------------------------------------------------------------------

/// One server-sent event: the raw lines of the block and the joined `data`.
#[derive(Debug, Default, PartialEq, Eq)]
struct Frame {
    lines: Vec<String>,
    data: Option<String>,
    /// A line or the whole event exceeded the size cap and was cut.
    truncated: bool,
}

/// Splits a byte stream into SSE events. Chunk boundaries may fall anywhere,
/// including inside a UTF-8 sequence: bytes are only decoded per full line.
/// An event ends at a blank line; its `data:` lines are joined with `\n`.
struct SseFramer {
    max: usize,
    buf: Vec<u8>,
    line_over: bool,
    cur: Frame,
    data: Vec<String>,
    size: usize,
}

impl SseFramer {
    fn new(max: usize) -> Self {
        Self {
            max,
            buf: Vec::new(),
            line_over: false,
            cur: Frame::default(),
            data: Vec::new(),
            size: 0,
        }
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        let mut frames = Vec::new();
        for &b in bytes {
            if b != b'\n' {
                if self.buf.len() < self.max {
                    self.buf.push(b);
                } else {
                    self.line_over = true;
                }
            } else if let Some(f) = self.end_line() {
                frames.push(f);
            }
        }
        frames
    }

    fn end_line(&mut self) -> Option<Frame> {
        let mut line = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        if line.ends_with('\r') {
            line.pop();
        }
        if std::mem::take(&mut self.line_over) {
            self.cur.truncated = true;
            line.push_str(TRUNCATED_MARK);
        }
        if line.is_empty() {
            if self.cur.lines.is_empty() && !self.cur.truncated {
                return None;
            }
            let mut f = std::mem::take(&mut self.cur);
            if !self.data.is_empty() {
                f.data = Some(self.data.join("\n"));
            }
            self.data.clear();
            self.size = 0;
            return Some(f);
        }
        self.size += line.len();
        if self.size > self.max {
            self.cur.truncated = true;
            return None;
        }
        if let Some(d) = line.strip_prefix("data:") {
            self.data.push(d.strip_prefix(' ').unwrap_or(d).to_string());
        }
        self.cur.lines.push(line);
        None
    }

    /// Lines of an unterminated event when the stream ends.
    fn finish(&mut self) -> Vec<String> {
        let mut lines = std::mem::take(&mut self.cur.lines);
        if !self.buf.is_empty() {
            lines.push(String::from_utf8_lossy(&self.buf).into_owned());
        }
        *self = Self::new(self.max);
        lines
    }
}

/// Records a frame's lines and attaches the mapped events to the last one.
fn dispatch(frame: Frame, out: &Out, mapper: &mut EventMapper) {
    let mut events = Vec::new();
    if frame.truncated {
        events.push(default_truncated_event());
        events.extend(mapper.on_stream_end("opencode event exceeded 4 MiB"));
    } else if let Some(d) = &frame.data {
        events = mapper.map_line(d);
    }
    let n = frame.lines.len();
    if n == 0 {
        // Only a cut line was recorded.
        out.emit(Dir::In, Channel::Sse, TRUNCATED_MARK, events);
        return;
    }
    let mut events = Some(events);
    for (i, line) in frame.lines.iter().enumerate() {
        let evs = if i + 1 == n {
            events.take().unwrap_or_default()
        } else {
            vec![]
        };
        out.emit(Dir::In, Channel::Sse, line, evs);
    }
}

/// Stop signals for the SSE task: the control was dropped or shut down, or
/// the server process exited.
struct Signals {
    stop: watch::Receiver<()>,
    exit: Option<ExitWatch>,
}

impl Signals {
    async fn fired(&mut self) {
        match &mut self.exit {
            Some(e) => tokio::select! {
                _ = self.stop.changed() => {}
                _ = e.wait_for(|v| v.is_some()) => {}
            },
            None => {
                let _ = self.stop.changed().await;
            }
        }
    }
}

enum Read {
    Stopped,
    Dropped(String),
}

async fn read_stream(
    resp: &mut reqwest::Response,
    framer: &mut SseFramer,
    out: &Out,
    mapper: &mut EventMapper,
    sig: &mut Signals,
) -> Read {
    loop {
        let chunk = tokio::select! {
            _ = sig.fired() => return Read::Stopped,
            c = tokio::time::timeout(SSE_IDLE_TIMEOUT, resp.chunk()) => c,
        };
        let bytes = match chunk {
            Err(_) => return Read::Dropped("no data for 10 minutes".into()),
            Ok(Ok(Some(b))) => b,
            Ok(Ok(None)) => return Read::Dropped("stream closed by the server".into()),
            Ok(Err(e)) => return Read::Dropped(format!("stream failed: {e}")),
        };
        for frame in framer.feed(&bytes) {
            dispatch(frame, out, mapper);
        }
    }
}

/// Reconnects `GET /event` with backoff. `None` if stopped meanwhile.
async fn reconnect(api: &Api, sig: &mut Signals) -> Option<Result<reqwest::Response, DriverError>> {
    let mut last = DriverError::Other("no attempt".into());
    for delay in RECONNECT_BACKOFF {
        tokio::select! {
            _ = sig.fired() => return None,
            _ = tokio::time::sleep(delay) => {}
        }
        api.out
            .emit(Dir::Out, Channel::Http, "GET /event (reconnect)", vec![]);
        match api.open_events().await {
            Ok(r) => return Some(Ok(r)),
            Err(e) => last = e,
        }
    }
    Some(Err(last))
}

/// Reads `resp` as server-sent events until stopped. A dropped stream is
/// reopened; if that fails the run ends: open turn failed, `Error`, server
/// stopped, `Exited`.
async fn sse_loop(
    mut resp: reqwest::Response,
    api: Api,
    mut mapper: EventMapper,
    mut sig: Signals,
    proc: Option<Arc<ProcessHandle>>,
) {
    let out = api.out.clone();
    let mut framer = SseFramer::new(MAX_LINE_BYTES);
    loop {
        let reason = match read_stream(&mut resp, &mut framer, &out, &mut mapper, &mut sig).await {
            Read::Stopped => return,
            Read::Dropped(r) => r,
        };
        for line in framer.finish() {
            out.emit(Dir::In, Channel::Sse, &line, vec![]);
        }
        match reconnect(&api, &mut sig).await {
            None => return,
            Some(Ok(r)) => {
                resp = r;
                framer = SseFramer::new(MAX_LINE_BYTES);
                resync(&api, &mut mapper).await;
            }
            Some(Err(e)) => {
                let msg = format!("opencode event stream lost ({reason}): {e}");
                for ev in mapper.on_stream_end(&msg) {
                    out.event(ev);
                }
                out.event(HarnessEvent::Error {
                    kind: ErrorKind::Other,
                    message: msg,
                });
                match &proc {
                    // The forwarder closes the channel on `Exited`.
                    Some(p) => {
                        p.shutdown(Duration::ZERO).await;
                    }
                    None => out.close_with(HarnessEvent::Exited { code: None }),
                }
                return;
            }
        }
    }
}

/// After a reconnect events may have been missed. If a turn is open but the
/// server says the session is idle, close the turn.
async fn resync(api: &Api, mapper: &mut EventMapper) {
    if !mapper.active() {
        return;
    }
    let Ok(text) = api
        .call(reqwest::Method::GET, "/session/status", None)
        .await
    else {
        return;
    };
    let busy = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get(&mapper.session_id).cloned())
        .is_some_and(|s| str_at(&s, &["type"]) != Some("idle"));
    if !busy {
        for ev in mapper.on_idle_detected() {
            api.out.event(ev);
        }
    }
}

// ---------------------------------------------------------------------------
// Process
// ---------------------------------------------------------------------------

/// Finds `opencode server listening on <url>` on stdout.
struct ListenParser {
    url: Option<oneshot::Sender<String>>,
}

impl LineParser for ListenParser {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        if self.url.is_some()
            && let Some((_, rest)) = line.split_once("listening on ")
            && let Some(url) = rest.split_whitespace().next()
            && url.starts_with("http")
            && let Some(tx) = self.url.take()
        {
            let _ = tx.send(url.trim_end_matches('/').to_string());
        }
        vec![]
    }
}

// ---------------------------------------------------------------------------
// Control
// ---------------------------------------------------------------------------

struct PromptDefaults {
    /// The custom agent the session runs as.
    agent: Option<String>,
    model: Option<(String, String)>,
    variant: Option<String>,
    system: Option<String>,
    format: Option<Value>,
}

struct Inner {
    api: Api,
    session_id: String,
    defaults: PromptDefaults,
    stop: Arc<watch::Sender<()>>,
    sse: Arc<Mutex<Option<JoinHandle<()>>>>,
    proc: Option<Arc<ProcessHandle>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.stop.send(());
    }
}

/// Cheap clonable handle for live control of one opencode run.
#[derive(Clone)]
pub struct OpencodeControl {
    inner: Arc<Inner>,
}

impl fmt::Debug for OpencodeControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpencodeControl")
            .field("url", &self.inner.api.base)
            .field("session", &self.inner.session_id)
            .field("pid", &self.pid())
            .finish_non_exhaustive()
    }
}

impl OpencodeControl {
    fn prompt_body(&self, text: &str) -> Value {
        let d = &self.inner.defaults;
        let mut body = Map::new();
        if let Some(a) = &d.agent {
            body.insert("agent".into(), json!(a));
        }
        if let Some((provider, model)) = &d.model {
            body.insert(
                "model".into(),
                json!({"providerID": provider, "modelID": model}),
            );
        }
        if let Some(v) = &d.variant {
            body.insert("variant".into(), json!(v));
        }
        if let Some(s) = &d.system {
            body.insert("system".into(), json!(s));
        }
        if let Some(f) = &d.format {
            body.insert("format".into(), f.clone());
        }
        body.insert("parts".into(), json!([{"type": "text", "text": text}]));
        Value::Object(body)
    }

    /// Queues a user message (`prompt_async`, 204).
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        let path = format!("/session/{}/prompt_async", self.inner.session_id);
        self.inner
            .api
            .call(reqwest::Method::POST, &path, Some(&self.prompt_body(text)))
            .await
            .map(|_| ())
    }

    /// Aborts the running turn (`POST /session/{id}/abort`); the session
    /// stays usable.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        let path = format!("/session/{}/abort", self.inner.session_id);
        self.inner
            .api
            .call(reqwest::Method::POST, &path, None)
            .await
            .map(|_| ())
    }

    /// Drops the event stream and stops the server. A server is not stopped by
    /// closing input, so it is sent SIGTERM at once (SIGKILL after the grace
    /// period); `deadline` is not used. With a pre-started server nothing is
    /// killed.
    pub async fn shutdown(&self, _deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        let _ = self.inner.stop.send(());
        let sse = self.inner.sse.lock().ok().and_then(|mut s| s.take());
        if let Some(h) = sse {
            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
        }
        match &self.inner.proc {
            Some(p) => {
                // The forwarder task closes the event channel on `Exited`.
                let outcome = p.shutdown(Duration::ZERO).await;
                Ok(outcome)
            }
            None => {
                self.inner
                    .api
                    .out
                    .close_with(HarnessEvent::Exited { code: None });
                Ok(ShutdownOutcome::Exited(None))
            }
        }
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.proc.as_ref().and_then(|p| p.pid)
    }
}

// ---------------------------------------------------------------------------
// Driver
// ---------------------------------------------------------------------------

impl Driver for OpencodeDriver {
    fn harness(&self) -> Harness {
        Harness::Opencode
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        create_private_dir(&spec.run_dir)?;
        let defaults = PromptDefaults {
            agent: spec.agent.agent_name.clone(),
            model: spec.model.as_deref().map(split_model).transpose()?,
            variant: spec.effort.clone(),
            system: Some(spec.system_addendum.clone()).filter(|s| !s.is_empty()),
            format: spec.output_schema.as_ref().map(|schema| {
                json!({"type": "json_schema", "schema": schema, "retryCount": STRUCTURED_RETRIES})
            }),
        };
        let (tx, events) = mpsc::unbounded_channel();
        // One budget for the URL and the health check.
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;

        // Server: spawned (own transcript via the process pipeline) or given.
        let (proc, forward, out, base, password) = match &self.server {
            ServerMode::Spawn => {
                let config_path = spec.run_dir.join("opencode.json");
                write_private(
                    &config_path,
                    &serde_json::to_vec_pretty(&run_config(&spec)).map_err(std::io::Error::from)?,
                )?;
                let password = random_password().map_err(|e| {
                    DriverError::Other(format!("cannot read random bytes from /dev/urandom: {e}"))
                })?;
                let (url_tx, url_rx) = oneshot::channel();
                let mut cmd = Command::new(&spec.launch.program);
                cmd.args(&spec.launch.argv_prefix)
                    .args(&spec.launch.extra_args)
                    .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
                    .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
                    .env("OPENCODE_CONFIG", &config_path)
                    .env("OPENCODE_SERVER_USERNAME", USERNAME)
                    .env(PASSWORD_ENV, &password)
                    .current_dir(&spec.cwd);
                let sup = spawn_supervised(
                    cmd,
                    &spec.run_dir,
                    self.config.shutdown_grace,
                    ListenParser { url: Some(url_tx) },
                )?;
                let out = Out::new(tx, sup.transcript.clone());
                let mut exit = sup.handle.exit.clone();
                let url = tokio::select! {
                    u = tokio::time::timeout_at(deadline, url_rx) => match u {
                        Ok(Ok(u)) => Ok(u),
                        Ok(Err(_)) => Err("opencode serve exited before printing its URL".to_string()),
                        Err(_) => Err("opencode serve did not print its URL within 20 s".to_string()),
                    },
                    _ = exit.wait_for(|v| v.is_some()) => {
                        Err("opencode serve exited before printing its URL".to_string())
                    }
                };
                match url {
                    Ok(url) => (
                        Some(Arc::new(sup.handle)),
                        Some(sup.events),
                        out,
                        url,
                        password,
                    ),
                    Err(msg) => {
                        sup.handle.shutdown(Duration::ZERO).await;
                        let mut sup_events = sup.events;
                        return Err(startup_failure(
                            DriverError::Other(msg),
                            &mut sup_events,
                            |t| self.classify_error(t),
                        )
                        .await);
                    }
                }
            }
            ServerMode::Existing { url, password } => {
                let transcript: SharedTranscript =
                    Arc::new(Mutex::new(TranscriptWriter::create(&spec.run_dir)?));
                (
                    None,
                    None,
                    Out::new(tx, transcript),
                    url.clone(),
                    password.clone(),
                )
            }
        };

        let http = reqwest::Client::builder()
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| DriverError::Other(format!("http client: {e}")))?;
        let api = Api {
            http,
            base,
            password,
            directory: spec.cwd.clone(),
            out: out.clone(),
        };

        // A server that dies or stalls before it is healthy says why on its stderr (not
        // signed in, out of quota): report that, classified, so the run can fail over.
        let mut forward = forward;
        if let Err(e) = api
            .wait_ready(deadline, proc.as_ref().map(|p| p.exit.clone()))
            .await
        {
            if let Some(p) = &proc {
                p.shutdown(Duration::ZERO).await;
            }
            return Err(match forward.as_mut() {
                Some(sup_events) => {
                    startup_failure(e, sup_events, |t| self.classify_error(t)).await
                }
                None => e,
            });
        }

        let sse_slot: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        let (stop, stop_rx) = watch::channel(());
        let stop = Arc::new(stop);
        if let Some(mut sup_events) = forward {
            let out = out.clone();
            let slot = sse_slot.clone();
            let stop = stop.clone();
            tokio::spawn(async move {
                while let Some(ev) = sup_events.recv().await {
                    if let HarnessEvent::Exited { .. } = ev {
                        // Tell the SSE task first so it does not retry against a dead server.
                        let _ = stop.send(());
                        let sse = slot.lock().ok().and_then(|mut s| s.take());
                        if let Some(h) = sse {
                            let _ = tokio::time::timeout(Duration::from_secs(2), h).await;
                        }
                        out.close_with(ev);
                        return;
                    }
                    out.event(ev);
                }
            });
        }

        let started = self
            .start_session(&spec, &api, &out, (stop_rx, &proc), &sse_slot)
            .await;
        let session_id = match started {
            Ok(id) => id,
            Err(e) => {
                let _ = stop.send(());
                match &proc {
                    Some(p) => {
                        p.shutdown(Duration::ZERO).await;
                    }
                    None => out.close_with(HarnessEvent::Exited { code: None }),
                }
                return Err(e);
            }
        };

        let control = OpencodeControl {
            inner: Arc::new(Inner {
                api: api.clone(),
                session_id: session_id.clone(),
                defaults,
                stop,
                sse: sse_slot,
                proc,
            }),
        };
        if let Err(e) = control.send(&spec.prompt).await {
            let _ = control.shutdown(Duration::ZERO).await;
            return Err(e);
        }
        Ok(RunHandle {
            events,
            control: RunControl::Opencode(control),
            native_session_id: Some(session_id.clone()),
            join: JoinInfo {
                account_name: spec.account.name.clone(),
                native_session_id: session_id,
                endpoint: Some(Endpoint {
                    url: api.base.clone(),
                    secret_env: Some((PASSWORD_ENV.into(), api.password.clone())),
                    directory: Some(spec.cwd.clone()),
                }),
                cwd: spec.cwd.clone(),
            },
        })
    }

    /// Live (endpoint known): `opencode attach <url> --session <id>` with the
    /// password in the environment. Afterwards: `<account> --session <id>`.
    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        match &j.endpoint {
            Some(ep) => {
                let mut argv: Vec<String> = [
                    "opencode",
                    "attach",
                    ep.url.as_str(),
                    "--session",
                    j.native_session_id.as_str(),
                ]
                .map(String::from)
                .into();
                if let Some(d) = &ep.directory {
                    argv.extend(["--dir".into(), d.display().to_string()]);
                }
                JoinCommand {
                    argv,
                    env: ep.secret_env.iter().cloned().collect(),
                    cwd: j.cwd.clone(),
                }
            }
            None => JoinCommand {
                argv: vec![
                    j.account_name.clone(),
                    "--session".into(),
                    j.native_session_id.clone(),
                ],
                env: vec![],
                cwd: j.cwd.clone(),
            },
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        classify_text(text)
    }
}

impl OpencodeDriver {
    /// Waits for the server, opens the event stream, then creates (or reuses)
    /// the session. The stream is opened first so no event is missed.
    async fn start_session(
        &self,
        spec: &LaunchSpec,
        api: &Api,
        out: &Arc<Out>,
        (stop_rx, proc): (watch::Receiver<()>, &Option<Arc<ProcessHandle>>),
        sse_slot: &Arc<Mutex<Option<JoinHandle<()>>>>,
    ) -> Result<String, DriverError> {
        let resp = api.open_events().await?;
        out.emit(Dir::Out, Channel::Http, "GET /event", vec![]);

        let session_id = match &spec.resume {
            Some(id) if spec.fork => {
                let text = api
                    .call(
                        reqwest::Method::POST,
                        &format!("/session/{id}/fork"),
                        Some(&json!({})),
                    )
                    .await?;
                serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(String::from))
                    .ok_or_else(|| {
                        DriverError::Other("opencode session fork: response has no id".into())
                    })?
            }
            Some(id) => id.clone(),
            None => {
                let body = json!({"title": spec.run_id});
                let text = api
                    .call(reqwest::Method::POST, "/session", Some(&body))
                    .await?;
                serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("id").and_then(Value::as_str).map(String::from))
                    .ok_or_else(|| {
                        DriverError::Other("opencode POST /session: response has no id".into())
                    })?
            }
        };
        let mapper = EventMapper::new(&session_id, spec.output_schema.is_some());
        let sig = Signals {
            stop: stop_rx,
            exit: proc.as_ref().map(|p| p.exit.clone()),
        };
        let handle = tokio::spawn(sse_loop(resp, api.clone(), mapper, sig, proc.clone()));
        if let Ok(mut slot) = sse_slot.lock() {
            *slot = Some(handle);
        }
        out.event(HarnessEvent::SessionStarted {
            native_session_id: session_id.clone(),
        });
        Ok(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SID: &str = "ses_1";

    fn ev(v: Value) -> String {
        v.to_string()
    }

    fn text_part(msg: &str, id: &str, text: &str) -> String {
        ev(
            json!({"type":"message.part.updated","properties":{"sessionID":SID,"part":{
            "id":id,"type":"text","messageID":msg,"sessionID":SID,"text":text,
            "time":{"start":1,"end":2}}}}),
        )
    }

    fn role(msg: &str, role: &str) -> String {
        ev(
            json!({"type":"message.updated","properties":{"sessionID":SID,
            "info":{"id":msg,"role":role,"sessionID":SID,"time":{"created":1}}}}),
        )
    }

    fn busy() -> String {
        ev(json!({"type":"session.status","properties":{"sessionID":SID,"status":{"type":"busy"}}}))
    }

    fn idle() -> String {
        ev(json!({"type":"session.idle","properties":{"sessionID":SID}}))
    }

    #[test]
    fn text_before_role_is_held_until_the_role_is_known() {
        let mut m = EventMapper::new(SID, false);
        assert_eq!(m.map_line(&busy()), vec![HarnessEvent::TurnStarted]);
        assert!(m.map_line(&text_part("msg_a", "p1", "hello")).is_empty());
        assert_eq!(
            m.map_line(&role("msg_a", "assistant")),
            vec![HarnessEvent::AssistantMessage {
                text: "hello".into()
            }]
        );
        // A user message's finished text part never becomes an assistant message.
        assert!(m.map_line(&text_part("msg_u", "p2", "prompt")).is_empty());
        assert!(m.map_line(&role("msg_u", "user")).is_empty());
        // Re-sending the same part is not a second message.
        assert!(m.map_line(&text_part("msg_a", "p1", "hello")).is_empty());
    }

    #[test]
    fn idle_ends_a_turn_once_whichever_form_comes_first() {
        let status_idle = ev(
            json!({"type":"session.status","properties":{"sessionID":SID,"status":{"type":"idle"}}}),
        );
        let mut m = EventMapper::new(SID, false);
        // Idle without a turn is noise.
        assert!(m.map_line(&idle()).is_empty());
        assert!(m.map_line(&status_idle).is_empty());
        m.map_line(&busy());
        let first = m.map_line(&status_idle);
        assert!(matches!(
            first.as_slice(),
            [HarnessEvent::TurnFinished {
                is_error: false,
                ..
            }]
        ));
        assert!(m.map_line(&idle()).is_empty());
        assert!(m.map_line(&status_idle).is_empty());
        // And a second turn works again.
        m.map_line(&busy());
        assert!(matches!(
            m.map_line(&idle()).as_slice(),
            [HarnessEvent::TurnFinished { .. }]
        ));
    }

    #[test]
    fn session_error_without_session_id_belongs_to_the_active_turn() {
        let err = ev(json!({"type":"session.error","properties":{
            "error":{"name":"APIError","data":{"message":"boom","statusCode":429}}}}));
        let mut m = EventMapper::new(SID, false);
        assert!(m.map_line(&err).is_empty(), "no turn: not ours");
        m.map_line(&busy());
        let out = m.map_line(&err);
        assert!(matches!(
            out.as_slice(),
            [HarnessEvent::TurnFinished {
                is_error: true,
                error_kind: Some(ErrorKind::RateLimit),
                ..
            }]
        ));
        // Another session's error is never ours.
        m.map_line(&busy());
        let other = ev(
            json!({"type":"session.error","properties":{"sessionID":"ses_x",
            "error":{"name":"APIError","data":{"message":"x"}}}}),
        );
        assert!(m.map_line(&other).is_empty());
    }

    #[test]
    fn framer_handles_arbitrary_chunk_boundaries_and_utf8() {
        let line = "data: {\"t\":\"h\u{e9}llo \u{1f600}\"}\n\n: ping\n\ndata: {\"n\":2}\r\n\r\n";
        let whole: Vec<Frame> = SseFramer::new(1024).feed(line.as_bytes());
        assert_eq!(whole.len(), 3);
        assert_eq!(
            whole[0].data.as_deref(),
            Some("{\"t\":\"h\u{e9}llo \u{1f600}\"}")
        );
        assert_eq!(whole[1].data, None);
        assert_eq!(whole[1].lines, [": ping"]);
        assert_eq!(whole[2].data.as_deref(), Some("{\"n\":2}"));
        // Every split point, including inside the two- and four-byte characters.
        for cut in 0..line.len() {
            let mut f = SseFramer::new(1024);
            let mut got = f.feed(&line.as_bytes()[..cut]);
            got.extend(f.feed(&line.as_bytes()[cut..]));
            assert_eq!(got, whole, "cut at {cut}");
        }
        // One byte at a time.
        let mut f = SseFramer::new(1024);
        let got: Vec<Frame> = line.as_bytes().iter().flat_map(|b| f.feed(&[*b])).collect();
        assert_eq!(got, whole);
    }

    #[test]
    fn framer_joins_multiline_data_and_maps_it() {
        let mut f = SseFramer::new(1024);
        let frames = f.feed(
            format!(
                "data: {{\"type\":\"session.status\",\ndata:  \"properties\":{{\"sessionID\":\"{SID}\",\ndata: \"status\":{{\"type\":\"busy\"}}}}}}\n\n"
            )
            .as_bytes(),
        );
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].lines.len(), 3);
        let mut m = EventMapper::new(SID, false);
        assert_eq!(
            m.map_line(frames[0].data.as_deref().unwrap()),
            vec![HarnessEvent::TurnStarted]
        );
    }

    #[test]
    fn framer_flags_oversized_events() {
        let mut f = SseFramer::new(16);
        // One long line.
        let frames = f.feed(format!("data: {}\n\n", "x".repeat(100)).as_bytes());
        assert_eq!(frames.len(), 1);
        assert!(frames[0].truncated);
        // Many short lines adding up.
        let many: String = (0..10).map(|_| "data: abcdef\n").collect::<String>() + "\n";
        let frames = f.feed(many.as_bytes());
        assert!(frames[0].truncated);
        // The next event is fine again.
        let frames = f.feed(b"data: ok\n\n");
        assert!(!frames[0].truncated && frames[0].data.as_deref() == Some("ok"));
        // A turn open when an oversized event arrives is failed.
        let mut m = EventMapper::new(SID, false);
        m.map_line(&busy());
        assert!(matches!(
            m.on_stream_end("opencode event exceeded 4 MiB").as_slice(),
            [HarnessEvent::TurnFinished { is_error: true, .. }]
        ));
    }
}
