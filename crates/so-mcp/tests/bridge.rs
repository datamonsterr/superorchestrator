#![allow(clippy::unwrap_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ClientRequest, ProgressNotificationParam, Request,
    RequestMetaObject,
};
use rmcp::service::{NotificationContext, PeerRequestOptions, RunningService};
use rmcp::{ClientHandler, RoleClient, ServiceExt};
use serde_json::json;
use so_core::proto::{DEPTH_LIMIT, HarnessesView, RpcError};
use so_core::{Config, Paths};
use so_mcp::McpBridge;
use so_supervisor::{Supervisor, SupervisorClient, SupervisorHandle, SupervisorOptions};
use tempfile::TempDir;

#[derive(Clone, Default)]
struct Collector {
    progress: Arc<Mutex<Vec<ProgressNotificationParam>>>,
}

impl ClientHandler for Collector {
    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.progress.lock().unwrap().push(params);
    }
}

struct Fixture {
    _dir: TempDir,
    sup: SupervisorHandle,
    client: RunningService<RoleClient, Collector>,
    collector: Collector,
}

fn fixed_view() -> HarnessesView {
    HarnessesView::default()
}

async fn fixture(subagent: bool) -> Fixture {
    let dir = tempfile::Builder::new()
        .prefix("somcp-")
        .tempdir_in("/tmp")
        .unwrap();
    let cfg = dir.path().join("cfg");
    std::fs::create_dir_all(&cfg).unwrap();
    let sock = dir.path().join("run/s.sock");
    let opts = SupervisorOptions::new(Paths::from_dir(cfg), Config::default(), sock.clone())
        .with_idle_exit(Duration::from_secs(600))
        .with_setup(|st| {
            st.router.register("list_harnesses", |_c, _p| async {
                Ok(serde_json::to_value(fixed_view()).unwrap())
            });
            st.router.register("fork_agent", |_c, _p| async {
                Err(RpcError::new(DEPTH_LIMIT, "depth limit reached"))
            });
            st.router.register("wait_agent", |ctx, p| async move {
                for step in ["a", "b"] {
                    ctx.notifier
                        .notify(
                            "event",
                            json!({"id":"e","session_id":"s","run_id":"abcdefgh1234",
                                   "ts":"2026-01-01T00:00:00Z","type":"progress",
                                   "data":{"step":step,"status":"running"}}),
                        )
                        .await;
                }
                Ok(json!({"finished": p["runs"], "updates": [], "timed_out": false}))
            });
        });
    let sup = Supervisor::start(opts).await.unwrap();
    let (sc, _) = SupervisorClient::connect_or_spawn_hello(
        &sock,
        || Ok(()),
        "test",
        "/tmp",
        None, // the subagent flag only gates the tool list
        None,
    )
    .await
    .unwrap();
    let (st, ct) = tokio::io::duplex(65536);
    let bridge = McpBridge::new(sc, subagent);
    tokio::spawn(async move {
        let svc = bridge.serve(st).await.unwrap();
        let _ = svc.waiting().await;
    });
    let collector = Collector::default();
    let client = collector.clone().serve(ct).await.unwrap();
    Fixture {
        _dir: dir,
        sup,
        client,
        collector,
    }
}

fn text(r: &CallToolResult) -> String {
    serde_json::to_string(&r.content).unwrap()
}

#[tokio::test]
async fn server_info_and_tool_counts() {
    let f = fixture(false).await;
    let info = f.client.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "superorchestrator");
    assert_eq!(
        info.server_info.as_ref().unwrap().version,
        env!("CARGO_PKG_VERSION")
    );
    assert!(
        info.instructions
            .as_deref()
            .unwrap()
            .contains("spawn_agent")
    );
    let tools = f.client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 14);
    let spawn = tools.iter().find(|t| t.name == "spawn_agent").unwrap();
    assert!(spawn.input_schema.contains_key("properties"));
    assert!(tools.iter().all(|t| t.name != "ask_parent"));
    f.sup.shutdown().await;

    let f = fixture(true).await;
    let tools = f.client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 16);
    for n in ["report_progress", "ask_parent", "spawn_agent", "fork_agent"] {
        assert!(tools.iter().any(|t| t.name == n), "missing {n}");
    }
    f.sup.shutdown().await;
}

#[tokio::test]
async fn forwards_result_and_maps_rpc_error_to_tool_error() {
    let f = fixture(false).await;
    let r = f
        .client
        .call_tool(CallToolRequestParams::new("list_harnesses"))
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(false));
    assert_eq!(
        r.structured_content,
        Some(json!({"accounts":[],"choices":[]}))
    );

    let args = json!({"run": "r0", "prompt": "x"});
    let r = f
        .client
        .call_tool(
            CallToolRequestParams::new("fork_agent")
                .with_arguments(args.as_object().unwrap().clone()),
        )
        .await
        .unwrap();
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("depth limit reached"));

    // Unknown tool is a protocol error.
    assert!(
        f.client
            .call_tool(CallToolRequestParams::new("ask_parent"))
            .await
            .is_err()
    );
    f.sup.shutdown().await;
}

#[tokio::test]
async fn wait_agent_streams_progress() {
    let f = fixture(false).await;
    let mut meta = RequestMetaObject::default();
    let mut params = CallToolRequestParams::new("wait_agent").with_arguments(
        json!({"runs": ["abcdefgh1234"]})
            .as_object()
            .unwrap()
            .clone(),
    );
    params.meta = Some(std::mem::take(&mut meta));
    let handle = f
        .client
        .send_cancellable_request(
            ClientRequest::CallToolRequest(Request::new(params)),
            PeerRequestOptions::no_options(),
        )
        .await
        .unwrap();
    let resp = handle.await_response().await.unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["structuredContent"]["finished"], json!(["abcdefgh1234"]));
    let got = f.collector.progress.lock().unwrap().clone();
    assert!(got.len() >= 2, "got {got:?}");
    assert!(got[1].progress > got[0].progress);
    assert_eq!(
        got[0].message.as_deref(),
        Some("run abcdefgh: progress step=a status=running")
    );
    f.sup.shutdown().await;
}

#[tokio::test]
async fn no_progress_without_token_and_calls_are_concurrent() {
    let f = fixture(false).await;
    let a = f.client.call_tool(
        CallToolRequestParams::new("wait_agent")
            .with_arguments(json!({"runs": ["r"]}).as_object().unwrap().clone()),
    );
    let b = f
        .client
        .call_tool(CallToolRequestParams::new("list_harnesses"));
    let (a, b) = tokio::join!(a, b);
    assert_eq!(a.unwrap().is_error, Some(false));
    assert_eq!(b.unwrap().is_error, Some(false));
    assert!(f.collector.progress.lock().unwrap().is_empty());
    f.sup.shutdown().await;
}
