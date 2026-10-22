//! Continuing a finished run (or one at the review gate) on its native session, for a
//! message or a rejection.

use std::time::Duration;

use serde_json::json;
use so_core::proto::UNAVAILABLE;
use so_core::{ErrorKind, EventType, RunState};
use so_store::{ResumeClaim, RunRow};

use std::path::Path;

use super::lifecycle::excerpt;
use super::{LockKind, R, RunManager, lock, rpc, worktree};

/// Longest a resume waits for the run's wrap-up to finish.
const WRAP_UP_WAIT: Duration = Duration::from_secs(15);

/// What became of a resume request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resumed {
    /// The run is running again on its native session.
    Started,
    /// No slot was free: the run is queued and resumes when one is.
    Queued,
    /// Someone else already moved the run.
    Skipped,
}

impl RunManager {
    /// Waits until a finished run's registry entry is gone, which `finish` removes only
    /// after its wrap-up (harness shutdown, worktree cleanup, session end) is complete.
    /// Reopening before that would let the wrap-up undo the resume.
    async fn wait_wrapped_up(&self, run_id: &str) -> R<()> {
        let mut changed = self.inner.finished.subscribe();
        let deadline = tokio::time::Instant::now() + WRAP_UP_WAIT;
        loop {
            if !lock(&self.inner.runs).contains_key(run_id) {
                return Ok(());
            }
            tokio::select! {
                _ = changed.changed() => {}
                () = tokio::time::sleep(Duration::from_millis(50)) => {}
                () = tokio::time::sleep_until(deadline) => {
                    return Err(rpc(UNAVAILABLE, "the run is still being wrapped up; try again"));
                }
            }
        }
    }

    /// Resumes `row`'s native session with `prompt`. The run claims a slot like any start:
    /// with room it is `running` at once, without it goes back to `queued` and the queue
    /// resumes it later. `msg_id` is the stored message that carries `prompt`, marked read
    /// once the resume is committed. `reason` is recorded on the state change and `actor`
    /// (see `ConnCtx::actor`) on `review_rejected`.
    ///
    /// The run's gate lock is taken first and held from the fresh read of the run through
    /// the claim and the events, so a reject of a run that is just entering the review
    /// gate waits until the gate has been announced, and two resumes cannot both claim.
    /// It is released before the process is launched: nothing after that point orders
    /// against the gate, and a launch can take long.
    pub(crate) async fn resume_finished(
        &self,
        row: &RunRow,
        prompt: &str,
        reason: &str,
        msg_id: Option<String>,
        actor: &'static str,
    ) -> R<Resumed> {
        let gate = self.lock_run(LockKind::Gate, &row.id).await;
        // The caller's snapshot may be stale: decide on the run as it is now.
        let id = row.id.clone();
        let fresh = self
            .db(move |s| s.get_run(&id))
            .await?
            .ok_or_else(|| rpc(so_core::proto::NOT_FOUND, "run not found"))?;
        let row = &fresh;
        if row.native_session_id.is_none() {
            return Err(rpc(UNAVAILABLE, "the run has no native session to resume"));
        }
        let account_name = row
            .account
            .clone()
            .ok_or_else(|| rpc(UNAVAILABLE, "the run has no account"))?;
        let account = self.load_account(&account_name).await?;
        if row.state.is_terminal() {
            self.wait_wrapped_up(&row.id).await?;
        }
        // Putting the worktree back, claiming, and undoing the former when the claim
        // loses are one step under the run's worktree lock: no other resumer can be
        // using the directory this call removes.
        let wt = self.lock_run(LockKind::Worktree, &row.id).await;
        // Make sure the run can work where it left off before it changes state: a run
        // that cannot be resumed stays as it is and the caller gets the error.
        let (dir, recreated) = match self.resume_cwd_checked(row).await {
            Ok(r) => r,
            Err(e) => {
                // Someone else may have resumed (or ended) the run meanwhile.
                let id = row.id.clone();
                let now = self.db(move |s| s.get_run(&id)).await?;
                return match now {
                    Some(r)
                        if matches!(
                            r.state,
                            RunState::Succeeded | RunState::Failed | RunState::PendingReview
                        ) =>
                    {
                        Err(rpc(UNAVAILABLE, e))
                    }
                    _ => Ok(Resumed::Skipped),
                };
            }
        };
        let (id, name, prompt_owned) = (row.id.clone(), account_name, prompt.to_string());
        let caps = (
            i64::from(account.cap),
            i64::from(self.config().limits.global_cap),
        );
        let claim = self
            .db(move |s| s.claim_resume(&id, &name, caps.0, caps.1, &prompt_owned))
            .await?;
        let (from, to) = match claim {
            ResumeClaim::Moved { from, to } => (from, to),
            ResumeClaim::Conflict(_) => {
                if recreated {
                    // Nobody will use the worktree this call put back.
                    if let Some(repo) = row.worktree_repo.clone() {
                        let _ = self
                            .blocking(move || worktree::remove_dir(Path::new(&repo), &dir))
                            .await;
                    }
                }
                return Ok(Resumed::Skipped);
            }
        };
        drop(wt);
        lock(&self.inner.runs).entry(row.id.clone()).or_default();
        if let Some(m) = msg_id {
            let _ = self
                .db(move |s| s.claim_messages(&[m], chrono::Utc::now()))
                .await;
        }
        if from == RunState::PendingReview {
            self.emit(
                &row.session_id,
                Some(&row.id),
                EventType::ReviewRejected,
                json!({"feedback": excerpt(prompt, 280), "via": reason, "actor": actor}),
            )
            .await;
        }
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::StateChanged,
            json!({"from": from, "to": to, "reason": reason}),
        )
        .await;
        drop(gate);
        if to == RunState::Queued {
            self.request_drain();
            return Ok(Resumed::Queued);
        }
        if let Err(why) = self.resume_run(row, prompt).await {
            self.fail_run(&row.id, ErrorKind::Other, &why).await;
            return Err(rpc(UNAVAILABLE, why));
        }
        Ok(Resumed::Started)
    }

    /// Where a resumed run works: its own worktree (recreated from its branch when the
    /// directory is gone), or its original directory when it has no worktree.
    pub(crate) async fn resume_cwd(&self, row: &RunRow) -> Result<std::path::PathBuf, String> {
        let _serial = self.lock_run(LockKind::Worktree, &row.id).await;
        self.resume_cwd_checked(row).await.map(|(dir, _)| dir)
    }

    /// Like [`RunManager::resume_cwd`] but without taking the run's worktree lock (the
    /// caller holds it); also says whether the worktree was recreated by this call.
    async fn resume_cwd_checked(&self, row: &RunRow) -> Result<(std::path::PathBuf, bool), String> {
        let worktree_run = row.isolation == so_core::Isolation::Worktree;
        if !worktree_run {
            let dir = std::path::PathBuf::from(row.cwd.clone().ok_or("run has no directory")?);
            let probe = dir.clone();
            return if self
                .blocking(move || probe.is_dir())
                .await
                .map_err(|e| e.message)?
            {
                Ok((dir, false))
            } else {
                Err(format!("the run's directory {} is gone", dir.display()))
            };
        }
        let path = std::path::PathBuf::from(
            row.worktree_path
                .clone()
                .ok_or("the run has no worktree to resume in")?,
        );
        let (repo, branch) = (
            row.worktree_repo.clone().map(std::path::PathBuf::from),
            row.branch.clone(),
        );
        let probe = path.clone();
        if self
            .blocking(move || probe.is_dir())
            .await
            .map_err(|e| e.message)?
        {
            return Ok((path, false));
        }
        let (Some(repo), Some(branch)) = (repo, branch) else {
            return Err("the run's worktree is gone and its branch is unknown".into());
        };
        let target = path.clone();
        let (b, base) = (branch.clone(), row.worktree_base.clone());
        self.blocking(move || worktree::recreate(&repo, &target, &b, base.as_deref()))
            .await
            .map_err(|e| e.message)??;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::WorktreeCreated,
            json!({"path": path, "branch": branch, "recreated": true}),
        )
        .await;
        Ok((path, true))
    }
}

#[cfg(test)]
mod tests {
    use super::super::testutil::{insert_run, manager};
    use super::*;

    /// A resume must not reopen a run whose wrap-up (registry entry) is still running,
    /// or the wrap-up would undo it.
    #[tokio::test]
    async fn resume_waits_for_the_wrap_up_of_a_finished_run() {
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Succeeded);
        let patch = so_store::RunPatch {
            native_session_id: Some(Some("native".into())),
            ..so_store::RunPatch::default()
        };
        m.store().patch_run(&row.id, &patch).unwrap();
        let row = m.store().get_run(&row.id).unwrap().unwrap();
        // `insert_run` leaves a registry entry: the run is still being wrapped up.
        assert!(lock(&m.inner.runs).contains_key(&row.id));
        let resume = {
            let (m, row) = (m.clone(), row.clone());
            tokio::spawn(async move {
                m.resume_finished(&row, "go", "message", None, "orchestrator")
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            m.store().get_run(&row.id).unwrap().unwrap().state,
            RunState::Succeeded,
            "reopened before the wrap-up was done"
        );
        assert!(!resume.is_finished());
        // The wrap-up finishes: the entry goes and the resume proceeds.
        lock(&m.inner.runs).remove(&row.id);
        m.inner.finished.send_modify(|n| *n += 1);
        let r = tokio::time::timeout(Duration::from_secs(5), resume)
            .await
            .unwrap()
            .unwrap();
        // (The test run has no harness to launch, so the resume itself fails.)
        assert!(r.is_err());
        assert_ne!(
            m.store().get_run(&row.id).unwrap().unwrap().state,
            RunState::Succeeded
        );
    }
}
