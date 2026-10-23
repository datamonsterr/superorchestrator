//! Failover (SPEC 8.5): a run that hits a quota, rate-limit or auth error before it has a
//! result moves to the next-ranked account, keeping its run id, worktree and cap claim.
//!
//! The failing account is marked exhausted (SPEC 6.3), the run is routed again within the
//! pins it was spawned under (never onto an account it already failed on), and a new
//! attempt starts there with the prompt of the run's current turn. When every other account
//! is full the run goes back to the queue with its failover pending and starts on the first
//! account that frees a slot. Runs whose account the caller named, and runs that were not
//! routed, fail as before.
//!
//! A native session lives in its account's credential home, so it is never portable to
//! another account: a failed-over run always starts fresh there (with a note saying that
//! part of the work may be in the working tree).

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Harness, RunState};
use so_harness::RunControl;
use so_store::{Reassign, RunRow};
use tokio::sync::mpsc::UnboundedReceiver;

use super::lifecycle::{LaunchFail, Limit, Outcome, excerpt};
use super::routing::{ChoiceFilter, RoutePins};
use super::{Inner, RunManager, SHUTDOWN_DEADLINE, lock};

/// Moves a run may make to another account. Each one costs a limit hit on a real account,
/// so a run that keeps failing stops.
/// A run is launched at most `1 + MAX_FAILOVERS` times (launch failures count).
pub(crate) const MAX_FAILOVERS: usize = 3;
/// Times the new account is looked for again when another run takes its last slot first.
const MAX_ROUTE_TRIES: usize = 3;

/// Appended to the prompt of a run that starts on another account.
pub(crate) const FAILOVER_NOTE: &str = "\n\nThis task was started on another account that ran out of \
    quota; the working tree may already hold part of the work. Check what is there before you \
    continue.";

/// A run to move because its account hit a limit.
pub(super) struct FailoverRequest {
    run_id: String,
    kind: ErrorKind,
    reason: String,
    /// The live process to stop; none when the harness never came up.
    control: Option<RunControl>,
    limit: Limit,
}

/// A failover that has been decided and is announced once the run is launched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Pending {
    from: String,
    from_harness: Harness,
    reason: String,
    error_kind: ErrorKind,
    /// The `route_decision` event data of the move.
    decision: Value,
}

impl Pending {
    /// Replaces the route decision announced with the failover.
    pub(crate) fn set_decision(&mut self, decision: Value) {
        self.decision = decision;
    }
}

/// What came of looking for another account.
enum Moved {
    /// The run ended meanwhile.
    Gone,
    /// The run holds a slot on its new account and is to be started there.
    Started(Box<Next>),
    /// Every candidate is full: the run waits in the queue.
    Queued,
}

/// Where a failed-over run goes.
struct Next {
    account: AccountFile,
    pending: Pending,
}

/// Starts the task that fails runs over. It holds only a weak reference, so it ends with
/// the manager.
pub(super) fn spawn_failover(inner: &Arc<Inner>, mut rx: UnboundedReceiver<FailoverRequest>) {
    let weak = Arc::downgrade(inner);
    tokio::spawn(async move {
        while let Some(req) = rx.recv().await {
            let Some(inner) = weak.upgrade() else { break };
            let mgr = RunManager { inner };
            tokio::spawn(async move { mgr.failover(req).await });
        }
    });
}

fn failover_kind(kind: ErrorKind) -> bool {
    matches!(
        kind,
        ErrorKind::Quota | ErrorKind::RateLimit | ErrorKind::Auth | ErrorKind::Config
    )
}

/// The tool controls of an agent that `harness` cannot enforce (SPEC 10: gaps).
fn tool_gaps(def: &so_agents::AgentDef, harness: Harness) -> bool {
    (!def.tools.is_empty() || !def.disallowed_tools.is_empty())
        && so_agents::gaps(def, harness)
            .iter()
            .any(|g| g.starts_with("tools") || g.starts_with("disallowedTools"))
}

impl RunManager {
    /// Called by a run's pump when the run failed with `kind`. Returns `true` when a
    /// failover has been started: the pump must then leave the run alone. `false` means the
    /// run is to fail now (its account was named, it was not routed, or it has used up its
    /// failovers).
    pub(crate) async fn begin_failover(
        &self,
        run_id: &str,
        control: Option<&RunControl>,
        kind: ErrorKind,
        reason: &str,
        limit: &Limit,
    ) -> bool {
        if !failover_kind(kind) || self.is_cancelling(run_id) {
            return false;
        }
        let id = run_id.to_string();
        let Ok((pins, attempts)) = self
            .db(move |s| Ok((s.get_run_route(&id)?, s.list_attempts(&id, 1000)?)))
            .await
        else {
            return false;
        };
        let pins = pins.and_then(|j| serde_json::from_str::<RoutePins>(&j).ok());
        // A bad model the caller pinned (or a harness pin with no choice behind it) is the
        // same on every account: fail now with the real error.
        // A pins record that is missing or unreadable counts as pinned, so a Config failure
        // fails fast rather than failing over blind.
        if kind == ErrorKind::Config
            && (pins.as_ref().is_none_or(|p| p.model.is_some()) || self.run_is_pinned(run_id).await)
        {
            return false;
        }
        let routed = pins.is_some_and(|p| p.account.is_none());
        let moves = attempts
            .iter()
            .filter(|a| a.outcome.as_deref() == Some("failover"))
            .count();
        if !routed || moves >= MAX_FAILOVERS {
            return false;
        }
        self.inner
            .failover_tx
            .send(FailoverRequest {
                run_id: run_id.to_string(),
                kind,
                reason: reason.to_string(),
                control: control.cloned(),
                limit: limit.clone(),
            })
            .is_ok()
    }

    /// The run was routed to a pin, not to one of the configured choices.
    async fn run_is_pinned(&self, run_id: &str) -> bool {
        let id = run_id.to_string();
        matches!(
            self.db(move |s| s.get_run(&id)).await,
            Ok(Some(r)) if r.choice_id.as_deref().is_some_and(so_core::is_pinned_choice)
        )
    }

    async fn failover(&self, req: FailoverRequest) {
        let run_id = req.run_id.clone();
        let (kind, why) = match self.move_to_next_account(&req).await {
            Ok(Moved::Started(next)) => {
                let account = next.account.clone();
                match self.start_on(&req, *next).await {
                    Ok(()) => return,
                    // The new account's harness did not come up and said why: a limit or
                    // auth error there moves the run on again (within MAX_FAILOVERS).
                    Err(fail) => {
                        let limit = Limit::for_account(&account, fail.kind, None);
                        // The failed launch is an attempt, so it counts and is not retried.
                        self.record_launch_attempt(&run_id, &account).await;
                        if self
                            .begin_failover(&run_id, None, fail.kind, &fail.message, &limit)
                            .await
                        {
                            return;
                        }
                        // The bound is reached (or the run cannot move): the account that
                        // failed here was not flagged by a move, so flag it now.
                        if fail.kind.flags_account() {
                            self.flag_account(&run_id, &fail.message, Some(&limit))
                                .await;
                        }
                        (fail.kind, fail.message)
                    }
                }
            }
            Ok(Moved::Queued) => {
                self.request_drain();
                return;
            }
            Ok(Moved::Gone) => return,
            Err(why) => (req.kind, why),
        };
        let reason = format!("{why}; last error: {}", excerpt(&req.reason, 200));
        let mut o = Outcome::failed(kind, &reason, None);
        // The account was marked when the failover began; flagging it again would only
        // restart its cooldown.
        o.flag_account = false;
        if !self.finish(&run_id, o).await {
            self.close_open_attempt(&run_id, "failed").await;
        }
    }

    /// The prompt of the run's current turn: what a failover must send again.
    pub(crate) async fn turn_prompt(&self, row: &RunRow) -> String {
        let id = row.id.clone();
        self.db(move |s| s.get_run_turn_prompt(&id))
            .await
            .ok()
            .flatten()
            .or_else(|| row.prompt.clone())
            .unwrap_or_default()
    }

    /// Accounts a failed-over agent run must not go to: those of a harness that cannot
    /// enforce the agent's tool controls, unless it is the harness the run is on (SPEC 10).
    /// With `tool_controls` (the agent had them when it was spawned) this fails closed:
    /// when the agent file cannot be read again, every other harness is ineligible.
    pub(crate) async fn tool_ineligible_accounts(
        &self,
        row: &RunRow,
        tool_controls: bool,
    ) -> Vec<String> {
        let Some(name) = row.agent.clone() else {
            return Vec::new();
        };
        let (cwd, paths) = (
            row.cwd.clone().map(std::path::PathBuf::from),
            self.inner.paths.clone(),
        );
        let on = row.harness;
        self.blocking(move || {
            let all = AccountFile::list(&paths).unwrap_or_default();
            let def = cwd.and_then(|c| so_agents::resolve_for_spawn(&c, &paths, &name).ok());
            let others = all.into_iter().filter(|a| a.harness != on);
            match def {
                Some(def) => others
                    .filter(|a| tool_gaps(&def, a.harness))
                    .map(|a| a.name)
                    .collect(),
                None if tool_controls => others.map(|a| a.name).collect(),
                None => Vec::new(),
            }
        })
        .await
        .unwrap_or_default()
    }

    /// Marks the failing account, stops its process and takes a slot on the next account
    /// (or queues the run when all are full).
    async fn move_to_next_account(&self, req: &FailoverRequest) -> Result<Moved, String> {
        let id = req.run_id.clone();
        let (row, route, attempts) = {
            let id2 = id.clone();
            self.db(move |s| {
                Ok((
                    s.get_run(&id2)?,
                    s.get_run_route(&id2)?,
                    s.list_attempts(&id2, 1000)?,
                ))
            })
            .await
            .map_err(|e| e.message)?
        };
        let row = row.ok_or("the run is gone")?;
        // Only a run with a process of its own moves: anything else is already done with.
        let working = matches!(
            row.state,
            RunState::Starting
                | RunState::Running
                | RunState::Waiting
                | RunState::Stalled
                | RunState::Escalated
        );
        if !working || self.is_cancelling(&id) {
            self.stop_control(req).await;
            return Ok(Moved::Gone);
        }
        // The account that hit the limit is marked whatever happens next, and its process
        // goes away before another one starts for the same run.
        // A bad model is the choice's fault, not the account's: no cooldown for it.
        if req.kind.flags_account() {
            self.flag_account(&id, &req.reason, Some(&req.limit)).await;
        }
        let pins: RoutePins = route
            .and_then(|j| serde_json::from_str(&j).ok())
            .ok_or("the run has no route to repeat")?;
        let from = row.account.clone().ok_or("the run has no account")?;
        if let Some(rt) = lock(&self.inner.runs).get_mut(&id) {
            rt.live = None;
        }
        self.stop_control(req).await;

        let mut tried: Vec<String> = attempts.iter().map(|a| a.account.clone()).collect();
        tried.push(from.clone());
        tried.extend(
            self.tool_ineligible_accounts(&row, pins.tool_controls)
                .await,
        );
        tried.sort();
        tried.dedup();
        let prompt = row.prompt.clone().unwrap_or_default();
        let fail = |tried: &[String], why: String| {
            format!(
                "failover found no account to continue on (tried {}): {why}",
                tried.join(", ")
            )
        };

        // The next account: the same choice first, then any eligible choice, never an
        // account this run already failed on. Taking the slot is one atomic step.
        // The attempt this move ends, by id: a new one may open before it is closed.
        let old_attempt = attempts
            .iter()
            .rev()
            .find(|a| a.ended_at.is_none())
            .map(|a| a.id.clone());
        let mut chosen = None;
        // A model error is the choice's: move to a different choice instead.
        let choice = if req.kind == ErrorKind::Config {
            ChoiceFilter {
                avoid: row.choice_id.as_deref(),
                ..Default::default()
            }
        } else {
            ChoiceFilter {
                only: row.choice_id.as_deref(),
                ..Default::default()
            }
        };
        // Set once the decider is found down: this recovery does not ask it again.
        let outage = std::sync::atomic::AtomicBool::new(false);
        for _ in 0..MAX_ROUTE_TRIES {
            let routed = match self
                .route_run(&prompt, &pins, &tried, choice, false, Some(&outage))
                .await
            {
                Ok(r) => r,
                // Nothing has a free slot: wait in the queue for one, if any could take it.
                Err(e) => {
                    let queued = self
                        .route_run(&prompt, &pins, &tried, choice, true, Some(&outage))
                        .await
                        .map_err(|_| fail(&tried, e.message.clone()))?;
                    return self
                        .requeue(req, &row, &from, queued, &mut tried, old_attempt)
                        .await;
                }
            };
            let mut picked = None;
            for name in &routed.result.ranked_accounts {
                let account = match self.usable_account(name, Some(routed.result.harness)).await {
                    Ok(a) => a,
                    Err(_) => {
                        tried.push(name.clone());
                        continue;
                    }
                };
                let to = reassign_of(&routed.result, &account);
                let (rid, f, cap) = (id.clone(), from.clone(), i64::from(account.cap));
                match self
                    .db_raw(move |s| s.reassign_run(&rid, &f, &to, cap))
                    .await
                {
                    Ok(true) => {
                        picked = Some(account);
                        break;
                    }
                    // Another run took its last slot first: look at the next one.
                    Ok(false) => {}
                    // The run ended (or was cancelled) while we looked.
                    Err(so_store::StoreError::StateConflict { .. }) => return Ok(Moved::Gone),
                    Err(e) => return Err(format!("could not move the run: {e}")),
                }
            }
            if let Some(account) = picked {
                chosen = Some((account, routed));
                break;
            }
        }
        let (account, routed) =
            chosen.ok_or_else(|| fail(&tried, "no account with a free slot".to_string()))?;
        // Only now is the old attempt over.
        self.close_attempt(old_attempt, "failover").await;
        Ok(Moved::Started(Box::new(Next {
            account,
            pending: Pending {
                from,
                from_harness: row.harness,
                reason: excerpt(&req.reason, 200),
                error_kind: req.kind,
                decision: routed.decision,
            },
        })))
    }

    async fn stop_control(&self, req: &FailoverRequest) {
        if let Some(c) = &req.control {
            let _ = c.shutdown(SHUTDOWN_DEADLINE).await;
        }
    }

    async fn close_attempt(&self, attempt_id: Option<String>, outcome: &'static str) {
        if let Some(aid) = attempt_id {
            let _ = self
                .db(move |s| s.finish_attempt(&aid, outcome, Utc::now()))
                .await;
        }
    }

    /// Every other account is full: the run goes back to the queue, on the account that is
    /// best otherwise, with its failover pending.
    async fn requeue(
        &self,
        req: &FailoverRequest,
        row: &RunRow,
        from: &str,
        routed: super::routing::Routed,
        tried: &mut Vec<String>,
        old_attempt: Option<String>,
    ) -> Result<Moved, String> {
        let mut target = None;
        for name in &routed.result.ranked_accounts {
            match self.usable_account(name, Some(routed.result.harness)).await {
                Ok(a) => {
                    target = Some(a);
                    break;
                }
                Err(_) => tried.push(name.clone()),
            }
        }
        let account = target.ok_or("no account can take the run")?;
        let pending = Pending {
            from: from.to_string(),
            from_harness: row.harness,
            reason: excerpt(&req.reason, 200),
            error_kind: req.kind,
            decision: routed.decision,
        };
        let to = reassign_of(&routed.result, &account);
        let (rid, f, json) = (
            row.id.clone(),
            from.to_string(),
            serde_json::to_string(&pending).map_err(|e| e.to_string())?,
        );
        match self
            .db_raw(move |s| s.requeue_for_failover(&rid, &f, &to, &json))
            .await
        {
            Ok(()) => {}
            Err(so_store::StoreError::StateConflict { .. }) => return Ok(Moved::Gone),
            Err(e) => return Err(format!("could not queue the run: {e}")),
        }
        self.close_attempt(old_attempt, "failover").await;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::StateChanged,
            json!({
                "from": row.state,
                "to": RunState::Queued,
                "reason": "failover: waiting for a free slot",
            }),
        )
        .await;
        Ok(Moved::Queued)
    }

    /// Starts the run on its new account; the failover is announced once the launch
    /// succeeded, so a failover that never started leaves no trace.
    async fn start_on(&self, req: &FailoverRequest, next: Next) -> Result<(), LaunchFail> {
        let id = req.run_id.clone();
        let row = self
            .db(move |s| s.get_run(&id))
            .await
            .map_err(|e| LaunchFail::other(e.message))?
            .ok_or_else(|| LaunchFail::other("the run is gone".into()))?;
        let cwd = self.resume_cwd(&row).await.map_err(LaunchFail::other)?;
        let mut prompt = self.turn_prompt(&row).await;
        prompt.push_str(FAILOVER_NOTE);
        let spec = self
            .build_spec(&row, &next.account, cwd, prompt, None, false)
            .await
            .map_err(LaunchFail::other)?;
        let handle = self.launch_harness(&row, spec).await?;
        self.announce_failover(&row, &next.pending).await;
        // The old account has a free slot now.
        self.request_drain();
        // A run that failed over while it was still starting has not gone live yet.
        let resumed = row.state != RunState::Starting;
        self.go_live(&row, &next.account, handle, resumed).await;
        Ok(())
    }

    /// The `failover` and `route_decision` events of a move, for the run's current account.
    pub(crate) async fn announce_failover(&self, row: &RunRow, p: &Pending) {
        let sid = &row.session_id;
        self.emit(
            sid,
            Some(&row.id),
            EventType::Failover,
            json!({
                "from": p.from,
                "to": row.account,
                "from_harness": p.from_harness,
                "to_harness": row.harness,
                "reason": p.reason,
                "error_kind": p.error_kind,
            }),
        )
        .await;
        self.emit(
            sid,
            Some(&row.id),
            EventType::RouteDecision,
            p.decision.clone(),
        )
        .await;
    }
}

fn reassign_of(r: &so_core::proto::RouteResult, account: &AccountFile) -> Reassign {
    Reassign {
        account: account.name.clone(),
        harness: r.harness,
        choice_id: Some(r.choice_id.clone()),
        model: Some(r.model.clone()).filter(|m| !m.is_empty()),
        effort: r.effort.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_errors_fail_over_and_other_errors_do_not() {
        for k in [
            ErrorKind::Quota,
            ErrorKind::RateLimit,
            ErrorKind::Auth,
            ErrorKind::Config,
        ] {
            assert!(failover_kind(k), "{k:?}");
        }
        assert!(!failover_kind(ErrorKind::Other));
    }
}
