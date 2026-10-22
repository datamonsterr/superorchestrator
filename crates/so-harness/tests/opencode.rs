#![allow(clippy::unwrap_used)]
//! opencode driver against an in-process axum server that replays the
//! recorded fixtures.

use std::convert::Infallible;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_core::Stream;
use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    Driver, DriverConfig, DriverError, HarnessEvent, JoinInfo, LaunchSpec, McpServerSpec,
    OpencodeDriver, RunControl, RunHandle, ShutdownOutcome,
};
use tokio::sync::mpsc;

const TASK: &str = include_str!("fixtures/opencode/task.sse");
const ABORT: &str = include_str!("fixtures/opencode/abort.sse");
const QUOTA: &str = include_str!("fixtures/opencode/error-402.sse");
const SESSION: &str = include_str!("fixtures/opencode/session-create.response.json");
const SESSION_ID: &str = "ses_e9d0e831fffejyPMluk5gnwrFx";
const FIXTURE_ID: &str = "ses_e9d0c9735ffeANTApp10GZgEp6";
const PROMPT: &str =
    "Create a file hello.txt containing the word hi, then reply with exactly: done";

// ---------------------------------------------------------------------------
// Fake server
// ---------------------------------------------------------------------------

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in input.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn basic(password: &str) -> String {
    format!(
        "Basic {}",
        base64(format!("opencode:{password}").as_bytes())
    )
}

struct Rx(mpsc::UnboundedReceiver<Vec<u8>>);

impl Stream for Rx {
    type Item = Result<Vec<u8>, Infallible>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|o| o.map(Ok))
    }
}

#[derive(Default)]
struct Fake {
    /// `Some`: reject other credentials with 401.
    expected_auth: Option<String>,
    on_prompt: Vec<String>,
    on_abort: Vec<String>,
    auth_seen: Mutex<Vec<String>>,
    dirs_seen: Mutex<Vec<(String, String)>>,
    prompts: Mutex<Vec<Value>>,
    session_bodies: Mutex<Vec<Value>>,
    aborts: Mutex<u32>,
    subscribers: Mutex<Vec<mpsc::UnboundedSender<Vec<u8>>>>,
    /// `/event` answers 503 while set.
    refuse_event: std::sync::atomic::AtomicBool,
    /// Body of `GET /session/status`.
    status: Mutex<Option<Value>>,
    /// Send events one byte per chunk.
    byte_chunks: bool,
}

impl Fake {
    /// Ends every open event stream (the server side closes the response).
    fn drop_streams(&self) {
        self.subscribers.lock().unwrap().clear();
    }

    fn streams(&self) -> usize {
        self.subscribers.lock().unwrap().len()
    }

    fn push(&self, lines: &[String]) {
        let subs = self.subscribers.lock().unwrap();
        for l in lines {
            for s in subs.iter() {
                let data = format!("{l}\n\n").into_bytes();
                if self.byte_chunks {
                    for b in data {
                        let _ = s.send(vec![b]);
                    }
                } else {
                    let _ = s.send(data);
                }
            }
        }
    }
}

/// Event lines of a fixture (without `server.connected`), recorded session
/// id replaced.
fn fixture_lines(sse: &str) -> Vec<String> {
    sse.lines()
        .filter(|l| l.starts_with("data:") && !l.contains("\"server.connected\""))
        .map(|l| l.replace(FIXTURE_ID, SESSION_ID))
        .collect()
}

async fn guard(State(f): State<Arc<Fake>>, req: Request, next: Next) -> Response {
    let auth = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    f.auth_seen.lock().unwrap().push(auth.clone());
    if f.expected_auth.as_ref().is_some_and(|e| *e != auth) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let path = req.uri().path().to_string();
    if path != "/global/health" {
        let dir = req
            .headers()
            .get("x-opencode-directory")
            .and_then(|v| v.to_str().ok());
        let Some(dir) = dir else {
            return (StatusCode::BAD_REQUEST, "missing x-opencode-directory").into_response();
        };
        f.dirs_seen.lock().unwrap().push((path, dir.to_string()));
    }
    next.run(req).await
}

async fn serve(fake: Fake) -> (String, Arc<Fake>) {
    let fake = Arc::new(fake);
    let app = Router::new()
        .route(
            "/global/health",
            get(|| async { Json(json!({"healthy": true, "version": "test"})) }),
        )
        .route(
            "/session",
            post(
                |State(f): State<Arc<Fake>>, Json(b): Json<Value>| async move {
                    f.session_bodies.lock().unwrap().push(b);
                    (
                        [(header::CONTENT_TYPE, "application/json")],
                        SESSION.to_string(),
                    )
                },
            ),
        )
        .route(
            "/event",
            get(|State(f): State<Arc<Fake>>| async move {
                if f.refuse_event.load(std::sync::atomic::Ordering::SeqCst) {
                    return StatusCode::SERVICE_UNAVAILABLE.into_response();
                }
                let (tx, rx) = mpsc::unbounded_channel();
                let _ = tx.send(
                    b"data: {\"id\": \"evt_0\", \"type\": \"server.connected\", \"properties\": {}}\n\n"
                        .to_vec(),
                );
                f.subscribers.lock().unwrap().push(tx);
                Response::builder()
                    .header(header::CONTENT_TYPE, "text/event-stream")
                    .body(Body::from_stream(Rx(rx)))
                    .unwrap()
            }),
        )
        .route(
            "/session/status",
            get(|State(f): State<Arc<Fake>>| async move {
                Json(f.status.lock().unwrap().clone().unwrap_or(json!({})))
            }),
        )
        .route(
            "/session/{id}/prompt_async",
            post(
                |State(f): State<Arc<Fake>>,
                 Path(_id): Path<String>,
                 _h: HeaderMap,
                 Json(b): Json<Value>| async move {
                    f.prompts.lock().unwrap().push(b);
                    f.push(&f.on_prompt);
                    StatusCode::NO_CONTENT
                },
            ),
        )
        .route(
            "/session/{id}/abort",
            post(
                |State(f): State<Arc<Fake>>, Path(_id): Path<String>| async move {
                    *f.aborts.lock().unwrap() += 1;
                    f.push(&f.on_abort);
                    Json(json!(true))
                },
            ),
        )
        .layer(middleware::from_fn_with_state(fake.clone(), guard))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, fake)
}

// ---------------------------------------------------------------------------
// Rig
// ---------------------------------------------------------------------------

struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn run_dir(&self) -> PathBuf {
        self.dir.path().join("run")
    }

    fn cwd(&self) -> PathBuf {
        self.dir.path().to_path_buf()
    }

    fn spec(&self) -> LaunchSpec {
        LaunchSpec {
            run_id: "run-1".into(),
            run_dir: self.run_dir(),
            cwd: self.cwd(),
            account: AccountFile::new("opencode-acc1", Harness::Opencode, "~/.oc", "opencode")
                .unwrap(),
            launch: Launch {
                program: PathBuf::from("/bin/sh"),
                vars: vec![("XDG_DATA_HOME".into(), "/acct/data".into())],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: Some("opencode/glm-5.3-flash-free".into()),
            effort: Some("high".into()),
            prompt: PROMPT.into(),
            system_addendum: "You are a worker.".into(),
            mcp: McpServerSpec {
                command: PathBuf::from("/bin/so-mcp"),
                args: vec!["mcp".into()],
                env: vec![("SO_SECRET_TOKEN".into(), "mcp-secret".into())],
            },
            output_schema: None,
            max_turns: None,
            resume: None,
        }
    }

    fn transcript(&self) -> String {
        std::fs::read_to_string(self.run_dir().join("transcript.jsonl")).unwrap()
    }
}

fn driver(url: &str, password: &str) -> OpencodeDriver {
    OpencodeDriver::with_existing_server(
        DriverConfig {
            shutdown_grace: Duration::from_millis(500),
        },
        url,
        password,
    )
}

async fn next(h: &mut RunHandle) -> HarnessEvent {
    tokio::time::timeout(Duration::from_secs(10), h.events.recv())
        .await
        .expect("timed out waiting for event")
        .expect("event stream closed")
}

/// Collects non-raw events until `stop` matches (inclusive).
async fn until(h: &mut RunHandle, stop: impl Fn(&HarnessEvent) -> bool) -> Vec<HarnessEvent> {
    let mut out = Vec::new();
    loop {
        let e = next(h).await;
        if matches!(e, HarnessEvent::Raw { .. }) {
            continue;
        }
        let done = stop(&e);
        out.push(e);
        if done {
            return out;
        }
    }
}

fn is_turn(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::TurnFinished { .. })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn task_fixture_event_order_and_requests() {
    let (url, fake) = serve(Fake {
        expected_auth: Some(basic("pw-1")),
        on_prompt: fixture_lines(TASK),
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let mut h = driver(&url, "pw-1").launch(rig.spec()).await.unwrap();
    assert_eq!(h.native_session_id.as_deref(), Some(SESSION_ID));

    let events = until(&mut h, is_turn).await;
    let want = vec![
        HarnessEvent::SessionStarted {
            native_session_id: SESSION_ID.into(),
        },
        HarnessEvent::TurnStarted,
        HarnessEvent::ToolCallStarted {
            name: "write".into(),
        },
        HarnessEvent::ToolCallFinished {
            name: "write".into(),
            ok: true,
        },
        HarnessEvent::Usage {
            input_tokens: 13753,
            output_tokens: 69,
            cost: Some(0.0),
            rate_limits: None,
        },
        HarnessEvent::AssistantMessage {
            text: "done".into(),
        },
        HarnessEvent::Usage {
            input_tokens: 775 + 13056,
            output_tokens: 3,
            cost: Some(0.0),
            rate_limits: None,
        },
        HarnessEvent::TurnFinished {
            result_text: Some("done".into()),
            structured: None,
            is_error: false,
            error_kind: None,
            error_text: None,
        },
    ];
    assert_eq!(events, want);

    // The duplicate trailing session.idle of the fixture must not add a turn.
    let rig_cwd = rig.cwd().display().to_string();
    let prompts = fake.prompts.lock().unwrap().clone();
    assert_eq!(prompts.len(), 1);
    assert_eq!(
        prompts[0],
        json!({
            "model": {"providerID": "opencode", "modelID": "glm-5.3-flash-free"},
            "variant": "high",
            "system": "You are a worker.",
            "parts": [{"type": "text", "text": PROMPT}],
        })
    );
    // Every call carried the directory header (and valid credentials).
    let dirs = fake.dirs_seen.lock().unwrap().clone();
    let paths: Vec<_> = dirs.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"/event") && paths.contains(&"/session"));
    assert!(paths.contains(&format!("/session/{SESSION_ID}/prompt_async").as_str()));
    assert!(dirs.iter().all(|(_, d)| *d == rig_cwd), "{dirs:?}");
    assert!(
        fake.auth_seen
            .lock()
            .unwrap()
            .iter()
            .all(|a| *a == basic("pw-1"))
    );

    // Transcript: SSE and HTTP channels, no credentials anywhere.
    let t = rig.transcript();
    let channels: Vec<String> = t
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["channel"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert!(channels.iter().any(|c| c == "sse"));
    assert!(channels.iter().any(|c| c == "http"));
    assert!(t.contains("POST /session") && t.contains("prompt_async"));
    for leaked in ["pw-1", &basic("pw-1"), "Authorization", "authorization"] {
        assert!(!t.contains(leaked), "{leaked} leaked into the transcript");
    }

    // Live join: attach with the secret only in env; endpoint carries it.
    let ep = h.join.endpoint.clone().unwrap();
    assert_eq!(ep.url, url);
    assert_eq!(ep.directory.as_deref(), Some(rig.cwd().as_path()));
    let d = driver(&url, "pw-1");
    let live = d.join_command(&h.join);
    assert_eq!(
        live.argv,
        [
            "opencode",
            "attach",
            url.as_str(),
            "--session",
            SESSION_ID,
            "--dir",
            rig_cwd.as_str()
        ]
    );
    assert_eq!(
        live.env,
        vec![("OPENCODE_SERVER_PASSWORD".to_string(), "pw-1".to_string())]
    );
    assert!(!live.argv.iter().any(|a| a.contains("pw-1")));
    let after = d.join_command(&JoinInfo {
        endpoint: None,
        ..h.join.clone()
    });
    assert_eq!(after.argv, ["opencode-acc1", "--session", SESSION_ID]);
    assert!(after.env.is_empty());

    // Debug output never shows the password.
    for dbg in [format!("{h:?}"), format!("{:?}", h.join), format!("{d:?}")] {
        assert!(!dbg.contains("pw-1"), "{dbg}");
    }

    // A follow-up goes through prompt_async too; shutdown ends the stream.
    h.control.send("again").await.unwrap();
    assert_eq!(fake.prompts.lock().unwrap().len(), 2);
    assert_eq!(
        h.control.shutdown(Duration::from_secs(1)).await.unwrap(),
        ShutdownOutcome::Exited(None)
    );
    let mut last = None;
    while let Some(e) = h.events.recv().await {
        last = Some(e);
    }
    assert!(matches!(last, Some(HarnessEvent::Exited { code: None })));
}

#[tokio::test]
async fn structured_output_uses_format_and_returns_structured() {
    let sid = SESSION_ID;
    let msg = "msg_aaa";
    let lines: Vec<String> = [
        json!({"type":"session.status","properties":{"sessionID":sid,"status":{"type":"busy"}}}),
        json!({"type":"message.updated","properties":{"sessionID":sid,"info":{
            "id":msg,"role":"assistant","sessionID":sid,"time":{"created":1,"completed":2},
            "tokens":{"input":10,"output":2,"reasoning":0,"cache":{"read":0,"write":0}},
            "cost":0.5,"structured":{"answer":42}}}}),
        json!({"type":"message.part.updated","properties":{"sessionID":sid,"part":{
            "id":"prt_1","type":"text","messageID":msg,"sessionID":sid,
            "text":"{\"answer\":42}","time":{"start":1,"end":2}}}}),
        json!({"type":"session.idle","properties":{"sessionID":sid}}),
    ]
    .iter()
    .map(|v| format!("data: {v}"))
    .collect();
    let (url, fake) = serve(Fake {
        on_prompt: lines,
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let schema = json!({"type":"object","properties":{"answer":{"type":"number"}}});
    let mut spec = rig.spec();
    spec.output_schema = Some(schema.clone());
    let mut h = driver(&url, "pw").launch(spec).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert_eq!(
        events.last().unwrap(),
        &HarnessEvent::TurnFinished {
            result_text: Some("{\"answer\":42}".into()),
            structured: Some(json!({"answer": 42})),
            is_error: false,
            error_kind: None,
            error_text: None,
        }
    );
    assert!(events.contains(&HarnessEvent::Usage {
        input_tokens: 10,
        output_tokens: 2,
        cost: Some(0.5),
        rate_limits: None,
    }));
    let body = fake.prompts.lock().unwrap()[0].clone();
    assert_eq!(
        body["format"],
        json!({"type":"json_schema","schema":schema,"retryCount":2})
    );
}

#[tokio::test]
async fn interrupt_posts_abort_and_ends_turn() {
    let all = fixture_lines(ABORT);
    let split = all
        .iter()
        .position(|l| l.contains("session.error"))
        .unwrap();
    let (url, fake) = serve(Fake {
        on_prompt: all[..split].to_vec(),
        on_abort: all[split..].to_vec(),
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let mut h = driver(&url, "pw").launch(rig.spec()).await.unwrap();
    until(&mut h, |e| matches!(e, HarnessEvent::TurnStarted)).await;
    assert_eq!(*fake.aborts.lock().unwrap(), 0);
    h.control.interrupt().await.unwrap();
    assert_eq!(*fake.aborts.lock().unwrap(), 1);
    let events = until(&mut h, is_turn).await;
    assert_eq!(
        events.last().unwrap(),
        &HarnessEvent::TurnFinished {
            result_text: None,
            structured: None,
            is_error: true,
            error_kind: Some(ErrorKind::Other),
            error_text: Some("turn aborted".into()),
        }
    );
    // The abort ack and the trailing idle events do not start another turn.
    let t = rig.transcript();
    assert!(t.contains(&format!("POST /session/{SESSION_ID}/abort")));
}

#[tokio::test]
async fn insufficient_funds_is_quota() {
    let (url, _fake) = serve(Fake {
        on_prompt: fixture_lines(QUOTA),
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let d = driver(&url, "pw");
    let mut h = d.launch(rig.spec()).await.unwrap();
    let events = until(&mut h, is_turn).await;
    let Some(HarnessEvent::TurnFinished {
        is_error,
        error_kind,
        error_text,
        ..
    }) = events.last()
    else {
        panic!("no turn end");
    };
    assert!(is_error);
    assert_eq!(*error_kind, Some(ErrorKind::Quota));
    let text = error_text.clone().unwrap();
    assert!(text.contains("402") && text.contains("Insufficient account funds"));
    assert_eq!(d.classify_error(&text), ErrorKind::Quota);
}

#[tokio::test]
async fn wrong_password_is_auth() {
    let (url, _fake) = serve(Fake {
        expected_auth: Some(basic("right")),
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let d = driver(&url, "wrong");
    let err = d.launch(rig.spec()).await.unwrap_err();
    assert!(matches!(err, DriverError::Other(_)), "{err:?}");
    let msg = err.to_string();
    assert!(msg.contains("401"), "{msg}");
    assert_eq!(d.classify_error(&msg), ErrorKind::Auth);
    assert!(!msg.contains("wrong") && !msg.contains("right"));
}

#[tokio::test]
async fn model_must_be_provider_slash_model() {
    let (url, _fake) = serve(Fake::default()).await;
    let rig = Rig::new();
    let mut spec = rig.spec();
    spec.model = Some("just-a-model".into());
    let err = driver(&url, "pw").launch(spec).await.unwrap_err();
    assert!(err.to_string().contains("provider/model"), "{err}");
}

#[tokio::test]
async fn spawned_server_gets_password_and_config_via_env_and_file() {
    let (url, fake) = serve(Fake {
        on_prompt: fixture_lines(TASK),
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let out = rig.dir.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    let script = rig.dir.path().join("fake-opencode.sh");
    std::fs::write(
        &script,
        format!(
            "printf '%s\\n' \"$@\" > {out}/argv\nenv > {out}/env\n\
             echo \"opencode server listening on {url}\"\nexec sleep 30\n",
            out = out.display()
        ),
    )
    .unwrap();
    let mut spec = rig.spec();
    spec.launch.argv_prefix = vec![script.display().to_string()];
    let d = OpencodeDriver::new(DriverConfig {
        shutdown_grace: Duration::from_millis(500),
    });
    let mut h = d.launch(spec).await.unwrap();
    until(&mut h, is_turn).await;

    let argv = std::fs::read_to_string(out.join("argv")).unwrap();
    assert_eq!(
        argv.lines().collect::<Vec<_>>(),
        ["serve", "--hostname", "127.0.0.1", "--port", "0"]
    );
    let env = std::fs::read_to_string(out.join("env")).unwrap();
    let var = |k: &str| {
        env.lines()
            .find_map(|l| l.strip_prefix(&format!("{k}=")))
            .map(String::from)
    };
    let pw = var("OPENCODE_SERVER_PASSWORD").unwrap();
    assert_eq!(pw.len(), 64);
    assert!(pw.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(var("OPENCODE_SERVER_USERNAME").as_deref(), Some("opencode"));
    assert_eq!(var("XDG_DATA_HOME").as_deref(), Some("/acct/data"));
    let config_path = rig.run_dir().join("opencode.json");
    assert_eq!(
        var("OPENCODE_CONFIG").unwrap(),
        config_path.display().to_string()
    );

    // opencode.json: content and mode.
    assert_eq!(
        std::fs::metadata(&config_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(&config_path).unwrap()).unwrap();
    assert_eq!(
        cfg["mcp"]["superorchestrator"],
        json!({
            "type": "local",
            "command": ["/bin/so-mcp", "mcp"],
            "environment": {"SO_SECRET_TOKEN": "mcp-secret"},
            "enabled": true,
        })
    );
    assert_eq!(cfg["permission"], json!({"*": "allow"}));

    // The same password authenticated every HTTP call.
    assert!(
        fake.auth_seen
            .lock()
            .unwrap()
            .iter()
            .all(|a| *a == basic(&pw))
    );
    // And it is nowhere else: not argv, transcript, Debug, join argv.
    assert!(!argv.contains(&pw));
    let t = rig.transcript();
    assert!(!t.contains(&pw) && !t.contains(&basic(&pw)));
    for dbg in [format!("{h:?}"), format!("{:?}", h.join), format!("{d:?}")] {
        assert!(!dbg.contains(&pw), "{dbg}");
    }
    let ep = h.join.endpoint.clone().unwrap();
    assert_eq!(
        ep.secret_env,
        Some(("OPENCODE_SERVER_PASSWORD".to_string(), pw.clone()))
    );
    let live = d.join_command(&h.join);
    assert!(!live.argv.iter().any(|a| a.contains(&pw)));
    assert_eq!(live.env[0].1, pw);

    // Shutdown kills the server and ends the event stream with Exited.
    let pid = h.control.pid().unwrap();
    assert!(matches!(h.control, RunControl::Opencode(_)));
    assert_eq!(
        h.control.shutdown(Duration::from_secs(1)).await.unwrap(),
        ShutdownOutcome::Killed
    );
    let mut saw_exit = false;
    while let Some(e) = h.events.recv().await {
        saw_exit = matches!(e, HarnessEvent::Exited { .. });
    }
    assert!(saw_exit, "Exited must be the last event");
    let p = rustix_gone(pid);
    assert!(p, "server process {pid} survived");
}

fn rustix_gone(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')))
        .unwrap_or(true)
}

#[tokio::test]
async fn server_that_exits_before_listening_fails_fast() {
    let rig = Rig::new();
    let mut spec = rig.spec();
    spec.launch.argv_prefix = vec!["-c".into(), "echo boom >&2; exit 3".into()];
    let d = OpencodeDriver::new(DriverConfig::default());
    let err = d.launch(spec).await.unwrap_err();
    assert!(err.to_string().contains("exited before printing"), "{err}");
}

fn line(v: Value) -> String {
    format!("data: {v}")
}

fn busy_line() -> String {
    line(
        json!({"type":"session.status","properties":{"sessionID":SESSION_ID,"status":{"type":"busy"}}}),
    )
}

fn idle_line() -> String {
    line(json!({"type":"session.idle","properties":{"sessionID":SESSION_ID}}))
}

async fn wait_streams(fake: &Fake, n: usize) {
    for _ in 0..100 {
        if fake.streams() >= n {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no event stream reconnected");
}

#[tokio::test]
async fn events_split_into_single_byte_chunks_still_map() {
    let msg = "msg_u8";
    let text = "caf\u{e9} \u{1f600} done";
    let lines = vec![
        busy_line(),
        line(
            json!({"type":"message.updated","properties":{"sessionID":SESSION_ID,"info":{
            "id":msg,"role":"assistant","sessionID":SESSION_ID,"time":{"created":1}}}}),
        ),
        line(
            json!({"type":"message.part.updated","properties":{"sessionID":SESSION_ID,"part":{
            "id":"prt_1","type":"text","messageID":msg,"sessionID":SESSION_ID,
            "text":text,"time":{"start":1,"end":2}}}}),
        ),
        idle_line(),
    ];
    let (url, _fake) = serve(Fake {
        on_prompt: lines,
        byte_chunks: true,
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let mut h = driver(&url, "pw").launch(rig.spec()).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(events.contains(&HarnessEvent::AssistantMessage { text: text.into() }));
}

#[tokio::test]
async fn dropped_stream_reconnects_and_the_turn_continues() {
    let (url, fake) = serve(Fake {
        on_prompt: vec![busy_line()],
        ..Fake::default()
    })
    .await;
    *fake.status.lock().unwrap() = Some(json!({SESSION_ID: {"type": "busy"}}));
    let rig = Rig::new();
    let mut h = driver(&url, "pw").launch(rig.spec()).await.unwrap();
    until(&mut h, |e| matches!(e, HarnessEvent::TurnStarted)).await;
    fake.drop_streams();
    wait_streams(&fake, 1).await;
    fake.push(&[idle_line()]);
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: false,
            ..
        })
    ));
}

#[tokio::test]
async fn idle_session_after_reconnect_closes_the_open_turn() {
    // The idle event was lost while the stream was down.
    let (url, fake) = serve(Fake {
        on_prompt: vec![busy_line()],
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let mut h = driver(&url, "pw").launch(rig.spec()).await.unwrap();
    until(&mut h, |e| matches!(e, HarnessEvent::TurnStarted)).await;
    fake.drop_streams();
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: false,
            ..
        })
    ));
}

#[tokio::test]
async fn failed_reconnect_fails_the_turn_and_exits() {
    let (url, fake) = serve(Fake {
        on_prompt: vec![busy_line()],
        ..Fake::default()
    })
    .await;
    let rig = Rig::new();
    let mut h = driver(&url, "pw").launch(rig.spec()).await.unwrap();
    until(&mut h, |e| matches!(e, HarnessEvent::TurnStarted)).await;
    fake.refuse_event
        .store(true, std::sync::atomic::Ordering::SeqCst);
    fake.drop_streams();
    let events = until(&mut h, |e| matches!(e, HarnessEvent::Exited { .. })).await;
    let kinds: Vec<&str> = events
        .iter()
        .map(|e| match e {
            HarnessEvent::TurnFinished { is_error: true, .. } => "turn_error",
            HarnessEvent::Error { .. } => "error",
            HarnessEvent::Exited { .. } => "exited",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["turn_error", "error", "exited"], "{events:?}");
    // The channel closes after Exited.
    assert!(h.events.recv().await.is_none());
}
