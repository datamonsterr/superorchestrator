//! Pure per-level analysis of routing history.
use chrono::{DateTime, Utc};
use serde::Serialize;
use so_core::{ErrorKind, RunState, config::Level};
use std::collections::BTreeMap;

pub const BOUNDARY_BAND: f64 = 5.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryRecord {
    pub run_id: String,
    pub prompt: Option<String>,
    pub state: RunState,
    pub error_kind: Option<ErrorKind>,
    pub choice_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub complexity: f64,
    pub level: Option<String>,
    pub source: Option<String>,
    pub reviews_accepted: usize,
    pub reviews_rejected: usize,
    pub failovers: usize,
    pub escalations: usize,
}
impl TryFrom<so_store::HistoryRow> for HistoryRecord {
    type Error = ();
    fn try_from(r: so_store::HistoryRow) -> Result<Self, Self::Error> {
        Ok(Self {
            run_id: r.run_id,
            prompt: r.prompt,
            state: r.state,
            error_kind: r.error_kind,
            choice_id: r.choice_id,
            created_at: r.created_at,
            started_at: r.started_at,
            finished_at: r.finished_at,
            complexity: r.complexity.ok_or(())?,
            level: r.level,
            source: r.source,
            reviews_accepted: r.reviews_accepted,
            reviews_rejected: r.reviews_rejected,
            failovers: r.failovers,
            escalations: r.escalations,
        })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Rejected,
    Accepted,
    Failed,
    Succeeded,
    Other,
}
impl HistoryRecord {
    pub fn outcome(&self) -> Outcome {
        if self.reviews_rejected > 0 {
            Outcome::Rejected
        } else if self.reviews_accepted > 0 {
            Outcome::Accepted
        } else if self.state == RunState::Failed {
            Outcome::Failed
        } else if self.state == RunState::Succeeded {
            Outcome::Succeeded
        } else {
            Outcome::Other
        }
    }
    pub fn bad(&self) -> bool {
        matches!(self.outcome(), Outcome::Rejected | Outcome::Failed)
            || self.failovers > 0
            || self.escalations > 0
    }
}
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvalExample {
    pub id: String,
    pub task: String,
    pub expected_complexity: (u8, u8),
}
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Bucket {
    pub from: u8,
    pub to: u8,
    pub count: usize,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct OutcomeCounts {
    pub accepted: usize,
    pub rejected: usize,
    pub failed: usize,
    pub succeeded: usize,
    pub other: usize,
    pub failover: usize,
    pub escalated: usize,
}
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ModelInsight {
    pub choice_id: Option<String>,
    pub runs: usize,
    pub success_rate: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct InsightExample {
    pub run_id: Option<String>,
    pub eval_id: Option<String>,
    pub excerpt: String,
    pub complexity: f64,
    pub outcome: Option<Outcome>,
    pub source: String,
    pub borderline: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct LevelInsight {
    pub name: String,
    pub min: u8,
    pub max: u8,
    pub runs: usize,
    pub complexity_min: Option<f64>,
    pub complexity_median: Option<f64>,
    pub complexity_max: Option<f64>,
    pub buckets: Vec<Bucket>,
    pub outcomes: OutcomeCounts,
    pub success_rate: Option<f64>,
    pub accepted_rate: Option<f64>,
    pub rejected_rate: Option<f64>,
    pub failed_rate: Option<f64>,
    pub succeeded_rate: Option<f64>,
    pub failover_rate: Option<f64>,
    pub escalated_rate: Option<f64>,
    pub median_duration_ms: Option<f64>,
    pub models: Vec<ModelInsight>,
    pub examples: Vec<InsightExample>,
}
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    Some(if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    })
}
fn rate(n: usize, d: usize) -> Option<f64> {
    (d > 0).then(|| n as f64 / d as f64)
}
fn success(records: &[&HistoryRecord]) -> Option<f64> {
    let counted: Vec<_> = records
        .iter()
        .filter(|r| r.outcome() != Outcome::Other)
        .collect();
    rate(counted.iter().filter(|r| !r.bad()).count(), counted.len())
}
fn borderline(level: &Level, c: f64) -> bool {
    (c - f64::from(level.min)).abs() <= BOUNDARY_BAND
        || (c - f64::from(level.max)).abs() <= BOUNDARY_BAND
}
fn mapped(levels: &[Level], c: f64) -> Option<usize> {
    crate::levels::levels_by_distance(levels, c)
        .first()
        .copied()
}
/// Analyze records under the current level configuration.
pub fn analyze(
    levels: &[Level],
    records: &[HistoryRecord],
    eval: &[EvalExample],
) -> Vec<LevelInsight> {
    levels
        .iter()
        .enumerate()
        .map(|(index, level)| {
            let rows: Vec<_> = records
                .iter()
                .filter(|r| mapped(levels, r.complexity) == Some(index))
                .collect();
            let mut values: Vec<_> = rows.iter().map(|r| r.complexity).collect();
            values.sort_by(f64::total_cmp);
            let mut buckets = Vec::new();
            let mut from = u16::from(level.min);
            while from <= u16::from(level.max) {
                let to = (from + 9).min(u16::from(level.max));
                buckets.push(Bucket {
                    from: from as u8,
                    to: to as u8,
                    count: 0,
                });
                from = to + 1;
            }
            for r in &rows {
                if !buckets.is_empty() {
                    let offset = ((r.complexity - f64::from(level.min)).floor().max(0.0) as usize
                        / 10)
                        .min(buckets.len() - 1);
                    buckets[offset].count += 1;
                }
            }
            let mut outcomes = OutcomeCounts::default();
            let mut models: BTreeMap<Option<String>, Vec<&HistoryRecord>> = BTreeMap::new();
            for r in &rows {
                match r.outcome() {
                    Outcome::Accepted => outcomes.accepted += 1,
                    Outcome::Rejected => outcomes.rejected += 1,
                    Outcome::Failed => outcomes.failed += 1,
                    Outcome::Succeeded => outcomes.succeeded += 1,
                    Outcome::Other => outcomes.other += 1,
                }
                if r.failovers > 0 {
                    outcomes.failover += 1;
                }
                if r.escalations > 0 {
                    outcomes.escalated += 1;
                }
                models.entry(r.choice_id.clone()).or_default().push(r);
            }
            let counted = rows.len() - outcomes.other;
            let failover_rate = rate(
                rows.iter()
                    .filter(|r| r.outcome() != Outcome::Other && r.failovers > 0)
                    .count(),
                counted,
            );
            let escalated_rate = rate(
                rows.iter()
                    .filter(|r| r.outcome() != Outcome::Other && r.escalations > 0)
                    .count(),
                counted,
            );
            let mut models: Vec<_> = models
                .into_iter()
                .map(|(choice_id, rs)| ModelInsight {
                    choice_id,
                    runs: rs.len(),
                    success_rate: success(&rs),
                })
                .collect();
            models.sort_by(|a, b| b.runs.cmp(&a.runs).then(a.choice_id.cmp(&b.choice_id)));
            let centre = (f64::from(level.min) + f64::from(level.max)) / 2.0;
            let mut sorted = rows.clone();
            sorted.sort_by(|a, b| {
                (a.complexity - centre)
                    .abs()
                    .total_cmp(&(b.complexity - centre).abs())
                    .then(b.created_at.cmp(&a.created_at))
                    .then(a.run_id.cmp(&b.run_id))
            });
            let mut chosen: Vec<_> = sorted.iter().take(3).copied().collect();
            let mut edges: Vec<_> = sorted
                .iter()
                .copied()
                .filter(|r| {
                    borderline(level, r.complexity) && !chosen.iter().any(|x| x.run_id == r.run_id)
                })
                .collect();
            edges.sort_by(|a, b| {
                let dist = |r: &HistoryRecord| {
                    (r.complexity - f64::from(level.min))
                        .abs()
                        .min((r.complexity - f64::from(level.max)).abs())
                };
                dist(a)
                    .total_cmp(&dist(b))
                    .then(b.created_at.cmp(&a.created_at))
                    .then(a.run_id.cmp(&b.run_id))
            });
            chosen.extend(edges.into_iter().take(2));
            let mut examples: Vec<_> = chosen
                .iter()
                .map(|r| InsightExample {
                    run_id: Some(r.run_id.clone()),
                    eval_id: None,
                    excerpt: so_core::redact::prompt_excerpt(
                        r.prompt.as_deref().unwrap_or(""),
                        160,
                    ),
                    complexity: r.complexity,
                    outcome: Some(r.outcome()),
                    source: "history".into(),
                    borderline: borderline(level, r.complexity),
                })
                .collect();
            if rows.len() < 3 {
                let mut candidates: Vec<_> = eval
                    .iter()
                    .filter(|e| {
                        mapped(
                            levels,
                            (f64::from(e.expected_complexity.0)
                                + f64::from(e.expected_complexity.1))
                                / 2.0,
                        ) == Some(index)
                    })
                    .collect();
                candidates.sort_by(|a, b| a.id.cmp(&b.id));
                for e in candidates.into_iter().take(3 - rows.len()) {
                    let complexity = (f64::from(e.expected_complexity.0)
                        + f64::from(e.expected_complexity.1))
                        / 2.0;
                    examples.push(InsightExample {
                        run_id: None,
                        eval_id: Some(e.id.clone()),
                        excerpt: so_core::redact::prompt_excerpt(&e.task, 160),
                        complexity,
                        outcome: None,
                        source: "eval".into(),
                        borderline: borderline(level, complexity),
                    });
                }
            }
            LevelInsight {
                name: level.name.clone(),
                min: level.min,
                max: level.max,
                runs: rows.len(),
                complexity_min: values.first().copied(),
                complexity_median: median(values.clone()),
                complexity_max: values.last().copied(),
                buckets,
                success_rate: success(&rows),
                accepted_rate: rate(outcomes.accepted, counted),
                rejected_rate: rate(outcomes.rejected, counted),
                failed_rate: rate(outcomes.failed, counted),
                succeeded_rate: rate(outcomes.succeeded, counted),
                failover_rate,
                escalated_rate,
                outcomes,
                median_duration_ms: median(
                    rows.iter()
                        .filter_map(|r| {
                            Some((r.finished_at? - r.started_at?).num_milliseconds() as f64)
                        })
                        .collect(),
                ),
                models,
                examples,
            }
        })
        .collect()
}
/// Cases shipped with the router, including the holdout set.
pub fn bundled_eval_examples() -> Vec<EvalExample> {
    let cases = crate::eval::bundled_cases().into_iter().chain(
        crate::eval::parse_cases(include_str!("../evals/holdout.jsonl"))
            .expect("bundled holdout cases are valid"),
    );
    cases
        .filter_map(|c| {
            let [lo, hi] = c.expected_complexity?;
            Some(EvalExample {
                id: c.id,
                task: c.task,
                expected_complexity: (lo as u8, hi as u8),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, Utc};
    use so_core::RunState;

    fn level() -> Level {
        Level {
            name: "mid".into(),
            min: 38,
            max: 62,
            tier: None,
            models: vec![],
        }
    }
    fn rec(id: &str, complexity: f64) -> HistoryRecord {
        let now = Utc::now();
        HistoryRecord {
            run_id: id.into(),
            prompt: Some("task".into()),
            state: RunState::Succeeded,
            error_kind: None,
            choice_id: Some("model".into()),
            created_at: now,
            started_at: Some(now),
            finished_at: Some(now + Duration::milliseconds(100)),
            complexity,
            level: Some("old".into()),
            source: None,
            reviews_accepted: 0,
            reviews_rejected: 0,
            failovers: 0,
            escalations: 0,
        }
    }
    #[test]
    fn aggregates_and_current_mapping() {
        let mut records = vec![
            rec("a", 40.0),
            rec("b", 50.0),
            rec("c", 60.0),
            rec("d", 61.0),
        ];
        records[0].reviews_accepted = 1;
        records[1].reviews_rejected = 1;
        records[2].failovers = 1;
        records[3].state = RunState::Cancelled;
        let result = analyze(&[level()], &records, &[]);
        let x = &result[0];
        assert_eq!(x.runs, 4);
        assert_eq!(x.complexity_median, Some(55.0));
        assert_eq!(
            (
                x.outcomes.accepted,
                x.outcomes.rejected,
                x.outcomes.succeeded,
                x.outcomes.other
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(x.success_rate, Some(1.0 / 3.0));
        assert_eq!(x.median_duration_ms, Some(100.0));
        assert_eq!(x.models[0].success_rate, Some(1.0 / 3.0));
        assert_eq!(
            x.buckets
                .iter()
                .map(|b| (b.from, b.to, b.count))
                .collect::<Vec<_>>(),
            vec![(38, 47, 1), (48, 57, 1), (58, 62, 2)]
        );
        assert_eq!(
            analyze(&[level()], &records[..3], &[])[0].complexity_median,
            Some(50.0)
        );
        assert_eq!(result, analyze(&[level()], &records, &[]));
    }
    #[test]
    fn outcomes_and_model_rates_exclude_other_runs() {
        let mut accepted = rec("accepted", 50.0);
        accepted.reviews_accepted = 1;
        accepted.state = RunState::Failed;
        let mut rejected = rec("rejected", 50.0);
        rejected.reviews_accepted = 1;
        rejected.reviews_rejected = 1;
        let mut failed = rec("failed", 50.0);
        failed.state = RunState::Failed;
        failed.choice_id = Some("other".into());
        let mut other = rec("other", 50.0);
        other.state = RunState::Cancelled;
        other.escalations = 1;
        let result = analyze(&[level()], &[accepted, rejected, failed, other], &[]);
        let insight = &result[0];
        assert_eq!(
            (
                insight.outcomes.accepted,
                insight.outcomes.rejected,
                insight.outcomes.failed,
                insight.outcomes.other
            ),
            (1, 1, 1, 1)
        );
        assert_eq!(insight.success_rate, Some(1.0 / 3.0));
        assert_eq!(insight.escalated_rate, Some(0.0));
        assert_eq!(insight.models[0].runs, 3);
        assert_eq!(insight.models[0].success_rate, Some(0.5));
        assert_eq!(insight.models[1].success_rate, Some(0.0));
    }

    #[test]
    fn examples_are_redacted_bounded_and_eval_fills_thin_history() {
        let mut records = vec![
            rec("a", 50.0),
            rec("b", 51.0),
            rec("c", 49.0),
            rec("d", 38.0),
            rec("e", 62.0),
            rec("f", 39.0),
        ];
        records[0].prompt = Some(format!("sk-abcdef1234567890 {}", "x".repeat(300)));
        let eval = vec![EvalExample {
            id: "eval".into(),
            task: "eval task".into(),
            expected_complexity: (45, 55),
        }];
        let examples = &analyze(&[level()], &records, &eval)[0].examples;
        assert_eq!(examples.len(), 5);
        assert_eq!(examples[0].run_id.as_deref(), Some("a"));
        assert!(!examples[0].excerpt.contains("sk-abcdef1234567890"));
        assert!(examples[0].excerpt.chars().count() <= 160);
        assert!(examples[3..].iter().all(|e| e.borderline));
        assert!(examples.iter().all(|e| e.source == "history"));
        let examples = &analyze(&[level()], &records[..1], &eval)[0].examples;
        assert_eq!(examples.len(), 2);
        assert_eq!(examples[1].source, "eval");
        assert_eq!(examples[1].eval_id.as_deref(), Some("eval"));
        assert!(!bundled_eval_examples().is_empty());
    }
}
