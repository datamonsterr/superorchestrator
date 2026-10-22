//! Run lifecycle, caps, stall watcher, notifications and socket API.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod client;
pub mod dispatch;
pub mod doctor;
pub mod error;
pub mod handlers;
mod lines;
pub mod lock;
pub mod notify;
pub mod reconcile;
pub mod runs;
pub mod server;
pub mod services;

use std::sync::Arc;

use so_core::{Config, Paths};

pub use client::{ClientError, SupervisorClient, spawn_daemon_process};
pub use dispatch::{ConnCtx, HandlerFuture, Notifier, Router, status_result};
pub use error::{Result, SupervisorError};
pub use handlers::{agents_view, default_handlers, harnesses_view};
pub use lock::SupervisorLock;
pub use reconcile::ReconcileHook;
pub use runs::RunManager;
pub use server::{
    ActivityGuard, ActivityProbe, MAX_LINE_BYTES, Supervisor, SupervisorHandle, SupervisorOptions,
    SupervisorState,
};

/// Runs the supervisor until it is stopped by a signal or by idle exit.
///
/// `foreground` only affects logging here: detaching is the spawner's job
/// ([`spawn_daemon_process`]). Log file setup is left to the CLI.
pub async fn run_daemon(paths: Paths, config: Config, foreground: bool) -> Result<()> {
    run_daemon_with(paths, config, foreground, default_handlers).await
}

/// Like [`run_daemon`], with `setup` run after the state exists. `setup` must register
/// [`default_handlers`] itself (call it first); the binary uses this to add the web UI.
pub async fn run_daemon_with(
    paths: Paths,
    config: Config,
    foreground: bool,
    setup: impl FnOnce(&Arc<SupervisorState>) + Send + 'static,
) -> Result<()> {
    let socket_path = Paths::socket_path()?;
    tracing::info!(foreground, socket = %socket_path.display(), "starting supervisor");
    let opts = SupervisorOptions::new(paths, config, socket_path)
        .with_signals()
        .with_setup(move |state| {
            setup(state);
            state.runs.enable_notifications();
        });
    Supervisor::start(opts).await?.wait().await;
    Ok(())
}
