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
fn seeded_choices_parse_with_core_config() {
    let e = Env::new();
    e.init();
    let cfg = so_core::Config::load(&e.paths()).unwrap();
    assert!(cfg.choices.len() >= 11, "{}", cfg.choices.len());
    let by = |id: &str| cfg.choices.iter().find(|c| c.id == id).unwrap();
    assert!(by("agy-flash-medium").enabled);
    assert_eq!(by("agy-sonnet-thinking").model, "claude-sonnet-4-6");
    assert!(
        cfg.choices
            .iter()
            .filter(|c| matches!(
                c.harness,
                so_core::Harness::Codex | so_core::Harness::Claude
            ))
            .all(|c| !c.enabled)
    );
    assert!(
        cfg.choices
            .iter()
            .any(|c| c.harness == so_core::Harness::Codex)
    );
    assert!(
        cfg.choices
            .iter()
            .any(|c| c.harness == so_core::Harness::Claude)
    );
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
    assert!(so_core::Config::load(&e.paths()).unwrap().choices.len() >= 11);

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
fn unimplemented_commands_exit_4_and_name_the_command() {
    let e = Env::new();
    for args in [
        vec!["install"],
        vec!["runs", "list"],
        vec!["route", "do a thing"],
        vec!["route", "eval"],
        vec!["mcp"],
        vec!["migrate", "--apply"],
        vec!["workflow", "list"],
    ] {
        e.cmd()
            .args(&args)
            .assert()
            .code(4)
            .stderr(predicate::str::contains("is not implemented yet"));
    }
}

#[test]
fn unimplemented_message_names_the_full_path() {
    let e = Env::new();
    e.cmd()
        .args(["route", "eval"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "`route eval` is not implemented yet",
        ));
    e.cmd()
        .args(["runs", "list"])
        .assert()
        .code(4)
        .stderr(predicate::str::contains(
            "`runs list` is not implemented yet",
        ));
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
