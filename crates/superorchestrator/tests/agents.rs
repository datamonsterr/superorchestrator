#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use serde_json::Value;

fn run(e: &Env, args: &[&str]) -> assert_cmd::Command {
    let mut c = e.cmd();
    c.current_dir(e.tmp.path()).args(args);
    c
}

fn list(e: &Env) -> Vec<Value> {
    let out = run(e, &["agent", "list", "--json"]).assert().success();
    serde_json::from_str::<Value>(&stdout(&out))
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn init_installs_starters_once() {
    let e = Env::new();
    e.init();
    let names: Vec<String> = list(&e)
        .iter()
        .map(|a| a["name"].as_str().unwrap().into())
        .collect();
    assert_eq!(names, ["issue-summarizer", "pr-reviewer", "ui-ux-designer"]);
    std::fs::write(
        e.cfg.join("agents/pr-reviewer/AGENT.md"),
        "---\nname: pr-reviewer\ndescription: mine\n---\nx",
    )
    .unwrap();
    let second = e.cmd().arg("init").assert().success();
    assert!(!stdout(&second).contains("created"));
    let a = list(&e)
        .into_iter()
        .find(|a| a["name"] == "pr-reviewer")
        .unwrap();
    assert_eq!(a["description"], "mine");
}

#[test]
fn create_list_show_remove() {
    let e = Env::new();
    e.init();
    run(
        &e,
        &[
            "agent",
            "create",
            "my-agent",
            "--harness",
            "codex",
            "--description",
            "Does: things",
        ],
    )
    .assert()
    .success();
    assert!(e.cfg.join("agents/my-agent/AGENT.md").is_file());
    run(&e, &["agent", "create", "my-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));
    run(&e, &["agent", "create", "bad:name"]).assert().failure();

    let a = list(&e)
        .into_iter()
        .find(|a| a["name"] == "my-agent")
        .unwrap();
    assert_eq!(a["harness"], "codex");
    assert_eq!(a["source"], "user");
    assert_eq!(a["description"], "Does: things");

    let out = run(&e, &["agent", "show", "my-agent", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["name"], "my-agent");
    assert!(v["gaps"]["codex"].as_array().unwrap().is_empty());

    run(&e, &["agent", "remove", "my-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    run(&e, &["agent", "remove", "my-agent", "--yes"])
        .assert()
        .success();
    assert!(!e.cfg.join("agents/my-agent").exists());
    run(&e, &["agent", "show", "my-agent"]).assert().code(3);
}

#[test]
fn show_reports_gaps_per_harness() {
    let e = Env::new();
    e.init();
    let p = e.cfg.join("agents/gappy.md");
    std::fs::write(
        &p,
        "---\nname: gappy\ndescription: d\ntools: Read, Grep\ndisallowedTools: Write\n---\nb",
    )
    .unwrap();
    let out = run(&e, &["agent", "show", "gappy"]).assert().success();
    let text = stdout(&out);
    assert!(text.contains("claude: none"), "{text}");
    assert!(text.contains("codex has only sandbox"), "{text}");
    assert!(text.contains("all-or-nothing"), "{text}");
}

#[test]
fn claude_imports_are_listed_but_read_only() {
    let e = Env::new();
    e.init();
    let dir = e.tmp.path().join(".claude/agents");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("imp.md"),
        "---\nname: imp\ndescription: imported\ntools: Read, Grep\n---\nb",
    )
    .unwrap();
    let a = list(&e).into_iter().find(|a| a["name"] == "imp").unwrap();
    assert_eq!(a["source"], "claude");
    run(&e, &["agent", "edit", "imp"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("read-only"));
    run(&e, &["agent", "remove", "imp", "--yes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("read-only"));
    assert!(dir.join("imp.md").exists());
}

#[test]
fn edit_runs_editor_and_validates() {
    let e = Env::new();
    e.init();
    let ed = e.script("fake-editor", "printf '\\nedited\\n' >> \"$1\"");
    run(&e, &["agent", "edit", "pr-reviewer"])
        .env("EDITOR", &ed)
        .assert()
        .success();
    let text = std::fs::read_to_string(e.cfg.join("agents/pr-reviewer/AGENT.md")).unwrap();
    assert!(text.ends_with("edited\n"));
    let bad = e.script("bad-editor", "echo garbage > \"$1\"");
    run(&e, &["agent", "edit", "pr-reviewer"])
        .env("EDITOR", &bad)
        .assert()
        .failure();
}
