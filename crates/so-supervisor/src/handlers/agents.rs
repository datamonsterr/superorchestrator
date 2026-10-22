//! `list_agents`: the custom agents `spawn_agent` can run (SPEC 10, 13).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use so_agents::discover;
use so_core::Paths;
use so_core::proto::{
    AgentView, AgentsView, EmptyParams, INVALID_PARAMS, METHOD_LIST_AGENTS, RpcError,
};

use crate::dispatch::internal;
use crate::server::SupervisorState;

/// The agents found from `cwd` and the config dir, in search order. A broken agent file is
/// logged and left out; a symlinked agent is not offered either, because the files it points
/// at are not under the agents directories.
pub fn agents_view(cwd: &Path, paths: &Paths) -> AgentsView {
    let d = discover(cwd, paths);
    for (path, msg) in &d.problems {
        tracing::warn!(path = %path.display(), "skipped agent: {}", msg.lines().next().unwrap_or_default());
    }
    let link = |p: &Path| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_symlink());
    AgentsView {
        agents: d
            .agents
            .iter()
            .filter(|a| !(link(&a.file) || a.dir.as_deref().is_some_and(link)))
            .map(|a| AgentView {
                name: a.name.clone(),
                description: a.description.clone(),
                source: Some(a.source.as_str().to_string()),
                path: Some(a.file.display().to_string()),
                harness: Some(a.harness.as_str().to_string()),
            })
            .collect(),
    }
}

pub(super) fn register_agents(state: &Arc<SupervisorState>) {
    let state = state.clone();
    state
        .clone()
        .router
        .register(METHOD_LIST_AGENTS, move |ctx, params| {
            let state = state.clone();
            async move {
                serde_json::from_value::<EmptyParams>(params)
                    .map_err(|e| RpcError::new(INVALID_PARAMS, format!("invalid params: {e}")))?;
                // Project agents resolve from the caller's directory.
                let session = match ctx.session_id.clone() {
                    Some(id) => state.db(move |s| s.get_session(&id)).await?,
                    None => None,
                };
                let cwd = session
                    .and_then(|s| s.cwd)
                    .map(PathBuf::from)
                    .or_else(|| std::env::current_dir().ok())
                    .unwrap_or_else(|| PathBuf::from("/"));
                let paths = state.paths.clone();
                let view = tokio::task::spawn_blocking(move || agents_view(&cwd, &paths))
                    .await
                    .map_err(internal)?;
                serde_json::to_value(view).map_err(internal)
            }
        });
}
