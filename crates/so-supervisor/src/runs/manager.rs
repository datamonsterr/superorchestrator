//! The run manager: spawn, queue, start, finish and cancel (SPEC 7).
//!
//! A run is created `queued` and started by [`RunManager::drain`], which claims a slot in
//! the store (account and global caps) and starts the run on a detached task. Everything
//! that ends a run goes through [`RunManager::finish`], so cleanup happens exactly once.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use so_accounts::{HostEnv, LaunchMode, build_launch, resolve_binary};
use so_core::proto::{
    CAP_EXCEEDED, DEPTH_LIMIT, INTERNAL_ERROR, INVALID_PARAMS, INVALID_REQUEST, JoinResult,
    NOT_FOUND, RpcError, RunView, SpawnParams, UNAVAILABLE, UsageTotals,
};
use so_core::{
    AccountFile, Config, ErrorKind, Event, EventType, Harness, Isolation, Paths, RunState, new_id,
};
use so_harness::{
    AnyDriver, Driver, DriverConfig, JoinCommand, JoinInfo, LaunchSpec, McpServerSpec, RunControl,
    RunHandle,
};
use so_store::{FinishPatch, RunPatch, RunRow, Store, StoreError};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::instructions::{MAX_DEPTH, child_instructions};
use super::pump::PumpCtx;
use super::worktree;
use crate::dispatch::{ConnCtx, internal};

pub(crate) type R<T> = Result<T, RpcError>;

const RECENT_EVENTS: usize = 50;
const ALL_EVENTS: usize = 1_000_000;
const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_secs(1);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn rpc(code: i64, msg: impl Into<String>) -> RpcError {
    RpcError::new(code, msg)
}

fn store_rpc(e: StoreError) -> RpcError {
    match e {
        StoreError::NotFound { .. } => rpc(NOT_FOUND, e.to_string()),
        StoreError::AmbiguousPrefix { .. } => rpc(INVALID_PARAMS, e.to_string()),
        other => rpc(INTERNAL_ERROR, other.to_string()),
    }
}

/// What a queued run needs besides its row.
#[derive(Debug, Clone)]
pub(crate) struct SpawnPlan {
    pub cwd: PathBuf,
    pub isolation: Isolation,
    pub max_turns: Option<u32>,
}

/// A run with a harness process.
pub(crate) struct LiveRun {
    pub control: RunControl,
    pub join: JoinInfo,
    #[allow(dead_code)] // owned so the pump is tied to the run; finishing detaches it
    pub task: JoinHandle<()>,
}

struct Inner {
    store: Store,
    config: Config,
    paths: Paths,
    host: Option<HostEnv>,
    socket_path: PathBuf,
    mcp_exe: PathBuf,
    driver_cfg: DriverConfig,
    live: Mutex<HashMap<String, LiveRun>>,
    plans: Mutex<HashMap<String, SpawnPlan>>,
    /// Runs from creation until `finish` has completed, for the activity probe.
    active: Mutex<HashSet<String>>,
    /// Runs in `finish`, with a count: a waiter must not see them as done before their
    /// closing events and cleanup are written.
    finalizing: Mutex<HashMap<String, usize>>,
    cancelling: Mutex<HashSet<String>>,
    /// Orchestrator sessions with a connected client.
    connected: Mutex<HashSet<String>>,
    /// Event subscribers per session.
    subs: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Event>>>>,
    drain: tokio::sync::Mutex<()>,
    finished: watch::Sender<u64>,
}

/// Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct RunManager {
    inner: Arc<Inner>,
}

/// Result of waiting on runs.
#[derive(Debug, Default)]
pub struct WaitOutcome {
    pub finished: Vec<String>,
    pub updates: Vec<Event>,
    pub timed_out: bool,
}

struct Validated {
    harness: Harness,
    account: AccountFile,
    prompt: String,
    cwd: PathBuf,
    isolation: Isolation,
    model: Option<String>,
    effort: Option<String>,
    depth: u8,
    warnings: Vec<String>,
}

impl RunManager {
    pub(crate) fn new(
        store: Store,
        config: Config,
        paths: Paths,
        host: Option<HostEnv>,
        socket_path: PathBuf,
        mcp_exe: Option<PathBuf>,
    ) -> Self {
        let mcp_exe = mcp_exe
            .or_else(|| std::env::current_exe().ok())
            .unwrap_or_else(|| PathBuf::from("superorchestrator"));
        Self {
            inner: Arc::new(Inner {
                store,
                config,
                paths,
                host,
                socket_path,
                mcp_exe,
                driver_cfg: DriverConfig::default(),
                live: Mutex::default(),
                plans: Mutex::default(),
                active: Mutex::default(),
                finalizing: Mutex::default(),
                cancelling: Mutex::default(),
                connected: Mutex::default(),
                subs: Mutex::default(),
                drain: tokio::sync::Mutex::new(()),
                finished: watch::channel(0).0,
            }),
        }
    }

    pub(crate) fn config(&self) -> &Config {
        &self.inner.config
    }

    pub(crate) fn host(&self) -> Option<&HostEnv> {
        self.inner.host.as_ref()
    }

    pub(crate) fn store(&self) -> &Store {
        &self.inner.store
    }

    // ------------------------------------------------------------ store helpers

    pub(crate) async fn db<T, F>(&self, f: F) -> R<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
    {
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(internal)?
            .map_err(store_rpc)
    }

    async fn db_raw<T, F>(&self, f: F) -> so_store::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
    {
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| StoreError::Io(std::io::Error::other(e.to_string())))?
    }

    /// Looks a run up by id or unique id prefix.
    pub async fn resolve_run(&self, prefix: &str) -> R<RunRow> {
        let p = prefix.to_string();
        self.db(move |s| s.get_run_by_prefix(&p)).await
    }

    // ---------------------------------------------------------------- events

    pub(crate) async fn emit(
        &self,
        session_id: &str,
        run_id: Option<&str>,
        event_type: EventType,
        data: Value,
    ) {
        let ev = Event {
            id: new_id(),
            session_id: session_id.to_string(),
            run_id: run_id.map(String::from),
            ts: Utc::now(),
            event_type,
            data,
        };
        let stored = ev.clone();
        if let Err(e) = self.db(move |s| s.append_event(&stored)).await {
            tracing::warn!(error = %e.message, "could not store event");
            return;
        }
        let mut subs = lock(&self.inner.subs);
        if let Some(v) = subs.get_mut(&ev.session_id) {
            v.retain(|tx| tx.send(ev.clone()).is_ok());
            if v.is_empty() {
                subs.remove(&ev.session_id);
            }
        }
    }

    /// Events of the given sessions, from now on.
    pub fn subscribe(&self, sessions: &[String]) -> mpsc::UnboundedReceiver<Event> {
        let (tx, rx) = mpsc::unbounded_channel();
        let mut subs = lock(&self.inner.subs);
        for s in sessions {
            subs.entry(s.clone()).or_default().push(tx.clone());
        }
        rx
    }

    // -------------------------------------------------------------- sessions

    pub(crate) fn session_connected(&self, id: &str) {
        lock(&self.inner.connected).insert(id.to_string());
    }

    /// Called when a session's client goes away; ends the session when it has no active
    /// runs left (otherwise the last run to finish ends it).
    pub(crate) fn session_disconnected(&self, id: &str) {
        lock(&self.inner.connected).remove(id);
    }

    async fn maybe_end_session(&self, session_id: &str) {
        if lock(&self.inner.connected).contains(session_id) {
            return;
        }
        let sid = session_id.to_string();
        let r = self
            .db(move |s| {
                let busy = s
                    .list_runs_by_session(&sid, 100_000)?
                    .iter()
                    .any(|r| r.state.is_active());
                if !busy {
                    s.end_session(&sid, "ended", Utc::now())?;
                }
                Ok(())
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(%session_id, error = %e.message, "could not end session");
        }
    }

    // ----------------------------------------------------------------- spawn

    async fn load_account(&self, name: &str) -> R<AccountFile> {
        let path = self.inner.paths.account_file(name);
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Err(rpc(NOT_FOUND, "account not found"));
            }
            AccountFile::load(&path).map_err(|e| rpc(UNAVAILABLE, e.to_string()))
        })
        .await
        .map_err(internal)?
        .map_err(|mut e| {
            e.message = format!("{}: {name}", e.message);
            e
        })
    }

    fn launch_for(&self, account: &AccountFile) -> R<so_accounts::Launch> {
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
        let prompt = p.prompt.clone().filter(|s| !s.trim().is_empty());
        let Some(prompt) = prompt else {
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
        if let Some(parent) = &ctx.run_id {
            let parent = parent.clone();
            let active = self
                .db(move |s| {
                    Ok(s.list_child_runs(&parent, 100_000)?
                        .iter()
                        .filter(|r| r.state.is_active())
                        .count())
                })
                .await?;
            let max = self.config().limits.max_children as usize;
            if active >= max {
                return Err(rpc(
                    CAP_EXCEEDED,
                    format!(
                        "this run already has {active} active children (limits.max_children = {max})"
                    ),
                ));
            }
        }
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
        let session = self.db(move |s| s.get_session(&sid)).await?;
        let base = session
            .and_then(|s| s.cwd)
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        let cwd = match &p.cwd {
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
        let mut isolation = p.isolation.unwrap_or(Isolation::Worktree);
        if isolation == Isolation::Worktree {
            let dir = cwd.clone();
            let info = tokio::task::spawn_blocking(move || worktree::repo_info(&dir))
                .await
                .map_err(internal)?;
            match info {
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
            warnings,
        })
    }

    /// Creates a queued run and starts it when a slot is free. Returns the run id and the
    /// warnings for the caller.
    pub async fn spawn(&self, ctx: &ConnCtx, p: SpawnParams) -> R<(String, Vec<String>)> {
        let v = self.validate(ctx, &p).await?;
        let session_id = ctx.session_id.clone().unwrap_or_default();
        let run_id = new_id();
        let warnings = v.warnings.clone();
        let row = RunRow {
            id: run_id.clone(),
            session_id,
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
        };
        let plan = SpawnPlan {
            cwd: v.cwd,
            isolation: v.isolation,
            max_turns: p.max_turns,
        };
        // Detached: a client that disconnects mid-call must not leave a queued run that
        // nothing will ever start.
        let mgr = self.clone();
        tokio::spawn(async move { mgr.enqueue(row, plan).await })
            .await
            .map_err(internal)??;
        Ok((run_id, warnings))
    }

    async fn enqueue(&self, row: RunRow, plan: SpawnPlan) -> R<()> {
        let id = row.id.clone();
        let session = row.session_id.clone();
        let data = json!({
            "harness": row.harness,
            "account": row.account,
            "name": row.name,
            "depth": row.depth,
            "parent_run": row.parent_run_id,
        });
        lock(&self.inner.active).insert(id.clone());
        lock(&self.inner.plans).insert(id.clone(), plan);
        let r = self.db(move |s| s.insert_run(&row)).await;
        if let Err(e) = r {
            lock(&self.inner.active).remove(&id);
            lock(&self.inner.plans).remove(&id);
            return Err(e);
        }
        self.emit(&session, Some(&id), EventType::RunQueued, data)
            .await;
        self.drain().await;
        Ok(())
    }

    // ----------------------------------------------------------- queue / start

    /// Starts queued runs, oldest first, while slots are free. A run blocked by its
    /// account cap does not hold back runs of other accounts.
    pub fn drain(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        // Boxed: draining can fail a run, and finishing a run drains again.
        Box::pin(self.drain_inner())
    }

    async fn drain_inner(&self) {
        let _guard = self.inner.drain.lock().await;
        let queued = match self.db(|s| s.list_queued_runs(100_000)).await {
            Ok(q) => q,
            Err(e) => {
                tracing::warn!(error = %e.message, "could not list queued runs");
                return;
            }
        };
        let global_cap = i64::from(self.config().limits.global_cap);
        for run in queued {
            let Some(name) = run.account.clone() else {
                self.fail_run(&run.id, ErrorKind::Other, "run has no account")
                    .await;
                continue;
            };
            let account = match self.load_account(&name).await {
                Ok(a) if a.enabled => a,
                Ok(_) => {
                    self.fail_run(
                        &run.id,
                        ErrorKind::Other,
                        &format!("account {name} is disabled"),
                    )
                    .await;
                    continue;
                }
                Err(e) => {
                    self.fail_run(&run.id, ErrorKind::Other, &e.message).await;
                    continue;
                }
            };
            let (id, cap) = (run.id.clone(), i64::from(account.cap));
            let r = self
                .db(move |s| {
                    s.try_claim_slot(
                        &id,
                        RunState::Queued,
                        RunState::Starting,
                        &name,
                        cap,
                        global_cap,
                        Utc::now(),
                    )
                })
                .await;
            match r {
                Ok(true) => {
                    self.emit(
                        &run.session_id,
                        Some(&run.id),
                        EventType::StateChanged,
                        json!({"from": RunState::Queued, "to": RunState::Starting}),
                    )
                    .await;
                    let mgr = self.clone();
                    tokio::spawn(async move { mgr.start_run(run).await });
                }
                // Cancelled while we were looking: nothing to start.
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(run_id = %run.id, error = %e.message, "could not claim a slot")
                }
            }
        }
    }

    fn take_plan(&self, row: &RunRow) -> SpawnPlan {
        lock(&self.inner.plans)
            .remove(&row.id)
            .unwrap_or_else(|| SpawnPlan {
                // Rebuilt after a restart: the original options are gone, use the defaults.
                cwd: row
                    .cwd
                    .as_deref()
                    .map_or_else(|| PathBuf::from("/"), PathBuf::from),
                isolation: Isolation::Worktree,
                max_turns: None,
            })
    }

    async fn start_run(&self, row: RunRow) {
        let plan = self.take_plan(&row);
        if let Err(reason) = self.try_start(&row, plan).await {
            self.fail_run(&row.id, ErrorKind::Other, &reason).await;
        }
    }

    async fn try_start(&self, row: &RunRow, plan: SpawnPlan) -> Result<(), String> {
        let account_name = row.account.clone().ok_or("run has no account")?;
        let account = self
            .load_account(&account_name)
            .await
            .map_err(|e| e.message)?;
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg)
            .map_err(|e| e.to_string())?;
        let launch = self.launch_for(&account).map_err(|e| e.message)?;
        let still_starting = self
            .db({
                let id = row.id.clone();
                move |s| s.get_run(&id)
            })
            .await
            .map_err(|e| e.message)?
            .is_some_and(|r| r.state == RunState::Starting);
        if !still_starting {
            return Ok(());
        }

        let mut cwd = plan.cwd.clone();
        if plan.isolation == Isolation::Worktree {
            cwd = self.make_worktree(row, &plan).await?;
        }

        let run_dir = self.inner.paths.run_dir(&row.id);
        create_private_dir(&run_dir)
            .map_err(|e| format!("could not create {}: {e}", run_dir.display()))?;
        let output_schema = row
            .output_schema
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let spec = LaunchSpec {
            run_id: row.id.clone(),
            run_dir,
            cwd,
            account: account.clone(),
            launch,
            model: row.model.clone(),
            effort: row.effort.clone(),
            prompt: row.prompt.clone().unwrap_or_default(),
            system_addendum: child_instructions(
                &row.id,
                u8::try_from(row.depth).unwrap_or(MAX_DEPTH),
                row.parent_run_id.as_deref(),
                output_schema.as_ref(),
            ),
            mcp: self.mcp_spec(&row.id),
            output_schema,
            max_turns: plan.max_turns,
            resume: None,
        };
        let handle = driver
            .launch(spec)
            .await
            .map_err(|e| format!("could not launch {}: {e}", row.harness))?;
        self.go_live(row, &account, handle, false).await;
        Ok(())
    }

    fn mcp_spec(&self, run_id: &str) -> McpServerSpec {
        McpServerSpec {
            command: self.inner.mcp_exe.clone(),
            args: vec!["mcp".to_string()],
            env: vec![
                ("SUPERORCHESTRATOR_RUN_ID".into(), run_id.to_string()),
                (
                    "SUPERORCHESTRATOR_SOCKET".into(),
                    self.inner.socket_path.to_string_lossy().into_owned(),
                ),
                (
                    "SUPERORCHESTRATOR_CONFIG_DIR".into(),
                    self.inner.paths.config_dir.to_string_lossy().into_owned(),
                ),
            ],
        }
    }

    /// Creates the run's worktree and returns the directory the harness starts in.
    async fn make_worktree(&self, row: &RunRow, plan: &SpawnPlan) -> Result<PathBuf, String> {
        let cwd = plan.cwd.clone();
        let template = self.config().worktrees.dir.clone();
        let template = match self.host() {
            Some(h) => h
                .expand(&template)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
            None => template,
        };
        let prefix = self.config().worktrees.branch_prefix.clone();
        let hint = row
            .name
            .clone()
            .or_else(|| row.prompt.clone())
            .unwrap_or_default();
        let id = row.id.clone();
        let created = tokio::task::spawn_blocking(move || {
            let info = worktree::repo_info(&cwd)
                .filter(|i| i.has_head)
                .ok_or_else(|| {
                    format!(
                        "{} is no longer a git repository with commits",
                        cwd.display()
                    )
                })?;
            let rel = cwd
                .strip_prefix(&info.toplevel)
                .unwrap_or(std::path::Path::new(""))
                .to_path_buf();
            worktree::create(&info, &template, &prefix, &hint, &id).map(|c| (c, rel))
        })
        .await
        .map_err(|e| e.to_string())??;
        let (created, rel) = created;
        let (path, branch) = (
            created.path.to_string_lossy().into_owned(),
            created.branch.clone(),
        );
        let patch = RunPatch {
            worktree_path: Some(Some(path.clone())),
            branch: Some(Some(branch.clone())),
            ..RunPatch::default()
        };
        let id = row.id.clone();
        self.db(move |s| s.patch_run(&id, &patch))
            .await
            .map_err(|e| e.message)?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::WorktreeCreated,
            json!({
                "path": path,
                "branch": branch,
                "base": created.base,
                "repo": created.repo,
            }),
        )
        .await;
        Ok(created.path.join(rel))
    }

    /// Records a launched process and starts the event pump. When the run was finished
    /// (cancelled) while it launched, the process is shut down instead.
    pub(crate) async fn go_live(
        &self,
        row: &RunRow,
        account: &AccountFile,
        handle: RunHandle,
        resumed: bool,
    ) {
        let RunHandle {
            events,
            control,
            native_session_id,
            join,
        } = handle;
        let id = row.id.clone();
        let from = if resumed {
            row.state
        } else {
            RunState::Starting
        };
        let moved = if from == RunState::Starting {
            let id = id.clone();
            self.db(move |s| s.update_run_state(&id, RunState::Starting, RunState::Running))
                .await
        } else {
            Ok(())
        };
        if moved.is_err() {
            // Cancelled or failed while launching.
            let _ = control.shutdown(Duration::ZERO).await;
            self.cleanup_worktree(&id).await;
            return;
        }
        if let Some(n) = native_session_id
            .clone()
            .filter(|n| row.native_session_id.as_ref() != Some(n))
        {
            let patch = RunPatch {
                native_session_id: Some(Some(n)),
                ..RunPatch::default()
            };
            let id = id.clone();
            let _ = self.db(move |s| s.patch_run(&id, &patch)).await;
        }
        let pool = account.pools.first().cloned();
        let attempt = {
            let (id, name, pool) = (id.clone(), account.name.clone(), pool.clone());
            self.db(move |s| s.start_attempt(&id, &name, pool.as_deref(), Utc::now()))
                .await
        };
        if let Err(e) = attempt {
            tracing::warn!(run_id = %id, error = %e.message, "could not record attempt");
        }
        let sid = &row.session_id;
        self.emit(
            sid,
            Some(&id),
            EventType::AttemptStarted,
            json!({"account": account.name, "harness": account.harness, "resumed": resumed}),
        )
        .await;
        if from == RunState::Starting {
            self.emit(
                sid,
                Some(&id),
                EventType::StateChanged,
                json!({"from": RunState::Starting, "to": RunState::Running}),
            )
            .await;
        }
        self.emit(
            sid,
            Some(&id),
            EventType::RunStarted,
            json!({
                "native_session_id": native_session_id,
                "model": row.model,
                "effort": row.effort,
                "resumed": resumed,
            }),
        )
        .await;

        let schema = row
            .output_schema
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .and_then(|s| jsonschema::validator_for(&s).ok());
        let pc = PumpCtx {
            run_id: id.clone(),
            session_id: row.session_id.clone(),
            harness: row.harness,
            account: account.clone(),
            pool: pool.unwrap_or_else(|| "default".into()),
            validator: schema,
        };
        let mut live = lock(&self.inner.live);
        let task = tokio::spawn(self.clone().pump(pc, events, control.clone()));
        live.insert(
            id,
            LiveRun {
                control,
                join,
                task,
            },
        );
    }

    // ---------------------------------------------------------------- finish

    /// Marks a run failed, flagging the account first for quota, rate limit and auth errors.
    pub(crate) async fn fail_run(&self, run_id: &str, kind: ErrorKind, reason: &str) {
        // TODO(failover): a quota or rate-limit failure should hand the run to the router
        // for another account instead of ending it.
        if matches!(
            kind,
            ErrorKind::Quota | ErrorKind::RateLimit | ErrorKind::Auth
        ) {
            self.flag_account(run_id, reason).await;
        }
        self.finish(
            run_id,
            RunState::Failed,
            Some(kind),
            None,
            Some(reason.to_string()),
        )
        .await;
    }

    async fn flag_account(&self, run_id: &str, reason: &str) {
        let Ok(Some(row)) = self
            .db({
                let id = run_id.to_string();
                move |s| s.get_run(&id)
            })
            .await
        else {
            return;
        };
        let Some(name) = row.account else { return };
        let Ok(account) = self.load_account(&name).await else {
            return;
        };
        let pool = account
            .pools
            .first()
            .cloned()
            .unwrap_or_else(|| "default".into());
        let cooldown = self.config().usage.default_cooldown.as_duration();
        let reason = format!("run {run_id}: {}", excerpt(reason, 200));
        let r = self
            .db(move |s| {
                so_usage::mark_exhausted(s, &name, &pool, None, cooldown, &reason, Utc::now())
                    .map(|_| ())
                    .map_err(|e| StoreError::Constraint(e.to_string()))
            })
            .await;
        if let Err(e) = r {
            tracing::warn!(%run_id, error = %e.message, "could not flag the account");
        }
    }

    /// Ends a run. Returns `true` when this call made the transition; a run that is
    /// already terminal is left alone.
    pub(crate) async fn finish(
        &self,
        run_id: &str,
        to: RunState,
        error_kind: Option<ErrorKind>,
        result_json: Option<String>,
        reason: Option<String>,
    ) -> bool {
        *lock(&self.inner.finalizing)
            .entry(run_id.to_string())
            .or_default() += 1;
        let won = self
            .finish_inner(run_id, to, error_kind, result_json, reason)
            .await;
        {
            let mut f = lock(&self.inner.finalizing);
            if let Some(n) = f.get_mut(run_id) {
                *n -= 1;
                if *n == 0 {
                    f.remove(run_id);
                }
            }
        }
        if won {
            lock(&self.inner.active).remove(run_id);
        }
        self.inner.finished.send_modify(|n| *n += 1);
        if won {
            self.drain().await;
        }
        won
    }

    async fn finish_inner(
        &self,
        run_id: &str,
        to: RunState,
        error_kind: Option<ErrorKind>,
        result_json: Option<String>,
        reason: Option<String>,
    ) -> bool {
        let mut transition = None;
        for _ in 0..8 {
            let id = run_id.to_string();
            let Ok(Some(row)) = self.db(move |s| s.get_run(&id)).await else {
                return false;
            };
            if row.state.is_terminal() {
                return false;
            }
            let patch = FinishPatch {
                result_json: result_json.clone(),
                error_kind,
                finished_at: Some(Utc::now()),
            };
            let (id, from) = (run_id.to_string(), row.state);
            // `succeeded` is reachable only from `running` and `pending_review`.
            let r = self
                .db_raw(move |s| {
                    if to == RunState::Succeeded
                        && matches!(
                            from,
                            RunState::Waiting | RunState::Stalled | RunState::Escalated
                        )
                    {
                        s.update_run_state(&id, from, RunState::Running)?;
                        return s.finish_run(&id, RunState::Running, to, &patch);
                    }
                    s.finish_run(&id, from, to, &patch)
                })
                .await;
            match r {
                Ok(()) => {
                    transition = Some(row);
                    break;
                }
                // Someone else moved the run: look again.
                Err(StoreError::StateConflict { .. }) => {}
                Err(e) => {
                    tracing::warn!(%run_id, error = %e, "could not finish run");
                    return false;
                }
            }
        }
        let Some(row) = transition else { return false };
        let sid = row.session_id.clone();
        self.emit(
            &sid,
            Some(run_id),
            EventType::StateChanged,
            json!({"from": row.state, "to": to, "reason": reason}),
        )
        .await;
        self.emit(
            &sid,
            Some(run_id),
            EventType::RunFinished,
            json!({
                "status": to,
                "error_kind": error_kind,
                "reason": reason,
                "result_ref": result_json.as_ref().map(|_| run_id),
            }),
        )
        .await;

        let id = run_id.to_string();
        let outcome = to.as_str();
        let _ = self
            .db(move |s| {
                let open = s
                    .list_attempts(&id, 1000)?
                    .into_iter()
                    .rev()
                    .find(|a| a.ended_at.is_none());
                match open {
                    Some(a) => s.finish_attempt(&a.id, outcome, Utc::now()),
                    None => Ok(()),
                }
            })
            .await;
        lock(&self.inner.live).remove(run_id);
        lock(&self.inner.plans).remove(run_id);
        lock(&self.inner.cancelling).remove(run_id);
        self.cleanup_worktree(run_id).await;
        self.maybe_end_session(&sid).await;
        true
    }

    /// Removes the run's worktree and branch when the run made no commits and left the
    /// tree clean; keeps it otherwise.
    pub(crate) async fn cleanup_worktree(&self, run_id: &str) {
        let id = run_id.to_string();
        let found = self
            .db(move |s| {
                let Some(row) = s.get_run(&id)? else {
                    return Ok(None);
                };
                if row.worktree_path.is_none() {
                    return Ok(None);
                }
                let created = s
                    .list_events_by_run(&id, ALL_EVENTS)?
                    .into_iter()
                    .find(|e| e.event_type == EventType::WorktreeCreated);
                Ok(created.map(|e| (row.session_id, e.data)))
            })
            .await;
        let Ok(Some((session_id, data))) = found else {
            return;
        };
        let get = |k: &str| data.get(k).and_then(Value::as_str).map(String::from);
        let (Some(path), Some(branch), Some(base), Some(repo)) =
            (get("path"), get("branch"), get("base"), get("repo"))
        else {
            return;
        };
        let created = worktree::Created {
            path: PathBuf::from(&path),
            branch: branch.clone(),
            base,
            repo: PathBuf::from(repo),
        };
        let r = tokio::task::spawn_blocking(move || worktree::remove_if_unchanged(&created)).await;
        match r {
            Ok(Ok(())) => {
                self.emit(
                    &session_id,
                    Some(run_id),
                    EventType::WorktreeRemoved,
                    json!({"path": path, "branch": branch}),
                )
                .await;
            }
            Ok(Err(kept)) => tracing::debug!(%run_id, ?kept, "worktree kept"),
            Err(e) => tracing::warn!(%run_id, error = %e, "worktree cleanup failed"),
        }
    }

    // ---------------------------------------------------------------- cancel

    pub(crate) fn is_cancelling(&self, run_id: &str) -> bool {
        lock(&self.inner.cancelling).contains(run_id)
    }

    /// Cancels a run and all of its descendants, deepest first. Returns the ids that were
    /// cancelled by this call.
    pub async fn cancel(&self, run_id: &str) -> R<Vec<String>> {
        let id = run_id.to_string();
        let order = self
            .db(move |s| {
                let mut order = Vec::new();
                collect_subtree(s, &id, &mut order, 0)?;
                Ok(order)
            })
            .await?;
        let mgr = self.clone();
        // Detached so that a disconnecting caller cannot stop half way.
        tokio::spawn(async move {
            let mut done = Vec::new();
            for id in order {
                if mgr.cancel_one(&id).await {
                    done.push(id);
                }
            }
            done
        })
        .await
        .map_err(internal)
    }

    async fn cancel_one(&self, run_id: &str) -> bool {
        let id = run_id.to_string();
        match self.db(move |s| s.get_run(&id)).await {
            Ok(Some(r)) if r.state.is_active() => {}
            _ => return false,
        }
        lock(&self.inner.cancelling).insert(run_id.to_string());
        let control = lock(&self.inner.live)
            .get(run_id)
            .map(|l| l.control.clone());
        if let Some(control) = control {
            if let Err(e) = control.interrupt().await {
                tracing::debug!(%run_id, error = %e, "interrupt failed");
            }
            if let Err(e) = control.shutdown(SHUTDOWN_DEADLINE).await {
                tracing::warn!(%run_id, error = %e, "shutdown failed");
            }
        }
        self.finish(
            run_id,
            RunState::Cancelled,
            None,
            None,
            Some("cancelled".to_string()),
        )
        .await
    }

    // ------------------------------------------------------------------ wait

    async fn terminal_among(&self, ids: &[String]) -> R<Vec<String>> {
        let list = ids.to_vec();
        let rows = self
            .db(move |s| {
                let mut out = Vec::new();
                for id in &list {
                    if let Some(r) = s.get_run(id)? {
                        out.push((r.id, r.state));
                    }
                }
                Ok(out)
            })
            .await?;
        let fin = lock(&self.inner.finalizing);
        Ok(rows
            .into_iter()
            .filter(|(id, st)| st.is_terminal() && !fin.contains_key(id))
            .map(|(id, _)| id)
            .collect())
    }

    /// Waits until any of `ids` is finished, `timeout` passes, or the client goes away.
    /// Events of those runs are pushed to the client as `event` notifications meanwhile.
    pub async fn wait_runs(
        &self,
        ctx: &ConnCtx,
        ids: &[String],
        rx: &mut mpsc::UnboundedReceiver<Event>,
        timeout: Option<Duration>,
    ) -> R<WaitOutcome> {
        let deadline = timeout.map(|t| tokio::time::Instant::now() + t);
        let mut finished_rx = self.inner.finished.subscribe();
        let mut out = WaitOutcome::default();
        let mut open = true;
        loop {
            let done = self.terminal_among(ids).await?;
            if !done.is_empty() {
                // Everything the run emitted is already queued: deliver it before returning.
                while let Ok(ev) = rx.try_recv() {
                    self.push_event(ctx, ids, ev, &mut out).await;
                }
                out.finished = done;
                return Ok(out);
            }
            tokio::select! {
                ev = rx.recv(), if open => match ev {
                    Some(ev) => self.push_event(ctx, ids, ev, &mut out).await,
                    None => open = false,
                },
                _ = finished_rx.changed() => {}
                () = async {
                    match deadline {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending().await,
                    }
                } => {
                    out.timed_out = true;
                    return Ok(out);
                }
                () = ctx.closed() => return Err(rpc(UNAVAILABLE, "client disconnected")),
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }

    async fn push_event(&self, ctx: &ConnCtx, ids: &[String], ev: Event, out: &mut WaitOutcome) {
        if !ev.run_id.as_ref().is_some_and(|r| ids.contains(r)) {
            return;
        }
        ctx.notifier.notify("event", &ev).await;
        if matches!(
            ev.event_type,
            EventType::Progress | EventType::QuestionAsked
        ) {
            out.updates.push(ev);
        }
    }

    // ----------------------------------------------------------------- views

    fn join_for(&self, row: &RunRow) -> Option<JoinCommand> {
        let info = match lock(&self.inner.live).get(&row.id) {
            Some(l) => l.join.clone(),
            None => JoinInfo {
                account_name: row.account.clone()?,
                native_session_id: row.native_session_id.clone()?,
                endpoint: None,
                cwd: row
                    .worktree_path
                    .as_ref()
                    .or(row.cwd.as_ref())
                    .map(PathBuf::from)?,
            },
        };
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg).ok()?;
        Some(driver.join_command(&info))
    }

    pub async fn run_view(&self, row: RunRow) -> R<RunView> {
        let id = row.id.clone();
        let events = self
            .db(move |s| s.list_events_by_run(&id, ALL_EVENTS))
            .await?;
        let mut usage = UsageTotals::default();
        for e in events.iter().filter(|e| e.event_type == EventType::Usage) {
            usage.input_tokens += e.data["input_tokens"].as_u64().unwrap_or(0);
            usage.output_tokens += e.data["output_tokens"].as_u64().unwrap_or(0);
            usage.cost += e.data["cost"].as_f64().unwrap_or(0.0);
        }
        let skip = events.len().saturating_sub(RECENT_EVENTS);
        let recent_events = events.into_iter().skip(skip).collect();
        let join = self.join_for(&row);
        Ok(RunView {
            run_id: row.id,
            state: row.state,
            harness: Some(row.harness),
            account: row.account,
            model: row.model,
            recent_events,
            result: row
                .result_json
                .as_deref()
                .and_then(|r| serde_json::from_str(r).ok()),
            usage,
            worktree: row.worktree_path,
            branch: row.branch,
            join_command: join.as_ref().map(|j| display_argv(&j.argv)),
            join_cwd: join.as_ref().map(|j| j.cwd.to_string_lossy().into_owned()),
            join_env: join
                .as_ref()
                .map(|j| j.env.iter().map(|(k, _)| k.clone()).collect())
                .unwrap_or_default(),
            name: row.name,
            depth: u32::try_from(row.depth).unwrap_or(0),
            parent_run: row.parent_run_id,
            error_kind: row.error_kind,
            cwd: row.cwd,
            created_at: Some(row.created_at),
            finished_at: row.finished_at,
        })
    }

    /// The command to join a run, with its environment values. Records
    /// `join_command_issued`.
    pub async fn join_run(&self, row: RunRow) -> R<JoinResult> {
        let join = self
            .join_for(&row)
            .ok_or_else(|| rpc(UNAVAILABLE, "this run has no native session to join yet"))?;
        self.emit(
            &row.session_id,
            Some(&row.id),
            EventType::JoinCommandIssued,
            json!({"argv": join.argv, "env": join.env.iter().map(|(k, _)| k).collect::<Vec<_>>()}),
        )
        .await;
        Ok(JoinResult {
            argv: join.argv,
            env: join.env,
            cwd: join.cwd.to_string_lossy().into_owned(),
        })
    }

    /// Final result of a finished run.
    pub fn result_of(row: &RunRow) -> Option<Value> {
        row.result_json
            .as_deref()
            .and_then(|r| serde_json::from_str(r).ok())
    }

    // ------------------------------------------------------------- reconcile

    pub(crate) fn busy(&self) -> bool {
        !lock(&self.inner.active).is_empty()
    }

    /// Takes over a run left active by a previous supervisor. Queued runs go back into
    /// the queue; started runs are resumed once from their native session.
    pub(crate) fn adopt(&self, row: &RunRow) -> bool {
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return false;
        };
        if row.state == RunState::Queued {
            lock(&self.inner.active).insert(row.id.clone());
            let mgr = self.clone();
            rt.spawn(async move { mgr.drain().await });
            return true;
        }
        let resumable = row.harness == Harness::Claude
            && row.native_session_id.is_some()
            && row.account.is_some()
            && self
                .inner
                .store
                .list_attempts(&row.id, 10)
                .is_ok_and(|a| a.len() < 2);
        if !resumable {
            return false;
        }
        lock(&self.inner.active).insert(row.id.clone());
        let (mgr, row) = (self.clone(), row.clone());
        rt.spawn(async move {
            if let Err(reason) = mgr.resume_run(&row).await {
                mgr.fail_run(&row.id, ErrorKind::Other, &reason).await;
            }
        });
        true
    }

    async fn resume_run(&self, row: &RunRow) -> Result<(), String> {
        let native = row.native_session_id.clone().ok_or("no native session")?;
        let name = row.account.clone().ok_or("run has no account")?;
        let account = self.load_account(&name).await.map_err(|e| e.message)?;
        let driver = AnyDriver::for_harness(row.harness, self.inner.driver_cfg)
            .map_err(|e| e.to_string())?;
        let launch = self.launch_for(&account).map_err(|e| e.message)?;
        let cwd = row
            .worktree_path
            .as_ref()
            .or(row.cwd.as_ref())
            .map(PathBuf::from)
            .ok_or("run has no working directory")?;
        if !cwd.is_dir() {
            return Err(format!("working directory {} is gone", cwd.display()));
        }
        let run_dir = self.inner.paths.run_dir(&row.id);
        create_private_dir(&run_dir).map_err(|e| e.to_string())?;
        let output_schema = row
            .output_schema
            .as_deref()
            .and_then(|s| serde_json::from_str::<Value>(s).ok());
        let spec = LaunchSpec {
            run_id: row.id.clone(),
            run_dir,
            cwd,
            account: account.clone(),
            launch,
            model: row.model.clone(),
            effort: row.effort.clone(),
            prompt: "Continue.".to_string(),
            system_addendum: child_instructions(
                &row.id,
                u8::try_from(row.depth).unwrap_or(MAX_DEPTH),
                row.parent_run_id.as_deref(),
                output_schema.as_ref(),
            ),
            mcp: self.mcp_spec(&row.id),
            output_schema,
            max_turns: None,
            resume: Some(native),
        };
        let handle = driver
            .launch(spec)
            .await
            .map_err(|e| format!("could not resume {}: {e}", row.harness))?;
        self.go_live(row, &account, handle, true).await;
        Ok(())
    }
}

impl crate::server::ActivityProbe for RunManager {
    fn busy(&self) -> bool {
        RunManager::busy(self)
    }
}

impl crate::reconcile::ReconcileHook for RunManager {
    fn reconcile(&self, _store: &Store, run: &RunRow) -> bool {
        self.adopt(run)
    }
}

/// Post-order walk: children before their parent.
fn collect_subtree(s: &Store, id: &str, out: &mut Vec<String>, depth: u8) -> so_store::Result<()> {
    if depth <= MAX_DEPTH + 1 {
        for child in s.list_child_runs(id, 100_000)? {
            collect_subtree(s, &child.id, out, depth + 1)?;
        }
    }
    out.push(id.to_string());
    Ok(())
}

fn create_private_dir(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

/// At most `max` characters, ending in an ellipsis when cut.
pub(crate) fn excerpt(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut s: String = text.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// argv for display: arguments with spaces or shell characters are single-quoted.
pub fn display_argv(argv: &[String]) -> String {
    argv.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c));
            if plain {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpt_is_bounded() {
        assert_eq!(excerpt("short", 280), "short");
        let long = "é".repeat(500);
        let e = excerpt(&long, 280);
        assert_eq!(e.chars().count(), 280);
        assert!(e.ends_with('…'));
    }

    #[test]
    fn argv_display_quotes_only_when_needed() {
        let a = |v: &[&str]| display_argv(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            a(&["claude-acc1", "--resume", "abc-123"]),
            "claude-acc1 --resume abc-123"
        );
        assert_eq!(a(&["x", "a b", "it's"]), "x 'a b' 'it'\\''s'");
    }
}
