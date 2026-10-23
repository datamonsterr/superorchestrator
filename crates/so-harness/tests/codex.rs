#![allow(clippy::unwrap_used)]
//! Codex driver against the scripted `fake-codex` app-server.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    CodexDriver, Driver, DriverConfig, DriverError, HarnessEvent, JoinInfo, LaunchSpec,
    McpServerSpec, RunHandle, ShutdownOutcome,
};
use tokio_tungstenite::tungstenite;

const TRANSCRIPT: &str = include_str!("fixtures/codex/transcript-stdio.jsonl");
const PROMPT: &str =
    "Create a file hello.txt containing the word hi, then reply with exactly: done";
const THREAD: &str = "01a162ee-5ba3-7400-be98-4eb69d75fead";

/// Script for a run recorded in the fixture: requests the driver does not make
/// (rate-limit and model probes) are dropped, the rest is replayed in order.
fn script_from_fixture() -> Vec<Value> {
    let lines: Vec<Value> = TRANSCRIPT
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let probe_ids: Vec<Value> = lines
        .iter()
        .filter(|v| {
            v["dir"] == "out"
                && matches!(
                    v["msg"]["method"].as_str(),
                    Some("account/rateLimits/read" | "model/list")
                )
        })
        .map(|v| v["msg"]["id"].clone())
        .collect();
    let mut steps = Vec::new();
    for v in &lines {
        let msg = &v["msg"];
        match v["dir"].as_str().unwrap() {
            "out" if probe_ids.contains(&msg["id"]) => {}
            "out" => {
                let mut step = json!({"expect": msg["method"]});
                let response = lines.iter().find(|r| {
                    r["dir"] == "in" && !msg["id"].is_null() && r["msg"]["id"] == msg["id"]
                });
                if let Some(r) = response {
                    step["respond"] = r["msg"]["result"].clone();
                }
                steps.push(step);
            }
            "in" if msg["method"].is_string() && msg["method"] != "account/updated" => {
                steps.push(json!({"send": msg}));
            }
            _ => {}
        }
    }
    steps
}

/// initialize, initialized, thread/start answering `thread`.
fn handshake(thread: &str) -> Vec<Value> {
    vec![
        json!({"expect": "initialize", "respond": {"userAgent": "x", "codexHome": "/h",
            "platformFamily": "unix", "platformOs": "linux"}}),
        json!({"expect": "initialized"}),
        json!({"expect": "thread/start", "respond": {"thread": {"id": thread}}}),
    ]
}

fn turn_start(turn: &str) -> Value {
    json!({"expect": "turn/start", "respond": {"turn": {"id": turn, "status": "inProgress"}}})
}

fn turn_started(turn: &str) -> Value {
    json!({"send": {"method": "turn/started", "params": {"threadId": THREAD, "turn": {"id": turn}}}})
}

fn message(id: &str, text: &str) -> Value {
    json!({"send": {"method": "item/completed", "params": {"item": {
        "type": "agentMessage", "id": id, "text": text, "phase": "final_answer"}}}})
}

fn turn_completed(turn: &str, status: &str) -> Value {
    json!({"send": {"method": "turn/completed", "params": {"threadId": THREAD,
        "turn": {"id": turn, "status": status, "items": []}}}})
}

fn script(steps: &[Value]) -> String {
    steps.iter().map(|s| format!("{s}\n")).collect()
}

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

    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("log.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn received(&self) -> Vec<Value> {
        self.log()
            .iter()
            .filter_map(|l| l.get("recv").cloned())
            .collect()
    }

    fn spec(&self, steps: &[Value]) -> LaunchSpec {
        let script_path = self.dir.path().join("script.jsonl");
        std::fs::write(&script_path, script(steps)).unwrap();
        LaunchSpec {
            run_id: "run-1".into(),
            run_dir: self.run_dir(),
            cwd: self.dir.path().to_path_buf(),
            account: AccountFile::new("codex-acc2", Harness::Codex, "~/.codex", "codex").unwrap(),
            launch: Launch {
                program: PathBuf::from(env!("CARGO_BIN_EXE_fake-codex")),
                vars: vec![
                    (
                        "FAKE_CODEX_SCRIPT".into(),
                        script_path.display().to_string(),
                    ),
                    (
                        "FAKE_CODEX_LOG".into(),
                        self.dir.path().join("log.jsonl").display().to_string(),
                    ),
                    ("CODEX_HOME".into(), "/acct".into()),
                ],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: Some("gpt-x".into()),
            effort: Some("low".into()),
            prompt: PROMPT.into(),
            system_addendum: "You are a worker.".into(),
            mcp: McpServerSpec {
                command: PathBuf::from("/bin/so-mcp"),
                args: vec!["--stdio".into()],
                env: vec![("SO_SECRET_TOKEN".into(), "s3cret".into())],
            },
            output_schema: None,
            max_turns: Some(8),
            resume: None,
            fork: false,
            agent: Default::default(),
        }
    }
}

fn driver() -> CodexDriver {
    CodexDriver::new(DriverConfig {
        shutdown_grace: Duration::from_millis(500),
    })
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

fn is_turn_start(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::TurnStarted)
}

fn is_exit(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::Exited { .. })
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn token_of(h: &RunHandle) -> String {
    h.join
        .endpoint
        .as_ref()
        .unwrap()
        .secret_env
        .as_ref()
        .unwrap()
        .1
        .clone()
}

fn port_of(h: &RunHandle) -> u16 {
    h.join
        .endpoint
        .as_ref()
        .unwrap()
        .url
        .rsplit(':')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[tokio::test]
async fn replays_the_recorded_run_with_a_steer() {
    let rig = Rig::new();
    let mut h = driver()
        .launch(rig.spec(&script_from_fixture()))
        .await
        .unwrap();
    assert_eq!(h.native_session_id.as_deref(), Some(THREAD));
    let steer = "Also: the reply must contain no punctuation.";
    let mut events = until(&mut h, is_turn_start).await;
    h.control.send(steer).await.unwrap();
    events.extend(until(&mut h, is_turn).await);

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
            native_session_id: THREAD.into()
        }
    );
    assert!(matches!(
        events.last(),
        Some(TurnFinished { result_text: Some(t), is_error: false, .. }) if t == "done"
    ));

    // The steer went out as turn/steer into the running turn.
    let recv = rig.received();
    let methods: Vec<&str> = recv.iter().filter_map(|f| f["method"].as_str()).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "thread/start",
            "turn/start",
            "turn/steer"
        ]
    );
    let steer_frame = &recv[4]["params"];
    assert_eq!(steer_frame["threadId"], THREAD);
    assert_eq!(
        steer_frame["expectedTurnId"],
        "01a162ee-5c0a-7793-ba41-1eb34f44e5fa"
    );
    assert_eq!(steer_frame["input"][0]["text"], steer);
}

#[tokio::test]
async fn sends_expected_params_and_starts_new_turns_when_idle() {
    let rig = Rig::new();
    let cwd = rig.dir.path().display().to_string();
    let mut steps = handshake(THREAD);
    steps[2]["check"] = json!({
        "/params/cwd": cwd,
        "/params/model": "gpt-x",
        "/params/sandbox": "danger-full-access",
        "/params/approvalPolicy": "never",
        "/params/developerInstructions": "You are a worker.",
        "/params/config/model_reasoning_effort": "low",
        "/params/config/mcp_servers/superorchestrator/command": "/bin/so-mcp",
        "/params/config/mcp_servers/superorchestrator/args/0": "--stdio",
        "/params/config/mcp_servers/superorchestrator/env/SO_SECRET_TOKEN": "s3cret",
    });
    let mut t1 = turn_start("turn-1");
    t1["check"] = json!({"/params/threadId": THREAD, "/params/input/0/text": PROMPT,
        "/params/input/0/type": "text", "/params/effort": "low"});
    let mut t2 = turn_start("turn-2");
    t2["check"] = json!({"/params/input/0/text": "again"});
    steps.extend([
        t1,
        turn_started("turn-1"),
        message("m1", "first"),
        turn_completed("turn-1", "completed"),
        t2,
        turn_started("turn-2"),
        message("m2", "second"),
        turn_completed("turn-2", "completed"),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    until(&mut h, is_turn).await;
    h.control.send("again").await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished { result_text: Some(t), .. }) if t == "second"
    ));
    // fake-codex exits 3 on any mismatch, which would have closed the socket.
    assert_eq!(rig.received().len(), 5);
}

#[tokio::test]
async fn interrupt_ends_the_turn_and_the_run_stays_usable() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    let mut int = json!({"expect": "turn/interrupt", "respond": {}});
    int["check"] = json!({"/params/threadId": THREAD, "/params/turnId": "turn-1"});
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        int,
        turn_completed("turn-1", "interrupted"),
        turn_start("turn-2"),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    until(&mut h, is_turn_start).await;
    h.control.interrupt().await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: false,
            error_kind: None,
            ..
        })
    ));
    h.control.send("next").await.unwrap();
}

#[tokio::test]
async fn usage_limit_error_finishes_the_turn_as_quota() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        json!({"send": {"method": "error", "params": {
            "error": {"message": "You've hit your usage limit.", "codexErrorInfo": "usageLimitExceeded"},
            "willRetry": false, "threadId": THREAD, "turnId": "turn-1"}}}),
        json!({"send": {"method": "thread/status/changed", "params": {
            "threadId": THREAD, "status": {"type": "systemError"}}}}),
        turn_start("turn-2"),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: true,
            error_kind: Some(ErrorKind::Quota),
            error_text: Some(t),
            ..
        }) if t.contains("usage limit")
    ));
    // The run is idle again: a new message starts a turn.
    h.control.send("retry").await.unwrap();
}

#[tokio::test]
async fn structured_output_and_resume() {
    let rig = Rig::new();
    let mut spec = rig.spec(&[]);
    spec.resume = Some("01a162ee-5ba3-7400-be98-4eb69d75fead".into());
    spec.output_schema = Some(json!({"type": "object"}));
    let mut resume = json!({"expect": "thread/resume", "respond": {"thread": {"id": "01a162ee-5ba3-7400-be98-4eb69d75fead"}}});
    resume["check"] = json!({"/params/threadId": "01a162ee-5ba3-7400-be98-4eb69d75fead"});
    let mut t1 = turn_start("turn-1");
    t1["check"] = json!({"/params/outputSchema/type": "object"});
    let steps = [
        json!({"expect": "initialize", "respond": {}}),
        json!({"expect": "initialized"}),
        resume,
        t1,
        turn_started("turn-1"),
        message("m1", "{\"answer\": 42}"),
        turn_completed("turn-1", "completed"),
    ];
    std::fs::write(rig.dir.path().join("script.jsonl"), script(&steps)).unwrap();
    let mut h = driver().launch(spec).await.unwrap();
    assert_eq!(
        h.native_session_id.as_deref(),
        Some("01a162ee-5ba3-7400-be98-4eb69d75fead")
    );
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished { structured: Some(s), .. }) if s["answer"] == 42
    ));
}

#[tokio::test]
async fn server_requests_are_declined() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        json!({"send": {"id": "srv-1", "method": "item/commandExecution/requestApproval",
            "params": {}}}),
        json!({"expect_response": true}),
        message("m1", "ok"),
        turn_completed("turn-1", "completed"),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(events.iter().any(|e| matches!(
        e,
        HarnessEvent::Error { message, .. } if message.contains("requestApproval")
    )));
    let reply = rig.received().into_iter().last().unwrap();
    assert_eq!(reply["id"], "srv-1");
    assert_eq!(reply["error"]["code"], -32601);
}

#[tokio::test]
async fn a_lost_connection_fails_the_open_turn() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        json!({"close": true}),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: true,
            error_kind: Some(ErrorKind::Other),
            ..
        })
    ));
    let err = h.control.send("x").await.unwrap_err();
    assert!(matches!(err, DriverError::Closed), "{err:?}");
}

#[tokio::test]
async fn waits_for_a_slow_server_to_become_ready() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.push(turn_start("turn-1"));
    let mut spec = rig.spec(&steps);
    spec.launch
        .vars
        .push(("FAKE_CODEX_READY_DELAY_MS".into(), "800".into()));
    let h = driver().launch(spec).await.unwrap();
    assert_eq!(h.native_session_id.as_deref(), Some(THREAD));
}

#[tokio::test]
async fn launch_fails_when_the_server_dies_before_it_is_ready() {
    let rig = Rig::new();
    let mut spec = rig.spec(&[]);
    let script_path = rig.dir.path().join("exit.jsonl");
    std::fs::write(&script_path, "{\"stderr\": \"boom\"}\n{\"exit\": 1}\n").unwrap();
    spec.launch.program = PathBuf::from(env!("CARGO_BIN_EXE_fake-harness"));
    spec.launch.vars = vec![(
        "FAKE_HARNESS_SCRIPT".into(),
        script_path.display().to_string(),
    )];
    let started = std::time::Instant::now();
    let err = driver().launch(spec).await.unwrap_err();
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(err.to_string().contains("before it printed"), "{err}");
}

/// A launch against a fake that prints `stderr_lines` on stderr and exits.
async fn launch_failing_with(stderr_lines: &[&str]) -> DriverError {
    let rig = Rig::new();
    let mut spec = rig.spec(&[]);
    let script_path = rig.dir.path().join("exit.jsonl");
    let mut script: String = stderr_lines
        .iter()
        .map(|l| format!("{}\n", json!({"stderr": l})))
        .collect();
    script.push_str("{\"exit\": 1}\n");
    std::fs::write(&script_path, script).unwrap();
    spec.launch.program = PathBuf::from(env!("CARGO_BIN_EXE_fake-harness"));
    spec.launch.vars = vec![(
        "FAKE_HARNESS_SCRIPT".into(),
        script_path.display().to_string(),
    )];
    driver().launch(spec).await.unwrap_err()
}

#[tokio::test]
async fn a_server_that_exits_with_an_auth_or_quota_line_is_classified() {
    match launch_failing_with(&["Error: not logged in. Run codex login"]).await {
        DriverError::Exited { kind, text } => {
            assert_eq!(kind, ErrorKind::Auth);
            assert!(text.contains("before it printed"), "{text}");
            assert!(text.contains("not logged in"), "{text}");
        }
        other => panic!("{other:?}"),
    }
    match launch_failing_with(&[
        "starting",
        "You have hit your usage limit. Try again later.",
    ])
    .await
    {
        DriverError::Exited { kind, text } => {
            assert_eq!(kind, ErrorKind::Quota);
            assert!(text.contains("starting"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn an_unrecognised_stderr_tail_is_kept_with_kind_other() {
    match launch_failing_with(&["segmentation fault"]).await {
        DriverError::Exited { kind, text } => {
            assert_eq!(kind, ErrorKind::Other);
            assert!(text.contains("segmentation fault"), "{text}");
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn token_never_reaches_argv_transcript_or_debug_and_join_commands_work() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        message("m1", "done"),
        turn_completed("turn-1", "completed"),
    ]);
    let spec = rig.spec(&steps);
    let debug_spec = format!("{spec:?}");
    let mut h = driver().launch(spec).await.unwrap();
    until(&mut h, is_turn).await;
    let token = token_of(&h);
    assert_eq!(token.len(), 64);

    // Token file: private, holds the token.
    let token_file = rig.run_dir().join("ws-token");
    assert_eq!(std::fs::read_to_string(&token_file).unwrap(), token);
    assert_eq!(mode(&token_file), 0o600);

    // argv names the token file and the loopback listener, never the token.
    let log = rig.log();
    let argv: Vec<String> = log[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect();
    assert_eq!(&argv[0], "app-server");
    let port = port_of(&h);
    assert!(argv.contains(&"ws://127.0.0.1:0".to_string()));
    assert!(argv.contains(&"capability-token".to_string()));
    assert!(argv.contains(&token_file.display().to_string()));
    assert!(!argv.iter().any(|a| a.contains(&token)));
    assert!(log[0]["env_keys"].to_string().contains("CODEX_HOME"));

    // Transcript: ws entries both ways, no token, no MCP secret.
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    assert!(!text.contains(&token) && !text.contains("s3cret"), "leak");
    let entries: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let ws = |dir: &str| {
        entries
            .iter()
            .filter(|e| e["channel"] == "ws" && e["dir"] == dir)
            .count()
    };
    assert!(ws("out") >= 4 && ws("in") >= 6, "{}", text.len());
    let init = entries
        .iter()
        .find(|e| e["dir"] == "out" && e["raw"].as_str().unwrap().contains("initialize"))
        .unwrap();
    assert_eq!(init["channel"], "ws");
    let done = entries
        .iter()
        .find(|e| e["raw"].as_str().unwrap().contains("turn/completed"))
        .unwrap();
    assert_eq!(done["event"]["type"], "turn_finished");

    // No token in Debug output.
    let debug = format!("{h:?} {:?} {debug_spec}", h.join);
    assert!(!debug.contains(&token), "token leaked in Debug");

    // Join commands.
    let ep = h.join.endpoint.clone().unwrap();
    assert_eq!(ep.url, format!("ws://127.0.0.1:{port}"));
    assert_eq!(ep.secret_env.as_ref().unwrap().0, "SO_WS_TOKEN");
    assert_eq!(ep.directory, None);
    let live = driver().join_command(&h.join);
    assert_eq!(
        live.argv,
        [
            "codex-acc2",
            "resume",
            "--remote",
            ep.url.as_str(),
            "--remote-auth-token-env",
            "SO_WS_TOKEN",
            THREAD
        ]
    );
    assert_eq!(live.env, [("SO_WS_TOKEN".to_string(), token.clone())]);
    assert!(!live.argv.iter().any(|a| a.contains(&token)));
    let after = driver().join_command(&JoinInfo {
        endpoint: None,
        ..h.join.clone()
    });
    assert_eq!(after.argv, ["codex-acc2", "resume", THREAD]);
    assert!(after.env.is_empty());
}

#[tokio::test]
async fn shutdown_closes_the_socket_and_kills_the_process() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.push(turn_start("turn-1"));
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    let pid = h.control.pid().unwrap();
    let outcome = h.control.shutdown(Duration::from_secs(2)).await.unwrap();
    assert!(
        matches!(outcome, ShutdownOutcome::Exited(_)),
        "SIGTERM suffices: {outcome:?}"
    );
    assert!(
        !rig.run_dir().join("ws-token").exists(),
        "token file deleted"
    );
    let events = until(&mut h, is_exit).await;
    assert!(is_exit(events.last().unwrap()));
    assert!(
        tokio::time::timeout(Duration::from_secs(2), h.events.recv())
            .await
            .unwrap()
            .is_none(),
        "event channel closes after Exited"
    );
    let p = rustix::process::Pid::from_raw(pid as i32).unwrap();
    assert!(
        rustix::process::test_kill_process(p).is_err(),
        "pid {pid} alive"
    );
}

async fn spawn_fake(rig: &Rig) -> (tokio::process::Child, u16, String) {
    let token = "abc123token".to_string();
    let token_file = rig.dir.path().join("tok");
    std::fs::write(&token_file, &token).unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_fake-codex"))
        .args([
            "app-server",
            "--listen",
            "ws://127.0.0.1:0",
            "--ws-auth",
            "capability-token",
            "--ws-token-file",
        ])
        .arg(&token_file)
        .env("FAKE_CODEX_LOG", rig.dir.path().join("log.jsonl"))
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(child.stderr.take().unwrap()).lines();
    while let Some(line) = lines.next_line().await.unwrap() {
        if let Some((_, addr)) = line.split_once("listening on: ws://127.0.0.1:") {
            return (child, addr.trim().parse().unwrap(), token);
        }
    }
    panic!("fake-codex printed no banner");
}

#[tokio::test]
async fn the_fake_server_rejects_missing_and_wrong_tokens() {
    use tungstenite::client::IntoClientRequest;
    let rig = Rig::new();
    let (_child, port, token) = spawn_fake(&rig).await;
    let url = format!("ws://127.0.0.1:{port}");

    let err = tokio_tungstenite::connect_async(url.as_str())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, tungstenite::Error::Http(r) if r.status() == 401),
        "{err:?}"
    );
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut()
        .insert("Authorization", "Bearer nope".parse().unwrap());
    let err = tokio_tungstenite::connect_async(req).await.unwrap_err();
    assert!(matches!(&err, tungstenite::Error::Http(r) if r.status() == 401));
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut()
        .insert("Authorization", format!("Bearer {token}").parse().unwrap());
    tokio_tungstenite::connect_async(req).await.unwrap();

    let rejected = rig
        .log()
        .iter()
        .filter(|l| l["auth_rejected"] == true)
        .count();
    assert_eq!(rejected, 2);
}

#[tokio::test]
async fn a_banner_with_another_host_is_refused_before_any_connection() {
    let rig = Rig::new();
    let mut spec = rig.spec(&handshake(THREAD));
    spec.launch
        .vars
        .push(("FAKE_CODEX_BANNER_HOST".into(), "10.0.0.7".into()));
    let err = driver().launch(spec).await.unwrap_err();
    assert!(err.to_string().contains("refusing"), "{err}");
    assert!(rig.received().is_empty(), "nothing was sent to the server");
}

#[tokio::test]
async fn process_death_mid_turn_fails_the_turn_before_exited() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([turn_start("turn-1"), turn_started("turn-1")]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    until(&mut h, is_turn_start).await;
    let pid = h.control.pid().unwrap();
    rustix::process::kill_process(
        rustix::process::Pid::from_raw(pid as i32).unwrap(),
        rustix::process::Signal::KILL,
    )
    .unwrap();
    let events = until(&mut h, is_exit).await;
    let finished = events.iter().position(is_turn).expect("TurnFinished");
    assert!(matches!(
        &events[finished],
        HarnessEvent::TurnFinished {
            is_error: true,
            error_kind: Some(ErrorKind::Other),
            ..
        }
    ));
    assert_eq!(finished, events.len() - 2, "right before Exited");
    // Nothing follows Exited: the stream is closed.
    assert!(
        tokio::time::timeout(Duration::from_secs(3), h.events.recv())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_rejected_steer_falls_back_to_a_new_turn() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        json!({"expect": "turn/steer",
            "respond_error": {"code": -32600, "message": "turn is not steerable"}}),
        turn_start("turn-2"),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    until(&mut h, is_turn_start).await;
    h.control.send("more").await.unwrap();
    let methods: Vec<String> = rig
        .received()
        .iter()
        .filter_map(|f| f["method"].as_str().map(String::from))
        .collect();
    assert_eq!(&methods[3..], ["turn/start", "turn/steer", "turn/start"]);
}

#[tokio::test]
async fn concurrent_sends_start_one_turn_and_steer_the_other() {
    let rig = Rig::new();
    let mut steps = handshake(THREAD);
    steps.extend([
        turn_start("turn-1"),
        turn_started("turn-1"),
        message("m1", "first"),
        turn_completed("turn-1", "completed"),
        turn_start("turn-2"),
        json!({"expect": "turn/steer", "respond": {"turnId": "turn-2"},
            "check": {"/params/expectedTurnId": "turn-2"}}),
    ]);
    let mut h = driver().launch(rig.spec(&steps)).await.unwrap();
    until(&mut h, is_turn).await;
    let (a, b) = tokio::join!(h.control.send("a"), h.control.send("b"));
    a.unwrap();
    b.unwrap();
    let methods: Vec<String> = rig
        .received()
        .iter()
        .filter_map(|f| f["method"].as_str().map(String::from))
        .collect();
    assert_eq!(&methods[4..], ["turn/start", "turn/steer"]);
}

#[tokio::test]
async fn effort_thinking_is_refused_before_spawn() {
    let rig = Rig::new();
    let mut spec = rig.spec(&[]);
    spec.effort = Some("thinking".into());
    let err = driver().launch(spec).await.err().unwrap();
    assert!(
        err.to_string()
            .contains("effort `thinking` is not valid for codex"),
        "{err}"
    );
}
