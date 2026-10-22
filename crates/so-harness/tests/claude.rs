#![allow(clippy::unwrap_used)]
//! Claude driver against the scripted fake harness.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    ClaudeDriver, Driver, HarnessEvent, JoinInfo, LaunchSpec, McpServerSpec, RunHandle,
};

const TRANSCRIPT: &str = include_str!("fixtures/claude/transcript.jsonl");
const INTERRUPT: &str = include_str!("fixtures/claude/interrupt.jsonl");
const PROMPT: &str =
    "Create a file hello.txt containing the word hi, then reply with exactly: done";
const SECOND: &str = "What is your codename? One word.";

/// Turns a recorded fixture into a fake-harness script: `out` lines become
/// stdin expectations, `in` lines become stdout, notes are skipped.
fn script_from_fixture(fixture: &str) -> String {
    let mut steps = Vec::new();
    for l in fixture.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        let msg = &v["msg"];
        match v["dir"].as_str().unwrap() {
            "out" if msg["type"] == "control_request" => {
                steps.push(json!({"expect_stdin_contains": msg["request"]["subtype"]}));
            }
            "out" => {
                steps.push(json!({"expect_stdin_contains": msg["message"]["content"][0]["text"]}));
            }
            "in" => steps.push(json!({"stdout": msg})),
            _ => {}
        }
    }
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

    fn spec(&self, script: &str) -> LaunchSpec {
        let script_path = self.dir.path().join("script.jsonl");
        std::fs::write(&script_path, script).unwrap();
        LaunchSpec {
            run_id: "run-1".into(),
            run_dir: self.run_dir(),
            cwd: self.dir.path().to_path_buf(),
            account: AccountFile::new("claude-acc1", Harness::Claude, "~/.claude", "claude")
                .unwrap(),
            launch: Launch {
                program: PathBuf::from(env!("CARGO_BIN_EXE_fake-harness")),
                vars: vec![
                    (
                        "FAKE_HARNESS_SCRIPT".into(),
                        script_path.display().to_string(),
                    ),
                    (
                        "FAKE_HARNESS_LOG".into(),
                        self.dir.path().join("log.jsonl").display().to_string(),
                    ),
                    ("CLAUDE_CONFIG_DIR".into(), "/acct".into()),
                ],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: Some("haiku".into()),
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
        }
    }
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
        if matches!(e, HarnessEvent::Raw { .. } | HarnessEvent::Stderr { .. }) {
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

#[tokio::test]
async fn normal_run_two_turns_and_transcript() {
    let rig = Rig::new();
    let spec = rig.spec(&script_from_fixture(TRANSCRIPT));
    let mut h = ClaudeDriver::default().launch(spec).await.unwrap();
    assert!(h.native_session_id.is_some());

    let first = until(&mut h, is_turn).await;
    let pos = |pred: fn(&HarnessEvent) -> bool| first.iter().position(pred).unwrap();
    let start = pos(|e| matches!(e, HarnessEvent::SessionStarted { .. }));
    let msg = pos(|e| matches!(e, HarnessEvent::AssistantMessage { text } if text == "done"));
    let usage = pos(|e| matches!(e, HarnessEvent::Usage { .. }));
    assert!(start < msg && msg < usage && usage < first.len() - 1);
    assert!(first.contains(&HarnessEvent::ToolCallStarted {
        name: "Bash".into()
    }));
    assert!(first.contains(&HarnessEvent::ToolCallFinished {
        name: "Bash".into(),
        ok: true
    }));

    h.control.send(SECOND).await.unwrap();
    let second = until(&mut h, is_turn).await;
    assert!(
        second
            .iter()
            .any(|e| matches!(e, HarnessEvent::AssistantMessage { text } if text == "PELICAN"))
    );
    assert!(matches!(
        second.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: false,
            ..
        })
    ));

    h.control.shutdown().await.unwrap();
    let rest = until(&mut h, |e| matches!(e, HarnessEvent::Exited { .. })).await;
    assert_eq!(rest.last(), Some(&HarnessEvent::Exited { code: Some(0) }));

    // Transcript: out prompt, in lines (with events), out second message.
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    let entries: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries[0]["dir"], "out");
    assert!(entries[0]["raw"].as_str().unwrap().contains(PROMPT));
    assert!(
        entries
            .iter()
            .any(|e| e["dir"] == "in" && e["event"]["type"] == "session_started")
    );
    assert!(entries.iter().filter(|e| e["dir"] == "out").count() == 2);
    // Raw events carry usable offsets.
    let rig2 = Rig::new();
    let mut h2 = ClaudeDriver::default()
        .launch(rig2.spec(&script_from_fixture(TRANSCRIPT)))
        .await
        .unwrap();
    let raw = loop {
        if let HarnessEvent::Raw { line, offset } = next(&mut h2).await {
            break (line, offset);
        }
    };
    assert!(raw.1 > 0 && !raw.0.is_empty());
}

#[tokio::test]
async fn argv_and_run_files() {
    let rig = Rig::new();
    let spec = rig.spec(&script_from_fixture(TRANSCRIPT));
    let mut h = ClaudeDriver::default().launch(spec).await.unwrap();
    let sid = h.native_session_id.clone().unwrap();
    until(&mut h, is_turn).await;
    h.control.shutdown().await.unwrap();

    let log = rig.log();
    let argv: Vec<&str> = log[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let run = rig.run_dir();
    let expect = [
        "-p",
        "--verbose",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--dangerously-skip-permissions",
        "--session-id",
        &sid,
        "--mcp-config",
        &format!("{}/mcp.json", run.display()),
        "--append-system-prompt-file",
        &format!("{}/system.md", run.display()),
        "--model",
        "haiku",
        "--effort",
        "low",
        "--max-turns",
        "8",
    ];
    assert_eq!(argv, expect);
    let joined = argv.join(" ");
    assert!(!joined.contains("hello.txt") && !joined.contains("s3cret"));

    // Env: account vars are passed, mcp env is not.
    let keys = log[0]["env_keys"].as_array().unwrap();
    assert!(keys.contains(&json!("CLAUDE_CONFIG_DIR")));
    assert!(!keys.contains(&json!("SO_SECRET_TOKEN")));
    // The prompt arrived on stdin.
    assert!(log[1]["stdin"].as_str().unwrap().contains(PROMPT));

    let mcp: Value = serde_json::from_slice(&std::fs::read(run.join("mcp.json")).unwrap()).unwrap();
    let srv = &mcp["mcpServers"]["superorchestrator"];
    assert_eq!(srv["command"], "/bin/so-mcp");
    assert_eq!(srv["args"], json!(["--stdio"]));
    assert_eq!(srv["env"]["SO_SECRET_TOKEN"], "s3cret");
    assert_eq!(
        std::fs::read_to_string(run.join("system.md")).unwrap(),
        "You are a worker."
    );
}

#[tokio::test]
async fn resume_uses_resume_flag_and_schema() {
    let rig = Rig::new();
    let mut spec = rig.spec("");
    spec.resume = Some("11111111-2222-7333-8444-555555555555".into());
    spec.output_schema = Some(json!({"type": "object"}));
    let mut h = ClaudeDriver::default().launch(spec).await.unwrap();
    assert_eq!(
        h.native_session_id.as_deref(),
        Some("11111111-2222-7333-8444-555555555555")
    );
    h.control.shutdown().await.unwrap();
    until(&mut h, |e| matches!(e, HarnessEvent::Exited { .. })).await;
    let argv = rig.log()[0]["argv"].clone();
    let argv: Vec<&str> = argv
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let i = argv.iter().position(|a| *a == "--resume").unwrap();
    assert_eq!(argv[i + 1], "11111111-2222-7333-8444-555555555555");
    assert!(!argv.contains(&"--session-id"));
    let j = argv.iter().position(|a| *a == "--json-schema").unwrap();
    assert_eq!(argv[j + 1], r#"{"type":"object"}"#);
}

#[tokio::test]
async fn interrupt_sends_control_request() {
    let rig = Rig::new();
    let mut spec = rig.spec(&script_from_fixture(INTERRUPT));
    spec.prompt = "Run the shell command `sleep 60` and then say done.".into();
    let mut h = ClaudeDriver::default().launch(spec).await.unwrap();
    until(&mut h, |e| {
        matches!(e, HarnessEvent::ToolCallStarted { .. })
    })
    .await;
    h.control.interrupt().await.unwrap();
    let rest = until(&mut h, is_turn).await;
    assert!(rest.contains(&HarnessEvent::ToolCallFinished {
        name: "Bash".into(),
        ok: false
    }));
    let log = rig.log();
    let line: Value = serde_json::from_str(log[2]["stdin"].as_str().unwrap()).unwrap();
    assert_eq!(line["type"], "control_request");
    assert_eq!(line["request"]["subtype"], "interrupt");
    assert!(line["request_id"].as_str().is_some());
    // The process stays alive after the interrupted turn.
    assert!(h.control.pid().is_some());
    h.control.shutdown().await.unwrap();
}

#[tokio::test]
async fn quota_error_is_classified() {
    let rig = Rig::new();
    let script = [
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"type": "result", "subtype": "success", "is_error": true,
            "result": "Claude AI usage limit reached|1792669200", "usage": {}}}),
    ]
    .iter()
    .map(|s| format!("{s}\n"))
    .collect::<String>();
    let mut h = ClaudeDriver::default()
        .launch(rig.spec(&script))
        .await
        .unwrap();
    let evs = until(&mut h, |e| matches!(e, HarnessEvent::Error { .. })).await;
    assert!(evs.iter().any(|e| matches!(
        e,
        HarnessEvent::TurnFinished { is_error: true, error_text: Some(t), .. } if t.contains("usage limit")
    )));
    assert!(matches!(
        evs.last(),
        Some(HarnessEvent::Error {
            kind: ErrorKind::Quota,
            ..
        })
    ));
    assert_eq!(
        ClaudeDriver::default().classify_error("usage limit reached"),
        ErrorKind::Quota
    );
    h.control.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_kills_after_grace_and_captures_stderr() {
    let rig = Rig::new();
    let script = [
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stderr": "boom"}),
        json!({"sleep_ms": 60000}),
    ]
    .iter()
    .map(|s| format!("{s}\n"))
    .collect::<String>();
    let spec = rig.spec(&script);
    let mut h = ClaudeDriver::default()
        .with_shutdown_grace(Duration::from_millis(200))
        .launch(spec)
        .await
        .unwrap();
    loop {
        if matches!(next(&mut h).await, HarnessEvent::Stderr { line } if line == "boom") {
            break;
        }
    }
    let started = std::time::Instant::now();
    h.control.shutdown().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    let rest = until(&mut h, |e| matches!(e, HarnessEvent::Exited { .. })).await;
    assert_eq!(rest.last(), Some(&HarnessEvent::Exited { code: None }));
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    assert!(text.contains(r#""dir":"err""#));
    assert!(matches!(
        h.control.send("x").await,
        Err(so_harness::DriverError::Closed)
    ));
}

#[test]
fn join_command_uses_wrapper() {
    let j = JoinInfo {
        account_name: "claude-acc1".into(),
        native_session_id: "abc".into(),
        endpoint: None,
    };
    assert_eq!(
        ClaudeDriver::default().join_command(&j),
        ["claude-acc1", "--resume", "abc"]
    );
}
