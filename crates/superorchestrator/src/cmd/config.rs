//! `config get|set|edit`.

use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use so_core::Config;

use crate::cli::ConfigCmd;
use crate::ctx::Ctx;

pub fn run(ctx: &Ctx, cmd: ConfigCmd) -> Result<ExitCode> {
    match cmd {
        ConfigCmd::Get { key } => {
            let cfg = ctx.config()?;
            match cfg.get(&key)? {
                Some(v) => {
                    println!("{v}");
                    Ok(ExitCode::SUCCESS)
                }
                None => Ok(ExitCode::FAILURE),
            }
        }
        ConfigCmd::Set { key, value } => {
            Config::set_in_file(&ctx.paths, &key, &value)?;
            Ok(ExitCode::SUCCESS)
        }
        ConfigCmd::Edit => edit(ctx),
    }
}

fn edit(ctx: &Ctx) -> Result<ExitCode> {
    let path = ctx.paths.config_toml();
    if !path.exists() {
        bail!(
            "{} does not exist; run `superorchestrator init`",
            path.display()
        );
    }
    let editor = std::env::var("VISUAL")
        .ok()
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var("EDITOR").ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "vi".to_string());
    // $EDITOR may carry arguments ("code --wait"); split on whitespace.
    let mut parts = editor.split_whitespace();
    let program = parts.next().context("empty $EDITOR")?;
    let status = Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .with_context(|| format!("could not start editor `{editor}`"))?;
    if !status.success() {
        bail!("editor `{editor}` exited with {status}");
    }
    match Config::load(&ctx.paths) {
        Ok(_) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            eprintln!("error: config.toml is invalid after editing: {e}");
            eprintln!("fix it with `superorchestrator config edit`");
            Ok(ExitCode::FAILURE)
        }
    }
}
