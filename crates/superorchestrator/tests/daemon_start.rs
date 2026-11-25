#![allow(clippy::unwrap_used)]

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use common::Env;
use so_supervisor::{DaemonProcess, SupervisorClient};

#[tokio::test]
async fn connect_or_spawn_fails_fast_with_daemon_reason_when_socket_dir_insecure() {
    use std::os::unix::process::CommandExt;

    let e = Env::new();
    e.init();

    let exe = assert_cmd::cargo::cargo_bin("superorchestrator");
    // Directory with mode 0o755 inside tempdir triggers InsecureSocketDir (& 0o077 != 0)
    let insecure_dir = e.tmp.path().join("insecure-dir");
    std::fs::create_dir(&insecure_dir).unwrap();
    std::fs::set_permissions(&insecure_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sock = insecure_dir.join("so.sock");

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
            let mut cmd = std::process::Command::new(&exe);
            cmd.arg("daemon")
                .env("SUPERORCHESTRATOR_CONFIG_DIR", &e.cfg)
                .env("SUPERORCHESTRATOR_SOCKET", &sock)
                .env("HOME", &e.home)
                .env_remove("XDG_RUNTIME_DIR")
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("XDG_DATA_HOME")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(log_file)
                .process_group(0);
            let child = cmd.spawn()?;
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
        msg.contains("socket directory") && msg.contains("is not owned by the current user"),
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

    let insecure_dir = e.tmp.path().join("insecure-dir");
    std::fs::create_dir(&insecure_dir).unwrap();
    std::fs::set_permissions(&insecure_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let sock = insecure_dir.join("so.sock");

    let mut cmd = e.cmd();
    cmd.arg("mcp").env("SUPERORCHESTRATOR_SOCKET", &sock);

    let start = Instant::now();
    let assert = cmd.assert().failure();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "expected fast failure, took {elapsed:?}"
    );

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("socket directory") && stderr.contains("is not owned by the current user"),
        "stderr should contain daemon startup reason, got: {stderr}"
    );
}

#[test]
fn daemon_fails_fast_when_socket_already_in_use() {
    let e = Env::new();
    e.init();

    // Bind a listener on the socket path inside the private tempdir.
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let sock = e.tmp.path().join("so.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();

    let mut cmd = e.cmd();
    cmd.arg("daemon").env("SUPERORCHESTRATOR_SOCKET", &sock);

    let start = Instant::now();
    let assert = cmd.assert().failure();
    let elapsed = start.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "expected fast failure, took {elapsed:?}"
    );

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(
        stderr.contains("another supervisor is already listening on"),
        "stderr should contain socket in use message, got: {stderr}"
    );

    // Existing listener must still be listening and accept connections.
    let stream = std::os::unix::net::UnixStream::connect(&sock);
    assert!(stream.is_ok(), "original listener should still accept");
    drop(listener);
}
