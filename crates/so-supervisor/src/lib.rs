//! Run lifecycle, caps, stall watcher, notifications and socket API.
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod client;
pub mod dispatch;
pub mod error;
mod lines;
pub mod lock;
pub mod reconcile;
pub mod server;

use so_core::{Config, Paths};

pub use client::{ClientError, SupervisorClient, spawn_daemon_process};
pub use dispatch::{ConnCtx, HandlerFuture, Notifier, Router};
pub use error::{Result, SupervisorError};
pub use lock::SupervisorLock;
pub use reconcile::ReconcileHook;
pub use server::{
    ActivityProbe, MAX_LINE_BYTES, Supervisor, SupervisorHandle, SupervisorOptions, SupervisorState,
};

/// Runs the supervisor until it is stopped by a signal or by idle exit.
///
/// `foreground` only affects logging here: detaching is the spawner's job
/// ([`spawn_daemon_process`]). Log file setup is left to the CLI.
pub async fn run_daemon(paths: Paths, config: Config, foreground: bool) -> Result<()> {
    let socket_path = Paths::socket_path()?;
    tracing::info!(foreground, socket = %socket_path.display(), "starting supervisor");
    let opts = SupervisorOptions::new(paths, config, socket_path).with_signals();
    Supervisor::start(opts).await?.wait().await;
    Ok(())
}
