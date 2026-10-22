---
name: use-superorchestrator-mcp
description: Delegate coding work to other agents, harnesses and accounts (claude, codex, opencode, agy) through the superorchestrator MCP tools. Use it when handing implementation, review or research work to another agent: route, spawn, wait on, message and review subagent runs.
---

# Delegate through superorchestrator

Use the `superorchestrator` MCP tools to hand work to subagents that run on other harnesses and accounts. This skill comes with the superorchestrator plugin.

## Accounts, choices and agents

Call `list_harnesses` for the accounts and choices on this machine, and `list_agents` for custom agents. Pass `agent: "<name>"` to `spawn_agent` to use one. `superorchestrator account add` and `superorchestrator init` set accounts and choices up.

Call `list_harnesses` for the live picture: signed-in state, headroom and active runs per account.

## route or spawn_agent

- `route` previews which choice and account the router would pick. It runs nothing. Use it when you want to see or sanity-check a decision first.
- `spawn_agent` starts a run. With `harness: auto` (the default) the router picks. Pass `account`, `model` or `effort` only when you have a reason to override it. Set `wait: true` to block until the run finishes; otherwise it returns a run id.

## Writing a task brief

The subagent starts with no context. Put in the `prompt`:

1. The goal in one or two sentences.
2. Acceptance criteria: observable checks that say the work is done (tests that must pass, commands that must succeed).
3. Out of scope: what it must not touch or decide.
4. Pointers: files, docs and constraints it needs.

Use `output_schema` when you need a structured final result, and `review: true` when you want to inspect the work before it counts as done.

## Waiting, messaging and reviewing

- `wait_agent(runs, timeout)` returns when a listed run finishes, asks a question or reports progress, or when the timeout ends.
- `get_run` shows state, recent events, the final result and the worktree.
- `send_message(run, text)` talks to a running run; sent to a finished run it resumes the session.
- `read_messages` returns unread messages for you. Answer a child's question with `send_message`.
- A run in `pending_review` waits for `accept_run` or `reject_run`.
- `cancel_agent` stops a run and its subtree.

## If you are a subagent

You run with a run id and must never wait for a person.

- `report_progress(step, status)` at each step.
- `ask_parent(question, options?)` for decisions and blockers. It blocks until answered.
- `read_messages` for messages from your parent.
- Finish with a final result, matching the schema if one was given.
- Call `spawn_agent` only when your depth is below 2.

## Checking on things

Run `superorchestrator status` for accounts, headroom and the supervisor, and `superorchestrator runs list` for runs. The `superorchestrator-cli` skill covers the rest of the command line.
