//! `spawn_agent`, `wait_agent`, `get_run`, `cancel_agent` and `join_run`.

use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde_json::Value;
use so_core::RunState;
use so_core::proto::{
    EmptyParams, INVALID_PARAMS, METHOD_ACCEPT_RUN, METHOD_ASK_PARENT, METHOD_CANCEL_AGENT,
    METHOD_FORK_AGENT, METHOD_GET_RUN, METHOD_JOIN_RUN, METHOD_READ_MESSAGES, METHOD_REJECT_RUN,
    METHOD_REPORT_PROGRESS, METHOD_SEND_MESSAGE, METHOD_SPAWN_AGENT, METHOD_WAIT_AGENT, RpcError,
    RunRef, SpawnParams, SpawnResult, WaitParams, WaitResult,
};

use super::view::WaitMode;
use super::{RunManager, WaitOutcome};
use crate::dispatch::{ConnCtx, internal};
use crate::server::SupervisorState;

/// `wait_agent` without `timeout_ms`.
const DEFAULT_WAIT: Duration = Duration::from_secs(600);

fn params<T: DeserializeOwned>(v: Value) -> Result<T, RpcError> {
    serde_json::from_value(v)
        .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid params: {e}")))
}

fn json<T: serde::Serialize>(v: T) -> Result<Value, RpcError> {
    serde_json::to_value(v).map_err(internal)
}

pub(super) fn register(state: &Arc<SupervisorState>) {
    let runs = state.runs.clone();

    let m = runs.clone();
    state.router.register(METHOD_SPAWN_AGENT, move |ctx, p| {
        let m = m.clone();
        async move { spawn_agent(&m, ctx, params(p)?).await }
    });
    let m = runs.clone();
    state.router.register(METHOD_WAIT_AGENT, move |ctx, p| {
        let m = m.clone();
        async move { wait_agent(&m, ctx, params(p)?).await }
    });
    let m = runs.clone();
    state.router.register(METHOD_GET_RUN, move |_ctx, p| {
        let m = m.clone();
        async move {
            let r: RunRef = params(p)?;
            let row = m.resolve_run(&r.run).await?;
            json(m.run_view(row).await?)
        }
    });
    let m = runs.clone();
    state.router.register(METHOD_CANCEL_AGENT, move |_ctx, p| {
        let m = m.clone();
        async move {
            let r: RunRef = params(p)?;
            let row = m.resolve_run(&r.run).await?;
            m.cancel(&row.id).await?;
            let row = m.resolve_run(&row.id).await?;
            json(m.run_view(row).await?)
        }
    });
    let m = runs.clone();
    state.router.register(METHOD_JOIN_RUN, move |_ctx, p| {
        let m = m.clone();
        async move {
            let r: RunRef = params(p)?;
            let row = m.resolve_run(&r.run).await?;
            json(m.join_run(row).await?)
        }
    });
    let m = runs.clone();
    state.router.register(METHOD_SEND_MESSAGE, move |ctx, p| {
        let m = m.clone();
        async move { json(m.send_message(&ctx, params(p)?).await?) }
    });
    let m = runs.clone();
    state.router.register(METHOD_READ_MESSAGES, move |ctx, p| {
        let m = m.clone();
        async move {
            let _: EmptyParams = params(p)?;
            json(m.read_messages(&ctx).await?)
        }
    });
    let m = runs.clone();
    state
        .router
        .register(METHOD_REPORT_PROGRESS, move |ctx, p| {
            let m = m.clone();
            async move { json(m.report_progress(&ctx, params(p)?).await?) }
        });
    let m = runs.clone();
    state.router.register(METHOD_ASK_PARENT, move |ctx, p| {
        let m = m.clone();
        async move { json(m.ask_parent(&ctx, params(p)?).await?) }
    });
    let m = runs.clone();
    state.router.register(METHOD_ACCEPT_RUN, move |ctx, p| {
        let m = m.clone();
        async move { json(m.accept_run(&ctx, params(p)?).await?) }
    });
    let m = runs.clone();
    state.router.register(METHOD_REJECT_RUN, move |ctx, p| {
        let m = m.clone();
        async move { json(m.reject_run(&ctx, params(p)?).await?) }
    });
    let m = runs;
    state.router.register(METHOD_FORK_AGENT, move |ctx, p| {
        let m = m.clone();
        async move { json(m.fork_run(&ctx, params(p)?).await?) }
    });
}

async fn spawn_agent(m: &RunManager, ctx: ConnCtx, p: SpawnParams) -> Result<Value, RpcError> {
    let wait = p.wait;
    // Subscribe first, so no event of the new run is missed.
    let mut sub = ctx
        .session_id
        .as_ref()
        .filter(|_| wait)
        .map(|s| m.subscribe(std::slice::from_ref(s), None));
    let (run_id, warnings) = m.spawn(&ctx, p).await?;
    if let Some(sub) = sub.as_mut() {
        m.wait_runs(
            &ctx,
            std::slice::from_ref(&run_id),
            sub,
            None,
            WaitMode::Finish,
        )
        .await?;
    }
    let row = m.resolve_run(&run_id).await?;
    let finished = wait && (row.state.is_terminal() || row.state == RunState::PendingReview);
    json(SpawnResult {
        run_id,
        state: row.state,
        result: finished.then(|| RunManager::result_of(&row)).flatten(),
        warnings,
        error_kind: finished.then_some(row.error_kind).flatten(),
    })
}

async fn wait_agent(m: &RunManager, ctx: ConnCtx, p: WaitParams) -> Result<Value, RpcError> {
    if p.runs.is_empty() {
        return Err(RpcError::new(INVALID_PARAMS, "runs must not be empty"));
    }
    let mut ids = Vec::new();
    let mut sessions: Vec<String> = Vec::new();
    for r in &p.runs {
        let row = m.resolve_run(r).await?;
        if !sessions.contains(&row.session_id) {
            sessions.push(row.session_id.clone());
        }
        ids.push(row.id);
    }
    let mut sub = m.subscribe(&sessions, Some(&ids));
    let timeout = p.timeout_ms.map_or(DEFAULT_WAIT, Duration::from_millis);
    let WaitOutcome {
        finished,
        updates,
        timed_out,
    } = {
        // A run that blocks on its own children is `waiting`, and is not stall-checked.
        let guard = match &ctx.run_id {
            Some(id) => Some(m.begin_wait(id, false).await),
            None => None,
        };
        let r = m
            .wait_runs(&ctx, &ids, &mut sub, Some(timeout), WaitMode::Update)
            .await;
        if let Some(g) = guard {
            g.end().await;
        }
        r?
    };
    json(WaitResult {
        finished,
        updates,
        timed_out,
    })
}
