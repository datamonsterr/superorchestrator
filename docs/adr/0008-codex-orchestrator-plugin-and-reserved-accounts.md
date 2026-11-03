# Codex orchestrator plugin, hook subcommand, bounded waits and reserved accounts

Status: accepted · 2026-11-04

## Context

Codex CLI can be the orchestrator, not only a subagent harness. That needs a plugin, session hooks, long waits through the MCP server and a way to keep one Codex account out of routing. Four choices came up.

Codex always parses the plugin's default `hooks/hooks.json`. In the Claude Code `plugin/` directory that file is the mod module list, which is not a hook file. Codex starts MCP servers with a scrubbed environment and limits startup to 10 s and tool calls to 60 s. A `wait_agent` call can last far longer than that.

## Decision

1. **A separate `plugin-codex/` directory.** The Codex plugin does not share `plugin/`. We tried a shared directory with the manifest pointing at another hooks file, tested against codex-cli 0.166, and rejected it: the default `hooks/hooks.json` is still parsed. `plugin-codex/` holds its own manifest, `.mcp.json`, hooks and generated skill copies. A test (`crates/superorchestrator/tests/plugin.rs`) keeps the copies in step. The repo marketplace `.agents/plugins/marketplace.json` points at it.
2. **A harness-neutral `superorchestrator hook` subcommand.** Hooks call `superorchestrator hook session-start|stop`, which prints JSON that Claude Code and Codex both accept. There are no shell scripts. The logic reads the store, ships in the binary and is tested in Rust. It is read-only, bounded to about 2 s, and never blocks a stop.
3. **Bounded waits.** `wait_agent` and `spawn_agent(wait: true)` return after at most 30 minutes (`so_core::MAX_WAIT`); the run continues. The Codex MCP entries set `tool_timeout_sec = 1860`, `startup_timeout_sec = 30` and `env_vars = ["XDG_RUNTIME_DIR"]`. The orchestrator is told to spawn with `wait: false` and loop on `wait_agent`. Verified with codex-cli 0.166: a 600 s `wait_agent` call completed.
4. **Reserved accounts are config, not routing priority.** `[orchestrator] reserved_accounts` lists accounts that auto routing never picks. A call that pins `account` may. We did not raise or lower account `priority`, because priority only orders candidates and a reserved account would still be chosen when it is the only one with headroom.

## Consequences

- The Codex plugin is a second directory to maintain. The sync test fails when the skill copies drift.
- A wait longer than 30 minutes needs more than one call. The orchestrator loops.
- A pinned call can spend a reserved account. That is the intended escape hatch.
- Users choose between `install` and the plugin for the MCP entry. An `[mcp_servers.superorchestrator]` entry in `config.toml` wins over the plugin's server of the same name.
- Codex asks the user to trust non-managed hooks through `/hooks`.
