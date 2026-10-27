# Routing, models and levels

When a spawn uses `harness: auto` (the default), the router picks the account, model and effort. This page explains how, and how to tune it.

## Models and levels

Routing has no separate list of approved combinations. A **model ref** names one harness, model and effort in a single string, and you list the refs you approve on the **levels** in `config.toml`. A level is a range of task complexity, and the models listed on it are the ones routing may pick for tasks in that range.

A model ref has the form `harness/model[/effort]`:

- `harness` is `codex`, `agy`, `opencode` or `claude`.
- For `codex`, `agy` and `claude`, the last segment is the effort when the harness accepts it. Otherwise the whole rest is the model, and there is no effort.
  - `claude`: `low`, `medium`, `high`, `xhigh`, `max`.
  - `agy`: `low`, `medium`, `high`, `xhigh`, `max`, `thinking`.
  - `codex`: `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, `ultra`.
- For `opencode` the rest is `provider/model[/variant]`. Two segments mean no effort; three or more mean the last segment is the effort. `opencode/opencode/deepseek-v4.1-flash-free` is a model with no effort, and `opencode/openai/gpt-5/high` has effort `high`.

Examples: `codex/gpt-6.1-sol/high`, `agy/gemini-3.8-flash-medium/medium`, `agy/claude-sonnet-4-6/thinking`, `claude/claude-sonnet-5-5/medium`.

`agy/claude-sonnet-4-6/thinking` and `agy/claude-sonnet-4-6` are both valid. `agy/claude-sonnet-4-6/high` is rejected when the config loads, because that model has no `high` effort. The ref string is the id everywhere: it appears as `choice_id` in runs, in `route_decision` events, in route results and in `list_harnesses`.

```toml
[[router.levels]]
name = "complex"
min = 63
max = 87
models = ["agy/claude-sonnet-4-6/thinking", "opencode/opencode/muse-spark-1.3", "codex/gpt-6.1-sol/high"]
```

A level lists its models in preference order. The first eligible model is preferred. A model is enabled by being listed; there is no separate enable switch, role, description or priority field.

Each level needs `models`, `tier`, or both. `tier` (`high`, `mid` or `fast`) is the catalog's model class. It applies only when `models` is empty, and then every eligible model of that tier is a candidate, in config order. When `models` is not empty, `tier` is ignored.

Duplicate refs in one level are rejected.

### Migrating from the choices table

Older configs defined a choices table and listed its entries by id on each level. That format is gone. A config that still has the old table at the top level, or a `choices` list on a level, fails to load with this message:

```
[[choices]] is no longer read: list models on each [[router.levels]] entry as harness/model/effort, e.g. models = ["codex/gpt-6.1-sol/high"] (docs/guide/routing.md)
```

There is no compatibility mode. To move, write each approved combination as a ref, put the refs on the level in the order you want, and delete the old choices table. The old `enabled`, role, `description` and `priority` keys have no equivalent: enable a model by listing it, and express preference by order.

### Seeded levels

`superorchestrator init` writes five levels into a new `config.toml`, listing the seeded models in this order:

| Level | Range | Models |
|---|---|---|
| `trivial` | 0-12 | `agy/gemini-3.8-flash-low/low`, `agy/gemini-3.8-flash-medium/medium` |
| `simple` | 13-37 | `agy/gemini-3.8-flash-medium/medium`, `codex/gpt-6-luna/medium`, `opencode/opencode/deepseek-v4.1-flash-free` |
| `moderate` | 38-62 | `codex/gpt-6-luna/medium`, `agy/gemini-3.8-flash-high/high`, `codex/gpt-6.1-sol/medium` |
| `complex` | 63-87 | `codex/gpt-6.1-sol/high`, `agy/claude-sonnet-4-6/thinking` |
| `expert` | 88-100 | `codex/gpt-6.1-sol/xhigh`, `codex/gpt-6.1-sol/high`, `agy/claude-sonnet-4-6/thinking` |

Codex leads from `moderate` up, so a signed-in Codex account is used once a task needs more than a Flash worker. `gpt-6-luna` is seeded only for `moderate` and below. Claude is not seeded. To allow a Claude model, list it on a level, for example `claude/claude-sonnet-5-5/medium`. Claude still ranks last unless a call names it (see [Claude rank](#claude-rank)).

`init` writes levels only when it creates the file. An existing config is never changed, so edit the levels yourself with `superorchestrator config edit` or on the Levels tab of the web UI.

### Managing levels from the command line

`superorchestrator levels` edits `router.levels` without opening the file. These commands change `config.toml` only, so restart the supervisor to apply:

```sh
superorchestrator levels list [--json]
superorchestrator levels add <name> --min N --max N [--tier high|mid|fast] [--model REF]...
superorchestrator levels edit <name> [--name NEW] [--min N] [--max N] [--tier T | --no-tier]
superorchestrator levels remove <name>
superorchestrator levels reset                  # remove router.levels; the defaults apply again
superorchestrator levels model add <level> <harness/model[/effort]> [--position N]
superorchestrator levels model move <level> <harness/model[/effort]> --position N
superorchestrator levels model remove <level> <harness/model[/effort]>
```

`levels list` shows each level's range and its models in preference order: position (1 is the most preferred), model ref, harness, model, effort and state. The state is `on` when the catalog lists the model and an enabled account serves its harness, `off` when the catalog lists it but no enabled account serves its harness, and `unknown` when the catalog does not list it. A level that lists no models shows its tier instead. With no levels in the file it says `(defaults)`.

`add` needs `--tier` or at least one `--model`. `model add` appends unless you pass `--position`, and `levels model add` on a level that has a tier makes the level use its listed models. Positions run from 1 to the number of models (one more for `add`). Refs are parsed with the same rules as the config, so a malformed ref or an effort the harness does not accept is refused. When the file has no `router.levels`, the current defaults are written first, so your edit is relative to what was in effect. Every command is one validated write, like a load: an overlap, a duplicate ref or a level left with neither models nor tier is refused and nothing is written. Rewriting `router.levels` drops any comments inside that block (comments elsewhere are kept). `levels reset` asks for confirmation; pass `--yes` in a script.

`levels remove` refuses to remove the last level, because nothing would be routable; use `levels reset`. With `--json`, `levels list` prints the resolved view above, while the other commands print `{ok, levels, materialised_defaults, restart_required}`, where `levels` has the config shape (`name`, `min`, `max`, `tier`, `models` as ref strings).

### Level ranges

- `min` and `max` are integers from 0 to 100. The range covers complexity from `min` up to, but not including, `max + 1`, so a complexity of 37.5 is in a level `0-37`. A level with `max = 100` also includes 100.
- Level names must be non-empty and unique. Ranges must not overlap. Gaps are allowed: a complexity in a gap goes to the nearest level, and a tie goes to the higher one.
- An explicit `router.levels = []` loads and turns levels off (see below). Without the key, the defaults apply. The default levels list no models, so nothing routes until some level lists models; `superorchestrator init` seeds five levels that do.

## How the router decides

1. **Eligible models.** A listed model is eligible when its harness has an enabled account that is signed in, not flagged, not in cooldown, has headroom for the model's pool and is below its cap. There is no role filter. A call can exclude models with `exclude` (a list of refs), and accounts with `exclude_accounts`. Claude ranks last (see below).
2. **Complexity.** The router needs a number from 0 to 100 that says how much reasoning, analysis and multi-step work the task needs. If a Jev key is set (below) and more than one model is eligible, the decider rates the task against five fixed anchors, from trivial to highly complex, and the router maps its answer to that scale. Without a key, or when the decider's answer is rejected, the router estimates the complexity from the task's keywords and length. The estimate is 20, 55 or 85.
3. **Level.** The router finds the level that contains that number (see [Level ranges](#level-ranges)). Outside every level, it takes the nearest one.
4. **Candidates.** The candidates of a level are its eligible listed models, in list order. With an empty `models` list, they are the eligible models of the level's `tier`, in config order. If the level has no candidate, the other levels are tried, nearest first.
5. **Pick.** Within the level, the first eligible model in list order wins. There are no priorities and no ties: the order you wrote is the answer. The rationale says `first in the level`, or `the only eligible choice of the level` when only one model of the level is eligible.
6. **Account.** Among the chosen model's harness accounts (see [Account choice](#account-choice)).

The decider sees only the task text, the agent profile name and the routing instruction. It never sees account names, quota or usage, and it does not choose a model. A rejected answer is ignored, and the heuristic estimate is used instead. The answer is accepted only when it is a complete score with probabilities over the five anchors that sum to 1 (within 0.02), and its confidence is at least `router.policy.min_confidence` (0.45). `router.policy.min_margin` is deprecated and ignored.

Task text is data. It cannot change which models are eligible.

If no level has an eligible candidate, every eligible model is ranked by the heuristic scorer. The rationale says so, and `level` in the result is null. An explicit empty `router.levels = []` always works this way: the decider still scores the task, `level` is null, and the rationale says "no levels configured".

`route --explain` shows the complexity, where it came from and the level, for example `complexity: 55.0 (jev)` and `level: mid`, followed by the ranked accounts. The MCP `route` tool returns the same fields.

## Account choice

Once the model is chosen, the router picks one of its harness's accounts. Only enabled accounts that are below their cap and whose pool is available are considered, in this order:

1. most headroom for the pool;
2. fewest active runs;
3. higher account `priority`;
4. least recently used;
5. name.

The first criterion that separates the accounts decides. The rationale ends with `account <name>: <why>`, where `why` is the criterion that decided it:

- `most headroom (N%)`
- `fewest active runs (N)`
- `higher priority`
- `least recently used`
- `the only usable account`
- `name order`

`RouteResult.ranked_accounts` lists every candidate account, best first, so you can see the order behind the pick. `route --explain` prints it as an `accounts:` line after `alternatives:`.

## Claude rank

Claude ranks last. The router picks a Claude model only when one of these holds:

- the call names it;
- no other eligible model has an account with headroom.

Claude accounts are kept for the orchestrator, so the router keeps Claude limits for it.

## Pinning a model

`model` on `spawn_agent`, `route` and `route --model` accepts either a plain model name or a model ref.

- A ref pins harness, model and effort together. `route --model codex/gpt-6.1-sol/high` picks that model.
- A value whose first segment is a harness name is a ref. If the ref's rest does not parse for that harness, the call fails.
- An explicit `harness` or `effort` that disagrees with the ref is an error (`INVALID_PARAMS`), for example `model ref names codex but harness is agy`.
- A value whose first segment is not a harness name is a plain model name, as before. `opencode/<one segment>` is also a plain model name, so `opencode/deepseek-v4.1-flash-free` works as a model.
- A pinned ref that no level lists still runs. It is a synthetic candidate, and its id is the ref.
- A pinned call keeps its pick. Its complexity comes from the heuristic, and `level` is the level that complexity maps to.

Invalid efforts cannot reach routing: a ref with an effort its harness does not accept fails when the config loads.

## Previewing a route

```sh
superorchestrator route "Rename the foo module to bar" --explain
superorchestrator route "Port the parser to the new API" --agent worker --model codex/gpt-6.1-sol/high
```

`route` previews only. It reserves nothing. The task needs no quotes: `route fix a typo in README` routes "fix a typo in README". A task that starts with `eval` or `-` needs `--` first, as in `route -- eval the parser`, because `route eval` is the evaluation command.

- `--agent NAME` names the agent profile (default `worker`). The decider receives the name as `agent`. Profiles do not change the model choice; see [agents](agents.md).
- `--model REF` pins a model (see [Pinning a model](#pinning-a-model)).
- `--explain` prints the decision: the source and value of complexity, the level, the rationale, the alternatives and the `accounts:` line.

The MCP tool `route` takes the same values as `prompt`, `agent`, `exclude`, `exclude_accounts`, `harness`, `account`, `model` and `effort`.

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

When a run fails with a quota, rate-limit or auth error, that account (and pool) is marked exhausted until its reset time. With no reset time, it is marked for `usage.default_cooldown` (5 hours). The run then restarts on the next account of the same model, or on the next eligible model. The run keeps its id. The timeline shows each attempt. Other non-zero exits are not retried.

To test it by hand, flag an account: `superorchestrator account flag codex-acc1 --hours 1`. The router skips it.

## Effort values

The effort of a ref is checked when the config loads, against the list in [Models and levels](#models-and-levels). `opencode` passes its variant on to the provider without checking it.

`thinking` is a special value for agy. Some agy models are the thinking variant themselves (`claude-sonnet-4-6` is "Claude Sonnet 4.6 (Thinking)"), so there is no effort to pass. `agy/claude-sonnet-4-6/thinking` tells the router to rank the model with the top effort tier, and the agy driver launches without `--effort`. The agy thinking-variant models (`claude-sonnet-4-6`, `claude-opus-4-6-thinking`) take no `--effort`, so a ref with a real effort on one of them is rejected when the config loads, and `spawn_agent` refuses that pairing with `INVALID_PARAMS` before creating a worktree. Leave the effort out or use `thinking`.

`models.toml` lists known models per harness, with tier, efforts, pool and cost. A default ships in the binary. `superorchestrator models refresh` collects models from the harness CLIs and models.dev, and reports listed models that the catalog does not know and catalog models that no level lists. `superorchestrator models list` prints them. The catalog is for discovery and validation. It never edits your config.

## Evaluating the router

```sh
superorchestrator route eval --require-live --min-accuracy 0.8
```

`route eval` replays labelled cases and reports accuracy, complexity accuracy, eligible-model compliance, fallback count, source counts, p50 and p95 latency, and total cost. Reports never include task text or credentials. English and Vietnamese cases ship in the binary.

- `--cases FILE` uses your cases instead. The file is JSON Lines, or one JSON array. Each case has `id`, `task` and `expected_choices`, which are model refs, plus `expected_complexity`, an inclusive `[min, max]` range on the 0-100 anchor scale (trivial `[0, 12]`, simple `[13, 37]`, moderate `[38, 62]`, complex `[63, 87]`, expert `[88, 100]`). Every bundled and held-out case has one. A `role` key is ignored.
- `--require-live` fails when there is no Jev key, and when any case fell back from Jev.
- `--min-accuracy X` (0 to 1) fails when accuracy is lower.
- `--min-complexity-accuracy X` (0 to 1) fails when complexity accuracy is lower, or when no case has both a range and a score.

The text report lists each case with its model ref, source, complexity (`CX`) and `LEVEL`. Complexity accuracy is the share of cases, among those with an `expected_complexity` and a score, whose score is inside the range. It measures the decider and the anchors on their own, before any level mapping, so a low accuracy with a high complexity accuracy points at your levels. The JSON report adds `complexity`, `level` and `complexity_ok` to every case and `complexity_accuracy` to the summary.

The command exits non-zero when a check fails.

Accuracy depends on your levels. A case passes when the router picks one of its `expected_choices`, and the pick comes from the level the task's complexity falls in, then the order of that level's models. A case that misses may mean its complexity landed in the wrong level, or that a level lists the wrong models, rather than that the decider is wrong. Check the level and the complexity in `route --explain` before you change the decider or `router.instruction`. Tune `router.policy.min_confidence` from live results.

The decider reads the task text, the agent profile name and `router.instruction`. It does not read model descriptions, because models no longer have them.

## Defaults

`defaults.harness`, `defaults.model` and `defaults.effort` apply when a spawn names none of them. Order of precedence: call argument, agent file, `defaults`, router. A `defaults.model` may be a model ref, under the same rules as [Pinning a model](#pinning-a-model).
