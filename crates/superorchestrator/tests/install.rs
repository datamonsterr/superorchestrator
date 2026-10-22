#![allow(clippy::unwrap_used)]

mod common;

use std::path::{Path, PathBuf};

use common::{Env, stdout};
use serde_json::{Value, json};
use so_core::{AccountFile, Harness};

fn exe() -> PathBuf {
    assert_cmd::cargo::cargo_bin("superorchestrator")
}

fn account(e: &Env, name: &str, harness: Harness, home: &Path, bin: &str) {
    std::fs::create_dir_all(home).unwrap();
    AccountFile::new(name, harness, &home.to_string_lossy(), bin)
        .unwrap()
        .save(&e.paths())
        .unwrap();
}

/// A fake `claude` that logs its argv and remembers `mcp add`.
fn fake_claude(e: &Env) -> PathBuf {
    let log = e.tmp.path().join("claude.log");
    let mark = e.tmp.path().join("claude.mark");
    e.script(
        "claude",
        &format!(
            "echo \"$@\" >> {log}\n\
             case \"$1 $2\" in\n\
             \"mcp get\") [ -f {mark} ] && {{ cat {mark}; exit 0; }}; exit 1;;\n\
             \"mcp add\") echo \"$@\" > {mark}; exit 0;;\n\
             \"mcp remove\") rm -f {mark}; exit 0;;\n\
             esac",
            log = log.display(),
            mark = mark.display()
        ),
    );
    log
}

fn log_lines(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

fn json_at(p: &Path) -> Value {
    serde_json::from_str(&read(p)).unwrap()
}

fn backups(e: &Env) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd {
            let p = entry.unwrap().path();
            if p.is_dir() {
                walk(&p, out);
            } else {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&e.cfg.join("legacy-backup"), &mut out);
    out.sort();
    out
}

struct Seeded {
    codex_default: PathBuf,
    codex_acc: PathBuf,
    opencode_default: PathBuf,
    opencode_acc: PathBuf,
    agy_default: PathBuf,
    agy_acc: PathBuf,
}

fn seed(e: &Env) -> Seeded {
    e.init();
    let h = &e.home;
    let s = Seeded {
        codex_default: h.join(".codex/config.toml"),
        codex_acc: h.join(".codex-acc2/config.toml"),
        opencode_default: h.join(".config/opencode/opencode.json"),
        opencode_acc: h.join(".oc1/config/opencode/opencode.json"),
        agy_default: h.join(".gemini/config/mcp_config.json"),
        agy_acc: h.join(".agy1/.gemini/config/mcp_config.json"),
    };
    account(
        e,
        "codex-acc2",
        Harness::Codex,
        &h.join(".codex-acc2"),
        "codex",
    );
    account(e, "oc1", Harness::Opencode, &h.join(".oc1"), "opencode");
    account(e, "agy1", Harness::Agy, &h.join(".agy1"), "agy");
    std::fs::create_dir_all(h.join(".agy1/.gemini")).unwrap();
    for (p, text) in [
        (
            &s.codex_default,
            "# keep me\nmodel = \"x\"\n\n[mcp_servers.other]\ncommand = \"o\"\n",
        ),
        (&s.codex_acc, "# acc comment\n[tui]\ntheme = \"dark\"\n"),
        (
            &s.opencode_default,
            r#"{"model":"m","mcp":{"ctx":{"type":"remote","url":"u"}}}"#,
        ),
        (
            &s.agy_default,
            r#"{"mcpServers":{"hr":{"command":"h","args":[]}}}"#,
        ),
    ] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    s
}

#[test]
fn install_registers_everywhere_and_is_idempotent_then_uninstall_reverses() {
    let e = Env::new();
    let log = fake_claude(&e);
    let s = seed(&e);
    let exe = exe();
    let exe_s = exe.to_string_lossy().into_owned();

    e.cmd().arg("install").assert().success();

    // claude
    let lines = log_lines(&log);
    let add = format!("mcp add --scope user superorchestrator -- {exe_s} mcp");
    assert!(lines.contains(&add), "{lines:?}");

    // codex: default and account, comments and other entries kept
    let c = read(&s.codex_default);
    assert!(c.contains("# keep me") && c.contains("[mcp_servers.other]"));
    assert!(c.contains("[mcp_servers.superorchestrator]"));
    assert!(c.contains(&format!("command = \"{exe_s}\"")));
    let c = read(&s.codex_acc);
    assert!(c.contains("# acc comment") && c.contains("theme = \"dark\""));
    assert!(c.contains("[mcp_servers.superorchestrator]"));

    // opencode
    let want = json!({"type":"local","command":[exe_s, "mcp"],"enabled":true});
    for p in [&s.opencode_default, &s.opencode_acc] {
        let v = json_at(p);
        assert_eq!(v["mcp"]["superorchestrator"], want, "{p:?}");
    }
    assert_eq!(json_at(&s.opencode_default)["model"], "m");
    assert_eq!(json_at(&s.opencode_default)["mcp"]["ctx"]["url"], "u");

    // agy
    for p in [&s.agy_default, &s.agy_acc] {
        let v = json_at(p);
        assert_eq!(v["mcpServers"]["superorchestrator"]["command"], exe_s);
        assert_eq!(v["mcpServers"]["superorchestrator"]["args"], json!(["mcp"]));
    }
    assert_eq!(json_at(&s.agy_default)["mcpServers"]["hr"]["command"], "h");

    // skills
    for d in [".claude", ".agents"] {
        let skill = read(&e.home.join(d).join("skills/superorchestrator/SKILL.md"));
        assert!(skill.starts_with("---\nname: superorchestrator\n"));
        assert!(skill.contains("codex-acc2"));
    }

    // backups: one per pre-existing changed file (files we created have none), mirrored under legacy-backup/<ts>/
    let b = backups(&e);
    assert_eq!(b.len(), 4, "{b:?}");
    let mirrored = b
        .iter()
        .find(|p| p.ends_with(s.codex_default.strip_prefix("/").unwrap()))
        .unwrap();
    assert!(read(mirrored).contains("# keep me"));
    assert!(!read(mirrored).contains("superorchestrator"));

    // second run changes nothing
    let before: Vec<String> = [&s.codex_default, &s.opencode_acc, &s.agy_acc]
        .iter()
        .map(|p| read(p))
        .collect();
    let n_log = log_lines(&log).len();
    let out = e.cmd().arg("install").assert().success();
    assert!(stdout(&out).contains("unchanged"));
    let after: Vec<String> = [&s.codex_default, &s.opencode_acc, &s.agy_acc]
        .iter()
        .map(|p| read(p))
        .collect();
    assert_eq!(before, after);
    assert_eq!(backups(&e).len(), 4);
    let new_lines = &log_lines(&log)[n_log..];
    assert!(
        new_lines.iter().all(|l| l.starts_with("mcp get")),
        "{new_lines:?}"
    );

    // uninstall removes only ours
    e.cmd().arg("uninstall").assert().success();
    assert!(log_lines(&log).contains(&"mcp remove --scope user superorchestrator".to_string()));
    let c = read(&s.codex_default);
    assert!(c.contains("# keep me") && c.contains("[mcp_servers.other]"));
    assert!(!c.contains("superorchestrator"));
    assert!(!read(&s.codex_acc).contains("superorchestrator"));
    assert!(read(&s.codex_acc).contains("# acc comment"));
    let v = json_at(&s.opencode_default);
    assert!(v["mcp"].get("superorchestrator").is_none());
    assert_eq!(v["mcp"]["ctx"]["url"], "u");
    assert!(json_at(&s.opencode_acc).get("mcp").is_none());
    let v = json_at(&s.agy_default);
    assert!(v["mcpServers"].get("superorchestrator").is_none());
    assert_eq!(v["mcpServers"]["hr"]["command"], "h");
    for d in [".claude", ".agents"] {
        assert!(!e.home.join(d).join("skills/superorchestrator").exists());
    }
}

#[test]
fn dry_run_changes_nothing() {
    let e = Env::new();
    let log = fake_claude(&e);
    let s = seed(&e);
    let before = read(&s.codex_default);
    let out = e.cmd().args(["install", "--dry-run"]).assert().success();
    let text = stdout(&out);
    assert!(text.contains("dry run"), "{text}");
    assert!(text.contains("create") || text.contains("update"), "{text}");
    assert_eq!(read(&s.codex_default), before);
    assert!(backups(&e).is_empty());
    assert!(!e.home.join(".claude/skills").exists());
    assert!(log_lines(&log).iter().all(|l| l.starts_with("mcp get")));
}

#[test]
fn foreign_skill_is_left_alone_and_missing_claude_is_skipped() {
    let e = Env::new();
    e.init();
    let skill = e.home.join(".claude/skills/superorchestrator/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "mine").unwrap();
    // A PATH without any `claude`.
    let out = e
        .cmd()
        .env("PATH", &e.bin)
        .arg("install")
        .assert()
        .success();
    let text = stdout(&out);
    assert!(text.contains("claude is not on PATH"), "{text}");
    assert_eq!(read(&skill), "mine");
    e.cmd()
        .env("PATH", &e.bin)
        .arg("uninstall")
        .assert()
        .success();
    assert_eq!(read(&skill), "mine");
    assert!(
        e.home
            .join(".agents/skills/superorchestrator")
            .read_dir()
            .is_err()
    );
}

#[test]
fn invalid_config_is_reported_and_not_overwritten() {
    let e = Env::new();
    fake_claude(&e);
    e.init();
    let p = e.home.join(".codex/config.toml");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, "= not toml").unwrap();
    e.cmd().arg("install").assert().failure();
    assert_eq!(read(&p), "= not toml");
}

#[test]
fn default_home_accounts_are_not_edited_twice() {
    let e = Env::new();
    fake_claude(&e);
    e.init();
    let h = &e.home;
    account(
        &e,
        "oc-default",
        Harness::Opencode,
        &h.join(".local/share/opencode"),
        "opencode",
    );
    account(
        &e,
        "codex-default",
        Harness::Codex,
        &h.join(".codex"),
        "codex",
    );
    let global = h.join(".config/opencode/opencode.json");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(&global, "{}").unwrap();
    std::fs::create_dir_all(h.join(".codex")).unwrap();
    let out = e.cmd().arg("install").assert().success();
    let text = stdout(&out);
    assert!(!h.join(".local/share/opencode/config").exists());
    assert!(!text.contains("opencode (oc-default)"), "{text}");
    assert!(!text.contains("codex (codex-default)"), "{text}");
    assert!(json_at(&global)["mcp"]["superorchestrator"].is_object());
    assert_eq!(text.matches(".codex/config.toml").count(), 1, "{text}");
}
