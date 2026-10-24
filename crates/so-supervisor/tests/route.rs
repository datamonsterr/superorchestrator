#![allow(clippy::unwrap_used)]
//! The `route` method over the supervisor socket.

mod common;

use std::time::Duration;

use common::level;

use serde_json::{Value, json};
use so_core::proto::{RouteResult, RouteSource};
use so_core::{AccountFile, Config, EventType, Harness, Paths};
use so_store::Store;
use so_supervisor::{ClientError, Supervisor, SupervisorClient, SupervisorOptions};

#[tokio::test]
async fn route_previews_and_records_a_route_decision_event() {
    let dir = tempfile::Builder::new()
        .prefix("so-")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = Paths::from_dir(dir.path().join("cfg"));
    AccountFile::new("agy-a", Harness::Agy, "/h/a", "agy")
        .unwrap()
        .save(&paths)
        .unwrap();
    AccountFile::new("codex-a", Harness::Codex, "/h/c", "codex")
        .unwrap()
        .save(&paths)
        .unwrap();
    let mut config = Config::default();
    config.router.levels = vec![
        level("low", 0, 37, &["agy/gemini-3.8-flash-low"]),
        level("high", 38, 100, &["codex/gpt-6.1-sol"]),
    ];
    config.router.jev.key_env = "SO_ROUTE_TEST_NO_SUCH_KEY".into();
    let opts = SupervisorOptions::new(paths.clone(), config, dir.path().join("run/s.sock"))
        .with_idle_exit(Duration::from_secs(600))
        .with_lock_wait(Duration::ZERO)
        .with_setup(so_supervisor::default_handlers);
    let h = Supervisor::start(opts).await.unwrap();
    let c = SupervisorClient::connect(h.socket_path()).await.unwrap();
    let hello = c.hello("test", "/w", None, Some(7)).await.unwrap();

    let r: RouteResult = c
        .call(
            "route",
            json!({"prompt": "Refactor the architecture and concurrency of the scheduler"}),
        )
        .await
        .unwrap();
    assert_eq!(r.source, RouteSource::Heuristic);
    assert_eq!(r.choice_id, "codex/gpt-6.1-sol");
    assert_eq!(r.account, "codex-a");

    let store = Store::open(paths.state_db()).unwrap();
    let events = store
        .list_events_by_session(&hello.session_id, None, 100)
        .unwrap();
    let ev = events
        .iter()
        .find(|e| e.event_type == EventType::RouteDecision)
        .expect("route_decision event");
    assert_eq!(ev.data["choice_id"], "codex/gpt-6.1-sol");
    assert!(!ev.data.to_string().contains("scheduler"), "no task text");
    // Preview only: no run was created.
    assert_eq!(store.count_active_runs_total().unwrap(), 0);

    let bad: Result<Value, _> = c
        .call("route", json!({"prompt": "x", "model": "codex/"}))
        .await;
    match bad {
        Err(ClientError::Rpc(e)) => assert_eq!(e.code, so_core::proto::INVALID_PARAMS),
        other => panic!("{other:?}"),
    }
    let bad: Result<Value, _> = c.call("route", json!({"nope": 1})).await;
    assert!(matches!(bad, Err(ClientError::Rpc(_))));
    h.shutdown().await;
}
