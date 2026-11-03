//! Shared routing insights report for CLI and web clients.
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use so_core::Config;
use so_router::{
    Catalog,
    catalog::{ModelRef, unassigned_models},
    insights::{HistoryRecord, LevelInsight, analyze, bundled_eval_examples},
    suggest::{Suggestion, suggest},
};
use so_store::Store;

pub const HISTORY_LIMIT: usize = 5000;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct InsightsReport {
    pub generated_at: DateTime<Utc>,
    pub history_runs: usize,
    pub levels: Vec<LevelInsight>,
    pub suggestions: Vec<Suggestion>,
    pub unassigned_models: Vec<ModelRef>,
}

pub fn insights_report(
    store: &Store,
    config: &Config,
    catalog: &Catalog,
    now: DateTime<Utc>,
) -> so_store::Result<InsightsReport> {
    let records: Vec<_> = store
        .routing_history(HISTORY_LIMIT)?
        .into_iter()
        .filter_map(|row| HistoryRecord::try_from(row).ok())
        .collect();
    let levels = analyze(&config.router.levels, &records, &bundled_eval_examples());
    let suggestions = suggest(&config.router.levels, &levels, &records, catalog);
    Ok(InsightsReport {
        generated_at: now,
        history_runs: records.len(),
        levels,
        suggestions,
        unassigned_models: unassigned_models(catalog, &config.choices()),
    })
}

#[derive(Debug, thiserror::Error)]
#[error("unknown suggestion `{id}`; valid ids: {valid}")]
pub struct UnknownSuggestion {
    id: String,
    valid: String,
}

pub fn find_suggestion<'a>(
    report: &'a InsightsReport,
    id: &str,
) -> Result<&'a Suggestion, UnknownSuggestion> {
    report
        .suggestions
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| UnknownSuggestion {
            id: id.into(),
            valid: report
                .suggestions
                .iter()
                .map(|s| s.id.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::{Event, EventType, Harness, Isolation, Paths, RunState, SessionKind, new_id};
    use so_store::{RunRow, SessionRow};

    #[test]
    fn report_analyses_history_and_finds_suggestions() {
        let store = Store::open_in_memory().unwrap();
        let now = Utc::now();
        let session = SessionRow {
            id: new_id(),
            kind: SessionKind::Orchestrator,
            client: "test".into(),
            cwd: None,
            account: None,
            pid: None,
            started_at: now,
            ended_at: None,
            status: "active".into(),
        };
        store.insert_session(&session).unwrap();
        for _ in 0..5 {
            let run = RunRow {
                id: new_id(),
                session_id: session.id.clone(),
                parent_run_id: None,
                depth: 0,
                name: None,
                agent: None,
                choice_id: Some("codex/model".into()),
                harness: Harness::Codex,
                account: None,
                model: None,
                effort: None,
                cwd: None,
                worktree_path: None,
                branch: None,
                native_session_id: None,
                state: RunState::Failed,
                review: false,
                output_schema: None,
                result_json: None,
                error_kind: None,
                prompt: Some("fixture task".into()),
                isolation: Isolation::None,
                max_turns: None,
                worktree_base: None,
                worktree_repo: None,
                timeout_ms: None,
                created_at: now,
                started_at: None,
                finished_at: None,
            };
            store.insert_run(&run).unwrap();
            store
                .append_event(&Event {
                    id: new_id(),
                    session_id: session.id.clone(),
                    run_id: Some(run.id),
                    ts: now,
                    event_type: EventType::RouteDecision,
                    data: serde_json::json!({"complexity": 37}),
                })
                .unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::load(&Paths::from_dir(dir.path().to_path_buf())).unwrap();
        let report = insights_report(&store, &Config::default(), &catalog, now).unwrap();
        assert_eq!(report.generated_at, now);
        assert_eq!(report.history_runs, 5);
        assert_eq!(report.levels[0].runs, 5);
        let suggestion = report
            .suggestions
            .iter()
            .find(|s| s.id.starts_with("shift-boundary:"))
            .unwrap();
        assert_eq!(
            find_suggestion(&report, &suggestion.id).unwrap().id,
            suggestion.id
        );
        let error = find_suggestion(&report, "unknown").unwrap_err().to_string();
        assert!(error.contains(&suggestion.id), "{error}");
    }
}
