//! The installed skill follows accounts, choices and agents (SPEC 19.1).
#![allow(clippy::unwrap_used)]

mod common;

use std::path::PathBuf;

use common::Env;
use so_agents::skill::{MARKER, skill_files};

fn files(e: &Env) -> Vec<PathBuf> {
    skill_files(&e.home)
}

/// What `install` leaves behind: marked files for the skill.
fn install_skill(e: &Env) {
    for p in files(e) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, format!("stale\n{MARKER}\n")).unwrap();
    }
}

fn skill(e: &Env) -> String {
    std::fs::read_to_string(&files(e)[0]).unwrap()
}

fn run(e: &Env, args: &[&str]) {
    e.cmd()
        .current_dir(e.tmp.path())
        .args(args)
        .assert()
        .success();
}

#[test]
fn nothing_is_written_when_the_skill_is_not_installed() {
    let e = Env::new();
    e.init();
    run(&e, &["agent", "create", "helper"]);
    assert!(!e.home.join(".claude").exists());
    assert!(!e.home.join(".agents").exists());
}

#[test]
fn account_commands_refresh_both_skill_files() {
    let e = Env::new();
    e.init();
    install_skill(&e);
    let home = e.tmp.path().join("codex-home");
    std::fs::create_dir_all(&home).unwrap();
    run(
        &e,
        &[
            "account",
            "import",
            "codex-a",
            "--harness",
            "codex",
            "--home",
            home.to_str().unwrap(),
        ],
    );
    for p in files(&e) {
        assert!(std::fs::read_to_string(p).unwrap().contains("`codex-a`"));
    }
    run(&e, &["account", "disable", "codex-a"]);
    assert!(skill(&e).contains("`codex-a`: codex, cap 2 (disabled)"));
    run(&e, &["account", "enable", "codex-a"]);
    assert!(!skill(&e).contains("(disabled)"));
    run(&e, &["account", "remove", "codex-a", "--yes"]);
    assert!(!skill(&e).contains("`codex-a`"));
}

#[test]
fn agent_commands_refresh_the_skill() {
    let e = Env::new();
    e.init();
    install_skill(&e);
    run(
        &e,
        &["agent", "create", "helper", "--description", "Helps out"],
    );
    assert!(skill(&e).contains("- `helper` (auto): Helps out"));
    run(&e, &["agent", "remove", "helper", "--yes"]);
    assert!(!skill(&e).contains("`helper`"));
}

#[test]
fn setting_choices_refreshes_the_skill() {
    let e = Env::new();
    e.init();
    install_skill(&e);
    run(
        &e,
        &[
            "config",
            "set",
            "choices",
            r#"[{id = "mine", harness = "codex", model = "m1", effort = "high", roles = ["worker"], description = "Custom", enabled = true}]"#,
        ],
    );
    let s = skill(&e);
    assert!(
        s.contains("`mine`: codex / m1 (high), roles: worker. Custom"),
        "{s}"
    );
}
