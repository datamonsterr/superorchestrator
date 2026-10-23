//! Method registry and the built-in `hello` and `status` methods.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::Value;
use so_core::SessionKind;
use so_core::proto::{
    HelloParams, HelloResult, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_CANCEL_AGENT,
    METHOD_CANCEL_WORKFLOW, METHOD_GET_RUN, METHOD_JOIN_RUN, METHOD_LIST_AGENTS,
    METHOD_LIST_HARNESSES, METHOD_NOT_FOUND, METHOD_READ_MESSAGES, METHOD_ROUTE, METHOD_STATUS,
    METHOD_UI_START, METHOD_WAIT_AGENT, METHOD_WORKFLOW_STATUS, NOT_FOUND, Notification, Request,
    Response, RpcError, StatusResult,
};
use so_store::SessionRow;
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, watch};

use crate::run_identity::run_token_hash;
use crate::server::SupervisorState;

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>>;
type Handler = Arc<dyn Fn(ConnCtx, Value) -> HandlerFuture + Send + Sync>;

/// Sends server-initiated notifications on one connection.
#[derive(Debug, Clone)]
pub struct Notifier {
    tx: mpsc::Sender<String>,
}

impl Notifier {
    pub(crate) fn new(tx: mpsc::Sender<String>) -> Self {
        Self { tx }
    }

    /// Queues a notification line. Returns `false` when the connection is gone.
    pub async fn notify(&self, method: &str, params: impl Serialize) -> bool {
        let Ok(params) = serde_json::to_value(params) else {
            return false;
        };
        let note = Notification {
            jsonrpc: so_core::proto::JSONRPC.to_string(),
            method: method.to_string(),
            params,
        };
        match serde_json::to_string(&note) {
            Ok(line) => self.send_line(line).await,
            Err(_) => false,
        }
    }

    pub(crate) async fn send_line(&self, mut line: String) -> bool {
        line.push('\n');
        self.tx.send(line).await.is_ok()
    }

    /// Non-blocking send for protocol-level replies; `false` when the queue is full or the
    /// connection is gone (the caller then drops the connection).
    pub(crate) fn try_send_line(&self, mut line: String) -> bool {
        line.push('\n');
        self.tx.try_send(line).is_ok()
    }
}

/// Per-connection context handed to every handler.
#[derive(Debug, Clone)]
pub struct ConnCtx {
    /// `None` until `hello`, and after an `inspect` hello (no session was opened).
    pub session_id: Option<String>,
    /// `hello` has completed.
    pub(crate) greeted: bool,
    pub run_id: Option<String>,
    pub depth: u8,
    pub notifier: Notifier,
    closed: watch::Receiver<bool>,
    /// The caller is the web UI, not an orchestrator or run (recorded in event data).
    pub via_ui: bool,
    /// Keeps the `closed` channel open for contexts that have no connection.
    _keep: Option<Arc<watch::Sender<bool>>>,
}

impl ConnCtx {
    pub(crate) fn new(notifier: Notifier, closed: watch::Receiver<bool>) -> Self {
        Self {
            session_id: None,
            greeted: false,
            run_id: None,
            depth: 0,
            notifier,
            closed,
            via_ui: false,
            _keep: None,
        }
    }

    /// A greeted local operator connection that opened no session and claimed no run.
    pub(crate) fn is_inspect(&self) -> bool {
        self.greeted && self.session_id.is_none() && self.run_id.is_none()
    }

    /// A context for a person acting through the web UI: it belongs to `session_id` and
    /// is the session's orchestrator (no run). It has no connection, so nothing is
    /// notified and `closed()` never resolves.
    ///
    /// Authorization is not checked here: whoever reaches the web API (loopback, or the
    /// bearer token on other binds) may act in any session. The caller must have
    /// authenticated the request.
    /// Who is acting, for event data.
    pub fn actor(&self) -> &'static str {
        if self.via_ui { "ui" } else { "orchestrator" }
    }

    pub fn acting_in(session_id: &str) -> Self {
        let (tx, _rx) = mpsc::channel(1);
        let (closed_tx, closed) = watch::channel(false);
        Self {
            session_id: Some(session_id.to_string()),
            greeted: true,
            run_id: None,
            depth: 0,
            notifier: Notifier::new(tx),
            closed,
            via_ui: true,
            _keep: Some(Arc::new(closed_tx)),
        }
    }

    /// The same caller, but its disconnecting no longer ends what it started: for a workflow
    /// that keeps running after the call that began it has gone. Notifications still go to
    /// the client while it is there.
    pub fn detached(&self) -> Self {
        let (closed_tx, closed) = watch::channel(false);
        Self {
            closed,
            _keep: Some(Arc::new(closed_tx)),
            ..self.clone()
        }
    }

    /// Resolves when the client has disconnected (read EOF or write failure) or the
    /// supervisor is shutting down. Long-running handlers (for example `wait_agent`)
    /// should `select!` on this to stop waiting.
    pub async fn closed(&self) {
        let mut rx = self.closed.clone();
        let _ = rx.wait_for(|v| *v).await;
    }

    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }
}

/// Registry `method name -> async handler`. Later tasks register their tools here.
#[derive(Default)]
pub struct Router {
    map: RwLock<HashMap<String, Handler>>,
}

impl Router {
    /// Registers (or replaces) the handler for `method`.
    ///
    /// A handler keeps running after the client half-closes (EOF), so its reply can still
    /// be delivered, but it is dropped (aborted at an await point) when the write side
    /// fails or the supervisor shuts down. Handlers must therefore be cancel-safe, or hand
    /// their work to a detached task. Handlers that wait for long should `select!` on
    /// [`ConnCtx::closed`] to stop waiting once the client is gone. A panicking handler
    /// yields an INTERNAL_ERROR reply for that request only.
    pub fn register<F, Fut>(&self, method: &str, f: F)
    where
        F: Fn(ConnCtx, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value, RpcError>> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |ctx, params| Box::pin(f(ctx, params)));
        self.map
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(method.to_string(), handler);
    }

    fn get(&self, method: &str) -> Option<Handler> {
        self.map
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(method)
            .cloned()
    }
}

pub(crate) fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(INTERNAL_ERROR, e.to_string())
}

fn into_response(id: Value, r: Result<Value, RpcError>) -> Response {
    match r {
        Ok(v) => Response {
            jsonrpc: so_core::proto::JSONRPC.to_string(),
            id,
            result: Some(v),
            error: None,
        },
        Err(e) => Response::err(id, e),
    }
}

/// `hello` mutates the connection context, so the connection loop runs it inline.
pub(crate) async fn handle_hello(
    state: &Arc<SupervisorState>,
    ctx: &mut ConnCtx,
    req: Request,
) -> Response {
    let r = hello(state, ctx, req.params).await;
    into_response(req.id, r)
}

/// What a connection that opened no session (`hello` with `inspect`) may call: reads, plus the
/// controls the CLI offers on any run (`runs cancel`, `join`), `ui_start` and `status`.
/// Everything else (spawning, messaging, review, fork) belongs to a session.
const INSPECT_METHODS: &[&str] = &[
    METHOD_STATUS,
    METHOD_ROUTE,
    METHOD_GET_RUN,
    METHOD_WAIT_AGENT,
    METHOD_READ_MESSAGES,
    METHOD_LIST_HARNESSES,
    METHOD_LIST_AGENTS,
    METHOD_WORKFLOW_STATUS,
    METHOD_CANCEL_AGENT,
    METHOD_CANCEL_WORKFLOW,
    METHOD_JOIN_RUN,
    METHOD_UI_START,
];

/// Every other method, run on a clone of the context.
pub(crate) async fn handle_other(
    state: &Arc<SupervisorState>,
    ctx: ConnCtx,
    req: Request,
) -> Response {
    // `hello` never gets here: the connection loop runs it through `handle_hello`.
    let r = if !ctx.greeted {
        Err(RpcError::new(INVALID_REQUEST, "hello required"))
    } else if ctx.session_id.is_none() && !INSPECT_METHODS.contains(&req.method.as_str()) {
        Err(RpcError::new(
            INVALID_REQUEST,
            format!(
                "this connection is read-only (inspect hello opened no session): {} needs one",
                req.method
            ),
        ))
    } else if req.method == METHOD_STATUS {
        status_result(state)
            .await
            .and_then(|r| serde_json::to_value(r).map_err(internal))
    } else if let Some(h) = state.router.get(&req.method) {
        h(ctx, req.params).await
    } else {
        Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("unknown method: {}", req.method),
        ))
    };
    into_response(req.id, r)
}

async fn hello(
    state: &Arc<SupervisorState>,
    ctx: &mut ConnCtx,
    params: Value,
) -> Result<Value, RpcError> {
    if ctx.greeted {
        return Err(RpcError::new(INVALID_REQUEST, "hello already completed"));
    }
    let p: HelloParams = serde_json::from_value(params)
        .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid hello params: {e}")))?;
    if p.inspect && p.run_id.is_none() {
        // Inspection only: no session row, so a `status` or `runs show` leaves no trace.
        ctx.greeted = true;
        return serde_json::to_value(HelloResult {
            session_id: String::new(),
            run_id: None,
            depth: 0,
        })
        .map_err(internal);
    }
    let result = if let Some(run_id) = p.run_id {
        let id = run_id.clone();
        let run = state
            .db(move |s| s.get_run(&id))
            .await?
            .ok_or_else(|| RpcError::new(NOT_FOUND, format!("run not found: {run_id}")))?;
        let id = run.id.clone();
        let expected = state.db(move |s| s.get_run_token_hash(&id)).await?;
        let valid = p
            .run_token
            .as_deref()
            .zip(expected.as_deref())
            .is_some_and(|(token, hash)| {
                let actual = run_token_hash(token);
                bool::from(actual.as_slice().ct_eq(hash))
            });
        if !valid {
            return Err(RpcError::new(INVALID_REQUEST, "invalid run identity token"));
        }
        HelloResult {
            session_id: run.session_id,
            run_id: Some(run.id),
            depth: u32::try_from(run.depth).unwrap_or(0),
        }
    } else {
        let row = SessionRow {
            id: so_core::new_id(),
            kind: SessionKind::Orchestrator,
            client: p.client,
            cwd: Some(p.cwd),
            account: None,
            pid: p.pid.map(i64::from),
            started_at: chrono::Utc::now(),
            ended_at: None,
            status: "active".to_string(),
        };
        let id = row.id.clone();
        state.db(move |s| s.insert_session(&row)).await?;
        HelloResult {
            session_id: id,
            run_id: None,
            depth: 0,
        }
    };
    ctx.greeted = true;
    ctx.session_id = Some(result.session_id.clone());
    ctx.run_id = result.run_id.clone();
    ctx.depth = u8::try_from(result.depth).unwrap_or(u8::MAX);
    serde_json::to_value(result).map_err(internal)
}

/// The `status` result: pid, version, uptime and counts.
pub async fn status_result(state: &Arc<SupervisorState>) -> Result<StatusResult, RpcError> {
    let (sessions, runs) = state
        .db(|s| {
            // Dead wrapper sessions are not active sessions.
            s.reap_dead_interactive_sessions()?;
            Ok((s.count_active_sessions()?, s.count_active_runs()?))
        })
        .await?;
    Ok(StatusResult {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_s: state.started.elapsed().as_secs(),
        sessions: u32::try_from(sessions).unwrap_or(u32::MAX),
        active_runs: u32::try_from(runs).unwrap_or(u32::MAX),
    })
}
