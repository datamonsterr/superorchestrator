//! Shared command context.

use std::io::IsTerminal;
use std::path::PathBuf;

use anyhow::{Context, Result};
use so_accounts::HostEnv;
use so_core::{Config, Paths, expand_tilde};

pub struct Ctx {
    pub paths: Paths,
    pub json: bool,
    pub yes: bool,
}

impl Ctx {
    pub fn new(config_dir: Option<PathBuf>, json: bool, yes: bool) -> Result<Self> {
        let paths = match config_dir {
            Some(dir) => Paths::from_dir(expand_tilde(&dir.to_string_lossy())?),
            None => Paths::resolve()?,
        };
        Ok(Self { paths, json, yes })
    }

    pub fn config(&self) -> Result<Config> {
        Config::load(&self.paths).context("could not load config.toml")
    }

    pub fn host(&self) -> Result<HostEnv> {
        Ok(HostEnv::detect()?)
    }
}

pub fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

pub fn interactive() -> bool {
    stdin_is_tty() && std::io::stdout().is_terminal()
}

/// Debug logging for paths that must stay silent otherwise (`exec`).
pub fn debug_enabled() -> bool {
    std::env::var("SUPERORCHESTRATOR_LOG").is_ok_and(|v| v.contains("debug") || v.contains("trace"))
}
