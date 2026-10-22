# Superorchestrator plugin for Claude Code

This plugin registers the `superorchestrator` MCP server and adds the `use-superorchestrator-mcp` and `superorchestrator-cli` skills.

It needs the `superorchestrator` binary on `PATH`. Install it first (see the repository README).

The plugin starts `superorchestrator mcp`. Do not also run `superorchestrator install` for Claude Code, or the server is registered twice.
Accounts, choices and agents come from your own config. Run `superorchestrator setup` to create the config and add accounts before use. See the [getting started guide](../docs/guide/getting-started.md).

Validate the plugin with `claude plugin validate plugin --strict`.
