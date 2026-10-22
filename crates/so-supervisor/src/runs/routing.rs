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

use super::{R, RunManager, rpc};
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

impl RunManager {
    fn route_env(&self) -> RouteEnv<'_> {
        RouteEnv {
            store: &self.inner.store,
            config: &self.inner.config,
            paths: &self.inner.paths,
            http: &self.inner.http,
        }
    }

    /// Routes `prompt` within `pins`, skipping `exclude_accounts`. With `only_choice` the
    /// same choice is tried first (SPEC 8.5: the next account of the same choice) and the
    /// whole set only when that has no account left.
    pub(crate) async fn route_run(
        &self,
        prompt: &str,
        pins: &RoutePins,
        exclude_accounts: &[String],
        only_choice: Option<&str>,
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
        let env = self.route_env();
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
        let outcome = match attempt {
            Some(Ok(o)) => Ok(o),
            _ => route_decide(&env, &params).await,
        };
        let outcome = match outcome {
            Err(e) if e.code == UNAVAILABLE && pins.harness.is_some() => {
                self.fallback_route(&params, &e).await?
            }
            other => other?,
        };
        let decision = decision_data(&outcome, &role)?;
        Ok(Routed {
            result: outcome.result,
            decision,
        })
    }

    /// The caller named a harness but no enabled choice matches it (choices of a harness the
    /// operator keeps for itself are off by default): take the best usable account of that
    /// harness directly. A pin is honoured even when no choice covers it.
    async fn fallback_route(&self, params: &RouteParams, why: &RpcError) -> R<RouteOutcome> {
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
                let snap = Snapshot::collect(&store, &accounts, low, Utc::now())
                    .map_err(|e| e.to_string())?;
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
                ranked_accounts: ranked,
            },
            fallback_reason: Some(why.message.clone()),
        })
    }
}
