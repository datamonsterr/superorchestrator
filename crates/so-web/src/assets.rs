//! The embedded frontend (`web/dist`, or a placeholder page when it is not built).

use axum::body::Body;
use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

use crate::error::ApiError;

#[derive(RustEmbed)]
#[folder = "$OUT_DIR/dist"]
struct Dist;

const INDEX: &str = "index.html";

fn file_response(path: &str, cache: &'static str) -> Option<Response> {
    let f = Dist::get(path)?;
    let mime = f.metadata.mimetype().to_string();
    let html = mime.starts_with("text/html");
    let (data, policy) = if html {
        let nonce = match crate::token::random_token() {
            Ok(nonce) => nonce,
            Err(e) => return Some(ApiError::internal(e).into_response()),
        };
        let meta = format!("<meta name=\"so-style-nonce\" content=\"{nonce}\"></head>");
        let data = String::from_utf8_lossy(&f.data)
            .replace("</head>", &meta)
            .into_bytes();
        let policy = crate::security::CSP.replace(
            "style-src-elem 'self'",
            &format!("style-src-elem 'self' 'nonce-{nonce}'"),
        );
        (data, Some(policy))
    } else {
        (f.data.into_owned(), None)
    };
    let mut res = Response::new(Body::from(data));
    let h = res.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if html { "no-store" } else { cache }),
    );
    if let Some(policy) = policy {
        match HeaderValue::from_str(&policy) {
            Ok(value) => {
                h.insert(header::CONTENT_SECURITY_POLICY, value);
            }
            Err(e) => return Some(ApiError::internal(e).into_response()),
        }
    }
    Some(res)
}

/// Serves a file, or `index.html` for any other path without an extension (SPA routes).
/// Unknown `/api` paths get a JSON 404.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path == "api" || path.starts_with("api/") {
        return ApiError::route_not_found().into_response();
    }
    let hashed = path.starts_with("assets/");
    let cache = if hashed {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    if !path.is_empty()
        && let Some(res) = file_response(path, cache)
    {
        return res;
    }
    let last = path.rsplit('/').next().unwrap_or_default();
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    file_response(INDEX, "no-cache").unwrap_or_else(|| StatusCode::NOT_FOUND.into_response())
}
