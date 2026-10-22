//! The model catalog, route preview and saved workflows.

use axum::Json;
use axum::extract::State;
use chrono::{DateTime, Utc};
use so_core::proto::{RouteParams, RouteResult};
use so_router::Catalog;
use so_supervisor::handlers::route::route_for;
use so_supervisor::services::models::refresh_and_save;
use so_workflow::SavedWorkflows;

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use crate::types::{ModelCatalog, ModelRow, WorkflowSummary};

fn view(catalog: &Catalog, st: &AppState, report: Option<serde_json::Value>) -> ModelCatalog {
    let s = |v: &dyn erased::Named| v.name();
    let models = catalog
        .entries()
        .into_iter()
        .map(|(h, m)| ModelRow {
            harness: h,
            id: m.id.clone(),
            tier: s(&m.tier),
            cost: s(&m.cost),
            pool: m.pool.clone(),
            efforts: m.efforts.clone(),
        })
        .collect();
    let refreshed_at = std::fs::metadata(st.sup.paths.models_toml())
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from);
    ModelCatalog {
        refreshed_at,
        models,
        report,
    }
}

/// Wire name of a serde enum value.
mod erased {
    pub trait Named {
        fn name(&self) -> String;
    }
    impl<T: serde::Serialize> Named for T {
        fn name(&self) -> String {
            serde_json::to_value(self)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default()
        }
    }
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<ModelCatalog>> {
    let s = st.clone();
    st.clone()
        .blocking(move || {
            let catalog = Catalog::load(&s.sup.paths).map_err(ApiError::internal)?;
            Ok(Json(view(&catalog, &s, None)))
        })
        .await
}

pub async fn refresh(State(st): State<AppState>) -> ApiResult<Json<ModelCatalog>> {
    let host = st
        .sup
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("the host environment (HOME) is not available"))?;
    // Config is read fresh: the user may have edited choices since the supervisor started.
    let config = so_core::Config::load(&st.sup.paths)?;
    let (catalog, report) = refresh_and_save(&st.sup.paths, &config, &host, &st.sup.http)
        .await
        .map_err(ApiError::internal)?;
    let report = serde_json::to_value(&report).ok();
    let s = st.clone();
    st.blocking(move || Ok(Json(view(&catalog, &s, report))))
        .await
}

/// `POST /route`: a preview. Nothing is reserved or started, and no event is recorded.
pub async fn route(
    State(st): State<AppState>,
    ApiJson(p): ApiJson<RouteParams>,
) -> ApiResult<Json<RouteResult>> {
    let mut out = route_for(&st.sup, None, None, &p).await?.result;
    out.rationale = so_core::redact::redact(&out.rationale);
    Ok(Json(out))
}

pub async fn workflows(State(st): State<AppState>) -> ApiResult<Json<Vec<WorkflowSummary>>> {
    st.clone()
        .blocking(move || {
            let cwd = std::env::current_dir().unwrap_or_else(|_| "/".into());
            let saved = SavedWorkflows::new(st.sup.paths.workflows_dir(), cwd).list();
            Ok(Json(
                saved
                    .into_iter()
                    .map(|w| WorkflowSummary {
                        name: w.name,
                        path: w.path.display().to_string(),
                        source: match w.source {
                            so_workflow::SavedSource::Project => "project",
                            so_workflow::SavedSource::Config => "config",
                        }
                        .to_string(),
                    })
                    .collect(),
            ))
        })
        .await
}
