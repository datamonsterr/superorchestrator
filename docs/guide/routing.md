# Routing and choices

When a spawn uses `harness: auto` (the default), the router picks the account, model and effort. This page explains how, and how to tune it.

## Choices

A choice is one routable combination of harness, model and effort that you approved. A choice binds the three together, so the router can never produce an unsupported mix. Choices live in `config.toml`:

```toml
[[choices]]
id = "agy-flash-medium"
harness = "agy"
model = "gemini-3.8-flash-medium"
effort = "medium"
roles = ["worker"]
description = "Fast worker for bounded implementation, scripts, docs and routine UI work."
enabled = true
```

`init` seeds a set of choices. Codex choices start enabled. A choice with no signed-in account is skipped at routing time, so adding a Codex account is enough to use them. Claude choices start disabled. Set `enabled = true` to let auto routing use them as a last resort. Only you change `enabled`; `init` never rewrites it after the first run.

Edit choices with `superorchestrator config edit`, or on the Choices page of the web UI. The installed `use-superorchestrator-mcp` skill lists enabled choices and refreshes when they change.

## Previewing a route

```sh
superorchestrator route "Rename the foo module to bar" --explain
```

`route` previews only. It reserves nothing. The task needs no quotes: `route fix a typo in README` routes "fix a typo in README". A task that starts with `eval` or `-` needs `--` first, as in `route -- eval the parser`, because `route eval` is the evaluation command. Use `--role` to route for another role (default `worker`). The MCP tool `route` does the same.

## How the router decides

1. **Eligible choices.** A choice is eligible when it is enabled, matches the role, and has an account that is signed in, not flagged, not exhausted and below its cap.
2. **Decider.** If a Jev key is set (below), the router asks the decider to pick among the eligible choices.
3. **Validation.** The answer is rejected unless it is a valid pick from the eligible set, with probabilities that sum to 1 and a confidence and margin above `router.policy.min_confidence` (0.45) and `router.policy.min_margin` (0.15).
4. **Heuristic.** With no key, or when the decider's answer is rejected by validation or the gate, a scorer picks: fit to the task, headroom, load, cost and harness priority. The weights are `router.weights`. The default priority is `agy`, `opencode`, `codex`, `claude`.
5. **Account.** Among the chosen choice's accounts: most headroom first, then fewest active runs, then `priority`, then least recently used.

Task text is data. It cannot change which choices are eligible.

Claude ranks last. The router picks a Claude choice only when you name it, when nothing else has headroom, or when it is the only match for the role.

## Jev key setup

Jev is a routing model on OpenRouter. It is optional. Without a key the heuristic routes.

Set the key with an environment variable. The default name is `OPENROUTER_API_KEY`:

```sh
export OPENROUTER_API_KEY=sk-or-...
```

Or keep it in a dotenv file and point the config at it:

```sh
superorchestrator config set router.jev.key_file '~/.config/jev/.env'
```

The file holds a line such as `OPENROUTER_API_KEY=sk-or-...`. To use another variable name, set `router.jev.key_env`. The environment wins over the file. The supervisor needs the variable in its own environment, so set it before the supervisor starts, or use the file. The web UI shows where the key comes from, never its value.

Other keys: `router.jev.endpoint` and `router.jev.timeout` (default `20s`, for the primary and the fallback together; with a fallback the primary gets 3/5 of it).

### The decider model

The default decider is `inception/mercury-decide:free`, a free OpenRouter model. Free tiers carry risks: they can be rate limited, they can be withdrawn, and the id resolves to a dated snapshot (for example `mercury-decide-20260930`) that changes over time. For that reason `router.jev.fallback_model` (default `~typesafe/jev-latest`) is tried automatically when the primary model cannot be reached: a transport error, a timeout, HTTP 429 or 5xx, or HTTP 400/404 for a missing model. A 429, 5xx or refused connection is retried once first; a timeout is not retried, because it uses up the time left for that model. A rejected answer is never sent to the fallback, and neither is a refused key. Other 4xx responses, invalid bodies and a disallowed endpoint go to the heuristic.

Both settings take any OpenRouter model id. Precedence is environment, then `config.toml`, then the default; an empty value counts as unset.

```sh
# one run of the shell
export SUPERORCHESTRATOR_ROUTER_MODEL='~typesafe/jev-latest'
export SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL=none      # no fallback
# or persistently
superorchestrator config set router.jev.model '~typesafe/jev-latest'
```

`route --explain`, `doctor` and `route eval` show the models in effect and where each came from (`env`, `config` or `default`). The environment is read by the supervisor, so set it before the supervisor starts.

### When the decider cannot be reached

If a key is set but no model can be reached (after the fallback), `route` fails instead of quietly routing by heuristic. The error names the models, the HTTP status or cause, and the next step (check the key, the model id or the network, or choose another model). `route` exits non-zero, the MCP `route` tool returns an error, `spawn_agent` with `harness: auto` spawns nothing, `POST /api/v1/route` returns 502, and `route eval` counts an error. This applies to explicit requests only. A failover, or the re-routing of a queued run, never fails a run because the decider is down: it routes with the heuristic, records `decider unreachable: <models>` as the decision's fallback reason, logs a warning, and asks the decider only once for that recovery. To degrade to the heuristic instead, set `router.jev.on_unreachable = "heuristic"`. With no key at all, routing always uses the heuristic and records why.

## Failover

When a run fails with a quota, rate-limit or auth error, that account (and pool) is marked exhausted until its reset time. With no reset time, it is marked for `usage.default_cooldown` (5 hours). The run then restarts on the next account of the same choice, or on the next eligible choice. The run keeps its id. The timeline shows each attempt. Other non-zero exits are not retried.

To test it by hand, flag an account: `superorchestrator account flag codex-acc1 --hours 1`. The router skips it.

## Models catalog

### Effort values

The router checks each choice's `effort` against what its harness accepts: claude and agy take `low|medium|high|xhigh|max`, codex takes `minimal|low|medium|high|xhigh|max|ultra`, and opencode passes it on as a provider-specific variant without checking. A choice with any other value is skipped when routing (the route rationale names it) and `superorchestrator doctor` reports it under "choice efforts". Change the value in `config.toml` to fix it.

`thinking` is a special value for agy. Some agy models are the thinking variant themselves (`claude-sonnet-4-6` is "Claude Sonnet 4.6 (Thinking)"), so there is no effort to pass. `effort = "thinking"` tells the router to rank the choice with the top effort tier, and the agy driver launches without `--effort`. Use it only on agy choices. The agy thinking-variant models (`claude-sonnet-4-6`, `claude-opus-4-6-thinking`) take no `--effort`: a choice with a real effort on one is skipped and reported by `doctor`, and `spawn_agent` refuses that pairing with `INVALID_PARAMS` before creating a worktree. Leave `effort` out or use `thinking`.

`models.toml` lists known models per harness, with tier, efforts, pool and cost. A default ships in the binary. `superorchestrator models refresh` collects models from the harness CLIs and models.dev. `superorchestrator models list` prints them. The catalog is for discovery and validation. It never edits your choices, and `models refresh` tells you about models that no choice uses.

## Evaluating the router

```sh
superorchestrator route eval --require-live --min-accuracy 0.8
```

`route eval` replays labelled cases and reports accuracy, eligible-choice compliance, fallback count, source counts, p50 and p95 latency, and total cost. Reports never include task text or credentials. English and Vietnamese cases ship in the binary.

- `--cases FILE` uses your cases instead. The file is JSON Lines, or one JSON array. Each case has `id`, `task`, `expected_choices` and an optional `role` (default `worker`) and `available_choice_ids`.
- `--require-live` fails when there is no Jev key, and when any case fell back from Jev.
- `--min-accuracy X` (0 to 1) fails when accuracy is lower.

The command exits non-zero when a check fails. Tune `router.policy.min_confidence` and `router.policy.min_margin` from live results. The decider reads only the choice descriptions and the routing instruction, so write descriptions that separate the tiers. Choices already in your `config.toml` keep the descriptions they were seeded with; copy newer seeded descriptions by hand if you want them.

## Defaults

`defaults.harness`, `defaults.model` and `defaults.effort` apply when a spawn names none of them. Order of precedence: call argument, agent file, `defaults`, router.
