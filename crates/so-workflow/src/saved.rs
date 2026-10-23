//! Saved workflows: `<config>/workflows/*.js` and `<cwd>/.superorchestrator/workflows/*.js`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::WorkflowError;

/// Where a saved workflow was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SavedSource {
    /// `<cwd>/.superorchestrator/workflows/`; wins over `Config` on a name clash.
    Project,
    /// `<config>/workflows/`.
    Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedWorkflow {
    pub name: String,
    pub path: PathBuf,
    pub source: SavedSource,
}

/// Finder for saved workflows.
#[derive(Debug, Clone)]
pub struct SavedWorkflows {
    config_dir: PathBuf,
    project_dir: Option<PathBuf>,
}

impl SavedWorkflows {
    /// `config_workflows_dir` is `Paths::workflows_dir()`; `cwd` is the caller's directory.
    pub fn new(config_workflows_dir: impl Into<PathBuf>, cwd: impl AsRef<Path>) -> Self {
        Self {
            config_dir: config_workflows_dir.into(),
            project_dir: Some(cwd.as_ref().join(".superorchestrator").join("workflows")),
        }
    }

    /// Finder restricted to the user config directory; never inspects project files.
    pub fn config_only(config_workflows_dir: impl Into<PathBuf>) -> Self {
        Self {
            config_dir: config_workflows_dir.into(),
            project_dir: None,
        }
    }

    /// All saved workflows sorted by name; a project script hides a config one of the same name.
    pub fn list(&self) -> Vec<SavedWorkflow> {
        let mut out = self
            .project_dir
            .as_ref()
            .map(|dir| scan(dir, SavedSource::Project))
            .unwrap_or_default();
        for w in scan(&self.config_dir, SavedSource::Config) {
            if !out.iter().any(|o| o.name == w.name) {
                out.push(w);
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    /// Finds a workflow by name (the file stem) and reads its source.
    pub fn load(&self, name: &str) -> Result<(SavedWorkflow, String), WorkflowError> {
        let found = valid_name(name)
            .then(|| self.list().into_iter().find(|w| w.name == name))
            .flatten()
            .ok_or_else(|| WorkflowError::NotFound(name.to_string()))?;
        let src = fs::read_to_string(&found.path)
            .map_err(|e| WorkflowError::Io(format!("{}: {e}", found.path.display())))?;
        Ok((found, src))
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\'])
}

fn scan(dir: &Path, source: SavedSource) -> Vec<SavedWorkflow> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension()? != "js" || !path.is_file() {
                return None;
            }
            let name = path.file_stem()?.to_str()?.to_string();
            valid_name(&name).then_some(SavedWorkflow { name, path, source })
        })
        .collect()
}
