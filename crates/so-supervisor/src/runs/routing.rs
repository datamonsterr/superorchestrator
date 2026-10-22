//! Routing for spawns and failovers (SPEC 8.3, 8.5).
//!
//! A run whose harness or account the caller left open is routed. What the caller (or the
//! agent file, or `defaults`) did name is a pin: the router keeps to it, and a failover
//! stays inside it. The pins are stored with the run so a failover can route again.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use so_core::proto::{RouteParams, RouteResult, RouteSource, RpcError, UNAVAILABLE};
use so_core::{AccountFile, Harness};
use so_router::{Catalog, RouteOutcome, Snapshot, route::MAX_PROMPT_CHARS};

use super::{R, RunManager, lock, rpc};
use crate::handlers::route::{RouteEnv, decision_data, route_decide};

/// What fixed a run's route.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoutePins {
    pub role: Option<String>,
    pub harness: Option<String>,
    pub account: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// The agent file's `accounts` list; empty means any account.
    #[serde(default)]
    pub allow_accounts: Vec<String>,
    /// The agent has tool controls (`tools`/`disallowedTools`): a failover must keep them
    /// enforced, and fails closed when it cannot check.
    #[serde(default)]
    pub tool_controls: bool,
}

/// A route decision plus the pins it was made under, ready to be stored with the run.
pub(crate) struct RoutePlan {
    pub pins: RoutePins,
    /// The `route_decision` event data.
    pub decision: Value,
}

/// A routed account choice: the decision and the accounts that can take it, best first.
pub(crate) struct Routed {
    pub result: RouteResult,
    pub decision: Value,
}

/// Least time between two re-routing passes for the same queued run.
const REROUTE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

impl RunManager {
    fn route_env(&self, ignore_caps: bool) -> RouteEnv<'_> {
        RouteEnv {
            store: &self.inner.store,
            config: &self.inner.config,
            paths: &self.inner.paths,
            http: &self.inner.http,
            ignore_caps,
        }
    }

    /// Routes `prompt` within `pins`, skipping `exclude_accounts`. With `only_choice` the
    /// same choice is tried first (SPEC 8.5: the next account of the same choice) and the
    /// whole set only when that has no account left.
    ///
    /// With `queue_over_cap`, a choice whose accounts are all at their cap is still routed
    /// to (the run waits in the queue); a failover cannot wait, so it does not set it.
    pub(crate) async fn route_run(
        &self,
        prompt: &str,
        pins: &RoutePins,
        exclude_accounts: &[String],
        only_choice: Option<&str>,
        queue_over_cap: bool,
    ) -> R<Routed> {
        let role = pins.role.clone().unwrap_or_else(|| "worker".into());
        let mut skip = exclude_accounts.to_vec();
        if !pins.allow_accounts.is_empty() {
            let paths = self.inner.paths.clone();
            let all = self
                .blocking(move || AccountFile::list(&paths))
                .await?
                .map_err(|e| rpc(UNAVAILABLE, e.to_string()))?;
            skip.extend(
                all.into_iter()
                    .map(|a| a.name)
                    .filter(|n| !pins.allow_accounts.contains(n)),
            );
        }
        let task: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
        let mut params = RouteParams {
            prompt: task,
            role: Some(role.clone()),
            exclude: Vec::new(),
            exclude_accounts: skip,
            harness: pins.harness.clone(),
            account: pins.account.clone(),
            model: pins.model.clone(),
            effort: pins.effort.clone(),
        };
        let env = self.route_env(false);
        let mut attempt = None;
        if let Some(cid) = only_choice {
            params.exclude = self
                .config()
                .choices
                .iter()
                .filter(|c| c.id != cid)
                .map(|c| c.id.clone())
                .collect();
            attempt = Some(route_decide(&env, &params).await);
            params.exclude.clear();
        }
        let mut outcome = match attempt {
            Some(Ok(o)) => Ok(o),
            _ => route_decide(&env, &params).await,
        };
        if queue_over_cap && outcome.as_ref().is_err_and(|e| e.code == UNAVAILABLE) {
            outcome = route_decide(&self.route_env(true), &params).await;
        }
        let outcome = match outcome {
            Err(e) if e.code == UNAVAILABLE && pins.harness.is_some() => {
                self.fallback_route(&params, &e, queue_over_cap).await?
            }
            other => other?,
        };
        let decision = decision_data(&outcome, &role, &params.prompt)?;
        Ok(Routed {
            result: outcome.result,
            decision,
        })
    }

    /// The caller named a harness but no enabled choice matches it (choices of a harness the
    /// operator keeps for itself are off by default): take the best usable account of that
    /// harness directly. A pin is honoured even when no choice covers it.
    async fn fallback_route(
        &self,
        params: &RouteParams,
        why: &RpcError,
        queue_over_cap: bool,
    ) -> R<RouteOutcome> {
        let harness: Harness = params
            .harness
            .as_deref()
            .and_then(|h| h.parse().ok())
            .ok_or_else(|| why.clone())?;
        let (store, paths, low) = (
            self.inner.store.clone(),
            self.inner.paths.clone(),
            f64::from(self.config().usage.low_pct),
        );
        let (exclude, pin_account, model) = (
            params.exclude_accounts.clone(),
            params.account.clone(),
            params.model.clone(),
        );
        let ranked = self
            .blocking(move || -> Result<Vec<String>, String> {
                let accounts = AccountFile::list(&paths).map_err(|e| e.to_string())?;
                let catalog = Catalog::load(&paths).map_err(|e| e.to_string())?;
                let mut snap = Snapshot::collect(&store, &accounts, low, Utc::now())
                    .map_err(|e| e.to_string())?;
                if queue_over_cap {
                    snap.accounts.iter_mut().for_each(|a| a.active = 0);
                }
                let pool = catalog.pool_for(harness, model.as_deref().unwrap_or_default());
                let usable: Vec<_> = so_router::eligible::usable_accounts(&snap, harness, &pool)
                    .into_iter()
                    .filter(|a| !exclude.contains(&a.file.name))
                    .filter(|a| pin_account.as_ref().is_none_or(|n| n == &a.file.name))
                    .collect();
                Ok(so_router::select::rank_accounts(&usable, &pool)
                    .into_iter()
                    .map(|a| a.file.name.clone())
                    .collect())
            })
            .await?
            .map_err(|e| rpc(UNAVAILABLE, e))?;
        let Some(first) = ranked.first().cloned() else {
            return Err(why.clone());
        };
        Ok(RouteOutcome {
            result: RouteResult {
                choice_id: format!("pinned-{harness}"),
                harness,
                account: first,
                model: params.model.clone().unwrap_or_default(),
                effort: params.effort.clone(),
                source: RouteSource::Pinned,
                rationale: format!(
                    "pinned to {harness}; no enabled choice matches, so the best usable account was taken"
                ),
                confidence: None,
                probabilities: Default::default(),
                usage: Default::default(),
                alternatives: Vec::new(),
                config_fingerprint: so_router::config_fingerprint(self.config()),
                decider_model: None,
                ranked_accounts: ranked,
            },
            fallback_reason: Some(why.message.clone()),
        })
    }

    /// A queued run waits for a slot on an account that is full. When it was routed (not
    /// pinned to its account) and has not started, look for an account with a free slot
    /// within its pins and move it there. Returns whether it moved.
    pub(crate) async fn reroute_queued(&self, run: &so_store::RunRow) -> bool {
        if run.native_session_id.is_some() {
            return false;
        }
        let id = run.id.clone();
        let Ok(Some(pins)) = self.db(move |s| s.get_run_route(&id)).await else {
            return false;
        };
        let Ok(pins) = serde_json::from_str::<RoutePins>(&pins) else {
            return false;
        };
        let Some(from) = run.account.clone() else {
            return false;
        };
        if pins.account.is_some() {
            return false;
        }
        // The whole system being full is not something another account fixes.
        let global = i64::from(self.config().limits.global_cap);
        if self
            .db(|s| s.count_active_runs_total())
            .await
            .is_ok_and(|n| n >= global)
        {
            return false;
        }
        // No full routing pass per drain wake for the same run.
        {
            let mut seen = lock(&self.inner.reroute_at);
            let now = std::time::Instant::now();
            seen.retain(|_, t| now.duration_since(*t) < REROUTE_BACKOFF);
            if seen.insert(run.id.clone(), now).is_some() {
                return false;
            }
        }
        // A run waiting out a failover keeps its constraints: not the accounts it failed
        // on, and not a harness that cannot enforce its agent's tools.
        let id = run.id.clone();
        let failing_over = self
            .db(move |s| s.get_failover_pending(&id))
            .await
            .ok()
            .flatten()
            .is_some();
        let mut exclude = Vec::new();
        if failing_over {
            let id = run.id.clone();
            if let Ok(attempts) = self.db(move |s| s.list_attempts(&id, 1000)).await {
                exclude.extend(attempts.into_iter().map(|a| a.account));
            }
            exclude.extend(self.tool_ineligible_accounts(run, pins.tool_controls).await);
            exclude.sort();
            exclude.dedup();
        }
        let prompt = self.turn_prompt(run).await;
        let Ok(routed) = self.route_run(&prompt, &pins, &exclude, None, false).await else {
            return false;
        };
        for name in &routed.result.ranked_accounts {
            if *name == from {
                continue;
            }
            let Ok(account) = self.usable_account(name, Some(routed.result.harness)).await else {
                continue;
            };
            let to = so_store::Reassign {
                account: account.name.clone(),
                harness: routed.result.harness,
                choice_id: Some(routed.result.choice_id.clone()),
                model: Some(routed.result.model.clone()).filter(|m| !m.is_empty()),
                effort: routed.result.effort.clone(),
            };
            let (rid, f) = (run.id.clone(), from.clone());
            if let Ok(true) = self.db_raw(move |s| s.retarget_queued(&rid, &f, &to)).await {
                // Announced when the run starts, not while it still waits.
                let (rid, json) = (run.id.clone(), routed.decision.to_string());
                let _ = self
                    .db(move |s| s.set_queued_route_event(&rid, &json))
                    .await;
                lock(&self.inner.reroute_at).remove(&run.id);
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::testutil::insert_run;
    use so_accounts::HostEnv;
    use so_core::{Config, Harness, Paths, Platform, RunState};
    use so_store::Store;

    /// A manager with a host (so accounts can be launched) and two claude accounts, the
    /// first of which is full.
    fn two_accounts() -> (RunManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        for n in ["claude-t", "claude-u"] {
            AccountFile::new(n, Harness::Claude, &format!("~/.{n}"), "/bin/sh")
                .unwrap()
                .save(&paths)
                .unwrap();
        }
        let host = HostEnv {
            home: dir.path().to_path_buf(),
            path_var: Some("/usr/bin:/bin".into()),
            platform: Platform::current(),
        };
        let m = RunManager::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            paths,
            Some(host),
            dir.path().join("s.sock"),
            None,
        );
        (m, dir)
    }

    fn queued_on_the_full_account(m: &RunManager) -> so_store::RunRow {
        // Two running runs fill `claude-t` (cap 2).
        for _ in 0..2 {
            insert_run(m, RunState::Running);
        }
        let run = insert_run(m, RunState::Queued);
        let pins = RoutePins {
            harness: Some("claude".into()),
            ..RoutePins::default()
        };
        m.store()
            .set_run_route(&run.id, &serde_json::to_string(&pins).unwrap())
            .unwrap();
        m.store().set_run_turn_prompt(&run.id, "do it").unwrap();
        run
    }

    #[tokio::test]
    async fn a_queued_run_moves_to_a_free_account_but_not_more_than_once_in_a_while() {
        let (m, _d) = two_accounts();
        let run = queued_on_the_full_account(&m);
        assert!(m.reroute_queued(&run).await);
        let now = m.store().get_run(&run.id).unwrap().unwrap();
        assert_eq!(now.account.as_deref(), Some("claude-u"));
        // The decision waits for the launch; nothing is announced yet.
        assert!(
            m.store()
                .take_queued_route_event(&run.id)
                .unwrap()
                .is_some()
        );
        assert!(
            m.store()
                .list_events_by_run(&run.id, 100)
                .unwrap()
                .iter()
                .all(|e| e.event_type != so_core::EventType::RouteDecision)
        );
        // Looked at again right away: no second routing pass for the same run.
        m.store()
            .retarget_queued(
                &run.id,
                "claude-u",
                &so_store::Reassign {
                    account: "claude-t".into(),
                    harness: Harness::Claude,
                    choice_id: None,
                    model: None,
                    effort: None,
                },
            )
            .unwrap();
        lock(&m.inner.reroute_at).insert(run.id.clone(), std::time::Instant::now());
        let again = m.store().get_run(&run.id).unwrap().unwrap();
        assert!(!m.reroute_queued(&again).await, "backed off");
    }

    #[tokio::test]
    async fn a_run_waiting_out_a_failover_does_not_go_back_to_an_account_it_failed_on() {
        let (m, _d) = two_accounts();
        let run = queued_on_the_full_account(&m);
        // It failed on `claude-u` earlier, and is waiting for a slot.
        let aid = m
            .store()
            .start_attempt(&run.id, "claude-u", None, chrono::Utc::now())
            .unwrap();
        m.store()
            .finish_attempt(&aid, "failover", chrono::Utc::now())
            .unwrap();
        // `failover_json` is what marks it as a failover that is waiting.
        let pending = serde_json::json!({
            "from": "claude-u", "from_harness": "claude", "reason": "x",
            "error_kind": "quota", "decision": {}
        });
        let _ = m
            .store()
            .set_failover_pending(&run.id, &pending.to_string());
        assert!(!m.reroute_queued(&run).await);
        assert_eq!(
            m.store()
                .get_run(&run.id)
                .unwrap()
                .unwrap()
                .account
                .as_deref(),
            Some("claude-t")
        );
    }
}
