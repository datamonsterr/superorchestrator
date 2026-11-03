# The supervisor writes the decider key file

Status: accepted · 2026-11-04

## Context

The routing decider needs an OpenRouter key. Until now there were two ways to supply it: export the variable named by `router.jev.key_env` in the supervisor's environment, or write a dotenv file by hand and point `router.jev.key_file` at it. Users who set up the decider from the web UI or the TUI wanted to paste the key into a form. Neither existing way allows that, and both need a shell and, for the environment, a supervisor restart under the right environment.

## Decision

1. **The supervisor writes the key file.** When a key is pasted, the supervisor writes `<config dir>/decider.env` with the line `<key_env>=<key>`. It creates the file with mode 0600 (never chmod after writing) and replaces it atomically. It then sets `router.jev.key_file` to that path. The web API calls this in process and the TUI calls it through the `decider_set` RPC method on an inspect connection, so one implementation serves both.
2. **The key is never returned, logged or stored.** API responses, `GET /api/v1/config`, tracing output and events carry no key. The view shows the source (`env`, `file` or `unset`), the variable or file name and the key masked to its last four characters. A `Debug` of the patch redacts the pasted value. The web form clears the field after saving and keeps the value out of form state and the mutation cache.
3. **The environment still wins.** The `key_env` variable, when set, shadows `key_file`, and `SUPERORCHESTRATOR_ROUTER_MODEL` and `_FALLBACK_MODEL` shadow the model fields. The view reports each override so the UI can say so.
4. **Failure is atomic.** If the config write fails after the key file was written, the key file is restored to its previous state.

## Consequences

- The key is at rest on disk, protected by file permissions only. This is the same trust level as the harness credential homes, which also hold secrets in the user's config directory with owner-only modes. Anyone who can read the user's files can read the key. The environment variable remains the choice for people who do not want a key on disk.
- A rollback path exists, so a failed save leaves neither a new key file nor a half-updated config.
- A pasted key is shadowed when the `key_env` variable is set in the supervisor's environment. The UI says so, with the variable name, instead of silently using the other key.
- The key file is part of the config directory, so backups of that directory include it.
