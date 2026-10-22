//! The model catalog and route preview.

use std::time::Duration;

use axum::Json;
use axum::extract::State;
use so_core::proto::{RouteParams, RouteResult};
use so_router::Catalog;
use so_supervisor::handlers::route::route_for;
use so_supervisor::services::models::{ModelsError, last_refresh, refresh_and_save};

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use crate::types::{ModelCatalog, ModelRow};

/// A refresh runs harness binaries and a network fetch; it gets this long.
pub const REFRESH_TIMEOUT: Duration = Duration::from_secs(60);

fn view(
    catalog: &Catalog,
    refreshed_at: Option<chrono::DateTime<chrono::Utc>>,
    report: Option<serde_json::Value>,
) -> ModelCatalog {
    ModelCatalog {
        refreshed_at,
        models: catalog
            .entries()
            .into_iter()
            .map(|(h, m)| ModelRow {
                harness: h,
                id: m.id.clone(),
                tier: m.tier.as_str().to_string(),
                cost: m.cost.as_str().to_string(),
                pool: m.pool.clone(),
                efforts: m.efforts.clone(),
            })
            .collect(),
        report,
    }
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<ModelCatalog>> {
    st.clone()
        .blocking(move || {
            let catalog = Catalog::load(&st.sup.paths).map_err(|e| {
                tracing::warn!("model catalog unreadable: {e}");
                ApiError::internal("the model catalog could not be read")
            })?;
            Ok(Json(view(&catalog, last_refresh(&st.sup.paths), None)))
        })
        .await
}

/// `POST /models/refresh`: one at a time, and bounded.
pub async fn refresh(State(st): State<AppState>) -> ApiResult<Json<ModelCatalog>> {
    let host = st
        .sup
        .host
        .clone()
        .ok_or_else(|| ApiError::unavailable("the host environment (HOME) is not available"))?;
    let _one_at_a_time = st.refresh_lock.lock().await;
    let run = refresh_and_save(&st.sup.paths, &host, &st.sup.http, None);
    let (catalog, report, at) = tokio::time::timeout(REFRESH_TIMEOUT, run)
        .await
        .map_err(|_| ApiError::timeout("the model refresh took too long"))?
        .map_err(|e| match e {
            ModelsError::Config(m) => ApiError::invalid(m.lines().next().unwrap_or_default()),
            ModelsError::Io(m) => {
                tracing::warn!("model refresh failed: {m}");
                ApiError::internal("the model catalog could not be updated")
            }
        })?;
    Ok(Json(view(
        &catalog,
        Some(at),
        serde_json::to_value(&report).ok(),
    )))
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
