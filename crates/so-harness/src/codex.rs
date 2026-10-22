//! Codex driver (ADR-0004 "codex"): `codex app-server` listening on a loopback
//! WebSocket, spoken to with JSON-RPC (no `jsonrpc` field).
//!
//! Launch: pick a free port, write a random bearer token to
//! `<run_dir>/ws-token` (0600), start
//! `<launch> codex app-server --listen ws://127.0.0.1:<port> --ws-auth
//! capability-token --ws-token-file <run_dir>/ws-token`, poll `GET /readyz`,
//! connect with `Authorization: Bearer <token>`, then `initialize`,
//! `thread/start` (or `thread/resume`) and `turn/start`.
//!
//! The server is started on `ws://127.0.0.1:0`; its banner line
//! `listening on: ws://127.0.0.1:<port>` gives the real port, and the host must
//! be 127.0.0.1 before the token is sent anywhere. The token only ever lives in
//! the token file (deleted on shutdown), in the WebSocket upgrade header, and
//! in the join command environment, never in argv, the transcript or `Debug`
//! output.
//!
//! The superorchestrator MCP server and the system addendum travel in the
//! `thread/start` params (`config.mcp_servers.superorchestrator` and
//! `developerInstructions`), not in files. Their env values are redacted in
//! the transcript.

use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Map, Value, json};
use so_core::{ErrorKind, Harness};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::Message;

use crate::driver::{
    Channel, Driver, DriverConfig, DriverError, HarnessEvent, JoinCommand, JoinInfo, LaunchSpec,
    RateLimits, RunControl, RunHandle, ShutdownOutcome, classify_common,
};
use crate::process::{
    EventSink, ExitWatch, LineParser, ProcessHandle, ProducerGuard, Supervised, create_private_dir,
    record_with, spawn_supervised_with, startup_failure, write_private,
};
use crate::transcript::{Dir, SharedTranscript};

/// How long the server gets to print its listening banner.
const BANNER_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the server gets to answer `/readyz`.
const READY_TIMEOUT: Duration = Duration::from_secs(15);
/// Time allowed for the WebSocket upgrade.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time allowed for an ordinary JSON-RPC response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `interrupt` waits for the `turn/interrupt` response.
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(5);
/// `app-server` does not exit when its client leaves, so a clean exit is only
/// waited for briefly before the process group is terminated.
const CLEAN_EXIT_WAIT: Duration = Duration::from_millis(500);
/// Name of the env variable that carries the token to `codex resume --remote`.
pub const WS_TOKEN_ENV: &str = "SO_WS_TOKEN";

/// Driver for `codex`.
#[derive(Debug, Clone, Default)]
pub struct CodexDriver {
    config: DriverConfig,
}

impl CodexDriver {
    pub fn new(config: DriverConfig) -> Self {
        Self { config }
    }
}

/// Harness arguments (after `program argv_prefix extra_args`).
pub fn build_args(token_file: &Path) -> Vec<String> {
    vec![
        "app-server".into(),
        "--listen".into(),
        "ws://127.0.0.1:0".into(),
        "--ws-auth".into(),
        "capability-token".into(),
        "--ws-token-file".into(),
        token_file.display().to_string(),
    ]
}

fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `thread/start` or `thread/resume` params. The sandbox and approval policy
/// give the worker full permissions without prompts (SPEC 4).
pub fn thread_params(spec: &LaunchSpec) -> Value {
    let mut config = Map::new();
    if let Some(e) = &spec.effort {
        config.insert("model_reasoning_effort".into(), json!(e));
    }
    let env: Map<String, Value> = spec
        .mcp
        .env
        .iter()
        .map(|(k, v)| (k.clone(), Value::String(v.clone())))
        .collect();
    config.insert(
        "mcp_servers".into(),
        json!({"superorchestrator": {
            "command": spec.mcp.command,
            "args": spec.mcp.args,
            "env": env,
        }}),
    );
    let mut p = Map::new();
    if let Some(id) = &spec.resume {
        p.insert("threadId".into(), json!(id));
    }
    p.insert("cwd".into(), json!(spec.cwd));
    if let Some(m) = &spec.model {
        p.insert("model".into(), json!(m));
    }
    p.insert("sandbox".into(), json!("danger-full-access"));
    p.insert("approvalPolicy".into(), json!("never"));
    if !spec.system_addendum.is_empty() {
        p.insert("developerInstructions".into(), json!(spec.system_addendum));
    }
    let mut config = Value::Object(config);
    spec.agent
        .merge_into(&mut config, ("mcp_servers", "superorchestrator"));
    p.insert("config".into(), config);
    Value::Object(p)
}

impl CodexDriver {
    /// Stops the server that never became ready and explains why from its stderr.
    async fn startup_error(
        &self,
        control: &CodexControl,
        events: &mut mpsc::UnboundedReceiver<HarnessEvent>,
        cause: DriverError,
    ) -> DriverError {
        let _ = control.shutdown(Duration::ZERO).await;
        startup_failure(cause, events, |t| self.classify_error(t)).await
    }
}

impl Driver for CodexDriver {
    fn harness(&self) -> Harness {
        Harness::Codex
    }

    async fn launch(&self, spec: LaunchSpec) -> Result<RunHandle, DriverError> {
        create_private_dir(&spec.run_dir)?;
        let token = random_token()?;
        let token_file = spec.run_dir.join("ws-token");
        write_private(&token_file, token.as_bytes())?;

        let mut cmd = Command::new(&spec.launch.program);
        cmd.args(&spec.launch.argv_prefix)
            .args(&spec.launch.extra_args)
            .args(build_args(&token_file))
            .envs(spec.launch.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd);
        let (banner_tx, banner_rx) = oneshot::channel();
        let banner = Banner(Arc::new(Mutex::new(Some(banner_tx))));
        let sup = spawn_supervised_with(
            cmd,
            &spec.run_dir,
            self.config.shutdown_grace,
            banner.clone(),
            Some(Box::new(banner)),
        )?;
        let guard = sup.producer_guard();
        let Supervised {
            mut events,
            handle,
            transcript,
            events_sink,
            ..
        } = sup;

        let shared = Arc::new(Mutex::new(Shared::default()));
        let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
        let control = CodexControl {
            inner: Arc::new(Inner {
                proc: handle,
                cmd: cmd_tx,
                shared: shared.clone(),
                effort: spec.effort.clone(),
                output_schema: spec.output_schema.clone(),
                send_lock: tokio::sync::Mutex::new(()),
                token_file,
            }),
        };

        let conn = ConnParts {
            transcript,
            events: events_sink,
            cmd_rx,
            guard,
        };
        let exit = control.inner.proc.exit.clone();
        // A server that dies or stalls before it is ready says why on its stderr (not signed
        // in, out of quota): report that, classified, so the run can fail over. A banner
        // that names another host is a different failure and stays as it is.
        let addr = match wait_banner(banner_rx, &exit).await {
            Ok(addr) => addr,
            Err(e) => return Err(self.startup_error(&control, &mut events, e).await),
        };
        let port = match loopback_port(&addr) {
            Ok(port) => port,
            Err(e) => {
                let _ = control.shutdown(Duration::ZERO).await;
                return Err(e);
            }
        };
        if let Err(e) = wait_ready(port, &exit).await {
            return Err(self.startup_error(&control, &mut events, e).await);
        }
        match start(&spec, &control, &token, port, conn).await {
            Ok((thread_id, port)) => Ok(RunHandle {
                events,
                control: RunControl::Codex(control),
                native_session_id: Some(thread_id.clone()),
                join: JoinInfo {
                    account_name: spec.account.name.clone(),
                    native_session_id: thread_id,
                    endpoint: Some(crate::driver::Endpoint {
                        url: format!("ws://127.0.0.1:{port}"),
                        secret_env: Some((WS_TOKEN_ENV.into(), token)),
                        directory: None,
                    }),
                    cwd: spec.cwd.clone(),
                },
            }),
            Err(e) => {
                let _ = control.shutdown(Duration::ZERO).await;
                Err(e)
            }
        }
    }

    /// Live: `<account> resume --remote <ws url> --remote-auth-token-env
    /// SO_WS_TOKEN <thread>` with the token in the environment. After the run
    /// (no endpoint): `<account> resume <thread>`.
    fn join_command(&self, j: &JoinInfo) -> JoinCommand {
        let mut argv = vec![j.account_name.clone(), "resume".into()];
        let mut env = Vec::new();
        if let Some(ep) = &j.endpoint {
            argv.extend(["--remote".into(), ep.url.clone()]);
            if let Some((var, value)) = &ep.secret_env {
                argv.extend(["--remote-auth-token-env".into(), var.clone()]);
                env.push((var.clone(), value.clone()));
            }
        }
        argv.push(j.native_session_id.clone());
        JoinCommand {
            argv,
            env,
            cwd: j.cwd.clone(),
        }
    }

    fn classify_error(&self, text: &str) -> ErrorKind {
        classify_common(text)
    }
}

/// Everything after the server is ready: WebSocket, handshake, thread and first turn.
/// Returns the thread id.
async fn start(
    spec: &LaunchSpec,
    control: &CodexControl,
    token: &str,
    port: u16,
    conn: ConnParts,
) -> Result<(String, u16), DriverError> {
    let exit = control.inner.proc.exit.clone();

    let mut req = format!("ws://127.0.0.1:{port}")
        .into_client_request()
        .map_err(|e| DriverError::Other(format!("bad websocket request: {e}")))?;
    let bearer = format!("Bearer {token}")
        .parse()
        .map_err(|_| DriverError::Other("token is not a valid header value".into()))?;
    req.headers_mut().insert("Authorization", bearer);
    let ws =
        match tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async(req)).await {
            Ok(Ok((ws, _))) => ws,
            Ok(Err(e)) => {
                return Err(DriverError::Other(format!(
                    "codex websocket connect failed: {e}"
                )));
            }
            Err(_) => {
                return Err(DriverError::Other(
                    "codex websocket connect timed out".into(),
                ));
            }
        };
    let (sink, stream) = ws.split();
    tokio::spawn(
        Conn {
            sink,
            stream,
            cmd_rx: conn.cmd_rx,
            pending: HashMap::new(),
            next_id: 0,
            parser: CodexParser::new(control.inner.shared.clone(), spec.output_schema.is_some()),
            transcript: conn.transcript,
            events: conn.events,
            exit,
            _guard: conn.guard,
        }
        .run(),
    );

    control
        .request(
            "initialize",
            json!({"clientInfo": {
                "name": "superorchestrator",
                "title": null,
                "version": env!("CARGO_PKG_VERSION"),
            }}),
            Kind::Plain,
            REQUEST_TIMEOUT,
        )
        .await?;
    let _ = control.inner.cmd.send(Cmd::Notify {
        method: "initialized",
    });

    let method = if spec.resume.is_some() && spec.fork {
        "thread/fork"
    } else if spec.resume.is_some() {
        "thread/resume"
    } else {
        "thread/start"
    };
    let res = control
        .request(method, thread_params(spec), Kind::Thread, REQUEST_TIMEOUT)
        .await?;
    let thread_id = res["thread"]["id"]
        .as_str()
        .ok_or_else(|| DriverError::Other(format!("{method} response has no thread id")))?
        .to_string();
    control.send(&spec.prompt).await?;
    Ok((thread_id, port))
}

struct ConnParts {
    transcript: SharedTranscript,
    events: EventSink,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    guard: ProducerGuard,
}

/// Waits for the server's `listening on:` banner.
async fn wait_banner(
    rx: oneshot::Receiver<String>,
    exit: &ExitWatch,
) -> Result<String, DriverError> {
    let mut exit = exit.clone();
    tokio::select! {
        r = rx => r.map_err(|_| DriverError::Other(
            "codex app-server ended its output before it printed a listening address".into())),
        _ = exit.wait_for(|v| v.is_some()) => Err(DriverError::Other(
            "codex app-server exited before it printed a listening address".into())),
        _ = tokio::time::sleep(BANNER_TIMEOUT) => Err(DriverError::Other(format!(
            "codex app-server printed no listening address within {} s", BANNER_TIMEOUT.as_secs()))),
    }
}

/// The port of a `ws://127.0.0.1:<port>` address. Any other host is refused,
/// so the token is never sent to a non-loopback address.
fn loopback_port(addr: &str) -> Result<u16, DriverError> {
    addr.strip_prefix("ws://127.0.0.1:")
        .and_then(|p| p.trim().parse::<u16>().ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| {
            DriverError::Other(format!(
                "codex app-server listens on {addr:?}, not ws://127.0.0.1:<port>; refusing to send the token"
            ))
        })
}

/// Finds the listening address in the server's log lines (either stream).
#[derive(Clone)]
struct Banner(Arc<Mutex<Option<oneshot::Sender<String>>>>);

impl LineParser for Banner {
    fn parse_line(&mut self, line: &str) -> Vec<HarnessEvent> {
        if let Some((_, addr)) = line.split_once("listening on: ")
            && let Some(tx) = self.0.lock().unwrap_or_else(|p| p.into_inner()).take()
        {
            let _ = tx.send(addr.trim().to_string());
        }
        vec![]
    }
}

/// Polls `GET /readyz` until 200, the process exits or the deadline passes.
async fn wait_ready(port: u16, exit: &ExitWatch) -> Result<(), DriverError> {
    let deadline = tokio::time::Instant::now() + READY_TIMEOUT;
    loop {
        if let Some(code) = *exit.borrow() {
            return Err(DriverError::Other(format!(
                "codex app-server exited before it was ready (code {code:?})"
            )));
        }
        if readyz(port).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(DriverError::Other(format!(
                "codex app-server not ready on port {port} after {} s",
                READY_TIMEOUT.as_secs()
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn readyz(port: u16) -> bool {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)).await else {
        return false;
    };
    let req =
        format!("GET /readyz HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n");
    if s.write_all(req.as_bytes()).await.is_err() {
        return false;
    }
    let mut buf = Vec::new();
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        (&mut s).take(8192).read_to_end(&mut buf),
    )
    .await;
    buf.starts_with(b"HTTP/1.1 200") || buf.starts_with(b"HTTP/1.0 200")
}

/// How a response is folded into parser state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Plain,
    /// `thread/start` or `thread/resume`.
    Thread,
    /// `turn/start`.
    Turn,
}

#[derive(Debug)]
struct RpcError {
    code: i64,
    message: String,
}

type Reply = oneshot::Sender<Result<Value, RpcError>>;

enum Cmd {
    Request {
        method: &'static str,
        params: Value,
        kind: Kind,
        reply: Reply,
    },
    Notify {
        method: &'static str,
    },
    Close(oneshot::Sender<()>),
}

/// Run state shared between the connection task (writer) and the control.
#[derive(Debug, Default)]
struct Shared {
    thread_id: Option<String>,
    turn_id: Option<String>,
    /// A turn is running (between its start and its completion or failure).
    active: bool,
    /// Last turn that ended; a late `turn/start` response must not reopen it.
    last_finished: Option<String>,
}

impl Shared {
    fn begin(&mut self, turn_id: &str) {
        if self.last_finished.as_deref() != Some(turn_id) {
            self.turn_id = Some(turn_id.to_string());
            self.active = true;
        }
    }

    fn finish(&mut self, turn_id: Option<&str>) {
        self.active = false;
        if let Some(id) = turn_id.or(self.turn_id.as_deref()) {
            self.last_finished = Some(id.to_string());
        }
    }
}

#[derive(Debug)]
struct Inner {
    proc: ProcessHandle,
    cmd: mpsc::UnboundedSender<Cmd>,
    shared: Arc<Mutex<Shared>>,
    effort: Option<String>,
    output_schema: Option<Value>,
    /// Serialises `send`, so a second concurrent send sees the turn the first
    /// one started and steers it instead of starting another.
    send_lock: tokio::sync::Mutex<()>,
    token_file: std::path::PathBuf,
}

/// Clonable control handle for a running codex app-server. Dropping the last
/// clone kills the process group.
#[derive(Debug, Clone)]
pub struct CodexControl {
    inner: Arc<Inner>,
}

impl CodexControl {
    async fn request(
        &self,
        method: &'static str,
        params: Value,
        kind: Kind,
        timeout: Duration,
    ) -> Result<Value, DriverError> {
        let (reply, rx) = oneshot::channel();
        self.inner
            .cmd
            .send(Cmd::Request {
                method,
                params,
                kind,
                reply,
            })
            .map_err(|_| DriverError::Closed)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(DriverError::Other(format!(
                "codex {method} failed: {} (code {})",
                e.message, e.code
            ))),
            Ok(Err(_)) => Err(DriverError::Closed),
            Err(_) => Err(DriverError::Other(format!(
                "codex {method} timed out after {} s",
                timeout.as_secs()
            ))),
        }
    }

    fn snapshot(&self) -> (Option<String>, Option<String>) {
        let s = self.inner.shared.lock().unwrap_or_else(|p| p.into_inner());
        (
            s.thread_id.clone(),
            s.active.then(|| s.turn_id.clone()).flatten(),
        )
    }

    /// `turn/start` when idle, `turn/steer` into the running turn otherwise.
    pub async fn send(&self, text: &str) -> Result<(), DriverError> {
        let _serial = self.inner.send_lock.lock().await;
        let (thread, turn) = self.snapshot();
        let thread = thread.ok_or(DriverError::Closed)?;
        let input = json!([{"type": "text", "text": text}]);
        if let Some(turn) = turn {
            let steer = self
                .request(
                    "turn/steer",
                    json!({"threadId": thread, "expectedTurnId": turn, "input": input}),
                    Kind::Plain,
                    REQUEST_TIMEOUT,
                )
                .await;
            if steer.is_ok() {
                return Ok(());
            }
            // Rejected (the turn may have just ended): start a new turn.
        }
        let mut params = json!({"threadId": thread, "input": input});
        if let Some(e) = &self.inner.effort {
            params["effort"] = json!(e);
        }
        if let Some(s) = &self.inner.output_schema {
            params["outputSchema"] = s.clone();
        }
        self.request("turn/start", params, Kind::Turn, REQUEST_TIMEOUT)
            .await
            .map(|_| ())
    }

    /// `turn/interrupt` for the running turn and waits (5 s) for the response.
    /// The turn then ends with `TurnFinished`; the run stays alive. A no-op
    /// when no turn is running.
    pub async fn interrupt(&self) -> Result<(), DriverError> {
        let (thread, turn) = self.snapshot();
        let (Some(thread), Some(turn)) = (thread, turn) else {
            return Ok(());
        };
        self.request(
            "turn/interrupt",
            json!({"threadId": thread, "turnId": turn}),
            Kind::Plain,
            INTERRUPT_TIMEOUT,
        )
        .await
        .map(|_| ())
    }

    /// Closes the WebSocket, deletes the token file and ends the process:
    /// `Exited` if SIGTERM sufficed, `Killed` only after SIGKILL. app-server
    /// never exits by itself, so the wait before SIGTERM is capped at 500 ms.
    pub async fn shutdown(&self, deadline: Duration) -> Result<ShutdownOutcome, DriverError> {
        let (tx, rx) = oneshot::channel();
        if self.inner.cmd.send(Cmd::Close(tx)).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(1), rx).await;
        }
        let _ = std::fs::remove_file(&self.inner.token_file);
        Ok(self
            .inner
            .proc
            .shutdown_by_signal(deadline.min(CLEAN_EXIT_WAIT))
            .await)
    }

    pub fn pid(&self) -> Option<u32> {
        self.inner.proc.pid
    }
}

type WsStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Owns the WebSocket: writes requests, matches responses, turns
/// notifications into events. Ends when the socket closes, the process exits
/// or every control handle is gone, and drops its event sender then so the
/// event channel can close.
struct Conn {
    sink: futures_util::stream::SplitSink<WsStream, Message>,
    stream: futures_util::stream::SplitStream<WsStream>,
    cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    pending: HashMap<u64, (Kind, Reply)>,
    next_id: u64,
    parser: CodexParser,
    transcript: SharedTranscript,
    events: EventSink,
    exit: ExitWatch,
    /// Holds back `Exited` until this task is done with the event stream.
    _guard: ProducerGuard,
}

impl Conn {
    async fn run(mut self) {
        let mut closing = false;
        let mut process_exited = false;
        let mut exit = self.exit.clone();
        loop {
            tokio::select! {
                cmd = self.cmd_rx.recv() => match cmd {
                    None => { closing = true; break }
                    Some(Cmd::Close(done)) => {
                        closing = true;
                        let _ = self.sink.close().await;
                        let _ = done.send(());
                        break;
                    }
                    Some(Cmd::Notify { method }) => {
                        self.write(json!({"method": method})).await;
                    }
                    Some(Cmd::Request { method, params, kind, reply }) => {
                        self.next_id += 1;
                        let id = self.next_id;
                        self.pending.insert(id, (kind, reply));
                        self.write(json!({"method": method, "id": id, "params": params})).await;
                    }
                },
                msg = self.stream.next() => match msg {
                    Some(Ok(Message::Text(t))) => self.incoming(t.as_str()).await,
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let _ = self.events.send(HarnessEvent::Error {
                            kind: ErrorKind::Other,
                            message: format!("codex websocket error: {e}"),
                        });
                        break;
                    }
                },
                _ = exit.changed() => { process_exited = true; break }
            }
        }
        // Requests still waiting get `Closed` (their senders drop here).
        self.pending.clear();
        let why = if process_exited {
            Some("harness exited")
        } else if !closing {
            Some("connection to codex app-server lost")
        } else {
            None
        };
        if let Some(why) = why {
            for e in self.parser.fail_open_turn(why) {
                self.events.send(e);
            }
        }
    }

    /// Sends one frame, recording it first (secrets redacted).
    async fn write(&mut self, frame: Value) {
        let shown = redact_frame(&frame).to_string();
        let offset = record_with(&self.transcript, Dir::Out, Channel::Ws, &shown, &[], |e| {
            self.events.send(e);
        });
        let _ = self.events.send(HarnessEvent::Raw {
            channel: Channel::Ws,
            line: shown,
            offset,
        });
        if let Err(e) = self
            .sink
            .send(Message::Text(frame.to_string().into()))
            .await
        {
            let _ = self.events.send(HarnessEvent::Error {
                kind: ErrorKind::Other,
                message: format!("codex websocket write failed: {e}"),
            });
        }
    }

    async fn incoming(&mut self, text: &str) {
        let v: Option<Value> = serde_json::from_str(text).ok();
        let mut events = Vec::new();
        let mut reply = None;
        if let Some(v) = &v {
            let method = v["method"].as_str();
            match (method, v.get("id")) {
                (Some(m), Some(id)) => {
                    events.push(HarnessEvent::Error {
                        kind: ErrorKind::Other,
                        message: format!("unsupported server request {m}"),
                    });
                    reply = Some(json!({
                        "id": id,
                        "error": {"code": -32601, "message": format!("method not found: {m}")},
                    }));
                }
                (Some(_), None) => events = self.parser.notification(v),
                (None, Some(id)) => {
                    if let Some((kind, tx)) = id.as_u64().and_then(|n| self.pending.remove(&n)) {
                        let res = match v.get("error") {
                            Some(e) => Err(RpcError {
                                code: e["code"].as_i64().unwrap_or(0),
                                message: e["message"].as_str().unwrap_or("unknown error").into(),
                            }),
                            None => Ok(v["result"].clone()),
                        };
                        if let Ok(r) = &res {
                            events = self.parser.response(kind, r);
                        }
                        let _ = tx.send(res);
                    }
                }
                (None, None) => {}
            }
        }
        let offset = record_with(&self.transcript, Dir::In, Channel::Ws, text, &events, |e| {
            self.events.send(e);
        });
        let _ = self.events.send(HarnessEvent::Raw {
            channel: Channel::Ws,
            line: text.to_string(),
            offset,
        });
        for e in events {
            let _ = self.events.send(e);
        }
        if let Some(r) = reply {
            self.write(r).await;
        }
    }
}

/// A copy of `frame` safe for the transcript: MCP env values (which can hold
/// secrets) are replaced.
fn redact_frame(frame: &Value) -> Value {
    let mut f = frame.clone();
    if let Some(servers) = f
        .pointer_mut("/params/config/mcp_servers")
        .and_then(Value::as_object_mut)
    {
        for server in servers.values_mut() {
            if let Some(env) = server.get_mut("env").and_then(Value::as_object_mut) {
                for v in env.values_mut() {
                    *v = json!("<redacted>");
                }
            }
        }
    }
    f
}

/// `codexErrorInfo` (string or `{variant: {httpStatusCode}}`) plus the message
/// to an [`ErrorKind`] (ADR-0004 cheat sheet); falls back to text patterns.
pub fn classify_info(info: &Value, message: &str) -> ErrorKind {
    let by_status = |code: Option<u64>| match code {
        Some(401) => Some(ErrorKind::Auth),
        Some(429) => Some(ErrorKind::RateLimit),
        _ => None,
    };
    let known = match info {
        Value::String(s) => match s.as_str() {
            "usageLimitExceeded" | "sessionBudgetExceeded" => Some(ErrorKind::Quota),
            "rateLimitExceeded" | "serverOverloaded" => Some(ErrorKind::RateLimit),
            "unauthorized" => Some(ErrorKind::Auth),
            _ => None,
        },
        Value::Object(o) => o
            .values()
            .find_map(|v| by_status(v["httpStatusCode"].as_u64())),
        _ => None,
    };
    known.unwrap_or_else(|| classify_common(message))
}

/// Codex error messages are often a JSON document
/// (`{"error":{"message": ...}}`); show the inner message.
fn readable_message(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(String::from))
        .unwrap_or_else(|| raw.to_string())
}

/// `None` when no window of a known length (5 h, 7 d) is present.
fn rate_limits(snapshot: &Value) -> Option<RateLimits> {
    let mut out = RateLimits::default();
    let mut any = false;
    for w in [&snapshot["primary"], &snapshot["secondary"]] {
        let used = w["usedPercent"].as_f64();
        let resets = w["resetsAt"].as_i64();
        match w["windowDurationMins"].as_i64() {
            Some(300) => {
                any = true;
                out.five_hour_used_pct = used;
                out.five_hour_resets_at = resets;
            }
            Some(10080) => {
                any = true;
                out.weekly_used_pct = used;
                out.weekly_resets_at = resets;
            }
            _ => {}
        }
    }
    any.then_some(out)
}

/// Display name for an item that is a tool call, `None` for messages,
/// reasoning and the like.
fn tool_name(item: &Value) -> Option<String> {
    let ty = item["type"].as_str()?;
    match ty {
        "commandExecution" | "fileChange" | "webSearch" | "imageGeneration" => Some(ty.into()),
        "mcpToolCall" => Some(format!(
            "{}/{}",
            item["server"].as_str().unwrap_or("mcp"),
            item["tool"].as_str().unwrap_or("unknown")
        )),
        "dynamicToolCall" | "collabAgentToolCall" => {
            Some(item["tool"].as_str().unwrap_or(ty).to_string())
        }
        _ => None,
    }
}

fn item_ok(item: &Value) -> bool {
    item["status"].as_str() == Some("completed") && item["success"].as_bool() != Some(false)
}

/// Turns server notifications and responses into [`HarnessEvent`]s.
#[derive(Debug)]
pub struct CodexParser {
    shared: Arc<Mutex<Shared>>,
    expects_structured: bool,
    session_started: bool,
    /// `agentMessage` text collected from deltas, by item id.
    deltas: HashMap<String, String>,
    final_text: Option<String>,
    last_text: Option<String>,
    /// Turn that already ended through an `error` notification.
    errored_turn: Option<String>,
}

impl CodexParser {
    fn new(shared: Arc<Mutex<Shared>>, expects_structured: bool) -> Self {
        Self {
            shared,
            expects_structured,
            session_started: false,
            deltas: HashMap::new(),
            final_text: None,
            last_text: None,
            errored_turn: None,
        }
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn session(&mut self, thread_id: &str) -> Vec<HarnessEvent> {
        self.shared().thread_id = Some(thread_id.to_string());
        if std::mem::replace(&mut self.session_started, true) {
            return vec![];
        }
        vec![HarnessEvent::SessionStarted {
            native_session_id: thread_id.to_string(),
        }]
    }

    /// Events for a successful response.
    fn response(&mut self, kind: Kind, result: &Value) -> Vec<HarnessEvent> {
        match kind {
            Kind::Thread => result["thread"]["id"]
                .as_str()
                .map(|id| self.session(id))
                .unwrap_or_default(),
            Kind::Turn => {
                if let Some(id) = result["turn"]["id"].as_str() {
                    self.shared().begin(id);
                }
                vec![]
            }
            Kind::Plain => vec![],
        }
    }

    /// Events for one notification (`method` and `params`, no `id`).
    pub fn notification(&mut self, v: &Value) -> Vec<HarnessEvent> {
        let p = &v["params"];
        match v["method"].as_str() {
            Some("thread/started") => p["thread"]["id"]
                .as_str()
                .map(|id| self.session(id))
                .unwrap_or_default(),
            Some("turn/started") => {
                if let Some(id) = p["turn"]["id"].as_str() {
                    self.shared().begin(id);
                }
                self.final_text = None;
                self.last_text = None;
                vec![HarnessEvent::TurnStarted]
            }
            Some("item/agentMessage/delta") => {
                if let (Some(id), Some(d)) = (p["itemId"].as_str(), p["delta"].as_str()) {
                    self.deltas.entry(id.to_string()).or_default().push_str(d);
                }
                vec![]
            }
            Some("item/started") => tool_name(&p["item"])
                .map(|name| vec![HarnessEvent::ToolCallStarted { name }])
                .unwrap_or_default(),
            Some("item/completed") => self.item_completed(&p["item"]),
            Some("thread/tokenUsage/updated") => {
                let last = &p["tokenUsage"]["last"];
                let n = |k: &str| last[k].as_u64().unwrap_or(0);
                // `last` is this model call, so summing Usage events gives the
                // run total. codex input counts include cached tokens.
                vec![HarnessEvent::Usage {
                    input_tokens: n("inputTokens"),
                    output_tokens: n("outputTokens"),
                    cost: None,
                    rate_limits: None,
                }]
            }
            Some("account/rateLimits/updated") => self.rate_limits_updated(&p["rateLimits"]),
            Some("turn/completed") => self.turn_completed(&p["turn"]),
            Some("error") => self.error(p),
            _ => vec![],
        }
    }

    fn item_completed(&mut self, item: &Value) -> Vec<HarnessEvent> {
        if let Some(name) = tool_name(item) {
            return vec![HarnessEvent::ToolCallFinished {
                name,
                ok: item_ok(item),
            }];
        }
        if item["type"] != "agentMessage" {
            return vec![];
        }
        let buffered = item["id"].as_str().and_then(|id| self.deltas.remove(id));
        let text = item["text"]
            .as_str()
            .filter(|t| !t.is_empty())
            .map(String::from)
            .or(buffered)
            .unwrap_or_default();
        if text.is_empty() {
            return vec![];
        }
        if item["phase"] == "final_answer" {
            self.final_text = Some(text.clone());
        }
        self.last_text = Some(text.clone());
        vec![HarnessEvent::AssistantMessage { text }]
    }

    /// A zero-token `Usage` carrying the fresh limits (the token counts come
    /// with `thread/tokenUsage/updated`), plus a quota `Error` when codex says
    /// a limit was reached.
    fn rate_limits_updated(&mut self, snapshot: &Value) -> Vec<HarnessEvent> {
        let mut out: Vec<HarnessEvent> = rate_limits(snapshot)
            .map(|rl| HarnessEvent::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cost: None,
                rate_limits: Some(rl),
            })
            .into_iter()
            .collect();
        if let Some(reached) = snapshot["rateLimitReachedType"].as_str() {
            out.push(HarnessEvent::Error {
                kind: ErrorKind::Quota,
                message: format!("rate limit reached ({reached})"),
            });
        }
        out
    }

    fn turn_completed(&mut self, turn: &Value) -> Vec<HarnessEvent> {
        let id = turn["id"].as_str();
        self.shared().finish(id);
        let final_text = self.final_text.take();
        let last_text = self.last_text.take();
        self.deltas.clear();
        if id.is_some() && self.errored_turn.take().as_deref() == id {
            return vec![];
        }
        let from_items = || {
            let msgs = || turn["items"].as_array().into_iter().flatten();
            let text = |i: &Value| i["text"].as_str().map(String::from);
            msgs()
                .rev()
                .find(|i| i["type"] == "agentMessage" && i["phase"] == "final_answer")
                .and_then(text)
                .or_else(|| {
                    msgs()
                        .rev()
                        .find(|i| i["type"] == "agentMessage")
                        .and_then(text)
                })
        };
        let result_text = final_text.or_else(from_items).or(last_text);
        let status = turn["status"].as_str().unwrap_or("completed");
        let failed = status == "failed";
        let (error_kind, error_text) = if failed {
            let e = &turn["error"];
            let raw = e["message"].as_str().unwrap_or("codex turn failed");
            (
                Some(classify_info(&e["codexErrorInfo"], raw)),
                Some(readable_message(raw)),
            )
        } else {
            (None, None)
        };
        let structured = if self.expects_structured && !failed {
            result_text
                .as_deref()
                .and_then(|t| serde_json::from_str::<Value>(t).ok())
        } else {
            None
        };
        vec![HarnessEvent::TurnFinished {
            result_text,
            structured,
            is_error: failed,
            error_kind,
            error_text,
        }]
    }

    /// A terminal `error` notification fails the running turn (and `codex`
    /// may or may not follow with `turn/completed`; that one is then skipped).
    /// Retried errors (`willRetry`) are only logged.
    fn error(&mut self, p: &Value) -> Vec<HarnessEvent> {
        if p["willRetry"].as_bool() == Some(true) {
            return vec![];
        }
        let e = &p["error"];
        let raw = e["message"].as_str().unwrap_or("codex error");
        let kind = classify_info(&e["codexErrorInfo"], raw);
        let text = readable_message(raw);
        let turn_id = p["turnId"].as_str().map(String::from);
        let mut shared = self.shared();
        if !shared.active {
            return vec![HarnessEvent::Error {
                kind,
                message: text,
            }];
        }
        let current = shared.turn_id.clone();
        shared.finish(turn_id.as_deref());
        drop(shared);
        self.errored_turn = turn_id.or(current);
        self.deltas.clear();
        let partial = self.final_text.take().or_else(|| self.last_text.take());
        vec![HarnessEvent::TurnFinished {
            result_text: partial,
            structured: None,
            is_error: true,
            error_kind: Some(kind),
            error_text: Some(text),
        }]
    }

    /// The socket or the process died: fail the open turn.
    fn fail_open_turn(&mut self, why: &str) -> Vec<HarnessEvent> {
        let mut shared = self.shared();
        if !shared.active {
            return vec![];
        }
        shared.finish(None);
        drop(shared);
        vec![HarnessEvent::TurnFinished {
            result_text: None,
            structured: None,
            is_error: true,
            error_kind: Some(ErrorKind::Other),
            error_text: Some(why.into()),
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/codex/transcript-stdio.jsonl");
    const SCHEMA: &str = include_str!("../tests/fixtures/codex/schema-subset.json");

    fn parser(structured: bool) -> CodexParser {
        CodexParser::new(Arc::new(Mutex::new(Shared::default())), structured)
    }

    fn msgs(dir: &str) -> Vec<Value> {
        FIXTURE
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|v| v["dir"] == dir)
            .map(|v| v["msg"].clone())
            .collect()
    }

    #[test]
    fn parses_real_fixture() {
        let mut p = parser(false);
        let mut events = Vec::new();
        for m in msgs("in") {
            match (m["method"].as_str(), m.get("id")) {
                (Some(_), None) => events.extend(p.notification(&m)),
                (None, Some(_)) if m["result"]["thread"].is_object() => {
                    events.extend(p.response(Kind::Thread, &m["result"]));
                }
                (None, Some(_)) if m["result"]["turn"].is_object() => {
                    events.extend(p.response(Kind::Turn, &m["result"]));
                }
                _ => {}
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
                Usage {
                    rate_limits: None, ..
                } => "usage",
                Usage { .. } => "limits",
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
                "usage",
                "limits",
                "usage",
                "limits",
                "msg",
                "usage",
                "limits",
                "turn_end"
            ]
        );
        assert_eq!(
            events[0],
            SessionStarted {
                native_session_id: "01a162ee-5ba3-7400-be98-4eb69d75fead".into()
            }
        );
        assert_eq!(
            events[2],
            ToolCallStarted {
                name: "fileChange".into()
            }
        );
        assert_eq!(
            events[3],
            ToolCallFinished {
                name: "fileChange".into(),
                ok: true
            }
        );
        assert_eq!(
            events[4],
            Usage {
                input_tokens: 16096,
                output_tokens: 103,
                cost: None,
                rate_limits: None
            }
        );
        let Usage {
            rate_limits: Some(rl),
            ..
        } = &events[5]
        else {
            panic!()
        };
        assert_eq!(rl.five_hour_used_pct, Some(21.0));
        assert_eq!(rl.five_hour_resets_at, Some(1792665377));
        assert_eq!(rl.weekly_used_pct, Some(70.0));
        assert_eq!(rl.weekly_resets_at, Some(1793082987));
        assert_eq!(
            events[8],
            AssistantMessage {
                text: "done".into()
            }
        );
        assert_eq!(
            events[11],
            TurnFinished {
                result_text: Some("done".into()),
                structured: None,
                is_error: false,
                error_kind: None,
                error_text: None
            }
        );
    }

    #[test]
    fn deltas_fill_an_empty_final_item() {
        let mut p = parser(false);
        p.notification(&json!({"method": "item/agentMessage/delta",
            "params": {"itemId": "m1", "delta": "do"}}));
        p.notification(&json!({"method": "item/agentMessage/delta",
            "params": {"itemId": "m1", "delta": "ne"}}));
        let ev = p.notification(&json!({"method": "item/completed",
            "params": {"item": {"type": "agentMessage", "id": "m1", "text": "", "phase": "final_answer"}}}));
        assert_eq!(
            ev,
            [HarnessEvent::AssistantMessage {
                text: "done".into()
            }]
        );
    }

    #[test]
    fn error_classification() {
        use ErrorKind::*;
        let cases = [
            (json!("usageLimitExceeded"), "x", Quota),
            (json!("sessionBudgetExceeded"), "x", Quota),
            (json!("rateLimitExceeded"), "x", RateLimit),
            (json!("serverOverloaded"), "x", RateLimit),
            (json!("unauthorized"), "x", Auth),
            (
                json!({"httpConnectionFailed": {"httpStatusCode": 429}}),
                "x",
                RateLimit,
            ),
            (
                json!({"responseStreamDisconnected": {"httpStatusCode": 401}}),
                "x",
                Auth,
            ),
            (
                json!({"httpConnectionFailed": {"httpStatusCode": 500}}),
                "boom",
                Other,
            ),
            (json!("other"), "You've hit your usage limit", Quota),
            (json!("other"), "model not supported", Other),
            (Value::Null, "Please login", Auth),
        ];
        for (info, text, want) in cases {
            assert_eq!(classify_info(&info, text), want, "{info} {text}");
        }
    }

    #[test]
    fn error_notification_fails_the_turn_once() {
        let mut p = parser(false);
        p.notification(&json!({"method": "turn/started", "params": {"turn": {"id": "t1"}}}));
        let retry = p.notification(&json!({"method": "error", "params": {
            "error": {"message": "x", "codexErrorInfo": "rateLimitExceeded"},
            "willRetry": true, "threadId": "T", "turnId": "t1"}}));
        assert!(retry.is_empty());
        let ev = p.notification(&json!({"method": "error", "params": {
            "error": {"message": "{\"error\":{\"message\":\"limit hit\"}}",
                      "codexErrorInfo": "usageLimitExceeded"},
            "willRetry": false, "threadId": "T", "turnId": "t1"}}));
        assert!(matches!(
            &ev[..],
            [HarnessEvent::TurnFinished {
                is_error: true,
                error_kind: Some(ErrorKind::Quota),
                error_text: Some(t),
                ..
            }] if t == "limit hit"
        ));
        let after = p.notification(&json!({"method": "turn/completed", "params": {
            "turn": {"id": "t1", "status": "failed", "error": {"message": "x"}}}}));
        assert!(after.is_empty(), "no second TurnFinished");
    }

    #[test]
    fn error_outside_a_turn_is_an_error_event() {
        let mut p = parser(false);
        let ev = p.notification(&json!({"method": "error", "params": {
            "error": {"message": "nope", "codexErrorInfo": "unauthorized"},
            "willRetry": false, "threadId": "T", "turnId": "t9"}}));
        assert!(matches!(
            &ev[..],
            [HarnessEvent::Error {
                kind: ErrorKind::Auth,
                ..
            }]
        ));
    }

    #[test]
    fn structured_output_from_final_message() {
        let mut p = parser(true);
        p.notification(&json!({"method": "turn/started", "params": {"turn": {"id": "t1"}}}));
        let ev = p.notification(&json!({"method": "turn/completed", "params": {"turn": {
            "id": "t1", "status": "completed",
            "items": [{"type": "agentMessage", "phase": "final_answer", "text": "{\"a\":1}"}]}}}));
        assert!(matches!(
            &ev[..],
            [HarnessEvent::TurnFinished { structured: Some(s), result_text: Some(_), .. }] if s["a"] == 1
        ));
    }

    #[test]
    fn rate_limit_reached_is_a_quota_error() {
        let mut p = parser(false);
        let ev = p.notification(&json!({"method": "account/rateLimits/updated", "params": {
            "rateLimits": {"primary": {"usedPercent": 100, "windowDurationMins": 300, "resetsAt": 5},
                           "rateLimitReachedType": "rate_limit_reached"}}}));
        assert!(matches!(
            &ev[..],
            [
                HarnessEvent::Usage { .. },
                HarnessEvent::Error {
                    kind: ErrorKind::Quota,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn rate_limits_become_a_zero_token_usage_only_with_a_known_window() {
        let mut p = parser(false);
        let ev = p.notification(&json!({"method": "account/rateLimits/updated", "params": {
            "rateLimits": {
                "primary": {"usedPercent": 21, "windowDurationMins": 300, "resetsAt": 10},
                "secondary": {"usedPercent": 70, "windowDurationMins": 10080, "resetsAt": 20},
                "rateLimitReachedType": null}}}));
        assert_eq!(
            ev,
            [HarnessEvent::Usage {
                input_tokens: 0,
                output_tokens: 0,
                cost: None,
                rate_limits: Some(RateLimits {
                    five_hour_used_pct: Some(21.0),
                    five_hour_resets_at: Some(10),
                    weekly_used_pct: Some(70.0),
                    weekly_resets_at: Some(20),
                })
            }]
        );
        let none = p.notification(&json!({"method": "account/rateLimits/updated", "params": {
            "rateLimits": {"primary": {"usedPercent": 5, "windowDurationMins": 60}}}}));
        assert!(none.is_empty(), "no recognised window, no Usage");
    }

    #[test]
    fn banner_address_must_be_loopback() {
        assert_eq!(loopback_port("ws://127.0.0.1:4242").unwrap(), 4242);
        for bad in [
            "ws://0.0.0.0:4242",
            "ws://localhost:4242",
            "ws://10.0.0.1:1",
            "ws://127.0.0.1:0",
            "wss://127.0.0.1:1",
            "garbage",
        ] {
            assert!(loopback_port(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn late_turn_start_response_does_not_reopen_a_finished_turn() {
        let mut p = parser(false);
        p.notification(&json!({"method": "turn/started", "params": {"turn": {"id": "t1"}}}));
        p.notification(&json!({"method": "turn/completed", "params": {"turn": {"id": "t1"}}}));
        p.response(Kind::Turn, &json!({"turn": {"id": "t1"}}));
        assert!(!p.shared().active);
    }

    /// The params the driver sends only use properties that exist in the
    /// recorded schema.
    #[test]
    fn params_match_the_schema() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let props = |def: &str| -> Vec<String> {
            schema["definitions"][def]["properties"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect()
        };
        let spec = spec();
        let start = thread_params(&spec);
        for k in start.as_object().unwrap().keys() {
            assert!(props("ThreadStartParams").contains(k), "thread/start {k}");
        }
        let mut resume_spec = spec.clone();
        resume_spec.resume = Some("T".into());
        let resume = thread_params(&resume_spec);
        for k in resume.as_object().unwrap().keys() {
            assert!(props("ThreadResumeParams").contains(k), "thread/resume {k}");
        }
        for k in ["threadId", "input", "effort", "outputSchema"] {
            assert!(props("TurnStartParams").contains(&k.to_string()), "{k}");
        }
        for k in ["threadId", "expectedTurnId", "input"] {
            assert!(props("TurnSteerParams").contains(&k.to_string()), "{k}");
        }
        for k in ["threadId", "turnId"] {
            assert!(props("TurnInterruptParams").contains(&k.to_string()), "{k}");
        }
    }

    pub(super) fn spec() -> LaunchSpec {
        use crate::driver::McpServerSpec;
        LaunchSpec {
            run_id: "r".into(),
            run_dir: "/run".into(),
            cwd: "/cwd".into(),
            account: so_core::AccountFile::new("codex-acc1", Harness::Codex, "~/.codex", "codex")
                .unwrap(),
            launch: so_accounts::Launch {
                program: "/bin/codex".into(),
                vars: vec![],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: Some("m".into()),
            effort: Some("low".into()),
            prompt: "p".into(),
            system_addendum: "sys".into(),
            mcp: McpServerSpec {
                command: "/bin/mcp".into(),
                args: vec!["--stdio".into()],
                env: vec![("K".into(), "secret".into())],
            },
            output_schema: None,
            max_turns: None,
            resume: None,
            fork: false,
            agent: Default::default(),
        }
    }

    #[test]
    fn an_agent_adds_mcp_servers_but_cannot_replace_ours() {
        let mut spec = spec();
        spec.agent.config_fragment = Some(json!({"mcp_servers": {
            "docs": {"command": "docs-server"},
            "superorchestrator": {"command": "evil"},
        }}));
        let p = thread_params(&spec);
        let servers = &p["config"]["mcp_servers"];
        assert_eq!(servers["docs"]["command"], "docs-server");
        assert_eq!(servers["superorchestrator"]["command"], "/bin/mcp");
        assert_eq!(p["config"]["model_reasoning_effort"], "low");
    }

    #[test]
    fn redaction_hides_mcp_env_values() {
        let frame = json!({"method": "thread/start", "id": 2, "params": thread_params(&spec())});
        let shown = redact_frame(&frame).to_string();
        assert!(!shown.contains("secret"));
        assert!(shown.contains("<redacted>") && shown.contains("\"K\""));
        assert!(frame.to_string().contains("secret"));
        let plain = json!({"method": "initialize", "id": 1, "params": {"clientInfo": {}}});
        assert_eq!(
            redact_frame(&plain),
            plain,
            "frames without MCP config are untouched"
        );
    }

    #[test]
    fn join_commands() {
        let d = CodexDriver::default();
        let mut j = JoinInfo {
            account_name: "codex-acc2".into(),
            native_session_id: "thr".into(),
            endpoint: Some(crate::driver::Endpoint {
                url: "ws://127.0.0.1:9".into(),
                secret_env: Some((WS_TOKEN_ENV.into(), "s3kr1t".into())),
                directory: None,
            }),
            cwd: "/w".into(),
        };
        let live = d.join_command(&j);
        assert_eq!(
            live.argv,
            [
                "codex-acc2",
                "resume",
                "--remote",
                "ws://127.0.0.1:9",
                "--remote-auth-token-env",
                "SO_WS_TOKEN",
                "thr"
            ]
        );
        assert_eq!(
            live.env,
            [("SO_WS_TOKEN".to_string(), "s3kr1t".to_string())]
        );
        assert!(!live.argv.iter().any(|a| a.contains("s3kr1t")));
        j.endpoint = None;
        let after = d.join_command(&j);
        assert_eq!(after.argv, ["codex-acc2", "resume", "thr"]);
        assert!(after.env.is_empty());
    }
}
