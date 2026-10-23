//! `status`: supervisor state (never spawned) plus the accounts summary.

use anyhow::Result;
use serde_json::json;
use so_core::Paths;
use so_core::buildinfo::{build_id, mismatch};
use so_core::proto::{METHOD_STATUS, StatusResult};
use so_supervisor::SupervisorClient;

use crate::ctx::Ctx;
use crate::views::{collect_views, open_existing_store, views_json, write_views_table};

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
            .hello_inspect("cli", &cwd, Some(std::process::id()))
            .await
            .ok()?;
        client
            .call::<_, StatusResult>(METHOD_STATUS, json!({}))
            .await
            .ok()
    })
}

/// The warning `status` prints under the supervisor line, `None` when daemon and CLI agree.
pub fn daemon_warning(sup: &StatusResult) -> Option<String> {
    mismatch(
        build_id(),
        sup.build_id.as_deref(),
        sup.stale,
        Some(sup.pid),
    )
}

/// The `supervisor:` line: the daemon's version and build id next to this binary's.
pub fn supervisor_line(s: &StatusResult) -> String {
    format!(
        "supervisor: running (pid {}, v{} build {}, up {}s, {} session(s), {} active run(s)); this CLI: v{} build {}",
        s.pid,
        s.version,
        s.build_id.as_deref().unwrap_or("unknown"),
        s.uptime_s,
        s.sessions,
        s.active_runs,
        env!("CARGO_PKG_VERSION"),
        build_id()
    )
}

pub fn run(ctx: &Ctx) -> Result<()> {
    if let Some(store) = open_existing_store(&ctx.paths) {
        let _ = store.reap_dead_interactive_sessions();
    }
    let sup = supervisor_status();
    let views = collect_views(ctx)?;
    let sup_json = sup.as_ref().map(|s| {
        json!({
            "pid": s.pid, "version": s.version, "uptime_s": s.uptime_s,
            "sessions": s.sessions, "active_runs": s.active_runs,
            "build_id": s.build_id, "stale": s.stale, "stale_reason": s.stale_reason,
            "cli_version": env!("CARGO_PKG_VERSION"), "cli_build_id": build_id(),
            "warning": daemon_warning(s),
        })
    });
    let value = json!({ "supervisor": sup_json, "accounts": views_json(&views) });
    ctx.emit(&value, |w| {
        match &sup {
            Some(s) => {
                writeln!(w, "{}", supervisor_line(s))?;
                if let Some(msg) = daemon_warning(s) {
                    writeln!(w, "warning: {msg}")?;
                }
            }
            None => writeln!(w, "supervisor: not running")?,
        }
        writeln!(w)?;
        write_views_table(w, &views)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(build: Option<&str>, stale: bool) -> StatusResult {
        StatusResult {
            pid: 42,
            version: "0.1.0".into(),
            uptime_s: 5,
            sessions: 1,
            active_runs: 0,
            build_id: build.map(String::from),
            stale,
            stale_reason: None,
        }
    }

    #[test]
    fn matching_daemon_has_no_warning() {
        assert_eq!(daemon_warning(&status(Some(build_id()), false)), None);
        let line = supervisor_line(&status(Some(build_id()), false));
        assert!(line.contains(&format!("build {}", build_id())));
    }

    #[test]
    fn different_build_warns_with_the_fix() {
        let s = status(Some("old-1"), false);
        let w = daemon_warning(&s).unwrap();
        assert!(w.contains("old-1") && w.contains(build_id()), "{w}");
        assert!(
            w.contains("restart the daemon") && w.contains("kill 42"),
            "{w}"
        );
        assert!(supervisor_line(&s).contains("build old-1"));
    }

    #[test]
    fn stale_daemon_warns_even_with_equal_builds() {
        let w = daemon_warning(&status(Some(build_id()), true)).unwrap();
        assert!(w.contains("replaced on disk") && w.contains("restart the daemon"));
    }

    #[test]
    fn daemon_without_build_id_warns() {
        let s = status(None, false);
        assert!(daemon_warning(&s).unwrap().contains("predates"));
        assert!(supervisor_line(&s).contains("build unknown"));
    }
}
