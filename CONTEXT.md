# Superorchestrator

Superorchestrator lets one coding agent delegate work to other coding agents (Codex, Antigravity, OpenCode, Claude) across many logged-in accounts, and records every delegation so it can be traced.

## Language

### Agents and work

**Orchestrator**:
The coding agent that the user drives directly and that delegates work through the MCP tools. It sits at depth 0.
_Avoid_: Parent session, main agent, controller

**Subagent**:
A coding agent started by superorchestrator on behalf of an orchestrator or another subagent.
_Avoid_: Worker, child agent, delegate

**Run**:
One subagent's life from spawn to finish, on one account, with one native session behind it. Runs form a tree through their parent run.
_Avoid_: Job, task, execution

**Session**:
One orchestrator's tree of runs, rooted at one connection to superorchestrator, such as one Claude Code conversation. An interactive launch through a wrapper is also a session, with no runs.
_Avoid_: Conversation, workspace, trace

**Native session**:
A harness's own resumable conversation behind a run, such as a Codex thread or a Claude session id.
_Avoid_: Thread, harness session, conversation id

**Depth**:
The number of superorchestrator spawn hops between a run and the orchestrator. The orchestrator is depth 0 and the deepest allowed run is depth 2. A harness's own built-in subagents do not add depth.
_Avoid_: Level, nesting level

**Supervisor**:
The one per-user superorchestrator process that owns every run's harness process and serves the web UI.
_Avoid_: Daemon, server, host, runner

### Harnesses and accounts

**Harness**:
A coding-agent CLI family that superorchestrator can drive: `codex`, `agy`, `opencode` or `claude`.
_Avoid_: Provider, backend, tool, CLI

**Account**:
One named login for one harness, backed by its own credential home, such as `codex-acc2`. An account is the unit that is routed, capped and metered.
_Avoid_: Profile, login, credential, worker

**Credential home**:
The folder that holds one account's login and harness state, selected through the harness's own environment variable.
_Avoid_: Home dir, config dir, profile folder

**Wrapper**:
The command on `PATH` named after an account that launches its harness with that account's credential home.
_Avoid_: Shim, alias, launcher

**Pass-through environment**:
The extra inherited environment variable names explicitly allowed by an account's
`pass_env` and the global `accounts.pass_env`, in addition to the baseline
allowlist of supervisor-launched processes. Values come from the supervisor's environment and are not saved in account config. Interactive CLI commands keep the operator's environment.

### Access and identity

**Run identity token**:
The random per-run secret that proves a connection belongs to one run. The supervisor stores only its hash, the run's `superorchestrator mcp` process presents it in `hello`, and it is rotated when the run is resumed.
_Avoid_: RPC key, run password, session token

**Inspect connection**:
A sessionless, runless connection to the supervisor socket that acts as the operator (the CLI, the terminal UI, the web UI). It is not scoped to a session or subtree, and it is the only kind that may call `ui_start`.
_Avoid_: Admin connection, root connection, operator session

**Login code**:
A single-use browser sign-in secret valid for 60 seconds, minted per `ui_start` call and carried in the login URL fragment. The browser exchanges it for the session cookie; it is never the persistent UI token.
_Avoid_: One-time token, magic link, session key

### Routing and usage

**Route**:
The ranked choice of account, model and effort for a piece of work, with a rationale.
_Avoid_: Decision, selection, placement

**Choice**:
One routable combination of harness, model and effort that the operator has approved, with a description of the work it suits. Routing picks a choice from the level that the task's complexity falls in, then picks an account for the choice.
_Avoid_: Option, route option, model config

**Complexity**:
The 0 to 100 estimate of how much reasoning, analysis and multi-step work a task needs. The decider scores it against fixed anchors, or the heuristic estimates it when no decider answers.
_Avoid_: Difficulty, size

**Level**:
A named, operator-defined range of complexity, with the choices allowed for tasks in that range or a model tier when it lists none. A level's choice list is not a Pool.
_Avoid_: Tier, bucket, band, pool

**Pool**:
A separately metered slice of one account's usage, such as agy's Gemini and Claude model families. Most accounts have one pool.
_Avoid_: Family, bucket, quota group

**Headroom**:
How much of an account's 5-hour and weekly usage windows remain.
_Avoid_: Quota left, capacity, budget

**Cap**:
The highest number of runs an account, or the whole system, may have active at once.
_Avoid_: Limit, concurrency limit, max sessions
