# Monorepo Workspace Pattern

For Python monorepos using `uv` workspaces, the venv is created at the **workspace root**. Sub-packages share the root venv.

```toml
# Root mise.toml
[env]
_.python.venv = { path = ".venv", create = true }
```

## Hoisted Dev Dependencies (PEP 735)

Dev dependencies (`pytest`, `ruff`, `jupyterlab`, etc.) should be **hoisted to workspace root** `pyproject.toml` using `[dependency-groups]`:

```toml
# SSoT-OK: example workspace configuration
# Root pyproject.toml
[tool.uv.workspace]
members = ["packages/*"]

[dependency-groups]
dev = [
    "pytest>=<version>",
    "ruff>=<version>",
    "jupyterlab>=<version>",
]
```

**Why hoist?** Sub-package `[dependency-groups]` are NOT automatically installed by `uv sync` from root. Hoisting ensures:

- Single command: `uv sync --group dev`
- No "unnecessary package" warnings
- Unified dev environment across all packages

> **Reference**: [bootstrap-monorepo.md](../../mise-tasks/references/bootstrap-monorepo.md#root-pyprojecttoml-workspace) for complete workspace setup
