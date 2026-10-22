//! Test-only fixture (feature `test-support`): fills a supervisor with runs for the web UI's
//! real-server browser tests (`web/e2e/real`).
//!
//! `e2e-seed --config-dir D --home H --cwd W --fake-harness F --daemon-exe E` writes three
//! `claude` accounts whose binary is the scripted `fake-harness`, starts the supervisor (the
//! socket comes from `SUPERORCHESTRATOR_SOCKET`), spawns a run per state worth drawing
//! (succeeded, failed, running, cancelled), opens a second, empty session, prints one JSON
//! line `{"ready":true,...}` and then stays connected until it is killed: the runs belong to
//! this connection's session, and the running one keeps running only while it is open.
//! No real harness is ever started.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::print_stdout)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use so_core::Paths;
use so_core::proto::{
    METHOD_CANCEL_AGENT, METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, RunRef, SpawnParams, SpawnResult,
    WaitParams, WaitResult,
};
use so_supervisor::{SupervisorClient, spawn_daemon_process};

const TRANSCRIPT: &str = include_str!("../../../so-harness/tests/fixtures/claude/transcript.jsonl");

fn arg(name: &str) -> PathBuf {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == name {
            return it
                .next()
                .unwrap_or_else(|| panic!("{name} needs a value"))
                .into();
        }
    }
    panic!("missing {name}");
}

/// First claude turn of the recorded transcript, ending in a successful `result`.
fn success_script() -> Vec<Value> {
    let mut steps = vec![json!({"expect_stdin_contains": "go"})];
    for l in TRANSCRIPT.lines() {
        let v: Value = serde_json::from_str(l).unwrap();
        if v["dir"] != "in" {
            continue;
        }
        let last = v["msg"]["type"] == "result";
        steps.push(json!({"stdout": v["msg"]}));
        if last {
            break;
        }
    }
    steps
}

/// The same turn up to (not including) the result, then `tail`.
fn partial_script(tail: Value) -> Vec<Value> {
    let mut steps = success_script();
    steps.pop();
    steps.push(tail);
    steps
}

fn write_account(cfg: &Path, home: &Path, fake: &Path, name: &str, steps: &[Value]) {
    let script = cfg.join(format!("{name}.script.jsonl"));
    let body: String = steps.iter().map(|s| format!("{s}\n")).collect();
    std::fs::write(&script, body).unwrap();
    let wrapper = cfg.join(format!("{name}.sh"));
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nexport FAKE_HARNESS_SCRIPT={}\nexec {} \"$@\"\n",
            script.display(),
            fake.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::create_dir_all(cfg.join("accounts")).unwrap();
    std::fs::write(
        cfg.join(format!("accounts/{name}.toml")),
        format!(
            "name = \"{name}\"\nharness = \"claude\"\nhome = \"{}\"\nbinary = \"{}\"\n",
            home.join(format!(".{name}")).display(),
            wrapper.display()
        ),
    )
    .unwrap();
}

async fn spawn(c: &SupervisorClient, account: &str, name: &str, cwd: &Path) -> String {
    let r: SpawnResult = c
        .call(
            METHOD_SPAWN_AGENT,
            SpawnParams {
                prompt: Some("go".into()),
                harness: Some("claude".into()),
                account: Some(account.into()),
                isolation: Some(so_core::Isolation::None),
                cwd: Some(cwd.to_string_lossy().into_owned()),
                name: Some(name.into()),
                ..SpawnParams::default()
            },
        )
        .await
        .unwrap();
    r.run_id
}

#[tokio::main]
async fn main() {
    let cfg = arg("--config-dir");
    let home = arg("--home");
    let cwd = arg("--cwd");
    let fake = arg("--fake-harness");
    let exe = arg("--daemon-exe");
    std::fs::create_dir_all(&cfg).unwrap();

    write_account(&cfg, &home, &fake, "claude-acc1", &success_script());
    write_account(&cfg, &home, &fake, "claude-acc2", &[json!({"exit": 1})]);
    write_account(
        &cfg,
        &home,
        &fake,
        "claude-acc3",
        &partial_script(json!({"sleep_ms": 3_600_000})),
    );

    let socket = Paths::socket_path().unwrap();
    let (c, _) = SupervisorClient::connect_or_spawn_hello(
        &socket,
        || spawn_daemon_process(&exe, &cfg),
        "e2e-seed",
        &cwd.to_string_lossy(),
        None,
        None,
    )
    .await
    .unwrap();

    let ok = spawn(&c, "claude-acc1", "write-readme", &cwd).await;
    let bad = spawn(&c, "claude-acc2", "flaky-migration", &cwd).await;
    // `wait_agent` returns when any of the runs finishes: ask again for the rest.
    let mut left = vec![ok.clone(), bad.clone()];
    while !left.is_empty() {
        let w: WaitResult = c
            .call(
                METHOD_WAIT_AGENT,
                WaitParams {
                    runs: left.clone(),
                    timeout_ms: Some(60_000),
                },
            )
            .await
            .unwrap();
        assert!(!w.timed_out, "{w:?}");
        left.retain(|r| !w.finished.contains(r));
    }

    let running = spawn(&c, "claude-acc3", "long-refactor", &cwd).await;
    let cancelled = spawn(&c, "claude-acc3", "abandoned-spike", &cwd).await;
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let _: Value = c
        .call(
            METHOD_CANCEL_AGENT,
            RunRef {
                run: cancelled.clone(),
            },
        )
        .await
        .unwrap();

    // A second session with no runs: its timeline shows the empty state.
    let (_idle, _) = SupervisorClient::connect_or_spawn_hello(
        &socket,
        || spawn_daemon_process(&exe, &cfg),
        "e2e-empty",
        &cwd.to_string_lossy(),
        None,
        None,
    )
    .await
    .unwrap();

    println!(
        "{}",
        json!({"ready": true, "runs": {"succeeded": ok, "failed": bad, "running": running, "cancelled": cancelled}})
    );
    // Stay connected: closing the session would end the running run.
    tokio::time::sleep(Duration::from_secs(3600)).await;
}
