//! The rmcp server and tool definitions: a stdio MCP thin client that forwards every tool
//! call to the supervisor (SPEC 3.1, 13).
//!
//! Logging: this crate only uses `tracing`; the caller must install a subscriber that writes
//! to stderr, because stdout is the MCP channel.
//!
//! Tasks (SEP-2663): `spawn_agent` with `wait`, `wait_agent` and `run_workflow` return a task
//! when the client declares the tasks extension. The bridge keeps them in rmcp's
//! `TaskManager` and writes progress to the task's status message. Clients without tasks
//! block on the call and get MCP progress notifications instead.
//!
//! Elicitation: `wait_agent` surfaces a run's questions to the person through MCP
//! elicitation when the client supports it (`bridge.rs`).
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bridge;
pub mod compact;
mod tools;

use std::path::PathBuf;

use anyhow::Context;
use rmcp::ServiceExt;
use so_core::proto::HelloParams;
use so_supervisor::{SupervisorClient, spawn_daemon_process};

pub use bridge::{McpBridge, mcp_reply, rpc_args};
pub use tools::{ORCHESTRATOR_TOOLS, SUBAGENT_TOOLS, ToolSpec, tool_specs};

/// Options for [`serve_stdio`].
#[derive(Debug, Clone)]
pub struct McpOptions {
    pub socket_path: PathBuf,
    pub config_dir: PathBuf,
    /// `SUPERORCHESTRATOR_RUN_ID`: set when this process acts for a subagent run.
    pub run_id: Option<String>,
    /// Name reported to the supervisor in `hello`.
    pub client_name: String,
    pub cwd: PathBuf,
    /// Executable started as `<exe> daemon` when no supervisor is running.
    pub spawn_exe: PathBuf,
}

/// The warning for a daemon that runs another build than this process (or whose executable
/// was replaced), from the `hello` it answered.
pub fn daemon_notice(hello: &so_core::proto::HelloResult) -> Option<String> {
    so_core::buildinfo::mismatch(
        so_core::buildinfo::build_id(),
        hello.build_id.as_deref(),
        hello.stale,
        None,
    )
}

/// Connects to the supervisor (starting it if needed), then serves MCP over stdio until
/// stdin closes.
pub async fn serve_stdio(opts: McpOptions) -> anyhow::Result<()> {
    let cwd = opts.cwd.to_string_lossy().into_owned();
    let (client, hello) = SupervisorClient::connect_or_spawn_authenticated(
        &opts.socket_path,
        || spawn_daemon_process(&opts.spawn_exe, &opts.config_dir),
        HelloParams {
            client: opts.client_name.clone(),
            cwd,
            run_id: opts.run_id.clone(),
            run_token: opts
                .run_id
                .as_ref()
                .and_then(|_| std::env::var("SUPERORCHESTRATOR_RUN_TOKEN").ok()),
            pid: Some(std::process::id()),
            inspect: false,
        },
    )
    .await
    .context("connecting to the supervisor")?;
    let subagent = hello.run_id.is_some() || opts.run_id.is_some();
    let bridge = McpBridge::new(client, subagent);
    if let Some(notice) = daemon_notice(&hello) {
        tracing::warn!("{notice}");
        bridge.set_daemon_notice(notice);
    }
    let service = bridge
        .clone()
        .serve(rmcp::transport::stdio())
        .await
        .context("starting the MCP server")?;
    let waited = service.waiting().await.context("MCP server task failed");
    bridge.shutdown();
    waited?;
    Ok(())
}
