# Superorchestrator plugin for Claude Code

This plugin registers the `superorchestrator` MCP server and adds the `use-superorchestrator-mcp` and `superorchestrator-cli` skills.

It needs the `superorchestrator` binary on `PATH`. Install it first (see the repository README).

The plugin starts `superorchestrator mcp`. Do not also run `superorchestrator install` for Claude Code, or the server is registered twice.
Accounts, choices and agents come from your own config. Run `superorchestrator setup` to create the config and add accounts before use. See the [getting started guide](../docs/guide/getting-started.md).

Validate the plugin with `claude plugin validate plugin --strict`.

## Monitor

The monitor needs `superorchestrator` 0.1 or later on `PATH`. It shows account quota,
run counts and a timeline of the supervisor, orchestrator and subagents. Docked
panes stack these sections; inline panes pack quota across the available width.
Press a run to toggle its model, account, branch (when available) and open question.
Press `w` in the focused pane to cycle the timeline through 30, 60 and 180 minutes.

Use `/so-monitor` to open or focus the pane, or retry after the binary becomes
available. The `monitor` option (default `true`) opens the pane at session start;
`band` (default `true`) shows an attention line above the prompt. Change these in
Claude Code's plugin configuration.

| Mark | Meaning / color |
| --- | --- |
| ▶ | Running / IDE accent |
| ◆ | Claude or review / Claude accent |
| ● | Person needed / shell accent; supervisor up / green |
| ⚠ | Stuck or stale supervisor / yellow |
| ✗ | Failed / red |
| ✓ | Succeeded / green |
| ○ | Queued / muted |
| ■ | Cancelled / muted |

Waiting uses shaded cells; idle and empty tracks follow the current light or dark
theme. New attention produces a toast, and session run counts appear in the status
line.
