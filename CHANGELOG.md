# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - Unreleased

First release. Pending the `v0.1.0` tag.

### Changed

- Routing tuned against live Jev (`jev-1.14-20261014`): `router.policy` defaults are now
  `min_confidence = 0.45` and `min_margin = 0.15`. The seeded choice descriptions and the
  default routing instruction now separate the effort and capability tiers. Live accuracy on the
  bundled cases rose from about 50% (9-10 of 15 falling back to the heuristic) to above 90% with
  no fallbacks. The heuristic breaks ties inside a tier by effort. `route eval --json` reports
  each case's probabilities and fallback reason.

### Added

- Cargo workspace with a React, Vite and TypeScript frontend in `web/`.
- Core types, paths, config, account and protocol modules, and a SQLite store with versioned
  migrations.
- Command line: `init`, `config`, `account`, `agent`, `models`, `route`, `runs`, `join`,
  `worktrees`, `workflow`, `status`, `doctor`, `exec`, `daemon`, `mcp`, `ui`, `migrate`,
  `install` and `uninstall`, plus an interactive menu. `init` prints the next steps.
- Supervisor daemon on a local socket with sessions, a lock, idle exit and reconcile, and a
  run manager (spawn, wait, get, cancel) with worktrees per run.
- Harness drivers for Claude Code, Codex (app-server), OpenCode (serve) and agy, with a
  scripted fake harness for tests.
- Accounts: launch environments, wrappers, sign-in checks, account scanning, and usage probes
  with a ledger, headroom and exhaustion tracking.
- Router: model catalog, eligibility, heuristic and Jev decider, selection and evaluation
  (`route eval`).
- Failover: a run that hits a quota, rate-limit or auth error, including one that exits while
  starting (for example agy's "Eligibility check failed"), moves to the next account or choice.
  Launch failures count as attempts, so failover is bounded and never returns to an account,
  and an auth failure flags every pool of the account.
- Codex choices are seeded enabled. A choice without a signed-in account is skipped at
  routing time, so adding a codex account makes `harness: auto` use it. Claude choices stay off
  by default. `doctor` warns about configs made by an older `init` that keep codex choices off.
- Custom agents defined by `AGENT.md`, with discovery, per-harness materialisation, starter
  agents and a list of gaps per harness.
- Workflows: a QuickJS engine with `agent`, `parallel`, `pipeline`, `phase` and `log`, a result
  cache with resume, stall restarts, usage-limit waits and saved workflows. The engine is wired
  to the supervisor: `run_workflow`, `workflow_status`, `workflow run|list|status|cancel` and
  `runs watch --workflow`.
- MCP stdio server that forwards tools to the supervisor. Clients that support MCP tasks get a
  task for `spawn_agent` with `wait`, `wait_agent` and `run_workflow`. Other clients block and
  get progress notifications. Questions from subagents surface through elicitation.
- Run messaging, `ask_parent`, a review gate, stall handling, resume after death, fork and
  notifications.
- Web UI served by the supervisor: dashboard, sessions, a session timeline with a run panel
  (brief, route decision, events, transcript, result, join command and actions), accounts with
  login flows, choices, agents, workflows and settings. `superorchestrator ui` starts it, and
  `ui --install-service` writes a systemd or launchd service.
- JSON API under `/api/v1` and a server-sent events stream at `/api/v1/stream`. API types are
  generated from the Rust types (`gen-ts`).
- `migrate`: a dry-run-first cutover from the old tooling, with backups.
- Inspection connections: `status`, `runs show`, `join`, `route` and `ui` open no session. A
  new `inspect` option of the `hello` handshake limits the connection to reads and run controls.
  A newer CLI talking to a supervisor started before this change falls back to the old
  handshake until that supervisor restarts.
- The shipped skill lists accounts, choices and agents, and is regenerated whenever they change,
  from the CLI and from the web API. Only an installed skill with our marker is touched.
- Claude Code plugin in `plugin/`: the `superorchestrator mcp` server and the skill.
- `scripts/smoke.sh`: a manual smoke test on a real machine, with an isolated config dir and
  wrappers.
- User documentation: README quickstart and guides under `docs/guide/`.
- Release pipeline: cargo-dist builds for Linux and macOS on x86_64 and aarch64 on version tags,
  and an `install.sh` installer that verifies sha256 checksums. `install.sh` points to the
  quickstart when it finishes.
- CI: lint, tests on Linux and macOS, Arch Linux tests, web checks and bundle size budget,
  coverage report, end-to-end smoke with fake harnesses, browser tests against a real, seeded
  supervisor (`pnpm -C web e2e:real`), shellcheck and actionlint.

### Fixed

- Web UI: dashboard crash against the real server, black timeline state colours on an empty
  session, a CSP console error from zod, and a `1970-01-01` headroom `observed_at` (now `null`).

[Unreleased]: https://github.com/datamonsterr/superorchestrator/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/datamonsterr/superorchestrator/releases/tag/v0.1.0
