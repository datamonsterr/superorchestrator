#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use std::os::unix::fs::PermissionsExt;

#[test]
fn init_creates_layout_and_is_idempotent() {
    let e = Env::new();
    let first = e.cmd().arg("init").assert().success();
    assert!(stdout(&first).contains("created"));
    for d in ["accounts", "agents", "workflows", "runs", "logs"] {
        assert!(e.cfg.join(d).is_dir(), "{d}");
    }
    let mode = std::fs::metadata(&e.cfg).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700);
    assert!(e.cfg.join("models.toml").is_file());

    // A user edit survives a second init.
    let cfg_path = e.cfg.join("config.toml");
    let mut text = std::fs::read_to_string(&cfg_path).unwrap();
    text.push_str("\n# my edit\n");
    std::fs::write(&cfg_path, &text).unwrap();
    let second = e.cmd().arg("init").assert().success();
    let out = stdout(&second);
    assert!(!out.contains("created"), "{out}");
    assert_eq!(std::fs::read_to_string(&cfg_path).unwrap(), text);
}

#[test]
fn init_prints_the_next_steps_in_order() {
    let e = Env::new();
    let out = stdout(&e.cmd().arg("init").assert().success());
    let tail = out
        .split("Next steps:")
        .nth(1)
        .expect("a next steps section");
    let at = |needle: &str| {
        tail.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing: {tail}"))
    };
    assert!(
        tail.trim_start().starts_with("1. superorchestrator setup"),
        "{tail}"
    );
    assert!(at("superorchestrator setup") < at("superorchestrator doctor"));
    assert!(at("superorchestrator doctor") < at("Claude Code and ask"));
    // JSON output carries them too, and a repeat run still prints them.
    let json = stdout(&e.cmd().args(["--json", "init"]).assert().success());
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["next_steps"].as_array().unwrap().len(), 3);
    assert!(stdout(&e.cmd().arg("init").assert().success()).contains("Next steps:"));
}

#[test]
fn seeded_levels_parse_with_core_config() {
    let e = Env::new();
    e.init();
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    let refs: Vec<String> = cfg.choices().iter().map(|c| c.id()).collect();
    assert!(refs.len() >= 8, "{refs:?}");
    assert!(refs.contains(&"agy/gemini-3.8-flash-medium/medium".to_string()));
    assert!(refs.contains(&"agy/claude-sonnet-4-6/thinking".to_string()));
    // Codex refs are listed: without a signed-in account they are just not eligible.
    assert!(
        cfg.choices()
            .iter()
            .any(|c| c.harness == so_core::Harness::Codex)
    );
    // Claude stays out of the seed: auto routing never spends it unless the user adds a line.
    assert!(
        cfg.choices()
            .iter()
            .all(|c| c.harness != so_core::Harness::Claude)
    );
}

#[test]
fn init_seeds_levels_that_validate_and_cover_the_scale() {
    let e = Env::new();
    e.init();
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    let l: Vec<_> = cfg
        .router
        .levels
        .iter()
        .map(|l| (l.name.as_str(), l.min, l.max, l.models.len()))
        .collect();
    assert_eq!(
        l,
        [
            ("trivial", 0, 12, 2),
            ("simple", 13, 37, 2),
            ("moderate", 38, 62, 2),
            ("complex", 63, 87, 3),
            ("expert", 88, 100, 3),
        ]
    );
}

#[test]
fn init_leaves_an_existing_config_without_levels_alone() {
    let e = Env::new();
    std::fs::create_dir_all(&e.cfg).unwrap();
    let path = e.cfg.join("config.toml");
    std::fs::write(&path, "[limits]\nglobal_cap = 3\n").unwrap();
    e.init();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "[limits]\nglobal_cap = 3\n"
    );
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    assert_eq!(cfg.router.levels.len(), 3);
}

#[test]
fn config_get_set_roundtrip_and_validation() {
    let e = Env::new();
    e.init();
    e.cmd()
        .args(["config", "get", "limits.global_cap"])
        .assert()
        .success()
        .stdout("16\n");
    e.cmd()
        .args(["config", "set", "limits.global_cap", "4"])
        .assert()
        .success();
    e.cmd()
        .args(["config", "get", "limits.global_cap"])
        .assert()
        .success()
        .stdout("4\n");
    // Comments survive a set.
    let text = std::fs::read_to_string(e.cfg.join("config.toml")).unwrap();
    assert!(text.contains("# superorchestrator configuration."));
    assert!(so_core::Config::load(&e.paths()).unwrap().choices().len() >= 8);

    // Wrong type and unknown key are rejected and change nothing.
    e.cmd()
        .args(["config", "set", "limits.global_cap", "lots"])
        .assert()
        .failure();
    e.cmd()
        .args(["config", "set", "nope.key", "1"])
        .assert()
        .failure();
    e.cmd()
        .args(["config", "get", "limits.global_cap"])
        .assert()
        .stdout("4\n");
}

#[test]
fn config_get_unset_optional_key_prints_nothing_and_exits_3() {
    let e = Env::new();
    e.init();
    e.cmd()
        .args(["config", "get", "ui.token"])
        .assert()
        .code(3)
        .stdout("");
}

#[test]
fn config_edit_validates_after_editor() {
    let e = Env::new();
    e.init();
    let bad = e.script("bad-editor", "echo 'limits = 7' >> \"$1\"");
    e.cmd()
        .args(["config", "edit"])
        .env("EDITOR", &bad)
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
    let good = e.script("good-editor", "exit 0");
    // Still invalid from the previous edit.
    e.cmd()
        .args(["config", "edit"])
        .env("EDITOR", &good)
        .assert()
        .failure();
    std::fs::remove_file(e.cfg.join("config.toml")).unwrap();
    e.init();
    e.cmd()
        .args(["config", "edit"])
        .env("EDITOR", &good)
        .assert()
        .success();
}

#[test]
fn usage_errors_exit_2() {
    let e = Env::new();
    e.cmd().arg("no-such-command").assert().code(2);
    e.cmd().args(["account", "flag"]).assert().code(2);
}

#[test]
fn json_output_for_init_and_config() {
    let e = Env::new();
    let out = e.cmd().args(["--json", "init"]).assert().success();
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["ok"], true);
    assert!(
        v["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["status"] == "created")
    );
    let out = e
        .cmd()
        .args(["--json", "config", "set", "limits.global_cap", "5"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["ok"], true);
    let out = e
        .cmd()
        .args(["--json", "config", "get", "ui.token"])
        .assert()
        .code(3);
    let v: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(v["value"].is_null());
}

#[test]
fn no_args_without_tty_prints_help() {
    let e = Env::new();
    let out = e.cmd().assert().success();
    let s = stdout(&out);
    assert!(s.contains("Usage") && s.contains("account"), "{s}");
}
