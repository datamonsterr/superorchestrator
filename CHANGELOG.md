# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- A harness that exits while starting with a quota, rate-limit or auth error (for example agy's
  "Eligibility check failed") now fails the run with that kind and fails it over to the next
  account. Launch failures count as attempts, so failover is bounded and never returns to an
  account, and an auth failure flags every pool of the account.
- Codex choices are now seeded enabled; a choice without a signed-in account is skipped at
  routing time, so adding a codex account makes `harness: auto` use it. Configs made by an
  older `init` keep their codex choices off: `doctor` warns and says to set `enabled = true`.
  Claude choices stay off by default.
- Inspection-only CLI commands (`status`, `runs show`, `join`, `route`, `ui`) no longer open an empty session in the Sessions list. A new `inspect` option of the `hello` handshake opens none and limits the connection to reads and run controls. A newer CLI talking to a supervisor started before this change falls back to the old handshake (one session per call) until that supervisor restarts.
- Web UI: dashboard crash against the real server (API types now come from the generated Rust types), black timeline state colours on an empty session, a CSP console error from zod, and a `1970-01-01` headroom `observed_at` (now `null`).

### Added

- `pnpm -C web e2e:real`: browser tests against a real, seeded supervisor (CI job `browser-e2e-real`).

## [0.1.0] - Unreleased

First release. Pending the `v0.1.0` tag.

### Added

- Cargo workspace with a React, Vite and TypeScript frontend in `web/` (app shell, dashboard,
  sessions, accounts and choices pages).
- Core types, paths, config, account and protocol modules, and a SQLite store with versioned
  migrations.
- Command line: `init`, `config`, `account`, `agent`, `models`, `route`, `runs`, `join`,
  `worktrees`, `status`, `doctor`, `exec`, `daemon`, `mcp`, `install` and `uninstall`, plus an
  interactive menu.
- Supervisor daemon on a local socket with sessions, a lock, idle exit and reconcile, and a
  run manager (spawn, wait, get, cancel) with worktrees per run.
- Harness drivers for Claude Code, Codex (app-server), OpenCode (serve) and agy, with a
  scripted fake harness for tests.
- Accounts: launch environments, wrappers, sign-in checks, account scanning, and usage probes
  with a ledger, headroom and exhaustion tracking.
- Router: model catalog, eligibility, heuristic and Jev decider, selection and evaluation.
- Custom agents defined by `AGENT.md`, with discovery, materialisation and starter agents.
- QuickJS workflow engine.
- MCP stdio server that forwards tools to the supervisor; `install` registers it with
  Claude Code and other harnesses.
- Run messaging, `ask_parent`, a review gate, stall handling, resume after death, fork and
  notifications.
- Release pipeline: cargo-dist builds for Linux and macOS on x86_64 and aarch64 on version tags,
  and an `install.sh` installer that verifies sha256 checksums.
- CI: lint, tests on Linux and macOS, Arch Linux tests, web checks and bundle size budget,
  coverage report, end-to-end smoke with fake harnesses, shellcheck and actionlint.

[Unreleased]: https://github.com/datamonsterr/superorchestrator/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/datamonsterr/superorchestrator/releases/tag/v0.1.0
