# Workflows

A workflow is a JavaScript script that starts many agents and combines their results. It runs in an embedded QuickJS engine. The script API matches Claude Code workflow scripts. Any harness can run a step.

## Running a workflow

- MCP: the `run_workflow` tool takes a saved name or a script, and `args`. Inline `script` is restricted to orchestrators (depth 0). Subagents may run saved workflows by `name`. `workflow_status` reports on a run.
- CLI: `superorchestrator workflow run <name>` or `workflow run --file script.js`. Add `--args '<json>'` to pass `args`, `--detach` to return the run id at once, and `--fresh` to ignore cached results.
- `superorchestrator workflow list` shows saved workflows and recent runs. `workflow status <id>` shows steps and the result. `workflow cancel <id>` stops a run and its steps.
- `superorchestrator runs watch --workflow <id>` follows the steps live.

## Saved workflows

Save a script as `<name>.js` in `~/.config/superorchestrator/workflows/` or `.superorchestrator/workflows/` in your project. Run it by name. A project script hides a config script of the same name. The web UI lists them on the Workflows page.

## Script API

```js
phase("review", async () => {
  const [a, b] = await parallel([
    (agent) => agent("Review the parser for bugs", { harness: "codex" }),
    (agent) => agent("Review the lexer for bugs", { harness: "agy" }),
  ]);
  log("both reviews done");
  return { a, b };
});
```

| Name | Behaviour |
|---|---|
| `agent(prompt, opts)` | Starts a run and returns its final result. `opts` are the spawn options (`harness`, `account`, `model`, `effort`, `cwd`, `isolation`, `output_schema`, `max_turns`, `timeout`, `name`, `agent`). camelCase works (`maxTurns`). `schema` means `output_schema` and `label` means `name`. A failed step throws. `review: true` is rejected: nobody can accept the run, so use the workflow's own result to review. |
| `parallel([...])` | Runs the items at once. Items are functions that get a worktree-defaulting `agent`, or promises. A failed item gives `null`. |
| `pipeline(items, fn)` | Runs `fn` for each item at once. A failed item gives `null`. |
| `pipeline([fn, ...])` | Runs stages in order. Each stage gets the previous result. |
| `phase(name, fn)` | Groups steps under a name in the status and the UI. |
| `log(msg)` | Writes a progress line. |
| `args` | The JSON you passed, or `undefined`. |

Every step's `cwd` resolves relative to the workflow base and must canonicalise to an existing directory inside it. Parent-directory escapes, absolute paths outside the base and symlinks leading outside are rejected before cache lookup or spawn. This applies to all workflows, including those started at depth 0. A subagent-started workflow uses the calling run's worktree, or its cwd when there is no worktree, as its base.

`parallel()` defaults `isolation` to `worktree` for the steps it starts. Calls made after an `await` inside an item keep their own options.

`stallMs` in `opts` sets the stall window for one step. A stalled step restarts up to 5 times.

Scripts must be deterministic, because resume replays them. `Date.now()`, `new Date()` with no argument and `Math.random()` throw. Pass such values in through `args`. Workflows take no input from a person while they run.

## Cache and resume

Each `agent()` result is stored in `state.db`. The key is a hash of the prompt, the options, the workflow name, the working directory and how many times the same call came before. Run the same workflow again and cached steps return at once with state `cached`. Only the remaining steps run. That makes a failed or cancelled workflow cheap to resume. Pass `--fresh` to run every step again. New results are still stored.

If you change a prompt or an option, that step and the steps that depend on it run again.

## Failover and limits

A step that hits a usage limit fails over as in [routing](routing.md). If no account is available, the step waits until the earliest reset time. `workflows.max_duration` (default `6h`) bounds one whole run, waits included. It also stops a script that never yields.
