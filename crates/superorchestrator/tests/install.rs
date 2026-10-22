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

/// `install` command with PATH limited to the test bin dir (so only the fake `claude`
/// can be found, never a real one).
fn cmd(e: &Env) -> assert_cmd::Command {
    let mut c = e.cmd();
    c.env("PATH", &e.bin).env_remove("CLAUDE_CONFIG_DIR");
    c
}

/// A fake `claude` that logs its argv and keeps its user-scope MCP entry in `$HOME/.claude.json`
/// like the real one (`mcp add` fails when the name exists). Uses only builtins and absolute
/// paths, since PATH holds nothing else.
fn fake_claude(e: &Env) -> PathBuf {
    let log = e.tmp.path().join("claude.log");
    e.script(
        "claude",
        &format!(
            "echo \"$@\" >> {log}\n\
             CFG=\"$HOME/.claude.json\"\n\
             case \"$1 $2\" in\n\
             \"mcp add\")\n\
               if [ -f \"$CFG\" ]; then echo 'MCP server superorchestrator already exists in user config' >&2; exit 1; fi\n\
               printf '{{\"mcpServers\":{{\"superorchestrator\":{{\"command\":\"%s\",\"args\":[\"%s\"]}}}}}}' \"$7\" \"$8\" > \"$CFG\"; exit 0;;\n\
             \"mcp remove\") /bin/rm -f \"$CFG\"; exit 0;;\n\
             esac",
            log = log.display(),
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

    cmd(&e).arg("install").assert().success();

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
    let out = cmd(&e).arg("install").assert().success();
    assert!(stdout(&out).contains("unchanged"));
    let after: Vec<String> = [&s.codex_default, &s.opencode_acc, &s.agy_acc]
        .iter()
        .map(|p| read(p))
        .collect();
    assert_eq!(before, after);
    assert_eq!(backups(&e).len(), 4);
    // The registration is recognised from ~/.claude.json: claude is not run again.
    assert_eq!(log_lines(&log).len(), n_log);

    // uninstall removes only ours
    cmd(&e).arg("uninstall").assert().success();
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
    let out = cmd(&e).args(["install", "--dry-run"]).assert().success();
    let text = stdout(&out);
    assert!(text.contains("dry run"), "{text}");
    assert!(text.contains("create") || text.contains("update"), "{text}");
    assert_eq!(read(&s.codex_default), before);
    assert!(backups(&e).is_empty());
    assert!(!e.home.join(".claude/skills").exists());
    assert!(log_lines(&log).is_empty());
}

#[test]
fn foreign_skill_is_left_alone_and_missing_claude_is_skipped() {
    let e = Env::new();
    e.init();
    let skill = e.home.join(".claude/skills/superorchestrator/SKILL.md");
    std::fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::fs::write(&skill, "mine").unwrap();
    // A PATH without any `claude`.
    let out = cmd(&e).arg("install").assert().success();
    let text = stdout(&out);
    assert!(text.contains("claude is not on PATH"), "{text}");
    assert_eq!(read(&skill), "mine");
    cmd(&e).arg("uninstall").assert().success();
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
    cmd(&e).arg("install").assert().failure();
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
    let out = cmd(&e).arg("install").assert().success();
    let text = stdout(&out);
    assert!(!h.join(".local/share/opencode/config").exists());
    assert!(!text.contains("opencode (oc-default)"), "{text}");
    assert!(!text.contains("codex (codex-default)"), "{text}");
    assert!(json_at(&global)["mcp"]["superorchestrator"].is_object());
    assert_eq!(text.matches(".codex/config.toml").count(), 1, "{text}");
}

fn mode(p: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[test]
fn writes_are_atomic_keep_mode_and_symlinks_and_new_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    fake_claude(&e);
    let s = seed(&e);
    // The default codex config is a symlink to a dotfiles repo, with a looser mode.
    let real = e.home.join("dotfiles/codex.toml");
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::rename(&s.codex_default, &real).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
    std::os::unix::fs::symlink(&real, &s.codex_default).unwrap();

    cmd(&e).arg("install").assert().success();

    assert!(s.codex_default.is_symlink());
    assert!(read(&real).contains("[mcp_servers.superorchestrator]"));
    assert_eq!(mode(&real), 0o640);
    // Files and directories install created are private.
    assert_eq!(mode(&s.opencode_acc), 0o600);
    assert_eq!(mode(s.opencode_acc.parent().unwrap()), 0o700);
    let b = backups(&e);
    let root = b[0]
        .ancestors()
        .find(|p| p.parent().is_some_and(|q| q.ends_with("legacy-backup")))
        .unwrap();
    assert_eq!(mode(root), 0o700);
    // The backup directory name is unique per run: <ts>-<millis>-<pid>.
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(name.split('-').count(), 3, "{name}");
    // No temp files are left behind.
    for d in [real.parent().unwrap(), s.opencode_acc.parent().unwrap()] {
        for f in std::fs::read_dir(d).unwrap() {
            let n = f.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!n.contains(".tmp."), "{n}");
        }
    }
}

#[test]
fn backups_never_overwrite_an_earlier_run() {
    let e = Env::new();
    fake_claude(&e);
    let s = seed(&e);
    cmd(&e).arg("install").assert().success();
    cmd(&e).arg("uninstall").assert().success();
    cmd(&e).arg("install").assert().success();
    // Three runs changed the default codex config: three distinct backups of it.
    let n = backups(&e)
        .iter()
        .filter(|p| p.ends_with(s.codex_default.strip_prefix("/").unwrap()))
        .count();
    assert_eq!(n, 3);
}

#[test]
fn jsonc_config_is_not_shadowed_by_a_new_json() {
    let e = Env::new();
    e.init();
    let dir = e.home.join(".config/opencode");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("opencode.jsonc"), "{ // mine\n}").unwrap();
    let out = cmd(&e).arg("install").assert().success();
    assert!(stdout(&out).contains(".jsonc config exists"));
    assert!(!dir.join("opencode.json").exists());
}

#[test]
fn stale_claude_entry_is_replaced_and_a_failed_readd_is_reported_loudly() {
    let e = Env::new();
    let log = fake_claude(&e);
    e.init();
    std::fs::write(
        e.home.join(".claude.json"),
        r#"{"mcpServers":{"superorchestrator":{"command":"/old","args":["mcp"]}}}"#,
    )
    .unwrap();
    cmd(&e).arg("install").assert().success();
    let lines = log_lines(&log);
    assert_eq!(lines.len(), 3, "{lines:?}");
    assert!(lines[0].starts_with("mcp add"));
    assert_eq!(lines[1], "mcp remove --scope user superorchestrator");
    assert!(lines[2].starts_with("mcp add"));
    assert!(read(&e.home.join(".claude.json")).contains(&exe().to_string_lossy().into_owned()));

    // A claude whose add always fails: the removal must be reported, not hidden.
    let e = Env::new();
    e.init();
    e.script(
        "claude",
        "case \"$2\" in add) echo 'already exists' >&2; exit 1;; *) exit 0;; esac",
    );
    std::fs::write(
        e.home.join(".claude.json"),
        r#"{"mcpServers":{"superorchestrator":{"command":"/old","args":["mcp"]}}}"#,
    )
    .unwrap();
    let assert = cmd(&e).arg("install").assert().failure();
    let text = stdout(&assert);
    assert!(
        text.contains("registration removed but re-add failed"),
        "{text}"
    );
}

#[test]
fn arguments_must_match_exactly_to_count_as_registered() {
    let e = Env::new();
    let log = fake_claude(&e);
    e.init();
    let exe_s = exe().to_string_lossy().into_owned();
    std::fs::write(
        e.home.join(".claude.json"),
        format!(r#"{{"mcpServers":{{"superorchestrator":{{"command":"{exe_s}","args":["mcp","--x"]}}}}}}"#),
    )
    .unwrap();
    cmd(&e).arg("install").assert().success();
    assert!(!log_lines(&log).is_empty());
}
