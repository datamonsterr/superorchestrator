//! Runs: the run manager and the tools built on it (SPEC 7, 13).
//!
//! `RunManager` is split by concern: `spawn` (validate and enqueue), `queue` (claim slots),
//! `lifecycle` (start, finish, cancel, worktree cleanup), `view` (read models and waiting),
//! `reconcile` (take over runs after a restart) and `events` (store and fan out events).

mod events;
mod handlers;
mod instructions;
mod lifecycle;
mod pump;
mod queue;
mod reconcile;
mod spawn;
mod view;
pub mod worktree;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use so_accounts::HostEnv;
use so_core::proto::{INTERNAL_ERROR, INVALID_PARAMS, NOT_FOUND, RpcError};
use so_core::{Config, Paths};
use so_harness::{DriverConfig, JoinInfo, RunControl};
use so_store::{RunRow, Store, StoreError};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;

pub use events::{Delivered, Subscription};
pub use view::{WaitOutcome, display_argv};

use crate::dispatch::internal;
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

/// A run with a harness process.
pub(crate) struct LiveRun {
    pub control: RunControl,
    pub join: JoinInfo,
    #[allow(dead_code)] // owned so the pump is tied to the run; finishing detaches it
    pub task: Option<JoinHandle<()>>,
}

/// Everything the supervisor knows about one unfinished run besides its row. An entry
/// exists from enqueue until `finish` has completed, so a run that is terminal in the store
/// but still has an entry is being wrapped up and waiters must not report it done yet.
#[derive(Default)]
pub(crate) struct RunRt {
    pub live: Option<LiveRun>,
    /// A cancel was requested: a launch still in flight must not go live.
    pub cancel_requested: bool,
}

struct Inner {
    store: Store,
    config: Config,
    paths: Paths,
    host: Option<HostEnv>,
    socket_path: PathBuf,
    mcp_exe: PathBuf,
    driver_cfg: DriverConfig,
    /// The single place run state lives. Held only for short, non-awaiting sections.
    runs: Mutex<HashMap<String, RunRt>>,
    /// Orchestrator sessions with a connected client.
    connected: Mutex<HashSet<String>>,
    bus: Arc<events::Bus>,
    drain: tokio::sync::Mutex<()>,
    drain_wake: Arc<Notify>,
    finished: watch::Sender<u64>,
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
        let inner = Arc::new(Inner {
            store,
            config,
            paths,
            host,
            socket_path,
            mcp_exe,
            driver_cfg: DriverConfig::default(),
            runs: Mutex::default(),
            connected: Mutex::default(),
            bus: Arc::default(),
            drain: tokio::sync::Mutex::new(()),
            drain_wake: drain_wake.clone(),
            finished: watch::channel(0).0,
        });
        queue::spawn_drainer(&inner, drain_wake);
        Self { inner }
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
