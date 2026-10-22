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
    // The wrapper process is gone, but the row stays open until something reaps it.
    let open = || {
        e.store()
            .list_sessions(so_store::SessionFilter::Active, 100)
            .unwrap()
            .len()
    };
    assert_eq!(open(), 1);
    e.cmd().arg("status").assert().success().stdout(
        predicate::str::contains("supervisor: not running")
            .and(predicate::str::contains("codex-acc1")),
    );
    assert_eq!(open(), 0, "status reaps sessions whose pid is gone");
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

fn doctor_checks(e: &Env) -> Vec<serde_json::Value> {
    let out = e.cmd().args(["--json", "doctor"]).output().unwrap();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["checks"].as_array().unwrap().clone()
}

#[test]
fn doctor_warns_about_codex_choices_left_off_by_the_old_seed() {
    let e = Env::new();
    e.init();
    // A fresh seed has nothing to warn about.
    assert!(!doctor_checks(&e).iter().any(|c| c["name"] == "choices"));

    // An older config: the seeded text with the codex choices off.
    let path = e.cfg.join("config.toml");
    let mut text = std::fs::read_to_string(&path).unwrap();
    for id in ["codex-high", "codex-medium"] {
        let at = text.find(&format!("id = \"{id}\"")).unwrap();
        let off = text[at..].find("enabled = true").unwrap() + at;
        text.replace_range(off..off + "enabled = true".len(), "enabled = false");
    }
    text = text.replace(
        "Not eligible until a codex account is signed in.",
        "Disabled until a codex account is signed in.",
    );
    std::fs::write(&path, &text).unwrap();
    let checks = doctor_checks(&e);
    let w = checks.iter().find(|c| c["name"] == "choices").unwrap();
    assert_eq!(w["status"], "warn");
    let d = w["detail"].as_str().unwrap();
    assert!(
        d.contains("codex-high") && d.contains("codex-medium") && d.contains("enabled = true"),
        "{d}"
    );

    // A choice the user turned off with their own description is theirs.
    let text = text.replace(
        "Disabled until a codex account is signed in.",
        "Off on purpose.",
    );
    std::fs::write(&path, text).unwrap();
    assert!(!doctor_checks(&e).iter().any(|c| c["name"] == "choices"));
}

#[test]
fn doctor_flags_a_choice_effort_its_harness_rejects() {
    let e = Env::new();
    e.init();
    // The seed's agy `thinking` hint is valid.
    assert!(
        !doctor_checks(&e)
            .iter()
            .any(|c| c["name"] == "choice efforts")
    );

    let path = e.cfg.join("config.toml");
    let text = std::fs::read_to_string(&path).unwrap();
    let at = text.find("id = \"codex-high\"").unwrap();
    let eff = text[at..].find("effort = \"").unwrap() + at;
    let end = text[eff..].find('\n').unwrap() + eff;
    let mut edited = text.clone();
    edited.replace_range(eff..end, "effort = \"thinking\"");
    std::fs::write(&path, edited).unwrap();
    let checks = doctor_checks(&e);
    let w = checks
        .iter()
        .find(|c| c["name"] == "choice efforts")
        .unwrap();
    assert_eq!(w["status"], "warn");
    let d = w["detail"].as_str().unwrap();
    assert!(
        d.contains(
            "effort `thinking` is not valid for codex (minimal|low|medium|high|xhigh|max|ultra)"
        ) && d.contains("config.toml"),
        "{d}"
    );
}
