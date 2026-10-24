//! API errors: `{ "error": { code, message, data? } }` with an HTTP status.

use axum::Json;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use so_accounts::AccountsError;
use so_core::CoreError;
use so_core::proto::{
    CAP_EXCEEDED, DECIDER_UNREACHABLE, DEPTH_LIMIT, INTERNAL_ERROR, INVALID_PARAMS,
    INVALID_REQUEST, METHOD_NOT_FOUND, NOT_FOUND, PARSE_ERROR, RpcError, TIMEOUT, UNAVAILABLE,
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
    clear_cookie: bool,
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
            clear_cookie: false,
        }))
    }

    pub fn unauthorized() -> Self {
        Self::named(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid token is required",
        )
    }

    /// A 401 that also expires the session cookie (it no longer matches the token).
    pub fn unauthorized_clearing_cookie() -> Self {
        let mut e = Self::unauthorized();
        e.0.clear_cookie = true;
        e
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

    pub fn from_rpc(e: RpcError) -> Self {
        let status = match e.code {
            NOT_FOUND => StatusCode::NOT_FOUND,
            CAP_EXCEEDED | DEPTH_LIMIT => StatusCode::CONFLICT,
            INVALID_PARAMS => StatusCode::UNPROCESSABLE_ENTITY,
            UNAVAILABLE => StatusCode::SERVICE_UNAVAILABLE,
            TIMEOUT => StatusCode::GATEWAY_TIMEOUT,
            DECIDER_UNREACHABLE => StatusCode::BAD_GATEWAY,
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
            clear_cookie: false,
        }))
    }

    pub fn rpc(code: i64, message: impl Into<String>) -> Self {
        Self::from_rpc(RpcError::new(code, message))
    }

    /// A 409 for an action the run's current state does not allow.
    pub fn state_conflict(message: impl Into<String>) -> Self {
        let mut e = Self::rpc(so_core::proto::INVALID_REQUEST, message);
        e.0.status = StatusCode::CONFLICT;
        e
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

    pub fn timeout(message: impl Into<String>) -> Self {
        Self::rpc(TIMEOUT, message)
    }

    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::rpc(UNAVAILABLE, message)
    }

    /// A 500. Only the first line of the message is sent: parse errors quote the file
    /// they came from on the following lines.
    pub fn internal(message: impl std::fmt::Display) -> Self {
        let m = message.to_string();
        tracing::warn!("internal error: {m}");
        Self::rpc(INTERNAL_ERROR, m.lines().next().unwrap_or_default())
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
            StoreError::Io(ref source) => Self::io(std::path::Path::new("state.db"), source),
            other => Self::internal(other),
        }
    }
}

impl ApiError {
    /// A filesystem failure: the path stays in the log, not in the response.
    fn io(path: &std::path::Path, source: &std::io::Error) -> Self {
        tracing::warn!(path = %path.display(), "filesystem error: {source}");
        Self::rpc(INTERNAL_ERROR, "a filesystem operation failed")
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
            CoreError::Io { path, source } => Self::io(&path, &source),
            other => Self::internal(other),
        }
    }
}

impl From<so_agents::AgentError> for ApiError {
    fn from(e: so_agents::AgentError) -> Self {
        use so_agents::AgentError as E;
        match e {
            E::NotFound(_) => Self::not_found(e.to_string()),
            // Parse errors quote the file on later lines; only the first goes out.
            E::Parse { .. } | E::InvalidName(_) => {
                Self::invalid(e.to_string().lines().next().unwrap_or_default())
            }
            E::Io { path, source } => Self::io(&path, &source),
        }
    }
}

impl From<AccountsError> for ApiError {
    fn from(e: AccountsError) -> Self {
        match e {
            AccountsError::NotFound(_) => Self::not_found(e.to_string()),
            AccountsError::AlreadyExists(_) => Self::conflict(e.to_string()),
            AccountsError::Core(c) => c.into(),
            AccountsError::Io { path, source } => Self::io(&path, &source),
            AccountsError::AgyMultiAccountUnsupported(_)
            | AccountsError::AgySecondAccount { .. }
            | AccountsError::BinaryNotFound(_)
            | AccountsError::HomeMissing(_)
            | AccountsError::InvalidName(_)
            | AccountsError::OverlappingHome { .. }
            | AccountsError::UnsafePurge { .. }
            | AccountsError::ShadowsCommand { .. }
            | AccountsError::WrapperIsSelf { .. } => Self::invalid(e.to_string()),
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
        if i.clear_cookie {
            res.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_static("so_token=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"),
            );
        }
        if let Some(s) = i.retry_after
            && let Ok(v) = HeaderValue::from_str(&s.to_string())
        {
            res.headers_mut().insert(header::RETRY_AFTER, v);
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unreachable_decider_is_a_502_with_its_message() {
        let e = ApiError::rpc(
            so_core::proto::DECIDER_UNREACHABLE,
            "decider unreachable: x",
        );
        assert_eq!(e.0.status, StatusCode::BAD_GATEWAY);
        assert_eq!(e.0.message, "decider unreachable: x");
    }

    #[test]
    fn filesystem_errors_do_not_carry_paths() {
        let io = || std::io::Error::other("boom");
        let errs = [
            ApiError::from(so_agents::AgentError::Io {
                path: "/secret/dir/AGENT.md".into(),
                source: io(),
            }),
            ApiError::from(CoreError::io("/secret/dir/config.toml", io())),
            ApiError::from(AccountsError::io("/secret/dir/acc.toml", io())),
        ];
        for e in errs {
            assert_eq!(e.0.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert!(!format!("{e:?}").contains("secret"), "{e:?}");
        }
    }

    #[test]
    fn timeouts_are_504() {
        assert_eq!(
            ApiError::timeout("slow").0.status,
            StatusCode::GATEWAY_TIMEOUT
        );
    }
}
