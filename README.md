# superorchestrator

Superorchestrator lets one coding agent hand work to other coding agents. The agent you drive (for example Claude Code) calls MCP tools. Superorchestrator starts the work on Codex, Antigravity (agy), OpenCode or Claude, on whichever logged-in account has room. It records every delegation, so you can trace it, join it and debug it.

It is one Rust binary. It works as a CLI, a stdio MCP server, a per-user supervisor and a local web UI.

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

## Install

Install a release binary. The installer checks the sha256 checksum.

```sh
curl -fsSL https://raw.githubusercontent.com/datamonsterr/superorchestrator/main/install.sh | sh
```

The binary goes to `~/.local/bin`. Set `SUPERORCHESTRATOR_INSTALL_DIR` to change that. Pass a version to pin one: `sh install.sh 0.1.0`.

To build from source you need Rust (see `rust-toolchain.toml`), Node 24 and pnpm. Build the web UI first, because the binary embeds it.

```sh
pnpm -C web install --frozen-lockfile
pnpm -C web build
cargo build --release -p superorchestrator
cp target/release/superorchestrator ~/.local/bin/
```

`cargo install --path crates/superorchestrator` also works after the web build.

## Quickstart

1. Install the binary (above) and check it runs: `superorchestrator --version`.
2. Create the config: `superorchestrator init`. It seeds `config.toml`, the model choices and three starter agents.
3. Add accounts. Link logins you already have with `superorchestrator account import --scan`. Or make a new account and sign in: `superorchestrator account add codex --name codex-acc1`, then `superorchestrator account login codex-acc1`.
4. Register with Claude Code: `superorchestrator install`. It runs `claude mcp add`, adds the MCP entry to the other harness configs, writes the `use-superorchestrator-mcp` and `superorchestrator-cli` skills, adds a short delegation block to `~/.claude/CLAUDE.md` (skip it with `--no-claude-md`), and backs up every file it changes. Use `--dry-run` to preview.
5. Check the setup: `superorchestrator doctor`. See [troubleshooting](docs/guide/troubleshooting.md) for each check.
6. Use it from Claude Code. Start a session and ask it to delegate a task. Claude Code now has the `superorchestrator` tools and the two skills.

Check progress any time with `superorchestrator status` and `superorchestrator runs list`.

You can use the [Claude Code plugin](plugin/README.md) instead of step 4.

## Terminal UI

Run `superorchestrator` in a terminal, or `superorchestrator tui`, for a full-screen view of your setup, accounts and runs. It can add and sign in accounts, accept or reject reviewed runs and run the first-run steps. See the [terminal UI guide](docs/guide/tui.md).

## Web UI

`superorchestrator ui` starts the supervisor if needed and opens the UI at `http://127.0.0.1:7878`. It shows the dashboard, sessions with a timeline of every run, accounts, choices, agents, workflows and settings. See the [web UI guide](docs/guide/web-ui.md).

## Documentation

- [Accounts and wrappers](docs/guide/accounts.md)
- [Routing and choices](docs/guide/routing.md)
- [Custom agents](docs/guide/agents.md)
- [Workflows](docs/guide/workflows.md)
- [The terminal UI](docs/guide/tui.md)
- [The web UI](docs/guide/web-ui.md)
- [Notifications](docs/guide/notifications.md)
- [Migrating from the old tooling](docs/guide/migrate.md)
- [Config reference](docs/guide/config.md)
- [Security model](docs/guide/security.md)
- [Troubleshooting](docs/guide/troubleshooting.md)

For maintainers: [SPEC.md](SPEC.md) says what to build, [CONTEXT.md](CONTEXT.md) defines the terms, [docs/adr/](docs/adr/) records decisions, and [docs/release.md](docs/release.md) covers releasing.

## Development

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

```sh
scripts/setup-hooks.sh   # once: enables the pre-commit and pre-push hooks
scripts/check.sh         # full gate; --rust-only or --web-only to narrow
```

The full check needs pnpm and Node 24 (see `web/.nvmrc`). Use `scripts/check.sh --rust-only` without them. Set `SKIP_PREPUSH=1` to bypass the pre-push gate (CI still runs it).

`scripts/smoke.sh` is a manual test on a real machine. It runs real harnesses and spends real quota. Read its header before you run it. CI never runs it.

## Layout

- `crates/`: the Rust workspace. `superorchestrator` is the binary. The `so-*` crates hold the libraries.
- `web/`: the React, Vite and TypeScript frontend, embedded into the binary at release.
- `plugin/`: the Claude Code plugin.
- `docs/`: guides, architecture decision records and plans.

## License

MIT. See [LICENSE](LICENSE).
