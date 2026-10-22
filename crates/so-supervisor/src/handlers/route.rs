//! `route`: preview a route decision (SPEC 8.3). Reserves nothing and starts nothing; the
//! decision is appended to the caller's session as a `route_decision` event.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use so_core::proto::{INVALID_PARAMS, METHOD_ROUTE, RouteParams, RpcError, UNAVAILABLE};
use so_core::{AccountFile, Event, EventType, new_id};
use so_router::{Catalog, RouteCtx, RouteOutcome, RouterError, Snapshot, route_with};

use crate::dispatch::internal;
use crate::server::SupervisorState;

/// Runs the routing procedure against the supervisor's live state and, when `session_id` is
/// given, appends a `route_decision` event (result, role and fallback reason; no task text).
/// The RPC handler and the auto-spawn path both go through here. Blocking reads (account
/// files, sqlite, the key file) run off the async threads.
pub async fn route_for(
    state: &Arc<SupervisorState>,
    session_id: Option<&str>,
    run_id: Option<&str>,
    params: &RouteParams,
) -> Result<RouteOutcome, RpcError> {
    let role = params.role.clone().unwrap_or_else(|| "worker".into());
    let st = state.clone();
    let (catalog, snapshot) =
        tokio::task::spawn_blocking(move || -> Result<(Catalog, Snapshot), String> {
            let accounts = AccountFile::list(&st.paths).map_err(|e| e.to_string())?;
            let catalog = Catalog::load(&st.paths).map_err(|e| e.to_string())?;
            let snap = Snapshot::collect(
                &st.store,
                &accounts,
                f64::from(st.config.usage.low_pct),
                Utc::now(),
            )
            .map_err(|e| e.to_string())?;
            Ok((catalog, snap))
        })
        .await
        .map_err(internal)?
        .map_err(internal)?;
    let rc = RouteCtx {
        config: &state.config,
        catalog: &catalog,
        snapshot: &snapshot,
        http: &state.http,
        env: so_router::jev::process_env,
    };
    let outcome = route_with(&rc, params).await.map_err(|e| match e {
        RouterError::NoEligible(m) => RpcError::new(UNAVAILABLE, m),
        RouterError::Invalid(m) => RpcError::new(INVALID_PARAMS, m),
        other => internal(other),
    })?;
    if let Some(session_id) = session_id {
        let mut data = serde_json::to_value(&outcome.result).map_err(internal)?;
        data["role"] = json!(role);
        data["fallback_reason"] = json!(outcome.fallback_reason);
        let ev = Event {
            id: new_id(),
            session_id: session_id.to_string(),
            run_id: run_id.map(String::from),
            ts: Utc::now(),
            event_type: EventType::RouteDecision,
            data,
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
