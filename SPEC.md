# Superorchestrator specification

Status: accepted for implementation · 2026-10-22 · @datamonsterr

Superorchestrator is one Rust binary. It works as a CLI, as a stdio MCP server, as a per-user supervisor and as a local web UI. An orchestrator such as Claude Code uses it to delegate work to Codex, Antigravity (agy), OpenCode or another Claude, across many logged-in accounts. Every delegation is recorded so it can be traced, joined and debugged.

Terms used here are defined in [CONTEXT.md](CONTEXT.md). Architectural decisions are recorded in [docs/adr/](docs/adr/).

## 1. Goals and non-goals

### Goals

- Delegate work from any orchestrator to any harness and account through MCP tools. The router picks the account, model and effort, or the caller names them.
- Route by real usage. Read each account's 5-hour and weekly headroom, count active runs, and fail over when an account is exhausted.
- Manage many accounts per harness. Each account has a credential home under `$HOME`, a wrapper command on `PATH`, and the harness's own login flow.
- Let parent and child talk while a run is in progress. Allow nested runs up to depth 2.
- Make every run joinable with the harness's own CLI, live where the harness supports it, so a person can step in.
- Give each subagent the features of a native Claude Code subagent: agent files, tools, skills, MCP servers, memory, isolation, structured output. Also run Claude Code style workflow scripts.
- Provide a self-hosted web UI with account configuration, a usage dashboard, a session list, and a per-session timeline of all runs.
- Replace the existing local tooling: the `orchestrate` scripts, `coding-agent-management`, `check-account-usage`, and agent-dispatcher's `jev-router-mcp`.

### Non-goals (v1)

- Windows support. Linux (Ubuntu, Arch) is first class. macOS is supported, but agy there has only a single account (see 6.4).
- tmux workers or any visible terminal driven by superorchestrator.
- Forking or reading the orchestrator's own conversation.
- Mid-run user input inside workflow scripts. This matches Claude Code.
- Remote or multi-user hosting of the UI. The UI binds to loopback by default.
- PTY-driven TUIs. All harnesses are driven through their structured interfaces.

## 2. Platform and dependencies

| Item | Choice |
|---|---|
| Language | Rust (stable, 2024 edition), tokio |
| MCP | Official Rust SDK `rmcp` 3.x, targeting MCP spec `2026-07-28` and negotiating down to `2025-11-25`. Features: tools, tasks, progress, elicitation. |
| Storage | SQLite through `rusqlite`, WAL mode ([ADR-0002](docs/adr/0002-sqlite-is-the-system-of-record.md)) |
| HTTP (UI and API) | `axum`, server-sent events for live updates |
| Frontend | React + Vite + TypeScript, built at release time and embedded with `rust-embed` |
| Workflows | QuickJS through `rquickjs` |
| CLI | `clap`; `dialoguer` for prompts in plain commands; `ratatui` with `crossterm` for the terminal UI |
| HTTP client | `reqwest` (OpenRouter, models.dev) |
| Process isolation | agy accounts select their state with `--gemini_dir`; supervised agy runs use `bwrap` (Linux, optional) only for `--die-with-parent --new-session` |
| Distribution | GitHub `datamonsterr/superorchestrator`. Release binaries built by cargo-dist for linux x86_64/aarch64 and macOS aarch64/x86_64. An `install.sh` curl installer, with `cargo install` as a fallback. |

Before coding against `rmcp`, `axum`, `rquickjs` or any harness protocol, check the current documentation. The flags in this spec were checked against codex-cli 0.163.0, Claude Code 2.1.308, opencode 1.18.35 and agy 1.4.4 on 2026-10-22.

## 3. Architecture

```
 Claude Code / Codex / any MCP client (orchestrator, depth 0)
        │ stdio
        ▼
 superorchestrator mcp ──── unix socket ────► superorchestrator daemon (supervisor)
 (thin client, one per                          │  owns all harness processes
  orchestrator connection)                       │  routes, enforces caps, watches stalls
                                                  │  serves web UI + JSON API on 127.0.0.1:7878
                                                  ├─► codex app-server (per run, account CODEX_HOME)
                                                  ├─► opencode serve   (per run, account XDG dirs)
                                                  ├─► claude -p stream-json (per run, CLAUDE_CONFIG_DIR)
                                                  └─► [bwrap …] agy -p= stream-json (per run, --gemini_dir)
                                                        │
                                each child has its own superorchestrator mcp,
                                with SUPERORCHESTRATOR_RUN_ID set, so it can
                                message its parent and spawn children
                                                  │
                                           state.db (SQLite, WAL)
 wrapper codex-acc2 ─► superorchestrator exec ─┘ (records interactive sessions)
```

### 3.1 Processes

- **`superorchestrator mcp`.** A stdio MCP server started by the MCP client. It is a thin client: each tool call is forwarded to the supervisor over `$XDG_RUNTIME_DIR/superorchestrator.sock` (on macOS, `~/Library/Caches/superorchestrator/sock`). If no supervisor is running, it starts one and waits up to 5 s for the socket. At startup it opens a **session**. A session has no run id, unless `SUPERORCHESTRATOR_RUN_ID` is set, in which case the process acts for that run.
- **Supervisor (`superorchestrator daemon`).** One per user ([ADR-0001](docs/adr/0001-per-user-supervisor-owns-runs.md)). It holds a single-instance lock file. It owns every run's harness process, the router, caps, the usage probes, stall detection, notifications and the web UI. It exits after `supervisor.idle_exit` (default 10 min) with no active runs, no connected `mcp` clients and no open UI streams. On restart it reconciles `state.db`: runs marked active whose process is gone are resumed from their native session when the harness supports resume, and marked `failed` otherwise.
- **`superorchestrator exec <account> -- <args>`.** Called by wrappers. It writes a session row (kind `interactive`) and a pid, sets the account's environment, then `exec`s the real binary. If `state.db` is unavailable, it still launches the binary. It never blocks on the supervisor.
- **`superorchestrator ui`.** Ensures the supervisor runs and opens the browser at the UI URL. `ui --install-service` writes a systemd user unit (Linux) or a launchd agent (macOS) that keeps the supervisor running.

### 3.2 Modules (Cargo workspace)

| Crate | Responsibility |
|---|---|
| `so-core` | Config loading, paths, glossary types (Session, Run, Account, Choice), errors |
| `so-store` | SQLite schema, migrations, queries, event append |
| `so-harness` | One driver per harness: launch, event parsing, message injection, usage extraction, cancel, resume, join command |
| `so-accounts` | Account files, credential homes, wrappers, login flows, sign-in check, import |
| `so-usage` | Active probes, passive capture, local ledger, headroom per (account, pool) |
| `so-router` | Catalog, choices, heuristic scorer, Jev decider, account selection, failover |
| `so-agents` | Agent file parsing and per-harness materialisation |
| `so-workflow` | QuickJS runtime, script API, result cache |
| `so-supervisor` | Run lifecycle, caps, stall watcher, notifications, socket API |
| `so-mcp` | rmcp server, tool definitions, tasks and progress |
| `so-web` | axum API, SSE, embedded frontend |
| `superorchestrator` | Binary: clap CLI and terminal UI |
| `web/` | React frontend source |

## 4. Harness drivers

Each driver implements: `launch(run) -> Handle`, an `events()` stream normalised to the event types in section 12, `send(message)`, `cancel()`, `resume(native_session_id)`, `join_command(run) -> argv`, and `classify_error(text) -> {quota, rate_limit, auth, config, other}`.

| Harness | Launch | Native session id | Live message to child | Join |
|---|---|---|---|---|
| codex | `codex app-server --listen ws://127.0.0.1:<port> --ws-auth …`, one per run, with the account's `CODEX_HOME`. The supervisor starts a thread with `cwd`, `model`, `model_reasoning_effort`, sandbox `danger-full-access` and approval `never`. | thread id from `thread/started` | `turn/start` when idle, `turn/steer` while a turn runs, `turn/interrupt` to cancel | Live: `codex-acc2 resume --remote ws://127.0.0.1:<port> --remote-auth-token-env SO_WS_TOKEN <thread>`. After the run: `codex-acc2 resume <thread>`. |
| opencode | `opencode serve --hostname 127.0.0.1 --port 0`, one per run, with `OPENCODE_SERVER_PASSWORD` and the account's XDG dirs. A session is created with `POST /session`. | opencode session id | `POST /session/{id}/prompt_async`. Cancel: `POST /session/{id}/abort`. Events: `GET /event` (SSE). | Live: `opencode attach <url> --session <id>`. After the run: `opencode-acc1 --session <id>`. |
| claude | `claude -p --input-format stream-json --output-format stream-json --dangerously-skip-permissions --session-id <uuid> --mcp-config <run>/mcp.json --append-system-prompt-file <run>/system.md`, with the account's `CLAUDE_CONFIG_DIR` | the `--session-id` uuid | NDJSON user message on stdin | After the run pauses or ends: `claude-acc1 --resume <uuid>`, from the run's cwd |
| agy | `[bwrap --die-with-parent --new-session --bind / / --dev-bind /dev /dev --] agy --gemini_dir=<run>/gemini -p= --input-format stream-json --output-format stream-json --dangerously-skip-permissions --model M --effort E` | conversation id from the stream | NDJSON user message on stdin | After the run pauses or ends: `agy-acc1 --conversation <id>` |

Rules:

- **Full permissions.** Children run with every tool allowed. The run's system instructions (section 7.4) tell the child never to wait for a person, and to use `ask_parent` instead.
- **Structured output.** Use `codex` `outputSchema` on the turn, `claude --json-schema` and `agy --json-schema`. opencode has no schema option: superorchestrator validates the final message and re-prompts up to 5 times.
- **Model and effort.** Codex uses `model` plus `model_reasoning_effort`. agy uses `--model` plus `--effort` (low, medium, high, xhigh, max). Claude uses `--model` plus `--effort`. OpenCode uses the model id plus a provider `variant`, where the model has one.
- **`join`.** `superorchestrator join <run>` prints the join command, or runs it with `--exec`. Co-drive: the child keeps working while a person is attached, and both see the same native session. The timeline records `user_joined` and `user_left` where the harness reports them, otherwise `join_command_issued`.
- **Spikes (M3).** These need checking on a real machine before the driver is final:
  - How agy loads per-run MCP servers without `--mcp-config` (candidates: `agy mcp add` scoped to the run's `.gemini`, or a project-scope config in the run cwd).
  - The agy conversation id field in `stream-json`.
  - How opencode takes per-run MCP and agent config (`OPENCODE_CONFIG` or the project `opencode.json`).
  - The Codex `thread/start` parameter names in the app-server schema (`codex app-server generate-json-schema`).
  - Whether `claude --max-turns` exists in the installed version.

## 5. Accounts

### 5.1 Model

An account is one login for one harness. It is defined by `~/.config/superorchestrator/accounts/<name>.toml`:

```toml
name = "codex-acc2"
harness = "codex"            # codex | agy | opencode | claude
home = "~/.codex-acc2"       # credential home
binary = "codex"             # resolved on PATH at launch; absolute path allowed
priority = 0                 # tie-breaker inside a harness, higher first
cap = 2                      # max concurrent runs, interactive sessions included
enabled = true
pools = ["default"]          # agy: ["gemini", "claude"]
```

Account names match `^[a-z][a-z0-9-]{1,31}$`. The name doubles as the wrapper command name, so it must not shadow a real binary (`codex`, `agy`, `opencode`, `claude`).

### 5.2 Credential homes and environment

| Harness | Default home for `add` | Selected by |
|---|---|---|
| codex | `~/.<name>` | `CODEX_HOME=<home>` |
| opencode | `~/.<name>` | `XDG_CONFIG_HOME=<home>/config`, `XDG_DATA_HOME=<home>/data`, `XDG_STATE_HOME=<home>/state` |
| claude | `~/.<name>` | `CLAUDE_CONFIG_DIR=<home>` |
| agy | `~/.<name>` holding `.gemini/` | `--gemini_dir=<home>/.gemini` (a per-run gemini dir for supervised runs), with `HOME` unchanged ([ADR-0003](docs/adr/0003-agy-accounts-isolate-gemini-dir.md), [ADR-0005](docs/adr/0005-agy-runs-use-per-run-gemini-dir.md)) |

Credential homes live in `$HOME`, so a person can log in with the normal CLI through the wrapper. Superorchestrator only links to them.

### 5.3 Wrappers

`superorchestrator account add` and `import` write `~/.local/bin/<name>` (configurable with `accounts.wrapper_dir`):

```sh
#!/bin/sh
exec superorchestrator exec codex-acc2 -- "$@"
```

`exec` applies the environment from 5.2, records an interactive session, and execs the binary. Wrappers that already exist and are not owned by superorchestrator are moved to `~/.config/superorchestrator/wrappers.bak/` first. A wrapper owned by superorchestrator carries a marker comment.

### 5.4 Commands

- `account add <harness> --name <name> [--home PATH]`: creates the home, writes the toml and the wrapper, then runs the login.
- `account import <name> --harness H --home PATH`: links an existing home without copying credentials.
- `account import --scan`: detects the existing layout and offers an import for each account found:
  - `~/.codex`, `~/.codex-accN`
  - `~/.agent-farm/profiles/agy_worker_N/.gemini`
  - `~/.local/share/opencode`
  - `~/.claude`
- `account login <name>`: runs the harness's login with the account's environment. Codex uses `codex login --device-auth` when no TTY is attached. Other harnesses print the command to run in a terminal.
- `account list`: shows name, harness, signed in, enabled, cap, active runs, and headroom per pool.
- `account remove <name>`: asks the user to type the name, unless `--yes` is passed. Removes the toml and the wrapper. Deletes the credential home only with `--purge`.
- `account enable|disable <name>`
- `account flag <name> [--pool P] --until <time>|--hours N|--clear`: manual cooldown.

The sign-in check only tests that the harness's auth file exists. Auth file contents are never read into prompts or logs, never printed and never served.

## 6. Usage and headroom

### 6.1 Sources

| Source | Harness | When |
|---|---|---|
| Active probe: app-server `account/rateLimits/read` (no inference) | codex | Before routing, if the cached value is older than `usage.probe_ttl` (default 5 min) |
| Active probe: `agy --gemini_dir=… --print /usage`, parsed per pool (`gemini`, `claude`) and window (`five_hour`, `weekly`) | agy | Same |
| Passive capture: rate-limit fields in run events | codex | During every run |
| Local ledger: runs and tokens in a rolling 5 h and 7 d window | all | Always. The only source for opencode and claude. |

### 6.2 Headroom record

Headroom is recorded per (account, pool) with these fields:
- `five_hour_remaining_pct`, `five_hour_resets_at`
- `weekly_remaining_pct`, `weekly_resets_at`
- `status` (`ok`, `low`, `exhausted`, `unknown`)
- `source` (`probe`, `event`, `ledger`), `observed_at`

`status = low` when either window is below `usage.low_pct` (default 15). An account or pool is skipped by the router when its status is `exhausted`, it is flagged, or its active run count is at its cap.

### 6.3 Exhaustion

When a run fails with a quota, rate-limit or auth error (`classify_error`), the (account, pool) is marked exhausted until the reported reset time. When no reset time is known, it is marked for `usage.default_cooldown` (default 5 h). The run then fails over (section 8.5).

### 6.4 macOS

agy multi-account is Linux-only until it is checked on a Mac (ADR-0005). On macOS, superorchestrator allows only one enabled agy account, and it uses the real `~/.gemini`. `doctor` reports this.

## 7. Runs

### 7.1 States

```
queued ─► starting ─► running ⇄ waiting
                         │  ⇄ stalled ─► escalated
                         ▼
                 pending_review ─► succeeded
                         │ reject_run
                         └────────► running
 any active state ─► failed | cancelled
```

- `waiting`: blocked in `wait_agent` on its children, or blocked in `ask_parent`.
- `stalled`: no event for `runs.stall_ms` (default 300 000).
- `escalated`: still stalled after 3 nudges.
- `pending_review`: only when the spawn set `review: true`. Otherwise a finished run goes straight to `succeeded`.

### 7.2 Spawn options

`spawn_agent` takes:

| Field | Meaning |
|---|---|
| `prompt` | Task brief (required unless `agent` supplies one) |
| `agent` | Custom agent name (section 10) |
| `harness` | `auto` (default), or a harness name |
| `account` | Account name, or omitted for the router's choice |
| `model`, `effort` | Overrides |
| `cwd` | Working directory, default the caller's cwd |
| `isolation` | `worktree` (default) or `none` |
| `output_schema` | JSON Schema for the final result |
| `review` | `true` puts the run in `pending_review` |
| `checks` | Verify steps run before `pending_review`: `"ci"`, `"pr_threads"`, `"cmd:<shell command>"` |
| `wait` | `true` blocks until the run finishes (an MCP task when the client supports tasks) |
| `max_turns`, `timeout` | Budget. Claude uses the CLI flag where available. Other harnesses get a turn or time budget enforced by superorchestrator. |
| `name` | Short label for the UI |

Model selection order: call argument, then agent file, then `config.toml` default, then router.

### 7.3 Worktree isolation

Worktree isolation is the default. The supervisor creates `git worktree add -b <branch_prefix><slug>-<id> <dir> HEAD`, where:
- `dir = worktrees.dir` with `{repo_parent}` and `{repo}` expanded; default `{repo_parent}/{repo}-worktrees/<slug>-<id>`
- `branch_prefix` defaults to `so/`

Rules:
- When the cwd's tree is dirty, the parent is warned that uncommitted changes are not carried over.
- When `cwd` is not a git repository, isolation falls back to `none`, with a warning.
- `accept_run` with `merge: true` lands the branch in the base checkout (7.5).
- After the run: a worktree with no commits and a clean tree is removed. Otherwise it is kept until `accept_run`, after which it is removed when its branch is merged or pushed, or until `superorchestrator worktrees prune`.

### 7.4 Child instructions

Every run gets a system addendum, appended in each harness's own way. It covers:
- its run id, depth and parent
- that it must never wait for a person
- to use `ask_parent` for decisions and blockers
- to call `report_progress(step, status)` at each step
- to finish with a final result, matching the schema when one is given
- that it may call `spawn_agent` only when its depth is below 2

The child's MCP config adds `superorchestrator mcp`, with `SUPERORCHESTRATOR_RUN_ID` and `SUPERORCHESTRATOR_SOCKET` set.

### 7.5 Review gate

A run with `review: true`:
1. Runs its `checks` when it finishes:
   - `ci`: `gh pr checks` green for the linked PR
   - `pr_threads`: no unresolved review threads and no outstanding `CHANGES_REQUESTED`
   - `cmd:`: the command exits 0 in the run cwd
2. Enters `pending_review` when the checks pass. A failed check is sent back to the child as a message once, then the run is reported to the parent.
3. Waits for the parent. The parent reviews the work with `get_run(run, diff: true)` (7.5 notes). `accept_run` marks it `succeeded`. `reject_run(feedback)` sends the feedback on the same native session and returns the run to `running`.

Notes on the implementation:
- `cmd:<shell>` runs with `sh -c` in the run's worktree (or cwd) with a 10 minute limit, in its own process group (killed whole on timeout or cancel), with at most 64 KiB of output kept. The command is chosen by the orchestrator in the `checks` of its `spawn_agent` call; it is never built from task text or run output. It runs with the daemon's privileges, outside the harness sandbox, so treat `checks` like any command you would run yourself.
- `ci` and `pr_threads` use `gh` against the pull request of the run's branch. `ci` is green when every check is SUCCESS, SKIPPED or NEUTRAL.
- At `pending_review` the harness process is stopped and its slot is freed; the native session stays so `reject_run` (or `send_message`, which counts as a rejection) can resume it. A resume claims a slot like a new start; with none free the run goes back to `queued` and resumes when one frees. A worktree run resumes in its worktree. If the directory is gone it is recreated from the run's branch; if the branch went with an untouched worktree, it is recreated at the run's base commit. A run that still cannot be resumed keeps its state and the caller gets the error.
- `accept_run` removes the run's worktree and branch when the branch is merged into the repository's `HEAD` or contained in a remote branch, and the tree is clean (7.3). Otherwise the worktree stays until `worktrees prune`.
- `get_run` with `diff: true` adds `diff = {base, branch, stat, patch, truncated}` for a run with a worktree: `base` is the merge base of the run's branch, `patch` is `git diff <base>` of the worktree (committed and uncommitted changes) plus untracked files, cut at 64 KiB (`truncated: true`). It is absent for a run without a worktree, and carries `error` instead (with empty fields) when it cannot be made, so `get_run` itself does not fail. Git runs with `--no-ext-diff --no-textconv` and is stopped at the cap rather than read to the end; at most 100 untracked files are listed (`(+N more files)` after that); binary files show as git's `Binary files ... differ`. The web API's run view omits it.
- `accept_run(run, merge)` with `merge: true` also lands the work. The run must have a worktree branch; the base checkout (the repository the run was spawned from) must be on the branch it was on at spawn (recorded in `worktree_created.base_branch`; for older runs, any branch that contains the run's base commit) with no changes to tracked files (untracked files are fine). Work the run left uncommitted in its worktree (tracked or untracked, `.gitignore` respected) is first committed there with `git add -A` and the message `<run title> (<short run id>)` (the title is sanitised like the merge commit's), using the repository's git identity or `-c user.name=superorchestrator -c user.email=superorchestrator@localhost` when it has none; the repository's hooks run for it under the same no-prompt environment and 120 second limit as the merge, and a hook failure or timeout fails the call with the hook's output, leaves the run `pending_review` and merges nothing. The result's `note` then starts `committed uncommitted work as <sha>`. The branch is merged with `git merge` (fast-forward when possible, otherwise a merge commit with the message `merge: <run title> (<short run id>)`), then the worktree and branch are removed as above. On a refusal or a conflict (the merge is aborted) the call fails with an `INVALID_REQUEST` error that names the branch and, for a conflict, the files; nothing changes and the run stays `pending_review`. With isolation `none`, or when the run has no branch left, `merge` is a successful no-op with `merged: false` and a `note`. The result is `{ok, merged, branch?, merge_command?, note?}`; when the work is not merged and its branch exists, `branch` and `merge_command` (`git -C <repo> merge <branch>`) say how to land it. Details: the merge is refused while a merge, rebase, cherry-pick or revert is in progress in the base checkout (it is never aborted, because it is the person's); only a merge this call started is aborted, and nothing is ever reset or discarded: after a killed merge the error says what state the checkout is in and gives `git -C <repo> status`, `merge --abort` and `reset --merge` to inspect or undo it (a stale `index.lock` is removed only when it is newer than the merge and no other git process runs, otherwise reported). A `send_message` to a run at the gate takes the same per-run turn as accept and reject. Merges into one repository run one at a time, and `accept_run` and `reject_run` of one run take turns (the second sees the new state). The merge runs in its own task, so a dropped request cannot leave it half done. **Hooks:** the repository's own git hooks run (`pre-merge-commit`, `post-merge`, signing), because the merge happens in the person's repository. The merge has no stdin, `GIT_EDITOR=true`, `GIT_TERMINAL_PROMPT=0` and `GIT_ASKPASS`/`SSH_ASKPASS=true`, and a 120 second limit; on a timeout the process group is killed and the error says `merge timed out (hooks or signing?)`. A hook that refuses the merge gives an error with the hook's output, and the merge is undone. A held `index.lock` gives `another git operation is running in <repo>; try again`. A run whose base checkout was on a detached `HEAD` is never merged: the error gives the command to run by hand. The result's `note` says whether the merge was a fast-forward or a merge commit. Nothing is merged without the flag, and nothing is pushed. The web UI accepts without `merge`.
- Failed checks are sent to the child once. If they fail again the run still goes to `pending_review`, and the parent gets a `result` message that lists the failed checks.

### 7.6 Stall handling

When a run is `stalled`:
1. The supervisor sends a nudge message ("Status? Continue, or call ask_parent if blocked.").
2. Up to 3 nudges are sent, `stall_ms` apart.
3. After that the run is `escalated`: the parent gets a message, the UI highlights the run, and a notification fires.

A run whose process died unexpectedly is resumed once from its native session, if the harness supports resume. Otherwise it is marked `failed`.

### 7.7 Limits

- Depth: the orchestrator is 0, and runs exist at depth 1 and 2. Fixed at 2, not configurable. `spawn_agent` from a depth-2 run returns an error.
- Caps: per account `cap` (default 2), and `limits.global_cap` (default 16) over all active runs. Spawns over a cap are `queued`, in FIFO order per session.
- `limits.max_children` per run (default 8).
- `cancel_agent` cancels the run and all of its descendants.

## 8. Routing

### 8.1 Catalog

`~/.config/superorchestrator/models.toml` lists known models per harness, with `tier` (`high`, `mid`, `fast`), supported efforts, `pool` (agy), and `cost` (`free`, `included`, `paid`). A default catalog ships in the binary. `superorchestrator models refresh` collects models from:
- codex app-server `model/list`
- `opencode models`, plus free models from models.dev, cached 24 h
- `agy models`

The catalog is used only for discovery and validation. `models refresh` reports models that no choice uses, and never edits choices.

### 8.2 Choices

Choices are the routable set, in `config.toml`. A choice binds harness, model and effort atomically, so the router can never produce an unsupported combination.

```toml
[[choices]]
id = "agy-flash-medium"
harness = "agy"
model = "gemini-3.8-flash-medium"
effort = "medium"
roles = ["worker"]
description = "Fast worker for bounded implementation, scripts, docs and routine UI work."
enabled = true
```

`init` seeds the choices from agent-dispatcher's current `config.example.json`, plus Codex and Claude choices. Codex choices are enabled: a choice with no usable account is not eligible (8.3), so they start routing when a codex account is added. Claude choices are disabled by default (8.4); set `enabled = true` to let auto routing use them as the last resort. `enabled` is the user's switch and `init` is the only thing that writes it.

### 8.3 Decision procedure (`route`)

1. **Eligible choices.** Keep choices that are enabled, match the role (`worker` by default), and have at least one account with headroom for their pool and below its cap. Drop choices the call excludes.
2. **Decider.** If an OpenRouter key is configured (`router.jev.key_env`, default `OPENROUTER_API_KEY`, or `router.jev.key_file`), call `POST https://openrouter.ai/api/alpha/decisions` with model `router.jev.model` (default `inception/mercury-decide:free`). The model is any OpenRouter model id, set in `config.toml` or by `SUPERORCHESTRATOR_ROUTER_MODEL`; the environment beats the config, which beats the default, and an empty value counts as unset. `router.jev.fallback_model` (default `~typesafe/jev-latest`, env `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL`, `none` disables it) is tried only when the primary model is unusable: a transport error, a timeout, HTTP 429 or 5xx, or HTTP 400/404 for a missing model. A 429, 5xx, 408 or refused connection is retried once first; a timeout is not retried, because it uses up the time left for that model. Other 4xx responses, invalid or oversized bodies and a disallowed endpoint (plain http to a non-loopback host) are not about the model: they go to the heuristic. A refused key (401/403) and a rejected answer never reach the fallback. Both calls together stay within `router.jev.timeout`; with a fallback the primary may use 3/5 of it. `route --explain`, `doctor` and the eval summary show the models in effect and where each came from:
   - `state`: `task_content`, `system_instruction`, `routing_instruction`, `role`, `context`. The context carries `quota` (remaining % per family and window) and `usage_5h` (attempts per family and account).
   - `questions.route`: `type: "choice"`, the routing `instructions`, and `criteria` mapping each eligible choice id to `description; harness=…; model=…; effort=…`.
3. **Validation.** Reject the answer unless all of these hold:
   - `type` is `choice`
   - the choice is eligible
   - `confidence` is a finite value in [0, 1]
   - the probabilities cover exactly the eligible set and sum to 1 ± 0.02
   - the chosen id has the highest probability

   A response over 1 MB is rejected. A decision is accepted only if `confidence ≥ router.policy.min_confidence` and the margin between the top two probabilities is `≥ router.policy.min_margin`.
4. **Heuristic.** Used when there is no key, when validation or the gate rejects the answer, or when the decider is unreachable and `router.jev.on_unreachable = "heuristic"`. With the default `on_unreachable = "error"`, a configured decider that cannot be reached after its fallback (transport error, timeout, 5xx, 429, a missing model, or a refused key) fails the call instead: `route` exits non-zero, the MCP tool returns `isError`, `spawn_agent` with `harness: auto` spawns nothing, `POST /route` returns 502 and `route eval` counts an error. The message names the models, the cause and the next step, never the key. With no key at all, routing uses the heuristic and records `fallback_reason`:

   `score = fit + w_headroom·headroom − w_load·active_runs − w_cost·cost + priority`

   - `fit` compares task complexity with model tier. Complexity is estimated from prompt length and keywords: refactor, architecture, migrate, concurrency and security point to high; typo, rename, format and docs point to low.
   - Within a tier, a choice loses 0.05 per step between the effort the complexity wants (medium for low and mid, high for high) and its own effort. This only breaks ties. A choice with no effort is not penalised. Low effort is never preferred: on the bundled and held-out cases, wanting low effort for low complexity lowered accuracy, because short prompts count as low.
   - `priority` is the harness priority. Default `agy > opencode > codex > claude`.
   - Free models get a cost bonus.

   Permanent auth or validation errors from the decider are not retried.
5. **Account selection.** In code, among the chosen choice's accounts: most headroom for its pool first, then fewest active runs, then `priority`, then least recently used.
6. **Result.** `{choice_id, harness, account, model, effort, source: jev|heuristic|pinned, rationale, confidence, probabilities, usage{input_tokens, output_tokens, cost}, alternatives[3], decider_model, decider_models, config_fingerprint}`, where `decider_model` is the model that answered (also when its answer was rejected) and `decider_models` the models in effect with their source. The result is stored as a `route_decision` event.

`route` previews only; it reserves nothing. `spawn_agent` with `harness: auto` runs the same procedure and reserves the cap slot in the same supervisor step. Task text is data and cannot change the eligible set.

### 8.4 Claude rank

Claude ranks lowest by default. The router picks a Claude choice only when one of these holds:
- the call names it
- no other eligible choice has an account with headroom
- the choice's role is the only one that matches

This keeps Claude limits for the orchestrator.

### 8.5 Failover

On a quota, rate-limit or auth error during a run:
1. The (account, pool) is marked exhausted (6.3).
2. The run restarts on the next-ranked account of the same choice. If none exists, it moves to the next eligible choice.
3. The run keeps its run id. The UI shows each attempt as a segment on the run's lane.

Other non-zero exits are not retried. The run fails and the parent is told.

Failing on an unreachable decider (8.3 step 4) applies to explicit routing requests only: `route`, `spawn_agent` with `harness: auto`, `POST /route` and `route eval`. Recovery of a run that already exists never fails because the decider is down. A failover and the re-routing of a queued run route with the heuristic in that case, record a `route_decision` whose `fallback_reason` is `decider unreachable: <models>`, log a warning, and ask the decider at most once (primary model, then fallback) for that recovery.

### 8.6 Evaluation

`superorchestrator route eval [--cases FILE] [--require-live] [--min-accuracy X]` replays labelled cases. It ships agent-dispatcher's English and Vietnamese cases (the tuning set) and keeps a separate held-out set, `crates/so-router/evals/holdout.jsonl`, that is never tuned on (`--cases` runs it). The held-out cases are same-distribution paraphrases: new wording and domains for the same kinds of task, not a different distribution, so its score is an optimistic estimate. A case may list a second expected choice with a written `reason`. It reports accuracy, eligible-choice compliance, fallback count, error count, source counts, the decider models in effect, p50/p95 latency and total cost. A decider that cannot be reached counts as an error, not a fallback. Reports never include task text or credentials.

## 9. Communication and nesting

- **Messages.** A `messages` table holds `(id, session_id, from_run, to_run, kind, body, created_at, read_at)`. `from_run`/`to_run` are null for the orchestrator. Kinds: `message`, `progress`, `question`, `answer`, `nudge`, `result`.
- **Parent side.**
  - `send_message(run, text)`: delivered live when the harness supports live input (all four drivers do); otherwise queued for the child's next tool call.
  - `send_message` to a finished run resumes its native session with the message.
  - `read_messages()` returns unread messages for the caller.
  - `wait_agent(runs[], timeout)` returns when any listed run finishes or posts a `question`/`progress`, or when the timeout ends. While it waits, child events stream to the client as MCP progress notifications.
- **Child side.**
  - `report_progress(step, status, note?)`
  - `ask_parent(question, options?)`: blocks until an answer or `runs.ask_timeout` (default 30 min), then returns `timeout`.
  - `read_messages()`
- **Elicitation.** When an `ask_parent` question reaches the orchestrator and the client supports elicitation, `wait_agent` can surface it to the person. Otherwise the orchestrator answers with `send_message(kind: answer)`.
- **Fork.** `fork_agent(run, prompt)` starts a new run that resumes a copy of a finished run's native session, where the harness supports forking resume. Otherwise it returns an error that names the harness.

## 10. Custom agents

An agent is `AGENT.md` with YAML frontmatter, in the same format as Claude Code subagent files plus `harness` and `accounts`. It can sit in a folder with `skills/` and extra files. Search order:
1. `.superorchestrator/agents/`
2. `~/.config/superorchestrator/agents/`
3. `.claude/agents/`, read-only import

```yaml
---
name: pr-reviewer
description: Reviews a PR diff for correctness bugs.
harness: auto            # or codex | agy | opencode | claude
accounts: [codex-acc2]   # optional allow-list
model: gpt-6.1-sol
effort: high
tools: [Read, Grep, Bash]
disallowedTools: [Write]
skills: [gh-cli]
mcpServers: [github]
hooks: {}
memory: project          # user | project | local
maxTurns: 40
isolation: worktree
review: false
---
System prompt body…
```

**Materialisation at spawn**, in each harness's native format inside the run directory:
- Codex: `config.toml` overrides, `AGENTS.md` and `skills/`.
- OpenCode: `opencode.json` agent and permission entries.
- Claude: `--mcp-config` and `--append-system-prompt-file`.
- agy: `--json-schema`, the system addendum, and skills linked into the run workspace.

**Field mapping:**
- `tools` and `disallowedTools` map to each harness's controls where they exist. Codex has only sandbox and approval policy; agy has all-or-nothing. Gaps are listed by `agent show`, never faked.
- `memory` loads `MEMORY.md` from the scope into the instructions.
- `hooks` are passed through only to Claude and Codex.

**Starter agents**, installed by `init`: `ui-ux-designer`, `pr-reviewer`, `issue-summarizer`.

**CLI:** `agent create|list|show|edit|remove`.

## 11. Workflows

`run_workflow(name|script, args)` runs a JavaScript workflow in embedded QuickJS. The API matches Claude Code workflow scripts:
- `agent(prompt, opts)`: opts are the spawn options from 7.2, so any harness can be used.
- `parallel([...])`: defaults to `isolation: worktree`. A failed agent gives `null`.
- `pipeline([...])`
- `phase(name, fn)`
- `log(msg)`
- `args`

Behaviour:
- **Resume.** Each `agent()` result is cached in `state.db`, keyed by a hash of the prompt, the options and the workflow name. Relaunching a workflow replays cached results and runs only the remaining steps.
- **Failover.** Usage-limit failover follows 8.5. If no account is available, the step waits until the earliest reset time.
- **Stalls.** `stallMs` restarts a stalled step up to 5 times.
- **Saved workflows.** Scripts in `~/.config/superorchestrator/workflows/` and `.superorchestrator/workflows/` can be run by name.
- **Progress.** `workflow_status(id)` and `superorchestrator runs watch --workflow <id>`.
- **No input.** Workflows have no mid-run input from a person.

## 12. Events and timeline data

Every observable fact is appended to `events(id, session_id, run_id, ts, type, data_json)`. Event types:

- Lifecycle: `run_queued`, `run_started`, `state_changed{from,to}`, `run_finished{status, result_ref}`, `attempt_started{account}`, `failover{from,to,reason}`
- Routing: `route_decision{…}`
- Activity: `tool_call_started{name}`, `tool_call_finished{name, ok}`, `assistant_message{excerpt}`, `progress{step,status}`
- Messages: `message_sent{to, kind}`, `question_asked`, `question_answered`
- Usage: `usage{input_tokens, output_tokens, cost?, rate_limits?}`
- Join: `user_joined`, `user_left`, `join_command_issued`
- Stalls: `stall_detected`, `nudge_sent`, `escalated`
- Worktree and review: `worktree_created{path, branch}`, `worktree_removed`, `check_result{name, ok}`, `review_accepted`, `review_rejected`

Full transcripts are stored as files under `runs/<run_id>/transcript.jsonl`, in normalised form plus the raw harness lines. Events refer to them by offset.

## 13. MCP tools

**Orchestrator and subagents**, with depth limits:

| Tool | Purpose |
|---|---|
| `route` | Preview a route decision (8.3). Runs nothing. |
| `spawn_agent` | Start a run (7.2). Returns the run id, or the result when `wait: true`. |
| `wait_agent` | Wait on runs, with a timeout. Streams progress notifications. |
| `get_run` | State, result, branch, worktree, usage and the last key events; `diff: true` adds the run's changes (7.5); `verbose: true` returns the full record |
| `send_message` | Message a run. Resumes a finished run. |
| `read_messages` | Unread messages for the caller |
| `cancel_agent` | Cancel a run and its subtree |
| `accept_run`, `reject_run` | Review gate (7.5); `accept_run` with `merge: true` lands the branch |
| `fork_agent` | Fork a finished run (9) |
| `list_harnesses` | One short row per account and per choice, and the free models (13.1) |
| `list_agents` | Custom agents |
| `run_workflow`, `workflow_status` | Workflows (11) |

**Subagents only:** `report_progress`, `ask_parent`.

### 13.1 Tool surface and reply shapes

Every orchestrator session pays for `tools/list` and for each reply, so both are kept short (`docs/plans/2026-10-23-token-tuning.md` has the measured sizes, from `cargo test -p so-supervisor --test token_report -- --nocapture`). Input schemas carry no defaults, format hints or `additionalProperties`; parameter descriptions appear only where the name and type do not say enough. The supervisor still validates every call.

The supervisor's replies stay complete, because the CLI and the web UI read them. The MCP bridge (`so-mcp`, `compact.rs`) projects what the orchestrator sees. In every projection a missing key means null, empty or false, timestamps are cut to seconds and ids are not repeated inside nested objects. The agent's own `result` is never altered.

| Tool | MCP reply |
|---|---|
| `spawn_agent`, `fork_agent` | `{run_id, state, result?, warnings?, error_kind?}`. With `wait`, `result` is set once the run finished or reached the review gate (`state: pending_review`). |
| `wait_agent` | `{finished: [{run_id, name?, state, result?, error_kind?, branch?, failed_checks?, failover?, question?}], updates: [...], timed_out?}`. The bridge reads each finished run, so no `get_run` is needed to act; a run it cannot read is `{run_id, state: "unknown", error}`. An update is an event with its data flattened: `{run_id, type: "question", question, options?}` for a question, `{run_id, type: "progress", step?, status?}` for progress. |
| `get_run` | `{run_id, name?, state, harness, account, model?, result?, error_kind?, branch?, worktree?, parent_run?, usage?, finished_at?, question?, events?, diff?}`. `events` are the last 5 of the significant ones (failover, stall, nudge, escalation, questions, check results, review decisions, run end, progress; at most the last 2 progress events), plus the latest check result, failed-check gate, failover, escalation and stall even when older; each is `{ts, type, data}` with strings cut at 300 characters (2000 for a check result). `question` is set while the run waits. `diff` is `{base (10 characters), stat, patch, truncated?, error?}`; the branch is above. `verbose: true` returns the supervisor's full `RunView`. |
| `route` | `{choice_id, harness, account, model, effort?, source, rationale, alternatives: [choice ids], ranked_accounts? (when more than one)}`. |
| `accept_run` | `{merged, branch?, merge_command?, note?}`. A refused merge is an error. |
| `list_harnesses` | `{accounts: [{name, harness, signed_in?, disabled?, load: "active/cap", headroom?, cooldown?, error?}], choices: [{id, harness, model, effort?, roles?, description?, free?, eligible?: false, why?}], free_models?: {opencode: [...]}}`. `headroom` is `status 5hN wkN` per pool (remaining percent; `pool:` prefix unless `default`), left out when unknown, as is `signed_in`. `cooldown` is the latest `until`. `roles` is left out when it is `["worker"]`. A choice is eligible when the router would take it; otherwise `eligible: false` and `why` is `disabled`, `no account`, `account disabled`, `at cap`, `cooldown`, `no headroom` or `not eligible` (the router found no usable account for a reason the view cannot name). A choice is `free` when the catalog entry says `cost = "free"`; a model the catalog does not list is free when its id ends in `-free` or `:free` (one rule, `so_core::model_name_is_free`, shared with the generated skill). `free_models` lists the free catalog models of each harness that has an enabled account (opencode; `models refresh` updates the catalog, which is cached by file time). The supervisor builds this view when `list_harnesses` is called with `{"brief": true}`, which the bridge always does; without it the reply is the full `HarnessesView`. |
| `read_messages` | `{messages: [{from_run, kind, body, created_at}]}`. |
| `run_workflow` | `{id, state, result?, error?}`. |
| `workflow_status` | `{id, name, state, result?, error?, started_at, finished_at?, steps: [{name, state, run_id?, phase?}]}`, steps in start order. |

`spawn_agent` accepts `model` to pin a model, so an orchestrator can pick one of `free_models` for a simple task.

Long tools (`spawn_agent` with `wait`, `wait_agent`, `run_workflow`) are MCP tasks when the client declares the tasks extension. Progress goes to the task's status message, and `tasks/cancel` stops the wait without cancelling the runs. Otherwise the tools block, with progress notifications.

## 14. Web UI

`superorchestrator ui` serves at `127.0.0.1:7878` (`ui.bind`, `ui.port`). Binding to anything other than loopback requires `--token`. The browser then asks for the token once and stores it in an HttpOnly cookie. Credential homes and auth files are never served.

**Pages:**
- **Dashboard.**
  - Accounts table: harness, signed-in state, 5 h and weekly headroom bars per pool, reset times, active runs against the cap.
  - Global cap usage.
  - Live runs.
  - Recent escalations.
  - Cost and tokens per day.
- **Sessions.** A list of sessions (orchestrator and interactive) with start time, orchestrator, run count, status and cost. Filters: active, escalated, failed.
- **Session timeline.** A Gantt chart with one lane per run, nested under its parent. The orchestrator is the top lane.
  - Each lane shows coloured state segments, failover attempts, tool-call ticks, message arrows between lanes, and stall and escalation markers.
  - Clicking a lane opens the run panel: brief, route decision with probabilities and rationale, events, transcript, final result, worktree and branch, join command, and actions (send message, cancel, accept, reject).
  - The view updates live over SSE.
- **Accounts.** Add (with a login flow), import, edit cap, priority and pools, enable or disable, flag cooldown, remove.
  - Login runs `codex login --device-auth` where available and shows the URL and code. For other harnesses, the page shows the command to run, then polls the sign-in check.
- **Choices and routing.** Edit choices and the routing instruction, run a route preview, and view the decision log.
- **Agents.** List, view and edit agent files.
- **Workflows.** Saved workflows, runs and step status.
- **Settings.** `config.toml` fields, notifications, worktree defaults, and the OpenRouter key source. The key value is never shown.

**API:** The JSON API under `/api/v1` mirrors the CLI. `/api/v1/stream` is SSE for events. The frontend uses only this API.

## 15. CLI

`superorchestrator` with no arguments on a terminal opens the terminal UI (`tui`, below); without a terminal it prints the help. Every action in it is also a subcommand:

```
setup [--codex N] [--claude N] [--opencode N] [--agy N] [--no-login] [--no-install] [--no-claude-md]
                          first-run guide: init, detect harnesses, offer account import --scan,
                          add N accounts per harness, sign in, install, doctor summary
init                      create config dir, seed config, choices, catalog, starter agents
install                   register the MCP server with Claude Code (claude mcp add) and other harnesses, install skills
uninstall
doctor                    binaries on PATH, versions, bwrap, sign-ins, socket, db, UI port
status                    accounts and headroom, active runs, supervisor state
account add [--name N] [--count K] [--login]|import|login|list|remove|enable|disable|flag
agent create|list|show|edit|remove
models refresh|list
route "<task>" [--role R] [--explain]
route eval [...]
runs list|show|watch|cancel
join <run> [--exec]
tui                       full-screen terminal UI: overview, accounts, runs, setup
worktrees list|prune
workflow run|list|status
config get|set|edit
ui [--bind ADDR] [--port N] [--token T] [--install-service]
daemon [--foreground]
exec <account> -- <args>
mcp
migrate [--apply]
```

`tui` (also the bare command on a terminal) is a ratatui UI with four tabs: Overview (setup checklist with the fix for each missing item, account and run counts), Accounts (add, sign in (claude and agy in a session of their wrapper), enable or disable, flag a cooldown, remove with a typed-name confirmation, import scan), Runs (recent runs from the store refreshed every 2 s, a detail pane with the diff of a run waiting for review, cancel, open session (`o`), accept (`a` accepts and merges a run with a worktree, `A` only accepts) and reject) and Setup (the `setup` steps one at a time with live status, plus `install` and `doctor`). `?` opens help, `Tab` and `1` to `4` switch tabs, `j`/`k`/`g`/`G` or the arrow keys move, `q` or `Esc` quits from the top level. It calls the same library functions as the commands above and adds no business logic. Sign-in and join hand the terminal to the child process and restore it afterwards. The diff, cancel, open session, accept and reject use the supervisor socket and never start it; when it is not running they report that. It honors `NO_COLOR`, needs at least 60x16, redraws on resize and restores the terminal on every exit path. Without a terminal `tui` exits 2.

`setup` is the first command a new user runs and is idempotent. It runs `init` (silent when nothing is created), prints the installed harnesses, offers the `account import --scan` results, asks for each installed harness how many accounts to add (default 0, or 1 when the harness has no account or importable login; claude defaults to 0 because its accounts are kept for the orchestrator and its choices are disabled, 8.2), signs in to the new accounts on a terminal (otherwise prints the commands), runs `install`, and prints a short `doctor` summary. `--yes` takes the defaults, `--codex N --claude N --opencode N --agy N` set the counts, `--no-login` and `--no-install` skip those steps, and `--no-claude-md` is passed to `install`. Accounts that are still signed out, new or from an earlier run, are offered a sign-in on a terminal and get their `account login` command printed otherwise. A run that adds nothing prints which accounts are already set up. A harness whose count was asked for but whose binary is not on `PATH` fails the run (exit 1) after the other steps. Ctrl-C at a prompt exits 130. `setup --yes --json` prints one summary object (`ok`, `created`, `imported`, `installed`, `signed_in_needed`, `doctor`). Without a terminal and without `--yes` it prints the plan and exits 0. Counts that are not asked are never added twice, so a second run changes nothing. On macOS only one agy account is added (ADR-0005).

`account add <harness>` without `--name` names the account `<harness>-<n>` with the lowest free `n` from 1; a name is skipped when the account exists or when it would shadow a command on `PATH`. `--count K` creates K accounts; `--name` with `--count` above 1 is a usage error (exit 2). `--login` signs in to each new account in turn on a terminal, and prints the commands otherwise. With `--json` the result lists every created account under `accounts`.

`exec`, `daemon` and `mcp` are internal (wrappers, the supervisor and MCP clients start them): they work but are hidden from `--help`, which ends with "Get started: superorchestrator setup".

## 16. Configuration

```
~/.config/superorchestrator/
  config.toml        defaults, limits, router, usage, worktrees, ui, notifications
  models.toml        model catalog
  accounts/<name>.toml
  agents/<name>/AGENT.md
  workflows/<name>.js
  state.db
  runs/<run_id>/     transcript.jsonl, mcp.json, system.md, materialised agent files
  logs/              supervisor.log (rotated)
  wrappers.bak/
  legacy-backup/<ts>/
```

```toml
[limits]
global_cap = 16
max_children = 8

[runs]
stall_ms = 300000
ask_timeout = "30m"

[usage]
probe_ttl = "5m"
low_pct = 15
default_cooldown = "5h"

[router]
priority = ["agy", "opencode", "codex", "claude"]
weights = { headroom = 1.0, load = 0.5, cost = 0.3 }
instruction = "Choose the least costly eligible choice that can reliably complete task_content…"

[router.jev]
endpoint = "https://openrouter.ai/api/alpha/decisions"
model = "inception/mercury-decide:free"      # any OpenRouter model id; env SUPERORCHESTRATOR_ROUTER_MODEL
fallback_model = "~typesafe/jev-latest"        # env SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL; "none" disables
on_unreachable = "error"                        # or "heuristic" to degrade
key_env = "OPENROUTER_API_KEY"
key_file = "~/dev/jev_skill_selector/.env"   # optional dotenv
timeout = "20s"

[router.policy]
min_confidence = 0.45
min_margin = 0.15

[worktrees]
dir = "{repo_parent}/{repo}-worktrees"
branch_prefix = "so/"

[accounts]
wrapper_dir = "~/.local/bin"

[ui]
bind = "127.0.0.1"
port = 7878

[supervisor]
idle_exit = "10m"

[notifications]
discord_webhook_env = "SO_DISCORD_WEBHOOK"
desktop = true
on = ["escalated", "question_waiting_10m", "session_finished", "account_exhausted"]
```

Environment variables: `SUPERORCHESTRATOR_CONFIG_DIR`, `SUPERORCHESTRATOR_RUN_ID`, `SUPERORCHESTRATOR_SOCKET`, `SUPERORCHESTRATOR_LOG`, `SUPERORCHESTRATOR_ROUTER_MODEL`, `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL`.

The default `min_confidence` (0.45) and `min_margin` (0.15) were tuned on 2026-10-23 against `jev-1.14-20261014` (`~typesafe/jev-latest`). With the first, untuned descriptions, correct answers had confidence 0.47-0.56 and margin 0.29-0.42, while answers that should be rejected had confidence 0.20-0.36 and margin 0.00-0.22; the defaults sit in that gap. Live results with the shipped seeds, routing instruction and these defaults (one run each on 2026-10-23): on `inception/mercury-decide:free` the tuning set scored 87% (13 of 15, no fallbacks) and the held-out set 88% (14 of 16, one fallback); on `~typesafe/jev-latest` the tuning set scored 87% (3 fallbacks) and the held-out set 75% (2 fallbacks). The remaining misses were mostly a neighbouring tier (flash-high for sonnet-thinking work). Without a decider the heuristic scores 73% on the tuning set and 50% on the held-out set. Choices already in a user's `config.toml` keep their old descriptions: `init` never rewrites them (8.2), so copy the seeded descriptions to benefit from the tuning. The free `mercury-decide` tier can be rate limited or withdrawn and resolves to a dated snapshot; the fallback covers outages, and `SUPERORCHESTRATOR_ROUTER_MODEL=~typesafe/jev-latest` switches the primary.

## 17. Data model (SQLite)

| Table | Key columns |
|---|---|
| `sessions` | id, kind (`orchestrator`, `interactive`), client name, cwd, account (interactive), pid, started_at, ended_at, status |
| `runs` | id, session_id, parent_run_id, depth, name, agent, choice_id, harness, account, model, effort, cwd, worktree_path, branch, native_session_id, state, review, output_schema, result_json, error_kind, created_at, started_at, finished_at |
| `attempts` | id, run_id, account, started_at, ended_at, outcome |
| `events` | id, session_id, run_id, ts, type, data_json (indexed by session_id, ts) |
| `messages` | as in section 9 |
| `run_meta` | run_id, checks_json (review checks), fork_from (native session a fork resumes), resume_prompt (set while a finished run waits in `queued` to be resumed). No foreign key: written before the run row. Deleted by `worktrees prune`. |
| `headroom` | account, pool, five_hour_remaining_pct, five_hour_resets_at, weekly_remaining_pct, weekly_resets_at, status, source, observed_at |
| `ledger` | account, pool, run_id, ts, input_tokens, output_tokens, cost |
| `flags` | account, pool, until, reason |
| `workflow_cache` | key_hash, workflow, result_json, created_at |
| `workflow_runs` | id, name, session_id, state, args_json, started_at, finished_at |
| `workflow_steps` | id, workflow_run_id, step_index, phase, prompt_hash, run_id, state, started_at, finished_at |

Migrations are embedded and run on open. Each table has a schema version.

## 18. Notifications

The supervisor sends notifications for the events listed in `notifications.on`, through a Discord webhook (URL taken from the env var named in config) and the desktop (`notify-send` on Linux, `osascript` on macOS). Notifications are rate limited to one per run per event type every 10 minutes.

## 19. Shipped skill and migration

### 19.1 Skill

`install` writes two skills, each to `~/.claude/skills/<name>/` and `~/.agents/skills/<name>/`, both carrying the `<!-- superorchestrator:managed -->` marker. It removes the older single `superorchestrator` skill only when its `SKILL.md` carries the marker. `uninstall` removes both (marker-checked).

`superorchestrator-cli` is static text: a guide to the command line for people and agents (setup, accounts, `status`, `doctor`, runs, `join`, `ui`, workflows, `route`, config and where troubleshooting is documented).

`use-superorchestrator-mcp` is regenerated whenever accounts, choices or agents change (a refresh also writes the CLI skill when it is missing). It explains:
- the accounts and choices that exist
- when to call `route` versus `spawn_agent`
- how to write a task brief with acceptance criteria and out-of-scope items
- how to wait, message and review runs
- how a subagent talks to its parent
- how to check `superorchestrator status`

`install` also adds a short managed block to `~/.claude/CLAUDE.md` (created if missing) between `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->`. It says to delegate implementation through superorchestrator, to load `use-superorchestrator-mcp` before the first delegation, and to use `superorchestrator-cli` for accounts, status and runs. Re-installing replaces the block in place, text outside it is never touched, `--no-claude-md` skips it, and `uninstall` removes only the block. The edit goes through the same backup, atomic write and `--dry-run` path as the other files.

`install` also runs `claude mcp add superorchestrator -- superorchestrator mcp`. It adds the same MCP entry to the Codex, OpenCode and agy configs, without touching unrelated entries, and writes a backup of every file it changes.

### 19.2 Cutover (`migrate`)

`superorchestrator migrate` replaces the old tooling once M3 is done. It shows a dry run by default; `--apply` executes:

1. Back up everything it touches to `legacy-backup/<ts>/`.
2. `account import --scan` the existing accounts:
   - `codex-acc1` → `~/.codex`
   - `codex-acc2..5` → `~/.codex-accN`
   - `agy-acc1..5` → `~/.agent-farm/profiles/agy_worker_N/.gemini`
   - the opencode and claude defaults

   Then rewrite their wrappers.
3. Import cooldowns from `~/.local/state/coding-agent/.state/*/*.meta` as flags.
4. Remove:
   - `~/.claude/orchestrator/`
   - `~/.claude/orchestrator-state*.json`
   - `~/.claude/skills/coding-agent-management/`
   - `~/.local/bin/check-account-usage`
   - `~/.local/bin/add-agy-worker`
   - the `jev-router-mcp` entries in the Codex, agy and OpenCode configs
5. Rewrite the delegation rules in `~/.claude/CLAUDE.md`: implementation is delegated through the superorchestrator MCP tools and skill, not the `orchestrate` skill. Other lines are left unchanged. A diff is shown first.
6. Seed `choices` from `~/.config/jev-router/config.json`, if it exists.

## 20. Security

- **Credentials.** Never printed, logged, copied into prompts, served by the UI, or committed. Probes and sign-in checks test only that auth files exist. The supervisor log redacts tokens matching known key patterns.
- **Socket and UI.**
  - The socket is created with mode `0600` in a user-only directory.
  - The UI binds to loopback by default. A non-loopback bind requires a token.
  - Codex app-server websockets bind to `127.0.0.1` with a per-run token.
  - `opencode serve` uses a per-run password.
- **Task text.** Task text is passed to children as argv elements or JSON, never through a shell string. It cannot change routing eligibility.
- **Permissions.** Children run with full permissions, so worktree isolation is the default. `isolation: none` must be set explicitly per call or per agent.
- **Destructive commands.** `account remove`, `worktrees prune` and `migrate --apply` ask for confirmation unless `--yes` is passed.

## 21. Testing

- **Unit tests:** routing score, Jev response validation and gate, event normalisation per harness, account environment building, agent materialisation, worktree naming, config parsing.
- **Driver tests** use recorded fixtures (codex app-server JSON-RPC transcripts, opencode SSE, claude and agy stream-json) and fake harness binaries that replay them. They cover live messages, cancel, resume, quota errors and failover.
- **Store tests** use a temporary `state.db` and include a crash and reconcile of the supervisor.
- **MCP tests** drive `superorchestrator mcp` over stdio with an rmcp client: tools, tasks, progress, depth and cap errors.
- **UI tests:** API contract tests in Rust and component tests for the timeline in the frontend.
- **CI:** GitHub Actions on Ubuntu, Arch (container) and macOS, running `cargo test`, `cargo clippy -D warnings`, `cargo fmt --check` and the frontend build. CI never logs in to real accounts.
- **Manual smoke** (`scripts/smoke.sh`): on a real machine, one task per harness and account, a live join for codex and opencode, nested spawn, failover by flagging an account, and `route eval --require-live`.

## 22. Milestones

Each milestone is one or more PRs with `cargo test` coverage, and is usable by itself.

1. **M1 Skeleton.**
   - Scope: workspace, config dir and `init`, `config`, `doctor`, `state.db` with migrations, supervisor lifecycle with socket and idle exit, `mcp` thin client exposing `list_harnesses`, `install` (MCP registration), `install.sh`, release pipeline.
   - Done when: Claude Code lists the server and `list_harnesses` returns the configured accounts.
2. **M2 Accounts and usage.**
   - Scope: `account add|import|login|list|remove|enable|disable|flag`, wrappers and `exec` with interactive sessions, agy per-run gemini dir, the codex and agy probes, ledger, headroom, `status`.
   - Done when: two Codex accounts and two agy accounts show correct headroom, and a wrapper launch appears as a session.
3. **M3 Drivers and runs.**
   - Scope: the four drivers (after the section 4 spikes), `spawn_agent`, `wait_agent`, `get_run`, `cancel_agent`, worktree isolation, child instructions, structured output, events and transcripts, `join`, `runs list|show|watch`, `migrate`.
   - Done when: Claude delegates one task to each harness on a named account, reads the result, and joins a codex run live.
4. **M4 Communication, nesting, review.**
   - Scope: messages, `send_message` (live and resume), `read_messages`, `report_progress`, `ask_parent`, elicitation, progress notifications, nested spawn with depth 2 and caps, `fork_agent`, review gate and checks, stall nudges and escalation, notifications.
5. **M5 Routing.**
   - Scope: catalog and `models refresh`, choices, heuristic, Jev decider with validation and gate, account selection, `route`, `harness: auto`, failover across accounts and choices, `route eval` with the shipped cases.
6. **M6 Web UI.**
   - Scope: axum API and SSE, dashboard, sessions list, session timeline Gantt with the run panel, accounts pages with login flows, choices and settings pages, `ui --install-service`.
7. **M7 Custom agents.**
   - Scope: `AGENT.md` parsing with Claude Code parity, per-harness materialisation, `.claude/agents` import, `agent` CLI, starter agents, the UI agents page.
8. **M8 Workflows.**
   - Scope: `run_workflow` in QuickJS with the Claude Code script API, result cache and resume, stall restarts, usage-limit waits, saved workflows, `workflow_status`, `runs watch --workflow`, the UI workflows page.
9. **M9 Hardening.**
   - Scope: docs, the smoke script, Claude Code plugin packaging, performance limits for the timeline (10 000 events per session), log rotation.

## 23. Open items

- The section 4 driver spikes (agy MCP injection and conversation id, opencode per-run config, codex thread parameters, claude `--max-turns`).
