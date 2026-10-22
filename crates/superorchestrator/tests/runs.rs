#![allow(clippy::unwrap_used)]
//! `runs` and `join` against an in-process supervisor driving the fake harness.

mod common;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use common::{Env, add_account, cli, stdout};
use serde_json::{Value, json};
use so_accounts::HostEnv;
use so_core::proto::{
    METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, SpawnParams, SpawnResult, WaitParams, WaitResult,
};
use so_core::{Config, Platform};
use so_supervisor::{Supervisor, SupervisorClient, SupervisorOptions, default_handlers};

#[tokio::test(flavor = "multi_thread")]
async fn runs_list_show_and_join_print() {
    let e = Env::new();
    // The socket directory must be private.
    std::fs::set_permissions(e.tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    add_account(&e);
    let host = HostEnv {
        home: e.home.clone(),
        path_var: Some("/usr/bin:/bin".into()),
        platform: Platform::current(),
    };
    let opts = SupervisorOptions::new(e.paths(), Config::default(), e.tmp.path().join("so.sock"))
        .with_host(host)
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(default_handlers);
    let sup = Supervisor::start(opts).await.unwrap();

    let c = SupervisorClient::connect(sup.socket_path()).await.unwrap();
    c.hello("test", e.tmp.path().to_str().unwrap(), None, None)
        .await
        .unwrap();
    let r: SpawnResult = c
        .call(
            METHOD_SPAWN_AGENT,
            SpawnParams {
                prompt: Some("go".into()),
                harness: Some("claude".into()),
                account: Some("claude-acc1".into()),
                isolation: Some(so_core::Isolation::None),
                name: Some("demo".into()),
                ..SpawnParams::default()
            },
        )
        .await
        .unwrap();
    let w: WaitResult = c
        .call(
            METHOD_WAIT_AGENT,
            WaitParams {
                runs: vec![r.run_id.clone()],
                timeout_ms: Some(20_000),
            },
        )
        .await
        .unwrap();
    assert_eq!(w.finished, vec![r.run_id.clone()]);

    // Active-only list is empty, --all has the run.
    let (out, ok) = cli(&e, &["runs", "list", "--json"]).await;
    assert!(ok);
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), json!([]));
    let (out, ok) = cli(&e, &["runs", "list", "--all", "--json"]).await;
    assert!(ok);
    let rows: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(rows[0]["id"], r.run_id);
    assert_eq!(rows[0]["state"], "succeeded");
    assert_eq!(rows[0]["name"], "demo");

    let prefix = &r.run_id[..13];
    let (out, ok) = cli(&e, &["runs", "show", prefix]).await;
    assert!(ok);
    assert!(
        out.contains("succeeded") && out.contains("run_finished"),
        "{out}"
    );

    let (out, ok) = cli(&e, &["join", prefix]).await;
    assert!(ok);
    assert!(out.contains("claude-acc1 --resume "), "{out}");
    assert!(!out.contains(".claude-acc1/"), "{out}");

    let (out, ok) = cli(&e, &["runs", "show", "ffffffff-no-such-run"]).await;
    assert!(!ok && out.is_empty());
    // Inspection-only commands (SPEC 3.1: only `mcp` opens a session) left no session behind:
    // the one session is this test's own client.
    let (_, ok) = cli(&e, &["status"]).await;
    assert!(ok);
    let sessions = e
        .store()
        .list_sessions(so_store::SessionFilter::All, 100)
        .unwrap();
    assert_eq!(sessions.len(), 1, "{sessions:?}");
    assert_eq!(sessions[0].client, "test");
    sup.shutdown().await;
}

#[test]
fn worktrees_list_json_is_empty_without_a_store() {
    let e = Env::new();
    let out = e
        .cmd()
        .args(["worktrees", "list", "--json"])
        .assert()
        .success();
    assert_eq!(
        serde_json::from_str::<Value>(&stdout(&out)).unwrap(),
        json!([])
    );
    e.cmd()
        .args(["worktrees", "prune", "--yes"])
        .assert()
        .success();
}
