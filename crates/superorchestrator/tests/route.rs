#![allow(clippy::unwrap_used)]
//! `route`, `route eval` and `models` from the command line.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use common::{Env, stdout};
use serde_json::Value;
use so_accounts::HostEnv;
use so_core::{AccountFile, Config, Harness, Platform};
use so_supervisor::{Supervisor, SupervisorOptions, default_handlers};

fn setup() -> Env {
    let e = Env::new();
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    e.init();
    // A key variable that is never set, so nothing reaches the network.
    e.cmd()
        .args(["config", "set", "router.jev.key_env", "SO_CLI_TEST_NO_KEY"])
        .assert()
        .success();
    for (name, h, bin) in [
        ("agy-a", Harness::Agy, "agy"),
        ("oc-a", Harness::Opencode, "opencode"),
    ] {
        AccountFile::new(name, h, &e.home.join(name).to_string_lossy(), bin)
            .unwrap()
            .save(&e.paths())
            .unwrap();
    }
    e
}

#[tokio::test(flavor = "multi_thread")]
async fn route_accepts_an_unquoted_multi_word_task() {
    let e = setup();
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let mut cmd = e.cmd();
    cmd.args(["route", "--json", "fix", "a", "typo", "in", "README"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(v["choice_id"].is_string(), "{v}");
    sup.shutdown().await;
}

#[test]
fn route_without_a_task_is_a_usage_error() {
    let e = setup();
    e.cmd().arg("route").assert().code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn route_json_goes_through_the_supervisor() {
    let e = setup();
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let mut cmd = e.cmd();
    cmd.args(["route", "--json", "rename a variable in one file"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["source"], "heuristic");
    assert_eq!(v["harness"], "agy");
    assert!(v["account"].as_str().unwrap().starts_with("agy-"));
    assert_eq!(v["config_fingerprint"].as_str().unwrap().len(), 64);
    assert!(v["complexity"].is_number(), "{v}");
    assert!(v["level"].is_string(), "{v}");

    let mut cmd = e.cmd();
    cmd.args(["route", "--explain", "--agent", "worker", "fix typo"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success());
    assert!(
        text.contains("account:") && text.contains("config:"),
        "{text}"
    );
    assert!(
        text.contains("complexity: ") && text.contains("(heuristic)") && text.contains("level:"),
        "{text}"
    );
    assert!(text.contains("accounts:   agy-a"), "{text}");

    sup.shutdown().await;
}

#[test]
fn route_eval_runs_locally_with_the_heuristic() {
    let e = setup();
    let cases = e.tmp.path().join("cases.jsonl");
    std::fs::write(
        &cases,
        concat!(
            r#"{"id":"a","task":"SECRET-TEXT rename a variable","expected_choices":["agy/gemini-3.8-flash-low/low","agy/gemini-3.8-flash-medium/medium"]}"#,
            "\n",
            r#"{"id":"b","task":"Tìm file khai báo timeout","expected_choices":["agy/gemini-3.8-flash-medium/medium","agy/gemini-3.8-flash-low/low"]}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args([
            "route",
            "eval",
            "--json",
            "--min-accuracy",
            "0.5",
            "--cases",
        ])
        .arg(&cases)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!text.contains("SECRET-TEXT"));
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["case_count"], 2);
    assert_eq!(v["sources"]["heuristic"], 2);
    assert_eq!(v["eligible_compliance"], 1.0);
    assert_eq!(v["accuracy"], 1.0);
    assert!(v["latency_ms"]["p95"].is_number());

    // An unreachable accuracy bar fails the command.
    e.cmd()
        .args(["route", "eval", "--min-accuracy", "1.0", "--cases"])
        .arg(&cases)
        .assert()
        .success();
    let cases2 = e.tmp.path().join("c2.jsonl");
    std::fs::write(
        &cases2,
        r#"{"id":"a","task":"rename","expected_choices":["nope"]}"#,
    )
    .unwrap();
    e.cmd()
        .args(["route", "eval", "--min-accuracy", "0.5", "--cases"])
        .arg(&cases2)
        .assert()
        .failure();
}

#[test]
fn route_eval_scores_complexity_against_expected_ranges() {
    let e = setup();
    let cases = e.tmp.path().join("cx.jsonl");
    std::fs::write(
        &cases,
        concat!(
            r#"{"id":"a","task":"rename a variable","expected_choices":["agy/gemini-3.8-flash-low/low","agy/gemini-3.8-flash-medium/medium"],"expected_complexity":[0,37]}"#,
            "\n",
            r#"{"id":"b","task":"rename a variable","expected_choices":["agy/gemini-3.8-flash-low/low","agy/gemini-3.8-flash-medium/medium"],"expected_complexity":[88,100]}"#,
            "\n",
            r#"{"id":"c","task":"rename a variable","expected_choices":["agy/gemini-3.8-flash-low/low","agy/gemini-3.8-flash-medium/medium"]}"#,
            "\n"
        ),
    )
    .unwrap();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--json", "--cases"])
        .arg(&cases)
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["complexity_accuracy"], 0.5);
    assert_eq!(v["cases"][0]["complexity_ok"], true);
    assert_eq!(v["cases"][1]["complexity_ok"], false);
    assert!(v["cases"][2]["complexity_ok"].is_null());
    assert!(v["cases"][0]["complexity"].is_number());
    assert!(v["cases"][0]["level"].is_string());

    let text = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--cases"])
        .arg(&cases)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&text.stdout).into_owned();
    assert!(text.contains("complexity accuracy: 50.0%"), "{text}");
    assert!(text.contains("CX") && text.contains("LEVEL"), "{text}");

    for (bar, ok) in [("0.5", true), ("0.9", false)] {
        let st = e
            .cmd()
            .env_remove("OPENROUTER_API_KEY")
            .args(["route", "eval", "--min-complexity-accuracy", bar, "--cases"])
            .arg(&cases)
            .output()
            .unwrap()
            .status;
        assert_eq!(st.success(), ok, "bar {bar}");
    }
}

#[test]
fn route_eval_require_live_fails_without_a_key() {
    let e = setup();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--require-live"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--require-live"));
}

#[test]
fn route_eval_bundled_cases_report() {
    let e = setup();
    let out = e
        .cmd()
        .env_remove("OPENROUTER_API_KEY")
        .args(["route", "eval", "--json"])
        .output()
        .unwrap();
    // Conductor cases may be unroutable here; the report is still produced.
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["case_count"], 15);
}

#[test]
fn models_list_json_uses_the_default_catalog() {
    let e = setup();
    let out = e
        .cmd()
        .args(["models", "list", "--json"])
        .assert()
        .success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    let arr = v.as_array().unwrap();
    assert!(arr.len() > 50);
    let m = arr
        .iter()
        .find(|m| m["model"] == "claude-sonnet-4-6")
        .unwrap();
    assert_eq!(m["harness"], "agy");
    assert_eq!(m["pool"], "claude");
    assert_eq!(m["tier"], "high");
}

/// `init` with no accounts, then a codex account added later; `route` runs through a
/// supervisor started on the config as it is now. With `drop_codex_refs` the codex refs are
/// first removed from every level.
async fn route_after_adding_codex(drop_codex_refs: bool, args: &[&str]) -> std::process::Output {
    let e = Env::new();
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    e.init();
    e.cmd()
        .args(["config", "set", "router.jev.key_env", "SO_CLI_TEST_NO_KEY"])
        .assert()
        .success();
    if drop_codex_refs {
        let path = e.cfg.join("config.toml");
        let text = std::fs::read_to_string(&path).unwrap();
        let dropped: String = text
            .lines()
            .map(|line| match line.strip_prefix("models = [") {
                Some(rest) => {
                    let kept: Vec<&str> = rest
                        .trim_end_matches(']')
                        .split(", ")
                        .filter(|r| !r.contains("codex/"))
                        .collect();
                    format!("models = [{}]", kept.join(", "))
                }
                None => line.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!dropped.contains("codex/"), "{dropped}");
        std::fs::write(&path, dropped).unwrap();
    }
    AccountFile::new(
        "codex-acc1",
        Harness::Codex,
        &e.home.join("codex-acc1").to_string_lossy(),
        "codex",
    )
    .unwrap()
    .save(&e.paths())
    .unwrap();
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();
    let mut cmd = e.cmd();
    cmd.arg("route").args(args);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    sup.shutdown().await;
    out
}

const ROUTE_ARGS: [&str; 2] = ["--json", "rename a variable in one file"];

#[tokio::test(flavor = "multi_thread")]
async fn a_codex_account_added_after_init_makes_the_seeded_codex_refs_eligible() {
    let out = route_after_adding_codex(false, &ROUTE_ARGS).await;
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["harness"], "codex");
    assert_eq!(v["account"], "codex-acc1");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ref_removed_from_every_level_is_not_routed() {
    let out = route_after_adding_codex(true, &ROUTE_ARGS).await;
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no eligible choice"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn route_model_pins_a_ref_and_explain_lists_the_accounts() {
    let out = route_after_adding_codex(
        false,
        &[
            "--model",
            "codex/gpt-6.1-sol/high",
            "--explain",
            "rename a variable in one file",
        ],
    )
    .await;
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "{}{text}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(text.starts_with("codex/gpt-6.1-sol/high"), "{text}");
    assert!(text.contains("account:    codex-acc1"), "{text}");
    assert!(text.contains("accounts:   codex-acc1"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn route_fails_when_the_decider_is_unreachable() {
    let e = setup();
    let key = e.tmp.path().join("jev.env");
    std::fs::write(&key, "SO_CLI_TEST_NO_KEY=sk-test-secret\n").unwrap();
    for (k, v) in [
        ("router.jev.key_file", key.to_str().unwrap()),
        ("router.jev.endpoint", "http://127.0.0.1:1/decisions"),
    ] {
        e.cmd().args(["config", "set", k, v]).assert().success();
    }
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let mut cmd = e.cmd();
    cmd.args(["route", "--explain", "rename a variable in one file"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("decider unreachable"), "{err}");
    assert!(err.contains("inception/mercury-decide:free"), "{err}");
    assert!(!err.contains("sk-test-secret"), "{err}");

    sup.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn route_model_with_an_effort_the_harness_rejects_fails() {
    let e = setup();
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let config = Config::load(&e.paths()).unwrap();
    let opts = SupervisorOptions::new(e.paths(), config, e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let mut cmd = e.cmd();
    cmd.args(["route", "--model", "agy/claude-sonnet-4-6/high", "fix typo"]);
    let out = tokio::task::spawn_blocking(move || cmd.output().unwrap())
        .await
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("effort"), "{err}");
    sup.shutdown().await;
}
