//! Jev decider client (SPEC 8.3 steps 2-3): OpenRouter `POST /api/alpha/decisions`.
//! The API key is never logged, printed or placed in an error message.
//!
//! The decider rates the complexity of the task against fixed anchors (a `score` question).
//! Privacy: only the task text, the role and the routing instruction are sent. Choices,
//! account names, quota, credentials and tokens are not.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use so_core::config::{COMPLEXITY_ANCHORS, Jev, JevModels, Policy};
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
}

/// An answer that passed validation.
#[derive(Debug, Clone, PartialEq)]
pub struct Validated {
    /// The probability-weighted anchor index, 0 to `anchors - 1`.
    pub score: f64,
    /// `score` mapped to 0 to 100, one decimal.
    pub complexity: f64,
    pub confidence: f64,
    /// Probability per anchor, keyed "0" to "n-1".
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

/// Maps an anchor index in `[0, anchors - 1]` to complexity 0 to 100, one decimal.
pub fn complexity_of(score: f64, anchors: usize) -> f64 {
    let top = (anchors - 1) as f64;
    (score / top * 100.0 * 10.0).round() / 10.0
}

/// Validation of a response body against the anchor list.
pub fn validate(data: &Value) -> Result<Validated, String> {
    let bad = |m: &str| Err(m.to_string());
    let n = COMPLEXITY_ANCHORS.len();
    let Some(answer) = data.pointer("/answers/complexity") else {
        return bad("response has no complexity answer");
    };
    if answer.get("type").and_then(Value::as_str) != Some("score") {
        return bad("answer type is not score");
    }
    let Some(score) = answer
        .get("score")
        .and_then(Value::as_f64)
        .filter(|x| x.is_finite() && (0.0..=(n - 1) as f64).contains(x))
    else {
        return bad("score is not a finite number within the anchor range");
    };
    let Some(confidence) = answer.get("confidence").and_then(unit) else {
        return bad("confidence is not a finite number in [0, 1]");
    };
    let Some(map) = answer.get("probabilities").and_then(Value::as_object) else {
        return bad("probabilities missing");
    };
    let expected: Vec<String> = (0..n).map(|i| i.to_string()).collect();
    if map.len() != n || !expected.iter().all(|k| map.contains_key(k)) {
        return bad("probabilities do not cover exactly the anchors");
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
    Ok(Validated {
        score,
        complexity: complexity_of(score, n),
        confidence,
        probabilities,
    })
}

/// Policy gate: confidence only.
pub fn gate(v: &Validated, policy: &Policy) -> Result<(), String> {
    if v.confidence < policy.min_confidence {
        return Err("confidence below the configured threshold".into());
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
        },
        "questions": {
            "complexity": {
                "type": "score",
                "instructions": req.routing_instruction,
                "criteria": COMPLEXITY_ANCHORS,
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
                format!(
                    "Jev request failed (HTTP {code}: the model id may not exist or the request was refused)"
                ),
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
                let validated = validate(&data).map_err(|e| {
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

const NEXT_STEP: &str = "Check the model id (it may not exist), the OpenRouter key and the network, or choose another model with router.jev.model (or SUPERORCHESTRATOR_ROUTER_MODEL).";

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

/// Asks the decider. A transient failure (5xx, 429, 408, a refused connection) is retried once
/// while time remains; a timeout is not retried, as it uses up the time left for that model.
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

    fn body(score: Value, conf: Value, probs: Value) -> Value {
        json!({"answers": {"complexity": {"type": "score", "score": score,
            "confidence": conf, "probabilities": probs, "legend": {}}}})
    }

    fn probs() -> Value {
        json!({"0": 0.0, "1": 0.1, "2": 0.6, "3": 0.3, "4": 0.0})
    }

    fn ok_body() -> Value {
        body(json!(2.2), json!(0.9), probs())
    }

    #[test]
    fn accepts_a_valid_answer_and_maps_the_score() {
        let v = validate(&ok_body()).unwrap();
        assert_eq!(v.score, 2.2);
        assert_eq!(v.complexity, 55.0);
        assert_eq!(v.probabilities.len(), 5);
    }

    #[test]
    fn complexity_mapping_is_linear_and_rounded_to_one_decimal() {
        assert_eq!(complexity_of(0.0, 5), 0.0);
        assert_eq!(complexity_of(1.0, 5), 25.0);
        assert_eq!(complexity_of(4.0, 5), 100.0);
        assert_eq!(complexity_of(1.0 / 3.0, 5), 8.3);
        assert_eq!(complexity_of(2.999, 5), 75.0);
    }

    #[test]
    fn rejects_each_bad_case() {
        let mut wrong_type = ok_body();
        wrong_type["answers"]["complexity"]["type"] = json!("choice");
        let mut other_question = ok_body();
        other_question["answers"] = json!({"route": other_question["answers"]["complexity"]});
        let cases = [
            ("wrong type", wrong_type),
            ("wrong question", other_question),
            ("score above range", body(json!(4.5), json!(0.9), probs())),
            ("score negative", body(json!(-0.1), json!(0.9), probs())),
            ("score string", body(json!("2"), json!(0.9), probs())),
            ("score null (NaN)", body(Value::Null, json!(0.9), probs())),
            ("conf > 1", body(json!(2.0), json!(1.5), probs())),
            ("conf negative", body(json!(2.0), json!(-0.1), probs())),
            ("conf string", body(json!(2.0), json!("0.9"), probs())),
            ("conf null (NaN)", body(json!(2.0), Value::Null, probs())),
            (
                "missing key",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.2, "1": 0.2, "2": 0.3, "3": 0.3}),
                ),
            ),
            (
                "extra key",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.1, "2": 0.6, "3": 0.2, "4": 0.1, "5": 0.0}),
                ),
            ),
            (
                "wrong key names",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.1, "2": 0.6, "3": 0.3, "five": 0.0}),
                ),
            ),
            (
                "sum low",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.1, "2": 0.5, "3": 0.3, "4": 0.0}),
                ),
            ),
            (
                "sum high",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.1, "2": 0.7, "3": 0.4, "4": 0.0}),
                ),
            ),
            (
                "prob > 1",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.0, "2": 1.2, "3": -0.2, "4": 0.0}),
                ),
            ),
            (
                "prob string",
                body(
                    json!(2.0),
                    json!(0.9),
                    json!({"0": 0.0, "1": 0.1, "2": "0.6", "3": 0.3, "4": 0.0}),
                ),
            ),
            ("probabilities missing", {
                let mut b = ok_body();
                b["answers"]["complexity"]
                    .as_object_mut()
                    .unwrap()
                    .remove("probabilities");
                b
            }),
            ("no answer", json!({})),
        ];
        for (name, data) in cases {
            assert!(validate(&data).is_err(), "{name}");
        }
    }

    #[test]
    fn score_bounds_are_inclusive() {
        assert!(validate(&body(json!(0), json!(0.9), probs())).is_ok());
        assert!(validate(&body(json!(4), json!(0.9), probs())).is_ok());
    }

    #[test]
    fn sum_tolerance_is_two_percent() {
        let p = |s: f64| json!({"0": 0.0, "1": 0.1, "2": s, "3": 0.3, "4": 0.0});
        assert!(validate(&body(json!(2.0), json!(0.9), p(0.61))).is_ok());
        assert!(validate(&body(json!(2.0), json!(0.9), p(0.65))).is_err());
    }

    #[test]
    fn gate_checks_confidence_only() {
        let v = validate(&ok_body()).unwrap();
        let p = |c, m| Policy {
            min_confidence: c,
            min_margin: m,
        };
        assert!(gate(&v, &p(0.6, 0.1)).is_ok());
        assert!(gate(&v, &p(0.95, 0.1)).is_err());
        // The deprecated margin is ignored.
        assert!(gate(&v, &p(0.6, 0.99)).is_ok());
    }

    #[test]
    fn payload_asks_one_score_question_and_sends_no_account_context() {
        let req = JevRequest {
            task: "t",
            system_instruction: "",
            routing_instruction: "rate it",
            role: "worker",
        };
        let b = build_payload("m", &req);
        let q = &b["questions"]["complexity"];
        assert_eq!(q["type"], "score");
        assert_eq!(q["instructions"], "rate it");
        assert_eq!(q["criteria"].as_array().unwrap().len(), 5);
        assert_eq!(q["criteria"][0], COMPLEXITY_ANCHORS[0]);
        assert_eq!(b["questions"].as_object().unwrap().len(), 1);
        assert!(b["state"].get("context").is_none());
        assert_eq!(b["state"]["task_content"], "t");
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
