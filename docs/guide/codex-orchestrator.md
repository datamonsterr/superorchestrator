# Codex as orchestrator

The orchestrator is the session you talk to. It plans, delegates through the MCP tools and reviews the runs. Claude Code is the default orchestrator, but Codex CLI works too. This page covers the Codex-specific setup.

## When to use it

Use Codex as the orchestrator when you work in Codex and want it to delegate to subagents on other accounts and harnesses. Keep one Codex account for the orchestrator (a [reserved account](accounts.md#reserved-accounts)) so routing never spends its limits on subagents.

## Set up

```sh
superorchestrator setup --orchestrator codex
```

This is the guided `setup` with Codex as the orchestrator. It:

- adds two Codex accounts when `codex` is installed and you have no account or importable login: one reserved for the orchestrator, one for subagents;
- adds one Claude account when `claude` is installed and you have none;
- reserves the Codex account whose home is `$CODEX_HOME` or `~/.codex`, else the first Codex account by name;
- runs `install`, which registers the MCP server with Codex and writes the `AGENTS.md` block (see below).

Useful flags: `--reserve NAME` (repeatable, overrides the choice; the name must be an account that exists after setup's account steps: existing, imported, or created by this setup. Otherwise setup fails with "account NAME does not exist"), `--dry-run` (prints the plan, including `orchestrator` and `reserve`), and `--no-agents-md`. Without `--orchestrator`, `setup` reserves nothing. Running it again changes nothing.

## Install or plugin

Choose one way to give Codex the MCP server.

| | `superorchestrator install` | Codex plugin |
|---|---|---|
| Registers | `[mcp_servers.superorchestrator]` in `~/.codex/config.toml` and each Codex account's `config.toml` | The plugin's `.mcp.json` |
| Skills | `~/.agents/skills` and `~/.claude/skills` | Bundled copies |
| `AGENTS.md` block | Yes | No |
| Hooks | No | Yes |

`install` is what `setup` runs. It writes these settings on the entry:

```toml
[mcp_servers.superorchestrator]
command = "/home/you/.local/bin/superorchestrator"
args = ["mcp"]
startup_timeout_sec = 30
tool_timeout_sec = 1860
env_vars = ["XDG_RUNTIME_DIR"]
```

`install` writes the absolute path of the running binary (the example shows a typical one). Only the plugin's `.mcp.json` uses the bare `superorchestrator` from `PATH`.

Codex defaults are 10 s to start a server and 60 s per tool call. Codex also starts MCP servers with a scrubbed environment. Without `XDG_RUNTIME_DIR`, the server cannot find the supervisor socket.

The plugin lives in `plugin-codex/`, separate from the Claude Code `plugin/`. Add it with:

```sh
codex plugin marketplace add datamonsterr/superorchestrator   # or a local path
codex plugin add superorchestrator@superorchestrator
```

Enable or disable it in `config.toml`:

```toml
[plugins."superorchestrator@superorchestrator"]
enabled = true
```

If `install` also wrote `[mcp_servers.superorchestrator]` to `config.toml`, that entry takes precedence over the plugin's server of the same name. Using both is harmless but redundant. Pick one.

## Reserved accounts

`setup` sets `[orchestrator] reserved_accounts` for you. Auto routing never picks a reserved account. The orchestrator's instructions tell it never to pin a reserved account for a subagent. Pinning `account` in a call is only a manual escape hatch for an operator. `list_harnesses` marks reserved rows `reserved: true`, and a choice reachable only through reserved accounts shows `why: "account reserved"`. See [config](config.md#orchestrator).

## Waiting for subagents

Codex limits how long a tool call may run, so wait in short steps:

1. Call `spawn_agent` with `wait: false`. It returns a run id at once.
2. Call `wait_agent` with `timeout_ms` of 30 minutes or less. It returns when a run finishes, posts a question, or the timeout ends. On timeout, call it again.

`wait_agent` caps its timeout at 30 minutes. `spawn_agent(wait: true)` also returns after at most 30 minutes with the run's current state. The run keeps going. `tool_timeout_sec = 1860` (31 minutes) leaves room for that cap. A 600 s `wait_agent` call was verified end to end with codex-cli 0.166.

## The `AGENTS.md` block

`install` writes a managed block between `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->` to `$CODEX_HOME/AGENTS.md` (default `~/.codex/AGENTS.md`) and to each Codex account home's `AGENTS.md`. It matches the `~/.claude/CLAUDE.md` block: delegate through superorchestrator, load `use-superorchestrator-mcp` first, use `superorchestrator-cli` for the shell, leave `harness: auto`, and keep reserved accounts for the orchestrator. The `AGENTS.md` version adds the wait pattern above.

Text outside the markers is never touched. Symlinked files are edited through the link. Pass `--no-agents-md` to `install` or `setup` to skip it. `uninstall` removes the block. Codex reads `AGENTS.override.md` instead when that file is present and not empty, so put the block there if you use one.

## Hooks and trust

The plugin registers two hooks, which call `superorchestrator hook session-start` and `superorchestrator hook stop`:

- **SessionStart** adds context that lists runs in the same git repository that wait at the review gate or have an open question.
- **Stop** prints a warning when runs wait at the review gate. It never blocks the stop.

Both are silent when nothing waits or the supervisor is down. They read the store read-only and stop after about 2 s. The hook output works in Claude Code and Codex alike.

Codex runs only hooks you trust. Open `/hooks` in Codex and trust the superorchestrator hooks once.

## Resuming native sessions

`superorchestrator runs show <run>` prints a `join:` line (`get_run` returns it as `join_command`). For a Codex run it is `<account-name> resume <native session id>`, run through the account's wrapper so the right credential home is used. `superorchestrator join <run> [--exec]` runs it. A bare `codex resume <id>` uses the default `CODEX_HOME` and may not find the session.

## Troubleshooting

**The tools are missing in Codex.** Check what Codex has registered:

```sh
codex mcp get superorchestrator --json
```

Look for `startup_timeout_sec: 30`, `tool_timeout_sec: 1860` and `XDG_RUNTIME_DIR` in `env_vars`. If they are absent, run `superorchestrator install` again, or check which entry wins (see "Install or plugin"). If the server starts but cannot reach the supervisor, `XDG_RUNTIME_DIR` is not passed. Run `superorchestrator doctor` and see [troubleshooting](troubleshooting.md).

**A long wait fails at 60 s.** `tool_timeout_sec` is missing from the entry Codex uses. Fix it as above.

**Hooks do not run.** Trust them in Codex with `/hooks`. Hooks come from the plugin, not from `install`.

**A reserved account was used.** Only a call that pins `account` may use one, and the orchestrator is told not to. Check `reserved_accounts` with `superorchestrator config get orchestrator.reserved_accounts`.
