# Superorchestrator

Read [SPEC.md](SPEC.md) for what to build, [CONTEXT.md](CONTEXT.md) for the terms, and [docs/adr/](docs/adr/) for decisions already made. Use the glossary terms exactly.

## Implementation workflow (overrides the global orchestrate rule)

The global `~/.claude/CLAUDE.md` rule that sends all implementation through the `orchestrate` skill does not apply in this repository. That skill no longer exists, and superorchestrator replaces it once SPEC milestone M3 ships.

In this repository:

- Implement through the `subagent-driven-development` skill in `.claude/skills/`. Write the plan first, with the goal, acceptance criteria and out-of-scope items for each task.
- The main session stays the controller. It plans, dispatches, reviews diffs and makes decisions. It does not edit source files itself.
- Subagents use only `claude-haiku-5-5` and `claude-sonnet-5-5`, both at medium effort:
  - `implementer-haiku`: mechanical tasks (1-2 files, complete spec)
  - `implementer-sonnet`: integration and judgment tasks
  - `spec-reviewer` and `code-quality-reviewer`: the two review stages
- Do not pass `model` or `effort` on Agent calls; the agent files pin both. `.claude/hooks/enforce-subagent-models.py` denies other models, other effort levels and `general-purpose` subagents.
- Work on a branch or worktree, never on `main` without the user's consent.

## Stack notes

- Rust workspace (tokio, rmcp, axum, rusqlite, clap, rquickjs) with a React + Vite + TypeScript frontend in `web/`.
- Check current docs before coding against any library or harness CLI. Do not write APIs from memory.
- Project skills are routed by OtherSkill (`otherskill.config.json`): the categories are backend/rust, protocols/mcp, frontend/react, devops, testing, security and others.
