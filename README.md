# superorchestrator

Superorchestrator lets one coding agent hand work to other coding agents. The agent you drive (for example Claude Code) calls MCP tools. Superorchestrator starts the work on Codex, Antigravity (agy), OpenCode or Claude, on whichever logged-in account has room. It records every delegation, so you can trace it, join it and debug it.

It is one Rust binary. It works as a CLI, a stdio MCP server, a per-user supervisor and a local web UI.

## Use cases

Each use case has a worked example in the [use cases guide](docs/guide/use-cases.md).

- **Spread work across subscriptions.** Use several Codex, Claude, OpenCode and agy subscriptions and accounts as one pool. When an account runs out, the run moves to the next one. See [accounts](docs/guide/accounts.md) and [routing](docs/guide/routing.md).
- **Claude Code as the orchestrator.** Claude Code plans and reviews. It delegates the bulk work to cheaper or different agents through MCP. See [custom agents](docs/guide/agents.md).
- **Reviewed delegation.** Ask for a review gate, read the diff, reject with feedback or accept and land the branch with `merge: true`. See [reviewing and landing runs](docs/guide/review.md).
- **Parallel runs and workflows.** Start many runs at once, or script them in JavaScript with `parallel()` and `pipeline()`. See [workflows](docs/guide/workflows.md).
- **Watch runs.** Follow every run in the web UI and the terminal UI (`superorchestrator`), or from the shell with `runs watch`. See the [web UI guide](docs/guide/web-ui.md) and the [terminal UI guide](docs/guide/tui.md).

## Install

<a id="quickstart"></a>

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

## Get started in 3 commands

```sh
curl -fsSL https://raw.githubusercontent.com/datamonsterr/superorchestrator/main/install.sh | sh
superorchestrator setup
claude
```

1. Install the binary.
2. Run `superorchestrator setup`.
3. Open Claude Code and ask it to delegate a task.

Check the setup any time with `superorchestrator doctor` and `superorchestrator status`. The [getting started guide](docs/guide/getting-started.md) covers each step.

### What `setup` does

`setup` is a guided first run. It is safe to repeat: it adds nothing unless you ask for more. It:

1. creates the config directory (the same as `init`);
2. finds the installed harnesses (`codex`, `claude`, `opencode`, `agy`);
3. offers to import logins it finds on disk;
4. asks how many accounts to add for each harness (default: one for each installed harness that has none; Claude gets none, to keep your limits for the orchestrator);
5. signs in to accounts that are signed out, or prints the commands when there is no terminal;
6. runs `install`: registers the MCP server with Claude Code and the other harnesses, writes the `use-superorchestrator-mcp` and `superorchestrator-cli` skills, and adds a short delegation block to `~/.claude/CLAUDE.md`;
7. prints a `doctor` summary.

Useful flags: `--yes` (take the defaults), `--codex N --claude N --opencode N --agy N` (accounts to add), `--no-login`, `--no-install` and `--no-claude-md`. Without a terminal and without `--yes`, `setup` prints its plan and changes nothing. You can use the [Claude Code plugin](plugin/README.md) instead of the `install` step.

## Your first delegation

Start Claude Code and ask:

> Delegate this to another agent with superorchestrator: add a unit test for the date parser in `src/date.rs`. Run it in a worktree, wait for it, then show me the diff.

Claude Code loads the `use-superorchestrator-mcp` skill, calls `spawn_agent`, waits with `wait_agent` and reads the result. Watch it from another terminal with `superorchestrator runs list` and `superorchestrator ui`.

## Documentation

- [Getting started](docs/guide/getting-started.md)
- [Use cases](docs/guide/use-cases.md)
- [Accounts and wrappers](docs/guide/accounts.md)
- [Routing, models and levels](docs/guide/routing.md)
- [Custom agents](docs/guide/agents.md)
- [Workflows](docs/guide/workflows.md)
- [Reviewing and landing runs](docs/guide/review.md)
- [The terminal UI](docs/guide/tui.md)
- [The web UI](docs/guide/web-ui.md)
- [Notifications](docs/guide/notifications.md)
- [Migrating from the old tooling](docs/guide/migrate.md)
- [Config reference](docs/guide/config.md)
- [Security model](docs/guide/security.md)
- [Troubleshooting](docs/guide/troubleshooting.md)

For maintainers: [SPEC.md](SPEC.md) says what to build, [CONTEXT.md](CONTEXT.md) defines the terms, [docs/adr/](docs/adr/) records decisions, and [docs/release.md](docs/release.md) covers releasing.

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
