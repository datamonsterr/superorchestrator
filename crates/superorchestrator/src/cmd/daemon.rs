//! `daemon [--foreground]`.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

use crate::ctx::Ctx;
use crate::redact::MakeRedacting;

/// Appends through a shared file handle.
struct SharedFile(Arc<File>);

impl Write for SharedFile {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        (&*self.0).write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

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
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("could not create {}", dir.display()))?;
        let path = dir.join("supervisor.log");
        let file = Arc::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .mode(0o600)
                .open(&path)
                .with_context(|| format!("could not open {}", path.display()))?,
        );
        builder
            .with_writer(MakeRedacting(move || SharedFile(file.clone())))
            .try_init()
            .map_err(|e| anyhow::anyhow!("could not set up logging: {e}"))?;
    }
    Ok(())
}

pub fn run(ctx: &Ctx, foreground: bool) -> Result<()> {
    let config = ctx.config()?;
    init_tracing(ctx, foreground)?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(so_supervisor::run_daemon(
        ctx.paths.clone(),
        config,
        foreground,
    ))?;
    Ok(())
}
