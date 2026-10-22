//! `daemon [--foreground]`.

use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::ctx::Ctx;
use crate::logfile::{KEEP, MAX_BYTES, RotatingLog};
use crate::redact::MakeRedacting;

fn filter() -> EnvFilter {
    std::env::var("SUPERORCHESTRATOR_LOG")
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new("info"))
}

fn init_tracing(ctx: &Ctx, foreground: bool) -> Result<()> {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter())
        .with_ansi(false);
    if foreground {
        builder
            .with_writer(MakeRedacting(io::stderr))
            .try_init()
            .map_err(|e| anyhow::anyhow!("could not set up logging: {e}"))?;
    } else {
        let dir = ctx.paths.logs_dir();
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        // `create` leaves an existing directory as it is: tighten it.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("could not restrict {}", dir.display()))?;
        let path = dir.join("supervisor.log");
        let log = RotatingLog::open(&path, MAX_BYTES, KEEP)
            .with_context(|| format!("could not open {}", path.display()))?;
        builder
            .with_writer(MakeRedacting(move || log.clone()))
            .try_init()
            .map_err(|e| anyhow::anyhow!("could not set up logging: {e}"))?;
    }
    Ok(())
}

pub fn run(ctx: &Ctx, foreground: bool) -> Result<()> {
    // Logging first, so a config error also lands in the log.
    init_tracing(ctx, foreground)?;
    let config = ctx.config().inspect_err(|e| tracing::error!("{e:#}"))?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // The web UI lives in the supervisor process: `install` adds the `ui_start` method and
    // starts the server at once when `[ui] enabled = true`.
    rt.block_on(so_supervisor::run_daemon_with(
        ctx.paths.clone(),
        config,
        foreground,
        |state| {
            so_supervisor::default_handlers(state);
            so_web::install(state);
        },
    ))?;
    Ok(())
}
