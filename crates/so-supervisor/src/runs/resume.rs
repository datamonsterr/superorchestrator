//! Continuing a finished run (or one at the review gate) on its native session, for a
//! message or a rejection.

use std::time::Duration;

use serde_json::json;
use so_core::proto::UNAVAILABLE;
use so_core::{ErrorKind, EventType, RunState};
use so_store::{ResumeClaim, RunRow};

use super::lifecycle::excerpt;
use super::{R, RunManager, lock, rpc, worktree};

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
    /// once the resume is committed. `reason` is recorded on the state change.
    pub(crate) async fn resume_finished(
        &self,
        row: &RunRow,
        prompt: &str,
        reason: &str,
        msg_id: Option<String>,
    ) -> R<Resumed> {
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
            ResumeClaim::Conflict(_) => return Ok(Resumed::Skipped),
        };
        lock(&self.inner.runs).entry(row.id.clone()).or_default();
        if let Some(m) = msg_id {
            let _ = self
                .db(move |s| s.mark_messages_read(&[m], chrono::Utc::now()))
                .await;
        }
        if from == RunState::PendingReview {
            self.emit(
                &row.session_id,
                Some(&row.id),
                EventType::ReviewRejected,
                json!({"feedback": excerpt(prompt, 280), "via": reason}),
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
        let worktree_run = row.isolation == so_core::Isolation::Worktree;
        if !worktree_run {
            let dir = std::path::PathBuf::from(row.cwd.clone().ok_or("run has no directory")?);
            let probe = dir.clone();
            return if self
                .blocking(move || probe.is_dir())
                .await
                .map_err(|e| e.message)?
            {
                Ok(dir)
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
            return Ok(path);
        }
        let (Some(repo), Some(branch)) = (repo, branch) else {
            return Err("the run's worktree is gone and its branch is unknown".into());
        };
        let target = path.clone();
        let b = branch.clone();
        self.blocking(move || worktree::recreate(&repo, &target, &b))
            .await
            .map_err(|e| e.message)??;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::WorktreeCreated,
            json!({"path": path, "branch": branch, "recreated": true}),
        )
        .await;
        Ok(path)
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
        let mut row = insert_run(&m, RunState::Succeeded);
        row.native_session_id = Some("native".into());
        // `insert_run` leaves a registry entry: the run is still being wrapped up.
        assert!(lock(&m.inner.runs).contains_key(&row.id));
        let resume = {
            let (m, row) = (m.clone(), row.clone());
            tokio::spawn(async move { m.resume_finished(&row, "go", "message", None).await })
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
