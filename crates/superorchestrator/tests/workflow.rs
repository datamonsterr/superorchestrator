#![allow(clippy::unwrap_used)]
//! `workflow run|list|status` and `runs watch --workflow` against an in-process supervisor
//! driving the fake harness.

mod common;

use common::{Env, add_account, cli, start_supervisor, stdout};
use predicates::prelude::*;
use serde_json::{Value, json};

const HELLO: &str = "return {greeting: args.who, got: await agent(\"go\", \
    {harness: 'claude', account: 'claude-acc1', isolation: 'none'})};";

fn write_saved(e: &Env, name: &str, source: &str) {
    let dir = e.cfg.join("workflows");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(format!("{name}.js")), source).unwrap();
}

#[test]
fn list_shows_saved_workflows_without_a_store() {
    let e = Env::new();
    write_saved(&e, "hello", HELLO);
    let out = e
        .cmd()
        .args(["workflow", "list", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["saved"][0]["name"], "hello");
    assert_eq!(v["saved"][0]["source"], "config");
    assert_eq!(v["runs"], json!([]));
    let out = e.cmd().args(["workflow", "list"]).assert().success();
    let s = stdout(&out);
    assert!(
        s.contains("hello") && s.contains("saved workflows:") && s.contains("runs:"),
        "{s}"
    );
}

#[test]
fn usage_errors_are_clap_errors() {
    let e = Env::new();
    // A name or a file is required, and not both.
    e.cmd().args(["workflow", "run"]).assert().code(2);
    e.cmd()
        .args(["workflow", "run", "x", "--file", "y.js"])
        .assert()
        .code(2);
    e.cmd().args(["runs", "watch"]).assert().code(2);
    e.cmd()
        .args(["runs", "watch", "r", "--workflow", "w"])
        .assert()
        .code(2);
    e.cmd()
        .args(["workflow", "run", "x", "--args", "{not json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--args must be JSON"));
}

#[tokio::test(flavor = "multi_thread")]
async fn run_status_and_watch_follow_a_workflow() {
    let e = Env::new();
    add_account(&e);
    write_saved(&e, "hello", HELLO);
    let sup = start_supervisor(&e).await;

    // Run by name with arguments: the result is on stdout, step progress on stderr.
    let mut cmd = e.cmd();
    cmd.args([
        "workflow",
        "run",
        "hello",
        "--args",
        r#"{"who":"me"}"#,
        "--json",
    ]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["state"], "succeeded");
    assert_eq!(v["result"]["greeting"], "me");
    assert!(v["result"]["got"].is_string());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("step 1: running") && stderr.contains("step 1: succeeded"),
        "{stderr}"
    );
    let id = v["id"].as_str().unwrap().to_string();
    // The id is on stderr before the progress lines, so it is known while the workflow runs.
    let first = stderr.lines().next().unwrap_or_default();
    assert_eq!(first, format!("workflow {id}"), "{stderr}");

    // status by prefix, human and JSON.
    let (out, ok) = cli(&e, &["workflow", "status", &id[..12]]).await;
    assert!(ok);
    assert!(
        out.contains("state:    succeeded") && out.contains("#1 step 1  succeeded  run "),
        "{out}"
    );
    let (out, ok) = cli(&e, &["workflow", "status", &id, "--json"]).await;
    assert!(ok);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["steps"][0]["state"], "succeeded");
    assert_eq!(v["name"], "hello");

    // The run is listed from the store.
    let (out, ok) = cli(&e, &["workflow", "list", "--json"]).await;
    assert!(ok);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["runs"][0]["id"], id);

    // watch a finished workflow prints its steps and the outcome.
    let (out, ok) = cli(&e, &["runs", "watch", "--workflow", &id]).await;
    assert!(ok);
    assert!(
        out.contains("#1 step 1  succeeded") && out.trim_end().ends_with("succeeded"),
        "{out}"
    );

    // --detach prints the id at once; watch follows it to the end.
    let (out, ok) = cli(
        &e,
        &[
            "workflow",
            "run",
            "hello",
            "--detach",
            "--args",
            r#"{"who":"you"}"#,
        ],
    )
    .await;
    assert!(ok);
    let detached = out.trim().to_string();
    assert_eq!(detached.len(), id.len());
    let (out, ok) = cli(&e, &["runs", "watch", "--workflow", &detached]).await;
    assert!(ok, "{out}");
    assert!(out.trim_end().ends_with("succeeded"), "{out}");
    sup.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn failures_and_missing_workflows_exit_non_zero() {
    let e = Env::new();
    add_account(&e);
    let sup = start_supervisor(&e).await;
    let file = e.tmp.path().join("boom.js");
    std::fs::write(&file, "throw new Error('boom');").unwrap();

    let mut cmd = e.cmd();
    cmd.args(["workflow", "run", "--file", file.to_str().unwrap()]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed") && stderr.contains("boom"),
        "{stderr}"
    );

    let mut cmd = e.cmd();
    cmd.args(["workflow", "run", "no-such-workflow"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let mut cmd = e.cmd();
    cmd.args(["workflow", "status", "ffffffff-nothing"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    sup.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_and_fresh() {
    let e = Env::new();
    add_account(&e);
    write_saved(&e, "hello", HELLO);
    let sup = start_supervisor(&e).await;
    let args = r#"{"who":"me"}"#;
    let (out, ok) = cli(&e, &["workflow", "run", "hello", "--args", args, "--json"]).await;
    assert!(ok);
    let id = serde_json::from_str::<Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A finished workflow cannot be cancelled; an unknown one is not found.
    let mut cmd = e.cmd();
    cmd.args(["workflow", "cancel", &id]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not running"));
    let mut cmd = e.cmd();
    cmd.args(["workflow", "cancel", "ffffffff-nothing"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    // --fresh runs the step again: its step is not `cached`.
    let (out, ok) = cli(
        &e,
        &[
            "workflow", "run", "hello", "--args", args, "--fresh", "--json",
        ],
    )
    .await;
    assert!(ok);
    let id2 = serde_json::from_str::<Value>(&out).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (out, _) = cli(&e, &["workflow", "status", &id2, "--json"]).await;
    assert_eq!(
        serde_json::from_str::<Value>(&out).unwrap()["steps"][0]["state"],
        "succeeded"
    );
    sup.shutdown().await;
}
