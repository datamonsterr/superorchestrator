# E2: ACP harnesses (2026-11-03)

Design: [ADR-0011](../adr/0011-acp-harnesses-share-one-driver.md). New harnesses: `copilot`, `gemini`, `kimi`, `cursor`, `qwen`.

## Per-CLI facts (check against the installed CLI before coding)

| Harness | Binary, ACP launch | Credential home | Model, effort | Resume / join |
|---|---|---|---|---|
| copilot | `copilot --acp` (npm `@github/copilot` 1.0.105) | `COPILOT_HOME=<home>` | `--model M`; `--reasoning-effort none\|minimal\|low\|medium\|high\|xhigh\|max` | `copilot --resume=<id>` |
| gemini | `gemini --acp` (npm `@google/gemini-cli` 0.66.0) | `GEMINI_CLI_HOME=<home>` (state in `<home>/.gemini`) | `-m M`; no effort | `gemini --resume <id>` |
| kimi | `kimi acp` (npm `@moonshot-ai/kimi-code` 2.4.1) | `KIMI_CODE_HOME=<home>` | `-m M` if `acp` takes it, else `session/set_config_option`; no effort | `kimi --session <id>` |
| cursor | `agent acp` (official install script; not installed here) | `CURSOR_CONFIG_DIR=<home>`; if auth lives elsewhere, isolate `HOME` like agy | `--model M`; no effort | `agent --resume <id>` |
| qwen | `qwen --acp` (npm `@qwen-code/qwen-code` 0.26.6) | `QWEN_HOME=<home>` | `-m M`; no effort | `qwen --resume <id>` |

The npm packages are installed globally in `~/.npm-global/lib/node_modules/`, so their sources can be read to confirm flags, env names and the files that mark a signed-in home.

## Tasks

### Task 1: Harness variants, accounts and plumbing

- **Goal:** add the five `Harness` variants and make the whole workspace compile and pass with them. Covers so-core (`types.rs` enum, `str_enum!`, `accepted_efforts`, model-ref parsing, `account.rs`, `config.rs` priority and defaults, `proto.rs`), so-accounts (`env.rs` credential homes, `signin.rs` signed-in detection, `scan.rs`, `manage.rs` `LoginPlan`, wrappers) and so-store. Every other exhaustive `match` in the workspace gets the minimal arm: `AnyDriver::for_harness` returns `DriverError::Unsupported` for the new harnesses until Task 2, the default catalog has no entries for them until Task 3.
- **Acceptance:** `cargo test -p so-core -p so-accounts -p so-store`; `cargo check --workspace --all-targets`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --check`.
- **Tests first:** model-ref parsing for each harness (`copilot/gpt-5/high` accepted, `gemini/x/high` refused for effort), credential-home env per harness, signed-in detection per harness from a temp home, wrapper text, login plan.
- **Out of scope:** the driver, router catalog, web, docs.

### Task 2: AcpDriver and fake ACP agent

- **Goal:** `so-harness/src/acp/` with the shared `AcpDriver`, the `AcpSpec` table for the five harnesses, `AnyDriver`/`RunControl` variants and resume-id validation. A `fake-acp` test-support bin scripts an ACP agent.
- **Acceptance:** `cargo test -p so-harness`; `cargo clippy -p so-harness --all-targets -- -D warnings`.
- **Tests first (against `fake-acp`):** launch and session id; `session/update` mapped to SPEC 12 events; permission auto-allow; send while idle and mid-turn (queued); cancel; resume via `session/load`; structured output fallback (validate, re-prompt up to 5); error classification (-32000, quota text, refusal). Per-harness argv and join command from the spec table. One `#[ignore]` test that runs a trivial prompt through a real CLI chosen by env var with a scratch credential home.
- **Out of scope:** router, supervisor, web, migrating other drivers.

### Task 3: Router, usage, supervisor, agents, CLI and TUI

- **Goal:** the new harnesses are routable and usable end to end: so-router catalog entries and discovery, `heuristic.rs`, `eligible.rs`, `select.rs`; so-usage headroom unknown with cooldown from classified errors; so-agents `materialise.rs`/`parse.rs` (agent instructions as a prompt prefix, the rest per ADR-0011); so-supervisor `doctor.rs`, `handlers/harnesses.rs`, `runs/*`; so-mcp `tools.rs` harness lists; CLI `cmd/setup.rs`, `cli.rs`, `cmd/agent.rs`, `cmd/install.rs` (no MCP registration needed for runs); TUI `app.rs` and `effects.rs` harness lists.
- **Acceptance:** `cargo test -p so-router -p so-usage -p so-agents -p so-supervisor -p so-mcp -p superorchestrator`; clippy and fmt clean.
- **Out of scope:** driver internals, web.

### Task 4: Web

- **Goal:** regenerate `web/src/api/generated.ts` with `so-web/src/bin/gen_ts.rs`; the accounts and routing `schemas.ts`, `HarnessBadge.tsx`, `AddAccountDialog.tsx` and MSW fixtures know the new harnesses.
- **Acceptance:** `pnpm -C web test`, `pnpm -C web lint`, `pnpm -C web typecheck` (or the scripts CI runs); generated file matches.
- **Tests first:** vitest that the add-account dialog lists the five harnesses.
- **Out of scope:** Rust changes beyond the generator.

### Task 5: Docs

- **Goal:** SPEC 4 table rows and rules, SPEC 5.1/5.2 rows, CONTEXT.md harness list, `docs/guide/accounts.md` sections for adding each new harness account, README harness list.
- **Acceptance:** `scripts/` doc checks used by CI pass.
- **Out of scope:** code.

## Order

Task 1 first. Then Tasks 2, 3 and 4 in parallel (separate crates and files). Task 5 last. Then the real-CLI ignored test, `cargo test --workspace`, PR.
