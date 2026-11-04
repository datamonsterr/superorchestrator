# Superorchestrator for Codex

Install with `codex plugin marketplace add <repo URL or path>`, then
`codex plugin add superorchestrator@superorchestrator`. The plugin's hooks need
trust: review and trust them with `/hooks` in Codex. Codex documents plugin
lifecycle hooks as supported for manually installed plugins. If they do not run
in your Codex surface, run `superorchestrator runs list` instead.

`superorchestrator install` also writes `[mcp_servers.superorchestrator]` in
Codex `config.toml`. That entry takes precedence over the plugin's server of the
same name.
