# Troubleshooting

Start with `superorchestrator doctor`. It prints one line per check. A check is `✓` (ok), `!` (warning) or `✗` (failed). It exits non-zero when a check failed. `--json` gives the same data for scripts.

## What doctor checks

| Check | Meaning and fix |
|---|---|
| `codex`, `agy`, `opencode`, `claude` | The binary is on `PATH` and answers `--version`. A failure means an enabled account needs it. Install the harness or fix `PATH`. A warning means no enabled account needs it. |
| `bwrap` (Linux) | Optional. Without it, agy runs start without `--die-with-parent`. Install `bubblewrap`. |
| `config dir` | The directory is missing. Run `superorchestrator init`. |
| `config.toml` | It does not load. Read the error, then `superorchestrator config edit`. A missing file is a warning, and defaults apply. |
| `choices` | Codex choices from an older `init` are still off. Set `enabled = true` on them. |
| `state.db` | `not created yet` is normal before first use. A failure means the database does not open. |
| `supervisor` | `not running` is normal, because it starts on demand. `stale socket` means the socket file is left over from a crash. Remove the file, or run `superorchestrator daemon`. |
| `ui port` | The UI port is free, or held by the supervisor. A warning means another process has it. Change `ui.port`. |
| `account <name>` | `signed out` means the harness has no login. See [accounts](accounts.md#logging-in-per-harness). `unknown` is fine (Claude keyring). |
| `agy accounts` (macOS) | Only one agy account may be enabled. Disable the others. |

`SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS` raises the 5 second `--version` timeout on a slow machine.

## Common problems

**Claude Code does not list the tools.** Run `superorchestrator install` (or install the plugin, not both), then restart Claude Code. `claude mcp list` should show `superorchestrator`.

**`harness: auto` never picks an account.** Run `superorchestrator route "task" --explain`. The choice may be disabled, the account signed out, flagged, exhausted or at its cap. Check `superorchestrator status` and `superorchestrator account list`.

**A run sits in `queued`.** An account cap or `limits.global_cap` is full. Wait, cancel a run (`superorchestrator runs cancel <id>`), or raise a cap.

**A run is `stalled` or `escalated`.** No event arrived for `runs.stall_ms`. The supervisor nudges it three times, then escalates. Open it in the UI, or run `superorchestrator join <run>` and look.

**A run failed with a quota or auth error.** The account is flagged until its reset. Failover moves the work to another account when one exists. Clear a flag with `superorchestrator account flag <name> --clear`.

**I installed a fix but behaviour is unchanged.** The daemon keeps running the binary it started with, so reinstalling does not reach it. `superorchestrator status` shows the daemon's version and build id next to the CLI's, and prints a warning when they differ or the daemon's executable was replaced on disk. The MCP server warns in its instructions the same way, and `list_harnesses` carries `stale: true` with a reason. Restart the daemon: wait for active runs to finish, `kill` the pid `status` shows, and run any command (it starts the new binary on demand). Stopping the daemon ends its active runs. Nothing restarts it for you. The check compares the file's inode, size and mtime, so `touch` or a reinstall of identical content also shows as changed.

**The wrapper command is not found.** Put `~/.local/bin` (or your `accounts.wrapper_dir`) on `PATH`.

**The skill is out of date.** Superorchestrator regenerates `use-superorchestrator-mcp` when accounts, choices or agents change, but only if it is installed and the file still has the `superorchestrator:managed` marker. It also writes `superorchestrator-cli` if that is missing. Run `superorchestrator install` to write them. A skill file you edited by hand without the marker is left alone.

**`~/.claude/CLAUDE.md` has a block I did not write.** `install` adds a short delegation block between `<!-- superorchestrator:begin -->` and `<!-- superorchestrator:end -->`. Re-running `install` replaces it in place, `install --no-claude-md` skips it and `uninstall` removes only the block. Text outside the markers is never touched.

**The UI does not load.** Run `superorchestrator ui` and read the URL. A token is asked for on non-loopback binds.

**Old worktrees pile up.** `superorchestrator worktrees list`, then `worktrees prune`.

## Logs and state

- Supervisor log: `~/.config/superorchestrator/logs/supervisor.log`. Set `SUPERORCHESTRATOR_LOG=debug` for more.
- Run files: `~/.config/superorchestrator/runs/<run_id>/`.
- `superorchestrator runs show <run>` shows state, recent events, result and join command.

## Reset

Stop the supervisor, then move the config directory aside. `init` makes a fresh one. Your credential homes are not in it.
