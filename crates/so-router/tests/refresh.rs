#![allow(clippy::unwrap_used)]
//! `models refresh` with a fake process runner and a fake models.dev.

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::routing::get;
use serde_json::json;
use so_accounts::HostEnv;
use so_core::{AccountFile, Choice, Harness, Paths, Platform};
use so_router::catalog::{Catalog, Cost, RefreshEnv, SourceStatus, refresh};
use so_usage::{BoxFuture, ProbeOutput, ProbeRequest, ProbeRunner, UsageError};

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<ProbeRequest>>,
}

impl ProbeRunner for Fake {
    fn run<'a>(&'a self, req: ProbeRequest) -> BoxFuture<'a, so_usage::Result<ProbeOutput>> {
        let name = req
            .program
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let args = req.args.clone();
        self.calls.lock().unwrap().push(req);
        Box::pin(async move {
            match name.as_str() {
                "codex" => Ok(ProbeOutput::ok(
                    json!({"id": 2, "result": {"data": [{"model": "gpt-new", "hidden": false,
                        "supportedReasoningEfforts": [{"reasoningEffort": "low"}]}]}})
                    .to_string(),
                )),
                "opencode" => Ok(ProbeOutput::ok(
                    "opencode/brand-new-free\nopencode/paid-one\nopencode/big-pickle\n",
                )),
                "agy" => {
                    assert!(args.iter().any(|a| a.starts_with("--gemini_dir=")));
                    Ok(ProbeOutput::ok(
                        "Fetching available models...\ngemini-9-flash\tGemini 9\n",
                    ))
                }
                _ => Err(UsageError::Probe("unexpected".into())),
            }
        })
    }
}

fn stub(dir: &std::path::Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir.join("bin")).unwrap();
    let p = dir.join("bin").join(name);
    std::fs::write(&p, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn refresh_merges_reports_and_caches_models_dev() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    std::fs::create_dir_all(home.join("agy1/.gemini")).unwrap();
    for b in ["codex", "opencode", "agy"] {
        stub(d.path(), b);
    }
    let host = HostEnv {
        home: home.clone(),
        path_var: Some(d.path().join("bin").to_string_lossy().into_owned()),
        platform: Platform::Linux,
    };
    let acct =
        |n: &str, h, b: &str| AccountFile::new(n, h, &home.join(n).to_string_lossy(), b).unwrap();
    let accounts = vec![
        acct("codex1", Harness::Codex, "codex"),
        acct("oc1", Harness::Opencode, "opencode"),
        acct("agy1", Harness::Agy, "agy"),
    ];

    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let h2 = hits.clone();
    let app = Router::new().route(
        "/api.json",
        get(move || {
            h2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async {
                axum::Json(json!({"opencode": {"models": {
                    "brand-new-free": {"cost": {"input": 0, "output": 0}},
                    "paid-one": {"cost": {"input": 1, "output": 1}}}}}))
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/api.json", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });

    let paths = Paths::from_dir(d.path().join("cfg"));
    let http = reqwest::Client::new();
    let runner = Fake::default();
    let env = RefreshEnv {
        paths: &paths,
        host: &host,
        accounts: &accounts,
        runner: &runner,
        http: &http,
        models_dev_url: Some(&url),
        now: chrono::Utc::now(),
    };
    let choice = |id: &str, h, m: &str| Choice {
        id: id.into(),
        harness: h,
        model: m.into(),
        effort: None,
        roles: vec!["worker".into()],
        description: String::new(),
        enabled: true,
    };
    let choices = vec![
        choice("known", Harness::Agy, "gemini-3.8-flash-low"),
        choice("typo", Harness::Agy, "gemini-3.8-flash-lwo"),
    ];
    let before = Catalog::default_catalog();
    let (cat, rep) = refresh(&env, before.clone(), &choices).await;

    assert!(
        rep.sources.iter().all(|s| s.status == SourceStatus::Ok),
        "{:?}",
        rep.sources
    );
    let added: Vec<_> = rep.added.iter().map(|m| m.model.as_str()).collect();
    assert!(added.contains(&"gpt-new") && added.contains(&"gemini-9-flash"));
    assert!(added.contains(&"opencode/brand-new-free") && added.contains(&"opencode/paid-one"));
    assert!(!added.contains(&"opencode/big-pickle"), "already known");
    assert_eq!(
        cat.cost(Harness::Opencode, "opencode/brand-new-free"),
        Cost::Free
    );
    assert_eq!(cat.cost(Harness::Opencode, "opencode/paid-one"), Cost::Paid);
    assert_eq!(cat.pool_for(Harness::Agy, "gemini-9-flash"), "gemini");
    assert_eq!(
        cat.get(Harness::Codex, "gpt-new").unwrap().efforts,
        vec!["low"]
    );
    // Existing entries are untouched.
    assert_eq!(
        cat.get(Harness::Codex, "gpt-6.1-sol"),
        before.get(Harness::Codex, "gpt-6.1-sol")
    );
    // Reported against choices, which are never edited.
    assert_eq!(rep.unknown_choice_models.len(), 1);
    assert_eq!(rep.unknown_choice_models[0].choice, "typo");
    // The fake agy lists only gemini-9-flash: both agy choices are absent from that list,
    // while the (unlisted) codex and opencode harnesses have no choice here to report.
    let missing: Vec<_> = rep
        .missing_choice_models
        .iter()
        .map(|c| c.choice.as_str())
        .collect();
    assert_eq!(missing, ["known", "typo"]);
    assert!(rep.unused_models.iter().any(|m| m.model == "gpt-new"));
    assert!(
        !rep.unused_models
            .iter()
            .any(|m| m.model == "gemini-3.8-flash-low")
    );
    assert_eq!(choices.len(), 2);

    // Second refresh within 24 h uses the cache.
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(paths.config_dir.join("cache/models.dev.json").is_file());
    let (_, rep2) = refresh(&env, cat, &choices).await;
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(
        rep2.sources
            .iter()
            .any(|s| s.source == "models.dev" && s.detail.contains("cache"))
    );
    assert!(rep2.added.is_empty());

    // No accounts: sources are skipped, not failed.
    let none = RefreshEnv {
        accounts: &[],
        ..env
    };
    let (_, rep3) = refresh(&none, before, &choices).await;
    assert!(
        rep3.sources
            .iter()
            .filter(|s| s.status == SourceStatus::Skipped)
            .count()
            == 3
    );
}
