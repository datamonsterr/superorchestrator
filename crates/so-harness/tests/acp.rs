#![allow(clippy::unwrap_used)]
use serde_json::{Value, json};
use so_accounts::Launch;
use so_core::{AccountFile, ErrorKind, Harness};
use so_harness::{
    Driver, DriverConfig, DriverError, HarnessEvent, LaunchSpec, McpServerSpec, RunHandle,
    acp::AcpDriver,
};
use std::{path::PathBuf, time::Duration};

struct Rig {
    dir: tempfile::TempDir,
}
impl Rig {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn spec(&self, mode: &str, harness: Harness) -> LaunchSpec {
        let bin = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("fake-acp");
        LaunchSpec {
            run_id: "test".into(),
            run_dir: self.dir.path().join("run"),
            cwd: self.dir.path().into(),
            account: AccountFile::new("acct", harness, "/tmp/acct", harness.binary()).unwrap(),
            launch: Launch {
                program: bin,
                pass_env: vec![],
                vars: vec![
                    ("FAKE_ACP_MODE".into(), mode.into()),
                    (
                        "FAKE_ACP_LOG".into(),
                        self.dir.path().join("log").display().to_string(),
                    ),
                ],
                argv_prefix: vec![],
                extra_args: vec![],
            },
            model: None,
            effort: None,
            prompt: "hello".into(),
            system_addendum: "instructions".into(),
            mcp: McpServerSpec {
                command: PathBuf::from("/mcp"),
                args: vec!["serve".into()],
                env: vec![("TOKEN".into(), "secret".into())],
            },
            output_schema: None,
            max_turns: None,
            resume: None,
            fork: false,
            agent: Default::default(),
        }
    }
    fn log(&self) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join("log"))
            .unwrap()
            .lines()
            .map(|s| serde_json::from_str(s).unwrap())
            .collect()
    }
}
fn driver(h: Harness) -> AcpDriver {
    AcpDriver::new(h, DriverConfig::default())
}
async fn turn(h: &mut RunHandle) -> Vec<HarnessEvent> {
    let mut events = vec![];
    loop {
        let e = tokio::time::timeout(Duration::from_secs(5), h.events.recv())
            .await
            .unwrap()
            .unwrap();
        let done = matches!(e, HarnessEvent::TurnFinished { .. });
        events.push(e);
        if done {
            return events;
        }
    }
}
#[tokio::test]
async fn session_mapping_permissions_and_idle_send() {
    let rig = Rig::new();
    let mut h = driver(Harness::Copilot)
        .launch(rig.spec("permission", Harness::Copilot))
        .await
        .unwrap();
    assert_eq!(h.native_session_id.as_deref(), Some("native-1"));
    let events = turn(&mut h).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HarnessEvent::SessionStarted { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e,HarnessEvent::AssistantMessage{text} if text=="hello world"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e,HarnessEvent::ToolCallStarted{name} if name=="Read"))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e,HarnessEvent::ToolCallFinished{name,ok:true} if name=="Read"))
    );
    h.control.send("second").await.unwrap();
    turn(&mut h).await;
    h.control.shutdown(Duration::from_secs(1)).await.unwrap();
    let log = rig.log();
    assert!(
        log.iter()
            .any(|v| v["id"] == 700 && v["result"]["outcome"]["optionId"] == "always")
    );
    assert!(
        log.iter()
            .any(|v| v["id"] == 701 && v["error"]["code"] == -32601)
    );
    let new = log.iter().find(|v| v["method"] == "session/new").unwrap();
    assert_eq!(new["params"]["mcpServers"][0]["env"][0]["value"], "secret");
    assert!(
        log.iter().any(
            |v| v["method"] == "session/prompt" && v["params"]["prompt"][0]["text"] == "second"
        )
    );
}
#[tokio::test]
async fn resume_and_config_model() {
    let rig = Rig::new();
    let mut spec = rig.spec("", Harness::Kimi);
    spec.resume = Some("native-1".into());
    spec.model = Some("test-model".into());
    let mut h = driver(Harness::Kimi).launch(spec).await.unwrap();
    let ev = turn(&mut h).await;
    assert!(
        !ev.iter().any(
            |e| matches!(e,HarnessEvent::AssistantMessage{text} if text.contains("old history"))
        )
    );
    h.control.shutdown(Duration::from_secs(1)).await.unwrap();
    let log = rig.log();
    assert!(log.iter().any(|v| v["method"] == "session/load"));
    assert!(log.iter().any(
        |v| v["method"] == "session/set_config_option" && v["params"]["value"] == "test-model"
    ));
}
#[tokio::test]
async fn structured_retries_and_failure() {
    for (mode, ok, count) in [("structured", true, 2), ("invalid", false, 6)] {
        let rig = Rig::new();
        let mut spec = rig.spec(mode, Harness::Copilot);
        spec.output_schema = Some(json!({"type":"object","required":["ok"]}));
        let mut h = driver(Harness::Copilot).launch(spec).await.unwrap();
        let ev = turn(&mut h).await;
        assert!(ev.iter().any(|e| matches!(e,HarnessEvent::TurnFinished{structured,is_error,..} if *is_error != ok && structured.is_some()==ok)));
        h.control.shutdown(Duration::from_secs(1)).await.unwrap();
        assert_eq!(
            rig.log()
                .iter()
                .filter(|v| v["method"] == "session/prompt")
                .count(),
            count
        );
    }
}
#[tokio::test]
async fn errors_and_cancel_queue() {
    for (mode, kind) in [
        ("auth", ErrorKind::Auth),
        ("quota", ErrorKind::Quota),
        ("refusal", ErrorKind::Refused),
    ] {
        let rig = Rig::new();
        let mut h = driver(Harness::Copilot)
            .launch(rig.spec(mode, Harness::Copilot))
            .await
            .unwrap();
        let ev = turn(&mut h).await;
        assert!(ev.iter().any(|e| matches!(e,HarnessEvent::TurnFinished{is_error:true,error_kind:Some(k),..} if *k==kind)));
        h.control.shutdown(Duration::from_secs(1)).await.unwrap();
    }
    let rig = Rig::new();
    let mut h = driver(Harness::Copilot)
        .launch(rig.spec("slow", Harness::Copilot))
        .await
        .unwrap();
    h.control.send("queued").await.unwrap();
    h.control.interrupt().await.unwrap();
    let ev = turn(&mut h).await;
    assert!(ev.iter().any(|e| matches!(
        e,
        HarnessEvent::TurnFinished {
            is_error: false,
            ..
        }
    )));
    turn(&mut h).await;
    h.control.shutdown(Duration::from_secs(1)).await.unwrap();
    assert!(
        rig.log().iter().any(
            |v| v["method"] == "session/prompt" && v["params"]["prompt"][0]["text"] == "queued"
        )
    );
    let rig = Rig::new();
    assert!(matches!(
        driver(Harness::Copilot)
            .launch(rig.spec("exit", Harness::Copilot))
            .await,
        Err(DriverError::Exited {
            kind: ErrorKind::Quota,
            ..
        })
    ));
    let rig = Rig::new();
    assert!(matches!(
        driver(Harness::Copilot)
            .launch(rig.spec("exit_silent", Harness::Copilot))
            .await,
        Err(DriverError::Exited {
            kind: ErrorKind::Other,
            ..
        })
    ));
}

#[tokio::test]
#[ignore = "requires an installed, signed-in ACP CLI"]
async fn real_acp_cli() {
    let harness: Harness = std::env::var("SO_ACP_REAL_HARNESS")
        .unwrap()
        .parse()
        .unwrap();
    let home = std::env::var("SO_ACP_REAL_HOME").unwrap();
    let host = so_accounts::HostEnv::detect().unwrap();
    let account = AccountFile::new("real-acp", harness, &home, harness.binary()).unwrap();
    let launch = so_accounts::build_launch(&account, &host).unwrap();
    let rig = Rig::new();
    let mut spec = rig.spec("", harness);
    spec.account = account;
    spec.launch = launch;
    spec.model = std::env::var("SO_ACP_REAL_MODEL").ok();
    spec.prompt = "Reply with exactly the word pong.".into();
    spec.system_addendum.clear();
    spec.mcp = McpServerSpec {
        command: "/bin/true".into(),
        ..Default::default()
    };
    let mut h = driver(harness).launch(spec).await.unwrap();
    let events = turn(&mut h).await;
    for event in &events {
        println!("{event:?}");
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HarnessEvent::SessionStarted { .. }))
    );
    assert!(events.iter().any(|e| matches!(e,HarnessEvent::TurnFinished{is_error:false,result_text:Some(text),..} if text.to_lowercase().contains("pong"))));
    h.control.shutdown(Duration::from_secs(5)).await.unwrap();
}

#[test]
fn argv_and_join_for_each_harness() {
    use so_harness::JoinInfo;
    let rig = Rig::new();
    for (h, expected, join) in [
        (
            Harness::Copilot,
            vec![
                "--acp",
                "--allow-all-tools",
                "--allow-all-paths",
                "--model",
                "test-model",
                "--reasoning-effort",
                "high",
            ],
            vec!["acct", "--resume=native-1"],
        ),
        (
            Harness::Gemini,
            vec!["--acp", "--approval-mode", "yolo", "-m", "test-model"],
            vec!["acct", "--resume", "native-1"],
        ),
        (
            Harness::Kimi,
            vec!["acp"],
            vec!["acct", "--session", "native-1"],
        ),
        (
            Harness::Cursor,
            vec!["acp", "--force", "--approve-mcps", "--model", "test-model"],
            vec!["acct", "--resume", "native-1"],
        ),
        (
            Harness::Qwen,
            vec!["--acp", "--approval-mode", "yolo", "-m", "test-model"],
            vec!["acct", "--resume", "native-1"],
        ),
    ] {
        let mut spec = rig.spec("", h);
        spec.model = Some("test-model".into());
        if h == Harness::Copilot {
            spec.effort = Some("high".into());
        }
        assert_eq!(so_harness::acp::build_args(h, &spec), expected);
        let info = JoinInfo {
            account_name: "acct".into(),
            native_session_id: "native-1".into(),
            endpoint: None,
            cwd: rig.dir.path().into(),
        };
        let cmd = driver(h).join_command(&info);
        assert_eq!(cmd.argv, join);
        assert_eq!(cmd.cwd, info.cwd);
        assert!(cmd.env.is_empty());
    }
}
