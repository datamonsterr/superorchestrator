//! Classification and credential validation for Anthropic-compatible endpoints.
use std::time::Duration;

use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use so_core::ErrorKind;

const CODES: &[(&str, ErrorKind)] = &[
    ("1308", ErrorKind::Quota),
    ("1309", ErrorKind::Quota),
    ("1310", ErrorKind::Quota),
    ("1316", ErrorKind::Quota),
    ("1317", ErrorKind::Quota),
    ("1318", ErrorKind::Quota),
    ("1319", ErrorKind::Quota),
    ("1320", ErrorKind::Quota),
    ("1321", ErrorKind::Quota),
    ("2056", ErrorKind::Quota),
    ("1302", ErrorKind::RateLimit),
    ("1305", ErrorKind::RateLimit),
    ("1312", ErrorKind::RateLimit),
    ("1002", ErrorKind::RateLimit),
    ("1000", ErrorKind::Auth),
    ("1001", ErrorKind::Auth),
    ("1003", ErrorKind::Auth),
    ("2049", ErrorKind::Auth),
];

/// Recognize provider-specific failures without changing other harnesses' classification.
pub fn classify_endpoint(text: &str) -> Option<ErrorKind> {
    let text = text.to_lowercase();
    for (phrases, kind) in [
        (
            &[
                "limit exhausted",
                "usage limit reached for",
                "5-hour usage limit",
                "weekly (7-day) usage limit",
                "monthly usage limit",
            ][..],
            ErrorKind::Quota,
        ),
        (
            &[
                "engine is currently overloaded",
                "too many requests",
                "concurrent request limit",
            ][..],
            ErrorKind::RateLimit,
        ),
        (&["membership benefits"][..], ErrorKind::Auth),
    ] {
        if phrases.iter().any(|p| text.contains(p)) {
            return Some(kind);
        }
    }
    let code = endpoint_code(&text)?;
    CODES
        .iter()
        .find_map(|&(candidate, kind)| (candidate == code).then_some(kind))
}

// The input is lowercased so the same contextual number rules apply everywhere.
fn endpoint_code(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        let before = &text[..start];
        let trimmed = before.trim_end_matches(|c: char| c.is_ascii_whitespace());
        let whitespace = trimmed.len() < before.len();
        let (without_quote, quoted_value) = match trimmed.strip_suffix('"') {
            Some(prefix) => (
                prefix.trim_end_matches(|c: char| c.is_ascii_whitespace()),
                true,
            ),
            None => (trimmed, false),
        };
        let json_code = without_quote.strip_suffix(':').is_some_and(|prefix| {
            prefix
                .trim_end_matches(|c: char| c.is_ascii_whitespace())
                .ends_with("\"code\"")
        }) && (!quoted_value || text[i..].starts_with('"'));
        let plain_code = !quoted_value && {
            let (label, has_colon) = match trimmed.strip_suffix(':') {
                Some(prefix) => (
                    prefix.trim_end_matches(|c: char| c.is_ascii_whitespace()),
                    true,
                ),
                None => (trimmed, false),
            };
            ["code", "error"].iter().any(|name| {
                (if has_colon {
                    *name == "code"
                } else {
                    whitespace
                }) && label.strip_suffix(name).is_some_and(|prefix| {
                    prefix
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
                })
            })
        };
        let code = &text[start..i];
        if (json_code || plain_code) && CODES.iter().any(|&(candidate, _)| candidate == code) {
            return Some(code);
        }
    }
    None
}

/// Whether a Z.ai reset describes the weekly or monthly window.
pub(crate) fn weekly_reset(text: &str) -> bool {
    matches!(
        endpoint_code(&text.to_lowercase()),
        Some("1310" | "1317" | "1319" | "1321")
    )
}

/// Extract the reset time reported by Z.ai, whose timestamp is in UTC+8.
pub(crate) fn reset_time(text: &str) -> Option<i64> {
    let lower = text.to_ascii_lowercase();
    let timestamp = ["your limit will reset at ", "resets at "]
        .iter()
        .find_map(|marker| {
            lower
                .find(marker)
                .and_then(|start| text.get(start + marker.len()..))
        })?
        .get(..19)?;
    let local = NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S").ok()?;
    FixedOffset::east_opt(8 * 3600)?
        .from_local_datetime(&local)
        .single()
        .map(|t| t.timestamp())
}

/// An endpoint validation failure. Details contain no supplied token.
#[derive(Debug)]
pub struct ValidateError {
    pub kind: ErrorKind,
    pub status: Option<u16>,
    pub detail: String,
}

/// Resolve and validate an endpoint account's credentials using its haiku slot.
/// Call from a blocking thread when token resolution may perform file I/O.
pub async fn validate_login(
    account: &so_core::AccountFile,
    host: &so_accounts::HostEnv,
) -> so_accounts::Result<Result<(), (String, String)>> {
    let endpoint = account
        .endpoint
        .as_ref()
        .ok_or_else(|| so_accounts::AccountsError::NotEndpoint(account.name.clone()))?;
    account.validate()?;
    let token = match so_accounts::resolve_endpoint_token(account, host) {
        Ok(token) => token,
        Err(e) => return Ok(Err(("auth".into(), e.to_string()))),
    };
    Ok(validate(
        endpoint.base_url(),
        endpoint.token_var(),
        &token,
        endpoint.slot(so_core::Slot::Haiku),
    )
    .await
    .map_err(|e| (e.kind.to_string(), e.detail)))
}

/// Check credentials with a minimal Anthropic-compatible messages request.
pub async fn validate(
    base_url: &str,
    token_var: &str,
    token: &str,
    model: &str,
) -> Result<(), ValidateError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|_| ValidateError {
            kind: ErrorKind::Other,
            status: None,
            detail: "Failed to create HTTP client".into(),
        })?;
    let request = client
        .post(format!("{}/v1/messages", base_url.trim_end_matches('/')))
        .header("anthropic-version", "2023-06-01")
        .json(&serde_json::json!({
            "model": model,
            "max_tokens": 1,
            "messages": [{"role": "user", "content": "ping"}],
        }));
    let request = if token_var == "ANTHROPIC_API_KEY" {
        request.header("x-api-key", token)
    } else {
        request.bearer_auth(token)
    };
    let response = request.send().await.map_err(|_| ValidateError {
        kind: ErrorKind::Other,
        status: None,
        detail: "Endpoint request failed".into(),
    })?;
    let status = response.status();
    if status.is_success() {
        return Ok(());
    }
    let body = response.text().await.map_err(|_| ValidateError {
        kind: ErrorKind::Other,
        status: Some(status.as_u16()),
        detail: "Failed to read endpoint response".into(),
    })?;
    let kind = classify_endpoint(&body).unwrap_or(match status.as_u16() {
        401 | 403 => ErrorKind::Auth,
        429 => ErrorKind::RateLimit,
        _ => ErrorKind::Other,
    });
    let safe = if token.is_empty() {
        body
    } else {
        body.replace(token, "[REDACTED]")
    };
    let detail = so_core::redact::redact(&safe).chars().take(500).collect();
    Err(ValidateError {
        kind,
        status: Some(status.as_u16()),
        detail,
    })
}

#[cfg(test)]
mod reset_tests {
    use super::reset_time;

    #[tokio::test]
    async fn login_rejects_non_endpoint_account_with_typed_error() {
        let account =
            so_core::AccountFile::new("plain", so_core::Harness::Claude, "~/.plain", "claude")
                .unwrap();
        let host = so_accounts::HostEnv {
            vars: Default::default(),
            home: "/tmp".into(),
            path_var: None,
            platform: so_core::Platform::current(),
        };
        assert!(
            matches!(super::validate_login(&account, &host).await, Err(so_accounts::AccountsError::NotEndpoint(name)) if name == "plain")
        );
    }

    #[test]
    fn reset_marker_is_case_insensitive() {
        for marker in [
            "your limit will reset at",
            "YOUR LIMIT WILL RESET AT",
            "resets at",
            "RESETS AT",
        ] {
            assert_eq!(
                reset_time(&format!("{marker} 2026-11-03 18:00:00")),
                Some(1793700000),
                "{marker}"
            );
        }
    }
}
