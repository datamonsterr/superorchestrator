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
//! TODO(M4): elicitation (`ask_parent` / questions from the supervisor) is out of scope for now.
#![cfg_attr(test, allow(clippy::unwrap_used))]

mod bridge;
mod tools;

use std::path::PathBuf;

use anyhow::Context;
use rmcp::ServiceExt;
use so_supervisor::{SupervisorClient, spawn_daemon_process};

pub use bridge::McpBridge;
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

/// Connects to the supervisor (starting it if needed), then serves MCP over stdio until
/// stdin closes.
pub async fn serve_stdio(opts: McpOptions) -> anyhow::Result<()> {
    let cwd = opts.cwd.to_string_lossy().into_owned();
    let (client, hello) = SupervisorClient::connect_or_spawn_hello(
        &opts.socket_path,
        || spawn_daemon_process(&opts.spawn_exe, &opts.config_dir),
        &opts.client_name,
        &cwd,
        opts.run_id.clone(),
        Some(std::process::id()),
    )
    .await
    .context("connecting to the supervisor")?;
    let subagent = hello.run_id.is_some() || opts.run_id.is_some();
    let bridge = McpBridge::new(client, subagent);
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
