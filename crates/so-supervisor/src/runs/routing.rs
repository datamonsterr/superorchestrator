//! Routing for spawns and failovers (SPEC 8.3, 8.5).
//!
//! A run whose harness or account the caller left open is routed. What the caller (or the
//! agent file, or `defaults`) did name is a pin: the router keeps to it, and a failover
//! stays inside it. The pins are stored with the run so a failover can route again.

use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use so_core::config::OnUnreachable;
use so_core::proto::{
    DECIDER_UNREACHABLE, RouteParams, RouteResult, RouteSource, RpcError, UNAVAILABLE,
};
use so_core::{AccountFile, Config, Harness};
use so_router::{Catalog, RouteOutcome, Snapshot, route::MAX_PROMPT_CHARS};

use super::{R, RunManager, lock, rpc};
use crate::handlers::route::{RouteEnv, decision_data, route_decide};

/// What fixed a run's route.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RoutePins {
    /// The agent profile's name, which informs the decider.
    #[serde(alias = "role")]
    pub agent: Option<String>,
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

/// Which choice a route favours or steers clear of.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ChoiceFilter<'a> {
    /// Try this choice first (SPEC 8.5: the next account of the same choice).
    pub only: Option<&'a str>,
    /// Never route to this choice (a failover must not retry the one that failed).
    pub avoid: Option<&'a str>,
}

/// Least time between two re-routing passes for the same queued run.
const REROUTE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(30);

impl RunManager {
    fn route_env<'a>(&'a self, config: &'a Config, ignore_caps: bool) -> RouteEnv<'a> {
        RouteEnv {
            store: &self.inner.store,
            config,
            paths: &self.inner.paths,
            http: &self.inner.http,
            ignore_caps,
        }
    }

    /// Routes `prompt` within `pins`, skipping `exclude_accounts`. With `choice.only` the
    /// same choice is tried first (SPEC 8.5: the next account of the same choice) and the
    /// whole set only when that has no account left.
    ///
    /// With `queue_over_cap`, a choice whose accounts are all at their cap is still routed
    /// to (the run waits in the queue); a failover cannot wait, so it does not set it.
    ///
    /// `recovery` is set for mid-run recovery (a failover or a queued reroute). Those must not
    /// fail a run because the decider is down: an outage degrades to the heuristic, and the
    /// flag remembers it so the decider is not asked again within the same recovery. An
    /// explicit request (`recovery: None`) follows `router.jev.on_unreachable`.
    pub(crate) async fn route_run(
        &self,
        prompt: &str,
        pins: &RoutePins,
        exclude_accounts: &[String],
        choice: ChoiceFilter<'_>,
        queue_over_cap: bool,
        recovery: Option<&AtomicBool>,
    ) -> R<Routed> {
        let Some(outage) = recovery else {
            return self
                .route_run_with(
                    &self.inner.config,
                    prompt,
                    pins,
                    exclude_accounts,
                    choice,
                    queue_over_cap,
                )
                .await;
        };
        let mut config = self.inner.config.clone();
        config.router.jev.on_unreachable = OnUnreachable::Heuristic;
        if outage.load(Ordering::SeqCst) {
            // Already known to be down in this recovery: no key means no decider call.
            config.router.jev.key_env = "SUPERORCHESTRATOR_DECIDER_SKIPPED".into();
            config.router.jev.key_file = None;
        }
        let routed = self
            .route_run_with(
                &config,
                prompt,
                pins,
                exclude_accounts,
                choice,
                queue_over_cap,
            )
            .await?;
        if let Some(why) = routed.decision["fallback_reason"]
            .as_str()
            .filter(|w| w.contains("decider unreachable"))
            && !outage.swap(true, Ordering::SeqCst)
        {
            tracing::warn!(
                reason = why,
                "routing decider unreachable during recovery; used the heuristic"
            );
        }
        Ok(routed)
    }

    async fn route_run_with(
        &self,
        config: &Config,
        prompt: &str,
        pins: &RoutePins,
        exclude_accounts: &[String],
        choice: ChoiceFilter<'_>,
        queue_over_cap: bool,
    ) -> R<Routed> {
        let agent = pins.agent.clone().unwrap_or_else(|| "worker".into());
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
            agent: Some(agent.clone()),
            exclude: Vec::new(),
            exclude_accounts: skip,
            harness: pins.harness.clone(),
            account: pins.account.clone(),
            model: pins.model.clone(),
            effort: pins.effort.clone(),
        };
        let env = self.route_env(config, false);
        let mut attempt = None;
        if let Some(cid) = choice.only {
            params.exclude = config
                .choices()
                .iter()
                .map(so_core::Choice::id)
                .filter(|id| id != cid)
                .collect();
            attempt = Some(route_decide(&env, &params).await);
            params.exclude.clear();
        }
        // A choice whose model the harness lacks is skipped, whatever else is eligible.
        params.exclude.extend(choice.avoid.map(str::to_string));
        let mut outcome = match attempt {
            Some(Ok(o)) => Ok(o),
            // An unreachable decider fails the same way on the wider route: do not ask again.
            Some(Err(e)) if e.code == DECIDER_UNREACHABLE => Err(e),
            _ => route_decide(&env, &params).await,
        };
        if queue_over_cap && outcome.as_ref().is_err_and(|e| e.code == UNAVAILABLE) {
            outcome = route_decide(&self.route_env(config, true), &params).await;
        }
        let outcome = match outcome {
            Err(e) if e.code == UNAVAILABLE && pins.harness.is_some() => {
                self.fallback_route(&params, &e, queue_over_cap).await?
            }
            other => other?,
        };
        let decision = decision_data(&outcome, &agent, &params.prompt)?;
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
        let model = params.model.clone().unwrap_or_default();
        // No level lists this pin, so its id is a ref built from what was pinned; with no model
        // named there is no ref to form and the id is the harness name.
        let choice_id = if model.is_empty() {
            harness.to_string()
        } else {
            so_core::Choice {
                harness,
                model: model.clone(),
                effort: params.effort.clone(),
            }
            .id()
        };
        Ok(RouteOutcome {
            result: RouteResult {
                choice_id,
                harness,
                account: first,
                model,
                effort: params.effort.clone(),
                source: RouteSource::Pinned,
                rationale: format!(
                    "pinned to {harness}; no enabled choice matches, so the best usable account was taken"
                ),
                confidence: None,
                probabilities: Default::default(),
                complexity: None,
                level: None,
                usage: Default::default(),
                alternatives: Vec::new(),
                config_fingerprint: so_router::config_fingerprint(self.config()),
                decider_model: None,
                decider_models: None,
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
        let Ok(routed) = self
            .route_run(
                &prompt,
                &pins,
                &exclude,
                ChoiceFilter::default(),
                false,
                Some(&AtomicBool::new(false)),
            )
            .await
        else {
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
