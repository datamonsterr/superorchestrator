#![allow(clippy::unwrap_used)]
mod common;

use std::os::unix::fs::PermissionsExt;

use common::Env;

#[test]
fn install_service_writes_a_unit_and_enables_the_ui_in_config() {
    let e = Env::new();
    e.init();
    let out = e
        .cmd()
        .args(["ui", "--install-service", "--port", "7999"])
        .assert()
        .success();
    let text = common::stdout(&out);
    assert!(
        text.contains("systemctl --user") || text.contains("launchctl"),
        "{text}"
    );
    let unit = if cfg!(target_os = "macos") {
        e.home
            .join("Library/LaunchAgents/dev.superorchestrator.plist")
    } else {
        e.home
            .join(".config/systemd/user/superorchestrator.service")
    };
    let body = std::fs::read_to_string(&unit).unwrap();
    assert!(
        body.contains("daemon") && body.contains("--foreground"),
        "{body}"
    );
    assert!(body.contains(&*e.cfg.to_string_lossy()));
    let cfg = std::fs::read_to_string(e.cfg.join("config.toml")).unwrap();
    assert!(
        cfg.contains("enabled = true") && cfg.contains("port = 7999"),
        "{cfg}"
    );
}

#[test]
fn ui_command_starts_the_server_inside_the_supervisor() {
    let e = Env::new();
    // The socket directory must be private.
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    e.init();
    let out = e
        .cmd()
        .args(["--json", "ui", "--no-open", "--port", "0"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&common::stdout(&out)).unwrap();
    let url = v["url"].as_str().unwrap();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");
    // A second call reports the running server.
    let out = e
        .cmd()
        .args(["--json", "ui", "--no-open"])
        .assert()
        .success();
    let v2: serde_json::Value = serde_json::from_str(&common::stdout(&out)).unwrap();
    assert_eq!(v2["already_running"], true);
    // Stop the supervisor this test started.
    let st = e.cmd().args(["--json", "status"]).assert().success();
    let st: serde_json::Value = serde_json::from_str(&common::stdout(&st)).unwrap();
    let pid = st["supervisor"]["pid"].as_u64().unwrap();
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .unwrap();
}
