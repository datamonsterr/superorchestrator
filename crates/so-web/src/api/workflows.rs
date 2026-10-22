//! Saved workflows.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Deserialize;
use so_core::proto::{RunWorkflowResult, WorkflowRunView, WorkflowStatusResult};
use so_core::redact::{redact, redact_json};
use so_workflow::{SavedSource, SavedWorkflows};

use crate::error::ApiResult;
use crate::extract::{ApiQuery, OptionalJson};
use crate::page::{clamp_limit, decode_cursor, encode_cursor};
use crate::state::AppState;
use crate::types::{Page, RunWorkflowBody, WorkflowSummary};

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<Vec<WorkflowSummary>>> {
    st.clone()
        .blocking(move || {
            let saved = SavedWorkflows::new(st.sup.paths.workflows_dir(), st.project_cwd()).list();
            Ok(Json(
                saved
                    .into_iter()
                    .map(|w| WorkflowSummary {
                        name: w.name,
                        path: w.path.display().to_string(),
                        source: match w.source {
                            SavedSource::Project => "project",
                            SavedSource::Config => "config",
                        }
                        .to_string(),
                    })
                    .collect(),
            ))
        })
        .await
}

#[derive(Debug, Deserialize)]
pub struct RunsQuery {
    limit: Option<usize>,
    cursor: Option<String>,
}

/// Workflow runs, newest first.
pub async fn runs(
    State(st): State<AppState>,
    ApiQuery(q): ApiQuery<RunsQuery>,
) -> ApiResult<Json<Page<WorkflowRunView>>> {
    let limit = clamp_limit(q.limit);
    let cursor = q.cursor.as_deref().map(decode_cursor).transpose()?;
    // One more than a page, to know whether there is a next one; the position is in SQL.
    let mut items: Vec<WorkflowRunView> = st.sup.runs.list_workflow_runs(cursor, limit + 1).await?;
    let next_cursor = (items.len() > limit).then(|| {
        items.truncate(limit);
        items
            .last()
            .map(|r| encode_cursor(&r.started_at, &r.id))
            .unwrap_or_default()
    });
    Ok(Json(Page { items, next_cursor }))
}

/// One workflow run with its steps and result; `id` may be a unique prefix.
pub async fn run_detail(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<WorkflowStatusResult>> {
    let mut s = st.sup.runs.workflow_status(&id).await?;
    if let Some(r) = s.result.as_mut() {
        redact_json(r);
    }
    s.error = s.error.map(|e| redact(&e));
    Ok(Json(s))
}

/// Starts a saved workflow in a session of its own and returns at once; follow it through
/// `GET /workflows/runs/:id` and the `workflow` stream message.
pub async fn run(
    State(st): State<AppState>,
    Path(name): Path<String>,
    OptionalJson(body): OptionalJson<RunWorkflowBody>,
) -> ApiResult<(StatusCode, Json<RunWorkflowResult>)> {
    let started = st
        .sup
        .runs
        .run_workflow_ui(st.project_cwd(), &name, body.args)
        .await?;
    Ok((StatusCode::ACCEPTED, Json(started)))
}

/// Stops a running workflow and its steps (an extension: SPEC 13 has no such tool).
pub async fn cancel(
    State(st): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<so_core::proto::AckResult>> {
    Ok(Json(st.sup.runs.cancel_workflow(&id).await?))
}
