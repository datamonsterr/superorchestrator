//! `spawn_agent`: validate the request, insert a queued run and ask for a slot.

use std::path::PathBuf;

use chrono::Utc;
use serde_json::{Value, json};
use so_accounts::{LaunchMode, build_launch, resolve_binary};
use so_agents::{AgentDef, AgentError, HarnessPref};
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, INVALID_PARAMS, INVALID_REQUEST, NOT_FOUND, RpcError, SpawnParams,
    UNAVAILABLE,
};
use so_core::{AccountFile, EventType, Harness, HumanDuration, Isolation, RunState, new_id};
use so_harness::AnyDriver;
use so_store::{InsertRun, RunRow};

use super::instructions::MAX_DEPTH;
use super::routing::{RoutePins, RoutePlan};
use super::worktree;
use super::{R, RunManager, RunRt, lock, rpc};
use crate::dispatch::{ConnCtx, internal};

struct Validated {
    harness: Harness,
    account: AccountFile,
    prompt: String,
    cwd: PathBuf,
    isolation: Isolation,
    model: Option<String>,
    effort: Option<String>,
    depth: u8,
    timeout_ms: Option<u64>,
    warnings: Vec<String>,
    max_turns: Option<u32>,
    review: bool,
    /// Set when the router chose (some of) the route.
    choice_id: Option<String>,
    route: Option<RoutePlan>,
}

/// The brief of an agent that was spawned without one.
fn default_brief(agent: &AgentDef) -> String {
    format!(
        "Carry out your role as the {} agent: {}",
        agent.name, agent.description
    )
}

fn agent_error(e: AgentError) -> RpcError {
    match e {
        AgentError::NotFound(n) => rpc(NOT_FOUND, format!("no agent named `{n}`")),
        other => rpc(INVALID_PARAMS, other.to_string()),
    }
}

impl RunManager {
    pub(crate) async fn load_account(&self, name: &str) -> R<AccountFile> {
        let path = self.inner.paths.account_file(name);
        let name = name.to_string();
        self.blocking(move || {
            if !path.exists() {
                return Err(rpc(NOT_FOUND, format!("account not found: {name}")));
            }
            AccountFile::load(&path).map_err(|e| rpc(UNAVAILABLE, format!("{e}: {name}")))
        })
        .await?
    }

    pub(crate) fn launch_for(&self, account: &AccountFile) -> R<so_accounts::Launch> {
        let host = self
            .host()
            .ok_or_else(|| rpc(UNAVAILABLE, "the host environment (HOME) is not available"))?;
        let bwrap = (account.harness == Harness::Agy)
            .then(|| resolve_binary("bwrap", host.path_var.as_deref()).ok())
            .flatten();
        build_launch(account, host, bwrap.as_deref(), LaunchMode::Supervised).map_err(|e| {
            rpc(
                UNAVAILABLE,
                format!("cannot launch account {}: {e}", account.name),
            )
        })
    }

    /// An account a run can start on now: it exists, is enabled, matches `harness` when one
    /// is given, and its harness can be launched.
    pub(super) async fn usable_account(
        &self,
        name: &str,
        harness: Option<Harness>,
    ) -> R<AccountFile> {
        let account = self.load_account(name).await?;
        if let Some(h) = harness
            && account.harness != h
        {
            return Err(rpc(
                INVALID_PARAMS,
                format!(
                    "account {} is a {} account, not {h}",
                    account.name, account.harness
                ),
            ));
        }
        if !account.enabled {
            return Err(rpc(
                UNAVAILABLE,
                format!("account {} is disabled", account.name),
            ));
        }
        AnyDriver::for_harness(account.harness, self.inner.driver_cfg)
            .map_err(|e| rpc(UNAVAILABLE, e.to_string()))?;
        self.launch_for(&account)?;
        Ok(account)
    }

    async fn validate(&self, ctx: &ConnCtx, p: &SpawnParams) -> R<Validated> {
        let session_id = ctx
            .session_id
            .clone()
            .ok_or_else(|| rpc(INVALID_REQUEST, "hello required"))?;
        let depth = ctx.depth.saturating_add(1);
        if depth > MAX_DEPTH {
            return Err(rpc(
                DEPTH_LIMIT,
                format!(
                    "a run at depth {} cannot spawn: the deepest run is depth {MAX_DEPTH}",
                    ctx.depth
                ),
            ));
        }
        if p.agent.is_none() && p.prompt.as_ref().is_none_or(|s| s.trim().is_empty()) {
            return Err(rpc(INVALID_PARAMS, "prompt is required"));
        }
        if let Some(schema) = &p.output_schema
            && let Err(e) = jsonschema::validator_for(schema)
        {
            return Err(rpc(
                INVALID_PARAMS,
                format!("output_schema is not a valid JSON Schema: {e}"),
            ));
        }
        let timeout_ms = match &p.timeout {
            None => None,
            Some(t) => {
                let d: HumanDuration = t
                    .parse()
                    .map_err(|e: so_core::CoreError| rpc(INVALID_PARAMS, e.to_string()))?;
                u64::try_from(d.as_duration().as_millis()).ok()
            }
        };

        let sid = session_id.clone();
        let session = self
            .db(move |s| s.get_session(&sid))
            .await?
            .ok_or_else(|| rpc(NOT_FOUND, format!("session not found: {session_id}")))?;
        let (base, requested) = (session.cwd.map(PathBuf::from), p.cwd.clone());
        let cwd = self
            .blocking(move || resolve_cwd(base, requested))
            .await??;

        // The agent, if any, supplies defaults the call can override (SPEC 7.2: call
        // argument, then agent file, then config, then the router).
        let agent = match &p.agent {
            None => None,
            Some(name) => {
                let (dir, paths, name) = (cwd.clone(), self.inner.paths.clone(), name.clone());
                let def = self
                    .blocking(move || so_agents::resolve_for_spawn(&dir, &paths, &name))
                    .await?
                    .map_err(agent_error)?;
                Some(def)
            }
        };
        let prompt = match (&p.prompt, &agent) {
            (Some(t), _) if !t.trim().is_empty() => t.clone(),
            (_, Some(a)) => default_brief(a),
            _ => return Err(rpc(INVALID_PARAMS, "prompt is required")),
        };
        let review = p.review || agent.as_ref().is_some_and(|a| a.review);
        validate_checks(review, &p.checks)?;
        let defaults = &self.config().defaults;
        let harness_pin = match p.harness.as_deref() {
            Some("auto") => None,
            Some(h) => Some(
                h.parse::<Harness>()
                    .map_err(|e| rpc(INVALID_PARAMS, e.to_string()))?,
            ),
            // An account named by the call decides the harness, over the agent's own.
            None if p.account.is_some() => None,
            None => match agent.as_ref().map(|a| a.harness) {
                Some(HarnessPref::Fixed(h)) => Some(h),
                Some(HarnessPref::Auto) => None,
                None => defaults.harness,
            },
        };
        let model = p
            .model
            .clone()
            .or_else(|| agent.as_ref().and_then(|a| a.model.clone()))
            .or_else(|| defaults.model.clone());
        let effort = p
            .effort
            .clone()
            .or_else(|| agent.as_ref().and_then(|a| a.effort.clone()))
            .or_else(|| defaults.effort.clone());
        let allow = agent
            .as_ref()
            .map(|a| a.accounts.clone())
            .unwrap_or_default();
        if let Some(name) = &p.account
            && !allow.is_empty()
            && !allow.contains(name)
        {
            return Err(rpc(
                INVALID_PARAMS,
                format!(
                    "agent `{}` may only run on: {}",
                    p.agent.as_deref().unwrap_or_default(),
                    allow.join(", ")
                ),
            ));
        }

        // Harness and account both named: nothing to route. Otherwise the router fills in
        // what is open and keeps to what is named.
        let (account, choice_id, route, model, effort) = match (harness_pin, p.account.clone()) {
            (Some(h), Some(name)) => {
                let account = self.usable_account(&name, Some(h)).await?;
                (account, None, None, model, effort)
            }
            (harness, account_pin) => {
                let pins = RoutePins {
                    role: None,
                    harness: harness.map(|h| h.as_str().to_string()),
                    account: account_pin,
                    model: model.clone(),
                    effort: effort.clone(),
                    allow_accounts: allow,
                };
                let routed = self.route_run(&prompt, &pins, &[], None, true).await?;
                let mut found = None;
                let mut last = rpc(UNAVAILABLE, "no usable account");
                for name in &routed.result.ranked_accounts {
                    match self.usable_account(name, Some(routed.result.harness)).await {
                        Ok(a) => {
                            found = Some(a);
                            break;
                        }
                        Err(e) => last = e,
                    }
                }
                let account = found.ok_or(last)?;
                let r = routed.result;
                let plan = RoutePlan {
                    pins,
                    decision: routed.decision,
                };
                (
                    account,
                    Some(r.choice_id),
                    Some(plan),
                    Some(r.model).filter(|m| !m.is_empty()),
                    r.effort,
                )
            }
        };

        let want = p
            .isolation
            .or_else(|| agent.as_ref().and_then(|a| a.isolation))
            .unwrap_or(Isolation::Worktree);
        let probe = cwd.clone();
        let (mut warnings, isolation) = self
            .blocking(move || inspect_isolation(&probe, want))
            .await?;
        // What the agent file asks for and the chosen harness cannot do.
        if let Some(a) = &agent {
            warnings.extend(
                so_agents::gaps(a, account.harness)
                    .into_iter()
                    .map(|g| format!("agent `{}`: {g}", a.name)),
            );
        }
        Ok(Validated {
            harness: account.harness,
            account,
            prompt,
            cwd,
            isolation,
            model,
            effort,
            depth,
            timeout_ms,
            warnings,
            max_turns: p
                .max_turns
                .or_else(|| agent.as_ref().and_then(|a| a.max_turns)),
            review,
            choice_id,
            route,
        })
    }

    /// Creates a queued run and starts it when a slot is free. Returns the run id and the
    /// warnings for the caller.
    pub async fn spawn(&self, ctx: &ConnCtx, p: SpawnParams) -> R<(String, Vec<String>)> {
        let v = self.validate(ctx, &p).await?;
        let run_id = new_id();
        let warnings = v.warnings.clone();
        let row = RunRow {
            id: run_id.clone(),
            session_id: ctx.session_id.clone().unwrap_or_default(),
            parent_run_id: ctx.run_id.clone(),
            depth: i64::from(v.depth),
            name: p.name.clone(),
            agent: p.agent.clone(),
            choice_id: v.choice_id,
            harness: v.harness,
            account: Some(v.account.name.clone()),
            model: v.model,
            effort: v.effort,
            cwd: Some(v.cwd.to_string_lossy().into_owned()),
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state: RunState::Queued,
            review: v.review,
            output_schema: p.output_schema.as_ref().map(Value::to_string),
            result_json: None,
            error_kind: None,
            prompt: Some(v.prompt),
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            isolation: v.isolation,
            max_turns: v.max_turns,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: v.timeout_ms,
        };
        // Detached: a client that disconnects mid-call must not leave a queued run that
        // nothing will ever start.
        let mgr = self.clone();
        let checks = p.checks.clone();
        let route = v.route;
        tokio::spawn(async move { mgr.enqueue(row, checks, None, route).await })
            .await
            .map_err(internal)??;
        Ok((run_id, warnings))
    }

    /// Inserts the run and asks for a slot. `checks` and `fork_from` are written before the
    /// run exists, so the queue never starts it without them.
    pub(super) async fn enqueue(
        &self,
        row: RunRow,
        checks: Vec<String>,
        fork_from: Option<String>,
        route: Option<RoutePlan>,
    ) -> R<()> {
        let id = row.id.clone();
        let (pins, decision) = match route {
            Some(r) => (
                Some(serde_json::to_string(&r.pins).map_err(internal)?),
                Some(r.decision),
            ),
            None => (None, None),
        };
        let turn_prompt = row.prompt.clone();
        {
            let (rid, fork) = (id.clone(), fork_from);
            self.db(move |s| {
                if let Some(p) = &turn_prompt {
                    s.set_run_turn_prompt(&rid, p)?;
                }
                if !checks.is_empty() {
                    s.set_run_checks(&rid, &checks)?;
                }
                if let Some(p) = &pins {
                    s.set_run_route(&rid, p)?;
                }
                match &fork {
                    Some(n) => s.set_run_fork(&rid, n),
                    None => Ok(()),
                }
            })
            .await?;
        }
        let session = row.session_id.clone();
        let data = json!({
            "harness": row.harness,
            "account": row.account,
            "name": row.name,
            "depth": row.depth,
            "parent_run": row.parent_run_id,
        });
        lock(&self.inner.runs).insert(id.clone(), RunRt::default());
        let max = self.config().limits.max_children as usize;
        let inserted = self.db(move |s| s.insert_run_checked(&row, max)).await;
        match inserted {
            Ok(InsertRun::Inserted) => {}
            Ok(InsertRun::ChildCap { active }) => {
                lock(&self.inner.runs).remove(&id);
                return Err(rpc(
                    CAP_EXCEEDED,
                    format!(
                        "this run already has {active} active children (limits.max_children = {max})"
                    ),
                ));
            }
            Err(e) => {
                lock(&self.inner.runs).remove(&id);
                return Err(e);
            }
        }
        self.emit(&session, Some(&id), EventType::RunQueued, data)
            .await;
        if let Some(decision) = decision {
            self.emit(&session, Some(&id), EventType::RouteDecision, decision)
                .await;
        }
        self.drain_once().await;
        Ok(())
    }
}

/// `checks` are `ci`, `pr_threads` or `cmd:<shell command>`, and only make sense with
/// `review: true`.
fn validate_checks(review: bool, checks: &[String]) -> R<()> {
    if !checks.is_empty() && !review {
        return Err(rpc(INVALID_PARAMS, "checks need review: true"));
    }
    for c in checks {
        let ok = c == "ci"
            || c == "pr_threads"
            || c.strip_prefix("cmd:")
                .is_some_and(|cmd| !cmd.trim().is_empty());
        if !ok {
            return Err(rpc(
                INVALID_PARAMS,
                format!("unknown check `{c}`: use ci, pr_threads or cmd:<command>"),
            ));
        }
    }
    Ok(())
}

/// The directory a spawn works in: `requested` relative to the session's, which falls
/// back to the supervisor's own.
fn resolve_cwd(base: Option<PathBuf>, requested: Option<String>) -> R<PathBuf> {
    let base = base
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    let cwd = match requested {
        Some(c) => base.join(c),
        None => base,
    };
    let cwd = cwd.canonicalize().map_err(|e| {
        rpc(
            INVALID_PARAMS,
            format!("cwd {} is not usable: {e}", cwd.display()),
        )
    })?;
    if !cwd.is_dir() {
        return Err(rpc(
            INVALID_PARAMS,
            format!("cwd {} is not a directory", cwd.display()),
        ));
    }
    Ok(cwd)
}

/// Decides whether worktree isolation is possible in `cwd`.
fn inspect_isolation(cwd: &std::path::Path, want: Isolation) -> (Vec<String>, Isolation) {
    let mut warnings = Vec::new();
    let mut isolation = want;
    if want == Isolation::Worktree {
        match worktree::repo_info(cwd) {
            Some(i) if i.has_head => {
                if i.dirty {
                    warnings.push(format!(
                        "{} has uncommitted changes; they are not carried over to the worktree",
                        i.toplevel.display()
                    ));
                }
            }
            Some(i) => {
                warnings.push(format!(
                    "{} has no commits; running without worktree isolation",
                    i.toplevel.display()
                ));
                isolation = Isolation::None;
            }
            None => {
                warnings.push(format!(
                    "{} is not a git repository; running without worktree isolation",
                    cwd.display()
                ));
                isolation = Isolation::None;
            }
        }
    }
    (warnings, isolation)
}
