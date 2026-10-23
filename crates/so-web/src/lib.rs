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
//! Loopback by default. Every bind requires a token; startup generates and persists one
//! when no explicit token is configured.
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
mod token;
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
    #[error("could not generate a UI login code: {0}")]
    LoginCode(#[source] std::io::Error),
    #[error(
        "could not load the UI token file {}: {source}; remove {} to regenerate it",
        path.display(), path.display()
    )]
    TokenFile {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{0}")]
    BadAddress(String),
    #[error("could not listen on {addr}: {source}")]
    Bind {
        addr: String,
        source: std::io::Error,
    },
}

fn make_app(state: Arc<SupervisorState>, opts: WebOptions) -> AppState {
    let hub = stream::Hub::new();
    hub.spawn_pump(state.clone());
    AppState {
        sup: state,
        hub,
        opts: Arc::new(opts),
        limiter: Arc::default(),
        login_codes: Arc::default(),
        stop: Arc::new(tokio::sync::watch::channel(false).0),
        refresh_lock: Arc::default(),
    }
}

/// Builds the router for `state` with a running [`stream::Hub`] pump.
pub fn router(state: Arc<SupervisorState>, mut opts: WebOptions) -> Result<Router, WebError> {
    prepare(&state, &mut opts)?;
    Ok(build(make_app(state, opts)))
}

fn prepare(state: &SupervisorState, opts: &mut WebOptions) -> Result<(), WebError> {
    if opts.token.as_deref().is_none_or(str::is_empty) {
        let path = token::token_path(&state.paths.config_dir);
        opts.token = Some(
            token::load_or_create(&state.paths.config_dir)
                .map_err(|source| WebError::TokenFile { path, source })?,
        );
    }
    validate(opts)
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
    if opts.token.as_deref().is_none_or(str::is_empty) {
        return Err(WebError::TokenRequired(opts.bind.clone()));
    }
    Ok(())
}

/// A running web server.
pub struct WebServer {
    addr: SocketAddr,
    login_codes: Arc<security::LoginCodes>,
    task: Option<JoinHandle<()>>,
    stop: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Drop for WebServer {
    /// Dropping the handle stops the server (open streams are cut).
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl WebServer {
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Base URL without any credentials.
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

    /// Mint a fresh single-use, 60-second browser login URL. Only hand it out over
    /// the private supervisor socket; the long-lived token never enters the URL.
    pub fn login_url(&self) -> Result<String, WebError> {
        let code = self.login_codes.mint().map_err(WebError::LoginCode)?;
        Ok(format!("{}/token#token={code}", self.url()))
    }

    /// Stops accepting, ends open streams and waits for the server task.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(t) = self.task.take() {
            let _ = t.await;
        }
    }
}

/// Binds and serves in the background.
pub async fn start(
    state: Arc<SupervisorState>,
    mut opts: WebOptions,
) -> Result<WebServer, WebError> {
    let st = state.clone();
    let token_path = token::token_path(&state.paths.config_dir);
    opts = tokio::task::spawn_blocking(move || {
        prepare(&st, &mut opts)?;
        Ok::<_, WebError>(opts)
    })
    .await
    .map_err(|e| WebError::TokenFile {
        path: token_path,
        source: std::io::Error::other(e),
    })??;
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
    let app = make_app(state, opts);
    let stop = app.stop.clone();
    let login_codes = app.login_codes.clone();
    let mut stopped = stop.subscribe();
    let app = build(app);
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
    Ok(WebServer {
        addr,
        login_codes,
        task: Some(task),
        stop,
    })
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

/// Hooks the web UI into a supervisor: registers `ui_start` (reuses the running UI,
/// minting a fresh single-use login URL for each call) and starts the server right away when `[ui] enabled`.
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
                    url: running.login_url().map_err(|e| {
                        RpcError::new(so_core::proto::INTERNAL_ERROR, e.to_string())
                    })?,
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
                url: server
                    .login_url()
                    .map_err(|e| RpcError::new(so_core::proto::INTERNAL_ERROR, e.to_string()))?,
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
            // Holding the slot across `start` keeps a concurrent `ui_start` from binding too.
            let mut guard = slot.lock().await;
            if guard.is_some() {
                return;
            }
            match start(st.clone(), WebOptions::from_config(&st.config.ui)).await {
                Ok(server) => *guard = Some(server),
                Err(e) => tracing::error!("web UI not started: {e}"),
            }
        });
    }
}
