//! `route` (SPEC 8.3): eligible choices, Jev decider, heuristic fallback, account selection.
//! It previews only: nothing is reserved and nothing is started.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use so_core::proto::{RouteAlternative, RouteParams, RouteResult, RouteSource, RouteUsage};
use so_core::{AccountFile, Choice, Config, Harness};
use so_store::Store;

use crate::catalog::Catalog;
use crate::eligible::{Candidate, Snapshot, apply_claude_rank, candidate, eligible};
use crate::error::{Result, RouterError};
use crate::heuristic::{
    Complexity, ScoreInputs, effort_gap, estimate_complexity, harness_priority, score,
};
use crate::jev::{EnvFn, JevFailure, JevRequest, decide, process_env, resolve_key};
use crate::select::{headroom_of, rank_accounts, select_account};

pub const MAX_PROMPT_CHARS: usize = 200_000;
const ALTERNATIVES: usize = 3;

/// Everything `route` reads except the store, already collected.
pub struct RouteCtx<'a> {
    pub config: &'a Config,
    pub catalog: &'a Catalog,
    pub snapshot: &'a Snapshot,
    pub http: &'a reqwest::Client,
    pub env: EnvFn,
}

/// Inputs of the convenience [`route`]: it reads the store itself.
pub struct RouteInputs<'a> {
    pub config: &'a Config,
    pub store: &'a Store,
    pub accounts: &'a [AccountFile],
    pub catalog: &'a Catalog,
    pub now: DateTime<Utc>,
    pub http: &'a reqwest::Client,
    pub env: EnvFn,
}

impl<'a> RouteInputs<'a> {
    pub fn new(
        config: &'a Config,
        store: &'a Store,
        accounts: &'a [AccountFile],
        catalog: &'a Catalog,
        http: &'a reqwest::Client,
    ) -> Self {
        Self {
            config,
            store,
            accounts,
            catalog,
            now: Utc::now(),
            http,
            env: process_env,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RouteOutcome {
    pub result: RouteResult,
    /// Why the decider did not decide, when it was tried or wanted but could not be used.
    pub fallback_reason: Option<String>,
}

/// SHA-256 over choices, policy and instruction: changes when routing behaviour would.
pub fn config_fingerprint(config: &Config) -> String {
    let v = json!({
        "choices": config.choices,
        "policy": config.router.policy,
        "instruction": config.router.instruction,
    });
    let digest = Sha256::digest(serde_json::to_vec(&v).unwrap_or_default());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Collects the snapshot on the blocking pool, then routes. The store, accounts and
/// config are cloned for that.
pub async fn route_detailed(
    inputs: &RouteInputs<'_>,
    params: &RouteParams,
) -> Result<RouteOutcome> {
    let (store, accounts, low, now) = (
        inputs.store.clone(),
        inputs.accounts.to_vec(),
        f64::from(inputs.config.usage.low_pct),
        inputs.now,
    );
    let snapshot =
        tokio::task::spawn_blocking(move || Snapshot::collect(&store, &accounts, low, now))
            .await
            .map_err(|e| RouterError::Invalid(format!("snapshot task failed: {e}")))??;
    let ctx = RouteCtx {
        config: inputs.config,
        catalog: inputs.catalog,
        snapshot: &snapshot,
        http: inputs.http,
        env: inputs.env,
    };
    route_with(&ctx, params).await
}

/// Ids of the choices the decider would be offered for `role`.
pub fn eligible_choice_ids(ctx: &RouteCtx<'_>, role: &str, exclude: &[String]) -> Vec<String> {
    apply_claude_rank(eligible(
        &ctx.config.choices,
        ctx.catalog,
        ctx.snapshot,
        role,
        exclude,
    ))
    .into_iter()
    .map(|c| c.choice.id)
    .collect()
}

struct Pins {
    harness: Option<Harness>,
    account: Option<String>,
    model: Option<String>,
    effort: Option<String>,
}

impl Pins {
    fn any(&self) -> bool {
        self.harness.is_some()
            || self.account.is_some()
            || self.model.is_some()
            || self.effort.is_some()
    }
}

fn parse_pins(p: &RouteParams) -> Result<Pins> {
    let harness = p
        .harness
        .as_deref()
        .map(Harness::from_str)
        .transpose()
        .map_err(|e| RouterError::Invalid(e.to_string()))?;
    let blank = |s: &Option<String>| s.clone().filter(|s| !s.trim().is_empty());
    Ok(Pins {
        harness,
        account: blank(&p.account),
        model: blank(&p.model),
        effort: blank(&p.effort),
    })
}

fn pinned_candidates<'a>(
    ctx: &RouteCtx<'a>,
    params: &RouteParams,
    pins: &Pins,
) -> Vec<Candidate<'a>> {
    let matches = |c: &Choice| {
        c.enabled
            && !params.exclude.contains(&c.id)
            && params
                .role
                .as_ref()
                .is_none_or(|r| c.roles.iter().any(|x| x == r))
            && pins.harness.is_none_or(|h| c.harness == h)
            && pins.model.as_ref().is_none_or(|m| &c.model == m)
            && pins
                .effort
                .as_ref()
                .is_none_or(|e| c.effort.as_ref() == Some(e))
    };
    let mut cands: Vec<Candidate<'a>> = ctx
        .config
        .choices
        .iter()
        .filter(|c| matches(c))
        .filter_map(|c| candidate(c, ctx.catalog, ctx.snapshot))
        .collect();
    if cands.is_empty()
        && let (Some(harness), Some(model)) = (pins.harness, pins.model.clone())
    {
        let synthetic = Choice {
            id: format!("pinned-{harness}-{model}"),
            harness,
            model,
            effort: pins.effort.clone(),
            roles: params.role.iter().cloned().collect(),
            description: "named by the call".into(),
            enabled: true,
        };
        cands.extend(candidate(&synthetic, ctx.catalog, ctx.snapshot));
    }
    if let Some(name) = &pins.account {
        for c in &mut cands {
            c.accounts.retain(|a| &a.file.name == name);
        }
        cands.retain(|c| !c.accounts.is_empty());
    }
    cands
}

struct Ranked<'a> {
    cand: &'a Candidate<'a>,
    score: f64,
}

fn heuristic_rank<'a>(
    ctx: &RouteCtx<'_>,
    cands: &'a [Candidate<'a>],
    complexity: Complexity,
) -> Vec<Ranked<'a>> {
    let mut out: Vec<Ranked<'a>> = cands
        .iter()
        .filter_map(|cand| {
            let account = select_account(&cand.accounts, &cand.pool)?;
            let inputs = ScoreInputs {
                complexity,
                tier: cand.tier,
                headroom: (headroom_of(account, &cand.pool) / 100.0).clamp(0.0, 1.0),
                active_runs: account.active,
                cost: cand.cost,
                priority: harness_priority(&ctx.config.router.priority, cand.choice.harness),
                effort_gap: effort_gap(complexity, cand.choice.effort.as_deref()),
            };
            Some(Ranked {
                cand,
                score: score(&inputs, &ctx.config.router.weights),
            })
        })
        .collect();
    // Stable: equal scores keep config order.
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out
}

/// `quota` (remaining % per pool and window) and `usage_5h` (attempts per pool and account).
fn jev_context(snapshot: &Snapshot) -> Value {
    let mut quota: BTreeMap<&str, BTreeMap<&str, f64>> = BTreeMap::new();
    let mut usage: BTreeMap<&str, BTreeMap<&str, i64>> = BTreeMap::new();
    for a in &snapshot.accounts {
        for (pool, p) in &a.pools {
            let q = quota.entry(pool).or_default();
            for (win, pct) in [("five_hour", p.five_hour_pct), ("weekly", p.weekly_pct)] {
                if let Some(pct) = pct {
                    let e = q.entry(win).or_insert(pct);
                    *e = e.max(pct);
                }
            }
            usage
                .entry(pool)
                .or_default()
                .insert(&a.file.name, p.attempts_5h);
        }
    }
    json!({"quota": quota, "usage_5h": usage})
}

fn criteria(cands: &[Candidate<'_>]) -> BTreeMap<String, String> {
    cands
        .iter()
        .map(|c| {
            (
                c.choice.id.clone(),
                format!(
                    "{}; harness={}; model={}; effort={}",
                    c.choice.description,
                    c.choice.harness,
                    c.choice.model,
                    c.choice.effort.as_deref().unwrap_or("default"),
                ),
            )
        })
        .collect()
}

fn alternative(cand: &Candidate<'_>, score: Option<f64>) -> RouteAlternative {
    RouteAlternative {
        choice_id: cand.choice.id.clone(),
        account: select_account(&cand.accounts, &cand.pool).map(|a| a.file.name.clone()),
        score,
    }
}

pub async fn route_with(ctx: &RouteCtx<'_>, params: &RouteParams) -> Result<RouteOutcome> {
    if params.prompt.trim().is_empty() || params.prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(RouterError::Invalid(format!(
            "prompt must be 1 to {MAX_PROMPT_CHARS} characters"
        )));
    }
    let role = params.role.clone().unwrap_or_else(|| "worker".to_string());
    if role.trim().is_empty() {
        return Err(RouterError::Invalid("role must not be empty".into()));
    }
    let pins = parse_pins(params)?;
    let pinned = pins.any();
    let mut cands: Vec<Candidate<'_>> = if pinned {
        pinned_candidates(ctx, params, &pins)
    } else {
        eligible(
            &ctx.config.choices,
            ctx.catalog,
            ctx.snapshot,
            &role,
            &params.exclude,
        )
    };
    // Failover: skipped accounts leave some choices with none, which makes them ineligible.
    if !params.exclude_accounts.is_empty() {
        for c in &mut cands {
            c.accounts
                .retain(|a| !params.exclude_accounts.contains(&a.file.name));
        }
        cands.retain(|c| !c.accounts.is_empty());
    }
    if !pinned {
        cands = apply_claude_rank(cands);
    }
    if cands.is_empty() {
        return Err(RouterError::NoEligible(if pinned {
            "nothing matches the named harness, account, model and effort with an available account"
                .into()
        } else {
            format!(
                "no eligible choice for role `{role}`: every choice is disabled, excluded, over its cap or out of headroom"
            )
        }));
    }
    let complexity = estimate_complexity(&params.prompt);
    let ranked = heuristic_rank(ctx, &cands, complexity);
    let fingerprint = config_fingerprint(ctx.config);

    let finish = |pick: usize,
                  source: RouteSource,
                  rationale: String,
                  fail: Option<&JevFailure>,
                  jev: Option<(f64, BTreeMap<String, f64>, RouteUsage)>,
                  probs_rank: Option<Vec<(usize, f64)>>,
                  fallback_reason: Option<String>|
     -> RouteOutcome {
        let cand = &cands[pick];
        let ranked_accounts: Vec<String> = rank_accounts(&cand.accounts, &cand.pool)
            .iter()
            .map(|a| a.file.name.clone())
            .collect();
        let account = ranked_accounts.first().cloned().unwrap_or_default();
        let (confidence, probabilities, usage) = match (jev, fail) {
            (Some((c, p, u)), _) => (Some(c), p, u),
            (None, Some(f)) => (f.confidence, f.probabilities.clone(), f.usage.clone()),
            (None, None) => (None, BTreeMap::new(), RouteUsage::default()),
        };
        let order: Vec<(usize, Option<f64>)> = match probs_rank {
            Some(v) => v.into_iter().map(|(i, p)| (i, Some(p))).collect(),
            None => ranked
                .iter()
                .filter_map(|r| {
                    cands
                        .iter()
                        .position(|c| c.choice.id == r.cand.choice.id)
                        .map(|i| (i, Some(r.score)))
                })
                .collect(),
        };
        let alternatives = order
            .into_iter()
            .filter(|(i, _)| *i != pick)
            .take(ALTERNATIVES)
            .map(|(i, s)| alternative(&cands[i], s))
            .collect();
        RouteOutcome {
            result: RouteResult {
                choice_id: cand.choice.id.clone(),
                harness: cand.choice.harness,
                account,
                model: cand.choice.model.clone(),
                effort: cand.choice.effort.clone(),
                source,
                rationale,
                confidence,
                probabilities,
                usage,
                alternatives,
                config_fingerprint: fingerprint.clone(),
                ranked_accounts,
            },
            fallback_reason,
        }
    };

    let best_heuristic = |ranked: &[Ranked<'_>]| -> usize {
        ranked
            .first()
            .and_then(|r| cands.iter().position(|c| c.choice.id == r.cand.choice.id))
            .unwrap_or(0)
    };

    if pinned {
        let pick = best_heuristic(&ranked);
        let why = format!(
            "pinned by the call; best match by heuristic score {:.2} (complexity {})",
            ranked.first().map_or(0.0, |r| r.score),
            complexity.as_str()
        );
        return Ok(finish(
            pick,
            RouteSource::Pinned,
            why,
            None,
            None,
            None,
            None,
        ));
    }

    let heuristic = |reason: Option<String>, fail: Option<&JevFailure>, note: &str| {
        let pick = best_heuristic(&ranked);
        let why = format!(
            "heuristic: {note}; complexity {}, score {:.2}",
            complexity.as_str(),
            ranked.first().map_or(0.0, |r| r.score)
        );
        finish(pick, RouteSource::Heuristic, why, fail, None, None, reason)
    };

    if cands.len() == 1 {
        return Ok(heuristic(None, None, "only one eligible choice"));
    }
    // Reads a dotenv file, so it stays off the async threads.
    let (jev_cfg, env) = (ctx.config.router.jev.clone(), ctx.env);
    let key = tokio::task::spawn_blocking(move || resolve_key(&jev_cfg, env))
        .await
        .ok()
        .flatten();
    let Some(key) = key else {
        let reason = "Jev API key unavailable.".to_string();
        return Ok(heuristic(Some(reason.clone()), None, &reason));
    };
    let criteria = criteria(&cands);
    let request = JevRequest {
        task: &params.prompt,
        system_instruction: "",
        routing_instruction: &ctx.config.router.instruction,
        role: &role,
        context: jev_context(ctx.snapshot),
        criteria: &criteria,
    };
    match decide(
        ctx.http,
        &ctx.config.router.jev,
        &ctx.config.router.policy,
        &key,
        &request,
    )
    .await
    {
        Ok(d) => {
            let pick = cands
                .iter()
                .position(|c| c.choice.id == d.validated.choice_id)
                .unwrap_or(0);
            let mut by_prob: Vec<(usize, f64)> = cands
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    (
                        i,
                        d.validated
                            .probabilities
                            .get(&c.choice.id)
                            .copied()
                            .unwrap_or(0.0),
                    )
                })
                .collect();
            by_prob.sort_by(|a, b| b.1.total_cmp(&a.1));
            let why = format!(
                "Jev selected the configured choice (confidence {:.2}).",
                d.validated.confidence
            );
            Ok(finish(
                pick,
                RouteSource::Jev,
                why,
                None,
                Some((d.validated.confidence, d.validated.probabilities, d.usage)),
                Some(by_prob),
                None,
            ))
        }
        Err(f) => {
            let reason = f.reason.clone();
            Ok(heuristic(Some(reason.clone()), Some(&f), &reason))
        }
    }
}
