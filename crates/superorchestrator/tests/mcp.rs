#![allow(clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::Env;
use rmcp::ServiceExt;
use rmcp::model::CallToolRequestParams;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use so_core::{AccountFile, Harness};

#[tokio::test]
async fn mcp_serves_tools_and_list_harnesses_returns_configured_accounts() {
    // Short paths: unix socket paths are limited to ~108 bytes. The socket sits in a
    // subdirectory the supervisor creates with a private mode.
    let dir = tempfile::Builder::new()
        .prefix("somcp-")
        .tempdir_in("/tmp")
        .unwrap();
    let cfg = dir.path().join("cfg");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let e = Env {
        cfg: cfg.clone(),
        home: home.clone(),
        bin: dir.path().join("bin"),
        tmp: tempfile::tempdir().unwrap(),
    };
    e.init();
    // Let the auto-spawned supervisor exit quickly once the test is done.
    e.cmd()
        .args(["config", "set", "supervisor.idle_exit", "1s"])
        .assert()
        .success();
    for (name, harness, bin) in [
        ("codex-acc1", Harness::Codex, "codex"),
        ("claude-acc1", Harness::Claude, "claude"),
    ] {
        let h = home.join(format!(".{name}"));
        std::fs::create_dir_all(&h).unwrap();
        AccountFile::new(name, harness, &h.to_string_lossy(), bin)
            .unwrap()
            .save(&e.paths())
            .unwrap();
    }
    let sock = dir.path().join("run/s.sock");

    let exe = assert_cmd::cargo::cargo_bin("superorchestrator");
    let transport = TokioChildProcess::new(tokio::process::Command::new(&exe).configure(|c| {
        c.arg("--config-dir")
            .arg(&cfg)
            .arg("mcp")
            .env("HOME", &home)
            .env("SUPERORCHESTRATOR_SOCKET", &sock)
            .env_remove("SUPERORCHESTRATOR_RUN_ID")
            .env_remove("SUPERORCHESTRATOR_CONFIG_DIR");
    }))
    .unwrap();
    let client = tokio::time::timeout(Duration::from_secs(20), ().serve(transport))
        .await
        .expect("initialize timed out")
        .unwrap();

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(
        tools.len(),
        14,
        "{:?}",
        tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );

    let res = client
        .call_tool(CallToolRequestParams::new("list_harnesses"))
        .await
        .unwrap();
    assert_ne!(res.is_error, Some(true), "{res:?}");
    let view: serde_json::Value = match res.structured_content {
        Some(v) => serde_json::from_value(v).unwrap(),
        None => {
            let t = res.content[0].as_text().unwrap().text.clone();
            serde_json::from_str(&t).unwrap()
        }
    };
    let mut names: Vec<_> = view["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["claude-acc1", "codex-acc1"]);

    client.cancel().await.unwrap();
    // The supervisor idles out after 1s and removes its socket; wait so nothing lingers.
    for _ in 0..100 {
        if !sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!sock.exists(), "supervisor did not exit");
}
