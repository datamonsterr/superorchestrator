//! Custom agents: the same files and parser as `agent create|list|show|edit|remove`.

use std::path::{Path as FsPath, PathBuf};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use so_agents::{
    AgentDef, AgentError, AgentSource, HarnessPref, discover, gaps, parse_agent, remove_files,
    resolve_for_spawn, validate_agent_name,
};
use so_core::proto::{AckResult, AgentView, AgentsView};

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use crate::types::{AgentDetail, AgentWriteResult, CreateAgentRequest, UpdateAgentRequest};

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

fn find(st: &AppState, name: &str) -> ApiResult<AgentDef> {
    validate_agent_name(name)?;
    Ok(resolve_for_spawn(&cwd(), &st.sup.paths, name)?)
}

fn editable(a: &AgentDef, verb: &str) -> ApiResult<()> {
    if a.source.read_only() {
        return Err(ApiError::invalid(format!(
            "`{}` is a read-only import; copy it to the config agents dir to {verb} it",
            a.name
        )));
    }
    Ok(())
}

/// What the agent's pinned harness cannot honour (nothing for `auto`).
fn warnings(def: &AgentDef) -> Vec<String> {
    match def.harness {
        HarnessPref::Fixed(h) => gaps(def, h),
        HarnessPref::Auto => Vec::new(),
    }
}

/// Parses `text` as the agent `name` living at `file`; the frontmatter name must agree.
fn parse_for(
    name: &str,
    text: &str,
    file: &FsPath,
    dir: Option<PathBuf>,
    source: AgentSource,
) -> ApiResult<AgentDef> {
    let def = parse_agent(text, file, dir, source, name)?;
    if def.name != name {
        return Err(ApiError::invalid(format!(
            "the frontmatter name `{}` must match `{name}`",
            def.name
        )));
    }
    Ok(def)
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<AgentsView>> {
    st.clone().blocking(move || {
        let d = discover(&cwd(), &st.sup.paths);
        for (path, msg) in &d.problems {
            tracing::warn!(path = %path.display(), "skipped agent: {}", msg.lines().next().unwrap_or_default());
        }
        Ok(Json(AgentsView {
            agents: d
                .agents
                .iter()
                .map(|a| AgentView {
                    name: a.name.clone(),
                    description: a.description.clone(),
                    source: Some(a.source.as_str().to_string()),
                    path: Some(a.file.display().to_string()),
                    harness: Some(a.harness.as_str().to_string()),
                })
                .collect(),
        }))
    })
    .await
}

pub async fn get(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDetail>> {
    st.clone()
        .blocking(move || {
            let a = find(&st, &name)?;
            let (source, _) =
                so_agents::parse::read_capped(&a.file, so_agents::parse::MAX_AGENT_FILE)
                    .map_err(ApiError::internal)?;
            Ok(Json(AgentDetail {
                name: a.name,
                description: a.description,
                path: a.file.display().to_string(),
                source,
                origin: a.source.as_str().to_string(),
                read_only: a.source.read_only(),
            }))
        })
        .await
}

pub async fn create(
    State(st): State<AppState>,
    ApiJson(b): ApiJson<CreateAgentRequest>,
) -> ApiResult<(StatusCode, Json<AgentWriteResult>)> {
    st.clone()
        .blocking(move || {
            validate_agent_name(&b.name)?;
            let dir = st.sup.paths.agents_dir();
            let folder = dir.join(&b.name);
            let file = folder.join("AGENT.md");
            if file.exists() || dir.join(format!("{}.md", b.name)).exists() {
                return Err(ApiError::conflict(format!(
                    "agent `{}` already exists",
                    b.name
                )));
            }
            let def = parse_for(
                &b.name,
                &b.source,
                &file,
                Some(folder.clone()),
                AgentSource::User,
            )?;
            std::fs::create_dir_all(&folder).map_err(ApiError::internal)?;
            std::fs::write(&file, &b.source).map_err(ApiError::internal)?;
            Ok((
                StatusCode::CREATED,
                Json(AgentWriteResult {
                    ok: true,
                    warnings: warnings(&def),
                }),
            ))
        })
        .await
}

pub async fn update(
    State(st): State<AppState>,
    Path(name): Path<String>,
    ApiJson(b): ApiJson<UpdateAgentRequest>,
) -> ApiResult<Json<AgentWriteResult>> {
    st.clone()
        .blocking(move || {
            let a = find(&st, &name)?;
            editable(&a, "edit")?;
            let def = parse_for(&name, &b.source, &a.file, a.dir.clone(), a.source)?;
            std::fs::write(&a.file, &b.source).map_err(ApiError::internal)?;
            Ok(Json(AgentWriteResult {
                ok: true,
                warnings: warnings(&def),
            }))
        })
        .await
}

pub async fn remove(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AckResult>> {
    st.clone()
        .blocking(move || {
            let a = find(&st, &name)?;
            editable(&a, "remove")?;
            remove_files(&a).map_err(|e: AgentError| ApiError::from(e))?;
            Ok(Json(AckResult { ok: true }))
        })
        .await
}
