//! Unix-socket server: listener, connections, session lifecycle, idle exit, shutdown.

use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use so_core::proto::{INVALID_REQUEST, METHOD_HELLO, PARSE_ERROR, Request, Response, RpcError};
use so_core::{Config, Paths};
use so_store::Store;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;

use crate::dispatch::{self, ConnCtx, Notifier, Router, internal};
use crate::error::{Result, SupervisorError};
use crate::lock::SupervisorLock;
use crate::reconcile::{self, ReconcileHook};

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
    /// Runs after the state exists and before reconcile and bind: register methods, the
    /// activity probe and the reconcile hook here.
    pub setup: Option<Setup>,
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
            setup: None,
        }
    }

    pub fn with_idle_exit(mut self, d: Duration) -> Self {
        self.idle_exit = d;
        self
    }

    pub fn with_signals(mut self) -> Self {
        self.handle_signals = true;
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
    pub(crate) started: Instant,
    clients: AtomicUsize,
    /// Bumped on every connect so a short-lived client resets the idle clock.
    epoch: AtomicU64,
    probe: RwLock<Option<Arc<dyn ActivityProbe>>>,
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

    fn busy(&self) -> bool {
        self.probe
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|p| p.busy())
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
            setup,
        } = opts;

        let lock = SupervisorLock::acquire(&paths.config_dir)?;
        let store = {
            let db = paths.state_db();
            tokio::task::spawn_blocking(move || Store::open(db))
                .await
                .map_err(|e| std::io::Error::other(e.to_string()))??
        };
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let state = Arc::new(SupervisorState {
            store,
            config,
            paths,
            router: Router::default(),
            started: Instant::now(),
            clients: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            probe: RwLock::new(None),
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
                reconcile::reconcile(&st.store, hook.as_deref())
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

fn euid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// Creates the socket directory (0700) if needed, refuses one owned by someone else,
/// removes a stale socket, binds and chmods the socket to 0600.
fn bind_socket(socket_path: &Path) -> Result<UnixListener> {
    if let Some(dir) = socket_path.parent().filter(|d| !d.as_os_str().is_empty()) {
        match std::fs::metadata(dir) {
            Ok(meta) => {
                if meta.uid() != euid() {
                    return Err(SupervisorError::InsecureSocketDir(dir.to_path_buf()));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir_all(dir)?;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
            Err(e) => return Err(e.into()),
        }
    }
    match std::fs::symlink_metadata(socket_path) {
        Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(socket_path)?,
        Ok(_) => return Err(SupervisorError::NotASocket(socket_path.to_path_buf())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let listener = UnixListener::bind(socket_path)?;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
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
                    state.clients.fetch_add(1, Ordering::SeqCst);
                    state.epoch.fetch_add(1, Ordering::SeqCst);
                    let st = state.clone();
                    let rx = conn_rx.clone();
                    conns.spawn(async move {
                        serve_connection(&st, stream, rx).await;
                        st.clients.fetch_sub(1, Ordering::SeqCst);
                        st.epoch.fetch_add(1, Ordering::SeqCst);
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            },
        }
        while conns.try_join_next().is_some() {}
    }
    drop(listener);
    if let Err(e) = std::fs::remove_file(&socket_path) {
        tracing::warn!(error = %e, "could not remove socket file");
    }
    // Connections watch the same shutdown flag; give them time to end their sessions.
    let drain = async { while conns.join_next().await.is_some() {} };
    if tokio::time::timeout(CONN_GRACE, drain).await.is_err() {
        tracing::warn!("connections did not close in time");
        conns.abort_all();
    }
    drop(lock);
    tracing::info!("supervisor stopped");
}

enum Line {
    Eof,
    TooLong,
    Data(Vec<u8>),
}

/// Reads one `\n`-terminated line. A line longer than `cap` is consumed and discarded
/// (not buffered) and reported as `TooLong`, so the connection stays in sync.
async fn read_line<R: AsyncBufRead + Unpin>(r: &mut R, cap: usize) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let mut too_long = false;
    loop {
        let chunk = r.fill_buf().await?;
        if chunk.is_empty() {
            return Ok(if too_long {
                Line::TooLong
            } else if buf.is_empty() {
                Line::Eof
            } else {
                Line::Data(buf)
            });
        }
        let pos = chunk.iter().position(|b| *b == b'\n');
        let take = pos.unwrap_or(chunk.len());
        if !too_long {
            buf.extend_from_slice(&chunk[..take]);
            if buf.len() > cap {
                too_long = true;
                buf = Vec::new();
            }
        }
        r.consume(take + usize::from(pos.is_some()));
        if pos.is_some() {
            return Ok(if too_long {
                Line::TooLong
            } else {
                Line::Data(buf)
            });
        }
    }
}

fn error_line(id: Value, code: i64, message: &str) -> String {
    serde_json::to_string(&Response::err(id, RpcError::new(code, message))).unwrap_or_default()
}

fn response_line(resp: &Response) -> String {
    serde_json::to_string(resp).unwrap_or_else(|e| {
        error_line(
            resp.id.clone(),
            so_core::proto::INTERNAL_ERROR,
            &e.to_string(),
        )
    })
}

async fn serve_connection(
    state: &Arc<SupervisorState>,
    stream: UnixStream,
    mut shutdown: watch::Receiver<bool>,
) {
    let (rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::channel::<String>(1024);
    let writer = tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if wr.write_all(line.as_bytes()).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });
    let notifier = Notifier::new(tx);
    let mut ctx = ConnCtx::new(notifier.clone());
    let mut reader = BufReader::new(rd);
    let mut inflight = JoinSet::new();

    loop {
        let line = tokio::select! {
            _ = wait_shutdown(&mut shutdown) => break,
            l = read_line(&mut reader, MAX_LINE_BYTES) => l,
        };
        let bytes = match line {
            Ok(Line::Data(b)) => b,
            Ok(Line::Eof) | Err(_) => break,
            Ok(Line::TooLong) => {
                let msg = format!("request line exceeds {MAX_LINE_BYTES} bytes");
                notifier
                    .send_line(error_line(Value::Null, INVALID_REQUEST, &msg))
                    .await;
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
                notifier
                    .send_line(error_line(Value::Null, PARSE_ERROR, &msg))
                    .await;
                continue;
            }
        };
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let req: Request = match serde_json::from_value(value) {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("invalid request: {e}");
                notifier
                    .send_line(error_line(id, INVALID_REQUEST, &msg))
                    .await;
                continue;
            }
        };
        if req.method == METHOD_HELLO {
            let resp = dispatch::handle_hello(state, &mut ctx, req).await;
            notifier.send_line(response_line(&resp)).await;
        } else {
            let st = state.clone();
            let c = ctx.clone();
            inflight.spawn(async move {
                let resp = dispatch::handle_other(&st, c.clone(), req).await;
                c.notifier.send_line(response_line(&resp)).await;
            });
        }
    }

    // Drop in-flight handlers so their notifier clones release the writer.
    inflight.abort_all();
    while inflight.join_next().await.is_some() {}
    let session = ctx.session_id.clone();
    let owns_session = ctx.run_id.is_none();
    drop(ctx);
    drop(notifier);
    let _ = writer.await;
    if let (Some(session_id), true) = (session, owns_session) {
        end_orchestrator_session(state, session_id).await;
    }
}

/// Ends the session unless it still has active runs (a later task ends it then).
async fn end_orchestrator_session(state: &Arc<SupervisorState>, session_id: String) {
    let id = session_id.clone();
    let result = state
        .db(move |s| {
            let active = s
                .list_runs_by_session(&id, 100_000)?
                .iter()
                .any(|r| r.state.is_active());
            if active {
                return Ok(false);
            }
            s.end_session(&id, "ended", chrono::Utc::now())?;
            Ok(true)
        })
        .await;
    match result {
        Ok(true) => tracing::debug!(%session_id, "session ended"),
        Ok(false) => tracing::debug!(%session_id, "session kept open: active runs"),
        Err(e) => tracing::warn!(%session_id, error = %e.message, "could not end session"),
    }
}

/// Resolves once shutdown was requested. Returns `()` so no `watch::Ref` (not `Send`) is
/// kept across an await by callers.
async fn wait_shutdown(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|v| *v).await;
}
