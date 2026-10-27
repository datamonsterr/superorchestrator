---
name: use-superorchestrator-mcp
description: "Delegate coding work to agents on other harnesses and accounts via superorchestrator MCP tools: route, spawn, wait, review."
---

# Delegate via superorchestrator

## Workflow

1. `list_harnesses` first; inspect signed-in accounts, headroom and eligible choices. `list_agents` discovers agent profiles.
2. Leave `harness: "auto"` and model/effort unset initially; keep Claude accounts for the orchestrator. `route` previews a route.
3. Brief each subagent with goal, acceptance commands, out-of-scope items and file pointers; it has no conversation context.
4. `spawn_agent(prompt, agent: "implementer", cwd: "/repo", review: true, wait: false)`; use an existing agent profile. Default isolation is `worktree`.
5. `wait_agent(runs: [id], timeout_ms: 30000)` until finish or `pending_review`. Answer question updates with `send_message(run: id, text: "...", kind: "answer")`, then wait again.
6. At the review gate, inspect `get_run(run: id, diff: true)`; `accept_run(run: id, merge: true)` lands the branch, or `reject_run(run: id, feedback: "...")` continues fixes. Wait and review again.

Accounts, choices: `list_harnesses`. Agents: `list_agents`, then `agent: "<name>"`.

## Controls

| Tool / option | Use |
|---|---|
| `spawn_agent` | `agent` selects instructions/defaults; `cwd` selects the checkout. Subagent callers must stay inside their run worktree/cwd. |
| `review: true`, `checks: ["ci", "pr_threads"]` | Hold at the review gate after checks. `"cmd:<shell>"` checks require depth 0. Commit changes and leave a clean tree for merging. |
| `output_schema` | JSON Schema for the final result; include its required fields in the brief. |
| `wait: true` | Block until finish or review gate; `false` returns a run id for `wait_agent`. Progress or timeout means wait again. |
| `send_message(run, text)` | Steer an active run or resume a finished run. `read_messages` reads unread messages. |
| `fork_agent(run, prompt)` | Start a new run from a finished run's context. |
| Escalation | Use a new `spawn_agent` with a `model` ref from a higher level, e.g. `codex/gpt-6.1-sol/high`; use choices from `list_harnesses` and include prior findings. |
| `cancel_agent(run)` | Cancel the run and its subtree. |
| `get_run(run, verbose: true)` | Full record; `diff: true` adds the patch (cut at 64 KiB). |
| `accept_run` | Merge requires the original checkout on its original branch and both trees clean; conflicts leave the run at the gate. Without `merge`, returns branch and merge command. |
| `run_workflow(name: "review", args: {}, wait: false)` | Start saved JS; poll `workflow_status(id)`. Inline `script` requires depth 0; subagents may use saved names from the user config directory only. |

## Subagents

Orchestrator depth is 0; deepest run is depth 2. Spawn or start workflow steps only while depth < 2.
Subagents use `report_progress(step, status)` each step and `ask_parent(question, options?)` for decisions; never wait for a person. Finish with the requested result schema.
