#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod cmd;
mod ctx;
mod exit;
mod output;
mod redact;
mod register;
mod skill;
mod views;

use std::process::ExitCode;

use anyhow::Result;
use clap::{ArgMatches, CommandFactory, FromArgMatches};

use cli::{Cli, Command};
use ctx::{Ctx, interactive};
use exit::CliError;

fn main() -> ExitCode {
    let matches = Cli::command().get_matches();
    let cli = match Cli::from_arg_matches(&matches) {
        Ok(c) => c,
        Err(e) => e.exit(),
    };
    match dispatch(cli, &command_path(&matches)) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::from(exit::code_for(&e))
        }
    }
}

/// The subcommand chain as typed, e.g. `route eval` or `runs list`.
fn command_path(matches: &ArgMatches) -> String {
    let mut parts = Vec::new();
    let mut cur = matches;
    while let Some((name, sub)) = cur.subcommand() {
        parts.push(name);
        cur = sub;
    }
    parts.join(" ")
}

fn dispatch(cli: Cli, path: &str) -> Result<ExitCode> {
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
        Command::Init => ok(cmd::init::run(&ctx)),
        Command::Config(c) => cmd::config::run(&ctx, c),
        Command::Account(a) => cmd::account::run(&ctx, a),
        // `exec::run` only returns on error: success replaces the process, and
        // `Infallible` has no values, so the empty match proves it.
        Command::Exec(e) => match cmd::exec::run(&ctx, &e.account, &e.args)? {},
        Command::Daemon(d) => ok(cmd::daemon::run(&ctx, d.foreground)),
        Command::Status => ok(cmd::status::run(&ctx)),
        Command::Install(a) => ok(cmd::install::install(&ctx, a.force, a.dry_run)),
        Command::Uninstall(a) => ok(cmd::install::uninstall(&ctx, a.dry_run)),
        Command::Mcp(a) => ok(cmd::mcp::run(&ctx, &a.client)),
        Command::Doctor => cmd::doctor::run(&ctx),
        // Commands without an arm yet; later tasks add only their own arm above.
        _ => Err(CliError::not_implemented(path).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn command_path_follows_subcommands() {
        let path = |args: &[&str]| command_path(&Cli::command().get_matches_from(args));
        assert_eq!(path(&["so", "route", "eval"]), "route eval");
        assert_eq!(path(&["so", "route", "do it"]), "route");
        assert_eq!(path(&["so", "runs", "list"]), "runs list");
        assert_eq!(path(&["so", "migrate", "--apply"]), "migrate");
    }
}
