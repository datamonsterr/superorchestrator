#![allow(clippy::unwrap_used)]

mod common;

use assert_cmd::Command;
use common::{Env, stdout};
use serde_json::Value;

fn accounts(e: &Env) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(e.cfg.join("accounts"))
        .map(|rd| {
            rd.flatten()
                .filter_map(|f| f.file_name().to_str().map(str::to_string))
                .filter_map(|n| n.strip_suffix(".toml").map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The command with a PATH holding only the fake binaries, so a harness installed on the
/// machine running the tests cannot leak in.
fn cmd(e: &Env) -> assert_cmd::Command {
    let mut c = e.cmd();
    c.env("PATH", &e.bin);
    c
}

fn env_with_harnesses() -> Env {
    let e = Env::new();
    e.script("codex", "echo codex 1.0");
    e.script("opencode", "echo opencode 1.0");
    e.script("claude", "echo claude 1.0");
    e
}

#[test]
fn without_a_tty_and_without_yes_it_prints_the_plan_and_changes_nothing() {
    let e = env_with_harnesses();
    let out = cmd(&e).arg("setup").assert().success();
    let s = stdout(&out);
    assert!(s.contains("setup would"), "{s}");
    assert!(s.contains("codex"), "{s}");
    assert!(s.contains("not installed"), "{s}");
    assert!(s.contains("--yes"), "{s}");
    assert!(!e.cfg.exists(), "plan must not create the config dir");
    assert!(!e.home.join(".local").exists());
    assert!(!e.home.join(".claude.json").exists());

    let out = cmd(&e)
        .args(["--json", "setup", "--codex", "2"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["dry_run"], true);
    let codex = v["add"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["harness"] == "codex")
        .unwrap();
    assert_eq!(codex["count"], 2);
}

#[test]
fn yes_adds_the_default_accounts_and_runs_install_in_the_isolated_home() {
    let e = env_with_harnesses();
    let out = cmd(&e)
        .args(["setup", "--yes", "--no-login"])
        .assert()
        .success();
    let s = stdout(&out);
    // One account for each installed harness except claude, which is kept for the orchestrator.
    assert_eq!(accounts(&e), ["codex-1", "opencode-1"], "{s}");
    assert!(e.home.join(".local/bin/codex-1").is_file());
    assert!(s.contains("added account codex-1"), "{s}");
    assert!(s.contains("install:"), "{s}");
    let md = std::fs::read_to_string(e.home.join(".claude/CLAUDE.md")).unwrap();
    assert!(md.contains("superorchestrator:begin"), "{md}");
    assert!(s.contains("doctor:"), "{s}");
    assert!(s.contains("ask it to delegate"), "{s}");
    // install wrote into the isolated HOME.
    assert!(
        e.home
            .join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file(),
        "{s}"
    );
}

#[test]
fn counts_flags_add_that_many_and_claude_prints_the_orchestrator_note() {
    let e = env_with_harnesses();
    let out = cmd(&e)
        .args([
            "setup",
            "--yes",
            "--no-login",
            "--no-install",
            "--codex",
            "2",
            "--claude",
            "1",
            "--opencode",
            "0",
        ])
        .assert()
        .success();
    let s = stdout(&out);
    assert_eq!(accounts(&e), ["claude-1", "codex-1", "codex-2"], "{s}");
    assert!(s.contains("kept for the orchestrator"), "{s}");
    assert!(s.contains("claude-sonnet"), "{s}");
    assert!(!s.contains("install:"), "{s}");
}

#[test]
fn running_twice_adds_nothing_unless_counts_are_asked() {
    let e = env_with_harnesses();
    let args = ["setup", "--yes", "--no-login", "--no-install"];
    cmd(&e).args(args).assert().success();
    let first = accounts(&e);
    let out = cmd(&e).args(args).assert().success();
    assert_eq!(accounts(&e), first);
    let t = stdout(&out);
    assert!(
        t.contains("2 accounts already set up: codex-1, opencode-1"),
        "{t}"
    );
    // Existing accounts that are still signed out get their sign-in command again.
    let out = cmd(&e)
        .args(["setup", "--yes", "--no-install"])
        .assert()
        .success();
    assert!(
        stdout(&out).contains("account login codex-1"),
        "{}",
        stdout(&out)
    );
    cmd(&e).args(args).args(["--codex", "1"]).assert().success();
    assert_eq!(accounts(&e), ["codex-1", "codex-2", "opencode-1"]);
}

#[test]
fn a_requested_harness_that_is_not_installed_fails_at_the_end_with_a_message() {
    let e = Env::new();
    let out = cmd(&e)
        .args(["setup", "--yes", "--no-login", "--no-install", "--agy", "1"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "agy was requested but its binary is not on PATH",
        ));
    let s = stdout(&out);
    assert!(accounts(&e).is_empty(), "{:?} {s}", accounts(&e));
}

#[test]
fn without_a_tty_new_accounts_get_their_sign_in_commands_printed() {
    let e = env_with_harnesses();
    let out = cmd(&e)
        .args(["setup", "--yes", "--no-install"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("account login codex-1"), "{s}");
}

#[test]
fn help_hides_internal_commands_orders_the_rest_and_points_at_setup() {
    let e = Env::new();
    let out = cmd(&e).arg("--help").assert().success();
    let s = stdout(&out);
    let cmds: Vec<&str> = s
        .split("Commands:\n")
        .nth(1)
        .unwrap()
        .split("\n\n")
        .next()
        .unwrap()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(
        cmds,
        [
            "setup",
            "init",
            "status",
            "account",
            "route",
            "runs",
            "join",
            "ui",
            "workflow",
            "agent",
            "models",
            "config",
            "doctor",
            "install",
            "uninstall",
            "migrate",
            "worktrees",
            "help"
        ],
        "{s}"
    );
    assert!(s.contains("Get started: superorchestrator setup"), "{s}");
    // Hidden commands still work.
    for c in ["exec", "daemon", "mcp"] {
        cmd(&e).args([c, "--help"]).assert().success();
    }
}

#[test]
fn json_with_yes_prints_one_summary_object_and_nothing_else() {
    let e = env_with_harnesses();
    let out = cmd(&e)
        .args(["--json", "setup", "--yes", "--no-login"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["created"], serde_json::json!(["codex-1", "opencode-1"]));
    assert_eq!(v["installed"], true);
    assert_eq!(v["signed_in_needed"].as_array().unwrap().len(), 2);
    assert!(v["doctor"]["ok"].is_number());
    assert!(
        e.home
            .join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );
}

#[test]
fn json_without_yes_and_without_a_tty_still_prints_the_plan() {
    let e = env_with_harnesses();
    let out = cmd(&e).args(["--json", "setup"]).assert().success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["dry_run"], true);
}

#[test]
fn setup_leaves_a_poisoned_parent_environment_alone() {
    let e = env_with_harnesses();
    let poison = e.tmp.path().join("poison");
    let dirs: Vec<_> = ["config", "data", "state", "cache", "claude", "runtime"]
        .iter()
        .map(|n| poison.join(n))
        .collect();
    for d in &dirs {
        std::fs::create_dir_all(d).unwrap();
    }
    // A command whose environment carries the poison, as a parent shell's would...
    let mut c = Command::cargo_bin("superorchestrator").unwrap();
    c.env("XDG_CONFIG_HOME", &dirs[0])
        .env("XDG_DATA_HOME", &dirs[1])
        .env("XDG_STATE_HOME", &dirs[2])
        .env("XDG_CACHE_HOME", &dirs[3])
        .env("CLAUDE_CONFIG_DIR", &dirs[4])
        .env("XDG_RUNTIME_DIR", &dirs[5]);
    // ...is isolated the way `Env::cmd` isolates every test command.
    common::scrub(&mut c);
    c.arg("--config-dir")
        .arg(&e.cfg)
        .env("HOME", &e.home)
        .env("PATH", &e.bin)
        .args(["setup", "--yes", "--no-login"])
        .assert()
        .success();
    for d in &dirs {
        assert_eq!(std::fs::read_dir(d).unwrap().count(), 0, "{}", d.display());
    }
    assert!(
        e.home
            .join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );
}

#[test]
fn no_claude_md_skips_the_claude_md_block() {
    let e = env_with_harnesses();
    cmd(&e)
        .args(["setup", "--yes", "--no-login", "--no-claude-md"])
        .assert()
        .success();
    assert!(
        e.home
            .join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );
    assert!(!e.home.join(".claude/CLAUDE.md").exists());
}

#[test]
fn a_second_setup_does_not_import_homes_that_install_and_probes_created() {
    let e = Env::new();
    // Fakes that create what the real harnesses create on first use, without signing in.
    e.script(
        "claude",
        "/bin/mkdir -p \"$HOME/.claude/backups\"; echo '{}' > \"$HOME/.claude.json\"; \
         echo '{}' > \"$HOME/.claude/backups/.claude.json.backup.1\"; echo claude 1.0",
    );
    e.script(
        "opencode",
        "/bin/mkdir -p \"$HOME/.local/share/opencode/log\" \"$HOME/.local/share/opencode/repos\"; \
         echo opencode 1.0",
    );
    let args = ["setup", "--yes", "--no-login"];
    let o1 = cmd(&e).args(args).assert().success();
    let first = accounts(&e);
    assert!(e.home.join(".claude/backups").is_dir(), "{}", stdout(&o1));
    assert!(e.home.join(".claude/backups").is_dir());
    assert!(e.home.join(".local/share/opencode/log").is_dir());
    let out = cmd(&e).args(args).assert().success();
    assert_eq!(accounts(&e), first);
    assert!(
        !first.iter().any(|n| n.ends_with("-acc1")),
        "unexpected imports: {first:?}"
    );
    let t = stdout(&out);
    assert!(
        !t.contains("claude-acc1") && !t.contains("opencode-acc1"),
        "{t}"
    );
}

#[test]
fn install_output_uses_tilde_paths_and_names_the_claude_registration() {
    let e = env_with_harnesses();
    let out = cmd(&e).args(["install"]).assert().success();
    let t = stdout(&out);
    assert!(
        t.contains("claude  (claude mcp add superorchestrator)"),
        "{t}"
    );
    assert!(!t.contains(e.home.to_str().unwrap()), "{t}");
    assert!(t.contains("~/"), "{t}");
    let out = cmd(&e)
        .args(["--json", "install", "--force"])
        .assert()
        .success();
    assert!(stdout(&out).contains(e.home.to_str().unwrap()));
}
