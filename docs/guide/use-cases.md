# Use cases

One worked example for each way to use superorchestrator. Each example gives the commands or prompts to use and what you will see. Set up first: see [getting started](getting-started.md).

Run ids look like `01J...`. Every command that takes a run id also takes a unique prefix.

## 1. Spread work across several subscriptions

You have more than one Codex, OpenCode or agy login and want to use all of them. Superorchestrator treats each login as an account. It picks the account with the most room and moves to the next one when a limit is hit.

Add three Codex accounts and one OpenCode account, then sign in:

```sh
superorchestrator account add codex --count 3 --login
superorchestrator account add opencode --login
superorchestrator account list
```

`--count 3` creates `codex-1`, `codex-2` and `codex-3` (the lowest free numbers). `--login` signs in to each in turn when you have a terminal, and prints the commands otherwise. If you already have logins on disk, link them instead: `superorchestrator account import --scan`.

`account list` shows each account with its sign-in state, load and headroom. Preview which account a task would use. This runs nothing:

```sh
superorchestrator route fix the flaky retry test --explain
```

You see the chosen model ref (`choice_id`), harness, account and model, with the reason and the runner-up models.

When an account runs out, ask Claude Code to start several tasks. Runs spread over the accounts. If a run fails with a quota or rate-limit error, that account is marked exhausted until its reset time, and the run restarts on the next account. The run keeps its id, and the timeline shows each attempt. To take an account out by hand for five hours:

```sh
superorchestrator account flag codex-1 --hours 5
superorchestrator account flag codex-1 --clear
```

More: [accounts](accounts.md), [routing](routing.md#failover).

## 2. Claude Code as the orchestrator

Claude Code keeps the plan and the review. Other agents do the bulk work, so your Claude limits last longer.

After `superorchestrator setup`, open Claude Code in your project and ask:

> Use superorchestrator to add input validation to `src/api/users.rs`. Send it to a cheap agent, wait for it, and summarise what changed.

Claude Code loads the `use-superorchestrator-mcp` skill and then calls:

1. `list_harnesses` to see the accounts, levels and agents;
2. `spawn_agent` with your task (`harness` stays `auto`, so the router picks);
3. `wait_agent` until the run finishes;
4. `get_run` for the result.

To pin a harness, say so in the prompt: "run it on opencode". Claude Code passes `harness: "opencode"`. To use a saved custom agent, name it: "use the `pr-reviewer` agent on this branch". See [custom agents](agents.md).

You see the run in `superorchestrator runs list` while it works. Claude accounts rank last in routing, so the orchestrator keeps its limits.

## 3. Reviewed delegation

You want to read the work before it lands. Ask for a review gate:

> Delegate "rename `Config::load` to `Config::read` across the repo" with superorchestrator. Use review with the check `cmd:cargo test`. When it stops, show me the diff.

Claude Code calls `spawn_agent` with `review: true` and `checks: ["cmd:cargo test"]`. The run works in its own git worktree. When it finishes and the check passes, it stops at `pending_review`. Claude Code then calls `get_run` with `diff: true` and shows you the branch, the `git diff --stat` and the patch.

From a shell, see the same diff with:

```sh
superorchestrator runs show 01J --diff
```

Then pick one:

- **Reject.** Say "the rename missed the docs; send it back". Claude Code calls `reject_run` with your feedback. The same session continues and stops at the gate again.
- **Accept and land.** Say "accept it and merge". Claude Code calls `accept_run` with `merge: true`. The branch merges into the checkout the run started from, and the worktree is removed.
- **Accept without merging.** `accept_run` alone marks the run `succeeded`. The result gives the `branch` and a `merge_command` to run yourself.

A merge is refused, and the run stays at `pending_review`, when your checkout has changes to tracked files or is on another branch, or when the merge conflicts. Nothing is pushed. See [reviewing and landing runs](review.md).

## 4. Parallel runs and JavaScript workflows

Ask for several tasks at once:

> Start three superorchestrator runs in parallel, one per package: add tests to `parser`, `lexer` and `printer`. Wait for all three.

Claude Code calls `spawn_agent` three times without `wait`, then `wait_agent` with the three run ids. Each run gets its own worktree.

For a repeatable job, write a workflow. Save this as `.superorchestrator/workflows/review-two.js` in your project:

```js
const [a, b] = await parallel([
  (agent) => agent("Review the parser for bugs", { harness: "codex", name: "parser" }),
  (agent) => agent("Review the lexer for bugs", { harness: "opencode", name: "lexer" }),
]);
return { a, b };
```

Run it from a shell:

```sh
superorchestrator workflow run review-two
superorchestrator workflow list
```

The first command prints the result when both steps finish. In another terminal, follow the steps with `superorchestrator runs watch --workflow <id>` (the id comes from `workflow list`). Or run it from Claude Code with the `run_workflow` tool. Run it again and finished steps come back from the cache with state `cached`; pass `--fresh` to run all of them again.

Workflow steps cannot use `review: true`: nobody is there to accept the run. Review the result in the workflow's return value instead. See [workflows](workflows.md).

## 5. Watch runs

Open the web UI:

```sh
superorchestrator ui
```

It opens `http://127.0.0.1:7878`. The dashboard shows accounts, headroom and active runs. Open a session to see a timeline with one lane per run. Click a lane for the brief, the route decision, events, transcript, result and worktree, with buttons to message, cancel, accept or reject.

From a shell:

```sh
superorchestrator runs list
superorchestrator runs watch 01J
superorchestrator runs show 01J
superorchestrator join 01J --exec
```

- `runs list` shows active runs (`--all` adds finished ones).
- `runs watch` follows a run's events until it finishes.
- `runs show` prints the state, recent events and result.
- `join --exec` attaches to the run's own session in its harness. Without `--exec` it prints the command.

See the [web UI guide](web-ui.md).
