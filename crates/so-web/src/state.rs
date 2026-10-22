//! Shared state of the web server and the options it starts with.

use std::net::IpAddr;
use std::sync::Arc;

use so_supervisor::SupervisorState;

use crate::error::{ApiError, ApiResult};
use crate::security::RateLimiter;
use crate::stream::Hub;

/// How the web server listens and who may talk to it.
#[derive(Debug, Clone)]
pub struct WebOptions {
    /// Address to bind: an IP literal or `localhost`.
    pub bind: String,
    /// Port; `0` picks a free one (the real port is in [`crate::WebServer::addr`]).
    pub port: u16,
    /// Shared secret. Required for any non-loopback bind; when set on a loopback bind it is
    /// enforced there too.
    pub token: Option<String>,
    /// Mark the session cookie `Secure` when a trusted proxy says the request was HTTPS
    /// (`X-Forwarded-Proto: https`). Off by default: the header is client-controlled.
    pub trust_forwarded_proto: bool,
    /// Where project-scoped agents (`.superorchestrator/agents`, `.claude/agents`) and
    /// workflows are looked up. Default: the supervisor's working directory.
    pub project_dir: Option<std::path::PathBuf>,
}

impl Default for WebOptions {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 7878,
            token: None,
            trust_forwarded_proto: false,
            project_dir: None,
        }
    }
}

impl WebOptions {
    /// Options from `[ui]` in `config.toml`.
    pub fn from_config(ui: &so_core::config::Ui) -> Self {
        Self {
            bind: ui.bind.clone(),
            port: ui.port,
            token: ui.token.clone().filter(|t| !t.is_empty()),
            trust_forwarded_proto: false,
            project_dir: None,
        }
    }

    /// The bind address as an IP. `localhost` means `127.0.0.1`.
    pub fn bind_ip(&self) -> ApiResult<IpAddr> {
        if self.bind.eq_ignore_ascii_case("localhost") {
            return Ok(IpAddr::from([127, 0, 0, 1]));
        }
        self.bind
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| ApiError::invalid(format!("ui.bind `{}` is not an IP address", self.bind)))
    }

    pub fn is_loopback(&self) -> bool {
        self.bind_ip().is_ok_and(|ip| ip.is_loopback())
    }

    /// Whether a bind to `0.0.0.0` or `::`.
    pub fn is_wildcard(&self) -> bool {
        self.bind_ip().is_ok_and(|ip| ip.is_unspecified())
    }
}

#[derive(Clone)]
pub struct AppState {
    pub sup: Arc<SupervisorState>,
    pub hub: Arc<Hub>,
    pub opts: Arc<WebOptions>,
    pub limiter: Arc<RateLimiter>,
    /// Set to `true` to stop the server; open SSE streams end when it is.
    pub stop: Arc<tokio::sync::watch::Sender<bool>>,
    /// One model refresh at a time.
    pub refresh_lock: Arc<tokio::sync::Mutex<()>>,
}

impl AppState {
    /// The directory project-scoped agents and workflows resolve from.
    pub fn project_cwd(&self) -> std::path::PathBuf {
        self.opts
            .project_dir
            .clone()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("/"))
    }

    /// Regenerates the installed skill after accounts, choices or agents changed (SPEC 19.1).
    /// Blocking file I/O: call it from inside [`Self::blocking`]. A no-op when the skill is
    /// not installed; a failure is logged because the change itself already succeeded.
    pub fn refresh_skill(&self) {
        let Some(host) = &self.sup.host else { return };
        for e in so_agents::skill::refresh(&self.sup.paths, &host.home).errors {
            tracing::warn!("{e}");
        }
    }

    /// Runs a blocking closure (store, filesystem, git) off the async runtime.
    pub async fn blocking<T, F>(&self, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce() -> ApiResult<T> + Send + 'static,
    {
        tokio::task::spawn_blocking(f)
            .await
            .map_err(ApiError::internal)?
    }

    /// Runs a blocking store call.
    pub async fn db<T, F>(&self, f: F) -> ApiResult<T>
    where
        T: Send + 'static,
        F: FnOnce(&so_store::Store) -> so_store::Result<T> + Send + 'static,
    {
        let store = self.sup.store.clone();
        self.blocking(move || f(&store).map_err(ApiError::from))
            .await
    }
}
