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
use crate::eligible::{Candidate, Snapshot, apply_claude_rank, candidate, eligible};
use crate::error::{Result, RouterError};
use crate::heuristic::{
    Complexity, ScoreInputs, effort_gap, estimate_complexity_score, harness_priority, score,
};
use crate::jev::{EnvFn, JevFailure, JevRequest, decide, process_env, resolve_key};
use crate::levels;
use crate::select::{headroom_of, rank_accounts, select_account, winner_with_reason};

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

/// SHA-256 over levels (which hold the models), policy and instruction: changes when routing
/// behaviour would.
pub fn config_fingerprint(config: &Config) -> String {
    let v = json!({
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

/// Ids of the choices the decider would be offered.
pub fn eligible_choice_ids(ctx: &RouteCtx<'_>, exclude: &[String]) -> Vec<String> {
    apply_claude_rank(eligible(
        &ctx.config.choices(),
        ctx.catalog,
        ctx.snapshot,
        exclude,
    ))
    .into_iter()
    .map(|c| c.choice.id())
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
    let (mut harness, mut model, mut effort) = (harness, blank(&p.model), blank(&p.effort));
    // A model that starts with a harness name and `/` is a ref (`harness/model[/effort]`) and
    // pins all of its parts; a malformed one is an error. Anything else, such as `gpt-6.1-sol`
    // or `x/y`, is a plain model name. So is `opencode/<one segment>`, because opencode model
    // ids start with their provider, which can be `opencode`.
    let is_ref = |m: &str| match m.split_once('/') {
        Some((h, rest)) if h.parse::<Harness>().is_ok() => {
            !(h == "opencode" && !rest.is_empty() && !rest.contains('/'))
        }
        _ => false,
    };
    let invalid = |reason: String| RouterError::Invalid(reason);
    if let Some(e) = &effort
        && let Some(problem) = so_core::selector_problem("effort", e)
    {
        return Err(invalid(problem));
    }
    if let Some(m) = model.as_deref()
        && !is_ref(m)
        && let Some(problem) = so_core::selector_problem("model", m)
    {
        return Err(invalid(problem));
    }
    if let Some(r) = model.as_deref().filter(|m| is_ref(m)) {
        let r = r.parse::<Choice>().map_err(|e| invalid(e.to_string()))?;
        // Only opencode model ids contain `/`. Elsewhere a `/` left in the model means the
        // last segment was meant as an effort the harness does not accept.
        if r.harness != Harness::Opencode && r.model.contains('/') {
            let list = r.harness.accepted_efforts().unwrap_or_default().join("|");
            return Err(invalid(format!(
                "invalid model ref: `{}` is not a {} effort ({list})",
                r.model.rsplit('/').next().unwrap_or_default(),
                r.harness
            )));
        }
        if let Some(h) = harness.filter(|h| *h != r.harness) {
            return Err(RouterError::Invalid(format!(
                "model ref names {} but harness is {h}",
                r.harness
            )));
        }
        if let (Some(re), Some(e)) = (&r.effort, &effort)
            && re != e
        {
            return Err(RouterError::Invalid(format!(
                "model ref has effort {re} but effort is {e}"
            )));
        }
        harness = Some(r.harness);
        effort = r.effort.or(effort);
        model = Some(r.model);
    }
    Ok(Pins {
        harness,
        account: blank(&p.account),
        model,
        effort,
    })
}

fn pinned_candidates<'a>(
    ctx: &RouteCtx<'a>,
    params: &RouteParams,
    pins: &Pins,
) -> Result<Vec<Candidate<'a>>> {
    let matches = |c: &Choice| {
        !params.exclude.contains(&c.id())
            && pins.harness.is_none_or(|h| c.harness == h)
            && pins.model.as_ref().is_none_or(|m| &c.model == m)
            && pins
                .effort
                .as_ref()
                .is_none_or(|e| c.effort.as_ref() == Some(e))
    };
    let mut cands: Vec<Candidate<'a>> = ctx
        .config
        .choices()
        .iter()
        .filter(|c| matches(c))
        .filter_map(|c| candidate(c, ctx.catalog, ctx.snapshot))
        .collect();
    if cands.is_empty()
        && let (Some(harness), Some(model)) = (pins.harness, pins.model.clone())
    {
        // No level lists it, but the call named it: it still runs.
        if let Some(effort) = &pins.effort
            && let Some(problem) = harness.model_effort_problem(&model, effort)
        {
            return Err(RouterError::Invalid(problem));
        }
        let synthetic = Choice {
            harness,
            model,
            effort: pins.effort.clone(),
        };
        cands.extend(candidate(&synthetic, ctx.catalog, ctx.snapshot));
    }
    if let Some(name) = &pins.account {
        for c in &mut cands {
            c.accounts.retain(|a| &a.file.name == name);
        }
        cands.retain(|c| !c.accounts.is_empty());
    }
    Ok(cands)
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
        choice_id: cand.choice.id(),
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
    let agent = params.agent.clone().unwrap_or_else(|| "worker".to_string());
    if agent.trim().is_empty() {
        return Err(RouterError::Invalid("agent must not be empty".into()));
    }
    let pins = parse_pins(params)?;
    let pinned = pins.any();
    let mut cands: Vec<Candidate<'_>> = if pinned {
        pinned_candidates(ctx, params, &pins)?
    } else {
        eligible(
            &ctx.config.choices(),
            ctx.catalog,
            ctx.snapshot,
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
            "no eligible choice: every model listed on a level is excluded, over its cap or out of headroom (or no level lists a model)"
                .into()
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
        agent: &agent,
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
        .filter_map(|r| cands.iter().position(|c| c.choice == r.cand.choice))
        .collect();
    let missing: Vec<usize> = (0..cands.len())
        .filter(|i| !fallback_order.contains(i))
        .collect();
    fallback_order.extend(missing);
    let top_score = ranked.first().map_or(0.0, |r| r.score);

    let mut sel = if rated.source == RouteSource::Pinned {
        let mapped = levels::levels_by_distance(&ctx.config.router.levels, rated.complexity)
            .first()
            .map(|&i| ctx.config.router.levels[i].name.clone());
        levels::Selection {
            order: fallback_order,
            level: mapped.clone(),
            mapped,
            heuristic_fallback: false,
            why: format!("pinned by the call; best match by heuristic score {top_score:.2}"),
        }
    } else {
        levels::select(
            &ctx.config.router.levels,
            cands,
            rated.complexity,
            &fallback_order,
        )
    };
    if let Some(w) = rated
        .forced_why
        .filter(|_| rated.source != RouteSource::Pinned)
    {
        sel.why = w;
    } else if sel.heuristic_fallback {
        sel.why = format!("{} (score {top_score:.2})", sel.why);
    }
    let pick = sel.order[0];
    let cand = &cands[pick];
    let by_account = rank_accounts(&cand.accounts, &cand.pool);
    let ranked_accounts: Vec<String> = by_account.iter().map(|a| a.file.name.clone()).collect();
    let (account, account_why) = winner_with_reason(&by_account, &cand.pool)
        .map(|(a, why)| (a.file.name.clone(), why))
        .unwrap_or_default();
    let account_note = format!("account {account}: {account_why}");
    let alternatives = sel
        .order
        .iter()
        .skip(1)
        .take(ALTERNATIVES)
        .map(|&i| alternative(&cands[i], None))
        .collect();
    let level = match (&sel.mapped, &sel.level) {
        (None, _) => "no levels configured".to_string(),
        (Some(_), Some(u)) if sel.mapped.as_ref() == Some(u) => format!("level `{u}`"),
        (Some(m), Some(u)) => {
            format!("maps to level `{m}`; no eligible choice there; used level `{u}`")
        }
        (Some(m), None) => format!("maps to level `{m}`; no level has an eligible choice"),
    };
    let c = rated.complexity;
    let id = cand.choice.id();
    let rationale = match (rated.source, &rated.note, rated.confidence) {
        (RouteSource::Jev, _, conf) => format!(
            "Jev rated complexity {c:.1} (confidence {:.2}); {level}; `{id}` chosen: {}; {account_note}.",
            conf.unwrap_or(0.0),
            sel.why
        ),
        (RouteSource::Pinned, _, _) => format!(
            "complexity {c:.1} (heuristic estimate); {level}; `{id}` chosen: {}; {account_note}.",
            sel.why
        ),
        (RouteSource::Heuristic, note, _) => format!(
            "heuristic: {}; complexity {c:.1} (heuristic estimate); {level}; `{id}` chosen: {}; {account_note}.",
            note.as_deref().unwrap_or("no decider answer"),
            sel.why
        ),
    };
    RouteResult {
        choice_id: id,
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
