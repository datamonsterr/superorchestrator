#![cfg_attr(test, allow(clippy::unwrap_used))]

mod cli;
mod cmd;
mod ctx;
mod redact;

use std::process::ExitCode;

use anyhow::Result;
use clap::{CommandFactory, Parser};

use cli::{AgentCmd, Cli, Command, ModelsCmd, RouteSub, RunsCmd, WorkflowCmd, WorktreesCmd};
use ctx::{Ctx, interactive, not_implemented};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match dispatch(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(cli: Cli) -> Result<ExitCode> {
    let Some(command) = cli.command else {
        if interactive() {
            let ctx = Ctx::from_cli(&cli)?;
            cmd::menu::run(&ctx)?;
        } else {
            Cli::command().print_help()?;
            println!();
        }
        return Ok(ExitCode::SUCCESS);
    };
    let ctx = Ctx::from_cli(&Cli {
        command: None,
        ..cli
    })?;
    let ok = |r: Result<()>| r.map(|()| ExitCode::SUCCESS);
    match command {
        Command::Init => ok(cmd::init::run(&ctx.paths)),
        Command::Config(c) => cmd::config::run(&ctx, c),
        Command::Account(a) => cmd::account::run(&ctx, a),
        Command::Exec(e) => match cmd::exec::run(&ctx, &e.account, &e.args)? {},
        Command::Daemon(d) => ok(cmd::daemon::run(&ctx, d.foreground)),
        Command::Status => ok(cmd::status::run(&ctx)),
        Command::Doctor => cmd::doctor::run(&ctx),
        Command::Install => Ok(not_implemented("install")),
        Command::Uninstall => Ok(not_implemented("uninstall")),
        Command::Agent(a) => Ok(not_implemented(match a {
            AgentCmd::Create => "agent create",
            AgentCmd::List => "agent list",
            AgentCmd::Show { .. } => "agent show",
            AgentCmd::Edit { .. } => "agent edit",
            AgentCmd::Remove { .. } => "agent remove",
        })),
        Command::Models(m) => Ok(not_implemented(match m {
            ModelsCmd::Refresh => "models refresh",
            ModelsCmd::List => "models list",
        })),
        Command::Route(r) => Ok(not_implemented(match r.sub {
            Some(RouteSub::Eval { .. }) => "route eval",
            None => "route",
        })),
        Command::Runs(r) => Ok(not_implemented(match r {
            RunsCmd::List => "runs list",
            RunsCmd::Show { .. } => "runs show",
            RunsCmd::Watch { .. } => "runs watch",
            RunsCmd::Cancel { .. } => "runs cancel",
        })),
        Command::Join(_) => Ok(not_implemented("join")),
        Command::Worktrees(w) => Ok(not_implemented(match w {
            WorktreesCmd::List => "worktrees list",
            WorktreesCmd::Prune => "worktrees prune",
        })),
        Command::Workflow(w) => Ok(not_implemented(match w {
            WorkflowCmd::Run { .. } => "workflow run",
            WorkflowCmd::List => "workflow list",
            WorkflowCmd::Status { .. } => "workflow status",
        })),
        Command::Ui(_) => Ok(not_implemented("ui")),
        Command::Mcp => Ok(not_implemented("mcp")),
        Command::Migrate(_) => Ok(not_implemented("migrate")),
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
