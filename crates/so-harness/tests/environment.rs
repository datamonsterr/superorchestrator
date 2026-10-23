#![allow(clippy::unwrap_used)]
//! Run the probe in a separate test process so no test mutates the shared environment.

use std::process::Command;
use std::time::Duration;

use so_harness::{ClaudeDriver, Driver, DriverConfig, LaunchSpec, McpServerSpec};

#[test]
fn supervised_environment_isolation() {
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "supervised_environment_child",
            "--nocapture",
        ])
        .env("SO_ENV_TEST_CHILD", "1")
        .env("SO_TEST_SECRET", "must-not-inherit")
        .env("LC_SO_TEST", "allowlisted")
        .env("SO_ACCOUNT_VAR", "account")
        .env("SO_GLOBAL_VAR", "global")
        .env("SO_DRIVER_VAR", "parent")
        .env("OPENCODE_SERVER_PASSWORD", "unrelated-parent-password")
        .status()
        .unwrap();
    assert!(status.success());
}

/// Helper run only by `supervised_environment_isolation` in a fresh process
/// (`--ignored --exact`), so no test mutates this process's environment.
#[tokio::test]
#[ignore = "helper: run by supervised_environment_isolation"]
async fn supervised_environment_child() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("script.jsonl");
    let log = dir.path().join("env.jsonl");
    let checks = r#"test "$SO_DRIVER_VAR" = explicit && test "$SO_ACCOUNT_VAR" = account && test "$SO_GLOBAL_VAR" = global"#;
    let steps = format!(
        "{}\n{}\n{}\n",
        serde_json::json!({"expect_stdin_contains": "environment check"}),
        serde_json::json!({"shell": checks}),
        serde_json::json!({"exit": 0})
    );
    std::fs::write(&script, steps).unwrap();
    let mut account = so_core::AccountFile::new(
        "test-account",
        so_core::Harness::Claude,
        "~/.claude",
        env!("CARGO_BIN_EXE_fake-harness"),
    )
    .unwrap();
    account.pass_env = vec![
        "SO_ACCOUNT_VAR".into(),
        "SO_DRIVER_VAR".into(),
        "MISSING_OPT_IN".into(),
    ];
    let mut launch = so_accounts::build_launch(&account, &so_accounts::HostEnv::detect().unwrap())
        .unwrap()
        .with_default_pass_env(&["SO_GLOBAL_VAR".into()]);
    launch.vars.extend([
        ("FAKE_HARNESS_SCRIPT".into(), script.display().to_string()),
        ("FAKE_HARNESS_LOG".into(), log.display().to_string()),
        ("SO_DRIVER_VAR".into(), "explicit".into()),
    ]);
    let spec = LaunchSpec {
        run_id: "environment-test".into(),
        run_dir: dir.path().join("run"),
        cwd: dir.path().to_path_buf(),
        account,
        launch,
        model: None,
        effort: None,
        prompt: "environment check".into(),
        system_addendum: String::new(),
        mcp: McpServerSpec {
            command: "/bin/true".into(),
            args: vec![],
            env: vec![],
        },
        output_schema: None,
        max_turns: None,
        resume: None,
        fork: false,
        agent: Default::default(),
    };
    let mut process = ClaudeDriver::new(DriverConfig {
        shutdown_grace: Duration::ZERO,
    })
    .launch(spec)
    .await
    .unwrap();
    while process.events.recv().await.is_some() {}
    let first = std::fs::read_to_string(log).unwrap();
    let lines: Vec<serde_json::Value> = first
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(
        lines
            .iter()
            .find(|line| line.get("shell").is_some())
            .unwrap()["ok"],
        true,
        "explicit settings must override inherited pass_env values"
    );
    let log = &lines[0];
    let keys = log["env_keys"].as_array().unwrap();
    assert!(!keys.iter().any(|k| k == "SO_TEST_SECRET"));
    assert!(keys.iter().any(|k| k == "LC_SO_TEST"));
    assert!(keys.iter().any(|k| k == "SO_DRIVER_VAR"));
    assert!(keys.iter().any(|k| k == "SO_ACCOUNT_VAR"));
    assert!(keys.iter().any(|k| k == "SO_GLOBAL_VAR"));
    assert!(!keys.iter().any(|k| k == "MISSING_OPT_IN"));
    assert!(!keys.iter().any(|k| k == "OPENCODE_SERVER_PASSWORD"));
}
