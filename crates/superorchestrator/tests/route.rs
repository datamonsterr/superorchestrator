#![allow(clippy::unwrap_used)]
//! `route`, `route eval` and `models` from the command line.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use common::{Env, stdout};
use serde_json::Value;
use so_accounts::HostEnv;
use so_core::{AccountFile, Config, Harness, Platform};
use so_supervisor::{Supervisor, SupervisorOptions, default_handlers};

fn setup() -> Env {
    let e = Env::new();
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    e.init();
    // A key variable that is never set, so nothing reaches the network.
    e.cmd()
        .args(["config", "set", "router.jev.key_env", "SO_CLI_TEST_NO_KEY"])
        .assert()
        .success();
    for (name, h, bin) in [
        ("agy-a", Harness::Agy, "agy"),
        ("oc-a", Harness::Opencode, "opencode"),
    ] {
        AccountFile::new(name, h, &e.home.join(name).to_string_lossy(), bin)
            .unwrap()
            .save(&e.paths())
            .unwrap();
    }
    e
}

#[tokio::test(flavor = "multi_thread")]
async fn route_json_goes_through_the_supervisor() {
    let e = setup();
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let mut cmd = e.cmd();
    cmd.args(["route", "--json", "rename a variable in one file"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["source"], "heuristic");
    assert_eq!(v["harness"], "agy");
    assert!(v["account"].as_str().unwrap().starts_with("agy-"));
    assert_eq!(v["config_fingerprint"].as_str().unwrap().len(), 64);

    let mut cmd = e.cmd();
    cmd.args(["route", "--explain", "--role", "worker", "fix typo"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success());
    assert!(
        text.contains("account:") && text.contains("config:"),
        "{text}"
    );

    let mut cmd = e.cmd();
    cmd.args(["route", "x", "--role", "nobody"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(!out.status.success());
    sup.shutdown().await;
}

#[test]
fn route_eval_runs_locally_with_the_heuristic() {
    let e = setup();
    let cases = e.tmp.path().join("cases.jsonl");
    std::fs::write(
        &cases,
        concat!(
            r#"{"id":"a","task":"SECRET-TEXT rename a variable","expected_choices":["agy-flash-low","agy-flash-medium"]}"#,
            "\n",
            r#"{"id":"b","task":"Tìm file khai báo timeout","expected_choices":["agy-flash-medium","agy-flash-low"]}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args([
            "route",
            "eval",
            "--json",
            "--min-accuracy",
            "0.5",
            "--cases",
        ])
        .arg(&cases)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!text.contains("SECRET-TEXT"));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["case_count"], 2);
    assert_eq!(v["sources"]["heuristic"], 2);
    assert_eq!(v["eligible_compliance"], 1.0);
    assert_eq!(v["accuracy"], 1.0);
    assert!(v["latency_ms"]["p95"].is_number());

    // An unreachable accuracy bar fails the command.
    e.cmd()
        .args(["route", "eval", "--min-accuracy", "1.0", "--cases"])
        .arg(&cases)
        .assert()
        .success();
    let cases2 = e.tmp.path().join("c2.jsonl");
    std::fs::write(
        &cases2,
        r#"{"id":"a","task":"rename","expected_choices":["nope"]}"#,
    )
    .unwrap();
    e.cmd()
        .args(["route", "eval", "--min-accuracy", "0.5", "--cases"])
        .arg(&cases2)
        .assert()
        .failure();
}

#[test]
fn route_eval_require_live_fails_without_a_key() {
    let e = setup();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--require-live"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--require-live"));
}

#[test]
fn route_eval_bundled_cases_report() {
    let e = setup();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--json"])
        .output()
        .unwrap();
    // Conductor cases may be unroutable here; the report is still produced.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["case_count"], 15);
}

#[test]
fn models_list_json_uses_the_default_catalog() {
    let e = setup();
    let out = e
        .cmd()
        .args(["models", "list", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let arr = v.as_array().unwrap();
    assert!(arr.len() > 50);
    let m = arr
        .iter()
        .find(|m| m["model"] == "claude-sonnet-4-6")
        .unwrap();
    assert_eq!(m["harness"], "agy");
    assert_eq!(m["pool"], "claude");
    assert_eq!(m["tier"], "high");
}
