//! Interactive menu shown when no subcommand is given on a TTY.

use anyhow::Result;
use dialoguer::{Input, Select, theme::ColorfulTheme};
use so_core::Harness;

use crate::cmd::{account, init, status};
use crate::ctx::Ctx;

const MAIN: [&str; 7] = [
    "init",
    "accounts",
    "status",
    "open web UI",
    "install into Claude Code",
    "migrate",
    "quit",
];

pub fn run(ctx: &Ctx) -> Result<()> {
    let theme = ColorfulTheme::default();
    loop {
        let pick = Select::with_theme(&theme)
            .with_prompt("superorchestrator")
            .items(MAIN)
            .default(0)
            .interact_opt()?;
        match pick {
            Some(0) => init::run(&ctx.paths)?,
            Some(1) => accounts_menu(ctx, &theme)?,
            Some(2) => status::run(ctx)?,
            Some(3) => eprintln!("`ui` is not implemented yet"),
            Some(4) => eprintln!("`install` is not implemented yet"),
            Some(5) => eprintln!("`migrate` is not implemented yet"),
            _ => return Ok(()),
        }
        println!();
    }
}

fn accounts_menu(ctx: &Ctx, theme: &ColorfulTheme) -> Result<()> {
    let items = ["list", "add", "import (scan)", "back"];
    let pick = Select::with_theme(theme)
        .with_prompt("accounts")
        .items(items)
        .default(0)
        .interact_opt()?;
    match pick {
        Some(0) => account::print_accounts(ctx)?,
        Some(1) => {
            let harnesses = ["codex", "agy", "opencode", "claude"];
            let h = Select::with_theme(theme)
                .with_prompt("harness")
                .items(harnesses)
                .default(0)
                .interact()?;
            let name: String = Input::with_theme(theme)
                .with_prompt("account name")
                .interact_text()?;
            let harness: Harness = harnesses[h]
                .parse()
                .map_err(|_| anyhow::anyhow!("bad harness"))?;
            let cfg = ctx.config()?;
            let host = ctx.host()?;
            match so_accounts::manage::add(&ctx.paths, &cfg, &host, harness, &name, None) {
                Ok(a) => println!("added account {} ({})", a.name, a.harness),
                Err(e) => eprintln!("error: {e}"),
            }
        }
        Some(2) => {
            let cfg = ctx.config()?;
            let host = ctx.host()?;
            account::import_scan(ctx, &cfg, &host)?;
        }
        _ => {}
    }
    Ok(())
}
