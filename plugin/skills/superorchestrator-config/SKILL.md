---
name: superorchestrator-config
description: Configure superorchestrator workflows, accounts, agent profiles, routing and levels.
---

# Configure superorchestrator

Use the selected config directory (`--config-dir DIR`, otherwise `$SUPERORCHESTRATOR_CONFIG_DIR` or `~/.config/superorchestrator`). Run `superorchestrator init` if new. Commands below use that same directory.

## Workflow

Save `workflows/review.js` under the config directory. This uses the API in [workflows guide](https://github.com/datamonsterr/superorchestrator/blob/main/docs/guide/workflows.md):

```js
const reviews = await parallel([
  (agent) => agent("Review parser correctness"),
  (agent) => agent("Review lexer correctness"),
]);
return await pipeline([
  () => agent("Summarize these reviews: " + JSON.stringify(reviews)),
  (summary) => { log(JSON.stringify(summary)); return summary; },
]);
```

Run `superorchestrator workflow run review --args '{}'`; `--fresh` bypasses cached steps.
`parallel` supplies an agent that defaults to worktree isolation. Sequential `pipeline([fn, ...])` passes each result to the next stage. Handle `null` for failed parallel items. Keep scripts deterministic; pass variable inputs through `args`. Step `cwd` must resolve inside the workflow base. Workflow steps cannot use `review: true`.

## Account

`superorchestrator account add codex --name codex-review --login` creates an account and signs in. For an existing credential home, use `superorchestrator account import codex-review --harness codex --home /path/to/credentials`. Inspect with `superorchestrator status`.

## Agent profile

`superorchestrator agent create reviewer --description "Review correctness"`, then edit `agents/reviewer/AGENT.md` in the config directory (or use `superorchestrator agent edit reviewer`):

```yaml
---
name: reviewer
description: Review correctness
harness: auto
isolation: worktree
review: true
maxTurns: 40
---
Review the requested diff and report concrete bugs with file references.
```

Per [agents guide](https://github.com/datamonsterr/superorchestrator/blob/main/docs/guide/agents.md), other fields: `accounts`, `model`, `effort`, `tools`, `disallowedTools`, `skills`, `mcpServers`, `hooks`, `memory` (`user`, `project`, `local`). `description` is required. Model refs in `model` set harness and effort too; explicit fields must agree. Call arguments override the agent profile, then config defaults, then routing.
Project `.superorchestrator/agents/` shadows user profiles, then `.claude/agents/` imports. Project profiles reject `hooks`, command-capable inline `mcpServers`, `isolation: none`, and `extra_args`; review definitions before moving them into user config. `superorchestrator agent show reviewer` reports harness gaps.

## Routing and levels

Inspect `superorchestrator levels list` and `superorchestrator config get router.levels`. List model refs in preference order; each level needs models or a tier (`fast`, `mid`, `high`). Complexity ranges are 0–100 and cannot overlap.

| Change | Command |
|---|---|
| Prefer a model | `superorchestrator levels model add complex codex/gpt-6.1-sol/high --position 1` (use `levels model move` if already listed) |
| Remove a model | `superorchestrator levels model remove complex codex/gpt-6.1-sol/high` |
| Adjust a range | `superorchestrator levels edit complex --min 63 --max 87` |
| Replace levels | `superorchestrator config set router.levels '[{name="all", min=0, max=100, models=["codex/gpt-6.1-sol/high"]}]'` |
| Edit routing | `superorchestrator config edit` (the `[router]` settings in `config.toml`) |
| Verify selection | `superorchestrator route "Review parser correctness" --agent reviewer --explain` |

Level edits take effect after the supervisor restarts; do not restart it while runs are open. Preserve unrelated config and check ranges/models before applying changes.
