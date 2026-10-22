//! Client side of the supervisor socket.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use so_core::proto::{
    HelloParams, HelloResult, METHOD_HELLO, Notification, Request, Response, RpcError,
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
    #[error("call did not complete within {0:?}")]
    CallTimeout(Duration),
    #[error("request exceeds {MAX_LINE_BYTES} bytes")]
    RequestTooLarge,
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
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        lock(self.pending).map.remove(&self.id);
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
    pub async fn connect_or_spawn(
        socket_path: &Path,
        spawn: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Self, ClientError> {
        let mut spawn = Some(spawn);
        Self::connect_with(socket_path, &mut || match spawn.take() {
            Some(f) => f(),
            None => Ok(()),
        })
        .await
    }

    /// Like [`connect_or_spawn`](Self::connect_or_spawn) followed by `hello`. If the daemon
    /// closes the connection before answering (for example it was just idling out), the whole
    /// sequence is retried once, so `spawn` may be called twice.
    pub async fn connect_or_spawn_hello(
        socket_path: &Path,
        spawn: impl FnMut() -> std::io::Result<()>,
        client: &str,
        cwd: &str,
        run_id: Option<String>,
        pid: Option<u32>,
    ) -> Result<(Self, HelloResult), ClientError> {
        Self::connect_or_spawn_with(socket_path, spawn, client, cwd, run_id, pid, false).await
    }

    /// Like [`connect_or_spawn_hello`](Self::connect_or_spawn_hello), but the `hello` opens no
    /// session: for commands that only read or control (`status`, `runs show`, `join`).
    pub async fn connect_or_spawn_inspect(
        socket_path: &Path,
        spawn: impl FnMut() -> std::io::Result<()>,
        client: &str,
        cwd: &str,
        pid: Option<u32>,
    ) -> Result<Self, ClientError> {
        let (c, _) =
            Self::connect_or_spawn_with(socket_path, spawn, client, cwd, None, pid, true).await?;
        Ok(c)
    }

    async fn connect_or_spawn_with(
        socket_path: &Path,
        mut spawn: impl FnMut() -> std::io::Result<()>,
        client: &str,
        cwd: &str,
        run_id: Option<String>,
        pid: Option<u32>,
        inspect: bool,
    ) -> Result<(Self, HelloResult), ClientError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let c = Self::connect_with(socket_path, &mut spawn).await?;
            let hello = if inspect {
                c.hello_inspect(client, cwd, pid).await
            } else {
                c.hello(client, cwd, run_id.clone(), pid).await
            };
            match hello {
                Ok(r) => return Ok((c, r)),
                Err(ClientError::Disconnected) if attempt < 2 => {}
                Err(e) => return Err(e),
            }
        }
    }

    async fn connect_with(
        socket_path: &Path,
        spawn: &mut dyn FnMut() -> std::io::Result<()>,
    ) -> Result<Self, ClientError> {
        match Self::connect(socket_path).await {
            Ok(c) => return Ok(c),
            Err(e) if is_not_listening(&e) => {}
            Err(e) => return Err(e),
        }
        spawn()?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            match Self::connect(socket_path).await {
                Ok(c) => return Ok(c),
                Err(e) if is_not_listening(&e) && Instant::now() < deadline => {}
                Err(e) if is_not_listening(&e) => return Err(ClientError::Timeout(SPAWN_WAIT)),
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
        let _guard = PendingGuard {
            pending: &self.pending,
            id,
        };
        self.tx
            .send(line)
            .await
            .map_err(|_| ClientError::Disconnected)?;
        let resp = orx.await.map_err(|_| ClientError::Disconnected)?;
        match (resp.error, resp.result) {
            (Some(e), _) => Err(ClientError::Rpc(e)),
            (None, result) => Ok(serde_json::from_value(result.unwrap_or_default())?),
        }
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
        self.call(
            METHOD_HELLO,
            HelloParams {
                client: client.to_string(),
                cwd: cwd.to_string(),
                run_id: None,
                pid,
                inspect: true,
            },
        )
        .await
    }
}

/// Starts `<exe> daemon` detached in its own process group with stdio on /dev/null.
/// `config_dir` is passed through `SUPERORCHESTRATOR_CONFIG_DIR`.
pub fn spawn_daemon_process(exe: &Path, config_dir: &Path) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let mut child = Command::new(exe)
        .arg("daemon")
        .env("SUPERORCHESTRATOR_CONFIG_DIR", PathBuf::from(config_dir))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Reap the child if it exits while we are still alive.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}
