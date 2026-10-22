# Release UX plan (v0.1.0)

Goal: a new user installs the binary, runs one command, and has working accounts, the MCP
server and the skills in Claude Code. The CLI is safe and predictable. The docs lead with use
cases.

Integration branch: `feat/release-ux`. Each task works in its own worktree and branch off it,
and merges back after spec and quality review. One PR to `main` at the end.

Findings from a new-user walkthrough (isolated `$HOME`, 2026-10-23):

- `doctor` prints "1 check(s) failed" and exits 0.
- `account add codex --name ls` succeeds and writes `~/.local/bin/ls`, which shadows the system `ls`.
- `route fix a typo in README` fails: "the subcommand 'a' cannot be used with '[TASK]'".
- `account add` needs `--name`; there is no way to create several accounts at once.
- After `account add` the hint is a raw `CODEX_HOME=/abs/path codex login --device-auth` line
  instead of `superorchestrator account login <name>`.
- No single setup command. `init` lists four manual steps.
- Top-level help mixes internal commands (`exec`, `daemon`, `mcp`) with user commands and has no
  "get started" pointer.
- One skill mixes CLI use and MCP delegation. Users paste long delegation rules into
  `~/.claude/CLAUDE.md`.
- macOS CI flake: `failover_with_every_target_full_waits_in_the_queue_and_starts_on_the_freed_account`
  (`crates/so-supervisor/tests/routing.rs:319`) reads `claude-b` stdin before the fake harness wrote it.

## T1: CLI bug fixes (implementer-sonnet)

- `doctor` exits non-zero (exit code 1) when any check fails. Warnings (`!`) do not fail.
- Account names that would shadow an existing command are refused: when `<name>` resolves on
  `PATH` to a file that is not our wrapper (no marker), `account add` and `account import` fail
  with a clear error naming the path. Harness names stay refused as today.
- `route <TASK...>` accepts an unquoted multi-word task (joined with spaces). `route eval` still works.
- The post-`add` login hint prints `superorchestrator account login <name>` (plus the existing
  note for claude/agy). JSON keeps `login.command` as today and adds `login.cli`.
- Fix the routing.rs:319 flake: poll `stdin_lines("claude-b")` until the line appears (bounded,
  like the other waits in that file).

Acceptance: new tests for each item; `cargo test -p superorchestrator -p so-accounts`,
`cargo test -p so-supervisor --test routing`, `cargo clippy --workspace --all-targets -- -D warnings`.

Out of scope: new commands, help layout, skills.

## T2: skills split and CLAUDE.md pointer (implementer-sonnet)

- Replace the single generated skill with two, both carrying the managed marker:
  - `use-superorchestrator-mcp`: delegation through the MCP tools (route, spawn, brief format,
    wait, messages, review, subagent rules) plus the generated accounts, choices and agents lists.
  - `superorchestrator-cli`: the CLI for people and agents (setup, accounts, login, status,
    doctor, runs, join, ui, workflow, troubleshooting pointers). Static text.
- `install` writes both to `~/.claude/skills/` and `~/.agents/skills/`, and removes the old
  `superorchestrator` skill dir only when its `SKILL.md` carries our marker. `uninstall` removes both.
  Refresh (`ctx.refresh_skill`, web `refresh_skill`) regenerates the MCP skill.
- `install` adds a short managed block to `~/.claude/CLAUDE.md` (created if missing), between
  `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->`: delegate implementation
  through superorchestrator, load `use-superorchestrator-mcp` before the first delegation, use
  `superorchestrator-cli` for accounts and status. Re-install replaces the block in place; text
  outside it is never touched. `--no-claude-md` skips it. `uninstall` removes the block only.
  The file is backed up the same way other install edits are.
- The plugin (`plugin/skills/`) ships both skills as static files.

Acceptance: unit and CLI tests for install/uninstall (block insert, replace, remove, untouched
outside text, marker-only removal of the old skill, `--dry-run`); existing install tests pass.

Out of scope: setup wizard, account commands.

## T3: bulk accounts and `setup` (implementer-sonnet, after T1 merges)

- `account add <harness> [--name N] [--count K] [--login]`. Without `--name`, names are
  `<harness>-<n>` with the lowest free `n` starting at 1. `--count K` creates K accounts
  (`--name` with `--count` > 1 is an error). `--login` runs `account login` for each new
  account in turn when stdin is a TTY; otherwise it prints the commands.
- `superorchestrator setup`: the first command a new user runs. Idempotent. Steps:
  1. `init` (silent when nothing is created).
  2. Detect installed harnesses (as `doctor` does) and print them.
  3. Offer `account import --scan` results.
  4. For each installed harness, ask how many accounts to add (default 0, or 1 when the harness
     has no account yet), then add them and offer to sign in to each.
  5. Run `install`.
  6. Print a short `doctor` summary and "next: open Claude Code and ask it to delegate a task".
  Non-interactive: `--yes` accepts defaults, `--codex N --claude N --opencode N --agy N` set counts,
  `--no-login`, `--no-install`. Without a TTY and without `--yes`, it prints what it would do and exits 0.
- Help: hide `exec`, `daemon` and `mcp` from the top-level list (they keep working), order
  commands by use (setup, status, account, route, runs, join, ui, workflow, agent, models,
  config, doctor, install, uninstall, migrate, worktrees), and add an after-help line
  "Get started: superorchestrator setup". `init`'s next steps point to `setup`.
- `install.sh` ends with "run `superorchestrator setup`".

Acceptance: CLI tests for naming, count, conflicts, setup non-interactive paths and dry output;
help snapshot test; `cargo test -p superorchestrator`.

Out of scope: web UI changes.

## T4: docs (implementer-haiku after T2 and T3)

- README: lead with use cases (spread work across several subscriptions; Claude Code as the
  orchestrator delegating to Codex/OpenCode/agy; reviewed delegation; workflows; watching runs in
  the web UI), then install, `setup`, first delegation, links.
- New `docs/guide/use-cases.md` with one worked example per use case.
- Update `accounts.md` (bulk add, naming rule), `troubleshooting.md`, `CHANGELOG.md`, SPEC
  sections for `setup`, `account add --count`, the two skills and the CLAUDE.md block.

## T5: real-session testing (controller, after T3)

Run real Claude Code sessions against the built binary in an isolated config with the real
accounts (as `scripts/smoke.sh` does) over varied tasks: single delegation, parallel runs,
review accept and reject, `ask_parent`, cancel, a workflow, and a decider outage. File each bug
found as a task and fix it before release.

## T6: release

Date the CHANGELOG, merge the PR, tag `v0.1.0`, watch the release workflow.
