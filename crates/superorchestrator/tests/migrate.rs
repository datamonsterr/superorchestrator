#![allow(clippy::unwrap_used)]

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use common::{Env, stdout};
use serde_json::Value;

const SECRET: &str = "SECRET-TOKEN-do-not-print-9f3a";

fn write(p: &Path, text: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

fn future_epoch() -> i64 {
    chrono::Utc::now().timestamp() + 3 * 3600
}

/// Seeds the old tooling the SPEC 19.2 cutover migrates from.
fn seed(e: &Env) {
    let h = &e.home;
    // Credential homes (auth files hold a secret that must never be printed).
    write(&h.join(".codex/auth.json"), SECRET);
    write(&h.join(".codex-acc2/auth.json"), SECRET);
    write(
        &h.join(".agent-farm/profiles/agy_worker_1/.gemini/oauth_creds.json"),
        SECRET,
    );
    std::fs::create_dir_all(h.join(".local/share/opencode")).unwrap();
    write(&h.join(".local/share/opencode/auth.json"), SECRET);
    // An old, unmanaged wrapper.
    write(
        &h.join(".local/bin/codex-acc2"),
        "#!/bin/sh\nexec codex-old\n",
    );
    // Old tooling to remove.
    write(&h.join(".claude/orchestrator/run.sh"), "old orchestrate");
    write(&h.join(".claude/orchestrator-state.json"), "{}");
    write(&h.join(".claude/orchestrator-state-2.json"), "{}");
    write(
        &h.join(".claude/skills/coding-agent-management/SKILL.md"),
        "old",
    );
    write(&h.join(".local/bin/check-account-usage"), "#!/bin/sh\n");
    write(&h.join(".local/bin/add-agy-worker"), "#!/bin/sh\n");
    // jev-router-mcp entries next to unrelated ones.
    write(
        &h.join(".codex/config.toml"),
        "model = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n\n[mcp_servers.jev-router-mcp]\ncommand = \"jev\"\n",
    );
    write(
        &h.join(".config/opencode/opencode.json"),
        r#"{"mcp":{"keep":{"type":"remote","url":"u"},"jev-router-mcp":{"type":"local","command":["jev"]}}}"#,
    );
    write(
        &h.join(".gemini/config/mcp_config.json"),
        r#"{"mcpServers":{"keep":{"command":"k"},"jev-router-mcp":{"command":"jev"}}}"#,
    );
    // Delegation rules.
    write(
        &h.join(".claude/CLAUDE.md"),
        "- Be brief\n- Delegate all implementation through the `orchestrate` skill\n- Check docs first\n",
    );
    // Cooldowns: agy-acc1 is exhausted, codex-acc2's is long past.
    let state = h.join(".local/state/coding-agent/.state");
    write(
        &state.join("agy/agy-acc1.meta"),
        &format!("exhausted_until={}\n", future_epoch()),
    );
    write(
        &state.join("codex/codex-acc2.meta"),
        "exhausted_until=1000\n",
    );
    // Router choices.
    write(
        &h.join(".config/jev-router/config.json"),
        r#"{"choices":[{"id":"legacy-pick","harness":"codex","model":"gpt-x","effort":"high","roles":["worker"],"description":"from jev","enabled":true}]}"#,
    );
}

fn cmd(e: &Env) -> assert_cmd::Command {
    let mut c = e.cmd();
    // No real `claude` can be found; env vars of the caller must not leak in.
    c.env("PATH", &e.bin).env_remove("CLAUDE_CONFIG_DIR");
    c
}

/// Every file and symlink under `home` and the config dir, with its content.
fn snapshot(e: &Env) -> BTreeMap<PathBuf, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for ent in rd {
            let p = ent.unwrap().path();
            let rel = p.strip_prefix(root).unwrap().to_path_buf();
            let meta = std::fs::symlink_metadata(&p).unwrap();
            if meta.is_dir() {
                out.insert(rel, "<dir>".into());
                walk(root, &p, out);
            } else {
                let body = std::fs::read(&p).map(|b| String::from_utf8_lossy(&b).into_owned());
                out.insert(rel, body.unwrap_or_default());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(e.tmp.path(), &e.tmp.path().join("home"), &mut out);
    walk(e.tmp.path(), &e.cfg, &mut out);
    out
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn backup_dirs(e: &Env) -> Vec<PathBuf> {
    let root = e.cfg.join("legacy-backup");
    let Ok(rd) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut v: Vec<_> = rd.map(|d| d.unwrap().path()).collect();
    v.sort();
    v
}

fn assert_clean(label: &str, text: &str) {
    assert!(
        !text.contains(SECRET),
        "{label} leaked a credential:\n{text}"
    );
}

#[test]
fn dry_run_is_the_default_prints_the_plan_and_changes_nothing() {
    let e = Env::new();
    seed(&e);
    let before = snapshot(&e);
    let out = cmd(&e).arg("migrate").assert().success();
    let s = stdout(&out);
    assert!(s.contains("dry run"), "{s}");
    for want in [
        "codex-acc1",
        "codex-acc2",
        "agy-acc1",
        "opencode-acc1",
        "claude-acc1",
        "orchestrator-state.json",
        "orchestrator-state-2.json",
        "coding-agent-management",
        "check-account-usage",
        "add-agy-worker",
        "jev-router-mcp",
        "legacy-pick",
    ] {
        assert!(s.contains(want), "plan lacks {want}:\n{s}");
    }
    // The CLAUDE.md diff is shown first.
    assert!(
        s.contains("-- Delegate all implementation through the `orchestrate` skill"),
        "{s}"
    );
    assert!(s.contains("+- "), "{s}");
    // Only the live cooldown is planned.
    let line = |words: &[&str]| s.lines().any(|l| words.iter().all(|w| l.contains(w)));
    assert!(line(&["cooldown", "flag", "agy-acc1", "until"]), "{s}");
    assert!(!line(&["flag", "codex-acc2"]), "{s}");
    assert!(line(&["choices", "create", "legacy-pick"]), "{s}");
    assert!(line(&["rules", "update", "CLAUDE.md"]), "{s}");
    assert!(s.contains("--- ~/.claude/CLAUDE.md"), "{s}");
    assert_eq!(before, snapshot(&e), "dry run changed files");
    assert!(!e.cfg.exists());
    assert_clean("dry run", &s);
}

#[test]
fn dry_run_json_describes_the_plan() {
    let e = Env::new();
    seed(&e);
    let out = cmd(&e).args(["migrate", "--json"]).assert().success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["changed"], true);
    let items = v["items"].as_array().unwrap();
    assert!(items.iter().any(|i| i["step"] == "accounts"
        && i["action"] == "import"
        && i["target"].as_str().unwrap().contains("codex-acc2")));
    assert!(v["diff"].as_str().unwrap().contains("orchestrate"));
    assert_eq!(v["legacy"], true);
    // Nothing was backed up, so no backup directory is reported.
    assert!(v["backup_dir"].is_null());
}

#[test]
fn apply_performs_every_step_with_backups() {
    let e = Env::new();
    seed(&e);
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert_clean("apply", &stdout(&out));
    let h = &e.home;

    // 2. accounts imported, wrappers rewritten (old one replaced).
    for n in [
        "codex-acc1",
        "codex-acc2",
        "agy-acc1",
        "opencode-acc1",
        "claude-acc1",
    ] {
        assert!(e.cfg.join(format!("accounts/{n}.toml")).is_file(), "{n}");
        assert!(h.join(format!(".local/bin/{n}")).is_file(), "wrapper {n}");
    }
    let w = read(&h.join(".local/bin/codex-acc2"));
    assert!(w.contains("managed-by: superorchestrator"), "{w}");

    // 3. only the live cooldown becomes a flag.
    let store = e.store();
    let agy = so_core::AccountFile::load(&e.cfg.join("accounts/agy-acc1.toml")).unwrap();
    for pool in &agy.pools {
        let f = store.get_flag("agy-acc1", pool).unwrap().unwrap();
        assert_eq!(f.reason, "migrated");
    }
    assert!(store.get_flag("codex-acc2", "default").unwrap().is_none());

    // 4. old tooling is gone, unrelated entries stay.
    for gone in [
        ".claude/orchestrator",
        ".claude/orchestrator-state.json",
        ".claude/orchestrator-state-2.json",
        ".claude/skills/coding-agent-management",
        ".local/bin/check-account-usage",
        ".local/bin/add-agy-worker",
    ] {
        assert!(!h.join(gone).exists(), "{gone} still exists");
    }
    assert!(h.join(".local/bin").is_dir());
    let codex = read(&h.join(".codex/config.toml"));
    assert!(!codex.contains("jev-router-mcp"), "{codex}");
    assert!(codex.contains("[mcp_servers.other]"));
    assert!(codex.contains("[mcp_servers.superorchestrator]"), "{codex}");
    let oc: Value = serde_json::from_str(&read(&h.join(".config/opencode/opencode.json"))).unwrap();
    assert!(oc["mcp"].get("jev-router-mcp").is_none());
    assert_eq!(oc["mcp"]["keep"]["url"], "u");
    let agy: Value =
        serde_json::from_str(&read(&h.join(".gemini/config/mcp_config.json"))).unwrap();
    assert!(agy["mcpServers"].get("jev-router-mcp").is_none());
    assert_eq!(agy["mcpServers"]["keep"]["command"], "k");

    // 5. only the orchestrate rule changed.
    let md = read(&h.join(".claude/CLAUDE.md"));
    assert!(md.starts_with("- Be brief\n"), "{md}");
    assert!(md.ends_with("- Check docs first\n"), "{md}");
    assert!(!md.contains("`orchestrate`"), "{md}");
    assert!(md.contains("superorchestrator MCP tools"), "{md}");

    // 6. choices seeded.
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    let c = cfg.choices.iter().find(|c| c.id == "legacy-pick").unwrap();
    assert_eq!(c.model, "gpt-x");

    // MCP and skill installed.
    assert!(
        h.join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );
    assert!(
        h.join(".agents/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );

    // 1. everything touched is backed up under one legacy-backup/<ts>/.
    let dirs = backup_dirs(&e);
    assert_eq!(dirs.len(), 1, "{dirs:?}");
    let b = |rel: &str| dirs[0].join(h.strip_prefix("/").unwrap()).join(rel);
    assert_eq!(read(&b(".claude/orchestrator/run.sh")), "old orchestrate");
    assert!(b(".claude/orchestrator-state.json").is_file());
    assert!(b(".claude/orchestrator-state-2.json").is_file());
    assert!(b(".claude/skills/coding-agent-management/SKILL.md").is_file());
    assert!(b(".local/bin/check-account-usage").is_file());
    assert!(b(".local/bin/add-agy-worker").is_file());
    assert_eq!(
        read(&b(".local/bin/codex-acc2")),
        "#!/bin/sh\nexec codex-old\n"
    );
    assert!(read(&b(".codex/config.toml")).contains("jev-router-mcp"));
    assert!(read(&b(".config/opencode/opencode.json")).contains("jev-router-mcp"));
    assert!(read(&b(".gemini/config/mcp_config.json")).contains("jev-router-mcp"));
    assert!(read(&b(".claude/CLAUDE.md")).contains("`orchestrate`"));
    assert!(dirs[0].parent().unwrap().ends_with("legacy-backup"));
    // Credentials are linked, never copied.
    let mut leaked = Vec::new();
    for (p, body) in snapshot(&e) {
        if p.starts_with("cfg/legacy-backup") && body.contains(SECRET) {
            leaked.push(p);
        }
    }
    assert!(leaked.is_empty(), "{leaked:?}");
    // Credential homes are untouched.
    assert_eq!(read(&h.join(".codex/auth.json")), SECRET);
}

#[test]
fn second_apply_changes_nothing() {
    let e = Env::new();
    seed(&e);
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let before = snapshot(&e);
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("nothing to migrate"), "{s}");
    assert_eq!(before, snapshot(&e), "second apply changed files");
    assert_eq!(backup_dirs(&e).len(), 1);
    // A repeat without --yes needs no confirmation either: there is nothing to confirm.
    cmd(&e).args(["migrate", "--apply"]).assert().success();
    // And the dry run reports no change.
    let out = cmd(&e).args(["migrate", "--json"]).assert().success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["changed"], false);
}

#[test]
fn apply_without_tty_and_without_yes_refuses_and_changes_nothing() {
    let e = Env::new();
    seed(&e);
    let before = snapshot(&e);
    cmd(&e)
        .args(["migrate", "--apply"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("--yes"));
    assert_eq!(before, snapshot(&e));
}

#[test]
fn credentials_never_appear_in_any_output() {
    let e = Env::new();
    seed(&e);
    for args in [
        vec!["migrate"],
        vec!["migrate", "--json"],
        vec!["migrate", "--apply", "--yes", "--json"],
        vec!["migrate", "--apply", "--yes"],
    ] {
        let out = cmd(&e).args(&args).assert().success();
        let o = out.get_output();
        assert_clean("stdout", &String::from_utf8_lossy(&o.stdout));
        assert_clean("stderr", &String::from_utf8_lossy(&o.stderr));
    }
}

#[test]
fn unreadable_jev_config_entries_are_skipped_not_fatal() {
    let e = Env::new();
    write(
        &e.home.join(".config/jev-router/config.json"),
        r#"{"choices":[{"id":"ok","harness":"codex","model":"m"},{"nope":1}]}"#,
    );
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    assert!(cfg.choices.iter().any(|c| c.id == "ok"));
}

fn minimal_env() -> Env {
    let e = Env::new();
    // A legacy artifact so that migrate has something to do.
    write(
        &e.home.join(".local/bin/check-account-usage"),
        "#!/bin/sh\n",
    );
    e
}

fn tree_has_secret(root: &Path) -> bool {
    fn walk(p: &Path) -> bool {
        let meta = std::fs::symlink_metadata(p).unwrap();
        if meta.is_dir() {
            std::fs::read_dir(p)
                .unwrap()
                .any(|d| walk(&d.unwrap().path()))
        } else if meta.is_file() {
            std::fs::read_to_string(p).is_ok_and(|t| t.contains(SECRET))
        } else {
            false
        }
    }
    root.exists() && walk(root)
}

#[test]
fn clean_machine_has_nothing_to_migrate_and_installs_nothing() {
    let e = Env::new();
    let before = snapshot(&e);
    for args in [vec!["migrate"], vec!["migrate", "--apply"]] {
        let out = cmd(&e).args(&args).assert().success();
        let s = stdout(&out);
        assert!(s.contains("nothing to migrate"), "{s}");
        assert!(s.contains("superorchestrator install"), "{s}");
    }
    assert_eq!(before, snapshot(&e));
}

#[test]
fn a_symlinked_legacy_dir_is_unlinked_not_followed() {
    let e = Env::new();
    let outside = e.home.join("elsewhere");
    write(&outside.join("keep.txt"), SECRET);
    let link = e.home.join(".claude/skills/coding-agent-management");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert!(std::fs::symlink_metadata(&link).is_err(), "link remains");
    assert_eq!(read(&outside.join("keep.txt")), SECRET);
    let backed = backup_dirs(&e)[0]
        .join(e.home.strip_prefix("/").unwrap())
        .join(".claude/skills/coding-agent-management");
    assert!(std::fs::symlink_metadata(&backed).unwrap().is_symlink());
    assert!(!tree_has_secret(&backup_dirs(&e)[0]));
}

#[test]
fn a_symlinked_foreign_wrapper_never_touches_the_file_it_points_at() {
    let e = Env::new();
    write(&e.home.join(".codex-acc2/auth.json"), SECRET);
    std::fs::create_dir_all(e.home.join(".local/bin")).unwrap();
    std::os::unix::fs::symlink(
        e.home.join(".codex-acc2/auth.json"),
        e.home.join(".local/bin/codex-acc2"),
    )
    .unwrap();
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert_eq!(read(&e.home.join(".codex-acc2/auth.json")), SECRET);
    let w = read(&e.home.join(".local/bin/codex-acc2"));
    assert!(w.contains("managed-by: superorchestrator"), "{w}");
    assert!(!tree_has_secret(&backup_dirs(&e)[0]));
}

#[test]
fn legacy_trees_are_backed_up_as_they_are() {
    // Documented behaviour: a legacy tree is copied verbatim, whatever it holds.
    let e = Env::new();
    write(
        &e.home.join(".claude/orchestrator/notes/secret.txt"),
        SECRET,
    );
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert_clean("apply", &stdout(&out));
    assert!(tree_has_secret(&backup_dirs(&e)[0]));
    assert!(!e.home.join(".claude/orchestrator").exists());
}

#[test]
fn a_broken_config_is_an_error_item_while_the_other_steps_still_run() {
    for broken in ["{ not json", "{\n // a comment\n \"mcp\": {}\n}"] {
        let e = minimal_env();
        write(&e.home.join(".config/opencode/opencode.json"), broken);
        write(
            &e.home.join(".claude/CLAUDE.md"),
            "- Use the `orchestrate` skill\n",
        );
        write(&e.home.join(".codex-acc2/auth.json"), SECRET);
        let out = cmd(&e)
            .args(["migrate", "--apply", "--yes", "--json"])
            .assert()
            .code(1);
        let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
        assert_eq!(v["ok"], false);
        let items = v["items"].as_array().unwrap();
        assert!(
            items.iter().any(|i| i["action"] == "error"
                && i["target"].as_str().unwrap().contains("opencode.json")),
            "{items:?}"
        );
        // The rest was done.
        assert!(!e.home.join(".local/bin/check-account-usage").exists());
        assert!(e.cfg.join("accounts/codex-acc2.toml").is_file());
        assert!(!read(&e.home.join(".claude/CLAUDE.md")).contains("`orchestrate`"));
        // The broken file was left alone.
        assert_eq!(read(&e.home.join(".config/opencode/opencode.json")), broken);
    }
}

#[test]
fn only_the_first_orchestrate_rule_is_rewritten_and_the_rest_are_noted() {
    let e = Env::new();
    let md = "- Use the `orchestrate` skill for all work\r\n\
              - Never use the orchestrate skill for tiny edits\r\n\
              - Run the orchestrate skill again after review\r\n\
              - Keep the orchestrator-state files out of git\r\n";
    write(&e.home.join(".claude/CLAUDE.md"), md);
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let s = stdout(&out);
    let new = read(&e.home.join(".claude/CLAUDE.md"));
    let lines: Vec<&str> = new.split("\r\n").collect();
    assert!(lines[0].starts_with("- Delegate implementation through the superorchestrator"));
    assert_eq!(lines[1], "- Never use the orchestrate skill for tiny edits");
    assert_eq!(lines[2], "- Run the orchestrate skill again after review");
    assert_eq!(lines[3], "- Keep the orchestrator-state files out of git");
    assert!(
        !new.replace("\r\n", "").contains('\n'),
        "CRLF lost: {new:?}"
    );
    // The untouched rules are listed for the user.
    assert!(s.contains("line 2") && s.contains("line 3"), "{s}");
    assert!(!s.contains("line 4"), "{s}");
    // A second apply changes nothing.
    let before = snapshot(&e);
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert_eq!(before, snapshot(&e));
}

#[test]
fn only_real_state_files_are_removed() {
    let e = Env::new();
    for f in [
        "orchestrator-state.json",
        "orchestrator-state-12.json",
        "orchestrator-state-x.json",
        "orchestrator-state.json.bak",
    ] {
        write(&e.home.join(".claude").join(f), "{}");
    }
    std::fs::create_dir_all(e.home.join(".claude/orchestrator-state-3.json")).unwrap();
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let c = e.home.join(".claude");
    assert!(!c.join("orchestrator-state.json").exists());
    assert!(!c.join("orchestrator-state-12.json").exists());
    assert!(c.join("orchestrator-state-x.json").exists());
    assert!(c.join("orchestrator-state.json.bak").exists());
    assert!(c.join("orchestrator-state-3.json").is_dir());
}

#[test]
fn choices_are_not_written_into_an_invalid_config() {
    let e = Env::new();
    write(
        &e.home.join(".config/jev-router/config.json"),
        r#"{"choices":[{"id":"x","harness":"codex","model":"m"}]}"#,
    );
    let broken = "limits = \"not a table\"\n";
    write(&e.cfg.join("config.toml"), broken);
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .code(1)
        .stderr(predicates::str::contains("config.toml"));
    assert_eq!(read(&e.cfg.join("config.toml")), broken);
}

#[test]
fn cooldowns_for_unknown_accounts_are_skipped_and_stay_idempotent() {
    let e = Env::new();
    write(
        &e.home
            .join(".local/state/coding-agent/.state/codex/codex-ghost.meta"),
        &format!("exhausted_until={}\n", future_epoch()),
    );
    write(&e.home.join(".local/bin/add-agy-worker"), "#!/bin/sh\n");
    std::fs::create_dir_all(e.home.join(".claude")).unwrap();
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let before = snapshot(&e);
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("nothing to migrate"), "{s}");
    assert_eq!(before, snapshot(&e));
}

#[test]
fn installing_the_skill_does_not_create_a_claude_account_on_the_next_run() {
    let e = Env::new();
    write(&e.home.join(".codex/auth.json"), SECRET);
    write(&e.home.join(".local/bin/add-agy-worker"), "#!/bin/sh\n");
    assert!(!e.home.join(".claude").exists());
    cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    assert!(
        e.home
            .join(".claude/skills/use-superorchestrator-mcp/SKILL.md")
            .is_file()
    );
    let before = snapshot(&e);
    let out = cmd(&e)
        .args(["migrate", "--apply", "--yes"])
        .assert()
        .success();
    let s = stdout(&out);
    assert!(s.contains("nothing to migrate"), "{s}");
    assert_eq!(before, snapshot(&e));
    assert!(!e.cfg.join("accounts/claude-acc1.toml").exists());
}
