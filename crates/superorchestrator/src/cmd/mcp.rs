//! `mcp`: the stdio MCP server, a thin client of the supervisor.

use anyhow::{Context, Result};
use so_core::Paths;
use tracing_subscriber::EnvFilter;

use crate::ctx::Ctx;
use crate::redact::MakeRedacting;

/// Logs to stderr only: stdout is the MCP channel.
fn init_tracing() {
    let filter = std::env::var("SUPERORCHESTRATOR_LOG")
        .ok()
        .and_then(|v| EnvFilter::try_new(v).ok())
        .unwrap_or_else(|| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_writer(MakeRedacting(std::io::stderr))
        .try_init();
}

pub fn run(ctx: &Ctx, client: &str) -> Result<()> {
    init_tracing();
    let opts = so_mcp::McpOptions {
        socket_path: Paths::socket_path()?,
        config_dir: ctx.paths.config_dir.clone(),
        run_id: std::env::var("SUPERORCHESTRATOR_RUN_ID")
            .ok()
            .filter(|v| !v.is_empty()),
        client_name: client.to_string(),
        cwd: std::env::current_dir().context("could not read the current directory")?,
        spawn_exe: std::env::current_exe().context("could not find this executable")?,
    };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(so_mcp::serve_stdio(opts))
}
