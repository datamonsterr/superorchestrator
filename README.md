# superorchestrator

Superorchestrator lets an orchestrator delegate work to Codex, Antigravity (`agy`), OpenCode or Claude across many logged-in accounts. It records every delegation so you can trace, join and review each run.

![Superorchestrator web UI dashboard](docs/img/web-dashboard.png)

## What it does

- Routes work to an account based on model, complexity, usage and configured caps.
- Runs subagents in worktrees with a review gate to inspect, reject or land changes.
- Tracks sessions, runs, events, usage and account headroom in the web UI and terminal UI.
- Provides workflow primitives for parallel and multi-step work.
- Adds a Claude Code monitor plugin: open `/so-monitor` for account quota and the run timeline.

## Install

Run the installer (it verifies the checksum):

```sh
curl -fsSL https://raw.githubusercontent.com/datamonsterr/superorchestrator/main/install.sh | sh
```

The binary installs to `~/.local/bin`. Set `SUPERORCHESTRATOR_INSTALL_DIR` to change that. To pin a version, use `sh install.sh <version>`.

To build from source, install Rust (see `rust-toolchain.toml`), Node 24 and pnpm. Build the web UI first because the binary embeds it.

```sh
pnpm -C web install --frozen-lockfile
pnpm -C web build
cargo build --release -p superorchestrator
```

## Quickstart

```sh
superorchestrator setup
claude
```

Ask Claude Code to delegate a task. `setup` initializes the config, offers to add accounts for installed harnesses, and registers the MCP server and skills. Run `superorchestrator doctor` to check the setup. Useful flags: `--yes`, `--codex N`, `--claude N`, `--opencode N`, `--agy N`, `--no-login`, `--no-install` and `--no-claude-md`. See the [getting started guide](docs/guide/getting-started.md).

## Your first delegation

In Claude Code, try:

> Delegate adding a unit test for the date parser in `src/date.rs` to another agent. Run it in a worktree, wait for it, then show me the diff.

Watch runs from the shell with `superorchestrator runs list` and `superorchestrator ui`.

## Interfaces

- [Web UI](docs/guide/web-ui.md): sessions, accounts, routing levels, settings and live run timelines.
- [Terminal UI](docs/guide/tui.md): monitor runs and manage levels.
- [Claude Code plugin](plugin/README.md): MCP setup and the `/so-monitor` pane.

## Guides

- [Use cases](docs/guide/use-cases.md) · [Accounts](docs/guide/accounts.md) · [Routing](docs/guide/routing.md)
- [Agents](docs/guide/agents.md) · [Workflows](docs/guide/workflows.md) · [Reviewing runs](docs/guide/review.md)
- [Notifications](docs/guide/notifications.md) · [Configuration](docs/guide/config.md) · [Security](docs/guide/security.md)
- [Troubleshooting](docs/guide/troubleshooting.md) · [Migrating](docs/guide/migrate.md)

## Supported platforms and harnesses

| Platform | Support |
|---|---|
| Linux (Ubuntu, Arch), x86_64 and aarch64 | First class |
| macOS, aarch64 and x86_64 | Supported. agy allows one account only. |
| Windows | Not supported |

| Harness | Command | Notes |
|---|---|---|
| Codex | `codex` | Live join through the app-server |
| Antigravity | `agy` | One `.gemini` directory per account. Linux only for several accounts. |
| OpenCode | `opencode` | Live attach |
| Claude | `claude` | Ranks last in routing, to keep your limits for the orchestrator |

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check

scripts/setup-hooks.sh   # once: enables the pre-commit and pre-push hooks
scripts/check.sh         # full gate; --rust-only or --web-only to narrow
```

The full check needs pnpm and Node 24 (see `web/.nvmrc`). `scripts/smoke.sh` runs real harnesses and spends real quota; read its header before running it.

## Layout

- `crates/`: the Rust workspace. `superorchestrator` is the binary; `so-*` crates hold the libraries.
- `web/`: the React, Vite and TypeScript frontend, embedded into the binary at release.
- `plugin/`: the Claude Code plugin.
- `docs/`: guides, architecture decision records and plans.

For maintainers: [SPEC.md](SPEC.md) says what to build, [CONTEXT.md](CONTEXT.md) defines the terms, [docs/adr/](docs/adr/0001-per-user-supervisor-owns-runs.md) records decisions, and [docs/release.md](docs/release.md) covers releasing.

MIT licensed. See [LICENSE](LICENSE).
