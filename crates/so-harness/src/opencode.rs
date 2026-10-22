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
    LineParser, MAX_LINE_BYTES, ProcessHandle, create_private_dir, record, spawn_supervised,
    write_private,
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
    json!({
        "$schema": "https://opencode.ai/config.json",
        "mcp": {"superorchestrator": {
            "type": "local",
            "command": command,
            "environment": env,
            "enabled": true,
        }},
        "permission": {"*": "allow"},
    })
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

fn http_error(method: &str, path: &str, status: reqwest::StatusCode, body: &str) -> DriverError {
    DriverError::Other(format!(
        "opencode {method} {path}: HTTP {status} {}",
        truncate(body.trim(), 300)
    ))
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

    async fn wait_ready(&self) -> Result<(), DriverError> {
        let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
        loop {
            if self.healthy().await? {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(DriverError::Other(
                    "opencode server did not become healthy within 20 s".into(),
                ));
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
        _ => classify_common(&text),
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
        if sid != Some(self.session_id.as_str()) {
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
            "session.idle" if self.active => self.finish(false, None, None, &mut out),
            _ => {}
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

/// Reads `resp` as server-sent events until the stream ends or `stop` fires.
async fn sse_loop(
    mut resp: reqwest::Response,
    out: Arc<Out>,
    mut mapper: EventMapper,
    mut stop: watch::Receiver<()>,
) {
    let mut buf: Vec<u8> = Vec::new();
    let mut discarding = false;
    let reason = loop {
        let chunk = tokio::select! {
            _ = stop.changed() => return,
            c = resp.chunk() => c,
        };
        let bytes = match chunk {
            Ok(Some(b)) => b,
            Ok(None) => break "opencode event stream closed".to_string(),
            Err(e) => break format!("opencode event stream failed: {e}"),
        };
        for &b in bytes.iter() {
            if b != b'\n' {
                if buf.len() < MAX_LINE_BYTES {
                    buf.push(b);
                } else {
                    discarding = true;
                }
                continue;
            }
            let mut line = String::from_utf8_lossy(&buf).into_owned();
            buf.clear();
            if line.ends_with('\r') {
                line.pop();
            }
            if std::mem::take(&mut discarding) {
                line.push_str(crate::process::TRUNCATED_MARK);
                out.emit(
                    Dir::In,
                    Channel::Sse,
                    &line,
                    vec![crate::process::default_truncated_event()],
                );
                continue;
            }
            if line.is_empty() {
                continue;
            }
            let events = match line.strip_prefix("data:") {
                Some(d) => mapper.map_line(d.strip_prefix(' ').unwrap_or(d)),
                None => vec![],
            };
            out.emit(Dir::In, Channel::Sse, &line, events);
        }
    };
    for e in mapper.on_stream_end(&reason) {
        out.event(e);
    }
    out.event(HarnessEvent::Error {
        kind: ErrorKind::Other,
        message: reason,
    });
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
    model: Option<(String, String)>,
    variant: Option<String>,
    system: Option<String>,
    format: Option<Value>,
}

struct Inner {
    api: Api,
    session_id: String,
    defaults: PromptDefaults,
    stop: watch::Sender<()>,
    sse: Arc<Mutex<Option<JoinHandle<()>>>>,
    proc: Option<ProcessHandle>,
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
            model: spec.model.as_deref().map(split_model).transpose()?,
            variant: spec.effort.clone(),
            system: Some(spec.system_addendum.clone()).filter(|s| !s.is_empty()),
            format: spec.output_schema.as_ref().map(|schema| {
                json!({"type": "json_schema", "schema": schema, "retryCount": STRUCTURED_RETRIES})
            }),
        };
        let (tx, events) = mpsc::unbounded_channel();

        // Server: spawned (own transcript via the process pipeline) or given.
        let (proc, forward, out, base, password) = match &self.server {
            ServerMode::Spawn => {
                let config_path = spec.run_dir.join("opencode.json");
                write_private(
                    &config_path,
                    &serde_json::to_vec_pretty(&run_config(&spec)).map_err(std::io::Error::from)?,
                )?;
                let password = random_password()?;
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
                    u = tokio::time::timeout(READY_TIMEOUT, url_rx) => match u {
                        Ok(Ok(u)) => Ok(u),
                        Ok(Err(_)) => Err("opencode serve exited before printing its URL".to_string()),
                        Err(_) => Err("opencode serve did not print its URL within 20 s".to_string()),
                    },
                    _ = exit.wait_for(|v| v.is_some()) => {
                        Err("opencode serve exited before printing its URL".to_string())
                    }
                };
                match url {
                    Ok(url) => (Some(sup.handle), Some(sup.events), out, url, password),
                    Err(msg) => {
                        sup.handle.shutdown(Duration::ZERO).await;
                        return Err(DriverError::Other(msg));
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

        let sse_slot: Arc<Mutex<Option<JoinHandle<()>>>> = Arc::new(Mutex::new(None));
        if let Some(mut sup_events) = forward {
            let out = out.clone();
            let slot = sse_slot.clone();
            tokio::spawn(async move {
                while let Some(ev) = sup_events.recv().await {
                    if let HarnessEvent::Exited { .. } = ev {
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

        let (stop, stop_rx) = watch::channel(());
        let started = self
            .start_session(&spec, &api, &out, stop_rx, &sse_slot)
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
        classify_common(text)
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
        stop_rx: watch::Receiver<()>,
        sse_slot: &Arc<Mutex<Option<JoinHandle<()>>>>,
    ) -> Result<String, DriverError> {
        api.wait_ready().await?;
        let resp = api
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
        out.emit(Dir::Out, Channel::Http, "GET /event", vec![]);

        let session_id = match &spec.resume {
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
        let handle = tokio::spawn(sse_loop(resp, out.clone(), mapper, stop_rx));
        if let Ok(mut slot) = sse_slot.lock() {
            *slot = Some(handle);
        }
        out.event(HarnessEvent::SessionStarted {
            native_session_id: session_id.clone(),
        });
        Ok(session_id)
    }
}
