//! Method registry and the built-in `hello` and `status` methods.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use serde::Serialize;
use serde_json::Value;
use so_core::SessionKind;
use so_core::proto::{
    HelloParams, HelloResult, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_HELLO,
    METHOD_NOT_FOUND, METHOD_STATUS, NOT_FOUND, Notification, Request, Response, RpcError,
    StatusResult,
};
use so_store::SessionRow;
use tokio::sync::mpsc;

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
}

/// Per-connection context handed to every handler.
#[derive(Debug, Clone)]
pub struct ConnCtx {
    pub session_id: Option<String>,
    pub run_id: Option<String>,
    pub depth: u8,
    pub notifier: Notifier,
}

impl ConnCtx {
    pub(crate) fn new(notifier: Notifier) -> Self {
        Self {
            session_id: None,
            run_id: None,
            depth: 0,
            notifier,
        }
    }
}

/// Registry `method name -> async handler`. Later tasks register their tools here.
#[derive(Default)]
pub struct Router {
    map: RwLock<HashMap<String, Handler>>,
}

impl Router {
    /// Registers (or replaces) the handler for `method`.
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

/// Every other method, run on a clone of the context.
pub(crate) async fn handle_other(
    state: &Arc<SupervisorState>,
    ctx: ConnCtx,
    req: Request,
) -> Response {
    let r = if req.method == METHOD_HELLO {
        Err(RpcError::new(INVALID_REQUEST, "hello already completed"))
    } else if ctx.session_id.is_none() {
        Err(RpcError::new(INVALID_REQUEST, "hello required"))
    } else if req.method == METHOD_STATUS {
        status(state).await
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
    if ctx.session_id.is_some() {
        return Err(RpcError::new(INVALID_REQUEST, "hello already completed"));
    }
    let p: HelloParams = serde_json::from_value(params)
        .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid hello params: {e}")))?;
    let result = if let Some(run_id) = p.run_id {
        let id = run_id.clone();
        let run = state
            .db(move |s| s.get_run(&id))
            .await?
            .ok_or_else(|| RpcError::new(NOT_FOUND, format!("run not found: {run_id}")))?;
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
    ctx.session_id = Some(result.session_id.clone());
    ctx.run_id = result.run_id.clone();
    ctx.depth = u8::try_from(result.depth).unwrap_or(u8::MAX);
    serde_json::to_value(result).map_err(internal)
}

async fn status(state: &Arc<SupervisorState>) -> Result<Value, RpcError> {
    let (sessions, runs) = state
        .db(|s| {
            Ok((
                s.list_sessions(so_store::SessionFilter::Active, 100_000)?
                    .len(),
                s.list_active_runs(100_000)?.len(),
            ))
        })
        .await?;
    serde_json::to_value(StatusResult {
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_s: state.started.elapsed().as_secs(),
        sessions: u32::try_from(sessions).unwrap_or(u32::MAX),
        active_runs: u32::try_from(runs).unwrap_or(u32::MAX),
    })
    .map_err(internal)
}
