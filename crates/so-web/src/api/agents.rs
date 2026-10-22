//! Custom agents: the same files and parser as `agent create|list|show|edit|remove`.
//!
//! Writes are atomic (a temp file renamed into place), never go through a symlink, and an
//! agent that no longer parses can still be read, replaced and removed.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use so_agents::{
    AgentDef, AgentSource, HarnessPref, MAX_AGENT_FILE, discover, gaps, parse_agent,
    read_source_of, search_dirs, validate_agent_name,
};
use so_core::proto::{AckResult, AgentsView};
use so_core::redact::redact;

use crate::error::{ApiError, ApiResult};
use crate::extract::ApiJson;
use crate::state::AppState;
use crate::types::{AgentDetail, AgentWriteResult, CreateAgentRequest, UpdateAgentRequest};

/// An agent on disk, parsed or not.
struct Target {
    name: String,
    file: PathBuf,
    /// The agent's folder; `None` for a flat `<name>.md`.
    dir: Option<PathBuf>,
    source: AgentSource,
    /// `None` when the file does not parse.
    def: Option<AgentDef>,
}

impl Target {
    fn from_def(a: AgentDef) -> Self {
        Self {
            name: a.name.clone(),
            file: a.file.clone(),
            dir: a.dir.clone(),
            source: a.source,
            def: Some(a),
        }
    }

    fn editable(&self, verb: &str) -> ApiResult<()> {
        if self.source.read_only() {
            return Err(ApiError::invalid(format!(
                "`{}` is a read-only import; copy it to the config agents dir to {verb} it",
                self.name
            )));
        }
        Ok(())
    }

    /// Symlinked agents are not managed here: they may point outside the agents dir.
    fn ensure_regular(&self) -> ApiResult<()> {
        let link = |p: &FsPath| std::fs::symlink_metadata(p).is_ok_and(|m| m.is_symlink());
        if link(&self.file) || self.dir.as_deref().is_some_and(link) {
            return Err(ApiError::invalid(format!(
                "`{}` is a symlink and cannot be managed here",
                self.name
            )));
        }
        Ok(())
    }
}

/// The name an unparsable agent file answers to: its folder, or its file stem.
fn problem_name(file: &FsPath) -> String {
    let part = if file.file_name().is_some_and(|n| n == "AGENT.md") {
        file.parent().and_then(FsPath::file_name)
    } else {
        file.file_stem()
    };
    part.map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn find(st: &AppState, name: &str) -> ApiResult<Target> {
    validate_agent_name(name)?;
    let cwd = st.project_cwd();
    let d = discover(&cwd, &st.sup.paths);
    if let Some(a) = d.agents.into_iter().find(|a| a.name == name) {
        return Ok(Target::from_def(a));
    }
    let (file, _) = d
        .problems
        .into_iter()
        .find(|(p, _)| problem_name(p) == name)
        .ok_or_else(|| ApiError::not_found(format!("no agent named `{name}`")))?;
    let source = search_dirs(&cwd, &st.sup.paths)
        .into_iter()
        .find(|(dir, _)| file.starts_with(dir))
        .map_or(AgentSource::User, |(_, s)| s);
    let dir = (file.file_name().is_some_and(|n| n == "AGENT.md"))
        .then(|| file.parent().map(FsPath::to_path_buf))
        .flatten();
    Ok(Target {
        name: name.to_string(),
        file,
        dir,
        source,
        def: None,
    })
}

/// The UI shows redacted sources; saving one back would write the marker over a secret.
fn refuse_redacted(source: &str) -> ApiResult<()> {
    if source.contains("[REDACTED]") {
        return Err(ApiError::invalid(
            "source contains redacted values; edit the file directly or re-enter the secret",
        ));
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
    if text.len() as u64 > MAX_AGENT_FILE {
        return Err(ApiError::invalid(format!(
            "the agent file is larger than {} KiB",
            MAX_AGENT_FILE / 1024
        )));
    }
    let def = parse_agent(text, file, dir, source, name)?;
    if def.name != name {
        return Err(ApiError::invalid(format!(
            "the frontmatter name `{}` must match `{name}`",
            def.name
        )));
    }
    Ok(def)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes `bytes` to a new temp file next to `file`, then renames it over `file`. The
/// rename replaces a symlink itself rather than writing through it.
fn write_atomic(file: &FsPath, bytes: &[u8]) -> std::io::Result<()> {
    let dir = file.parent().unwrap_or_else(|| FsPath::new("."));
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{name}.tmp.{}.{n}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o644)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, file)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub async fn list(State(st): State<AppState>) -> ApiResult<Json<AgentsView>> {
    st.clone()
        .blocking(move || {
            Ok(Json(so_supervisor::agents_view(
                &st.project_cwd(),
                &st.sup.paths,
            )))
        })
        .await
}

pub async fn get(
    State(st): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<AgentDetail>> {
    st.clone()
        .blocking(move || {
            let t = find(&st, &name)?;
            t.ensure_regular()?;
            let raw = read_source_of(&t.file)?;
            let source = redact(&raw);
            Ok(Json(AgentDetail {
                description: t
                    .def
                    .as_ref()
                    .map(|d| d.description.clone())
                    .unwrap_or_default(),
                path: t.file.display().to_string(),
                redacted: source != raw,
                origin: t.source.as_str().to_string(),
                read_only: t.source.read_only(),
                name: t.name,
                source,
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
            refuse_redacted(&b.source)?;
            let dir = st.sup.paths.agents_dir();
            let folder = dir.join(&b.name);
            let file = folder.join("AGENT.md");
            let exists = || ApiError::conflict(format!("agent `{}` already exists", b.name));
            if dir.join(format!("{}.md", b.name)).exists() {
                return Err(exists());
            }
            let def = parse_for(
                &b.name,
                &b.source,
                &file,
                Some(folder.clone()),
                AgentSource::User,
            )?;
            std::fs::create_dir_all(&dir).map_err(ApiError::internal)?;
            // `create_dir` claims the name: of two racing creators, only one succeeds.
            match std::fs::create_dir(&folder) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Err(exists()),
                Err(e) => return Err(ApiError::internal(e)),
            }
            if let Err(e) = write_atomic(&file, b.source.as_bytes()) {
                let _ = std::fs::remove_dir_all(&folder);
                return Err(ApiError::internal(e));
            }
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
            let t = find(&st, &name)?;
            t.editable("edit")?;
            t.ensure_regular()?;
            refuse_redacted(&b.source)?;
            let def = parse_for(&name, &b.source, &t.file, t.dir.clone(), t.source)?;
            write_atomic(&t.file, b.source.as_bytes()).map_err(ApiError::internal)?;
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
            let t = find(&st, &name)?;
            t.editable("remove")?;
            t.ensure_regular()?;
            match &t.dir {
                Some(dir) => std::fs::remove_dir_all(dir),
                None => std::fs::remove_file(&t.file),
            }
            .map_err(ApiError::internal)?;
            Ok(Json(AckResult { ok: true }))
        })
        .await
}
