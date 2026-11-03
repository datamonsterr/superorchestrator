//! Decider settings and test endpoint.
use crate::{
    error::{ApiError, ApiResult},
    extract::{ApiJson, OptionalJson},
    state::AppState,
};
use axum::{Json, extract::State};
use so_supervisor::services::decider::{
    self, DeciderPatch, DeciderTestRequest, DeciderTestResult, DeciderView,
};

pub async fn get(State(st): State<AppState>) -> ApiResult<Json<DeciderView>> {
    let s = st.clone();
    st.blocking(move || {
        decider::view(&s.sup.paths, &s.sup.config)
            .map(Json)
            .map_err(ApiError::internal)
    })
    .await
}
pub async fn patch(
    State(st): State<AppState>,
    ApiJson(patch): ApiJson<DeciderPatch>,
) -> ApiResult<Json<DeciderView>> {
    let s = st.clone();
    st.blocking(move || {
        decider::set(&s.sup.paths, &s.sup.config, patch)
            .map(Json)
            .map_err(ApiError::invalid)
    })
    .await
}
pub async fn test(
    State(st): State<AppState>,
    OptionalJson(request): OptionalJson<DeciderTestRequest>,
) -> ApiResult<Json<DeciderTestResult>> {
    decider::test_decider(&st.sup.paths, &st.sup.http, request.prompt)
        .await
        .map(Json)
        .map_err(ApiError::internal)
}
