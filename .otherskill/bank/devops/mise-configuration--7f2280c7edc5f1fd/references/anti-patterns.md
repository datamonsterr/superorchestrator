# mise Configuration Anti-Patterns

## General Anti-Patterns

| Anti-Pattern                   | Why                             | Instead                                    |
| ------------------------------ | ------------------------------- | ------------------------------------------ |
| `mise exec -- script.py`       | Forces mise dependency          | Use env vars with defaults                 |
| Secrets in `.mise.toml`        | Visible in repo                 | Use Doppler or `redact = true`             |
| No defaults in scripts         | Breaks without mise             | Always provide fallback                    |
| Mixing env/tools resolution    | Order matters                   | Use `tools = true` for tool-dependent vars |
| `[env]` secrets for pueue jobs | Pueue runs clean shell, no mise | Use `python-dotenv` + `.env` file          |
| `__MISE_DIFF` leaks via SSH    | Remote trust errors             | `unset __MISE_DIFF` before SSH             |

## Critical: mise `[env]` Secrets and Non-Interactive Shells

**Do NOT** put secrets in `mise.toml [env]` if they will be consumed by pueue jobs, cron jobs, or systemd services. These execution contexts run in clean shells without mise activation — `[env]` variables are invisible.

**Preferred pattern**: Use `mise.toml` for task definitions and non-secret configuration only. Put secrets in a `.env` file (gitignored) and load them with `python-dotenv` at runtime:

```toml
# mise.toml — tasks only, no secrets in [env]
[env]
DATABASE_NAME = "mydb"       # OK: non-secret defaults

[tasks.backfill]
run = "bash scripts/backfill.sh"
```

```bash
# .env (gitignored) — secrets loaded by python-dotenv
API_KEY=sk-abc123
DATABASE_PASSWORD=hunter2
```

This works identically in interactive shells, pueue jobs, cron, systemd, and across macOS/Linux.

**Cross-reference**: See `devops-tools:distributed-job-safety` — [G-15](../../distributed-job-safety/references/environment-gotchas.md), [AP-16](../../distributed-job-safety/SKILL.md)
