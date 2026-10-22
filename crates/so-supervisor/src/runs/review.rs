//! The review gate (SPEC 7.5): checks when a `review: true` run finishes, then
//! `accept_run` / `reject_run`.
//!
//! `cmd:` checks run a shell command chosen by the orchestrator (the `checks` of its
//! `spawn_agent` call; never task text or the run's output) with `sh -c` in the run's
//! directory, under a 10 minute limit. They run with the daemon's privileges, outside the
//! harness sandbox.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use so_core::proto::{AckResult, INVALID_REQUEST, RejectParams, RunRef};
use so_core::{EventType, MessageKind, RunState};
use so_harness::RunControl;
use so_store::{RunPatch, RunRow};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use super::lifecycle::{Outcome, excerpt};
use super::resume::Resumed;
use super::worktree;
use super::{LockKind, R, RunManager, SHUTDOWN_DEADLINE, lock, rpc};
use crate::dispatch::ConnCtx;

/// Longest a single check may run.
pub(crate) const CHECK_TIMEOUT: Duration = Duration::from_secs(600);
const OUTPUT_TAIL: usize = 600;
const GH_TIMEOUT: Duration = Duration::from_secs(60);

/// The result of one check.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CheckOutcome {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

impl CheckOutcome {
    fn pass(name: &str) -> Self {
        Self {
            name: name.into(),
            ok: true,
            detail: String::new(),
        }
    }

    fn fail(name: &str, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ok: false,
            detail: detail.into(),
        }
    }
}

/// Everything captured from a finished child process.
struct Ran {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

/// Most output kept per stream; the rest is read and dropped so the child never blocks.
const OUTPUT_CAP: usize = 64 * 1024;
/// How long output may keep arriving after the child itself has exited.
const PIPE_GRACE: Duration = Duration::from_millis(250);

/// Kills the child's whole process group when dropped: on a time limit and on a cancelled
/// check. After a clean exit it is disarmed when both pipes closed, so a background process
/// that detached itself (output redirected away) is left running; one that still holds
/// the pipes is killed.
struct GroupGuard(Option<u32>);

impl GroupGuard {
    fn kill(&self) {
        let pid = self.0.and_then(|p| i32::try_from(p).ok());
        if let Some(pid) = pid.and_then(rustix::process::Pid::from_raw) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

impl Drop for GroupGuard {
    fn drop(&mut self) {
        self.kill();
    }
}

async fn read_capped<R: tokio::io::AsyncRead + Unpin>(
    mut r: R,
    buf: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
) {
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                let mut b = buf.lock().unwrap_or_else(|e| e.into_inner());
                let room = OUTPUT_CAP.saturating_sub(b.len());
                b.extend_from_slice(&chunk[..n.min(room)]);
            }
        }
    }
}

/// Runs `cmd` in its own process group and waits for it, at most `limit`. Output is
/// capped. A background process that holds the pipes open does not delay the result: the
/// readers get a short grace after the child exits, then the group is killed. Dropping
/// the future kills the group too.
async fn run_capture(mut cmd: Command, limit: Duration) -> Result<Ran, String> {
    use std::sync::{Arc, Mutex};
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    let guard = GroupGuard(child.id());
    let out = child.stdout.take().ok_or("no stdout")?;
    let err = child.stderr.take().ok_or("no stderr")?;
    let (out_buf, err_buf) = (
        Arc::new(Mutex::new(Vec::new())),
        Arc::new(Mutex::new(Vec::new())),
    );
    let mut out_task = tokio::spawn(read_capped(out, out_buf.clone()));
    let mut err_task = tokio::spawn(read_capped(err, err_buf.clone()));
    let status = match tokio::time::timeout(limit, child.wait()).await {
        Ok(status) => status.map_err(|e| e.to_string()),
        Err(_) => {
            guard.kill();
            let _ = child.wait().await;
            out_task.abort();
            err_task.abort();
            return Err(format!("timed out after {}s", limit.as_secs()));
        }
    };
    let _ = tokio::time::timeout(PIPE_GRACE, async {
        let _ = (&mut out_task).await;
        let _ = (&mut err_task).await;
    })
    .await;
    // Both pipes closed: nothing is left holding them, so there is no group to clean up.
    let pipes_closed = out_task.is_finished() && err_task.is_finished();
    out_task.abort();
    err_task.abort();
    let mut guard = guard;
    if pipes_closed {
        guard.0 = None;
    }
    drop(guard);
    let text = |b: &Mutex<Vec<u8>>| {
        String::from_utf8_lossy(&b.lock().unwrap_or_else(|e| e.into_inner())).into_owned()
    };
    Ok(Ran {
        code: status?.code(),
        stdout: text(&out_buf),
        stderr: text(&err_buf),
    })
}

fn tail(text: &str) -> String {
    let t = text.trim();
    let n = t.chars().count();
    if n <= OUTPUT_TAIL {
        return t.to_string();
    }
    let s: String = t.chars().skip(n - OUTPUT_TAIL).collect();
    format!("…{s}")
}

async fn check_cmd(name: &str, cmd: &str, cwd: &Path, limit: Duration) -> CheckOutcome {
    let mut c = Command::new("sh");
    c.arg("-c").arg(cmd).current_dir(cwd);
    match run_capture(c, limit).await {
        Ok(r) if r.code == Some(0) => CheckOutcome::pass(name),
        Ok(r) => {
            let both = format!("{}\n{}", r.stdout, r.stderr);
            CheckOutcome::fail(
                name,
                format!(
                    "exited with {}: {}",
                    r.code.map_or("a signal".to_string(), |c| c.to_string()),
                    tail(&both)
                ),
            )
        }
        Err(e) => CheckOutcome::fail(name, e),
    }
}

async fn gh(args: &[&str], cwd: &Path) -> Result<Ran, String> {
    let mut c = Command::new("gh");
    c.args(args).current_dir(cwd);
    run_capture(c, GH_TIMEOUT)
        .await
        .map_err(|e| format!("gh {e}"))
}

/// The pull request number of `branch`.
async fn pr_number(branch: &str, cwd: &Path) -> Result<u64, String> {
    let r = gh(&["pr", "view", branch, "--json", "number"], cwd).await?;
    if r.code != Some(0) {
        return Err(format!(
            "no pull request for branch {branch}: {}",
            tail(&r.stderr)
        ));
    }
    parse_pr_number(&r.stdout)
}

pub(crate) fn parse_pr_number(json: &str) -> Result<u64, String> {
    serde_json::from_str::<Value>(json)
        .ok()
        .and_then(|v| v["number"].as_u64())
        .ok_or_else(|| "gh pr view returned no pull request number".to_string())
}

/// `gh pr checks --json state`: green when every check is SUCCESS, SKIPPED or NEUTRAL.
/// An empty list is not green: nothing has reported yet.
pub(crate) fn ci_verdict(json: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "gh pr checks gave no JSON")?;
    let checks = v.as_array().ok_or("gh pr checks gave no list")?;
    if checks.is_empty() {
        return Err("the pull request has no checks yet".into());
    }
    let bad: Vec<String> = checks
        .iter()
        .filter(|c| !matches!(c["state"].as_str(), Some("SUCCESS" | "SKIPPED" | "NEUTRAL")))
        .map(|c| {
            format!(
                "{} ({})",
                c["name"].as_str().unwrap_or("?"),
                c["state"].as_str().unwrap_or("?")
            )
        })
        .collect();
    if bad.is_empty() {
        Ok(())
    } else {
        Err(format!("checks not green: {}", bad.join(", ")))
    }
}

/// The GraphQL answer for review threads: no unresolved thread and no outstanding
/// `CHANGES_REQUESTED`.
pub(crate) fn threads_verdict(json: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(json).map_err(|_| "gh api gave no JSON")?;
    let pr = &v["data"]["repository"]["pullRequest"];
    if pr.is_null() {
        return Err("gh api returned no pull request".into());
    }
    if pr["reviewThreads"]["pageInfo"]["hasNextPage"] == json!(true) {
        return Err("more than 100 review threads; cannot verify them all".into());
    }
    let unresolved = pr["reviewThreads"]["nodes"].as_array().map_or(0, |n| {
        n.iter().filter(|t| t["isResolved"] != json!(true)).count()
    });
    let mut problems = Vec::new();
    if unresolved > 0 {
        problems.push(format!("{unresolved} unresolved review thread(s)"));
    }
    if pr["reviewDecision"].as_str() == Some("CHANGES_REQUESTED") {
        problems.push("changes requested".to_string());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join(", "))
    }
}

const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$number:Int!){repository(owner:$owner,name:$name){pullRequest(number:$number){reviewDecision reviewThreads(first:100){pageInfo{hasNextPage} nodes{isResolved}}}}}";

async fn check_ci(branch: Option<&str>, cwd: &Path) -> CheckOutcome {
    let Some(branch) = branch else {
        return CheckOutcome::fail("ci", "the run has no branch, so no pull request");
    };
    let number = match pr_number(branch, cwd).await {
        Ok(n) => n,
        Err(e) => return CheckOutcome::fail("ci", e),
    };
    // `gh pr checks` exits non-zero while checks are pending or failing but still prints.
    match gh(
        &["pr", "checks", &number.to_string(), "--json", "name,state"],
        cwd,
    )
    .await
    {
        Ok(r) => match ci_verdict(&r.stdout) {
            Ok(()) => CheckOutcome::pass("ci"),
            Err(e) => CheckOutcome::fail("ci", e),
        },
        Err(e) => CheckOutcome::fail("ci", e),
    }
}

async fn check_threads(branch: Option<&str>, cwd: &Path) -> CheckOutcome {
    let Some(branch) = branch else {
        return CheckOutcome::fail("pr_threads", "the run has no branch, so no pull request");
    };
    let number = match pr_number(branch, cwd).await {
        Ok(n) => n,
        Err(e) => return CheckOutcome::fail("pr_threads", e),
    };
    let number = number.to_string();
    let args = [
        "api",
        "graphql",
        "-F",
        "owner={owner}",
        "-F",
        "name={repo}",
        "-F",
        &format!("number={number}"),
        "-f",
        &format!("query={THREADS_QUERY}"),
    ];
    match gh(&args, cwd).await {
        Ok(r) if r.code == Some(0) => match threads_verdict(&r.stdout) {
            Ok(()) => CheckOutcome::pass("pr_threads"),
            Err(e) => CheckOutcome::fail("pr_threads", e),
        },
        Ok(r) => CheckOutcome::fail("pr_threads", tail(&r.stderr)),
        Err(e) => CheckOutcome::fail("pr_threads", e),
    }
}

/// Runs the checks in order, all of them.
pub(crate) async fn run_checks(
    checks: &[String],
    cwd: &Path,
    branch: Option<&str>,
) -> Vec<CheckOutcome> {
    let mut out = Vec::new();
    for c in checks {
        out.push(match c.as_str() {
            "ci" => check_ci(branch, cwd).await,
            "pr_threads" => check_threads(branch, cwd).await,
            other => match other.strip_prefix("cmd:") {
                Some(cmd) => check_cmd(other, cmd, cwd, CHECK_TIMEOUT).await,
                None => CheckOutcome::fail(other, "unknown check"),
            },
        });
    }
    out
}

/// What the pump should do after a run's final result.
pub(crate) enum Gate {
    /// The run went to `pending_review`; the pump is finished.
    Entered,
    /// The failed checks were sent to the child; keep going and check again on its next
    /// finished turn.
    Retry,
}

impl RunManager {
    /// Review gate for a `review: true` run that produced `result_json`. `failures` is how
    /// many times its checks already failed.
    pub(crate) async fn review_gate(
        &self,
        run_id: &str,
        control: &RunControl,
        result_json: Option<String>,
        failures: u32,
    ) -> Gate {
        let id = run_id.to_string();
        let loaded = self
            .db(move |s| Ok((s.get_run(&id)?, s.get_run_checks(&id)?)))
            .await;
        let Ok((Some(row), checks)) = loaded else {
            return self.enter_review(run_id, control, result_json, &[]).await;
        };
        let cwd = row
            .worktree_path
            .as_ref()
            .or(row.cwd.as_ref())
            .map(std::path::PathBuf::from);
        let outcomes = match cwd {
            Some(cwd) => {
                // A cancel stops the checks (dropping them kills their processes).
                tokio::select! {
                    o = run_checks(&checks, &cwd, row.branch.as_deref()) => o,
                    () = self.cancelled(run_id) => return Gate::Entered,
                }
            }
            None => checks
                .iter()
                .map(|c| CheckOutcome::fail(c, "the run has no working directory to check"))
                .collect(),
        };
        for o in &outcomes {
            self.emit(
                &row.session_id,
                Some(run_id),
                EventType::CheckResult,
                json!({"name": o.name, "ok": o.ok, "detail": excerpt(&o.detail, 300)}),
            )
            .await;
        }
        let failed: Vec<CheckOutcome> = outcomes.into_iter().filter(|o| !o.ok).collect();
        if failed.is_empty() {
            return self.enter_review(run_id, control, result_json, &[]).await;
        }
        if failures == 0 {
            let mut text = String::from(
                "Your work did not pass its review checks. Fix the problems, then finish again.\n",
            );
            for f in &failed {
                text.push_str(&format!("- {}: {}\n", f.name, f.detail));
            }
            if control.send(&text).await.is_ok() {
                let _ = self
                    .store_message(
                        &row.session_id,
                        row.parent_run_id.as_deref(),
                        Some(run_id),
                        MessageKind::Message,
                        &text,
                        true,
                    )
                    .await;
                return Gate::Retry;
            }
        }
        self.enter_review(run_id, control, result_json, &failed)
            .await
    }

    /// Resolves once a cancel of the run was requested.
    async fn cancelled(&self, run_id: &str) {
        while !self.is_cancelling(run_id) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// `running -> pending_review`. The harness process is stopped; its native session
    /// stays for `reject_run` to resume. The parent gets a `result` message.
    async fn enter_review(
        &self,
        run_id: &str,
        control: &RunControl,
        result_json: Option<String>,
        failed: &[CheckOutcome],
    ) -> Gate {
        // Take the process out of service first, while the run is still `running`. With no
        // live handle a message that arrives now stays unread and is handed to the next
        // process; once the state reads `pending_review` a reject may resume the run, and
        // a teardown still in flight would clear the new process's handle and attempt.
        if let Some(rt) = lock(&self.inner.runs).get_mut(run_id) {
            rt.live = None;
        }
        let _ = control.shutdown(SHUTDOWN_DEADLINE).await;
        let attempt = {
            let id = run_id.to_string();
            self.db(move |s| {
                let open = s
                    .list_attempts(&id, 1000)?
                    .into_iter()
                    .rev()
                    .find(|a| a.ended_at.is_none());
                if let Some(a) = &open {
                    s.finish_attempt(&a.id, "pending_review", chrono::Utc::now())?;
                }
                Ok(open.map(|a| a.id))
            })
            .await
            .ok()
            .flatten()
        };
        // From the state change to the message nothing may resume the run: a reject
        // waits on this lock, so its events come after these.
        let _gate = self.lock_run(LockKind::Gate, run_id).await;
        let id = run_id.to_string();
        let row = match self.db(move |s| s.get_run(&id)).await {
            Ok(Some(row)) => row,
            other => {
                // Without the row there is nothing to announce the gate with.
                let why = match other {
                    Err(e) => e.message,
                    _ => "the run is gone".to_string(),
                };
                tracing::error!(%run_id, %why, "cannot read the run at the review gate");
                self.relabel_attempt(attempt, "failed").await;
                let reason = format!("could not read the run at the review gate: {why}");
                self.fail_run(run_id, so_core::ErrorKind::Other, &reason)
                    .await;
                return Gate::Entered;
            }
        };
        let id = run_id.to_string();
        let moved = self
            .db_raw(move |s| s.update_run_state(&id, RunState::Running, RunState::PendingReview))
            .await;
        match moved {
            Ok(()) => {}
            Err(
                so_store::StoreError::StateConflict { .. }
                | so_store::StoreError::InvalidTransition { .. },
            ) => {
                // Someone moved the run first: a cancel, or a failure. Label the attempt
                // with what the run is now.
                let id = run_id.to_string();
                let now = self.db(move |s| s.get_run(&id)).await.ok().flatten();
                let label = now.map_or("lost", |r| r.state.as_str());
                self.relabel_attempt(attempt, label).await;
                return Gate::Entered;
            }
            Err(e) => {
                self.relabel_attempt(attempt, "failed").await;
                let reason = format!("could not record the review state: {e}");
                self.fail_run(run_id, so_core::ErrorKind::Other, &reason)
                    .await;
                return Gate::Entered;
            }
        }
        let delay = self.gate_delay();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        // The state is committed: whatever else goes wrong, the run is at the gate and
        // must be announced.
        let id = run_id.to_string();
        let patch = RunPatch {
            result_json: Some(result_json),
            ..RunPatch::default()
        };
        let faulty = self
            .inner
            .fault_gate_result
            .load(std::sync::atomic::Ordering::SeqCst);
        let stored = if faulty {
            Err(rpc(so_core::proto::INTERNAL_ERROR, "injected store fault"))
        } else {
            self.db(move |s| s.patch_run(&id, &patch)).await
        };
        if let Err(e) = stored {
            tracing::error!(%run_id, error = %e.message, "could not store the result at the review gate");
        }
        let names: Vec<&str> = failed.iter().map(|f| f.name.as_str()).collect();
        self.emit(
            &row.session_id,
            Some(run_id),
            EventType::StateChanged,
            json!({"from": RunState::Running, "to": RunState::PendingReview, "failed_checks": names}),
        )
        .await;
        self.inner.finished.send_modify(|n| *n += 1);
        // A pending review holds no slot: let the queue use it.
        self.request_drain();
        let body = if failed.is_empty() {
            format!("Run {run_id} is ready for review.")
        } else {
            let mut b = format!("Run {run_id} is ready for review, but checks failed:\n");
            for f in failed {
                b.push_str(&format!("- {}: {}\n", f.name, f.detail));
            }
            b
        };
        let _ = self
            .store_message(
                &row.session_id,
                Some(run_id),
                row.parent_run_id.as_deref(),
                MessageKind::Result,
                &body,
                false,
            )
            .await;
        Gate::Entered
    }

    /// Changes the outcome recorded on an attempt that `enter_review` already closed.
    async fn relabel_attempt(&self, attempt: Option<String>, outcome: &str) {
        if let Some(aid) = attempt {
            let outcome = outcome.to_string();
            let _ = self
                .db(move |s| s.finish_attempt(&aid, &outcome, chrono::Utc::now()))
                .await;
        }
    }

    async fn pending_review_run(&self, ctx: &ConnCtx, run: &str) -> R<RunRow> {
        let row = self.resolve_run(run).await?;
        if ctx.session_id.as_deref() != Some(row.session_id.as_str()) {
            return Err(rpc(
                so_core::proto::INVALID_PARAMS,
                "that run belongs to another session",
            ));
        }
        if row.state != RunState::PendingReview {
            return Err(rpc(
                INVALID_REQUEST,
                format!("the run is {}, not pending_review", row.state),
            ));
        }
        Ok(row)
    }

    /// `accept_run`: `pending_review -> succeeded`. The worktree is kept when the run made
    /// changes (see `cleanup_worktree`); `worktrees prune` removes it later.
    pub async fn accept_run(&self, ctx: &ConnCtx, r: RunRef) -> R<AckResult> {
        let row = self.pending_review_run(ctx, &r.run).await?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::ReviewAccepted,
            json!({"actor": ctx.actor()}),
        )
        .await;
        let outcome = Outcome {
            to: RunState::Succeeded,
            error_kind: None,
            result_json: None,
            reason: Some("review accepted".into()),
            flag_account: false,
            control: None,
        };
        if !self.finish(&row.id, outcome).await {
            return Err(rpc(INVALID_REQUEST, "the run changed state; try again"));
        }
        self.remove_accepted_worktree(&row.id).await;
        Ok(AckResult { ok: true })
    }

    /// `reject_run`: sends `feedback` on the run's native session (resuming it) and moves
    /// `pending_review -> running`.
    pub async fn reject_run(&self, ctx: &ConnCtx, p: RejectParams) -> R<AckResult> {
        if p.feedback.trim().is_empty() {
            return Err(rpc(
                so_core::proto::INVALID_PARAMS,
                "feedback must not be empty",
            ));
        }
        let row = self.pending_review_run(ctx, &p.run).await?;
        let _ = self
            .store_message(
                &row.session_id,
                ctx.run_id.as_deref(),
                Some(&row.id),
                MessageKind::Message,
                &p.feedback,
                true,
            )
            .await;
        match self
            .resume_finished(&row, &p.feedback, "review rejected", None, ctx.actor())
            .await?
        {
            Resumed::Skipped => Err(rpc(INVALID_REQUEST, "the run changed state; try again")),
            Resumed::Started | Resumed::Queued => Ok(AckResult { ok: true }),
        }
    }

    /// After an accepted review a kept worktree is removed once its branch is merged or
    /// pushed and nothing is left uncommitted (SPEC 7.3).
    async fn remove_accepted_worktree(&self, run_id: &str) {
        let id = run_id.to_string();
        let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
            return;
        };
        // A message may have resumed the run already; its worktree is in use again.
        if !row.state.is_terminal() {
            return;
        }
        let (Some(path), Some(branch), Some(base), Some(repo)) = (
            row.worktree_path.clone(),
            row.branch.clone(),
            row.worktree_base.clone(),
            row.worktree_repo.clone(),
        ) else {
            return;
        };
        let created = worktree::Created {
            path: std::path::PathBuf::from(&path),
            branch: branch.clone(),
            base,
            repo: std::path::PathBuf::from(repo),
        };
        let removed = self
            .blocking(move || {
                if !created.path.is_dir() || !worktree::merged_or_pushed(&created) {
                    return false;
                }
                // `remove` without force refuses a tree with local changes.
                worktree::remove(&created, false).is_ok()
            })
            .await
            .unwrap_or(false);
        if removed {
            self.emit(
                &row.session_id,
                Some(run_id),
                EventType::WorktreeRemoved,
                json!({"path": path, "branch": branch, "reason": "merged or pushed"}),
            )
            .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ci_green_only_when_every_check_is_ok() {
        let ok = r#"[{"name":"a","state":"SUCCESS"},{"name":"b","state":"SKIPPED"},{"name":"c","state":"NEUTRAL"}]"#;
        assert!(ci_verdict(ok).is_ok());
        let bad = r#"[{"name":"a","state":"SUCCESS"},{"name":"b","state":"FAILURE"},{"name":"c","state":"PENDING"}]"#;
        let e = ci_verdict(bad).unwrap_err();
        assert!(e.contains("b (FAILURE)") && e.contains("c (PENDING)") && !e.contains("a ("));
        assert!(ci_verdict("[]").is_err());
        assert!(ci_verdict("oops").is_err());
    }

    #[test]
    fn threads_need_resolution_and_no_changes_requested() {
        let t = |decision: &str, resolved: &[bool]| {
            let nodes: Vec<Value> = resolved.iter().map(|r| json!({"isResolved": r})).collect();
            json!({"data": {"repository": {"pullRequest": {
                "reviewDecision": decision, "reviewThreads": {"nodes": nodes}}}}})
            .to_string()
        };
        assert!(threads_verdict(&t("APPROVED", &[true, true])).is_ok());
        assert!(threads_verdict(&t("", &[])).is_ok());
        let e = threads_verdict(&t("CHANGES_REQUESTED", &[true, false])).unwrap_err();
        assert!(e.contains("1 unresolved") && e.contains("changes requested"));
        assert!(threads_verdict(r#"{"data":{"repository":{"pullRequest":null}}}"#).is_err());
        let more = json!({"data": {"repository": {"pullRequest": {"reviewDecision": "",
            "reviewThreads": {"pageInfo": {"hasNextPage": true}, "nodes": []}}}}});
        assert!(
            threads_verdict(&more.to_string())
                .unwrap_err()
                .contains("100")
        );
    }

    #[test]
    fn pr_number_parsing() {
        assert_eq!(parse_pr_number(r#"{"number": 42}"#), Ok(42));
        assert!(parse_pr_number("{}").is_err());
    }

    #[tokio::test]
    async fn a_background_process_holding_the_pipes_does_not_stall_a_check() {
        let dir = std::env::temp_dir();
        let started = std::time::Instant::now();
        let c = check_cmd(
            "cmd:bg",
            "sleep 30 & echo ok",
            &dir,
            Duration::from_secs(20),
        )
        .await;
        assert!(c.ok, "{c:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn a_detached_background_process_survives_a_clean_exit() {
        let dir = std::env::temp_dir();
        let marker = dir.join(format!("so-detached-{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // Output redirected away: the pipes close with the shell, so there is nothing to
        // clean up and the daemon-like helper keeps running.
        let cmd = format!(
            "(sleep 1; touch {}) >/dev/null 2>&1 </dev/null & echo ok",
            marker.display()
        );
        let c = check_cmd("cmd:detached", &cmd, &dir, Duration::from_secs(20)).await;
        assert!(c.ok, "{c:?}");
        for _ in 0..200 {
            if marker.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(marker.exists(), "the detached process was killed");
        let _ = std::fs::remove_file(&marker);
    }

    #[tokio::test]
    async fn a_timeout_kills_the_whole_process_group() {
        let dir = std::env::temp_dir();
        let pidfile = dir.join(format!("so-grandchild-{}", std::process::id()));
        let _ = std::fs::remove_file(&pidfile);
        let cmd = format!("sleep 30 & echo $! > {}; wait", pidfile.display());
        let t = check_cmd("cmd:slow", &cmd, &dir, Duration::from_millis(500)).await;
        assert!(!t.ok && t.detail.contains("timed out"));
        let pid: u32 = std::fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // The grandchild must go too; poll until it does.
        let p = rustix::process::Pid::from_raw(pid as i32).unwrap();
        let gone = || rustix::process::test_kill_process(p).is_err();
        for _ in 0..200 {
            if gone() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(gone(), "the grandchild survived the timeout");
        let _ = std::fs::remove_file(&pidfile);
    }

    #[tokio::test]
    async fn output_is_capped() {
        let dir = std::env::temp_dir();
        let c = check_cmd(
            "cmd:loud",
            "head -c 300000 /dev/zero | tr '\\0' x; exit 1",
            &dir,
            Duration::from_secs(20),
        )
        .await;
        assert!(!c.ok);
        assert!(c.detail.chars().count() < 1000, "{}", c.detail.len());
    }

    #[tokio::test]
    async fn cmd_checks_pass_fail_and_time_out() {
        let dir = std::env::temp_dir();
        let long = Duration::from_secs(5);
        assert!(check_cmd("cmd:true", "true", &dir, long).await.ok);
        let f = check_cmd("cmd:x", "echo boom >&2; exit 3", &dir, long).await;
        assert!(!f.ok && f.detail.contains("exited with 3") && f.detail.contains("boom"));
        let t = check_cmd("cmd:sleep", "sleep 5", &dir, Duration::from_millis(100)).await;
        assert!(!t.ok && t.detail.contains("timed out"));
    }

    #[tokio::test]
    async fn accepted_worktrees_are_only_removed_for_terminal_runs() {
        use super::super::testutil::{insert_run, manager};
        let (m, dir) = manager();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@e"])
                .args(args)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["commit", "-q", "--allow-empty", "-m", "init"]);
        let info = worktree::repo_info(&repo).unwrap();
        let created = worktree::create(
            &info,
            &dir.path().join("wt").to_string_lossy(),
            "so/",
            "x",
            "0123456789abcdef",
        )
        .unwrap();
        let row = insert_run(&m, RunState::Running);
        let patch = RunPatch {
            worktree_path: Some(Some(created.path.to_string_lossy().into_owned())),
            branch: Some(Some(created.branch.clone())),
            worktree_base: Some(Some(created.base.clone())),
            worktree_repo: Some(Some(created.repo.to_string_lossy().into_owned())),
            ..RunPatch::default()
        };
        m.store().patch_run(&row.id, &patch).unwrap();
        // Resumed meanwhile (running): the worktree is in use and stays.
        m.remove_accepted_worktree(&row.id).await;
        assert!(created.path.is_dir());
        // Terminal and merged (nothing was committed): removed.
        let finish = so_store::FinishPatch::default();
        m.store()
            .finish_run(&row.id, RunState::Running, RunState::Succeeded, &finish)
            .unwrap();
        m.remove_accepted_worktree(&row.id).await;
        assert!(!created.path.exists());
    }
}
