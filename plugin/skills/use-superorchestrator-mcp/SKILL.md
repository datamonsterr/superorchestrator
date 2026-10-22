---
name: use-superorchestrator-mcp
description: Delegate coding work to agents on other harnesses and accounts via superorchestrator MCP tools: route, spawn, wait, review.
---

# Delegate via superorchestrator

## Workflow

1. `list_harnesses` first: signed_in, headroom, cooldown, load, eligible, free_models.
2. Pick. `harness` auto unless reason. Simple task: free opencode `model` or low `effort`. Keep Claude for orchestrator.
3. `prompt` = goal / acceptance (commands that must pass) / out-of-scope / pointers. Subagent has no context.
4. `spawn_agent`. Parallel: `wait: false`, then `wait_agent(runs, timeout_ms)`.
5. Review `get_run(run, diff: true)`, not file reads. Then `accept_run(run, merge: true)` or `reject_run(run, feedback)`.

Accounts, choices: `list_harnesses`. Agents: `list_agents`, then `agent: "<name>"`.

## Tools

- `route`: preview pick, runs nothing.
- `spawn_agent`: `wait: true` blocks to finish or review gate. `review: true` holds at `pending_review`. `output_schema`, `checks` (ci, pr_threads, cmd:<shell>).
- `wait_agent`: finished (state, result), updates, timed_out. Question: `send_message(run, text, kind: answer)`.
- `get_run`: short; `verbose: true` full.
- `send_message` resumes finished run. `read_messages` unread. `cancel_agent` run + subtree. `fork_agent` finished run, new prompt.
- `accept_run` merge refused (run stays at gate): checkout dirty or on other branch, worktree dirty, conflict. Error names it. No merge: branch + merge_command.
- `run_workflow`, `workflow_status`: JS workflows.
- Shell: `superorchestrator status`.

## Subagent

No person to wait for. `report_progress(step, status)` each step. `ask_parent(question, options?)` blocks for answer. Final result matches schema. `spawn_agent` only if depth < 2.
