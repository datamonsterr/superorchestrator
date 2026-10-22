//! Pure parsers for usage output of each harness. They never log their input.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::error::{Result, UsageError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    FiveHour,
    Weekly,
}

/// One window of one pool. `remaining_pct == None` means disabled or unknown.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReading {
    pub pool: String,
    pub window: Window,
    pub remaining_pct: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
}

/// Epoch seconds, or milliseconds (auto-detected). Rejects negative values and years after 2100.
fn epoch(v: &Value) -> Option<DateTime<Utc>> {
    const MAX_SECS: i64 = 4_102_444_800; // 2100-01-01
    let mut n = v.as_f64()? as i64;
    if n > MAX_SECS {
        n /= 1000;
    }
    if !(0..=MAX_SECS).contains(&n) {
        return None;
    }
    DateTime::from_timestamp(n, 0)
}

fn clamp_pct(p: f64) -> f64 {
    p.clamp(0.0, 100.0)
}

/// Window by duration in minutes (tolerance 10%), never by name.
fn window_for_minutes(mins: f64) -> Option<Window> {
    let near = |target: f64| (mins - target).abs() <= target * 0.1;
    if near(300.0) {
        Some(Window::FiveHour)
    } else if near(10080.0) {
        Some(Window::Weekly)
    } else {
        None
    }
}

/// Parses the `account/rateLimits/read` response (the whole JSON-RPC message, its `result`,
/// or the `rateLimits` object) and `account/rateLimits/updated` notification params.
pub fn parse_codex_rate_limits(v: &Value) -> Vec<WindowReading> {
    let mut node = v.get("result").unwrap_or(v);
    if let Some(rl) = node.get("rateLimits") {
        node = rl;
    }
    let mut out = Vec::new();
    for key in ["primary", "secondary"] {
        let Some(w) = node.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let Some(window) = w
            .get("windowDurationMins")
            .and_then(Value::as_f64)
            .and_then(window_for_minutes)
        else {
            continue;
        };
        out.push(WindowReading {
            pool: "default".into(),
            window,
            remaining_pct: w
                .get("usedPercent")
                .and_then(Value::as_f64)
                .map(|u| clamp_pct(100.0 - u)),
            resets_at: w.get("resetsAt").and_then(epoch),
        });
    }
    out
}

/// Parses `agy --print /usage` TSV output.
pub fn parse_agy_usage(text: &str) -> Result<Vec<WindowReading>> {
    if text.lines().any(|l| {
        l.trim_start()
            .starts_with("error: Eligibility check failed")
    }) {
        return Err(UsageError::NotEligible);
    }
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 {
            continue;
        }
        let pool = match f[0].trim() {
            "Gemini Models" => "gemini",
            "Claude and GPT models" => "claude",
            _ => continue,
        };
        let window = match f[1].trim() {
            "Five Hour Limit Remaining" => Window::FiveHour,
            "Weekly Limit Remaining" => Window::Weekly,
            _ => continue,
        };
        let raw = f[2].trim();
        let remaining_pct = if raw.eq_ignore_ascii_case("disabled") {
            None
        } else {
            match raw.trim_end_matches('%').parse::<f64>() {
                Ok(p) => Some(clamp_pct(p)),
                Err(_) => continue,
            }
        };
        let resets_at = f
            .get(3)
            .and_then(|s| DateTime::parse_from_rfc3339(s.trim()).ok())
            .map(|d| d.with_timezone(&Utc));
        out.push(WindowReading {
            pool: pool.into(),
            window,
            remaining_pct,
            resets_at,
        });
    }
    Ok(out)
}

/// Parses a claude `rate_limit_event` (utilization is a 0-1 fraction).
pub fn parse_claude_rate_limit_event(v: &Value) -> Vec<WindowReading> {
    let Some(windows) = v
        .get("rate_limit_info")
        .and_then(|i| i.get("unifiedWindows"))
    else {
        return Vec::new();
    };
    [
        ("five_hour", Window::FiveHour),
        ("seven_day", Window::Weekly),
    ]
    .into_iter()
    .filter_map(|(key, window)| {
        let w = windows.get(key)?;
        Some(WindowReading {
            pool: "default".into(),
            window,
            remaining_pct: w
                .get("utilization")
                .and_then(Value::as_f64)
                // A fraction in 0-1; anything above 1 is already a percentage.
                .map(|u| {
                    clamp_pct(if u > 1.0 {
                        100.0 - u
                    } else {
                        (1.0 - u) * 100.0
                    })
                }),
            resets_at: w.get("resetsAt").and_then(epoch),
        })
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

    fn fx(p: &str) -> String {
        std::fs::read_to_string(format!("{FX}/{p}")).unwrap()
    }

    #[test]
    fn codex_read_response() {
        let v: Value = serde_json::from_str(&fx("codex/rate-limits-read.response.json")).unwrap();
        let r = parse_codex_rate_limits(&v);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].window, Window::FiveHour);
        assert_eq!(r[0].remaining_pct, Some(79.0));
        assert_eq!(r[0].resets_at.unwrap().timestamp(), 1792665377);
        assert_eq!(r[1].window, Window::Weekly);
        assert_eq!(r[1].remaining_pct, Some(30.0));
        assert_eq!(r[0].pool, "default");
    }

    #[test]
    fn codex_updated_notification_and_duration_mapping() {
        // Swapped names: mapping must follow the duration.
        let v = json!({"rateLimits": {
            "primary": {"usedPercent": 10, "windowDurationMins": 10080, "resetsAt": 100},
            "secondary": {"usedPercent": 40.5, "windowDurationMins": 300, "resetsAt": null},
            "extra": {"usedPercent": 1, "windowDurationMins": 60}
        }});
        let r = parse_codex_rate_limits(&v);
        assert_eq!(r[0].window, Window::Weekly);
        assert_eq!(r[0].remaining_pct, Some(90.0));
        assert_eq!(r[1].window, Window::FiveHour);
        assert_eq!(r[1].remaining_pct, Some(59.5));
        assert_eq!(r[1].resets_at, None);
        assert!(parse_codex_rate_limits(&json!({"x": 1})).is_empty());
    }

    #[test]
    fn agy_ok() {
        let r = parse_agy_usage(&fx("agy/usage-ok.txt")).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(r[0].pool, "gemini");
        assert_eq!(r[0].window, Window::Weekly);
        assert_eq!(r[0].remaining_pct, Some(92.0));
        assert_eq!(
            r[0].resets_at.unwrap().to_rfc3339(),
            "2026-10-23T22:36:54+00:00"
        );
        assert_eq!(r[3].pool, "claude");
        assert_eq!(r[3].window, Window::FiveHour);
    }

    #[test]
    fn agy_exhausted_with_disabled() {
        let r = parse_agy_usage(&fx("agy/usage-exhausted.txt")).unwrap();
        assert_eq!(r[0].remaining_pct, Some(0.0));
        assert_eq!(r[1].remaining_pct, None);
        assert_eq!(r[1].resets_at, None);
        assert_eq!(r[2].remaining_pct, Some(1.0));
    }

    #[test]
    fn agy_ignores_noise_and_detects_ineligible() {
        let r =
            parse_agy_usage("warning: hi\nGemini Models\tWeekly Limit Remaining\t50%\t\n").unwrap();
        assert_eq!(r.len(), 1);
        let e = parse_agy_usage(
            "error: Eligibility check failed: Your current account is not eligible\nhttps://x\n",
        );
        assert!(matches!(e, Err(UsageError::NotEligible)));
        // Only a line that starts with the error counts.
        assert!(parse_agy_usage("note: Eligibility check failed earlier\n").is_ok());
    }

    #[test]
    fn claude_events_from_transcript() {
        let t = fx("claude/transcript.jsonl");
        let ev: Vec<Value> = t
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|l| l.get("msg").cloned())
            .filter(|m| m["type"] == "rate_limit_event")
            .collect();
        assert!(!ev.is_empty());
        let r = parse_claude_rate_limit_event(&ev[0]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].window, Window::FiveHour);
        assert!((r[0].remaining_pct.unwrap() - 79.0).abs() < 1e-6);
        assert!((r[1].remaining_pct.unwrap() - 97.0).abs() < 1e-6);
        assert_eq!(r[0].resets_at.unwrap().timestamp(), 1792669200);
        assert!(parse_claude_rate_limit_event(&json!({"type": "result"})).is_empty());
        let pct =
            json!({"rate_limit_info": {"unifiedWindows": {"five_hour": {"utilization": 25}}}});
        assert_eq!(
            parse_claude_rate_limit_event(&pct)[0].remaining_pct,
            Some(75.0)
        );
    }

    #[test]
    fn epoch_validation() {
        let at = |v: Value| {
            parse_codex_rate_limits(&json!({"primary": {"usedPercent": 1,
                "windowDurationMins": 300, "resetsAt": v}}))[0]
                .resets_at
        };
        assert_eq!(at(json!(1792665377)).unwrap().timestamp(), 1792665377);
        assert_eq!(
            at(json!(1792665377000_i64)).unwrap().timestamp(),
            1792665377
        );
        assert_eq!(at(json!(-5)), None);
        assert_eq!(at(json!(99_999_999_999_999_i64 * 1000)), None);
    }
}
