# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - Unreleased

First release. Pending the `v0.1.0` tag.

### Added

- Routing decider: `inception/mercury-decide:free` is the default model, with
  `~typesafe/jev-latest` as an automatic fallback when the primary is unusable (transport
  error, timeout, 429, 5xx or a missing model; never for a rejected answer). Both take any
  OpenRouter id in `router.jev.model` and `router.jev.fallback_model`, or from
  `SUPERORCHESTRATOR_ROUTER_MODEL` and `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL` (env beats
  config beats default, `none` disables the fallback). `route --explain`, `doctor` and
  `route eval` show the models and their source, and records name the model that answered.
- A configured decider that cannot be reached now fails the call with an error that names the
  models, the cause and the next step (`route` exits non-zero, MCP `isError`, `spawn_agent`
  auto spawns nothing, `POST /route` 502, `route eval` counts an error).
  `router.jev.on_unreachable = "heuristic"` keeps the old degrade. No key still routes by
  heuristic. Recovery of an existing run (failover, queued reroute) never fails on a decider
  outage: it routes with the heuristic and records why.
- Held-out routing eval set (`crates/so-router/evals/holdout.jsonl`), kept separate from the
  tuning set. `route eval --json` reports each case's probabilities, fallback reason and
  answering model.
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

- Routing against live Jev. `router.policy` defaults are now `min_confidence = 0.45` and
  `min_margin = 0.15`, and the seeded choice descriptions and routing instruction describe the
  tiers in general terms. Live results (one run each, 2026-10-23): tuning set 87% on
  mercury-decide and 87% on jev-latest; held-out set 88% on mercury-decide and 75% on
  jev-latest; before the change the tuning set scored about 50% with most cases falling back to
  the heuristic. The heuristic breaks ties inside a tier by effort (73% tuning set, 50%
  held-out without a decider). Existing `config.toml` files keep their old choice descriptions.

- Web UI: dashboard crash against the real server, black timeline state colours on an empty
  session, a CSP console error from zod, and a `1970-01-01` headroom `observed_at` (now `null`).

- `account add` and `account import` refuse a name that would shadow an existing command: when
  the name resolves on `PATH` to an executable that is not one of our wrappers, they fail and name the
  path, so `account add codex --name ls` no longer hides `/usr/bin/ls`.

- `superorchestrator route fix a typo in README` works without quotes; the words are joined with
  spaces. `route eval` is unchanged.

- After `account add`, the sign-in hint for codex and opencode is now
  `superorchestrator account login <name>`. Claude and agy keep the wrapper hint, because they
  sign in inside the session. The JSON output gains `login.cli` next to `login.command`.

- The failover test that waits for a freed account no longer races the fake harness's stdin log.

[Unreleased]: https://github.com/datamonsterr/superorchestrator/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/datamonsterr/superorchestrator/releases/tag/v0.1.0
