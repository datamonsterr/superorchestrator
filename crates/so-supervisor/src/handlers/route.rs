//! `route`: preview a route decision (SPEC 8.3). Reserves nothing and starts nothing; the
//! decision is appended to the caller's session as a `route_decision` event.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use so_core::proto::{INVALID_PARAMS, METHOD_ROUTE, RouteParams, RpcError, UNAVAILABLE};
use so_core::{AccountFile, Config, Event, EventType, Paths, new_id};
use so_router::{Catalog, RouteCtx, RouteOutcome, RouterError, Snapshot, route_with};
use so_store::Store;

use crate::dispatch::internal;
use crate::server::SupervisorState;

/// `RpcError::data["reason"]` of the error raised when no choice has an account to run on.
pub const NO_ELIGIBLE: &str = "no_eligible";

/// What `route` reads: the supervisor's live state, without the supervisor itself, so the
/// run manager can route a spawn or a failover.
pub struct RouteEnv<'a> {
    pub store: &'a Store,
    pub config: &'a Config,
    pub paths: &'a Paths,
    pub http: &'a reqwest::Client,
    /// Treat every account as idle, so a choice whose accounts are all at their cap can
    /// still be picked: a spawn then waits in the queue (SPEC 7.7) instead of failing.
    pub ignore_caps: bool,
}

/// Runs the routing procedure (SPEC 8.3) and returns the outcome without recording it.
/// Blocking reads (account files, sqlite, the key file) run off the async threads.
pub async fn route_decide(
    env: &RouteEnv<'_>,
    params: &RouteParams,
) -> Result<RouteOutcome, RpcError> {
    let (store, config, paths) = (env.store.clone(), env.config.clone(), env.paths.clone());
    let ignore_caps = env.ignore_caps;
    let (catalog, snapshot) =
        tokio::task::spawn_blocking(move || -> Result<(Catalog, Snapshot), String> {
            let accounts = AccountFile::list(&paths).map_err(|e| e.to_string())?;
            let catalog = Catalog::load(&paths).map_err(|e| e.to_string())?;
            let mut snap = Snapshot::collect(
                &store,
                &accounts,
                f64::from(config.usage.low_pct),
                Utc::now(),
            )
            .map_err(|e| e.to_string())?;
            if ignore_caps {
                snap.accounts.iter_mut().for_each(|a| a.active = 0);
            }
            Ok((catalog, snap))
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
    let rc = RouteCtx {
        config: env.config,
        catalog: &catalog,
        snapshot: &snapshot,
        http: env.http,
        env: so_router::jev::process_env,
    };
    route_with(&rc, params).await.map_err(|e| match e {
        RouterError::NoEligible(m) => RpcError {
            code: UNAVAILABLE,
            message: m,
            data: Some(json!({"reason": NO_ELIGIBLE})),
        },
        RouterError::Invalid(m) => RpcError::new(INVALID_PARAMS, m),
        other => internal(other),
    })
}

/// The `route_decision` event data: the result (the shape `GET /runs/:id/route` and the
/// routing log parse), the role and the fallback reason. No task text.
pub fn decision_data(outcome: &RouteOutcome, role: &str) -> Result<Value, RpcError> {
    let mut data = serde_json::to_value(&outcome.result).map_err(internal)?;
    data["role"] = json!(role);
    data["fallback_reason"] = json!(outcome.fallback_reason);
    Ok(data)
}

/// Runs the routing procedure against the supervisor's live state and, when `session_id` is
/// given, appends a `route_decision` event. The RPC handler goes through here.
pub async fn route_for(
    state: &Arc<SupervisorState>,
    session_id: Option<&str>,
    run_id: Option<&str>,
    params: &RouteParams,
) -> Result<RouteOutcome, RpcError> {
    let role = params.role.clone().unwrap_or_else(|| "worker".into());
    let env = RouteEnv {
        store: &state.store,
        config: &state.config,
        paths: &state.paths,
        http: &state.http,
        ignore_caps: false,
    };
    let outcome = route_decide(&env, params).await?;
    if let Some(session_id) = session_id {
        let ev = Event {
            id: new_id(),
            session_id: session_id.to_string(),
            run_id: run_id.map(String::from),
            ts: Utc::now(),
            event_type: EventType::RouteDecision,
            data: decision_data(&outcome, &role)?,
        };
        state.db(move |s| s.append_event(&ev).map(|_| ())).await?;
    }
    Ok(outcome)
}

pub(super) fn register_route(state: &Arc<SupervisorState>) {
    let state = state.clone();
    state
        .clone()
        .router
        .register(METHOD_ROUTE, move |ctx, params| {
            let state = state.clone();
            async move {
                let params: RouteParams = serde_json::from_value(params)
                    .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid params: {e}")))?;
                let out = route_for(
                    &state,
                    ctx.session_id.as_deref(),
                    ctx.run_id.as_deref(),
                    &params,
                )
                .await?;
                serde_json::to_value(&out.result).map_err(internal)
            }
        });
}
