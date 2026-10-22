//! Taking over runs left active by a previous supervisor (SPEC 3.1, 7.6).

use std::path::PathBuf;

use so_core::{ErrorKind, RunState};
use so_harness::AnyDriver;
use so_store::{RunRow, Store};

use super::{RunManager, RunRt, lock};

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
        let resumable = row.native_session_id.is_some()
            && row.account.is_some()
            && AnyDriver::for_harness(row.harness, self.inner.driver_cfg).is_ok()
            && self
                .inner
                .store
                .list_attempts(&row.id, 10)
                .is_ok_and(|a| a.len() < 2);
        if !resumable {
            return false;
        }
        lock(&self.inner.runs).insert(row.id.clone(), RunRt::default());
        let (mgr, row) = (self.clone(), row.clone());
        rt.spawn(async move {
            if let Err(reason) = mgr.resume_run(&row).await {
                mgr.fail_run(&row.id, ErrorKind::Other, &reason).await;
            }
        });
        true
    }

    async fn resume_run(&self, row: &RunRow) -> Result<(), String> {
        let native = row.native_session_id.clone().ok_or("no native session")?;
        let name = row.account.clone().ok_or("run has no account")?;
        let account = self.load_account(&name).await.map_err(|e| e.message)?;
        let cwd = row
            .worktree_path
            .as_ref()
            .or(row.cwd.as_ref())
            .map(PathBuf::from)
            .ok_or("run has no working directory")?;
        let dir = cwd.clone();
        if !self
            .blocking(move || dir.is_dir())
            .await
            .map_err(|e| e.message)?
        {
            return Err(format!("working directory {} is gone", cwd.display()));
        }
        let spec = self
            .build_spec(row, &account, cwd, "Continue.".to_string(), Some(native))
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
