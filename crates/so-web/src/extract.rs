//! JSON body and query extractors that fail with the API's error body.

use axum::extract::{FromRequest, FromRequestParts, Query, Request};
use axum::http::request::Parts;
use serde::de::DeserializeOwned;

use crate::error::ApiError;

/// `Json<T>` whose rejection is a 422 `ApiError`.
pub struct ApiJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        axum::Json::<T>::from_request(req, state)
            .await
            .map(|j| Self(j.0))
            .map_err(|e| ApiError::invalid(e.body_text()))
    }
}

/// Like [`ApiJson`], but an empty body is the default value (for optional bodies).
pub struct OptionalJson<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Default> FromRequest<S> for OptionalJson<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let (parts, body) = req.into_parts();
        let bytes = axum::body::to_bytes(body, 1024 * 1024)
            .await
            .map_err(|e| ApiError::invalid(e.to_string()))?;
        if bytes.iter().all(u8::is_ascii_whitespace) {
            return Ok(Self(T::default()));
        }
        let req = Request::from_parts(parts, axum::body::Body::from(bytes));
        ApiJson::<T>::from_request(req, state)
            .await
            .map(|j| Self(j.0))
    }
}

/// `Query<T>` whose rejection is a 422 `ApiError`.
pub struct ApiQuery<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequestParts<S> for ApiQuery<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        Query::<T>::from_request_parts(parts, state)
            .await
            .map(|q| Self(q.0))
            .map_err(|e| ApiError::invalid(e.body_text()))
    }
}
