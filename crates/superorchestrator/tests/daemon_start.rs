#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::Env;
use so_supervisor::{DaemonProcess, SupervisorClient};

#[tokio::test]
async fn connect_or_spawn_fails_fast_with_daemon_reason_when_socket_dir_insecure() {
    use std::os::unix::process::CommandExt;

    let e = Env::new();
    e.init();

    let exe = assert_cmd::cargo::cargo_bin("superorchestrator");
    // /tmp is not owned by the test user (e.g. root/nobody), so bind_socket rejects it
    let sock = PathBuf::from("/tmp/so-insecure-daemon-test.sock");

    let start = Instant::now();
    let Err(err) = SupervisorClient::connect_or_spawn_hello(
        &sock,
        || {
            let logs_dir = e.cfg.join("logs");
            std::fs::create_dir_all(&logs_dir)?;
            let log_path = logs_dir.join("daemon-start.log");
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&log_path)?;
            let child = std::process::Command::new(&exe)
                .arg("daemon")
                .env("SUPERORCHESTRATOR_CONFIG_DIR", &e.cfg)
                .env("SUPERORCHESTRATOR_SOCKET", &sock)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(log_file)
                .process_group(0)
                .spawn()?;
            Ok(DaemonProcess::new(child, Some(log_path)))
        },
        "test",
        "/tmp",
        None,
        None,
    )
    .await
    else {
        panic!("expected connect_or_spawn_hello to fail");
    };

    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "expected fast failure, took {elapsed:?}"
    );

    let msg = err.to_string();
    assert!(
        msg.contains("socket directory /tmp is not owned by the current user"),
        "error message should contain daemon startup reason, got: {msg}"
    );
    assert!(
        msg.contains("exit status: 1") || msg.contains("status"),
        "error message should contain exit status, got: {msg}"
    );
}

#[test]
fn cli_mcp_fails_fast_with_daemon_reason_when_socket_dir_insecure() {
    let e = Env::new();
    e.init();

    let mut cmd = assert_cmd::Command::cargo_bin("superorchestrator").unwrap();
    cmd.arg("--config-dir").arg(&e.cfg).arg("mcp").env(
        "SUPERORCHESTRATOR_SOCKET",
        "/tmp/so-insecure-daemon-test.sock",
    );

    let start = Instant::now();
    let assert = cmd.assert().failure();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "expected fast failure, took {elapsed:?}"
    );

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("socket directory /tmp is not owned by the current user"),
        "stderr should contain daemon startup reason, got: {stderr}"
    );
}
