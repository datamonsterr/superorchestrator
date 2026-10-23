//! Unix-socket server: listener, connections, session lifecycle, idle exit, shutdown.

use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use so_accounts::HostEnv;
use so_core::proto::{
    INTERNAL_ERROR, INVALID_REQUEST, METHOD_HELLO, PARSE_ERROR, Request, Response, RpcError,
    UNAVAILABLE,
};
use so_core::{Config, Paths};
use so_store::Store;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc, watch};
use tokio::task::JoinSet;

use crate::dispatch::{self, ConnCtx, Notifier, Router, internal};
use crate::error::{Result, SupervisorError};
use crate::lines::{Line, read_line};
use crate::lock::SupervisorLock;
use crate::reconcile::{self, ReconcileHook};
use crate::runs::RunManager;

/// Longest accepted request line.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const CONN_GRACE: Duration = Duration::from_secs(2);

/// Reports work that must keep the supervisor alive even with no client connected
/// (active runs, UI streams). Later tasks plug theirs in through [`SupervisorState`].
pub trait ActivityProbe: Send + Sync {
    fn busy(&self) -> bool;
}

impl<F: Fn() -> bool + Send + Sync> ActivityProbe for F {
    fn busy(&self) -> bool {
        self()
    }
}

type Setup = Box<dyn FnOnce(&Arc<SupervisorState>) + Send>;

pub struct SupervisorOptions {
    pub paths: Paths,
    pub config: Config,
    pub socket_path: PathBuf,
    pub idle_exit: Duration,
    /// Install SIGTERM/SIGINT handlers that shut the supervisor down gracefully.
    pub handle_signals: bool,
    /// Longest wait for the single-instance lock when a previous supervisor is still
    /// shutting down; after that `AlreadyRunning` is returned.
    pub lock_wait: Duration,
    /// Requests one connection may have in flight before further ones get UNAVAILABLE.
    pub max_inflight: usize,
    /// Runs after the state exists and before reconcile and bind: register methods, the
    /// activity probe and the reconcile hook here.
    pub setup: Option<Setup>,
    /// Host environment for sign-in checks; default is [`HostEnv::detect`].
    pub host: Option<HostEnv>,
    /// Executable children run as their `mcp` server; default the current executable.
    pub mcp_exe: Option<PathBuf>,
    /// How often dead interactive wrapper sessions are ended (they hold account caps).
    /// Cap checks end them too, so this only keeps the stored state fresh.
    pub reap_interval: Duration,
}

impl SupervisorOptions {
    pub fn new(paths: Paths, config: Config, socket_path: PathBuf) -> Self {
        let idle_exit = config.supervisor.idle_exit.as_duration();
        Self {
            paths,
            config,
            socket_path,
            idle_exit,
            handle_signals: false,
            lock_wait: LOCK_WAIT,
            max_inflight: MAX_INFLIGHT,
            setup: None,
            host: None,
            mcp_exe: None,
            reap_interval: REAP_INTERVAL,
        }
    }

    pub fn with_reap_interval(mut self, d: Duration) -> Self {
        self.reap_interval = d.max(Duration::from_millis(1));
        self
    }

    pub fn with_mcp_exe(mut self, exe: PathBuf) -> Self {
        self.mcp_exe = Some(exe);
        self
    }

    pub fn with_lock_wait(mut self, d: Duration) -> Self {
        self.lock_wait = d;
        self
    }

    pub fn with_max_inflight(mut self, n: usize) -> Self {
        self.max_inflight = n.max(1);
        self
    }

    pub fn with_idle_exit(mut self, d: Duration) -> Self {
        self.idle_exit = d;
        self
    }

    pub fn with_signals(mut self) -> Self {
        self.handle_signals = true;
        self
    }

    pub fn with_host(mut self, host: HostEnv) -> Self {
        self.host = Some(host);
        self
    }

    pub fn with_setup(mut self, f: impl FnOnce(&Arc<SupervisorState>) + Send + 'static) -> Self {
        self.setup = Some(Box::new(f));
        self
    }
}

/// State shared by every connection.
pub struct SupervisorState {
    pub store: Store,
    pub config: Config,
    pub paths: Paths,
    pub router: Router,
    /// `None` when the host environment could not be detected (no `HOME`).
    pub host: Option<HostEnv>,
    /// Runs: spawn, queue, wait, cancel.
    pub runs: RunManager,
    /// Shared HTTP client for outbound calls (the route decider). Never follows redirects, so
    /// the bearer key cannot be forwarded to another host.
    pub http: reqwest::Client,
    max_inflight: usize,
    pub(crate) started: Instant,
    clients: AtomicUsize,
    /// Bumped on every connect so a short-lived client resets the idle clock.
    epoch: AtomicU64,
    probe: RwLock<Option<Arc<dyn ActivityProbe>>>,
    /// Open [`ActivityGuard`]s (UI streams): the supervisor does not idle-exit while > 0.
    guards: AtomicUsize,
    hook: RwLock<Option<Arc<dyn ReconcileHook>>>,
    shutdown: watch::Sender<bool>,
}

impl SupervisorState {
    pub fn set_probe(&self, probe: Arc<dyn ActivityProbe>) {
        *self.probe.write().unwrap_or_else(|e| e.into_inner()) = Some(probe);
    }

    pub fn set_reconcile_hook(&self, hook: Arc<dyn ReconcileHook>) {
        *self.hook.write().unwrap_or_else(|e| e.into_inner()) = Some(hook);
    }

    /// Number of currently connected clients.
    pub fn connected_clients(&self) -> usize {
        self.clients.load(Ordering::SeqCst)
    }

    /// A receiver that turns `true` when shutdown is requested.
    pub fn shutdown_signal(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    /// Asks the supervisor to shut down gracefully.
    pub fn request_shutdown(&self) {
        self.shutdown.send_replace(true);
    }

    /// Runs a blocking store call off the async runtime.
    pub async fn db<T, F>(&self, f: F) -> std::result::Result<T, RpcError>
    where
        T: Send + 'static,
        F: FnOnce(&Store) -> so_store::Result<T> + Send + 'static,
    {
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || f(&store))
            .await
            .map_err(internal)?
            .map_err(internal)
    }

    /// Marks the supervisor busy until the guard is dropped (for example while a UI stream
    /// is open), so idle exit waits.
    pub fn activity_guard(self: &Arc<Self>) -> ActivityGuard {
        self.guards.fetch_add(1, Ordering::SeqCst);
        ActivityGuard(self.clone())
    }

    /// True while a run is active or an [`ActivityGuard`] is open.
    pub fn busy(&self) -> bool {
        self.guards.load(Ordering::SeqCst) > 0
            || self
                .probe
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
                .is_some_and(|p| p.busy())
    }
}

/// Keeps the supervisor from idle-exiting while alive. See [`SupervisorState::activity_guard`].
pub struct ActivityGuard(Arc<SupervisorState>);

impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.0.guards.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct Supervisor;

/// Handle to a running supervisor.
pub struct SupervisorHandle {
    state: Arc<SupervisorState>,
    socket_path: PathBuf,
    join: tokio::task::JoinHandle<()>,
}

impl SupervisorHandle {
    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub fn state(&self) -> &Arc<SupervisorState> {
        &self.state
    }

    /// Waits until the supervisor has shut down (socket removed, lock released).
    pub async fn wait(self) {
        let _ = self.join.await;
    }

    /// Requests a graceful shutdown and waits for it.
    pub async fn shutdown(self) {
        self.state.request_shutdown();
        self.wait().await;
    }
}

impl Supervisor {
    /// Takes the single-instance lock, reconciles the store, binds the socket (0600 in a
    /// user-only directory) and starts serving.
    pub async fn start(opts: SupervisorOptions) -> Result<SupervisorHandle> {
        let SupervisorOptions {
            paths,
            config,
            socket_path,
            idle_exit,
            handle_signals,
            lock_wait,
            max_inflight,
            setup,
            host,
            mcp_exe,
            reap_interval,
        } = opts;

        let lock = acquire_lock(&paths.config_dir, lock_wait).await?;
        let store = {
            let db = paths.state_db();
            tokio::task::spawn_blocking(move || Store::open(db))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))??
        };
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let host = host.or_else(|| HostEnv::detect().ok());
        let runs = RunManager::new(
            store.clone(),
            config.clone(),
            paths.clone(),
            host.clone(),
            socket_path.clone(),
            mcp_exe,
        );
        let state = Arc::new(SupervisorState {
            store,
            config,
            paths,
            router: Router::default(),
            host,
            runs,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap_or_default(),
            max_inflight,
            started: Instant::now(),
            clients: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            probe: RwLock::new(None),
            guards: AtomicUsize::new(0),
            hook: RwLock::new(None),
            shutdown: shutdown_tx,
        });
        if let Some(setup) = setup {
            setup(&state);
        }

        {
            let st = state.clone();
            let failed = tokio::task::spawn_blocking(move || {
                let hook = st.hook.read().unwrap_or_else(|e| e.into_inner()).clone();
                let failed = reconcile::reconcile(&st.store, hook.as_deref())?;
                reconcile::cleanup_stale_codex_ws_tokens(&st.store, &st.paths);
                Ok::<_, SupervisorError>(failed)
            })
            .await
            .map_err(|e| std::io::Error::other(e.to_string()))??;
            if failed > 0 {
                tracing::warn!(failed, "reconciled orphaned runs");
            }
        }

        // The lock is held, so any socket file left behind belongs to a dead supervisor.
        let listener = bind_socket(&socket_path)?;
        tracing::info!(socket = %socket_path.display(), "supervisor listening");

        if handle_signals {
            spawn_signal_task(state.clone())?;
        }
        tokio::spawn(idle_monitor(state.clone(), idle_exit, shutdown_rx.clone()));
        tokio::spawn(reap_loop(state.clone(), reap_interval, shutdown_rx.clone()));
        let join = tokio::spawn(accept_loop(
            state.clone(),
            listener,
            lock,
            socket_path.clone(),
            shutdown_rx,
        ));
        Ok(SupervisorHandle {
            state,
            socket_path,
            join,
        })
    }
}

/// Takes the lock, retrying while a previous supervisor is still shutting down.
async fn acquire_lock(config_dir: &Path, wait: Duration) -> Result<SupervisorLock> {
    let deadline = Instant::now() + wait;
    loop {
        match SupervisorLock::acquire(config_dir) {
            Err(SupervisorError::AlreadyRunning { .. }) if Instant::now() < deadline => {
                tokio::time::sleep(LOCK_STEP).await;
            }
            r => return r,
        }
    }
}

fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Creates the socket directory (0700) if needed and refuses an existing one that is owned
/// by someone else or open to group/other. Binds under a temporary name, chmods it to 0600
/// and renames it into place, so the socket never appears with looser permissions.
fn bind_socket(socket_path: &Path) -> Result<UnixListener> {
    let dir = socket_path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    match std::fs::metadata(dir) {
        Ok(meta) => {
            if meta.uid() != euid() || meta.permissions().mode() & 0o077 != 0 {
                return Err(SupervisorError::InsecureSocketDir(dir.to_path_buf()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)?;
        }
        Err(e) => return Err(e.into()),
    }
    // The lock is held, so any socket file left behind belongs to a dead supervisor.
    match std::fs::symlink_metadata(socket_path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(socket_path)?,
        Ok(_) => return Err(SupervisorError::NotASocket(socket_path.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let tmp = dir.join(format!(".s{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let listener = UnixListener::bind(&tmp)?;
    let finish = || -> std::io::Result<()> {
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        std::fs::rename(&tmp, socket_path)
    };
    if let Err(e) = finish() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(listener)
}

fn spawn_signal_task(state: Arc<SupervisorState>) -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    tokio::spawn(async move {
        tokio::select! {
            _ = term.recv() => tracing::info!("SIGTERM received"),
            _ = int.recv() => tracing::info!("SIGINT received"),
        }
        state.request_shutdown();
    });
    Ok(())
}

/// Ends interactive sessions whose process died, so their account caps free up.
async fn reap_loop(
    state: Arc<SupervisorState>,
    every: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        let store = state.store.clone();
        match tokio::task::spawn_blocking(move || store.reap_dead_interactive_sessions()).await {
            Ok(Ok(0)) => {}
            Ok(Ok(n)) => {
                tracing::info!(ended = n, "ended dead interactive sessions");
                state.runs.request_drain();
            }
            Ok(Err(e)) => tracing::warn!(error = %e, "could not end dead interactive sessions"),
            Err(e) => tracing::warn!(error = %e, "session reaper panicked"),
        }
        tokio::select! {
            _ = wait_shutdown(&mut shutdown) => return,
            _ = tokio::time::sleep(every) => {}
        }
    }
}

/// Requests shutdown once no client has been connected and the probe has been quiet for
/// `idle_exit`.
async fn idle_monitor(
    state: Arc<SupervisorState>,
    idle_exit: Duration,
    mut shutdown: watch::Receiver<bool>,
) {
    let tick = (idle_exit / 10).clamp(Duration::from_millis(10), Duration::from_millis(500));
    let mut idle_since = Instant::now();
    let mut seen_epoch = state.epoch.load(Ordering::SeqCst);
    loop {
        tokio::select! {
            _ = wait_shutdown(&mut shutdown) => return,
            _ = tokio::time::sleep(tick) => {}
        }
        let epoch = state.epoch.load(Ordering::SeqCst);
        if epoch != seen_epoch || state.connected_clients() > 0 || state.busy() {
            seen_epoch = epoch;
            idle_since = Instant::now();
        } else if idle_since.elapsed() >= idle_exit {
            // Re-check right before deciding: a client may have just connected.
            if state.epoch.load(Ordering::SeqCst) != epoch
                || state.connected_clients() > 0
                || state.busy()
            {
                continue;
            }
            tracing::info!(?idle_exit, "idle, shutting down");
            state.request_shutdown();
            return;
        }
    }
}

async fn accept_loop(
    state: Arc<SupervisorState>,
    listener: UnixListener,
    lock: SupervisorLock,
    socket_path: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut conns = JoinSet::new();
    let conn_rx = shutdown.clone();
    loop {
        tokio::select! {
            _ = wait_shutdown(&mut shutdown) => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => {
                    let guard = ConnGuard::new(state.clone());
                    conns.spawn(serve_connection(
                        state.clone(),
                        guard,
                        stream,
                        conn_rx.clone(),
                    ));
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        }
        while let Some(r) = conns.try_join_next() {
            log_join(r);
        }
    }
    drop(listener);
    if let Err(e) = std::fs::remove_file(&socket_path) {
        tracing::warn!(error = %e, "could not remove socket file");
    }
    // Release the lock before draining so a replacement daemon can start right away; the
    // connections left only end their sessions in the shared store.
    drop(lock);
    // Connections watch the same shutdown flag; give them time to end their sessions.
    let drain = async {
        while let Some(r) = conns.join_next().await {
            log_join(r);
        }
    };
    if tokio::time::timeout(CONN_GRACE, drain).await.is_err() {
        tracing::warn!("connections did not close in time");
        conns.abort_all();
    }
    tracing::info!("supervisor stopped");
}

const MAX_INFLIGHT: usize = 64;
const REAP_INTERVAL: Duration = Duration::from_secs(30);
const LOCK_WAIT: Duration = Duration::from_secs(3);
const LOCK_STEP: Duration = Duration::from_millis(50);
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
const WRITER_FLUSH: Duration = Duration::from_secs(1);
const FALLBACK_ERROR: &str = r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"response serialization failed"}}"#;

/// Aborts the task when dropped.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Per-connection bookkeeping that must happen even when the connection task is aborted:
/// the client count and ending the orchestrator session.
pub(crate) struct ConnGuard {
    state: Arc<SupervisorState>,
    /// Orchestrator session to end when the connection goes away.
    pub(crate) session: Option<String>,
}

impl ConnGuard {
    pub(crate) fn new(state: Arc<SupervisorState>) -> Self {
        state.clients.fetch_add(1, Ordering::SeqCst);
        state.epoch.fetch_add(1, Ordering::SeqCst);
        Self {
            state,
            session: None,
        }
    }
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.state.clients.fetch_sub(1, Ordering::SeqCst);
        self.state.epoch.fetch_add(1, Ordering::SeqCst);
        if let Some(id) = self.session.take() {
            self.state.runs.session_disconnected(&id);
            let store = self.state.store.clone();
            let work = move || log_session_end(&id, end_session_blocking(&store, &id));
            match tokio::runtime::Handle::try_current() {
                Ok(h) => {
                    h.spawn_blocking(work);
                }
                Err(_) => work(),
            }
        }
    }
}

fn log_join(r: std::result::Result<(), tokio::task::JoinError>) {
    if let Err(e) = r
        && !e.is_cancelled()
    {
        tracing::error!(error = %e, "connection task failed");
    }
}

fn error_line(id: Value, code: i64, message: &str) -> String {
    serde_json::to_string(&Response::err(id, RpcError::new(code, message)))
        .unwrap_or_else(|_| FALLBACK_ERROR.to_string())
}

fn response_line(resp: &Response) -> String {
    serde_json::to_string(resp)
        .unwrap_or_else(|e| error_line(resp.id.clone(), INTERNAL_ERROR, &e.to_string()))
}

enum End {
    /// Client closed its write side: stop reading, let in-flight handlers finish.
    Eof,
    /// Shutdown, write failure or a client that cannot keep up: drop everything.
    Abort,
}

async fn writer_task(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::Receiver<String>,
    closed: Arc<watch::Sender<bool>>,
) {
    while let Some(line) = rx.recv().await {
        let written = tokio::time::timeout(WRITE_TIMEOUT, wr.write_all(line.as_bytes())).await;
        if !matches!(written, Ok(Ok(()))) {
            closed.send_replace(true);
            return;
        }
    }
    let _ = wr.shutdown().await;
}

/// Runs one request in its own task so a panic becomes an INTERNAL_ERROR reply.
async fn run_handler(
    state: Arc<SupervisorState>,
    ctx: ConnCtx,
    req: Request,
    _permit: OwnedSemaphorePermit,
) {
    let id = req.id.clone();
    let method = req.method.clone();
    let notifier = ctx.notifier.clone();
    let mut inner = AbortOnDrop(tokio::spawn(async move {
        dispatch::handle_other(&state, ctx, req).await
    }));
    let resp = match (&mut inner.0).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(%method, error = %e, "handler failed");
            Response::err(
                id,
                RpcError::new(INTERNAL_ERROR, "internal error: handler failed"),
            )
        }
    };
    notifier.send_line(response_line(&resp)).await;
}

async fn serve_connection(
    state: Arc<SupervisorState>,
    mut guard: ConnGuard,
    stream: UnixStream,
    mut shutdown: watch::Receiver<bool>,
) {
    let (rd, wr) = stream.into_split();
    let (tx, rx) = mpsc::channel::<String>(1024);
    let (closed_tx, closed_rx) = watch::channel(false);
    let closed_tx = Arc::new(closed_tx);
    let mut writer = AbortOnDrop(tokio::spawn(writer_task(wr, rx, closed_tx.clone())));
    let notifier = Notifier::new(tx);
    let mut ctx = ConnCtx::new(notifier.clone(), closed_rx.clone());
    let mut write_failed = closed_rx;
    let mut reader = BufReader::new(rd);
    let mut inflight = JoinSet::new();
    let permits = Arc::new(Semaphore::new(state.max_inflight));

    let end = 'conn: loop {
        while inflight.try_join_next().is_some() {}
        let line = tokio::select! {
            _ = wait_shutdown(&mut shutdown) => break End::Abort,
            _ = wait_shutdown(&mut write_failed) => break End::Abort,
            l = read_line(&mut reader, MAX_LINE_BYTES) => l,
        };
        // Protocol-level replies never wait: a full queue means the client is not reading.
        macro_rules! reply {
            ($line:expr) => {
                if !notifier.try_send_line($line) {
                    break 'conn End::Abort;
                }
            };
        }
        let bytes = match line {
            Ok(Line::Data(b)) => b,
            Ok(Line::Eof) | Err(_) => break End::Eof,
            Ok(Line::TooLong) => {
                let msg = format!("request line exceeds {MAX_LINE_BYTES} bytes");
                reply!(error_line(Value::Null, INVALID_REQUEST, &msg));
                continue;
            }
        };
        if bytes.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(e) => {
                let msg = format!("parse error: {e}");
                reply!(error_line(Value::Null, PARSE_ERROR, &msg));
                continue;
            }
        };
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let req: Request = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid request: {e}");
                reply!(error_line(id, INVALID_REQUEST, &msg));
                continue;
            }
        };
        if req.method == METHOD_HELLO {
            let resp = dispatch::handle_hello(&state, &mut ctx, req).await;
            if guard.session.is_none() && ctx.run_id.is_none() {
                guard.session = ctx.session_id.clone();
                if let Some(id) = &guard.session {
                    state.runs.session_connected(id);
                }
            }
            reply!(response_line(&resp));
        } else {
            // Never wait for a permit: the loop must keep reading to see EOF.
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                reply!(error_line(
                    req.id,
                    UNAVAILABLE,
                    "too many requests in flight"
                ));
                continue;
            };
            inflight.spawn(run_handler(state.clone(), ctx.clone(), req, permit));
        }
    };

    // Tell long-running handlers the client is gone.
    closed_tx.send_replace(true);
    let mut aborted = matches!(end, End::Abort);
    if aborted {
        inflight.abort_all();
    }
    loop {
        tokio::select! {
            r = inflight.join_next() => {
                if r.is_none() {
                    break;
                }
            }
            _ = wait_shutdown(&mut shutdown), if !aborted => {
                inflight.abort_all();
                aborted = true;
            }
        }
    }
    drop(ctx);
    drop(notifier);
    if tokio::time::timeout(WRITER_FLUSH, &mut writer.0)
        .await
        .is_err()
    {
        writer.0.abort();
    }
    if let Some(session_id) = guard.session.take() {
        // Not connected any more: a run (or workflow) that finishes later may end the session.
        state.runs.session_disconnected(&session_id);
        end_orchestrator_session(&state, session_id).await;
    }
}

/// Ends the session unless it still has active runs (a later task ends it then).
fn end_session_blocking(store: &Store, id: &str) -> so_store::Result<bool> {
    // A workflow between two steps has no active run, but its session is still in use.
    let active = store
        .list_runs_by_session(id, 100_000)?
        .iter()
        .any(|r| r.state.is_active())
        || store.session_has_running_workflow(id)?;
    if active {
        return Ok(false);
    }
    store.end_session(id, "ended", chrono::Utc::now())?;
    Ok(true)
}

fn log_session_end(session_id: &str, r: so_store::Result<bool>) {
    match r {
        Ok(true) => tracing::debug!(%session_id, "session ended"),
        Ok(false) => tracing::debug!(%session_id, "session kept open: active runs"),
        Err(e) => tracing::warn!(%session_id, error = %e, "could not end session"),
    }
}

async fn end_orchestrator_session(state: &Arc<SupervisorState>, session_id: String) {
    let id = session_id.clone();
    let r = state.db(move |s| end_session_blocking(s, &id)).await;
    match r {
        Ok(v) => log_session_end(&session_id, Ok(v)),
        Err(e) => tracing::warn!(%session_id, error = %e.message, "could not end session"),
    }
}

/// Resolves once shutdown was requested. Returns `()` so no `watch::Ref` (not `Send`) is
/// kept across an await by callers.
async fn wait_shutdown(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use so_core::SessionKind;
    use so_store::SessionRow;

    /// A connection task that is aborted must still decrement the client count and end its
    /// orchestrator session (the shutdown-timeout abort path).
    #[tokio::test]
    async fn aborted_connection_task_still_cleans_up() {
        let dir = tempfile::Builder::new()
            .prefix("so-")
            .tempdir_in("/tmp")
            .unwrap();
        let paths = Paths::from_dir(dir.path().join("cfg"));
        let opts = SupervisorOptions::new(paths, Config::default(), dir.path().join("run/s.sock"));
        let h = Supervisor::start(opts).await.unwrap();
        let state = h.state().clone();
        let row = SessionRow {
            id: so_core::new_id(),
            kind: SessionKind::Orchestrator,
            client: "t".into(),
            cwd: None,
            account: None,
            pid: None,
            started_at: chrono::Utc::now(),
            ended_at: None,
            status: "active".into(),
        };
        let id = row.id.clone();
        state.store.insert_session(&row).unwrap();
        let mut guard = ConnGuard::new(state.clone());
        guard.session = Some(id.clone());
        assert_eq!(state.connected_clients(), 1);
        let task = tokio::spawn(async move {
            let _guard = guard;
            std::future::pending::<()>().await;
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        for _ in 0..100 {
            let ended = state.store.get_session(&id).unwrap().unwrap().status == "ended";
            if ended && state.connected_clients() == 0 {
                h.shutdown().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        panic!("cleanup did not run");
    }
}
