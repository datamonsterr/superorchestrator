//! Taking over runs left active by a previous supervisor (SPEC 3.1, 7.6).

use so_core::{ErrorKind, RunState};
use so_harness::AnyDriver;
use so_store::{RunRow, Store};

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;

use super::{Inner, RunManager, RunRt, lock};

/// A run to resume because its process died.
pub(super) struct ResumeRequest {
    row: RunRow,
    prompt: String,
}

/// Starts the task that resumes runs after their process died. It holds only a weak
/// reference, so it ends with the manager.
pub(super) fn spawn_resumer(inner: &Arc<Inner>, mut rx: UnboundedReceiver<ResumeRequest>) {
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let Some(inner) = weak.upgrade() else { break };
            let mgr = RunManager { inner };
            tokio::spawn(async move {
                if let Err(reason) = mgr.resume_run(&req.row, &req.prompt).await {
                    mgr.fail_run(&req.row.id, ErrorKind::Other, &reason).await;
                }
            });
        }
    });
}

impl RunManager {
    /// Takes over a run. Queued runs go back into the queue; started runs are resumed once
    /// from their native session when the harness has a driver (every driver supports
    /// `LaunchSpec::resume`). Returns `false` to leave the run to be failed.
    pub(crate) fn adopt(&self, row: &RunRow) -> bool {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        if row.state == RunState::Queued {
            lock(&self.inner.runs).insert(row.id.clone(), RunRt::default());
            self.request_drain();
            return true;
        }
        if row.state == RunState::PendingReview {
            // No process is expected at the review gate.
            lock(&self.inner.runs).insert(row.id.clone(), RunRt::default());
            return true;
        }
        let resumable = row.native_session_id.is_some()
            && row.account.is_some()
            && AnyDriver::for_harness(row.harness, self.inner.driver_cfg).is_ok()
            // One resume per death (SPEC 7.6): attempts that ended in a failover or a
            // review do not count.
            && self
                .inner
                .store
                .list_attempts(&row.id, 1000)
                .is_ok_and(|a| !a.iter().any(|x| x.outcome.as_deref() == Some("died")));
        if !resumable {
            return false;
        }
        lock(&self.inner.runs).insert(row.id.clone(), RunRt::default());
        let (mgr, row) = (self.clone(), row.clone());
        rt.spawn(async move {
            if let Err(reason) = mgr.resume_run(&row, "Continue.").await {
                mgr.fail_run(&row.id, ErrorKind::Other, &reason).await;
            }
        });
        true
    }

    /// Queues a resume of a run whose process died; a failure to resume fails the run.
    pub(crate) fn request_resume(&self, row: RunRow, prompt: &str) {
        let _ = self.inner.resume_tx.send(ResumeRequest {
            row,
            prompt: prompt.to_string(),
        });
    }

    /// Resumes the run's native session with `prompt` as the next user message and makes
    /// the new process the run's live one. The run must already be in an active state.
    /// See [`RunManager::resume_cwd`] for where it works.
    pub(crate) async fn resume_run(&self, row: &RunRow, prompt: &str) -> Result<(), String> {
        let native = row.native_session_id.clone().ok_or("no native session")?;
        let name = row.account.clone().ok_or("run has no account")?;
        let account = self.load_account(&name).await.map_err(|e| e.message)?;
        let cwd = self.resume_cwd(row).await?;
        self.renew_run_token(&row.id).await.map_err(|e| e.message)?;
        self.issue_run_token(&row.id).await.map_err(|e| e.message)?;
        let spec = self
            .build_spec(row, &account, cwd, prompt.to_string(), Some(native), false)
            .await?;
        let handle = self.launch_harness(row, spec).await?;
        self.go_live(row, &account, handle, true).await;
        Ok(())
    }
}

impl crate::server::ActivityProbe for RunManager {
    fn busy(&self) -> bool {
        RunManager::busy(self)
    }
}

impl crate::reconcile::ReconcileHook for RunManager {
    fn reconcile(&self, _store: &Store, run: &RunRow) -> bool {
        self.adopt(run)
    }
}
