# Plan: README, release flow, tailnet UI, skills, TUI levels

Audit (2026-10-27): TUI has no levels management (Overview shows read-only Levels block); web UI guide has no screenshots; v0.1.0 tag is 149 commits behind origin/main and no GitHub release exists; plugin monitor exists in `plugin/` but is not installed for the user; `ui.bind = "0.0.0.0"` already skips the Host check (token is the guard).

## Task 1: README, web UI screenshots, tailnet docs
Goal: README is short, current and scannable; web UI guide has screenshots and a tailnet section.
- Build the web UI and the binary in the worktree, start the UI against a throwaway config dir (`--config-dir` under the scratchpad, `init` first, fake data only, never the user's real config or token), take screenshots with the global `playwright` of Dashboard, Sessions, Accounts, Levels (`/routing`) pages. Save as `docs/img/web-*.png`, each under 300 KB. No tokens, emails or real account names in them.
- `docs/guide/web-ui.md`: add the screenshots; add a "Reach it over a tailnet" section: `superorchestrator config set ui.bind 0.0.0.0`, restart supervisor, open `http://<hostname>:7878`, sign in with the token from the `ui-token` file (the `ui` command prints a 127.0.0.1 URL; say how to swap the host), warn that 0.0.0.0 also listens on LAN and show a firewall rule limiting it to `tailscale0`. No code changes.
- `README.md`: tighten; add a screenshot, a feature list, the Claude Code monitor (plugin, `/so-monitor`), the TUI, the release badge/link; keep install/quickstart. Keep glossary terms from CONTEXT.md.
- Out of scope: Rust code, skills, release docs.
- Acceptance: `scripts/check.sh --web-only` is not needed; run `git diff --stat`; all relative links in README.md and docs/guide/web-ui.md resolve (`test -f`); png sizes under 300 KB.

## Task 2: git release flow and skill
Goal: one documented, repeatable flow to cut a release, plus a project skill.
- Read `docs/release.md`, `scripts/check-release-tag.sh`, `CHANGELOG.md`, `.github/workflows/release*.yml`. Add a "Cut a release" section to `docs/release.md`: choose version, `CHANGELOG.md` section, bump `Cargo.toml` workspace version and `Cargo.lock`, run `scripts/check.sh`, PR to main, tag `vX.Y.Z` on main, push tag, watch the workflow (`gh run watch`), verify with `gh release view` and `install.sh`. Include how to roll back a bad tag.
- Add `scripts/release.sh` only if it removes real manual steps (bump + changelog check + dry-run print); default to dry run, never pushes unless `--push`. Skip it if not worth it.
- Add skill `.claude/skills/release/SKILL.md` (frontmatter name/description; under 60 lines; imperative steps; refuses to tag without user consent).
- Out of scope: cutting a release, changing workflows, other docs.
- Acceptance: `bash -n` on any new script; `scripts/check-release-tag.sh` untouched or still passes; markdown links resolve.

## Task 3: orchestrator skills
Goal: skills teach an orchestrator to drive the CLI, edit config and run subagents. Minimal, direct, accurate to the current CLI (`superorchestrator --help` and each subcommand's `--help` are the source of truth).
- Find where `install` ships skills (`crates/superorchestrator/src/` register/skill code, `plugin/skills/`, `~/.claude/skills/*`). Keep the plugin copy and the embedded copy consistent; the `use-superorchestrator-mcp` file is generated with a managed marker, edit its template, not the output.
- `superorchestrator-cli`: cover status, account add/login/enable/disable/remove, levels (list/set/add/remove models), `config get/set/edit`, `models`, `agent`, `workflow`, `runs list/watch/join`, `ui`, `tui`, `doctor`, `install`. One compact command table per topic.
- New skill `superorchestrator-config`: create a workflow (JS file in the config `workflows/` dir, with a minimal `parallel()`/`pipeline()` example verified against docs/guide/workflows.md), add an account, configure an agent (AGENT.md frontmatter fields from docs/guide/agents.md), modify routing and levels.
- `use-superorchestrator-mcp`: make it teach the full subagent loop: `spawn_agent` options, `wait_agent` question handling, `send_message`, `fork_agent`, `cancel_agent`, review gate, `run_workflow`, depth limit. Keep it short.
- Each SKILL.md under 70 lines; no prose that the `--help` output already gives.
- Out of scope: README, TUI, release, web.
- Acceptance: `cargo test -p superorchestrator`; `claude plugin validate plugin --strict` if `claude` is available; every command shown in a skill runs with `--help` without error.

## Task 4: TUI Levels tab
Goal: manage level routing from the TUI.
- Add tab 5 "Levels" (keys `1`..`5`) to `crates/superorchestrator/src/tui/`: list levels (name, min-max, models). Keys: move selection; open a level to see its model refs; `a` add a model ref (pick from the catalog), `x` remove one, `J`/`K` reorder, `r` edit range (min, max), `n` new level, `D` delete level (confirm by typing name), `u` reset to defaults (confirm). Reuse the same code path as the `levels` CLI command (`cmd/levels.rs`) so validation (no overlap, 0-100) is shared; show errors in the status line. Note config edits need a supervisor restart: show that hint after a change.
- Follow the existing TUI patterns (`app.rs`, `effects.rs`, `ui.rs`, snapshot tests in `tests.rs`). Update `docs/guide/tui.md` (tab table, keys) and the help overlay.
- Out of scope: web UI, other tabs' behaviour, restarting the supervisor.
- Acceptance: `cargo clippy -p superorchestrator --all-targets -- -D warnings`; `cargo test -p superorchestrator`; `cargo fmt --all --check`.
