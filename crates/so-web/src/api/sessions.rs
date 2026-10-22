//! Sessions: list, detail and events.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use so_core::Event;
use so_store::{EventQuery, SessionFilter, SessionRow, Store};
use so_supervisor::SupervisorState;

use super::{redact_event, redact_run};
use crate::error::{ApiError, ApiResult};
use crate::extract::ApiQuery;
use crate::page::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::types::{Page, RunViewPublic, SessionDetail, SessionStatus, SessionSummary};

/// Most sessions one list request scans.
const SCAN: usize = 5000;

pub fn summarize(store: &Store, row: &SessionRow) -> so_store::Result<SessionSummary> {
    let stats = store.session_stats(&row.id)?;
    let status = if row.ended_at.is_none() {
        if stats.escalated > 0 {
            SessionStatus::Escalated
        } else {
            SessionStatus::Active
        }
    } else if row.status == "failed" || stats.failed > 0 {
        SessionStatus::Failed
    } else {
        SessionStatus::Done
    };
    Ok(SessionSummary {
        id: row.id.clone(),
        kind: row.kind,
        client: row.client.clone(),
        cwd: row.cwd.clone(),
        account: row.account.clone(),
        started_at: row.started_at,
        ended_at: row.ended_at,
        status,
        run_count: u32::try_from(stats.runs).unwrap_or(u32::MAX),
        cost: stats.cost,
    })
}

/// Summary of one session by id (also used by the SSE `session` message).
pub async fn summary_of(sup: &Arc<SupervisorState>, id: &str) -> ApiResult<SessionSummary> {
    let id = id.to_string();
    let store = sup.store.clone();
    tokio::task::spawn_blocking(move || {
        let row = store
            .get_session(&id)?
            .ok_or_else(|| ApiError::not_found(format!("session not found: {id}")))?;
        Ok(summarize(&store, &row)?)
    })
    .await
    .map_err(ApiError::internal)?
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    status: Option<String>,
    q: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

pub async fn list(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<ListQuery>,
) -> ApiResult<Json<Page<SessionSummary>>> {
    let limit = clamp_limit(q.limit);
    let status = match q.status.as_deref() {
        None | Some("") => None,
        Some("active") => Some(SessionStatus::Active),
        Some("escalated") => Some(SessionStatus::Escalated),
        Some("failed") => Some(SessionStatus::Failed),
        Some(other) => return Err(ApiError::invalid(format!("unknown status `{other}`"))),
    };
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    let needle = q.q.map(|s| s.to_lowercase()).filter(|s| !s.is_empty());
    st.db(move |s| {
        let mut items = Vec::new();
        let mut next = None;
        for row in s.list_sessions(SessionFilter::All, SCAN)? {
            if let Some((ts, id)) = &cursor
                && (row.started_at, &row.id) >= (*ts, id)
            {
                continue;
            }
            if let Some(n) = &needle
                && !matches_query(s, &row, n)?
            {
                continue;
            }
            let sum = summarize(s, &row)?;
            if status.is_some_and(|w| w != sum.status) {
                continue;
            }
            if items.len() == limit {
                let last: &SessionSummary = &items[limit - 1];
                next = Some(encode_cursor(&last.started_at, &last.id));
                break;
            }
            items.push(sum);
        }
        Ok(Page {
            items,
            next_cursor: next,
        })
    })
    .await
    .map(Json)
}

/// Substring match on client, cwd, run names and run ids.
fn matches_query(s: &Store, row: &SessionRow, needle: &str) -> so_store::Result<bool> {
    let has = |t: &str| t.to_lowercase().contains(needle);
    if has(&row.client) || row.cwd.as_deref().is_some_and(has) || has(&row.id) {
        return Ok(true);
    }
    Ok(s.list_runs_by_session(&row.id, 10_000)?
        .iter()
        .any(|r| has(&r.id) || r.name.as_deref().is_some_and(has)))
}

pub async fn detail(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<SessionDetail>> {
    let summary = summary_of(&st.sup, &id).await?;
    let store = st.sup.store.clone();
    let sid = id.clone();
    let rows = st
        .blocking(move || Ok(store.list_runs_by_session(&sid, 100_000)?))
        .await?;
    let mut runs = Vec::with_capacity(rows.len());
    for row in rows {
        let mut v = RunViewPublic::from(st.sup.runs.run_view(row).await?);
        redact_run(&mut v);
        runs.push(v);
    }
    Ok(Json(SessionDetail { summary, runs }))
}

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    after: Option<String>,
    limit: Option<usize>,
    r#type: Option<String>,
}

/// Runs `q` for one page and builds the next cursor.
pub async fn events_page(
    st: &AppState,
    mut q: EventQuery,
    params: &EventsQuery,
) -> ApiResult<Page<Event>> {
    let limit = clamp_limit(params.limit);
    q.limit = limit + 1;
    q.after = params.after.as_deref().map(decode_cursor).transpose()?;
    if let Some(t) = &params.r#type {
        q.event_type = Some(
            t.parse()
                .map_err(|_| ApiError::invalid(format!("unknown event type `{t}`")))?,
        );
    }
    let mut items = st.db(move |s| s.query_events(&q)).await?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|e| encode_cursor(&e.ts, &e.id))
    } else {
        None
    };
    items.iter_mut().for_each(redact_event);
    Ok(Page { items, next_cursor })
}

pub async fn events(
    State(st): State<AppState>,
    Path(id): Path<String>,
    ApiQuery(params): ApiQuery<EventsQuery>,
) -> ApiResult<Json<Page<Event>>> {
    summary_of(&st.sup, &id).await?;
    let q = EventQuery {
        session_id: Some(id),
        ..Default::default()
    };
    Ok(Json(events_page(&st, q, &params).await?))
}
