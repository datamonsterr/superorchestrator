# Account headroom refresh

Goal: populate shared usage records from the installed Codex and agy quota interfaces so status, account lists, board and TUI show measured headroom.

1. Verify current harness protocols and diagnose missing data. Acceptance: read repository guidance, installed help, current official Codex docs and read-only real quota results; identify root cause. Out of scope: credentials, account config and permission changes.
2. Connect existing quota probes to user-facing refresh paths and the supervisor lifecycle. Acceptance: enabled signed-in Codex/agy accounts refresh on TTL; periodic supervisor refresh covers board; CLI and TUI work without a supervisor; errors preserve prior readings; simultaneous supervisor refreshes do not multiply probes. Out of scope: UI redesign and Claude/OpenCode usage changes.
3. Add regressions using fake harness executables and run formatting, cargo check, cargo clippy, changed-crate tests, workspace tests and a safe real-account smoke. Acceptance: all checks pass; report root cause, measured headroom and limitations. Out of scope: deployment/restarting active supervisor.

## Findings

The active `UsageService::refresh` and `refresh_all` methods had no production callers. CLI/TUI views and the supervisor account API only read stored headroom. Claude populated those records through passive events, while idle Codex and agy accounts remained unmeasured. The existing Codex and agy protocols and parsers work on the installed binaries; no harness protocol change was needed. The text formatter also discarded weekly percentages when five-hour data was absent.

Verified codex-cli 0.164.0 with `codex --help`, `codex app-server --help`, and a live `initialize` / `initialized` / `account/rateLimits/read` exchange. Current official protocol documentation: https://learn.chatgpt.com/docs/app-server. Verified `agy --help` and the installed hidden `--gemini_dir=<home>/.gemini --print /usage` command against real accounts. These read quota without starting an inference turn.

The fix uses startup and periodic supervisor polling, synchronous CLI refresh, and background TUI refresh. All paths share account filtering and existing store freshness checks. Failed attempts are throttled in long-lived callers, retain stored readings, and return diagnostics without raw harness output. Real-account CLI and supervisor account API smoke checks returned measured headroom for every configured Codex and agy account. Account home configs and approval/sandbox settings were not edited.

## Validation

- `cargo check` passed.
- `cargo clippy --workspace --all-targets -- -D warnings` passed.
- `cargo test -p so-usage -p so-supervisor -p superorchestrator` passed (581 tests reported passed, two existing ignored).
- `TMPDIR=/tmp cargo test --workspace -- --skip a_step_waiting_for_a_permit_is_queued_without_an_event` passed (1,217 tests reported passed, three existing ignored).
- `cargo fmt --all --check` and `git diff --check` passed.

The initial workspace invocation inherited a long run-specific `TMPDIR` that exceeded Unix socket path limits in existing agy harness tests; setting `TMPDIR=/tmp` resolved it. The unfiltered workspace run then stalled in the unchanged workflow-engine permit-queue test named above. That test passed in isolation; the rest of the workspace passed with only that test skipped. Workflow code was left unchanged.

The running supervisor was an older build and was not restarted while runs were active. Both the rebuilt CLI and the running supervisor's account API read the newly populated shared records. Automatic background refresh uses the new supervisor code after deployment/restart.
