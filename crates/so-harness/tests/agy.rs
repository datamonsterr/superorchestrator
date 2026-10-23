#![allow(clippy::unwrap_used)]
//! agy driver against the scripted fake harness (and small shell scripts for
//! signal behaviour).

use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    AgyDriver, AnyDriver, Driver, DriverConfig, HarnessEvent, JoinInfo, LaunchSpec, McpServerSpec,
    RunHandle, ShutdownOutcome,
};

const TRANSCRIPT: &str = include_str!("fixtures/agy/transcript.jsonl");
const PROMPT: &str =
    "Create a file hello.txt containing the word hi, then reply with exactly: done";
const SECOND: &str =
    "Which MCP servers or tools named projmcp do you see? Answer in one short sentence.";
const CONVERSATION: &str = "c0f1599d-37b9-4c82-be74-b28e7c59be25";

/// Turns the recorded fixture into a fake-harness script: `out` lines become
/// stdin expectations, `in` lines become stdout, notes are skipped.
fn script_from_fixture() -> String {
    let mut steps = Vec::new();
    for l in TRANSCRIPT.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        let msg = &v["msg"];
        match v["dir"].as_str().unwrap() {
            "out" => steps.push(json!({"expect_stdin_contains": msg["message"]["content"]})),
            "in" => steps.push(json!({"stdout": msg})),
            _ => {}
        }
    }
    script(&steps)
}

fn script(steps: &[Value]) -> String {
    steps.iter().map(|s| format!("{s}\n")).collect()
}

/// Reads the first message and announces the conversation, then idles.
fn init_script() -> String {
    script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"event": "init", "conversation_id": "c1", "init": {}}}),
    ])
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Rig {
    dir: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let rig = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        // Operator-managed accounts share a symlinked config root; no `projects`.
        let g = rig.account_gemini();
        for d in ["antigravity-cli", "plugins", "skills"] {
            std::fs::create_dir_all(g.join(d)).unwrap();
        }
        std::fs::write(g.join("antigravity-cli/state.db"), "db").unwrap();
        let real = rig.dir.path().join("real-config");
        std::fs::create_dir_all(real.join("sub")).unwrap();
        std::fs::write(real.join("settings.json"), r#"{"theme":"dark"}"#).unwrap();
        std::fs::write(
            real.join("mcp_config.json"),
            r#"{"mcpServers":{"other":{"command":"/bin/other","args":["x"],"env":{},"disabled":false}},"extra":1}"#,
        )
        .unwrap();
        let _sock = UnixListener::bind(real.join("agy.sock")).unwrap();
        std::os::unix::fs::symlink(&real, g.join("config")).unwrap();
        rig
    }

    fn account_gemini(&self) -> PathBuf {
        self.dir.path().join("acct/.gemini")
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
            account: AccountFile::new("agy-acc1", Harness::Agy, "~/acct", "agy").unwrap(),
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
                ],
                argv_prefix: vec![],
                extra_args: vec![format!("--gemini_dir={}", self.account_gemini().display())],
            },
            model: Some("gemini-3.8-flash-low".into()),
            effort: Some("low".into()),
            prompt: PROMPT.into(),
            system_addendum: "You are a worker.".into(),
            mcp: McpServerSpec {
                command: PathBuf::from("/bin/so-mcp"),
                args: vec!["mcp".into()],
                env: vec![("SUPERORCHESTRATOR_RUN_ID".into(), "run-1".into())],
            },
            output_schema: None,
            max_turns: Some(8),
            resume: None,
            fork: false,
            agent: Default::default(),
        }
    }
}

fn driver() -> AgyDriver {
    AgyDriver::with_bwrap(
        DriverConfig {
            shutdown_grace: Duration::from_millis(500),
        },
        None,
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

fn is_exit(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::Exited { .. })
}

fn is_turn(e: &HarnessEvent) -> bool {
    matches!(e, HarnessEvent::TurnFinished { .. })
}

fn mode(p: &Path) -> u32 {
    std::fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
}

fn argv(rig: &Rig) -> Vec<String> {
    rig.log()[0]["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn two_turns_events_and_first_message() {
    let rig = Rig::new();
    let mut h = driver()
        .launch(rig.spec(&script_from_fixture()))
        .await
        .unwrap();
    assert_eq!(h.native_session_id.as_deref(), Some(CONVERSATION));
    assert_eq!(h.join.native_session_id, CONVERSATION);

    let mut events = until(&mut h, is_turn).await;
    h.control.send(SECOND).await.unwrap();
    events.extend(until(&mut h, is_turn).await);
    assert!(matches!(
        h.control.shutdown(Duration::from_secs(5)).await.unwrap(),
        ShutdownOutcome::Exited(Some(0))
    ));
    events.extend(until(&mut h, is_exit).await);

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
            Exited { .. } => "exit",
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
            "turn_end",
            "exit"
        ]
    );
    assert_eq!(
        events[0],
        SessionStarted {
            native_session_id: CONVERSATION.into()
        }
    );
    assert!(matches!(&events[4], AssistantMessage { text } if text == "done"));

    // Stdin: protocol shape, addendum prepended to the first message only.
    let stdin: Vec<Value> = rig
        .log()
        .iter()
        .filter_map(|l| l["stdin"].as_str())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(stdin.len(), 2);
    assert_eq!(stdin[0]["event"], "user");
    let first = stdin[0]["message"]["content"].as_str().unwrap();
    assert!(first.starts_with("<superorchestrator-system-instructions>\nYou are a worker.\n"));
    assert!(first.ends_with(PROMPT));
    assert_eq!(stdin[1]["message"]["content"], SECOND);
}

#[tokio::test]
async fn argv_has_no_prompt_and_launches_directly_without_bwrap() {
    let rig = Rig::new();
    let mut h = driver().launch(rig.spec(&init_script())).await.unwrap();
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    until(&mut h, is_exit).await;
    let run = rig.run_dir();
    assert_eq!(
        argv(&rig),
        [
            &format!("--gemini_dir={}/gemini", run.display()),
            "-p=",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--dangerously-skip-permissions",
            "--model",
            "gemini-3.8-flash-low",
            "--effort",
            "low",
        ]
    );
    let joined = argv(&rig).join(" ");
    assert!(!joined.contains("worker") && !joined.contains("hello.txt"));
    // MCP env values stay out of the process environment.
    let keys = rig.log()[0]["env_keys"].to_string();
    assert!(!keys.contains("SUPERORCHESTRATOR_RUN_ID"));
}

#[tokio::test]
async fn bwrap_wraps_the_launch_when_available() {
    let rig = Rig::new();
    let stub = rig.dir.path().join("bwrap-stub");
    executable(
        &stub,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$BWRAP_LOG\"\nwhile [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n",
    );
    let mut spec = rig.spec(&init_script());
    let log = rig.dir.path().join("bwrap.log");
    spec.launch
        .vars
        .push(("BWRAP_LOG".into(), log.display().to_string()));
    let d = AgyDriver::with_bwrap(DriverConfig::default(), Some(stub));
    let mut h = d.launch(spec).await.unwrap();
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    until(&mut h, is_exit).await;
    let lines: Vec<String> = std::fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(String::from)
        .collect();
    let fake = env!("CARGO_BIN_EXE_fake-harness");
    assert_eq!(
        &lines[..10],
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
            fake,
        ]
    );
    // No bind of the account over ~/.gemini (ADR-0005), and the agy argv follows.
    assert_eq!(lines.iter().filter(|l| *l == "--bind").count(), 1);
    assert_eq!(
        lines[10],
        format!("--gemini_dir={}/gemini", rig.run_dir().display())
    );
    assert_eq!(argv(&rig)[1], "-p=");
}

#[tokio::test]
async fn per_run_gemini_dir_contents_modes_and_mcp_merge() {
    let rig = Rig::new();
    let mut h = driver().launch(rig.spec(&init_script())).await.unwrap();
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    until(&mut h, is_exit).await;

    let g = rig.run_dir().join("gemini");
    let acct = rig.account_gemini();
    assert_eq!(mode(&g), 0o700);
    for d in ["antigravity-cli", "plugins", "skills"] {
        assert_eq!(std::fs::read_link(g.join(d)).unwrap(), acct.join(d), "{d}");
    }
    assert!(
        !g.join("projects").exists(),
        "missing entries are not linked"
    );
    assert!(std::fs::read_to_string(g.join("antigravity-cli/state.db")).is_ok());

    let config = g.join("config");
    assert!(!std::fs::symlink_metadata(&config).unwrap().is_symlink());
    assert_eq!(mode(&config), 0o700);
    let mut names: Vec<String> = std::fs::read_dir(&config)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["mcp_config.json", "settings.json"], "files only");
    for n in &names {
        assert_eq!(mode(&config.join(n)), 0o600, "{n}");
    }
    assert_eq!(
        std::fs::read_to_string(config.join("settings.json")).unwrap(),
        r#"{"theme":"dark"}"#
    );

    let mcp: Value =
        serde_json::from_slice(&std::fs::read(config.join("mcp_config.json")).unwrap()).unwrap();
    assert_eq!(mcp["extra"], 1, "unrelated keys survive");
    assert_eq!(mcp["mcpServers"]["other"]["command"], "/bin/other");
    assert_eq!(
        mcp["mcpServers"]["superorchestrator"],
        json!({
            "command": "/bin/so-mcp",
            "args": ["mcp"],
            "env": {"SUPERORCHESTRATOR_RUN_ID": "run-1"},
            "disabled": false,
        })
    );
    // The account's own config is untouched.
    let original =
        std::fs::read_to_string(rig.account_gemini().join("config/mcp_config.json")).unwrap();
    assert!(!original.contains("superorchestrator"));
}

#[tokio::test]
async fn resume_uses_conversation_flag_and_schema() {
    let rig = Rig::new();
    let mut spec = rig.spec(&init_script());
    spec.resume = Some("c0f1599d-37b9-4c82-be74-b28e7c59be25".into());
    spec.output_schema = Some(json!({"type": "object"}));
    let mut h = driver().launch(spec).await.unwrap();
    assert_eq!(
        h.native_session_id.as_deref(),
        Some("c0f1599d-37b9-4c82-be74-b28e7c59be25")
    );
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
    until(&mut h, is_exit).await;
    let a = argv(&rig);
    let i = a.iter().position(|x| x == "--conversation").unwrap();
    assert_eq!(a[i + 1], "c0f1599d-37b9-4c82-be74-b28e7c59be25");
    let j = a.iter().position(|x| x == "--json-schema").unwrap();
    assert_eq!(a[j + 1], r#"{"type":"object"}"#);
}

#[tokio::test]
async fn quota_error_result_is_classified() {
    let rig = Rig::new();
    let text = "Individual quota reached. Please upgrade your subscription. Resets in 40h19m40s.";
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"event": "init", "conversation_id": "c1", "init": {}}}),
        json!({"stderr": format!("error: {text}")}),
        json!({"stdout": {"event": "result", "result": {
            "conversation_id": "c1", "status": "ERROR", "error": text}}}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    let events = until(&mut h, is_turn).await;
    assert!(
        matches!(events.last(), Some(HarnessEvent::TurnFinished {
            is_error: true, error_kind: Some(ErrorKind::Quota), error_text: Some(t), ..
        }) if t == text),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, HarnessEvent::Error { .. }))
    );
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[tokio::test]
async fn shutdown_kills_the_process_group_without_bwrap() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stdout": {"event": "init", "conversation_id": "c1", "init": {}}}),
        json!({"spawn_sleep": 60}),
        json!({"sleep_ms": 60000}),
    ]);
    let mut h = driver().launch(rig.spec(&s)).await.unwrap();
    for _ in 0..100 {
        if rig.log().iter().any(|l| l["grandchild"].is_u64()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let pid = rig
        .log()
        .iter()
        .find_map(|l| l["grandchild"].as_u64())
        .unwrap() as i32;
    let outcome = h.control.shutdown(Duration::ZERO).await.unwrap();
    assert_eq!(outcome, ShutdownOutcome::Killed);
    until(&mut h, is_exit).await;
    let p = rustix::process::Pid::from_raw(pid).unwrap();
    for _ in 0..80 {
        // Linux also reports an unreaped zombie as gone; /proc is absent elsewhere,
        // where kill(0) alone decides.
        let gone = rustix::process::test_kill_process(p).is_err()
            || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|s| s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')))
                .unwrap_or(false);
        if gone {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("grandchild {pid} survived");
}

#[tokio::test]
async fn interrupt_sends_sigint_and_the_turn_ends_with_an_error_result() {
    let rig = Rig::new();
    // Like agy: SIGINT ends the turn with an ERROR result and exit 1.
    let prog = rig.dir.path().join("fake-agy.sh");
    executable(
        &prog,
        r#"#!/bin/sh
trap 'echo "{\"event\":\"result\",\"result\":{\"conversation_id\":\"c1\",\"status\":\"ERROR\",\"error\":\"interrupted\"}}"; exit 1' INT
read -r line
echo '{"event":"init","conversation_id":"c1","init":{}}'
while :; do sleep 0.05; done
"#,
    );
    let mut spec = rig.spec(&init_script());
    spec.launch.program = prog;
    let mut h = driver().launch(spec).await.unwrap();
    h.control.interrupt().await.unwrap();
    let events = until(&mut h, is_exit).await;
    assert!(events.iter().any(|e| matches!(
        e,
        HarnessEvent::TurnFinished {
            is_error: true,
            error_text: Some(t),
            ..
        } if t == "interrupted"
    )));
    assert!(matches!(
        events.last(),
        Some(HarnessEvent::Exited { code: Some(1) })
    ));
    // The process is gone: further control fails.
    assert!(h.control.interrupt().await.is_err());
}

#[tokio::test]
async fn launch_without_gemini_dir_fails() {
    let rig = Rig::new();
    let mut spec = rig.spec(&init_script());
    spec.launch.extra_args.clear();
    assert!(driver().launch(spec).await.is_err());
}

#[test]
fn any_driver_selects_agy_and_join_command_uses_conversation() {
    let d = AnyDriver::for_harness(Harness::Agy, DriverConfig::default()).unwrap();
    assert_eq!(d.harness(), Harness::Agy);
    let j = JoinInfo {
        account_name: "agy-acc1".into(),
        native_session_id: "abc".into(),
        endpoint: None,
        cwd: "/work".into(),
    };
    let c = d.join_command(&j);
    assert_eq!(c.argv, ["agy-acc1", "--conversation", "abc"]);
    assert!(c.env.is_empty());
    assert_eq!(c.cwd, PathBuf::from("/work"));
}

#[tokio::test]
async fn exit_before_init_fails_the_launch_with_the_classified_error() {
    let rig = Rig::new();
    let s = script(&[
        json!({"expect_stdin_contains": PROMPT}),
        json!({"stderr": "error: Eligibility check failed: not eligible"}),
        json!({"exit": 1}),
    ]);
    let Err(err) = driver().launch(rig.spec(&s)).await else {
        panic!("launch should fail");
    };
    let msg = err.to_string();
    assert!(
        msg.contains("exited before init") && msg.contains("Eligibility"),
        "{msg}"
    );
    assert!(msg.contains("Auth"), "{msg}");
}

#[test]
fn thinking_hint_adds_no_effort_flag() {
    let rig = Rig::new();
    let mut spec = rig.spec(&init_script());
    spec.effort = Some("thinking".into());
    let a = so_harness::agy::build_args(&spec);
    assert!(!a.iter().any(|x| x == "--effort"), "{a:?}");
    spec.effort = Some("xhigh".into());
    let a = so_harness::agy::build_args(&spec);
    assert!(a.windows(2).any(|w| w == ["--effort", "xhigh"]), "{a:?}");
}

#[tokio::test]
async fn invalid_effort_fails_before_spawn() {
    let rig = Rig::new();
    let mut spec = rig.spec(&init_script());
    spec.effort = Some("bogus".into());
    let err = driver().launch(spec).await.err().unwrap();
    assert!(
        err.to_string()
            .contains("effort `bogus` is not valid for agy (low|medium|high|xhigh|max)"),
        "{err}"
    );
}

#[test]
fn prepare_refuses_config_file_symlinks_outside_the_account() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    std::fs::remove_dir_all(account.join("config")).unwrap();
    std::fs::create_dir(account.join("config")).unwrap();
    let outside = rig.dir.path().join("secret");
    std::fs::write(&outside, "private secret").unwrap();
    std::os::unix::fs::symlink(&outside, account.join("config/settings.json")).unwrap();
    let error =
        so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(!rig.run_dir().join("gemini/config/settings.json").exists());
}

#[test]
fn prepare_refuses_writable_symlinked_config_root() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    std::fs::remove_dir_all(account.join("config")).unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("settings.json"), "private secret").unwrap();
    std::fs::set_permissions(outside.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
    std::os::unix::fs::symlink(outside.path(), account.join("config")).unwrap();
    let error =
        so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(!rig.run_dir().join("gemini/config/settings.json").exists());
}

#[test]
fn prepare_allows_owned_private_symlinked_config_root() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("settings.json"), "trusted settings").unwrap();
    std::fs::remove_dir_all(account.join("config")).unwrap();
    std::os::unix::fs::symlink(source.path(), account.join("config")).unwrap();
    let dir = so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config/settings.json")).unwrap(),
        "trusted settings"
    );
}

#[test]
fn prepare_checks_entry_symlinks_against_the_shared_canonical_root() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    let config = account.join("config").canonicalize().unwrap();
    std::os::unix::fs::symlink(config.join("settings.json"), config.join("linked.json")).unwrap();
    let dir = so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap();
    assert_eq!(
        std::fs::read(dir.join("config/linked.json")).unwrap(),
        std::fs::read(config.join("settings.json")).unwrap()
    );
    let outside = rig.dir.path().join("secret");
    std::fs::write(&outside, "private secret").unwrap();
    std::os::unix::fs::symlink(outside, config.join("escape.json")).unwrap();
    let error =
        so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(!dir.join("config/escape.json").exists());
}

#[test]
fn review_real_group_writable_config_inside_account_is_allowed() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    std::fs::remove_file(account.join("config")).unwrap();
    std::fs::create_dir(account.join("config")).unwrap();
    std::fs::set_permissions(
        account.join("config"),
        std::fs::Permissions::from_mode(0o775),
    )
    .unwrap();
    std::fs::write(account.join("config/settings.json"), "local settings").unwrap();
    let dir = so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config/settings.json")).unwrap(),
        "local settings"
    );
}

#[test]
fn review_dangling_config_entry_is_skipped() {
    let rig = Rig::new();
    let account = rig.account_gemini();
    std::os::unix::fs::symlink(
        account.join("missing"),
        account.join("config/dangling.json"),
    )
    .unwrap();
    let dir = so_harness::agy::prepare_gemini_dir(&rig.spec(&init_script()), &account).unwrap();
    assert!(!dir.join("config/dangling.json").exists());
    assert!(dir.join("config/settings.json").exists());
}
