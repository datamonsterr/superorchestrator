//! A run's life: start, go live, finish and cancel.
//!
//! Everything that ends a run goes through [`RunManager::finish`], so cleanup happens
//! exactly once and a waiter never sees a run as done before it is wrapped up.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Isolation, RunState};
use so_harness::{AnyDriver, Driver, LaunchSpec, McpServerSpec, RunControl, RunHandle};
use so_store::{FinishPatch, RunPatch, RunRow, StoreError};

use super::instructions::{MAX_DEPTH, child_instructions};
use super::pump::PumpCtx;
use super::worktree;
use super::{LAUNCH_TIMEOUT, LiveRun, R, RunManager, SHUTDOWN_DEADLINE, lock};

/// How a run ends.
pub(crate) struct Outcome {
    pub to: RunState,
    pub error_kind: Option<ErrorKind>,
    pub result_json: Option<String>,
    pub reason: Option<String>,
    /// Flag the run's account (quota, rate limit, auth) if this call ends the run and it
    /// was not cancelled.
    pub flag_account: bool,
    /// Harness to shut down once the state is committed, before the worktree is cleaned.
    pub control: Option<RunControl>,
}

impl Outcome {
    pub(crate) fn succeeded(result_json: Option<String>, control: RunControl) -> Self {
        Self {
            to: RunState::Succeeded,
            error_kind: None,
            result_json,
            reason: None,
            flag_account: false,
            control: Some(control),
        }
    }

    pub(crate) fn failed(kind: ErrorKind, reason: &str, control: Option<RunControl>) -> Self {
        Self {
            to: RunState::Failed,
            error_kind: Some(kind),
            result_json: None,
            reason: Some(reason.to_string()),
            flag_account: matches!(
                kind,
                ErrorKind::Quota | ErrorKind::RateLimit | ErrorKind::Auth
            ),
            control,
        }
    }

    fn cancelled() -> Self {
        Self {
            to: RunState::Cancelled,
            error_kind: None,
            result_json: None,
            reason: Some("cancelled".to_string()),
            flag_account: false,
            control: None,
        }
    }
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// At most `max` characters, ending in an ellipsis when cut.
pub(crate) fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

impl RunManager {
    pub(crate) fn is_cancelling(&self, run_id: &str) -> bool {
        lock(&self.inner.runs)
            .get(run_id)
            .is_some_and(|rt| rt.cancel_requested)
    }

    fn mcp_spec(&self, run_id: &str) -> McpServerSpec {
        McpServerSpec {
            command: self.inner.mcp_exe.clone(),
            args: vec!["mcp".to_string()],
            env: vec![
                ("SUPERORCHESTRATOR_RUN_ID".into(), run_id.to_string()),
                (
                    "SUPERORCHESTRATOR_SOCKET".into(),
                    self.inner.socket_path.to_string_lossy().into_owned(),
                ),
                (
                    "SUPERORCHESTRATOR_CONFIG_DIR".into(),
                    self.inner.paths.config_dir.to_string_lossy().into_owned(),
                ),
            ],
        }
    }

    /// Builds the launch spec for a run and creates its private run directory.
    pub(crate) async fn build_spec(
        &self,
        row: &RunRow,
        account: &AccountFile,
        cwd: PathBuf,
        prompt: String,
        resume: Option<String>,
    ) -> Result<LaunchSpec, String> {
        let launch = self.launch_for(account).map_err(|e| e.message)?;
        let run_dir = self.inner.paths.run_dir(&row.id);
        let dir = run_dir.clone();
        self.blocking(move || create_private_dir(&dir))
            .await
            .map_err(|e| e.message)?
            .map_err(|e| format!("could not create {}: {e}", run_dir.display()))?;
        let output_schema = row
            .output_schema
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        Ok(LaunchSpec {
            run_id: row.id.clone(),
            run_dir,
            cwd,
            account: account.clone(),
            launch,
            model: row.model.clone(),
            effort: row.effort.clone(),
            prompt,
            system_addendum: child_instructions(
                &row.id,
                u8::try_from(row.depth).unwrap_or(MAX_DEPTH),
                row.parent_run_id.as_deref(),
                output_schema.as_ref(),
            ),
            mcp: self.mcp_spec(&row.id),
            output_schema,
            max_turns: row.max_turns,
            resume,
        })
    }

    /// Launches with a time limit, so a harness that never comes up fails the run.
    pub(crate) async fn launch_harness(
        &self,
        row: &RunRow,
        spec: LaunchSpec,
    ) -> Result<RunHandle, String> {
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg)
            .map_err(|e| e.to_string())?;
        match tokio::time::timeout(LAUNCH_TIMEOUT, driver.launch(spec)).await {
            Ok(Ok(h)) => Ok(h),
            Ok(Err(e)) => Err(format!("could not launch {}: {e}", row.harness)),
            Err(_) => Err(format!(
                "{} did not start within {} s",
                row.harness,
                LAUNCH_TIMEOUT.as_secs()
            )),
        }
    }

    pub(crate) async fn start_run(&self, row: RunRow) {
        if let Err(reason) = self.try_start(&row).await {
            self.fail_run(&row.id, ErrorKind::Other, &reason).await;
        }
    }

    async fn try_start(&self, row: &RunRow) -> Result<(), String> {
        let account_name = row.account.clone().ok_or("run has no account")?;
        let account = self
            .load_account(&account_name)
            .await
            .map_err(|e| e.message)?;
        let still_starting = {
            let id = row.id.clone();
            self.db(move |s| s.get_run(&id))
                .await
                .map_err(|e| e.message)?
                .is_some_and(|r| r.state == RunState::Starting)
        };
        if !still_starting {
            return Ok(());
        }
        let mut cwd = PathBuf::from(row.cwd.clone().ok_or("run has no working directory")?);
        if row.isolation == Isolation::Worktree {
            cwd = self.make_worktree(row, &cwd).await?;
        }
        let spec = self
            .build_spec(
                row,
                &account,
                cwd,
                row.prompt.clone().unwrap_or_default(),
                None,
            )
            .await?;
        let handle = self.launch_harness(row, spec).await?;
        self.go_live(row, &account, handle, false).await;
        Ok(())
    }

    /// Creates the run's worktree and returns the directory the harness starts in.
    async fn make_worktree(&self, row: &RunRow, cwd: &Path) -> Result<PathBuf, String> {
        let template = self.config().worktrees.dir.clone();
        let template = match self.host() {
            Some(h) => h
                .expand(&template)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            None => template,
        };
        let prefix = self.config().worktrees.branch_prefix.clone();
        let hint = row
            .name
            .clone()
            .or_else(|| row.prompt.clone())
            .unwrap_or_default();
        let (id, cwd) = (row.id.clone(), cwd.to_path_buf());
        let (created, rel) = self
            .blocking(move || {
                let info = worktree::repo_info(&cwd)
                    .filter(|i| i.has_head)
                    .ok_or_else(|| {
                        format!(
                            "{} is no longer a git repository with commits",
                            cwd.display()
                        )
                    })?;
                let rel = cwd
                    .strip_prefix(&info.toplevel)
                    .unwrap_or(Path::new(""))
                    .to_path_buf();
                worktree::create(&info, &template, &prefix, &hint, &id).map(|c| (c, rel))
            })
            .await
            .map_err(|e| e.message)??;
        let (path, branch) = (
            created.path.to_string_lossy().into_owned(),
            created.branch.clone(),
        );
        // One patch, so cleanup never sees a path without its base and repository.
        let patch = RunPatch {
            worktree_path: Some(Some(path.clone())),
            branch: Some(Some(branch.clone())),
            worktree_base: Some(Some(created.base.clone())),
            worktree_repo: Some(Some(created.repo.to_string_lossy().into_owned())),
            ..RunPatch::default()
        };
        let id = row.id.clone();
        self.db(move |s| s.patch_run(&id, &patch))
            .await
            .map_err(|e| e.message)?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::WorktreeCreated,
            json!({"path": path, "branch": branch, "base": created.base, "repo": created.repo}),
        )
        .await;
        Ok(created.path.join(rel))
    }

    /// Records a launched process and starts the event pump.
    ///
    /// The process is registered before the first await, so a cancel that arrives while
    /// this runs either finds it (and shuts it down) or has already set `cancel_requested`
    /// (and it is shut down here).
    pub(crate) async fn go_live(
        &self,
        row: &RunRow,
        account: &AccountFile,
        handle: RunHandle,
        resumed: bool,
    ) {
        let RunHandle {
            events,
            control,
            native_session_id,
            join,
        } = handle;
        let id = row.id.clone();
        let registered = {
            let mut runs = lock(&self.inner.runs);
            match runs.get_mut(&id) {
                Some(rt) if !rt.cancel_requested => {
                    rt.live = Some(LiveRun {
                        control: control.clone(),
                        join,
                        task: None,
                    });
                    true
                }
                _ => false,
            }
        };
        if !registered {
            self.abandon_launch(&id, &control).await;
            return;
        }
        if !resumed {
            let moved = {
                let id = id.clone();
                self.db(move |s| s.update_run_state(&id, RunState::Starting, RunState::Running))
                    .await
            };
            if moved.is_err() {
                // Finished (cancelled) while launching.
                self.abandon_launch(&id, &control).await;
                return;
            }
        }
        if let Some(n) = native_session_id
            .clone()
            .filter(|n| row.native_session_id.as_ref() != Some(n))
        {
            let patch = RunPatch {
                native_session_id: Some(Some(n)),
                ..RunPatch::default()
            };
            let id = id.clone();
            let _ = self.db(move |s| s.patch_run(&id, &patch)).await;
        }
        let pool = account.pools.first().cloned();
        let attempt = {
            let (id, name, pool) = (id.clone(), account.name.clone(), pool.clone());
            self.db(move |s| s.start_attempt(&id, &name, pool.as_deref(), Utc::now()))
                .await
        };
        if let Err(e) = attempt {
            tracing::warn!(run_id = %id, error = %e.message, "could not record attempt");
        }
        let sid = &row.session_id;
        self.emit(
            sid,
            Some(&id),
            EventType::AttemptStarted,
            json!({"account": account.name, "harness": account.harness, "resumed": resumed}),
        )
        .await;
        if !resumed {
            self.emit(
                sid,
                Some(&id),
                EventType::StateChanged,
                json!({"from": RunState::Starting, "to": RunState::Running}),
            )
            .await;
        }
        self.emit(
            sid,
            Some(&id),
            EventType::RunStarted,
            json!({
                "native_session_id": native_session_id,
                "model": row.model,
                "effort": row.effort,
                "resumed": resumed,
            }),
        )
        .await;

        let validator = row
            .output_schema
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|s| jsonschema::validator_for(&s).ok());
        let pc = PumpCtx {
            run_id: id.clone(),
            session_id: row.session_id.clone(),
            harness: row.harness,
            account: account.clone(),
            pool: pool.unwrap_or_else(|| "default".into()),
            validator,
        };
        let task = tokio::spawn(self.clone().pump(pc, events, control));
        if let Some(live) = lock(&self.inner.runs)
            .get_mut(&id)
            .and_then(|rt| rt.live.as_mut())
        {
            live.task = Some(task);
        }
    }

    /// A launch whose run was finished meanwhile: stop the process, drop the entry and
    /// remove a worktree nobody used.
    async fn abandon_launch(&self, id: &str, control: &RunControl) {
        if let Some(rt) = lock(&self.inner.runs).get_mut(id) {
            rt.live = None;
        }
        let _ = control.shutdown(std::time::Duration::ZERO).await;
        self.cleanup_worktree(id).await;
    }

    // ---------------------------------------------------------------- finish

    pub(crate) async fn fail_run(&self, run_id: &str, kind: ErrorKind, reason: &str) {
        // TODO(failover): a quota or rate-limit failure should hand the run to the router
        // for another account instead of ending it.
        self.finish(run_id, Outcome::failed(kind, reason, None))
            .await;
    }

    async fn flag_account(&self, run_id: &str, reason: &str) {
        let id = run_id.to_string();
        let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
            return;
        };
        let Some(name) = row.account else { return };
        let Ok(account) = self.load_account(&name).await else {
            return;
        };
        let pool = account
            .pools
            .first()
            .cloned()
            .unwrap_or_else(|| "default".into());
        let cooldown = self.config().usage.default_cooldown.as_duration();
        let reason = format!("run {run_id}: {}", excerpt(reason, 200));
        let r = self
            .db(move |s| {
                so_usage::mark_exhausted(s, &name, &pool, None, cooldown, &reason, Utc::now())
                    .map(|_| ())
                    .map_err(|e| StoreError::Constraint(e.to_string()))
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(%run_id, error = %e.message, "could not flag the account");
        }
    }

    /// Ends a run. Returns `true` when this call made the transition; a run that is
    /// already terminal is left alone.
    pub(crate) async fn finish(&self, run_id: &str, outcome: Outcome) -> bool {
        let cancelled = self.is_cancelling(run_id);
        let won = self.finish_inner(run_id, outcome, cancelled).await;
        if won {
            lock(&self.inner.runs).remove(run_id);
        }
        self.inner.finished.send_modify(|n| *n += 1);
        if won {
            self.request_drain();
        }
        won
    }

    async fn finish_inner(&self, run_id: &str, o: Outcome, cancelled: bool) -> bool {
        let Outcome {
            to,
            error_kind,
            result_json,
            reason,
            flag_account,
            control,
        } = o;
        let mut transition = None;
        for _ in 0..8 {
            let id = run_id.to_string();
            let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
                return false;
            };
            if row.state.is_terminal() {
                return false;
            }
            let patch = FinishPatch {
                result_json: result_json.clone(),
                error_kind,
                finished_at: Some(Utc::now()),
            };
            let (id, from) = (run_id.to_string(), row.state);
            // `succeeded` is reachable only from `running` and `pending_review`.
            let r = self
                .db_raw(move |s| {
                    if to == RunState::Succeeded
                        && matches!(
                            from,
                            RunState::Waiting | RunState::Stalled | RunState::Escalated
                        )
                    {
                        s.update_run_state(&id, from, RunState::Running)?;
                        return s.finish_run(&id, RunState::Running, to, &patch);
                    }
                    s.finish_run(&id, from, to, &patch)
                })
                .await;
            match r {
                Ok(()) => {
                    transition = Some(row);
                    break;
                }
                // Someone else moved the run: look again.
                Err(StoreError::StateConflict { .. }) => {}
                Err(e) => {
                    tracing::warn!(%run_id, error = %e, "could not finish run");
                    return false;
                }
            }
        }
        let Some(row) = transition else { return false };
        let sid = row.session_id.clone();
        self.emit(
            &sid,
            Some(run_id),
            EventType::StateChanged,
            json!({"from": row.state, "to": to, "reason": reason}),
        )
        .await;
        self.emit(
            &sid,
            Some(run_id),
            EventType::RunFinished,
            json!({
                "status": to,
                "error_kind": error_kind,
                "reason": reason,
                "result_ref": result_json.as_ref().map(|_| run_id),
            }),
        )
        .await;

        // The state is committed; now stop the harness, then tidy up after it.
        if let Some(control) = control {
            let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
        }
        if flag_account && !cancelled {
            self.flag_account(run_id, reason.as_deref().unwrap_or(""))
                .await;
        }
        let id = run_id.to_string();
        let outcome = to.as_str();
        let _ = self
            .db(move |s| {
                let open = s
                    .list_attempts(&id, 1000)?
                    .into_iter()
                    .rev()
                    .find(|a| a.ended_at.is_none());
                match open {
                    Some(a) => s.finish_attempt(&a.id, outcome, Utc::now()),
                    None => Ok(()),
                }
            })
            .await;
        if let Some(rt) = lock(&self.inner.runs).get_mut(run_id) {
            rt.live = None;
        }
        self.cleanup_worktree(run_id).await;
        self.maybe_end_session(&sid).await;
        true
    }

    async fn maybe_end_session(&self, session_id: &str) {
        if lock(&self.inner.connected).contains(session_id) {
            return;
        }
        let sid = session_id.to_string();
        let r = self
            .db(move |s| {
                let busy = s
                    .list_runs_by_session(&sid, 100_000)?
                    .iter()
                    .any(|r| r.state.is_active());
                if !busy {
                    s.end_session(&sid, "ended", Utc::now())?;
                }
                Ok(!busy)
            })
            .await;
        match r {
            Ok(true) => self.forget_session_seq(session_id),
            Ok(false) => {}
            Err(e) => tracing::warn!(%session_id, error = %e.message, "could not end session"),
        }
    }

    /// Removes the run's worktree and branch when the run made no commits and left the
    /// tree clean; keeps it otherwise. Reads everything from the run's row.
    pub(crate) async fn cleanup_worktree(&self, run_id: &str) {
        let id = run_id.to_string();
        let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
            return;
        };
        let (Some(path), Some(branch), Some(base), Some(repo)) = (
            row.worktree_path,
            row.branch,
            row.worktree_base,
            row.worktree_repo,
        ) else {
            return;
        };
        let created = worktree::Created {
            path: PathBuf::from(&path),
            branch: branch.clone(),
            base,
            repo: PathBuf::from(repo),
        };
        match self
            .blocking(move || worktree::remove_if_unchanged(&created))
            .await
        {
            Ok(Ok(())) => {
                self.emit(
                    &row.session_id,
                    Some(run_id),
                    EventType::WorktreeRemoved,
                    json!({"path": path, "branch": branch}),
                )
                .await;
            }
            Ok(Err(kept)) => tracing::debug!(%run_id, ?kept, "worktree kept"),
            Err(e) => tracing::warn!(%run_id, error = %e.message, "worktree cleanup failed"),
        }
    }

    // ---------------------------------------------------------------- cancel

    /// Cancels a run and all of its descendants, deepest first. Returns the ids that were
    /// cancelled by this call.
    pub async fn cancel(&self, run_id: &str) -> R<Vec<String>> {
        let id = run_id.to_string();
        let order = self
            .db(move |s| {
                let mut order = Vec::new();
                collect_subtree(s, &id, &mut order, 0)?;
                Ok(order)
            })
            .await?;
        let mgr = self.clone();
        // Detached so that a disconnecting caller cannot stop half way.
        tokio::spawn(async move {
            let mut done = Vec::new();
            for id in order {
                if mgr.cancel_one(&id).await {
                    done.push(id);
                }
            }
            done
        })
        .await
        .map_err(crate::dispatch::internal)
    }

    async fn cancel_one(&self, run_id: &str) -> bool {
        let id = run_id.to_string();
        match self.db(move |s| s.get_run(&id)).await {
            Ok(Some(r)) if r.state.is_active() => {}
            _ => return false,
        }
        // Flag first: a launch in flight sees it, and a live process is found below.
        let control = {
            let mut runs = lock(&self.inner.runs);
            runs.get_mut(run_id).and_then(|rt| {
                rt.cancel_requested = true;
                rt.live.as_ref().map(|l| l.control.clone())
            })
        };
        if let Some(control) = control {
            if let Err(e) = control.interrupt().await {
                tracing::debug!(%run_id, error = %e, "interrupt failed");
            }
            if let Err(e) = control.shutdown(SHUTDOWN_DEADLINE).await {
                tracing::warn!(%run_id, error = %e, "shutdown failed");
            }
        }
        self.finish(run_id, Outcome::cancelled()).await
    }
}

/// Post-order walk: children before their parent.
fn collect_subtree(
    s: &so_store::Store,
    id: &str,
    out: &mut Vec<String>,
    depth: u8,
) -> so_store::Result<()> {
    if depth <= MAX_DEPTH + 1 {
        for child in s.list_child_runs(id, 100_000)? {
            collect_subtree(s, &child.id, out, depth + 1)?;
        }
    }
    out.push(id.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_is_bounded() {
        assert_eq!(excerpt("short", 280), "short");
        let long = "é".repeat(500);
        let e = excerpt(&long, 280);
        assert_eq!(e.chars().count(), 280);
        assert!(e.ends_with('…'));
    }
}
