//! A run's life: start, go live, finish and cancel.
//!
//! Everything that ends a run goes through [`RunManager::finish`], so cleanup happens
//! exactly once and a waiter never sees a run as done before it is wrapped up.

use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Isolation, RunState};
use so_harness::{
    AnyDriver, Driver, DriverError, LaunchSpec, McpServerSpec, RunControl, RunHandle,
};
use so_store::{FinishPatch, RunPatch, RunRow, StoreError};

use super::instructions::{MAX_DEPTH, child_instructions};
use super::pump::PumpCtx;
use super::worktree;
use super::{LiveRun, R, RunManager, SHUTDOWN_DEADLINE, lock};

enum Step {
    Won(Box<RunRow>),
    /// Already terminal (or gone): another caller ended it.
    Lost,
    /// A store error; try again.
    Retry(String),
}

/// The pool of the account that hit a limit and the reset time the harness reported.
#[derive(Debug, Clone, Default)]
pub(crate) struct Limit {
    pub pool: Option<String>,
    pub reset: Option<chrono::DateTime<Utc>>,
    /// Flag every pool of the account: an auth failure is the account's, not a pool's.
    pub all_pools: bool,
}

impl Limit {
    /// The limit `account` hit with `kind`: its first pool, or all of them for auth.
    pub(crate) fn for_account(
        account: &AccountFile,
        kind: ErrorKind,
        reset: Option<chrono::DateTime<Utc>>,
    ) -> Self {
        Self {
            pool: account.pools.first().cloned(),
            reset,
            all_pools: kind == ErrorKind::Auth,
        }
    }
}

/// A harness that did not come up, with the kind its output classifies as.
#[derive(Debug)]
pub(crate) struct LaunchFail {
    pub kind: ErrorKind,
    pub message: String,
}

impl LaunchFail {
    pub(crate) fn other(message: String) -> Self {
        Self {
            kind: ErrorKind::Other,
            message,
        }
    }
}

/// Lets `?` turn a launch failure into the plain message where only the text matters
/// (resuming a run, see `reconcile.rs`: those paths fail the run as `other`).
impl From<LaunchFail> for String {
    fn from(f: LaunchFail) -> String {
        f.message
    }
}

/// How a run ends.
pub(crate) struct Outcome {
    pub to: RunState,
    pub error_kind: Option<ErrorKind>,
    pub result_json: Option<String>,
    pub reason: Option<String>,
    /// Flag the run's account (quota, rate limit, auth) if this call ends the run and it
    /// was not cancelled.
    pub flag_account: bool,
    /// Which pool hit the limit and when the harness says it resets, for the flag.
    pub limit: Option<Limit>,
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
            limit: None,
            control: Some(control),
        }
    }

    pub(crate) fn failed(kind: ErrorKind, reason: &str, control: Option<RunControl>) -> Self {
        Self {
            to: RunState::Failed,
            error_kind: Some(kind),
            result_json: None,
            reason: Some(reason.to_string()),
            flag_account: kind.flags_account(),
            limit: None,
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
            limit: None,
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
        fork: bool,
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
        let mut spec = LaunchSpec {
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
            fork,
            agent: Default::default(),
        };
        if let Some(name) = row.agent.clone() {
            self.apply_agent(row, &name, &mut spec).await?;
        }
        Ok(spec)
    }

    /// Launches with a time limit, so a harness that never comes up fails the run.
    pub(crate) async fn launch_harness(
        &self,
        row: &RunRow,
        spec: LaunchSpec,
    ) -> Result<RunHandle, LaunchFail> {
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg)
            .map_err(|e| LaunchFail::other(e.to_string()))?;
        let limit = self.launch_timeout();
        let delay = self.launch_delay();
        let launch = async {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            driver.launch(spec).await
        };
        match tokio::time::timeout(limit, launch).await {
            Ok(Ok(h)) => Ok(h),
            Ok(Err(e)) => {
                // A harness that exits during startup says why in its output; a limit or
                // an auth error there must still move the run to another account.
                // Only the process's own output is classified: a spawn, io, websocket or
                // timeout error may merely mention a word like "quota" in a path.
                let kind = match &e {
                    DriverError::Exited { kind, .. } => *kind,
                    _ => ErrorKind::Other,
                };
                Err(LaunchFail {
                    kind,
                    message: format!("could not launch {}: {e}", row.harness),
                })
            }
            Err(_) => Err(LaunchFail::other(format!(
                "{} did not start within {:?}",
                row.harness, limit
            ))),
        }
    }

    /// The harness did not come up. A limit or auth error moves a routed run to another
    /// account (SPEC 8.5); anything else, or a run without that option, fails with the
    /// classified kind.
    async fn launch_failed(&self, row: &RunRow, account: &AccountFile, fail: LaunchFail) {
        let limit = Limit::for_account(account, fail.kind, None);
        // The failed launch is an attempt: failover counts it and never goes back to it.
        self.record_launch_attempt(&row.id, account).await;
        if self
            .begin_failover(&row.id, None, fail.kind, &fail.message, &limit)
            .await
        {
            return;
        }
        let mut o = Outcome::failed(fail.kind, &fail.message, None);
        o.limit = Some(limit);
        if !self.finish(&row.id, o).await {
            // The run ended first (cancelled): an attempt this failure opened is closed.
            self.close_open_attempt(&row.id, "failed").await;
        }
    }

    /// Opens an attempt for a harness that never came up, so it is counted, shown on the
    /// timeline and tried once. A run that was cancelled (or is over) meanwhile gets none:
    /// nothing would close it.
    pub(crate) async fn record_launch_attempt(&self, run_id: &str, account: &AccountFile) -> bool {
        if self.is_cancelling(run_id) {
            return false;
        }
        let id = run_id.to_string();
        let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
            return false;
        };
        if !row.state.is_active() {
            return false;
        }
        let (id, name, pool) = (
            run_id.to_string(),
            account.name.clone(),
            account.pools.first().cloned(),
        );
        if let Err(e) = self
            .db(move |s| s.start_attempt(&id, &name, pool.as_deref(), Utc::now()))
            .await
        {
            tracing::warn!(%run_id, error = %e.message, "could not record launch attempt");
            return false;
        }
        self.emit(
            &row.session_id,
            Some(run_id),
            EventType::AttemptStarted,
            json!({"account": account.name, "harness": account.harness, "resumed": false, "launch_failed": true}),
        )
        .await;
        true
    }

    /// Closes the run's open attempt, if any.
    pub(crate) async fn close_open_attempt(&self, run_id: &str, outcome: &'static str) {
        let id = run_id.to_string();
        let _ = self
            .db(move |s| {
                match s
                    .list_attempts(&id, 1000)?
                    .into_iter()
                    .rev()
                    .find(|a| a.ended_at.is_none())
                {
                    Some(a) => s.finish_attempt(&a.id, outcome, Utc::now()),
                    None => Ok(()),
                }
            })
            .await;
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
        // A finished run that found no free slot when it was sent a message waits in the
        // queue with the text to resume with.
        let resume_prompt = {
            let id = row.id.clone();
            self.db(move |s| s.take_resume_prompt(&id))
                .await
                .map_err(|e| e.message)?
        };
        // A run that failed over while every other account was full starts on the account
        // that freed a slot, in the working tree it already has.
        let failover = {
            let id = row.id.clone();
            let pending = self
                .db(move |s| s.take_failover_pending(&id))
                .await
                .map_err(|e| e.message)?;
            pending.and_then(|j| serde_json::from_str::<super::failover::Pending>(&j).ok())
        };
        let cwd = if resume_prompt.is_some() || failover.is_some() {
            self.resume_cwd(row).await?
        } else {
            let cwd = PathBuf::from(row.cwd.clone().ok_or("run has no working directory")?);
            if row.isolation == Isolation::Worktree {
                self.make_worktree(row, &cwd).await?
            } else {
                cwd
            }
        };
        let fork_from = match &resume_prompt {
            // Continue the run's own session, not a copy.
            Some(_) => row.native_session_id.clone(),
            None if failover.is_some() => None,
            None => {
                let id = row.id.clone();
                self.db(move |s| s.get_run_fork(&id))
                    .await
                    .map_err(|e| e.message)?
            }
        };
        let mut prompt = match (&resume_prompt, &failover) {
            (Some(p), _) => p.clone(),
            (None, Some(_)) => {
                let mut p = self.turn_prompt(row).await;
                p.push_str(super::failover::FAILOVER_NOTE);
                p
            }
            (None, None) => row.prompt.clone().unwrap_or_default(),
        };
        let early = self.start_messages(row).await;
        if !early.is_empty() {
            // Part of what the run was told, so a failover keeps it.
            let (id, t) = (row.id.clone(), early.trim_start().to_string());
            let _ = self.db(move |s| s.append_run_turn_prompt(&id, &t)).await;
        }
        prompt.push_str(&early);
        let fork = fork_from.is_some() && resume_prompt.is_none();
        let spec = self
            .build_spec(row, &account, cwd, prompt, fork_from, fork)
            .await?;
        let handle = match self.launch_harness(row, spec).await {
            Ok(h) => h,
            Err(fail) => {
                self.launch_failed(row, &account, fail).await;
                return Ok(());
            }
        };
        // Announced once the harness is up; the row is as the queue left it. A move made
        // while the run waited replaces the decision a pending failover carries.
        let moved = {
            let id = row.id.clone();
            self.db(move |s| s.take_queued_route_event(&id))
                .await
                .ok()
                .flatten()
                .and_then(|j| serde_json::from_str::<serde_json::Value>(&j).ok())
        };
        let id = row.id.clone();
        if let Ok(Some(now)) = self.db(move |s| s.get_run(&id)).await {
            match (failover, moved) {
                (Some(mut p), moved) => {
                    if let Some(d) = moved {
                        p.set_decision(d);
                    }
                    self.announce_failover(&now, &p).await;
                }
                (None, Some(d)) => {
                    self.emit(&now.session_id, Some(&now.id), EventType::RouteDecision, d)
                        .await;
                }
                (None, None) => {}
            }
        }
        self.go_live(row, &account, handle, false).await;
        Ok(())
    }

    /// Messages sent to the run while it was queued, ready to append to its prompt. They
    /// are marked read.
    async fn start_messages(&self, row: &RunRow) -> String {
        let msgs = self.claim_input(row).await;
        if msgs.is_empty() {
            return String::new();
        }
        let mut s = String::from("\n\nMessages sent to you before you started:\n");
        for m in msgs {
            s.push_str(&format!("- {}\n", m.body));
        }
        s
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
        let (created, rel, base_branch) = self
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
                let base_branch = worktree::current_branch(&info.toplevel);
                worktree::create(&info, &template, &prefix, &hint, &id)
                    .map(|c| (c, rel, base_branch))
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
            json!({"path": path, "branch": branch, "base": created.base, "repo": created.repo, "base_branch": base_branch}),
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
        self.wait_go_live_gate().await;
        let registered = {
            let mut runs = lock(&self.inner.runs);
            match runs.get_mut(&id) {
                Some(rt) if !rt.cancel_requested => {
                    rt.live = Some(LiveRun {
                        control: control.clone(),
                        join,
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
                self.db_raw(move |s| s.update_run_state(&id, RunState::Starting, RunState::Running))
                    .await
            };
            match moved {
                Ok(()) => {}
                // Finished (cancelled) while launching.
                Err(StoreError::StateConflict { .. } | StoreError::InvalidTransition { .. }) => {
                    self.abandon_launch(&id, &control).await;
                    return;
                }
                Err(e) => {
                    let reason = format!("could not record that the run started: {e}");
                    self.finish(
                        &id,
                        Outcome::failed(ErrorKind::Other, &reason, Some(control)),
                    )
                    .await;
                    return;
                }
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

        // Messages that arrived while the run had no process (during launch, or between a
        // death and its resume) are handed over now.
        self.flush_unread(row, &control).await;

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
            review: row.review,
        };
        tokio::spawn(self.clone().pump(pc, events, control));
    }

    async fn flush_unread(&self, row: &RunRow, control: &RunControl) {
        let batch = self.claim_input(row).await;
        self.hand_over(batch, |text| {
            let control = control.clone();
            async move { control.send(&text).await.map_err(|e| e.to_string()) }
        })
        .await;
    }

    /// Claims the run's unread input messages. The claim is atomic, so two flushes racing
    /// over the same messages each get a different share and none is delivered twice.
    pub(crate) async fn claim_input(&self, row: &RunRow) -> Vec<so_store::MessageRow> {
        let (sid, id) = (row.session_id.clone(), row.id.clone());
        // Only the kinds meant as input are read (in SQL, so the limit cannot cut them
        // off behind a child's progress messages), and all are claimed in one step.
        let kinds = [so_core::MessageKind::Message, so_core::MessageKind::Answer];
        let mut claimed = self
            .db(move |s| {
                let pending = s.unread_messages_of_kinds(&sid, Some(&id), &kinds, 100)?;
                let ids: Vec<String> = pending.iter().map(|m| m.id.clone()).collect();
                let won = s.claim_messages(&ids, Utc::now())?;
                Ok(pending
                    .into_iter()
                    .filter(|m| won.contains(&m.id))
                    .collect::<Vec<_>>())
            })
            .await
            .unwrap_or_default();
        // An answer to a blocked `ask_parent` belongs to that call. Checked after the
        // claim, so an `ask_parent` that began in between still gets it back.
        if self.is_asking(&row.id) {
            let back: Vec<String> = claimed
                .iter()
                .filter(|m| m.kind == so_core::MessageKind::Answer)
                .map(|m| m.id.clone())
                .collect();
            if !back.is_empty() {
                claimed.retain(|m| m.kind != so_core::MessageKind::Answer);
                self.unclaim(back).await;
            }
        }
        claimed
    }

    /// Sends claimed messages in order. At the first failure that message and the rest go
    /// back to unread, for the next flush.
    ///
    /// Two flushes running at once each hold a different share of the messages, so the
    /// order across them is not guaranteed; within one flush it is. In practice flushes
    /// are rare (a process start) and the messages are independent notes.
    pub(crate) async fn hand_over<F, Fut>(&self, batch: Vec<so_store::MessageRow>, mut send: F)
    where
        F: FnMut(String) -> Fut,
        Fut: std::future::Future<Output = Result<(), String>>,
    {
        for (i, m) in batch.iter().enumerate() {
            if let Err(e) = send(m.body.clone()).await {
                tracing::warn!(error = %e, "could not hand over a message; keeping it unread");
                let rest: Vec<String> = batch[i..].iter().map(|m| m.id.clone()).collect();
                let _ = self.db(move |s| s.mark_messages_unread(&rest)).await;
                return;
            }
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
        self.finish(run_id, Outcome::failed(kind, reason, None))
            .await;
    }

    pub(crate) async fn flag_account(&self, run_id: &str, reason: &str, limit: Option<&Limit>) {
        let id = run_id.to_string();
        let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
            return;
        };
        let Some(name) = row.account else { return };
        let Ok(account) = self.load_account(&name).await else {
            return;
        };
        let account_label = account.name.clone();
        // The pool that failed, else the account's first; every pool for an auth failure.
        let pool = limit
            .and_then(|l| l.pool.clone())
            .or_else(|| account.pools.first().cloned())
            .unwrap_or_else(|| "default".into());
        let pools: Vec<String> = if limit.is_some_and(|l| l.all_pools) && !account.pools.is_empty()
        {
            account.pools.clone()
        } else {
            vec![pool]
        };
        let reset = limit.and_then(|l| l.reset);
        let cooldown = self.config().usage.default_cooldown.as_duration();
        let reason = format!("run {run_id}: {}", excerpt(reason, 200));
        let r = self
            .db(move |s| {
                for pool in &pools {
                    so_usage::mark_exhausted(s, &name, pool, reset, cooldown, &reason, Utc::now())
                        .map_err(|e| StoreError::Constraint(e.to_string()))?;
                }
                Ok(())
            })
            .await;
        match r {
            Ok(()) => {
                let account_name = account_label;
                self.notify(
                    "account_exhausted",
                    &account_name,
                    &account_name,
                    "the account hit a limit and is in cooldown",
                );
            }
            Err(e) => tracing::warn!(%run_id, error = %e.message, "could not flag the account"),
        }
    }

    /// Ends a run. Returns `true` when this call made the transition; a run that is
    /// already terminal is left alone.
    ///
    /// Store errors are retried with backoff and then the run is force-failed. However it
    /// goes, the harness is shut down, a worktree nobody can use is cleaned up and the
    /// registry entry is dropped once the row is terminal (or gone).
    pub(crate) async fn finish(&self, run_id: &str, outcome: Outcome) -> bool {
        let cancelled = self.is_cancelling(run_id);
        let mut o = outcome;
        let mut step = Step::Retry(String::new());
        for attempt in 0..5u32 {
            step = self.transition(run_id, &o).await;
            if !matches!(step, Step::Retry(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50 << attempt)).await;
        }
        if let Step::Retry(why) = &step {
            tracing::error!(%run_id, %why, "could not record the outcome; failing the run");
            let reason = format!("could not record the outcome: {why}");
            o = Outcome::failed(ErrorKind::Other, &reason, o.control.take());
            o.flag_account = false;
            step = self.transition(run_id, &o).await;
        }
        let won = match step {
            Step::Won(row) => {
                self.wrap_up(&row, o, cancelled).await;
                true
            }
            Step::Lost | Step::Retry(_) => {
                if let Some(control) = &o.control {
                    let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
                }
                // The run is over (or unreadable); a worktree it left behind is still ours.
                self.cleanup_worktree(run_id).await;
                false
            }
        };
        let id = run_id.to_string();
        let settled = match self.db(move |s| s.get_run(&id)).await {
            Ok(row) => row.is_none_or(|r| r.state.is_terminal()),
            Err(_) => false,
        };
        if settled {
            lock(&self.inner.runs).remove(run_id);
        }
        self.inner.finished.send_modify(|n| *n += 1);
        if won {
            // A slot may be free somewhere: queued runs may look for it again at once.
            lock(&self.inner.reroute_at).clear();
            self.request_drain();
        }
        won
    }

    /// One try at moving the run to its final state.
    async fn transition(&self, run_id: &str, o: &Outcome) -> Step {
        let to = o.to;
        for _ in 0..8 {
            let id = run_id.to_string();
            let row = match self.db_raw(move |s| s.get_run(&id)).await {
                Ok(Some(row)) => row,
                Ok(None) => return Step::Lost,
                Err(e) => return Step::Retry(e.to_string()),
            };
            if row.state.is_terminal() {
                return Step::Lost;
            }
            let patch = FinishPatch {
                result_json: o.result_json.clone(),
                error_kind: o.error_kind,
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
                Ok(()) => return Step::Won(Box::new(row)),
                // Someone else moved the run: look again.
                Err(StoreError::StateConflict { .. }) => {}
                Err(e) => return Step::Retry(e.to_string()),
            }
        }
        Step::Retry("the run kept changing state".to_string())
    }

    /// Everything that follows a won transition.
    async fn wrap_up(&self, row: &RunRow, o: Outcome, cancelled: bool) {
        let run_id = row.id.as_str();
        let to = o.to;
        let sid = row.session_id.clone();
        // A queued failover or move that never started has nothing left to announce.
        {
            let id = run_id.to_string();
            let _ = self.db(move |s| s.clear_queued_pending(&id)).await;
        }
        self.emit(
            &sid,
            Some(run_id),
            EventType::StateChanged,
            json!({"from": row.state, "to": to, "reason": o.reason}),
        )
        .await;
        self.emit(
            &sid,
            Some(run_id),
            EventType::RunFinished,
            json!({
                "status": to,
                "error_kind": o.error_kind,
                "reason": o.reason,
                "result_ref": o.result_json.as_ref().map(|_| run_id),
            }),
        )
        .await;

        // The state is committed; now stop the harness, then tidy up after it.
        if let Some(control) = &o.control {
            let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
        }
        if o.flag_account && !cancelled {
            self.flag_account(run_id, o.reason.as_deref().unwrap_or(""), o.limit.as_ref())
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
        // A resume that raced this wrap-up must not lose its process or worktree.
        if !self.still_terminal(run_id).await {
            return;
        }
        if let Some(rt) = lock(&self.inner.runs).get_mut(run_id) {
            rt.live = None;
        }
        self.cleanup_worktree(run_id).await;
        self.maybe_end_session(&sid).await;
    }

    async fn still_terminal(&self, run_id: &str) -> bool {
        let id = run_id.to_string();
        matches!(self.db(move |s| s.get_run(&id)).await, Ok(Some(r)) if r.state.is_terminal())
    }

    pub(crate) async fn maybe_end_session(&self, session_id: &str) {
        if lock(&self.inner.connected).contains(session_id) {
            return;
        }
        let sid = session_id.to_string();
        let r = self
            .db(move |s| {
                let busy = s
                    .list_runs_by_session(&sid, 100_000)?
                    .iter()
                    .any(|r| r.state.is_active())
                    || s.session_has_running_workflow(&sid)?;
                if !busy {
                    s.end_session(&sid, "ended", Utc::now())?;
                }
                Ok(!busy)
            })
            .await;
        match r {
            Ok(true) => {
                self.forget_session_seq(session_id);
                self.notify(
                    "session_finished",
                    session_id,
                    "session",
                    "all of its runs have finished",
                );
            }
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

    #[tokio::test]
    async fn account_is_flagged_only_when_the_finish_wins() {
        use super::super::testutil::{insert_run, manager};
        let (m, _dir) = manager();
        let quota = || Outcome::failed(ErrorKind::Quota, "usage limit reached", None);

        // Already cancelled: the late quota error loses and flags nothing.
        let cancelled = insert_run(&m, RunState::Cancelled);
        assert!(!m.finish(&cancelled.id, quota()).await);
        assert!(m.store().get_flag("claude-t", "default").unwrap().is_none());

        // A running run is failed by it and the account is flagged.
        let running = insert_run(&m, RunState::Running);
        assert!(m.finish(&running.id, quota()).await);
        assert!(m.store().get_flag("claude-t", "default").unwrap().is_some());
        assert_eq!(m.live_count(), 0);
        assert!(!m.busy());
    }

    async fn queue_messages(m: &RunManager, row: &RunRow, bodies: &[&str]) {
        for b in bodies {
            m.store_message(
                &row.session_id,
                None,
                Some(&row.id),
                so_core::MessageKind::Message,
                b,
                false,
            )
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn concurrent_flushes_deliver_each_message_once() {
        use super::super::testutil::{insert_run, manager};
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Running);
        let bodies: Vec<String> = (0..20).map(|i| format!("note {i}")).collect();
        queue_messages(
            &m,
            &row,
            &bodies.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .await;
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut tasks = Vec::new();
        for _ in 0..4 {
            let (m, row, sent) = (m.clone(), row.clone(), sent.clone());
            tasks.push(tokio::spawn(async move {
                let batch = m.claim_input(&row).await;
                m.hand_over(batch, |text| {
                    let sent = sent.clone();
                    async move {
                        sent.lock().unwrap().push(text);
                        Ok(())
                    }
                })
                .await;
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        let mut got = sent.lock().unwrap().clone();
        got.sort();
        let mut want = bodies;
        want.sort();
        assert_eq!(got, want, "every message exactly once");
    }

    #[tokio::test]
    async fn a_failed_send_leaves_the_message_unread_for_the_next_flush() {
        use super::super::testutil::{insert_run, manager};
        let (m, _dir) = manager();
        let row = insert_run(&m, RunState::Running);
        queue_messages(&m, &row, &["first", "second", "third"]).await;
        let batch = m.claim_input(&row).await;
        assert_eq!(batch.len(), 3);
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = sent.clone();
        m.hand_over(batch, move |text| {
            let sent = s2.clone();
            async move {
                if text == "second" {
                    return Err("pipe closed".to_string());
                }
                sent.lock().unwrap().push(text);
                Ok(())
            }
        })
        .await;
        assert_eq!(*sent.lock().unwrap(), ["first"]);
        // The failed one and the rest are unread again, in order.
        let again = m.claim_input(&row).await;
        let bodies: Vec<&str> = again.iter().map(|m| m.body.as_str()).collect();
        assert_eq!(bodies, ["second", "third"]);
        assert!(m.claim_input(&row).await.is_empty());
    }

    #[tokio::test]
    async fn finish_removes_the_registry_entry_of_an_already_finished_run() {
        use super::super::testutil::{insert_run, manager};
        let (m, _dir) = manager();
        let done = insert_run(&m, RunState::Succeeded);
        assert!(m.busy());
        assert!(!m.finish(&done.id, Outcome::cancelled()).await);
        assert!(!m.busy());
    }
}
