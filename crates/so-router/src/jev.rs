//! Jev decider client (SPEC 8.3 steps 2-3): OpenRouter `POST /api/alpha/decisions`.
//! The API key is never logged, printed or placed in an error message.
//!
//! Privacy: the task text, the choice descriptions and the account names (inside the
//! `usage_5h` context) are sent to the decider. Credentials and tokens are not.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use so_core::config::{Jev, JevModels, Policy};
use so_core::expand_tilde;
use so_core::proto::RouteUsage;

use crate::http::read_capped;

pub const MAX_RESPONSE: usize = 1_000_000;
const SUM_TOLERANCE: f64 = 0.02;

/// Environment lookup, a plain function so tests never touch the process environment.
pub type EnvFn = fn(&str) -> Option<String>;

pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// `KEY=VALUE` lines; `export ` prefixes, quotes and `#` comments are handled.
pub fn parse_dotenv(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let mut v = v.trim();
        if let Some(q) = v.chars().next().filter(|c| *c == '"' || *c == '\'')
            && let Some(end) = v[1..].find(q)
        {
            v = &v[1..=end];
        } else if let Some(i) = v.find(" #") {
            v = v[..i].trim();
        }
        out.insert(k.trim().to_string(), v.to_string());
    }
    out
}

/// The key from `key_env`, else from the dotenv `key_file`. `None` when neither has one.
pub fn resolve_key(cfg: &Jev, env: EnvFn) -> Option<String> {
    if let Some(k) = env(&cfg.key_env).filter(|k| !k.trim().is_empty()) {
        return Some(k.trim().to_string());
    }
    let path = expand_tilde(cfg.key_file.as_deref()?).ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    parse_dotenv(&text)
        .remove(&cfg.key_env)
        .filter(|k| !k.is_empty())
}

pub struct JevRequest<'a> {
    pub task: &'a str,
    /// The caller's own system prompt, when it has one; routing never supplies it today.
    pub system_instruction: &'a str,
    pub routing_instruction: &'a str,
    pub role: &'a str,
    pub context: Value,
    /// Eligible choice id -> criteria text.
    pub criteria: &'a BTreeMap<String, String>,
}

/// An answer that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    pub choice_id: String,
    pub confidence: f64,
    pub probabilities: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JevDecision {
    pub validated: Validated,
    pub usage: RouteUsage,
    /// The model that answered: the response's model id, else the requested one.
    pub model: String,
}

/// Why the decider did not decide. `reason` carries no secrets.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct JevFailure {
    pub reason: String,
    pub confidence: Option<f64>,
    pub probabilities: BTreeMap<String, f64>,
    pub usage: RouteUsage,
    /// The model that gave the rejected answer; `None` when no model answered.
    pub model: Option<String>,
    /// The decider could not be reached or refused the request (transport error, timeout,
    /// 429, 5xx, a missing model, or a rejected key), as opposed to an answer that validation
    /// or the gate rejected. `reason` then names the models, the cause and the next step.
    pub unreachable: bool,
}

impl JevFailure {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            ..Self::default()
        }
    }
}

fn unit(v: &Value) -> Option<f64> {
    v.as_f64()
        .filter(|x| x.is_finite() && (0.0..=1.0).contains(x))
}

/// Step 3 validation of a response body against the eligible ids.
pub fn validate(data: &Value, eligible: &BTreeSet<&str>) -> Result<Validated, String> {
    let bad = |m: &str| Err(m.to_string());
    let Some(answer) = data.pointer("/answers/route") else {
        return bad("response has no route answer");
    };
    if answer.get("type").and_then(Value::as_str) != Some("choice") {
        return bad("answer type is not choice");
    }
    let Some(choice) = answer.get("choice").and_then(Value::as_str) else {
        return bad("answer has no choice");
    };
    if !eligible.contains(choice) {
        return bad("chosen id is not eligible");
    }
    let Some(confidence) = answer.get("confidence").and_then(unit) else {
        return bad("confidence is not a finite number in [0, 1]");
    };
    let Some(map) = answer.get("probabilities").and_then(Value::as_object) else {
        return bad("probabilities missing");
    };
    let keys: BTreeSet<&str> = map.keys().map(String::as_str).collect();
    if keys != *eligible {
        return bad("probabilities do not cover exactly the eligible set");
    }
    let mut probabilities = BTreeMap::new();
    for (k, v) in map {
        let Some(p) = unit(v) else {
            return bad("a probability is not a finite number in [0, 1]");
        };
        probabilities.insert(k.clone(), p);
    }
    if (probabilities.values().sum::<f64>() - 1.0).abs() > SUM_TOLERANCE {
        return bad("probabilities do not sum to 1");
    }
    let max = probabilities.values().copied().fold(f64::MIN, f64::max);
    if probabilities[choice] < max {
        return bad("chosen id does not have the highest probability");
    }
    Ok(Validated {
        choice_id: choice.to_string(),
        confidence,
        probabilities,
    })
}

/// Policy gate: confidence and the margin between the two highest probabilities.
pub fn gate(v: &Validated, policy: &Policy) -> Result<(), String> {
    if v.confidence < policy.min_confidence {
        return Err("confidence below the configured threshold".into());
    }
    let mut p: Vec<f64> = v.probabilities.values().copied().collect();
    p.sort_by(|a, b| b.total_cmp(a));
    let margin = p[0] - p.get(1).copied().unwrap_or(0.0);
    if margin < policy.min_margin {
        return Err("probability margin below the configured threshold".into());
    }
    Ok(())
}

fn parse_usage(data: &Value) -> RouteUsage {
    let num = |k: &str| {
        data.pointer(&format!("/usage/{k}"))
            .and_then(Value::as_f64)
            .filter(|x| x.is_finite() && *x >= 0.0)
    };
    RouteUsage {
        input_tokens: num("input_tokens").map_or(0, |x| x as u64),
        output_tokens: num("output_tokens").map_or(0, |x| x as u64),
        cost: num("cost").unwrap_or(0.0),
    }
}

pub fn build_payload(model: &str, req: &JevRequest<'_>) -> Value {
    json!({
        "model": model,
        "state": {
            "task_content": req.task,
            "system_instruction": req.system_instruction,
            "routing_instruction": req.routing_instruction,
            "role": req.role,
            "context": req.context,
        },
        "questions": {
            "route": {
                "type": "choice",
                "instructions": req.routing_instruction,
                "criteria": req.criteria,
            },
        },
    })
}

/// Longest wait honoured from `Retry-After`.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(3);
/// Pause before retrying a failure that gave no `Retry-After`.
const BACKOFF: Duration = Duration::from_millis(150);

enum Attempt {
    Done(Value),
    /// Worth one retry after the given pause.
    Transient(String, Duration),
    /// Not retried.
    Permanent(String, Perm),
}

/// Why a failure is permanent.
#[derive(Clone, Copy)]
enum Perm {
    /// The model cannot be used (HTTP 400/404 for a missing model, or a request that never
    /// completed): the fallback model may try.
    Model,
    /// The key was refused (HTTP 401/403): another model would be refused too.
    Auth,
    /// Anything else, such as an unreadable or oversized response.
    Other,
}

fn retry_after(resp: &reqwest::Response) -> Duration {
    resp.headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map_or(BACKOFF, |s| Duration::from_secs(s).min(MAX_RETRY_WAIT))
}

/// https, or plain http to a loopback host (tests and local proxies). The bearer key must not
/// travel in clear text.
fn endpoint_allowed(endpoint: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return false;
    };
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|h| {
            h == "localhost"
                || h.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        _ => false,
    }
}

async fn attempt(
    http: &reqwest::Client,
    cfg: &Jev,
    key: &str,
    payload: &Value,
    timeout: Duration,
) -> Attempt {
    let sent = http
        .post(&cfg.endpoint)
        .timeout(timeout)
        .bearer_auth(key)
        .json(payload)
        .send()
        .await;
    let resp = match sent {
        Ok(r) => r,
        Err(e) if e.is_timeout() || e.is_connect() => {
            return Attempt::Transient(
                "Jev request timed out or could not connect".into(),
                BACKOFF,
            );
        }
        Err(_) => return Attempt::Permanent("Jev request failed".into(), Perm::Model),
    };
    let status = resp.status();
    // 408 and 429 say "try again": 429 may carry Retry-After.
    if status.is_server_error() || status.as_u16() == 408 || status.as_u16() == 429 {
        return Attempt::Transient(
            format!("Jev request failed (HTTP {})", status.as_u16()),
            retry_after(&resp),
        );
    }
    if !status.is_success() {
        let code = status.as_u16();
        return match code {
            400 | 404 => Attempt::Permanent(
                format!("Jev request failed (HTTP {code}; the model id may not exist)"),
                Perm::Model,
            ),
            401 | 403 => Attempt::Permanent(
                format!("Jev request failed (HTTP {code}; the key was refused)"),
                Perm::Auth,
            ),
            _ => Attempt::Permanent(format!("Jev request failed (HTTP {code})"), Perm::Other),
        };
    }
    match read_capped(resp, MAX_RESPONSE).await {
        Ok(body) => match serde_json::from_slice(&body) {
            Ok(v) => Attempt::Done(v),
            Err(_) => Attempt::Permanent("Jev returned an invalid response".into(), Perm::Other),
        },
        Err(e) if e.contains("size limit") => {
            Attempt::Permanent("Jev response exceeds the size limit".into(), Perm::Other)
        }
        Err(_) => Attempt::Transient("Jev response could not be read".into(), BACKOFF),
    }
}

/// Share of `router.jev.timeout` the primary model may use when a fallback is configured; the
/// rest is left for the fallback, so the whole call stays within the timeout.
const PRIMARY_SHARE: (u32, u32) = (3, 5);

/// Why one model gave no usable answer.
enum Failed {
    /// The model could not be reached or refused the request: the fallback may try.
    Unusable(JevFailure),
    /// An answer arrived but validation or the gate rejected it, or the key was refused, or
    /// the failure is permanent and not about the model: the fallback is not asked.
    Final(JevFailure),
}

async fn decide_model(
    http: &reqwest::Client,
    cfg: &Jev,
    model: &str,
    policy: &Policy,
    key: &str,
    req: &JevRequest<'_>,
    deadline: Instant,
) -> Result<JevDecision, Failed> {
    let payload = build_payload(model, req);
    let eligible: BTreeSet<&str> = req.criteria.keys().map(String::as_str).collect();
    let mut last = String::new();
    for n in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(Failed::Unusable(JevFailure::new(if last.is_empty() {
                "Jev request timed out".to_string()
            } else {
                last
            })));
        }
        match attempt(http, cfg, key, &payload, left).await {
            Attempt::Done(data) => {
                let usage = parse_usage(&data);
                let answered = data
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|m| !m.is_empty() && m.len() <= 200)
                    .unwrap_or(model)
                    .to_string();
                let validated = validate(&data, &eligible).map_err(|e| {
                    Failed::Final(JevFailure {
                        usage: usage.clone(),
                        model: Some(answered.clone()),
                        ..JevFailure::new(format!("Jev returned an invalid response ({e})"))
                    })
                })?;
                return match gate(&validated, policy) {
                    Ok(()) => Ok(JevDecision {
                        validated,
                        usage,
                        model: answered,
                    }),
                    Err(e) => Err(Failed::Final(JevFailure {
                        reason: format!("Jev decision rejected: {e}"),
                        confidence: Some(validated.confidence),
                        probabilities: validated.probabilities,
                        usage,
                        model: Some(answered),
                        unreachable: false,
                    })),
                };
            }
            Attempt::Permanent(r, why) => {
                let mut f = JevFailure::new(r);
                return Err(match why {
                    Perm::Model => Failed::Unusable(f),
                    Perm::Auth => {
                        f.unreachable = true;
                        Failed::Final(f)
                    }
                    Perm::Other => Failed::Final(f),
                });
            }
            Attempt::Transient(r, wait) => {
                last = r;
                if n == 0 {
                    let pause = wait.min(deadline.saturating_duration_since(Instant::now()));
                    tokio::time::sleep(pause).await;
                }
            }
        }
    }
    Err(Failed::Unusable(JevFailure::new(last)))
}

const NEXT_STEP: &str = "Check the OpenRouter key, the model id and the network, or choose another model with router.jev.model (or SUPERORCHESTRATOR_ROUTER_MODEL).";

/// The error text for a decider that could not be reached. Names every model tried and the
/// cause; never the key.
fn unreachable_reason(key_env: &str, tried: &[(&str, &str, bool)]) -> String {
    let parts: Vec<String> = tried
        .iter()
        .enumerate()
        .map(|(i, (model, cause, _))| {
            let who = if i == 0 { "model" } else { "fallback model" };
            format!("{who} `{model}`: {cause}")
        })
        .collect();
    let auth = tried.iter().any(|t| t.2);
    let next = if auth {
        format!("Check {key_env} or router.jev.key_file.")
    } else {
        NEXT_STEP.to_string()
    };
    format!("decider unreachable: {}. {next}", parts.join("; "))
}

/// Asks the decider. A transient failure (5xx, 429, timeout, connection) is retried once.
/// When the primary model is unusable (transport error, timeout, 429, 5xx, or 400/404 for a
/// missing model) the fallback model is tried; a rejection by validation or the gate is final
/// and never reaches the fallback, and a refused key (401/403) does not either, since the
/// fallback would be refused too. The whole call stays within `router.jev.timeout`.
/// Failures where no model could be reached carry `unreachable`. Rejections keep the usage
/// the call cost.
pub async fn decide(
    http: &reqwest::Client,
    cfg: &Jev,
    models: &JevModels,
    policy: &Policy,
    key: &str,
    req: &JevRequest<'_>,
) -> Result<JevDecision, JevFailure> {
    if !endpoint_allowed(&cfg.endpoint) {
        return Err(JevFailure::new(
            "Jev endpoint must use https (http is allowed only for loopback)",
        ));
    }
    let start = Instant::now();
    let total = cfg.timeout.as_duration();
    let fallback = models.fallback.as_deref();
    let primary_end = match fallback {
        Some(_) => start + total * PRIMARY_SHARE.0 / PRIMARY_SHARE.1,
        None => start + total,
    };
    let primary = models.primary.as_str();
    let first = match decide_model(http, cfg, primary, policy, key, req, primary_end).await {
        Ok(d) => return Ok(d),
        Err(Failed::Final(mut f)) => {
            if f.unreachable {
                f.reason = unreachable_reason(&cfg.key_env, &[(primary, &f.reason, true)]);
            }
            return Err(f);
        }
        Err(Failed::Unusable(f)) => f,
    };
    let unreachable = |reason: String| JevFailure {
        reason,
        unreachable: true,
        ..JevFailure::default()
    };
    let Some(fb) = fallback else {
        return Err(unreachable(unreachable_reason(
            &cfg.key_env,
            &[(primary, &first.reason, false)],
        )));
    };
    match decide_model(http, cfg, fb, policy, key, req, start + total).await {
        Ok(d) => Ok(d),
        Err(Failed::Final(mut f)) => {
            if f.unreachable {
                f.reason = unreachable_reason(
                    &cfg.key_env,
                    &[(primary, &first.reason, false), (fb, &f.reason, true)],
                );
            }
            Err(f)
        }
        Err(Failed::Unusable(f)) => Err(unreachable(unreachable_reason(
            &cfg.key_env,
            &[(primary, &first.reason, false), (fb, &f.reason, false)],
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set<'a>(ids: &[&'a str]) -> BTreeSet<&'a str> {
        ids.iter().copied().collect()
    }

    fn body(choice: &str, conf: Value, probs: Value) -> Value {
        json!({"answers": {"route": {"type": "choice", "choice": choice,
            "confidence": conf, "probabilities": probs}}})
    }

    fn ok_body() -> Value {
        body("a", json!(0.9), json!({"a": 0.8, "b": 0.2}))
    }

    #[test]
    fn accepts_a_valid_answer() {
        let v = validate(&ok_body(), &set(&["a", "b"])).unwrap();
        assert_eq!(v.choice_id, "a");
        assert_eq!(v.probabilities.len(), 2);
    }

    #[test]
    fn rejects_each_bad_case() {
        let e = set(&["a", "b"]);
        let mut wrong_type = ok_body();
        wrong_type["answers"]["route"]["type"] = json!("score");
        let cases = [
            ("wrong type", wrong_type),
            (
                "ineligible",
                body("z", json!(0.9), json!({"a": 0.8, "b": 0.2})),
            ),
            (
                "conf > 1",
                body("a", json!(1.5), json!({"a": 0.8, "b": 0.2})),
            ),
            (
                "conf negative",
                body("a", json!(-0.1), json!({"a": 0.8, "b": 0.2})),
            ),
            (
                "conf string",
                body("a", json!("0.9"), json!({"a": 0.8, "b": 0.2})),
            ),
            (
                "conf null (NaN)",
                body("a", Value::Null, json!({"a": 0.8, "b": 0.2})),
            ),
            ("missing key", body("a", json!(0.9), json!({"a": 1.0}))),
            (
                "extra key",
                body("a", json!(0.9), json!({"a": 0.7, "b": 0.2, "c": 0.1})),
            ),
            (
                "sum low",
                body("a", json!(0.9), json!({"a": 0.5, "b": 0.2})),
            ),
            (
                "sum high",
                body("a", json!(0.9), json!({"a": 0.8, "b": 0.3})),
            ),
            (
                "not max",
                body("b", json!(0.9), json!({"a": 0.8, "b": 0.2})),
            ),
            (
                "prob > 1",
                body("a", json!(0.9), json!({"a": 1.2, "b": -0.2})),
            ),
            ("no answer", json!({})),
        ];
        for (name, data) in cases {
            assert!(validate(&data, &e).is_err(), "{name}");
        }
    }

    #[test]
    fn sum_tolerance_is_two_percent() {
        let e = set(&["a", "b"]);
        assert!(validate(&body("a", json!(0.9), json!({"a": 0.81, "b": 0.20})), &e).is_ok());
        assert!(validate(&body("a", json!(0.9), json!({"a": 0.85, "b": 0.20})), &e).is_err());
    }

    #[test]
    fn gate_checks_confidence_and_margin() {
        let v = validate(&ok_body(), &set(&["a", "b"])).unwrap();
        let p = |c, m| Policy {
            min_confidence: c,
            min_margin: m,
        };
        assert!(gate(&v, &p(0.6, 0.1)).is_ok());
        assert!(gate(&v, &p(0.95, 0.1)).is_err());
        assert!(gate(&v, &p(0.6, 0.7)).is_err());
    }

    #[test]
    fn endpoint_policy() {
        assert!(endpoint_allowed(
            "https://openrouter.ai/api/alpha/decisions"
        ));
        assert!(endpoint_allowed("http://127.0.0.1:9/x"));
        assert!(endpoint_allowed("http://localhost/x"));
        assert!(endpoint_allowed("http://[::1]:1/x"));
        assert!(!endpoint_allowed("http://openrouter.ai/x"));
        assert!(!endpoint_allowed("ftp://127.0.0.1/x"));
        assert!(!endpoint_allowed("not a url"));
    }

    #[test]
    fn dotenv_parsing() {
        let m = parse_dotenv(
            "# c\nA=1\nexport B=\"two words\"\nC='x' # note\n\nD=plain # tail\nbad line\n",
        );
        assert_eq!(m["A"], "1");
        assert_eq!(m["B"], "two words");
        assert_eq!(m["C"], "x");
        assert_eq!(m["D"], "plain");
        assert!(!m.contains_key("bad line"));
    }

    #[test]
    fn key_from_env_or_dotenv_file() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join(".env");
        std::fs::write(&f, "SO_TEST_KEY=from-file\n").unwrap();
        let cfg = Jev {
            key_env: "SO_TEST_KEY".into(),
            key_file: Some(f.to_string_lossy().into_owned()),
            ..Jev::default()
        };
        assert_eq!(resolve_key(&cfg, |_| None).as_deref(), Some("from-file"));
        assert_eq!(
            resolve_key(&cfg, |_| Some("from-env".into())).as_deref(),
            Some("from-env")
        );
        let none = Jev {
            key_file: None,
            ..cfg
        };
        assert_eq!(resolve_key(&none, |_| None), None);
        assert_eq!(resolve_key(&none, |_| Some("  ".into())), None);
    }
}
