//! Runs: the run manager and the tools built on it (SPEC 7, 13).
//!
//! `RunManager` is split by concern: `spawn` (validate and enqueue), `queue` (claim slots),
//! `lifecycle` (start, finish, cancel, worktree cleanup), `view` (read models and waiting),
//! `reconcile` (take over runs after a restart) and `events` (store and fan out events).

mod agent;
mod comm;
mod events;
mod failover;
mod fork;
mod handlers;
mod instructions;
mod lifecycle;
mod pump;
mod queue;
mod reconcile;
mod resume;
mod review;
mod routing;
mod spawn;
mod view;
pub mod worktree;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use so_accounts::HostEnv;
use so_core::proto::{INTERNAL_ERROR, INVALID_PARAMS, NOT_FOUND, RpcError};
use so_core::{Config, Paths};
use so_harness::{DriverConfig, JoinInfo, RunControl};
use so_store::{RunRow, Store, StoreError};
use tokio::sync::{Notify, watch};

pub use events::{Delivered, Subscription};
pub use view::{WaitMode, WaitOutcome, display_argv, mask_argv};

use crate::dispatch::internal;
use crate::notify::{Notifications, Sink, sinks_from_config};
use crate::server::SupervisorState;

pub(crate) type R<T> = Result<T, RpcError>;

/// How long a harness may take to start before the run fails.
pub(crate) const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
/// Grace for a harness to exit after its input is closed.
pub(crate) const SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn rpc(code: i64, msg: impl Into<String>) -> RpcError {
    RpcError::new(code, msg)
}

fn store_rpc(e: StoreError) -> RpcError {
    match e {
        StoreError::NotFound { .. } => rpc(NOT_FOUND, e.to_string()),
        StoreError::AmbiguousPrefix { .. } => rpc(INVALID_PARAMS, e.to_string()),
        other => rpc(INTERNAL_ERROR, other.to_string()),
    }
}

/// Weak references to the live per-run locks.
type RunLocks = HashMap<(LockKind, String), std::sync::Weak<tokio::sync::Mutex<()>>>;

/// What a per-run lock protects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LockKind {
    /// A run entering the review gate, or being resumed: the state change, its events and
    /// the claim are one step, so a reject sees the gate announced before it acts.
    Gate,
    /// Putting a run's worktree back, and undoing it.
    Worktree,
}

/// A run with a harness process.
pub(crate) struct LiveRun {
    pub control: RunControl,
    pub join: JoinInfo,
}

/// Everything the supervisor knows about one unfinished run besides its row. An entry
/// exists from enqueue until `finish` has completed, so a run that is terminal in the store
/// but still has an entry is being wrapped up and waiters must not report it done yet.
#[derive(Default)]
pub(crate) struct RunRt {
    pub live: Option<LiveRun>,
    /// A cancel was requested: a launch still in flight must not go live.
    pub cancel_requested: bool,
    /// Blocking calls in progress (`ask_parent`, `wait_agent` on children). While there is
    /// one the run is `waiting` and is not stall-checked.
    pub waits: u32,
    /// How many of those are `ask_parent` calls.
    pub asks: u32,
}

struct Inner {
    store: Store,
    config: Config,
    paths: Paths,
    host: Option<HostEnv>,
    socket_path: PathBuf,
    mcp_exe: PathBuf,
    driver_cfg: DriverConfig,
    /// For the route decider. Never follows redirects, so the bearer key cannot be forwarded.
    http: reqwest::Client,
    /// The single place run state lives. Held only for short, non-awaiting sections.
    runs: Mutex<HashMap<String, RunRt>>,
    /// Orchestrator sessions with a connected client.
    connected: Mutex<HashSet<String>>,
    bus: Arc<events::Bus>,
    drain: tokio::sync::Mutex<()>,
    drain_wake: Arc<Notify>,
    finished: watch::Sender<u64>,
    /// Bumped when a message is stored, so blocked `ask_parent` calls look again.
    msg_tick: watch::Sender<u64>,
    /// Per-run locks (see [`LockKind`]).
    locks: Mutex<RunLocks>,
    /// Test hook: how long `enter_review` holds the gate between the state change and
    /// its announcement.
    gate_delay_ms: AtomicU64,
    /// Test hook: make storing a gated run's result fail.
    fault_gate_result: std::sync::atomic::AtomicBool,
    /// Runs whose process died, to be resumed by the resumer task.
    resume_tx: tokio::sync::mpsc::UnboundedSender<reconcile::ResumeRequest>,
    /// Runs to move to another account after a quota or auth error.
    failover_tx: tokio::sync::mpsc::UnboundedSender<failover::FailoverRequest>,
    notifications: Notifications,
    /// Test hooks: hold `go_live`, shorten the launch limit, slow the launch down.
    go_live_open: watch::Sender<bool>,
    launch_timeout_ms: AtomicU64,
    launch_delay_ms: AtomicU64,
    /// How long a question may wait before `question_waiting_10m` fires.
    question_notify_ms: AtomicU64,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Let the drainer notice the manager is gone and end.
        self.drain_wake.notify_one();
    }
}

/// Cheap to clone; all clones share one state.
#[derive(Clone)]
pub struct RunManager {
    inner: Arc<Inner>,
}

impl RunManager {
    /// Must be called inside a tokio runtime: it starts the queue drainer.
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
        let drain_wake = Arc::new(Notify::new());
        let (resume_tx, resume_rx) = tokio::sync::mpsc::unbounded_channel();
        let (failover_tx, failover_rx) = tokio::sync::mpsc::unbounded_channel();
        let notifications = Notifications::new(&config.notifications.on);
        let inner = Arc::new(Inner {
            store,
            config,
            paths,
            host,
            socket_path,
            mcp_exe,
            driver_cfg: DriverConfig::default(),
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            runs: Mutex::default(),
            connected: Mutex::default(),
            bus: Arc::default(),
            drain: tokio::sync::Mutex::new(()),
            drain_wake: drain_wake.clone(),
            finished: watch::channel(0).0,
            msg_tick: watch::channel(0).0,
            resume_tx,
            failover_tx,
            locks: Mutex::default(),
            gate_delay_ms: AtomicU64::new(0),
            fault_gate_result: std::sync::atomic::AtomicBool::new(false),
            notifications,
            go_live_open: watch::channel(true).0,
            launch_timeout_ms: AtomicU64::new(
                u64::try_from(LAUNCH_TIMEOUT.as_millis()).unwrap_or(u64::MAX),
            ),
            launch_delay_ms: AtomicU64::new(0),
            question_notify_ms: AtomicU64::new(600_000),
        });
        queue::spawn_drainer(&inner, drain_wake);
        reconcile::spawn_resumer(&inner, resume_rx);
        failover::spawn_failover(&inner, failover_rx);
        Self { inner }
    }

    /// Starts sending notifications to the sinks the config asks for (Discord, desktop).
    /// Off by default so tests and embedded use never notify.
    pub fn enable_notifications(&self) {
        let client = reqwest::Client::new();
        let sinks = sinks_from_config(&self.inner.config.notifications, &client);
        self.inner.notifications.set_sinks(sinks);
    }

    /// Test hook: replace the notification sinks.
    #[doc(hidden)]
    pub fn set_notify_sinks(&self, sinks: Vec<Arc<dyn Sink>>) {
        self.inner.notifications.set_sinks(sinks);
    }

    /// Sends a notification about `key` (rate limited, filtered by `notifications.on`).
    /// Delivery runs on its own task: a slow webhook never holds up a run.
    pub(crate) fn notify(&self, event: &str, key: &str, name: &str, reason: &str) {
        let inner = self.inner.clone();
        let (event, key, name, reason) = (
            event.to_string(),
            key.to_string(),
            name.to_string(),
            reason.to_string(),
        );
        tokio::spawn(async move {
            inner
                .notifications
                .notify(&event, &key, &name, &reason)
                .await;
        });
    }

    /// Takes the per-run lock of `kind`. The map keeps only weak references and drops dead
    /// ones as it grows, so finished runs leave nothing behind.
    pub(crate) async fn lock_run(
        &self,
        kind: LockKind,
        run_id: &str,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        let m = {
            let mut map = lock(&self.inner.locks);
            let key = (kind, run_id.to_string());
            match map.get(&key).and_then(std::sync::Weak::upgrade) {
                Some(m) => m,
                None => {
                    map.retain(|_, w| w.strong_count() > 0);
                    let m = Arc::new(tokio::sync::Mutex::new(()));
                    map.insert(key, Arc::downgrade(&m));
                    m
                }
            }
        };
        m.lock_owned().await
    }

    pub(crate) fn gate_delay(&self) -> Duration {
        Duration::from_millis(self.inner.gate_delay_ms.load(Ordering::SeqCst))
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

    /// Number of runs with a harness process (for tests and status).
    pub fn live_count(&self) -> usize {
        lock(&self.inner.runs)
            .values()
            .filter(|r| r.live.is_some())
            .count()
    }

    /// Test hook: while closed, a launched harness waits before it is registered.
    #[doc(hidden)]
    pub fn set_go_live_open(&self, open: bool) {
        self.inner.go_live_open.send_replace(open);
    }

    /// Test hook: hold `enter_review` this long between the state change and its
    /// announcement, to show that a resume of the run waits.
    #[doc(hidden)]
    pub fn set_gate_delay(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.inner.gate_delay_ms.store(ms, Ordering::SeqCst);
    }

    /// Test hook: make storing the result of a gated run fail.
    #[doc(hidden)]
    pub fn set_fault_gate_result(&self, on: bool) {
        self.inner.fault_gate_result.store(on, Ordering::SeqCst);
    }

    /// Test hook: how long a harness may take to start.
    #[doc(hidden)]
    pub fn set_launch_timeout(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.inner.launch_timeout_ms.store(ms, Ordering::SeqCst);
    }

    /// Test hook: extra time every launch takes (counts against the launch limit).
    #[doc(hidden)]
    pub fn set_launch_delay(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.inner.launch_delay_ms.store(ms, Ordering::SeqCst);
    }

    /// Test hook: how long a question may wait before it raises `question_waiting_10m`.
    #[doc(hidden)]
    pub fn set_question_notify_after(&self, d: Duration) {
        let ms = u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        self.inner.question_notify_ms.store(ms, Ordering::SeqCst);
    }

    pub(crate) fn question_notify_after(&self) -> Duration {
        Duration::from_millis(self.inner.question_notify_ms.load(Ordering::SeqCst))
    }

    pub(crate) fn launch_timeout(&self) -> Duration {
        Duration::from_millis(self.inner.launch_timeout_ms.load(Ordering::SeqCst))
    }

    pub(crate) fn launch_delay(&self) -> Duration {
        Duration::from_millis(self.inner.launch_delay_ms.load(Ordering::SeqCst))
    }

    pub(crate) async fn wait_go_live_gate(&self) {
        let mut rx = self.inner.go_live_open.subscribe();
        let _ = rx.wait_for(|open| *open).await;
    }

    /// True while any run is queued, starting or live.
    pub(crate) fn busy(&self) -> bool {
        !lock(&self.inner.runs).is_empty()
    }

    // ------------------------------------------------------------ store helpers

    pub(crate) async fn db_raw<T, F>(&self, f: F) -> so_store::Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
    {
        let store = self.inner.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(|e| StoreError::Io(std::io::Error::other(e.to_string())))?
    }

    pub(crate) async fn db<T, F>(&self, f: F) -> R<T>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
    {
        self.db_raw(f).await.map_err(store_rpc)
    }

    /// Runs blocking non-store work (filesystem, git) off the async runtime.
    pub(crate) async fn blocking<T, F>(&self, f: F) -> R<T>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        tokio::task::spawn_blocking(f).await.map_err(internal)
    }

    /// Looks a run up by id or unique id prefix.
    pub async fn resolve_run(&self, prefix: &str) -> R<RunRow> {
        let p = prefix.to_string();
        self.db(move |s| s.get_run_by_prefix(&p)).await
    }

    // -------------------------------------------------------------- sessions

    pub(crate) fn session_connected(&self, id: &str) {
        lock(&self.inner.connected).insert(id.to_string());
    }

    /// Called when a session's client goes away. The session itself is ended by the
    /// connection when it has no active runs, otherwise by the last run to finish.
    pub(crate) fn session_disconnected(&self, id: &str) {
        lock(&self.inner.connected).remove(id);
    }
}

/// Registers the run tools and connects the manager to the supervisor's activity probe
/// and reconcile hook.
pub(crate) fn register(state: &Arc<SupervisorState>) {
    handlers::register(state);
    state.set_probe(Arc::new(state.runs.clone()));
    state.set_reconcile_hook(Arc::new(state.runs.clone()));
}

#[cfg(test)]
pub(crate) mod testutil {
    use chrono::Utc;
    use so_core::{AccountFile, Harness, Isolation, RunState, SessionKind, new_id};
    use so_store::{RunRow, SessionRow};

    use super::*;

    /// A manager over an in-memory store with one claude account `claude-t`.
    pub fn manager() -> (RunManager, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_dir(dir.path().to_path_buf());
        AccountFile::new("claude-t", Harness::Claude, "~/.claude-t", "claude")
            .unwrap()
            .save(&paths)
            .unwrap();
        let m = RunManager::new(
            Store::open_in_memory().unwrap(),
            Config::default(),
            paths,
            None,
            dir.path().join("s.sock"),
            None,
        );
        (m, dir)
    }

    pub fn insert_run(m: &RunManager, state: RunState) -> RunRow {
        let session = SessionRow {
            id: new_id(),
            kind: SessionKind::Orchestrator,
            client: "t".into(),
            cwd: None,
            account: None,
            pid: None,
            started_at: Utc::now(),
            ended_at: None,
            status: "active".into(),
        };
        m.store().insert_session(&session).unwrap();
        let row = RunRow {
            id: new_id(),
            session_id: session.id,
            parent_run_id: None,
            depth: 1,
            name: None,
            agent: None,
            choice_id: None,
            harness: Harness::Claude,
            account: Some("claude-t".into()),
            model: None,
            effort: None,
            cwd: Some("/tmp".into()),
            worktree_path: None,
            branch: None,
            native_session_id: None,
            state,
            review: false,
            output_schema: None,
            result_json: None,
            error_kind: None,
            prompt: None,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
            isolation: Isolation::None,
            max_turns: None,
            worktree_base: None,
            worktree_repo: None,
            timeout_ms: None,
        };
        m.store().insert_run(&row).unwrap();
        lock(&m.inner.runs).insert(row.id.clone(), RunRt::default());
        row
    }
}
