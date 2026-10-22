---
name: superorchestrator-cli
description: Use the superorchestrator command line: manage accounts (add, sign in, enable, disable), check status and headroom, list, watch and join runs from a shell.
---

# superorchestrator CLI

Delegation = MCP tools (skill `use-superorchestrator-mcp`).
Global: `--json`, `-y`/`--yes`, `--config-dir DIR`. `<cmd> --help` lists flags.

## Set up

- `superorchestrator setup`: first command. Config, detect harnesses, import logins, add accounts, `install`. Re-run safe. Flags: `--yes`, `--codex N --claude N --opencode N --agy N` (accounts per harness; claude default 0), `--no-login`, `--no-install`, `--no-claude-md`. No terminal, no `--yes`: prints plan only.
- `superorchestrator` or `superorchestrator tui`: terminal UI, tabs Overview/Accounts/Runs/Setup. `j`/`k` move, Enter detail, `a` accept+merge, `r` reject, `o` join, `?` help.
- `init`: config dir, `config.toml`, choices, model catalog.
- `install [--force] [--dry-run] [--no-claude-md]`: MCP server per harness, both skills, managed block in `~/.claude/CLAUDE.md`. `uninstall [--dry-run]` reverses.

## Accounts

Account = one signed-in credential home, one harness (codex, agy, opencode, claude).

- `account add <harness> [--name NAME] [--count K] [--login]`. Default name `<harness>-<n>`.
- `account import NAME --harness H --home DIR`: link existing home. `account import --scan`: find, offer.
- `account login NAME`, `account list`, `account enable NAME`, `account disable NAME`.
- `account flag NAME (--hours N | --until RFC3339 | --clear) [--pool POOL]`: manual cooldown.
- `account remove NAME [--purge]`: `--purge` deletes home.

## Check

- `status`: accounts, headroom, supervisor.
- `doctor`: binaries, config, db, socket, UI port, sign-ins. Run first when broken.
- `route "<task>" [--role ROLE] [--explain]`: preview pick, runs nothing.

## Runs

- `runs list [--all]`, `runs show RUN`, `runs cancel RUN`.
- `runs watch RUN`, `runs watch --workflow ID`: follow.
- `join RUN [--exec]`: print or run command attaching to native session.
- `worktrees list`, `worktrees prune`.

## Workflows

- `workflow run NAME` or `--file FILE`; `--args JSON`, `--detach`, `--fresh`.
- `workflow list`, `workflow status ID`, `workflow cancel ID`.

## UI and config

- `superorchestrator ui [--port N] [--bind ADDR] [--token T] [--no-open]`; `--install-service`.
- `config get KEY`, `config set KEY VALUE`, `config edit`.
- `agent list`, `agent show NAME`, `agent create NAME`, `models list`.
