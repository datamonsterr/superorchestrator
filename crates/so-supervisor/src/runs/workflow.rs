//! Workflows (SPEC 11): the QuickJS engine of `so-workflow` driving runs through the run
//! manager.
//!
//! The engine owns the script, the result cache and the retry policy; this module is its
//! host. A step is one run: spawned as a child of the caller (so depth and `max_children`
//! apply), waited on, and cancelled if the engine drops the step. Failover between accounts
//! is the run's own (SPEC 8.5); when none is left the step waits for the earliest reset.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, NOT_FOUND,
    RpcError, RunWorkflowParams, RunWorkflowResult, SpawnParams, UNAVAILABLE, WorkflowEventData,
    WorkflowRunView, WorkflowStatusResult, WorkflowStep,
};
use so_core::{ErrorKind, EventType, RunState, SessionKind, new_id};
use so_store::{EventQuery, SessionRow, WorkflowRunRow, WorkflowStepRow};
use so_workflow::{
    BoxFut, CancelToken, HostError, RunOptions, SavedWorkflows, StepEvent, StepOutcome,
    StepRequest, WorkflowEngine, WorkflowError, WorkflowHost,
};
use tokio::sync::broadcast;

use super::instructions::MAX_DEPTH;
use super::view::WaitMode;
use super::{R, RunManager, rpc};
use crate::dispatch::{ConnCtx, internal};
use crate::handlers::route::NO_ELIGIBLE;

/// How long a step waits before it tries to start again when its parent has too many
/// active children.
const CHILD_CAP_RETRY: Duration = Duration::from_secs(5);
/// Most flags read when looking for the next reset.
const MAX_LIST: usize = 1000;

/// How a workflow ended.
struct Outcome {
    state: &'static str,
    result: Option<Value>,
    error: Option<String>,
}

/// A started workflow: its id and the task that ends with its outcome.
struct Started {
    id: String,
    done: tokio::task::JoinHandle<Outcome>,
}

fn step_name(index: usize, phase: Option<&str>) -> String {
    phase.map_or_else(|| format!("step {}", index + 1), str::to_string)
}

fn step_view(row: &WorkflowStepRow) -> WorkflowStep {
    let index = u32::try_from(row.step_index).unwrap_or(0);
    WorkflowStep {
        index,
        name: step_name(row.step_index as usize, row.phase.as_deref()),
        phase: row.phase.clone(),
        state: row.state.clone(),
        run_id: row.run_id.clone(),
        started_at: Some(row.started_at),
        finished_at: row.finished_at,
    }
}

impl RunManager {
    /// Workflow run and step changes, from the moment of subscribing.
    pub fn subscribe_workflows(&self) -> broadcast::Receiver<WorkflowEventData> {
        self.inner.workflow_bus.subscribe()
    }

    fn publish_workflow(
        &self,
        id: &str,
        name: &str,
        state: &str,
        step: Option<WorkflowStep>,
        session_id: Option<&str>,
    ) {
        let _ = self.inner.workflow_bus.send(WorkflowEventData {
            id: id.to_string(),
            name: name.to_string(),
            state: state.to_string(),
            step,
            session_id: session_id.map(str::to_string),
            ts: Utc::now(),
        });
    }

    /// `run_workflow`: runs a saved workflow or an inline script as the caller's steps. With
    /// `wait` (the default) it returns when the workflow has finished; the workflow itself
    /// runs on a task of its own, so a caller that goes away does not stop it.
    pub async fn run_workflow(&self, ctx: &ConnCtx, p: RunWorkflowParams) -> R<RunWorkflowResult> {
        let session_id = ctx
            .session_id
            .clone()
            .ok_or_else(|| rpc(INVALID_REQUEST, "hello required"))?;
        // Its steps are runs one level down.
        if ctx.depth.saturating_add(1) > MAX_DEPTH {
            return Err(rpc(
                DEPTH_LIMIT,
                format!(
                    "a run at depth {} cannot start a workflow: its agents would be deeper than {MAX_DEPTH}",
                    ctx.depth
                ),
            ));
        }
        let sid = session_id.clone();
        let session = self
            .db(move |s| s.get_session(&sid))
            .await?
            .ok_or_else(|| rpc(NOT_FOUND, format!("session not found: {session_id}")))?;
        let cwd = session
            .cwd
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let (name, source) = self.resolve_workflow(&cwd, &p).await?;
        let started = self
            .start_workflow(
                ctx.detached(),
                &session_id,
                name,
                source,
                p.args.unwrap_or(Value::Null),
                cwd,
                false,
            )
            .await?;
        if !p.wait {
            return Ok(RunWorkflowResult {
                id: started.id,
                state: Some("running".into()),
                result: None,
                error: None,
                session_id: Some(session_id),
            });
        }
        let out = started.done.await.map_err(internal)?;
        Ok(RunWorkflowResult {
            id: started.id,
            state: Some(out.state.into()),
            result: out.result,
            error: out.error,
            session_id: Some(session_id),
        })
    }

    /// Starts a saved workflow for the web UI: a session of its own holds the steps, and the
    /// id comes back at once.
    pub async fn run_workflow_ui(
        &self,
        cwd: PathBuf,
        name: &str,
        args: Option<Value>,
    ) -> R<RunWorkflowResult> {
        let p = RunWorkflowParams {
            name: Some(name.to_string()),
            ..RunWorkflowParams::default()
        };
        let (name, source) = self.resolve_workflow(&cwd, &p).await?;
        let session = SessionRow {
            id: new_id(),
            kind: SessionKind::Orchestrator,
            client: "web".into(),
            cwd: Some(cwd.to_string_lossy().into_owned()),
            account: None,
            pid: None,
            started_at: Utc::now(),
            ended_at: None,
            status: "active".into(),
        };
        let session_id = session.id.clone();
        self.db(move |s| s.insert_session(&session)).await?;
        // The session stays open until the workflow ends.
        self.session_connected(&session_id);
        let started = self
            .start_workflow(
                ConnCtx::acting_in(&session_id),
                &session_id,
                name,
                source,
                args.unwrap_or(Value::Null),
                cwd,
                true,
            )
            .await;
        match started {
            Ok(s) => Ok(RunWorkflowResult {
                id: s.id,
                state: Some("running".into()),
                result: None,
                error: None,
                session_id: Some(session_id),
            }),
            Err(e) => {
                self.session_disconnected(&session_id);
                self.maybe_end_session(&session_id).await;
                Err(e)
            }
        }
    }

    /// The script of a saved workflow, or of an inline one under a name derived from it (so
    /// two different inline scripts never share cached steps).
    async fn resolve_workflow(&self, cwd: &Path, p: &RunWorkflowParams) -> R<(String, String)> {
        match (&p.name, &p.script) {
            (Some(name), None) => {
                let (dir, cwd, name) = (
                    self.inner.paths.workflows_dir(),
                    cwd.to_path_buf(),
                    name.clone(),
                );
                self.blocking(move || SavedWorkflows::new(dir, cwd).load(&name))
                    .await?
                    .map(|(w, src)| (w.name, src))
                    .map_err(|e| match e {
                        WorkflowError::NotFound(n) => {
                            rpc(NOT_FOUND, format!("no saved workflow named `{n}`"))
                        }
                        other => rpc(UNAVAILABLE, other.to_string()),
                    })
            }
            (None, Some(script)) if !script.trim().is_empty() => {
                let digest = Sha256::digest(script.as_bytes());
                let short: String = digest.iter().take(4).map(|b| format!("{b:02x}")).collect();
                Ok((format!("inline-{short}"), script.clone()))
            }
            _ => Err(rpc(
                INVALID_PARAMS,
                "give either `name` (a saved workflow) or `script`, not both and not neither",
            )),
        }
    }

    /// Records the run, then runs the script on a task of its own.
    #[allow(clippy::too_many_arguments)] // one start, spelled out
    async fn start_workflow(
        &self,
        ctx: ConnCtx,
        session_id: &str,
        name: String,
        source: String,
        args: Value,
        cwd: PathBuf,
        hold_session: bool,
    ) -> R<Started> {
        let id = new_id();
        let row = WorkflowRunRow {
            id: id.clone(),
            name: name.clone(),
            session_id: Some(session_id.to_string()),
            state: "running".into(),
            args_json: args.to_string(),
            started_at: Utc::now(),
            finished_at: None,
            result_json: None,
            error: None,
        };
        self.db(move |s| s.insert_workflow_run(&row)).await?;
        self.publish_workflow(&id, &name, "running", None, Some(session_id));

        let host = Arc::new(StepHost {
            mgr: self.clone(),
            ctx,
            session_id: session_id.to_string(),
            workflow_id: id.clone(),
            name: name.clone(),
            cwd: cwd.clone(),
        });
        let engine = WorkflowEngine::new(self.store().clone());
        let (mgr, sid, wid) = (self.clone(), session_id.to_string(), id.clone());
        let done = tokio::spawn(async move {
            let opts = RunOptions {
                run_id: Some(wid.clone()),
                cancel: CancelToken::new(),
                cwd: Some(cwd),
                fresh: false,
            };
            let ran = engine.run_with(opts, &source, &name, args, host).await;
            let out = match ran {
                Ok((_, v)) => Outcome {
                    state: "succeeded",
                    result: Some(v),
                    error: None,
                },
                Err(WorkflowError::Cancelled) => Outcome {
                    state: "cancelled",
                    result: None,
                    error: Some("workflow cancelled".into()),
                },
                Err(e) => Outcome {
                    state: "failed",
                    result: None,
                    error: Some(e.to_string()),
                },
            };
            mgr.publish_workflow(&wid, &name, out.state, None, Some(&sid));
            if hold_session {
                mgr.session_disconnected(&sid);
                mgr.maybe_end_session(&sid).await;
            }
            out
        });
        Ok(Started { id, done })
    }

    /// `workflow_status`: state, steps and result of a run, by id or unique id prefix.
    pub async fn workflow_status(&self, id: &str) -> R<WorkflowStatusResult> {
        let p = id.to_string();
        let (row, steps) = self
            .db(move |s| {
                let row = s.get_workflow_run_by_prefix(&p)?;
                let steps = s.list_workflow_steps(&row.id)?;
                Ok((row, steps))
            })
            .await?;
        Ok(WorkflowStatusResult {
            id: row.id,
            name: row.name,
            state: row.state,
            started_at: Some(row.started_at),
            finished_at: row.finished_at,
            session_id: row.session_id,
            result: row
                .result_json
                .as_deref()
                .and_then(|j| serde_json::from_str(j).ok()),
            error: row.error,
            steps: steps.iter().map(step_view).collect(),
        })
    }

    /// Workflow runs, newest first.
    pub async fn list_workflow_runs(&self, limit: usize) -> R<Vec<WorkflowRunView>> {
        let rows = self.db(move |s| s.list_workflow_runs(limit)).await?;
        Ok(rows
            .into_iter()
            .map(|r| WorkflowRunView {
                id: r.id,
                name: r.name,
                state: r.state,
                started_at: r.started_at,
                finished_at: r.finished_at,
                session_id: r.session_id,
            })
            .collect())
    }

    /// When an account may take a step again: the earliest end of a cooldown or exhausted
    /// window. `None` when nothing is waiting for a reset, so waiting would not help.
    async fn earliest_reset(&self) -> Option<DateTime<Utc>> {
        let now = Utc::now();
        let (flags, headroom) = self
            .db(move |s| Ok((s.active_flags(now, MAX_LIST)?, s.list_headroom(10_000)?)))
            .await
            .ok()?;
        let flagged = flags.iter().map(|f| f.until);
        let exhausted = headroom
            .iter()
            .filter(|h| h.status == so_core::HeadroomStatus::Exhausted)
            .filter_map(|h| {
                [h.five_hour_resets_at, h.weekly_resets_at]
                    .into_iter()
                    .flatten()
                    .filter(|t| *t > now)
                    .min()
            });
        flagged.chain(exhausted).min()
    }
}

/// Cancels a step's run when the step is dropped before it finished (the engine does that
/// when the script ends, is cancelled or times out).
struct CancelOnDrop {
    mgr: RunManager,
    run_id: String,
    armed: bool,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (mgr, id) = (self.mgr.clone(), self.run_id.clone());
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                let _ = mgr.cancel(&id).await;
            });
        }
    }
}

/// The engine's view of the supervisor for one workflow run.
struct StepHost {
    mgr: RunManager,
    /// The caller, detached: the workflow outlives the connection that began it.
    ctx: ConnCtx,
    session_id: String,
    workflow_id: String,
    name: String,
    cwd: PathBuf,
}

/// The spawn options of one `agent()` call. `opts` are SPEC 7.2's, in snake_case, plus the
/// engine's own `stall_ms`.
fn spawn_params(req: &StepRequest, workflow: &str, cwd: &Path) -> Result<SpawnParams, String> {
    let mut opts = match &req.opts {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    opts.remove("stall_ms");
    // A workflow has nobody to accept a review or to wait for.
    opts.remove("wait");
    if opts.get("review") == Some(&Value::Bool(true)) {
        return Err("review is not available in workflows: nobody can accept the run".into());
    }
    let dir = match opts.remove("cwd") {
        Some(Value::String(rel)) => cwd.join(rel),
        Some(_) => return Err("cwd must be a string".into()),
        None => cwd.to_path_buf(),
    };
    opts.insert("cwd".into(), json!(dir.to_string_lossy()));
    opts.insert("prompt".into(), json!(req.prompt));
    opts.entry("name").or_insert_with(|| {
        json!(format!(
            "{workflow}: {}",
            step_name(req.step_index, req.phase.as_deref())
        ))
    });
    serde_json::from_value(Value::Object(opts)).map_err(|e| format!("invalid agent() options: {e}"))
}

impl StepHost {
    fn failed(msg: impl Into<String>) -> HostError {
        HostError::Failed(msg.into())
    }

    /// Spawns the step's run and waits for it.
    async fn run_step(&self, req: &StepRequest) -> Result<StepOutcome, HostError> {
        let params = spawn_params(req, &self.name, &self.cwd).map_err(Self::failed)?;
        let stall = req.stall_ms.map(Duration::from_millis);
        let spawned = self.mgr.spawn(&self.ctx, params).await;
        let (run_id, _warnings) = match spawned {
            Ok(v) => v,
            Err(e) => return self.spawn_failed(e).await,
        };
        let mut guard = CancelOnDrop {
            mgr: self.mgr.clone(),
            run_id: run_id.clone(),
            armed: true,
        };
        // Tie the run to its step now, so `workflow_status` shows it while it works.
        {
            let (wf, idx, rid) = (
                self.workflow_id.clone(),
                req.step_index as i64,
                run_id.clone(),
            );
            let _ = self
                .mgr
                .db(move |s| s.update_workflow_step(&wf, idx, "running", Some(&rid), None))
                .await;
        }
        let ids = std::slice::from_ref(&run_id);
        let mut sub = self
            .mgr
            .subscribe(std::slice::from_ref(&self.session_id), Some(ids));
        loop {
            let waited = self
                .mgr
                .wait_runs(&self.ctx, ids, &mut sub, stall, WaitMode::Finish)
                .await
                .map_err(|e| HostError::Fatal(e.message))?;
            if !waited.finished.is_empty() {
                break;
            }
            // Timed out: stalled only if the run is working and has been quiet that long.
            if let Some(window) = stall
                && self.is_stalled(&run_id, window).await
            {
                // Cancelling can take a while (the harness is asked to stop first); the
                // restart does not wait for it.
                guard.armed = false;
                let (mgr, id) = (self.mgr.clone(), run_id.clone());
                tokio::spawn(async move {
                    let _ = mgr.cancel(&id).await;
                });
                return Ok(StepOutcome::Stalled {
                    run_id: Some(run_id),
                });
            }
        }
        guard.armed = false;
        self.finished(&run_id).await
    }

    /// A run that works (not queued, not waiting on a call of its own) and has had no event
    /// for `window`.
    async fn is_stalled(&self, run_id: &str, window: Duration) -> bool {
        let id = run_id.to_string();
        let q = EventQuery {
            run_id: Some(id.clone()),
            newest_first: true,
            limit: 1,
            ..EventQuery::default()
        };
        let Ok((row, last)) = self
            .mgr
            .db(move |s| Ok((s.get_run(&id)?, s.query_events(&q)?.pop())))
            .await
        else {
            return false;
        };
        let working = row.is_some_and(|r| {
            matches!(
                r.state,
                RunState::Running | RunState::Stalled | RunState::Escalated
            )
        });
        let quiet = last.is_none_or(|e| {
            (Utc::now() - e.ts)
                .to_std()
                .is_ok_and(|idle| idle >= window)
        });
        working && quiet
    }

    /// What a finished run means for its step.
    async fn finished(&self, run_id: &str) -> Result<StepOutcome, HostError> {
        let row = self
            .mgr
            .resolve_run(run_id)
            .await
            .map_err(|e| HostError::Fatal(e.message))?;
        let id = Some(row.id.clone());
        match row.state {
            RunState::Succeeded | RunState::PendingReview => Ok(StepOutcome::Done {
                result: RunManager::result_of(&row).unwrap_or(Value::Null),
                run_id: id,
            }),
            RunState::Cancelled => Err(Self::failed(format!("run {} was cancelled", row.id))),
            _ => {
                let why = self.failure_reason(&row.id).await;
                // Out of quota everywhere: the step waits for the earliest reset.
                if matches!(
                    row.error_kind,
                    Some(ErrorKind::Quota | ErrorKind::RateLimit)
                ) && let Some(retry_at) = self.mgr.earliest_reset().await
                {
                    return Ok(StepOutcome::Unavailable { retry_at });
                }
                Err(Self::failed(format!("run {} failed: {why}", row.id)))
            }
        }
    }

    async fn failure_reason(&self, run_id: &str) -> String {
        let id = run_id.to_string();
        self.mgr
            .db(move |s| s.latest_event_of_type(&id, EventType::RunFinished))
            .await
            .ok()
            .flatten()
            .and_then(|e| e.data["reason"].as_str().map(str::to_string))
            .unwrap_or_else(|| "no reason recorded".to_string())
    }

    /// A step that could not even start.
    async fn spawn_failed(&self, e: RpcError) -> Result<StepOutcome, HostError> {
        match e.code {
            CAP_EXCEEDED => Ok(StepOutcome::Unavailable {
                retry_at: Utc::now()
                    + chrono::Duration::from_std(CHILD_CAP_RETRY).unwrap_or_default(),
            }),
            UNAVAILABLE if e.data.as_ref().is_some_and(|d| d["reason"] == NO_ELIGIBLE) => {
                match self.mgr.earliest_reset().await {
                    Some(retry_at) => Ok(StepOutcome::Unavailable { retry_at }),
                    None => Err(Self::failed(e.message)),
                }
            }
            INTERNAL_ERROR => Err(HostError::Fatal(e.message)),
            _ => Err(Self::failed(e.message)),
        }
    }

    /// A workflow-level progress event for the session: `step`/`status` are what MCP clients
    /// show as progress.
    async fn progress(&self, run_id: Option<&str>, data: Value) {
        let delivered = self
            .mgr
            .emit_delivered(&self.session_id, run_id, EventType::Progress, data)
            .await;
        if let Some(d) = delivered {
            self.ctx.notifier.notify("event", d.to_json()).await;
        }
    }
}

impl WorkflowHost for StepHost {
    fn spawn_and_wait<'a>(
        &'a self,
        req: &'a StepRequest,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>> {
        Box::pin(self.run_step(req))
    }

    fn restart<'a>(
        &'a self,
        req: &'a StepRequest,
        stalled_run_id: Option<&'a str>,
        _attempt: u32,
    ) -> BoxFut<'a, Result<StepOutcome, HostError>> {
        Box::pin(async move {
            // The stalled run is being cancelled since it was found; nothing to wait for.
            let _ = stalled_run_id;
            self.run_step(req).await
        })
    }

    fn log<'a>(&'a self, msg: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let data = json!({
                "workflow_id": self.workflow_id,
                "workflow": self.name,
                "step": "log",
                "status": super::lifecycle::excerpt(msg, 500),
            });
            self.progress(None, data).await;
        })
    }

    fn phase_started<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let data = json!({
                "workflow_id": self.workflow_id,
                "workflow": self.name,
                "step": name,
                "status": "started",
            });
            self.progress(None, data).await;
        })
    }

    fn phase_finished<'a>(&'a self, name: &'a str) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let data = json!({
                "workflow_id": self.workflow_id,
                "workflow": self.name,
                "step": name,
                "status": "finished",
            });
            self.progress(None, data).await;
        })
    }

    fn step_status<'a>(&'a self, event: &'a StepEvent) -> BoxFut<'a, ()> {
        Box::pin(async move {
            let name = step_name(event.step_index, event.phase.as_deref());
            let data = json!({
                "workflow_id": self.workflow_id,
                "workflow": self.name,
                "step": name,
                "status": event.state,
                "step_index": event.step_index,
                "step_run": event.run_id,
            });
            self.progress(None, data).await;
            let step = WorkflowStep {
                index: u32::try_from(event.step_index).unwrap_or(0),
                name,
                phase: event.phase.clone(),
                state: event.state.clone(),
                run_id: event.run_id.clone(),
                started_at: None,
                finished_at: None,
            };
            self.mgr.publish_workflow(
                &self.workflow_id,
                &self.name,
                "running",
                Some(step),
                Some(&self.session_id),
            );
        })
    }
}
