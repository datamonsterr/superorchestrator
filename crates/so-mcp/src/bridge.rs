//! The `ServerHandler` that forwards tool calls to the supervisor.

use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ProgressNotificationParam, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use so_core::proto::{METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, Notification};
use so_supervisor::{ClientError, SupervisorClient};
use tokio::sync::{broadcast, mpsc};

use crate::tools::{ToolSpec, tool_specs};

const INSTRUCTIONS: &str = "Superorchestrator delegates work to coding agents on other harnesses and accounts. \
Use list_harnesses to see accounts and headroom and route to preview a routing decision; spawn_agent starts a run \
(wait=true blocks for the result), wait_agent waits on runs, get_run inspects one, send_message and read_messages \
exchange messages, cancel_agent stops a run and its subtree, accept_run and reject_run decide reviewed runs, \
fork_agent branches a finished run, list_agents lists custom agents, and run_workflow and workflow_status run \
scripted workflows. Subagents also get report_progress and ask_parent. Tool failures are returned as errors you can read.";

const PROGRESS_QUEUE: usize = 64;

struct Inner {
    client: SupervisorClient,
    subagent: bool,
    events: broadcast::Sender<Arc<Notification>>,
    fan_out: tokio::task::JoinHandle<()>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.fan_out.abort();
    }
}

/// The MCP server: one per stdio process, wrapping an already-connected supervisor client.
#[derive(Clone)]
pub struct McpBridge {
    inner: Arc<Inner>,
}

impl McpBridge {
    /// `client` must already have completed `hello`. `subagent` adds `report_progress` and
    /// `ask_parent` to the tool list.
    pub fn new(client: SupervisorClient, subagent: bool) -> Self {
        let (events, _) = broadcast::channel(256);
        let mut rx: mpsc::UnboundedReceiver<Notification> =
            client.notifications().unwrap_or_else(|| {
                tracing::warn!("supervisor notifications already taken; progress is disabled");
                mpsc::unbounded_channel().1
            });
        let tx = events.clone();
        let fan_out = tokio::spawn(async move {
            while let Some(n) = rx.recv().await {
                let _ = tx.send(Arc::new(n)); // no subscribers is fine
            }
        });
        Self {
            inner: Arc::new(Inner {
                client,
                subagent,
                events,
                fan_out,
            }),
        }
    }

    fn spec(&self, name: &str) -> Option<&'static ToolSpec> {
        tool_specs(self.inner.subagent).find(|s| s.name == name)
    }

    /// Forwards one call. Cancelling the MCP request drops the call but does not cancel the
    /// run on the supervisor (use `cancel_agent`). Progress for `spawn_agent` (wait) and
    /// `run_workflow` is connection-scoped: every event pushed to this session is relayed.
    ///
    /// For long tools with a progress token, relays supervisor `event` notifications as MCP progress while the call is pending.
    async fn forward(
        &self,
        spec: &ToolSpec,
        args: Value,
        ctx: &RequestContext<RoleServer>,
    ) -> Result<Value, ClientError> {
        let token = ctx.meta.get_progress_token();
        let long = spec.long
            && (spec.name != METHOD_SPAWN_AGENT
                || args.get("wait").and_then(Value::as_bool) == Some(true));
        let call = self.inner.client.call::<_, Value>(spec.name, &args);
        let (Some(token), true) = (token, long) else {
            return tokio::select! {
                r = call => r,
                _ = ctx.ct.cancelled() => Err(ClientError::Disconnected),
            };
        };
        let runs: Option<Vec<String>> = (spec.name == METHOD_WAIT_AGENT)
            .then(|| serde_json::from_value(args["runs"].clone()).ok())
            .flatten();
        let mut rx = self.inner.events.subscribe();
        // Progress goes through a per-call queue and sender task, so a slow client never
        // stalls the select below (which must keep polling the call and cancellation).
        let (ptx, mut prx) = mpsc::channel::<ProgressNotificationParam>(PROGRESS_QUEUE);
        let peer = ctx.peer.clone();
        let sender = tokio::spawn(async move {
            while let Some(p) = prx.recv().await {
                let _ = peer.notify_progress(p).await;
            }
        });
        let mut counter = 0f64;
        let mut relay = |n: &Notification| {
            let Some(msg) = progress_message(n, runs.as_deref()) else {
                return;
            };
            counter += 1.0;
            let p = ProgressNotificationParam::new(token.clone(), counter).with_message(msg);
            if ptx.try_send(p).is_err() {
                tracing::debug!("progress queue full; dropped an update");
            }
        };
        tokio::pin!(call);
        let result = loop {
            tokio::select! {
                r = &mut call => break r,
                _ = ctx.ct.cancelled() => {
                    sender.abort();
                    return Err(ClientError::Disconnected);
                }
                ev = rx.recv() => match ev {
                    Ok(n) => relay(&n),
                    // The bridge owns a sender, so the channel cannot close while we run.
                    Err(e) => tracing::debug!("skipped progress events: {e}"),
                },
            }
        };
        // Events travel through the fan-out task and may land just after the response:
        // yield so it can run, then drain until the channel is empty.
        loop {
            tokio::task::yield_now().await;
            match rx.try_recv() {
                Ok(n) => relay(&n),
                Err(broadcast::error::TryRecvError::Lagged(n)) => {
                    tracing::debug!("skipped {n} progress events");
                }
                Err(_) => break,
            }
        }
        drop(relay);
        drop(ptx);
        let _ = sender.await; // deliver queued progress before the result
        result
    }
}

/// "run <id8>: <type> step=<..> status=<..>" for a supervisor `event` notification;
/// `None` for other notifications or events of runs outside `runs`.
fn progress_message(n: &Notification, runs: Option<&[String]>) -> Option<String> {
    if n.method != "event" {
        return None;
    }
    let p = &n.params;
    let run = p.get("run_id").and_then(Value::as_str);
    if let (Some(run), Some(runs)) = (run, runs)
        && !runs.iter().any(|r| r == run)
    {
        return None;
    }
    let kind = p.get("type").and_then(Value::as_str).unwrap_or("event");
    let mut msg = match run {
        Some(r) => format!("run {}: {kind}", r.chars().take(8).collect::<String>()),
        None => kind.to_string(),
    };
    for key in ["step", "status"] {
        if let Some(v) = p.pointer(&format!("/data/{key}")).and_then(Value::as_str) {
            msg.push_str(&format!(" {key}={v}"));
        }
    }
    Some(msg)
}

fn tool_error(text: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn into_result(value: Value) -> CallToolResult {
    if value.is_object() {
        CallToolResult::structured(value)
    } else {
        CallToolResult::success(vec![ContentBlock::text(value.to_string())])
    }
}

impl ServerHandler for McpBridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "superorchestrator",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            tool_specs(self.inner.subagent)
                .map(ToolSpec::tool)
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(spec) = self.spec(&request.name) else {
            return Err(ErrorData::invalid_params(
                format!("unknown tool `{}`", request.name),
                None,
            ));
        };
        let args = request.arguments.map_or_else(|| json!({}), Value::Object);
        let result = match self.forward(spec, args, &context).await {
            Ok(v) => into_result(v),
            Err(e) => tool_error(e.to_string()),
        };
        Ok(result.into())
    }
}
