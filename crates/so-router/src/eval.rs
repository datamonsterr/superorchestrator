//! `route eval` (SPEC 8.6): replay labelled cases and report. Reports never carry task text
//! or credentials.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use so_core::proto::RouteParams;

use crate::route::{RouteCtx, eligible_choice_ids, route_with};

const BUNDLED_EN: &str = include_str!("../evals/en.jsonl");
const BUNDLED_VI: &str = include_str!("../evals/vi.jsonl");

/// One labelled case. `context` is accepted for compatibility with agent-dispatcher's files
/// and ignored: the decider sees only the task, the role and the routing instruction.
#[derive(Debug, Clone, Deserialize)]
pub struct Case {
    pub id: String,
    pub task: String,
    #[serde(default = "worker")]
    pub role: String,
    pub expected_choices: Vec<String>,
    #[serde(default)]
    pub available_choice_ids: Option<Vec<String>>,
    /// Why a second expected choice is as correct as the first. Documentation only.
    #[serde(default)]
    #[allow(dead_code)]
    pub reason: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub context: Option<Value>,
}

fn worker() -> String {
    "worker".into()
}

/// The English and Vietnamese cases that ship in the binary.
pub fn bundled_cases() -> Vec<Case> {
    parse_cases(&format!("{BUNDLED_EN}\n{BUNDLED_VI}")).expect("bundled cases are valid")
}

/// JSON Lines, or a single JSON array (agent-dispatcher's `cases.json`).
pub fn parse_cases(text: &str) -> Result<Vec<Case>, String> {
    let t = text.trim();
    let cases: Vec<Case> = if t.starts_with('[') {
        serde_json::from_str(t).map_err(|e| format!("invalid cases: {e}"))?
    } else {
        t.lines()
            .filter(|l| !l.trim().is_empty())
            .enumerate()
            .map(|(i, l)| {
                serde_json::from_str(l).map_err(|e| format!("invalid case on line {}: {e}", i + 1))
            })
            .collect::<Result<_, _>>()?
    };
    if cases.is_empty() {
        return Err("no cases".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for c in &cases {
        if c.id.is_empty() || !seen.insert(c.id.clone()) {
            return Err("case ids must be non-empty and unique".into());
        }
        if c.task.trim().is_empty() || c.expected_choices.is_empty() {
            return Err(format!("case `{}` needs a task and expected_choices", c.id));
        }
    }
    Ok(cases)
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseRecord {
    pub id: String,
    pub choice_id: Option<String>,
    pub expected_choices: Vec<String>,
    pub correct: bool,
    pub eligible: bool,
    pub source: String,
    /// The decider model that answered (also when its answer was rejected).
    pub decider_model: Option<String>,
    pub confidence: Option<f64>,
    /// Jev's probabilities per eligible choice, also kept when the gate rejected the answer.
    pub probabilities: BTreeMap<String, f64>,
    /// Why the decider's answer was not used (no task text, no credentials).
    pub fallback_reason: Option<String>,
    pub latency_ms: f64,
    pub cost: f64,
    /// A failure category, never the task text.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Latency {
    pub p50: f64,
    pub p95: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    pub case_count: usize,
    pub accuracy: f64,
    pub eligible_compliance: f64,
    /// Cases where the decider was wanted but its answer was not used.
    pub fallback_count: usize,
    pub errors_count: usize,
    /// The decider models in effect (env > config > default) and where each came from.
    pub decider_models: so_core::config::JevModels,
    pub sources: BTreeMap<String, usize>,
    pub latency_ms: Latency,
    pub total_cost: f64,
    pub cases: Vec<CaseRecord>,
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let pos = (sorted.len() - 1) as f64 * q;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - pos.floor())
}

fn source_name(s: so_core::proto::RouteSource) -> &'static str {
    match s {
        so_core::proto::RouteSource::Jev => "jev",
        so_core::proto::RouteSource::Heuristic => "heuristic",
        so_core::proto::RouteSource::Pinned => "pinned",
    }
}

/// Replays `cases` sequentially against the live router state in `ctx`.
pub async fn evaluate(ctx: &RouteCtx<'_>, cases: &[Case]) -> EvalReport {
    let mut records = Vec::new();
    let mut sources: BTreeMap<String, usize> = BTreeMap::new();
    let mut latencies = Vec::new();
    let mut fallbacks = 0;
    let mut errors = 0;
    let mut total_cost = 0.0;
    for case in cases {
        let exclude: Vec<String> = case
            .available_choice_ids
            .as_ref()
            .map(|avail| {
                ctx.config
                    .choices
                    .iter()
                    .filter(|c| !avail.contains(&c.id))
                    .map(|c| c.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let eligible = eligible_choice_ids(ctx, &case.role, &exclude);
        let params = RouteParams {
            prompt: case.task.clone(),
            role: Some(case.role.clone()),
            exclude,
            ..RouteParams::default()
        };
        let started = Instant::now();
        let routed = route_with(ctx, &params).await;
        let latency_ms = started.elapsed().as_secs_f64() * 1000.0;
        latencies.push(latency_ms);
        let rec = match routed {
            Ok(o) => {
                if o.fallback_reason.is_some() {
                    fallbacks += 1;
                }
                let fallback_reason = o.fallback_reason.clone();
                total_cost += o.result.usage.cost;
                let r = o.result;
                CaseRecord {
                    correct: case.expected_choices.contains(&r.choice_id),
                    eligible: eligible.contains(&r.choice_id),
                    source: source_name(r.source).into(),
                    decider_model: r.decider_model.clone(),
                    confidence: r.confidence,
                    probabilities: r.probabilities.clone(),
                    fallback_reason,
                    cost: r.usage.cost,
                    choice_id: Some(r.choice_id),
                    id: case.id.clone(),
                    expected_choices: case.expected_choices.clone(),
                    latency_ms,
                    error: None,
                }
            }
            Err(e) => {
                errors += 1;
                CaseRecord {
                    id: case.id.clone(),
                    choice_id: None,
                    expected_choices: case.expected_choices.clone(),
                    correct: false,
                    eligible: false,
                    source: "error".into(),
                    decider_model: None,
                    confidence: None,
                    probabilities: BTreeMap::new(),
                    fallback_reason: None,
                    latency_ms,
                    cost: 0.0,
                    error: Some(match e {
                        crate::RouterError::NoEligible(_) => "no_eligible_choice".into(),
                        crate::RouterError::Invalid(_) => "invalid_request".into(),
                        crate::RouterError::DeciderUnreachable(_) => "decider_unreachable".into(),
                        _ => "router_error".into(),
                    }),
                }
            }
        };
        *sources.entry(rec.source.clone()).or_default() += 1;
        records.push(rec);
    }
    let n = records.len().max(1) as f64;
    latencies.sort_by(f64::total_cmp);
    EvalReport {
        case_count: records.len(),
        accuracy: records.iter().filter(|r| r.correct).count() as f64 / n,
        eligible_compliance: records.iter().filter(|r| r.eligible).count() as f64 / n,
        fallback_count: fallbacks,
        errors_count: errors,
        decider_models: ctx.config.router.jev.resolve_models(ctx.env),
        sources,
        latency_ms: Latency {
            p50: percentile(&latencies, 0.5),
            p95: percentile(&latencies, 0.95),
        },
        total_cost,
        cases: records,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_cases_load() {
        let c = bundled_cases();
        assert_eq!(c.len(), 15);
        assert!(c.iter().any(|c| c.id.starts_with("vietnamese")));
    }

    #[test]
    fn holdout_cases_load_and_stay_separate_from_the_bundled_ones() {
        let hold = parse_cases(include_str!("../evals/holdout.jsonl")).unwrap();
        assert!(hold.len() >= 15);
        assert!(hold.iter().any(|c| c.role == "conductor"));
        assert!(hold.iter().any(|c| c.id.contains("injection")));
        let bundled = bundled_cases();
        assert!(hold.iter().all(|h| bundled.iter().all(|b| b.id != h.id)));
        // A second expected choice always carries its reason.
        assert!(
            hold.iter()
                .filter(|c| c.expected_choices.len() > 1)
                .all(|c| c.reason.is_some())
        );
    }

    #[test]
    fn parses_jsonl_and_array_and_rejects_bad() {
        let line = r#"{"id":"a","task":"t","expected_choices":["x"]}"#;
        assert_eq!(parse_cases(line).unwrap()[0].role, "worker");
        assert_eq!(parse_cases(&format!("[{line}]")).unwrap().len(), 1);
        assert!(parse_cases("").is_err());
        assert!(parse_cases(&format!("{line}\n{line}")).is_err());
        assert!(parse_cases(r#"{"id":"a","task":" ","expected_choices":["x"]}"#).is_err());
    }

    #[test]
    fn percentiles() {
        assert_eq!(percentile(&[], 0.5), 0.0);
        assert_eq!(percentile(&[1.0, 3.0], 0.5), 2.0);
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 0.95), 4.8);
    }
}
