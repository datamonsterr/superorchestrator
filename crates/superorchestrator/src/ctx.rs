//! Shared command context and small output helpers.

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::{Context, Result};
use so_accounts::HostEnv;
use so_core::{Config, Paths, expand_tilde};

use crate::cli::Cli;

pub struct Ctx {
    pub paths: Paths,
    pub json: bool,
    pub yes: bool,
}

impl Ctx {
    pub fn from_cli(cli: &Cli) -> Result<Self> {
        let paths = match &cli.config_dir {
            Some(dir) => Paths::from_dir(expand_tilde(&dir.to_string_lossy())?),
            None => Paths::resolve()?,
        };
        Ok(Self {
            paths,
            json: cli.json,
            yes: cli.yes,
        })
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

/// Exit code 2 for commands that are planned but not built yet.
pub fn not_implemented(cmd: &str) -> ExitCode {
    eprintln!("`{cmd}` is not implemented yet");
    ExitCode::from(2)
}

/// Debug logging for paths that must stay silent otherwise (`exec`).
pub fn debug_enabled() -> bool {
    std::env::var("SUPERORCHESTRATOR_LOG").is_ok_and(|v| v.contains("debug") || v.contains("trace"))
}

/// Prints a left-aligned table.
pub fn print_table(header: &[&str], rows: &[Vec<String>]) {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for r in rows {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let line = |cells: Vec<&str>| {
        let parts: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{c:<w$}", w = widths[i]))
            .collect();
        println!("{}", parts.join("  ").trim_end());
    };
    line(header.to_vec());
    for r in rows {
        line(r.iter().map(String::as_str).collect());
    }
}
