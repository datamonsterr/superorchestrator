//! `exec <account> -- <args>`: record an interactive session, then exec the harness.

use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use chrono::Utc;
use so_accounts::build_launch;
use so_core::{AccountFile, SessionKind, new_id};
use so_store::{SessionRow, Store};

use crate::ctx::{Ctx, debug_enabled};
use crate::exit::CliError;

/// Never returns on success: the process image is replaced.
pub fn run(ctx: &Ctx, account: &str, args: &[String]) -> Result<std::convert::Infallible> {
    let file = ctx.paths.account_file(account);
    if !file.exists() {
        return Err(CliError::not_found(format!("account `{account}` not found")).into());
    }
    let acc = AccountFile::load(&file).with_context(|| format!("account `{account}`"))?;
    let host = ctx.host()?;
    let launch = build_launch(&acc, &host)
        .with_context(|| format!("cannot launch account `{account}`"))?
        .with_default_pass_env(&ctx.config()?.accounts.pass_env);

    record_session(&ctx.paths.state_db(), account);

    let mut command = Command::new(&launch.program);
    launch.isolate_env(&mut command);
    let err = command
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
        let id = new_id();
        store.insert_session(&SessionRow {
            id: id.clone(),
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
        // `exec` keeps the pid and the start time, so the supervisor can tell this process
        // from an unrelated one that later reuses the pid.
        if let Some(start) = so_core::procinfo::process_start(i64::from(std::process::id())) {
            store.set_session_pid_start(&id, &start)?;
        }
        Ok(())
    })();
    if let Err(e) = result
        && debug_enabled()
    {
        eprintln!("superorchestrator: could not record session: {e}");
    }
}
