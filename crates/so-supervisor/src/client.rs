//! Client side of the supervisor socket.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use so_core::proto::{
    HelloParams, HelloResult, INVALID_PARAMS, METHOD_HELLO, Notification, Request, Response,
    RpcError,
};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

use crate::lines::{Line, read_line};
use crate::server::MAX_LINE_BYTES;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const SPAWN_WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{} (code {})", .0.message, .0.code)]
    Rpc(RpcError),
    #[error("bad json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("connection to the supervisor closed")]
    Disconnected,
    #[error("supervisor did not come up within {0:?}")]
    Timeout(Duration),
    #[error("{0}")]
    Startup(String),
    #[error("call did not complete within {0:?}")]
    CallTimeout(Duration),
    #[error("request exceeds {MAX_LINE_BYTES} bytes")]
    RequestTooLarge,
}

/// Trait for the result of a supervisor spawn action.
///
/// Implementors can report if a child process has already terminated and provide
/// any startup error output captured from standard error.
pub trait SpawnResult {
    fn check_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
        Ok(None)
    }
    fn read_stderr(&self) -> Option<String> {
        None
    }
}

impl SpawnResult for () {}

impl SpawnResult for DaemonProcess {
    fn check_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.try_wait()
    }
    fn read_stderr(&self) -> Option<String> {
        self.read_stderr_tail()
    }
}

impl<T: SpawnResult> SpawnResult for Option<T> {
    fn check_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
        match self {
            Some(inner) => inner.check_exit(),
            None => Ok(None),
        }
    }
    fn read_stderr(&self) -> Option<String> {
        self.as_ref().and_then(|inner| inner.read_stderr())
    }
}

impl SpawnResult for std::process::Child {
    fn check_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.try_wait()
    }
    fn read_stderr(&self) -> Option<String> {
        None
    }
}

#[derive(Default)]
struct Pending {
    /// Set under the same lock that guards `map` once the connection is dead, so a late
    /// `call` can never register an entry nobody will answer.
    closed: bool,
    map: HashMap<u64, oneshot::Sender<Response>>,
}

type Shared = Arc<Mutex<Pending>>;

fn lock(p: &Shared) -> std::sync::MutexGuard<'_, Pending> {
    p.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marks the connection dead and wakes every waiting caller with `Disconnected`.
fn close_pending(p: &Shared) {
    let mut g = lock(p);
    g.closed = true;
    g.map.clear();
}

/// Removes a call's pending entry when its future finishes or is dropped.
struct PendingGuard<'a> {
    pending: &'a Shared,
    id: u64,
    tx: mpsc::Sender<String>,
    completed: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        lock(self.pending).map.remove(&self.id);
        if !self.completed {
            let note = Notification {
                jsonrpc: so_core::proto::JSONRPC.to_string(),
                method: so_core::proto::NOTIFICATION_CANCEL_REQUEST.to_string(),
                params: serde_json::json!({"id": self.id}),
            };
            if let Ok(mut line) = serde_json::to_string(&note) {
                line.push('\n');
                if self.tx.try_send(line.clone()).is_err()
                    && let Ok(rt) = tokio::runtime::Handle::try_current()
                {
                    let tx = self.tx.clone();
                    rt.spawn(async move {
                        let _ = tx.send(line).await;
                    });
                }
            }
        }
    }
}

pub struct SupervisorClient {
    /// Whole request lines; a writer task owns the socket so a cancelled `call` can never
    /// leave half a line on the wire.
    tx: mpsc::Sender<String>,
    pending: Shared,
    next_id: AtomicU64,
    notifications: Mutex<Option<mpsc::UnboundedReceiver<Notification>>>,
    reader: tokio::task::JoinHandle<()>,
    writer: tokio::task::JoinHandle<()>,
}

impl Drop for SupervisorClient {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

fn is_not_listening(e: &ClientError) -> bool {
    matches!(e, ClientError::Io(e) if matches!(
        e.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    ))
}

async fn writer_task(mut wr: OwnedWriteHalf, mut rx: mpsc::Receiver<String>, pending: Shared) {
    while let Some(line) = rx.recv().await {
        if wr.write_all(line.as_bytes()).await.is_err() {
            close_pending(&pending);
            return;
        }
    }
    let _ = wr.shutdown().await;
}

impl SupervisorClient {
    pub async fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (rd, wr) = stream.into_split();
        let pending: Shared = Arc::default();
        let (ntx, nrx) = mpsc::unbounded_channel();
        let (tx, rx) = mpsc::channel::<String>(256);
        let writer = tokio::spawn(writer_task(wr, rx, pending.clone()));
        let p = pending.clone();
        let reader = tokio::spawn(async move {
            let mut rd = BufReader::new(rd);
            loop {
                let bytes = match read_line(&mut rd, MAX_LINE_BYTES).await {
                    Ok(Line::Data(b)) => b,
                    Ok(Line::TooLong) => {
                        tracing::debug!("dropped an oversized line from the supervisor");
                        continue;
                    }
                    Ok(Line::Eof) | Err(_) => break,
                };
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                    tracing::debug!("dropped an unparseable line from the supervisor");
                    continue;
                };
                if value.get("id").is_some_and(|id| !id.is_null()) {
                    if let Ok(resp) = serde_json::from_value::<Response>(value)
                        && let Some(id) = resp.id.as_u64()
                        && let Some(tx) = lock(&p).map.remove(&id)
                    {
                        let _ = tx.send(resp);
                    }
                } else if let Ok(note) = serde_json::from_value::<Notification>(value) {
                    let _ = ntx.send(note);
                } else {
                    tracing::debug!("dropped an unrecognised message from the supervisor");
                }
            }
            close_pending(&p);
        });
        Ok(Self {
            tx,
            pending,
            next_id: AtomicU64::new(1),
            notifications: Mutex::new(Some(nrx)),
            reader,
            writer,
        })
    }

    /// Connects; if nothing listens (socket missing or refusing), runs `spawn` (which should
    /// start the daemon) and polls every 50 ms for up to 5 s.
    pub async fn connect_or_spawn<R: SpawnResult>(
        socket_path: &Path,
        spawn: impl FnOnce() -> std::io::Result<R>,
    ) -> Result<Self, ClientError> {
        let mut spawn = Some(spawn);
        Self::connect_with(socket_path, move || match spawn.take() {
            Some(f) => f(),
            None => Err(std::io::Error::other("spawn already called")),
        })
        .await
    }

    /// Like [`connect_or_spawn`](Self::connect_or_spawn) followed by `hello`. If the daemon
    /// closes the connection before answering (for example it was just idling out), the whole
    /// sequence is retried once, so `spawn` may be called twice.
    pub async fn connect_or_spawn_hello<R: SpawnResult>(
        socket_path: &Path,
        spawn: impl FnMut() -> std::io::Result<R>,
        client: &str,
        cwd: &str,
        run_id: Option<String>,
        pid: Option<u32>,
    ) -> Result<(Self, HelloResult), ClientError> {
        Self::connect_or_spawn_with(socket_path, spawn, client, cwd, run_id, pid, false).await
    }

    /// Like [`connect_or_spawn_hello`](Self::connect_or_spawn_hello), but the `hello` opens no
    /// session: for commands that only read or control (`status`, `runs show`, `join`).
    pub async fn connect_or_spawn_inspect<R: SpawnResult>(
        socket_path: &Path,
        spawn: impl FnMut() -> std::io::Result<R>,
        client: &str,
        cwd: &str,
        pid: Option<u32>,
    ) -> Result<Self, ClientError> {
        let (c, _) =
            Self::connect_or_spawn_with(socket_path, spawn, client, cwd, None, pid, true).await?;
        Ok(c)
    }

    async fn connect_or_spawn_with<R: SpawnResult>(
        socket_path: &Path,
        spawn: impl FnMut() -> std::io::Result<R>,
        client: &str,
        cwd: &str,
        run_id: Option<String>,
        pid: Option<u32>,
        inspect: bool,
    ) -> Result<(Self, HelloResult), ClientError> {
        Self::connect_or_spawn_authenticated(
            socket_path,
            spawn,
            HelloParams {
                client: client.to_string(),
                cwd: cwd.to_string(),
                run_id,
                run_token: None,
                pid,
                inspect,
            },
        )
        .await
    }

    /// Connects and proves a run identity in `hello`, retrying a disconnected hello once.
    pub async fn connect_or_spawn_authenticated<R: SpawnResult>(
        socket_path: &Path,
        mut spawn: impl FnMut() -> std::io::Result<R>,
        params: HelloParams,
    ) -> Result<(Self, HelloResult), ClientError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let c = Self::connect_with(socket_path, &mut spawn).await?;
            // An inspect hello that claims a run id must still prove that run's identity.
            let hello = if params.inspect && params.run_id.is_none() {
                c.hello_inspect(&params.client, &params.cwd, params.pid)
                    .await
            } else {
                c.call(METHOD_HELLO, &params).await
            };
            match hello {
                Ok(r) => return Ok((c, r)),
                Err(ClientError::Disconnected) if attempt < 2 => {}
                Err(e) => return Err(e),
            }
        }
    }

    async fn connect_with<R: SpawnResult>(
        socket_path: &Path,
        mut spawn: impl FnMut() -> std::io::Result<R>,
    ) -> Result<Self, ClientError> {
        match Self::connect(socket_path).await {
            Ok(c) => return Ok(c),
            Err(e) if is_not_listening(&e) => {}
            Err(e) => return Err(e),
        }
        let mut spawned = spawn()?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            match Self::connect(socket_path).await {
                Ok(c) => return Ok(c),
                Err(e) if is_not_listening(&e) => {
                    if let Some(status) = spawned.check_exit()? {
                        return Err(startup_error(Some(status), spawned.read_stderr(), false));
                    }
                    if Instant::now() >= deadline {
                        let status = spawned.check_exit().ok().flatten();
                        return Err(startup_error(status, spawned.read_stderr(), true));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Takes the receiver of server-pushed notifications (available once).
    pub fn notifications(&self) -> Option<mpsc::UnboundedReceiver<Notification>> {
        self.notifications
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }

    /// Number of calls currently waiting for a response.
    pub fn pending_calls(&self) -> usize {
        lock(&self.pending).map.len()
    }

    /// Sends a request and waits for its response. Safe to cancel: dropping the future
    /// removes the pending entry and never writes a partial line.
    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = Request::new(id, method, params)?;
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        if line.len() > MAX_LINE_BYTES {
            return Err(ClientError::RequestTooLarge);
        }
        let (otx, orx) = oneshot::channel();
        {
            let mut p = lock(&self.pending);
            if p.closed {
                return Err(ClientError::Disconnected);
            }
            p.map.insert(id, otx);
        }
        let mut guard = PendingGuard {
            pending: &self.pending,
            id,
            tx: self.tx.clone(),
            completed: false,
        };
        self.tx
            .send(line)
            .await
            .map_err(|_| ClientError::Disconnected)?;
        let resp = orx.await.map_err(|_| ClientError::Disconnected)?;
        guard.completed = true;
        match (resp.error, resp.result) {
            (Some(e), _) => Err(ClientError::Rpc(e)),
            (None, result) => Ok(serde_json::from_value(result.unwrap_or_default())?),
        }
    }

    /// Sends a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: impl Serialize) -> Result<(), ClientError> {
        let note = Notification {
            jsonrpc: so_core::proto::JSONRPC.to_string(),
            method: method.to_string(),
            params: serde_json::to_value(params)?,
        };
        let mut line = serde_json::to_string(&note)?;
        line.push('\n');
        self.tx
            .send(line)
            .await
            .map_err(|_| ClientError::Disconnected)
    }

    /// [`call`](Self::call) with a deadline; on expiry the call is abandoned.
    pub async fn call_timeout<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
        timeout: Duration,
    ) -> Result<R, ClientError> {
        tokio::time::timeout(timeout, self.call(method, params))
            .await
            .map_err(|_| ClientError::CallTimeout(timeout))?
    }

    pub async fn hello(
        &self,
        client: &str,
        cwd: &str,
        run_id: Option<String>,
        pid: Option<u32>,
    ) -> Result<HelloResult, ClientError> {
        self.call(
            METHOD_HELLO,
            HelloParams {
                client: client.to_string(),
                cwd: cwd.to_string(),
                run_id,
                run_token: None,
                pid,
                inspect: false,
            },
        )
        .await
    }

    /// `hello` that opens no session (see [`HelloParams::inspect`]).
    pub async fn hello_inspect(
        &self,
        client: &str,
        cwd: &str,
        pid: Option<u32>,
    ) -> Result<HelloResult, ClientError> {
        let r = self
            .call(
                METHOD_HELLO,
                HelloParams {
                    client: client.to_string(),
                    cwd: cwd.to_string(),
                    run_id: None,
                    run_token: None,
                    pid,
                    inspect: true,
                },
            )
            .await;
        match r {
            // A supervisor older than `inspect` rejects the unknown field (`deny_unknown_fields`):
            // fall back to the plain hello, which opens a session as it always did.
            Err(ClientError::Rpc(e))
                if e.code == INVALID_PARAMS && e.message.contains("inspect") =>
            {
                self.hello(client, cwd, None, pid).await
            }
            other => other,
        }
    }
}

fn startup_error(
    status: Option<ExitStatus>,
    stderr: Option<String>,
    timed_out: bool,
) -> ClientError {
    match (status, stderr, timed_out) {
        (None, None, true) => ClientError::Timeout(SPAWN_WAIT),
        (Some(status), Some(err), false) => {
            ClientError::Startup(format!("supervisor did not come up ({status}): {err}"))
        }
        (Some(status), None, false) => {
            ClientError::Startup(format!("supervisor did not come up ({status})"))
        }
        (Some(status), Some(err), true) => ClientError::Startup(format!(
            "supervisor did not come up within {SPAWN_WAIT:?} ({status}): {err}"
        )),
        (Some(status), None, true) => ClientError::Startup(format!(
            "supervisor did not come up within {SPAWN_WAIT:?} ({status})"
        )),
        (None, Some(err), true) => ClientError::Startup(format!(
            "supervisor did not come up within {SPAWN_WAIT:?}: {err}"
        )),
        (None, None, false) => ClientError::Timeout(SPAWN_WAIT),
        (None, Some(err), false) => {
            ClientError::Startup(format!("supervisor did not come up: {err}"))
        }
    }
}

/// A spawned daemon child whose exit status and startup log can be monitored.
pub struct DaemonProcess {
    child: Option<std::process::Child>,
    log_path: Option<PathBuf>,
}

impl DaemonProcess {
    pub fn new(child: std::process::Child, log_path: Option<PathBuf>) -> Self {
        Self {
            child: Some(child),
            log_path,
        }
    }

    /// Checks if the child process has exited without blocking.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if let Some(child) = self.child.as_mut() {
            let res = child.try_wait()?;
            if res.is_some() {
                self.child = None;
            }
            Ok(res)
        } else {
            Ok(None)
        }
    }

    /// Reads the last few non-empty lines from the daemon startup log if available.
    pub fn read_stderr_tail(&self) -> Option<String> {
        let path = self.log_path.as_ref()?;
        let content = std::fs::read_to_string(path).ok()?;
        let lines: Vec<&str> = content
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| {
                s.strip_prefix("error: ")
                    .or_else(|| s.strip_prefix("Error: "))
                    .unwrap_or(s)
            })
            .collect();
        if lines.is_empty() {
            return None;
        }
        let tail = if lines.len() > 5 {
            &lines[lines.len() - 5..]
        } else {
            &lines[..]
        };
        Some(tail.join("\n"))
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            // Reap the child if it exits while we are still alive.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

/// Starts `<exe> daemon` detached in its own process group with stderr redirected to
/// `<config_dir>/logs/daemon-start.log` (truncated per spawn).
/// `config_dir` is passed through `SUPERORCHESTRATOR_CONFIG_DIR`.
pub fn spawn_daemon_process(exe: &Path, config_dir: &Path) -> std::io::Result<DaemonProcess> {
    use std::os::unix::process::CommandExt;

    let logs_dir = config_dir.join("logs");
    if let Err(e) = std::fs::create_dir_all(&logs_dir) {
        tracing::debug!(
            "could not create logs directory {}: {e}",
            logs_dir.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&logs_dir, std::fs::Permissions::from_mode(0o700))
        {
            tracing::debug!("could not set permissions on {}: {e}", logs_dir.display());
        }
    }
    let log_path = logs_dir.join("daemon-start.log");

    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let (stderr_stdio, log_path_opt) = match opts.open(&log_path) {
        Ok(f) => (Stdio::from(f), Some(log_path)),
        Err(e) => {
            tracing::warn!("could not open {}: {e}", log_path.display());
            (Stdio::null(), None)
        }
    };

    let child = Command::new(exe)
        .arg("daemon")
        .env("SUPERORCHESTRATOR_CONFIG_DIR", PathBuf::from(config_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr_stdio)
        .process_group(0)
        .spawn()?;

    Ok(DaemonProcess::new(child, log_path_opt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn daemon_process_reads_stderr_tail_and_strips_error_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("daemon-start.log");
        std::fs::write(
            &log,
            "line 1\nerror: socket directory /tmp is not owned by the current user\n",
        )
        .unwrap();
        let proc = DaemonProcess {
            child: None,
            log_path: Some(log),
        };
        let tail = proc.read_stderr_tail().unwrap();
        assert_eq!(
            tail,
            "line 1\nsocket directory /tmp is not owned by the current user"
        );
    }

    #[test]
    fn startup_error_formats_exit_status_and_stderr() {
        let status = ExitStatus::from_raw(1 << 8);

        let err = startup_error(Some(status), Some("permission denied".into()), false);
        let msg = err.to_string();
        assert!(msg.contains("supervisor did not come up"));
        assert!(msg.contains("exit status: 1") || msg.contains("status"));
        assert!(msg.contains("permission denied"));

        let timeout_err = startup_error(None, Some("timed out detail".into()), true);
        assert_eq!(
            timeout_err.to_string(),
            format!("supervisor did not come up within {SPAWN_WAIT:?}: timed out detail")
        );
    }

    struct MockFailedDaemon;

    impl SpawnResult for MockFailedDaemon {
        fn check_exit(&mut self) -> std::io::Result<Option<ExitStatus>> {
            Ok(Some(ExitStatus::from_raw(3 << 8)))
        }
        fn read_stderr(&self) -> Option<String> {
            Some("something crashed at startup".into())
        }
    }

    #[tokio::test]
    async fn connect_or_spawn_fails_fast_when_daemon_exits_early() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("missing.sock");

        let start = Instant::now();
        let Err(err) = SupervisorClient::connect_or_spawn(&sock, || Ok(MockFailedDaemon)).await
        else {
            panic!("expected connect_or_spawn to fail");
        };

        let elapsed = start.elapsed();
        assert!(elapsed < Duration::from_secs(2), "took {elapsed:?}");
        let msg = err.to_string();
        assert!(msg.contains("something crashed at startup"), "{msg}");
        assert!(msg.contains("3"), "{msg}");
    }
}
