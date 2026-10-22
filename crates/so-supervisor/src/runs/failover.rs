//! Failover (SPEC 8.5): a run that hits a quota, rate-limit or auth error before it has a
//! result moves to the next-ranked account, keeping its run id, worktree and cap claim.
//!
//! The failing account is marked exhausted (SPEC 6.3), the run is routed again within the
//! pins it was spawned under (never onto an account it already failed on), and a new
//! attempt starts there. Runs whose account the caller named, and runs that were not routed,
//! fail as before.

use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};
use so_core::{AccountFile, ErrorKind, EventType, Harness};
use so_harness::RunControl;
use so_store::Reassign;
use tokio::sync::mpsc::UnboundedReceiver;

use super::lifecycle::{Outcome, excerpt};
use super::pump::PumpCtx;
use super::routing::RoutePins;
use super::{Inner, RunManager, SHUTDOWN_DEADLINE, lock};

/// Moves a run may make to another account. Each one costs a limit hit on a real account,
/// so a run that keeps failing stops.
pub(crate) const MAX_FAILOVERS: usize = 3;
/// Times the new account is looked for again when another run takes its last slot first.
const MAX_ROUTE_TRIES: usize = 3;

/// A run to move because its account hit a limit.
pub(super) struct FailoverRequest {
    run_id: String,
    kind: ErrorKind,
    reason: String,
    control: RunControl,
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
        ErrorKind::Quota | ErrorKind::RateLimit | ErrorKind::Auth
    )
}

impl RunManager {
    /// Called by a run's pump when the run failed with `kind`. Returns `true` when a
    /// failover has been started: the pump must then leave the run alone. `false` means the
    /// run is to fail now (its account was named, it was not routed, or it has used up its
    /// failovers).
    pub(crate) async fn begin_failover(
        &self,
        pc: &PumpCtx,
        control: &RunControl,
        kind: ErrorKind,
        reason: &str,
    ) -> bool {
        if !failover_kind(kind) || self.is_cancelling(&pc.run_id) {
            return false;
        }
        let id = pc.run_id.clone();
        let Ok((pins, attempts)) = self
            .db(move |s| Ok((s.get_run_route(&id)?, s.list_attempts(&id, 1000)?)))
            .await
        else {
            return false;
        };
        let routed = pins
            .and_then(|j| serde_json::from_str::<RoutePins>(&j).ok())
            .is_some_and(|p| p.account.is_none());
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
                run_id: pc.run_id.clone(),
                kind,
                reason: reason.to_string(),
                control: control.clone(),
            })
            .is_ok()
    }

    async fn failover(&self, req: FailoverRequest) {
        let run_id = req.run_id.clone();
        let (kind, why) = match self.move_to_next_account(&req).await {
            Ok(Some(next)) => match self.start_on(&req, next).await {
                Ok(()) => return,
                // The run is on the new account but its harness did not come up: that is
                // not a limit, so it does not claim to be one.
                Err(why) => (ErrorKind::Other, why),
            },
            Ok(None) => return,
            Err(why) => (req.kind, why),
        };
        let reason = format!("{why}; last error: {}", excerpt(&req.reason, 200));
        let mut o = Outcome::failed(kind, &reason, None);
        // The account was marked when the failover began; flagging it again would only
        // restart its cooldown.
        o.flag_account = false;
        self.finish(&run_id, o).await;
    }

    /// Marks the failing account, stops its process and takes a slot on the next account.
    /// `None` when the run ended meanwhile.
    async fn move_to_next_account(&self, req: &FailoverRequest) -> Result<Option<Next>, String> {
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
        if !row.state.is_active() || self.is_cancelling(&id) {
            let _ = req.control.shutdown(SHUTDOWN_DEADLINE).await;
            return Ok(None);
        }
        let pins: RoutePins = route
            .and_then(|j| serde_json::from_str(&j).ok())
            .ok_or("the run has no route to repeat")?;
        let from = row.account.clone().ok_or("the run has no account")?;
        let from_harness = row.harness;

        // 1. The account that hit the limit is marked, and its attempt closed.
        self.flag_account(&id, &req.reason).await;
        {
            let open = attempts.iter().rev().find(|a| a.ended_at.is_none());
            if let Some(a) = open {
                let aid = a.id.clone();
                let _ = self
                    .db(move |s| s.finish_attempt(&aid, "failover", Utc::now()))
                    .await;
            }
        }
        // 2. Its process goes away before another one starts for the same run.
        if let Some(rt) = lock(&self.inner.runs).get_mut(&id) {
            rt.live = None;
        }
        let _ = req.control.shutdown(SHUTDOWN_DEADLINE).await;

        // 3. The next account: the same choice first, then any eligible choice, never an
        //    account this run already failed on. Taking the slot is one atomic step.
        let mut tried: Vec<String> = attempts.iter().map(|a| a.account.clone()).collect();
        tried.push(from.clone());
        tried.sort();
        tried.dedup();
        let prompt = row.prompt.clone().unwrap_or_default();
        let mut moved = None;
        for _ in 0..MAX_ROUTE_TRIES {
            let routed = self
                .route_run(&prompt, &pins, &tried, row.choice_id.as_deref(), false)
                .await
                .map_err(|e| {
                    format!(
                        "failover found no account to continue on (tried {}): {}",
                        tried.join(", "),
                        e.message
                    )
                })?;
            let mut chosen = None;
            for name in &routed.result.ranked_accounts {
                let Ok(account) = self.load_account(name).await else {
                    tried.push(name.clone());
                    continue;
                };
                if !account.enabled || self.launch_for(&account).is_err() {
                    tried.push(name.clone());
                    continue;
                }
                let to = Reassign {
                    account: name.clone(),
                    harness: routed.result.harness,
                    choice_id: Some(routed.result.choice_id.clone()),
                    model: Some(routed.result.model.clone()).filter(|m| !m.is_empty()),
                    effort: routed.result.effort.clone(),
                };
                let (rid, f, cap) = (id.clone(), from.clone(), i64::from(account.cap));
                let t = to.clone();
                match self
                    .db_raw(move |s| s.reassign_run(&rid, &f, &t, cap))
                    .await
                {
                    Ok(true) => {
                        chosen = Some((account, to));
                        break;
                    }
                    // Another run took its last slot first: look at the next one.
                    Ok(false) => {}
                    Err(e) => return Err(format!("could not move the run: {e}")),
                }
            }
            if let Some(c) = chosen {
                moved = Some((c, routed));
                break;
            }
        }
        let ((account, to), routed) = moved.ok_or_else(|| {
            format!(
                "failover found no account with a free slot (tried {})",
                tried.join(", ")
            )
        })?;

        Ok(Some(Next {
            from,
            from_harness,
            account,
            to,
            decision: routed.decision,
        }))
    }

    /// Records the move and starts the run on its new account.
    async fn start_on(&self, req: &FailoverRequest, next: Next) -> Result<(), String> {
        let id = req.run_id.clone();
        let row = {
            let id = id.clone();
            self.db(move |s| s.get_run(&id))
                .await
                .map_err(|e| e.message)?
                .ok_or("the run is gone")?
        };
        let sid = row.session_id.clone();
        self.emit(
            &sid,
            Some(&id),
            EventType::Failover,
            json!({
                "from": next.from,
                "to": next.account.name,
                "from_harness": next.from_harness,
                "to_harness": next.to.harness,
                "reason": excerpt(&req.reason, 200),
                "error_kind": req.kind,
            }),
        )
        .await;
        self.emit(&sid, Some(&id), EventType::RouteDecision, next.decision)
            .await;
        // The old account has a free slot now.
        self.request_drain();

        let cwd = self.resume_cwd(&row).await?;
        let mut prompt = row.prompt.clone().unwrap_or_default();
        prompt.push_str(FAILOVER_NOTE);
        let spec = self
            .build_spec(&row, &next.account, cwd, prompt, None, false)
            .await?;
        let handle = self.launch_harness(&row, spec).await?;
        self.go_live(&row, &next.account, handle, true).await;
        Ok(())
    }
}

/// Where a failed-over run goes.
struct Next {
    from: String,
    from_harness: Harness,
    account: AccountFile,
    to: Reassign,
    decision: Value,
}

const FAILOVER_NOTE: &str = "\n\nThis task was started on another account that ran out of \
    quota; the working tree may already hold part of the work. Check what is there before you \
    continue.";
