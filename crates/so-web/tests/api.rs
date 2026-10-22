#![allow(clippy::unwrap_used)]
//! Contract tests: the router driven with `oneshot` against a real in-process supervisor
//! (temp dirs, real store). No sockets are opened except the supervisor's own.

use std::path::PathBuf;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode, header};
use chrono::{Duration as Chrono, Utc};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use so_accounts::HostEnv;
use so_core::proto::RouteResult;
use so_core::{
    Config, Event, EventType, Harness, Isolation, Paths, Platform, RunState, SessionKind, new_id,
};
use so_store::{LedgerEntry, RunRow, SessionRow};
use so_supervisor::{Supervisor, SupervisorHandle, SupervisorOptions, default_handlers};
use so_web::{WebOptions, router};
use tempfile::TempDir;
use tower::ServiceExt;

const SECRET: &str = "sk-abcdef1234567890";

struct Fx {
    dir: TempDir,
    handle: SupervisorHandle,
    app: Router,
    session: String,
    run: String,
}

fn run_row(session: &str, state: RunState) -> RunRow {
    RunRow {
        id: new_id(),
        session_id: session.into(),
        parent_run_id: None,
        depth: 0,
        name: Some("refactor".into()),
        agent: None,
        choice_id: None,
        harness: Harness::Claude,
        account: Some("claude-acc1".into()),
        model: Some("m".into()),
        effort: None,
        cwd: Some("/tmp".into()),
        worktree_path: None,
        branch: None,
        native_session_id: Some("native-1".into()),
        state,
        review: false,
        output_schema: None,
        result_json: Some(format!("{{\"note\":\"{SECRET}\"}}")),
        error_kind: None,
        prompt: Some("do it".into()),
        isolation: Isolation::None,
        max_turns: None,
        worktree_base: None,
        worktree_repo: None,
        timeout_ms: None,
        created_at: Utc::now(),
        started_at: None,
        finished_at: None,
    }
}

fn event(session: &str, run: Option<&str>, t: EventType, data: Value, age_s: i64) -> Event {
    Event {
        id: new_id(),
        session_id: session.into(),
        run_id: run.map(Into::into),
        ts: Utc::now() - Chrono::seconds(age_s),
        event_type: t,
        data,
    }
}

async fn fx_with(token: Option<&str>) -> Fx {
    let dir = tempfile::Builder::new()
        .prefix("sow-")
        .tempdir_in("/tmp")
        .unwrap();
    let cfg = dir.path().join("cfg");
    let home = dir.path().join("home");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let paths = Paths::from_dir(cfg);
    let host = HostEnv {
        home: home.clone(),
        path_var: Some(home.join("bin").to_string_lossy().into_owned()),
        platform: Platform::Linux,
    };
    let opts = SupervisorOptions::new(
        paths.clone(),
        Config::default(),
        dir.path().join("run/s.sock"),
    )
    .with_host(host)
    .with_setup(default_handlers);
    let handle = Supervisor::start(opts).await.unwrap();
    let state = handle.state().clone();

    let store = state.store.clone();
    let session = new_id();
    store
        .insert_session(&SessionRow {
            id: session.clone(),
            kind: SessionKind::Orchestrator,
            client: "claude-code".into(),
            cwd: Some("/work/project".into()),
            account: None,
            pid: None,
            started_at: Utc::now() - Chrono::seconds(100),
            ended_at: None,
            status: "active".into(),
        })
        .unwrap();
    let run = run_row(&session, RunState::Running);
    store.insert_run(&run).unwrap();
    for (i, d) in [
        json!({"text": format!("leaked {SECRET}")}),
        json!({"text": "two"}),
        json!({"text": "three"}),
    ]
    .into_iter()
    .enumerate()
    {
        store
            .append_event(&event(
                &session,
                Some(&run.id),
                EventType::AssistantMessage,
                d,
                50 - i as i64,
            ))
            .unwrap();
    }
    let route = RouteResult {
        choice_id: "c1".into(),
        harness: Harness::Claude,
        account: "claude-acc1".into(),
        model: "m".into(),
        effort: None,
        source: so_core::proto::RouteSource::Heuristic,
        rationale: "because".into(),
        confidence: None,
        probabilities: Default::default(),
        usage: Default::default(),
        alternatives: vec![],
        config_fingerprint: "f".into(),
        ranked_accounts: Default::default(),
    };
    store
        .append_event(&event(
            &session,
            Some(&run.id),
            EventType::RouteDecision,
            json!({"result": route, "prompt_excerpt": "do it"}),
            10,
        ))
        .unwrap();
    store
        .insert_ledger(&LedgerEntry {
            account: "claude-acc1".into(),
            pool: "default".into(),
            run_id: Some(run.id.clone()),
            ts: Utc::now(),
            input_tokens: 10,
            output_tokens: 5,
            cost: 0.25,
        })
        .unwrap();
    let rd = paths.run_dir(&run.id);
    std::fs::create_dir_all(&rd).unwrap();
    let t = |e: Value| {
        format!(
            "{}\n",
            json!({"ts": "2026-01-01T00:00:00Z", "dir": "in", "channel": "stdout", "raw": format!("{{\"k\":\"{SECRET}\"}}"), "event": e})
        )
    };
    std::fs::write(
        rd.join("transcript.jsonl"),
        [
            t(json!({"type": "assistant_message", "text": format!("hi {SECRET}")})),
            t(json!({"type": "tool_call_finished", "name": "bash", "ok": true})),
        ]
        .concat(),
    )
    .unwrap();

    let app = router(
        state,
        WebOptions {
            token: token.map(Into::into),
            ..WebOptions::default()
        },
    );
    Fx {
        session,
        run: run.id,
        dir,
        handle,
        app,
    }
}

async fn fx() -> Fx {
    fx_with(None).await
}

fn request(method: &str, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::HOST, "localhost:7878");
    let body = match body {
        Some(v) => {
            b = b.header(header::CONTENT_TYPE, "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let res = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = res.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    let v = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()));
    (parts.status, parts.headers, v)
}

async fn get(app: &Router, uri: &str) -> (StatusCode, Value) {
    let (s, _, v) = send(app, request("GET", uri, None)).await;
    (s, v)
}

async fn post(app: &Router, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let (s, _, v) = send(app, request("POST", uri, body)).await;
    (s, v)
}

impl Fx {
    fn path(&self, p: &str) -> PathBuf {
        self.dir.path().join(p)
    }
}

// ------------------------------------------------------------------ read endpoints

#[tokio::test]
async fn system_endpoints_have_the_documented_shape() {
    let f = fx().await;
    let (s, v) = get(&f.app, "/api/v1/status").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["pid"].is_u64() && v["caps"]["global"].is_u64() && v["version"].is_string());

    let (s, v) = get(&f.app, "/api/v1/dashboard").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["live_runs"].as_array().unwrap().len(), 1);
    assert!(v["daily"].as_array().unwrap().len() == 1);
    assert!(v["accounts"].is_array() && v["escalations"].is_array());

    let (s, v) = get(&f.app, "/api/v1/usage/daily?days=7").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v[0]["by_account"]["claude-acc1"], 0.25);
    assert_eq!(v[0]["input_tokens"], 10);

    let (s, v) = get(&f.app, "/api/v1/doctor").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["checks"].is_array() && v["ok"].is_boolean());

    let (s, v) = get(&f.app, "/api/v1/harnesses").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["accounts"].is_array() && v["choices"].is_array());
}

#[tokio::test]
async fn sessions_list_filters_and_pages_with_a_cursor() {
    let f = fx().await;
    let store = f.handle.state().store.clone();
    for i in 0..3 {
        store
            .insert_session(&SessionRow {
                id: new_id(),
                kind: SessionKind::Interactive,
                client: format!("codex-{i}"),
                cwd: None,
                account: None,
                pid: None,
                started_at: Utc::now() - Chrono::seconds(10 + i),
                ended_at: Some(Utc::now()),
                status: "done".into(),
            })
            .unwrap();
    }
    let (s, v) = get(&f.app, "/api/v1/sessions?limit=2").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["items"].as_array().unwrap().len(), 2);
    let cursor = v["next_cursor"].as_str().unwrap().to_string();
    let (_, v2) = get(&f.app, &format!("/api/v1/sessions?limit=2&cursor={cursor}")).await;
    let items = v2["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(v2["next_cursor"].is_null());
    let first: Vec<_> = v["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["id"].clone())
        .collect();
    assert!(items.iter().all(|i| !first.contains(&i["id"])));

    let (_, v) = get(&f.app, "/api/v1/sessions?status=active").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    assert_eq!(v["items"][0]["run_count"], 1);
    let (_, v) = get(&f.app, "/api/v1/sessions?q=refactor").await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (_, v) = get(&f.app, "/api/v1/sessions?q=nomatch").await;
    assert!(v["items"].as_array().unwrap().is_empty());
    let (s, v) = get(&f.app, "/api/v1/sessions?status=bogus").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(v["error"]["code"], -32602);
    let (s, _) = get(&f.app, "/api/v1/sessions?limit=999999").await;
    assert_eq!(s, StatusCode::OK, "limit is clamped, not rejected");
    let (s, _) = get(&f.app, "/api/v1/sessions?cursor=zz").await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn session_detail_and_event_paging() {
    let f = fx().await;
    let (s, v) = get(&f.app, &format!("/api/v1/sessions/{}", f.session)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["id"], f.session.as_str());
    assert_eq!(v["runs"][0]["run_id"], f.run.as_str());
    let (s, v) = get(&f.app, "/api/v1/sessions/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], -32003);

    let uri = format!("/api/v1/sessions/{}/events?limit=3", f.session);
    let (_, p1) = get(&f.app, &uri).await;
    assert_eq!(p1["items"].as_array().unwrap().len(), 3);
    let cur = p1["next_cursor"].as_str().unwrap();
    let (_, p2) = get(&f.app, &format!("{uri}&after={cur}")).await;
    assert_eq!(p2["items"].as_array().unwrap().len(), 1);
    assert!(p2["next_cursor"].is_null());
    assert_eq!(p2["items"][0]["type"], "route_decision");
    let ids: Vec<_> = p1["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(p2["items"].as_array().unwrap())
        .map(|e| e["id"].clone())
        .collect();
    assert_eq!(ids.len(), 4);

    let (_, v) = get(
        &f.app,
        &format!("/api/v1/runs/{}/events?type=route_decision", f.run),
    )
    .await;
    assert_eq!(v["items"].as_array().unwrap().len(), 1);
    let (s, _) = get(&f.app, &format!("/api/v1/runs/{}/events?type=bogus", f.run)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn run_view_public_has_no_join_fields_and_no_secrets() {
    let f = fx().await;
    let (s, v) = get(&f.app, &format!("/api/v1/runs/{}", f.run)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["session_id"], f.session.as_str());
    let text = v.to_string();
    for k in ["join_command", "join_cwd", "join_env"] {
        assert!(!text.contains(k), "{k} leaked");
    }
    assert!(
        !text.contains(SECRET),
        "a secret reached the response: {text}"
    );
    // The id prefix resolves too.
    let (s, _) = get(&f.app, &format!("/api/v1/runs/{}", &f.run[..12])).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = get(&f.app, "/api/v1/runs/zzzz").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, d) = get(&f.app, &format!("/api/v1/sessions/{}", f.session)).await;
    assert!(!d.to_string().contains(SECRET));
    let (_, d) = get(&f.app, &format!("/api/v1/sessions/{}/events", f.session)).await;
    assert!(!d.to_string().contains(SECRET));
}

#[tokio::test]
async fn join_returns_masked_argv_and_audits() {
    let f = fx().await;
    let (s, v) = post(&f.app, &format!("/api/v1/runs/{}/join", f.run), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert!(v["argv"].is_array() && v["cwd"].is_string() && v["env_names"].is_array());
    assert!(v.get("env").is_none());
    let (_, ev) = get(
        &f.app,
        &format!("/api/v1/runs/{}/events?type=join_command_issued", f.run),
    )
    .await;
    assert_eq!(ev["items"].as_array().unwrap().len(), 1);
    let (s, _) = post(&f.app, "/api/v1/runs/zzzz/join", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn transcript_serves_normalised_redacted_lines_only() {
    let f = fx().await;
    let uri = format!("/api/v1/runs/{}/transcript", f.run);
    let (s, v) = get(&f.app, &uri).await;
    assert_eq!(s, StatusCode::OK);
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["role"], "assistant");
    assert_eq!(lines[0]["text"], "hi [REDACTED]");
    assert_eq!(lines[1]["tool"]["name"], "bash");
    assert!(!v.to_string().contains("sk-"));
    assert!(!v.to_string().contains("raw"));
    let next = lines[1]["offset"].as_u64().unwrap();
    let (_, v2) = get(&f.app, &format!("{uri}?offset={next}&limit=1")).await;
    assert_eq!(v2["lines"].as_array().unwrap().len(), 1);
    assert_eq!(v2["lines"][0]["offset"], next);
    let end = v2["next_offset"].as_u64().unwrap();
    let (_, v3) = get(&f.app, &format!("{uri}?offset={end}")).await;
    assert!(v3["lines"].as_array().unwrap().is_empty());
    assert_eq!(v3["total_bytes"], v3["next_offset"]);
}

#[tokio::test]
async fn route_decision_and_route_log() {
    let f = fx().await;
    let (s, v) = get(&f.app, &format!("/api/v1/runs/{}/route", f.run)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["choice_id"], "c1");
    let (s, v) = get(&f.app, "/api/v1/routing/records").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["items"][0]["prompt_excerpt"], "do it");
    assert_eq!(v["items"][0]["result"]["account"], "claude-acc1");
    let other = run_row(&f.session, RunState::Running);
    f.handle.state().store.insert_run(&other).unwrap();
    let (s, _) = get(&f.app, &format!("/api/v1/runs/{}/route", other.id)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn cancel_a_queued_run() {
    let f = fx().await;
    let queued = run_row(&f.session, RunState::Queued);
    f.handle.state().store.insert_run(&queued).unwrap();
    let (s, v) = post(&f.app, &format!("/api/v1/runs/{}/cancel", queued.id), None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v, json!({"ok": true}));
    let (_, r) = get(&f.app, &format!("/api/v1/runs/{}", queued.id)).await;
    assert_eq!(r["state"], "cancelled");
    let (s, _) = post(&f.app, "/api/v1/runs/zzzz/cancel", None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn endpoints_without_a_backend_return_501() {
    let f = fx().await;
    for (method, path) in so_web::api::NOT_IMPLEMENTED {
        let uri = path.replace("{id}", "x").replace("{name}", "x");
        let body = (*method != "GET" && *method != "DELETE").then(|| json!({}));
        let (s, _, v) = send(&f.app, request(method, &uri, body)).await;
        assert_eq!(s, StatusCode::NOT_IMPLEMENTED, "{method} {uri}");
        assert_eq!(v["error"]["code"], "not_implemented", "{method} {uri}");
    }
    let (s, v) = get(&f.app, "/api/v1/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(v["error"]["code"], "not_found");
}

// --------------------------------------------------------------------- accounts

#[tokio::test]
async fn account_lifecycle() {
    let f = fx().await;
    let (s, v) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "codex-acc1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(v["name"], "codex-acc1");
    assert_eq!(v["signed_in"], "signed_out");
    assert_eq!(v["priority"], 0);
    assert_eq!(v["pools"], json!(["default"]));
    assert_eq!(v["flags"], json!([]));
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "codex-acc1"})),
    )
    .await;
    assert_eq!(s, StatusCode::CONFLICT);
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "Bad Name"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "nope", "name": "x1"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (_, list) = get(&f.app, "/api/v1/accounts").await;
    assert_eq!(list.as_array().unwrap().len(), 1);

    let (s, v) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/accounts/codex-acc1",
        json!({"cap": 3, "priority": 2, "enabled": false}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(
        (
            v["cap"].clone(),
            v["priority"].clone(),
            v["enabled"].clone()
        ),
        (json!(3), json!(2), json!(false))
    );
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/accounts/codex-acc1",
        json!({"cap": 0}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/accounts/missing",
        json!({"cap": 1}),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, v) = post(
        &f.app,
        "/api/v1/accounts/codex-acc1/flag",
        Some(json!({"hours": 2, "reason": "quota"})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["flags"][0]["pool"], "default");
    assert_eq!(v["flags"][0]["reason"], "quota");
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts/codex-acc1/flag",
        Some(json!({"pool": "zzz", "hours": 1})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = post(&f.app, "/api/v1/accounts/codex-acc1/flag", Some(json!({}))).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (_, v) = post(
        &f.app,
        "/api/v1/accounts/codex-acc1/flag",
        Some(json!({"clear": true})),
    )
    .await;
    assert_eq!(v["flags"], json!([]));

    let (s, v) = post(&f.app, "/api/v1/accounts/codex-acc1/login", None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["mode"], "manual");
    assert!(v["command"].as_str().unwrap().contains("login"));
    let (s, v) = get(&f.app, "/api/v1/accounts/codex-acc1/login").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        (v["state"].clone(), v["signed_in"].clone()),
        (json!("pending"), json!("signed_out"))
    );
    let (s, _) = get(&f.app, "/api/v1/accounts/missing/login").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let (s, v) = get(&f.app, "/api/v1/accounts/scan").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v.is_array());

    let (s, v) = send_json_empty(&f.app, "DELETE", "/api/v1/accounts/codex-acc1").await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, _) = send_json_empty(&f.app, "DELETE", "/api/v1/accounts/codex-acc1").await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let home = f.path("home/.codex-import");
    std::fs::create_dir_all(&home).unwrap();
    let (s, v) = post(
        &f.app,
        "/api/v1/accounts/import",
        Some(json!({"name": "codex-imp", "harness": "codex", "home": home.to_string_lossy()})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts/import",
        Some(json!({"name": "codex-imp2", "harness": "codex", "home": "/nonexistent/x"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

async fn send_json(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let (s, _, v) = send(app, request(method, uri, Some(body))).await;
    (s, v)
}

async fn send_json_empty(app: &Router, method: &str, uri: &str) -> (StatusCode, Value) {
    let (s, _, v) = send(app, request(method, uri, None)).await;
    (s, v)
}

// ------------------------------------------------------------------ config etc.

#[tokio::test]
async fn choices_config_and_worktrees() {
    let f = fx().await;
    let (s, v) = get(&f.app, "/api/v1/choices").await;
    assert_eq!(s, StatusCode::OK);
    assert!(v["routing_instruction"].is_string());
    let body = json!({
        "choices": [{"id": "codex-high", "harness": "codex", "model": "gpt", "effort": "high", "roles": ["worker"], "description": "d", "enabled": true}],
        "routing_instruction": "be cheap",
    });
    let (s, v) = send_json(&f.app, "PUT", "/api/v1/choices", body.clone()).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["choices"][0]["id"], "codex-high");
    assert_eq!(v["routing_instruction"], "be cheap");
    assert_eq!(v["restart_required"], true);
    let dup = json!({"choices": [body["choices"][0].clone(), body["choices"][0].clone()], "routing_instruction": "x"});
    let (s, _) = send_json(&f.app, "PUT", "/api/v1/choices", dup).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, v) = get(&f.app, "/api/v1/config").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["platform"], "linux");
    assert!(v["config"]["ui"].get("token").is_none() || v["config"]["ui"]["token"].is_null());
    assert_eq!(v["ui_token"]["source"], "unset");
    let (s, v) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/config",
        json!({"limits": {"global_cap": 4}, "runs": {"stall_ms": 1000}}),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["config"]["limits"]["global_cap"], 4);
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/config",
        json!({"limits": {"nope": 1}}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/config",
        json!({"ui": {"token": "x"}}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/config",
        json!({"limits": {"global_cap": 0}}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);

    let (s, v) = get(&f.app, "/api/v1/worktrees").await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!([]));
    let (s, v) = post(
        &f.app,
        "/api/v1/worktrees/prune",
        Some(json!({"dry_run": true})),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v, json!({"removed": []}));
    let (s, _) = post(&f.app, "/api/v1/worktrees/prune", None).await;
    assert_eq!(s, StatusCode::OK);
}

// --------------------------------------------------------------------- security

#[tokio::test]
async fn host_and_origin_are_checked_everywhere() {
    let f = fx().await;
    let with = |host: &str, origin: Option<&str>, uri: &str, method: &str| {
        let mut b = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, host);
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    };
    for uri in ["/api/v1/status", "/api/v1/stream", "/", "/sessions/x"] {
        let (s, _, _) = send(&f.app, with("evil.example:7878", None, uri, "GET")).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "host on {uri}");
    }
    let (s, _, _) = send(
        &f.app,
        Request::builder()
            .uri("/api/v1/status")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "no Host");
    for uri in ["/api/v1/status", "/api/v1/stream"] {
        let (s, _, v) = send(
            &f.app,
            with("localhost:7878", Some("http://evil.example"), uri, "GET"),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "origin on {uri}");
        assert_eq!(v["error"]["code"], "forbidden");
    }
    let (s, _, _) = send(
        &f.app,
        with(
            "localhost:7878",
            Some("http://localhost:7878"),
            "/api/v1/status",
            "GET",
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = send(&f.app, with("[::1]:7878", None, "/api/v1/status", "GET")).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _, _) = send(
        &f.app,
        with("localhost:7878", Some("null"), "/api/v1/status", "GET"),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // State-changing request with a cross-site fetch hint.
    let mut r = with("localhost:7878", None, "/api/v1/worktrees/prune", "POST");
    r.headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    let (s, _, _) = send(&f.app, r).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn request_bodies_must_be_json() {
    let f = fx().await;
    let r = Request::builder()
        .method("POST")
        .uri("/api/v1/worktrees/prune")
        .header(header::HOST, "localhost:7878")
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::CONTENT_LENGTH, "2")
        .body(Body::from("{}"))
        .unwrap();
    let (s, _, _) = send(&f.app, r).await;
    assert_eq!(s, StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn token_flow_cookie_and_rate_limit() {
    let f = fx_with(Some("s3cret-token")).await;
    let (s, v) = get(&f.app, "/api/v1/status").await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(v["error"]["code"], "unauthorized");
    // The SPA itself loads without a token (it shows the token prompt).
    let (s, _, _) = send(&f.app, request("GET", "/", None)).await;
    assert_eq!(s, StatusCode::OK);

    let (s, headers, _) = send(
        &f.app,
        request(
            "POST",
            "/api/v1/token",
            Some(json!({"token": "s3cret-token"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let set = headers
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    for want in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=604800"] {
        assert!(set.contains(want), "{set}");
    }
    assert!(!set.contains("s3cret-token") && !set.contains("Secure"));
    let cookie = set.split(';').next().unwrap().to_string();
    let mut authed = request("GET", "/api/v1/status", None);
    authed
        .headers_mut()
        .insert(header::COOKIE, cookie.parse().unwrap());
    let (s, _, _) = send(&f.app, authed).await;
    assert_eq!(s, StatusCode::OK);
    let mut bearer = request("GET", "/api/v1/status", None);
    bearer.headers_mut().insert(
        header::AUTHORIZATION,
        "Bearer s3cret-token".parse().unwrap(),
    );
    assert_eq!(send(&f.app, bearer).await.0, StatusCode::OK);
    let mut bad = request("GET", "/api/v1/status", None);
    bad.headers_mut()
        .insert(header::COOKIE, "so_token=nope".parse().unwrap());
    assert_eq!(send(&f.app, bad).await.0, StatusCode::UNAUTHORIZED);

    // The bad cookie above already counted as one failure.
    for _ in 0..4 {
        let (s, _, _) = send(
            &f.app,
            request("POST", "/api/v1/token", Some(json!({"token": "wrong"}))),
        )
        .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }
    let (s, h, _) = send(
        &f.app,
        request("POST", "/api/v1/token", Some(json!({"token": "wrong"}))),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    assert!(h.contains_key(header::RETRY_AFTER));
    // Even the right token is refused while limited.
    let (s, _, _) = send(
        &f.app,
        request(
            "POST",
            "/api/v1/token",
            Some(json!({"token": "s3cret-token"})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn non_loopback_bind_requires_a_token() {
    let f = fx().await;
    let state = f.handle.state().clone();
    let open = WebOptions {
        bind: "0.0.0.0".into(),
        port: 0,
        ..WebOptions::default()
    };
    let err = so_web::start(state.clone(), open).await.err().unwrap();
    assert!(matches!(err, so_web::WebError::TokenRequired(_)), "{err}");
    let empty = WebOptions {
        bind: "0.0.0.0".into(),
        port: 0,
        token: Some(String::new()),
        ..WebOptions::default()
    };
    assert!(so_web::start(state.clone(), empty).await.is_err());
    let ok = WebOptions {
        bind: "127.0.0.1".into(),
        port: 0,
        ..WebOptions::default()
    };
    let server = so_web::start(state, ok).await.unwrap();
    assert!(server.url().starts_with("http://127.0.0.1:"));
    server.shutdown().await;
}

#[tokio::test]
async fn spa_fallback_headers_and_no_file_serving() {
    let f = fx().await;
    for uri in ["/", "/sessions/abc", "/dashboard"] {
        let (s, h, v) = send(&f.app, request("GET", uri, None)).await;
        assert_eq!(s, StatusCode::OK, "{uri}");
        assert!(
            h[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert!(
            h[header::CONTENT_SECURITY_POLICY]
                .to_str()
                .unwrap()
                .contains("default-src 'self'")
        );
        assert_eq!(h[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        assert!(
            v.as_str()
                .unwrap()
                .to_lowercase()
                .contains("<!doctype html")
        );
    }
    let (s, _, _) = send(&f.app, request("GET", "/assets/missing-abc123.js", None)).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _, _) = send(&f.app, request("GET", "/../Cargo.toml", None)).await;
    assert_ne!(s, StatusCode::OK);
    let (_, h, _) = send(&f.app, request("GET", "/api/v1/status", None)).await;
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert_eq!(h[header::REFERRER_POLICY], "no-referrer");
}

// -------------------------------------------------------------------------- SSE

async fn read_until(body: &mut Body, needle: &str) -> String {
    let mut acc = String::new();
    let fut = async {
        while !acc.contains(needle) {
            let frame = body.frame().await.expect("stream ended").unwrap();
            if let Ok(data) = frame.into_data() {
                acc.push_str(&String::from_utf8_lossy(&data));
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), fut)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; got {acc:?}"));
    acc
}

fn stream_request(last_id: Option<&str>) -> Request<Body> {
    let mut r = request("GET", "/api/v1/stream", None);
    if let Some(id) = last_id {
        r.headers_mut().insert("last-event-id", id.parse().unwrap());
    }
    r
}

#[tokio::test]
async fn sse_hello_event_delivery_replay_and_resync() {
    let f = fx().await;
    let state = f.handle.state().clone();
    assert!(!state.busy());
    let res = f.app.clone().oneshot(stream_request(None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/event-stream")
    );
    let mut body = res.into_body();
    let first = read_until(&mut body, "\n\n").await;
    assert!(first.starts_with("event: hello"), "{first}");
    assert!(first.contains("boot_id") && first.contains("\"seq\":0"));
    let boot = first
        .split("\"boot_id\":\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap()
        .to_string();
    assert!(state.busy(), "an open stream counts as activity");

    state
        .runs
        .emit(
            &f.session,
            Some(&f.run),
            EventType::Progress,
            json!({"step": 1, "text": SECRET}),
        )
        .await;
    let got = read_until(&mut body, "event: event").await;
    let block = got
        .split("\n\n")
        .find(|b| b.contains("event: event"))
        .unwrap();
    assert!(block.contains(&format!("id: {boot}:1\n")), "{block}");
    assert!(block.contains("\"seq\":1"), "per-session seq: {block}");
    assert!(block.contains("\"type\":\"progress\""));
    assert!(!block.contains(SECRET), "events are redacted");

    // A second event, then a reconnect that has seen only the first message.
    state
        .runs
        .emit(
            &f.session,
            Some(&f.run),
            EventType::Progress,
            json!({"step": 2}),
        )
        .await;
    let _ = read_until(&mut body, "\"step\":2").await;
    drop(body);
    assert!(
        !state.busy(),
        "closing the stream releases the activity guard"
    );

    let res = f
        .app
        .clone()
        .oneshot(stream_request(Some(&format!("{boot}:1"))))
        .await
        .unwrap();
    let mut body = res.into_body();
    let replay = read_until(&mut body, "\"step\":2").await;
    assert!(replay.contains("event: hello"));
    assert!(!replay.contains("\"step\":1"), "already seen: {replay}");
    assert!(replay.contains(&format!("id: {boot}:2\n")));

    // An id from before a restart (ahead of the counter) asks for a full resync.
    let res = f
        .app
        .clone()
        .oneshot(stream_request(Some(&format!("{boot}:999999"))))
        .await
        .unwrap();
    let mut body = res.into_body();
    let r = read_until(&mut body, "event: resync").await;
    assert!(r.contains("event: hello"));

    // An id from another boot, or one that does not parse, also resyncs.
    for bad in ["other-boot:1", "1", "garbage"] {
        let res = f
            .app
            .clone()
            .oneshot(stream_request(Some(bad)))
            .await
            .unwrap();
        let mut body = res.into_body();
        read_until(&mut body, "event: resync").await;
    }

    // The session filter hides other sessions' events.
    let mut req = request("GET", "/api/v1/stream?session=other", None);
    req.headers_mut()
        .insert("last-event-id", "0".parse().unwrap());
    let res = f.app.clone().oneshot(req).await.unwrap();
    let mut body = res.into_body();
    let r = read_until(&mut body, "event: hello").await;
    state
        .runs
        .emit(
            &f.session,
            Some(&f.run),
            EventType::Progress,
            json!({"step": 3}),
        )
        .await;
    let more = tokio::time::timeout(
        Duration::from_millis(300),
        read_until(&mut body, "\"step\":3"),
    )
    .await;
    assert!(more.is_err(), "filtered event arrived: {r}");
}

#[tokio::test]
async fn sse_derives_run_and_session_messages_and_account_pushes() {
    let f = fx().await;
    let state = f.handle.state().clone();
    let res = f.app.clone().oneshot(stream_request(None)).await.unwrap();
    let mut body = res.into_body();
    read_until(&mut body, "event: hello").await;
    state
        .runs
        .emit(
            &f.session,
            Some(&f.run),
            EventType::StateChanged,
            json!({"from": "running", "to": "waiting"}),
        )
        .await;
    let got = read_until(&mut body, "event: run").await;
    assert!(got.contains("event: event"));
    assert!(
        !got.contains("recent_events"),
        "run messages carry no events"
    );
    // Creating an account pushes an `account` message.
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "codex-acc9"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let got = read_until(&mut body, "event: account").await;
    assert!(got.contains("codex-acc9"));
}

#[tokio::test]
async fn non_loopback_stream_requires_an_origin() {
    let f = fx().await;
    let opts = WebOptions {
        bind: "192.168.1.5".into(),
        port: 7878,
        token: Some("tok".into()),
        ..WebOptions::default()
    };
    let app = router(f.handle.state().clone(), opts);
    let build = |origin: Option<&str>| {
        let mut b = Request::builder()
            .uri("/api/v1/stream")
            .header(header::HOST, "192.168.1.5:7878")
            .header(header::AUTHORIZATION, "Bearer tok");
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    };
    let res = app.clone().oneshot(build(None)).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let res = app
        .clone()
        .oneshot(build(Some("http://192.168.1.5:7878")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    // Plain reads without an Origin stay allowed.
    let mut r = request("GET", "/api/v1/status", None);
    *r.headers_mut().get_mut(header::HOST).unwrap() = "192.168.1.5:7878".parse().unwrap();
    r.headers_mut()
        .insert(header::AUTHORIZATION, "Bearer tok".parse().unwrap());
    assert_eq!(app.oneshot(r).await.unwrap().status(), StatusCode::OK);
}

#[tokio::test]
async fn wrong_credentials_count_toward_the_rate_limit() {
    let f = fx_with(Some("good-token")).await;
    let bad = || {
        let mut r = request("GET", "/api/v1/status", None);
        r.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer wrong".parse().unwrap());
        r
    };
    for _ in 0..5 {
        assert_eq!(send(&f.app, bad()).await.0, StatusCode::UNAUTHORIZED);
    }
    assert_eq!(send(&f.app, bad()).await.0, StatusCode::TOO_MANY_REQUESTS);
    let mut good = request("GET", "/api/v1/status", None);
    good.headers_mut()
        .insert(header::AUTHORIZATION, "Bearer good-token".parse().unwrap());
    assert_eq!(send(&f.app, good).await.0, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn html_responses_forbid_framing() {
    let f = fx().await;
    let (_, h, _) = send(&f.app, request("GET", "/", None)).await;
    let csp = h[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
    for want in [
        "frame-ancestors 'none'",
        "base-uri 'none'",
        "form-action 'self'",
    ] {
        assert!(csp.contains(want), "{csp}");
    }
    assert_eq!(h["x-frame-options"], "DENY");
}

#[tokio::test]
async fn account_names_are_validated_before_touching_the_filesystem() {
    let f = fx().await;
    std::fs::write(f.path("cfg/config.toml"), "not = [valid toml\n").unwrap();
    for (m, uri) in [
        ("GET", "/api/v1/accounts/..%2Fconfig/login"),
        ("POST", "/api/v1/accounts/..%2Fconfig/login"),
        ("DELETE", "/api/v1/accounts/..%2Fconfig"),
        ("POST", "/api/v1/accounts/..%2Fconfig/flag"),
    ] {
        let body = (m == "POST" && uri.ends_with("flag")).then(|| json!({"hours": 1}));
        let (s, _, v) = send(&f.app, request(m, uri, body)).await;
        assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY, "{m} {uri}: {v}");
    }
    let (s, _) = send_json(
        &f.app,
        "PATCH",
        "/api/v1/accounts/..%2Fconfig",
        json!({"cap": 1}),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    let (s, _) = post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "../x"})),
    )
    .await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn parse_errors_do_not_echo_file_contents() {
    let f = fx().await;
    post(
        &f.app,
        "/api/v1/accounts",
        Some(json!({"harness": "codex", "name": "codex-acc1"})),
    )
    .await;
    std::fs::write(
        f.path("cfg/accounts/codex-acc1.toml"),
        "token = \"hunter2-secret\" = oops\n",
    )
    .unwrap();
    let (s, _, v) = send(
        &f.app,
        request("GET", "/api/v1/accounts/codex-acc1/login", None),
    )
    .await;
    let _ = s;
    assert!(!v.to_string().contains("hunter2"), "{v}");
}

#[tokio::test]
async fn stream_without_origin_needs_same_origin_fetch_metadata_off_loopback() {
    let f = fx().await;
    let opts = WebOptions {
        bind: "192.168.1.5".into(),
        port: 7878,
        token: Some("tok".into()),
        ..WebOptions::default()
    };
    let app = router(f.handle.state().clone(), opts);
    let build = |site: Option<&str>| {
        let mut b = Request::builder()
            .uri("/api/v1/stream")
            .header(header::HOST, "192.168.1.5:7878")
            .header(header::AUTHORIZATION, "Bearer tok");
        if let Some(v) = site {
            b = b.header("sec-fetch-site", v);
        }
        b.body(Body::empty()).unwrap()
    };
    assert_eq!(
        app.clone()
            .oneshot(build(Some("same-origin")))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(build(Some("cross-site")))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        app.clone().oneshot(build(None)).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn shutdown_ends_open_streams() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = fx().await;
    let opts = WebOptions {
        port: 0,
        ..WebOptions::default()
    };
    let server = so_web::start(f.handle.state().clone(), opts).await.unwrap();
    let addr = server.addr();
    let mut sock = tokio::net::TcpStream::connect(addr).await.unwrap();
    let req = format!(
        "GET /api/v1/stream HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: keep-alive\r\n\r\n",
        addr.port()
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    let mut buf = vec![0u8; 4096];
    let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&buf[..n]).contains("200"));
    tokio::time::timeout(Duration::from_secs(5), server.shutdown())
        .await
        .expect("shutdown must not wait for open streams");
}

#[tokio::test]
async fn oversized_transcript_entries_are_skipped_not_loaded() {
    let f = fx().await;
    let path = f.path(&format!("cfg/runs/{}/transcript.jsonl", f.run));
    let mut text = std::fs::read_to_string(&path).unwrap();
    let big = json!({"ts": "t", "event": {"type": "assistant_message", "text": "x".repeat(2 * 1024 * 1024)}});
    text = format!("{big}\n{text}");
    std::fs::write(&path, text).unwrap();
    let (s, v) = get(&f.app, &format!("/api/v1/runs/{}/transcript", f.run)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["lines"].as_array().unwrap().len(),
        2,
        "the two normal lines survive"
    );
    assert_eq!(v["next_offset"], v["total_bytes"]);
}
