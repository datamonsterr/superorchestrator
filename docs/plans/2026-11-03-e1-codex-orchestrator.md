# E1: Codex CLI as a first-class orchestrator

Branch: `claude/codex-orchestrator-0a0caa`. Parent plan: `docs/plans/2026-11-02-multi-harness-epics.md` (main checkout).

## Decisions

- **One plugin directory.** `plugin/` serves both Claude Code and Codex. Codex gets `plugin/.codex-plugin/plugin.json`, which reuses `plugin/.mcp.json` and `plugin/skills/`. Codex discovers `hooks/hooks.json` by default, and that file is the Claude Code mod module list, so the Codex manifest points `extensions["com.openai"].hooks` at `./hooks/codex-hooks.json`. An explicit value replaces default discovery. Recorded in ADR 0008.
- **Harness-neutral hooks.** Hook commands call a new `superorchestrator hook <event>` subcommand. It reads the hook JSON on stdin and prints hook JSON that Claude Code and Codex both accept. No shell or Python scripts.
- **Bounded waits.** `wait_agent` gets a maximum timeout (`MAX_WAIT`, 30 minutes) that the supervisor clamps to. `install` writes the Codex `tool_timeout_sec` as that maximum plus a 60 second margin, and `startup_timeout_sec = 30`.
- **Reserved accounts.** New config section `[orchestrator]` with `reserved_accounts = ["name", ...]`. Term "Reserved account" in CONTEXT.md. Auto routing never picks a reserved account. A spawn or route that pins `account` may use it.

## Tasks

### Task 1: Reserved orchestrator accounts and `setup --orchestrator`

Goal: config `orchestrator.reserved_accounts`; routing excludes them unless the account is pinned; `setup --orchestrator claude|codex` reserves accounts and sets account-count defaults.

- `so-core` config: new `Orchestrator { reserved_accounts: Vec<String> }` section, `deny_unknown_fields`, default empty, settable with `config set orchestrator.reserved_accounts`, JSON schema updated.
- `so-router`: reserved accounts never appear in `usable_accounts` for auto routing. A pinned `account` bypasses the reservation.
- `setup --orchestrator claude|codex`, plus repeatable `--reserve NAME`. No flag: today's behavior, nothing reserved.
  - `codex`: the codex default count is 2 when codex is installed and has no account (one reserved, one for subagents); claude follows the normal rule (1 when installed and none exist). Reserve the codex account whose home resolves to `$CODEX_HOME` or `~/.codex`, else the first codex account by name.
  - `claude`: claude default stays 0; reserve the first claude account when one exists.
  - `--reserve NAME` overrides the pick. Writes to `orchestrator.reserved_accounts` are idempotent (no duplicates, existing entries kept).
  - `--dry-run` JSON shows `orchestrator` and `reserve`.
- CONTEXT.md: add "Reserved account".
- Acceptance: `cargo test -p so-core -p so-router -p superorchestrator`, `cargo clippy --workspace --all-targets -- -D warnings`.
- Out of scope: web/TUI display (Task 5), docs pages, AGENTS.md.

### Task 2: Codex MCP timeouts, bounded wait, `~/.codex/AGENTS.md` block

- `wait_agent` timeout clamped to `MAX_WAIT` (30 min) in the supervisor. Export the constant where install can read it. If `spawn_agent(wait: true)` waits without bound, bound it the same way.
- `register::codex_add` writes `startup_timeout_sec = 30` and `tool_timeout_sec = MAX_WAIT secs + 60`; updates stale values; keeps other keys. Applies to `~/.codex/config.toml` and each codex account's `config.toml`.
- Managed block in `~/.codex/AGENTS.md` (respect `CODEX_HOME` for the global one; also each codex account home), same markers and functions as the CLAUDE.md block, generalized over the file. Opt-out `install --no-agents-md` and `setup --no-agents-md`. `uninstall` removes it. A symlinked AGENTS.md is written through, never replaced.
- Block body harness-neutral: no "Keep Claude accounts for the orchestrator"; say reserved orchestrator accounts and `harness: auto`.
- Acceptance: `cargo test -p superorchestrator -p so-supervisor -p so-mcp`, clippy clean. Temp-HOME install test asserts both timeouts and AGENTS.md create/update/remove idempotence.
- Out of scope: plugin files, skills text, reserved-account config.

### Task 3: Harness-neutral skills

- `so-agents/src/skill.rs`: the `use-superorchestrator-mcp`, `superorchestrator-cli` and `superorchestrator-config` texts stop assuming Claude is the orchestrator. Add short Codex notes: tool timeout and `wait: false` plus `wait_agent` with a `timeout_ms` under the limit, `codex resume`, `[mcp_servers.superorchestrator]` in `config.toml`, `orchestrator.reserved_accounts`.
- Regenerate `plugin/skills/*` (`UPDATE_PLUGIN_SKILL=1 cargo test -p superorchestrator --test plugin`).
- Acceptance: `cargo test -p so-agents -p superorchestrator --test plugin`.
- Out of scope: install block text, docs pages.

### Task 4: Codex plugin, marketplace, `superorchestrator hook`

- `plugin/.codex-plugin/plugin.json`: `name`, `version` (same as `.claude-plugin`), `description`, author, homepage, repository, license, keywords, `skills: "./skills/"`, `mcpServers: "./.mcp.json"`, `extensions["com.openai"]` with `interface` and `hooks: "./hooks/codex-hooks.json"`.
- `.agents/plugins/marketplace.json` at repo root: marketplace `superorchestrator`, one plugin `superorchestrator`, `source` local `./plugin`, policy, category.
- `plugin/hooks/codex-hooks.json`: `SessionStart` runs `superorchestrator hook session-start`; `Stop` runs `superorchestrator hook stop`.
- `superorchestrator hook session-start|stop`: reads hook JSON from stdin (`cwd`, `stop_hook_active`), asks the supervisor for open runs whose cwd lies in the same git repo, prints JSON. SessionStart: `hookSpecificOutput.additionalContext` listing runs at `pending_review` or with unanswered questions. Stop: `systemMessage` warning when runs wait at the review gate; never blocks; empty output when nothing waits or the supervisor is down. Always exit 0.
- `tests/plugin.rs`: manifest and marketplace parse and carry the documented fields; versions match.
- Real check: in a scratch `CODEX_HOME`, `codex plugin marketplace add <worktree>` succeeds.
- Acceptance: `cargo test -p superorchestrator`, clippy clean.
- Out of scope: Claude Code mod modules, install changes.

### Task 5: Reservation in web Settings and TUI (after Task 1)

- Web Settings shows and edits `orchestrator.reserved_accounts`; accounts list marks reserved accounts. TUI accounts view marks reserved accounts.
- Acceptance: `pnpm -C web lint typecheck test`, `cargo test -p superorchestrator -p so-web`.

### Task 6: Docs (after Tasks 1-4)

- `docs/guide/codex-orchestrator.md`, SPEC updates (install, config, hooks, plugin), ADR 0008, guide index links, CHANGELOG entry.

### Task 7: Real smoke (orchestrator)

- Scratch `CODEX_HOME` from a signed-in codex account: `codex plugin marketplace add <worktree>`; `codex exec` calls `list_harnesses`, spawns and waits on a trivial run through the installed supervisor. Transcript goes in the PR.
