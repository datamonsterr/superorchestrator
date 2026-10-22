//! The `ServerHandler` that forwards tool calls to the supervisor.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, CancelTaskParams, ContentBlock,
    CreateTaskResult, ElicitRequestParams, ElicitationAction, ElicitationSchema, GetTaskParams,
    GetTaskResult, Implementation, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, ProgressToken, ServerCapabilities, ServerConfig, UpdateTaskParams,
};
use rmcp::service::{ElicitationMode, Peer, RequestContext};
use rmcp::task_manager::{TaskContext, TaskExit, TaskManager, TaskOptions};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{Value, json};
use so_core::proto::{METHOD_SEND_MESSAGE, METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, Notification};
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
/// Most times one `wait_agent` call asks the person before it returns what it has.
const MAX_ELICIT_ROUNDS: usize = 20;

/// How often a client should poll `tasks/get`.
const TASK_POLL_MS: u64 = 2_000;

type Cancelled = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// What one forwarded call needs from the MCP request or the task that carries it.
struct CallCx {
    peer: Peer<RoleServer>,
    /// Plain call: the client's progress token. Task call: none (tasks report status instead).
    token: Option<ProgressToken>,
    /// Task call: progress goes to the task's status message.
    task: Option<TaskContext>,
    /// Resolves when the request (or task) is cancelled. Called once per forwarded call.
    cancelled: Cancelled,
}

struct Inner {
    client: SupervisorClient,
    tasks: TaskManager,
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
                tasks: TaskManager::new(),
                subagent,
                events,
                fan_out,
            }),
        }
    }

    fn spec(&self, name: &str) -> Option<&'static ToolSpec> {
        tool_specs(self.inner.subagent).find(|s| s.name == name)
    }

    /// `wait_agent`, surfacing questions to the person when the client can elicit input
    /// (SPEC 9): each question is asked, the answer goes back as a `send_message` of kind
    /// `answer`, and the wait resumes. Without elicitation, or when the person declines,
    /// the questions stay in `updates` for the orchestrator to answer.
    async fn forward_wait(
        &self,
        spec: &ToolSpec,
        mut args: Value,
        cx: &CallCx,
    ) -> Result<Value, ClientError> {
        let started = std::time::Instant::now();
        let timeout_ms = args.get("timeout_ms").and_then(Value::as_u64);
        let mut result = Value::Null;
        for _ in 0..MAX_ELICIT_ROUNDS {
            result = self.forward(spec, args.clone(), cx).await?;
            let questions = questions_in(&result);
            if questions.is_empty()
                || !cx
                    .peer
                    .supported_elicitation_modes()
                    .contains(&ElicitationMode::Form)
            {
                return Ok(result);
            }
            let mut answered = 0;
            for q in &questions {
                let Some(text) = elicit(&cx.peer, q).await else {
                    continue;
                };
                let sent = self
                    .inner
                    .client
                    .call::<_, Value>(
                        METHOD_SEND_MESSAGE,
                        &json!({"run": q.run, "text": text, "kind": "answer"}),
                    )
                    .await;
                match sent {
                    Ok(_) => answered += 1,
                    Err(e) => tracing::warn!(error = %e, "could not send the elicited answer"),
                }
            }
            // Wait again only when nothing but answered questions ended this wait.
            let only_questions = result["finished"].as_array().is_none_or(Vec::is_empty)
                && result["updates"].as_array().map_or(0, Vec::len) == questions.len();
            if answered != questions.len() || !only_questions {
                return Ok(result);
            }
            if let Some(total) = timeout_ms {
                let left = total.saturating_sub(
                    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                if left == 0 {
                    return Ok(result);
                }
                args["timeout_ms"] = json!(left);
            }
        }
        // A person who keeps answering does not hold the call forever.
        Ok(result)
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
        cx: &CallCx,
    ) -> Result<Value, ClientError> {
        let long = is_long(spec, &args);
        let call = self.inner.client.call::<_, Value>(spec.name, &args);
        if !long || (cx.token.is_none() && cx.task.is_none()) {
            return tokio::select! {
                r = call => r,
                _ = (cx.cancelled)() => Err(ClientError::Disconnected),
            };
        }
        let runs: Option<Vec<String>> = (spec.name == METHOD_WAIT_AGENT)
            .then(|| serde_json::from_value(args["runs"].clone()).ok())
            .flatten();
        let mut rx = self.inner.events.subscribe();
        // Progress goes through a per-call queue and sender task, so a slow client never
        // stalls the select below (which must keep polling the call and cancellation).
        let (ptx, mut prx) = mpsc::channel::<ProgressNotificationParam>(PROGRESS_QUEUE);
        let peer = cx.peer.clone();
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
            if let Some(task) = &cx.task {
                task.set_status_message(msg);
            } else if let Some(token) = &cx.token {
                counter += 1.0;
                let p = ProgressNotificationParam::new(token.clone(), counter).with_message(msg);
                if ptx.try_send(p).is_err() {
                    tracing::debug!("progress queue full; dropped an update");
                }
            }
        };
        let mut cancelled = (cx.cancelled)();
        tokio::pin!(call);
        let result = loop {
            tokio::select! {
                r = &mut call => break r,
                _ = &mut cancelled => {
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

/// A `question_asked` update of a `wait_agent` result.
struct Question {
    run: String,
    question: String,
    options: Vec<String>,
}

fn questions_in(result: &Value) -> Vec<Question> {
    result["updates"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|e| e["type"] == "question_asked")
        .filter_map(|e| {
            Some(Question {
                run: e["run_id"].as_str()?.to_string(),
                question: e["data"]["question"].as_str()?.to_string(),
                options: e["data"]["options"]
                    .as_array()
                    .map(|o| {
                        o.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// Asks the person through the MCP client; `None` when they decline or it fails.
async fn elicit(peer: &Peer<RoleServer>, q: &Question) -> Option<String> {
    let mut answer = json!({"type": "string", "title": "Answer"});
    if !q.options.is_empty() {
        answer["enum"] = json!(q.options);
    }
    let schema = ElicitationSchema::from_json_schema(
        json!({
            "type": "object",
            "properties": {"answer": answer},
            "required": ["answer"],
        })
        .as_object()?
        .clone(),
    )
    .ok()?;
    let message = format!(
        "Run {} asks: {}",
        q.run.chars().take(8).collect::<String>(),
        q.question
    );
    let r = peer
        .create_elicitation(ElicitRequestParams::FormElicitationParams {
            meta: None,
            message,
            requested_schema: schema,
        })
        .await
        .ok()?;
    if r.action != ElicitationAction::Accept {
        return None;
    }
    r.content?["answer"].as_str().map(String::from)
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

/// Tools that can run for a long time: they stream progress, and run as MCP tasks when the
/// client supports tasks. `spawn_agent` is long only with `wait: true`.
fn is_long(spec: &ToolSpec, args: &Value) -> bool {
    spec.long
        && (spec.name != METHOD_SPAWN_AGENT
            || args.get("wait").and_then(Value::as_bool) == Some(true))
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

impl McpBridge {
    /// Forwards one tool call, with the elicitation loop for `wait_agent`.
    async fn run_tool(
        &self,
        spec: &ToolSpec,
        args: Value,
        cx: &CallCx,
    ) -> Result<Value, ClientError> {
        if spec.name == METHOD_WAIT_AGENT {
            self.forward_wait(spec, args, cx).await
        } else {
            self.forward(spec, args, cx).await
        }
    }

    /// Starts a long call as an MCP task (SEP-2663) and returns its handle. Progress goes to
    /// the task's status message. `tasks/cancel` stops waiting; like cancelling a plain
    /// call, it does not cancel the runs (use `cancel_agent`).
    fn spawn_task(
        &self,
        spec: &'static ToolSpec,
        args: Value,
        peer: Peer<RoleServer>,
    ) -> CreateTaskResult {
        let bridge = self.clone();
        let options = TaskOptions::new()
            // A wait can last for hours. The bridge lives as long as one MCP connection,
            // so keeping finished tasks until then is bounded.
            .with_ttl_ms(None::<u64>)
            .with_poll_interval_ms(TASK_POLL_MS)
            .with_status_message(format!("{} started", spec.name));
        let task = self.inner.tasks.spawn(options, move |task_cx| {
            Box::pin(async move {
                let watcher = task_cx.clone();
                let cx = CallCx {
                    peer,
                    token: None,
                    task: Some(task_cx.clone()),
                    cancelled: Arc::new(move || {
                        let t = watcher.clone();
                        Box::pin(async move { t.cancelled().await })
                    }),
                };
                match bridge.run_tool(spec, args, &cx).await {
                    Err(ClientError::Disconnected) if task_cx.is_cancel_requested() => {
                        Err(TaskExit::Cancelled)
                    }
                    Ok(v) => Ok(into_result(v)),
                    Err(e) => Ok(tool_error(e.to_string())),
                }
            })
        });
        CreateTaskResult::new(task)
    }
}

impl ServerHandler for McpBridge {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tasks()
                .build(),
        )
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
        // Clients that declare the tasks extension get a task for the long tools; the others
        // block and get MCP progress notifications.
        let client_tasks = context
            .client_capabilities()
            .is_some_and(|caps| caps.supports_tasks());
        if client_tasks && is_long(spec, &args) {
            return Ok(CallToolResponse::Task(self.spawn_task(
                spec,
                args,
                context.peer.clone(),
            )));
        }
        let ct = context.ct.clone();
        let cx = CallCx {
            peer: context.peer.clone(),
            token: context.meta.get_progress_token(),
            task: None,
            cancelled: Arc::new(move || {
                let ct = ct.clone();
                Box::pin(async move { ct.cancelled().await })
            }),
        };
        let result = match self.run_tool(spec, args, &cx).await {
            Ok(v) => into_result(v),
            Err(e) => tool_error(e.to_string()),
        };
        Ok(result.into())
    }

    async fn get_task(
        &self,
        request: GetTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<GetTaskResult, ErrorData> {
        Ok(GetTaskResult::new(
            self.inner.tasks.get_task(&request.task_id)?,
        ))
    }

    async fn update_task(
        &self,
        request: UpdateTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.inner
            .tasks
            .update_task(&request.task_id, request.input_responses)
    }

    async fn cancel_task(
        &self,
        request: CancelTaskParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.inner.tasks.cancel_task(&request.task_id)
    }
}
