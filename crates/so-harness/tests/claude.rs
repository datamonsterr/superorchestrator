#![allow(clippy::unwrap_used)]
//! Claude driver against the scripted fake harness.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    Channel, ClaudeDriver, Driver, DriverConfig, HarnessEvent, JoinInfo, LaunchSpec, McpServerSpec,
    RunHandle, ShutdownOutcome,
};

const TRANSCRIPT: &str = include_str!("fixtures/claude/transcript.jsonl");
const INTERRUPT: &str = include_str!("fixtures/claude/interrupt.jsonl");
const PROMPT: &str =
    "Create a file hello.txt containing the word hi, then reply with exactly: done";
const SECOND: &str = "What is your codename? One word.";

/// Turns a recorded fixture into a fake-harness script: `out` lines become
/// stdin expectations, `in` lines become stdout, notes are skipped. The
/// fixture's interrupt request id is rewritten to the one the driver sends.
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
            "in" => {
                let fixed = msg.to_string().replace("req-int-1", "so-int-1");
                steps.push(json!({"stdout": serde_json::from_str::<Value>(&fixed).unwrap()}));
            }
            _ => {}
        }
    }
    script(&steps)
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

fn driver() -> ClaudeDriver {
    ClaudeDriver::new(DriverConfig {
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

async fn wait_stderr(h: &mut RunHandle, want: &str) {
    loop {
        if let HarnessEvent::Raw {
            channel: Channel::Stderr,
            line,
            ..
        } = next(h).await
            && line == want
        {
            return;
        }
    }
}

fn is_exit(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::Exited { .. })
}

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

/// Gone = no such process, or only a zombie awaiting reap.
#[cfg(target_os = "linux")]
fn process_gone(pid: u32) -> bool {
    let p = rustix::process::Pid::from_raw(pid as i32).unwrap();
    if rustix::process::test_kill_process(p).is_err() {
        return true;
    }
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')))
        .unwrap_or(true)
}

#[cfg(target_os = "linux")]
async fn assert_gets_killed(pid: u32) {
    for _ in 0..80 {
        if process_gone(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("process {pid} survived");
}

fn grandchild_pid(rig: &Rig) -> u32 {
    rig.log()
        .iter()
        .find_map(|l| l["grandchild"].as_u64())
        .unwrap() as u32
}

fn is_turn(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::TurnFinished { .. })
}

#[tokio::test]
async fn normal_run_two_turns_and_transcript() {
    let rig = Rig::new();
    let mut h = driver()
        .launch(rig.spec(&script_from_fixture(TRANSCRIPT)))
        .await
        .unwrap();
    assert!(h.native_session_id.is_some());
    assert_eq!(h.join.account_name, "claude-acc1");
    assert_eq!(
        Some(&h.join.native_session_id),
        h.native_session_id.as_ref()
    );

    let first = until(&mut h, is_turn).await;
    use HarnessEvent::*;
    assert!(matches!(
        &first[..],
        [
            SessionStarted { .. },
            TurnStarted,
            ToolCallStarted { .. },
            ToolCallFinished { .. },
            AssistantMessage { .. },
            Usage { .. },
            TurnFinished { .. }
        ]
    ));
    assert_eq!(
        first[2],
        ToolCallStarted {
            name: "Bash".into()
        }
    );
    assert_eq!(
        first[3],
        ToolCallFinished {
            name: "Bash".into(),
            ok: true
        }
    );
    assert!(matches!(&first[4], AssistantMessage { text } if text == "done"));

    h.control.send(SECOND).await.unwrap();
    let second = until(&mut h, is_turn).await;
    assert!(matches!(&second[0], TurnStarted));
    assert!(
        second
            .iter()
            .any(|e| matches!(e, AssistantMessage { text } if text == "PELICAN"))
    );
    assert!(matches!(
        second.last(),
        Some(TurnFinished {
            is_error: false,
            ..
        })
    ));

    let out = h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    assert_eq!(out, ShutdownOutcome::Exited(Some(0)));
    let rest = until(&mut h, is_exit).await;
    assert_eq!(rest.last(), Some(&Exited { code: Some(0) }));

    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    let entries: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(entries[0]["dir"], "out");
    assert_eq!(entries[0]["channel"], "stdin");
    assert!(entries[0]["raw"].as_str().unwrap().contains(PROMPT));
    assert!(entries.iter().any(|e| e["dir"] == "in"
        && e["event"][0]["type"] == "session_started"
        && e["event"][1]["type"] == "turn_started"));
    assert_eq!(entries.iter().filter(|e| e["dir"] == "out").count(), 2);
}

#[tokio::test]
async fn raw_offsets_point_into_transcript() {
    let rig = Rig::new();
    let mut h = driver()
        .launch(rig.spec(&script_from_fixture(TRANSCRIPT)))
        .await
        .unwrap();
    until(&mut h, is_turn).await;
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    let mut raws = Vec::new();
    loop {
        match next(&mut h).await {
            HarnessEvent::Raw {
                channel,
                line,
                offset,
            } => raws.push((channel, line, offset.unwrap())),
            HarnessEvent::Exited { .. } => break,
            _ => {}
        }
    }
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    // First events were consumed by `until`; the rest still map to entries.
    assert!(!raws.is_empty());
    for (channel, line, off) in raws {
        let entry: Value =
            serde_json::from_str(text[off as usize..].lines().next().unwrap()).unwrap();
        assert_eq!(entry["raw"], line.as_str());
        let want = match channel {
            Channel::Stdout => "stdout",
            Channel::Stderr => "stderr",
            _ => "stdin",
        };
        assert_eq!(entry["channel"], want);
    }
}

#[tokio::test]
async fn argv_modes_and_run_files() {
    let rig = Rig::new();
    let mut h = driver()
        .launch(rig.spec(&script_from_fixture(TRANSCRIPT)))
        .await
        .unwrap();
    let sid = h.native_session_id.clone().unwrap();
    until(&mut h, is_turn).await;
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();

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

    let keys = log[0]["env_keys"].as_array().unwrap();
    assert!(keys.contains(&json!("CLAUDE_CONFIG_DIR")));
    assert!(!keys.contains(&json!("SO_SECRET_TOKEN")));
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

    assert_eq!(mode(&run), 0o700);
    for f in ["mcp.json", "system.md", "transcript.jsonl"] {
        assert_eq!(mode(&run.join(f)), 0o600, "{f}");
    }
}

#[tokio::test]
async fn resume_uses_resume_flag_and_schema() {
    let rig = Rig::new();
    let mut spec = rig.spec("");
    spec.resume = Some("11111111-2222-7333-8444-555555555555".into());
    spec.output_schema = Some(json!({"type": "object"}));
    let mut h = driver().launch(spec).await.unwrap();
    assert_eq!(
        h.native_session_id.as_deref(),
        Some("11111111-2222-7333-8444-555555555555")
    );
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    until(&mut h, is_exit).await;
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
async fn interrupt_is_acknowledged_from_a_cloned_handle() {
    let rig = Rig::new();
    let mut spec = rig.spec(&script_from_fixture(INTERRUPT));
    spec.prompt = "Run the shell command `sleep 60` and then say done.".into();
    let mut h = driver().launch(spec).await.unwrap();
    until(&mut h, |e| {
        matches!(e, HarnessEvent::ToolCallStarted { .. })
    })
    .await;
    let c2 = h.control.clone();
    tokio::spawn(async move { c2.interrupt().await })
        .await
        .unwrap()
        .unwrap();
    let rest = until(&mut h, is_turn).await;
    assert!(rest.contains(&HarnessEvent::ToolCallFinished {
        name: "Bash".into(),
        ok: false
    }));
    let log = rig.log();
    let line: Value = serde_json::from_str(log[2]["stdin"].as_str().unwrap()).unwrap();
    assert_eq!(line["type"], "control_request");
    assert_eq!(line["request"]["subtype"], "interrupt");
    assert_eq!(line["request_id"], "so-int-1");
    assert!(h.control.pid().is_some());
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test]
async fn unacknowledged_interrupt_errors() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"expect_stdin_contains": "interrupt"}),
        json!({"sleep_ms": 60000}),
    ]);
    let h = driver().launch(rig.spec(&s)).await.unwrap();
    let err = h.control.interrupt().await.unwrap_err();
    assert!(err.to_string().contains("not acknowledged"), "{err}");
    h.control.shutdown(Duration::ZERO).await.unwrap();
}

#[tokio::test]
async fn failed_turn_carries_kind_without_duplicate_error() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"type": "result", "subtype": "success", "is_error": true,
            "result": "Claude AI usage limit reached|1792669200", "usage": {}}}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    let evs = until(&mut h, is_turn).await;
    assert!(matches!(
        evs.last(),
        Some(HarnessEvent::TurnFinished {
            is_error: true,
            error_kind: Some(ErrorKind::Quota),
            error_text: Some(t),
            ..
        }) if t.contains("usage limit")
    ));
    assert!(!evs.iter().any(|e| matches!(e, HarnessEvent::Error { .. })));
    assert_eq!(
        driver().classify_error("usage limit reached"),
        ErrorKind::Quota
    );
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test]
async fn rejected_rate_limit_event_is_quota() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"type": "rate_limit_event", "rate_limit_info": {
            "status": "rejected", "rateLimitType": "five_hour"}}}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    let evs = until(&mut h, |e| matches!(e, HarnessEvent::Error { .. })).await;
    assert!(matches!(
        evs.last(),
        Some(HarnessEvent::Error { kind: ErrorKind::Quota, message }) if message.contains("rejected")
    ));
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test]
async fn shutdown_kills_after_deadline_and_captures_stderr() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stderr": "boom"}),
        json!({"sleep_ms": 60000}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    wait_stderr(&mut h, "boom").await;
    let started = std::time::Instant::now();
    let out = h
        .control
        .shutdown(Duration::from_millis(200))
        .await
        .unwrap();
    assert_eq!(out, ShutdownOutcome::Killed);
    assert!(started.elapsed() < Duration::from_secs(5));
    let rest = until(&mut h, is_exit).await;
    assert_eq!(rest.last(), Some(&HarnessEvent::Exited { code: None }));
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    assert!(text.contains(r#""dir":"err""#) && text.contains(r#""channel":"stderr""#));
    assert!(matches!(
        h.control.send("x").await,
        Err(so_harness::DriverError::Closed)
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn shutdown_kills_grandchildren() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"spawn_sleep": 60}),
        json!({"stderr": "ready"}),
        json!({"sleep_ms": 60000}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    wait_stderr(&mut h, "ready").await;
    let pid = grandchild_pid(&rig);
    assert!(!process_gone(pid));
    h.control
        .shutdown(Duration::from_millis(200))
        .await
        .unwrap();
    assert_gets_killed(pid).await;
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn dropping_control_after_clean_exit_kills_leftover_group() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"spawn_sleep": 60}),
        json!({"exit": 0}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    until(&mut h, is_exit).await;
    let pid = grandchild_pid(&rig);
    assert!(!process_gone(pid), "grandchild should outlive the leader");
    drop(h);
    assert_gets_killed(pid).await;
}

#[tokio::test]
async fn undrained_events_never_block_shutdown() {
    let rig = Rig::new();
    let mut steps = vec![json!({"expect_stdin_contains": PROMPT})];
    for i in 0..3000 {
        steps.push(
            json!({"stdout": {"type": "assistant", "message": {"content": [
            {"type": "text", "text": format!("line {i}")}]}}}),
        );
        steps.push(json!({"stderr": format!("err {i}")}));
    }
    steps.push(json!({"stderr": "flooded"}));
    steps.push(json!({"sleep_ms": 60000}));
    let mut h = driver().launch(rig.spec(&script(&steps))).await.unwrap();
    // Do not read events; the process and transcript must keep going.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let text = std::fs::read_to_string(rig.run_dir().join("transcript.jsonl")).unwrap();
    assert!(text.contains("flooded"));
    let out = h
        .control
        .shutdown(Duration::from_millis(100))
        .await
        .unwrap();
    assert_eq!(out, ShutdownOutcome::Killed);
    let mut n = 0;
    loop {
        match next(&mut h).await {
            HarnessEvent::AssistantMessage { .. } => n += 1,
            HarnessEvent::Exited { .. } => break,
            _ => {}
        }
    }
    assert_eq!(n, 3000);
}

#[tokio::test]
async fn oversized_line_is_truncated_and_stream_continues() {
    let rig = Rig::new();
    let huge = "x".repeat(5 * 1024 * 1024);
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": huge}),
        json!({"stdout": {"type": "assistant", "message": {"content": [
            {"type": "text", "text": "after"}]}}}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    let mut saw_truncated = false;
    loop {
        match next(&mut h).await {
            HarnessEvent::Raw { line, .. }
                if line.ends_with(so_harness::process::TRUNCATED_MARK) =>
            {
                assert_eq!(
                    line.len(),
                    so_harness::process::MAX_LINE_BYTES + so_harness::process::TRUNCATED_MARK.len()
                );
                saw_truncated = true;
            }
            HarnessEvent::AssistantMessage { text } => {
                assert_eq!(text, "after");
                break;
            }
            _ => {}
        }
    }
    assert!(saw_truncated);
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[test]
fn join_command_uses_wrapper_and_cwd() {
    let j = JoinInfo {
        account_name: "claude-acc1".into(),
        native_session_id: "abc".into(),
        endpoint: None,
        cwd: "/work".into(),
    };
    let c = driver().join_command(&j);
    assert_eq!(c.argv, ["claude-acc1", "--resume", "abc"]);
    assert!(c.env.is_empty());
    assert_eq!(c.cwd, PathBuf::from("/work"));
}
