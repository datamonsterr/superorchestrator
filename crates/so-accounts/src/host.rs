//! Injectable view of the host environment, so the library never reads the
//! process environment by itself.

use std::path::{Path, PathBuf};

use so_core::Platform;
use so_core::paths::expand_tilde_with;

use crate::error::Result;

#[derive(Debug, Clone)]
pub struct HostEnv {
    pub home: PathBuf,
    pub path_var: Option<String>,
    pub platform: Platform,
}

impl HostEnv {
    /// Reads `HOME` and `PATH` from the process environment.
    pub fn detect() -> Result<Self> {
        let home = std::env::var("HOME")
            .ok()
            .filter(|h| !h.is_empty())
            .ok_or(so_core::CoreError::NoHomeDir)?;
        Ok(Self {
            home: PathBuf::from(home),
            path_var: std::env::var("PATH").ok(),
            platform: Platform::current(),
        })
    }

    /// Expands a leading `~` against this host's home.
    pub fn expand(&self, s: &str) -> Result<PathBuf> {
        let home = self.home.to_string_lossy().into_owned();
        let env = move |k: &str| (k == "HOME").then(|| home.clone());
        Ok(expand_tilde_with(&env, s)?)
    }

    /// Renders `p` as `~/rel` when it is under the home directory.
    pub fn tildify(&self, p: &Path) -> String {
        match p.strip_prefix(&self.home) {
            Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
            Ok(rel) => format!("~/{}", rel.display()),
            Err(_) => p.display().to_string(),
        }
    }
}

/// True when both paths are the same, lexically or after resolving symlinks.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}
