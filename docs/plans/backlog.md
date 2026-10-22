# Hardening backlog

Minor review findings deferred to M9 hardening. Each line names the file and the fix.

- `crates/superorchestrator/src/redact.rs`: oversize flush can split `Bearer <token>` across chunks; carry a "previous word was Bearer" flag.
- `crates/superorchestrator/src/cmd/account.rs`: argument-validation errors exit 1; route through `CliError::new(USAGE, ..)`.
- `crates/superorchestrator/src/views.rs`: `active_runs` shows 0 when the store is unavailable; emit `null` / "-".
- `crates/superorchestrator/src/cmd/daemon.rs`: tighten an existing logs dir to 0700.
- `crates/so-store`: `patch_run` can write `finished_at`/`error_kind` outside `finish_run`; narrow the patch.
- `crates/so-supervisor`: idle-exit negative tests use timed sleeps; server writer timeout and join-error logging have no tests.
- `crates/so-mcp`: progress for `spawn_agent wait` and `run_workflow` is connection-scoped, not per call.
- `crates/so-accounts`: failed `add` leaves the new home behind; `reserve_backup` leaks an empty file if both rename and copy fail.
- `crates/so-supervisor/src/runs/view.rs`: `run_view` loads every event of a run to sum usage; keep a running total or query sums.
- `crates/so-web/src/stream.rs`: the SSE pump reads `list_headroom` (all rows) on each `usage` event; query by account.
- `crates/so-web/src/api/sessions.rs`: the sessions list does a stats query per session (N+1) and scans up to 5000 sessions; push filters into SQL.
- `crates/so-web/src/security.rs`: the rate limiter `clear()`s everything at capacity and treats IPv4-mapped IPv6 as a different address; normalise and evict the oldest instead.
- `crates/so-web/src/stream.rs`: a lagged broadcast receiver sends `resync` but not a replay floor; message size is uncapped.
- `crates/so-supervisor/src/runs/events.rs`: the firehose subscription uses an unbounded mpsc; bound it and resync on overflow.
- `crates/so-supervisor/src/runs/events.rs`: `RunManager::emit` is now `pub` for tests; gate it behind a test-support feature or a narrower method.
- `crates/so-web/src/assets.rs`: static files have no ETag/conditional GET; add a hash ETag for non-hashed files.
- `crates/so-supervisor/src/runs/comm.rs` `unclaim`: log a failed put-back instead of ignoring it, since it can leave a message marked read.
- `crates/so-supervisor/tests/comm.rs`: the cancel-during-gate test never exercises `relabel_attempt`. Add a unit test, and give the reject-after-gate timing check a margin.
- `crates/so-supervisor/src/runs/resume.rs`: the gate is held across `wait_wrapped_up` (up to 15 s) and the worktree recreate. Narrow the hold if contention shows.
- `web/src/components/common/ScrollTable.tsx`: the sticky first column's `bg-background` hides row hover and selected backgrounds. Give the ScrollArea viewport `tabIndex={0}` and an aria-label so the keyboard can scroll it sideways.
- `web/src/api/msw/handlers.ts`: the `/accounts/import` mock does not record the harness, so imported claude accounts get a device-code login in mock mode.
- `crates/so-supervisor/src/runs/{comm,lifecycle}.rs`: `append_run_turn_prompt` errors are ignored. A failed write drops that message from a later failover; at least log it.
- `crates/so-supervisor/src/runs/routing.rs`: re-routing a queued first-spawn agent run with tool controls does not filter out harnesses with tool gaps. Only pending failovers are filtered.
- `crates/so-harness/src/{codex,opencode}.rs`: startup failures carry no process output, so a codex or opencode account that is not signed in or out of quota at launch fails as `other` with no failover. Capture the process's stderr and return `DriverError::Exited`.
