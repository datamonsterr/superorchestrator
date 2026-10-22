#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use serde_json::Value;

fn list_json(e: &Env) -> Vec<Value> {
    let out = e
        .cmd()
        .args(["account", "list", "--json"])
        .assert()
        .success();
    serde_json::from_str::<Value>(&stdout(&out))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn account_add_writes_files_and_prints_login_command() {
    let e = Env::new();
    e.init();
    e.script("codex", "exit 0");
    let out = e
        .cmd()
        .args(["account", "add", "codex", "--name", "codex-acc2"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("login"), "{s}");
    assert!(s.contains("CODEX_HOME="), "{s}");
    assert!(e.cfg.join("accounts/codex-acc2.toml").is_file());
    assert!(e.home.join(".codex-acc2").is_dir());
    assert!(e.home.join(".local/bin/codex-acc2").is_file());
    e.cmd()
        .args(["account", "add", "nope", "--name", "x-acc"])
        .assert()
        .failure();
}

#[test]
fn account_import_and_list_json() {
    let e = Env::new();
    e.init();
    let home = e.home.join(".codex-acc7");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("auth.json"), "{}").unwrap();
    e.cmd()
        .args([
            "account",
            "import",
            "codex-acc7",
            "--harness",
            "codex",
            "--home",
        ])
        .arg(&home)
        .assert()
        .success();
    let rows = list_json(&e);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "codex-acc7");
    assert_eq!(rows[0]["harness"], "codex");
    assert_eq!(rows[0]["signed_in"], "yes");
    assert_eq!(rows[0]["enabled"], true);
    assert_eq!(rows[0]["active_runs"], 0);
    assert_eq!(rows[0]["headroom"]["default"], "unknown");
    e.cmd()
        .args(["account", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("codex-acc7").and(predicate::str::contains("NAME")));
}

#[test]
fn import_scan_lists_without_yes_and_imports_with_yes() {
    let e = Env::new();
    e.init();
    for d in [".codex-acc1", ".codex-acc2", ".claude"] {
        std::fs::create_dir_all(e.home.join(d)).unwrap();
    }
    let out = e
        .cmd()
        .args(["account", "import", "--scan"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("codex-acc1") && s.contains("claude-acc1"), "{s}");
    assert!(
        list_json(&e).is_empty(),
        "non-tty without --yes must not import"
    );

    e.cmd()
        .args(["account", "import", "--scan", "--yes"])
        .assert()
        .success();
    let names: Vec<String> = list_json(&e)
        .iter()
        .map(|r| r["name"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(names, ["claude-acc1", "codex-acc1", "codex-acc2"]);
    e.cmd()
        .args(["account", "import", "--scan"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no new accounts"));
}

fn import_one(e: &Env, name: &str) {
    let home = e.home.join(format!(".{name}"));
    std::fs::create_dir_all(&home).unwrap();
    e.cmd()
        .args(["account", "import", name, "--harness", "codex", "--home"])
        .arg(&home)
        .assert()
        .success();
}

#[test]
fn enable_disable() {
    let e = Env::new();
    e.init();
    import_one(&e, "codex-acc1");
    e.cmd()
        .args(["account", "disable", "codex-acc1"])
        .assert()
        .success();
    assert_eq!(list_json(&e)[0]["enabled"], false);
    e.cmd()
        .args(["account", "enable", "codex-acc1"])
        .assert()
        .success();
    assert_eq!(list_json(&e)[0]["enabled"], true);
    e.cmd()
        .args(["account", "enable", "ghost-acc"])
        .assert()
        .failure();
}

#[test]
fn flag_hours_until_and_clear() {
    let e = Env::new();
    e.init();
    import_one(&e, "codex-acc1");
    e.cmd()
        .args(["account", "flag", "codex-acc1", "--hours", "2"])
        .assert()
        .success();
    let flag = e
        .store()
        .get_flag("codex-acc1", "default")
        .unwrap()
        .unwrap();
    let delta = flag.until - chrono::Utc::now();
    assert!(
        delta.num_minutes() > 100 && delta.num_minutes() <= 120,
        "{delta}"
    );
    assert!(
        list_json(&e)[0]["headroom"]["default"]
            .as_str()
            .unwrap()
            .starts_with("cooldown until")
    );

    e.cmd()
        .args([
            "account",
            "flag",
            "codex-acc1",
            "--until",
            "2099-01-01T00:00:00Z",
        ])
        .assert()
        .success();
    let flag = e
        .store()
        .get_flag("codex-acc1", "default")
        .unwrap()
        .unwrap();
    assert_eq!(flag.until.to_rfc3339(), "2099-01-01T00:00:00+00:00");

    e.cmd()
        .args(["account", "flag", "codex-acc1", "--clear"])
        .assert()
        .success();
    assert!(
        e.store()
            .get_flag("codex-acc1", "default")
            .unwrap()
            .is_none()
    );

    // One of --until/--hours/--clear is required; a bad time is rejected.
    e.cmd()
        .args(["account", "flag", "codex-acc1"])
        .assert()
        .failure();
    e.cmd()
        .args(["account", "flag", "codex-acc1", "--until", "tomorrow"])
        .assert()
        .failure();
}

#[test]
fn remove_requires_confirmation_without_tty() {
    let e = Env::new();
    e.init();
    import_one(&e, "codex-acc1");
    e.cmd()
        .args(["account", "remove", "codex-acc1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    assert!(e.cfg.join("accounts/codex-acc1.toml").exists());
    assert!(e.home.join(".codex-acc1").exists());

    e.cmd()
        .args(["account", "remove", "codex-acc1", "--yes"])
        .assert()
        .success();
    assert!(!e.cfg.join("accounts/codex-acc1.toml").exists());
    // Credential home is kept without --purge.
    assert!(e.home.join(".codex-acc1").exists());
    assert!(list_json(&e).is_empty());
}

#[test]
fn login_codex_without_tty_runs_device_auth_with_env() {
    let e = Env::new();
    e.init();
    e.script("codex", "echo \"ARGS=$*\"; echo \"CODEX_HOME=$CODEX_HOME\"");
    import_one(&e, "codex-acc1");
    let out = e
        .cmd()
        .args(["account", "login", "codex-acc1"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("ARGS=login --device-auth"), "{s}");
    assert!(
        s.contains(&format!(
            "CODEX_HOME={}",
            e.home.join(".codex-acc1").display()
        )),
        "{s}"
    );
}

#[test]
fn login_other_harness_without_tty_prints_command() {
    let e = Env::new();
    e.init();
    let home = e.home.join(".oc");
    std::fs::create_dir_all(&home).unwrap();
    e.cmd()
        .args([
            "account",
            "import",
            "oc-acc1",
            "--harness",
            "opencode",
            "--home",
        ])
        .arg(&home)
        .assert()
        .success();
    e.cmd()
        .args(["account", "login", "oc-acc1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("auth login"));
}
