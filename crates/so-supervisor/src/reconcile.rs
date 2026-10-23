//! Startup reconciliation of runs left active by a previous supervisor (ADR-0001).

use so_core::{ErrorKind, Event, EventType, RunState, new_id};
use so_store::{FinishPatch, RunRow, Store};

/// Lets the run manager adopt or resume a run instead of failing it.
pub trait ReconcileHook: Send + Sync {
    /// Returns `true` when the run was taken over; `false` leaves it to be marked failed.
    fn reconcile(&self, store: &Store, run: &RunRow) -> bool;
}

/// Marks every active run the hook does not take over as `failed` (`error_kind = other`).
/// Returns the number of runs failed.
pub(crate) fn reconcile(store: &Store, hook: Option<&dyn ReconcileHook>) -> crate::Result<usize> {
    // Workflows run inside the supervisor: one left `running` died with it. Running it
    // again replays its cached steps (SPEC 11).
    match store.fail_orphaned_workflows(chrono::Utc::now()) {
        Ok(0) => {}
        Ok(n) => tracing::warn!(count = n, "orphaned workflow runs marked failed"),
        Err(e) => tracing::warn!(error = %e, "could not fail orphaned workflow runs"),
    }
    let mut failed = 0;
    for run in store.list_active_runs(100_000)? {
        if hook.is_some_and(|h| h.reconcile(store, &run)) {
            tracing::info!(run_id = %run.id, "run adopted on supervisor start");
            continue;
        }
        let now = chrono::Utc::now();
        let patch = FinishPatch {
            result_json: None,
            error_kind: Some(ErrorKind::Other),
            finished_at: Some(now),
        };
        if let Err(e) = store.finish_run(&run.id, run.state, RunState::Failed, &patch) {
            tracing::warn!(run_id = %run.id, error = %e, "could not fail orphaned run");
            continue;
        }
        let reason = "supervisor restarted; harness process is gone";
        let events = [
            (
                EventType::StateChanged,
                serde_json::json!({"from": run.state, "to": RunState::Failed, "reason": reason}),
            ),
            (
                EventType::RunFinished,
                serde_json::json!({"status": "failed", "error_kind": "other", "reason": reason}),
            ),
        ];
        for (event_type, data) in events {
            store.append_event(&Event {
                id: new_id(),
                session_id: run.session_id.clone(),
                run_id: Some(run.id.clone()),
                ts: now,
                event_type,
                data,
            })?;
        }
        tracing::warn!(run_id = %run.id, from = %run.state.as_str(), "orphaned run marked failed");
        failed += 1;
    }
    Ok(failed)
}

/// Remove Codex websocket capability-token files (not the per-run `rpc-token`) left by crashes, including directories with no DB row.
/// Run after reconciliation: adopted/resuming runs are still active and keep their token.
/// Keep transcripts and other run evidence; never traverse a symlinked run directory.
pub(crate) fn cleanup_stale_codex_ws_tokens(store: &Store, paths: &so_core::Paths) -> usize {
    // Snapshot all active IDs once. Never delete tokens if the DB cannot establish
    // which runs are active; cleanup must not prevent the supervisor from starting.
    let limit = usize::try_from(i64::MAX).unwrap_or(usize::MAX);
    let active: std::collections::HashSet<String> = match store.list_active_runs(limit) {
        Ok(runs) => runs.into_iter().map(|run| run.id).collect(),
        Err(e) => {
            tracing::warn!(error = %e, "skipping stale token cleanup: active runs unavailable");
            return 0;
        }
    };
    let entries = match std::fs::read_dir(paths.runs_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return 0,
        Err(e) => {
            tracing::warn!(error = %e, "could not read run directories for stale token cleanup");
            return 0;
        }
    };
    let mut removed = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!(error = %e, "could not read run directory entry during token cleanup");
                continue;
            }
        };
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => {}
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!(path = %entry.path().display(), error = %e, "could not inspect run directory during token cleanup");
                continue;
            }
        }
        if entry
            .file_name()
            .to_str()
            .is_some_and(|id| active.contains(id))
        {
            continue;
        }
        let token = entry.path().join("ws-token");
        match std::fs::remove_file(&token) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %token.display(), error = %e, "could not remove stale run capability file")
            }
        }
    }
    if removed > 0 {
        tracing::info!(removed, "removed stale run capability files on startup");
    }
    removed
}
