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
    let launch =
        build_launch(&acc, &host).with_context(|| format!("cannot launch account `{account}`"))?;

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
