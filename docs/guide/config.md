# Config reference

Superorchestrator keeps its files in one directory:

```
~/.config/superorchestrator/
  config.toml        defaults, limits, router, usage, worktrees, ui, notifications
  models.toml        model catalog
  accounts/<name>.toml
  agents/<name>/AGENT.md
  workflows/<name>.js
  state.db           SQLite, WAL mode
  runs/<run_id>/     transcript.jsonl, mcp.json, system.md, materialised agent files
  logs/              supervisor.log
  wrappers.bak/      wrappers that `account add` moved aside
  legacy-backup/<ts>/ backups from `install` and `migrate`
```

The directory is `$SUPERORCHESTRATOR_CONFIG_DIR` if set. Otherwise it is `$XDG_CONFIG_HOME/superorchestrator` on Linux, or `~/.config/superorchestrator`. Use `--config-dir DIR` for one command.

## Editing

```sh
superorchestrator config get limits.global_cap
superorchestrator config set limits.global_cap 8
superorchestrator config edit
```

`config set` keeps comments. Every section is optional, and an unknown key is an error. Durations are strings such as `"30m"`, `"5h"` or `"20s"`. The supervisor reads the file at start. The web UI tells you when a restart is needed.

## Keys

### `[limits]`

| Key | Default | Meaning |
|---|---|---|
| `global_cap` | `16` | Most active runs over all accounts. At least 1. |
| `max_children` | `8` | Most children one run may start. At least 1. |

### `[runs]`

| Key | Default | Meaning |
|---|---|---|
| `stall_ms` | `300000` | Time with no event before a run is `stalled` |
| `ask_timeout` | `"30m"` | How long `ask_parent` waits before it returns `timeout` |

### `[workflows]`

| Key | Default | Meaning |
|---|---|---|
| `max_duration` | `"6h"` | Longest one workflow run, waits included |

### `[usage]`

| Key | Default | Meaning |
|---|---|---|
| `probe_ttl` | `"5m"` | How old a usage reading may be before a new probe |
| `low_pct` | `15` | Headroom below this percent is `low` |
| `default_cooldown` | `"5h"` | Cooldown when an error gives no reset time |

### `[router]`

| Key | Default | Meaning |
|---|---|---|
| `priority` | `["agy", "opencode", "codex", "claude"]` | Harness order for ties |
| `weights` | `{ headroom = 1.0, load = 0.5, cost = 0.3 }` | Heuristic weights (`router.weights.headroom`, `.load`, `.cost`) |
| `instruction` | A complexity-rating instruction | Instruction sent to the decider with its complexity question. The default asks only for a rating and treats `task_content` as data. `doctor` warns when a custom instruction still looks written for the old choice question. |
| `levels` | `low` 0-37, `mid` 38-62, `high` 63-100 | Complexity levels, set with `[[router.levels]]` below |

### `[router.jev]`

| Key | Default | Meaning |
|---|---|---|
| `endpoint` | `https://openrouter.ai/api/alpha/decisions` | Decider URL |
| `model` | `inception/mercury-decide:free` | Decider model, any OpenRouter id. Env `SUPERORCHESTRATOR_ROUTER_MODEL` overrides it |
| `fallback_model` | `~typesafe/jev-latest` | Tried only when the primary model cannot be reached. Env `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL` overrides it; `none` disables it |
| `on_unreachable` | `"error"` | When a configured decider cannot be reached: `"error"` fails the call, `"heuristic"` degrades |
| `key_env` | `OPENROUTER_API_KEY` | Environment variable that holds the key |
| `key_file` | unset | Optional dotenv file that holds the key |
| `timeout` | `"20s"` | Decider timeout, shared by the primary and the fallback |

### `[router.policy]`

| Key | Default | Meaning |
|---|---|---|
| `min_confidence` | `0.45` | Lowest confidence to accept a decision |
| `min_margin` | `0.15` | Deprecated and ignored. Still parsed so old configs load. A score has no margin between choices. |

`doctor` and `route eval` report the models from the environment of the shell that runs them. A supervisor started elsewhere may have a different environment. `min_confidence` was tuned on 2026-10-23 against `jev-1.14-20261014` for the earlier decider, which picked a choice; it has not been re-tuned for complexity scores. Re-run `route eval --require-live` after you change `router.instruction`, your levels or your choices, and adjust `min_confidence` from those results.

### `[[router.levels]]`

Each level maps a range of complexity (0 to 100, from the decider or from the heuristic estimate) to the models allowed for tasks in that range. Models are model refs (`harness/model[/effort]`, see [routing](routing.md#models-and-levels)), listed in preference order. [Routing](routing.md#how-the-router-decides) explains how a task is picked from them.

```toml
[[router.levels]]
name = "complex"
min = 63
max = 87
models = ["agy/claude-sonnet-4-6/thinking", "opencode/opencode/muse-spark-1.3", "codex/gpt-6.1-sol/high"]
```

| Key | Default | Meaning |
|---|---|---|
| `name` | required | Level name. Non-empty and unique after trimming. |
| `min` | required | Lowest complexity in the level, an integer from 0 to 100 |
| `max` | required | Highest integer in the level, from `min` to 100. The level covers up to `max + 1`, so 37.5 is in `0`-`37`. With `max = 100`, 100.0 is included too. |
| `models` | `[]` | Model refs in preference order, for example `"codex/gpt-6.1-sol/high"`. The first eligible one is preferred. A model is enabled by being listed. |
| `tier` | unset | `high`, `mid` or `fast`. Used only when `models` is empty: then every eligible model of that tier is a candidate, in config order. Ignored when `models` is set. |

A level needs `models`, `tier`, or both. With no `[[router.levels]]` in the file, the defaults are `low` 0-37 (tier `fast`), `mid` 38-62 (tier `mid`) and `high` 63-100 (tier `high`), with no models. The default levels list no models, so nothing routes until some level lists models; `superorchestrator init` seeds five levels that do. `superorchestrator init` writes the five seeded levels (`trivial`, `simple`, `moderate`, `complex`, `expert`) into a new config file; the list is in [routing](routing.md#seeded-levels). Gaps between levels are allowed. A complexity that falls in a gap goes to the nearest level, and a tie goes to the higher one. An explicit empty list, `router.levels = []`, loads and turns levels off: `level` is null and every eligible model is ranked by the heuristic scorer.

`superorchestrator levels` edits this list from the command line (see [routing](routing.md#managing-levels-from-the-command-line)).

The file is refused at load when:

- a name is empty or repeated
- `min` is greater than `max`, or `max` is greater than 100
- two ranges overlap
- a level has neither `models` nor `tier`
- a level lists the same ref twice
- a ref is malformed, or its effort is not one its harness accepts (for example `agy/claude-sonnet-4-6/high`)
- the file still has the old choices table or a `choices` list on a level (see [the removed choices table](#removed-choices-table))

`doctor` reports the levels under "levels" and warns when:

- a level's models have no enabled account, so the level falls through to the nearest one
- a listed model is not in the model catalog
- `tier` is set together with a non-empty `models` list, so `tier` is ignored

### `[worktrees]`

| Key | Default | Meaning |
|---|---|---|
| `dir` | `"{repo_parent}/{repo}-worktrees"` | Where run worktrees go. `{repo_parent}` and `{repo}` expand. |
| `branch_prefix` | `"so/"` | Prefix of run branches |

### `[accounts]`

| Key | Default | Meaning |
|---|---|---|
| `wrapper_dir` | `"~/.local/bin"` | Where wrappers are written |
| `pass_env` | `[]` | Exact names of extra inherited variables for every account; combined with each account's `pass_env`. See [accounts](accounts.md#inherited-environment). |

### `[ui]`

| Key | Default | Meaning |
|---|---|---|
| `bind` | `"127.0.0.1"` | Address to bind. Every bind needs authentication; a private token is generated when unset. A wildcard such as `0.0.0.0` skips the UI Host check and listens on the LAN; restrict it with a firewall. See [reaching the UI over a tailnet](web-ui.md#reach-it-over-a-tailnet). |
| `port` | `7878` | Port |
| `token` | unset | Shared-secret override for the UI (plain text). When unset, the supervisor reuses or creates the private `ui-token` file. The API cannot set it. |
| `enabled` | `false` | Start the UI whenever the supervisor starts. `ui --install-service` sets it. |

### `[supervisor]`

| Key | Default | Meaning |
|---|---|---|
| `idle_exit` | `"10m"` | The supervisor exits after this long with no runs, clients or UI streams |

### `[notifications]`

| Key | Default | Meaning |
|---|---|---|
| `discord_webhook_env` | `"SO_DISCORD_WEBHOOK"` | Name of the variable that holds the webhook URL |
| `desktop` | `true` | Desktop notifications |
| `on` | `["escalated", "question_waiting_10m", "session_finished", "account_exhausted"]` | Events that notify |

### `[defaults]`

Used when a spawn and its agent name no harness, model or effort.

| Key | Default | Meaning |
|---|---|---|
| `harness` | unset | `codex`, `agy`, `opencode` or `claude` |
| `model` | unset | Model id |
| `effort` | unset | Effort level |

### Removed: choices table

The old top-level choices table, and a `choices = [...]` list on a level, are no longer read. The config fails to load with this message: `[[choices]] is no longer read: list models on each [[router.levels]] entry as harness/model/effort, e.g. models = ["codex/gpt-6.1-sol/high"] (docs/guide/routing.md)`. Move each entry to a model ref on the level, as described in [routing](routing.md#migrating-from-the-choices-table).

## Environment variables

| Variable | Meaning |
|---|---|
| `SUPERORCHESTRATOR_CONFIG_DIR` | The config directory |
| `SUPERORCHESTRATOR_SOCKET` | The supervisor socket path |
| `SUPERORCHESTRATOR_RUN_ID` | Set for a subagent's MCP server, so it acts for that run |
| `SUPERORCHESTRATOR_LOG` | Log filter, for example `debug` |
| `SUPERORCHESTRATOR_SKIP_SKILL_REFRESH` | Any non-empty value turns off skill regeneration. Use it with a throwaway config dir against a real `HOME`. |
| `SUPERORCHESTRATOR_DOCTOR_TIMEOUT_MS` | `doctor` timeout for each `--version` call (default 5000) |
| `SUPERORCHESTRATOR_ROUTER_MODEL` | Overrides `router.jev.model` (empty is unset) |
| `SUPERORCHESTRATOR_ROUTER_FALLBACK_MODEL` | Overrides `router.jev.fallback_model` (`none` disables it) |
| `OPENROUTER_API_KEY` | The Jev key (name set by `router.jev.key_env`) |
| `SO_DISCORD_WEBHOOK` | The Discord webhook URL (name set by `notifications.discord_webhook_env`) |
| `SUPERORCHESTRATOR_INSTALL_DIR`, `SUPERORCHESTRATOR_RELEASE_URL` | Used by `install.sh` only |

The socket is `$XDG_RUNTIME_DIR/superorchestrator.sock` on Linux, with `/tmp/superorchestrator-<uid>/` as the fallback. On macOS it is `~/Library/Caches/superorchestrator/sock`.
