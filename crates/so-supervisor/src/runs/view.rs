//! Read models (`get_run`, join command) and waiting on runs.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use so_core::proto::{JoinResult, RunDiff, RunView, UNAVAILABLE, UsageTotals};
use so_core::{Event, EventType};
use so_harness::{AnyDriver, Driver, JoinCommand, JoinInfo};
use so_store::RunRow;

use super::{R, RunManager, Subscription, lock, rpc};
use crate::dispatch::ConnCtx;

const RECENT_EVENTS: usize = 50;
const ALL_EVENTS: usize = 1_000_000;
/// Longest patch `get_run` returns with `diff`.
pub(crate) const DIFF_CAP: usize = 64 * 1024;
const POLL: Duration = Duration::from_secs(1);

/// What ends a wait besides the timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitMode {
    /// `spawn_agent` with `wait`: a run is done when it finished or reached the review
    /// gate (its caller is the one who accepts it).
    Finish,
    /// `wait_agent`: a run is done when it finished; progress, a question, an escalation
    /// or the review gate also end the wait.
    Update,
}

/// Result of waiting on runs.
#[derive(Debug, Default)]
pub struct WaitOutcome {
    pub finished: Vec<String>,
    pub updates: Vec<Event>,
    pub timed_out: bool,
}

impl RunManager {
    fn join_for(&self, row: &RunRow) -> Option<JoinCommand> {
        let live = lock(&self.inner.runs)
            .get(&row.id)
            .and_then(|rt| rt.live.as_ref().map(|l| l.join.clone()));
        let info = match live {
            Some(j) => j,
            None => JoinInfo {
                account_name: row.account.clone()?,
                native_session_id: row.native_session_id.clone()?,
                endpoint: None,
                cwd: row
                    .worktree_path
                    .as_ref()
                    .or(row.cwd.as_ref())
                    .map(PathBuf::from)?,
            },
        };
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg).ok()?;
        Some(driver.join_command(&info))
    }

    pub async fn run_view(&self, row: RunRow) -> R<RunView> {
        self.run_view_with(row, false).await
    }

    /// `get_run`; with `diff` also what the run changed in its worktree.
    pub async fn run_view_with(&self, row: RunRow, diff: bool) -> R<RunView> {
        self.run_view_capped(row, diff, DIFF_CAP).await
    }

    pub(crate) async fn run_view_capped(&self, row: RunRow, diff: bool, cap: usize) -> R<RunView> {
        let diff = if diff {
            self.run_diff(&row, cap).await?
        } else {
            None
        };
        let id = row.id.clone();
        let events = self
            .db(move |s| s.list_events_by_run(&id, ALL_EVENTS))
            .await?;
        let mut usage = UsageTotals::default();
        for e in events.iter().filter(|e| e.event_type == EventType::Usage) {
            usage.input_tokens += e.data["input_tokens"].as_u64().unwrap_or(0);
            usage.output_tokens += e.data["output_tokens"].as_u64().unwrap_or(0);
            usage.cost += e.data["cost"].as_f64().unwrap_or(0.0);
        }
        let skip = events.len().saturating_sub(RECENT_EVENTS);
        let recent_events = events.into_iter().skip(skip).collect();
        let join = self.join_for(&row);
        let rid = row.id.clone();
        let messages = self
            .db(move |s| s.list_messages_by_run(&rid, ALL_EVENTS))
            .await?
            .into_iter()
            .map(super::comm::message_view)
            .collect();
        Ok(RunView {
            run_id: row.id,
            session_id: row.session_id,
            state: row.state,
            harness: Some(row.harness),
            account: row.account,
            model: row.model,
            recent_events,
            result: row
                .result_json
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok()),
            usage,
            worktree: row.worktree_path,
            branch: row.branch,
            diff,
            join_command: join.as_ref().map(|j| display_argv(&j.argv)),
            join_cwd: join.as_ref().map(|j| j.cwd.to_string_lossy().into_owned()),
            join_env: join
                .as_ref()
                .map(|j| j.env.iter().map(|(k, _)| k.clone()).collect())
                .unwrap_or_default(),
            name: row.name,
            depth: u32::try_from(row.depth).unwrap_or(0),
            parent_run: row.parent_run_id,
            error_kind: row.error_kind,
            cwd: row.cwd,
            created_at: Some(row.created_at),
            finished_at: row.finished_at,
            messages,
        })
    }

    /// The diff of a run with a worktree branch; `None` for a run without one. A diff that
    /// cannot be made is reported in `error`, never as a failure of `get_run`.
    async fn run_diff(&self, row: &RunRow, cap: usize) -> R<Option<RunDiff>> {
        let Some(created) = crate::services::worktrees::created_of(row) else {
            return Ok(None);
        };
        let branch = created.branch.clone();
        let failed = |error: String| RunDiff {
            base: String::new(),
            branch: branch.clone(),
            stat: String::new(),
            patch: String::new(),
            truncated: false,
            error: Some(error),
        };
        let out = self
            .blocking(move || {
                if !super::worktree::branch_exists(&created.repo, &created.branch) {
                    return Err(format!(
                        "branch {} no longer exists (merged and removed, or pruned)",
                        created.branch
                    ));
                }
                super::worktree::diff(&created, cap)
            })
            .await;
        Ok(Some(match out {
            Ok(Ok(d)) => RunDiff {
                base: d.base,
                branch: branch.clone(),
                stat: d.stat,
                patch: d.patch,
                truncated: d.truncated,
                error: None,
            },
            Ok(Err(e)) => failed(e),
            Err(e) => failed(e.message),
        }))
    }

    /// The command to join a run, with its environment values. Records
    /// `join_command_issued`.
    pub async fn join_run(&self, row: RunRow) -> R<JoinResult> {
        let join = self
            .join_for(&row)
            .ok_or_else(|| rpc(UNAVAILABLE, "this run has no native session to join yet"))?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::JoinCommandIssued,
            json!({
                "argv": mask_argv(&join.argv, &join.env),
                "env": join.env.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            }),
        )
        .await;
        Ok(JoinResult {
            argv: join.argv,
            env: join.env,
            cwd: join.cwd.to_string_lossy().into_owned(),
        })
    }

    /// Final result of a finished run.
    pub fn result_of(row: &RunRow) -> Option<Value> {
        row.result_json
            .as_deref()
            .and_then(|r| serde_json::from_str(r).ok())
    }

    /// Which of `ids` are done: terminal in the store and fully wrapped up (no registry
    /// entry left). One blocking call for all of them.
    async fn done_among(&self, ids: &[String], mode: WaitMode) -> R<Vec<String>> {
        let list = ids.to_vec();
        let rows = self
            .db(move |s| {
                let mut out = Vec::new();
                for id in &list {
                    if let Some(r) = s.get_run(id)? {
                        out.push((r.id, r.state));
                    }
                }
                Ok(out)
            })
            .await?;
        let runs = lock(&self.inner.runs);
        Ok(rows
            .into_iter()
            .filter(|(id, st)| {
                (st.is_terminal() && !runs.contains_key(id))
                    || (mode == WaitMode::Finish && *st == so_core::RunState::PendingReview)
            })
            .map(|(id, _)| id)
            .collect())
    }

    /// The latest question of each listed run that still waits for the caller's answer,
    /// and the review-gate event of each listed run waiting for a decision.
    async fn standing_updates(&self, ctx: &ConnCtx, ids: &[String]) -> R<Vec<Event>> {
        let (session, caller, ids) = (ctx.session_id.clone(), ctx.run_id.clone(), ids.to_vec());
        let Some(session) = session else {
            return Ok(Vec::new());
        };
        self.db(move |s| {
            let asked: Vec<String> = s
                .unread_messages(&session, caller.as_deref(), 1000)?
                .into_iter()
                .filter(|m| m.kind == so_core::MessageKind::Question)
                .filter_map(|m| m.from_run)
                .filter(|r| ids.contains(r))
                .collect();
            let mut out = Vec::new();
            for id in &ids {
                let Some(run) = s.get_run(id)? else { continue };
                let wanted = if asked.contains(id) {
                    Some(EventType::QuestionAsked)
                } else if run.state == so_core::RunState::PendingReview {
                    Some(EventType::StateChanged)
                } else {
                    None
                };
                let Some(want) = wanted else { continue };
                // Newest of its type; for the review gate that is the move into it.
                out.extend(s.latest_event_of_type(id, want)?);
            }
            Ok(out)
        })
        .await
    }

    /// Waits until any of `ids` is done, `timeout` passes, or the client goes away. Events
    /// of those runs are pushed to the client as `event` notifications meanwhile.
    pub async fn wait_runs(
        &self,
        ctx: &ConnCtx,
        ids: &[String],
        sub: &mut Subscription,
        timeout: Option<Duration>,
        mode: WaitMode,
    ) -> R<WaitOutcome> {
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        let mut finished_rx = self.inner.finished.subscribe();
        let mut out = WaitOutcome::default();
        let mut open = true;
        if mode == WaitMode::Update {
            // Questions and reviews that were raised before this call still need an answer.
            out.updates = self.standing_updates(ctx, ids).await?;
        }
        loop {
            let done = self.done_among(ids, mode).await?;
            if !done.is_empty() {
                // Everything the run emitted is already queued: deliver it before returning.
                while let Ok(d) = sub.try_recv() {
                    push_event(ctx, ids, d.seq, d.event, &mut out).await;
                }
                out.finished = done;
                return Ok(out);
            }
            if mode == WaitMode::Update && !out.updates.is_empty() {
                while let Ok(d) = sub.try_recv() {
                    push_event(ctx, ids, d.seq, d.event, &mut out).await;
                }
                return Ok(out);
            }
            tokio::select! {
                d = sub.recv(), if open => match d {
                    Some(d) => push_event(ctx, ids, d.seq, d.event, &mut out).await,
                    None => open = false,
                },
                _ = finished_rx.changed() => {}
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending().await,
                    }
                } => {
                    out.timed_out = true;
                    return Ok(out);
                }
                () = ctx.closed() => return Err(rpc(UNAVAILABLE, "client disconnected")),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }
}

/// Events that end a `wait_agent`.
fn is_update(ev: &Event) -> bool {
    match ev.event_type {
        EventType::Progress | EventType::QuestionAsked | EventType::Escalated => true,
        EventType::StateChanged => ev.data["to"] == "pending_review",
        _ => false,
    }
}

async fn push_event(ctx: &ConnCtx, ids: &[String], seq: u64, ev: Event, out: &mut WaitOutcome) {
    if !ev.run_id.as_ref().is_some_and(|r| ids.contains(r)) {
        return;
    }
    // A standing update may also arrive live (it was stored after we subscribed).
    if out.updates.iter().any(|u| u.id == ev.id) {
        return;
    }
    let payload = super::events::Delivered {
        seq,
        event: ev.clone(),
    }
    .to_json();
    ctx.notifier.notify("event", payload).await;
    if is_update(&ev) {
        out.updates.push(ev);
    }
}

/// argv with the values of the command's environment and anything that looks like a key
/// replaced by `***`. Used for the audit event and the web API.
pub fn mask_argv(argv: &[String], env: &[(String, String)]) -> Vec<String> {
    argv.iter()
        .map(|a| {
            let mut out = a.clone();
            for (_, v) in env.iter().filter(|(_, v)| v.len() >= 4) {
                out = out.replace(v.as_str(), "***");
            }
            so_core::redact::redact(&out).replace("[REDACTED]", "***")
        })
        .collect()
}

/// argv for display: arguments with spaces or shell characters are single-quoted.
pub fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
            if plain {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_masking_hides_env_values_and_key_patterns() {
        let argv = [
            "acc",
            "--token",
            "hunter2-secret",
            "sk-abcdef1234567890",
            "plain",
        ]
        .map(String::from);
        let env = [("API".to_string(), "hunter2-secret".to_string())];
        assert_eq!(
            mask_argv(&argv, &env),
            ["acc", "--token", "***", "***", "plain"].map(String::from)
        );
    }

    #[test]
    fn argv_display_quotes_only_when_needed() {
        let a = |v: &[&str]| display_argv(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            a(&["claude-acc1", "--resume", "abc-123"]),
            "claude-acc1 --resume abc-123"
        );
        assert_eq!(a(&["x", "a b", "it's"]), "x 'a b' 'it'\\''s'");
    }
}
