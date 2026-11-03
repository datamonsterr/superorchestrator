#![allow(clippy::unwrap_used)]
use axum::{
    Json, Router,
    extract::{OriginalUri, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde_json::{Value, json};
use so_core::ErrorKind;
use so_harness::{
    ClaudeDriver, Driver, classify_common,
    endpoint::{classify_endpoint, validate},
};

#[test]
fn classification_table() {
    for (codes, kind) in [
        (
            &[1308, 1309, 1310, 1316, 1317, 1318, 1319, 1320, 1321, 2056][..],
            ErrorKind::Quota,
        ),
        (&[1302, 1305, 1312, 1002][..], ErrorKind::RateLimit),
        (&[1000, 1001, 1003, 2049][..], ErrorKind::Auth),
    ] {
        for code in codes {
            assert_eq!(
                classify_endpoint(&format!("{{\"error\":{{\"code\":\"{code}\"}}}}")),
                Some(kind)
            );
        }
    }
    for (text, kind) in [
        ("limit exhausted", ErrorKind::Quota),
        ("5-hour usage limit", ErrorKind::Quota),
        ("weekly (7-day) usage limit", ErrorKind::Quota),
        ("monthly usage limit", ErrorKind::Quota),
        ("engine is currently overloaded", ErrorKind::RateLimit),
        ("too many requests", ErrorKind::RateLimit),
        ("concurrent request limit", ErrorKind::RateLimit),
        ("membership benefits", ErrorKind::Auth),
        ("code: 1308", ErrorKind::Quota),
        ("error 1002", ErrorKind::RateLimit),
    ] {
        assert_eq!(classify_endpoint(text), Some(kind), "{text}");
    }
    for text in [
        "113080",
        "2026-11-03 13:08:00",
        "port 1002 in an unrelated sentence",
        "code: 113080",
        "error 10020",
    ] {
        assert_eq!(classify_endpoint(text), None, "{text}");
        assert_eq!(
            ClaudeDriver::default().classify_error(text),
            classify_common(text)
        );
    }
}

async fn server(status: StatusCode, body: &'static str, expected_header: &'static str) -> String {
    let app = Router::new().route("/v1/messages", post(|State((status, body, expected_header)): State<(StatusCode, &'static str, &'static str)>, uri: OriginalUri, headers: HeaderMap, Json(data): Json<Value>| async move {
        assert_eq!(uri.path(), "/v1/messages");
        assert_eq!(headers["anthropic-version"], "2023-06-01");
        if expected_header == "key" { assert_eq!(headers["x-api-key"], "secret-token"); } else { assert_eq!(headers["authorization"], "Bearer secret-token"); }
        assert_eq!(data, json!({"model":"test-model","max_tokens":1,"messages":[{"role":"user","content":"ping"}]}));
        (status, body)
    })).with_state((status, body, expected_header));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}

#[tokio::test]
async fn validation_requests_and_errors() {
    for (var, header, slash) in [
        ("ANTHROPIC_API_KEY", "key", ""),
        ("ANTHROPIC_AUTH_TOKEN", "bearer", "/"),
    ] {
        let url = server(StatusCode::OK, "ok", header).await;
        validate(&format!("{url}{slash}"), var, "secret-token", "test-model")
            .await
            .unwrap();
    }
    for (status, body, kind) in [
        (StatusCode::UNAUTHORIZED, "secret-token", ErrorKind::Auth),
        (StatusCode::FORBIDDEN, "forbidden", ErrorKind::Auth),
        (StatusCode::TOO_MANY_REQUESTS, "busy", ErrorKind::RateLimit),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "{\"error\":{\"code\":\"1308\",\"message\":\"Usage limit reached for 5 hour\"}}",
            ErrorKind::Quota,
        ),
    ] {
        let url = server(status, body, "key").await;
        let err = validate(&url, "ANTHROPIC_API_KEY", "secret-token", "test-model")
            .await
            .unwrap_err();
        assert_eq!(err.kind, kind);
        assert_eq!(err.status, Some(status.as_u16()));
        assert!(!err.detail.contains("secret-token"));
        assert!(!format!("{err:?}").contains("secret-token"));
    }
}

#[tokio::test]
async fn transport_errors_do_not_expose_credentials() {
    let error = validate(
        "invalid url containing secret-token",
        "ANTHROPIC_API_KEY",
        "secret-token",
        "test-model",
    )
    .await
    .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Other);
    assert_eq!(error.status, None);
    assert!(!format!("{error:?}").contains("secret-token"));
}

#[tokio::test]
#[ignore]
async fn real_endpoint_smoke() {
    let vars = [
        "SO_SMOKE_ENDPOINT_URL",
        "SO_SMOKE_ENDPOINT_TOKEN_VAR",
        "SO_SMOKE_ENDPOINT_TOKEN",
        "SO_SMOKE_ENDPOINT_MODEL",
    ]
    .map(std::env::var);
    let [Ok(url), Ok(var), Ok(token), Ok(model)] = vars else {
        eprintln!("Skipping endpoint smoke: required environment variables are unset");
        return;
    };
    validate(&url, &var, &token, &model).await.unwrap();
}
