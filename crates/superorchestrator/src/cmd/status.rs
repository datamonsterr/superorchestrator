//! `status`: supervisor state (never spawned) plus the accounts summary.

use anyhow::Result;
use so_core::Paths;
use so_core::proto::{METHOD_STATUS, StatusResult};
use so_supervisor::SupervisorClient;

use crate::cmd::account::{collect_views, open_existing_store, print_views_table, views_json};
use crate::cmd::exec::end_dead_interactive_sessions;
use crate::ctx::Ctx;

/// Asks a running supervisor for its status; `None` when none answers.
pub fn supervisor_status() -> Option<StatusResult> {
    let socket = Paths::socket_path().ok()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let client = SupervisorClient::connect(&socket).await.ok()?;
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        client
            .hello("cli", &cwd, None, Some(std::process::id()))
            .await
            .ok()?;
        client
            .call::<_, StatusResult>(METHOD_STATUS, serde_json::json!({}))
            .await
            .ok()
    })
}

pub fn run(ctx: &Ctx) -> Result<()> {
    if let Some(store) = open_existing_store(&ctx.paths) {
        end_dead_interactive_sessions(&store);
    }
    let sup = supervisor_status();
    let views = collect_views(ctx)?;
    if ctx.json {
        let sup_json = sup.as_ref().map(|s| {
            serde_json::json!({
                "pid": s.pid, "version": s.version, "uptime_s": s.uptime_s,
                "sessions": s.sessions, "active_runs": s.active_runs,
            })
        });
        let out = serde_json::json!({ "supervisor": sup_json, "accounts": views_json(&views) });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    match sup {
        Some(s) => println!(
            "supervisor: running (pid {}, v{}, up {}s, {} session(s), {} active run(s))",
            s.pid, s.version, s.uptime_s, s.sessions, s.active_runs
        ),
        None => println!("supervisor: not running"),
    }
    println!();
    print_views_table(&views);
    Ok(())
}
