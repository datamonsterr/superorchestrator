---
name: superorchestrator-cli
description: Use the superorchestrator command line. Use it when managing superorchestrator accounts (add, sign in, enable, disable), checking status and headroom, or listing, watching and joining runs from a shell.
---

# superorchestrator command line

The `superorchestrator` binary manages accounts, shows what is running and starts the web UI. Delegating work goes through the MCP tools instead: see the `use-superorchestrator-mcp` skill. Global flags: `--json` (machine-readable output where a command lists things), `-y`/`--yes` (do not ask for confirmation) and `--config-dir DIR`. `superorchestrator <command> --help` shows every flag.

## Set up

- `superorchestrator setup` is the first command a new user runs: it creates the config, finds the installed harnesses, imports existing logins, adds and signs in accounts and runs `install`. It is safe to run again and adds nothing unless asked. Flags: `--yes` (take the defaults), `--codex N --claude N --opencode N --agy N` (accounts to add per harness; claude defaults to 0 because Claude limits are kept for the orchestrator), `--no-login`, `--no-install` and `--no-claude-md`. Without a terminal and without `--yes` it only prints the plan. `--yes --json` prints one summary object.
- `superorchestrator` with no command on a terminal (or `superorchestrator tui`) opens a full-screen terminal UI: overview with a setup checklist, accounts (add, sign in, enable, flag, remove, import), runs (details, cancel, join, accept, reject) and the setup steps. Without a terminal it prints the help.
- `superorchestrator init` creates the config dir and seeds `config.toml`, choices and the model catalog.
- `superorchestrator install [--force] [--dry-run] [--no-claude-md]` registers the MCP server with each harness, ships the two skills and adds a short managed block to `~/.claude/CLAUDE.md`. `superorchestrator uninstall [--dry-run]` removes what `install` added. Both back up every file they change.

## Accounts

An account is one signed-in credential home for one harness (codex, agy, opencode or claude).

- `account add <harness> [--name NAME] [--count K] [--login]` creates a credential home, the account file and a wrapper. Without `--name`, accounts are named `<harness>-<n>` with the lowest free `n`; `--count K` creates K of them (not with `--name`). `--login` signs in to each new account in turn on a terminal. `--home DIR` needs `--name`.
- `account import NAME --harness H --home DIR` links an existing credential home. `account import --scan` finds candidates and offers to import each.
- `account login NAME` signs in.
- `account list` shows sign-in state, load and headroom.
- `account enable NAME` and `account disable NAME`.
- `account flag NAME (--hours N | --until RFC3339 | --clear) [--pool POOL]` puts an account (or one pool of it) in manual cooldown, or clears it.
- `account remove NAME [--purge]` removes an account. `--purge` also deletes its credential home.

## Status and checks

- `superorchestrator status` shows accounts, headroom and the supervisor.
- `superorchestrator doctor` checks binaries, config, database, socket, UI port and sign-ins.
- `superorchestrator route "<task>" [--role ROLE] [--explain]` previews which choice and account the router would pick. It runs nothing.

## Runs

- `runs list [--all]` lists active runs, or all runs with `--all`.
- `runs show RUN` shows state, recent events, the result, usage and the join command.
- `runs watch RUN` follows a run until it finishes. `runs watch --workflow ID` follows a workflow run.
- `runs cancel RUN` stops a run and its subtree.
- `join RUN [--exec]` prints the command that attaches to the run's native session, or runs it with `--exec`.
- `worktrees list` and `worktrees prune` manage the worktrees of runs.

A run id may be a unique prefix.

## Workflows

- `workflow run NAME` or `workflow run --file FILE` runs a saved workflow or a script. Options: `--args JSON`, `--detach` (print the run id and return) and `--fresh` (do not replay cached steps).
- `workflow list`, `workflow status ID` and `workflow cancel ID`.

## Web UI and config

- `superorchestrator ui [--port N] [--bind ADDR] [--token T] [--no-open]` starts the web UI. `--install-service` writes a systemd user unit or launchd agent.
- `config get KEY`, `config set KEY VALUE` and `config edit` read and write `config.toml`.
- `agent list`, `agent show NAME`, `agent create NAME` and `models list` cover custom agents and the model catalog.

## When something is wrong

Run `superorchestrator doctor` first. The repository's `docs/guide/troubleshooting.md` lists each check and its fix, and `docs/guide/accounts.md` covers sign-in problems.
