//! The queue: claim slots for queued runs, oldest first.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;
use so_core::{ErrorKind, EventType, RunState};
use so_store::StoreError;
use tokio::sync::Notify;

use super::{Inner, RunManager, lock};

/// Starts the background drainer. It holds only a weak reference, so it ends with the
/// manager.
pub(super) fn spawn_drainer(inner: &Arc<Inner>, wake: Arc<Notify>) {
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        loop {
            wake.notified().await;
            let Some(inner) = weak.upgrade() else { break };
            let mgr = RunManager { inner };
            // A panic must not end the drainer: run the drain on its own task.
            if let Err(e) = tokio::spawn(async move { mgr.drain_once().await }).await {
                tracing::error!(error = %e, "queue drain panicked");
            }
        }
    });
}

impl RunManager {
    /// Asks the drainer to look at the queue. Never blocks and never recurses, so it is
    /// safe to call from `finish`.
    pub(crate) fn request_drain(&self) {
        self.inner.drain_wake.notify_one();
    }

    /// Starts queued runs, oldest first, while slots are free. A run blocked by its account
    /// cap does not hold back runs of other accounts. Runs that cannot start at all are
    /// failed after the drain lock is released, because failing a run asks for another
    /// drain.
    pub(crate) async fn drain_once(&self) {
        let failures = {
            let _guard = self.inner.drain.lock().await;
            self.claim_queued().await
        };
        for (id, reason) in failures {
            self.fail_run(&id, ErrorKind::Other, &reason).await;
        }
    }

    async fn claim_queued(&self) -> Vec<(String, String)> {
        let mut failures = Vec::new();
        let queued = match self.db(|s| s.list_queued_runs(100_000)).await {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(error = %e.message, "could not list queued runs");
                return failures;
            }
        };
        let global_cap = i64::from(self.config().limits.global_cap);
        for run in queued {
            let Some(name) = run.account.clone() else {
                failures.push((run.id, "run has no account".to_string()));
                continue;
            };
            let account = match self.load_account(&name).await {
                Ok(a) if a.enabled => a,
                Ok(_) => {
                    failures.push((run.id, format!("account {name} is disabled")));
                    continue;
                }
                Err(e) => {
                    failures.push((run.id, e.message));
                    continue;
                }
            };
            let (id, cap) = (run.id.clone(), i64::from(account.cap));
            let claimed = self
                .db_raw(move |s| {
                    s.try_claim_slot(
                        &id,
                        RunState::Queued,
                        RunState::Starting,
                        &name,
                        cap,
                        global_cap,
                        Utc::now(),
                    )
                })
                .await;
            match claimed {
                Ok(true) => {
                    // Normally present from enqueue; make sure after any other path.
                    lock(&self.inner.runs).entry(run.id.clone()).or_default();
                    self.emit(
                        &run.session_id,
                        Some(&run.id),
                        EventType::StateChanged,
                        json!({"from": RunState::Queued, "to": RunState::Starting}),
                    )
                    .await;
                    let mgr = self.clone();
                    tokio::spawn(async move { mgr.start_run(run).await });
                }
                Ok(false) => {}
                // Cancelled while we were looking: nothing to start.
                Err(StoreError::StateConflict { .. }) => {
                    tracing::debug!(run_id = %run.id, "queued run changed state before its claim");
                }
                Err(e) => {
                    tracing::warn!(run_id = %run.id, error = %e, "could not claim a slot");
                }
            }
        }
        failures
    }
}
