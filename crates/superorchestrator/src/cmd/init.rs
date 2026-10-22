//! `init`: create the config dir and seed files. Never overwrites.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use anyhow::{Context, Result};
use so_core::Paths;

use crate::ctx::Ctx;

const CONFIG_HEADER: &str = include_str!("../seed/config_header.toml");
/// Choices seeded from agent-dispatcher's `config.example.json`, plus Codex choices
/// and Claude choices that start disabled (SPEC 8.2). A Codex choice is enabled: with
/// no signed-in codex account it is simply not eligible at routing time (SPEC 8.3),
/// so adding an account later needs no config edit.
pub const SEED_CHOICES: &str = include_str!("../seed/choices.toml");
const MODELS_PLACEHOLDER: &str = "# Model catalog (SPEC 8.1): known models per harness with tier, efforts, pool and cost.\n\
# Filled in by `superorchestrator models refresh`. The catalog is only used for\n\
# discovery and validation; it never edits choices.\n";

pub fn config_toml_text() -> String {
    format!("{CONFIG_HEADER}\n{SEED_CHOICES}")
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Created,
    Exists,
}

fn ensure_dir(path: &Path) -> Result<Outcome> {
    if path.is_dir() {
        return Ok(Outcome::Exists);
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("could not create {}", path.display()))?;
    Ok(Outcome::Created)
}

fn ensure_file(path: &Path, text: &str) -> Result<Outcome> {
    let opened = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path);
    match opened {
        Ok(mut f) => {
            f.write_all(text.as_bytes())
                .with_context(|| format!("could not write {}", path.display()))?;
            Ok(Outcome::Created)
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(Outcome::Exists),
        Err(e) => Err(e).with_context(|| format!("could not create {}", path.display())),
    }
}

/// Creates what is missing and returns `(path, outcome)` in creation order.
pub fn init(paths: &Paths) -> Result<Vec<(std::path::PathBuf, Outcome)>> {
    let mut report = Vec::new();
    let mut dir = |p: std::path::PathBuf| -> Result<()> {
        let o = ensure_dir(&p)?;
        report.push((p, o));
        Ok(())
    };
    dir(paths.config_dir.clone())?;
    for d in [
        paths.accounts_dir(),
        paths.agents_dir(),
        paths.workflows_dir(),
        paths.runs_dir(),
        paths.logs_dir(),
    ] {
        dir(d)?;
    }
    for (p, text) in [
        (paths.config_toml(), config_toml_text()),
        (paths.models_toml(), MODELS_PLACEHOLDER.to_string()),
    ] {
        let o = ensure_file(&p, &text)?;
        report.push((p, o));
    }
    for (p, created) in so_agents::install_starters(paths)? {
        report.push((
            p,
            if created {
                Outcome::Created
            } else {
                Outcome::Exists
            },
        ));
    }
    Ok(report)
}

/// What to do after `init`, in order. Printed at the end of `init`.
pub const NEXT_STEPS: [&str; 3] = [
    "superorchestrator setup                    add accounts, sign in and register with Claude Code in one guided run",
    "superorchestrator doctor                   check binaries, sign-ins and the database",
    "then start Claude Code and ask it to delegate a task",
];

pub fn run(ctx: &Ctx) -> Result<()> {
    let report = init(&ctx.paths)?;
    let items: Vec<_> = report
        .iter()
        .map(|(p, o)| {
            serde_json::json!({
                "path": p,
                "status": match o { Outcome::Created => "created", Outcome::Exists => "exists" },
            })
        })
        .collect();
    ctx.emit(
        &serde_json::json!({ "ok": true, "items": items, "next_steps": NEXT_STEPS }),
        |w| {
            for (path, outcome) in &report {
                let word = match outcome {
                    Outcome::Created => "created",
                    Outcome::Exists => "exists ",
                };
                writeln!(w, "{word} {}", path.display())?;
            }
            writeln!(w, "\nNext steps:")?;
            for (i, step) in NEXT_STEPS.iter().enumerate() {
                writeln!(w, "  {}. {step}", i + 1)?;
            }
            Ok(())
        },
    )
}
