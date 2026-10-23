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

/// Remove Codex capability files left by crashes, including directories with no DB row.
/// Run after reconciliation: adopted/resuming runs are still active and keep their token.
/// Keep transcripts and other run evidence; never traverse a symlinked run directory.
pub(crate) fn cleanup_stale_run_tokens(
    store: &Store,
    paths: &so_core::Paths,
) -> crate::Result<usize> {
    let entries = match std::fs::read_dir(paths.runs_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let mut removed = 0;
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        // Query each row rather than imposing an active-run list limit that could
        // accidentally classify a live run as stale. Non-UTF-8 names cannot be run IDs.
        if let Some(id) = name.to_str()
            && store.get_run(id)?.is_some_and(|run| run.state.is_active())
        {
            continue;
        }
        match std::fs::remove_file(entry.path().join("ws-token")) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    if removed > 0 {
        tracing::info!(removed, "removed stale run capability files on startup");
    }
    Ok(removed)
}
