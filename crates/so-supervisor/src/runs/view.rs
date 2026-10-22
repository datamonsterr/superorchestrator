//! Read models (`get_run`, join command) and waiting on runs.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use so_core::proto::{JoinResult, RunView, UNAVAILABLE, UsageTotals};
use so_core::{Event, EventType};
use so_harness::{AnyDriver, Driver, JoinCommand, JoinInfo};
use so_store::RunRow;

use super::{R, RunManager, Subscription, lock, rpc};
use crate::dispatch::ConnCtx;

const RECENT_EVENTS: usize = 50;
const ALL_EVENTS: usize = 1_000_000;
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
            json!({"argv": join.argv, "env": join.env.iter().map(|(k, _)| k).collect::<Vec<_>>()}),
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
                let latest = s
                    .list_events_by_run(id, 1_000_000)?
                    .into_iter()
                    .rev()
                    .find(|e| {
                        e.event_type == want && (want != EventType::StateChanged || is_update(e))
                    });
                out.extend(latest);
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
    fn argv_display_quotes_only_when_needed() {
        let a = |v: &[&str]| display_argv(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            a(&["claude-acc1", "--resume", "abc-123"]),
            "claude-acc1 --resume abc-123"
        );
        assert_eq!(a(&["x", "a b", "it's"]), "x 'a b' 'it'\\''s'");
    }
}
