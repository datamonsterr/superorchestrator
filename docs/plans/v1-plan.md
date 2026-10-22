# Superorchestrator v1 implementation plan

Branch: `feat/v1`. Source of truth: [SPEC.md](../../SPEC.md), [CONTEXT.md](../../CONTEXT.md), [ADRs](../adr/).
Workflow: subagent-driven development, test-driven development inside each task.

## Cross-cutting decisions

These hold for every task. Record new decisions as ADRs.

- **Layout.** Cargo workspace at the repo root. Crates under `crates/` (`so-core`, `so-store`, `so-harness`, `so-accounts`, `so-usage`, `so-router`, `so-agents`, `so-workflow`, `so-supervisor`, `so-mcp`, `so-web`) and the binary crate `crates/superorchestrator`. Frontend in `web/`. Shared dependency versions in `[workspace.dependencies]`. Edition 2024, `rust-version` pinned in the workspace.
- **Pinned majors** (checked on crates.io 2026-10-22): rmcp 3.6, axum 0.8, rusqlite 0.40 (bundled), rquickjs 0.14, clap 4.6, tokio 1.54, reqwest 0.13 (rustls), rust-embed 8, dialoguer 0.12, schemars 1. Check current docs (context7 or docs.rs) before using any API.
- **Ids.** Session, run, event and message ids are UUID v7 strings. CLI commands that take a run id accept a unique prefix of at least 6 characters.
- **Errors.** Library crates use `thiserror`. The binary uses `anyhow`. No `unwrap()` outside tests.
- **Config dir.** Resolved from `SUPERORCHESTRATOR_CONFIG_DIR`, else `~/.config/superorchestrator`. Every test uses a temp dir through this variable or an explicit `Paths` value. Tests never touch the real home.
- **Supervisor protocol.** Newline-delimited JSON-RPC 2.0 over the unix socket. The first call on a connection is `hello{client, cwd, run_id?}`, which opens or attaches a session. Every MCP tool is one method of the same name with the same params. Request and response types live in `so-core::proto` so `so-mcp`, `so-web` and the CLI share them. Server-to-client notifications carry run events (`event`) for progress.
- **Harness drivers** implement one trait in `so-harness`. Every driver is tested against recorded fixtures replayed by a fake harness binary (`crates/so-harness/tests/fake-*`). CI never runs real harnesses.
- **Logging.** `tracing` with a redaction layer for key patterns (`sk-`, `sk-or-`, `ghp_`, bearer tokens).
- **Frontend.** React 19, Vite, TypeScript strict, Tailwind v4, shadcn/ui, TanStack Query, React Router. Vitest + Testing Library. The UI uses only `/api/v1`.

## Phase 0: Scaffold, hooks, CI

### T0.1 Cargo workspace skeleton (implementer-sonnet)
- Goal: workspace with every crate from SPEC 3.2 as a compiling stub, each with one dummy unit test; the binary prints `--version` and `--help` via clap.
- Acceptance: `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` pass. `rust-toolchain.toml` (stable, rustfmt, clippy), `rustfmt.toml`, `[workspace.lints]` (clippy pedantic off, `unwrap_used` warn outside tests). `.gitignore` updated.
- Out of scope: any real feature code.

### T0.2 Frontend skeleton (implementer-sonnet)
- Goal: `web/` with Vite + React + TS + Tailwind v4 + shadcn/ui initialised (`components.json`, `Button` added), one Vitest component test, ESLint, `pnpm` scripts `dev|build|test|lint|typecheck`.
- Acceptance: `pnpm -C web install --frozen-lockfile && pnpm -C web lint && pnpm -C web typecheck && pnpm -C web test && pnpm -C web build` pass. Vite dev proxy for `/api` to `127.0.0.1:7878`.
- Out of scope: real pages.

### T0.3 Git hooks, scripts and CI (implementer-sonnet)
- Goal: `.githooks/pre-commit` (cargo fmt check, web lint on staged web files), `.githooks/pre-push` (`scripts/check.sh`: fmt, clippy, cargo test, web lint/typecheck/test/build). `scripts/setup-hooks.sh` sets `core.hooksPath`. GitHub Actions `ci.yml`: Rust job matrix ubuntu-latest, macos-latest, Arch container; web job; `cargo deny`/audit optional job. Placeholder `release.yml` and `install.sh` (filled in T4.x).
- Acceptance: hooks run locally and pass; `actionlint` clean if available; CI YAML valid.
- Out of scope: release publishing.

## Phase 1: Backend MVP (SPEC M1–M5, M7, M8)

### T1.1 so-core: paths, config, types (implementer-sonnet)
- Goal: `Paths` (config dir, state.db, runs/, logs/, accounts/, agents/, workflows/, socket path per OS), `Config` with every field and default in SPEC 16 (serde, toml), duration parsing (`"5m"`, `"30m"`, `"5h"`, `"20s"`), `AccountFile` (SPEC 5.1) with name validation and reserved names, glossary types (`Harness`, `RunState` with the transitions in 7.1, `Choice`, `Headroom`, `HeadroomStatus`, `ErrorKind`), the `proto` module with JSON-RPC envelope and method param/result types for every tool in SPEC 13.
- Acceptance: unit tests for defaults, round-trip, name validation, state transition table, duration parse, `~` expansion.
- Out of scope: IO beyond reading/writing config files.

### T1.2 so-store: SQLite (implementer-sonnet)
- Goal: embedded migrations for every table in SPEC 17 (+ `schema_version`), WAL, busy timeout, typed queries: sessions, runs, attempts, events (append + range by session), messages, headroom upsert, ledger windows, flags, workflow cache/runs.
- Acceptance: tests on temp db for every query, migration idempotence, concurrent writer test, 10 000 event range query under 200 ms.
- Out of scope: business rules.

### T1.3 so-accounts (implementer-sonnet)
- Goal: account CRUD on toml files, environment per harness (SPEC 5.2), agy bwrap argv builder (ADR-0003, macOS single account rule), wrapper write/backup with marker (5.3), sign-in check by auth file existence per harness, `import --scan` detection (5.4), login command builder.
- Acceptance: unit tests with temp HOME; never reads auth file contents (test asserts only `exists`).
- Out of scope: running logins interactively.

### T1.4 CLI foundation (implementer-sonnet)
- Goal: clap tree for every command in SPEC 15 (unimplemented ones return a clear "not yet implemented" error until their task), `init` (seed config.toml, models.toml, choices from 8.2 defaults, starter agents placeholder), `config get|set|edit`, `account *`, `exec`, `doctor`, interactive menu with dialoguer when no args and a TTY.
- Acceptance: `assert_cmd` integration tests with temp config dir for init, config, account add/import/list/remove/enable/disable/flag, exec records an interactive session and execs a fake binary.
- Out of scope: supervisor-backed commands.

### T1.5 Supervisor core (implementer-sonnet)
- Goal: `daemon [--foreground]`, single-instance lock, socket `0600` in user-only dir, JSON-RPC server, session open on `hello`, idle exit (`supervisor.idle_exit`), client library `SupervisorClient` with auto-start + 5 s wait, reconcile on start (7.1 / ADR-0001), `status` CLI.
- Acceptance: integration tests: two daemons cannot start, client auto-starts daemon, idle exit with short config, reconcile marks orphaned runs failed.
- Out of scope: runs.

### T1.6 MCP thin client + install (implementer-sonnet)
- Goal: `mcp` stdio server with rmcp 3.x exposing all SPEC 13 tools (schemas from `proto`), forwarding to the supervisor; `list_harnesses` fully working; depth-restricted tool exposure (`report_progress`/`ask_parent` only when `SUPERORCHESTRATOR_RUN_ID` set); progress notifications; `install`/`uninstall` (claude mcp add, codex/opencode/agy config entries with backups, skill files 19.1).
- Acceptance: rmcp client test over stdio lists tools and calls `list_harnesses`; install test against temp HOME with fake `claude` binary.
- Out of scope: run tools beyond forwarding.

### T1.7 so-usage (implementer-sonnet)
- Goal: ledger windows, headroom record and status (6.2), exhaustion marking (6.3), codex probe via app-server `account/rateLimits/read`, agy `/usage` parser, probe TTL cache.
- Acceptance: parser tests on recorded outputs; status thresholds; exhaustion with and without reset time.
- Out of scope: routing.

### T1.8 Harness spikes (implementer-sonnet)
- Goal: answer SPEC 4 spikes on this machine and record fixtures: codex app-server schema + a short JSON-RPC transcript, claude stream-json transcript, opencode serve SSE transcript, agy stream-json transcript, `--max-turns`. Write `docs/adr/0004-harness-driver-findings.md`.
- Acceptance: fixtures committed under `crates/so-harness/tests/fixtures/<harness>/`, credentials scrubbed.
- Out of scope: driver code.

### T1.9–T1.12 Drivers: claude, codex, opencode, agy (implementer-sonnet each)
- Goal: driver trait (launch, events, send, cancel, resume, join_command, classify_error) and one driver per task, event normalisation to SPEC 12, structured output per 4 rules, transcripts.
- Acceptance: fake-harness replay tests: normal run, live message, cancel, resume, quota error classification.
- Out of scope: routing, supervisor wiring.

### T1.13 Run lifecycle (implementer-sonnet)
- Goal: supervisor run manager: spawn (explicit harness/account), worktree isolation (7.3), child instructions and MCP config (7.4), caps + FIFO queue (7.7), events/transcripts, `wait_agent`, `get_run`, `cancel_agent` (subtree), `join`, `runs list|show|watch|cancel`, `worktrees list|prune`.
- Acceptance: integration tests with fake harness via supervisor socket.
- Out of scope: routing, messages.

### T1.14 Communication, nesting, review, stalls (implementer-sonnet)
- Goal: SPEC 9 messages, `send_message` live/resume, `read_messages`, `report_progress`, `ask_parent` with timeout, elicitation hook, depth 2, `max_children`, `fork_agent`, review gate + checks (7.5), stall nudges/escalation (7.6), notifications (18).
- Acceptance: integration tests with fake harness.

### T1.15 Router (implementer-sonnet)
- Goal: catalog + `models refresh|list`, choices, heuristic score, Jev decider with validation and gate, account selection, `route`, `harness: auto`, failover (8.5), Claude rank (8.4), `route eval` with shipped cases.
- Acceptance: unit tests for score, validation rejects each bad case, gate, account ordering; wiremock test for Jev.

### T1.16 Custom agents (implementer-sonnet)
- Goal: SPEC 10 parsing, search order, materialisation per harness, `agent create|list|show|edit|remove`, starter agents, `list_agents`.

### T1.17 Workflows (implementer-sonnet)
- Goal: SPEC 11 in rquickjs, cache/resume, stalls, `workflow run|list|status`, `run_workflow`, `workflow_status`.

### T1.18 Migrate (implementer-sonnet)
- Goal: SPEC 19.2 dry run and `--apply` against a temp HOME.

### T1.19 MVP smoke with a real Claude session (controller)
- Register MCP into Claude Code for this repo, delegate a trivial task to each harness, read result, `join` a codex run.

## Phase 2: Web UI (SPEC M6)

### T2.1 so-web API + SSE (implementer-sonnet)
- `/api/v1` mirroring the CLI, `/api/v1/stream` SSE, token auth for non-loopback, rust-embed of `web/dist`, contract tests.

### T2.2 UI design (controller + implementer-sonnet)
- Design doc with page wireframes and shadcn component map; review before build.

### T2.3–T2.7 Pages (implementer-sonnet each)
- Dashboard; sessions list; session timeline Gantt + run panel; accounts + login flow; choices/routing, agents, workflows, settings. Component tests for the timeline.

## Phase 3: Integration

### T3.1 End-to-end suite
- Full supervisor + MCP + web with fake harnesses; scripts/smoke.sh for real machine.

## Phase 4: CI/CD and release

### T4.1 Release pipeline
- cargo-dist config, release workflow on tags, `install.sh`, frontend build embedded, changelog, versioning; CI jobs for e2e, coverage, security audit; first tag `v0.1.0`.
