#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod cmd;
mod ctx;
mod exit;
mod logfile;
mod output;
mod redact;
mod register;
mod skill;
mod views;

use std::process::ExitCode;

use anyhow::Result;
use clap::{CommandFactory, FromArgMatches};

use cli::{Cli, Command};
use ctx::{Ctx, interactive};

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(exit::code_for(&e))
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    let Cli {
        config_dir,
        json,
        yes,
        command,
    } = cli;
    let ctx = Ctx::new(config_dir, json, yes)?;
    let Some(command) = command else {
        if interactive() {
            cmd::menu::run(&ctx)?;
        } else {
            Cli::command().print_help()?;
            println!();
        }
        return Ok(ExitCode::SUCCESS);
    };
    let ok = |r: Result<()>| r.map(|()| ExitCode::SUCCESS);
    match command {
        Command::Setup(a) => cmd::setup::run(&ctx, a),
        Command::Init => ok(cmd::init::run(&ctx)),
        Command::Config(c) => cmd::config::run(&ctx, c),
        Command::Account(a) => cmd::account::run(&ctx, a),
        Command::Agent(a) => ok(cmd::agent::run(&ctx, a)),
        // `exec::run` only returns on error: success replaces the process, and
        // `Infallible` has no values, so the empty match proves it.
        Command::Exec(e) => match cmd::exec::run(&ctx, &e.account, &e.args)? {},
        Command::Daemon(d) => ok(cmd::daemon::run(&ctx, d.foreground)),
        Command::Status => ok(cmd::status::run(&ctx)),
        Command::Install(a) => ok(cmd::install::install(
            &ctx,
            a.force,
            a.dry_run,
            !a.no_claude_md,
        )),
        Command::Uninstall(a) => ok(cmd::install::uninstall(&ctx, a.dry_run)),
        Command::Mcp(a) => ok(cmd::mcp::run(&ctx, &a.client)),
        Command::Doctor => cmd::doctor::run(&ctx),
        Command::Ui(a) => ok(cmd::ui::run(&ctx, a)),
        Command::Runs(c) => cmd::runs::run(&ctx, c),
        Command::Join(a) => cmd::runs::join(&ctx, &a.run, a.exec),
        Command::Worktrees(c) => cmd::worktrees::run(&ctx, c),
        Command::Workflow(c) => cmd::workflow::run(&ctx, c),
        Command::Route(a) => cmd::route::run(&ctx, a),
        Command::Migrate(a) => cmd::migrate::run(&ctx, a.apply),
        Command::Models(c) => cmd::models::run(&ctx, c).map(|()| ExitCode::SUCCESS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }
}
