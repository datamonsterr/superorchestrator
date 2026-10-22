//! `worktrees list|prune`: reads the store and runs git directly.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Result, bail};
use chrono::Utc;
use dialoguer::Confirm;
use serde_json::{Value, json};
use so_core::{Event, EventType, new_id};
use so_store::RunRow;
use so_supervisor::runs::worktree::{Created, remove};

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
    let exists = |r: &RunRow| {
        r.worktree_path
            .as_deref()
            .is_some_and(|p| Path::new(p).is_dir())
    };
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

/// The worktree of a run, from the run's row.
fn created_of(r: &RunRow) -> Option<Created> {
    Some(Created {
        path: PathBuf::from(r.worktree_path.clone()?),
        branch: r.branch.clone()?,
        base: r.worktree_base.clone()?,
        repo: PathBuf::from(r.worktree_repo.clone()?),
    })
}

fn prune(ctx: &Ctx) -> Result<()> {
    let Some(store) = open_existing_store(&ctx.paths) else {
        return ctx.emit_ok(json!({"removed": []}), "nothing to prune");
    };
    let targets: Vec<(RunRow, Created)> = with_worktree(ctx)?
        .into_iter()
        .filter(|r| r.state.is_terminal())
        .filter_map(|r| created_of(&r).map(|c| (r, c)))
        .filter(|(_, c)| c.path.is_dir())
        .collect();
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
        match remove(c, true) {
            Ok(()) => {
                store.append_event(&Event {
                    id: new_id(),
                    session_id: run.session_id.clone(),
                    run_id: Some(run.id.clone()),
                    ts: Utc::now(),
                    event_type: EventType::WorktreeRemoved,
                    data: json!({"path": c.path, "branch": c.branch, "pruned": true}),
                })?;
                // Nothing left to resume in: drop the run's checks and resume intent too.
                let _ = store.delete_run_meta(&run.id);
                removed.push(run.id.clone());
            }
            Err(e) => eprintln!("could not remove {}: {e}", c.path.display()),
        }
    }
    ctx.emit_ok(
        json!({"removed": removed}),
        &format!("removed {} worktree(s)", removed.len()),
    )
}
