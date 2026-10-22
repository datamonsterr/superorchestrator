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
