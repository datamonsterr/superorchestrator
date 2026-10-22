#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use so_core::SessionKind;
use so_store::SessionFilter;

fn codex_account(e: &Env) {
    e.init();
    let home = e.home.join(".codex-acc1");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("auth.json"), "{}").unwrap();
    e.cmd()
        .args([
            "account",
            "import",
            "codex-acc1",
            "--harness",
            "codex",
            "--home",
        ])
        .arg(&home)
        .assert()
        .success();
}

#[test]
fn exec_sets_env_passes_args_and_records_session() {
    let e = Env::new();
    codex_account(&e);
    e.script(
        "codex",
        "echo \"CODEX_HOME=$CODEX_HOME\"; echo \"ARGS=$*\"; echo \"PID=$$\"",
    );
    let out = e
        .cmd()
        .args(["exec", "codex-acc1", "--", "resume", "--flag", "x y"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(
        s.contains(&format!(
            "CODEX_HOME={}",
            e.home.join(".codex-acc1").display()
        )),
        "{s}"
    );
    assert!(s.contains("ARGS=resume --flag x y"), "{s}");

    let sessions = e.store().list_sessions(SessionFilter::All, 10).unwrap();
    assert_eq!(sessions.len(), 1);
    let sess = &sessions[0];
    assert_eq!(sess.kind, SessionKind::Interactive);
    assert_eq!(sess.account.as_deref(), Some("codex-acc1"));
    let pid: i64 = s
        .lines()
        .find_map(|l| l.strip_prefix("PID="))
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(sess.pid, Some(pid), "exec keeps the pid");
    assert!(sess.cwd.is_some());
}

#[test]
fn exec_still_launches_when_the_store_is_unavailable() {
    let e = Env::new();
    codex_account(&e);
    e.script("codex", "echo launched");
    // A directory where state.db should be makes opening fail.
    std::fs::create_dir(e.cfg.join("state.db")).unwrap();
    e.cmd()
        .args(["exec", "codex-acc1", "--", "hi"])
        .assert()
        .success()
        .stdout("launched\n")
        .stderr("");
}

#[test]
fn exec_unknown_account_fails() {
    let e = Env::new();
    e.init();
    e.cmd().args(["exec", "ghost-acc", "--"]).assert().code(3);
}

#[test]
fn status_without_supervisor_says_not_running_and_ends_dead_sessions() {
    let e = Env::new();
    codex_account(&e);
    e.script("codex", "exit 0");
    e.cmd()
        .args(["exec", "codex-acc1", "--"])
        .assert()
        .success();
    assert_eq!(
        e.store()
            .count_active_interactive_sessions_by_account("codex-acc1")
            .unwrap(),
        1
    );
    e.cmd().arg("status").assert().success().stdout(
        predicate::str::contains("supervisor: not running")
            .and(predicate::str::contains("codex-acc1")),
    );
    assert_eq!(
        e.store()
            .count_active_interactive_sessions_by_account("codex-acc1")
            .unwrap(),
        0,
        "status reaps sessions whose pid is gone"
    );
    let out = e.cmd().args(["status", "--json"]).assert().success();
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(v["supervisor"].is_null());
}

#[test]
fn doctor_passes_with_fake_binaries() {
    let e = Env::new();
    codex_account(&e);
    for b in ["codex", "agy", "opencode", "claude", "bwrap"] {
        e.script(b, "echo \"$0 1.2.3\"");
    }
    let out = e.cmd().arg("doctor").assert().success();
    let s = stdout(&out);
    assert!(s.contains("✓ codex:") && s.contains("1.2.3"), "{s}");
    assert!(s.contains("✓ config.toml: loads"), "{s}");
    assert!(s.contains("✓ account codex-acc1: signed in"), "{s}");
    assert!(s.contains("! supervisor: not running"), "{s}");
}

#[test]
fn doctor_fails_when_a_needed_binary_is_missing() {
    let e = Env::new();
    codex_account(&e);
    // No codex on PATH but an enabled codex account exists.
    e.cmd()
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("✗ codex:"));
}

#[test]
fn doctor_fails_on_invalid_config_and_missing_dir() {
    let e = Env::new();
    e.cmd()
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("does not exist"));
    e.init();
    std::fs::write(e.cfg.join("config.toml"), "limits = 7\n").unwrap();
    e.cmd()
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(predicate::str::contains("✗ config.toml: does not load"));
}

#[test]
fn doctor_times_out_a_hanging_binary_using_the_configured_timeout() {
    let e = Env::new();
    e.init();
    e.script("claude", "exec sleep 30");
    let start = std::time::Instant::now();
    let out = e
        .cmd()
        .env("SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS", "200")
        .arg("doctor")
        .assert()
        .success();
    assert!(start.elapsed().as_secs() < 4);
    assert!(
        stdout(&out).contains("timed out after 200ms"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn doctor_json_lists_checks_and_reports_unloadable_accounts() {
    let e = Env::new();
    e.init();
    std::fs::write(e.cfg.join("accounts/bad.toml"), "not = [valid").unwrap();
    let out = e.cmd().args(["--json", "doctor"]).assert().code(1);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["ok"], false);
    let checks = v["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["name"] == "accounts" && c["status"] == "fail")
    );
    assert!(
        checks
            .iter()
            .any(|c| c["name"] == "config.toml" && c["status"] == "ok")
    );
}
