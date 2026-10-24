# Getting started

This page takes you from nothing to your first delegation.

## 1. Install

```sh
curl -fsSL https://raw.githubusercontent.com/datamonsterr/superorchestrator/main/install.sh | sh
superorchestrator --version
```

The binary goes to `~/.local/bin`. If that is not on your `PATH`, the installer says so. Add it to your shell profile.

You also need at least one harness CLI installed: `codex`, `opencode`, `agy` or `claude`. Superorchestrator does not install them.

## 2. Run `setup`

```sh
superorchestrator setup
```

`setup` asks a few questions and does the rest. It is safe to run again. It:

1. creates the config directory (the same as `superorchestrator init`);
2. lists the harnesses it found on `PATH`;
3. offers to import logins already on disk (`account import --scan`);
4. asks how many accounts to add for each harness;
5. signs in to accounts that are signed out;
6. runs `install` (see below);
7. prints a `doctor` summary.

Account defaults: one account for each installed harness that has none. Claude gets none, because its limits are kept for the orchestrator.

To skip the questions:

```sh
superorchestrator setup --yes
superorchestrator setup --yes --codex 3 --opencode 2 --no-login
```

Without a terminal and without `--yes`, `setup` only prints its plan. Other flags: `--claude N`, `--agy N`, `--no-install` and `--no-claude-md`.

## 3. Sign in

`setup` signs in for you when you have a terminal. To sign in later, or to fix a signed-out account:

| Harness | How to sign in |
|---|---|
| codex | `superorchestrator account login <name>` |
| opencode | `superorchestrator account login <name>` (needs a terminal) |
| claude | Run the account's wrapper (`<name>`), then type `/login` inside the session |
| agy | Run the account's wrapper (`<name>`), then sign in from inside the agy session |

More in [accounts](accounts.md#logging-in-per-harness). To add more accounts later: `superorchestrator account add codex --count 2 --login`.

## 4. What `install` puts on your machine

`setup` runs `install` for you. You can run it alone with `superorchestrator install` (`--dry-run` previews it). Every file it changes is backed up first. It does four things:

- **Registers the MCP server.** It runs `claude mcp add` for Claude Code and adds an entry to the Codex, agy and OpenCode configs.
- **Writes two skills**, to `~/.claude/skills/` and `~/.agents/skills/`:
  - `use-superorchestrator-mcp` teaches the agent to delegate through the MCP tools. It lists your accounts, levels and agents, and refreshes when they change.
  - `superorchestrator-cli` teaches the agent the command line: accounts, status and runs.
- **Adds a block to `~/.claude/CLAUDE.md`**, between `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->`. It says:
  - delegate implementation through superorchestrator, and load the `use-superorchestrator-mcp` skill before the first delegation;
  - use the `superorchestrator-cli` skill for accounts, status and runs from the shell;
  - keep Claude accounts for the orchestrator, and leave `harness: auto` unless there is a reason.
- **Leaves your text alone.** Running `install` again replaces the block in place. Nothing outside the markers is touched.

To opt out of the `CLAUDE.md` block, pass `--no-claude-md` to `setup` or `install`. To remove everything `install` added, including the block and the skills, run `superorchestrator uninstall`.

If you use the [Claude Code plugin](../../plugin/README.md), it registers the MCP server and skills itself. Do not also run `install` for Claude Code.

## 5. Check

```sh
superorchestrator doctor
superorchestrator status
```

`doctor` prints one line per check and exits non-zero when a check failed. [Troubleshooting](troubleshooting.md) explains each check. `status` shows accounts, headroom and the supervisor.

## 6. Delegate

Restart Claude Code (it reads MCP servers at start), then ask:

> Delegate this to another agent with superorchestrator: add a unit test for the date parser. Wait for it and show me the result.

Watch it with `superorchestrator runs list`, or open the web UI with `superorchestrator ui`.

## Next

- [Use cases](use-cases.md): worked examples.
- [Routing, models and levels](routing.md): how the router picks.
- [Reviewing and landing runs](review.md).
