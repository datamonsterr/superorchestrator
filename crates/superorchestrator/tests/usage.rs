#![allow(clippy::unwrap_used)]

mod common;

use common::{Env, stdout};
use serde_json::Value;
use so_core::{AccountFile, Harness};

fn setup() -> Env {
    let e = Env::new();
    e.init();
    e.script("codex", r#"
printf 'x\n' >> "$CODEX_HOME/probes"
IFS= read -r line
printf '%s\n' '{"id":1,"result":{}}'
IFS= read -r line
IFS= read -r line
printf '%s\n' '{"id":2,"result":{"rateLimits":{"primary":{"usedPercent":32,"windowDurationMins":300},"secondary":{"usedPercent":46,"windowDurationMins":10080}}}}'
sleep 5
"#);
    e.script("agy", r#"
case "$1" in --gemini_dir=*/.gemini) ;; *) exit 1;; esac
[ "$2" = --print ] && [ "$3" = /usage ] || exit 1
printf 'Gemini Models\tFive Hour Limit Remaining\t91%%\t\nGemini Models\tWeekly Limit Remaining\t85%%\t\nClaude and GPT models\tFive Hour Limit Remaining\tdisabled\t\nClaude and GPT models\tWeekly Limit Remaining\t0%%\t\n'
"#);
    for (name, harness, auth) in [
        ("codex-1", Harness::Codex, "auth.json"),
        ("agy-1", Harness::Agy, ".gemini/oauth_creds.json"),
    ] {
        let home = e.home.join(name);
        let path = home.join(auth);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "credential sentinel").unwrap();
        AccountFile::new(name, harness, home.to_str().unwrap(), harness.as_str())
            .unwrap()
            .save(&e.paths())
            .unwrap();
    }
    e
}

#[test]
fn status_populates_real_windows_without_a_supervisor_and_account_list_reuses_them() {
    let e = setup();
    let out = e.cmd().args(["status", "--json"]).assert().success();
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert!(v["supervisor"].is_null());
    let accounts = v["accounts"].as_array().unwrap();
    assert_eq!(accounts[0]["headroom"]["gemini"], "ok 5h 91% wk 85%");
    assert_eq!(accounts[0]["headroom"]["claude"], "exhausted wk 0%");
    assert_eq!(accounts[1]["headroom"]["default"], "ok 5h 68% wk 54%");
    e.cmd().args(["account", "list"]).assert().success();
    assert_eq!(
        std::fs::read_to_string(e.home.join("codex-1/probes"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    assert!(!stdout(&out).contains("credential sentinel"));
}

#[test]
fn probe_failure_reports_safe_diagnostic_and_keeps_the_last_measurement() {
    let e = setup();
    e.cmd().arg("status").assert().success();
    let config = e.cfg.join("config.toml");
    let mut c = so_core::Config::load(&e.paths()).unwrap();
    c.usage.probe_ttl = so_core::HumanDuration::from_secs(0);
    c.save(&e.paths()).unwrap();
    assert!(config.is_file());
    e.script(
        "agy",
        "printf 'sign-in URL secret should not leak\n'; exit 1",
    );
    let out = e.cmd().args(["status", "--json"]).assert().success();
    let err = String::from_utf8_lossy(&out.get_output().stderr);
    assert!(err.contains("usage for agy-1"), "{err}");
    assert!(!err.contains("secret"));
    let v: Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(v["accounts"][0]["headroom"]["gemini"], "ok 5h 91% wk 85%");
}
