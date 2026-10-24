#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use predicates::prelude::*;
use serde_json::{Value, json};
use so_core::Config;

const CHOICES: &str = r#"# my notes
[[choices]]
id = "agy-flash"
harness = "agy"
model = "gemini-3-flash"
effort = "medium"
roles = ["worker"]
enabled = true

[[choices]]
id = "codex-high"
harness = "codex"
model = "gpt-6.1-sol"
effort = "high"
roles = ["worker"]
enabled = false
"#;

fn env_with(extra: &str) -> Env {
    let e = Env::new();
    std::fs::create_dir_all(&e.cfg).unwrap();
    std::fs::write(e.cfg.join("config.toml"), format!("{CHOICES}\n{extra}")).unwrap();
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

const TWO_LEVELS: &str = r#"
[router]
levels = [
  { name = "low", min = 0, max = 49, tier = "fast" },
  { name = "high", min = 50, max = 100, choices = [{ id = "agy-flash", priority = 2 }, { id = "codex-high", priority = 1 }, { id = "ghost" }] },
]
"#;

#[test]
fn list_without_levels_says_defaults_and_shows_tier_choices() {
    let e = env_with("");
    let out = stdout(&e.cmd().args(["levels", "list"]).assert().success());
    assert!(out.contains("(defaults)"), "{out}");
    for h in [
        "LEVEL", "RANGE", "PRIORITY", "CHOICE", "HARNESS", "MODEL", "EFFORT", "STATE",
    ] {
        assert!(out.contains(h), "{h}: {out}");
    }
    assert!(out.contains("0-37") && out.contains("63-100"), "{out}");
    // The catalog does not know the model, so agy-flash is mid tier; the others have no choice.
    let mid = out.lines().find(|l| l.starts_with("mid")).unwrap();
    assert!(
        mid.contains("agy-flash") && mid.contains("tier mid"),
        "{out}"
    );
    let low = out.lines().find(|l| l.starts_with("low")).unwrap();
    assert!(low.contains("no enabled choice"), "{out}");

    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], true);
    assert_eq!(v["levels"][0]["name"], "low");
    assert_eq!(v["levels"][0]["tier"], "fast");
    assert_eq!(v["levels"][0]["tier_choices"], json!([]));
    assert_eq!(v["levels"][1]["tier_choices"], json!(["agy-flash"]));
}

#[test]
fn list_shows_choices_ordered_with_state() {
    let e = env_with(TWO_LEVELS);
    let out = stdout(&e.cmd().args(["levels", "list"]).assert().success());
    assert!(!out.contains("(defaults)"), "{out}");
    let high: Vec<&str> = out.lines().filter(|l| l.starts_with("high")).collect();
    assert_eq!(high.len(), 3, "{out}");
    assert!(
        high[0].contains("codex-high") && high[0].ends_with("off"),
        "{out}"
    );
    assert!(
        high[1].contains("agy-flash") && high[1].ends_with("on"),
        "{out}"
    );
    assert!(
        high[2].contains("ghost") && high[2].ends_with("unknown"),
        "{out}"
    );

    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], false);
    let c = &v["levels"][1]["choices"];
    assert_eq!(c[0]["id"], "codex-high");
    assert_eq!(c[0]["priority"], 1);
    assert_eq!(c[0]["harness"], "codex");
    assert_eq!(c[0]["state"], "off");
    assert_eq!(c[2]["state"], "unknown");
    assert_eq!(c[2]["known"], false);
}

#[test]
fn add_materialises_defaults_first_and_keeps_comments() {
    let e = env_with("");
    e.cmd()
        .args([
            "levels", "add", "tiny", "--min", "0", "--max", "0", "--tier", "fast",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("overlaps"));
    // The defaults cover 0..=100, so a new level overlaps; nothing is written.
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
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    let c = cfg(&e);
    assert_eq!(c.router.levels.len(), 3);
    assert_eq!(c.router.levels[2].name, "mid");
}

#[test]
fn add_json_shape_and_validation_errors() {
    let e = env_with(TWO_LEVELS);
    let v = json_of(&e, &["levels", "remove", "low"]);
    assert_eq!(v["ok"], true);
    assert_eq!(v["restart_required"], true);
    assert_eq!(v["materialised_defaults"], false);
    assert_eq!(v["levels"].as_array().unwrap().len(), 1);

    let e = env_with(TWO_LEVELS);
    let before = file(&e);
    let fail = |args: &[&str], needle: &str| {
        e.cmd()
            .arg("levels")
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(needle));
    };
    fail(
        &["add", "mid", "--min", "40", "--max", "60", "--tier", "mid"],
        "overlaps",
    );
    fail(
        &["add", "low", "--min", "0", "--max", "1", "--tier", "mid"],
        "duplicate name",
    );
    fail(
        &["add", "x", "--min", "9", "--max", "5", "--tier", "mid"],
        "min <= max",
    );
    fail(&["add", "x", "--min", "1", "--max", "1"], "--tier");
    fail(&["edit", "nope", "--min", "1"], "unknown level `nope`");
    fail(&["edit", "low"], "nothing to change");
    fail(&["edit", "low", "--max", "60"], "overlaps");
    fail(&["edit", "low", "--no-tier"], "needs `choices` or `tier`");
    fail(&["remove", "nope"], "unknown level");
    assert_eq!(file(&e), before, "failed commands must not write");
}

#[test]
fn edit_changes_name_range_and_tier() {
    let e = env_with(TWO_LEVELS);
    e.cmd()
        .args([
            "levels", "edit", "low", "--name", "easy", "--max", "39", "--tier", "mid",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restart the supervisor to apply"));
    let c = cfg(&e);
    let l = &c.router.levels[0];
    assert_eq!((l.name.as_str(), l.min, l.max), ("easy", 0, 39));
    assert_eq!(l.tier, Some(so_core::config::LevelTier::Mid));
    // The high level has choices, so its tier can go.
    e.cmd()
        .args(["levels", "edit", "high", "--tier", "fast"])
        .assert()
        .success();
    e.cmd()
        .args(["levels", "edit", "high", "--no-tier"])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[1].tier, None);
}

#[test]
fn reset_needs_yes_and_restores_defaults() {
    let e = env_with(&format!("# tail comment\n{TWO_LEVELS}"));
    // No terminal and no -y: refuses.
    e.cmd()
        .args(["levels", "reset"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--yes"));
    assert_eq!(cfg(&e).router.levels.len(), 2);

    let out = stdout(&e.cmd().args(["-y", "levels", "reset"]).assert().success());
    assert!(out.contains("restart the supervisor to apply"), "{out}");
    assert!(file(&e).contains("# my notes"));
    assert!(!file(&e).contains("levels ="));
    assert_eq!(cfg(&e).router.levels.len(), 3);
    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], true);

    // Already default: nothing to confirm.
    let v = json_of(&e, &["levels", "reset"]);
    assert_eq!(v["changed"], false);
}

#[test]
fn choice_add_existing_set_remove() {
    let e = env_with(TWO_LEVELS);
    e.cmd()
        .args([
            "levels",
            "choice",
            "add",
            "low",
            "codex-high",
            "--priority",
            "3",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("restart the supervisor to apply"));
    let c = cfg(&e);
    let low = &c.router.levels[0];
    assert_eq!(low.choices.len(), 1);
    assert_eq!(
        (low.choices[0].id.as_str(), low.choices[0].priority),
        ("codex-high", Some(3))
    );

    e.cmd()
        .args([
            "levels",
            "choice",
            "set",
            "low",
            "codex-high",
            "--priority",
            "1",
        ])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[0].choices[0].priority, Some(1));

    e.cmd()
        .args(["levels", "choice", "remove", "high", "ghost"])
        .assert()
        .success();
    assert_eq!(cfg(&e).router.levels[1].choices.len(), 2);

    let fail = |args: &[&str], needle: &str| {
        e.cmd()
            .args(["levels", "choice"])
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(needle));
    };
    fail(&["add", "nope", "codex-high"], "unknown level");
    fail(&["add", "low", "missing"], "unknown choice `missing`");
    fail(&["add", "low", "codex-high"], "duplicate choice");
    fail(
        &["set", "low", "agy-flash", "--priority", "1"],
        "does not list",
    );
    fail(&["remove", "low", "agy-flash"], "does not list");
    // `low` has a tier, so emptying it is fine; `high` has none and must keep a choice.
    e.cmd()
        .args(["levels", "choice", "remove", "low", "codex-high"])
        .assert()
        .success();
    assert!(cfg(&e).router.levels[0].choices.is_empty());
    e.cmd()
        .args(["levels", "edit", "low", "--no-tier"])
        .assert()
        .failure();
    fail(
        &[
            "add",
            "low",
            "agy-flash",
            "--harness",
            "codex",
            "--model",
            "m",
        ],
        "cannot be used",
    );
}

#[test]
fn choice_add_inline_creates_the_choice() {
    let e = env_with("");
    let out = stdout(
        &e.cmd()
            .args([
                "levels",
                "choice",
                "add",
                "high",
                "--harness",
                "codex",
                "--model",
                "GPT-6.1 Sol",
                "--effort",
                "high",
                "--priority",
                "1",
            ])
            .assert()
            .success(),
    );
    assert!(out.contains("wrote the current defaults first"), "{out}");
    assert!(
        out.contains("created choice codex-gpt-6-1-sol-high"),
        "{out}"
    );
    let c = cfg(&e);
    let ch = c
        .choices
        .iter()
        .find(|c| c.id == "codex-gpt-6-1-sol-high")
        .unwrap();
    assert_eq!(ch.model, "GPT-6.1 Sol");
    assert_eq!(ch.effort.as_deref(), Some("high"));
    assert_eq!(ch.roles, ["worker"]);
    assert_eq!(ch.description, "Added by `levels choice add`.");
    assert!(ch.enabled);
    assert_eq!(c.choices.len(), 3, "existing choices are kept");
    let high = c.router.levels.iter().find(|l| l.name == "high").unwrap();
    assert_eq!(high.choices[0].id, "codex-gpt-6-1-sol-high");
    assert_eq!(high.choices[0].priority, Some(1));

    // Explicit id and roles, no effort.
    e.cmd()
        .args([
            "levels",
            "choice",
            "add",
            "low",
            "--harness",
            "opencode",
            "--model",
            "x/y",
            "--id",
            "my-oc",
            "--role",
            "worker",
            "--role",
            "reviewer",
        ])
        .assert()
        .success();
    let c = cfg(&e);
    let ch = c.choices.iter().find(|c| c.id == "my-oc").unwrap();
    assert_eq!(
        (ch.effort.clone(), ch.roles.clone()),
        (None, vec!["worker".to_string(), "reviewer".into()])
    );

    // Default id without an effort.
    e.cmd()
        .args([
            "levels",
            "choice",
            "add",
            "mid",
            "--harness",
            "opencode",
            "--model",
            "a/b",
        ])
        .assert()
        .success();
    assert!(cfg(&e).choices.iter().any(|c| c.id == "opencode-a-b"));

    let before = file(&e);
    let fail = |args: &[&str], needle: &str| {
        e.cmd()
            .args(["levels", "choice", "add", "high"])
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(needle));
    };
    fail(
        &["--harness", "codex", "--model", "m", "--effort", "bogus"],
        "effort `bogus`",
    );
    fail(&["--harness", "nope", "--model", "m"], "nope");
    fail(
        &["--harness", "codex", "--model", "m", "--id", "my-oc"],
        "already exists",
    );
    fail(&["--harness", "codex"], "--model");
    assert_eq!(file(&e), before);
}

#[test]
fn removing_the_last_level_writes_an_empty_list_and_says_so() {
    let e =
        env_with("[router]\nlevels = [{ name = \"only\", min = 0, max = 100, tier = \"mid\" }]\n");
    let out = stdout(
        &e.cmd()
            .args(["levels", "remove", "only"])
            .assert()
            .success(),
    );
    assert!(
        out.contains("no levels left; levels are off, use `levels reset`"),
        "{out}"
    );
    assert!(cfg(&e).router.levels.is_empty());
    assert!(file(&e).contains("levels = []"), "{}", file(&e));
    let v = json_of(&e, &["levels", "list"]);
    assert_eq!(v["default_levels"], false);
}

#[test]
fn choice_add_notes_tier_replaced_and_rejects_bad_inline_values() {
    let e = env_with(TWO_LEVELS);
    let out = stdout(
        &e.cmd()
            .args(["levels", "choice", "add", "low", "agy-flash"])
            .assert()
            .success(),
    );
    assert!(
        out.contains("level low now uses its listed choices; tier fast no longer applies"),
        "{out}"
    );
    let before = file(&e);
    for (args, needle) in [
        (
            vec!["--harness", "codex", "--model", "m", "--effort", ""],
            "--effort",
        ),
        (
            vec!["--harness", "codex", "--model", "!!"],
            "only the harness name",
        ),
    ] {
        e.cmd()
            .args(["levels", "choice", "add", "high"])
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(needle));
    }
    assert_eq!(file(&e), before);
}
