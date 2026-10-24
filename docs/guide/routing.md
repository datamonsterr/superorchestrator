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

## How the router decides

1. **Eligible choices.** A choice is eligible when it is enabled, matches the role, and has an account that is signed in, not flagged, not exhausted and below its cap. Claude ranks last (see below).
2. **Complexity.** The router needs a number from 0 to 100 that says how much reasoning, analysis and multi-step work the task needs. If a Jev key is set (below) and more than one choice is eligible, the decider rates the task against five fixed anchors, from trivial to highly complex, and the router maps its answer to that scale. Without a key, or when the decider's answer is rejected, the router estimates the complexity from the task's keywords and length. The estimate is 20, 55 or 85.
3. **Level.** The router finds the complexity level that contains that number (see [Complexity levels](#complexity-levels)). Outside every level, it takes the nearest one.
4. **Choice.** It takes the level's candidate choices, the eligible ones you listed for the level, and picks the one with the lowest `priority` number. If the level has no eligible candidate, it tries the other levels, nearest first.
5. **Account.** Among that choice's accounts: most headroom first, then fewest active runs, then `priority`, then least recently used.

The decider sees only the task text, the role and the routing instruction. It never sees account names, quota or usage, and it does not choose a choice. A rejected answer is ignored, and the heuristic estimate is used instead. The answer is accepted only when it is a complete score with probabilities over the five anchors that sum to 1 (within 0.02), and its confidence is at least `router.policy.min_confidence` (0.45). `router.policy.min_margin` is deprecated and ignored.

Task text is data. It cannot change which choices are eligible.

Claude ranks last. The router picks a Claude choice only when you name it, when nothing else has headroom, or when it is the only match for the role.

## Complexity levels

A complexity level is a named range of complexity with the choices allowed for tasks in that range. You define levels in `config.toml`:

```toml
[[router.levels]]
name = "low"
min = 0
max = 39
tier = "fast"            # used only when `choices` is empty

[[router.levels]]
name = "mid"
min = 40
max = 79
tier = "mid"

[[router.levels]]
name = "high"
min = 80
max = 100
choices = [
  { id = "codex-gpt-high", priority = 1 },   # id must match a [[choices]] entry
  { id = "agy-pro-high" },                   # no priority: after the ones that have one
]
```

- `min` and `max` are integers from 0 to 100. The range covers complexity from `min` up to, but not including, `max + 1`, so a complexity of 37.5 is in `low`. A level with `max = 100` also includes 100.
- Level names must be non-empty and unique. Ranges must not overlap. Gaps are allowed: a complexity in a gap goes to the nearest level, and a tie goes to the higher one.
- A level needs `choices`, `tier`, or both. `tier` is the catalog's model class (`high`, `mid` or `fast`). It is used only when `choices` is empty, and then every eligible choice of that tier is a candidate, in config order. When `choices` is not empty, `tier` is ignored.
- Each `choices` entry names a choice from `[[choices]]`. An id that names no choice (for example after you delete the choice) is accepted, never eligible, and skipped when routing; `doctor` warns and names it. A choice may appear only once in a level. `priority` is optional, and a lower number is preferred.

When you set no levels, the defaults apply: `low` for 0-37 with tier `fast`, `mid` for 38-62 with tier `mid`, and `high` for 63-100 with tier `high`.

`superorchestrator init` writes five levels into a new `config.toml`: `trivial` 0-12, `simple` 13-37, `moderate` 38-62, `complex` 63-87 and `expert` 88-100, each listing seeded non-Claude choices with priorities. It does this only when it creates the file; an existing config is never changed, so add `[[router.levels]]` yourself (copy them from `seed/levels.toml` in the source) to use them.

A level's choice list is not a pool. A pool is a separately metered slice of an account's usage, so the config key is `choices` and the docs call them the level's choices.

For each task, the router picks like this:

1. Its candidates are the level's eligible choices. With an empty `choices` list, these are the eligible choices of the level's tier.
2. If the level has no eligible candidate, the other levels are tried, nearest to the complexity first, and the higher level first on a tie.
3. If no level has an eligible candidate, every eligible choice is ranked by the heuristic scorer. The rationale says so, and `level` in the result is null.
4. Among the candidates, the lowest `priority` number wins. Candidates without a priority come after those with one.
5. Ties, including two candidates without a priority, go to the choice whose best account has the most headroom for its pool. Then the one with fewer active runs on that account, then the one listed first in config.

An explicit empty list, `router.levels = []`, loads and turns levels off. The decider still scores the task, but `level` is null and every eligible choice is ranked by the heuristic scorer. The rationale says "no levels configured". The defaults apply only when `router.levels` is absent from the file.

`route --explain` shows the complexity, where it came from and the level, for example `complexity: 55.0 (jev)` and `level: mid`. The result of the MCP `route` tool carries the same fields.

## Previewing a route

```sh
superorchestrator route "Rename the foo module to bar" --explain
```

`route` previews only. It reserves nothing. The task needs no quotes: `route fix a typo in README` routes "fix a typo in README". A task that starts with `eval` or `-` needs `--` first, as in `route -- eval the parser`, because `route eval` is the evaluation command. Use `--role` to route for another role (default `worker`). The MCP tool `route` does the same.

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

`model` and `effort` values are validated before launch; see [agents](agents.md#model-and-effort-characters) for the allowed characters.

The router checks each choice's `effort` against what its harness accepts: claude and agy take `low|medium|high|xhigh|max`, codex takes `minimal|low|medium|high|xhigh|max|ultra`, and opencode passes it on as a provider-specific variant without checking. A choice with any other value is skipped when routing (the route rationale names it) and `superorchestrator doctor` reports it under "choice efforts". Change the value in `config.toml` to fix it.

`thinking` is a special value for agy. Some agy models are the thinking variant themselves (`claude-sonnet-4-6` is "Claude Sonnet 4.6 (Thinking)"), so there is no effort to pass. `effort = "thinking"` tells the router to rank the choice with the top effort tier, and the agy driver launches without `--effort`. Use it only on agy choices. The agy thinking-variant models (`claude-sonnet-4-6`, `claude-opus-4-6-thinking`) take no `--effort`: a choice with a real effort on one is skipped and reported by `doctor`, and `spawn_agent` refuses that pairing with `INVALID_PARAMS` before creating a worktree. Leave `effort` out or use `thinking`.

`models.toml` lists known models per harness, with tier, efforts, pool and cost. A default ships in the binary. `superorchestrator models refresh` collects models from the harness CLIs and models.dev. `superorchestrator models list` prints them. The catalog is for discovery and validation. It never edits your choices, and `models refresh` tells you about models that no choice uses.

## Evaluating the router

```sh
superorchestrator route eval --require-live --min-accuracy 0.8
```

`route eval` replays labelled cases and reports accuracy, complexity accuracy, eligible-choice compliance, fallback count, source counts, p50 and p95 latency, and total cost. Reports never include task text or credentials. English and Vietnamese cases ship in the binary.

- `--cases FILE` uses your cases instead. The file is JSON Lines, or one JSON array. Each case has `id`, `task`, `expected_choices` and an optional `role` (default `worker`), `available_choice_ids` and `expected_complexity`, an inclusive `[min, max]` range on the 0-100 anchor scale (trivial `[0, 12]`, simple `[13, 37]`, moderate `[38, 62]`, complex `[63, 87]`, expert `[88, 100]`). Every bundled and held-out case has one.
- `--require-live` fails when there is no Jev key, and when any case fell back from Jev.
- `--min-accuracy X` (0 to 1) fails when accuracy is lower.
- `--min-complexity-accuracy X` (0 to 1) fails when complexity accuracy is lower, or when no case has both a range and a score.

The text report lists each case with its choice, source, complexity (`CX`) and `LEVEL`. Complexity accuracy is the share of cases, among those with an `expected_complexity` and a score, whose score is inside the range. It measures the decider and the anchors on their own, before any level mapping, so a low accuracy with a high complexity accuracy points at your levels. The JSON report adds `complexity`, `level` and `complexity_ok` to every case and `complexity_accuracy` to the summary.

The command exits non-zero when a check fails.

Accuracy depends on your levels. A case passes when the router picks one of its `expected_choices`, and the pick comes from the level the task's complexity falls in, then the level's priorities. A case that misses may mean its complexity landed in the wrong level, or that a level lists the wrong choices, rather than that the decider is wrong. Check the level and the complexity in `route --explain` before you change the decider or `router.instruction`. Tune `router.policy.min_confidence` from live results.

The decider reads the task text and `router.instruction`. It does not read choice descriptions, so those descriptions no longer affect routing. They still describe the choices to people and to the installed skill.

## Defaults

`defaults.harness`, `defaults.model` and `defaults.effort` apply when a spawn names none of them. Order of precedence: call argument, agent file, `defaults`, router.
