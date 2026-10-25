# Superorchestrator

Read [SPEC.md](SPEC.md) for what to build, [CONTEXT.md](CONTEXT.md) for the terms, and [docs/adr/](docs/adr/) for decisions already made. Use the glossary terms exactly.

## Implementation workflow

Implementation goes through superorchestrator, the tool this repository builds.

- Implement through the `subagent-driven-development` skill in `.claude/skills/`. It dispatches each task through the superorchestrator MCP tools. Write the plan first, with the goal, acceptance criteria (the commands that must pass) and out-of-scope items for each task.
- Load the `use-superorchestrator-mcp` skill and call `list_harnesses` before the first dispatch.
- The main session is the orchestrator. It plans, briefs, dispatches, reviews diffs with `get_run(diff: true)`, and lands work with `accept_run(merge: true)`. It does not edit source files itself.
- Agent profiles for the workflow are in `.superorchestrator/agents/`:
  - `implementer`: one task in its own worktree, held at the review gate
  - `spec-reviewer` and `code-quality-reviewer`: the two review stages, read-only
- Leave `harness` at `auto` and pass no `model` or `effort` on the first dispatch; the router picks. Escalate with a model ref from a higher level. Never pick the claude harness or a Claude account for a subagent: Claude accounts belong to the orchestrator.
- Do not stop, restart or reinstall the installed superorchestrator while runs are open. Its supervisor runs every subagent, including the ones that change this code.
- Fallback when superorchestrator is unavailable: native subagents from `.claude/agents/`, on `claude-haiku-5-5` (`implementer-haiku`) and `claude-sonnet-5-5` (`implementer-sonnet`, `spec-reviewer`, `code-quality-reviewer`), both at medium effort. Do not pass `model` or `effort` on Agent calls; the agent files pin both. `.claude/hooks/enforce-subagent-models.py` denies other models, other effort levels and `general-purpose` subagents.
- Work on a branch or worktree, never on `main` without the user's consent.

## Stack notes

- Rust workspace (tokio, rmcp, axum, rusqlite, clap, rquickjs) with a React + Vite + TypeScript frontend in `web/`.
- Check current docs before coding against any library or harness CLI. Do not write APIs from memory.
- Project skills are routed by OtherSkill (`otherskill.config.json`): the categories are backend/rust, protocols/mcp, frontend/react, devops, testing, security and others.

## Builds and disk

The disk is small, and several agents build at once. Follow these rules:

- The main checkout builds into `~/.cache/cargo-target`, set by `build.target-dir` in `~/.cargo/config.toml`. Each git worktree gets its own target dir on the media disk: put an untracked `.cargo/config.toml` in the worktree with `[build] target-dir = "/media/storage/cache/cargo-target/<worktree>"` (`/.cargo/config.toml` is in `.git/info/exclude`). Never share one target dir between checkouts: workspace crates get the same artifact hash, so one checkout overwrites the other's builds. Do not set `CARGO_TARGET_DIR`. This covers the worktrees superorchestrator creates under `~/dev/superorchestrator-worktrees/`: the `implementer` profile writes the file before its first build, and the review-gate checks confirm it.
- sccache wraps rustc (`build.rustc-wrapper`). Its cache is on `/media/storage/cache/sccache`, capped at 20 GiB, oldest evicted first. Do not turn on incremental builds, because sccache cannot cache them.
- Dev and test profiles keep `debug = "line-tables-only"` and dependencies build without debug info. Do not raise these.
- While iterating, verify with `cargo check` (or `cargo clippy`), and `cargo test --no-run` for test code. Run `cargo test` for the crate you changed when the logic is complete. Run `cargo test --workspace` once before reporting.
- Tests that need a binary find the target dir from the test executable path, not from `CARGO_TARGET_DIR` or `<repo>/target`.
- Run at most one implementer per checkout. Concurrent implementers each get their own git worktree; superorchestrator runs get one by default.
