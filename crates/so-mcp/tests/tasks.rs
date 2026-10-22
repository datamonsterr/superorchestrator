//! MCP tasks (SEP-2663): clients that declare the extension get a task for the long tools;
//! everything else keeps the plain call (see `bridge.rs`).
#![allow(clippy::unwrap_used)]

use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ClientCapabilities,
    ClientConfig, CreateTaskResult, GetTaskParams, Implementation, TaskPayload, TaskStatus,
};
use rmcp::service::RunningService;
use rmcp::{RoleClient, ServiceExt};
use serde_json::{Value, json};
use so_core::{Config, Paths};
use so_supervisor::{Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions};
use tempfile::TempDir;

struct Fixture {
    _dir: TempDir,
    sup: SupervisorHandle,
    client: RunningService<RoleClient, ClientConfig>,
}

async fn fixture(tasks: bool) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("somcpt-")
        .tempdir_in("/tmp")
        .unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let sock = dir.path().join("run/s.sock");
    let opts = SupervisorOptions::new(Paths::from_dir(cfg), Config::default(), sock.clone())
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(|st| {
            st.router
                .register("list_agents", |_c, _p| async { Ok(json!({"agents": []})) });
            st.router.register("spawn_agent", |ctx, p| async move {
                ctx.notifier
                    .notify(
                        "event",
                        json!({"type": "progress", "run_id": "abcdefgh1234",
                               "data": {"step": "build", "status": "running"}}),
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(json!({"run_id": "abcdefgh1234", "wait": p["wait"]}))
            });
            st.router.register("wait_agent", |_c, p| async move {
                Ok(json!({"finished": p["runs"], "updates": [], "timed_out": false}))
            });
            st.router.register("run_workflow", |_c, p| async move {
                if p["script"] == "hang" {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
                Err(so_core::proto::RpcError::new(
                    so_core::proto::DEPTH_LIMIT,
                    "workflow refused",
                ))
            });
        });
    let sup = Supervisor::start(opts).await.unwrap();
    let (sc, _) =
        SupervisorClient::connect_or_spawn_hello(&sock, || Ok(()), "test", "/tmp", None, None)
            .await
            .unwrap();
    let (st, ct) = tokio::io::duplex(65536);
    let bridge = so_mcp::McpBridge::new(sc, false);
    tokio::spawn(async move {
        let svc = bridge.serve(st).await.unwrap();
        let _ = svc.waiting().await;
    });
    let caps = if tasks {
        ClientCapabilities::builder().enable_tasks().build()
    } else {
        ClientCapabilities::builder().build()
    };
    let client = ClientConfig::new(caps, Implementation::from_build_env())
        .serve(ct)
        .await
        .unwrap();
    Fixture {
        _dir: dir,
        sup,
        client,
    }
}

async fn call(f: &Fixture, name: &'static str, args: Value) -> CallToolResponse {
    f.client
        .call_tool_once(
            CallToolRequestParams::new(name).with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap()
}

fn task_of(r: CallToolResponse) -> CreateTaskResult {
    match r {
        CallToolResponse::Task(t) => t,
        other => panic!("expected a task, got {other:?}"),
    }
}

async fn settle(f: &Fixture, id: &str) -> rmcp::model::DetailedTask {
    for _ in 0..200 {
        let info = f
            .client
            .peer()
            .get_task(GetTaskParams::new(id))
            .await
            .unwrap();
        if info.task.status().is_terminal() {
            return info.task;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("task did not settle");
}

fn completed(t: rmcp::model::DetailedTask) -> CallToolResult {
    match t.payload {
        TaskPayload::Completed { result } => serde_json::from_value(Value::Object(result)).unwrap(),
        other => panic!("expected a completed task, got {other:?}"),
    }
}

#[tokio::test]
async fn the_server_advertises_tasks() {
    let f = fixture(true).await;
    let caps = &f.client.peer_info().unwrap().capabilities;
    assert!(caps.supports_tasks());
    f.sup.shutdown().await;
}

#[tokio::test]
async fn long_tools_become_tasks_for_a_task_client() {
    let f = fixture(true).await;

    let t = task_of(call(&f, "spawn_agent", json!({"prompt": "x", "wait": true})).await);
    let r = completed(settle(&f, &t.task.task_id).await);
    assert_eq!(r.is_error, Some(false));
    assert_eq!(
        r.structured_content,
        Some(json!({"run_id": "abcdefgh1234", "wait": true}))
    );

    let t = task_of(call(&f, "wait_agent", json!({"runs": ["abcdefgh1234"]})).await);
    let r = completed(settle(&f, &t.task.task_id).await);
    assert_eq!(
        r.structured_content.unwrap()["finished"],
        json!(["abcdefgh1234"])
    );

    // A tool error is a completed task whose result has isError, as for a plain call.
    let t = task_of(call(&f, "run_workflow", json!({"script": "x"})).await);
    let r = completed(settle(&f, &t.task.task_id).await);
    assert_eq!(r.is_error, Some(true));
    f.sup.shutdown().await;
}

#[tokio::test]
async fn progress_becomes_the_task_status_message() {
    let f = fixture(true).await;
    let t = task_of(call(&f, "spawn_agent", json!({"prompt": "x", "wait": true})).await);
    let mut seen = false;
    for _ in 0..200 {
        let info = f
            .client
            .peer()
            .get_task(GetTaskParams::new(t.task.task_id.clone()))
            .await
            .unwrap();
        if info.task.task.status_message.as_deref()
            == Some("run abcdefgh: progress step=build status=running")
        {
            seen = true;
            break;
        }
        if info.task.status().is_terminal() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(seen, "the progress event never reached the status message");
    f.sup.shutdown().await;
}

#[tokio::test]
async fn short_calls_stay_plain_for_a_task_client() {
    let f = fixture(true).await;
    // list_agents is not long; spawn_agent without wait returns at once.
    for (name, args) in [
        ("list_agents", json!({})),
        ("spawn_agent", json!({"prompt": "x"})),
    ] {
        match call(&f, name, args).await {
            CallToolResponse::Complete(r) => assert_eq!(r.is_error, Some(false), "{name}"),
            other => panic!("{name}: expected a plain result, got {other:?}"),
        }
    }
    f.sup.shutdown().await;
}

#[tokio::test]
async fn a_client_without_tasks_blocks_and_gets_the_result() {
    let f = fixture(false).await;
    let r = f
        .client
        .call_tool(
            CallToolRequestParams::new("spawn_agent").with_arguments(
                json!({"prompt": "x", "wait": true})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(
        r.structured_content,
        Some(json!({"run_id": "abcdefgh1234", "wait": true}))
    );
    f.sup.shutdown().await;
}

#[tokio::test]
async fn cancelling_a_task_settles_it_as_cancelled() {
    let f = fixture(true).await;
    let t = task_of(call(&f, "run_workflow", json!({"script": "hang"})).await);
    let id = t.task.task_id;
    let info = f
        .client
        .peer()
        .get_task(GetTaskParams::new(id.clone()))
        .await
        .unwrap();
    assert_eq!(info.task.status(), TaskStatus::Working);
    f.client
        .peer()
        .cancel_task(CancelTaskParams::new(id.clone()))
        .await
        .unwrap();
    assert_eq!(settle(&f, &id).await.status(), TaskStatus::Cancelled);
    f.sup.shutdown().await;
}

#[tokio::test]
async fn an_unknown_task_id_is_an_error() {
    let f = fixture(true).await;
    assert!(
        f.client
            .peer()
            .get_task(GetTaskParams::new("nope"))
            .await
            .is_err()
    );
    f.sup.shutdown().await;
}
