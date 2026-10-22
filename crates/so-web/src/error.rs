//! API errors: `{ "error": { code, message, data? } }` with an HTTP status.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use so_accounts::AccountsError;
use so_core::CoreError;
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND,
    NOT_FOUND, PARSE_ERROR, RpcError, UNAVAILABLE,
};
use so_store::StoreError;

/// A failed request (boxed, so `Result<_, ApiError>` stays small).
#[derive(Debug)]
pub struct ApiError(Box<Inner>);

#[derive(Debug)]
struct Inner {
    status: StatusCode,
    code: Value,
    message: String,
    data: Option<Value>,
    retry_after: Option<u64>,
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    fn named(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self(Box::new(Inner {
            status,
            code: json!(code),
            message: message.into(),
            data: None,
            retry_after: None,
        }))
    }

    pub fn unauthorized() -> Self {
        Self::named(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid token is required",
        )
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::named(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn unsupported_media_type() -> Self {
        Self::named(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "request bodies must be application/json",
        )
    }

    pub fn rate_limited(retry_after: u64) -> Self {
        let mut e = Self::named(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "too many failed attempts; try again later",
        );
        e.0.retry_after = Some(retry_after);
        e
    }

    pub fn route_not_found() -> Self {
        Self::named(StatusCode::NOT_FOUND, "not_found", "no such API route")
    }

    pub fn not_implemented(what: &str) -> Self {
        Self::named(
            StatusCode::NOT_IMPLEMENTED,
            "not_implemented",
            format!("{what} is not available yet"),
        )
    }

    pub fn from_rpc(e: RpcError) -> Self {
        let status = match e.code {
            NOT_FOUND => StatusCode::NOT_FOUND,
            CAP_EXCEEDED | DEPTH_LIMIT => StatusCode::CONFLICT,
            INVALID_PARAMS => StatusCode::UNPROCESSABLE_ENTITY,
            UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE,
            INVALID_REQUEST | PARSE_ERROR => StatusCode::BAD_REQUEST,
            METHOD_NOT_FOUND => StatusCode::NOT_IMPLEMENTED,
            INTERNAL_ERROR => StatusCode::INTERNAL_SERVER_ERROR,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        Self(Box::new(Inner {
            status,
            code: json!(e.code),
            message: e.message,
            data: e.data,
            retry_after: None,
        }))
    }

    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self::from_rpc(RpcError::new(code, message))
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::rpc(NOT_FOUND, message)
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::rpc(INVALID_PARAMS, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::rpc(CAP_EXCEEDED, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::rpc(UNAVAILABLE, message)
    }

    pub fn internal(message: impl std::fmt::Display) -> Self {
        Self::rpc(INTERNAL_ERROR, message.to_string())
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.0.data = Some(data);
        self
    }
}

impl From<RpcError> for ApiError {
    fn from(e: RpcError) -> Self {
        Self::from_rpc(e)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::NotFound { .. } => Self::not_found(e.to_string()),
            StoreError::AmbiguousPrefix { .. } => Self::invalid(e.to_string()),
            other => Self::internal(other),
        }
    }
}

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::InvalidAccountName(_)
            | CoreError::InvalidConfigKey(_)
            | CoreError::InvalidValue { .. }
            | CoreError::InvalidDuration(_)
            | CoreError::UnknownValue { .. }
            | CoreError::UnknownHarness(_) => Self::invalid(e.to_string()),
            other => Self::internal(other),
        }
    }
}

impl From<AccountsError> for ApiError {
    fn from(e: AccountsError) -> Self {
        match e {
            AccountsError::NotFound(_) => Self::not_found(e.to_string()),
            AccountsError::AlreadyExists(_) => Self::conflict(e.to_string()),
            AccountsError::Core(c) => c.into(),
            AccountsError::AgyMultiAccountUnsupported(_)
            | AccountsError::BinaryNotFound(_)
            | AccountsError::HomeMissing(_)
            | AccountsError::InvalidName(_)
            | AccountsError::UnsafePurge { .. } => Self::invalid(e.to_string()),
            other => Self::internal(other),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let i = *self.0;
        let mut error = json!({"code": i.code, "message": i.message});
        if let Some(d) = i.data {
            error["data"] = d;
        }
        let mut res = (i.status, Json(json!({ "error": error }))).into_response();
        if let Some(s) = i.retry_after
            && let Ok(v) = HeaderValue::from_str(&s.to_string())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}
