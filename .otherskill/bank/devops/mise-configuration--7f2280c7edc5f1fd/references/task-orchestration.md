# Task Orchestration Integration

When you detect **workflow opportunities** during mise configuration, prescriptively invoke the `mise-tasks` skill.

## Detection Triggers

- Multi-step workflows (test + build + deploy)
- Repeatable project commands (lint, format, release)
- Dependency chains (db-migrate depends on db-init)
- File-tracked builds (sources/outputs caching)

## Prescriptive AI Reminder

> **PRESCRIPTIVE**: When detecting multi-step project workflows, **invoke [`mise-tasks` skill](../../mise-tasks/SKILL.md)** to generate appropriate `[tasks]` definitions with dependency management.

## Example: From Environment to Tasks

**Step 1**: Configure environment (mise-configuration skill):

```toml
[env]
DATABASE_URL = "postgresql://localhost/mydb"
_.python.venv = { path = ".venv", create = true }
```

**Step 2**: Define tasks (`mise-tasks` skill):

```toml
[tasks.test]
depends = ["lint"]
run = "pytest tests/"

[tasks.deploy]
depends = ["test", "build"]
run = "deploy.sh"
```

Tasks automatically inherit `[env]` values.
