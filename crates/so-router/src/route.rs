//! `route` (SPEC 8.3): eligible choices, complexity (Jev decider or heuristic), level, choice,
//! account selection.
//! It previews only: nothing is reserved and nothing is started.

use std::collections::BTreeMap;
use std::str::FromStr;

use chrono::{DateTime, Utc};
use serde_json::json;
use sha2::{Digest, Sha256};
use so_core::config::{JevModels, OnUnreachable};
use so_core::proto::{RouteAlternative, RouteParams, RouteResult, RouteSource, RouteUsage};
use so_core::{AccountFile, Choice, Config, Harness};
use so_store::Store;

use crate::catalog::Catalog;
use crate::eligible::{Candidate, Snapshot, apply_claude_rank, candidate, effort_skips, eligible};
use crate::error::{Result, RouterError};
use crate::heuristic::{
    Complexity, ScoreInputs, effort_gap, estimate_complexity_score, harness_priority, score,
};
use crate::jev::{EnvFn, JevFailure, JevRequest, decide, process_env, resolve_key};
use crate::levels;
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

/// SHA-256 over choices, policy, instruction and levels: changes when routing behaviour would.
pub fn config_fingerprint(config: &Config) -> String {
    let v = json!({
        "choices": config.choices,
        "policy": config.router.policy,
        "instruction": config.router.instruction,
        "levels": config.router.levels,
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
            && c.effort_problem().is_none()
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
            id: so_core::pinned_choice_id(harness, Some(&model)),
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

fn alternative(cand: &Candidate<'_>, score: Option<f64>) -> RouteAlternative {
    RouteAlternative {
        choice_id: cand.choice.id.clone(),
        account: select_account(&cand.accounts, &cand.pool).map(|a| a.file.name.clone()),
        score,
    }
}

pub async fn route_with(ctx: &RouteCtx<'_>, params: &RouteParams) -> Result<RouteOutcome> {
    let role = params.role.clone().unwrap_or_else(|| "worker".to_string());
    let skips = if parse_pins(params).is_ok_and(|p| p.any()) {
        Vec::new()
    } else {
        effort_skips(&ctx.config.choices, &role, &params.exclude)
    };
    let note =
        (!skips.is_empty()).then(|| format!(" Skipped, invalid effort: {}.", skips.join("; ")));
    match route_inner(ctx, params).await {
        Ok(mut out) => {
            if let Some(n) = &note {
                out.result.rationale.push_str(n);
            }
            Ok(out)
        }
        Err(RouterError::NoEligible(m)) => Err(RouterError::NoEligible(match &note {
            Some(n) => format!("{m}{n}"),
            None => m,
        })),
        Err(e) => Err(e),
    }
}

async fn route_inner(ctx: &RouteCtx<'_>, params: &RouteParams) -> Result<RouteOutcome> {
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
    let fingerprint = config_fingerprint(ctx.config);
    let heuristic_rated = |note: Option<String>, fail: Option<&JevFailure>| Rated {
        complexity: estimate_complexity_score(&params.prompt),
        source: RouteSource::Heuristic,
        note,
        confidence: fail.and_then(|f| f.confidence),
        probabilities: fail.map(|f| f.probabilities.clone()).unwrap_or_default(),
        usage: fail.map(|f| f.usage.clone()).unwrap_or_default(),
        decider_model: fail.and_then(|f| f.model.clone()),
        decider_models: None,
        forced_why: None,
    };
    let done = |rated: Rated, fallback_reason: Option<String>| RouteOutcome {
        result: finish(ctx, &cands, &fingerprint, rated),
        fallback_reason,
    };

    if pinned {
        let rated = Rated {
            source: RouteSource::Pinned,
            ..heuristic_rated(None, None)
        };
        return Ok(done(rated, None));
    }
    if cands.len() == 1 {
        let rated = Rated {
            forced_why: Some("the only eligible choice".into()),
            ..heuristic_rated(Some("only one eligible choice".into()), None)
        };
        return Ok(done(rated, None));
    }
    // Reads a dotenv file, so it stays off the async threads.
    let (jev_cfg, env) = (ctx.config.router.jev.clone(), ctx.env);
    let key = tokio::task::spawn_blocking(move || resolve_key(&jev_cfg, env))
        .await
        .ok()
        .flatten();
    let Some(key) = key else {
        let reason = "Jev API key unavailable.".to_string();
        return Ok(done(
            heuristic_rated(Some(reason.clone()), None),
            Some(reason),
        ));
    };
    let request = JevRequest {
        task: &params.prompt,
        system_instruction: "",
        routing_instruction: &ctx.config.router.instruction,
        role: &role,
    };
    let models = ctx.config.router.jev.resolve_models(ctx.env);
    match decide(
        ctx.http,
        &ctx.config.router.jev,
        &models,
        &ctx.config.router.policy,
        &key,
        &request,
    )
    .await
    {
        Ok(d) => Ok(done(
            Rated {
                complexity: d.validated.complexity,
                source: RouteSource::Jev,
                note: None,
                confidence: Some(d.validated.confidence),
                probabilities: d.validated.probabilities,
                usage: d.usage,
                decider_model: Some(d.model),
                decider_models: Some(models),
                forced_why: None,
            },
            None,
        )),
        Err(f) if f.unreachable && ctx.config.router.jev.on_unreachable == OnUnreachable::Error => {
            Err(RouterError::DeciderUnreachable(f.reason))
        }
        Err(f) => {
            let reason = f.reason.clone();
            let rated = Rated {
                decider_models: Some(models),
                ..heuristic_rated(Some(reason.clone()), Some(&f))
            };
            Ok(done(rated, Some(reason)))
        }
    }
}

/// What the complexity step produced, and where it came from.
struct Rated {
    complexity: f64,
    source: RouteSource,
    /// Why the heuristic stood in for the decider.
    note: Option<String>,
    confidence: Option<f64>,
    probabilities: BTreeMap<String, f64>,
    usage: RouteUsage,
    decider_model: Option<String>,
    decider_models: Option<JevModels>,
    /// Replaces the level's reason for the pick (a single eligible choice).
    forced_why: Option<String>,
}

/// Steps 3-7: level, choice, account, result.
fn finish(
    ctx: &RouteCtx<'_>,
    cands: &[Candidate<'_>],
    fingerprint: &str,
    rated: Rated,
) -> RouteResult {
    let ranked = heuristic_rank(ctx, cands, Complexity::from_score(rated.complexity));
    let mut fallback_order: Vec<usize> = ranked
        .iter()
        .filter_map(|r| cands.iter().position(|c| c.choice.id == r.cand.choice.id))
        .collect();
    let missing: Vec<usize> = (0..cands.len())
        .filter(|i| !fallback_order.contains(i))
        .collect();
    fallback_order.extend(missing);
    let top_score = ranked.first().map_or(0.0, |r| r.score);

    let mut sel = levels::select(
        &ctx.config.router.levels,
        cands,
        rated.complexity,
        &fallback_order,
    );
    if rated.source == RouteSource::Pinned {
        sel.order = fallback_order;
        sel.why = format!("pinned by the call; best match by heuristic score {top_score:.2}");
    } else if let Some(w) = rated.forced_why {
        sel.why = w;
    } else if sel.why.starts_with("no level") {
        sel.why = format!("{} (score {top_score:.2})", sel.why);
    }
    let pick = sel.order[0];
    let cand = &cands[pick];
    let ranked_accounts: Vec<String> = rank_accounts(&cand.accounts, &cand.pool)
        .iter()
        .map(|a| a.file.name.clone())
        .collect();
    let account = ranked_accounts.first().cloned().unwrap_or_default();
    let alternatives = sel
        .order
        .iter()
        .skip(1)
        .take(ALTERNATIVES)
        .map(|&i| alternative(&cands[i], None))
        .collect();
    let level = sel
        .level
        .as_deref()
        .map_or_else(|| "none".to_string(), |l| format!("`{l}`"));
    let c = rated.complexity;
    let rationale = match (rated.source, &rated.note, rated.confidence) {
        (RouteSource::Jev, _, conf) => format!(
            "Jev rated complexity {c:.1} (confidence {:.2}); level {level}; `{}` chosen: {}.",
            conf.unwrap_or(0.0),
            cand.choice.id,
            sel.why
        ),
        (RouteSource::Pinned, _, _) => format!(
            "complexity {c:.1} (heuristic estimate); level {level}; `{}` chosen: {}.",
            cand.choice.id, sel.why
        ),
        (RouteSource::Heuristic, note, _) => format!(
            "heuristic: {}; complexity {c:.1} (heuristic estimate); level {level}; `{}` chosen: {}.",
            note.as_deref().unwrap_or("no decider answer"),
            cand.choice.id,
            sel.why
        ),
    };
    RouteResult {
        choice_id: cand.choice.id.clone(),
        harness: cand.choice.harness,
        account,
        model: cand.choice.model.clone(),
        effort: cand.choice.effort.clone(),
        source: rated.source,
        rationale,
        confidence: rated.confidence,
        probabilities: rated.probabilities,
        complexity: Some(c),
        level: sel.level,
        usage: rated.usage,
        alternatives,
        config_fingerprint: fingerprint.to_string(),
        decider_model: rated.decider_model,
        decider_models: rated.decider_models,
        ranked_accounts,
    }
}
