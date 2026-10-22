//! The axum API, SSE and embedded frontend (SPEC 14).
//!
//! # Layout
//!
//! `so-web` runs *inside* the supervisor process and depends on `so-supervisor`; the
//! supervisor does not depend on it, so there is no cycle. Handlers use the same
//! functions as the JSON-RPC handlers (`RunManager`, `harnesses_view`, `services::*`,
//! `doctor::run_checks`) instead of going through the socket. The binary wires the two
//! together with [`install`], which registers the `ui_start` RPC and, when `[ui] enabled`
//! is set, starts the server with the supervisor.
//!
//! # Security
//!
//! Loopback by default. A non-loopback bind needs a token ([`WebError::TokenRequired`]).
//! Over plain HTTP the token and cookie travel in the clear; put TLS in front of a
//! non-loopback bind. Every request is checked for an allowed `Host`; `/api` requests also
//! for a same-origin `Origin`. Config changes made over the API are written to
//! `config.toml` and take effect for the supervisor on its next start (`restart_required`).
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod api;
mod assets;
pub mod error;
mod extract;
pub mod page;
pub mod security;
pub mod state;
pub mod stream;
pub mod tsgen;
pub mod types;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::middleware;
use so_core::proto::{METHOD_UI_START, RpcError, UiStartParams, UiStartResult};
use so_supervisor::SupervisorState;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

pub use state::{AppState, WebOptions};

#[derive(Debug, thiserror::Error)]
pub enum WebError {
    #[error("binding the UI to {0} requires a token (--token or ui.token)")]
    TokenRequired(String),
    #[error("{0}")]
    BadAddress(String),
    #[error("could not listen on {addr}: {source}")]
    Bind {
        addr: String,
        source: std::io::Error,
    },
}

/// Builds the router for `state` with an already running [`stream::Hub`] pump.
pub fn router(state: Arc<SupervisorState>, opts: WebOptions) -> Router {
    let hub = stream::Hub::new();
    hub.spawn_pump(state.clone());
    build(AppState {
        sup: state,
        hub,
        opts: Arc::new(opts),
        limiter: Arc::default(),
    })
}

fn build(app: AppState) -> Router {
    api::routes()
        .fallback(assets::serve)
        .layer(middleware::from_fn_with_state(app.clone(), security::guard))
        .with_state(app)
}

fn validate(opts: &WebOptions) -> Result<(), WebError> {
    opts.bind_ip()
        .map_err(|e| WebError::BadAddress(format!("{e:?}")))?;
    if !opts.is_loopback() && opts.token.as_deref().is_none_or(str::is_empty) {
        return Err(WebError::TokenRequired(opts.bind.clone()));
    }
    Ok(())
}

/// A running web server.
pub struct WebServer {
    addr: SocketAddr,
    task: JoinHandle<()>,
    stop: tokio::sync::watch::Sender<bool>,
}

impl WebServer {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://host:port` as a browser should open it.
    pub fn url(&self) -> String {
        let host = if self.addr.ip().is_unspecified() {
            "localhost".to_string()
        } else if self.addr.is_ipv6() {
            format!("[{}]", self.addr.ip())
        } else {
            self.addr.ip().to_string()
        };
        format!("http://{host}:{}", self.addr.port())
    }

    pub async fn shutdown(self) {
        let _ = self.stop.send(true);
        let _ = self.task.await;
    }
}

/// Binds and serves in the background.
pub async fn start(
    state: Arc<SupervisorState>,
    mut opts: WebOptions,
) -> Result<WebServer, WebError> {
    validate(&opts)?;
    let ip = opts
        .bind_ip()
        .map_err(|e| WebError::BadAddress(format!("{e:?}")))?;
    let want = SocketAddr::new(ip, opts.port);
    let listener = TcpListener::bind(want)
        .await
        .map_err(|source| WebError::Bind {
            addr: want.to_string(),
            source,
        })?;
    let addr = listener.local_addr().map_err(|source| WebError::Bind {
        addr: want.to_string(),
        source,
    })?;
    opts.port = addr.port();
    let app = router(state, opts);
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let served = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async move {
            let _ = stopped.wait_for(|v| *v).await;
        })
        .await;
        if let Err(e) = served {
            tracing::warn!("web server stopped: {e}");
        }
    });
    tracing::info!(%addr, "web UI listening");
    Ok(WebServer { addr, task, stop })
}

/// Serves until `shutdown` resolves.
pub async fn serve(
    state: Arc<SupervisorState>,
    opts: WebOptions,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), WebError> {
    let server = start(state, opts).await?;
    shutdown.await;
    server.shutdown().await;
    Ok(())
}

/// Hooks the web UI into a supervisor: registers the `ui_start` RPC (idempotent: a second
/// call reports the running URL) and starts the server right away when `[ui] enabled`.
/// Call it from the daemon's setup, after `default_handlers`.
pub fn install(state: &Arc<SupervisorState>) {
    type Slot = Arc<tokio::sync::Mutex<Option<WebServer>>>;
    let slot: Slot = Arc::default();
    let st = state.clone();
    let s = slot.clone();
    state.router.register(METHOD_UI_START, move |_ctx, params| {
        let (st, slot) = (st.clone(), s.clone());
        async move {
            let p: UiStartParams = serde_json::from_value(params)
                .map_err(|e| RpcError::new(so_core::proto::INVALID_PARAMS, e.to_string()))?;
            let mut guard = slot.lock().await;
            if let Some(running) = guard.as_ref() {
                return serde_json::to_value(UiStartResult {
                    url: running.url(),
                    already_running: true,
                })
                .map_err(|e| RpcError::new(so_core::proto::INTERNAL_ERROR, e.to_string()));
            }
            let mut opts = WebOptions::from_config(&st.config.ui);
            if let Some(b) = p.bind {
                opts.bind = b;
            }
            if let Some(port) = p.port {
                opts.port = port;
            }
            if let Some(t) = p.token.filter(|t| !t.is_empty()) {
                opts.token = Some(t);
            }
            let server = start(st.clone(), opts).await.map_err(|e| {
                let code = match e {
                    WebError::Bind { .. } => so_core::proto::UNAVAILABLE,
                    _ => so_core::proto::INVALID_PARAMS,
                };
                RpcError::new(code, e.to_string())
            })?;
            let result = UiStartResult {
                url: server.url(),
                already_running: false,
            };
            *guard = Some(server);
            serde_json::to_value(result)
                .map_err(|e| RpcError::new(so_core::proto::INTERNAL_ERROR, e.to_string()))
        }
    });
    if state.config.ui.enabled {
        let (st, slot) = (state.clone(), slot);
        tokio::spawn(async move {
            match start(st.clone(), WebOptions::from_config(&st.config.ui)).await {
                Ok(server) => *slot.lock().await = Some(server),
                Err(e) => tracing::error!("web UI not started: {e}"),
            }
        });
    }
}
