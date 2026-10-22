//! `worktrees list|prune`: reads the store and runs git directly.

use std::process::ExitCode;

use anyhow::{Result, bail};
use dialoguer::Confirm;
use serde_json::{Value, json};
use so_store::RunRow;
use so_supervisor::services::worktrees::{exists, prune_one, prune_targets};

use super::runs::read_runs;
use crate::cli::WorktreesCmd;
use crate::ctx::{Ctx, interactive};
use crate::output::write_table;
use crate::views::open_existing_store;

pub fn run(ctx: &Ctx, cmd: WorktreesCmd) -> Result<ExitCode> {
    match cmd {
        WorktreesCmd::List => list(ctx),
        WorktreesCmd::Prune => prune(ctx),
    }
    .map(|()| ExitCode::SUCCESS)
}

fn with_worktree(ctx: &Ctx) -> Result<Vec<RunRow>> {
    Ok(read_runs(ctx, true)?
        .into_iter()
        .filter(|r| r.worktree_path.is_some())
        .collect())
}

fn list(ctx: &Ctx) -> Result<()> {
    let rows = with_worktree(ctx)?;
    let value = Value::Array(
        rows.iter()
            .map(|r| {
                json!({
                    "run": r.id, "state": r.state, "branch": r.branch,
                    "path": r.worktree_path, "exists": exists(r),
                })
            })
            .collect(),
    );
    ctx.emit(&value, |w| {
        let table: Vec<Vec<String>> = rows
            .iter()
            .map(|r| {
                vec![
                    r.id.clone(),
                    r.state.to_string(),
                    r.branch.clone().unwrap_or_default(),
                    r.worktree_path.clone().unwrap_or_default(),
                    if exists(r) { "yes" } else { "removed" }.to_string(),
                ]
            })
            .collect();
        write_table(w, &["RUN", "STATE", "BRANCH", "PATH", "EXISTS"], &table)
    })
}

fn prune(ctx: &Ctx) -> Result<()> {
    let Some(store) = open_existing_store(&ctx.paths) else {
        return ctx.emit_ok(json!({"removed": []}), "nothing to prune");
    };
    let targets = prune_targets(with_worktree(ctx)?);
    if targets.is_empty() {
        return ctx.emit_ok(json!({"removed": []}), "nothing to prune");
    }
    if !ctx.yes {
        if !interactive() {
            bail!("refusing to prune without confirmation; pass --yes");
        }
        for (_, c) in &targets {
            println!("{}  ({})", c.path.display(), c.branch);
        }
        let ok = Confirm::new()
            .with_prompt(format!(
                "remove {} worktree(s) and their branches, including unmerged work?",
                targets.len()
            ))
            .default(false)
            .interact()?;
        if !ok {
            return Ok(());
        }
    }
    let mut removed = Vec::new();
    for (run, c) in &targets {
        match prune_one(&store, run, c) {
            Ok(()) => removed.push(run.id.clone()),
            Err(e) => eprintln!("could not remove {}: {e}", c.path.display()),
        }
    }
    ctx.emit_ok(
        json!({"removed": removed}),
        &format!("removed {} worktree(s)", removed.len()),
    )
}
