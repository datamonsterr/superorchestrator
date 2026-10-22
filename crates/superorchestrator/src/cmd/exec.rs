//! `exec <account> -- <args>`: record an interactive session, then exec the harness.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use chrono::Utc;
use so_accounts::{LaunchMode, build_launch, resolve_binary};
use so_core::{AccountFile, Harness, SessionKind, new_id};
use so_store::{SessionFilter, SessionRow, Store};

use crate::ctx::{Ctx, debug_enabled};

/// Never returns on success: the process image is replaced.
pub fn run(ctx: &Ctx, account: &str, args: &[String]) -> Result<std::convert::Infallible> {
    let file = ctx.paths.account_file(account);
    let acc = AccountFile::load(&file).with_context(|| format!("account `{account}`"))?;
    let host = ctx.host()?;
    let bwrap = (acc.harness == Harness::Agy)
        .then(|| resolve_binary("bwrap", host.path_var.as_deref()).ok())
        .flatten();
    let launch = build_launch(&acc, &host, bwrap.as_deref(), LaunchMode::Interactive)?;

    record_session(&ctx.paths.state_db(), account);

    let err = Command::new(&launch.program)
        .args(&launch.argv_prefix)
        .args(&launch.extra_args)
        .args(args)
        .envs(launch.vars.iter().map(|(k, v)| (k, v)))
        .exec();
    Err(err).with_context(|| format!("could not exec {}", launch.program.display()))
}

/// Best effort: any store problem is ignored (logged only at debug level).
fn record_session(db: &Path, account: &str) {
    let result = (|| -> Result<()> {
        let store = Store::open(db)?;
        store.insert_session(&SessionRow {
            id: new_id(),
            kind: SessionKind::Interactive,
            client: "wrapper".into(),
            cwd: std::env::current_dir()
                .ok()
                .map(|p| p.to_string_lossy().into_owned()),
            account: Some(account.to_string()),
            pid: Some(i64::from(std::process::id())),
            started_at: Utc::now(),
            ended_at: None,
            status: "active".into(),
        })?;
        Ok(())
    })();
    if let Err(e) = result
        && debug_enabled()
    {
        eprintln!("superorchestrator: could not record session: {e}");
    }
}

fn pid_alive(pid: i64) -> bool {
    let Some(p) = u32::try_from(pid)
        .ok()
        .and_then(|p| rustix::process::Pid::from_raw(p as i32))
    else {
        return false;
    };
    match rustix::process::test_kill_process(p) {
        Ok(()) => true,
        Err(e) => e != rustix::io::Errno::SRCH,
    }
}

/// Ends active interactive sessions whose process is gone (exec replaced the
/// wrapper, so nothing else records the end). Returns how many were ended.
pub fn end_dead_interactive_sessions(store: &Store) -> usize {
    let Ok(active) = store.list_sessions(SessionFilter::Active, 10_000) else {
        return 0;
    };
    let now = Utc::now();
    active
        .iter()
        .filter(|s| s.kind == SessionKind::Interactive)
        .filter(|s| s.pid.is_some_and(|p| !pid_alive(p)))
        .filter(|s| store.end_session(&s.id, "ended", now).is_ok())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dead_interactive_sessions_are_ended_and_live_ones_kept() {
        let store = Store::open_in_memory().unwrap();
        let mk = |pid: i64, kind: SessionKind| SessionRow {
            id: new_id(),
            kind,
            client: "wrapper".into(),
            cwd: None,
            account: Some("a".into()),
            pid: Some(pid),
            started_at: Utc::now(),
            ended_at: None,
            status: "active".into(),
        };
        let live = mk(i64::from(std::process::id()), SessionKind::Interactive);
        let dead = mk(i64::from(i32::MAX), SessionKind::Interactive);
        let orch = mk(i64::from(i32::MAX), SessionKind::Orchestrator);
        for s in [&live, &dead, &orch] {
            store.insert_session(s).unwrap();
        }
        assert_eq!(end_dead_interactive_sessions(&store), 1);
        assert!(
            store
                .get_session(&dead.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_some()
        );
        assert!(
            store
                .get_session(&live.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
        assert!(
            store
                .get_session(&orch.id)
                .unwrap()
                .unwrap()
                .ended_at
                .is_none()
        );
    }
}
