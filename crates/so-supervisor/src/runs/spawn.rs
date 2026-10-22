//! `spawn_agent`: validate the request, insert a queued run and ask for a slot.

use std::path::PathBuf;

use chrono::Utc;
use serde_json::{Value, json};
use so_accounts::{LaunchMode, build_launch, resolve_binary};
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, INVALID_PARAMS, INVALID_REQUEST, NOT_FOUND, SpawnParams, UNAVAILABLE,
};
use so_core::{AccountFile, EventType, Harness, HumanDuration, Isolation, RunState, new_id};
use so_harness::AnyDriver;
use so_store::{InsertRun, RunRow};

use super::instructions::MAX_DEPTH;
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
        let Some(prompt) = p.prompt.clone().filter(|s| !s.trim().is_empty()) else {
            return Err(if p.agent.is_some() {
                rpc(UNAVAILABLE, "custom agents are not available yet")
            } else {
                rpc(INVALID_PARAMS, "prompt is required")
            });
        };
        let harness = match p.harness.as_deref() {
            None | Some("auto") => return Err(rpc(UNAVAILABLE, "routing not available yet")),
            Some(h) => h
                .parse::<Harness>()
                .map_err(|e| rpc(INVALID_PARAMS, e.to_string()))?,
        };
        let Some(account_name) = p.account.clone() else {
            return Err(rpc(UNAVAILABLE, "routing not available yet"));
        };
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
        let account = self.load_account(&account_name).await?;
        if account.harness != harness {
            return Err(rpc(
                INVALID_PARAMS,
                format!(
                    "account {} is a {} account, not {harness}",
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
        AnyDriver::for_harness(harness, self.inner.driver_cfg)
            .map_err(|e| rpc(UNAVAILABLE, e.to_string()))?;
        self.launch_for(&account)?;

        let sid = session_id.clone();
        let session = self
            .db(move |s| s.get_session(&sid))
            .await?
            .ok_or_else(|| rpc(NOT_FOUND, format!("session not found: {session_id}")))?;
        let requested = p.cwd.clone();
        let (cwd, warnings, isolation) = {
            let want = p.isolation.unwrap_or(Isolation::Worktree);
            let base = session.cwd.map(PathBuf::from);
            self.blocking(move || inspect_cwd(base, requested, want))
                .await??
        };
        let defaults = &self.config().defaults;
        Ok(Validated {
            harness,
            account,
            prompt,
            cwd,
            isolation,
            model: p.model.clone().or_else(|| defaults.model.clone()),
            effort: p.effort.clone().or_else(|| defaults.effort.clone()),
            depth,
            timeout_ms,
            warnings,
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
            choice_id: None,
            harness: v.harness,
            account: Some(v.account.name.clone()),
            model: v.model,
            effort: v.effort,
            cwd: Some(v.cwd.to_string_lossy().into_owned()),
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state: RunState::Queued,
            review: p.review,
            output_schema: p.output_schema.as_ref().map(Value::to_string),
            result_json: None,
            error_kind: None,
            prompt: Some(v.prompt),
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            isolation: v.isolation,
            max_turns: p.max_turns,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: v.timeout_ms,
        };
        // Detached: a client that disconnects mid-call must not leave a queued run that
        // nothing will ever start.
        let mgr = self.clone();
        tokio::spawn(async move { mgr.enqueue(row).await })
            .await
            .map_err(internal)??;
        Ok((run_id, warnings))
    }

    async fn enqueue(&self, row: RunRow) -> R<()> {
        let id = row.id.clone();
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
        self.drain_once().await;
        Ok(())
    }
}

/// Resolves the working directory and decides whether worktree isolation is possible.
fn inspect_cwd(
    base: Option<PathBuf>,
    requested: Option<String>,
    want: Isolation,
) -> R<(PathBuf, Vec<String>, Isolation)> {
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
    let mut warnings = Vec::new();
    let mut isolation = want;
    if want == Isolation::Worktree {
        match worktree::repo_info(&cwd) {
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
    Ok((cwd, warnings, isolation))
}
