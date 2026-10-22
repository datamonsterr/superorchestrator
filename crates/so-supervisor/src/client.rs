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
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, oneshot};

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
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Response>>>>;

pub struct SupervisorClient {
    writer: tokio::sync::Mutex<OwnedWriteHalf>,
    pending: Pending,
    next_id: AtomicU64,
    notifications: Mutex<Option<mpsc::UnboundedReceiver<Notification>>>,
}

impl SupervisorClient {
    pub async fn connect(socket_path: &Path) -> Result<Self, ClientError> {
        let stream = UnixStream::connect(socket_path).await?;
        let (rd, wr) = stream.into_split();
        let pending: Pending = Arc::default();
        let (ntx, nrx) = mpsc::unbounded_channel();
        let p = pending.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(rd).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if value.get("id").is_some_and(|id| !id.is_null()) {
                    if let Ok(resp) = serde_json::from_value::<Response>(value)
                        && let Some(id) = resp.id.as_u64()
                        && let Some(tx) = p.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)
                    {
                        let _ = tx.send(resp);
                    }
                } else if let Ok(note) = serde_json::from_value::<Notification>(value) {
                    let _ = ntx.send(note);
                }
            }
            // Dropping the senders wakes every waiting caller with `Disconnected`.
            p.lock().unwrap_or_else(|e| e.into_inner()).clear();
        });
        Ok(Self {
            writer: tokio::sync::Mutex::new(wr),
            pending,
            next_id: AtomicU64::new(1),
            notifications: Mutex::new(Some(nrx)),
        })
    }

    /// Connects; if nothing listens, runs `spawn` (which should start the daemon) and polls
    /// every 50 ms for up to 5 s.
    pub async fn connect_or_spawn(
        socket_path: &Path,
        spawn: impl FnOnce() -> std::io::Result<()>,
    ) -> Result<Self, ClientError> {
        if let Ok(c) = Self::connect(socket_path).await {
            return Ok(c);
        }
        spawn()?;
        let deadline = Instant::now() + SPAWN_WAIT;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            match Self::connect(socket_path).await {
                Ok(c) => return Ok(c),
                Err(_) if Instant::now() < deadline => {}
                Err(_) => return Err(ClientError::Timeout(SPAWN_WAIT)),
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

    pub async fn call<P: Serialize, R: DeserializeOwned>(
        &self,
        method: &str,
        params: P,
    ) -> Result<R, ClientError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = Request::new(id, method, params)?;
        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, tx);
        let sent = self.writer.lock().await.write_all(line.as_bytes()).await;
        if let Err(e) = sent {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
            return Err(e.into());
        }
        let resp = rx.await.map_err(|_| ClientError::Disconnected)?;
        match (resp.error, resp.result) {
            (Some(e), _) => Err(ClientError::Rpc(e)),
            (None, result) => Ok(serde_json::from_value(result.unwrap_or_default())?),
        }
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
