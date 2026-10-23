# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The seeded choice `opencode-muse-long` uses `opencode/muse-spark-1.3`; the old model id
  (`opencode/muse-spark-1.3-contributor-free`) is gone from opencode and runs on it failed
  with "Model not found". Existing `config.toml` files keep the old id: edit the choice, and
  `models refresh` now reports it (#7).

### Added

- `ErrorKind::Config` (`"config"`): a harness that does not have the run's model
  ("Model not found") is classified as a config error. A routed run fails over to a different
  choice without putting the account in cooldown; a run pinned to a model fails at once (#7).
- `ErrorKind::Refused` (`"refused"`): an agy turn blocked by the Gemini safety filter ("This
  request was blocked by Gemini's filters") finishes the run as `failed` instead of
  `succeeded`. The account is not flagged and the run does not fail over. Only the English
  phrase is detected, from the result parser (#13).
- `models refresh` reports `missing_choice_models`: enabled choices whose model a harness did not
  list (#7).

### Fixed

- `send_message` to a run that failed with a config error is refused instead of resuming a run
  that cannot succeed (#7).

## [0.1.0] - 2026-10-23

First release.

### Added

- A full-screen terminal UI, opened by `superorchestrator` on a terminal or by `superorchestrator tui`,
  replaces the dialoguer menu. Tabs: Overview (setup checklist with fixes), Accounts (add, sign
  in, enable, flag, remove, import), Runs (live list, details, cancel, join, accept, reject) and
  Setup (the first-run steps with live status). It honors `NO_COLOR`, falls back to ASCII
  without UTF-8 and restores the terminal on every exit. Without a terminal the bare command
  still prints the help. See `docs/guide/tui.md`.
- `superorchestrator setup`: a guided, idempotent first-run command (init, detect harnesses,
  import existing logins, add accounts, sign in, install, doctor summary). `--yes`, per-harness
  counts (`--codex N --claude N --opencode N --agy N`), `--no-login`, `--no-install` and `--no-claude-md` drive it
  without prompts; without a terminal it prints the plan.
- `account add <harness>` takes `--count K` and `--login`; without `--name` it names accounts
  `<harness>-<n>` with the lowest free `n`.
- `--help` hides the internal `exec`, `daemon` and `mcp` commands, lists the rest in order of
  use and ends with "Get started: superorchestrator setup". `init` points to `setup` first, and
  `install.sh` ends with the same hint.
- Landing finished work: `accept_run` takes `merge: true` to merge the run's branch into the
  checkout it was started from (fast-forward, else a `merge: <title> (<id>)` commit) and remove
  the worktree. Work the run left uncommitted (untracked files included, `.gitignore` respected) is
  first committed in its worktree as `<title> (<id>)` (the repo's hooks run; the note says
  `committed uncommitted work as <sha>`). A dirty or different-branch base checkout, a failing
  commit hook, or a conflict (aborted) fails the call and leaves the run at the review gate. The
  result reports `merged`, and `branch` with the merge command when the work is not merged.
- `get_run` takes `diff: true` to return a run's changes (`base`, `branch`, `stat`, `patch`, cut
  at 64 KiB); `superorchestrator runs show <id> --diff` prints them.
- The `run_workflow` tool description documents the script API (`agent` and its options,
  `parallel`, `pipeline`, `phase`, `log`, `args`) with an example, and the MCP skill tells
  orchestrators to review with `diff: true` and land with `merge: true`.
- Fewer tokens for orchestrators: `tools/list` is 41% smaller (9,712 to 5,720 bytes for 14
  tools: terse descriptions, schemas without defaults and format hints), and the replies of
  the canonical flow are 74% smaller. Replies leave out null and empty keys, cut timestamps to
  seconds and no longer repeat ids. `get_run` shows state, result, branch, worktree, usage and
  the last key events (`verbose: true` returns the full record), `wait_agent` returns each
  finished run's state and result, and `list_harnesses` is one short row per account and
  choice with `eligible`/`why`, `free` and the free opencode models from the model catalog.
  The CLI and the web UI keep the full supervisor replies. Both skills are terse: the MCP
  skill starts with a five-step workflow (3,349 to 1,772 bytes) and the CLI skill went from
  4,394 to 2,482 bytes. `docs/plans/2026-10-23-token-tuning.md` has the numbers and
  `cargo test -p so-supervisor --test token_report -- --nocapture` reproduces them.
- Two skills replace the single generated one: `use-superorchestrator-mcp` (delegation through
  the MCP tools, with the accounts, choices and agents lists) and `superorchestrator-cli` (a
  static guide to the command line). `install` writes both to `~/.claude/skills` and
  `~/.agents/skills` and removes the old `superorchestrator` skill when it carries our marker.
- `install` adds a short managed delegation block to `~/.claude/CLAUDE.md`, between
  `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->`. Re-installing
  replaces it in place, `--no-claude-md` skips it, `uninstall` removes only the block.
- Routing decider: `inception/mercury-decide:free` is the default model, with
  `~typesafe/jev-latest` as an automatic fallback when the primary is unusable (transport
  error, timeout, 429, 5xx or a missing model; never for a rejected answer). Both take any
  OpenRouter id in `router.jev.model` and `router.jev.fallback_model`, or from
  `SUPERORCHESTRATOR_ROUTER_MODEL` and `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL` (env beats
  config beats default, `none` disables the fallback). `route --explain`, `doctor` and
  `route eval` show the models and their source, and records name the model that answered.
- A configured decider that cannot be reached now fails the call with an error that names the
  models, the cause and the next step (`route` exits non-zero, MCP `isError`, `spawn_agent`
  auto spawns nothing, `POST /route` 502, `route eval` counts an error).
  `router.jev.on_unreachable = "heuristic"` keeps the old degrade. No key still routes by
  heuristic. Recovery of an existing run (failover, queued reroute) never fails on a decider
  outage: it routes with the heuristic and records why.
- Held-out routing eval set (`crates/so-router/evals/holdout.jsonl`), kept separate from the
  tuning set. `route eval --json` reports each case's probabilities, fallback reason and
  answering model.
- Cargo workspace with a React, Vite and TypeScript frontend in `web/`.
- Core types, paths, config, account and protocol modules, and a SQLite store with versioned
  migrations.
- Command line: `init`, `config`, `account`, `agent`, `models`, `route`, `runs`, `join`,
  `worktrees`, `workflow`, `status`, `doctor`, `exec`, `daemon`, `mcp`, `ui`, `migrate`,
  `install` and `uninstall`, plus an interactive menu. `init` prints the next steps.
- Supervisor daemon on a local socket with sessions, a lock, idle exit and reconcile, and a
  run manager (spawn, wait, get, cancel) with worktrees per run.
- Harness drivers for Claude Code, Codex (app-server), OpenCode (serve) and agy, with a
  scripted fake harness for tests.
- Accounts: launch environments, wrappers, sign-in checks, account scanning, and usage probes
  with a ledger, headroom and exhaustion tracking.
- Router: model catalog, eligibility, heuristic and Jev decider, selection and evaluation
  (`route eval`).
- Failover: a run that hits a quota, rate-limit or auth error, including one that exits while
  starting (for example agy's "Eligibility check failed"), moves to the next account or choice.
  Launch failures count as attempts, so failover is bounded and never returns to an account,
  and an auth failure flags every pool of the account.
- Codex choices are seeded enabled. A choice without a signed-in account is skipped at
  routing time, so adding a codex account makes `harness: auto` use it. Claude choices stay off
  by default. `doctor` warns about configs made by an older `init` that keep codex choices off.
- Custom agents defined by `AGENT.md`, with discovery, per-harness materialisation, starter
  agents and a list of gaps per harness.
- Workflows: a QuickJS engine with `agent`, `parallel`, `pipeline`, `phase` and `log`, a result
  cache with resume, stall restarts, usage-limit waits and saved workflows. The engine is wired
  to the supervisor: `run_workflow`, `workflow_status`, `workflow run|list|status|cancel` and
  `runs watch --workflow`.
- MCP stdio server that forwards tools to the supervisor. Clients that support MCP tasks get a
  task for `spawn_agent` with `wait`, `wait_agent` and `run_workflow`. Other clients block and
  get progress notifications. Questions from subagents surface through elicitation.
- Run messaging, `ask_parent`, a review gate, stall handling, resume after death, fork and
  notifications.
- Web UI served by the supervisor: dashboard, sessions, a session timeline with a run panel
  (brief, route decision, events, transcript, result, join command and actions), accounts with
  login flows, choices, agents, workflows and settings. `superorchestrator ui` starts it, and
  `ui --install-service` writes a systemd or launchd service.
- JSON API under `/api/v1` and a server-sent events stream at `/api/v1/stream`. API types are
  generated from the Rust types (`gen-ts`).
- `migrate`: a dry-run-first cutover from the old tooling, with backups.
- Inspection connections: `status`, `runs show`, `join`, `route` and `ui` open no session. A
  new `inspect` option of the `hello` handshake limits the connection to reads and run controls.
  A newer CLI talking to a supervisor started before this change falls back to the old
  handshake until that supervisor restarts.
- The shipped MCP skill lists accounts, choices and agents, and is regenerated whenever they change,
  from the CLI and from the web API. Only an installed skill with our marker is touched.
- Claude Code plugin in `plugin/`: the `superorchestrator mcp` server and the two skills.
- `scripts/smoke.sh`: a manual smoke test on a real machine, with an isolated config dir and
  wrappers.
- User documentation: README quickstart and guides under `docs/guide/`.
- Release pipeline: cargo-dist builds for Linux and macOS on x86_64 and aarch64 on version tags,
  and an `install.sh` installer that verifies sha256 checksums. `install.sh` points to the
  quickstart when it finishes.
- CI: lint, tests on Linux and macOS, Arch Linux tests, web checks and bundle size budget,
  coverage report, end-to-end smoke with fake harnesses, browser tests against a real, seeded
  supervisor (`pnpm -C web e2e:real`), shellcheck and actionlint.

### Fixed

- `account import --scan` and `setup` only offer homes that show a sign-in (the harness's auth
  file, or a `projects` directory for Claude's default home). A second `setup --yes` no longer
  imports `claude-acc1` and `opencode-acc1` from directories that `install` and harness probes
  created. `install` prints `~/` paths and names the `claude mcp add` it ran.
- Choice effort is validated per harness. The seeded `agy-sonnet-thinking` choice (`effort =
  "thinking"`) made agy refuse to launch, because `agy --effort` takes only
  `low|medium|high|xhigh|max`. `thinking` is now a router hint for a model that reasons by
  itself: the agy driver launches without `--effort`, and the router still ranks it with the top
  effort tier, so existing configs keep working. Any other effort a harness rejects (for
  example `claude` with `thinking`) fails the launch before a process starts, makes routing skip
  the choice with the reason in the route rationale, and is reported by `superorchestrator
  doctor` ("choice efforts") with the values to use.
- Routing against live Jev. `router.policy` defaults are now `min_confidence = 0.45` and
  `min_margin = 0.15`, and the seeded choice descriptions and routing instruction describe the
  tiers in general terms. Live results (one run each, 2026-10-23): tuning set 87% on
  mercury-decide and 87% on jev-latest; held-out set 88% on mercury-decide and 75% on
  jev-latest; before the change the tuning set scored about 50% with most cases falling back to
  the heuristic. The heuristic breaks ties inside a tier by effort (73% tuning set, 50%
  held-out without a decider). Existing `config.toml` files keep their old choice descriptions.

- Web UI: dashboard crash against the real server, black timeline state colours on an empty
  session, a CSP console error from zod, and a `1970-01-01` headroom `observed_at` (now `null`).

- `account add` and `account import` refuse a name that would shadow an existing command: when
  the name resolves on `PATH` to an executable that is not one of our wrappers, they fail and name the
  path, so `account add codex --name ls` no longer hides `/usr/bin/ls`.

- `superorchestrator route fix a typo in README` works without quotes; the words are joined with
  spaces. `route eval` is unchanged.

- After `account add`, the sign-in hint for codex and opencode is now
  `superorchestrator account login <name>`. Claude and agy keep the wrapper hint, because they
  sign in inside the session. The JSON output gains `login.cli` next to `login.command`.

- The failover test that waits for a freed account no longer races the fake harness's stdin log.

[Unreleased]: https://github.com/datamonsterr/superorchestrator/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/datamonsterr/superorchestrator/releases/tag/v0.1.0
