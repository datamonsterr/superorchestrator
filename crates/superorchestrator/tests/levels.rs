#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use serde_json::Value;
use so_core::Config;

const NOTES: &str = "# my notes\n[limits]\nglobal_cap = 4\n";

const TWO_LEVELS: &str = r#"
[[router.levels]]
name = "low"
min = 0
max = 49
tier = "fast"

[[router.levels]]
name = "high"
min = 50
max = 100
models = ["codex/gpt-6.1-sol/high", "agy/gemini-3.8-flash-medium", "codex/not-a-model/high"]
"#;

/// A config dir with an enabled codex account and a disabled agy account.
fn env_with(extra: &str) -> Env {
    let e = Env::new();
    std::fs::create_dir_all(e.cfg.join("accounts")).unwrap();
    std::fs::write(e.cfg.join("config.toml"), format!("{NOTES}\n{extra}")).unwrap();
    for (name, harness, enabled) in [("codex-acc1", "codex", true), ("agy-acc1", "agy", false)] {
        std::fs::write(
            e.cfg.join(format!("accounts/{name}.toml")),
            format!(
                "name = \"{name}\"\nharness = \"{harness}\"\nhome = \"{}\"\nbinary = \"{harness}\"\nenabled = {enabled}\n",
                e.home.join(name).display()
            ),
        )
        .unwrap();
    }
    e
}

fn cfg(e: &Env) -> Config {
    Config::load(&e.paths()).unwrap()
}

fn file(e: &Env) -> String {
    std::fs::read_to_string(e.cfg.join("config.toml")).unwrap()
}

fn json_of(e: &Env, args: &[&str]) -> Value {
    let mut c = e.cmd();
    c.arg("--json").args(args);
    serde_json::from_str(&stdout(&c.assert().success())).unwrap()
}

fn refs(e: &Env, level: &str) -> Vec<String> {
    cfg(e)
        .router
        .levels
        .iter()
        .find(|l| l.name == level)
        .unwrap()
        .models
        .iter()
        .map(so_core::Choice::id)
        .collect()
}

#[test]
fn list_without_levels_says_defaults() {
    let e = env_with("");
    let out = stdout(&e.cmd().args(["levels", "list"]).assert().success());
    assert!(out.contains("(defaults)"), "{out}");
    for h in [
        "LEVEL",
        "RANGE",
        "POSITION",
        "MODEL REF",
        "HARNESS",
        "STATE",
    ] {
        assert!(out.contains(h), "{h}: {out}");
    }
    assert!(out.contains("0-37") && out.contains("63-100"), "{out}");
    assert!(out.contains("tier fast"), "{out}");

    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], true);
    assert_eq!(v["levels"][0]["name"], "low");
    assert_eq!(v["levels"][0]["tier"], "fast");
    assert_eq!(v["levels"][0]["models"].as_array().unwrap().len(), 0);
}

#[test]
fn list_shows_models_in_order_with_state() {
    let e = env_with(TWO_LEVELS);
    let out = stdout(&e.cmd().args(["levels", "list"]).assert().success());
    assert!(!out.contains("(defaults)"), "{out}");
    let high: Vec<&str> = out.lines().filter(|l| l.starts_with("high")).collect();
    assert_eq!(high.len(), 3, "{out}");
    assert!(
        high[0].contains("codex/gpt-6.1-sol/high") && high[0].ends_with("on"),
        "{out}"
    );
    assert!(
        high[1].contains("agy/gemini-3.8-flash-medium") && high[1].ends_with("off"),
        "{out}"
    );
    assert!(high[2].ends_with("unknown"), "{out}");

    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], false);
    let m = &v["levels"][1]["models"];
    assert_eq!(m[0]["position"], 1);
    assert_eq!(m[0]["ref"], "codex/gpt-6.1-sol/high");
    assert_eq!(m[0]["harness"], "codex");
    assert_eq!(m[0]["model"], "gpt-6.1-sol");
    assert_eq!(m[0]["effort"], "high");
    assert_eq!(m[0]["state"], "on");
    assert_eq!(m[1]["state"], "off");
    assert_eq!(m[2]["position"], 3);
    assert_eq!(m[2]["state"], "unknown");
}

#[test]
fn first_edit_writes_the_defaults_first_and_keeps_comments() {
    let e = env_with("");
    // The defaults cover 0..=100, so a new level overlaps; nothing is written.
    e.cmd()
        .args([
            "levels", "add", "tiny", "--min", "0", "--max", "0", "--tier", "fast",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("overlaps"));
    assert!(!file(&e).contains("router"), "{}", file(&e));

    let out = stdout(&e.cmd().args(["levels", "remove", "mid"]).assert().success());
    assert!(out.contains("wrote the current defaults first"), "{out}");
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    assert!(file(&e).contains("# my notes"));
    let names: Vec<_> = cfg(&e)
        .router
        .levels
        .iter()
        .map(|l| l.name.clone())
        .collect();
    assert_eq!(names, ["low", "high"]);

    let out = stdout(
        &e.cmd()
            .args([
                "levels", "add", "mid", "--min", "38", "--max", "62", "--tier", "mid",
            ])
            .assert()
            .success(),
    );
    assert!(!out.contains("wrote the current defaults"), "{out}");
    assert_eq!(cfg(&e).router.levels.len(), 3);
}

#[test]
fn add_with_models_and_validation() {
    let e = env_with("[router]\nlevels = []\n");
    e.cmd()
        .args(["levels", "add", "x", "--min", "0", "--max", "5"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--tier or at least one --model"));
    e.cmd()
        .args([
            "levels", "add", "x", "--min", "0", "--max", "5", "--model", "nope/m",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown harness"));
    e.cmd()
        .args([
            "levels",
            "add",
            "x",
            "--min",
            "0",
            "--max",
            "5",
            "--model",
            "codex/gpt-6.1-sol;rm/high",
        ])
        .assert()
        .failure();
    assert!(cfg(&e).router.levels.is_empty());

    e.cmd()
        .args([
            "levels",
            "add",
            "x",
            "--min",
            "0",
            "--max",
            "5",
            "--model",
            "codex/gpt-6.1-sol/high",
            "--model",
            "agy/gemini-3.8-flash-medium",
        ])
        .assert()
        .success();
    assert_eq!(
        refs(&e, "x"),
        ["codex/gpt-6.1-sol/high", "agy/gemini-3.8-flash-medium"]
    );
}

#[test]
fn edit_changes_fields_and_refuses_bad_input() {
    let e = env_with(TWO_LEVELS);
    e.cmd().args(["levels", "edit", "low"]).assert().failure();
    e.cmd()
        .args(["levels", "edit", "ghost", "--min", "1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown level"));
    e.cmd()
        .args(["levels", "edit", "low", "--max", "60"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("overlaps"));
    // `low` lists no models, so it cannot lose its tier.
    e.cmd()
        .args(["levels", "edit", "low", "--no-tier"])
        .assert()
        .failure();
    let before = file(&e);

    let out = stdout(
        &e.cmd()
            .args([
                "levels", "edit", "low", "--name", "tiny", "--max", "40", "--tier", "mid",
            ])
            .assert()
            .success(),
    );
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    assert_ne!(file(&e), before);
    let low = &cfg(&e).router.levels[0];
    assert_eq!((low.name.as_str(), low.max), ("tiny", 40));

    e.cmd()
        .args(["levels", "edit", "high", "--no-tier"])
        .assert()
        .success();
    assert!(cfg(&e).router.levels[1].tier.is_none());
}

#[test]
fn remove_and_reset() {
    let e = env_with(TWO_LEVELS);
    e.cmd()
        .args(["levels", "remove", "ghost"])
        .assert()
        .failure();
    e.cmd().args(["levels", "remove", "low"]).assert().success();
    let before = file(&e);
    e.cmd()
        .args(["levels", "remove", "high"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("levels reset"));
    assert_eq!(file(&e), before);
    // An explicitly empty list is a state the file may already be in.
    e.cmd()
        .args(["--yes", "levels", "reset"])
        .assert()
        .success();
    Config::set_in_file(&e.paths(), "router.levels", "[]").unwrap();
    assert!(cfg(&e).router.levels.is_empty());

    // Not a terminal and no --yes: refuse, change nothing.
    e.cmd()
        .args(["levels", "reset"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    assert!(file(&e).contains("router"));

    let out = stdout(
        &e.cmd()
            .args(["--yes", "levels", "reset"])
            .assert()
            .success(),
    );
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    assert!(!file(&e).contains("router.levels") && file(&e).contains("# my notes"));
    assert_eq!(cfg(&e).router.levels.len(), 3);

    let v = json_of(&e, &["--yes", "levels", "reset"]);
    assert_eq!(v["changed"], false);
}

#[test]
fn model_add_move_remove() {
    let e = env_with(TWO_LEVELS);
    let sol = "codex/gpt-6.1-sol/high";
    let flash = "agy/gemini-3.8-flash-medium";
    let ghost = "codex/not-a-model/high";

    // A tier level gains a model; the tier note is printed.
    let out = stdout(
        &e.cmd()
            .args(["levels", "model", "add", "low", flash])
            .assert()
            .success(),
    );
    assert!(out.contains("tier fast no longer applies"), "{out}");
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    assert_eq!(refs(&e, "low"), [flash]);

    // Insert at position 1.
    e.cmd()
        .args(["levels", "model", "add", "low", sol, "--position", "1"])
        .assert()
        .success();
    assert_eq!(refs(&e, "low"), [sol, flash]);

    // Move: the last becomes the first.
    e.cmd()
        .args(["levels", "model", "move", "high", ghost, "--position", "1"])
        .assert()
        .success();
    assert_eq!(refs(&e, "high"), [ghost, sol, flash]);

    // Remove.
    e.cmd()
        .args(["levels", "model", "remove", "high", ghost])
        .assert()
        .success();
    assert_eq!(refs(&e, "high"), [sol, flash]);
}

#[test]
fn model_commands_refuse_bad_input_and_write_nothing() {
    let e = env_with(TWO_LEVELS);
    let sol = "codex/gpt-6.1-sol/high";
    let before = file(&e);
    let fail = |args: &[&str], msg: &str| {
        e.cmd()
            .arg("levels")
            .arg("model")
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(msg));
        assert_eq!(file(&e), before, "{args:?} wrote");
    };
    fail(&["add", "ghost", sol], "unknown level");
    fail(&["add", "high", sol], "already lists");
    fail(&["add", "high", "nope/m"], "unknown harness");
    fail(&["add", "high", "codex"], "harness/model");
    fail(
        &["add", "low", "claude/opus", "--position", "2"],
        "--position",
    );
    fail(
        &["add", "low", "claude/opus", "--position", "0"],
        "--position",
    );
    fail(&["move", "high", sol, "--position", "4"], "--position");
    fail(
        &["move", "high", "claude/opus", "--position", "1"],
        "does not list",
    );
    fail(&["remove", "high", "claude/opus"], "does not list");
    fail(&["remove", "ghost", sol], "unknown level");
}

#[test]
fn removing_the_last_model_of_a_tierless_level_is_refused() {
    let e = env_with("");
    e.cmd()
        .args(["levels", "edit", "high", "--max", "100"])
        .assert()
        .success();
    let before = file(&e);
    // The default `high` has a tier; give it one model, drop the tier, then try to empty it.
    e.cmd()
        .args(["levels", "model", "add", "high", "codex/gpt-6.1-sol/high"])
        .assert()
        .success();
    e.cmd()
        .args(["levels", "edit", "high", "--no-tier"])
        .assert()
        .success();
    let mid = file(&e);
    assert_ne!(mid, before);
    e.cmd()
        .args([
            "levels",
            "model",
            "remove",
            "high",
            "codex/gpt-6.1-sol/high",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs `models` or `tier`"));
    assert_eq!(file(&e), mid);
}

#[test]
fn names_are_trimmed_and_must_not_be_empty() {
    let e = env_with(TWO_LEVELS);
    let before = file(&e);
    e.cmd()
        .args(["levels", "edit", "low", "--name", "  "])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
    e.cmd()
        .args(["levels", "edit", "low", "--name", "high"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("duplicate"));
    e.cmd()
        .args(["levels", "edit", "low", "--name", " low "])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[0].name, "low");
    e.cmd()
        .args(["levels", "edit", "low", "--name", " tiny "])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[0].name, "tiny");
    let kept = file(&e);
    e.cmd()
        .args([
            "levels", "add", " ", "--min", "0", "--max", "0", "--tier", "fast",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("must not be empty"));
    assert_eq!(file(&e), kept);
    assert_ne!(kept, before);

    let e = env_with("[router]\nlevels = []\n");
    e.cmd()
        .args([
            "levels", "add", "  big ", "--min", "0", "--max", "9", "--tier", "high",
        ])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[0].name, "big");
}

#[test]
fn list_json_for_a_tier_only_level_has_no_models() {
    let e = env_with(TWO_LEVELS);
    let v = json_of(&e, &["levels", "list"]);
    let low = &v["levels"][0];
    assert_eq!(low["name"], "low");
    assert_eq!(low["tier"], "fast");
    assert_eq!(low["models"], serde_json::json!([]));
}
