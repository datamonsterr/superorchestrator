---
name: use-superorchestrator-mcp
description: "Delegate coding work to agents on other harnesses and accounts via superorchestrator MCP tools: route, spawn, wait, review."
---

# Delegate via superorchestrator

## Workflow

1. Delegate in one call: `spawn_agent(prompt)` (optional `cwd`, `agent`, `review: true`). Do not call `list_harnesses` or `route` first; routing, harness and account selection are automatic. A failed spawn returns an error naming the fix.
2. Reuse the agent profile for standing instructions and send a short goal, acceptance and file pointers; it has no conversation context. Keep accounts in `orchestrator.reserved_accounts` out of subagent pins.
3. `wait_agent(runs: [id], include_progress: false, timeout_ms: 1800000)`: batch active run IDs and prefer quiet mode with up to 30-minute timeout (progress still streams as notifications). Returns on finish, questions, escalation, or `pending_review`; call `get_run` only for missing detail or review. Answer question updates with `send_message(run: id, text: "...", kind: "answer")`, then wait again. Codex orchestrator: prefer `spawn_agent(wait: false)`, then wait with `timeout_ms` ≤ `1800000` (30 min); install writes `startup_timeout_sec = 30` and `tool_timeout_sec = 1860` in `~/.codex/config.toml` or `$CODEX_HOME/config.toml` (rerun `superorchestrator install` if missing); resume with `superorchestrator join <run>` (or the `join_command` from `get_run`).
4. At the review gate, inspect with CLI `superorchestrator runs show RUN --diff` (fall back to `get_run(diff: true)` if the CLI fails); `accept_run(run: id, merge: true)` lands the branch, or `reject_run(run: id, feedback: "...")` continues fixes. Wait and review again.

## Inspect: CLI first

Inspect via CLI first: `status`, `runs list`, `runs show`, `runs watch` (export `XDG_RUNTIME_DIR=/run/user/$(id -u)` if the CLI says not running). Use MCP `get_run`, `list_agents`, `read_messages`, `list_harnesses` only if the CLI fails, and without `verbose` unless needed.

Inspect setup with CLI: `superorchestrator status`, `levels list`, `agent list`. Use `agent: "<name>"` in `spawn_agent`.

## Controls

| Tool / option | Use |
|---|---|
| `spawn_agent` | `agent` selects instructions/defaults; `cwd` selects the checkout. Subagent callers must stay inside their run worktree/cwd. |
| `review: true`, `checks: ["ci", "pr_threads"]` | Hold at the review gate after checks. `"cmd:<shell>"` checks require depth 0. Commit changes and leave a clean tree for merging. |
| `output_schema` | JSON Schema for the final result; supply output_schema only when a caller consumes structured fields. |
| `wait: true` | Block until finish or review gate; `false` returns a run id for `wait_agent`. |
| `wait_agent` | Batch active IDs (`runs: [ids]`), quiet mode `include_progress: false`, 30-min max (`timeout_ms: 1800000`). Returns on finish, question, escalation, review gate; call `get_run` only for missing detail or review. |
| `send_message(run, text)` | Steer an active run or resume a finished run. `read_messages` reads unread messages. |
| `fork_agent(run, prompt)` | Start a new run from a finished run's context. |
| Escalation | Use a new `spawn_agent` with a `model` ref from a higher level, e.g. `codex/gpt-6.1-sol/high`; pick refs from `superorchestrator levels list` (CLI) and include prior findings. |
| `cancel_agent(run)` | Cancel the run and its subtree. |
| `get_run(run, verbose: true)` | Full record; `diff: true` adds the patch (cut at 64 KiB). |
| `accept_run` | Merge requires the original checkout on its original branch and both trees clean; conflicts leave the run at the gate. Without `merge`, returns branch and merge command. |
| `run_workflow(name: "review", args: {}, wait: false)` | Start saved JS; poll `workflow_status(id)`. Inline `script` requires depth 0; subagents may use saved names from the user config directory only. |

## Subagents

Orchestrator depth is 0; deepest run is depth 2. Spawn or start workflow steps only while depth < 2.
Subagents use `report_progress(step, status)` each step and `ask_parent(question, options?)` for decisions; never wait for a person. Finish with the requested result schema.
