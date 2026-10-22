# Hub-Spoke Architecture for mise Configuration

Keep root `mise.toml` lean by delegating domain-specific tasks to subfolder `mise.toml` files.

> **Wiki Reference**: [Pattern-mise-Configuration](https://github.com/terrylica/cc-skills/wiki/Pattern-mise-Configuration) - Complete documentation with CLAUDE.md footer prompt

## When to Use

- Root `mise.toml` exceeds ~50 lines
- Project has multiple domains (packages, experiments, infrastructure)
- Different subfolders need different task sets

## Spoke Scenarios

Hub-spoke applies to any multi-domain project, not just packages:

| Scenario           | Spoke Folders                                      | Spoke Tasks                      |
| ------------------ | -------------------------------------------------- | -------------------------------- |
| **Monorepo**       | `packages/api/`, `packages/web/`                   | build, test, lint, deploy        |
| **ML/Research**    | `experiments/exp-001/`, `training/`, `evaluation/` | train, evaluate, notebook, sweep |
| **Infrastructure** | `terraform/`, `kubernetes/`, `ansible/`            | plan, apply, deploy, validate    |
| **Data Pipeline**  | `ingestion/`, `transform/`, `export/`              | extract, load, validate, export  |

## Directory Structure Examples

**Monorepo**:

```
project/
├── mise.toml              # Hub: [tools] + [env] + orchestration
├── packages/
│   ├── api/mise.toml      # Spoke: API tasks
│   └── web/mise.toml      # Spoke: Web tasks
└── scripts/mise.toml      # Spoke: Utility scripts
```

**ML/Research Project**:

```
ml-project/
├── mise.toml              # Hub: python, cuda, orchestration
├── experiments/
│   ├── baseline/mise.toml # Spoke: baseline experiment
│   └── ablation/mise.toml # Spoke: ablation study
├── training/mise.toml     # Spoke: training pipelines
└── evaluation/mise.toml   # Spoke: metrics, benchmarks
```

**Infrastructure**:

```
infra/
├── mise.toml              # Hub: terraform, kubectl, helm
├── terraform/
│   ├── prod/mise.toml     # Spoke: production infra
│   └── staging/mise.toml  # Spoke: staging infra
└── kubernetes/mise.toml   # Spoke: k8s manifests
```

## Hub Responsibilities (Root `mise.toml`)

```toml
# mise.toml - Hub: Keep this LEAN

[tools]
python = "<version>"
uv = "latest"

[env]
PROJECT_NAME = "my-project"
_.python.venv = { path = ".venv", create = true }

# Orchestration: delegate to spokes
[tasks.train-all]
run = """
cd experiments/baseline && mise run train
cd experiments/ablation && mise run train
"""

[tasks."build:api"]
run = "cd packages/api && mise run build"
```

## Spoke Responsibilities (Subfolder `mise.toml`)

```toml
# experiments/baseline/mise.toml - Spoke

[env]
EXPERIMENT_NAME = "baseline"
EPOCHS = "<num>"           # e.g., 100
LEARNING_RATE = "<float>"  # e.g., 0.001

[tasks.train]
run = "uv run python train.py"
sources = ["*.py", "config.yaml"]
outputs = ["checkpoints/*.pt"]

[tasks.evaluate]
depends = ["train"]
run = "uv run python evaluate.py"
```

## Inheritance Rules

- Spoke `mise.toml` **inherits** hub's `[tools]` automatically
- Spoke `[env]` **extends** hub's `[env]` (can override per domain)
- `.mise.local.toml` applies at directory level (secrets stay local)

## Anti-Patterns

| Anti-Pattern           | Problem                      | Fix                          |
| ---------------------- | ---------------------------- | ---------------------------- |
| All tasks in root      | Root grows to 200+ lines     | Delegate to spoke files      |
| Duplicated [tools]     | Version drift between spokes | Define [tools] only in hub   |
| Spoke defines runtimes | Conflicts with hub           | Spokes inherit hub's [tools] |
| No orchestration       | Must cd manually             | Hub orchestrates spoke tasks |
