//! Classification and credential validation for Anthropic-compatible endpoints.
use std::time::Duration;

use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use so_core::ErrorKind;

/// Recognize provider-specific failures without changing other harnesses' classification.
pub fn classify_endpoint(text: &str) -> Option<ErrorKind> {
    let text = text.to_lowercase();
    for (phrases, kind) in [
        (
            &[
                "limit exhausted",
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
        let before = text[..start]
            .trim_end_matches(|c: char| c.is_ascii_whitespace() || matches!(c, ':' | '"'));
        let separated = before.len() < start;
        let named = separated
            && ["code", "error"].iter().any(|name| {
                before.strip_suffix(name).is_some_and(|prefix| {
                    prefix
                        .chars()
                        .next_back()
                        .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_')
                })
            });
        let provider_message = ["usage limit reached for 5 hour", "z.ai", "minimax"]
            .iter()
            .any(|p| {
                text.match_indices(p).any(|(pos, phrase)| {
                    let distance = if pos >= i {
                        pos - i
                    } else {
                        start.saturating_sub(pos + phrase.len())
                    };
                    distance <= 80 && !text[pos.min(start)..pos.max(start)].contains('\n')
                })
            });
        if !named && !provider_message {
            continue;
        }
        let kind = match &text[start..i] {
            "1308" | "1309" | "1310" | "1316" | "1317" | "1318" | "1319" | "1320" | "1321"
            | "2056" => ErrorKind::Quota,
            "1302" | "1305" | "1312" | "1002" => ErrorKind::RateLimit,
            "1000" | "1001" | "1003" | "2049" => ErrorKind::Auth,
            _ => continue,
        };
        return Some(kind);
    }
    None
}

/// Extract the reset time reported by Z.ai, whose timestamp is in UTC+8.
pub(crate) fn reset_time(text: &str) -> Option<i64> {
    let (_, tail) = text.split_once("Your limit will reset at ")?;
    let timestamp = tail.get(..19)?;
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
    let request = client.post(format!("{}/v1/messages", base_url.trim_end_matches('/'))).header("anthropic-version", "2023-06-01").json(&serde_json::json!({"model": model, "max_tokens": 1, "messages": [{"role": "user", "content": "ping"}]}));
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
