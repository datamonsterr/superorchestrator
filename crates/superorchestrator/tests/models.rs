#![allow(clippy::unwrap_used)]
mod common;

use std::io::{Read, Write};

use common::Env;

/// Serves `{}` to every request on a local port; returns the URL.
fn stub_models_dev() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { return };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            );
        }
    });
    format!("http://{addr}/api.json")
}

#[test]
fn models_refresh_reports_sources_and_writes_the_catalog() {
    let e = Env::new();
    e.init();
    let out = e
        .cmd()
        .env("SUPERORCHESTRATOR_MODELS_DEV_URL", stub_models_dev())
        .args(["--json", "models", "refresh"])
        .assert()
        .success();
    let v: serde_json::Value = serde_json::from_str(&common::stdout(&out)).unwrap();
    assert!(!v["sources"].as_array().unwrap().is_empty(), "{v}");
    assert!(v["added"].is_array() && v["unassigned_models"].is_array());
    assert!(v.get("unused_models").is_none());
    assert!(e.cfg.join("models.toml").is_file());
    let before = std::fs::read(e.cfg.join("models.toml")).unwrap();
    let out = e
        .cmd()
        .env("SUPERORCHESTRATOR_MODELS_DEV_URL", stub_models_dev())
        .args(["models", "refresh"])
        .assert()
        .success();
    assert!(common::stdout(&out).contains("catalog model(s) not listed by any level"));
    assert_eq!(std::fs::read(e.cfg.join("models.toml")).unwrap(), before);
}

#[test]
fn models_list_shows_levels_in_table_and_json() {
    let e = Env::new();
    e.init();
    let out = e
        .cmd()
        .args(["--json", "models", "list"])
        .assert()
        .success();
    let rows: serde_json::Value = serde_json::from_str(&common::stdout(&out)).unwrap();
    let model = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["harness"] == "codex" && r["model"] == "gpt-6.1-sol")
        .unwrap();
    assert_eq!(
        model["levels"],
        serde_json::json!(["moderate", "complex", "expert"])
    );
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|r| r["levels"] == serde_json::json!([]))
    );
    let out = e.cmd().args(["models", "list"]).assert().success();
    let text = common::stdout(&out);
    assert!(text.contains("LEVELS"));
    assert!(text.contains("moderate,complex,expert"));
}
