//! `fork_agent`: a new run that continues a copy of a finished run's native session.

use chrono::Utc;
use so_core::proto::{
    DEPTH_LIMIT, ForkParams, INVALID_PARAMS, INVALID_REQUEST, SpawnResult, UNAVAILABLE,
};
use so_core::{Isolation, RunState, new_id};
use so_harness::{AnyDriver, Driver};
use so_store::RunRow;

use super::instructions::MAX_DEPTH;
use super::{R, RunManager, rpc};
use crate::dispatch::ConnCtx;

impl RunManager {
    /// The new run has the source's harness, account, model, effort, session and parent.
    /// It runs in the source's directory (its worktree while that still exists) without a
    /// worktree of its own, because the native session belongs to that directory.
    pub async fn fork_run(&self, ctx: &ConnCtx, p: ForkParams) -> R<SpawnResult> {
        if p.prompt.trim().is_empty() {
            return Err(rpc(INVALID_PARAMS, "prompt must not be empty"));
        }
        let src = self.resolve_run(&p.run).await?;
        if ctx.session_id.as_deref() != Some(src.session_id.as_str()) {
            return Err(rpc(INVALID_PARAMS, "that run belongs to another session"));
        }
        // The fork takes the source's place under its parent, so only that parent may
        // fork it, and the depth limit still applies (`max_children` is enforced when the
        // run is inserted).
        if ctx.run_id != src.parent_run_id {
            return Err(rpc(
                INVALID_PARAMS,
                "only the run's parent (or the orchestrator, for a top-level run) can fork it",
            ));
        }
        if u8::try_from(src.depth).unwrap_or(u8::MAX) > MAX_DEPTH {
            return Err(rpc(DEPTH_LIMIT, "the run is deeper than the depth limit"));
        }
        let driver = AnyDriver::for_harness(src.harness, self.inner.driver_cfg)
            .map_err(|e| rpc(UNAVAILABLE, e.to_string()))?;
        if !driver.supports_fork() {
            // The request asks for something the harness cannot do: not an outage.
            return Err(rpc(
                INVALID_PARAMS,
                format!("{} cannot fork a session", driver.harness()),
            ));
        }
        if !matches!(
            src.state,
            RunState::Succeeded | RunState::Failed | RunState::PendingReview
        ) {
            return Err(rpc(
                INVALID_REQUEST,
                format!(
                    "only a finished run can be forked; the run is {}",
                    src.state
                ),
            ));
        }
        let native = src
            .native_session_id
            .clone()
            .ok_or_else(|| rpc(UNAVAILABLE, "the run has no native session to fork"))?;
        let account_name = src
            .account
            .clone()
            .ok_or_else(|| rpc(UNAVAILABLE, "the run has no account"))?;
        let account = self.load_account(&account_name).await?;
        if !account.enabled {
            return Err(rpc(
                UNAVAILABLE,
                format!("account {account_name} is disabled"),
            ));
        }
        let dirs: Vec<std::path::PathBuf> = [src.worktree_path.as_ref(), src.cwd.as_ref()]
            .into_iter()
            .flatten()
            .map(std::path::PathBuf::from)
            .collect();
        let cwd = self
            .blocking(move || dirs.into_iter().find(|d| d.is_dir()))
            .await?
            .ok_or_else(|| rpc(UNAVAILABLE, "the run's working directory is gone"))?;
        let id = new_id();
        let label = src.name.clone().unwrap_or_else(|| id_prefix(&src.id));
        let row = RunRow {
            id: id.clone(),
            session_id: src.session_id.clone(),
            parent_run_id: src.parent_run_id.clone(),
            depth: src.depth,
            name: Some(format!("fork of {label}")),
            agent: None,
            choice_id: None,
            harness: src.harness,
            account: Some(account_name),
            model: src.model.clone(),
            effort: src.effort.clone(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state: RunState::Queued,
            review: false,
            output_schema: None,
            result_json: None,
            error_kind: None,
            prompt: Some(p.prompt),
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            isolation: Isolation::None,
            max_turns: src.max_turns,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: src.timeout_ms,
        };
        let mgr = self.clone();
        tokio::spawn(async move { mgr.enqueue(row, Vec::new(), Some(native)).await })
            .await
            .map_err(crate::dispatch::internal)??;
        let row = self.resolve_run(&id).await?;
        Ok(SpawnResult {
            run_id: id,
            state: row.state,
            result: None,
            warnings: Vec::new(),
            error_kind: None,
        })
    }
}

fn id_prefix(id: &str) -> String {
    id.chars().take(8).collect()
}
