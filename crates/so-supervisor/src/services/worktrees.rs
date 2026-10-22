//! Run worktrees (`worktrees list|prune`, `GET /worktrees`, `POST /worktrees/prune`).

use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::json;
use so_core::{Event, EventType, new_id};
use so_store::{RunRow, SessionFilter, Store};

use crate::runs::worktree::{Created, remove};

const LIMIT: usize = 100_000;

/// Every run that has (or had) a worktree, oldest first.
pub fn runs_with_worktree(store: &Store) -> so_store::Result<Vec<RunRow>> {
    let mut out: Vec<RunRow> = Vec::new();
    for s in store.list_sessions(SessionFilter::All, LIMIT)? {
        out.extend(
            store
                .list_runs_by_session(&s.id, LIMIT)?
                .into_iter()
                .filter(|r| r.worktree_path.is_some()),
        );
    }
    out.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    Ok(out)
}

/// Whether the worktree directory of `r` still exists.
pub fn exists(r: &RunRow) -> bool {
    r.worktree_path
        .as_deref()
        .is_some_and(|p| Path::new(p).is_dir())
}

/// The worktree of a run, from the run's row.
pub fn created_of(r: &RunRow) -> Option<Created> {
    Some(Created {
        path: PathBuf::from(r.worktree_path.clone()?),
        branch: r.branch.clone()?,
        base: r.worktree_base.clone()?,
        repo: PathBuf::from(r.worktree_repo.clone()?),
    })
}

/// Worktrees of finished runs that are still on disk.
pub fn prune_targets(rows: Vec<RunRow>) -> Vec<(RunRow, Created)> {
    rows.into_iter()
        .filter(|r| r.state.is_terminal())
        .filter_map(|r| created_of(&r).map(|c| (r, c)))
        .filter(|(_, c)| c.path.is_dir())
        .collect()
}

/// Removes one worktree and its branch (even unmerged) and records `worktree_removed`.
pub fn prune_one(store: &Store, run: &RunRow, c: &Created) -> Result<(), String> {
    remove(c, true)?;
    store
        .append_event(&Event {
            id: new_id(),
            session_id: run.session_id.clone(),
            run_id: Some(run.id.clone()),
            ts: Utc::now(),
            event_type: EventType::WorktreeRemoved,
            data: json!({"path": c.path, "branch": c.branch, "pruned": true}),
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}
