//! Decider settings and test endpoint.
use crate::{
    error::{ApiError, ApiResult},
    extract::OptionalJson,
    state::AppState,
};
use axum::{Json, body::Bytes, extract::State};
use so_supervisor::services::decider::{
    self, DeciderError, DeciderPatch, DeciderTestRequest, DeciderTestResult, DeciderView,
};

fn api_error(error: DeciderError) -> ApiError {
    match error {
        DeciderError::Invalid(message) => ApiError::invalid(message),
        DeciderError::Internal(message) => ApiError::internal(message),
    }
}

/// `GET /api/v1/decider`: the stored settings and what is effective now.
pub async fn get(State(st): State<AppState>) -> ApiResult<Json<DeciderView>> {
    let s = st.clone();
    st.blocking(move || {
        decider::view(&s.sup.paths, &s.sup.config)
            .map(Json)
            .map_err(api_error)
    })
    .await
}

/// `PATCH /api/v1/decider`: validates and stores a partial update. The body is parsed by
/// [`DeciderPatch::from_value`] so no error ever echoes a submitted value.
pub async fn patch(State(st): State<AppState>, body: Bytes) -> ApiResult<Json<DeciderView>> {
    let value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::invalid("request body is not valid JSON"))?;
    let patch = DeciderPatch::from_value(value).map_err(api_error)?;
    let s = st.clone();
    st.blocking(move || {
        decider::set(&s.sup.paths, &s.sup.config, patch)
            .map(Json)
            .map_err(api_error)
    })
    .await
}

/// `POST /api/v1/decider/test`: one live request to the decider with the stored settings.
pub async fn test(
    State(st): State<AppState>,
    OptionalJson(request): OptionalJson<DeciderTestRequest>,
) -> ApiResult<Json<DeciderTestResult>> {
    decider::test_decider(&st.sup.paths, &st.sup.http, request.prompt)
        .await
        .map(Json)
        .map_err(api_error)
}
